use std::collections::HashSet;
use std::fs;
use std::path::Path;

use git2::{BranchType, Repository, Status, StatusOptions};

use vizier_core::{
    agent_prompt,
    auditor::{self, Auditor},
    config,
    display::{self},
    vcs::{self, commit_paths_in_repo, repo_root},
};

use crate::plan;

use super::shared::{
    WorkdirGuard, append_agent_rows, copy_session_log_to_repo_root, current_verbosity,
    format_block, prompt_selection, require_agent_backend,
};
use super::types::{CommitMode, RefineOptions};

struct RefineOutcome {
    mode: agent_prompt::PlanRefineMode,
    summary: String,
    session_artifact: Option<auditor::SessionArtifact>,
    keep_worktree: bool,
}

pub(crate) async fn run_refine(
    opts: RefineOptions,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
) -> Result<(), Box<dyn std::error::Error>> {
    require_agent_backend(
        agent,
        config::PromptKind::PlanRefine,
        "vizier refine requires an agent-capable selector; update [agents.refine] or pass --agent codex|gemini",
    )?;

    let repo_root = repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    vcs::ensure_clean_worktree().map_err(|err| {
        Box::<dyn std::error::Error>::from(format!(
            "clean working tree required before refine: {err}"
        ))
    })?;

    let repo = Repository::discover(".")?;
    repo.find_branch(&opts.branch, BranchType::Local)
        .map_err(|_| format!("draft branch {} not found", opts.branch))?;

    let plan_rel_path = plan::plan_rel_path(&opts.slug);
    let plan_display = plan_rel_path.to_string_lossy().to_string();
    let worktree = plan::PlanWorktree::create(&opts.slug, &opts.branch, "refine")?;
    let worktree_path = worktree.path().to_path_buf();
    let plan_path = worktree.plan_path(&opts.slug);
    let mut worktree = Some(worktree);

    let mode = if opts.body.is_some() {
        agent_prompt::PlanRefineMode::Update
    } else {
        agent_prompt::PlanRefineMode::Questions
    };

    let refine_result: Result<RefineOutcome, Box<dyn std::error::Error>> = async {
        let _cwd_guard = WorkdirGuard::enter(&worktree_path)?;
        let plan_document = fs::read_to_string(&plan_path).map_err(|err| {
            Box::<dyn std::error::Error>::from(format!(
                "unable to read plan document {}: {err}",
                plan_path.display()
            ))
        })?;
        let plan_meta = plan::PlanMetadata::from_document(&plan_document)?;
        if plan_meta.slug != opts.slug {
            display::warn(format!(
                "Plan metadata references slug {} but CLI resolved to {}",
                plan_meta.slug, opts.slug
            ));
        }
        if plan_meta.branch != opts.branch {
            display::warn(format!(
                "Plan metadata references branch {} but CLI resolved to {}",
                plan_meta.branch, opts.branch
            ));
        }

        let prompt_agent = agent.for_prompt(config::PromptKind::PlanRefine)?;
        let selection = prompt_selection(&prompt_agent)?;
        let prompt = agent_prompt::build_plan_refine_prompt(
            selection,
            &opts.slug,
            &opts.branch,
            &plan_document,
            mode,
            opts.body.as_deref(),
            &prompt_agent.documentation,
        )
        .map_err(|err| -> Box<dyn std::error::Error> { Box::from(format!("build_prompt: {err}")) })?;
        let user_message = match mode {
            agent_prompt::PlanRefineMode::Questions => {
                format!("Refine plan {}: surface open questions", opts.slug)
            }
            agent_prompt::PlanRefineMode::Update => format!(
                "Refine plan {} with clarifications:\n{}",
                opts.slug,
                opts.body.as_deref().unwrap_or_default().trim()
            ),
        };
        let response = Auditor::llm_request_with_tools(
            &prompt_agent,
            Some(config::PromptKind::PlanRefine),
            prompt,
            user_message,
            Some(worktree_path.clone()),
        )
        .await
        .map_err(|err| Box::<dyn std::error::Error>::from(format!("Agent backend: {err}")))?;

        let mut summary = response.content.trim().to_string();
        if summary.is_empty() {
            summary = match mode {
                agent_prompt::PlanRefineMode::Questions => {
                    format!("No questions returned for plan {}.", opts.slug)
                }
                agent_prompt::PlanRefineMode::Update => {
                    format!("Refined plan {}.", opts.slug)
                }
            };
        }

        let plan_rel = plan_rel_path.to_string_lossy().replace('\\', "/");
        let mut changed_paths = collect_changed_paths(&worktree_path)?;

        match mode {
            agent_prompt::PlanRefineMode::Questions => {
                if !changed_paths.is_empty() {
                    let listed = changed_paths.join("\n");
                    return Err(format!(
                        "refine questions should not modify files; changes detected:\n{listed}"
                    )
                    .into());
                }
            }
            agent_prompt::PlanRefineMode::Update => {
                if !changed_paths.is_empty() {
                    let unexpected: Vec<String> = changed_paths
                        .iter()
                        .filter(|path| path.as_str() != plan_rel)
                        .cloned()
                        .collect();
                    if !unexpected.is_empty() {
                        let listed = unexpected.join("\n");
                        return Err(format!(
                            "refine updated unexpected files; only {} is allowed:\n{listed}",
                            plan_rel
                        )
                        .into());
                    }
                } else if let Some(clarifications) = opts.body.as_ref() {
                    let updated = append_refine_clarifications(&plan_document, clarifications);
                    if updated == plan_document {
                        return Err("clarifications did not modify the plan document".into());
                    }
                    plan::write_plan_file(&plan_path, &updated).map_err(
                        |err| -> Box<dyn std::error::Error> {
                            Box::from(format!(
                                "write_plan_file({}): {err}",
                                plan_path.display()
                            ))
                        },
                    )?;
                    changed_paths = collect_changed_paths(&worktree_path)?;
                }

                let unexpected: Vec<String> = changed_paths
                    .iter()
                    .filter(|path| path.as_str() != plan_rel)
                    .cloned()
                    .collect();
                if !unexpected.is_empty() {
                    let listed = unexpected.join("\n");
                    return Err(format!(
                        "refine updated unexpected files; only {} is allowed:\n{listed}",
                        plan_rel
                    )
                    .into());
                }
                if !changed_paths.iter().any(|path| path == &plan_rel) {
                    return Err("refine completed without updating the plan document".into());
                }

                let updated_doc = fs::read_to_string(&plan_path)?;
                let updated_meta = plan::PlanMetadata::from_document(&updated_doc)?;
                if updated_meta.slug != opts.slug {
                    display::warn(format!(
                        "Updated plan metadata references slug {} but CLI resolved to {}",
                        updated_meta.slug, opts.slug
                    ));
                }
                if updated_meta.branch != opts.branch {
                    display::warn(format!(
                        "Updated plan metadata references branch {} but CLI resolved to {}",
                        updated_meta.branch, opts.branch
                    ));
                }
            }
        }

        let mut keep_worktree = false;
        if matches!(mode, agent_prompt::PlanRefineMode::Update) {
            if commit_mode.should_commit() {
                let plan_rel = plan_rel_path.as_path();
                let _ = commit_paths_in_repo(
                    &worktree_path,
                    &[plan_rel],
                    &format!("docs: refine implementation plan {}", opts.slug),
                )
                .map_err(|err| -> Box<dyn std::error::Error> {
                    Box::from(format!("commit_plan({}): {err}", worktree_path.display()))
                })?;
            } else {
                keep_worktree = true;
                display::info(
                    "Plan document updated with --no-commit; leaving worktree dirty for manual review.",
                );
            }
        }

        let session_artifact = Auditor::persist_session_log();
        if session_artifact.is_some() {
            Auditor::clear_messages();
        }

        Ok(RefineOutcome {
            mode,
            summary,
            session_artifact,
            keep_worktree,
        })
    }
    .await;

    match refine_result {
        Ok(outcome) => {
            if let Some(artifact) = outcome.session_artifact.as_ref() {
                copy_session_log_to_repo_root(&repo_root, artifact);
            }

            if outcome.keep_worktree {
                display::info(format!(
                    "Plan worktree preserved at {}; inspect branch {} for pending changes.",
                    worktree_path.display(),
                    opts.branch
                ));
            } else if let Some(tree) = worktree.take()
                && let Err(err) = tree.cleanup()
            {
                display::warn(format!(
                    "temporary worktree cleanup failed ({}); remove manually with `git worktree prune`",
                    err
                ));
            }

            let mut rows = vec![
                (
                    "Outcome".to_string(),
                    match outcome.mode {
                        agent_prompt::PlanRefineMode::Questions => {
                            "Plan questions ready".to_string()
                        }
                        agent_prompt::PlanRefineMode::Update => "Plan refined".to_string(),
                    },
                ),
                ("Plan".to_string(), plan_display.clone()),
                ("Branch".to_string(), opts.branch.clone()),
            ];
            if outcome.keep_worktree {
                rows.push(("Worktree".to_string(), worktree_path.display().to_string()));
            }
            append_agent_rows(&mut rows, current_verbosity());
            println!("{}", format_block(rows));

            if matches!(outcome.mode, agent_prompt::PlanRefineMode::Questions) {
                println!();
                emit_refine_questions(&opts.slug, &outcome.summary);
            } else if !outcome.summary.trim().is_empty() {
                println!();
                println!("{}", outcome.summary.trim());
            }

            Ok(())
        }
        Err(err) => {
            if let Some(tree) = worktree.take() {
                display::info(format!(
                    "Plan worktree preserved at {}; inspect branch {} for partial changes.",
                    tree.path.display(),
                    opts.branch
                ));
            }
            Err(err)
        }
    }
}

