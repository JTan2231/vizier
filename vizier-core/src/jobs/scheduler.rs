use super::*;

#[derive(Debug, Default)]
pub struct SchedulerOutcome {
    pub started: Vec<String>,
    pub updated: Vec<String>,
    pub ephemeral_run_cleanups: Vec<EphemeralRunCleanupEvent>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EphemeralRunCleanupEvent {
    pub run_id: String,
    pub state: EphemeralCleanupState,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prune_vizier_root: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<CleanRemovedCounts>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub degraded_notes: Vec<String>,
}

pub(crate) fn job_is_terminal(status: JobStatus) -> bool {
    matches!(
        status,
        JobStatus::Succeeded
            | JobStatus::Failed
            | JobStatus::Cancelled
            | JobStatus::BlockedByDependency
            | JobStatus::BlockedByApproval
    )
}

pub(crate) fn job_is_active(status: JobStatus) -> bool {
    matches!(
        status,
        JobStatus::Queued
            | JobStatus::WaitingOnDeps
            | JobStatus::WaitingOnApproval
            | JobStatus::WaitingOnLocks
            | JobStatus::Running
    )
}

pub(crate) fn resolve_plan_artifact_branch(slug: &str, branch: &str) -> Option<String> {
    let branch = branch.trim();
    if !branch.is_empty() {
        return Some(branch.to_string());
    }
    let slug = slug.trim();
    if slug.is_empty() {
        None
    } else {
        Some(crate::plan::default_branch_for_slug(slug))
    }
}

pub(crate) fn artifact_exists(repo: &Repository, artifact: &JobArtifact) -> bool {
    match artifact {
        JobArtifact::PlanBranch { slug, branch } | JobArtifact::PlanCommits { slug, branch } => {
            let Some(branch) = resolve_plan_artifact_branch(slug, branch) else {
                return false;
            };
            repo.find_branch(&branch, git2::BranchType::Local).is_ok()
        }
        JobArtifact::PlanDoc { slug, branch } => {
            let plan_path = crate::plan::plan_rel_path(slug);
            let Some(branch) = resolve_plan_artifact_branch(slug, branch) else {
                return false;
            };
            let Ok(branch_ref) = repo.find_branch(&branch, git2::BranchType::Local) else {
                return false;
            };
            let Ok(commit) = branch_ref.into_reference().peel_to_commit() else {
                return false;
            };
            let Ok(tree) = commit.tree() else {
                return false;
            };
            tree.get_path(&plan_path).is_ok()
        }
        JobArtifact::TargetBranch { name } => {
            repo.find_branch(name, git2::BranchType::Local).is_ok()
        }
        JobArtifact::MergeSentinel { slug } => {
            let path = repo
                .path()
                .join(".vizier/tmp/merge-conflicts")
                .join(format!("{slug}.json"));
            path.exists()
        }
        JobArtifact::CommandPatch { job_id } => {
            let repo_root = repo.path().parent().unwrap_or_else(|| Path::new("."));
            let jobs_root = repo_root.join(".vizier/jobs");
            if command_patch_path(&jobs_root, job_id).exists()
                || legacy_command_patch_path(&jobs_root, job_id).exists()
            {
                return true;
            }

            let paths = paths_for(&jobs_root, job_id);
            if !paths.record_path.exists() {
                return false;
            }

            match read_record(&jobs_root, job_id) {
                Ok(record) => record.status == JobStatus::Succeeded,
                Err(_) => false,
            }
        }
        JobArtifact::Custom { type_id, key } => {
            let repo_root = repo.path().parent().unwrap_or_else(|| Path::new("."));
            custom_artifact_marker_exists(repo_root, type_id, key)
        }
    }
}

pub(crate) fn plan_doc_paths_from_artifacts(artifacts: &[JobArtifact]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    for artifact in artifacts {
        let JobArtifact::PlanDoc { slug, .. } = artifact else {
            continue;
        };
        let slug = slug.trim();
        if slug.is_empty() {
            continue;
        }
        if seen.insert(slug.to_string()) {
            paths.push(crate::plan::plan_rel_path(slug));
        }
    }
    paths
}

pub(crate) fn pinned_head_matches(
    repo: &Repository,
    pinned: &PinnedHead,
) -> Result<bool, git2::Error> {
    let branch_ref = repo.find_branch(&pinned.branch, git2::BranchType::Local)?;
    let commit = branch_ref.into_reference().peel_to_commit()?;
    let expected = Oid::from_str(&pinned.oid).ok();
    Ok(Some(commit.id()) == expected)
}

pub(crate) fn is_ephemeral_vizier_path(path: &str) -> bool {
    const EPHEMERAL_PREFIXES: [&str; 4] = [
        ".vizier/jobs",
        ".vizier/sessions",
        ".vizier/tmp",
        ".vizier/tmp-worktrees",
    ];
    EPHEMERAL_PREFIXES
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(&format!("{}/", prefix)))
}

