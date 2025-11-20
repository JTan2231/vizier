use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

use git2::{BranchType, Oid, Repository, RepositoryState};
use serde::{Deserialize, Serialize};
use tempfile::{Builder, NamedTempFile, TempPath};
use tokio::{sync::mpsc, task::JoinHandle};

use vizier_core::vcs::{
    AttemptOutcome, CredentialAttempt, MergePreparation, PushErrorKind, RemoteScheme,
    add_worktree_for_branch, commit_in_progress_merge, commit_paths_in_repo, commit_ready_merge,
    create_branch_from, delete_branch, detect_primary_branch, list_conflicted_paths, prepare_merge,
    remove_worktree, repo_root,
};
use vizier_core::{
    auditor,
    auditor::{Auditor, CommitMessageBuilder, CommitMessageType},
    bootstrap,
    bootstrap::{BootstrapOptions, IssuesProvider},
    codex, config,
    display::{self, LogLevel, ProgressEvent, Verbosity},
    file_tracking, tools, vcs,
};

use crate::plan;

fn clip_message(msg: &str) -> String {
    const LIMIT: usize = 90;
    let mut clipped = String::new();
    for (idx, ch) in msg.chars().enumerate() {
        if idx >= LIMIT {
            clipped.push('…');
            break;
        }
        clipped.push(ch);
    }
    clipped
}

fn format_credential_attempt(attempt: &CredentialAttempt) -> String {
    let label = attempt.strategy.label();
    match &attempt.outcome {
        AttemptOutcome::Success => format!("{label}=ok"),
        AttemptOutcome::Failure(message) => {
            format!("{label}=failed({})", clip_message(message))
        }
        AttemptOutcome::Skipped(message) => {
            format!("{label}=skipped({})", clip_message(message))
        }
    }
}

fn render_push_auth_failure(
    remote: &str,
    url: &str,
    scheme: &RemoteScheme,
    attempts: &[CredentialAttempt],
) {
    let scheme_label = scheme.label();
    display::emit(
        LogLevel::Error,
        format!("Push to {remote} failed ({scheme_label} {url})"),
    );

    if !attempts.is_empty() {
        let summary = attempts
            .iter()
            .map(format_credential_attempt)
            .collect::<Vec<_>>()
            .join("; ");
        display::emit(LogLevel::Error, format!("Credential strategies: {summary}"));
    }

    if matches!(scheme, RemoteScheme::Ssh) {
        display::emit(
            LogLevel::Error,
            "Hint: start ssh-agent and `ssh-add ~/.ssh/id_ed25519`, or switch the remote to HTTPS.",
        );
    }
}

fn push_origin_if_requested(should_push: bool) -> Result<(), Box<dyn std::error::Error>> {
    if !should_push {
        return Ok(());
    }

    display::info("Pushing current branch to origin...");
    match vcs::push_current_branch("origin") {
        Ok(_) => {
            display::info("Push to origin completed.");
            Ok(())
        }
        Err(err) => {
            match err.kind() {
                PushErrorKind::Auth {
                    remote,
                    url,
                    scheme,
                    attempts,
                } => {
                    render_push_auth_failure(remote, url, scheme, attempts);
                }
                PushErrorKind::General(message) => {
                    display::emit(
                        LogLevel::Error,
                        format!("Error pushing to origin: {message}"),
                    );
                }
            }

            Err(Box::<dyn std::error::Error>::from(err))
        }
    }
}

pub fn print_token_usage() {
    if let Some(report) = Auditor::latest_usage_report() {
        let agent_note = format_agent_annotation();
        if report.known {
            let mut message = format!("Token usage: {}", describe_usage_report(&report));
            if let Some(annotation) = agent_note {
                message.push_str(&format!("; {}", annotation));
            }
            display::info(message);
        } else {
            let mut message = "Token usage: unknown".to_string();
            if let Some(annotation) = agent_note {
                message.push_str(&format!("; {}", annotation));
            }
            display::info(message);
        }
        return;
    }

    let usage = Auditor::get_total_usage();
    if usage.known {
        let mut message = format!("Token usage: {}", describe_usage_totals(&usage));
        if let Some(annotation) = format_agent_annotation() {
            message.push_str(&format!("; {}", annotation));
        }
        display::info(message);
    } else {
        let mut message = "Token usage: unknown".to_string();
        if let Some(annotation) = format_agent_annotation() {
            message.push_str(&format!("; {}", annotation));
        }
        display::info(message);
    }
}

fn token_usage_suffix() -> String {
    let agent_note = format_agent_annotation();
    let mut detail = if let Some(report) = Auditor::latest_usage_report() {
        if report.known {
            describe_usage_report(&report)
        } else {
            "unknown".to_string()
        }
    } else {
        let usage = Auditor::get_total_usage();
        if usage.known {
            describe_usage_totals(&usage)
        } else {
            "unknown".to_string()
        }
    };
    if let Some(annotation) = agent_note {
        detail.push_str(&format!("; {}", annotation));
    }
    format!(" (tokens: {})", detail)
}

fn format_agent_annotation() -> Option<String> {
    auditor::Auditor::latest_agent_context().map(|context| {
        let mut parts = vec![format!("agent={}", context.backend)];
        parts.push(format!("scope={}", context.scope.as_str()));
        if context.backend == config::BackendKind::Wire {
            parts.push(format!("model={}", context.model));
            if let Some(reasoning) = context.reasoning_effort {
                parts.push(format!("reasoning={reasoning:?}"));
            }
        }
        parts.join(" ")
    })
}

fn describe_usage_report(report: &auditor::TokenUsageReport) -> String {
    let mut sections = Vec::new();
    sections.push(format!(
        "total={} (+{})",
        report.total(),
        report.delta_total()
    ));
    let mut input_section = format!("input={} (+{}", report.prompt_total, report.prompt_delta);
    if let Some(cached_delta) = report.cached_input_delta {
        input_section.push_str(&format!(", +{} cached", cached_delta));
    } else if let Some(cached_total) = report.cached_input_total {
        input_section.push_str(&format!(", {} cached", cached_total));
    }
    input_section.push(')');
    sections.push(input_section);
    let mut output_section = format!(
        "output={} (+{}",
        report.completion_total, report.completion_delta
    );
    if report.reasoning_output_total.is_some() || report.reasoning_output_delta.is_some() {
        let reasoning_value = report
            .reasoning_output_delta
            .or(report.reasoning_output_total)
            .unwrap_or(0);
        output_section.push_str(&format!(", reasoning {}", reasoning_value));
    }
    output_section.push(')');
    sections.join(" ")
}

fn describe_usage_totals(usage: &auditor::TokenUsage) -> String {
    let mut sections = Vec::new();
    sections.push(format!("total={}", usage.total_tokens));
    let mut input_section = format!("input={}", usage.input_tokens);
    if usage.cached_input_tokens > 0 {
        input_section.push_str(&format!(" (+{} cached)", usage.cached_input_tokens));
    }
    sections.push(input_section);
    let mut output_section = format!("output={}", usage.output_tokens);
    if usage.reasoning_output_tokens > 0 {
        output_section.push_str(&format!(" (reasoning {})", usage.reasoning_output_tokens));
    }
    sections.push(output_section);
    sections.join(" ")
}

fn spawn_plain_progress_logger(mut rx: mpsc::Receiver<ProgressEvent>) -> Option<JoinHandle<()>> {
    let cfg = display::get_display_config();
    if matches!(cfg.verbosity, Verbosity::Quiet) {
        return None;
    }

    let verbosity = cfg.verbosity;
    Some(tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            for line in display::render_progress_event(&event, verbosity) {
                eprintln!("{}", line);
            }
        }
    }))
}

