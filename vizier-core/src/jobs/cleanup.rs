use super::*;

pub(crate) fn note_waited(waited_on: &mut Vec<JobWaitKind>, kind: JobWaitKind) {
    if !waited_on.contains(&kind) {
        waited_on.push(kind);
    }
}

pub(crate) fn require_approval_mut(
    record: &mut JobRecord,
) -> Result<&mut JobApproval, Box<dyn std::error::Error>> {
    let schedule = record
        .schedule
        .as_mut()
        .ok_or_else(|| format!("job {} is not configured for approval gating", record.id))?;
    let approval = schedule
        .approval
        .as_mut()
        .ok_or_else(|| format!("job {} is not configured for approval gating", record.id))?;
    if !approval.required {
        return Err(format!("job {} is not configured for approval gating", record.id).into());
    }
    Ok(approval)
}

pub(crate) fn ensure_approval_transitionable(
    record: &JobRecord,
) -> Result<(), Box<dyn std::error::Error>> {
    if record.status == JobStatus::Running {
        return Err(format!("job {} is running", record.id).into());
    }
    if job_is_terminal(record.status) {
        return Err(format!(
            "job {} is terminal ({})",
            record.id,
            status_label(record.status)
        )
        .into());
    }
    Ok(())
}

pub fn approve_job(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    job_id: &str,
) -> Result<ApproveJobOutcome, Box<dyn std::error::Error>> {
    let _lock = SchedulerLock::acquire(jobs_root)?;
    let paths = paths_for(jobs_root, job_id);
    if !paths.record_path.exists() {
        return Err(format!("no background job {}", job_id).into());
    }
    let mut record = load_record(&paths)?;
    ensure_approval_transitionable(&record)?;
    {
        let approval = require_approval_mut(&mut record)?;
        match approval.state {
            JobApprovalState::Pending => {
                approval.state = JobApprovalState::Approved;
                approval.decided_at = Some(Utc::now());
                approval.decided_by = Some(resolve_approval_actor());
                approval.reason = None;
            }
            JobApprovalState::Approved => {
                return Err(format!("job {} is already approved", record.id).into());
            }
            JobApprovalState::Rejected => {
                return Err(format!("job {} approval is already rejected", record.id).into());
            }
        }
    }
    persist_record(&paths, &record)?;

    let scheduler_outcome = scheduler_tick_locked(project_root, jobs_root, binary)?;
    let record = load_record(&paths)?;

    Ok(ApproveJobOutcome {
        record,
        started: scheduler_outcome.started,
        updated: scheduler_outcome.updated,
    })
}

pub fn reject_job(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
    reason: Option<&str>,
) -> Result<JobRecord, Box<dyn std::error::Error>> {
    let _lock = SchedulerLock::acquire(jobs_root)?;
    let paths = paths_for(jobs_root, job_id);
    if !paths.record_path.exists() {
        return Err(format!("no background job {}", job_id).into());
    }
    let mut record = load_record(&paths)?;
    ensure_approval_transitionable(&record)?;
    let reason = reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let rejection_detail = reason
        .clone()
        .unwrap_or_else(|| "approval rejected".to_string());
    {
        let approval = require_approval_mut(&mut record)?;
        match approval.state {
            JobApprovalState::Pending => {
                approval.state = JobApprovalState::Rejected;
                approval.decided_at = Some(Utc::now());
                approval.decided_by = Some(resolve_approval_actor());
                approval.reason = reason.clone();
            }
            JobApprovalState::Approved => {
                return Err(format!("job {} is already approved", record.id).into());
            }
            JobApprovalState::Rejected => {
                return Err(format!("job {} approval is already rejected", record.id).into());
            }
        }
    }
    if let Some(schedule) = record.schedule.as_mut() {
        note_waited(&mut schedule.waited_on, JobWaitKind::Approval);
        schedule.wait_reason = Some(JobWaitReason {
            kind: JobWaitKind::Approval,
            detail: Some(rejection_detail),
        });
    }
    persist_record(&paths, &record)?;

    finalize_job(
        project_root,
        jobs_root,
        job_id,
        JobStatus::BlockedByApproval,
        10,
        None,
        None,
    )
}

#[derive(Debug, Clone, Serialize)]
pub struct RetryOutcome {
    pub requested_job: String,
    pub retry_root: String,
    pub last_successful_point: Vec<String>,
    pub retry_set: Vec<String>,
    pub reset: Vec<String>,
    pub restarted: Vec<String>,
    pub updated: Vec<String>,
}

/// Retry contract:
/// - `retry_root` is the requested job id.
/// - `last_successful_point` is the set of direct predecessors currently succeeded.
/// - `retry_set` is `retry_root` plus all downstream dependents (after/artifact edges).
/// - upstream predecessors are never rewound.
pub fn retry_job(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    requested_job_id: &str,
) -> Result<RetryOutcome, Box<dyn std::error::Error>> {
    retry_job_internal(project_root, jobs_root, binary, requested_job_id, None)
}

pub(crate) fn retry_job_internal(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    requested_job_id: &str,
    propagated_context: Option<&WorkflowExecutionContext>,
) -> Result<RetryOutcome, Box<dyn std::error::Error>> {
    let _lock = SchedulerLock::acquire(jobs_root)?;
    retry_job_internal_locked(
        project_root,
        jobs_root,
        binary,
        requested_job_id,
        propagated_context,
        true,
    )
}