pub(crate) fn clean_worktree_matches(repo: &Repository) -> Result<bool, git2::Error> {
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .exclude_submodules(true);
    let statuses = repo.statuses(Some(&mut opts))?;
    let has_relevant_changes = statuses.iter().any(|entry| {
        let Some(path) = entry.path() else {
            return true;
        };
        !is_ephemeral_vizier_path(path)
    });
    Ok(!has_relevant_changes)
}

pub(crate) fn branch_from_locks(locks: &[JobLock]) -> Option<String> {
    let mut branches = locks
        .iter()
        .filter_map(|lock| lock.key.strip_prefix("branch:"))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    branches.sort();
    branches.dedup();
    if branches.len() == 1 {
        branches.into_iter().next()
    } else {
        None
    }
}

pub(crate) fn resolve_branch_precondition_target(
    repo: &Repository,
    schedule: &JobSchedule,
    explicit: Option<&str>,
) -> Option<String> {
    explicit
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            schedule
                .pinned_head
                .as_ref()
                .map(|value| value.branch.clone())
        })
        .or_else(|| branch_from_locks(&schedule.locks))
        .or_else(|| {
            repo.head()
                .ok()
                .and_then(|head| head.shorthand().map(ToString::to_string))
        })
}

pub(crate) fn evaluate_precondition(
    repo: &Repository,
    schedule: &JobSchedule,
    precondition: &JobPrecondition,
) -> Result<JobPreconditionState, git2::Error> {
    match precondition {
        JobPrecondition::CleanWorktree => {
            if clean_worktree_matches(repo)? {
                Ok(JobPreconditionState::Satisfied)
            } else {
                Ok(JobPreconditionState::Waiting {
                    detail: "working tree has uncommitted or untracked changes".to_string(),
                })
            }
        }
        JobPrecondition::BranchExists { branch } => {
            let resolved = resolve_branch_precondition_target(repo, schedule, branch.as_deref());
            let Some(target) = resolved else {
                return Ok(JobPreconditionState::Blocked {
                    detail: "branch_exists precondition requires branch context (set precondition.branch, pinned_head, or a single branch lock)".to_string(),
                });
            };
            if repo.find_branch(&target, git2::BranchType::Local).is_ok() {
                Ok(JobPreconditionState::Satisfied)
            } else {
                Ok(JobPreconditionState::Waiting {
                    detail: format!("required branch `{target}` does not exist"),
                })
            }
        }
        JobPrecondition::Custom { id, args } => match id.as_str() {
            "clean_worktree" => {
                if clean_worktree_matches(repo)? {
                    Ok(JobPreconditionState::Satisfied)
                } else {
                    Ok(JobPreconditionState::Waiting {
                        detail: "custom precondition clean_worktree failed: working tree has uncommitted or untracked changes".to_string(),
                    })
                }
            }
            "branch_exists" => {
                let branch = args.get("branch").map(String::as_str);
                let resolved = resolve_branch_precondition_target(repo, schedule, branch);
                let Some(target) = resolved else {
                    return Ok(JobPreconditionState::Blocked {
                        detail: "custom precondition branch_exists requires `branch` arg or branch context".to_string(),
                    });
                };
                if repo.find_branch(&target, git2::BranchType::Local).is_ok() {
                    Ok(JobPreconditionState::Satisfied)
                } else {
                    Ok(JobPreconditionState::Waiting {
                        detail: format!(
                            "custom precondition branch_exists failed: `{target}` missing"
                        ),
                    })
                }
            }
            _ => Ok(JobPreconditionState::Blocked {
                detail: format!("custom precondition `{id}` is not supported by scheduler runtime"),
            }),
        },
    }
}