#[derive(Debug, Clone)]
pub struct SnapshotInitOptions {
    pub force: bool,
    pub depth: Option<usize>,
    pub paths: Vec<String>,
    pub exclude: Vec<String>,
    pub issues: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SpecSource {
    Inline,
    File(PathBuf),
    Stdin,
}

impl SpecSource {
    pub fn as_metadata_value(&self) -> String {
        match self {
            SpecSource::Inline => "inline".to_string(),
            SpecSource::File(path) => format!("file:{}", path.display()),
            SpecSource::Stdin => "stdin".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitMode {
    AutoCommit,
    HoldForReview,
}

impl CommitMode {
    pub fn should_commit(self) -> bool {
        matches!(self, CommitMode::AutoCommit)
    }

    pub fn label(self) -> &'static str {
        match self {
            CommitMode::AutoCommit => "auto",
            CommitMode::HoldForReview => "manual",
        }
    }
}

fn audit_disposition(mode: CommitMode) -> auditor::CommitDisposition {
    match mode {
        CommitMode::AutoCommit => auditor::CommitDisposition::Auto,
        CommitMode::HoldForReview => auditor::CommitDisposition::Hold,
    }
}

#[derive(Debug, Clone)]
pub struct DraftArgs {
    pub spec_text: String,
    pub spec_source: SpecSource,
    pub name_override: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ListOptions {
    pub target: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ApproveOptions {
    pub plan: Option<String>,
    pub list_only: bool,
    pub target: Option<String>,
    pub branch_override: Option<String>,
    pub assume_yes: bool,
    pub push_after: bool,
}

#[derive(Debug, Clone)]
pub struct ReviewOptions {
    pub plan: String,
    pub target: Option<String>,
    pub branch_override: Option<String>,
    pub assume_yes: bool,
    pub review_only: bool,
    pub skip_checks: bool,
    pub push_after: bool,
}

#[derive(Debug, Clone)]
pub struct MergeOptions {
    pub plan: String,
    pub target: Option<String>,
    pub branch_override: Option<String>,
    pub assume_yes: bool,
    pub delete_branch: bool,
    pub note: Option<String>,
    pub push_after: bool,
    pub conflict_strategy: MergeConflictStrategy,
    pub complete_conflict: bool,
    pub cicd_gate: CicdGateOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeConflictStrategy {
    Manual,
    Codex,
}

#[derive(Debug, Clone)]
pub struct CicdGateOptions {
    pub script: Option<PathBuf>,
    pub auto_resolve: bool,
    pub retries: u32,
}

impl CicdGateOptions {
    pub fn from_config(config: &config::MergeCicdGateConfig) -> Self {
        Self {
            script: config.script.clone(),
            auto_resolve: config.auto_resolve,
            retries: config.retries,
        }
    }

    #[allow(dead_code)]
    pub fn disabled() -> Self {
        Self {
            script: None,
            auto_resolve: false,
            retries: 1,
        }
    }
}

#[derive(Debug, Clone)]
struct CicdGateOutcome {
    script: PathBuf,
    attempts: u32,
    fixes: Vec<String>,
}

#[derive(Debug)]
struct CicdScriptResult {
    status: ExitStatus,
    duration: Duration,
    stdout: String,
    stderr: String,
}

impl CicdScriptResult {
    fn success(&self) -> bool {
        self.status.success()
    }

    fn status_label(&self) -> String {
        match self.status.code() {
            Some(code) => format!("exit={code}"),
            None => "terminated".to_string(),
        }
    }
}

#[derive(Debug)]
enum PendingMergeStatus {
    None,
    Ready { merge_oid: Oid, source_oid: Oid },
    Blocked(PendingMergeBlocker),
}

#[derive(Debug)]
enum PendingMergeBlocker {
    WrongCheckout { expected_branch: String },
    NotInMerge { target_branch: String },
    Conflicts { files: Vec<String> },
}

#[derive(Debug)]
struct PendingMergeError {
    slug: String,
    detail: PendingMergeBlocker,
}

impl PendingMergeError {
    fn new(slug: impl Into<String>, detail: PendingMergeBlocker) -> Self {
        PendingMergeError {
            slug: slug.into(),
            detail,
        }
    }
}

impl std::fmt::Display for PendingMergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.detail {
            PendingMergeBlocker::WrongCheckout { expected_branch } => write!(
                f,
                "Pending Vizier merge for plan {} is tied to {}; checkout that branch and rerun `vizier merge {} --complete-conflict` to finalize the conflict resolution.",
                self.slug, expected_branch, self.slug
            ),
            PendingMergeBlocker::NotInMerge { target_branch } => write!(
                f,
                "Vizier has merge metadata for plan {} but Git is no longer merging on {}; rerun `vizier merge {}` (without --complete-conflict) to start a new merge if needed.",
                self.slug, target_branch, self.slug
            ),
            PendingMergeBlocker::Conflicts { files } => {
                if files.is_empty() {
                    write!(
                        f,
                        "Merge conflicts for plan {} are still unresolved; fix them, stage the results, then rerun `vizier merge {} --complete-conflict`.",
                        self.slug, self.slug
                    )
                } else {
                    let preview = files.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
                    let more = if files.len() > 3 {
                        format!(" (+{} more)", files.len() - 3)
                    } else {
                        String::new()
                    };
                    write!(
                        f,
                        "Merge conflicts for plan {} remain ({preview}{more}); resolve and stage them, then rerun `vizier merge {} --complete-conflict`.",
                        self.slug, self.slug
                    )
                }
            }
        }
    }
}

impl std::error::Error for PendingMergeError {}

pub async fn run_snapshot_init(
    opts: SnapshotInitOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let depth_preview = bootstrap::preview_history_depth(opts.depth)?;

    display::info(format!(
        "Bootstrapping snapshot (history depth target: {})",
        depth_preview
    ));
    if !opts.paths.is_empty() {
        display::info(format!("Scope includes: {}", opts.paths.join(", ")));
    }
    if !opts.exclude.is_empty() {
        display::info(format!("Scope excludes: {}", opts.exclude.join(", ")));
    }

    let issues_provider = if let Some(provider) = opts.issues {
        Some(provider.parse::<IssuesProvider>()?)
    } else {
        None
    };

    let report = bootstrap::bootstrap_snapshot(BootstrapOptions {
        force: opts.force,
        depth: opts.depth,
        paths: opts.paths.clone(),
        exclude: opts.exclude.clone(),
        issues_provider,
    })
    .await?;

    if !report.warnings.is_empty() {
        for note in &report.warnings {
            display::warn(format!("Warning: {}", note));
        }
    }

    let mut detail_parts = Vec::new();
    detail_parts.push(format!("analyzed_at={}", report.analysis_timestamp));
    detail_parts.push(format!(
        "branch={}",
        report.branch.as_deref().unwrap_or("<detached HEAD>")
    ));
    detail_parts.push(format!(
        "head_commit={}",
        report.head_commit.as_deref().unwrap_or("<no HEAD commit>")
    ));
    detail_parts.push(format!(
        "working_tree={}",
        if report.dirty { "dirty" } else { "clean" }
    ));
    detail_parts.push(format!("history_depth_used={}", report.depth_used));

    if !report.scope_includes.is_empty() {
        detail_parts.push(format!(
            "scope_includes={}",
            report.scope_includes.join(", ")
        ));
    }
    if !report.scope_excludes.is_empty() {
        detail_parts.push(format!(
            "scope_excludes={}",
            report.scope_excludes.join(", ")
        ));
    }
    if let Some(provider) = report.issues_provider.as_ref() {
        detail_parts.push(format!("issues_provider={}", provider));
    }
    if !report.issues.is_empty() {
        detail_parts.push(format!("issues={}", report.issues.join(", ")));
    }
    if !report.files_touched.is_empty() {
        detail_parts.push(format!("files_updated={}", report.files_touched.join(", ")));
    }

    if !detail_parts.is_empty() {
        display::info(format!("Snapshot details: {}", detail_parts.join("; ")));
    }

    if !report.summary.trim().is_empty() {
        display::info(format!("Snapshot summary: {}", report.summary.trim()));
    }

    let files_updated = report.files_touched.len();
    let outcome = if files_updated == 0 {
        format!(
            "Snapshot bootstrap complete; depth_used={}; no .vizier changes",
            report.depth_used
        )
    } else {
        format!(
            "Snapshot bootstrap complete; updated {} file{}; depth_used={}",
            files_updated,
            if files_updated == 1 { "" } else { "s" },
            report.depth_used
        )
    };

    println!("{}", outcome);

    print_token_usage();

    Ok(())
}

pub async fn run_save(
    commit_ref: &str,
    exclude: &[&str],
    commit_message: Option<String>,
    use_editor: bool,
    commit_mode: CommitMode,
    push_after_commit: bool,
    agent: &config::AgentSettings,
) -> Result<(), Box<dyn std::error::Error>> {
    match vcs::get_diff(".", Some(commit_ref), Some(exclude)) {
        Ok(diff) => match save(
            diff,
            commit_message,
            use_editor,
            commit_mode,
            push_after_commit,
            agent,
        )
        .await
        {
            Ok(outcome) => {
                println!("{}", format_save_outcome(&outcome));
                Ok(())
            }
            Err(e) => {
                display::emit(LogLevel::Error, format!("Error running --save: {e}"));
                Err(Box::<dyn std::error::Error>::from(e))
            }
        },
        Err(e) => {
            display::emit(
                LogLevel::Error,
                format!("Error generating diff for {commit_ref}: {e}"),
            );
            Err(Box::<dyn std::error::Error>::from(e))
        }
    }
}

#[derive(Debug)]
pub struct SaveOutcome {
    pub session_log: Option<String>,
    pub code_commit: Option<String>,
    pub pushed: bool,
    pub audit_state: auditor::AuditState,
    pub commit_mode: CommitMode,
}

fn format_save_outcome(outcome: &SaveOutcome) -> String {
    let mut parts = vec!["Save complete".to_string()];

    match &outcome.session_log {
        Some(path) if !path.is_empty() => parts.push(format!("session={}", path)),
        _ => parts.push("session=none".to_string()),
    }

    match &outcome.code_commit {
        Some(hash) if !hash.is_empty() => parts.push(format!("code_commit={}", short_hash(hash))),
        _ => parts.push("code_commit=none".to_string()),
    }

    parts.push(format!("mode={}", outcome.commit_mode.label()));
    parts.push(format!(
        "narrative={}",
        match outcome.audit_state {
            auditor::AuditState::Committed => "committed",
            auditor::AuditState::Pending => "pending",
            auditor::AuditState::Clean => "clean",
        }
    ));

    if outcome.pushed {
        parts.push("pushed=true".to_string());
    }

    parts.join("; ")
}

fn short_hash(hash: &str) -> String {
    const MAX: usize = 8;
    if hash.len() <= MAX {
        hash.to_string()
    } else {
        hash.chars().take(MAX).collect()
    }
}

fn narrative_change_set(result: &auditor::AuditResult) -> (Vec<String>, Option<String>) {
    result
        .narrative_changes()
        .map(|changes| (changes.paths.clone(), changes.summary.clone()))
        .unwrap_or_else(|| (Vec::new(), None))
}

fn stage_narrative_paths(paths: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if paths.is_empty() {
        return Ok(());
    }

    let refs: Vec<&str> = paths.iter().map(|p| p.as_str()).collect();
    vcs::stage(Some(refs))?;
    Ok(())
}

fn trim_staged_vizier_paths(allowed: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let allowlist: HashSet<&str> = allowed.iter().map(|p| p.as_str()).collect();
    let staged = vcs::snapshot_staged(".")?;
    let mut to_unstage: Vec<String> = Vec::new();

    for item in staged {
        match &item.kind {
            vcs::StagedKind::Renamed { from, to } => {
                if to.starts_with(".vizier/") && !allowlist.contains(to.as_str()) {
                    to_unstage.push(from.clone());
                    to_unstage.push(to.clone());
                }
            }
            _ => {
                if item.path.starts_with(".vizier/") && !allowlist.contains(item.path.as_str()) {
                    to_unstage.push(item.path.clone());
                }
            }
        }
    }

    if !to_unstage.is_empty() {
        to_unstage.sort();
        to_unstage.dedup();
        let refs: Vec<&str> = to_unstage.iter().map(|p| p.as_str()).collect();
        vcs::unstage(Some(refs))?;
    }

    Ok(())
}

fn clear_narrative_tracker(paths: &[String]) {
    file_tracking::FileTracker::clear_tracked(paths);
}

fn build_save_instruction(note: Option<&str>) -> String {
    let mut instruction =
        "<instruction>Update the snapshot and existing TODOs as needed</instruction>".to_string();

    if let Some(text) = note {
        instruction.push_str(&format!(
            "<change_author_note>{}</change_author_note>",
            text
        ));
    }

    instruction
}

async fn save(
    _initial_diff: String,
    // NOTE: These two should never be Some(...) && true
    user_message: Option<String>,
    use_message_editor: bool,
    commit_mode: CommitMode,
    push_after_commit: bool,
    agent: &config::AgentSettings,
) -> Result<SaveOutcome, Box<dyn std::error::Error>> {
    let provided_note = if let Some(message) = user_message {
        Some(message)
    } else if use_message_editor {
        if let Ok(edited) = get_editor_message() {
            Some(edited)
        } else {
            None
        }
    } else {
        None
    };

    let save_instruction = build_save_instruction(provided_note.as_deref());

    let system_prompt = if agent.backend == config::BackendKind::Codex {
        codex::build_prompt_for_codex(
            agent.scope,
            &save_instruction,
            agent.codex.bounds_prompt_path.as_deref(),
        )
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?
    } else {
        crate::config::get_system_prompt_with_meta(agent.scope, None)?
    };

    let response = Auditor::llm_request_with_tools(
        agent,
        Some(config::PromptKind::Base),
        system_prompt,
        save_instruction,
        tools::active_tooling_for(agent),
        None,
        None,
    )
    .await?;

    let audit_result = auditor::Auditor::finalize(audit_disposition(commit_mode)).await?;
    let session_display = audit_result.session_display();
    let (narrative_paths, narrative_summary) = narrative_change_set(&audit_result);
    let has_narrative_changes = !narrative_paths.is_empty();

    display::info(format!(
        "Assistant summary: {}{}",
        response.content.trim(),
        token_usage_suffix()
    ));
    print_token_usage();

    let post_tool_diff = vcs::get_diff(".", Some("HEAD"), Some(&[".vizier/"]))?;
    let has_code_changes = !post_tool_diff.trim().is_empty();
    let mut code_commit = None;

    if commit_mode.should_commit() {
        if has_code_changes || has_narrative_changes {
            let commit_body = if has_code_changes {
                Auditor::llm_request(
                    config::get_config().get_prompt(config::PromptKind::Commit),
                    post_tool_diff.clone(),
                )
                .await?
                .content
            } else {
                narrative_summary
                    .clone()
                    .unwrap_or_else(|| "Update snapshot and TODO threads".to_string())
            };

            let mut message_builder = CommitMessageBuilder::new(commit_body);
            message_builder
                .set_header(if has_code_changes {
                    CommitMessageType::CodeChange
                } else {
                    CommitMessageType::NarrativeChange
                })
                .with_session_log_path(session_display.clone());

            if has_code_changes {
                message_builder.with_narrative_summary(narrative_summary.clone());
            }

            if let Some(note) = provided_note.as_ref() {
                message_builder.with_author_note(note.clone());
            }

            stage_narrative_paths(&narrative_paths)?;
            if has_code_changes {
                vcs::stage(Some(vec!["."]))?;
            } else {
                vcs::stage(None)?;
            }
            trim_staged_vizier_paths(&narrative_paths)?;

            let commit_message = message_builder.build();

            display::info("Committing combined changes...");
            let commit_oid = vcs::commit_staged(&commit_message, false)?;
            display::info(format!(
                "Changes committed with message: {}",
                commit_message
            ));

            clear_narrative_tracker(&narrative_paths);
            code_commit = Some(commit_oid.to_string());
        } else {
            display::info("No code or narrative changes detected; skipping commit.");
        }
    } else {
        if has_narrative_changes {
            display::info(
                "Snapshot/TODO updates left pending (--no-commit); review and commit when ready.",
            );
        }
        if has_code_changes {
            display::info(
                "Code changes detected but --no-commit is active; leaving them staged/dirty.",
            );
        } else if provided_note.is_some() {
            display::info(
                "Author note provided but no code changes detected; skipping code commit.",
            );
        }
    }

    let mut pushed = false;
    if commit_mode.should_commit() && push_after_commit {
        if code_commit.is_some() {
            push_origin_if_requested(true)?;
            pushed = true;
        } else {
            display::info("Push skipped because no commit was created.");
        }
    } else if push_after_commit {
        display::info("Push skipped because --no-commit is active.");
    }

    Ok(SaveOutcome {
        session_log: session_display,
        code_commit,
        pushed,
        audit_state: audit_result.state,
        commit_mode,
    })
}

enum Shell {
    Bash,
    Zsh,
    Fish,
    Other,
}

impl Shell {
    fn from_path(shell_path: &str) -> Self {
        let shell_name = std::path::PathBuf::from(shell_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_lowercase();

        match shell_name.as_str() {
            "bash" => Shell::Bash,
            "zsh" => Shell::Zsh,
            "fish" => Shell::Fish,
            _ => Shell::Other,
        }
    }

    fn get_rc_source_command(&self) -> String {
        match self {
            Shell::Bash => ". ~/.bashrc".to_string(),
            Shell::Zsh => ". ~/.zshrc".to_string(),
            Shell::Fish => "source ~/.config/fish/config.fish".to_string(),
            Shell::Other => "".to_string(),
        }
    }

    fn get_interactive_args(&self) -> Vec<String> {
        match self {
            Shell::Fish => vec!["-C".to_string()],
            _ => vec!["-i".to_string(), "-c".to_string()],
        }
    }
}

fn get_editor_message() -> Result<String, Box<dyn std::error::Error>> {
    let temp_file = Builder::new()
        .prefix("tllm_input")
        .suffix(".md")
        .tempfile()?;

    let temp_path: TempPath = temp_file.into_temp_path();

    match std::fs::write(temp_path.to_path_buf(), "") {
        Ok(_) => {}
        Err(e) => {
            display::emit(LogLevel::Error, "Error writing to temp file");
            return Err(Box::new(e));
        }
    };

    let shell_path = env::var("SHELL").unwrap_or_else(|_| "bash".to_string());
    let editor = env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
    let shell = Shell::from_path(&shell_path);

    let command = format!("{} {}", editor, temp_path.to_str().unwrap());
    let rc_source = shell.get_rc_source_command();
    let full_command = if rc_source.is_empty() {
        command
    } else {
        format!("{} && {}", rc_source, command)
    };

    let status = Command::new(shell_path)
        .args(shell.get_interactive_args())
        .arg("-c")
        .arg(&full_command)
        .status()?;

    if !status.success() {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Editor command failed",
        )));
    }

    let user_message = match fs::read_to_string(&temp_path) {
        Ok(contents) => {
            if contents.is_empty() {
                return Ok(String::new());
            }

            contents
        }
        Err(e) => {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Error reading file: {}", e),
            )));
        }
    };

    Ok(user_message)
}

