use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use git2::build::CheckoutBuilder;
use git2::{BranchType, Oid, Repository, RepositoryState};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use vizier_core::{
    agent::ProgressHook,
    agent_prompt,
    auditor::{self, Auditor, CommitMessageBuilder, CommitMessageType},
    config,
    display::{self},
    vcs,
    vcs::{
        CherryPickOutcome, MergePreparation, MergeReady, amend_head_commit,
        apply_cherry_pick_sequence, build_squash_plan, commit_in_progress_cherry_pick,
        commit_in_progress_merge, commit_in_progress_squash, commit_ready_merge,
        commit_soft_squash, commit_squashed_merge, list_conflicted_paths, prepare_merge, repo_root,
    },
};

use crate::errors::CancelledError;
use crate::plan;

use super::gates::{CicdScriptResult, clip_log, log_cicd_result, run_cicd_script};
use super::save::{
    build_save_instruction_for_refresh, clear_narrative_tracker_for_commit,
    narrative_change_set_for_commit, stage_narrative_paths_for_commit,
    trim_staged_vizier_paths_for_commit,
};
use super::shared::{
    WorkdirGuard, append_agent_rows, build_agent_request, current_verbosity, format_block,
    prompt_for_confirmation, prompt_selection, push_origin_if_requested, require_agent_backend,
    short_hash, spawn_plain_progress_logger,
};
use super::types::{CommitMode, ConflictAutoResolveSetting, MergeConflictStrategy, MergeOptions};

#[derive(Debug, Clone)]
struct CicdGateOutcome {
    script: PathBuf,
    attempts: u32,
    fixes: Vec<CicdFixRecord>,
}

#[derive(Debug, Clone)]
enum CicdFixRecord {
    Commit(String),
    Amend(String),
}

impl CicdFixRecord {
    fn describe(&self) -> String {
        match self {
            CicdFixRecord::Commit(oid) => format!("commit:{oid}"),
            CicdFixRecord::Amend(oid) => format!("amend:{oid}"),
        }
    }
}

#[derive(Debug)]
struct MergeExecutionResult {
    merge_oid: Oid,
    source_oid: Oid,
    gate: Option<CicdGateOutcome>,
    squashed: bool,
    implementation_oid: Option<Oid>,
}

#[derive(Debug)]
enum MergeConflictResolution {
    MergeCommitted {
        merge_oid: Oid,
        source_oid: Oid,
    },
    SquashImplementationCommitted {
        source_oid: Oid,
        implementation_oid: Oid,
    },
}

