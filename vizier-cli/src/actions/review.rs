use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use git2::{BranchType, Repository};
use tokio::sync::mpsc;

use vizier_core::{
    agent::{ReviewCheckContext, ReviewGateContext, ReviewGateStatus},
    agent_prompt,
    auditor::{self, Auditor, CommitMessageBuilder, CommitMessageType},
    config,
    display::{self},
    vcs,
};

use crate::plan;

use super::gates::{clip_log, log_cicd_result, run_cicd_script};
use super::save::{
    clear_narrative_tracker_for_commit, narrative_change_set_for_commit,
    stage_narrative_paths_for_commit, trim_staged_vizier_paths_for_commit,
};
use super::shared::{
    WorkdirGuard, append_agent_rows, audit_disposition, current_verbosity, format_block,
    prompt_selection, require_agent_backend, short_hash, spawn_plain_progress_logger,
};
use super::types::{CommitMode, ReviewOptions};

pub(crate) async fn run_review(
    opts: ReviewOptions,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
) -> Result<(), Box<dyn std::error::Error>> {
    require_agent_backend(
        agent,
        config::PromptKind::Review,
        "vizier review requires an agent-capable selector; update [agents.review] or pass --agent codex|gemini",
    )?;

    let spec = plan::PlanBranchSpec::resolve(
        Some(opts.plan.as_str()),
        opts.branch_override.as_deref(),
        opts.target.as_deref(),
    )?;

    vcs::ensure_clean_worktree().map_err(|err| {
        Box::<dyn std::error::Error>::from(format!(
            "clean working tree required before review: {err}"
        ))
    })?;

    let repo = Repository::discover(".")?;
    let repo_root_path = repo.workdir().map(|path| path.to_path_buf());
    let review_file_path = if opts.review_file {
        let root = repo_root_path
            .clone()
            .ok_or("review file output requires a working tree")?;
        Some(root.join("vizier-review.md"))
    } else {
        None
    };
    let source_ref = repo
        .find_branch(&spec.branch, BranchType::Local)
        .map_err(|_| format!("draft branch {} not found", spec.branch))?;
    let source_commit = source_ref.get().peel_to_commit()?;
    let source_oid = source_commit.id();

    let target_ref = repo
        .find_branch(&spec.target_branch, BranchType::Local)
        .map_err(|_| format!("target branch {} not found", spec.target_branch))?;
    let target_commit = target_ref.into_reference().peel_to_commit()?;
    let target_oid = target_commit.id();

    if repo.graph_descendant_of(target_oid, source_oid)? {
        let rows = vec![
            ("Outcome".to_string(), "Plan already merged".to_string()),
            ("Plan".to_string(), spec.slug.clone()),
            ("Target".to_string(), spec.target_branch.clone()),
            (
                "Latest commit".to_string(),
                short_hash(&source_oid.to_string()),
            ),
        ];
        println!("{}", format_block(rows));
        return Ok(());
    }

    if !repo.graph_descendant_of(source_oid, target_oid)? {
        display::warn(format!(
            "{} does not include the latest {} commits; review may miss upstream changes.",
            spec.branch, spec.target_branch
        ));
    }

    let plan_meta = spec.load_metadata()?;
    let worktree = plan::PlanWorktree::create(&spec.slug, &spec.branch, "review")?;
    let plan_path = worktree.plan_path(&spec.slug);
    let worktree_path = worktree.path().to_path_buf();
    let mut worktree = Some(worktree);

    let review_result = perform_review_workflow(
        &spec,
        &plan_meta,
        &worktree_path,
        &plan_path,
        ReviewExecution {
            assume_yes: opts.assume_yes,
            review_only: opts.review_only,
            skip_checks: opts.skip_checks,
            cicd_gate: opts.cicd_gate.clone(),
            auto_resolve_requested: opts.auto_resolve_requested,
            review_file_path,
        },
        commit_mode,
        agent,
    )
    .await;

    match review_result {
        Ok(outcome) => {
            if commit_mode.should_commit() {
                if let Some(tree) = worktree.take()
                    && let Err(err) = tree.cleanup()
                {
                    display::warn(format!(
                        "temporary worktree cleanup failed ({}); remove manually with `git worktree prune`",
                        err
                    ));
                }
            } else if let Some(tree) = worktree.take() {
                display::info(format!(
                    "Review worktree preserved at {}; inspect branch {} for pending critique/fix artifacts.",
                    tree.path().display(),
                    spec.branch
                ));
            }

            if outcome.branch_mutated && opts.push_after && commit_mode.should_commit() {
                super::shared::push_origin_if_requested(true)?;
            } else if outcome.branch_mutated && opts.push_after {
                display::info("Push skipped because --no-commit left review changes pending.");
            }

            if let Some(commit) = outcome.fix_commit.as_ref() {
                display::info(format!(
                    "Fixes addressing review feedback committed at {} on {}",
                    commit, spec.branch
                ));
            }

            let repo_root = vcs::repo_root().ok();
            let mut rows = vec![
                ("Outcome".to_string(), "Review complete".to_string()),
                ("Plan".to_string(), spec.slug.clone()),
                ("Branch".to_string(), spec.branch.clone()),
                ("Critique".to_string(), outcome.critique_label.to_string()),
                (
                    "CI/CD gate".to_string(),
                    outcome.cicd_gate.summary_label(repo_root.as_deref()),
                ),
                (
                    "Checks".to_string(),
                    format!(
                        "{}/{}",
                        display::format_number(outcome.checks_passed),
                        display::format_number(outcome.checks_total)
                    ),
                ),
                ("Diff".to_string(), outcome.diff_command.clone()),
                (
                    "Session".to_string(),
                    outcome
                        .session_path
                        .clone()
                        .unwrap_or_else(|| "<unknown>".to_string()),
                ),
            ];
            if let Some(path) = outcome.review_file_path.as_ref() {
                rows.insert(4, ("Review file".to_string(), path.clone()));
            }
            append_agent_rows(&mut rows, current_verbosity());
            println!("{}", format_block(rows));

            Ok(())
        }
        Err(err) => {
            if let Some(tree) = worktree.take() {
                display::warn(format!(
                    "Plan worktree preserved at {}; inspect branch {} for partial changes.",
                    tree.path().display(),
                    spec.branch
                ));
            }
            Err(err)
        }
    }
}