pub(crate) fn resolve_after_dependency_state(
    jobs_root: &Path,
    job_statuses: &HashMap<String, JobStatus>,
    dependency: &JobAfterDependency,
) -> AfterDependencyState {
    if let Some(status) = job_statuses.get(&dependency.job_id) {
        return AfterDependencyState::Status(*status);
    }

    let paths = paths_for(jobs_root, &dependency.job_id);
    if !paths.record_path.exists() {
        return AfterDependencyState::Missing;
    }

    match load_record(&paths) {
        Ok(record) => AfterDependencyState::Status(record.status),
        Err(err) => AfterDependencyState::Invalid {
            detail: err.to_string(),
        },
    }
}

pub(crate) fn build_scheduler_facts(
    repo: &Repository,
    jobs_root: &Path,
    records: &[JobRecord],
) -> Result<SchedulerFacts, Box<dyn std::error::Error>> {
    let mut facts = SchedulerFacts::default();
    let mut dependency_artifacts = HashSet::new();
    let mut job_statuses = HashMap::new();
    for record in records {
        job_statuses.insert(record.id.clone(), record.status);
    }

    for record in records {
        facts.job_order.push(record.id.clone());
        facts.job_statuses.insert(record.id.clone(), record.status);
        if !record.child_args.is_empty() {
            facts.has_child_args.insert(record.id.clone());
        }

        if let Some(schedule) = record.schedule.as_ref() {
            let mut after = schedule.after.clone();
            sort_after_dependencies(&mut after);
            after.dedup();
            let after = after
                .into_iter()
                .map(|dependency| JobAfterDependencyStatus {
                    job_id: dependency.job_id.clone(),
                    policy: dependency.policy,
                    state: resolve_after_dependency_state(jobs_root, &job_statuses, &dependency),
                })
                .collect::<Vec<_>>();
            if !after.is_empty() {
                facts
                    .job_after_dependencies
                    .insert(record.id.clone(), after);
            }

            let deps = schedule
                .dependencies
                .iter()
                .map(|dep| dep.artifact.clone())
                .collect::<Vec<_>>();
            if !deps.is_empty() {
                dependency_artifacts.extend(deps.iter().cloned());
                facts.job_dependencies.insert(record.id.clone(), deps);
                facts.job_missing_producer_policy.insert(
                    record.id.clone(),
                    schedule.dependency_policy.missing_producer,
                );
            }

            if !schedule.locks.is_empty() {
                facts
                    .job_locks
                    .insert(record.id.clone(), schedule.locks.clone());
            }

            if let Some(pinned) = schedule.pinned_head.as_ref() {
                let matches = pinned_head_matches(repo, pinned)?;
                facts.pinned_heads.insert(
                    record.id.clone(),
                    PinnedHeadFact {
                        branch: pinned.branch.clone(),
                        matches,
                    },
                );
            }

            if !schedule.preconditions.is_empty() {
                let mut preconditions = Vec::with_capacity(schedule.preconditions.len());
                for precondition in &schedule.preconditions {
                    let state = evaluate_precondition(repo, schedule, precondition)?;
                    preconditions.push(JobPreconditionFact {
                        precondition: precondition.clone(),
                        state,
                    });
                }
                facts
                    .job_preconditions
                    .insert(record.id.clone(), preconditions);
            }

            if let Some(approval) = schedule.approval.as_ref()
                && approval.required
            {
                facts.job_approvals.insert(
                    record.id.clone(),
                    JobApprovalFact {
                        required: true,
                        state: approval.state,
                        reason: approval.reason.clone(),
                    },
                );
            }

            if !schedule.waited_on.is_empty() {
                facts
                    .waited_on
                    .insert(record.id.clone(), schedule.waited_on.clone());
            }

            for artifact in &schedule.artifacts {
                facts
                    .producer_statuses
                    .entry(artifact.clone())
                    .or_default()
                    .push(record.status);
            }

            if record.status == JobStatus::Running && !schedule.locks.is_empty() {
                facts.lock_state.acquire(&schedule.locks);
            }
        }
    }

    for artifact in dependency_artifacts {
        if artifact_exists(repo, &artifact) {
            facts.artifact_exists.insert(artifact);
        }
    }

    Ok(facts)
}

