use std::path::Path;

use git2::{BranchType, Repository};
use tokio::sync::mpsc;

use vizier_core::{
    agent_prompt,
    auditor::{self, Auditor, CommitMessageBuilder, CommitMessageType},
    config, display, vcs,
};

use crate::errors::CancelledError;
use crate::plan;

use super::gates::{
    StopConditionScriptResult, log_stop_condition_result, record_stop_condition_attempt,
    record_stop_condition_summary, run_stop_condition_script, stop_condition_script_label,
};
use super::save::{
    clear_narrative_tracker_for_commit, narrative_change_set_for_commit,
    stage_narrative_paths_for_commit, trim_staged_vizier_paths_for_commit,
};
use super::shared::{
    WorkdirGuard, append_agent_rows, audit_disposition, current_verbosity, format_block,
    prompt_for_confirmation, push_origin_if_requested, require_agent_backend, short_hash,
    spawn_plain_progress_logger,
};
use super::types::{ApproveOptions, CommitMode};

struct PlanApplyResult {
    commit_oid: Option<String>,
}

pub(crate) async fn run_approve(
    opts: ApproveOptions,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
) -> Result<(), Box<dyn std::error::Error>> {
    require_agent_backend(
        agent,
        config::PromptKind::Documentation,
        "vizier approve requires an agent-capable selector; update [agents.approve] or pass --agent codex|gemini",
    )?;

    let spec = plan::PlanBranchSpec::resolve(
        Some(opts.plan.as_str()),
        opts.branch_override.as_deref(),
        opts.target.as_deref(),
    )?;

    vcs::ensure_clean_worktree().map_err(|err| {
        Box::<dyn std::error::Error>::from(format!(
            "clean working tree required before approval: {err}"
        ))
    })?;

    let repo = Repository::discover(".")?;
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
            "{} does not include the latest {} commits; merge may require manual resolution.",
            spec.branch, spec.target_branch
        ));
    }

    let plan_meta = spec.load_metadata()?;
    if plan_meta.branch != spec.branch {
        display::warn(format!(
            "Plan metadata references branch {} but CLI resolved to {}",
            plan_meta.branch, spec.branch
        ));
    }

    if !opts.assume_yes {
        spec.show_preview(&plan_meta);
        if !prompt_for_confirmation("Implement plan now? [y/N] ")? {
            println!("Approval cancelled; no changes were made.");
            return Err(Box::new(CancelledError::new("approval cancelled")));
        }
    }

    let worktree = plan::PlanWorktree::create(&spec.slug, &spec.branch, "approve")?;
    let worktree_path = worktree.path().to_path_buf();
    let plan_path = worktree.plan_path(&spec.slug);
    let mut worktree = Some(worktree);
    let stop_script = opts.stop_condition.script.clone();
    let mut stop_attempts: u32 = 0;
    let mut last_stop_result: Option<StopConditionScriptResult> = None;
    let mut remaining_retries = opts.stop_condition.retries;
    let mut stop_status = if stop_script.is_some() {
        "failed"
    } else {
        "none"
    };

    let mut last_plan_commit: Option<String> = None;
    let approval = loop {
        let result = apply_plan_in_worktree(
            &spec,
            &plan_meta,
            &worktree_path,
            &plan_path,
            commit_mode,
            agent,
        )
        .await;

        match result {
            Err(err) => {
                if stop_script.is_none() {
                    stop_status = "none";
                }
                break Err(err);
            }
            Ok(plan_result) => {
                if commit_mode.should_commit() {
                    last_plan_commit = plan_result.commit_oid.clone();
                }
                let Some(script_path) = stop_script.as_ref() else {
                    stop_status = "none";
                    break Ok(plan_result);
                };

                let stop_result = match run_stop_condition_script(script_path, &worktree_path) {
                    Ok(result) => result,
                    Err(err) => {
                        stop_status = "failed";
                        break Err(err);
                    }
                };

                stop_attempts += 1;
                log_stop_condition_result(script_path, &stop_result, stop_attempts);
                record_stop_condition_attempt("approve", script_path, stop_attempts, &stop_result);
                let success = stop_result.success();
                last_stop_result = Some(stop_result);

                if success {
                    stop_status = "passed";
                    break Ok(plan_result);
                }

                if remaining_retries == 0 {
                    stop_status = "failed";
                    let message = format!(
                        "Approve stop-condition script `{}` did not succeed after {} attempt(s); inspect {} for partial changes and script logs.",
                        script_path.display(),
                        stop_attempts,
                        worktree_path.display()
                    );
                    break Err(Box::<dyn std::error::Error>::from(message));
                }

                remaining_retries -= 1;
                display::info(format!(
                    "Approve stop-condition not yet satisfied; retrying plan application ({} retries remaining).",
                    remaining_retries
                ));
            }
        }
    };

    if stop_script.is_some() {
        record_stop_condition_summary(
            "approve",
            stop_script.as_deref(),
            stop_status,
            stop_attempts,
            last_stop_result.as_ref(),
        );
    } else {
        record_stop_condition_summary("approve", None, "none", 0, None);
    }

    match approval {
        Ok(result) => {
            if opts.push_after {
                if commit_mode.should_commit() {
                    if last_plan_commit.is_some() {
                        push_origin_if_requested(true)?;
                    } else {
                        display::info("Push skipped because no commit was created during approve.");
                    }
                } else {
                    display::info(
                        "Push skipped because --no-commit left approval changes pending.",
                    );
                }
            }

            let stop_condition_label = if let Some(script) = stop_script.as_ref() {
                let repo_root = vcs::repo_root().ok();
                let label = stop_condition_script_label(script, repo_root.as_deref());
                match stop_status {
                    "passed" => format!(
                        "passed ({}; attempts {})",
                        label,
                        display::format_number(stop_attempts as usize)
                    ),
                    "failed" => {
                        let exit_label = last_stop_result
                            .as_ref()
                            .and_then(|res| res.status.code())
                            .map(|code| code.to_string())
                            .unwrap_or_else(|| "signal".to_string());
                        format!(
                            "failed ({}; attempts {}; exit {})",
                            label,
                            display::format_number(stop_attempts as usize),
                            exit_label
                        )
                    }
                    _ => format!("configured ({})", label),
                }
            } else {
                "none".to_string()
            };

            if commit_mode.should_commit() {
                if let Some(tree) = worktree.take()
                    && let Err(err) = tree.cleanup()
                {
                    display::warn(format!(
                        "temporary worktree cleanup failed ({}); remove manually with `git worktree prune`",
                        err
                    ));
                }

                let mut rows = vec![
                    ("Outcome".to_string(), "Plan implemented".to_string()),
                    ("Plan".to_string(), spec.slug.clone()),
                    ("Branch".to_string(), spec.branch.clone()),
                    ("Stop condition".to_string(), stop_condition_label.clone()),
                    ("Review".to_string(), spec.diff_command()),
                ];
                if let Some(commit_oid) = result.commit_oid.as_ref() {
                    rows.push(("Latest commit".to_string(), short_hash(commit_oid)));
                }
                append_agent_rows(&mut rows, current_verbosity());
                println!("{}", format_block(rows));
            } else {
                display::info(format!(
                    "Plan worktree preserved at {}; inspect branch {} for pending changes.",
                    worktree_path.display(),
                    spec.branch
                ));
                let mut rows = vec![
                    (
                        "Outcome".to_string(),
                        "Plan pending manual commit".to_string(),
                    ),
                    ("Plan".to_string(), spec.slug.clone()),
                    ("Branch".to_string(), spec.branch.clone()),
                    ("Worktree".to_string(), worktree_path.display().to_string()),
                    ("Stop condition".to_string(), stop_condition_label),
                    ("Review".to_string(), spec.diff_command()),
                ];
                append_agent_rows(&mut rows, current_verbosity());
                println!("{}", format_block(rows));
            }
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

async fn apply_plan_in_worktree(
    spec: &plan::PlanBranchSpec,
    plan_meta: &plan::PlanMetadata,
    worktree_path: &Path,
    _plan_path: &Path,
    commit_mode: CommitMode,
    agent: &config::AgentSettings,
) -> Result<PlanApplyResult, Box<dyn std::error::Error>> {
    let _cwd = WorkdirGuard::enter(worktree_path)?;
    let prompt_agent = agent.for_prompt(config::PromptKind::Documentation)?;

    let plan_rel = spec.plan_rel_path();
    let mut instruction = format!(
        "<instruction>Read the implementation plan at {} and implement its Execution Plan on this branch. Apply the listed steps, update `.vizier/narrative/snapshot.md`, `.vizier/narrative/glossary.md`, plus any narrative docs as needed, and stage the resulting edits for commit.</instruction>",
        plan_rel.display()
    );
    instruction.push_str(&format!(
        "<planSummary>{}</planSummary>",
        plan::summarize_spec(plan_meta)
    ));

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
    let (narrative_paths, narrative_summary) = narrative_change_set_for_commit(&audit_result);
    let diff = vcs::get_diff(".", Some("HEAD"), None)?;
    if diff.trim().is_empty() {
        return Err("Agent completed without modifying files; nothing new to approve.".into());
    }

    let mut summary = response.content.trim().to_string();
    if summary.is_empty() {
        summary = format!(
            "Plan {} implemented on {}.\nSpec summary: {}",
            spec.slug,
            spec.branch,
            plan::summarize_spec(plan_meta)
        );
    }

    let mut builder = CommitMessageBuilder::new(summary);
    builder
        .set_header(CommitMessageType::CodeChange)
        .with_session_log_path(session_path.clone())
        .with_narrative_summary(narrative_summary.clone());

    let mut commit_oid: Option<String> = None;

    if commit_mode.should_commit() {
        stage_narrative_paths_for_commit(&narrative_paths)?;
        vcs::stage(Some(vec!["."]))?;
        trim_staged_vizier_paths_for_commit(&narrative_paths)?;
        let commit_message = builder.build();
        let oid = vcs::commit_staged(&commit_message, false)?;
        clear_narrative_tracker_for_commit(&narrative_paths);
        commit_oid = Some(oid.to_string());
    } else if !narrative_paths.is_empty() {
        display::info(
            "Narrative assets updated with --no-commit; changes left pending in the plan worktree.",
        );
    }

    Ok(PlanApplyResult { commit_oid })
}