struct ReviewExecution {
    assume_yes: bool,
    review_only: bool,
    skip_checks: bool,
    cicd_gate: super::types::CicdGateOptions,
    auto_resolve_requested: bool,
    review_file_path: Option<PathBuf>,
}

struct ReviewOutcome {
    critique_label: &'static str,
    session_path: Option<String>,
    checks_passed: usize,
    checks_total: usize,
    diff_command: String,
    branch_mutated: bool,
    fix_commit: Option<String>,
    cicd_gate: ReviewGateResult,
    review_file_path: Option<String>,
}

struct ReviewCheckResult {
    command: String,
    status_code: Option<i32>,
    success: bool,
    duration: Duration,
    stdout: String,
    stderr: String,
}

impl ReviewCheckResult {
    fn duration_label(&self) -> String {
        format!("{:.2}s", self.duration.as_secs_f64())
    }

    fn status_label(&self) -> String {
        match self.status_code {
            Some(code) => format!("exit={code}"),
            None => "terminated".to_string(),
        }
    }

    fn to_context(&self) -> ReviewCheckContext {
        ReviewCheckContext {
            command: self.command.clone(),
            status_code: self.status_code,
            success: self.success,
            duration_ms: self.duration.as_millis(),
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct ReviewGateResult {
    status: ReviewGateStatus,
    script: Option<PathBuf>,
    attempts: u32,
    exit_code: Option<i32>,
    duration: Option<Duration>,
    stdout: String,
    stderr: String,
    auto_resolve_enabled: bool,
}

impl ReviewGateResult {
    fn skipped() -> Self {
        ReviewGateResult {
            status: ReviewGateStatus::Skipped,
            script: None,
            attempts: 0,
            exit_code: None,
            duration: None,
            stdout: String::new(),
            stderr: String::new(),
            auto_resolve_enabled: false,
        }
    }

    fn script_label(&self, repo_root: Option<&Path>) -> String {
        let Some(script) = self.script.as_ref() else {
            return "unset".to_string();
        };

        repo_root
            .and_then(|root| script.strip_prefix(root).ok())
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| script.display().to_string())
    }

    fn exit_code_label(&self) -> String {
        self.exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string())
    }

    fn duration_ms(&self) -> Option<u128> {
        self.duration.map(|value| value.as_millis())
    }

    fn summary_label(&self, repo_root: Option<&Path>) -> String {
        match self.status {
            ReviewGateStatus::Skipped => "not configured".to_string(),
            ReviewGateStatus::Passed => format!("passed ({})", self.script_label(repo_root)),
            ReviewGateStatus::Failed => format!(
                "failed {} ({})",
                self.exit_code_label(),
                self.script_label(repo_root)
            ),
        }
    }

    fn to_prompt_context(&self, repo_root: Option<&Path>) -> Option<ReviewGateContext> {
        self.script.as_ref()?;
        Some(ReviewGateContext {
            script: Some(self.script_label(repo_root)),
            status: self.status,
            attempts: self.attempts,
            duration_ms: self.duration_ms(),
            exit_code: self.exit_code,
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
            auto_resolve_enabled: self.auto_resolve_enabled,
        })
    }
}

async fn perform_review_workflow(
    spec: &plan::PlanBranchSpec,
    plan_meta: &plan::PlanMetadata,
    worktree_path: &Path,
    plan_path: &Path,
    exec: ReviewExecution,
    commit_mode: CommitMode,
    agent: &config::AgentSettings,
) -> Result<ReviewOutcome, Box<dyn std::error::Error>> {
    let _cwd = WorkdirGuard::enter(worktree_path)?;
    let repo_root = vcs::repo_root().ok();

    if exec.auto_resolve_requested {
        display::warn(
            "CI/CD auto-remediation is disabled during review; rerun merge for gate auto-fixes.",
        );
    }

    let gate_result = run_cicd_gate_for_review(&exec.cicd_gate)?;
    record_gate_operation("review", &gate_result);
    if matches!(gate_result.status, ReviewGateStatus::Failed) {
        display::warn(
            "CI/CD gate failed before the review critique; continuing with failure context.",
        );
    }

    let commands = resolve_review_commands(worktree_path, exec.skip_checks);
    if commands.is_empty() && !exec.skip_checks {
        display::info("Review checks: none configured for this repository.");
    }

    let check_results = run_review_checks(&commands, worktree_path);
    let checks_passed = check_results.iter().filter(|res| res.success).count();
    let checks_total = check_results.len();

    let diff_summary = collect_diff_summary(spec, worktree_path)?;
    let plan_document = fs::read_to_string(plan_path)?;
    let check_contexts: Vec<_> = check_results
        .iter()
        .map(ReviewCheckResult::to_context)
        .collect();

    let critique_agent = agent.for_prompt(config::PromptKind::Review)?;
    let selection = prompt_selection(&critique_agent)?;
    let gate_context = gate_result.to_prompt_context(repo_root.as_deref());
    let prompt = agent_prompt::build_review_prompt(
        selection,
        agent_prompt::ReviewPromptInput {
            plan_slug: &spec.slug,
            branch_name: &spec.branch,
            target_branch: &spec.target_branch,
            plan_document: &plan_document,
            diff_summary: &diff_summary,
            check_results: &check_contexts,
            cicd_gate: gate_context.as_ref(),
            documentation: &critique_agent.documentation,
        },
    )
    .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;

    let user_message = format!(
        "Review plan {} ({}) against {}",
        spec.slug, spec.branch, spec.target_branch
    );
    let (event_tx, event_rx) = mpsc::channel(64);
    let progress_handle = spawn_plain_progress_logger(event_rx);
    let (text_tx, _text_rx) = mpsc::channel(1);

    let response = Auditor::llm_request_with_tools_no_display(
        &critique_agent,
        Some(config::PromptKind::Review),
        prompt,
        user_message,
        auditor::RequestStream::Status {
            text: text_tx,
            events: Some(event_tx),
        },
        Some(worktree_path.to_path_buf()),
    )
    .await?;
    if let Some(handle) = progress_handle {
        let _ = handle.await;
    }

    let audit_result = Auditor::finalize(audit_disposition(commit_mode)).await?;
    let session_path = audit_result.session_display();
    let session_artifact = audit_result.session_artifact.clone();
    let (narrative_paths, narrative_summary) = narrative_change_set_for_commit(&audit_result);
    let critique_text = response.content.trim().to_string();
    emit_review_critique(&spec.slug, &critique_text);
    if let Some(path) = exec.review_file_path.as_ref() {
        write_review_file(path, &spec.slug, &critique_text)?;
    }

    let review_diff = vcs::get_diff(".", Some("HEAD"), None)?;
    let mut branch_mutated = !review_diff.trim().is_empty();
    if commit_mode.should_commit() {
        if branch_mutated {
            let mut summary = format!(
                "Recorded backend critique for plan {} (checks {}/{} passed).",
                spec.slug, checks_passed, checks_total
            );
            summary.push_str(&format!("\nDiff command: {}", spec.diff_command()));
            summary.push_str(&format!(
                "\nCI/CD gate: {}",
                gate_result.summary_label(repo_root.as_deref())
            ));

            let mut builder = CommitMessageBuilder::new(summary);
            builder
                .set_header(CommitMessageType::NarrativeChange)
                .with_session_artifact(session_artifact.clone())
                .with_narrative_summary(narrative_summary.clone())
                .with_author_note(format!(
                    "Review critique streamed to terminal; session: {}",
                    session_path.as_deref().unwrap_or("<session unavailable>")
                ));
            let commit_message = builder.build();
            stage_narrative_paths_for_commit(&narrative_paths)?;
            vcs::stage(Some(vec!["."]))?;
            trim_staged_vizier_paths_for_commit(&narrative_paths)?;
            let _review_commit = vcs::commit_staged(&commit_message, false)?;
            clear_narrative_tracker_for_commit(&narrative_paths);
        } else {
            display::info("No file modifications produced during review; skipping commit.");
        }
    } else {
        let session_hint = session_path
            .as_deref()
            .unwrap_or("<session unavailable>")
            .to_string();
        if branch_mutated {
            display::info(format!(
                "Review critique not committed (--no-commit); consult the terminal output or session log {} before committing manually.",
                session_hint
            ));
            if !narrative_paths.is_empty() {
                display::info(
                    "Review critique artifacts held for manual review (--no-commit active).",
                );
            }
        } else {
            display::info(format!(
                "Review critique streamed (--no-commit); no file modifications generated. Session log: {}",
                session_hint
            ));
        }
    }

    let mut fix_commit: Option<String> = None;
    let diff_command = spec.diff_command();

    if exec.review_only || exec.review_file_path.is_some() {
        display::info("Review fixes skipped (--review-only or --review-file active).");
    } else {
        let mut apply_fixes = exec.assume_yes;
        if !exec.assume_yes {
            apply_fixes = super::shared::prompt_for_confirmation(&format!(
                "Apply suggested fixes on {}? [y/N] ",
                spec.branch
            ))?;
        }

        if apply_fixes {
            match apply_review_fixes(
                spec,
                plan_meta,
                worktree_path,
                &critique_text,
                commit_mode,
                agent,
            )
            .await?
            {
                Some(commit) => {
                    fix_commit = Some(commit);
                    branch_mutated = true;
                }
                None => {
                    let post_fix_diff = vcs::get_diff(".", Some("HEAD"), None)?;
                    let post_fix_changed = !post_fix_diff.trim().is_empty();
                    branch_mutated = branch_mutated || post_fix_changed;
                    if !post_fix_changed {
                        display::info(
                            "Backend reported no changes while addressing review feedback.",
                        );
                    }
                }
            }
        } else {
            display::info("Skipped automatic fixes; branch left untouched.");
        }
    }

    Ok(ReviewOutcome {
        critique_label: "terminal",
        session_path,
        checks_passed,
        checks_total,
        diff_command,
        branch_mutated,
        fix_commit,
        cicd_gate: gate_result,
        review_file_path: exec.review_file_path.as_ref().map(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.to_string())
                .unwrap_or_else(|| path.display().to_string())
        }),
    })
}