#[derive(Debug, Clone)]
pub(crate) struct RunningJobLivenessProbe {
    state: ProcessLivenessState,
    checked_at: DateTime<Utc>,
    failure_reason: Option<String>,
}

impl RunningJobLivenessProbe {
    fn alive(checked_at: DateTime<Utc>) -> Self {
        Self {
            state: ProcessLivenessState::Alive,
            checked_at,
            failure_reason: None,
        }
    }

    fn stale(
        state: ProcessLivenessState,
        checked_at: DateTime<Utc>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            state,
            checked_at,
            failure_reason: Some(reason.into()),
        }
    }

    fn is_stale(&self) -> bool {
        self.state != ProcessLivenessState::Alive
    }
}

pub(crate) enum ProcessIdentityProbe {
    Match,
    Mismatch(String),
    Unavailable(String),
}

pub(crate) fn process_identity_expected_token(record: &JobRecord) -> Option<&str> {
    record
        .child_args
        .first()
        .map(String::as_str)
        .or_else(|| record.command.first().map(String::as_str))
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
}

#[cfg(unix)]
pub(crate) fn probe_process_identity(record: &JobRecord, pid: u32) -> ProcessIdentityProbe {
    let Some(expected) = process_identity_expected_token(record) else {
        return ProcessIdentityProbe::Unavailable(
            "no command token available for process identity verification".to_string(),
        );
    };

    let output = match Command::new("ps")
        .arg("-o")
        .arg("command=")
        .arg("-p")
        .arg(pid.to_string())
        .output()
    {
        Ok(output) => output,
        Err(err) => {
            return ProcessIdentityProbe::Unavailable(format!("ps probe failed: {err}"));
        }
    };

    if !output.status.success() {
        return ProcessIdentityProbe::Unavailable(format!(
            "ps probe exited with status {}",
            output.status
        ));
    }

    let command_line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if command_line.is_empty() {
        return ProcessIdentityProbe::Unavailable(
            "ps probe returned empty command line".to_string(),
        );
    }

    if command_line.contains(expected) {
        ProcessIdentityProbe::Match
    } else {
        ProcessIdentityProbe::Mismatch(format!(
            "observed process identity did not match expected token `{expected}`"
        ))
    }
}

#[cfg(not(unix))]
pub(crate) fn probe_process_identity(_record: &JobRecord, _pid: u32) -> ProcessIdentityProbe {
    ProcessIdentityProbe::Unavailable("process identity guard unavailable on this platform".into())
}

pub(crate) fn probe_running_job_liveness(record: &JobRecord) -> RunningJobLivenessProbe {
    let checked_at = Utc::now();
    let Some(pid) = record.pid else {
        return RunningJobLivenessProbe::stale(
            ProcessLivenessState::StaleMissingPid,
            checked_at,
            "running job record is missing pid",
        );
    };

    if !pid_is_running(pid) {
        return RunningJobLivenessProbe::stale(
            ProcessLivenessState::StaleNotRunning,
            checked_at,
            format!("worker process {pid} is not running"),
        );
    }

    match probe_process_identity(record, pid) {
        ProcessIdentityProbe::Match => RunningJobLivenessProbe::alive(checked_at),
        ProcessIdentityProbe::Mismatch(detail) => RunningJobLivenessProbe::stale(
            ProcessLivenessState::StaleIdentityMismatch,
            checked_at,
            detail,
        ),
        ProcessIdentityProbe::Unavailable(_detail) => RunningJobLivenessProbe::alive(checked_at),
    }
}

pub(crate) fn apply_stale_workflow_failed_routes(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    record: &JobRecord,
) {
    let Some(metadata) = record.metadata.as_ref() else {
        return;
    };
    let Some(run_id) = metadata.workflow_run_id.as_deref() else {
        return;
    };
    let Some(node_id) = metadata.workflow_node_id.as_deref() else {
        return;
    };

    let manifest = match load_workflow_run_manifest(project_root, run_id) {
        Ok(manifest) => manifest,
        Err(err) => {
            display::warn(format!(
                "unable to load workflow manifest {run_id} for stale running job {}: {}",
                record.id, err
            ));
            return;
        }
    };

    let Some(node) = manifest.nodes.get(node_id) else {
        display::warn(format!(
            "workflow node `{node_id}` missing from run manifest {run_id} during stale-running reconciliation"
        ));
        return;
    };

    apply_workflow_routes(
        project_root,
        jobs_root,
        binary,
        record,
        node,
        &manifest,
        WorkflowNodeOutcome::Failed,
        true,
    );
}

