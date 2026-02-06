use std::collections::{BTreeMap, BTreeSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use chrono::Utc;
use git2::{BranchType, Repository};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use vizier_core::{
    agent_prompt,
    auditor::Auditor,
    config,
    display::{self, LogLevel},
    vcs::{
        add_worktree_for_branch, branch_exists, commit_paths_in_repo, create_branch_from,
        detect_primary_branch, remove_worktree,
    },
};

use crate::actions::shared::{
    WorkdirGuard, append_agent_rows, copy_session_log_to_repo_root, current_verbosity,
    format_block, format_table, prompt_selection, require_agent_backend,
};
use crate::actions::types::CommitMode;
use crate::cli::prompt::prompt_yes_no;
use crate::cli::scheduler::{
    background_config_snapshot, build_background_child_args, generate_job_id,
};
use crate::{jobs, plan};

const BUILD_PLAN_ROOT: &str = ".vizier/implementation-plans/builds";
const BUILD_EXECUTION_FILE: &str = "execution.json";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildFile {
    steps: Vec<BuildStep>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum BuildStep {
    Single(IntentDoc),
    Parallel(Vec<IntentDoc>),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentDoc {
    text: Option<String>,
    file: Option<String>,
}

#[derive(Debug, Clone)]
enum IntentSource {
    Text,
    File(PathBuf),
}

impl IntentSource {
    fn as_manifest_value(&self) -> String {
        match self {
            Self::Text => "text".to_string(),
            Self::File(path) => format!("file:{}", path.display()),
        }
    }
}

#[derive(Debug, Clone)]
struct ResolvedIntent {
    text: String,
    source: IntentSource,
}

#[derive(Debug, Clone)]
struct NormalizedStep {
    stage_index: usize,
    parallel_index: Option<usize>,
    step_key: String,
    slug: String,
    file_name: String,
    intent: ResolvedIntent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BuildManifest {
    build_id: String,
    created_at: String,
    target_branch: String,
    build_branch: String,
    input_file: ManifestInputFile,
    steps: Vec<ManifestStep>,
    artifacts: ManifestArtifacts,
    status: ManifestStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestInputFile {
    original_path: String,
    copied_path: String,
    digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestArtifacts {
    plan_docs: Vec<String>,
    summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestStep {
    step_key: String,
    stage_index: usize,
    parallel_index: Option<usize>,
    intent_source: String,
    intent_digest: String,
    output_plan_path: String,
    reads: Vec<ManifestPlanReference>,
    result: ManifestStepResult,
    summary: Option<String>,
    output_digest: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestPlanReference {
    step_key: String,
    plan_path: String,
    summary: String,
    digest: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ManifestStatus {
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ManifestStepResult {
    Pending,
    Succeeded,
    Failed,
}

impl ManifestStepResult {
    fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum BuildExecutionPipeline {
    Approve,
    ApproveReview,
    ApproveReviewMerge,
}

impl BuildExecutionPipeline {
    fn label(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::ApproveReview => "approve-review",
            Self::ApproveReviewMerge => "approve-review-merge",
        }
    }

    fn includes_review(self) -> bool {
        matches!(self, Self::ApproveReview | Self::ApproveReviewMerge)
    }

    fn includes_merge(self) -> bool {
        matches!(self, Self::ApproveReviewMerge)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BuildExecutionStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl BuildExecutionStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BuildExecutionState {
    build_id: String,
    pipeline: BuildExecutionPipeline,
    created_at: String,
    status: BuildExecutionStatus,
    steps: Vec<BuildExecutionStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BuildExecutionStep {
    step_key: String,
    stage_index: usize,
    build_plan_path: String,
    derived_slug: String,
    derived_branch: String,
    materialize_job_id: Option<String>,
    approve_job_id: Option<String>,
    review_job_id: Option<String>,
    merge_job_id: Option<String>,
    status: BuildExecutionStatus,
}

impl BuildExecutionStep {
    fn terminal_job_id(&self, pipeline: BuildExecutionPipeline) -> Option<&str> {
        match pipeline {
            BuildExecutionPipeline::Approve => self.approve_job_id.as_deref(),
            BuildExecutionPipeline::ApproveReview => self.review_job_id.as_deref(),
            BuildExecutionPipeline::ApproveReviewMerge => self.merge_job_id.as_deref(),
        }
    }
}

#[derive(Debug, Clone)]
struct BuildSession {
    build_id: String,
    build_branch: String,
    execution_rel: PathBuf,
    manifest: BuildManifest,
}

pub(crate) async fn run_build(
    build_file: PathBuf,
    name_override: Option<String>,
    project_root: &Path,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
) -> Result<(), Box<dyn std::error::Error>> {
    require_agent_backend(
        agent,
        config::PromptKind::ImplementationPlan,
        "vizier build requires an agent-capable selector; update [agents.draft] or pass --agent codex|gemini",
    )?;

    let build_path = std::fs::canonicalize(&build_file)
        .map_err(|err| format!("unable to read build file {}: {err}", build_file.display()))?;
    let build_dir = build_path
        .parent()
        .ok_or("build file path must have a parent directory")?;
    let repo_root = std::fs::canonicalize(project_root)?;

    let _repo_guard = WorkdirGuard::enter(&repo_root)?;

    let (parsed, build_contents) = parse_build_file(&build_path)?;
    let resolved_steps = resolve_steps(parsed, build_dir, &repo_root)?;
    let normalized_steps = normalize_steps(&resolved_steps);
    if normalized_steps.is_empty() {
        return Err("build file steps must be non-empty".into());
    }

    let target_branch = detect_primary_branch()
        .ok_or("unable to detect a primary branch (tried origin/HEAD, main, master)")?;
    let build_id = allocate_build_id(
        &build_path,
        &build_contents,
        &repo_root,
        name_override.as_deref(),
    )?;
    let build_branch = format!("build/{build_id}");

    let tmp_root = repo_root.join(".vizier/tmp-worktrees");
    std::fs::create_dir_all(&tmp_root)?;
    let worktree_name = format!("vizier-build-{build_id}");
    let worktree_path = tmp_root.join(format!("build-{build_id}"));

    create_branch_from(&target_branch, &build_branch).map_err(
        |err| -> Box<dyn std::error::Error> {
            Box::from(format!(
                "create_branch_from({}<-{}): {}",
                build_branch, target_branch, err
            ))
        },
    )?;

    add_worktree_for_branch(&worktree_name, &worktree_path, &build_branch).map_err(
        |err| -> Box<dyn std::error::Error> {
            Box::from(format!(
                "add_worktree({}, {}): {}",
                worktree_name,
                worktree_path.display(),
                err
            ))
        },
    )?;
    jobs::record_current_job_worktree(&repo_root, Some(&worktree_name), &worktree_path);

    let session_rel = Path::new(BUILD_PLAN_ROOT).join(&build_id);
    let input_name = build_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("build.toml");
    let input_rel = session_rel.join("input").join(input_name);
    let plans_rel_root = session_rel.join("plans");
    let manifest_rel = session_rel.join("manifest.json");
    let summary_rel = session_rel.join("summary.md");

    let mut artifact_paths = vec![input_rel.clone(), manifest_rel.clone(), summary_rel.clone()];
    for step in &normalized_steps {
        artifact_paths.push(plans_rel_root.join(&step.file_name));
    }

    let mut failed_step_key: Option<String> = None;
    let mut failure_reason: Option<String> = None;
    let mut committed = false;

    let final_manifest: Option<BuildManifest> = {
        let _worktree_guard = WorkdirGuard::enter(&worktree_path)?;

        let session_abs = worktree_path.join(&session_rel);
        let input_abs = worktree_path.join(&input_rel);
        let plans_abs = worktree_path.join(&plans_rel_root);
        let manifest_abs = worktree_path.join(&manifest_rel);
        let summary_abs = worktree_path.join(&summary_rel);

        if let Some(parent) = input_abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir_all(&plans_abs)?;

        std::fs::write(&input_abs, &build_contents)?;

        let input_digest = digest_hex(build_contents.as_bytes());
        let mut manifest = build_manifest_template(
            BuildManifestInit {
                build_id: &build_id,
                target_branch: &target_branch,
                build_branch: &build_branch,
                input_original: &build_path,
                input_copied: &input_rel,
                input_digest: &input_digest,
                plans_rel_root: &plans_rel_root,
            },
            &normalized_steps,
        );
        write_manifest(&manifest_abs, &manifest)?;

        let prompt_agent = agent.for_prompt(config::PromptKind::ImplementationPlan)?;
        let selection = prompt_selection(&prompt_agent)?;

        display::emit(
            LogLevel::Info,
            format!(
                "[build] session={} branch={} manifest={}",
                build_id,
                build_branch,
                manifest_rel.display()
            ),
        );

        for (step_idx, step) in normalized_steps.iter().enumerate() {
            let reads = prior_stage_refs(&manifest, step.stage_index);
            manifest.steps[step_idx].reads = reads.clone();
            if let Err(err) = write_manifest(&manifest_abs, &manifest) {
                let message = format!("write manifest before step {}: {err}", step.step_key);
                mark_step_failed(&mut manifest, step_idx, &message);
                failure_reason = Some(message);
                failed_step_key = Some(step.step_key.clone());
                let _ = write_manifest(&manifest_abs, &manifest);
                break;
            }

            let output_rel = plans_rel_root.join(&step.file_name);
            let output_abs = worktree_path.join(&output_rel);

            let prompt_refs = reads
                .iter()
                .map(|entry| agent_prompt::BuildPlanReference {
                    step_key: entry.step_key.clone(),
                    plan_path: entry.plan_path.clone(),
                    summary: entry.summary.clone(),
                    digest: entry.digest.clone(),
                })
                .collect::<Vec<_>>();

            let prompt = match agent_prompt::build_build_implementation_plan_prompt(
                selection,
                agent_prompt::BuildPlanPromptInput {
                    build_id: &build_id,
                    build_branch: &build_branch,
                    manifest_path: &to_repo_string(&manifest_rel),
                    step_key: &step.step_key,
                    stage_index: step.stage_index,
                    parallel_index: step.parallel_index,
                    output_plan_path: &to_repo_string(&output_rel),
                    intent_text: &step.intent.text,
                    references: &prompt_refs,
                    documentation: &prompt_agent.documentation,
                },
            ) {
                Ok(value) => value,
                Err(err) => {
                    let message = format!(
                        "build prompt assembly failed for step {}: {}",
                        step.step_key, err
                    );
                    mark_step_failed(&mut manifest, step_idx, &message);
                    failure_reason = Some(message);
                    failed_step_key = Some(step.step_key.clone());
                    let _ = write_manifest(&manifest_abs, &manifest);
                    break;
                }
            };

            display::emit(
                LogLevel::Info,
                format!(
                    "[build] step={} status=running plan={}",
                    step.step_key,
                    output_rel.display()
                ),
            );

            let llm_response = match Auditor::llm_request_with_tools(
                &prompt_agent,
                Some(config::PromptKind::ImplementationPlan),
                prompt,
                step.intent.text.clone(),
                Some(worktree_path.clone()),
            )
            .await
            {
                Ok(response) => response,
                Err(err) => {
                    let message = format!("Agent backend for step {}: {}", step.step_key, err);
                    mark_step_failed(&mut manifest, step_idx, &message);
                    failure_reason = Some(message);
                    failed_step_key = Some(step.step_key.clone());
                    let _ = write_manifest(&manifest_abs, &manifest);
                    break;
                }
            };

            let plan_body = llm_response.content;
            let plan_doc = render_build_plan_document(step, &build_branch, &plan_body);
            if let Err(err) = std::fs::write(&output_abs, &plan_doc) {
                let message = format!(
                    "write plan for step {} to {}: {}",
                    step.step_key,
                    output_rel.display(),
                    err
                );
                mark_step_failed(&mut manifest, step_idx, &message);
                failure_reason = Some(message);
                failed_step_key = Some(step.step_key.clone());
                let _ = write_manifest(&manifest_abs, &manifest);
                break;
            }

            let summary = summarize_text(&plan_body);
            let output_digest = digest_hex(plan_doc.as_bytes());
            if let Some(step_entry) = manifest.steps.get_mut(step_idx) {
                step_entry.result = ManifestStepResult::Succeeded;
                step_entry.summary = Some(summary.clone());
                step_entry.output_digest = Some(output_digest);
                step_entry.error = None;
            }

            if !manifest
                .artifacts
                .plan_docs
                .iter()
                .any(|entry| entry == &to_repo_string(&output_rel))
            {
                manifest
                    .artifacts
                    .plan_docs
                    .push(to_repo_string(&output_rel));
            }

            if let Err(err) = write_manifest(&manifest_abs, &manifest) {
                let message = format!("write manifest after step {}: {err}", step.step_key);
                mark_step_failed(&mut manifest, step_idx, &message);
                failure_reason = Some(message);
                failed_step_key = Some(step.step_key.clone());
                let _ = write_manifest(&manifest_abs, &manifest);
                break;
            }

            display::emit(
                LogLevel::Info,
                format!(
                    "[build] step={} status=succeeded plan={}",
                    step.step_key,
                    output_rel.display()
                ),
            );
        }

        manifest.status = if failure_reason.is_some() {
            ManifestStatus::Failed
        } else {
            ManifestStatus::Succeeded
        };

        let summary_document = render_session_summary(&manifest);
        if let Err(err) = std::fs::write(&summary_abs, summary_document) {
            let message = format!("write session summary {}: {err}", summary_rel.display());
            if failure_reason.is_none() {
                failure_reason = Some(message);
            }
            manifest.status = ManifestStatus::Failed;
        } else {
            manifest.artifacts.summary = Some(to_repo_string(&summary_rel));
        }

        if let Err(err) = write_manifest(&manifest_abs, &manifest) {
            let message = format!("write final manifest {}: {err}", manifest_rel.display());
            if failure_reason.is_none() {
                failure_reason = Some(message);
            }
            manifest.status = ManifestStatus::Failed;
        }

        if commit_mode.should_commit() {
            let commit_message = if manifest.status == ManifestStatus::Succeeded {
                format!("docs: add build session {}", build_id)
            } else {
                format!("docs: record failed build session {}", build_id)
            };
            match commit_session_artifacts(&worktree_path, &artifact_paths, &commit_message) {
                Ok(()) => {
                    committed = true;
                }
                Err(err) => {
                    let message = format!("commit build session artifacts: {err}");
                    if failure_reason.is_none() {
                        failure_reason = Some(message);
                    }
                    manifest.status = ManifestStatus::Failed;
                    let _ = write_manifest(&manifest_abs, &manifest);
                }
            }
        }

        if failure_reason.is_some() && failed_step_key.is_none() {
            failed_step_key = manifest
                .steps
                .iter()
                .find(|step| step.result == ManifestStepResult::Failed)
                .map(|step| step.step_key.clone());
        }

        if !session_abs.exists() {
            let message = format!(
                "build session path missing unexpectedly: {}",
                session_rel.display()
            );
            if failure_reason.is_none() {
                failure_reason = Some(message);
            }
        }

        Some(manifest)
    };

    if let Some(artifact) = Auditor::persist_session_log() {
        copy_session_log_to_repo_root(&repo_root, &artifact);
        Auditor::clear_messages();
    }

    if failure_reason.is_none() && commit_mode.should_commit() {
        cleanup_worktree(&worktree_name, &worktree_path);
    }

    let mut rows = Vec::new();
    let outcome = if failure_reason.is_none() {
        if commit_mode.should_commit() {
            "Build session ready"
        } else {
            "Build session pending (manual commit)"
        }
    } else {
        "Build session failed"
    };
    rows.push(("Outcome".to_string(), outcome.to_string()));
    rows.push(("Build".to_string(), build_id.clone()));
    rows.push(("Branch".to_string(), build_branch.clone()));
    rows.push(("Manifest".to_string(), to_repo_string(&manifest_rel)));

    if !commit_mode.should_commit() || failure_reason.is_some() {
        rows.push(("Worktree".to_string(), worktree_path.display().to_string()));
    }

    if let Some(step_key) = failed_step_key.as_ref() {
        rows.push(("Failed step".to_string(), step_key.clone()));
    }

    append_agent_rows(&mut rows, current_verbosity());
    println!("{}", format_block(rows));

    if let Some(manifest) = final_manifest.as_ref() {
        let mut table = vec![vec![
            "Step".to_string(),
            "Status".to_string(),
            "Plan".to_string(),
            "Reads".to_string(),
        ]];

        for step in &manifest.steps {
            table.push(vec![
                step.step_key.clone(),
                step.result.label().to_string(),
                step.output_plan_path.clone(),
                step.reads.len().to_string(),
            ]);
        }

        println!("Steps:");
        println!("{}", format_table(&table, 2));
    }

    if let Some(reason) = failure_reason {
        display::emit(
            LogLevel::Error,
            format!(
                "Build session artifacts preserved on {} (worktree {}).",
                build_branch,
                worktree_path.display()
            ),
        );
        return Err(reason.into());
    }

    if !commit_mode.should_commit() {
        display::info(format!(
            "Build session generated with --no-commit; inspect and commit in {}",
            worktree_path.display()
        ));
    }

    if committed {
        display::info(format!(
            "Build session artifacts committed on {}; inspect with `git checkout {}`",
            build_branch, build_branch
        ));
    }

    Ok(())
}

pub(crate) async fn run_build_execute(
    build_id: String,
    pipeline: BuildExecutionPipeline,
    resume: bool,
    assume_yes: bool,
    follow: bool,
    project_root: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let repo_root = std::fs::canonicalize(project_root)?;
    let _repo_guard = WorkdirGuard::enter(&repo_root)?;

    if !assume_yes {
        if !std::io::stdin().is_terminal() {
            return Err("vizier build execute requires --yes in non-interactive mode".into());
        }
        let confirmed = prompt_yes_no(&format!(
            "Queue build execution {} with pipeline {}?",
            build_id,
            pipeline.label()
        ))?;
        if !confirmed {
            return Err("aborted by user".into());
        }
    }

    let session = load_build_session(&repo_root, &build_id)?;
    if session.manifest.status != ManifestStatus::Succeeded {
        return Err(format!(
            "build session {} status is {}; only succeeded sessions can execute",
            build_id,
            session.manifest.status.as_label()
        )
        .into());
    }

    let repo = Repository::discover(&repo_root)?;
    for step in &session.manifest.steps {
        let plan_path = Path::new(&step.output_plan_path);
        if !branch_file_exists(&repo, &session.build_branch, plan_path)? {
            return Err(format!(
                "build step {} is missing plan artifact {} on {}",
                step.step_key, step.output_plan_path, session.build_branch
            )
            .into());
        }
    }

    let existing_state = load_execution_state(&repo_root, &session)?;
    if resume && existing_state.is_none() {
        return Err(format!(
            "build execution state not found for {}; run without --resume first",
            build_id
        )
        .into());
    }
    if !resume && existing_state.is_some() {
        return Err(format!(
            "build execution already exists for {}; rerun with --resume",
            build_id
        )
        .into());
    }

    let mut execution = reconcile_execution_state(
        existing_state,
        &session,
        pipeline,
        &repo_root,
        Utc::now().to_rfc3339(),
    )?;

    let jobs_root = jobs::ensure_jobs_root(&repo_root)?;
    let mut stage_terminal: BTreeMap<usize, Vec<jobs::JobArtifact>> = BTreeMap::new();
    let mut queued_job_ids = Vec::new();
    let mut resumed_job_ids = Vec::new();
    let config_snapshot = background_config_snapshot(&config::get_config());

    for step in &mut execution.steps {
        let plan_doc_artifact = jobs::JobArtifact::PlanDoc {
            slug: step.derived_slug.clone(),
            branch: step.derived_branch.clone(),
        };
        let plan_branch_artifact = jobs::JobArtifact::PlanBranch {
            slug: step.derived_slug.clone(),
            branch: step.derived_branch.clone(),
        };

        let mut prior_stage_dependencies = Vec::new();
        for (stage_idx, artifacts) in &stage_terminal {
            if *stage_idx < step.stage_index {
                prior_stage_dependencies.extend(artifacts.clone());
            }
        }

        let materialize_schedule = jobs::JobSchedule {
            after: Vec::new(),
            dependencies: prior_stage_dependencies
                .iter()
                .cloned()
                .map(|artifact| jobs::JobDependency { artifact })
                .collect(),
            locks: vec![
                jobs::JobLock {
                    key: format!("branch:{}", step.derived_branch),
                    mode: jobs::LockMode::Exclusive,
                },
                jobs::JobLock {
                    key: format!("temp_worktree:build-materialize-{}", step.step_key),
                    mode: jobs::LockMode::Exclusive,
                },
            ],
            artifacts: Vec::new(),
            pinned_head: None,
            wait_reason: None,
            waited_on: Vec::new(),
        };
        let materialize_args = vec![
            "build".to_string(),
            "__materialize".to_string(),
            build_id.clone(),
            "--step".to_string(),
            step.step_key.clone(),
            "--slug".to_string(),
            step.derived_slug.clone(),
            "--branch".to_string(),
            step.derived_branch.clone(),
        ];
        let materialize_job = ensure_phase_job(
            &repo_root,
            &jobs_root,
            &mut step.materialize_job_id,
            &materialize_args,
            jobs::JobMetadata {
                scope: Some("build_materialize".to_string()),
                plan: Some(step.derived_slug.clone()),
                branch: Some(step.derived_branch.clone()),
                target: Some(session.manifest.target_branch.clone()),
                ..Default::default()
            },
            materialize_schedule,
            &config_snapshot,
        )?;
        if materialize_job.enqueued {
            queued_job_ids.push(materialize_job.job_id.clone());
        } else {
            resumed_job_ids.push(materialize_job.job_id.clone());
        }
        let materialize_completion = phase_completion_artifact(&materialize_job.job_id);

        let materialize_artifacts = vec![
            plan_branch_artifact.clone(),
            plan_doc_artifact.clone(),
            materialize_completion.clone(),
        ];
        jobs::update_job_record(&jobs_root, &materialize_job.job_id, |record| {
            if let Some(schedule) = record.schedule.as_mut() {
                schedule.artifacts = materialize_artifacts.clone();
            } else {
                record.schedule = Some(jobs::JobSchedule {
                    after: Vec::new(),
                    dependencies: Vec::new(),
                    locks: Vec::new(),
                    artifacts: materialize_artifacts.clone(),
                    pinned_head: None,
                    wait_reason: None,
                    waited_on: Vec::new(),
                });
            }
        })?;

        let approve_schedule = jobs::JobSchedule {
            after: Vec::new(),
            dependencies: vec![
                jobs::JobDependency {
                    artifact: materialize_completion.clone(),
                },
                jobs::JobDependency {
                    artifact: plan_doc_artifact.clone(),
                },
            ],
            locks: vec![
                jobs::JobLock {
                    key: format!("branch:{}", step.derived_branch),
                    mode: jobs::LockMode::Exclusive,
                },
                jobs::JobLock {
                    key: format!("temp_worktree:build-approve-{}", step.step_key),
                    mode: jobs::LockMode::Exclusive,
                },
            ],
            artifacts: vec![jobs::JobArtifact::PlanCommits {
                slug: step.derived_slug.clone(),
                branch: step.derived_branch.clone(),
            }],
            pinned_head: None,
            wait_reason: None,
            waited_on: Vec::new(),
        };
        let approve_args = vec![
            "approve".to_string(),
            step.derived_slug.clone(),
            "--branch".to_string(),
            step.derived_branch.clone(),
            "--target".to_string(),
            session.manifest.target_branch.clone(),
            "--yes".to_string(),
        ];
        let approve_job = ensure_phase_job(
            &repo_root,
            &jobs_root,
            &mut step.approve_job_id,
            &approve_args,
            jobs::JobMetadata {
                scope: Some("approve".to_string()),
                plan: Some(step.derived_slug.clone()),
                branch: Some(step.derived_branch.clone()),
                target: Some(session.manifest.target_branch.clone()),
                ..Default::default()
            },
            approve_schedule,
            &config_snapshot,
        )?;
        if approve_job.enqueued {
            queued_job_ids.push(approve_job.job_id.clone());
        } else {
            resumed_job_ids.push(approve_job.job_id.clone());
        }
        let approve_completion = phase_completion_artifact(&approve_job.job_id);
        jobs::update_job_record(&jobs_root, &approve_job.job_id, |record| {
            if let Some(schedule) = record.schedule.as_mut()
                && !schedule.artifacts.iter().any(|artifact| {
                    matches!(artifact, jobs::JobArtifact::AskSavePatch { job_id } if job_id == &approve_job.job_id)
                })
            {
                schedule.artifacts.push(approve_completion.clone());
            }
        })?;

        let mut terminal_artifact = approve_completion.clone();

        if pipeline.includes_review() {
            let review_schedule = jobs::JobSchedule {
                after: Vec::new(),
                dependencies: vec![
                    jobs::JobDependency {
                        artifact: approve_completion.clone(),
                    },
                    jobs::JobDependency {
                        artifact: plan_branch_artifact.clone(),
                    },
                    jobs::JobDependency {
                        artifact: plan_doc_artifact.clone(),
                    },
                ],
                locks: vec![
                    jobs::JobLock {
                        key: format!("branch:{}", step.derived_branch),
                        mode: jobs::LockMode::Exclusive,
                    },
                    jobs::JobLock {
                        key: format!("temp_worktree:build-review-{}", step.step_key),
                        mode: jobs::LockMode::Exclusive,
                    },
                ],
                artifacts: vec![jobs::JobArtifact::PlanCommits {
                    slug: step.derived_slug.clone(),
                    branch: step.derived_branch.clone(),
                }],
                pinned_head: None,
                wait_reason: None,
                waited_on: Vec::new(),
            };
            let review_args = vec![
                "review".to_string(),
                step.derived_slug.clone(),
                "--branch".to_string(),
                step.derived_branch.clone(),
                "--target".to_string(),
                session.manifest.target_branch.clone(),
                "--yes".to_string(),
            ];
            let review_job = ensure_phase_job(
                &repo_root,
                &jobs_root,
                &mut step.review_job_id,
                &review_args,
                jobs::JobMetadata {
                    scope: Some("review".to_string()),
                    plan: Some(step.derived_slug.clone()),
                    branch: Some(step.derived_branch.clone()),
                    target: Some(session.manifest.target_branch.clone()),
                    ..Default::default()
                },
                review_schedule,
                &config_snapshot,
            )?;
            if review_job.enqueued {
                queued_job_ids.push(review_job.job_id.clone());
            } else {
                resumed_job_ids.push(review_job.job_id.clone());
            }
            let review_completion = phase_completion_artifact(&review_job.job_id);
            jobs::update_job_record(&jobs_root, &review_job.job_id, |record| {
                if let Some(schedule) = record.schedule.as_mut()
                    && !schedule.artifacts.iter().any(|artifact| {
                        matches!(artifact, jobs::JobArtifact::AskSavePatch { job_id } if job_id == &review_job.job_id)
                    })
                {
                    schedule.artifacts.push(review_completion.clone());
                }
            })?;
            terminal_artifact = review_completion.clone();
        }

        if pipeline.includes_merge() {
            let review_id = step
                .review_job_id
                .as_deref()
                .ok_or_else(|| format!("missing review job id for step {}", step.step_key))?;
            let merge_dependencies = vec![
                jobs::JobDependency {
                    artifact: phase_completion_artifact(review_id),
                },
                jobs::JobDependency {
                    artifact: plan_branch_artifact.clone(),
                },
            ];
            let merge_schedule = jobs::JobSchedule {
                after: Vec::new(),
                dependencies: merge_dependencies,
                locks: vec![
                    jobs::JobLock {
                        key: format!("branch:{}", session.manifest.target_branch),
                        mode: jobs::LockMode::Exclusive,
                    },
                    jobs::JobLock {
                        key: format!("branch:{}", step.derived_branch),
                        mode: jobs::LockMode::Exclusive,
                    },
                    jobs::JobLock {
                        key: format!("merge_sentinel:{}", step.derived_slug),
                        mode: jobs::LockMode::Exclusive,
                    },
                ],
                artifacts: vec![jobs::JobArtifact::TargetBranch {
                    name: session.manifest.target_branch.clone(),
                }],
                pinned_head: None,
                wait_reason: None,
                waited_on: Vec::new(),
            };
            let merge_args = vec![
                "merge".to_string(),
                step.derived_slug.clone(),
                "--branch".to_string(),
                step.derived_branch.clone(),
                "--target".to_string(),
                session.manifest.target_branch.clone(),
                "--yes".to_string(),
            ];
            let merge_job = ensure_phase_job(
                &repo_root,
                &jobs_root,
                &mut step.merge_job_id,
                &merge_args,
                jobs::JobMetadata {
                    scope: Some("merge".to_string()),
                    plan: Some(step.derived_slug.clone()),
                    branch: Some(step.derived_branch.clone()),
                    target: Some(session.manifest.target_branch.clone()),
                    ..Default::default()
                },
                merge_schedule,
                &config_snapshot,
            )?;
            if merge_job.enqueued {
                queued_job_ids.push(merge_job.job_id.clone());
            } else {
                resumed_job_ids.push(merge_job.job_id.clone());
            }
            let merge_completion = phase_completion_artifact(&merge_job.job_id);
            jobs::update_job_record(&jobs_root, &merge_job.job_id, |record| {
                if let Some(schedule) = record.schedule.as_mut()
                    && !schedule.artifacts.iter().any(|artifact| {
                        matches!(artifact, jobs::JobArtifact::AskSavePatch { job_id } if job_id == &merge_job.job_id)
                    })
                {
                    schedule.artifacts.push(merge_completion.clone());
                }
            })?;
            terminal_artifact = merge_completion;
        }

        stage_terminal
            .entry(step.stage_index)
            .or_default()
            .push(terminal_artifact);
    }

    let binary = std::env::current_exe()?;
    jobs::scheduler_tick(&repo_root, &jobs_root, &binary)
        .map_err(|err| format!("unable to advance scheduler after build execute enqueue: {err}"))?;

    execution.status = derive_execution_status(&execution, pipeline, &jobs_root);
    for step in &mut execution.steps {
        step.status = derive_step_status(step, pipeline, &jobs_root);
    }
    persist_execution_state(&repo_root, &session, &execution)?;

    let mut rows = Vec::new();
    rows.push((
        "Outcome".to_string(),
        if resume {
            "Build execution resumed".to_string()
        } else {
            "Build execution queued".to_string()
        },
    ));
    rows.push(("Build".to_string(), build_id.clone()));
    rows.push(("Pipeline".to_string(), pipeline.label().to_string()));
    rows.push((
        "Execution manifest".to_string(),
        to_repo_string(&session.execution_rel),
    ));
    rows.push(("Status".to_string(), execution.status.label().to_string()));
    if !queued_job_ids.is_empty() {
        rows.push(("Queued jobs".to_string(), queued_job_ids.join(",")));
    }
    if !resumed_job_ids.is_empty() {
        rows.push(("Reused jobs".to_string(), resumed_job_ids.join(",")));
    }
    if let Some(step_key) = first_failed_step(&execution, pipeline, &jobs_root) {
        rows.push(("Failed step".to_string(), step_key));
    }
    append_agent_rows(&mut rows, current_verbosity());
    println!("{}", format_block(rows));

    let mut table = vec![vec![
        "Step".to_string(),
        "Slug".to_string(),
        "Branch".to_string(),
        "Jobs".to_string(),
        "Status".to_string(),
    ]];
    for step in &execution.steps {
        table.push(vec![
            step.step_key.clone(),
            step.derived_slug.clone(),
            step.derived_branch.clone(),
            render_phase_jobs(step, pipeline),
            derive_step_status(step, pipeline, &jobs_root)
                .label()
                .to_string(),
        ]);
    }
    println!("Steps:");
    println!("{}", format_table(&table, 2));

    if follow {
        let follow_job = queued_job_ids
            .first()
            .cloned()
            .or_else(|| resumed_job_ids.first().cloned());
        if let Some(job_id) = follow_job {
            display::info(format!("Following job logs for {job_id}"));
            let exit = jobs::follow_job_logs_raw(&jobs_root, &job_id)?;
            if exit != 0 {
                return Err(format!("followed job {} exited with code {}", job_id, exit).into());
            }
        }
    }

    Ok(())
}

pub(crate) async fn run_build_materialize(
    build_id: String,
    step_key: String,
    slug: String,
    branch: String,
    project_root: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let repo_root = std::fs::canonicalize(project_root)?;
    let _repo_guard = WorkdirGuard::enter(&repo_root)?;
    let session = load_build_session(&repo_root, &build_id)?;

    let step = session
        .manifest
        .steps
        .iter()
        .find(|entry| entry.step_key == step_key)
        .ok_or_else(|| {
            format!(
                "build session {} missing step {} in manifest",
                build_id, step_key
            )
        })?;

    let repo = Repository::discover(&repo_root)?;
    let build_plan_text = read_branch_file(
        &repo,
        &session.build_branch,
        Path::new(&step.output_plan_path),
    )?;
    let materialized_doc = rewrite_plan_front_matter(&build_plan_text, &slug, &branch);

    if !branch_exists(&branch).map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })? {
        create_branch_from(&session.manifest.target_branch, &branch).map_err(
            |err| -> Box<dyn std::error::Error> {
                Box::from(format!(
                    "create_branch_from({}<-{}): {}",
                    branch, session.manifest.target_branch, err
                ))
            },
        )?;
    }

    let worktree = plan::PlanWorktree::create(&slug, &branch, "build-materialize")?;
    let plan_rel = plan::plan_rel_path(&slug);
    let plan_abs = worktree.path().join(&plan_rel);
    let materialize_result = (|| -> Result<bool, Box<dyn std::error::Error>> {
        let existing = std::fs::read_to_string(&plan_abs).ok();
        if existing.as_deref() == Some(materialized_doc.as_str()) {
            return Ok(false);
        }
        plan::write_plan_file(&plan_abs, &materialized_doc)?;
        commit_paths_in_repo(
            worktree.path(),
            &[plan_rel.as_path()],
            &format!("docs: materialize build step {} ({})", step_key, slug),
        )
        .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
        Ok(true)
    })();

    if let Err(err) = worktree.cleanup() {
        display::warn(format!(
            "temporary materialize worktree cleanup failed: {}",
            err
        ));
    }

    let committed = materialize_result?;
    let mut rows = Vec::new();
    rows.push((
        "Outcome".to_string(),
        if committed {
            "Build step materialized".to_string()
        } else {
            "Build step already materialized".to_string()
        },
    ));
    rows.push(("Build".to_string(), build_id));
    rows.push(("Step".to_string(), step_key));
    rows.push(("Plan".to_string(), to_repo_string(&plan_rel)));
    rows.push(("Branch".to_string(), branch));
    println!("{}", format_block(rows));

    Ok(())
}

#[derive(Debug, Clone)]
struct PhaseJobOutcome {
    job_id: String,
    enqueued: bool,
}

fn load_build_session(
    repo_root: &Path,
    build_id: &str,
) -> Result<BuildSession, Box<dyn std::error::Error>> {
    let repo = Repository::discover(repo_root)?;
    let build_branch = format!("build/{build_id}");
    repo.find_branch(&build_branch, BranchType::Local)
        .map_err(|_| format!("build session branch {} not found", build_branch))?;

    let session_rel = Path::new(BUILD_PLAN_ROOT).join(build_id);
    let manifest_rel = session_rel.join("manifest.json");
    let execution_rel = session_rel.join(BUILD_EXECUTION_FILE);
    let manifest_text = read_branch_file(&repo, &build_branch, &manifest_rel)?;
    let manifest: BuildManifest = serde_json::from_str(&manifest_text).map_err(|err| {
        format!(
            "unable to parse build manifest {} on {}: {}",
            manifest_rel.display(),
            build_branch,
            err
        )
    })?;

    Ok(BuildSession {
        build_id: build_id.to_string(),
        build_branch,
        execution_rel,
        manifest,
    })
}

fn load_execution_state(
    repo_root: &Path,
    session: &BuildSession,
) -> Result<Option<BuildExecutionState>, Box<dyn std::error::Error>> {
    let repo = Repository::discover(repo_root)?;
    let maybe_text = try_read_branch_file(&repo, &session.build_branch, &session.execution_rel)?;
    let Some(text) = maybe_text else {
        return Ok(None);
    };

    let state: BuildExecutionState = serde_json::from_str(&text).map_err(|err| {
        format!(
            "unable to parse execution state {} on {}: {}",
            session.execution_rel.display(),
            session.build_branch,
            err
        )
    })?;
    Ok(Some(state))
}

fn reconcile_execution_state(
    existing: Option<BuildExecutionState>,
    session: &BuildSession,
    pipeline: BuildExecutionPipeline,
    repo_root: &Path,
    created_at: String,
) -> Result<BuildExecutionState, Box<dyn std::error::Error>> {
    let mut used_slugs = BTreeSet::new();
    if let Some(state) = existing.as_ref() {
        for step in &state.steps {
            used_slugs.insert(step.derived_slug.clone());
        }
    }

    let mut steps = Vec::with_capacity(session.manifest.steps.len());
    if let Some(state) = existing {
        if state.build_id != session.build_id {
            return Err(format!(
                "execution state build id mismatch: expected {}, found {}",
                session.build_id, state.build_id
            )
            .into());
        }
        if state.pipeline != pipeline {
            return Err(format!(
                "execution pipeline mismatch: existing {}, requested {}",
                state.pipeline.label(),
                pipeline.label()
            )
            .into());
        }

        let mut by_key = BTreeMap::new();
        for step in state.steps {
            by_key.insert(step.step_key.clone(), step);
        }

        for manifest_step in &session.manifest.steps {
            if let Some(mut existing_step) = by_key.remove(&manifest_step.step_key) {
                existing_step.stage_index = manifest_step.stage_index;
                existing_step.build_plan_path = manifest_step.output_plan_path.clone();
                steps.push(existing_step);
                continue;
            }

            let new_step = build_execution_step_from_manifest(
                manifest_step,
                &session.build_id,
                repo_root,
                &mut used_slugs,
            )?;
            steps.push(new_step);
        }

        if !by_key.is_empty() {
            let dangling = by_key.keys().cloned().collect::<Vec<_>>().join(", ");
            return Err(format!(
                "execution state has steps not present in manifest: {}",
                dangling
            )
            .into());
        }

        Ok(BuildExecutionState {
            build_id: session.build_id.clone(),
            pipeline,
            created_at: state.created_at,
            status: state.status,
            steps,
        })
    } else {
        for manifest_step in &session.manifest.steps {
            let new_step = build_execution_step_from_manifest(
                manifest_step,
                &session.build_id,
                repo_root,
                &mut used_slugs,
            )?;
            steps.push(new_step);
        }
        Ok(BuildExecutionState {
            build_id: session.build_id.clone(),
            pipeline,
            created_at,
            status: BuildExecutionStatus::Queued,
            steps,
        })
    }
}

fn build_execution_step_from_manifest(
    manifest_step: &ManifestStep,
    build_id: &str,
    repo_root: &Path,
    used_slugs: &mut BTreeSet<String>,
) -> Result<BuildExecutionStep, Box<dyn std::error::Error>> {
    let source_slug = manifest_step_slug(manifest_step);
    let derived_slug = allocate_execution_slug(
        build_id,
        &manifest_step.step_key,
        &source_slug,
        repo_root,
        used_slugs,
    )?;
    let derived_branch = plan::default_branch_for_slug(&derived_slug);
    Ok(BuildExecutionStep {
        step_key: manifest_step.step_key.clone(),
        stage_index: manifest_step.stage_index,
        build_plan_path: manifest_step.output_plan_path.clone(),
        derived_slug,
        derived_branch,
        materialize_job_id: None,
        approve_job_id: None,
        review_job_id: None,
        merge_job_id: None,
        status: BuildExecutionStatus::Queued,
    })
}

fn manifest_step_slug(step: &ManifestStep) -> String {
    let stem = Path::new(&step.output_plan_path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let prefix = format!("{}-", step.step_key);
    let value = stem.strip_prefix(&prefix).unwrap_or(stem);
    let normalized = plan::normalize_slug(value);
    if normalized.is_empty() {
        "step".to_string()
    } else {
        normalized
    }
}

fn allocate_execution_slug(
    build_id: &str,
    step_key: &str,
    step_slug: &str,
    repo_root: &Path,
    used_slugs: &mut BTreeSet<String>,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut base = plan::normalize_slug(&format!("{build_id}-{step_key}-{step_slug}"));
    if base.is_empty() {
        base = plan::normalize_slug(&format!("{build_id}-{step_key}"));
    }
    if base.is_empty() {
        base = "build-step".to_string();
    }

    for attempt in 0..128u32 {
        let candidate = if attempt == 0 {
            base.clone()
        } else {
            let suffix = short_digest(format!("{build_id}:{step_key}:{attempt}").as_bytes());
            let value = plan::normalize_slug(&format!("{base}-{suffix}"));
            if value.is_empty() {
                plan::normalize_slug(&format!("{base}-{attempt}"))
            } else {
                value
            }
        };

        if candidate.is_empty() || used_slugs.contains(&candidate) {
            continue;
        }

        let branch = plan::default_branch_for_slug(&candidate);
        let branch_taken = branch_exists(&branch)
            .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
        let plan_taken = repo_root.join(plan::plan_rel_path(&candidate)).exists();
        if !branch_taken && !plan_taken {
            used_slugs.insert(candidate.clone());
            return Ok(candidate);
        }
    }

    Err(format!(
        "unable to derive a unique execution slug for step {} in build {}",
        step_key, build_id
    )
    .into())
}

fn phase_completion_artifact(job_id: &str) -> jobs::JobArtifact {
    jobs::JobArtifact::AskSavePatch {
        job_id: job_id.to_string(),
    }
}

fn phase_job_reusable(status: jobs::JobStatus) -> bool {
    matches!(
        status,
        jobs::JobStatus::Queued
            | jobs::JobStatus::WaitingOnDeps
            | jobs::JobStatus::WaitingOnLocks
            | jobs::JobStatus::Running
            | jobs::JobStatus::Succeeded
    )
}

fn ensure_phase_job(
    repo_root: &Path,
    jobs_root: &Path,
    existing_job_id: &mut Option<String>,
    command_args: &[String],
    metadata: jobs::JobMetadata,
    schedule: jobs::JobSchedule,
    config_snapshot: &serde_json::Value,
) -> Result<PhaseJobOutcome, Box<dyn std::error::Error>> {
    if let Some(job_id) = existing_job_id.as_ref()
        && let Ok(record) = jobs::read_record(jobs_root, job_id)
        && phase_job_reusable(record.status)
    {
        return Ok(PhaseJobOutcome {
            job_id: job_id.clone(),
            enqueued: false,
        });
    }

    let job_id = generate_job_id();
    let mut raw_args = vec!["vizier".to_string()];
    raw_args.extend(command_args.iter().cloned());
    let child_args = build_background_child_args(
        &raw_args,
        &job_id,
        &config::get_config().workflow.background,
        false,
        &[],
    );

    jobs::enqueue_job(
        repo_root,
        jobs_root,
        &job_id,
        &child_args,
        &raw_args,
        Some(metadata),
        Some(config_snapshot.clone()),
        Some(schedule),
    )?;
    *existing_job_id = Some(job_id.clone());

    Ok(PhaseJobOutcome {
        job_id,
        enqueued: true,
    })
}

fn derive_step_status(
    step: &BuildExecutionStep,
    pipeline: BuildExecutionPipeline,
    jobs_root: &Path,
) -> BuildExecutionStatus {
    let mut statuses = Vec::new();
    for job_id in [
        step.materialize_job_id.as_deref(),
        step.approve_job_id.as_deref(),
        step.review_job_id.as_deref(),
        step.merge_job_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if let Ok(record) = jobs::read_record(jobs_root, job_id) {
            statuses.push(record.status);
        }
    }

    if statuses.iter().any(|status| {
        matches!(
            status,
            jobs::JobStatus::Failed | jobs::JobStatus::BlockedByDependency
        )
    }) {
        return BuildExecutionStatus::Failed;
    }
    if statuses
        .iter()
        .any(|status| matches!(status, jobs::JobStatus::Cancelled))
    {
        return BuildExecutionStatus::Cancelled;
    }

    if let Some(job_id) = step.terminal_job_id(pipeline)
        && let Ok(record) = jobs::read_record(jobs_root, job_id)
        && record.status == jobs::JobStatus::Succeeded
    {
        return BuildExecutionStatus::Succeeded;
    }

    if statuses.iter().any(|status| {
        matches!(
            status,
            jobs::JobStatus::Queued
                | jobs::JobStatus::WaitingOnDeps
                | jobs::JobStatus::WaitingOnLocks
                | jobs::JobStatus::Running
        )
    }) {
        return BuildExecutionStatus::Running;
    }

    BuildExecutionStatus::Queued
}

fn derive_execution_status(
    state: &BuildExecutionState,
    pipeline: BuildExecutionPipeline,
    jobs_root: &Path,
) -> BuildExecutionStatus {
    let statuses = state
        .steps
        .iter()
        .map(|step| derive_step_status(step, pipeline, jobs_root))
        .collect::<Vec<_>>();

    if statuses
        .iter()
        .any(|status| matches!(status, BuildExecutionStatus::Failed))
    {
        return BuildExecutionStatus::Failed;
    }
    if statuses
        .iter()
        .any(|status| matches!(status, BuildExecutionStatus::Cancelled))
    {
        return BuildExecutionStatus::Cancelled;
    }
    if statuses
        .iter()
        .all(|status| matches!(status, BuildExecutionStatus::Succeeded))
    {
        return BuildExecutionStatus::Succeeded;
    }
    if statuses
        .iter()
        .any(|status| matches!(status, BuildExecutionStatus::Running))
    {
        return BuildExecutionStatus::Running;
    }
    BuildExecutionStatus::Queued
}

fn render_phase_jobs(step: &BuildExecutionStep, pipeline: BuildExecutionPipeline) -> String {
    let mut parts = Vec::new();
    if let Some(job_id) = step.materialize_job_id.as_ref() {
        parts.push(format!("materialize={job_id}"));
    }
    if let Some(job_id) = step.approve_job_id.as_ref() {
        parts.push(format!("approve={job_id}"));
    }
    if pipeline.includes_review()
        && let Some(job_id) = step.review_job_id.as_ref()
    {
        parts.push(format!("review={job_id}"));
    }
    if pipeline.includes_merge()
        && let Some(job_id) = step.merge_job_id.as_ref()
    {
        parts.push(format!("merge={job_id}"));
    }
    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(",")
    }
}

fn first_failed_step(
    state: &BuildExecutionState,
    pipeline: BuildExecutionPipeline,
    jobs_root: &Path,
) -> Option<String> {
    state.steps.iter().find_map(|step| {
        let status = derive_step_status(step, pipeline, jobs_root);
        if matches!(
            status,
            BuildExecutionStatus::Failed | BuildExecutionStatus::Cancelled
        ) {
            Some(step.step_key.clone())
        } else {
            None
        }
    })
}

fn rewrite_plan_front_matter(source: &str, slug: &str, branch: &str) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("plan: {slug}\n"));
    out.push_str(&format!("branch: {branch}\n"));
    out.push_str("---\n\n");

    if let Some(body) = strip_front_matter(source) {
        out.push_str(body.trim_start_matches(['\n', '\r']));
    } else {
        out.push_str(source.trim_start_matches(['\n', '\r']));
    }

    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn strip_front_matter(source: &str) -> Option<&str> {
    let stripped = source.strip_prefix("---\n")?;
    let marker = "\n---\n";
    let idx = stripped.find(marker)?;
    Some(&stripped[(idx + marker.len())..])
}

fn persist_execution_state(
    repo_root: &Path,
    session: &BuildSession,
    state: &BuildExecutionState,
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp_root = repo_root.join(".vizier/tmp-worktrees");
    std::fs::create_dir_all(&tmp_root)?;
    let suffix = plan::short_suffix();
    let worktree_name = format!("vizier-build-exec-{}-{}", session.build_id, suffix);
    let worktree_path = tmp_root.join(format!("build-exec-{}-{}", session.build_id, suffix));

    add_worktree_for_branch(&worktree_name, &worktree_path, &session.build_branch).map_err(
        |err| -> Box<dyn std::error::Error> {
            Box::from(format!(
                "add_worktree({}, {}): {}",
                worktree_name,
                worktree_path.display(),
                err
            ))
        },
    )?;
    jobs::record_current_job_worktree(repo_root, Some(&worktree_name), &worktree_path);

    let persist_result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let _guard = WorkdirGuard::enter(&worktree_path)?;
        let execution_abs = worktree_path.join(&session.execution_rel);
        if let Some(parent) = execution_abs.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let contents = serde_json::to_string_pretty(state)?;
        let current = std::fs::read_to_string(&execution_abs).ok();
        if current.as_deref() == Some(contents.as_str()) {
            return Ok(());
        }

        std::fs::write(&execution_abs, contents)?;
        commit_paths_in_repo(
            &worktree_path,
            &[session.execution_rel.as_path()],
            &format!("docs: update build execution {}", session.build_id),
        )
        .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
        Ok(())
    })();

    cleanup_worktree(&worktree_name, &worktree_path);
    persist_result
}

fn branch_file_exists(
    repo: &Repository,
    branch: &str,
    rel_path: &Path,
) -> Result<bool, Box<dyn std::error::Error>> {
    let branch_ref = repo.find_branch(branch, BranchType::Local).map_err(|_| {
        format!(
            "branch {} not found while checking {}",
            branch,
            rel_path.display()
        )
    })?;
    let commit = branch_ref.into_reference().peel_to_commit()?;
    let tree = commit.tree()?;
    Ok(tree.get_path(rel_path).is_ok())
}

fn read_branch_file(
    repo: &Repository,
    branch: &str,
    rel_path: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let maybe = try_read_branch_file(repo, branch, rel_path)?;
    maybe.ok_or_else(|| format!("branch {} missing {}", branch, rel_path.display()).into())
}

fn try_read_branch_file(
    repo: &Repository,
    branch: &str,
    rel_path: &Path,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let branch_ref = repo.find_branch(branch, BranchType::Local).map_err(|_| {
        format!(
            "branch {} not found while reading {}",
            branch,
            rel_path.display()
        )
    })?;
    let commit = branch_ref.into_reference().peel_to_commit()?;
    let tree = commit.tree()?;
    let entry = match tree.get_path(rel_path) {
        Ok(entry) => entry,
        Err(err) => {
            if err.code() == git2::ErrorCode::NotFound {
                return Ok(None);
            }
            return Err(Box::new(err));
        }
    };
    let blob = repo.find_blob(entry.id())?;
    let bytes = blob.content().to_vec();
    let text = String::from_utf8(bytes).map_err(|err| {
        format!(
            "branch {} file {} is not UTF-8: {}",
            branch,
            rel_path.display(),
            err
        )
    })?;
    Ok(Some(text))
}

struct BuildManifestInit<'a> {
    build_id: &'a str,
    target_branch: &'a str,
    build_branch: &'a str,
    input_original: &'a Path,
    input_copied: &'a Path,
    input_digest: &'a str,
    plans_rel_root: &'a Path,
}

fn build_manifest_template(init: BuildManifestInit<'_>, steps: &[NormalizedStep]) -> BuildManifest {
    let manifest_steps = steps
        .iter()
        .map(|step| ManifestStep {
            step_key: step.step_key.clone(),
            stage_index: step.stage_index,
            parallel_index: step.parallel_index,
            intent_source: step.intent.source.as_manifest_value(),
            intent_digest: digest_hex(step.intent.text.as_bytes()),
            output_plan_path: to_repo_string(&init.plans_rel_root.join(&step.file_name)),
            reads: Vec::new(),
            result: ManifestStepResult::Pending,
            summary: None,
            output_digest: None,
            error: None,
        })
        .collect::<Vec<_>>();

    BuildManifest {
        build_id: init.build_id.to_string(),
        created_at: Utc::now().to_rfc3339(),
        target_branch: init.target_branch.to_string(),
        build_branch: init.build_branch.to_string(),
        input_file: ManifestInputFile {
            original_path: init.input_original.display().to_string(),
            copied_path: to_repo_string(init.input_copied),
            digest: init.input_digest.to_string(),
        },
        steps: manifest_steps,
        artifacts: ManifestArtifacts {
            plan_docs: Vec::new(),
            summary: None,
        },
        status: ManifestStatus::Running,
    }
}

fn mark_step_failed(manifest: &mut BuildManifest, step_idx: usize, error: &str) {
    if let Some(entry) = manifest.steps.get_mut(step_idx) {
        entry.result = ManifestStepResult::Failed;
        entry.error = Some(error.to_string());
    }
    manifest.status = ManifestStatus::Failed;
}

fn prior_stage_refs(manifest: &BuildManifest, current_stage: usize) -> Vec<ManifestPlanReference> {
    manifest
        .steps
        .iter()
        .filter(|step| step.stage_index < current_stage)
        .filter(|step| step.result == ManifestStepResult::Succeeded)
        .map(|step| ManifestPlanReference {
            step_key: step.step_key.clone(),
            plan_path: step.output_plan_path.clone(),
            summary: step
                .summary
                .clone()
                .unwrap_or_else(|| "(summary unavailable)".to_string()),
            digest: step.output_digest.clone(),
        })
        .collect()
}

fn commit_session_artifacts(
    worktree_path: &Path,
    artifact_paths: &[PathBuf],
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let existing = artifact_paths
        .iter()
        .filter(|path| worktree_path.join(path).is_file())
        .cloned()
        .collect::<Vec<_>>();

    if existing.is_empty() {
        return Err("no build session artifacts found for commit".into());
    }

    let refs = existing
        .iter()
        .map(|path| path.as_path())
        .collect::<Vec<_>>();
    commit_paths_in_repo(worktree_path, &refs, message)
        .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    Ok(())
}

fn cleanup_worktree(worktree_name: &str, worktree_path: &Path) {
    if let Err(err) = remove_worktree(worktree_name, true) {
        display::warn(format!(
            "temporary build worktree cleanup failed ({}); remove manually with `git worktree prune`",
            err
        ));
    }
    if worktree_path.exists() {
        let _ = std::fs::remove_dir_all(worktree_path);
    }
}

fn render_build_plan_document(
    step: &NormalizedStep,
    build_branch: &str,
    plan_body: &str,
) -> String {
    let plan_slug = format!("{}-{}", step.step_key, step.slug);
    plan::render_plan_document(&plan_slug, build_branch, &step.intent.text, plan_body)
}

fn render_session_summary(manifest: &BuildManifest) -> String {
    let mut out = String::new();
    out.push_str("# Build Session Summary\n\n");
    out.push_str(&format!("- Build: `{}`\n", manifest.build_id));
    out.push_str(&format!("- Branch: `{}`\n", manifest.build_branch));
    out.push_str(&format!("- Target: `{}`\n", manifest.target_branch));
    out.push_str(&format!("- Status: `{}`\n", manifest.status.as_label()));
    out.push_str("- Manifest: `manifest.json`\n");
    out.push_str("\n## Steps\n\n");

    for step in &manifest.steps {
        out.push_str(&format!(
            "- `{}` `{}` -> `{}`",
            step.step_key,
            step.result.label(),
            step.output_plan_path
        ));
        if !step.reads.is_empty() {
            let refs = step
                .reads
                .iter()
                .map(|entry| entry.step_key.clone())
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(" (reads: {})", refs));
        }
        if let Some(error) = step.error.as_ref() {
            out.push_str(&format!(" (error: {})", summarize_text(error)));
        }
        out.push('\n');
    }

    out
}

impl ManifestStatus {
    fn as_label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }
}

fn summarize_text(input: &str) -> String {
    let first_line = input
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("(summary unavailable)");

    truncate_chars(first_line, 140)
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }

    let mut clipped = String::new();
    for (idx, ch) in input.chars().enumerate() {
        if idx >= max_chars.saturating_sub(1) {
            clipped.push('…');
            break;
        }
        clipped.push(ch);
    }

    clipped
}

fn parse_build_file(path: &Path) -> Result<(BuildFile, String), Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(path)?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let parsed = match extension.as_str() {
        "toml" => toml::from_str(&contents).map_err(|err| {
            Box::<dyn std::error::Error>::from(format!(
                "failed to parse TOML build file {}: {err}",
                path.display()
            ))
        })?,
        "json" => serde_json::from_str(&contents).map_err(|err| {
            Box::<dyn std::error::Error>::from(format!(
                "failed to parse JSON build file {}: {err}",
                path.display()
            ))
        })?,
        _ => {
            return Err(format!(
                "unsupported build file extension for {} (expected .toml or .json)",
                path.display()
            )
            .into());
        }
    };

    Ok((parsed, contents))
}

fn resolve_steps(
    parsed: BuildFile,
    build_dir: &Path,
    repo_root: &Path,
) -> Result<Vec<Vec<ResolvedIntent>>, Box<dyn std::error::Error>> {
    if parsed.steps.is_empty() {
        return Err("build file steps must be non-empty".into());
    }

    let mut resolved_steps = Vec::new();
    for (idx, step) in parsed.steps.into_iter().enumerate() {
        let step_index = idx + 1;
        match step {
            BuildStep::Single(intent) => {
                let resolved = resolve_intent(intent, build_dir, repo_root, step_index, None)?;
                resolved_steps.push(vec![resolved]);
            }
            BuildStep::Parallel(intents) => {
                if intents.is_empty() {
                    return Err(
                        format!("step {step_index} parallel group must be non-empty").into(),
                    );
                }
                let mut group = Vec::new();
                for (intent_idx, intent) in intents.into_iter().enumerate() {
                    let resolved = resolve_intent(
                        intent,
                        build_dir,
                        repo_root,
                        step_index,
                        Some(intent_idx + 1),
                    )?;
                    group.push(resolved);
                }
                resolved_steps.push(group);
            }
        }
    }

    Ok(resolved_steps)
}

fn resolve_intent(
    intent: IntentDoc,
    build_dir: &Path,
    repo_root: &Path,
    step_index: usize,
    parallel_index: Option<usize>,
) -> Result<ResolvedIntent, Box<dyn std::error::Error>> {
    match (intent.text, intent.file) {
        (Some(text), None) => {
            if text.trim().is_empty() {
                return Err(format!("step {step_index} intent text must be non-empty").into());
            }
            Ok(ResolvedIntent {
                text,
                source: IntentSource::Text,
            })
        }
        (None, Some(path)) => {
            let resolved_path = resolve_intent_path(build_dir, repo_root, &path)?;
            let contents = std::fs::read_to_string(&resolved_path).map_err(|err| {
                format!(
                    "unable to read intent file {}: {err}",
                    resolved_path.display()
                )
            })?;
            if contents.trim().is_empty() {
                return Err(format!(
                    "step {step_index} intent file {} is empty",
                    resolved_path.display()
                )
                .into());
            }
            Ok(ResolvedIntent {
                text: contents,
                source: IntentSource::File(resolved_path),
            })
        }
        (Some(_), Some(_)) => {
            Err(format!("step {step_index} intent must set exactly one of text or file").into())
        }
        (None, None) => {
            let detail = parallel_index
                .map(|idx| format!("step {step_index} intent {idx}"))
                .unwrap_or_else(|| format!("step {step_index} intent"));
            Err(format!("{detail} must set exactly one of text or file").into())
        }
    }
}

fn resolve_intent_path(
    build_dir: &Path,
    repo_root: &Path,
    raw_path: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let candidate = PathBuf::from(raw_path);
    let joined = if candidate.is_absolute() {
        candidate
    } else {
        build_dir.join(candidate)
    };
    let resolved = std::fs::canonicalize(&joined)
        .map_err(|err| format!("unable to resolve intent file {}: {err}", joined.display()))?;
    if !resolved.starts_with(repo_root) {
        return Err(format!(
            "intent file {} escapes repo root {}",
            resolved.display(),
            repo_root.display()
        )
        .into());
    }
    let metadata = std::fs::metadata(&resolved)?;
    if !metadata.is_file() {
        return Err(format!("intent file {} must be a file", resolved.display()).into());
    }
    Ok(resolved)
}

fn normalize_steps(resolved_steps: &[Vec<ResolvedIntent>]) -> Vec<NormalizedStep> {
    let mut normalized = Vec::new();

    for (stage_idx, group) in resolved_steps.iter().enumerate() {
        let stage_index = stage_idx + 1;
        let parallel = group.len() > 1;
        for (idx, intent) in group.iter().enumerate() {
            let parallel_index = if parallel { Some(idx + 1) } else { None };
            let step_key = build_step_key(stage_index, parallel_index);
            let slug = plan::slug_from_spec(&intent.text);
            let file_name = format!("{step_key}-{slug}.md");
            normalized.push(NormalizedStep {
                stage_index,
                parallel_index,
                step_key,
                slug,
                file_name,
                intent: intent.clone(),
            });
        }
    }

    normalized
}

fn build_step_key(stage_index: usize, parallel_index: Option<usize>) -> String {
    let prefix = format!("{stage_index:02}");
    match parallel_index {
        Some(index) => format!("{prefix}{}", alpha_suffix(index)),
        None => prefix,
    }
}

fn alpha_suffix(mut index: usize) -> String {
    let mut chars = Vec::new();
    while index > 0 {
        index -= 1;
        chars.push((b'a' + (index % 26) as u8) as char);
        index /= 26;
    }
    chars.iter().rev().collect()
}

fn write_manifest(path: &Path, manifest: &BuildManifest) -> Result<(), Box<dyn std::error::Error>> {
    let contents = serde_json::to_string_pretty(manifest)?;
    std::fs::write(path, contents)?;
    Ok(())
}

fn allocate_build_id(
    build_path: &Path,
    build_contents: &str,
    repo_root: &Path,
    name_override: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(raw_name) = name_override {
        let normalized = plan::sanitize_name_override(raw_name)
            .map_err(|err| format!("invalid build name `{raw_name}`: {err}"))?;
        let branch = format!("build/{normalized}");
        let builds_root = repo_root.join(BUILD_PLAN_ROOT);
        let branch_taken = branch_exists(&branch)
            .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
        let path_taken = builds_root.join(&normalized).exists();
        if branch_taken || path_taken {
            return Err(format!(
                "build session `{normalized}` already exists (branch `{branch}` or session path collision)"
            )
            .into());
        }
        return Ok(normalized);
    }

    let stem = build_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("build-session");
    let normalized_stem = {
        let value = plan::normalize_slug(stem);
        if value.is_empty() {
            "build-session".to_string()
        } else {
            value
        }
    };

    let digest = short_digest(build_contents.as_bytes());
    let base = format!("{normalized_stem}-{digest}");
    let builds_root = repo_root.join(BUILD_PLAN_ROOT);

    for suffix in 0..128 {
        let candidate = if suffix == 0 {
            base.clone()
        } else {
            format!("{base}-{suffix}")
        };
        let branch = format!("build/{candidate}");
        let branch_taken = branch_exists(&branch)
            .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
        let path_taken = builds_root.join(&candidate).exists();
        if !branch_taken && !path_taken {
            return Ok(candidate);
        }
    }

    Err("unable to allocate a unique build session id".into())
}

fn digest_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

fn short_digest(data: &[u8]) -> String {
    let digest = digest_hex(data);
    digest.chars().take(12).collect()
}

fn to_repo_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::{alpha_suffix, build_step_key, normalize_steps};
    use crate::actions::build::{IntentSource, ResolvedIntent};

    #[test]
    fn step_key_formats_serial_and_parallel() {
        assert_eq!(build_step_key(1, None), "01");
        assert_eq!(build_step_key(2, Some(1)), "02a");
        assert_eq!(build_step_key(2, Some(2)), "02b");
        assert_eq!(build_step_key(9, Some(27)), "09aa");
    }

    #[test]
    fn alpha_suffix_rolls_past_z() {
        assert_eq!(alpha_suffix(1), "a");
        assert_eq!(alpha_suffix(26), "z");
        assert_eq!(alpha_suffix(27), "aa");
        assert_eq!(alpha_suffix(52), "az");
        assert_eq!(alpha_suffix(53), "ba");
    }

    #[test]
    fn normalized_steps_are_deterministic() {
        let steps = vec![
            vec![ResolvedIntent {
                text: "Alpha scope".to_string(),
                source: IntentSource::Text,
            }],
            vec![
                ResolvedIntent {
                    text: "Bravo scope".to_string(),
                    source: IntentSource::Text,
                },
                ResolvedIntent {
                    text: "Charlie scope".to_string(),
                    source: IntentSource::Text,
                },
            ],
        ];

        let normalized = normalize_steps(&steps);
        let keys = normalized
            .iter()
            .map(|entry| entry.step_key.clone())
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["01", "02a", "02b"]);

        let names = normalized
            .iter()
            .map(|entry| entry.file_name.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "01-alpha-scope.md",
                "02a-bravo-scope.md",
                "02b-charlie-scope.md"
            ]
        );
    }
}