fn resolve_review_commands(worktree_path: &Path, skip_checks: bool) -> Vec<String> {
    if skip_checks {
        return Vec::new();
    }

    let cfg = config::get_config();
    if !cfg.review.checks.commands.is_empty() {
        return cfg.review.checks.commands.clone();
    }

    if worktree_path.join("Cargo.toml").exists() {
        return vec![
            "cargo check --all --all-targets".to_string(),
            "cargo test --all --all-targets".to_string(),
        ];
    }

    Vec::new()
}

fn run_review_checks(commands: &[String], worktree_path: &Path) -> Vec<ReviewCheckResult> {
    let mut results = Vec::new();

    for command in commands {
        display::info(format!("Running review check: `{}`", command));
        let start = Instant::now();
        match Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(worktree_path)
            .output()
        {
            Ok(output) => {
                let result = ReviewCheckResult {
                    command: command.clone(),
                    status_code: output.status.code(),
                    success: output.status.success(),
                    duration: start.elapsed(),
                    stdout: clip_log(&output.stdout),
                    stderr: clip_log(&output.stderr),
                };
                log_check_result(&result);
                results.push(result);
            }
            Err(err) => {
                let result = ReviewCheckResult {
                    command: command.clone(),
                    status_code: None,
                    success: false,
                    duration: start.elapsed(),
                    stdout: String::new(),
                    stderr: format!("failed to run command: {err}"),
                };
                log_check_result(&result);
                results.push(result);
            }
        }
    }

    results
}