pub(crate) fn reconcile_running_job_liveness_locked(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    records: &[JobRecord],
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut reconciled = Vec::new();

    for record in records {
        if record.status != JobStatus::Running {
            continue;
        }

        let probe = probe_running_job_liveness(record);
        if !probe.is_stale() {
            continue;
        }

        let mut metadata = JobMetadata {
            process_liveness_state: Some(probe.state),
            process_liveness_checked_at: Some(probe.checked_at),
            process_liveness_failure_reason: probe.failure_reason.clone(),
            ..JobMetadata::default()
        };
        if record
            .metadata
            .as_ref()
            .and_then(|entry| entry.workflow_node_id.as_deref())
            .is_some()
        {
            metadata.workflow_node_outcome = Some(WorkflowNodeOutcome::Failed.as_str().to_string());
        }

        let finalized = finalize_job(
            project_root,
            jobs_root,
            &record.id,
            JobStatus::Failed,
            1,
            None,
            Some(metadata),
        )?;
        apply_stale_workflow_failed_routes(project_root, jobs_root, binary, &finalized);
        reconciled.push(record.id.clone());
    }

    Ok(reconciled)
}

fn write_ephemeral_cleanup_state_to_manifest(
    project_root: &Path,
    run_id: &str,
    requested: bool,
    state: EphemeralCleanupState,
    detail: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = load_workflow_run_manifest(project_root, run_id)?;
    manifest.ephemeral_cleanup_requested = requested;
    manifest.ephemeral_cleanup_state = Some(state);
    manifest.ephemeral_cleanup_detail = detail
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    write_workflow_run_manifest(project_root, &manifest)
}

fn write_ephemeral_cleanup_state_to_jobs(
    jobs_root: &Path,
    job_ids: &[String],
    requested: bool,
    state: EphemeralCleanupState,
    detail: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let detail = detail
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);

    for job_id in job_ids {
        let paths = paths_for(jobs_root, job_id);
        if !paths.record_path.exists() {
            continue;
        }

        let mut record = load_record(&paths)?;
        let metadata = record.metadata.get_or_insert_with(Default::default);
        metadata.ephemeral_run = Some(true);
        metadata.ephemeral_cleanup_requested = Some(requested);
        metadata.ephemeral_cleanup_state = Some(state);
        metadata.ephemeral_cleanup_detail = detail.clone();
        persist_record(&paths, &record)?;
    }

    Ok(())
}

fn persist_ephemeral_cleanup_state(
    project_root: &Path,
    jobs_root: &Path,
    run_id: &str,
    job_ids: &[String],
    requested: bool,
    state: EphemeralCleanupState,
    detail: Option<&str>,
) {
    if let Err(err) =
        write_ephemeral_cleanup_state_to_manifest(project_root, run_id, requested, state, detail)
    {
        display::warn(format!(
            "unable to persist ephemeral cleanup manifest state for run {}: {}",
            run_id, err
        ));
    }
    if let Err(err) =
        write_ephemeral_cleanup_state_to_jobs(jobs_root, job_ids, requested, state, detail)
    {
        display::warn(format!(
            "unable to persist ephemeral cleanup job state for run {}: {}",
            run_id, err
        ));
    }
}

fn join_cleanup_reasons(reasons: &[String]) -> Option<String> {
    let detail = reasons
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    if detail.is_empty() {
        None
    } else {
        Some(detail)
    }
}