/// NOTE: Filters out hidden entries; every visible file in `.vizier/` is a TODO candidate.
///
/// This is `vizier ask`
pub async fn inline_command(
    user_message: String,
    push_after_commit: bool,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
) -> Result<(), Box<dyn std::error::Error>> {
    let system_prompt = if agent.backend == config::BackendKind::Codex {
        match codex::build_prompt_for_codex(
            agent.scope,
            &user_message,
            agent.codex.bounds_prompt_path.as_deref(),
        ) {
            Ok(prompt) => prompt,
            Err(e) => {
                display::emit(
                    LogLevel::Error,
                    format!("Error building Codex prompt: {}", e),
                );
                return Err(Box::<dyn std::error::Error>::from(e));
            }
        }
    } else {
        match crate::config::get_system_prompt_with_meta(agent.scope, None) {
            Ok(s) => s,
            Err(e) => {
                display::emit(LogLevel::Error, format!("Error loading system prompt: {e}"));
                return Err(Box::<dyn std::error::Error>::from(e));
            }
        }
    };

    let response = match Auditor::llm_request_with_tools(
        agent,
        Some(config::PromptKind::Base),
        system_prompt,
        user_message,
        tools::active_tooling_for(agent),
        None,
        None,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            display::emit(LogLevel::Error, format!("Error during LLM request: {e}"));
            return Err(Box::<dyn std::error::Error>::from(e));
        }
    };

    let audit_result = match auditor::Auditor::finalize(audit_disposition(commit_mode)).await {
        Ok(outcome) => outcome,
        Err(e) => {
            display::emit(LogLevel::Error, format!("Error finalizing audit: {e}"));
            return Err(Box::<dyn std::error::Error>::from(e));
        }
    };
    let session_display = audit_result.session_display();
    let (narrative_paths, narrative_summary) = narrative_change_set(&audit_result);
    let has_narrative_changes = !narrative_paths.is_empty();

    let post_tool_diff = vcs::get_diff(".", Some("HEAD"), Some(&[".vizier/"]))?;
    let has_code_changes = !post_tool_diff.trim().is_empty();
    let mut commit_oid: Option<String> = None;

    if commit_mode.should_commit() {
        if has_code_changes || has_narrative_changes {
            let commit_body = if has_code_changes {
                Auditor::llm_request(
                    config::get_config().get_prompt(config::PromptKind::Commit),
                    post_tool_diff.clone(),
                )
                .await?
                .content
            } else {
                narrative_summary
                    .clone()
                    .unwrap_or_else(|| "Update snapshot and TODO threads".to_string())
            };

            let mut builder = CommitMessageBuilder::new(commit_body);
            builder
                .set_header(if has_code_changes {
                    CommitMessageType::CodeChange
                } else {
                    CommitMessageType::NarrativeChange
                })
                .with_session_log_path(session_display.clone());

            if has_code_changes {
                builder.with_narrative_summary(narrative_summary.clone());
            }

            stage_narrative_paths(&narrative_paths)?;
            if has_code_changes {
                vcs::stage(Some(vec!["."]))?;
            } else {
                vcs::stage(None)?;
            }
            trim_staged_vizier_paths(&narrative_paths)?;

            let commit_message = builder.build();
            let oid = vcs::commit_staged(&commit_message, false)?;
            clear_narrative_tracker(&narrative_paths);
            commit_oid = Some(oid.to_string());
        } else {
            display::info("No code or narrative changes detected; skipping commit.");
        }
    } else {
        if has_narrative_changes {
            display::info(
                "Held .vizier changes for manual review (--no-commit active); commit them when ready.",
            );
        }
        if has_code_changes {
            display::info(
                "Code changes detected but --no-commit is active; leaving them staged/dirty.",
            );
        }
    }

    if commit_mode.should_commit() {
        if commit_oid.is_some() {
            push_origin_if_requested(push_after_commit)?;
        } else if push_after_commit {
            display::info("Push skipped because no commit was created.");
        }
    } else if push_after_commit {
        display::info("Push skipped because --no-commit is active.");
    }

    println!("{}", response.content.trim_end());
    print_token_usage();

    Ok(())
}