pub(crate) fn retry_job_internal_locked(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    requested_job_id: &str,
    propagated_context: Option<&WorkflowExecutionContext>,
    advance_scheduler: bool,
) -> Result<RetryOutcome, Box<dyn std::error::Error>> {
    let records = list_records(jobs_root)?;
    let graph = ScheduleGraph::new(records);

    if graph.record(requested_job_id).is_none() {
        return Err(format!("no background job {}", requested_job_id).into());
    }

    let retry_root = requested_job_id.to_string();
    let predecessors = collect_retry_predecessors(&graph, &retry_root);
    let last_successful_point = predecessors
        .into_iter()
        .filter(|job_id| {
            graph
                .record(job_id)
                .map(|record| record.status == JobStatus::Succeeded)
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    let retry_set = collect_retry_set(&graph, &retry_root);

    let mut running = Vec::new();
    for job_id in &retry_set {
        let Some(record) = graph.record(job_id) else {
            continue;
        };
        if record.status == JobStatus::Running {
            running.push(format!("{} ({})", record.id, status_label(record.status)));
        }
    }
    if !running.is_empty() {
        return Err(format!(
            "cannot retry while jobs are running in the retry set: {}",
            running.join(", ")
        )
        .into());
    }

    let mut merge_retry_slugs = HashSet::new();
    for job_id in &retry_set {
        if let Some(record) = graph.record(job_id) {
            merge_retry_slugs.extend(collect_merge_retry_slugs(record));
        }
    }
    if !merge_retry_slugs.is_empty() {
        ensure_retry_git_state_safe(project_root)?;
        remove_merge_sentinel_files(project_root, &merge_retry_slugs)?;
    }

    let mut reset = Vec::new();
    for job_id in &retry_set {
        let paths = paths_for(jobs_root, job_id);
        let mut record = load_record(&paths)?;
        rewind_job_record_for_retry(project_root, jobs_root, &mut record)?;
        persist_record(&paths, &record)?;
        reset.push(job_id.clone());
    }

    if let Some(context) = propagated_context {
        let _ = apply_workflow_execution_context(jobs_root, requested_job_id, context, true)?;
    }

    let retry_lookup = retry_set.iter().cloned().collect::<HashSet<_>>();
    let (restarted, updated) = if advance_scheduler {
        let scheduler_outcome = scheduler_tick_locked(project_root, jobs_root, binary)?;
        let restarted = scheduler_outcome
            .started
            .iter()
            .filter(|job_id| retry_lookup.contains(*job_id))
            .cloned()
            .collect::<Vec<_>>();
        let updated = scheduler_outcome
            .updated
            .iter()
            .filter(|job_id| retry_lookup.contains(*job_id))
            .cloned()
            .collect::<Vec<_>>();
        (restarted, updated)
    } else {
        (Vec::new(), Vec::new())
    };

    Ok(RetryOutcome {
        requested_job: requested_job_id.to_string(),
        retry_root,
        last_successful_point,
        retry_set,
        reset,
        restarted,
        updated,
    })
}

pub(crate) fn collect_retry_predecessors(graph: &ScheduleGraph, job_id: &str) -> Vec<String> {
    let mut predecessors = graph
        .after_for(job_id)
        .into_iter()
        .map(|dependency| dependency.job_id)
        .collect::<Vec<_>>();

    for dependency in graph.dependencies_for(job_id) {
        predecessors.extend(graph.producers_for(&dependency));
    }

    predecessors.sort();
    predecessors.dedup();

    let predecessor_lookup = predecessors.into_iter().collect::<HashSet<_>>();
    graph
        .job_ids_sorted()
        .into_iter()
        .filter(|candidate| predecessor_lookup.contains(candidate))
        .collect::<Vec<_>>()
}

pub(crate) fn collect_retry_set(graph: &ScheduleGraph, retry_root: &str) -> Vec<String> {
    if graph.record(retry_root).is_none() {
        return Vec::new();
    }

    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    seen.insert(retry_root.to_string());
    queue.push_back(retry_root.to_string());

    while let Some(job_id) = queue.pop_front() {
        let mut dependents = graph.after_dependents_for(&job_id);
        for artifact in graph.artifacts_for(&job_id) {
            dependents.extend(graph.consumers_for(&artifact));
        }
        dependents.sort();
        dependents.dedup();

        for dependent in dependents {
            if seen.insert(dependent.clone()) {
                queue.push_back(dependent);
            }
        }
    }

    let mut ordered = graph
        .job_ids_sorted()
        .into_iter()
        .filter(|job_id| seen.contains(job_id))
        .collect::<Vec<_>>();
    if let Some(index) = ordered.iter().position(|job_id| job_id == retry_root) {
        let root = ordered.remove(index);
        ordered.insert(0, root);
    }
    ordered
}

pub(crate) fn collect_merge_retry_slugs(record: &JobRecord) -> HashSet<String> {
    let mut slugs = HashSet::new();

    if let Some(schedule) = record.schedule.as_ref() {
        for dependency in &schedule.dependencies {
            if let JobArtifact::MergeSentinel { slug } = &dependency.artifact {
                slugs.insert(slug.clone());
            }
        }
        for artifact in &schedule.artifacts {
            if let JobArtifact::MergeSentinel { slug } = artifact {
                slugs.insert(slug.clone());
            }
        }
        for lock in &schedule.locks {
            if let Some(slug) = lock.key.strip_prefix("merge_sentinel:")
                && !slug.trim().is_empty()
            {
                slugs.insert(slug.trim().to_string());
            }
        }
    }

    if record
        .metadata
        .as_ref()
        .and_then(|meta| meta.command_alias.as_deref().or(meta.scope.as_deref()))
        == Some("merge")
        && let Some(slug) = record.metadata.as_ref().and_then(|meta| meta.plan.as_ref())
    {
        slugs.insert(slug.clone());
    }

    slugs
}

pub(crate) fn ensure_retry_git_state_safe(
    project_root: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repository::discover(project_root)?;
    let git_dir = repo.path();
    let merge_head = git_dir.join("MERGE_HEAD");
    let cherry_pick_head = git_dir.join("CHERRY_PICK_HEAD");
    if merge_head.exists() || cherry_pick_head.exists() {
        return Err("cannot retry merge-related jobs while Git has an in-progress merge/cherry-pick; run `git merge --abort` or `git cherry-pick --abort` (or resolve/commit), then retry".into());
    }
    Ok(())
}

pub(crate) fn remove_merge_sentinel_files(
    project_root: &Path,
    slugs: &HashSet<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if slugs.is_empty() {
        return Ok(());
    }

    let sentinel_root = project_root.join(".vizier/tmp/merge-conflicts");
    for slug in slugs {
        let sentinel = sentinel_root.join(format!("{slug}.json"));
        remove_file_if_exists(&sentinel)?;
    }

    if sentinel_root.exists() {
        let mut entries = fs::read_dir(&sentinel_root)?;
        if entries.next().is_none() {
            let _ = fs::remove_dir(&sentinel_root);
        }
    }

    Ok(())
}

pub(crate) fn remove_file_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

pub(crate) fn truncate_log(path: &Path) -> io::Result<()> {
    File::create(path).map(|_| ())
}

pub(crate) fn rewind_job_record_for_retry(
    project_root: &Path,
    jobs_root: &Path,
    record: &mut JobRecord,
) -> Result<(), Box<dyn std::error::Error>> {
    let retry_cleanup = attempt_retry_cleanup(project_root, record);
    if retry_cleanup.status == RetryCleanupStatus::Degraded {
        let detail = retry_cleanup
            .detail
            .as_deref()
            .unwrap_or("retry cleanup degraded");
        display::warn(format!(
            "retry cleanup degraded for {}: {}; worktree metadata retained for future cleanup",
            record.id, detail
        ));
    }

    let paths = paths_for(jobs_root, &record.id);
    if let Some(outcome_path) = record.outcome_path.take() {
        let outcome = resolve_recorded_path(project_root, &outcome_path);
        remove_file_if_exists(&outcome)?;
    }
    remove_file_if_exists(&paths.job_dir.join("outcome.json"))?;
    remove_file_if_exists(&command_patch_path(jobs_root, &record.id))?;
    remove_file_if_exists(&legacy_command_patch_path(jobs_root, &record.id))?;
    remove_file_if_exists(&save_input_patch_path(jobs_root, &record.id))?;
    if let Some(schedule) = record.schedule.as_ref() {
        remove_custom_artifact_markers(project_root, &record.id, &schedule.artifacts)?;
    }
    truncate_log(&paths.stdout_path)?;
    truncate_log(&paths.stderr_path)?;

    if let Some(schedule) = record.schedule.as_mut() {
        schedule.wait_reason = None;
        schedule.waited_on.clear();
        if let Some(approval) = schedule.approval.as_mut()
            && approval.required
        {
            approval.state = JobApprovalState::Pending;
            approval.requested_at = Utc::now();
            approval.requested_by = Some(resolve_approval_actor());
            approval.decided_at = None;
            approval.decided_by = None;
            approval.reason = None;
        }
    }

    if let Some(metadata) = record.metadata.as_mut() {
        if retry_cleanup.should_clear_worktree_metadata() {
            metadata.worktree_name = None;
            metadata.worktree_path = None;
            metadata.worktree_owned = None;
            metadata.execution_root = Some(".".to_string());
        }
        let next_attempt = metadata
            .workflow_node_attempt
            .unwrap_or(1)
            .saturating_add(1);
        metadata.workflow_node_attempt = Some(next_attempt);
        metadata.workflow_node_outcome = None;
        metadata.workflow_payload_refs = None;
        metadata.agent_exit_code = None;
        metadata.cancel_cleanup_status = None;
        metadata.cancel_cleanup_error = None;
        metadata.retry_cleanup_status = Some(retry_cleanup.status);
        metadata.retry_cleanup_error = retry_cleanup.detail.clone();
    }

    record.status = JobStatus::Queued;
    record.started_at = None;
    record.finished_at = None;
    record.pid = None;
    record.exit_code = None;
    record.session_path = None;

    Ok(())
}

pub struct CancelJobOutcome {
    pub record: JobRecord,
    pub cleanup: CancelCleanupResult,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CleanScope {
    Job,
    Run,
}

impl CleanScope {
    pub fn label(self) -> &'static str {
        match self {
            CleanScope::Job => "job",
            CleanScope::Run => "run",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct CleanRemovedCounts {
    pub jobs: usize,
    pub run_manifests: usize,
    pub artifact_markers: usize,
    pub artifact_payloads: usize,
    pub plan_state_deleted: usize,
    pub plan_state_rewritten: usize,
    pub worktrees: usize,
    pub branches: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct CleanSkippedItems {
    #[serde(default)]
    pub branches: Vec<String>,
    #[serde(default)]
    pub worktrees: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CleanJobOutcome {
    pub scope: CleanScope,
    pub requested_job_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub removed: CleanRemovedCounts,
    pub skipped: CleanSkippedItems,
    pub degraded: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub degraded_notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CleanJobOptions {
    pub requested_job_id: String,
    pub keep_branches: bool,
    pub force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanJobErrorKind {
    NotFound,
    Guard,
    Other,
}

#[derive(Debug, Clone)]
pub struct CleanJobError {
    kind: CleanJobErrorKind,
    message: String,
    reasons: Vec<String>,
}

impl CleanJobError {
    fn not_found(job_id: &str) -> Self {
        Self {
            kind: CleanJobErrorKind::NotFound,
            message: format!("job {job_id} not found"),
            reasons: Vec::new(),
        }
    }

    fn guard(reasons: Vec<String>) -> Self {
        Self {
            kind: CleanJobErrorKind::Guard,
            message: "cleanup blocked by safety guards".to_string(),
            reasons,
        }
    }

    fn other(message: impl Into<String>) -> Self {
        Self {
            kind: CleanJobErrorKind::Other,
            message: message.into(),
            reasons: Vec::new(),
        }
    }

    pub fn kind(&self) -> CleanJobErrorKind {
        self.kind
    }

    pub fn reasons(&self) -> &[String] {
        &self.reasons
    }
}

impl std::fmt::Display for CleanJobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.reasons.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}: {}", self.message, self.reasons.join("; "))
        }
    }
}

impl std::error::Error for CleanJobError {}

pub fn cancel_job_with_cleanup(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
    cleanup_worktree: bool,
) -> Result<CancelJobOutcome, Box<dyn std::error::Error>> {
    let record = read_record(jobs_root, job_id)?;
    if !job_is_active(record.status) {
        return Err(format!("job {job_id} is not active").into());
    }

    let cleanup = if let Some(pid) = record.pid {
        let status = Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()?;
        if !status.success() {
            return Err(format!("failed to signal job {job_id} (pid {pid})").into());
        }

        if cleanup_worktree {
            attempt_cancel_cleanup(project_root, &record, pid)
        } else {
            CancelCleanupResult::skipped()
        }
    } else {
        CancelCleanupResult::skipped()
    };

    let cleanup_metadata = JobMetadata {
        cancel_cleanup_status: Some(cleanup.status),
        cancel_cleanup_error: cleanup.error.clone(),
        ..JobMetadata::default()
    };

    let record = finalize_job(
        project_root,
        jobs_root,
        job_id,
        JobStatus::Cancelled,
        143,
        None,
        Some(cleanup_metadata),
    )?;

    Ok(CancelJobOutcome { record, cleanup })
}

pub fn record_job_worktree(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
    worktree_name: Option<&str>,
    worktree_path: &Path,
) -> Result<JobRecord, Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    if !paths.record_path.exists() {
        return Err(format!("no background job {}", job_id).into());
    }

    let mut record = load_record(&paths)?;
    let update = JobMetadata {
        execution_root: Some(relative_path(project_root, worktree_path)),
        worktree_owned: Some(true),
        worktree_path: Some(relative_path(project_root, worktree_path)),
        worktree_name: worktree_name.map(|name| name.to_string()),
        ..JobMetadata::default()
    };

    record.metadata = merge_metadata(record.metadata.take(), Some(update));
    persist_record(&paths, &record)?;
    Ok(record)
}

pub fn record_current_job_worktree(
    project_root: &Path,
    worktree_name: Option<&str>,
    worktree_path: &Path,
) {
    let Some(job_id) = current_job_id() else {
        return;
    };

    let jobs_root = match ensure_jobs_root(project_root) {
        Ok(path) => path,
        Err(err) => {
            display::warn(format!(
                "unable to ensure jobs root for worktree recording: {err}"
            ));
            return;
        }
    };

    if let Err(err) = record_job_worktree(
        project_root,
        &jobs_root,
        &job_id,
        worktree_name,
        worktree_path,
    ) {
        display::warn(format!(
            "unable to record worktree metadata for job {}: {err}",
            job_id
        ));
    }
}

pub(crate) fn attempt_cancel_cleanup(
    project_root: &Path,
    record: &JobRecord,
    pid: u32,
) -> CancelCleanupResult {
    if !wait_for_exit(pid, StdDuration::from_secs(2)) {
        return CancelCleanupResult::skipped();
    }

    let Some(metadata) = record.metadata.as_ref() else {
        return CancelCleanupResult::skipped();
    };

    if metadata.worktree_owned != Some(true) {
        return CancelCleanupResult::skipped();
    }

    let Some(worktree_path) = metadata.worktree_path.as_ref() else {
        return CancelCleanupResult::skipped();
    };

    let worktree_path = resolve_recorded_path(project_root, worktree_path);
    let worktree_name = metadata.worktree_name.as_deref();
    if !worktree_safe_to_remove(project_root, &worktree_path, worktree_name) {
        return CancelCleanupResult::skipped();
    }

    match cleanup_worktree(project_root, &worktree_path, worktree_name) {
        Ok(()) => CancelCleanupResult {
            status: CancelCleanupStatus::Done,
            error: None,
        },
        Err(err) => CancelCleanupResult {
            status: CancelCleanupStatus::Failed,
            error: Some(err),
        },
    }
}

pub(crate) fn attempt_retry_cleanup(project_root: &Path, record: &JobRecord) -> RetryCleanupResult {
    let Some(metadata) = record.metadata.as_ref() else {
        return RetryCleanupResult::skipped();
    };

    if metadata.worktree_owned != Some(true) {
        return RetryCleanupResult::skipped();
    }

    let Some(recorded_path) = metadata.worktree_path.as_ref() else {
        return RetryCleanupResult::degraded(
            "worktree metadata is incomplete (missing worktree_path)",
        );
    };

    let worktree_path = resolve_recorded_path(project_root, recorded_path);
    let worktree_name = metadata.worktree_name.as_deref();
    if !worktree_safe_to_remove(project_root, &worktree_path, worktree_name) {
        return RetryCleanupResult::degraded(format!(
            "refusing to clean unsafe worktree path {}",
            worktree_path.display()
        ));
    }

    match cleanup_worktree(project_root, &worktree_path, worktree_name) {
        Ok(()) => RetryCleanupResult::done(),
        Err(err) => RetryCleanupResult::degraded(err),
    }
}

pub(crate) fn resolve_recorded_path(project_root: &Path, recorded: &str) -> PathBuf {
    let path = PathBuf::from(recorded);
    if path.is_absolute() {
        path
    } else {
        project_root.join(path)
    }
}

pub(crate) fn worktree_safe_to_remove(
    project_root: &Path,
    worktree_path: &Path,
    worktree_name: Option<&str>,
) -> bool {
    let tmp_root = project_root.join(".vizier/tmp-worktrees");
    if !worktree_path.starts_with(&tmp_root) {
        return false;
    }

    if let Some(name) = worktree_name
        && name.starts_with("vizier-workspace-")
    {
        return false;
    }

    if let Some(dir_name) = worktree_path.file_name().and_then(|name| name.to_str())
        && dir_name.starts_with("workspace-")
    {
        return false;
    }

    true
}

pub(crate) fn cleanup_worktree(
    project_root: &Path,
    worktree_path: &Path,
    worktree_name: Option<&str>,
) -> Result<(), String> {
    let repo = Repository::open(project_root).map_err(|err| err.to_string())?;
    let mut errors = Vec::new();
    let mut candidates = Vec::new();
    if let Some(name) = worktree_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        candidates.push(name.to_string());
    }
    if let Some(name) = find_worktree_name_by_path(&repo, worktree_path)
        && !candidates.iter().any(|candidate| candidate == &name)
    {
        candidates.push(name);
    }

    if !candidates.is_empty() {
        let mut pruned = false;
        let mut prune_errors = Vec::new();
        for candidate in &candidates {
            match prune_worktree(&repo, candidate) {
                Ok(()) => {
                    pruned = true;
                    break;
                }
                Err(err) => prune_errors.push((candidate.clone(), err.to_string())),
            }
        }

        if !pruned {
            let fallback_detail = fallback_cleanup_detail(worktree_path, &candidates);
            if let Some((name, prune_error)) = prune_errors.last() {
                if prune_error_mentions_missing_shallow(prune_error) {
                    errors.push(format!(
                        "libgit2 prune for worktree {} could not stat .git/shallow; fallback cleanup failed: {}",
                        name, fallback_detail
                    ));
                } else {
                    errors.push(format!(
                        "failed to prune worktree {}: {}; fallback cleanup failed: {}",
                        name, prune_error, fallback_detail
                    ));
                }
            }
        }
    }

    if worktree_path.exists()
        && let Err(err) = fs::remove_dir_all(worktree_path)
    {
        errors.push(format!(
            "failed to remove worktree directory {}: {}",
            worktree_path.display(),
            err
        ));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

pub(crate) fn prune_worktree(repo: &Repository, worktree_name: &str) -> Result<(), git2::Error> {
    let worktree = repo.find_worktree(worktree_name)?;
    let mut opts = WorktreePruneOptions::new();
    opts.valid(true).locked(true).working_tree(true);
    worktree.prune(Some(&mut opts))
}

pub(crate) fn prune_error_mentions_missing_shallow(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains(".git/shallow")
        && (lower.contains("could not find")
            || lower.contains("couldn't find")
            || lower.contains("no such file")
            || lower.contains("to stat"))
}

pub(crate) fn fallback_cleanup_detail(worktree_path: &Path, candidates: &[String]) -> String {
    if worktree_path.join(".git").is_file() {
        format!(
            "unable to prune worktree metadata via candidates [{}]",
            candidates.join(", ")
        )
    } else {
        format!(
            "no registered worktree matched path {} (candidates: [{}])",
            worktree_path.display(),
            candidates.join(", ")
        )
    }
}

pub(crate) fn find_worktree_name_by_path(
    repo: &Repository,
    worktree_path: &Path,
) -> Option<String> {
    let target = worktree_path.canonicalize().ok()?;
    let worktrees = repo.worktrees().ok()?;
    for name in worktrees.iter().flatten() {
        if let Ok(worktree) = repo.find_worktree(name)
            && worktree.path().canonicalize().ok() == Some(target.clone())
        {
            return Some(name.to_string());
        }
    }
    None
}

pub(crate) fn wait_for_exit(pid: u32, timeout: StdDuration) -> bool {
    let start = std::time::Instant::now();
    loop {
        if !pid_is_running(pid) {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        thread::sleep(StdDuration::from_millis(100));
    }
}

pub(crate) fn pid_is_running(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[derive(Debug, Clone)]
pub(crate) struct CleanScopedWorktree {
    job_id: String,
    worktree_name: Option<String>,
    worktree_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct CleanScopeInventory {
    scope: CleanScope,
    job_ids: Vec<String>,
    run_id: Option<String>,
    worktrees: Vec<CleanScopedWorktree>,
    branches: Vec<String>,
    plan_state_refs: Vec<PathBuf>,
    plan_state_scan_errors: Vec<String>,
}

#[derive(Debug, Default)]
pub(crate) struct CleanSafetyEvaluation {
    active_scoped: Vec<String>,
    active_after_dependents: Vec<String>,
    active_artifact_dependents: Vec<String>,
}

pub fn clean_job_scope(
    project_root: &Path,
    jobs_root: &Path,
    options: CleanJobOptions,
) -> Result<CleanJobOutcome, CleanJobError> {
    let _lock =
        SchedulerLock::acquire(jobs_root).map_err(|err| CleanJobError::other(err.to_string()))?;
    let records = list_records(jobs_root).map_err(|err| CleanJobError::other(err.to_string()))?;
    let inventory =
        resolve_clean_scope_inventory(project_root, &records, &options.requested_job_id)?;

    let scoped_job_ids = inventory.job_ids.iter().cloned().collect::<HashSet<_>>();
    let safety = evaluate_cleanup_safety(&records, &scoped_job_ids);

    if !safety.active_scoped.is_empty() {
        return Err(CleanJobError::guard(safety.active_scoped));
    }

    let mut bypassable_reasons = Vec::new();
    bypassable_reasons.extend(safety.active_after_dependents);
    bypassable_reasons.extend(safety.active_artifact_dependents);

    if !options.force && !bypassable_reasons.is_empty() {
        return Err(CleanJobError::guard(bypassable_reasons));
    }
    if options.force {
        for reason in &bypassable_reasons {
            display::warn(format!("--force: {reason}"));
        }
    }

    let mut outcome = CleanJobOutcome {
        scope: inventory.scope,
        requested_job_id: options.requested_job_id,
        run_id: inventory.run_id.clone(),
        removed: CleanRemovedCounts::default(),
        skipped: CleanSkippedItems::default(),
        degraded: false,
        degraded_notes: Vec::new(),
    };

    for note in inventory.plan_state_scan_errors {
        mark_clean_degraded(&mut outcome, note);
    }

    clean_scoped_worktrees(project_root, &inventory.worktrees, &mut outcome);
    remove_scoped_job_dirs(jobs_root, &inventory.job_ids, &mut outcome);
    remove_scoped_artifact_files(project_root, &scoped_job_ids, &mut outcome);
    delete_run_manifest_if_needed(project_root, inventory.run_id.as_deref(), &mut outcome);
    rewrite_scoped_plan_state_refs(&inventory.plan_state_refs, &scoped_job_ids, &mut outcome);

    if !options.keep_branches {
        delete_scoped_branches(project_root, &inventory.branches, &mut outcome);
    }

    if let Err(err) = prune_empty_artifact_dirs(project_root) {
        mark_clean_degraded(
            &mut outcome,
            format!("unable to prune empty artifact directories: {err}"),
        );
    }

    outcome.skipped.branches.sort();
    outcome.skipped.branches.dedup();
    outcome.skipped.worktrees.sort();
    outcome.skipped.worktrees.dedup();
    outcome.degraded_notes.sort();
    outcome.degraded_notes.dedup();
    outcome.degraded = !outcome.degraded_notes.is_empty();

    Ok(outcome)
}

pub(crate) fn resolve_clean_scope_inventory(
    project_root: &Path,
    records: &[JobRecord],
    requested_job_id: &str,
) -> Result<CleanScopeInventory, CleanJobError> {
    let requested = records
        .iter()
        .find(|record| record.id == requested_job_id)
        .ok_or_else(|| CleanJobError::not_found(requested_job_id))?;

    let run_id = requested
        .metadata
        .as_ref()
        .and_then(|meta| meta.workflow_run_id.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    let scope = if run_id.is_some() {
        CleanScope::Run
    } else {
        CleanScope::Job
    };

    let mut job_ids = if let Some(run_id) = run_id.as_deref() {
        records
            .iter()
            .filter(|record| {
                record
                    .metadata
                    .as_ref()
                    .and_then(|meta| meta.workflow_run_id.as_deref())
                    .map(str::trim)
                    == Some(run_id)
            })
            .map(|record| record.id.clone())
            .collect::<Vec<_>>()
    } else {
        vec![requested.id.clone()]
    };
    if job_ids.is_empty() {
        job_ids.push(requested.id.clone());
    }
    job_ids.sort();
    job_ids.dedup();

    let scoped_job_ids = job_ids.iter().cloned().collect::<HashSet<_>>();
    let mut worktrees = Vec::new();
    let mut branches = Vec::new();

    for record in records
        .iter()
        .filter(|record| scoped_job_ids.contains(&record.id))
    {
        if let Some(metadata) = record.metadata.as_ref() {
            if metadata.worktree_owned == Some(true) {
                let worktree_path = metadata
                    .worktree_path
                    .as_deref()
                    .map(|recorded| resolve_recorded_path(project_root, recorded));
                worktrees.push(CleanScopedWorktree {
                    job_id: record.id.clone(),
                    worktree_name: metadata.worktree_name.clone(),
                    worktree_path,
                });
            }

            if let Some(branch) = metadata
                .branch
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                branches.push(branch.to_string());
            }
        }

        if let Some(schedule) = record.schedule.as_ref() {
            branches.extend(collect_candidate_branches_for_schedule(schedule));
        }
    }
    branches.sort();
    branches.dedup();

    let (plan_state_refs, plan_state_scan_errors) =
        collect_plan_state_refs(project_root, &scoped_job_ids);

    Ok(CleanScopeInventory {
        scope,
        job_ids,
        run_id,
        worktrees,
        branches,
        plan_state_refs,
        plan_state_scan_errors,
    })
}

pub(crate) fn collect_candidate_branches_for_schedule(schedule: &JobSchedule) -> Vec<String> {
    let mut branches = Vec::new();
    for artifact in &schedule.artifacts {
        match artifact {
            JobArtifact::PlanBranch { branch, .. }
            | JobArtifact::PlanDoc { branch, .. }
            | JobArtifact::PlanCommits { branch, .. } => {
                let branch = branch.trim();
                if !branch.is_empty() {
                    branches.push(branch.to_string());
                }
            }
            _ => {}
        }
    }
    branches
}

pub(crate) fn collect_plan_state_refs(
    project_root: &Path,
    scoped_job_ids: &HashSet<String>,
) -> (Vec<PathBuf>, Vec<String>) {
    let plan_state_dir = project_root.join(crate::plan::PLAN_STATE_DIR);
    if !plan_state_dir.exists() {
        return (Vec::new(), Vec::new());
    }

    let mut refs = Vec::new();
    let mut errors = Vec::new();
    let entries = match fs::read_dir(&plan_state_dir) {
        Ok(entries) => entries,
        Err(err) => {
            errors.push(format!(
                "unable to read plan state directory {}: {}",
                plan_state_dir.display(),
                err
            ));
            return (refs, errors);
        }
    };

    for entry in entries {
        let Ok(entry) = entry else {
            errors.push(format!(
                "unable to read entry in {}",
                plan_state_dir.display()
            ));
            continue;
        };
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }

        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(err) => {
                errors.push(format!(
                    "unable to read plan state {}: {}",
                    path.display(),
                    err
                ));
                continue;
            }
        };

        let record = match serde_json::from_str::<crate::plan::PlanRecord>(&raw) {
            Ok(record) => record,
            Err(err) => {
                errors.push(format!(
                    "unable to parse plan state {}: {}",
                    path.display(),
                    err
                ));
                continue;
            }
        };

        if plan_record_references_scoped_jobs(&record, scoped_job_ids) {
            refs.push(path);
        }
    }

    refs.sort();
    (refs, errors)
}

pub(crate) fn plan_record_references_scoped_jobs(
    record: &crate::plan::PlanRecord,
    scoped_job_ids: &HashSet<String>,
) -> bool {
    if let Some(job_id) = record.work_ref.as_deref().and_then(parse_workflow_job_ref)
        && scoped_job_ids.contains(job_id)
    {
        return true;
    }

    record
        .job_ids
        .values()
        .any(|job_id| scoped_job_ids.contains(job_id))
}

pub(crate) fn parse_workflow_job_ref(work_ref: &str) -> Option<&str> {
    work_ref
        .trim()
        .strip_prefix("workflow-job:")
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub(crate) fn evaluate_cleanup_safety(
    records: &[JobRecord],
    scoped_job_ids: &HashSet<String>,
) -> CleanSafetyEvaluation {
    let mut evaluation = CleanSafetyEvaluation::default();
    let graph = ScheduleGraph::new(records.to_vec());

    for record in records {
        if scoped_job_ids.contains(&record.id) {
            if job_is_active(record.status) {
                evaluation.active_scoped.push(format!(
                    "scoped job {} is active ({})",
                    record.id,
                    status_label(record.status)
                ));
            }
            continue;
        }

        if !job_is_active(record.status) {
            continue;
        }

        for after in graph.after_for(&record.id) {
            if scoped_job_ids.contains(&after.job_id) {
                evaluation.active_after_dependents.push(format!(
                    "active job {} has --after dependency on scoped job {}",
                    record.id, after.job_id
                ));
            }
        }

        for artifact in graph.dependencies_for(&record.id) {
            let mut producers = graph.producers_for(&artifact);
            if producers.is_empty() {
                continue;
            }
            producers.sort();
            producers.dedup();
            if producers
                .iter()
                .all(|producer| scoped_job_ids.contains(producer))
            {
                evaluation.active_artifact_dependents.push(format!(
                    "active job {} depends on {} produced only by scoped jobs ({})",
                    record.id,
                    format_artifact(&artifact),
                    producers.join(", ")
                ));
            }
        }
    }

    evaluation.active_scoped.sort();
    evaluation.active_scoped.dedup();
    evaluation.active_after_dependents.sort();
    evaluation.active_after_dependents.dedup();
    evaluation.active_artifact_dependents.sort();
    evaluation.active_artifact_dependents.dedup();
    evaluation
}

pub(crate) fn clean_scoped_worktrees(
    project_root: &Path,
    worktrees: &[CleanScopedWorktree],
    outcome: &mut CleanJobOutcome,
) {
    let mut seen_paths = HashSet::new();
    for worktree in worktrees {
        let Some(path) = worktree.worktree_path.as_ref() else {
            let label = format!("{}:<missing-worktree-path>", worktree.job_id);
            outcome.skipped.worktrees.push(label.clone());
            mark_clean_degraded(
                outcome,
                format!(
                    "job {} is marked worktree-owned but has no worktree_path metadata",
                    worktree.job_id
                ),
            );
            continue;
        };

        let path_label = relative_path(project_root, path);
        if !seen_paths.insert(path_label.clone()) {
            continue;
        }

        if !worktree_safe_to_remove(project_root, path, worktree.worktree_name.as_deref()) {
            outcome.skipped.worktrees.push(path_label.clone());
            mark_clean_degraded(
                outcome,
                format!("refusing to clean unsafe worktree path {}", path.display()),
            );
            continue;
        }

        match cleanup_worktree(project_root, path, worktree.worktree_name.as_deref()) {
            Ok(()) => outcome.removed.worktrees += 1,
            Err(err) => {
                outcome.skipped.worktrees.push(path_label.clone());
                mark_clean_degraded(
                    outcome,
                    format!("worktree cleanup degraded for {}: {}", path.display(), err),
                );
            }
        }
    }
}

pub(crate) fn remove_scoped_job_dirs(
    jobs_root: &Path,
    job_ids: &[String],
    outcome: &mut CleanJobOutcome,
) {
    for job_id in job_ids {
        let job_dir = paths_for(jobs_root, job_id).job_dir;
        if !job_dir.exists() {
            continue;
        }
        match fs::remove_dir_all(&job_dir) {
            Ok(()) => outcome.removed.jobs += 1,
            Err(err) => mark_clean_degraded(
                outcome,
                format!(
                    "unable to remove job directory {}: {}",
                    job_dir.display(),
                    err
                ),
            ),
        }
    }
}

pub(crate) fn remove_scoped_artifact_files(
    project_root: &Path,
    scoped_job_ids: &HashSet<String>,
    outcome: &mut CleanJobOutcome,
) {
    let marker_root = project_root.join(".vizier/jobs/artifacts/custom");
    match remove_scoped_artifact_files_in_root(&marker_root, scoped_job_ids) {
        Ok(removed) => outcome.removed.artifact_markers += removed,
        Err(err) => mark_clean_degraded(
            outcome,
            format!(
                "unable to remove artifact markers in {}: {}",
                marker_root.display(),
                err
            ),
        ),
    }

    let payload_root = project_root.join(".vizier/jobs/artifacts/data");
    match remove_scoped_artifact_files_in_root(&payload_root, scoped_job_ids) {
        Ok(removed) => outcome.removed.artifact_payloads += removed,
        Err(err) => mark_clean_degraded(
            outcome,
            format!(
                "unable to remove artifact payloads in {}: {}",
                payload_root.display(),
                err
            ),
        ),
    }
}

pub(crate) fn remove_scoped_artifact_files_in_root(
    root: &Path,
    scoped_job_ids: &HashSet<String>,
) -> io::Result<usize> {
    if !root.exists() {
        return Ok(0);
    }

    let mut removed = 0usize;
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if !scoped_job_ids.contains(stem) {
                continue;
            }
            remove_file_if_exists(&path)?;
            removed += 1;
        }
    }

    Ok(removed)
}

pub(crate) fn delete_run_manifest_if_needed(
    project_root: &Path,
    run_id: Option<&str>,
    outcome: &mut CleanJobOutcome,
) {
    let Some(run_id) = run_id else {
        return;
    };
    let path = workflow_run_manifest_path(project_root, run_id);
    if !path.exists() {
        return;
    }
    match remove_file_if_exists(&path) {
        Ok(()) => outcome.removed.run_manifests += 1,
        Err(err) => mark_clean_degraded(
            outcome,
            format!("unable to remove run manifest {}: {}", path.display(), err),
        ),
    }
}

pub(crate) fn rewrite_scoped_plan_state_refs(
    plan_state_refs: &[PathBuf],
    scoped_job_ids: &HashSet<String>,
    outcome: &mut CleanJobOutcome,
) {
    for path in plan_state_refs {
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(err) => {
                mark_clean_degraded(
                    outcome,
                    format!("unable to read plan state {}: {}", path.display(), err),
                );
                continue;
            }
        };

        let mut record = match serde_json::from_str::<crate::plan::PlanRecord>(&raw) {
            Ok(record) => record,
            Err(err) => {
                mark_clean_degraded(
                    outcome,
                    format!("unable to parse plan state {}: {}", path.display(), err),
                );
                continue;
            }
        };

        let mut changed = false;
        if let Some(scoped_ref_job) = record.work_ref.as_deref().and_then(parse_workflow_job_ref)
            && scoped_job_ids.contains(scoped_ref_job)
        {
            record.work_ref = None;
            changed = true;
        }

        let before = record.job_ids.len();
        record
            .job_ids
            .retain(|_, job_id| !scoped_job_ids.contains(job_id));
        if record.job_ids.len() != before {
            changed = true;
        }

        if !changed {
            continue;
        }

        if record.work_ref.is_none() && record.job_ids.is_empty() {
            match remove_file_if_exists(path) {
                Ok(()) => outcome.removed.plan_state_deleted += 1,
                Err(err) => mark_clean_degraded(
                    outcome,
                    format!("unable to delete plan state {}: {}", path.display(), err),
                ),
            }
            continue;
        }

        match serde_json::to_string_pretty(&record)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))
            .and_then(|contents| fs::write(path, contents))
        {
            Ok(()) => outcome.removed.plan_state_rewritten += 1,
            Err(err) => mark_clean_degraded(
                outcome,
                format!("unable to rewrite plan state {}: {}", path.display(), err),
            ),
        }
    }
}

pub(crate) fn delete_scoped_branches(
    project_root: &Path,
    branches: &[String],
    outcome: &mut CleanJobOutcome,
) {
    let repo = match Repository::open(project_root) {
        Ok(repo) => repo,
        Err(err) => {
            mark_clean_degraded(
                outcome,
                format!("unable to open repository for branch cleanup: {err}"),
            );
            return;
        }
    };

    let mut protected = collect_checked_out_branches_from_linked_worktrees(&repo);
    if let Ok(head) = repo.head()
        && head.is_branch()
        && let Some(name) = head
            .shorthand()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    {
        protected.insert(name.to_string());
    }

    let mut candidates = branches
        .iter()
        .map(|branch| branch.trim())
        .filter(|branch| !branch.is_empty())
        .filter(|branch| branch.starts_with("draft/"))
        .map(|branch| branch.to_string())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();

    for branch in candidates {
        if protected.contains(&branch) {
            outcome.skipped.branches.push(branch);
            continue;
        }

        match repo.find_branch(&branch, git2::BranchType::Local) {
            Ok(mut local_branch) => match local_branch.delete() {
                Ok(()) => outcome.removed.branches += 1,
                Err(err) if branch_delete_checked_out_error(&err) => {
                    outcome.skipped.branches.push(branch);
                }
                Err(err) => mark_clean_degraded(
                    outcome,
                    format!("unable to delete branch {}: {}", branch, err),
                ),
            },
            Err(err) if err.code() == ErrorCode::NotFound => {}
            Err(err) => mark_clean_degraded(
                outcome,
                format!("unable to inspect branch {}: {}", branch, err),
            ),
        }
    }
}

pub(crate) fn collect_checked_out_branches_from_linked_worktrees(
    repo: &Repository,
) -> HashSet<String> {
    let mut branches = HashSet::new();
    let Ok(worktrees) = repo.worktrees() else {
        return branches;
    };

    for name in worktrees.iter().flatten() {
        let Ok(worktree) = repo.find_worktree(name) else {
            continue;
        };
        let Ok(worktree_repo) = Repository::open(worktree.path()) else {
            continue;
        };
        let Ok(head) = worktree_repo.head() else {
            continue;
        };
        if !head.is_branch() {
            continue;
        }
        if let Some(branch) = head
            .shorthand()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            branches.insert(branch.to_string());
        }
    }

    branches
}

pub(crate) fn branch_delete_checked_out_error(err: &git2::Error) -> bool {
    err.message().to_ascii_lowercase().contains("checked out")
}

pub(crate) fn prune_empty_artifact_dirs(project_root: &Path) -> io::Result<()> {
    for root in [
        project_root.join(".vizier/jobs/artifacts/custom"),
        project_root.join(".vizier/jobs/artifacts/data"),
    ] {
        prune_empty_dirs_non_root(&root)?;
    }
    Ok(())
}

pub(crate) fn prune_empty_dirs_non_root(dir: &Path) -> io::Result<bool> {
    if !dir.is_dir() {
        return Ok(false);
    }

    let mut has_entries = false;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            let child_empty = prune_empty_dirs_non_root(&path)?;
            if child_empty {
                let _ = fs::remove_dir(&path);
            } else {
                has_entries = true;
            }
        } else {
            has_entries = true;
        }
    }

    Ok(!has_entries)
}