fn reconcile_ephemeral_run_cleanup_locked(
    project_root: &Path,
    jobs_root: &Path,
    records: &[JobRecord],
) -> Result<Vec<EphemeralRunCleanupEvent>, Box<dyn std::error::Error>> {
    let mut runs = BTreeMap::<String, Vec<JobRecord>>::new();
    for record in records {
        let Some(metadata) = record.metadata.as_ref() else {
            continue;
        };
        if metadata.ephemeral_run != Some(true) {
            continue;
        }
        let Some(run_id) = metadata
            .workflow_run_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        runs.entry(run_id.to_string())
            .or_default()
            .push(record.clone());
    }

    let mut events = Vec::new();
    for (run_id, run_records) in runs {
        if !run_records
            .iter()
            .all(|record| job_is_terminal(record.status))
        {
            continue;
        }

        let job_ids = run_records
            .iter()
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        let manifest = load_workflow_run_manifest(project_root, &run_id).ok();
        let requested = manifest
            .as_ref()
            .map(|entry| entry.ephemeral_cleanup_requested)
            .unwrap_or_else(|| {
                run_records.iter().any(|record| {
                    record
                        .metadata
                        .as_ref()
                        .and_then(|meta| meta.ephemeral_cleanup_requested)
                        == Some(true)
                })
            });

        if !requested {
            persist_ephemeral_cleanup_state(
                project_root,
                jobs_root,
                &run_id,
                &job_ids,
                true,
                EphemeralCleanupState::Pending,
                None,
            );
            continue;
        }

        let Some(manifest) = manifest else {
            let detail = format!(
                "ephemeral run {} is terminal but its workflow manifest is missing",
                run_id
            );
            persist_ephemeral_cleanup_state(
                project_root,
                jobs_root,
                &run_id,
                &job_ids,
                true,
                EphemeralCleanupState::Degraded,
                Some(&detail),
            );
            events.push(EphemeralRunCleanupEvent {
                run_id,
                state: EphemeralCleanupState::Degraded,
                prune_vizier_root: false,
                detail: Some(detail),
                removed: None,
                degraded_notes: Vec::new(),
            });
            continue;
        };
        let prune_vizier_root = manifest
            .ephemeral_baseline
            .as_ref()
            .map(|baseline| !baseline.vizier_root_existed)
            .unwrap_or(false);

        match clean_ephemeral_run_scope(
            project_root,
            jobs_root,
            &run_id,
            records,
            manifest.ephemeral_baseline.unwrap_or_default(),
        ) {
            Ok(outcome) if outcome.degraded => {
                let detail = join_cleanup_reasons(&outcome.degraded_notes);
                persist_ephemeral_cleanup_state(
                    project_root,
                    jobs_root,
                    &run_id,
                    &job_ids,
                    true,
                    EphemeralCleanupState::Degraded,
                    detail.as_deref(),
                );
                events.push(EphemeralRunCleanupEvent {
                    run_id,
                    state: EphemeralCleanupState::Degraded,
                    prune_vizier_root: false,
                    detail,
                    removed: Some(outcome.removed),
                    degraded_notes: outcome.degraded_notes,
                });
            }
            Ok(outcome) => {
                events.push(EphemeralRunCleanupEvent {
                    run_id,
                    state: EphemeralCleanupState::Completed,
                    prune_vizier_root,
                    detail: None,
                    removed: Some(outcome.removed),
                    degraded_notes: Vec::new(),
                });
            }
            Err(err) if err.kind() == CleanJobErrorKind::Guard => {
                let detail = join_cleanup_reasons(err.reasons());
                persist_ephemeral_cleanup_state(
                    project_root,
                    jobs_root,
                    &run_id,
                    &job_ids,
                    true,
                    EphemeralCleanupState::Deferred,
                    detail.as_deref(),
                );
                events.push(EphemeralRunCleanupEvent {
                    run_id,
                    state: EphemeralCleanupState::Deferred,
                    prune_vizier_root: false,
                    detail,
                    removed: None,
                    degraded_notes: Vec::new(),
                });
            }
            Err(err) => {
                let detail = err.to_string();
                persist_ephemeral_cleanup_state(
                    project_root,
                    jobs_root,
                    &run_id,
                    &job_ids,
                    true,
                    EphemeralCleanupState::Degraded,
                    Some(&detail),
                );
                events.push(EphemeralRunCleanupEvent {
                    run_id,
                    state: EphemeralCleanupState::Degraded,
                    prune_vizier_root: false,
                    detail: Some(detail),
                    removed: None,
                    degraded_notes: Vec::new(),
                });
            }
        }
    }

    Ok(events)
}

