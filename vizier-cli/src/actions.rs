use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use chrono::{SecondsFormat, Utc};
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
    tools, vcs,
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
        if report.known {
            display::info(format!(
                "Token usage: prompt={} (+{}) completion={} (+{}) total={} (+{})",
                report.prompt_total,
                report.prompt_delta,
                report.completion_total,
                report.completion_delta,
                report.total(),
                report.delta_total()
            ));
        } else {
            display::info("Token usage: unknown");
        }
        return;
    }

    let usage = Auditor::get_total_usage();
    if usage.known {
        display::info(format!(
            "Token usage: prompt={} completion={} total={}",
            usage.input_tokens,
            usage.output_tokens,
            usage.input_tokens + usage.output_tokens
        ));
    } else {
        display::info("Token usage: unknown");
    }
}

fn token_usage_suffix() -> String {
    if let Some(report) = Auditor::latest_usage_report() {
        return if report.known {
            format!(
                " (tokens: prompt={} [+{}] completion={} [+{}] total={} [+{}])",
                report.prompt_total,
                report.prompt_delta,
                report.completion_total,
                report.completion_delta,
                report.total(),
                report.delta_total()
            )
        } else {
            " (tokens: unknown)".to_string()
        };
    }

    let usage = Auditor::get_total_usage();
    if usage.known {
        format!(
            " (tokens: prompt={} completion={} total={})",
            usage.input_tokens,
            usage.output_tokens,
            usage.input_tokens + usage.output_tokens
        )
    } else {
        " (tokens: unknown)".to_string()
    }
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeConflictStrategy {
    Manual,
    Codex,
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
    push_after_commit: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match vcs::get_diff(".", Some(commit_ref), Some(exclude)) {
        Ok(diff) => match save(diff, commit_message, use_editor, push_after_commit).await {
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
    push_after_commit: bool,
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

    let system_prompt = if config::get_config().backend == config::BackendKind::Codex {
        codex::build_prompt_for_codex(&save_instruction)
            .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?
    } else {
        crate::config::get_system_prompt_with_meta(None)?
    };

    let response = Auditor::llm_request_with_tools(
        None,
        system_prompt,
        save_instruction,
        tools::active_tooling(),
        None,
        None,
    )
    .await?;

    let session_artifact = auditor::Auditor::commit_audit().await?;
    let session_display = session_artifact
        .as_ref()
        .map(|artifact| artifact.display_path());

    display::info(format!(
        "Assistant summary: {}{}",
        response.content.trim(),
        token_usage_suffix()
    ));
    print_token_usage();

    let post_tool_diff = vcs::get_diff(".", Some("HEAD"), Some(&[".vizier/"]))?;
    let has_code_changes = !post_tool_diff.trim().is_empty();
    let mut code_commit = None;

    if has_code_changes {
        let commit_body = Auditor::llm_request(
            config::get_config().get_prompt(config::SystemPrompt::Commit),
            post_tool_diff.clone(),
        )
        .await?
        .content;

        let mut message_builder = CommitMessageBuilder::new(commit_body);
        message_builder
            .set_header(CommitMessageType::CodeChange)
            .with_session_log_path(session_display.clone());

        if let Some(note) = provided_note.as_ref() {
            message_builder.with_author_note(note.clone());
        }

        let commit_message = message_builder.build();

        display::info("Committing remaining code changes...");
        let commit_oid = vcs::add_and_commit(None, &commit_message, false)?;
        display::info(format!(
            "Changes committed with message: {}",
            commit_message
        ));

        code_commit = Some(commit_oid.to_string());
    } else {
        if provided_note.is_some() {
            display::info(
                "Author note provided but no code changes detected; skipping code commit.",
            );
        } else {
            display::info("No code changes detected; skipping code commit.");
        }
    }

    push_origin_if_requested(push_after_commit)?;

    Ok(SaveOutcome {
        session_log: session_display,
        code_commit,
        pushed: push_after_commit,
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
pub async fn inline_command(
    user_message: String,
    push_after_commit: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let system_prompt = if config::get_config().backend == config::BackendKind::Codex {
        match codex::build_prompt_for_codex(&user_message) {
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
        match crate::config::get_system_prompt_with_meta(None) {
            Ok(s) => s,
            Err(e) => {
                display::emit(LogLevel::Error, format!("Error loading system prompt: {e}"));
                return Err(Box::<dyn std::error::Error>::from(e));
            }
        }
    };

    let response = match Auditor::llm_request_with_tools(
        None,
        system_prompt,
        user_message,
        tools::active_tooling(),
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

    if let Err(e) = auditor::Auditor::commit_audit().await {
        display::emit(LogLevel::Error, format!("Error committing audit: {e}"));
        return Err(Box::<dyn std::error::Error>::from(e));
    }

    push_origin_if_requested(push_after_commit)?;

    println!("{}", response.content.trim_end());
    print_token_usage();

    Ok(())
}

pub async fn run_draft(args: DraftArgs) -> Result<(), Box<dyn std::error::Error>> {
    if config::get_config().backend != config::BackendKind::Codex {
        return Err("vizier draft requires the Codex backend; rerun with --backend codex".into());
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

        let prompt = codex::build_implementation_plan_prompt(&slug, &branch_name, &spec_text)
            .map_err(|err| -> Box<dyn std::error::Error> {
                Box::from(format!("build_prompt: {err}"))
            })?;

        let llm_response = Auditor::llm_request_with_tools(
            None,
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

        Ok(())
    }
    .await;

    match plan_result {
        Ok(()) => {
            let plan_to_print = plan_document_preview
                .clone()
                .or_else(|| fs::read_to_string(&plan_in_worktree).ok());

            if worktree_created {
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

            display::info(format!(
                "View with: git checkout {branch_name} && $EDITOR {plan_display}"
            ));
            println!("Draft ready; plan={plan_display}; branch={branch_name}");
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

pub async fn run_approve(opts: ApproveOptions) -> Result<(), Box<dyn std::error::Error>> {
    if opts.list_only {
        display::warn("`vizier approve --list` is deprecated; use `vizier list` instead.");
        return list_pending_plans(opts.target.clone());
    }

    if config::get_config().backend != config::BackendKind::Codex {
        return Err("vizier approve requires the Codex backend; rerun with --backend codex".into());
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
    )
    .await;

    match approval {
        Ok(commit_oid) => {
            if let Some(tree) = worktree.take() {
                if let Err(err) = tree.cleanup() {
                    display::warn(format!(
                        "temporary worktree cleanup failed ({}); remove manually with `git worktree prune`",
                        err
                    ));
                }
            }

            println!(
                "Plan {} implemented on {}; latest commit={}; review with `{}`",
                spec.slug,
                spec.branch,
                commit_oid,
                spec.diff_command()
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

pub async fn run_review(opts: ReviewOptions) -> Result<(), Box<dyn std::error::Error>> {
    if config::get_config().backend != config::BackendKind::Codex {
        return Err("vizier review requires the Codex backend; rerun with --backend codex".into());
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
    )
    .await;

    match review_result {
        Ok(outcome) => {
            if let Some(tree) = worktree.take() {
                if let Err(err) = tree.cleanup() {
                    display::warn(format!(
                        "temporary worktree cleanup failed ({}); remove manually with `git worktree prune`",
                        err
                    ));
                }
            }

            if outcome.branch_mutated && opts.push_after {
                push_origin_if_requested(true)?;
            }

            if let Some(commit) = outcome.fix_commit.as_ref() {
                display::info(format!(
                    "Fixes addressing review feedback committed at {} on {}",
                    commit, spec.branch
                ));
            }

            println!(
                "Review complete; plan={} branch={} review={} checks={}/{} diff=\"{}\" session={}{}",
                spec.slug,
                spec.branch,
                outcome.review_rel,
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

pub async fn run_merge(opts: MergeOptions) -> Result<(), Box<dyn std::error::Error>> {
    let spec = plan::PlanBranchSpec::resolve(
        Some(opts.plan.as_str()),
        opts.branch_override.as_deref(),
        opts.target.as_deref(),
    )?;

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
            finalize_merge(
                &spec,
                merge_oid,
                source_oid,
                opts.delete_branch,
                opts.push_after,
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

    if let Err(err) = refresh_plan_branch(&spec, &plan_meta, &worktree_path, opts.push_after).await
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
            handle_merge_conflict(&spec, &merge_message, conflict, opts.conflict_strategy).await?
        }
    };

    finalize_merge(
        &spec,
        merge_oid,
        source_oid,
        opts.delete_branch,
        opts.push_after,
    )?;
    Ok(())
}

fn finalize_merge(
    spec: &plan::PlanBranchSpec,
    merge_oid: Oid,
    source_oid: Oid,
    delete_branch: bool,
    push_after: bool,
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
    println!(
        "Merged plan {} into {}; merge_commit={}",
        spec.slug, spec.target_branch, merge_oid
    );
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
            match try_auto_resolve_conflicts(spec, &state, &files).await {
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
) -> Result<(Oid, Oid), Box<dyn std::error::Error>> {
    display::info("Attempting to resolve conflicts with Codex...");
    let prompt = codex::build_merge_conflict_prompt(&spec.target_branch, &spec.branch, files)?;
    let repo_root = repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let cfg = config::get_config();
    let request = codex::CodexRequest {
        prompt,
        repo_root,
        profile: cfg.codex.profile.clone(),
        bin: cfg.codex.binary_path.clone(),
        extra_args: cfg.codex.extra_args.clone(),
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
    review_rel: String,
    session_path: Option<String>,
    checks_passed: usize,
    checks_total: usize,
    diff_command: String,
    branch_mutated: bool,
    fix_commit: Option<String>,
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
) -> Result<ReviewOutcome, Box<dyn std::error::Error>> {
    let _cwd = WorkdirGuard::enter(worktree_path)?;
    let review_rel = Path::new(".vizier")
        .join("reviews")
        .join(format!("{}.md", spec.slug));

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
        &spec.slug,
        &spec.branch,
        &spec.target_branch,
        &plan_document,
        &diff_summary,
        &check_contexts,
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
        None,
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

    let session_artifact = Auditor::commit_audit().await?;
    let session_path = session_artifact
        .as_ref()
        .map(|artifact| artifact.display_path());

    write_review_file(&review_rel, spec, response.content.trim())?;
    plan::set_plan_status(plan_path, "review-ready", Some("reviewed_at"))?;

    vcs::stage(Some(vec!["."]))?;

    let mut summary = format!(
        "Recorded Codex critique for plan {} (checks {}/{} passed).",
        spec.slug, checks_passed, checks_total
    );
    summary.push_str(&format!("\nDiff command: {}", spec.diff_command()));

    let mut builder = CommitMessageBuilder::new(summary);
    builder
        .set_header(CommitMessageType::NarrativeChange)
        .with_session_log_path(session_path.clone())
        .with_author_note(format!("Review file: {}", review_rel.to_string_lossy()));
    let commit_message = builder.build();
    let _review_commit = vcs::add_and_commit(None, &commit_message, false)?;

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
            match apply_review_fixes(spec, plan_meta, worktree_path, &review_rel).await? {
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
        review_rel: review_rel.to_string_lossy().to_string(),
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

fn write_review_file(
    rel_path: &Path,
    spec: &plan::PlanBranchSpec,
    critique: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let parent = rel_path
        .parent()
        .ok_or_else(|| "invalid review path: missing parent directory".to_string())?;
    fs::create_dir_all(parent)?;
    let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let mut document = String::new();
    document.push_str("---\n");
    document.push_str(&format!("plan: {}\n", spec.slug));
    document.push_str(&format!("branch: {}\n", spec.branch));
    document.push_str(&format!("target: {}\n", spec.target_branch));
    document.push_str(&format!("reviewed_at: {}\n", timestamp));
    document.push_str("reviewer: codex\n");
    document.push_str("---\n\n");
    if critique.trim().is_empty() {
        document.push_str("(Codex returned an empty critique.)\n");
    } else {
        document.push_str(critique.trim());
        document.push('\n');
    }
    fs::write(rel_path, document)?;
    Ok(())
}

async fn apply_review_fixes(
    spec: &plan::PlanBranchSpec,
    plan_meta: &plan::PlanMetadata,
    worktree_path: &Path,
    review_rel: &Path,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let plan_rel = spec.plan_rel_path();
    let mut instruction = format!(
        "<instruction>Read the implementation plan at {} and the review critique at {}. Address every Action Item without changing unrelated code.</instruction>",
        plan_rel.display(),
        review_rel.display()
    );
    instruction.push_str(&format!(
        "<planSummary>{}</planSummary>",
        plan::summarize_spec(plan_meta)
    ));
    instruction.push_str(
        "<note>Update `.vizier/.snapshot` and TODO threads when behavior changes.</note>",
    );

    let system_prompt = codex::build_prompt_for_codex(&instruction)
        .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;

    let (event_tx, event_rx) = mpsc::channel(64);
    let progress_handle = spawn_plain_progress_logger(event_rx);
    let (text_tx, _text_rx) = mpsc::channel(1);
    let response = Auditor::llm_request_with_tools_no_display(
        None,
        system_prompt,
        instruction.clone(),
        tools::active_tooling(),
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

    let session_artifact = Auditor::commit_audit().await?;
    let session_path = session_artifact
        .as_ref()
        .map(|artifact| artifact.display_path());

    let diff = vcs::get_diff(".", Some("HEAD"), None)?;
    if diff.trim().is_empty() {
        display::info("Codex reported no file modifications during fix-up.");
        return Ok(None);
    }

    vcs::stage(Some(vec!["."]))?;
    let mut summary = response.content.trim().to_string();
    if summary.is_empty() {
        summary = format!(
            "Addressed review feedback for plan {} using {}",
            spec.slug,
            review_rel.to_string_lossy()
        );
    }
    let mut builder = CommitMessageBuilder::new(summary);
    builder
        .set_header(CommitMessageType::CodeChange)
        .with_session_log_path(session_path.clone())
        .with_author_note(format!(
            "Review reference: {}",
            review_rel.to_string_lossy()
        ));
    let commit_message = builder.build();
    let commit_oid = vcs::add_and_commit(None, &commit_message, false)?;
    Ok(Some(commit_oid.to_string()))
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
) -> Result<String, Box<dyn std::error::Error>> {
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

    let system_prompt = codex::build_prompt_for_codex(&instruction)
        .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;

    let (event_tx, event_rx) = mpsc::channel(64);
    let progress_handle = spawn_plain_progress_logger(event_rx);
    let (text_tx, _text_rx) = mpsc::channel(1);
    let response = Auditor::llm_request_with_tools_no_display(
        None,
        system_prompt,
        instruction.clone(),
        tools::active_tooling(),
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

    let session_artifact = Auditor::commit_audit().await?;
    let session_path = session_artifact
        .as_ref()
        .map(|artifact| artifact.display_path());

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
        .with_session_log_path(session_path.clone());

    let commit_message = builder.build();
    vcs::stage(Some(vec!["."]))?;
    let commit_oid = vcs::add_and_commit(None, &commit_message, false)?;

    if push_after {
        push_origin_if_requested(true)?;
    }

    Ok(commit_oid.to_string())
}

async fn refresh_plan_branch(
    spec: &plan::PlanBranchSpec,
    plan_meta: &plan::PlanMetadata,
    worktree_path: &Path,
    push_after: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let _cwd = WorkdirGuard::enter(worktree_path)?;

    let instruction = build_save_instruction(None);
    let system_prompt = if config::get_config().backend == config::BackendKind::Codex {
        codex::build_prompt_for_codex(&instruction)
            .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?
    } else {
        crate::config::get_system_prompt_with_meta(None)?
    };

    let response = Auditor::llm_request_with_tools(
        None,
        system_prompt,
        instruction,
        tools::active_tooling(),
        None,
        Some(worktree_path.to_path_buf()),
    )
    .await?;
    let session_artifact = Auditor::commit_audit().await?;
    let session_path = session_artifact
        .as_ref()
        .map(|artifact| artifact.display_path());

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
        .with_session_log_path(session_path.clone());
    let commit_message = builder.build();

    vcs::stage(Some(vec!["."]))?;
    let commit_oid = vcs::add_and_commit(None, &commit_message, false)?;

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
