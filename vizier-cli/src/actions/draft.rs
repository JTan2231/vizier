use chrono::Utc;
use std::fs;
use vizier_core::{
    agent_prompt,
    auditor::{self, Auditor},
    config,
    display::{self, LogLevel},
    vcs::{
        add_worktree_for_branch, commit_paths_in_repo, create_branch_from, delete_branch,
        detect_primary_branch, remove_worktree, repo_root,
    },
};

use crate::{jobs, plan};

use super::shared::{
    WorkdirGuard, append_agent_rows, copy_session_log_to_repo_root, current_verbosity,
    format_block, prompt_selection, require_agent_backend,
};
use super::types::{CommitMode, DraftArgs};

pub(crate) async fn run_draft(
    args: DraftArgs,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
) -> Result<(), Box<dyn std::error::Error>> {
    require_agent_backend(
        agent,
        config::PromptKind::ImplementationPlan,
        "vizier draft requires an agent-capable selector; update [agents.commands.draft] (or legacy [agents.draft]) or pass --agent codex|gemini",
    )?;

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
    let plan_id = plan::new_plan_id();
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
    display::debug(format!(
        "Drafting plan {slug} from spec source: {spec_source_label}"
    ));

    let mut plan_document_preview: Option<String> = None;
    let mut session_artifact: Option<auditor::SessionArtifact> = None;
    let primary_branch = detect_primary_branch()
        .ok_or("unable to detect a primary branch (tried origin/HEAD, main, master)")?;

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
        jobs::record_current_job_worktree(&repo_root, Some(&worktree_name), &worktree_path);
        worktree_created = true;
        let _cwd_guard = WorkdirGuard::enter(&worktree_path)?;

        let prompt_agent = agent.for_prompt(config::PromptKind::ImplementationPlan)?;
        let selection = prompt_selection(&prompt_agent)?;
        let prompt = agent_prompt::build_implementation_plan_prompt(
            selection,
            agent_prompt::ImplementationPlanPromptInput {
                plan_id: &plan_id,
                plan_slug: &slug,
                branch_name: &branch_name,
                operator_spec: &spec_text,
                documentation: &prompt_agent.documentation,
            },
        )
        .map_err(|err| -> Box<dyn std::error::Error> { Box::from(format!("build_prompt: {err}")) })?;

        let llm_response = Auditor::llm_request_with_tools(
            &prompt_agent,
            Some(config::PromptKind::ImplementationPlan),
            prompt,
            spec_text.clone(),
            Some(worktree_path.clone()),
        )
        .await
        .map_err(|err| Box::<dyn std::error::Error>::from(format!("Agent backend: {err}")))?;

        let plan_body = llm_response.content;
        let document = plan::render_plan_document(
            &plan_id,
            &slug,
            &branch_name,
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
        let plan_state_rel = {
            let parsed = plan::PlanMetadata::from_document(&document).ok();
            let summary = parsed.as_ref().map(plan::summarize_spec);
            let now = Utc::now().to_rfc3339();
            plan::upsert_plan_record(
                &worktree_path,
                plan::PlanRecordUpsert {
                    plan_id: plan_id.clone(),
                    slug: Some(slug.clone()),
                    branch: Some(branch_name.clone()),
                    source: Some("draft".to_string()),
                    intent: Some(spec_source_label.to_string()),
                    target_branch: Some(primary_branch.clone()),
                    work_ref: Some(branch_name.clone()),
                    status: Some("draft".to_string()),
                    summary,
                    updated_at: now.clone(),
                    created_at: Some(now),
                    job_ids: None,
                },
            )?
        };

        if commit_mode.should_commit() {
            let plan_rel = plan_rel_path.as_path();
            let plan_state_path = plan_state_rel.as_path();
            commit_paths_in_repo(
                &worktree_path,
                &[plan_rel, plan_state_path],
                &format!("docs: add implementation plan {}", slug),
            )
            .map_err(|err| -> Box<dyn std::error::Error> {
                Box::from(format!("commit_plan({}): {err}", worktree_path.display()))
            })?;
            plan_committed = true;
            if let Some(artifact) = Auditor::persist_session_log() {
                session_artifact = Some(artifact);
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

            if let Some(artifact) = session_artifact.as_ref() {
                copy_session_log_to_repo_root(&repo_root, artifact);
            }

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

                let mut rows = vec![
                    ("Outcome".to_string(), "Draft ready".to_string()),
                    ("Plan".to_string(), plan_display.clone()),
                    ("Branch".to_string(), branch_name.clone()),
                ];
                append_agent_rows(&mut rows, current_verbosity());
                println!("{}", format_block(rows));
            } else {
                let mut rows = vec![
                    (
                        "Outcome".to_string(),
                        "Draft pending (manual commit)".to_string(),
                    ),
                    ("Branch".to_string(), branch_name.clone()),
                    ("Worktree".to_string(), worktree_path.display().to_string()),
                    ("Plan".to_string(), plan_display.clone()),
                ];
                append_agent_rows(&mut rows, current_verbosity());
                println!("{}", format_block(rows));
                display::info(format!(
                    "Review and commit manually: git -C {} status",
                    worktree_path.display()
                ));
            }

            if let Some(plan_text) = plan_to_print {
                println!();
                println!("{plan_text}");
            }
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
