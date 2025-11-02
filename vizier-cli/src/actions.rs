use std::env;
use std::fs;
use std::process::Command;

use tempfile::{Builder, TempPath};

use vizier_core::vcs::{AttemptOutcome, CredentialAttempt, PushErrorKind, RemoteScheme};
use vizier_core::{
    auditor,
    auditor::{Auditor, CommitMessageBuilder, CommitMessageType},
    bootstrap,
    bootstrap::{BootstrapOptions, IssuesProvider},
    config,
    display::{self, LogLevel},
    file_tracking, prompting, tools, vcs,
};

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
    display::info(format!(
        "Token usage: prompt={} completion={}",
        usage.input_tokens, usage.output_tokens
    ));
}

#[derive(Debug, Clone)]
pub struct SnapshotInitOptions {
    pub force: bool,
    pub depth: Option<usize>,
    pub paths: Vec<String>,
    pub exclude: Vec<String>,
    pub issues: Option<String>,
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

    let response = Auditor::llm_request_with_tools(
        crate::config::get_system_prompt_with_meta(None)?,
        save_instruction,
        tools::get_tools(),
    )
    .await?;

    let conversation_hash = auditor::Auditor::commit_audit().await?;

    display::info(format!("Assistant summary: {}", response.content.trim()));
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
    let system_prompt = match crate::config::get_system_prompt_with_meta(None) {
        Ok(s) => s,
        Err(e) => {
            display::emit(LogLevel::Error, format!("Error loading system prompt: {e}"));
            return Err(Box::<dyn std::error::Error>::from(e));
        }
    };

    let response = match Auditor::llm_request_with_tools(
        system_prompt,
        user_message,
        tools::get_tools(),
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