fn collect_changed_paths(repo_path: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let repo = Repository::open(repo_path)?;
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .exclude_submodules(true);
    let statuses = repo.statuses(Some(&mut opts))?;
    let mut paths = HashSet::new();
    for entry in statuses.iter() {
        let status = entry.status();
        if status.is_empty() || status.contains(Status::IGNORED) {
            continue;
        }
        if let Some(path) = entry.path() {
            paths.insert(path.replace('\\', "/"));
        }
    }
    let mut sorted: Vec<String> = paths.into_iter().collect();
    sorted.sort();
    Ok(sorted)
}

fn append_refine_clarifications(plan_document: &str, clarifications: &str) -> String {
    let mut doc = plan_document.trim_end_matches(['\n', '\r']).to_string();
    let clarifications = plan::trim_trailing_newlines(clarifications).trim();
    if clarifications.is_empty() {
        return doc;
    }
    doc.push_str("\n\n## Clarifications\n");
    doc.push_str(clarifications);
    doc.push('\n');
    doc
}

fn emit_refine_questions(plan_slug: &str, questions: &str) {
    println!("--- Plan refine questions for plan {plan_slug} ---");
    if questions.trim().is_empty() {
        println!("(Agent returned no questions.)");
    } else {
        println!("{}", questions.trim());
    }
    println!("--- End plan refine questions ---");
}
