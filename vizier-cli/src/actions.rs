use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use git2::{BranchType, Repository};
use tempfile::{Builder, TempPath};

use vizier_core::vcs::{
    AttemptOutcome, CredentialAttempt, PushErrorKind, RemoteScheme, add_worktree_for_branch,
    commit_paths_in_repo, create_branch_from, delete_branch, detect_primary_branch,
    remove_worktree, repo_root,
};
use vizier_core::{
    auditor,
    auditor::{Auditor, CommitMessageBuilder, CommitMessageType},
    bootstrap,
    bootstrap::{BootstrapOptions, IssuesProvider},
    codex, config,
    display::{self, LogLevel},
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
    pub delete_branch: bool,
    pub note: Option<String>,
    pub push_after: bool,
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
    pub conversation_hash: Option<String>,
    pub code_commit: Option<String>,
    pub pushed: bool,
}

fn format_save_outcome(outcome: &SaveOutcome) -> String {
    let mut parts = vec!["Save complete".to_string()];

    match &outcome.conversation_hash {
        Some(hash) if !hash.is_empty() => parts.push(format!("conversation={}", short_hash(hash))),
        _ => parts.push("conversation=none".to_string()),
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

    let mut save_instruction =
        "<instruction>Update the snapshot and existing TODOs as needed</instruction>".to_string();

    if let Some(note) = &provided_note {
        save_instruction = format!(
            "{}<change_author_note>{}</change_author_note>",
            save_instruction, note
        );
    }

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
    )
    .await?;

    let conversation_hash = auditor::Auditor::commit_audit().await?;

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
            .with_conversation_hash(conversation_hash.clone());

        if let Some(note) = provided_note.as_ref() {
            message_builder.with_author_note(note.clone());
        }

        let mut commit_message = message_builder.build();

        if crate::config::get_config().commit_confirmation {
            if let Some(new_message) = vizier_core::editor::run_editor(&commit_message).await? {
                commit_message = new_message;
            }
        }

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
        conversation_hash: if conversation_hash.is_empty() {
            None
        } else {
            Some(conversation_hash)
        },
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

    let conversation_hash = Auditor::commit_audit().await?;

    push_origin_if_requested(push_after_commit)?;

    let conversation_summary = if conversation_hash.is_empty() {
        "none".to_string()
    } else {
        short_hash(&conversation_hash)
    };

    println!(
        "Clean complete; revised={}; removed={}; conversation={}",
        revised, removed, conversation_summary
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

        let llm_response =
            Auditor::llm_request_with_tools(None, prompt, spec_text.clone(), Vec::new())
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

        Auditor::commit_audit()
            .await
            .map_err(|err| Box::<dyn std::error::Error>::from(format!("commit_audit: {err}")))?;
        Ok(())
    }
    .await;

    match plan_result {
        Ok(()) => {
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

pub fn run_approve(opts: ApproveOptions) -> Result<(), Box<dyn std::error::Error>> {
    if opts.list_only {
        return list_pending_plans(opts.target.clone());
    }

    let plan_name = opts
        .plan
        .as_deref()
        .ok_or_else(|| "plan name is required unless --list is set")?;
    let slug = plan::sanitize_name_override(plan_name)?;
    let source_branch = opts
        .branch_override
        .clone()
        .unwrap_or_else(|| plan::default_branch_for_slug(&slug));
    let target_branch = resolve_target_branch(opts.target.clone())?;

    vcs::ensure_clean_worktree().map_err(|err| {
        Box::<dyn std::error::Error>::from(format!(
            "clean working tree required before approval: {err}"
        ))
    })?;

    let mut repo = Repository::discover(".")?;
    let current_branch = current_branch_name(&repo)?;
    if current_branch.as_deref() != Some(target_branch.as_str()) {
        display::info(format!("Checking out {target_branch} before approval..."));
        vcs::checkout_branch(&target_branch)?;
        repo = Repository::discover(".")?;
    }

    let source = repo
        .find_branch(&source_branch, BranchType::Local)
        .map_err(|_| {
            format!("draft branch {source_branch} not found; rerun vizier draft or pass --branch")
        })?;
    let source_commit = source.get().peel_to_commit()?;
    let source_oid = source_commit.id();

    let target_ref = repo
        .find_branch(&target_branch, BranchType::Local)
        .map_err(|_| format!("target branch {target_branch} not found"))?;
    let target_commit = target_ref.into_reference().peel_to_commit()?;
    let target_oid = target_commit.id();

    if repo.graph_descendant_of(target_oid, source_oid)? {
        println!(
            "Plan {slug} already merged into {target_branch}; latest commit={}",
            source_oid
        );
        return Ok(());
    }

    if !repo.graph_descendant_of(source_oid, target_oid)? {
        display::warn(format!(
            "{source_branch} does not include the latest {target_branch} commits; merge may require manual resolution."
        ));
    }

    let plan_meta = plan::load_plan_from_branch(&slug, &source_branch)?;
    if plan_meta.branch != source_branch {
        display::warn(format!(
            "Plan metadata references branch {} but --branch resolved to {source_branch}",
            plan_meta.branch
        ));
    }

    if !opts.assume_yes {
        show_plan_preview(&plan_meta, &source_branch);
        if !prompt_for_confirmation("Proceed with merge? [y/N] ")? {
            println!("Approval cancelled; no changes were made.");
            return Ok(());
        }
    }

    let commit_message = build_approve_commit_message(
        &plan_meta,
        plan_meta.slug.as_str(),
        &source_branch,
        opts.note.as_deref(),
    );
    let merge_oid = vcs::merge_branch_no_ff(&source_branch, &commit_message)?;

    if opts.delete_branch {
        let repo = Repository::discover(".")?;
        if repo.graph_descendant_of(merge_oid, source_oid)? {
            vcs::delete_branch(&source_branch)?;
            display::info(format!("Deleted {source_branch} after approval"));
        } else {
            display::warn(format!(
                "Skipping deletion of {source_branch}; merge commit did not include the branch tip."
            ));
        }
    }

    push_origin_if_requested(opts.push_after)?;
    println!("Approved plan; merged {source_branch} into {target_branch}; commit={merge_oid}");
    Ok(())
}

fn show_plan_preview(meta: &plan::PlanMetadata, branch: &str) {
    println!("Plan: {}", meta.slug);
    println!("Branch: {}", branch);
    println!("Created: {}", meta.created_at_display());
    println!(
        "Spec source: {}",
        meta.spec_source
            .clone()
            .unwrap_or_else(|| "unknown".to_string())
    );
    println!("Spec summary: {}", plan::summarize_spec(meta));
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

fn current_branch_name(repo: &Repository) -> Result<Option<String>, git2::Error> {
    let head = repo.head()?;
    if head.is_branch() {
        Ok(head.shorthand().map(|s| s.to_string()))
    } else {
        Ok(None)
    }
}

fn build_approve_commit_message(
    meta: &plan::PlanMetadata,
    slug: &str,
    source_branch: &str,
    note: Option<&str>,
) -> String {
    let plan_doc = plan::plan_rel_path(slug);
    let mut body = format!(
        "Plan: {slug}\nBranch: {source_branch}\nPlan doc: {}\nStatus: {}\nSpec source: {}\nCreated: {}\nSummary: {}",
        plan_doc.display(),
        meta.status.clone().unwrap_or_else(|| "draft".to_string()),
        meta.spec_source
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        meta.created_at_display(),
        plan::summarize_spec(meta)
    );

    if let Some(note_text) = note.filter(|value| !value.trim().is_empty()) {
        body.push_str(&format!("\nNotes: {}", note_text));
    }

    format!("feat: approve plan {slug}\n\n{body}")
}