pub async fn run_draft(
    args: DraftArgs,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
) -> Result<(), Box<dyn std::error::Error>> {
    if agent.backend != config::BackendKind::Codex {
        return Err("vizier draft requires the Codex backend; update [agents.draft] or pass --backend codex".into());
    }

    let DraftArgs {
        spec_text,
        spec_source,
        name_override,
    } = args;

    let repo_root = repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let plan_dir_main = repo_root.join(".vizier/implementation-plans");
    let branch_prefix = "draft/";

    let base_slug = if let Some(name) = name_override {
        plan::sanitize_name_override(&name)?
    } else {
        plan::slug_from_spec(&spec_text)
    };

    let slug = plan::ensure_unique_slug(&base_slug, &plan_dir_main, branch_prefix)?;
    let branch_name = format!("{branch_prefix}{slug}");
    let plan_rel_path = plan::plan_rel_path(&slug);
    let plan_display = plan_rel_path.to_string_lossy().to_string();

    let tmp_root = repo_root.join(".vizier/tmp-worktrees");
    fs::create_dir_all(&tmp_root)?;
    let worktree_suffix = plan::short_suffix();
    let worktree_name = format!("vizier-draft-{slug}-{worktree_suffix}");
    let worktree_path = tmp_root.join(format!("{slug}-{worktree_suffix}"));
    let plan_in_worktree = worktree_path.join(&plan_rel_path);

    let spec_source_label = spec_source.as_metadata_value();
    let mut plan_document_preview: Option<String> = None;

    let primary_branch = detect_primary_branch()
        .ok_or_else(|| "unable to detect a primary branch (tried origin/HEAD, main, master)")?;

    let mut branch_created = false;
    let mut worktree_created = false;
    let mut plan_committed = false;

    let plan_result: Result<(), Box<dyn std::error::Error>> = async {
        create_branch_from(&primary_branch, &branch_name).map_err(|err| {
            Box::<dyn std::error::Error>::from(format!(
                "create_branch_from({}<-{}): {}",
                branch_name, primary_branch, err
            ))
        })?;
        branch_created = true;

        add_worktree_for_branch(&worktree_name, &worktree_path, &branch_name).map_err(|err| {
            display::emit(
                LogLevel::Debug,
                format!(
                    "failed adding worktree {} at {}: {}",
                    worktree_name,
                    worktree_path.display(),
                    err
                ),
            );
            Box::<dyn std::error::Error>::from(format!(
                "add_worktree({}, {}): {}",
                worktree_name,
                worktree_path.display(),
                err
            ))
        })?;
        worktree_created = true;
        let _cwd_guard = WorkdirGuard::enter(&worktree_path)?;

        let prompt = codex::build_implementation_plan_prompt(
            agent.scope,
            &slug,
            &branch_name,
            &spec_text,
            agent.codex.bounds_prompt_path.as_deref(),
        )
        .map_err(|err| -> Box<dyn std::error::Error> {
            Box::from(format!("build_prompt: {err}"))
        })?;

        let llm_response = Auditor::llm_request_with_tools(
            agent,
            Some(config::PromptKind::ImplementationPlan),
            prompt,
            spec_text.clone(),
            Vec::new(),
            Some(codex::CodexModel::Gpt5Codex),
            Some(worktree_path.clone()),
        )
        .await
        .map_err(|err| Box::<dyn std::error::Error>::from(format!("Codex: {err}")))?;

        let plan_body = llm_response.content;
        let document = plan::render_plan_document(
            &slug,
            &branch_name,
            &spec_source_label,
            &spec_text,
            &plan_body,
        );
        plan_document_preview = Some(document.clone());
        plan::write_plan_file(&plan_in_worktree, &document).map_err(
            |err| -> Box<dyn std::error::Error> {
                Box::from(format!(
                    "write_plan_file({}): {err}",
                    plan_in_worktree.display()
                ))
            },
        )?;

        if commit_mode.should_commit() {
            let plan_rel = plan_rel_path.as_path();
            commit_paths_in_repo(
                &worktree_path,
                &[plan_rel],
                &format!("docs: add implementation plan {}", slug),
            )
            .map_err(|err| -> Box<dyn std::error::Error> {
                Box::from(format!("commit_plan({}): {err}", worktree_path.display()))
            })?;
            plan_committed = true;
            if Auditor::persist_session_log().is_some() {
                Auditor::clear_messages();
            }
        } else {
            display::info(
                "Plan document generated with --no-commit; leaving worktree dirty for manual review.",
            );
        }

        Ok(())
    }
    .await;

    match plan_result {
        Ok(()) => {
            let plan_to_print = plan_document_preview
                .clone()
                .or_else(|| fs::read_to_string(&plan_in_worktree).ok());

            if worktree_created && commit_mode.should_commit() {
                if let Err(err) = remove_worktree(&worktree_name, true) {
                    display::warn(format!(
                        "temporary worktree cleanup failed ({}); remove manually with `git worktree prune`",
                        err
                    ));
                }
                if worktree_path.exists() {
                    let _ = fs::remove_dir_all(&worktree_path);
                }
            }

            if commit_mode.should_commit() {
                display::info(format!(
                    "View with: git checkout {branch_name} && $EDITOR {plan_display}"
                ));
                println!("Draft ready; plan={plan_display}; branch={branch_name}");
            } else {
                println!(
                    "Draft pending (manual commit); branch={branch_name}; worktree={}; plan={plan_display}",
                    worktree_path.display()
                );
                display::info(format!(
                    "Review and commit manually: git -C {} status",
                    worktree_path.display()
                ));
            }

            if let Some(plan_text) = plan_to_print {
                println!();
                println!("{plan_text}");
            }
            print_token_usage();
            Ok(())
        }
        Err(err) => {
            if worktree_created {
                let _ = remove_worktree(&worktree_name, true);
                if worktree_path.exists() {
                    let _ = fs::remove_dir_all(&worktree_path);
                }
            }
            if branch_created && !plan_committed {
                let _ = delete_branch(&branch_name);
            } else if plan_committed {
                display::info(format!(
                    "Draft artifacts preserved on {branch_name}; inspect with `git checkout {branch_name}`"
                ));
            }
            Err(err)
        }
    }
}

pub fn run_list(opts: ListOptions) -> Result<(), Box<dyn std::error::Error>> {
    list_pending_plans(opts.target)
}