pub(crate) fn mark_clean_degraded(outcome: &mut CleanJobOutcome, note: impl Into<String>) {
    outcome.degraded_notes.push(note.into());
    outcome.degraded = true;
}

pub fn gc_jobs(
    _project_root: &Path,
    jobs_root: &Path,
    older_than: Duration,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut removed = 0usize;
    let cutoff = Utc::now() - older_than;
    if !jobs_root.exists() {
        return Ok(removed);
    }

    let mut records = Vec::new();
    for entry in fs::read_dir(jobs_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        let Ok(record) = read_record(jobs_root, &id) else {
            continue;
        };
        records.push(record);
    }

    let mut protected = HashSet::new();
    for record in &records {
        if job_is_terminal(record.status) {
            continue;
        }
        if let Some(schedule) = record.schedule.as_ref() {
            for dependency in &schedule.after {
                protected.insert(dependency.job_id.clone());
            }
        }
    }

    for record in records {
        let finished_at = record.finished_at.unwrap_or(record.created_at);
        if job_is_active(record.status) || protected.contains(&record.id) {
            continue;
        }
        if finished_at < cutoff {
            let paths = paths_for(jobs_root, &record.id);
            if paths.job_dir.exists() && fs::remove_dir_all(&paths.job_dir).is_ok() {
                removed += 1;
            }
        }
    }

    // Refresh the top-level jobs root when empty to keep lookups predictable.
    if removed > 0 && jobs_root.exists() && fs::read_dir(jobs_root)?.next().is_none() {
        let _ = fs::remove_dir_all(jobs_root);
        let _ = fs::create_dir_all(jobs_root);
    }

    Ok(removed)
}