fn log_check_result(result: &ReviewCheckResult) {
    let status_label = if result.success { "passed" } else { "failed" };
    let message = format!(
        "Review check `{}` {status_label} ({}; {})",
        result.command,
        result.status_label(),
        result.duration_label()
    );
    if result.success {
        display::info(message);
    } else {
        display::warn(message);
        let trimmed = result.stderr.trim();
        if !trimmed.is_empty() {
            let snippet: String = trimmed
                .lines()
                .take(6)
                .map(|line| format!("    {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            display::warn(format!("  stderr:\n{}", snippet));
        }
    }
}

fn run_cicd_gate_for_review(
    gate_opts: &super::types::CicdGateOptions,
) -> Result<ReviewGateResult, Box<dyn std::error::Error>> {
    let Some(script) = gate_opts.script.as_ref() else {
        display::info("CI/CD gate: not configured for review; skipping.");
        return Ok(ReviewGateResult::skipped());
    };

    if gate_opts.auto_resolve {
        display::warn(
            "CI/CD gate auto-remediation is disabled during review; reporting status without applying fixes.",
        );
    }

    let repo_root =
        vcs::repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let result = run_cicd_script(script, &repo_root)?;
    log_cicd_result(script, &result, 1);

    let status = if result.success() {
        ReviewGateStatus::Passed
    } else {
        ReviewGateStatus::Failed
    };
    Ok(ReviewGateResult {
        status,
        script: Some(script.clone()),
        attempts: 1,
        exit_code: result.status.code(),
        duration: Some(result.duration),
        stdout: clip_log(result.stdout.as_bytes()),
        stderr: clip_log(result.stderr.as_bytes()),
        auto_resolve_enabled: false,
    })
}

fn record_gate_operation(scope: &str, gate: &ReviewGateResult) {
    let repo_root = vcs::repo_root().ok();
    let script = gate.script.as_ref().map(|path| {
        repo_root
            .as_ref()
            .and_then(|root| path.strip_prefix(root).ok())
            .map(|relative| relative.display().to_string())
            .unwrap_or_else(|| path.display().to_string())
    });
    Auditor::record_operation(
        "cicd_gate",
        serde_json::json!({
            "scope": scope,
            "script": script,
            "status": match gate.status {
                ReviewGateStatus::Passed => "passed",
                ReviewGateStatus::Failed => "failed",
                ReviewGateStatus::Skipped => "skipped",
            },
            "attempts": gate.attempts,
            "exit_code": gate.exit_code,
            "duration_ms": gate.duration_ms(),
            "stdout": gate.stdout,
            "stderr": gate.stderr,
            "auto_resolve_enabled": gate.auto_resolve_enabled,
        }),
    );
}

fn collect_diff_summary(
    spec: &plan::PlanBranchSpec,
    worktree_path: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    match vcs::diff_summary_against_target(worktree_path, &spec.target_branch) {
        Ok(summary) => Ok(format!(
            "Diff command: {}\n\n{}\n\n{}",
            spec.diff_command(),
            summary.stats.trim(),
            summary.name_status.trim()
        )),
        Err(err) => Ok(format!(
            "Diff command: {}\n\nUnable to compute diff via libgit2: {err}",
            spec.diff_command()
        )),
    }
}

fn write_review_file(
    path: &Path,
    plan_slug: &str,
    critique: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut contents = String::new();
    contents.push_str(&format!("# Review critique for plan {plan_slug}\n\n"));
    if critique.trim().is_empty() {
        contents.push_str("(Agent returned an empty critique.)\n");
    } else {
        contents.push_str(critique.trim());
        contents.push('\n');
    }
    fs::write(path, contents)?;
    Ok(())
}

fn emit_review_critique(plan_slug: &str, critique: &str) {
    println!("--- Review critique for plan {plan_slug} ---");
    if critique.trim().is_empty() {
        println!("(Agent returned an empty critique.)");
    } else {
        println!("{}", critique.trim());
    }
    println!("--- End review critique ---");
}

async fn apply_review_fixes(
    spec: &plan::PlanBranchSpec,
    plan_meta: &plan::PlanMetadata,
    worktree_path: &Path,
    critique_text: &str,
    commit_mode: CommitMode,
    agent: &config::AgentSettings,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let plan_rel = spec.plan_rel_path();
    let prompt_agent = agent.for_prompt(config::PromptKind::Documentation)?;
    let mut instruction = format!(
        "<instruction>Read the implementation plan at {} and the review critique below. Address every Action Item without changing unrelated code.</instruction>",
        plan_rel.display()
    );
    instruction.push_str(&format!(
        "<planSummary>{}</planSummary>",
        plan::summarize_spec(plan_meta)
    ));
    instruction.push_str("<reviewCritique>\n");
    if critique_text.trim().is_empty() {
        instruction
            .push_str("(Review critique was empty; explain whether any fixes are necessary.)\n");
    } else {
        instruction.push_str(critique_text.trim());
        instruction.push('\n');
    }
    instruction.push_str("</reviewCritique>");
    instruction.push_str(
        "<note>Update `.vizier/narrative/snapshot.md`, `.vizier/narrative/glossary.md`, and any relevant narrative docs when behavior changes.</note>",
    );
    let system_prompt = agent_prompt::build_documentation_prompt(
        prompt_agent.prompt_selection(),
        &instruction,
        &prompt_agent.documentation,
    )
    .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;

    let (event_tx, event_rx) = mpsc::channel(64);
    let progress_handle = spawn_plain_progress_logger(event_rx);
    let (text_tx, _text_rx) = mpsc::channel(1);
    let response = Auditor::llm_request_with_tools_no_display(
        &prompt_agent,
        None,
        system_prompt,
        instruction.clone(),
        auditor::RequestStream::Status {
            text: text_tx,
            events: Some(event_tx),
        },
        Some(worktree_path.to_path_buf()),
    )
    .await
    .map_err(|err| -> Box<dyn std::error::Error> {
        Box::from(format!("agent backend error: {err}"))
    })?;
    if let Some(handle) = progress_handle {
        let _ = handle.await;
    }

    let audit_result = Auditor::finalize(audit_disposition(commit_mode)).await?;
    let session_path = audit_result.session_display();
    let session_artifact = audit_result.session_artifact.clone();
    let (narrative_paths, narrative_summary) = narrative_change_set_for_commit(&audit_result);
    let diff = vcs::get_diff(".", Some("HEAD"), None)?;
    if diff.trim().is_empty() {
        display::info("Backend reported no file modifications during fix-up.");
        return Ok(None);
    }

    if commit_mode.should_commit() {
        stage_narrative_paths_for_commit(&narrative_paths)?;
        vcs::stage(Some(vec!["."]))?;
        trim_staged_vizier_paths_for_commit(&narrative_paths)?;
        let mut summary = response.content.trim().to_string();
        if summary.is_empty() {
            summary = format!(
                "Addressed review feedback for plan {} based on the latest critique",
                spec.slug
            );
        }
        let mut builder = CommitMessageBuilder::new(summary);
        builder
            .set_header(CommitMessageType::CodeChange)
            .with_session_artifact(session_artifact.clone())
            .with_narrative_summary(narrative_summary.clone())
            .with_author_note(format!(
                "Review critique streamed to terminal; session: {}",
                session_path.as_deref().unwrap_or("<session unavailable>")
            ));
        let commit_message = builder.build();
        let commit_oid = vcs::commit_staged(&commit_message, false)?;
        clear_narrative_tracker_for_commit(&narrative_paths);
        Ok(Some(commit_oid.to_string()))
    } else {
        display::info("Fixes left pending; commit manually once satisfied.");
        if !narrative_paths.is_empty() {
            display::info("Narrative updates left pending (--no-commit active).");
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::collect_diff_summary;
    use crate::plan::PlanBranchSpec;
    use std::fs;
    use vizier_core::vcs;

    #[test]
    fn collect_diff_summary_reports_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = git2::Repository::init(dir.path()).expect("init repo");

        fs::write(dir.path().join("one.txt"), "one\n").unwrap();
        fs::write(dir.path().join("two.txt"), "two\n").unwrap();
        vcs::add_and_commit_in(dir.path(), Some(vec!["."]), "init", false).expect("init commit");

        repo.branch(
            "target",
            &repo.head().unwrap().peel_to_commit().unwrap(),
            true,
        )
        .expect("create target branch");

        fs::write(dir.path().join("one.txt"), "one\nchanged\n").unwrap();
        fs::remove_file(dir.path().join("two.txt")).unwrap();
        fs::write(dir.path().join("three.txt"), "three\n").unwrap();
        vcs::stage_all_in(dir.path()).expect("stage all changes");
        vcs::commit_staged_in(dir.path(), "topic", false).expect("topic commit");

        let spec = PlanBranchSpec {
            slug: "diff-test".to_string(),
            branch: "draft/diff-test".to_string(),
            target_branch: "target".to_string(),
        };

        let summary = collect_diff_summary(&spec, dir.path()).expect("summary");
        assert!(
            summary.contains("Diff command:"),
            "summary should include diff command:\n{summary}"
        );
        assert!(
            summary.contains("one.txt"),
            "modified file should be mentioned:\n{summary}"
        );
        assert!(
            summary.contains("three.txt"),
            "added file should be mentioned:\n{summary}"
        );
        assert!(
            summary.contains("two.txt"),
            "deleted file should be mentioned:\n{summary}"
        );
    }
}