pub async fn run_approve(
    opts: ApproveOptions,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
) -> Result<(), Box<dyn std::error::Error>> {
    if opts.list_only {
        display::warn("`vizier approve --list` is deprecated; use `vizier list` instead.");
        return list_pending_plans(opts.target.clone());
    }

    if agent.backend != config::BackendKind::Codex {
        return Err("vizier approve requires the Codex backend; update [agents.approve] or pass --backend codex".into());
    }

    let spec = plan::PlanBranchSpec::resolve(
        opts.plan.as_deref(),
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
        println!(
            "Plan {} already merged into {}; latest commit={}",
            spec.slug, spec.target_branch, source_oid
        );
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
            return Ok(());
        }
    }

    let worktree = plan::PlanWorktree::create(&spec.slug, &spec.branch, "approve")?;
    let worktree_path = worktree.path().to_path_buf();
    let plan_path = worktree.plan_path(&spec.slug);
    let mut worktree = Some(worktree);

    let approval = apply_plan_in_worktree(
        &spec,
        &plan_meta,
        &worktree_path,
        &plan_path,
        opts.push_after,
        commit_mode,
        agent,
    )
    .await;

    match approval {
        Ok(result) => {
            if commit_mode.should_commit() {
                if let Some(tree) = worktree.take() {
                    if let Err(err) = tree.cleanup() {
                        display::warn(format!(
                            "temporary worktree cleanup failed ({}); remove manually with `git worktree prune`",
                            err
                        ));
                    }
                }

                if let Some(commit_oid) = result.commit_oid.as_ref() {
                    println!(
                        "Plan {} implemented on {}; latest commit={}; review with `{}`",
                        spec.slug,
                        spec.branch,
                        commit_oid,
                        spec.diff_command()
                    );
                } else {
                    println!(
                        "Plan {} implemented on {}; review with `{}`",
                        spec.slug,
                        spec.branch,
                        spec.diff_command()
                    );
                }
            } else {
                display::info(format!(
                    "Plan worktree preserved at {}; inspect branch {} for pending changes.",
                    worktree_path.display(),
                    spec.branch
                ));
                println!(
                    "Plan {} pending manual commit; branch={} worktree={} diff=\"{}\"",
                    spec.slug,
                    spec.branch,
                    worktree_path.display(),
                    spec.diff_command()
                );
            }
            print_token_usage();
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

pub async fn run_review(
    opts: ReviewOptions,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
) -> Result<(), Box<dyn std::error::Error>> {
    if agent.backend != config::BackendKind::Codex {
        return Err("vizier review requires the Codex backend; update [agents.review] or pass --backend codex".into());
    }

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
        println!(
            "Plan {} already merged into {}; latest commit={}",
            spec.slug, spec.target_branch, source_oid
        );
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
        },
        commit_mode,
        agent,
    )
    .await;

    match review_result {
        Ok(outcome) => {
            if commit_mode.should_commit() {
                if let Some(tree) = worktree.take() {
                    if let Err(err) = tree.cleanup() {
                        display::warn(format!(
                            "temporary worktree cleanup failed ({}); remove manually with `git worktree prune`",
                            err
                        ));
                    }
                }
            } else if let Some(tree) = worktree.take() {
                display::info(format!(
                    "Review worktree preserved at {}; inspect branch {} for pending critique/fix artifacts.",
                    tree.path().display(),
                    spec.branch
                ));
            }

            if outcome.branch_mutated && opts.push_after && commit_mode.should_commit() {
                push_origin_if_requested(true)?;
            } else if outcome.branch_mutated && opts.push_after {
                display::info("Push skipped because --no-commit left review changes pending.");
            }

            if let Some(commit) = outcome.fix_commit.as_ref() {
                display::info(format!(
                    "Fixes addressing review feedback committed at {} on {}",
                    commit, spec.branch
                ));
            }

            println!(
                "Review complete; plan={} branch={} critique={} checks={}/{} diff=\"{}\" session={}{}",
                spec.slug,
                spec.branch,
                outcome.critique_label,
                outcome.checks_passed,
                outcome.checks_total,
                outcome.diff_command,
                outcome
                    .session_path
                    .clone()
                    .unwrap_or_else(|| "<unknown>".to_string()),
                token_usage_suffix()
            );
            print_token_usage();
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

pub async fn run_merge(
    opts: MergeOptions,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
) -> Result<(), Box<dyn std::error::Error>> {
    if !commit_mode.should_commit() {
        return Err("--no-commit is not supported for vizier merge; rerun without the flag once you are ready to finalize the merge."
            .into());
    }
    let spec = plan::PlanBranchSpec::resolve(
        Some(opts.plan.as_str()),
        opts.branch_override.as_deref(),
        opts.target.as_deref(),
    )?;

    if matches!(opts.conflict_strategy, MergeConflictStrategy::Codex)
        && agent.backend != config::BackendKind::Codex
    {
        return Err(
            "Codex-based conflict resolution requires the Codex backend; update [agents.merge] or rerun with --backend codex"
                .into(),
        );
    }

    if opts.cicd_gate.auto_resolve
        && opts.cicd_gate.script.is_some()
        && agent.backend != config::BackendKind::Codex
    {
        display::warn(
            "CI/CD auto-remediation requested but [agents.merge] is not set to Codex; gate failures will abort without auto fixes.",
        );
    }

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
        println!(
            "Plan {} already merged into {}; latest commit={}",
            spec.slug, spec.target_branch, source_oid
        );
        return Ok(());
    }

    match try_complete_pending_merge(&spec)? {
        PendingMergeStatus::Ready {
            merge_oid,
            source_oid,
        } => {
            let gate_summary = run_cicd_gate_for_merge(&spec, &opts, agent).await?;
            finalize_merge(
                &spec,
                merge_oid,
                source_oid,
                opts.delete_branch,
                opts.push_after,
                gate_summary,
            )?;
            return Ok(());
        }
        PendingMergeStatus::Blocked(blocker) => {
            return Err(Box::new(PendingMergeError::new(spec.slug.clone(), blocker)));
        }
        PendingMergeStatus::None => {
            if opts.complete_conflict {
                return Err(Box::<dyn std::error::Error>::from(format!(
                    "No Vizier-managed merge is awaiting completion for plan {}; rerun `vizier merge {}` without --complete-conflict to start a merge.",
                    spec.slug, spec.slug
                )));
            }
        }
    }

    vcs::ensure_clean_worktree().map_err(|err| {
        Box::<dyn std::error::Error>::from(format!(
            "clean working tree required before merge: {err}"
        ))
    })?;

    let plan_meta = spec.load_metadata()?;

    if !opts.assume_yes {
        spec.show_preview(&plan_meta);
        if !prompt_for_confirmation("Merge this plan? [y/N] ")? {
            println!("Merge cancelled; no changes were made.");
            return Ok(());
        }
    }

    let worktree = plan::PlanWorktree::create(&spec.slug, &spec.branch, "merge")?;
    let worktree_path = worktree.path().to_path_buf();
    let plan_path = worktree.plan_path(&spec.slug);
    let mut worktree = Some(worktree);
    let plan_document = fs::read_to_string(&plan_path).ok();

    if plan_path.exists() {
        display::info(format!(
            "Removing {} from the plan branch before merge",
            spec.plan_rel_path().display()
        ));
        if let Err(err) = fs::remove_file(&plan_path) {
            return Err(Box::<dyn std::error::Error>::from(format!(
                "failed to remove {} before merge: {}",
                plan_path.display(),
                err
            )));
        }
    }

    if let Err(err) =
        refresh_plan_branch(&spec, &plan_meta, &worktree_path, opts.push_after, agent).await
    {
        display::warn(format!(
            "Plan worktree preserved at {}; inspect {} for unresolved narrative changes.",
            worktree.as_ref().unwrap().path().display(),
            spec.branch
        ));
        return Err(err);
    }

    if let Some(tree) = worktree.take() {
        if let Err(err) = tree.cleanup() {
            display::warn(format!(
                "temporary worktree cleanup failed ({}); remove manually with `git worktree prune`",
                err
            ));
        }
    }

    let current_branch = current_branch_name(&repo)?;
    if current_branch.as_deref() != Some(spec.target_branch.as_str()) {
        display::info(format!(
            "Checking out {} before merge...",
            spec.target_branch
        ));
        vcs::checkout_branch(&spec.target_branch)?;
    }

    let merge_message = build_merge_commit_message(
        &spec,
        &plan_meta,
        plan_document.as_deref(),
        opts.note.as_deref(),
    );

    let (merge_oid, source_oid) = match prepare_merge(&spec.branch)? {
        MergePreparation::Ready(ready) => {
            let source_tip = ready.source_oid;
            let oid = commit_ready_merge(&merge_message, ready)?;
            (oid, source_tip)
        }
        MergePreparation::Conflicted(conflict) => {
            handle_merge_conflict(
                &spec,
                &merge_message,
                conflict,
                opts.conflict_strategy,
                agent,
            )
            .await?
        }
    };

    let gate_summary = run_cicd_gate_for_merge(&spec, &opts, agent).await?;
    finalize_merge(
        &spec,
        merge_oid,
        source_oid,
        opts.delete_branch,
        opts.push_after,
        gate_summary,
    )?;
    Ok(())
}

async fn run_cicd_gate_for_merge(
    spec: &plan::PlanBranchSpec,
    opts: &MergeOptions,
    agent: &config::AgentSettings,
) -> Result<Option<CicdGateOutcome>, Box<dyn std::error::Error>> {
    let Some(script) = opts.cicd_gate.script.as_ref() else {
        return Ok(None);
    };

    let repo_root = repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let mut attempts: u32 = 0;
    let mut fix_attempts: u32 = 0;
    let mut fix_commits = Vec::new();

    loop {
        attempts += 1;
        let result = run_cicd_script(script, &repo_root)?;
        log_cicd_result(script, &result, attempts);

        if result.success() {
            return Ok(Some(CicdGateOutcome {
                script: script.clone(),
                attempts,
                fixes: fix_commits,
            }));
        }

        if !opts.cicd_gate.auto_resolve {
            return Err(cicd_gate_failure_error(script, &result));
        }

        if agent.backend != config::BackendKind::Codex {
            display::warn(
                "CI/CD gate auto-remediation requires the Codex backend; skipping automatic fixes.",
            );
            return Err(cicd_gate_failure_error(script, &result));
        }

        if fix_attempts >= opts.cicd_gate.retries {
            display::warn(format!(
                "CI/CD auto-remediation exhausted its retry budget ({} attempt(s)).",
                opts.cicd_gate.retries
            ));
            return Err(cicd_gate_failure_error(script, &result));
        }

        fix_attempts += 1;
        display::info(format!(
            "CI/CD gate failed; attempting Codex remediation ({}/{})...",
            fix_attempts, opts.cicd_gate.retries
        ));
        let truncated_stdout = clip_log(result.stdout.as_bytes());
        let truncated_stderr = clip_log(result.stderr.as_bytes());
        if let Some(commit) = attempt_cicd_auto_fix(
            spec,
            script,
            fix_attempts,
            opts.cicd_gate.retries,
            result.status.code(),
            &truncated_stdout,
            &truncated_stderr,
            agent,
        )
        .await?
        {
            display::info(format!("Remediation attempt committed at {}.", commit));
            fix_commits.push(commit);
        } else {
            display::info("Codex remediation reported no file changes.");
        }
    }
}

async fn attempt_cicd_auto_fix(
    spec: &plan::PlanBranchSpec,
    script: &Path,
    attempt: u32,
    max_attempts: u32,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    agent: &config::AgentSettings,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let prompt = codex::build_cicd_failure_prompt(
        &spec.slug,
        &spec.branch,
        &spec.target_branch,
        script,
        attempt,
        max_attempts,
        exit_code,
        stdout,
        stderr,
        agent.codex.bounds_prompt_path.as_deref(),
    )
    .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let instruction = format!(
        "CI/CD gate script {} failed while merging plan {} (attempt {attempt}/{max_attempts}). Apply fixes so the script succeeds.",
        script.display(),
        spec.slug
    );

    let repo_root = repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let request_root = repo_root.clone();
    let (event_tx, event_rx) = mpsc::channel(64);
    let progress_handle = spawn_plain_progress_logger(event_rx);
    let (text_tx, _text_rx) = mpsc::channel(1);
    let response = Auditor::llm_request_with_tools_no_display(
        agent,
        Some(config::PromptKind::Review),
        prompt,
        instruction,
        tools::active_tooling_for(agent),
        auditor::RequestStream::Status {
            text: text_tx,
            events: Some(event_tx),
        },
        Some(codex::CodexModel::Gpt5Codex),
        Some(request_root),
    )
    .await?;
    if let Some(handle) = progress_handle {
        let _ = handle.await;
    }

    #[cfg(feature = "mock_llm")]
    {
        mock_cicd_remediation(&repo_root)?;
    }

    let audit_result = Auditor::commit_audit().await?;
    let session_path = audit_result.session_display();
    let (narrative_paths, narrative_summary) = narrative_change_set(&audit_result);

    let diff = vcs::get_diff(".", Some("HEAD"), None)?;
    if diff.trim().is_empty() {
        return Ok(None);
    }

    let mut summary = response.content.trim().to_string();
    if summary.is_empty() {
        summary = format!(
            "Fix CI/CD gate failure for plan {} (attempt {attempt}/{max_attempts})",
            spec.slug
        );
    }
    let exit_label = exit_code
        .map(|code| code.to_string())
        .unwrap_or_else(|| "signal".to_string());
    let mut builder = CommitMessageBuilder::new(summary);
    builder
        .set_header(CommitMessageType::CodeChange)
        .with_session_log_path(session_path.clone())
        .with_narrative_summary(narrative_summary.clone())
        .with_author_note(format!(
            "CI/CD script: {} (exit={})",
            script.display(),
            exit_label
        ));
    let message = builder.build();

    stage_narrative_paths(&narrative_paths)?;
    vcs::stage(Some(vec!["."]))?;
    trim_staged_vizier_paths(&narrative_paths)?;
    let commit_oid = vcs::commit_staged(&message, false)?;
    clear_narrative_tracker(&narrative_paths);

    Ok(Some(commit_oid.to_string()))
}

fn finalize_merge(
    spec: &plan::PlanBranchSpec,
    merge_oid: Oid,
    source_oid: Oid,
    delete_branch: bool,
    push_after: bool,
    gate: Option<CicdGateOutcome>,
) -> Result<(), Box<dyn std::error::Error>> {
    if delete_branch {
        let repo = Repository::discover(".")?;
        if repo.graph_descendant_of(merge_oid, source_oid)? {
            vcs::delete_branch(&spec.branch)?;
            display::info(format!("Deleted {} after merge", spec.branch));
        } else {
            display::warn(format!(
                "Skipping deletion of {}; merge commit did not include the branch tip.",
                spec.branch
            ));
        }
    } else {
        display::info(format!(
            "Keeping {} after merge (branch deletion disabled).",
            spec.branch
        ));
    }

    push_origin_if_requested(push_after)?;
    if let Some(summary) = gate.as_ref() {
        let fix_count = summary.fixes.len();
        let fix_detail = if summary.fixes.is_empty() {
            String::new()
        } else {
            format!(" fixes=[{}]", summary.fixes.join(","))
        };
        let script_label = repo_root()
            .ok()
            .and_then(|root| {
                summary
                    .script
                    .strip_prefix(&root)
                    .ok()
                    .map(|p| p.display().to_string())
            })
            .unwrap_or_else(|| summary.script.display().to_string());
        println!(
            "Merged plan {} into {}; merge_commit={} cicd_script={} attempts={}{}{}{}",
            spec.slug,
            spec.target_branch,
            merge_oid,
            script_label,
            summary.attempts,
            if summary.fixes.is_empty() {
                String::new()
            } else {
                format!(" fix_commits={}", fix_count)
            },
            fix_detail,
            token_usage_suffix()
        );
    } else {
        println!(
            "Merged plan {} into {}; merge_commit={}{}",
            spec.slug,
            spec.target_branch,
            merge_oid,
            token_usage_suffix()
        );
    }
    print_token_usage();
    Ok(())
}

fn try_complete_pending_merge(
    spec: &plan::PlanBranchSpec,
) -> Result<PendingMergeStatus, Box<dyn std::error::Error>> {
    let Some(state) = read_conflict_state(&spec.slug)? else {
        return Ok(PendingMergeStatus::None);
    };

    if state.source_branch != spec.branch {
        display::warn(format!(
            "Found stale merge-conflict metadata for {}; stored branch {}!=requested {}. Cleaning it up.",
            spec.slug, state.source_branch, spec.branch
        ));
        let _ = clear_conflict_state(&spec.slug);
        return Ok(PendingMergeStatus::None);
    }

    if state.target_branch != spec.target_branch {
        display::warn(format!(
            "Found stale merge-conflict metadata for {}; stored target {}!=requested {}. Cleaning it up.",
            spec.slug, state.target_branch, spec.target_branch
        ));
        let _ = clear_conflict_state(&spec.slug);
        return Ok(PendingMergeStatus::None);
    }

    let repo = Repository::discover(".")?;
    let current_branch = current_branch_name(&repo)?;
    if current_branch.as_deref() != Some(spec.target_branch.as_str()) {
        return Ok(PendingMergeStatus::Blocked(
            PendingMergeBlocker::WrongCheckout {
                expected_branch: spec.target_branch.clone(),
            },
        ));
    }

    if repo.state() != RepositoryState::Merge {
        display::warn(format!(
            "Merge metadata for plan {} exists but the repository is no longer in a merge state; assuming it was aborted.",
            spec.slug
        ));
        let _ = clear_conflict_state(&spec.slug);
        return Ok(PendingMergeStatus::Blocked(
            PendingMergeBlocker::NotInMerge {
                target_branch: spec.target_branch.clone(),
            },
        ));
    }

    let outstanding = list_conflicted_paths()?;
    if !outstanding.is_empty() {
        display::warn("Merge conflicts remain:");
        for path in &outstanding {
            display::warn(format!("  - {path}"));
        }
        display::info(format!(
            "Resolve the conflicts above, stage the files, then rerun `vizier merge {} --complete-conflict`.",
            spec.slug
        ));
        return Ok(PendingMergeStatus::Blocked(
            PendingMergeBlocker::Conflicts { files: outstanding },
        ));
    }

    let head_oid = Oid::from_str(&state.head_oid)?;
    let source_oid = Oid::from_str(&state.source_oid)?;
    let merge_oid = commit_in_progress_merge(&state.merge_message, head_oid, source_oid)?;
    let _ = clear_conflict_state(&spec.slug);
    display::info("Conflicts resolved; finalizing merge now.");
    Ok(PendingMergeStatus::Ready {
        merge_oid,
        source_oid,
    })
}

async fn handle_merge_conflict(
    spec: &plan::PlanBranchSpec,
    merge_message: &str,
    conflict: vcs::MergeConflict,
    strategy: MergeConflictStrategy,
    agent: &config::AgentSettings,
) -> Result<(Oid, Oid), Box<dyn std::error::Error>> {
    let files = conflict.files.clone();
    let state = MergeConflictState {
        slug: spec.slug.clone(),
        source_branch: spec.branch.clone(),
        target_branch: spec.target_branch.clone(),
        head_oid: conflict.head_oid.to_string(),
        source_oid: conflict.source_oid.to_string(),
        merge_message: merge_message.to_string(),
    };

    let state_path = write_conflict_state(&state)?;
    match strategy {
        MergeConflictStrategy::Manual => {
            emit_conflict_instructions(&spec.slug, &files, &state_path);
            Err("merge blocked by conflicts; resolve them and rerun vizier merge".into())
        }
        MergeConflictStrategy::Codex => {
            match try_auto_resolve_conflicts(spec, &state, &files, agent).await {
                Ok((merge_oid, source_oid)) => Ok((merge_oid, source_oid)),
                Err(err) => {
                    display::warn(format!(
                        "Codex auto-resolution failed: {err}. Falling back to manual resolution."
                    ));
                    emit_conflict_instructions(&spec.slug, &files, &state_path);
                    Err("merge blocked by conflicts; resolve them and rerun vizier merge".into())
                }
            }
        }
    }
}

fn emit_conflict_instructions(slug: &str, files: &[String], state_path: &Path) {
    if files.is_empty() {
        display::warn("Merge resulted in conflicts; run `git status` to inspect them.");
    } else {
        display::warn("Merge conflicts detected in:");
        for file in files {
            display::warn(format!("  - {file}"));
        }
    }

    display::info(format!(
        "Resolve the conflicts, stage the results, then rerun `vizier merge {slug} --complete-conflict` to finish the merge."
    ));
    display::info(format!(
        "Vizier stored merge metadata at {}; keep it until the merge completes.",
        state_path.display()
    ));
}

async fn try_auto_resolve_conflicts(
    spec: &plan::PlanBranchSpec,
    state: &MergeConflictState,
    files: &[String],
    agent: &config::AgentSettings,
) -> Result<(Oid, Oid), Box<dyn std::error::Error>> {
    display::info("Attempting to resolve conflicts with Codex...");
    let prompt = codex::build_merge_conflict_prompt(
        agent.scope,
        &spec.target_branch,
        &spec.branch,
        files,
        agent.codex.bounds_prompt_path.as_deref(),
    )?;
    let repo_root = repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let request = codex::CodexRequest {
        prompt,
        repo_root,
        profile: agent.codex.profile.clone(),
        bin: agent.codex.binary_path.clone(),
        extra_args: agent.codex.extra_args.clone(),
        model: codex::CodexModel::Gpt5Codex,
        output_mode: codex::CodexOutputMode::EventsJson,
    };

    let (progress_tx, progress_rx) = mpsc::channel(64);
    let progress_handle = spawn_plain_progress_logger(progress_rx);
    let result = codex::run_exec(request, Some(codex::ProgressHook::Plain(progress_tx))).await;
    if let Some(handle) = progress_handle {
        let _ = handle.await;
    }
    result.map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;

    #[cfg(feature = "mock_llm")]
    {
        mock_conflict_resolution(files)?;
    }

    if files.is_empty() {
        vcs::stage(None)?;
    } else {
        let paths: Vec<&str> = files.iter().map(|s| s.as_str()).collect();
        vcs::stage(Some(paths))?;
    }

    let remaining = list_conflicted_paths()?;
    if !remaining.is_empty() {
        return Err("conflicts remain after Codex attempt".into());
    }

    let head_oid = Oid::from_str(&state.head_oid)?;
    let source_oid = Oid::from_str(&state.source_oid)?;
    let merge_oid = commit_in_progress_merge(&state.merge_message, head_oid, source_oid)?;
    clear_conflict_state(&state.slug)?;
    display::info("Codex resolved the conflicts; finalizing merge.");
    Ok((merge_oid, source_oid))
}

#[cfg(feature = "mock_llm")]
fn mock_conflict_resolution(files: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    for rel in files {
        let path = Path::new(rel);
        if !path.exists() {
            continue;
        }

        let contents = std::fs::read_to_string(path)?;
        if let Some(resolved) = strip_conflict_markers(&contents) {
            std::fs::write(path, resolved)?;
        }
    }

    Ok(())
}

#[cfg(feature = "mock_llm")]
fn strip_conflict_markers(input: &str) -> Option<String> {
    if !input.contains("<<<<<<<") {
        return None;
    }

    let mut output = String::new();
    let mut remainder = input;

    while let Some(start) = remainder.find("<<<<<<<") {
        let (before, after_start) = remainder.split_at(start);
        output.push_str(before);

        let (_, after_marker) = after_start.split_once("<<<<<<<")?;
        let (_, after_left) = after_marker.split_once("=======")?;
        let (right, after_right) = after_left.split_once(">>>>>>>")?;
        output.push_str(right);

        if let Some(idx) = after_right.find('\n') {
            remainder = &after_right[idx + 1..];
        } else {
            remainder = "";
        }
    }

    output.push_str(remainder);
    Some(output)
}

#[cfg(feature = "mock_llm")]
fn mock_cicd_remediation(repo_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let instructions = repo_root.join(".vizier/tmp/mock_cicd_fix_path");
    if !instructions.exists() {
        return Ok(());
    }
    let rel = std::fs::read_to_string(&instructions)?;
    let trimmed = rel.trim();
    if trimmed.is_empty() {
        let _ = std::fs::remove_file(&instructions);
        return Ok(());
    }
    let target = repo_root.join(trimmed);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, "mock ci fix applied\n")?;
    let _ = std::fs::remove_file(&instructions);
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct MergeConflictState {
    slug: String,
    source_branch: String,
    target_branch: String,
    head_oid: String,
    source_oid: String,
    merge_message: String,
}

fn merge_conflict_state_path(slug: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let root = repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    Ok(root
        .join(".vizier/tmp/merge-conflicts")
        .join(format!("{slug}.json")))
}

fn write_conflict_state(state: &MergeConflictState) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = merge_conflict_state_path(&state.slug)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        let mut tmp = NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(tmp.as_file_mut(), state)?;
        tmp.as_file_mut().flush()?;
        tmp.persist(&path)?;
    } else {
        return Err("unable to determine merge-conflict metadata directory".into());
    }

    Ok(path)
}