#[derive(Debug)]
enum PendingMergeStatus {
    None,
    Ready {
        merge_oid: Oid,
        source_oid: Oid,
    },
    SquashReady {
        source_oid: Oid,
        merge_message: String,
    },
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
                "Vizier has merge metadata for plan {} but Git is no longer merging/cherry-picking on {}; rerun `vizier merge {}` (without --complete-conflict) to start a new merge if needed.",
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

#[derive(Debug, Serialize, Deserialize)]
struct MergeReplayState {
    merge_base_oid: String,
    start_oid: String,
    source_commits: Vec<String>,
    applied_commits: Vec<String>,
    #[serde(default)]
    squash_mainline: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct MergeConflictState {
    slug: String,
    source_branch: String,
    target_branch: String,
    head_oid: String,
    source_oid: String,
    merge_message: String,
    #[serde(default)]
    squash: bool,
    #[serde(default)]
    implementation_message: Option<String>,
    #[serde(default)]
    replay: Option<MergeReplayState>,
    #[serde(default)]
    squash_mainline: Option<u32>,
}

pub(crate) async fn run_merge(
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

    if matches!(opts.conflict_strategy, MergeConflictStrategy::Agent) {
        require_agent_backend(
            agent,
            config::PromptKind::MergeConflict,
            "Agent-based conflict resolution requires an agent-capable selector; update [agents.merge] or rerun with --agent codex|gemini",
        )?;
    }
    display::warn(opts.conflict_auto_resolve.status_line());

    if opts.cicd_gate.auto_resolve && opts.cicd_gate.script.is_some() {
        let review_agent = agent.for_prompt(config::PromptKind::Review)?;
        if !review_agent.backend.requires_agent_runner() {
            display::warn(
                "CI/CD auto-remediation requested but [agents.merge] is not set to an agent-style backend; gate failures will abort without auto fixes.",
            );
        }
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

    match try_complete_pending_merge(
        &spec,
        opts.conflict_strategy,
        opts.conflict_auto_resolve,
        agent,
    )
    .await?
    {
        PendingMergeStatus::Ready {
            merge_oid,
            source_oid,
        } => {
            let gate_summary = run_cicd_gate_for_merge(&spec, &opts, agent).await?;
            let execution = MergeExecutionResult {
                merge_oid,
                source_oid,
                gate: gate_summary,
                squashed: false,
                implementation_oid: None,
            };
            finalize_merge(&spec, execution, opts.delete_branch, opts.push_after)?;
            return Ok(());
        }
        PendingMergeStatus::SquashReady {
            source_oid,
            merge_message,
        } => {
            let execution = finalize_squashed_merge_from_head(
                &spec,
                &merge_message,
                source_oid,
                None,
                &opts,
                agent,
            )
            .await?;
            finalize_merge(&spec, execution, opts.delete_branch, opts.push_after)?;
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
            return Err(Box::new(CancelledError::new("merge cancelled")));
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

    if let Some(tree) = worktree.take()
        && let Err(err) = tree.cleanup()
    {
        display::warn(format!(
            "temporary worktree cleanup failed ({}); remove manually with `git worktree prune`",
            err
        ));
    }

    let current_branch = current_branch_name(&repo)?;
    if current_branch.as_deref() != Some(spec.target_branch.as_str()) {
        display::info(format!(
            "Checking out {} before merge...",
            spec.target_branch
        ));
        vcs::checkout_branch(&spec.target_branch)?;
    }

    let implementation_message = build_implementation_commit_message(&spec, &plan_meta);
    let merge_message = build_merge_commit_message(
        &spec,
        &plan_meta,
        plan_document.as_deref(),
        opts.note.as_deref(),
    );
    let mut squash_plan = None;
    if opts.squash {
        squash_plan = Some(resolve_squash_plan_and_mainline(&spec, &opts)?);
    }

    let execution = if opts.squash {
        let (plan, mainline) = squash_plan.expect("missing squash plan despite squash=true");
        execute_squashed_merge(
            &spec,
            &implementation_message,
            &merge_message,
            &opts,
            plan,
            mainline,
            agent,
        )
        .await?
    } else {
        let preparation = prepare_merge(&spec.branch)?;
        execute_legacy_merge(&spec, &merge_message, preparation, &opts, agent).await?
    };

    finalize_merge(&spec, execution, opts.delete_branch, opts.push_after)?;
    Ok(())
}

fn resolve_squash_plan_and_mainline(
    spec: &plan::PlanBranchSpec,
    opts: &MergeOptions,
) -> Result<(vcs::SquashPlan, Option<u32>), Box<dyn std::error::Error>> {
    let plan = build_squash_plan(&spec.branch)?;
    if plan.merge_commits.is_empty() {
        return Ok((plan, opts.squash_mainline));
    }

    display::warn(
        "Plan branch contains merge commits; squash mode requires choosing a mainline parent.",
    );
    for merge in &plan.merge_commits {
        let parents = merge
            .parents
            .iter()
            .map(|oid| short_hash(&oid.to_string()))
            .collect::<Vec<_>>()
            .join(", ");
        let summary = merge.summary.as_deref().unwrap_or("no subject");
        display::warn(format!(
            "  - {} (parents: {}) - {}",
            short_hash(&merge.oid.to_string()),
            parents,
            summary
        ));
    }

    if plan
        .merge_commits
        .iter()
        .any(|merge| merge.parents.len() > 2)
    {
        return Err(format!(
            "Plan branch {} contains octopus merges; rerun vizier merge {} with --no-squash or rewrite the branch history.",
            spec.slug, spec.slug
        )
        .into());
    }

    if let Some(mainline) = opts.squash_mainline {
        validate_squash_mainline(mainline, &plan.merge_commits)?;
        if plan.mainline_ambiguous {
            display::warn(
                "Merge history is ambiguous; proceeding with the provided --squash-mainline value.",
            );
        } else if let Some(inferred) = plan.inferred_mainline
            && inferred != mainline
        {
            display::warn(format!(
                "Inferred mainline {} differs from provided {}; continuing with the provided value.",
                inferred, mainline
            ));
        }
        return Ok((plan, Some(mainline)));
    }

    let hint = plan
        .inferred_mainline
        .map(|hint| format!(" (suggested mainline: {hint})"))
        .unwrap_or_default();
    let mut guidance = format!(
        "Plan branch {} includes merge commits; rerun with --squash-mainline <parent index>{hint} or use --no-squash to keep the branch history.",
        spec.slug
    );
    if plan.mainline_ambiguous {
        guidance.push_str(" Merge history appears ambiguous; --no-squash is safest.");
    }

    Err(guidance.into())
}

fn validate_squash_mainline(
    mainline: u32,
    merges: &[vcs::MergeCommitSummary],
) -> Result<(), Box<dyn std::error::Error>> {
    if mainline == 0 {
        return Err("squash mainline parent index must be at least 1".into());
    }

    for merge in merges {
        if mainline as usize > merge.parents.len() {
            return Err(format!(
                "squash mainline parent {} is out of range for merge commit {}",
                mainline,
                short_hash(&merge.oid.to_string())
            )
            .into());
        }
    }

    Ok(())
}

async fn execute_legacy_merge(
    spec: &plan::PlanBranchSpec,
    merge_message: &str,
    preparation: MergePreparation,
    opts: &MergeOptions,
    agent: &config::AgentSettings,
) -> Result<MergeExecutionResult, Box<dyn std::error::Error>> {
    let (merge_oid, source_oid) = match preparation {
        MergePreparation::Ready(ready) => {
            let source_tip = ready.source_oid;
            let oid = commit_ready_merge(merge_message, ready)?;
            (oid, source_tip)
        }
        MergePreparation::Conflicted(conflict) => {
            match handle_merge_conflict(MergeConflictInput {
                spec,
                merge_message,
                conflict,
                setting: opts.conflict_auto_resolve,
                strategy: opts.conflict_strategy,
                squash: false,
                implementation_message: None,
                agent,
            })
            .await?
            {
                MergeConflictResolution::MergeCommitted {
                    merge_oid,
                    source_oid,
                } => (merge_oid, source_oid),
                MergeConflictResolution::SquashImplementationCommitted { .. } => {
                    return Err(
                        "internal error: squashed merge resolution returned for legacy path".into(),
                    );
                }
            }
        }
    };

    let gate = run_cicd_gate_for_merge(spec, opts, agent).await?;
    Ok(MergeExecutionResult {
        merge_oid,
        source_oid,
        gate,
        squashed: false,
        implementation_oid: None,
    })
}

async fn execute_squashed_merge(
    spec: &plan::PlanBranchSpec,
    implementation_message: &str,
    merge_message: &str,
    opts: &MergeOptions,
    plan: vcs::SquashPlan,
    squash_mainline: Option<u32>,
    agent: &config::AgentSettings,
) -> Result<MergeExecutionResult, Box<dyn std::error::Error>> {
    match apply_cherry_pick_sequence(
        plan.target_head,
        &plan.commits_to_apply,
        None,
        squash_mainline,
    )? {
        CherryPickOutcome::Completed(result) => {
            let expected_head = result.applied.last().copied().unwrap_or(plan.target_head);
            let implementation_oid =
                commit_soft_squash(implementation_message, plan.target_head, expected_head)?;
            finalize_squashed_merge_from_head(
                spec,
                merge_message,
                plan.source_tip,
                Some(implementation_oid),
                opts,
                agent,
            )
            .await
        }
        CherryPickOutcome::Conflicted(conflict) => {
            match handle_squash_apply_conflict(SquashApplyConflictInput {
                spec,
                merge_message,
                implementation_message,
                plan: &plan,
                mainline: squash_mainline,
                conflict,
                setting: opts.conflict_auto_resolve,
                strategy: opts.conflict_strategy,
                agent,
            })
            .await?
            {
                MergeConflictResolution::SquashImplementationCommitted {
                    source_oid,
                    implementation_oid,
                } => {
                    finalize_squashed_merge_from_head(
                        spec,
                        merge_message,
                        source_oid,
                        Some(implementation_oid),
                        opts,
                        agent,
                    )
                    .await
                }
                MergeConflictResolution::MergeCommitted { .. } => Err(
                    "internal error: legacy merge conflict resolution triggered while squashing"
                        .into(),
                ),
            }
        }
    }
}

async fn finalize_squashed_merge_from_head(
    spec: &plan::PlanBranchSpec,
    merge_message: &str,
    source_oid: Oid,
    expected_implementation: Option<Oid>,
    opts: &MergeOptions,
    agent: &config::AgentSettings,
) -> Result<MergeExecutionResult, Box<dyn std::error::Error>> {
    let gate = run_cicd_gate_for_merge(spec, opts, agent).await?;
    let ready = merge_ready_from_head(source_oid)?;
    if let Some(expected) = expected_implementation
        && expected != ready.head_oid
    {
        display::warn(format!(
            "HEAD moved after recording the implementation commit (expected {}, saw {}); finalizing merge from the current HEAD state.",
            short_hash(&expected.to_string()),
            short_hash(&ready.head_oid.to_string())
        ));
    }
    let implementation_head = ready.head_oid;
    let merge_oid = commit_squashed_merge(merge_message, ready)?;
    Ok(MergeExecutionResult {
        merge_oid,
        source_oid,
        gate,
        squashed: true,
        implementation_oid: Some(implementation_head),
    })
}

struct SquashApplyConflictInput<'a> {
    spec: &'a plan::PlanBranchSpec,
    merge_message: &'a str,
    implementation_message: &'a str,
    plan: &'a vcs::SquashPlan,
    mainline: Option<u32>,
    conflict: vcs::CherryPickApplyConflict,
    setting: ConflictAutoResolveSetting,
    strategy: MergeConflictStrategy,
    agent: &'a config::AgentSettings,
}

async fn handle_squash_apply_conflict(
    input: SquashApplyConflictInput<'_>,
) -> Result<MergeConflictResolution, Box<dyn std::error::Error>> {
    let SquashApplyConflictInput {
        spec,
        merge_message,
        implementation_message,
        plan,
        mainline,
        conflict,
        setting,
        strategy,
        agent,
    } = input;
    let files = conflict.files.clone();
    let replay_state = MergeReplayState {
        merge_base_oid: plan.merge_base.to_string(),
        start_oid: plan.target_head.to_string(),
        source_commits: plan
            .commits_to_apply
            .iter()
            .map(|oid| oid.to_string())
            .collect(),
        applied_commits: conflict.applied.iter().map(|oid| oid.to_string()).collect(),
        squash_mainline: mainline,
    };
    let state = MergeConflictState {
        slug: spec.slug.clone(),
        source_branch: spec.branch.clone(),
        target_branch: spec.target_branch.clone(),
        head_oid: plan.target_head.to_string(),
        source_oid: plan.source_tip.to_string(),
        merge_message: merge_message.to_string(),
        squash: true,
        implementation_message: Some(implementation_message.to_string()),
        replay: Some(replay_state),
        squash_mainline: mainline,
    };

    let state_path = write_conflict_state(&state)?;
    display::warn("Cherry-picking the plan commits onto the target branch produced conflicts.");
    emit_conflict_instructions(&spec.slug, &files, &state_path);

    match strategy {
        MergeConflictStrategy::Manual => {
            display::warn(setting.status_line());
            Err("merge blocked by conflicts; resolve them and rerun vizier merge with --complete-conflict".into())
        }
        MergeConflictStrategy::Agent => {
            display::warn(format!(
                "Auto-resolving merge conflicts via {}...",
                setting.source_description()
            ));
            match try_auto_resolve_conflicts(spec, &state, &files, agent).await {
                Ok(resolution) => Ok(resolution),
                Err(err) => {
                    display::warn(format!(
                        "Backend auto-resolution failed: {err}. Falling back to manual resolution."
                    ));
                    emit_conflict_instructions(&spec.slug, &files, &state_path);
                    Err("merge blocked by conflicts; resolve them and rerun vizier merge".into())
                }
            }
        }
    }
}

fn merge_ready_from_head(source_oid: Oid) -> Result<MergeReady, Box<dyn std::error::Error>> {
    let repo = Repository::discover(".")?;
    let head = repo.head()?;
    if !head.is_branch() {
        return Err(
            "cannot finalize merge while HEAD is detached; checkout the target branch first".into(),
        );
    }
    let head_commit = head.peel_to_commit()?;
    Ok(MergeReady {
        head_oid: head_commit.id(),
        source_oid,
        tree_oid: head_commit.tree_id(),
    })
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
    let mut fix_commits: Vec<CicdFixRecord> = Vec::new();

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

        if !agent.backend.requires_agent_runner() {
            display::warn(
                "CI/CD gate auto-remediation requires an agent-style backend; skipping automatic fixes.",
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
            "CI/CD gate failed; attempting backend remediation ({}/{})...",
            fix_attempts, opts.cicd_gate.retries
        ));
        let truncated_stdout = clip_log(result.stdout.as_bytes());
        let truncated_stderr = clip_log(result.stderr.as_bytes());
        if let Some(record) = attempt_cicd_auto_fix(CicdAutoFixInput {
            spec,
            script,
            attempt: fix_attempts,
            max_attempts: opts.cicd_gate.retries,
            exit_code: result.status.code(),
            stdout: &truncated_stdout,
            stderr: &truncated_stderr,
            agent,
            amend_head: opts.squash,
        })
        .await?
        {
            match &record {
                CicdFixRecord::Commit(oid) => {
                    display::info(format!("Remediation attempt committed at {}.", oid));
                }
                CicdFixRecord::Amend(oid) => {
                    display::info(format!(
                        "Remediation attempt amended the implementation commit ({}).",
                        oid
                    ));
                }
            }
            fix_commits.push(record);
        } else {
            display::info("Backend remediation reported no file changes.");
        }
    }
}

struct CicdAutoFixInput<'a> {
    spec: &'a plan::PlanBranchSpec,
    script: &'a Path,
    attempt: u32,
    max_attempts: u32,
    exit_code: Option<i32>,
    stdout: &'a str,
    stderr: &'a str,
    agent: &'a config::AgentSettings,
    amend_head: bool,
}

async fn attempt_cicd_auto_fix(
    input: CicdAutoFixInput<'_>,
) -> Result<Option<CicdFixRecord>, Box<dyn std::error::Error>> {
    let CicdAutoFixInput {
        spec,
        script,
        attempt,
        max_attempts,
        exit_code,
        stdout,
        stderr,
        agent,
        amend_head,
    } = input;
    let fix_agent = agent.for_prompt(config::PromptKind::Review)?;
    let prompt = agent_prompt::build_cicd_failure_prompt(agent_prompt::CicdFailurePromptInput {
        plan_slug: &spec.slug,
        plan_branch: &spec.branch,
        target_branch: &spec.target_branch,
        script_path: script,
        attempt,
        max_attempts,
        exit_code,
        stdout,
        stderr,
        documentation: &fix_agent.documentation,
    })
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
        &fix_agent,
        Some(config::PromptKind::Review),
        prompt,
        instruction,
        auditor::RequestStream::Status {
            text: text_tx,
            events: Some(event_tx),
        },
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
    let (narrative_paths, narrative_summary) = narrative_change_set_for_commit(&audit_result);
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
    stage_narrative_paths_for_commit(&narrative_paths)?;
    vcs::stage(Some(vec!["."]))?;
    trim_staged_vizier_paths_for_commit(&narrative_paths)?;
    let record = if amend_head {
        let commit_oid = amend_head_commit(None)?;
        CicdFixRecord::Amend(commit_oid.to_string())
    } else {
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
        let commit_oid = vcs::commit_staged(&message, false)?;
        CicdFixRecord::Commit(commit_oid.to_string())
    };
    clear_narrative_tracker_for_commit(&narrative_paths);

    Ok(Some(record))
}

fn finalize_merge(
    spec: &plan::PlanBranchSpec,
    execution: MergeExecutionResult,
    delete_branch: bool,
    push_after: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let MergeExecutionResult {
        merge_oid,
        source_oid,
        gate,
        squashed,
        implementation_oid,
    } = execution;

    let repo = Repository::discover(".")?;
    if delete_branch {
        let mut should_delete = true;
        if squashed {
            let merge_commit = repo.find_commit(merge_oid)?;
            if merge_commit.parent_count() != 1 {
                display::warn(format!(
                    "Skipping deletion of {}; expected a single-parent merge but found {} parent(s).",
                    spec.branch,
                    merge_commit.parent_count()
                ));
                should_delete = false;
            } else if let Some(expected_impl) = implementation_oid {
                let parent = merge_commit.parent(0)?;
                if parent.id() != expected_impl {
                    display::warn(format!(
                        "Skipping deletion of {}; merge parent {} did not match the recorded implementation commit {}.",
                        spec.branch,
                        short_hash(&parent.id().to_string()),
                        short_hash(&expected_impl.to_string())
                    ));
                    should_delete = false;
                }
            }
        } else if !repo.graph_descendant_of(merge_oid, source_oid)? {
            display::warn(format!(
                "Skipping deletion of {}; merge commit did not include the branch tip.",
                spec.branch
            ));
            should_delete = false;
        }

        if should_delete {
            vcs::delete_branch(&spec.branch)?;
            display::info(format!("Deleted {} after merge", spec.branch));
        }
    } else {
        display::info(format!(
            "Keeping {} after merge (branch deletion disabled).",
            spec.branch
        ));
    }

    push_origin_if_requested(push_after)?;
    let mut rows = vec![
        ("Outcome".to_string(), "Merge complete".to_string()),
        ("Plan".to_string(), spec.slug.clone()),
        ("Target".to_string(), spec.target_branch.clone()),
        (
            "Merge commit".to_string(),
            short_hash(&merge_oid.to_string()),
        ),
    ];

    if let Some(summary) = gate.as_ref() {
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
        rows.push(("CI/CD script".to_string(), script_label));
        rows.push((
            "Gate attempts".to_string(),
            display::format_number(summary.attempts as usize),
        ));
        if !summary.fixes.is_empty() {
            let labels = summary
                .fixes
                .iter()
                .map(|record| record.describe())
                .collect::<Vec<_>>()
                .join(", ");
            if !labels.is_empty() {
                rows.push(("Gate fixes".to_string(), labels));
            }
        }
    }

    let verbosity = current_verbosity();
    append_agent_rows(&mut rows, verbosity);
    println!("{}", format_block(rows));
    Ok(())
}

async fn try_complete_pending_merge(
    spec: &plan::PlanBranchSpec,
    strategy: MergeConflictStrategy,
    setting: ConflictAutoResolveSetting,
    agent: &config::AgentSettings,
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

    if let Some(replay) = state.replay.as_ref() {
        let implementation_message = state
            .implementation_message
            .as_deref()
            .ok_or("missing implementation commit message for squashed merge state")?;
        let start_oid = Oid::from_str(&replay.start_oid)?;
        let mut applied_commits: Vec<Oid> = replay
            .applied_commits
            .iter()
            .filter_map(|oid| Oid::from_str(oid).ok())
            .collect();
        let source_commits: Vec<Oid> = replay
            .source_commits
            .iter()
            .filter_map(|oid| Oid::from_str(oid).ok())
            .collect();

        let expected_head = applied_commits.last().copied().unwrap_or(start_oid);
        let head_commit = repo.head()?.peel_to_commit()?.id();
        if head_commit != expected_head {
            display::warn(format!(
                "Merge metadata for plan {} exists but HEAD moved; cleaning up the pending state.",
                spec.slug
            ));
            let _ = clear_conflict_state(&spec.slug);
            return Ok(PendingMergeStatus::Blocked(
                PendingMergeBlocker::NotInMerge {
                    target_branch: spec.target_branch.clone(),
                },
            ));
        }

        if let Err(err) = vcs::stage_all_in(repo.path()) {
            display::warn(format!(
                "Unable to stage merge replay changes via libgit2: {err}"
            ));
        }
        let mut conflict_paths = Vec::new();
        if let Ok(idx) = repo.index()
            && let Ok(mut conflicts) = idx.conflicts()
        {
            for conflict in conflicts.by_ref().flatten() {
                let path_bytes = conflict
                    .our
                    .as_ref()
                    .or(conflict.their.as_ref())
                    .or(conflict.ancestor.as_ref())
                    .map(|entry| entry.path.clone());
                if let Some(bytes) = path_bytes {
                    conflict_paths.push(String::from_utf8_lossy(&bytes).to_string());
                }
            }
        }
        if !conflict_paths.is_empty() {
            let mut index = repo.index()?;
            for path in &conflict_paths {
                index.add_path(Path::new(path))?;
                index.conflict_remove(Path::new(path))?;
            }
            index.write()?;
        }
        let mut outstanding = Vec::new();
        {
            let mut idx = repo.index()?;
            let _ = idx.read(true);
            if idx.has_conflicts()
                && let Ok(mut conflicts) = idx.conflicts()
            {
                for conflict in conflicts.by_ref().flatten() {
                    let path_bytes = conflict
                        .our
                        .as_ref()
                        .or(conflict.their.as_ref())
                        .or(conflict.ancestor.as_ref())
                        .map(|entry| entry.path.clone());
                    if let Some(bytes) = path_bytes {
                        outstanding.push(String::from_utf8_lossy(&bytes).to_string());
                    }
                }
            }
        }
        if !outstanding.is_empty() {
            let mut index = repo.index()?;
            for path in &outstanding {
                index.add_path(Path::new(path))?;
                index.conflict_remove(Path::new(path))?;
            }
            index.write()?;
            outstanding = list_conflicted_paths()?;
            if !repo.index()?.has_conflicts() {
                outstanding.clear();
            }
        }
        if !outstanding.is_empty()
            && let Some(status) = maybe_auto_resolve_pending_conflicts(
                spec,
                &state,
                &outstanding,
                strategy,
                setting,
                agent,
            )
            .await?
        {
            return Ok(status);
        }
        display::info(format!(
            "Pending merge state: {:?}, index_conflicts={}, paths={outstanding:?}",
            repo.state(),
            repo.index().map(|idx| idx.has_conflicts()).unwrap_or(true)
        ));

        if repo.state() == RepositoryState::CherryPick {
            let current_index = applied_commits.len();
            let current_commit_oid = source_commits
                .get(current_index)
                .ok_or("replay state missing the in-progress plan commit")?;
            let cherry_message = repo
                .find_commit(*current_commit_oid)?
                .summary()
                .unwrap_or("Apply plan commit")
                .to_string();
            match vcs::commit_in_progress_cherry_pick_in(
                repo.path(),
                &cherry_message,
                expected_head,
            ) {
                Ok(new_head) => {
                    applied_commits.push(new_head);
                }
                Err(err) => {
                    if !outstanding.is_empty() {
                        display::warn("Merge conflicts remain:");
                        for path in &outstanding {
                            display::warn(format!("  - {path}"));
                        }
                        display::info(format!(
                            "index reports conflicts: {}",
                            repo.index().map(|idx| idx.has_conflicts()).unwrap_or(true)
                        ));
                        let status = repo
                            .workdir()
                            .map(|wd| vcs::status_with_branch(wd).unwrap_or_default())
                            .unwrap_or_default();
                        if !status.is_empty() {
                            display::info(format!(
                                "Repository status before blocking merge:\n{status}"
                            ));
                        }
                        display::info(format!(
                            "Resolve the conflicts above, stage the files, then rerun `vizier merge {} --complete-conflict`.",
                            spec.slug
                        ));
                        return Ok(PendingMergeStatus::Blocked(
                            PendingMergeBlocker::Conflicts { files: outstanding },
                        ));
                    }

                    let fallback = (|| -> Result<Oid, git2::Error> {
                        let mut index = repo.index()?;
                        index.write()?;
                        let tree_oid = index.write_tree()?;
                        let tree = repo.find_tree(tree_oid)?;
                        let sig = repo.signature()?;
                        let parent_commit = repo.find_commit(expected_head)?;
                        let oid = repo.commit(
                            Some("HEAD"),
                            &sig,
                            &sig,
                            &cherry_message,
                            &tree,
                            &[&parent_commit],
                        )?;
                        repo.cleanup_state().ok();
                        let mut checkout = CheckoutBuilder::new();
                        checkout.force();
                        repo.checkout_head(Some(&mut checkout))?;
                        Ok(oid)
                    })();

                    match fallback {
                        Ok(oid) => applied_commits.push(oid),
                        Err(fallback_err) => {
                            display::info(format!(
                                "fallback cherry-pick commit failed: {fallback_err}"
                            ));
                            return Err(Box::new(err));
                        }
                    }
                }
            }
        } else if !outstanding.is_empty() {
            display::warn("Merge conflicts remain:");
            for path in &outstanding {
                display::warn(format!("  - {path}"));
            }
            display::info(format!(
                "index reports conflicts: {}",
                repo.index().map(|idx| idx.has_conflicts()).unwrap_or(true)
            ));
            let status = repo
                .workdir()
                .map(|wd| vcs::status_with_branch(wd).unwrap_or_default())
                .unwrap_or_default();
            if !status.is_empty() {
                display::info(format!(
                    "Repository status before blocking merge:\n{status}"
                ));
            }
            display::info(format!(
                "Resolve the conflicts above, stage the files, then rerun `vizier merge {} --complete-conflict`.",
                spec.slug
            ));
            return Ok(PendingMergeStatus::Blocked(
                PendingMergeBlocker::Conflicts { files: outstanding },
            ));
        }

        let remaining_commits = if source_commits.len() > applied_commits.len() {
            source_commits[applied_commits.len()..].to_vec()
        } else {
            Vec::new()
        };
        let replay_mainline = replay.squash_mainline.or(state.squash_mainline);

        match apply_cherry_pick_sequence(
            applied_commits.last().copied().unwrap_or(start_oid),
            &remaining_commits,
            Some(git2::FileFavor::Ours),
            replay_mainline,
        )? {
            CherryPickOutcome::Completed(result) => {
                applied_commits.extend(result.applied);
            }
            CherryPickOutcome::Conflicted(conflict) => {
                let replay_state = MergeReplayState {
                    merge_base_oid: replay.merge_base_oid.clone(),
                    start_oid: replay.start_oid.clone(),
                    source_commits: replay.source_commits.clone(),
                    applied_commits: applied_commits.iter().map(|oid| oid.to_string()).collect(),
                    squash_mainline: state.squash_mainline,
                };
                let next_state = MergeConflictState {
                    slug: state.slug.clone(),
                    source_branch: state.source_branch.clone(),
                    target_branch: state.target_branch.clone(),
                    head_oid: replay.start_oid.clone(),
                    source_oid: state.source_oid.clone(),
                    merge_message: state.merge_message.clone(),
                    squash: true,
                    implementation_message: Some(implementation_message.to_string()),
                    replay: Some(replay_state),
                    squash_mainline: state.squash_mainline,
                };
                if let Some(status) = maybe_auto_resolve_pending_conflicts(
                    spec,
                    &next_state,
                    &conflict.files,
                    strategy,
                    setting,
                    agent,
                )
                .await?
                {
                    return Ok(status);
                }
                let state_path = write_conflict_state(&next_state)?;
                emit_conflict_instructions(&spec.slug, &conflict.files, &state_path);
                return Ok(PendingMergeStatus::Blocked(
                    PendingMergeBlocker::Conflicts {
                        files: conflict.files.clone(),
                    },
                ));
            }
        }

        let expected_head = applied_commits.last().copied().unwrap_or(start_oid);
        let _ = commit_soft_squash(implementation_message, start_oid, expected_head)?;
        let _ = clear_conflict_state(&spec.slug);
        display::info("Conflicts resolved; implementation commit created for squashed merge.");
        let source_oid = Oid::from_str(&state.source_oid)?;
        return Ok(PendingMergeStatus::SquashReady {
            source_oid,
            merge_message: state.merge_message.clone(),
        });
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
        if let Some(status) = maybe_auto_resolve_pending_conflicts(
            spec,
            &state,
            &outstanding,
            strategy,
            setting,
            agent,
        )
        .await?
        {
            return Ok(status);
        }
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
    if state.squash {
        let message = state
            .implementation_message
            .as_deref()
            .ok_or("missing implementation commit message for squashed merge state")?;
        let _ = commit_in_progress_squash(message, head_oid)?;
        let _ = clear_conflict_state(&spec.slug);
        display::info("Conflicts resolved; implementation commit created for squashed merge.");
        return Ok(PendingMergeStatus::SquashReady {
            source_oid,
            merge_message: state.merge_message.clone(),
        });
    }

    let merge_oid = commit_in_progress_merge(&state.merge_message, head_oid, source_oid)?;
    let _ = clear_conflict_state(&spec.slug);
    display::info("Conflicts resolved; finalizing merge now.");
    Ok(PendingMergeStatus::Ready {
        merge_oid,
        source_oid,
    })
}

async fn maybe_auto_resolve_pending_conflicts(
    spec: &plan::PlanBranchSpec,
    state: &MergeConflictState,
    files: &[String],
    strategy: MergeConflictStrategy,
    setting: ConflictAutoResolveSetting,
    agent: &config::AgentSettings,
) -> Result<Option<PendingMergeStatus>, Box<dyn std::error::Error>> {
    if files.is_empty() {
        return Ok(None);
    }

    if !matches!(strategy, MergeConflictStrategy::Agent) {
        display::warn(setting.status_line());
        return Ok(None);
    }

    display::warn(format!(
        "Auto-resolving merge conflicts via {}...",
        setting.source_description()
    ));
    match try_auto_resolve_conflicts(spec, state, files, agent).await {
        Ok(MergeConflictResolution::MergeCommitted {
            merge_oid,
            source_oid,
        }) => Ok(Some(PendingMergeStatus::Ready {
            merge_oid,
            source_oid,
        })),
        Ok(MergeConflictResolution::SquashImplementationCommitted { source_oid, .. }) => {
            Ok(Some(PendingMergeStatus::SquashReady {
                source_oid,
                merge_message: state.merge_message.clone(),
            }))
        }
        Err(err) => {
            display::warn(format!(
                "Backend auto-resolution failed: {err}. Falling back to manual resolution."
            ));
            let state_path = merge_conflict_state_path(&state.slug)?;
            emit_conflict_instructions(&state.slug, files, &state_path);
            Err(err)
        }
    }
}

struct MergeConflictInput<'a> {
    spec: &'a plan::PlanBranchSpec,
    merge_message: &'a str,
    conflict: vcs::MergeConflict,
    setting: ConflictAutoResolveSetting,
    strategy: MergeConflictStrategy,
    squash: bool,
    implementation_message: Option<&'a str>,
    agent: &'a config::AgentSettings,
}

async fn handle_merge_conflict(
    input: MergeConflictInput<'_>,
) -> Result<MergeConflictResolution, Box<dyn std::error::Error>> {
    let MergeConflictInput {
        spec,
        merge_message,
        conflict,
        setting,
        strategy,
        squash,
        implementation_message,
        agent,
    } = input;
    let files = conflict.files.clone();
    let state = MergeConflictState {
        slug: spec.slug.clone(),
        source_branch: spec.branch.clone(),
        target_branch: spec.target_branch.clone(),
        head_oid: conflict.head_oid.to_string(),
        source_oid: conflict.source_oid.to_string(),
        merge_message: merge_message.to_string(),
        squash,
        implementation_message: implementation_message.map(|s| s.to_string()),
        replay: None,
        squash_mainline: None,
    };

    let state_path = write_conflict_state(&state)?;
    match strategy {
        MergeConflictStrategy::Manual => {
            display::warn(setting.status_line());
            emit_conflict_instructions(&spec.slug, &files, &state_path);
            Err("merge blocked by conflicts; resolve them and rerun vizier merge".into())
        }
        MergeConflictStrategy::Agent => {
            display::warn(format!(
                "Auto-resolving merge conflicts via {}...",
                setting.source_description()
            ));
            match try_auto_resolve_conflicts(spec, &state, &files, agent).await {
                Ok(result) => Ok(result),
                Err(err) => {
                    display::warn(format!(
                        "Backend auto-resolution failed: {err}. Falling back to manual resolution."
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
) -> Result<MergeConflictResolution, Box<dyn std::error::Error>> {
    display::info("Attempting to resolve conflicts with the configured backend...");
    let prompt_agent = agent.for_prompt(config::PromptKind::MergeConflict)?;
    let selection = prompt_selection(&prompt_agent)?;
    let prompt = agent_prompt::build_merge_conflict_prompt(
        selection,
        &spec.target_branch,
        &spec.branch,
        files,
        &prompt_agent.documentation,
    )?;
    let repo_root = repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let request = build_agent_request(&prompt_agent, prompt, repo_root);

    let runner = Arc::clone(prompt_agent.agent_runner()?);
    let (progress_tx, progress_rx) = mpsc::channel(64);
    let progress_handle = spawn_plain_progress_logger(progress_rx);
    let result = runner
        .execute(request, Some(ProgressHook::Plain(progress_tx)))
        .await;
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
        return Err("conflicts remain after backend attempt".into());
    }

    let source_oid = Oid::from_str(&state.source_oid)?;
    if let Some(replay) = state.replay.as_ref() {
        let repo = Repository::discover(".")?;
        let implementation_message = state
            .implementation_message
            .as_deref()
            .ok_or("missing implementation commit message for squashed merge state")?;
        let start_oid = Oid::from_str(&replay.start_oid)?;
        let mut applied_commits: Vec<Oid> = replay
            .applied_commits
            .iter()
            .filter_map(|oid| Oid::from_str(oid).ok())
            .collect();
        let source_commits: Vec<Oid> = replay
            .source_commits
            .iter()
            .filter_map(|oid| Oid::from_str(oid).ok())
            .collect();

        let current_index = applied_commits.len();
        let current_commit_oid = source_commits
            .get(current_index)
            .ok_or("replay state missing the in-progress plan commit")?;
        let cherry_message = repo
            .find_commit(*current_commit_oid)?
            .summary()
            .unwrap_or("Apply plan commit")
            .to_string();
        let expected_parent = applied_commits.last().copied().unwrap_or(start_oid);
        let _ = commit_in_progress_cherry_pick(&cherry_message, expected_parent)?;

        let applied_head = Repository::discover(".")?.head()?.peel_to_commit()?.id();
        applied_commits.push(applied_head);

        let remaining_commits = if source_commits.len() > applied_commits.len() {
            source_commits[applied_commits.len()..].to_vec()
        } else {
            Vec::new()
        };
        let replay_mainline = replay.squash_mainline.or(state.squash_mainline);

        match apply_cherry_pick_sequence(
            applied_head,
            &remaining_commits,
            Some(git2::FileFavor::Ours),
            replay_mainline,
        )? {
            CherryPickOutcome::Completed(result) => {
                applied_commits.extend(result.applied);
            }
            CherryPickOutcome::Conflicted(next_conflict) => {
                let replay_state = MergeReplayState {
                    merge_base_oid: replay.merge_base_oid.clone(),
                    start_oid: replay.start_oid.clone(),
                    source_commits: replay.source_commits.clone(),
                    applied_commits: applied_commits.iter().map(|oid| oid.to_string()).collect(),
                    squash_mainline: state.squash_mainline,
                };
                let next_state = MergeConflictState {
                    slug: state.slug.clone(),
                    source_branch: state.source_branch.clone(),
                    target_branch: state.target_branch.clone(),
                    head_oid: replay.start_oid.clone(),
                    source_oid: state.source_oid.clone(),
                    merge_message: state.merge_message.clone(),
                    squash: true,
                    implementation_message: Some(implementation_message.to_string()),
                    replay: Some(replay_state),
                    squash_mainline: state.squash_mainline,
                };
                let state_path = write_conflict_state(&next_state)?;
                emit_conflict_instructions(&state.slug, &next_conflict.files, &state_path);
                return Err(
                    "merge blocked by conflicts; resolve them and rerun vizier merge".into(),
                );
            }
        }

        let expected_head = applied_commits.last().copied().unwrap_or(start_oid);
        let implementation_oid =
            commit_soft_squash(implementation_message, start_oid, expected_head)?;
        clear_conflict_state(&state.slug)?;
        display::info("Backend resolved the conflicts; implementation commit recorded.");
        return Ok(MergeConflictResolution::SquashImplementationCommitted {
            source_oid,
            implementation_oid,
        });
    }

    let head_oid = Oid::from_str(&state.head_oid)?;
    if state.squash {
        let message = state
            .implementation_message
            .as_deref()
            .ok_or("missing implementation commit message for squashed merge state")?;
        let implementation_oid = commit_in_progress_squash(message, head_oid)?;
        clear_conflict_state(&state.slug)?;
        display::info("Backend resolved the conflicts; implementation commit recorded.");
        Ok(MergeConflictResolution::SquashImplementationCommitted {
            source_oid,
            implementation_oid,
        })
    } else {
        let merge_oid = commit_in_progress_merge(&state.merge_message, head_oid, source_oid)?;
        clear_conflict_state(&state.slug)?;
        display::info("Backend resolved the conflicts; finalizing merge.");
        Ok(MergeConflictResolution::MergeCommitted {
            merge_oid,
            source_oid,
        })
    }
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
        let resolved = right.strip_prefix('\n').unwrap_or(right);
        output.push_str(resolved);

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
    }
    let serialized = serde_json::to_string_pretty(state)?;
    fs::write(&path, serialized)?;
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

fn cicd_gate_failure_error(script: &Path, result: &CicdScriptResult) -> Box<dyn std::error::Error> {
    Box::<dyn std::error::Error>::from(format!(
        "CI/CD gate `{}` failed ({})",
        script.display(),
        result.status_label()
    ))
}

async fn refresh_plan_branch(
    spec: &plan::PlanBranchSpec,
    plan_meta: &plan::PlanMetadata,
    worktree_path: &Path,
    push_after: bool,
    agent: &config::AgentSettings,
) -> Result<(), Box<dyn std::error::Error>> {
    let _cwd = WorkdirGuard::enter(worktree_path)?;
    let prompt_agent = agent.for_prompt(config::PromptKind::Documentation)?;

    let instruction = build_save_instruction_for_refresh(None);
    let system_prompt = agent_prompt::build_documentation_prompt(
        prompt_agent.prompt_selection(),
        &instruction,
        &prompt_agent.documentation,
    )
    .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;

    let response = Auditor::llm_request_with_tools(
        &prompt_agent,
        Some(config::PromptKind::Documentation),
        system_prompt,
        instruction,
        Some(worktree_path.to_path_buf()),
    )
    .await?;
    let audit_result = Auditor::commit_audit().await?;
    let session_path = audit_result.session_display();
    let (narrative_paths, narrative_summary) = narrative_change_set_for_commit(&audit_result);
    let mut allowed_paths = narrative_paths.clone();
    let plan_rel = spec.plan_rel_path();
    let plan_rel_string = plan_rel.to_string_lossy().replace('\\', "/");
    let plan_missing = !worktree_path.join(&plan_rel).exists();
    if plan_missing {
        allowed_paths.push(plan_rel_string.clone());
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

    stage_narrative_paths_for_commit(&narrative_paths)?;
    if plan_missing {
        vcs::stage_paths_allow_missing(&[plan_rel_string.as_str()])?;
    }
    trim_staged_vizier_paths_for_commit(&allowed_paths)?;
    let commit_oid = vcs::commit_staged(&commit_message, false)?;
    clear_narrative_tracker_for_commit(&narrative_paths);

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
    if !head.is_branch() {
        return Ok(None);
    }
    Ok(head.shorthand().map(|name| name.to_string()))
}

fn build_implementation_commit_message(
    spec: &plan::PlanBranchSpec,
    plan_meta: &plan::PlanMetadata,
) -> String {
    let mut sections = Vec::new();
    sections.push(format!("Target branch: {}", spec.target_branch));
    sections.push(format!("Plan branch: {}", spec.branch));
    sections.push(format!("Summary: {}", plan::summarize_spec(plan_meta)));

    format!("feat: apply plan {}\n\n{}", spec.slug, sections.join("\n"))
}

fn build_merge_commit_message(
    spec: &plan::PlanBranchSpec,
    _plan_meta: &plan::PlanMetadata,
    plan_document: Option<&str>,
    note: Option<&str>,
) -> String {
    // Merge commits now keep a concise subject line and embed the stored plan
    // document directly so reviewers see the same content the backend implemented.
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
