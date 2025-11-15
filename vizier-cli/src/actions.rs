use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    display::{self, LogLevel, Verbosity},
    file_tracking, prompting, tools, vcs,
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
    let usage = Auditor::get_total_usage();
    if usage.known {
        display::info(format!(
            "Token usage: prompt={} completion={}",
            usage.input_tokens, usage.output_tokens
        ));
    } else {
        display::info("Token usage: unknown");
    }
}

fn token_usage_suffix() -> String {
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

fn spawn_plain_progress_logger(mut rx: mpsc::Receiver<String>) -> Option<JoinHandle<()>> {
    if matches!(display::get_display_config().verbosity, Verbosity::Quiet) {
        return None;
    }

    Some(tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            eprintln!("[codex] {trimmed}");
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
pub struct ApproveOptions {
    pub plan: Option<String>,
    pub list_only: bool,
    pub target: Option<String>,
    pub branch_override: Option<String>,
    pub assume_yes: bool,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeConflictStrategy {
    Manual,
    Codex,
}

pub async fn docs_prompt(cmd: crate::DocsPromptCmd) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;

    let crate::DocsPromptCmd {
        scope: cli_scope,
        write,
        scaffold,
        force,
    } = cmd;

    let scope = match cli_scope {
        crate::DocsPromptScope::ArchitectureOverview => {
            prompting::PromptScope::ArchitectureOverview
        }
        crate::DocsPromptScope::SubsystemDetail => prompting::PromptScope::SubsystemDetail,
        crate::DocsPromptScope::InterfaceSummary => prompting::PromptScope::InterfaceSummary,
        crate::DocsPromptScope::InvariantCapture => prompting::PromptScope::InvariantCapture,
        crate::DocsPromptScope::OperationalThread => prompting::PromptScope::OperationalThread,
    };

    if scaffold {
        prompting::ensure_prompt_directory()?;
        let destination = prompting::prompt_directory().join(scope.file_name());

        if destination.exists() && !force {
            return Err(format!(
                "prompt already exists at {}; pass --force to overwrite",
                destination.display()
            )
            .into());
        }

        std::fs::write(&destination, scope.default_template())?;
        display::info(format!(
            "{} prompt scaffolded at {}",
            scope.title(),
            destination.display()
        ));
        return Ok(());
    }

    if let Some(write_path) = write {
        if write_path.as_os_str() == "-" {
            let template = prompting::load_prompt(scope)?;
            let mut stdout = std::io::stdout();
            stdout.write_all(template.as_bytes())?;
            if !template.ends_with('\n') {
                stdout.write_all(b"\n")?;
            }
            stdout.flush()?;
            return Ok(());
        }

        let mut destination = write_path;
        if destination.is_dir() {
            destination = destination.join(scope.file_name());
        }

        if destination.exists() && !force {
            return Err(format!(
                "{} already exists; pass --force to overwrite",
                destination.display()
            )
            .into());
        }

        if let Some(parent) = destination.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        let template = prompting::load_prompt(scope)?;
        std::fs::write(&destination, template)?;
        display::info(format!(
            "{} prompt written to {}",
            scope.title(),
            destination.display()
        ));
        return Ok(());
    }

    let template = prompting::load_prompt(scope)?;
    let mut stdout = std::io::stdout();
    stdout.write_all(template.as_bytes())?;
    if !template.ends_with('\n') {
        stdout.write_all(b"\n")?;
    }
    stdout.flush()?;
    Ok(())
}

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
pub async fn clean(
    todo_list: String,
    push_after_commit: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let todo_dir = tools::get_todo_dir();
    let available = std::fs::read_dir(&todo_dir)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let file_type = entry.file_type().ok()?;

            if !file_type.is_file() {
                return None;
            }

            let name = entry.file_name().into_string().ok()?;
            if name.starts_with('.') {
                return None;
            }

            Some(name)
        })
        .collect::<Vec<_>>();

    let targets = match todo_list.as_str() {
        "*" => available
            .iter()
            .map(|name| format!("{}{}", todo_dir, name))
            .collect::<Vec<_>>(),
        _ => {
            let requested: std::collections::HashSet<_> = todo_list
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            let available_set: std::collections::HashSet<_> = available.iter().cloned().collect();

            requested
                .intersection(&available_set)
                .map(|name| format!("{}{}", todo_dir, name))
                .collect()
        }
    };

    let mut revised = 0;
    let mut removed = 0;

    for target in targets.iter() {
        display::info(format!("Cleaning {}...", target));
        let content = std::fs::read_to_string(target)?;
        let response = Auditor::llm_request(
            format!(
                "{}{}<snapshot>{}</snapshot>",
                vizier_core::REVISE_TODO_PROMPT,
                config::get_config()
                    .get_prompt(config::SystemPrompt::Base)
                    .replace("mainInstruction", "SYSTEM_PROMPT_BASE"),
                tools::read_snapshot()
            ),
            content.clone(),
        )
        .await?
        .content;

        let revised_content = match response.as_str() {
            "null" => Some(content.clone()),
            "delete" => None,
            _ => Some(response.clone()),
        };

        match revised_content {
            Some(rc) => {
                if response != "null" {
                    display::info(format!("Revising {}...", target));
                    file_tracking::FileTracker::write(target, &rc)?;
                    revised += 1;
                }
            }
            None => {
                display::info(format!("Removing {}...", target));
                file_tracking::FileTracker::delete(target)?;
                removed += 1
            }
        };
    }

    display::info(format!("Revised {} TODO items", revised));
    display::info(format!("Removed {} TODO items", removed));

    let session_artifact = Auditor::commit_audit().await?;

    push_origin_if_requested(push_after_commit)?;

    let session_summary = session_artifact
        .as_ref()
        .map(|artifact| artifact.display_path())
        .unwrap_or_else(|| "none".to_string());

    println!(
        "Clean complete; revised={}; removed={}; session={}",
        revised, removed, session_summary
    );

    print_token_usage();

    Ok(())
}

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
            None,
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

pub async fn run_approve(opts: ApproveOptions) -> Result<(), Box<dyn std::error::Error>> {
    if opts.list_only {
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

pub async fn run_merge(opts: MergeOptions) -> Result<(), Box<dyn std::error::Error>> {
    vcs::ensure_clean_worktree().map_err(|err| {
        Box::<dyn std::error::Error>::from(format!(
            "clean working tree required before merge: {err}"
        ))
    })?;

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

    let plan_meta = spec.load_metadata()?;

    if let Some((merge_oid, source_oid)) = try_complete_pending_merge(&spec)? {
        finalize_merge(
            &spec,
            merge_oid,
            source_oid,
            opts.delete_branch,
            opts.push_after,
        )?;
        return Ok(());
    }

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
) -> Result<Option<(Oid, Oid)>, Box<dyn std::error::Error>> {
    let Some(state) = read_conflict_state(&spec.slug)? else {
        return Ok(None);
    };

    if state.source_branch != spec.branch {
        display::warn(format!(
            "Found stale merge-conflict metadata for {}; stored branch {}!=requested {}. Cleaning it up.",
            spec.slug, state.source_branch, spec.branch
        ));
        let _ = clear_conflict_state(&spec.slug);
        return Ok(None);
    }

    if state.target_branch != spec.target_branch {
        display::warn(format!(
            "Found stale merge-conflict metadata for {}; stored target {}!=requested {}. Cleaning it up.",
            spec.slug, state.target_branch, spec.target_branch
        ));
        let _ = clear_conflict_state(&spec.slug);
        return Ok(None);
    }

    let repo = Repository::discover(".")?;
    let current_branch = current_branch_name(&repo)?;
    if current_branch.as_deref() != Some(spec.target_branch.as_str()) {
        return Err(format!(
            "merge for plan {} is still in progress; checkout {} to continue resolution",
            spec.slug, spec.target_branch
        )
        .into());
    }

    if repo.state() != RepositoryState::Merge {
        display::warn(format!(
            "Merge metadata for plan {} exists but the repository is no longer in a merge state; assuming it was aborted.",
            spec.slug
        ));
        let _ = clear_conflict_state(&spec.slug);
        return Ok(None);
    }

    let outstanding = list_conflicted_paths()?;
    if !outstanding.is_empty() {
        display::warn("Merge conflicts remain:");
        for path in outstanding {
            display::warn(format!("  - {path}"));
        }
        display::info(format!(
            "Resolve the conflicts above, stage the files, then rerun `vizier merge {}`.",
            spec.slug
        ));
        return Err("merge blocked until conflicts are resolved".into());
    }

    let head_oid = Oid::from_str(&state.head_oid)?;
    let source_oid = Oid::from_str(&state.source_oid)?;
    let merge_oid = commit_in_progress_merge(&state.merge_message, head_oid, source_oid)?;
    let _ = clear_conflict_state(&spec.slug);
    display::info("Conflicts resolved; finalizing merge now.");
    Ok(Some((merge_oid, source_oid)))
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
        "Resolve the conflicts, stage the results, then rerun `vizier merge {slug}` to finish the merge."
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
        output_mode: codex::CodexOutputMode::PassthroughHuman,
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

fn prompt_for_confirmation(prompt: &str) -> Result<bool, Box<dyn std::error::Error>> {
    use std::io::{self, Write};

    print!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let normalized = input.trim().to_ascii_lowercase();
    Ok(matches!(normalized.as_str(), "y" | "yes"))
}

fn resolve_target_branch(
    target_override: Option<String>,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(target) = target_override {
        return Ok(target);
    }
    detect_primary_branch().ok_or_else(|| "unable to detect primary branch; use --target".into())
}

fn list_pending_plans(target_override: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let target_branch = resolve_target_branch(target_override)?;
    let repo = Repository::discover(".")?;
    let target_ref = repo
        .find_branch(&target_branch, BranchType::Local)
        .map_err(|_| format!("target branch {target_branch} not found"))?;
    let target_commit = target_ref.into_reference().peel_to_commit()?;
    let target_oid = target_commit.id();

    let mut branches = repo.branches(Some(BranchType::Local))?;
    let mut pending = 0usize;
    while let Some(branch_res) = branches.next() {
        let (branch, _) = branch_res?;
        let Some(name) = branch.name()? else {
            continue;
        };
        if !name.starts_with("draft/") {
            continue;
        }
        let commit = branch.get().peel_to_commit()?;
        if repo.graph_descendant_of(target_oid, commit.id())? {
            continue;
        }

        let slug = name.trim_start_matches("draft/").to_string();
        match plan::load_plan_from_branch(&slug, name) {
            Ok(meta) => {
                let summary = plan::summarize_spec(&meta).replace('"', "'");
                println!(
                    "plan={} branch={} created={} summary=\"{}\"",
                    meta.slug,
                    name,
                    meta.created_at_display(),
                    summary
                );
                pending += 1;
            }
            Err(err) => {
                display::warn(format!("Failed to load plan metadata for {name}: {err}"));
            }
        }
    }

    if pending == 0 {
        println!("No pending draft branches");
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

    let response = Auditor::llm_request_with_tools_no_display(
        None,
        system_prompt,
        instruction.clone(),
        tools::active_tooling(),
        auditor::RequestStream::PassthroughStderr,
        Some(codex::CodexModel::Gpt5Codex),
        Some(worktree_path.to_path_buf()),
    )
    .await?;

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
