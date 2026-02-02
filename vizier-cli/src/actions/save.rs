use std::collections::HashSet;
use std::env;
use std::fs;
use std::process::Command;

use tempfile::{Builder, TempPath};

use vizier_core::{
    agent_prompt, auditor,
    auditor::{Auditor, CommitMessageBuilder, CommitMessageType},
    config,
    display::{self, LogLevel, Verbosity},
    file_tracking, vcs,
};

use super::shared::{
    WorkdirGuard, append_agent_rows, audit_disposition, current_verbosity, format_block,
    push_origin_if_requested, short_hash,
};
use super::types::CommitMode;

pub(crate) async fn run_save(
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
                println!("{}", format_save_outcome(&outcome, current_verbosity()));
                Ok(())
            }
            Err(e) => {
                display::emit(LogLevel::Error, format!("Error running --save: {e}"));
                Err(e)
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

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_save_in_worktree(
    commit_ref: &str,
    exclude: &[&str],
    commit_message: Option<String>,
    use_editor: bool,
    commit_mode: CommitMode,
    push_after_commit: bool,
    agent: &config::AgentSettings,
    worktree_path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let _guard = WorkdirGuard::enter(worktree_path)?;
    run_save(
        commit_ref,
        exclude,
        commit_message,
        use_editor,
        commit_mode,
        push_after_commit,
        agent,
    )
    .await
}

#[derive(Debug)]
struct SaveOutcome {
    session_log: Option<String>,
    code_commit: Option<String>,
    pushed: bool,
    audit_state: auditor::AuditState,
    commit_mode: CommitMode,
}

fn format_save_outcome(outcome: &SaveOutcome, verbosity: Verbosity) -> String {
    let mut rows = vec![("Outcome".to_string(), "Save complete".to_string())];

    match &outcome.session_log {
        Some(path) if !path.is_empty() => rows.push(("Session".to_string(), path.clone())),
        _ => rows.push(("Session".to_string(), "none".to_string())),
    }

    match &outcome.code_commit {
        Some(hash) if !hash.is_empty() => {
            rows.push(("Code commit".to_string(), short_hash(hash)));
        }
        _ => rows.push(("Code commit".to_string(), "none".to_string())),
    }

    rows.push(("Mode".to_string(), outcome.commit_mode.label().to_string()));
    rows.push((
        "Narrative".to_string(),
        match outcome.audit_state {
            auditor::AuditState::Committed => "committed",
            auditor::AuditState::Pending => "pending",
            auditor::AuditState::Clean => "clean",
        }
        .to_string(),
    ));

    if outcome.pushed {
        rows.push(("Push".to_string(), "pushed".to_string()));
    }

    append_agent_rows(&mut rows, verbosity);
    format_block(rows)
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
    vcs::stage_paths_allow_missing(&refs)?;
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
        "<instruction>Update the snapshot, glossary, and supporting narrative docs as needed</instruction>"
            .to_string();

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
        get_editor_message().ok()
    } else {
        None
    };

    let save_instruction = build_save_instruction(provided_note.as_deref());
    let prompt_agent = agent.for_prompt(config::PromptKind::Documentation)?;

    let system_prompt = agent_prompt::build_documentation_prompt(
        prompt_agent.prompt_selection(),
        &save_instruction,
        &prompt_agent.documentation,
    )
    .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;

    let response = Auditor::llm_request_with_tools(
        &prompt_agent,
        Some(config::PromptKind::Documentation),
        system_prompt,
        save_instruction,
        None,
    )
    .await?;

    let audit_result = auditor::Auditor::finalize(audit_disposition(commit_mode)).await?;
    let session_display = audit_result.session_display();
    let session_artifact = audit_result.session_artifact.clone();
    let (narrative_paths, narrative_summary) = narrative_change_set(&audit_result);
    let has_narrative_changes = !narrative_paths.is_empty();

    let mut summary_rows = vec![(
        "Assistant summary".to_string(),
        response.content.trim().to_string(),
    )];
    append_agent_rows(&mut summary_rows, current_verbosity());
    let summary_block = format_block(summary_rows);
    if !summary_block.is_empty() {
        for line in summary_block.lines() {
            display::info(line);
        }
    }

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
                    .unwrap_or_else(|| "Update snapshot, glossary, and narrative docs".to_string())
            };

            let mut message_builder = CommitMessageBuilder::new(commit_body);
            message_builder
                .set_header(if has_code_changes {
                    CommitMessageType::CodeChange
                } else {
                    CommitMessageType::NarrativeChange
                })
                .with_session_artifact(session_artifact.clone());

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
                "Snapshot/narrative updates left pending (--no-commit); review and commit when ready.",
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

    match std::fs::write(&temp_path, "") {
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
        return Err(Box::new(std::io::Error::other("Editor command failed")));
    }

    let user_message = match fs::read_to_string(&temp_path) {
        Ok(contents) => {
            if contents.is_empty() {
                return Ok(String::new());
            }

            contents
        }
        Err(e) => {
            return Err(Box::new(std::io::Error::other(format!(
                "Error reading file: {}",
                e
            ))));
        }
    };

    Ok(user_message)
}

pub(super) fn stage_narrative_paths_for_commit(
    paths: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    stage_narrative_paths(paths)
}

pub(super) fn trim_staged_vizier_paths_for_commit(
    allowed: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    trim_staged_vizier_paths(allowed)
}

pub(super) fn clear_narrative_tracker_for_commit(paths: &[String]) {
    clear_narrative_tracker(paths);
}

pub(super) fn narrative_change_set_for_commit(
    result: &auditor::AuditResult,
) -> (Vec<String>, Option<String>) {
    narrative_change_set(result)
}

pub(super) fn build_save_instruction_for_refresh(note: Option<&str>) -> String {
    build_save_instruction(note)
}

#[cfg(test)]
mod tests {
    use super::build_save_instruction;

    #[test]
    fn build_save_instruction_mentions_snapshot_and_glossary() {
        let instruction = build_save_instruction(None);
        assert!(
            instruction.contains("snapshot"),
            "instruction should mention snapshot: {instruction}"
        );
        assert!(
            instruction.contains("glossary"),
            "instruction should mention glossary: {instruction}"
        );
    }
}
