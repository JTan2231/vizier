use vizier_core::{
    agent_prompt, auditor,
    auditor::{Auditor, CommitMessageBuilder, CommitMessageType},
    config,
    display::{self, LogLevel},
    vcs,
};

use super::save::{
    clear_narrative_tracker_for_commit, narrative_change_set_for_commit,
    stage_narrative_paths_for_commit, trim_staged_vizier_paths_for_commit,
};
use super::shared::{
    WorkdirGuard, audit_disposition, print_agent_summary, push_origin_if_requested,
};
use super::types::CommitMode;

/// NOTE: Filters out hidden entries; every visible file in `.vizier/` is treated as part of the narrative surface.
///
/// This is `vizier ask`
pub(crate) async fn inline_command(
    user_message: String,
    push_after_commit: bool,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
) -> Result<(), Box<dyn std::error::Error>> {
    let prompt_agent = agent.for_prompt(config::PromptKind::Documentation)?;
    let system_prompt = match agent_prompt::build_documentation_prompt(
        prompt_agent.prompt_selection(),
        &user_message,
        &prompt_agent.documentation,
    ) {
        Ok(prompt) => prompt,
        Err(e) => {
            display::emit(
                LogLevel::Error,
                format!("Error building agent prompt: {}", e),
            );
            return Err(Box::<dyn std::error::Error>::from(e));
        }
    };

    let response = match Auditor::llm_request_with_tools(
        &prompt_agent,
        Some(config::PromptKind::Documentation),
        system_prompt,
        user_message,
        None,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            display::emit(LogLevel::Error, format!("Error during LLM request: {e}"));
            return Err(e);
        }
    };

    let audit_result = match auditor::Auditor::finalize(audit_disposition(commit_mode)).await {
        Ok(outcome) => outcome,
        Err(e) => {
            display::emit(LogLevel::Error, format!("Error finalizing audit: {e}"));
            return Err(e);
        }
    };
    let session_artifact = audit_result.session_artifact.clone();
    let (narrative_paths, narrative_summary) = narrative_change_set_for_commit(&audit_result);
    let has_narrative_changes = !narrative_paths.is_empty();

    let post_tool_diff = vcs::get_diff(".", Some("HEAD"), Some(&[".vizier/"]))?;
    let has_code_changes = !post_tool_diff.trim().is_empty();
    let mut commit_oid: Option<String> = None;

    if commit_mode.should_commit() {
        if has_code_changes || has_narrative_changes {
            let commit_body = if has_code_changes {
                Auditor::llm_request_for_command(
                    config::CommandScope::Ask,
                    config::get_config()
                        .prompt_for_command(config::CommandScope::Ask, config::PromptKind::Commit)
                        .text,
                    post_tool_diff.clone(),
                )
                .await?
                .content
            } else {
                narrative_summary
                    .clone()
                    .unwrap_or_else(|| "Update snapshot, glossary, and narrative docs".to_string())
            };

            let mut builder = CommitMessageBuilder::new(commit_body);
            builder
                .set_header(if has_code_changes {
                    CommitMessageType::CodeChange
                } else {
                    CommitMessageType::NarrativeChange
                })
                .with_session_artifact(session_artifact.clone());

            if has_code_changes {
                builder.with_narrative_summary(narrative_summary.clone());
            }

            stage_narrative_paths_for_commit(&narrative_paths)?;
            if has_code_changes {
                vcs::stage(Some(vec!["."]))?;
            } else {
                vcs::stage(None)?;
            }
            trim_staged_vizier_paths_for_commit(&narrative_paths)?;

            let commit_message = builder.build();
            let oid = vcs::commit_staged(&commit_message, false)?;
            clear_narrative_tracker_for_commit(&narrative_paths);
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
    print_agent_summary();

    Ok(())
}

pub(crate) async fn run_ask_in_worktree(
    user_message: String,
    push_after_commit: bool,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
    worktree_path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let _guard = WorkdirGuard::enter(worktree_path)?;
    inline_command(user_message, push_after_commit, agent, commit_mode).await
}