fn read_conflict_state(
    slug: &str,
) -> Result<Option<MergeConflictState>, Box<dyn std::error::Error>> {
    let path = merge_conflict_state_path(slug)?;
    if !path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(&path)?;
    let state = serde_json::from_str::<MergeConflictState>(&contents)?;
    Ok(Some(state))
}

fn clear_conflict_state(slug: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = merge_conflict_state_path(slug)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

struct ReviewExecution {
    assume_yes: bool,
    review_only: bool,
    skip_checks: bool,
}

struct ReviewOutcome {
    critique_label: &'static str,
    session_path: Option<String>,
    checks_passed: usize,
    checks_total: usize,
    diff_command: String,
    branch_mutated: bool,
    fix_commit: Option<String>,
}

struct PlanApplyResult {
    commit_oid: Option<String>,
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

    fn to_context(&self) -> codex::ReviewCheckContext {
        codex::ReviewCheckContext {
            command: self.command.clone(),
            status_code: self.status_code,
            success: self.success,
            duration_ms: self.duration.as_millis(),
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
        }
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

    let prompt = codex::build_review_prompt(
        agent.scope,
        &spec.slug,
        &spec.branch,
        &spec.target_branch,
        &plan_document,
        &diff_summary,
        &check_contexts,
        agent.codex.bounds_prompt_path.as_deref(),
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
        agent,
        Some(config::PromptKind::Base),
        prompt,
        user_message,
        Vec::new(),
        auditor::RequestStream::Status {
            text: text_tx,
            events: Some(event_tx),
        },
        Some(codex::CodexModel::Gpt5),
        Some(worktree_path.to_path_buf()),
    )
    .await?;
    if let Some(handle) = progress_handle {
        let _ = handle.await;
    }

    let audit_result = Auditor::finalize(audit_disposition(commit_mode)).await?;
    let session_path = audit_result.session_display();
    let (narrative_paths, narrative_summary) = narrative_change_set(&audit_result);

    let critique_text = response.content.trim().to_string();
    emit_review_critique(&spec.slug, &critique_text);
    plan::set_plan_status(plan_path, "review-ready", Some("reviewed_at"))?;

    if commit_mode.should_commit() {
        let mut summary = format!(
            "Recorded Codex critique for plan {} (checks {}/{} passed).",
            spec.slug, checks_passed, checks_total
        );
        summary.push_str(&format!("\nDiff command: {}", spec.diff_command()));

        let mut builder = CommitMessageBuilder::new(summary);
        builder
            .set_header(CommitMessageType::NarrativeChange)
            .with_session_log_path(session_path.clone())
            .with_narrative_summary(narrative_summary.clone())
            .with_author_note(format!(
                "Review critique streamed to terminal; session: {}",
                session_path.as_deref().unwrap_or("<session unavailable>")
            ));
        let commit_message = builder.build();
        stage_narrative_paths(&narrative_paths)?;
        vcs::stage(Some(vec!["."]))?;
        trim_staged_vizier_paths(&narrative_paths)?;
        let _review_commit = vcs::commit_staged(&commit_message, false)?;
        clear_narrative_tracker(&narrative_paths);
    } else {
        let session_hint = session_path
            .as_deref()
            .unwrap_or("<session unavailable>")
            .to_string();
        display::info(format!(
            "Review critique not committed (--no-commit); consult the terminal output or session log {} before committing manually.",
            session_hint
        ));
        if !narrative_paths.is_empty() {
            display::info("Review critique artifacts held for manual review (--no-commit active).");
        }
    }

    let mut fix_commit: Option<String> = None;
    let diff_command = spec.diff_command();

    if exec.review_only {
        display::info("Review-only mode: skipped automatic fix prompt.");
    } else {
        let mut apply_fixes = exec.assume_yes;
        if !exec.assume_yes {
            apply_fixes = prompt_for_confirmation(&format!(
                "Apply suggested fixes on {}? [y/N] ",
                spec.branch
            ))?;
        }

        if apply_fixes {
            plan::set_plan_status(plan_path, "review-fixes-in-progress", None)?;
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
                    plan::set_plan_status(
                        plan_path,
                        "review-addressed",
                        Some("review_addressed_at"),
                    )?;
                }
                None => {
                    display::info("Codex reported no changes while addressing review feedback.");
                    plan::set_plan_status(plan_path, "review-ready", None)?;
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
        branch_mutated: true,
        fix_commit,
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

fn clip_log(bytes: &[u8]) -> String {
    const LIMIT: usize = 8_192;
    if bytes.is_empty() {
        return String::new();
    }
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= LIMIT {
        text.to_string()
    } else {
        let mut clipped = text[..LIMIT].to_string();
        clipped.push_str("\n… output truncated …");
        clipped
    }
}

fn log_cicd_stream(label: &str, content: &str) {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return;
    }
    let snippet: String = trimmed
        .lines()
        .take(12)
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    display::warn(format!("  {label}:\n{snippet}"));
}

fn run_cicd_script(
    script: &Path,
    repo_root: &Path,
) -> Result<CicdScriptResult, Box<dyn std::error::Error>> {
    let start = Instant::now();
    let output = Command::new("sh")
        .arg(script)
        .current_dir(repo_root)
        .output()
        .map_err(|err| format!("failed to run CI/CD script {}: {err}", script.display()))?;
    Ok(CicdScriptResult {
        status: output.status,
        duration: start.elapsed(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

fn log_cicd_result(script: &Path, result: &CicdScriptResult, attempt: u32) {
    let label = if result.success() { "passed" } else { "failed" };
    let status = result.status_label();
    let duration = format!("{:.2}s", result.duration.as_secs_f64());
    let message = format!(
        "CI/CD gate `{}` {label} ({status}; {duration}) [attempt {attempt}]",
        script.display()
    );
    if result.success() {
        display::info(message);
    } else {
        display::warn(message);
        log_cicd_stream("stdout", &result.stdout);
        log_cicd_stream("stderr", &result.stderr);
    }
}

fn cicd_gate_failure_error(script: &Path, result: &CicdScriptResult) -> Box<dyn std::error::Error> {
    let status = result.status_label();
    let message = format!(
        "CI/CD gate `{}` failed ({status}); inspect the output above and rerun `vizier merge` once resolved.",
        script.display()
    );
    Box::<dyn std::error::Error>::from(message)
}

fn collect_diff_summary(
    spec: &plan::PlanBranchSpec,
    worktree_path: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let range = format!("{}...HEAD", spec.target_branch);
    let stat = run_git_capture(
        worktree_path,
        &["--no-pager", "diff", "--stat=2000", &range],
    )
    .unwrap_or_else(|err| format!("Unable to compute diff stats: {err}"));
    let names = run_git_capture(
        worktree_path,
        &["--no-pager", "diff", "--name-status", &range],
    )
    .unwrap_or_else(|err| format!("Unable to list changed files: {err}"));

    Ok(format!(
        "Diff command: {}\n\n{}\n\n{}",
        spec.diff_command(),
        stat.trim(),
        names.trim()
    ))
}

fn run_git_capture(
    worktree_path: &Path,
    args: &[&str],
) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(worktree_path)
        .output()?;
    if !output.status.success() {
        return Err(format!("git {:?} exited with {:?}", args, output.status.code()).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn emit_review_critique(plan_slug: &str, critique: &str) {
    println!("--- Review critique for plan {plan_slug} ---");
    if critique.trim().is_empty() {
        println!("(Codex returned an empty critique.)");
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
        "<note>Update `.vizier/.snapshot` and TODO threads when behavior changes.</note>",
    );

    let system_prompt = codex::build_prompt_for_codex(
        agent.scope,
        &instruction,
        agent.codex.bounds_prompt_path.as_deref(),
    )
    .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;

    let (event_tx, event_rx) = mpsc::channel(64);
    let progress_handle = spawn_plain_progress_logger(event_rx);
    let (text_tx, _text_rx) = mpsc::channel(1);
    let response = Auditor::llm_request_with_tools_no_display(
        agent,
        Some(config::PromptKind::Base),
        system_prompt,
        instruction.clone(),
        tools::active_tooling_for(agent),
        auditor::RequestStream::Status {
            text: text_tx,
            events: Some(event_tx),
        },
        Some(codex::CodexModel::Gpt5Codex),
        Some(worktree_path.to_path_buf()),
    )
    .await?;
    if let Some(handle) = progress_handle {
        let _ = handle.await;
    }

    let audit_result = Auditor::finalize(audit_disposition(commit_mode)).await?;
    let session_path = audit_result.session_display();
    let (narrative_paths, narrative_summary) = narrative_change_set(&audit_result);

    let diff = vcs::get_diff(".", Some("HEAD"), None)?;
    if diff.trim().is_empty() {
        display::info("Codex reported no file modifications during fix-up.");
        return Ok(None);
    }

    if commit_mode.should_commit() {
        stage_narrative_paths(&narrative_paths)?;
        vcs::stage(Some(vec!["."]))?;
        trim_staged_vizier_paths(&narrative_paths)?;
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
            .with_session_log_path(session_path.clone())
            .with_narrative_summary(narrative_summary.clone())
            .with_author_note(format!(
                "Review critique streamed to terminal; session: {}",
                session_path.as_deref().unwrap_or("<session unavailable>")
            ));
        let commit_message = builder.build();
        let commit_oid = vcs::commit_staged(&commit_message, false)?;
        clear_narrative_tracker(&narrative_paths);
        Ok(Some(commit_oid.to_string()))
    } else {
        display::info("Fixes left pending; commit manually once satisfied.");
        if !narrative_paths.is_empty() {
            display::info("Narrative updates left pending (--no-commit active).");
        }
        Ok(None)
    }
}

fn prompt_for_confirmation(prompt: &str) -> Result<bool, Box<dyn std::error::Error>> {
    use std::io::{self, Write};

    print!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let normalized = input.trim().to_ascii_lowercase();
    Ok(matches!(normalized.as_str(), "y" | "yes"))
}

fn list_pending_plans(target_override: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let entries = plan::PlanSlugInventory::collect(target_override.as_deref())?;
    if entries.is_empty() {
        println!("No pending draft branches");
        return Ok(());
    }

    for entry in entries {
        let summary = entry.summary.replace('"', "'");
        let status = entry.status.as_deref().unwrap_or("unknown");
        let reviewed = entry
            .reviewed_at
            .as_ref()
            .map(|value| format!(" reviewed_at={}", value))
            .unwrap_or_else(|| "".to_string());
        println!(
            "plan={} branch={} created={} status={}{} summary=\"{}\"",
            entry.slug, entry.branch, entry.created_at, status, reviewed, summary
        );
    }

    Ok(())
}

async fn apply_plan_in_worktree(
    spec: &plan::PlanBranchSpec,
    plan_meta: &plan::PlanMetadata,
    worktree_path: &Path,
    plan_path: &Path,
    push_after: bool,
    commit_mode: CommitMode,
    agent: &config::AgentSettings,
) -> Result<PlanApplyResult, Box<dyn std::error::Error>> {
    let _cwd = WorkdirGuard::enter(worktree_path)?;

    let plan_rel = spec.plan_rel_path();
    let mut instruction = format!(
        "<instruction>Read the implementation plan at {} and implement its Execution Plan on this branch. Apply the listed steps, update `.vizier/.snapshot` plus TODO threads as needed, and stage the resulting edits for commit.</instruction>",
        plan_rel.display()
    );
    instruction.push_str(&format!(
        "<planSummary>{}</planSummary>",
        plan::summarize_spec(plan_meta)
    ));

    let system_prompt = codex::build_prompt_for_codex(
        agent.scope,
        &instruction,
        agent.codex.bounds_prompt_path.as_deref(),
    )
    .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;

    let (event_tx, event_rx) = mpsc::channel(64);
    let progress_handle = spawn_plain_progress_logger(event_rx);
    let (text_tx, _text_rx) = mpsc::channel(1);
    let response = Auditor::llm_request_with_tools_no_display(
        agent,
        None,
        system_prompt,
        instruction.clone(),
        tools::active_tooling_for(agent),
        auditor::RequestStream::Status {
            text: text_tx,
            events: Some(event_tx),
        },
        Some(codex::CodexModel::Gpt5Codex),
        Some(worktree_path.to_path_buf()),
    )
    .await?;
    if let Some(handle) = progress_handle {
        let _ = handle.await;
    }

    let audit_result = Auditor::finalize(audit_disposition(commit_mode)).await?;
    let session_path = audit_result.session_display();
    let (narrative_paths, narrative_summary) = narrative_change_set(&audit_result);

    let diff = vcs::get_diff(".", Some("HEAD"), None)?;
    if diff.trim().is_empty() {
        return Err("Codex completed without modifying files; nothing new to approve.".into());
    }

    plan::set_plan_status(plan_path, "implemented", Some("implemented_at"))?;

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
        stage_narrative_paths(&narrative_paths)?;
        vcs::stage(Some(vec!["."]))?;
        trim_staged_vizier_paths(&narrative_paths)?;
        let commit_message = builder.build();
        let oid = vcs::commit_staged(&commit_message, false)?;
        clear_narrative_tracker(&narrative_paths);
        commit_oid = Some(oid.to_string());

        if push_after {
            push_origin_if_requested(true)?;
        }
    } else if push_after {
        display::info("Push skipped because --no-commit left changes pending.");
    } else if !narrative_paths.is_empty() {
        display::info(
            "Narrative assets updated with --no-commit; changes left pending in the plan worktree.",
        );
    }

    Ok(PlanApplyResult { commit_oid })
}

async fn refresh_plan_branch(
    spec: &plan::PlanBranchSpec,
    plan_meta: &plan::PlanMetadata,
    worktree_path: &Path,
    push_after: bool,
    agent: &config::AgentSettings,
) -> Result<(), Box<dyn std::error::Error>> {
    let _cwd = WorkdirGuard::enter(worktree_path)?;

    let instruction = build_save_instruction(None);
    let system_prompt = if agent.backend == config::BackendKind::Codex {
        codex::build_prompt_for_codex(
            agent.scope,
            &instruction,
            agent.codex.bounds_prompt_path.as_deref(),
        )
        .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?
    } else {
        crate::config::get_system_prompt_with_meta(agent.scope, None)?
    };

    let response = Auditor::llm_request_with_tools(
        agent,
        Some(config::PromptKind::Base),
        system_prompt,
        instruction,
        tools::active_tooling_for(agent),
        None,
        Some(worktree_path.to_path_buf()),
    )
    .await?;
    let audit_result = Auditor::commit_audit().await?;
    let session_path = audit_result.session_display();
    let (narrative_paths, narrative_summary) = narrative_change_set(&audit_result);
    let mut allowed_paths = narrative_paths.clone();
    let plan_rel = spec.plan_rel_path();
    if !worktree_path.join(&plan_rel).exists() {
        allowed_paths.push(plan_rel.to_string_lossy().replace('\\', "/"));
    }

    let diff = vcs::get_diff(".", Some("HEAD"), None)?;
    if diff.trim().is_empty() {
        display::info(format!(
            "Plan {} already has up-to-date narrative assets; no refresh needed.",
            spec.slug
        ));
        if push_after {
            push_origin_if_requested(true)?;
        }
        return Ok(());
    }

    let mut summary = response.content.trim().to_string();
    if summary.is_empty() {
        summary = format!(
            "Refreshed narrative assets before merging plan {}.\nSpec summary: {}",
            spec.slug,
            plan::summarize_spec(plan_meta)
        );
    }

    let mut builder = CommitMessageBuilder::new(summary);
    builder
        .set_header(CommitMessageType::NarrativeChange)
        .with_session_log_path(session_path.clone())
        .with_narrative_summary(narrative_summary.clone());
    let commit_message = builder.build();

    stage_narrative_paths(&narrative_paths)?;
    vcs::stage(Some(vec!["."]))?;
    trim_staged_vizier_paths(&allowed_paths)?;
    let commit_oid = vcs::commit_staged(&commit_message, false)?;
    clear_narrative_tracker(&narrative_paths);

    if push_after {
        push_origin_if_requested(true)?;
    }

    display::info(format!(
        "Refreshed {} at {}; ready to merge plan {}",
        spec.branch, commit_oid, spec.slug
    ));

    Ok(())
}

fn current_branch_name(repo: &Repository) -> Result<Option<String>, git2::Error> {
    let head = repo.head()?;
    if head.is_branch() {
        Ok(head.shorthand().map(|s| s.to_string()))
    } else {
        Ok(None)
    }
}

fn build_merge_commit_message(
    spec: &plan::PlanBranchSpec,
    _meta: &plan::PlanMetadata,
    plan_document: Option<&str>,
    note: Option<&str>,
) -> String {
    // Merge commits now keep a concise subject line and embed the stored plan
    // document directly so reviewers see the same content Codex implemented.
    let mut sections: Vec<String> = Vec::new();

    if let Some(note_text) = note.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }) {
        sections.push(format!("Operator Note: {}", note_text));
    }

    let plan_block = plan_document
        .and_then(|document| {
            let trimmed = document.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .unwrap_or_else(|| format!("Implementation plan document unavailable for {}", spec.slug));

    sections.push(format!("Implementation Plan:\n{}", plan_block));

    format!(
        "feat: merge plan {}\n\n{}",
        spec.slug,
        sections.join("\n\n")
    )
}

struct WorkdirGuard {
    previous: PathBuf,
}

impl WorkdirGuard {
    fn enter(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let previous = std::env::current_dir()?;
        std::env::set_current_dir(path)?;
        Ok(Self { previous })
    }
}

impl Drop for WorkdirGuard {
    fn drop(&mut self) {
        if let Err(err) = std::env::set_current_dir(&self.previous) {
            display::debug(format!("failed to restore working directory: {err}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::build_merge_commit_message;
    use crate::plan::{PlanBranchSpec, PlanMetadata};

    fn sample_spec() -> PlanBranchSpec {
        PlanBranchSpec {
            slug: "merge-headers".to_string(),
            branch: "draft/merge-headers".to_string(),
            target_branch: "main".to_string(),
        }
    }

    fn sample_meta(spec: &PlanBranchSpec) -> PlanMetadata {
        PlanMetadata {
            slug: spec.slug.clone(),
            branch: spec.branch.clone(),
            status: Some("implemented".to_string()),
            created_at: None,
            created_at_raw: None,
            spec_source: Some("inline".to_string()),
            spec_excerpt: None,
            spec_summary: Some("Trim redundant headers".to_string()),
            reviewed_at: None,
        }
    }

    #[test]
    fn merge_commit_message_embeds_plan_document() {
        let spec = sample_spec();
        let meta = sample_meta(&spec);
        let plan_doc = r#"---
plan: merge-headers
branch: draft/merge-headers
status: implemented
created_at: 2025-11-15T00:00:00Z
spec_source: inline

## Operator Spec

Tidy merge message bodies.
"#;

        let message = build_merge_commit_message(&spec, &meta, Some(plan_doc), None);

        assert!(
            message.starts_with("feat: merge plan merge-headers\n\nImplementation Plan:\n---"),
            "Implementation Plan block missing: {message}"
        );
        assert!(
            !message.contains("\nPlan: merge-headers"),
            "old Plan header should not appear: {message}"
        );
        assert!(
            !message.contains("\nBranch: draft/merge-headers"),
            "old Branch header should not appear: {message}"
        );
        assert!(
            !message.contains("Spec source: inline"),
            "old Spec source header should not appear: {message}"
        );
    }

    #[test]
    fn merge_commit_message_handles_notes_and_missing_document() {
        let spec = sample_spec();
        let meta = sample_meta(&spec);
        let message =
            build_merge_commit_message(&spec, &meta, None, Some("  needs manual review  "));

        assert!(
            message.contains("Operator Note: needs manual review"),
            "note should be trimmed and rendered: {message}"
        );
        assert!(
            message.contains("Implementation plan document unavailable for merge-headers"),
            "missing plan placeholder should be present: {message}"
        );
    }
}