pub(crate) fn scheduler_tick_locked(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
) -> Result<SchedulerOutcome, Box<dyn std::error::Error>> {
    scheduler_tick_locked_inner(project_root, jobs_root, binary, true)
}

pub(crate) fn scheduler_tick_locked_without_ephemeral_cleanup(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
) -> Result<SchedulerOutcome, Box<dyn std::error::Error>> {
    scheduler_tick_locked_inner(project_root, jobs_root, binary, false)
}

fn scheduler_tick_locked_inner(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    allow_ephemeral_cleanup: bool,
) -> Result<SchedulerOutcome, Box<dyn std::error::Error>> {
    let mut records = list_records(jobs_root)?;
    let mut outcome = SchedulerOutcome::default();

    if records.is_empty() {
        return Ok(outcome);
    }

    let reconciled =
        reconcile_running_job_liveness_locked(project_root, jobs_root, binary, &records)?;
    if !reconciled.is_empty() {
        outcome.updated.extend(reconciled);
        records = list_records(jobs_root)?;
    }

    if records.is_empty() {
        return Ok(outcome);
    }

    let repo = Repository::discover(project_root)?;

    records.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let facts = build_scheduler_facts(&repo, jobs_root, &records)?;
    let decisions = spec::evaluate_all(&facts);

    for mut record in records {
        if job_is_terminal(record.status) || record.status == JobStatus::Running {
            continue;
        }

        let decision = match decisions.get(&record.id) {
            Some(decision) => decision,
            None => continue,
        };

        let mut schedule = record.schedule.clone().unwrap_or_default();
        schedule.wait_reason = decision.wait_reason.clone();
        schedule.waited_on = decision.waited_on.clone();

        match decision.action {
            SchedulerAction::UpdateStatus => {
                if record.status != decision.next_status
                    || record
                        .schedule
                        .as_ref()
                        .and_then(|s| s.wait_reason.as_ref())
                        != schedule.wait_reason.as_ref()
                {
                    record.status = decision.next_status;
                    record.schedule = Some(schedule);
                    persist_record(&paths_for(jobs_root, &record.id), &record)?;
                    outcome.updated.push(record.id.clone());
                }
            }
            SchedulerAction::FailMissingChildArgs => {
                record.status = JobStatus::Failed;
                record.schedule = Some(schedule);
                persist_record(&paths_for(jobs_root, &record.id), &record)?;
                finalize_job(
                    project_root,
                    jobs_root,
                    &record.id,
                    JobStatus::Failed,
                    1,
                    None,
                    None,
                )?;
                outcome.updated.push(record.id.clone());
            }
            SchedulerAction::Start => {
                schedule.wait_reason = None;
                record.schedule = Some(schedule);
                persist_record(&paths_for(jobs_root, &record.id), &record)?;
                start_job(project_root, jobs_root, binary, &record.id)?;
                outcome.started.push(record.id.clone());
            }
        }
    }

    if allow_ephemeral_cleanup {
        let records = list_records(jobs_root)?;
        if !records.is_empty() {
            outcome.ephemeral_run_cleanups =
                reconcile_ephemeral_run_cleanup_locked(project_root, jobs_root, &records)?;
        }
    }

    Ok(outcome)
}

pub fn scheduler_tick(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
) -> Result<SchedulerOutcome, Box<dyn std::error::Error>> {
    let outcome = {
        let _lock = SchedulerLock::acquire(jobs_root)?;
        scheduler_tick_locked(project_root, jobs_root, binary)?
    };
    if outcome
        .ephemeral_run_cleanups
        .iter()
        .any(|event| event.prune_vizier_root)
    {
        let vizier_root = project_root.join(".vizier");
        if vizier_root.exists() && prune_empty_dirs_non_root(&vizier_root)? && vizier_root.exists()
        {
            let _ = fs::remove_dir(&vizier_root);
        }
    }
    Ok(outcome)
}

pub fn scheduler_tick_without_ephemeral_cleanup(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
) -> Result<SchedulerOutcome, Box<dyn std::error::Error>> {
    let _lock = SchedulerLock::acquire(jobs_root)?;
    scheduler_tick_locked_without_ephemeral_cleanup(project_root, jobs_root, binary)
}
