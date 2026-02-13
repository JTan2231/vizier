use chrono::{DateTime, Duration, Utc};
use git2::{Oid, Repository, WorktreePruneOptions};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
    thread,
    time::{Duration as StdDuration, SystemTime, UNIX_EPOCH},
};
use vizier_core::display;
use vizier_core::scheduler::spec::{
    self, AfterDependencyState, JobAfterDependencyStatus, JobPreconditionFact,
    JobPreconditionState, PinnedHeadFact, SchedulerAction, SchedulerFacts,
};
pub use vizier_core::scheduler::{
    AfterPolicy, JobAfterDependency, JobApprovalFact, JobApprovalState, JobArtifact, JobLock,
    JobPrecondition, JobStatus, JobWaitKind, JobWaitReason, LockMode, PinnedHead, format_artifact,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobDependency {
    pub artifact: JobArtifact,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobSchedule {
    #[serde(default)]
    pub after: Vec<JobAfterDependency>,
    #[serde(default)]
    pub dependencies: Vec<JobDependency>,
    #[serde(default)]
    pub locks: Vec<JobLock>,
    #[serde(default)]
    pub artifacts: Vec<JobArtifact>,
    #[serde(default)]
    pub pinned_head: Option<PinnedHead>,
    #[serde(default)]
    pub preconditions: Vec<JobPrecondition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<JobApproval>,
    #[serde(default)]
    pub wait_reason: Option<JobWaitReason>,
    #[serde(default)]
    pub waited_on: Vec<JobWaitKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobApproval {
    #[serde(default)]
    pub required: bool,
    pub state: JobApprovalState,
    pub requested_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl JobApproval {
    pub fn pending(requested_by: Option<String>) -> Self {
        Self {
            required: true,
            state: JobApprovalState::Pending,
            requested_at: Utc::now(),
            requested_by,
            decided_at: None,
            decided_by: None,
            reason: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub status: JobStatus,
    pub command: Vec<String>,
    #[serde(default)]
    pub child_args: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub stdout_path: String,
    pub stderr_path: String,
    pub session_path: Option<String>,
    #[serde(default)]
    pub outcome_path: Option<String>,
    #[serde(default)]
    pub metadata: Option<JobMetadata>,
    #[serde(default)]
    pub config_snapshot: Option<serde_json::Value>,
    #[serde(default)]
    pub schedule: Option<JobSchedule>,
}

#[derive(Debug, Clone)]
pub struct JobPaths {
    pub job_dir: PathBuf,
    pub record_path: PathBuf,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobMetadata {
    pub command_alias: Option<String>,
    pub scope: Option<String>,
    pub target: Option<String>,
    pub plan: Option<String>,
    pub branch: Option<String>,
    pub workflow_template_selector: Option<String>,
    pub workflow_template_id: Option<String>,
    pub workflow_template_version: Option<String>,
    pub workflow_node_id: Option<String>,
    pub workflow_capability_id: Option<String>,
    pub workflow_policy_snapshot_hash: Option<String>,
    pub workflow_gates: Option<Vec<String>>,
    pub build_pipeline: Option<String>,
    pub build_target: Option<String>,
    pub build_review_mode: Option<String>,
    pub build_skip_checks: Option<bool>,
    pub build_keep_branch: Option<bool>,
    pub build_dependencies: Option<Vec<String>>,
    pub patch_file: Option<String>,
    pub patch_index: Option<usize>,
    pub patch_total: Option<usize>,
    pub revision: Option<String>,
    pub worktree_name: Option<String>,
    pub worktree_path: Option<String>,
    pub worktree_owned: Option<bool>,
    pub agent_selector: Option<String>,
    pub agent_backend: Option<String>,
    pub agent_label: Option<String>,
    pub agent_command: Option<Vec<String>>,
    pub config_backend: Option<String>,
    pub config_agent_selector: Option<String>,
    pub config_agent_label: Option<String>,
    pub config_agent_command: Option<Vec<String>>,
    pub background_quiet: Option<bool>,
    pub agent_exit_code: Option<i32>,
    pub cancel_cleanup_status: Option<CancelCleanupStatus>,
    pub cancel_cleanup_error: Option<String>,
    pub retry_cleanup_status: Option<RetryCleanupStatus>,
    pub retry_cleanup_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobOutcome {
    pub id: String,
    pub status: JobStatus,
    pub command: Vec<String>,
    #[serde(default)]
    pub child_args: Vec<String>,
    pub exit_code: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub stdout_path: String,
    pub stderr_path: String,
    pub session_path: Option<String>,
    pub metadata: Option<JobMetadata>,
    #[serde(default)]
    pub config_snapshot: Option<serde_json::Value>,
    #[serde(default)]
    pub schedule: Option<JobSchedule>,
}

#[derive(Clone, Copy, Debug)]
pub enum LogStream {
    Stdout,
    Stderr,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LatestLogStream {
    Stdout,
    Stderr,
}

impl LatestLogStream {
    pub fn label(self) -> &'static str {
        match self {
            LatestLogStream::Stdout => "stdout",
            LatestLogStream::Stderr => "stderr",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatestLogLine {
    pub stream: LatestLogStream,
    pub line: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CancelCleanupStatus {
    Skipped,
    Done,
    Failed,
}

impl CancelCleanupStatus {
    pub fn label(self) -> &'static str {
        match self {
            CancelCleanupStatus::Skipped => "skipped",
            CancelCleanupStatus::Done => "done",
            CancelCleanupStatus::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CancelCleanupResult {
    pub status: CancelCleanupStatus,
    pub error: Option<String>,
}

impl CancelCleanupResult {
    fn skipped() -> Self {
        Self {
            status: CancelCleanupStatus::Skipped,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetryCleanupStatus {
    Skipped,
    Done,
    Degraded,
}

impl RetryCleanupStatus {
    pub fn label(self) -> &'static str {
        match self {
            RetryCleanupStatus::Skipped => "skipped",
            RetryCleanupStatus::Done => "done",
            RetryCleanupStatus::Degraded => "degraded",
        }
    }
}

#[derive(Debug, Clone)]
struct RetryCleanupResult {
    status: RetryCleanupStatus,
    detail: Option<String>,
}

impl RetryCleanupResult {
    fn skipped() -> Self {
        Self {
            status: RetryCleanupStatus::Skipped,
            detail: None,
        }
    }

    fn done() -> Self {
        Self {
            status: RetryCleanupStatus::Done,
            detail: None,
        }
    }

    fn degraded(detail: impl Into<String>) -> Self {
        Self {
            status: RetryCleanupStatus::Degraded,
            detail: Some(detail.into()),
        }
    }

    fn should_clear_worktree_metadata(&self) -> bool {
        !matches!(self.status, RetryCleanupStatus::Degraded)
    }
}

static CURRENT_JOB_ID: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static RECORD_TMP_NONCE: AtomicU64 = AtomicU64::new(0);

fn current_job_id_state() -> &'static Mutex<Option<String>> {
    CURRENT_JOB_ID.get_or_init(|| Mutex::new(None))
}

pub fn set_current_job_id(job_id: Option<String>) {
    let mut state = current_job_id_state().lock().expect("lock current job id");
    *state = job_id;
}

pub fn current_job_id() -> Option<String> {
    current_job_id_state()
        .lock()
        .expect("lock current job id")
        .clone()
}

fn resolve_approval_actor() -> String {
    for key in [
        "VIZIER_APPROVAL_ACTOR",
        "GIT_AUTHOR_NAME",
        "GIT_COMMITTER_NAME",
        "USER",
        "USERNAME",
    ] {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    "unknown".to_string()
}

pub fn pending_job_approval() -> JobApproval {
    JobApproval::pending(Some(resolve_approval_actor()))
}

pub fn approval_state_label(state: JobApprovalState) -> &'static str {
    match state {
        JobApprovalState::Pending => "pending",
        JobApprovalState::Approved => "approved",
        JobApprovalState::Rejected => "rejected",
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ApproveJobOutcome {
    pub record: JobRecord,
    pub started: Vec<String>,
    pub updated: Vec<String>,
}

pub fn ensure_jobs_root(project_root: &Path) -> io::Result<PathBuf> {
    let root = project_root.join(".vizier").join("jobs");
    fs::create_dir_all(&root)?;
    Ok(root)
}

fn scheduler_lock_path(jobs_root: &Path) -> PathBuf {
    jobs_root.join("scheduler.lock")
}

pub struct SchedulerLock {
    path: PathBuf,
}

impl SchedulerLock {
    pub fn acquire(jobs_root: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        fs::create_dir_all(jobs_root)?;
        let path = scheduler_lock_path(jobs_root);
        let mut attempts = 0u32;
        let mut wait_ms = 10u64;
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "pid={}", std::process::id());
                    return Ok(Self { path });
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    attempts += 1;
                    if attempts > 32 {
                        return Err("scheduler is busy; retry the command".into());
                    }
                    thread::sleep(StdDuration::from_millis(wait_ms));
                    wait_ms = (wait_ms * 2).min(80);
                }
                Err(err) => return Err(Box::new(err)),
            }
        }
    }
}

impl Drop for SchedulerLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn paths_for(jobs_root: &Path, job_id: &str) -> JobPaths {
    let job_dir = jobs_root.join(job_id);
    JobPaths {
        record_path: job_dir.join("job.json"),
        stdout_path: job_dir.join("stdout.log"),
        stderr_path: job_dir.join("stderr.log"),
        job_dir,
    }
}

pub(crate) fn command_patch_path(jobs_root: &Path, job_id: &str) -> PathBuf {
    jobs_root.join(job_id).join("command.patch")
}

pub(crate) fn legacy_command_patch_path(jobs_root: &Path, job_id: &str) -> PathBuf {
    jobs_root.join(job_id).join("ask-save.patch")
}

pub(crate) fn save_input_patch_path(jobs_root: &Path, job_id: &str) -> PathBuf {
    jobs_root.join(job_id).join("save-input.patch")
}

fn hex_encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        let hi = byte >> 4;
        let lo = byte & 0x0f;
        out.push(char::from(if hi < 10 {
            b'0' + hi
        } else {
            b'a' + (hi - 10)
        }));
        out.push(char::from(if lo < 10 {
            b'0' + lo
        } else {
            b'a' + (lo - 10)
        }));
    }
    out
}

fn custom_artifact_marker_dir(project_root: &Path, type_id: &str, key: &str) -> PathBuf {
    project_root
        .join(".vizier/jobs/artifacts/custom")
        .join(hex_encode_component(type_id))
        .join(hex_encode_component(key))
}

fn custom_artifact_marker_path(
    project_root: &Path,
    job_id: &str,
    type_id: &str,
    key: &str,
) -> PathBuf {
    custom_artifact_marker_dir(project_root, type_id, key).join(format!("{job_id}.json"))
}

fn custom_artifact_marker_exists(project_root: &Path, type_id: &str, key: &str) -> bool {
    let dir = custom_artifact_marker_dir(project_root, type_id, key);
    if !dir.is_dir() {
        return false;
    }
    fs::read_dir(dir)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

fn write_custom_artifact_markers(
    project_root: &Path,
    job_id: &str,
    artifacts: &[JobArtifact],
) -> Result<(), Box<dyn std::error::Error>> {
    for artifact in artifacts {
        let JobArtifact::Custom { type_id, key } = artifact else {
            continue;
        };
        let marker = custom_artifact_marker_path(project_root, job_id, type_id, key);
        if let Some(parent) = marker.parent() {
            fs::create_dir_all(parent)?;
        }
        let payload = serde_json::json!({
            "job_id": job_id,
            "type_id": type_id,
            "key": key,
            "written_at": Utc::now().to_rfc3339(),
        });
        fs::write(marker, serde_json::to_vec_pretty(&payload)?)?;
    }
    Ok(())
}

fn remove_custom_artifact_markers(
    project_root: &Path,
    job_id: &str,
    artifacts: &[JobArtifact],
) -> Result<(), Box<dyn std::error::Error>> {
    for artifact in artifacts {
        let JobArtifact::Custom { type_id, key } = artifact else {
            continue;
        };
        let marker = custom_artifact_marker_path(project_root, job_id, type_id, key);
        remove_file_if_exists(&marker)?;
        let key_dir = custom_artifact_marker_dir(project_root, type_id, key);
        if key_dir.is_dir() && fs::read_dir(&key_dir)?.next().is_none() {
            let _ = fs::remove_dir(&key_dir);
        }
        if let Some(type_dir) = key_dir.parent()
            && type_dir.is_dir()
            && fs::read_dir(type_dir)?.next().is_none()
        {
            let _ = fs::remove_dir(type_dir);
        }
    }
    Ok(())
}

fn relative_path(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn is_empty_vec(value: &Option<Vec<String>>) -> bool {
    value
        .as_ref()
        .map(|entries| entries.is_empty())
        .unwrap_or(true)
}

fn merge_metadata(
    existing: Option<JobMetadata>,
    update: Option<JobMetadata>,
) -> Option<JobMetadata> {
    match (existing, update) {
        (None, None) => None,
        (Some(meta), None) => Some(meta),
        (None, Some(meta)) => Some(meta),
        (Some(mut base), Some(update)) => {
            if base.command_alias.is_none() {
                base.command_alias = update.command_alias;
            }
            if base.scope.is_none() {
                base.scope = update.scope;
            }
            if base.target.is_none() {
                base.target = update.target;
            }
            if base.plan.is_none() {
                base.plan = update.plan;
            }
            if base.branch.is_none() {
                base.branch = update.branch;
            }
            if base.workflow_template_selector.is_none() {
                base.workflow_template_selector = update.workflow_template_selector;
            }
            if base.workflow_template_id.is_none() {
                base.workflow_template_id = update.workflow_template_id;
            }
            if base.workflow_template_version.is_none() {
                base.workflow_template_version = update.workflow_template_version;
            }
            if base.workflow_node_id.is_none() {
                base.workflow_node_id = update.workflow_node_id;
            }
            if base.workflow_capability_id.is_none() {
                base.workflow_capability_id = update.workflow_capability_id;
            }
            if base.workflow_policy_snapshot_hash.is_none() {
                base.workflow_policy_snapshot_hash = update.workflow_policy_snapshot_hash;
            }
            if is_empty_vec(&base.workflow_gates) {
                base.workflow_gates = update.workflow_gates;
            }
            if base.build_pipeline.is_none() {
                base.build_pipeline = update.build_pipeline;
            }
            if base.build_target.is_none() {
                base.build_target = update.build_target;
            }
            if base.build_review_mode.is_none() {
                base.build_review_mode = update.build_review_mode;
            }
            if base.build_skip_checks.is_none() {
                base.build_skip_checks = update.build_skip_checks;
            }
            if base.build_keep_branch.is_none() {
                base.build_keep_branch = update.build_keep_branch;
            }
            if is_empty_vec(&base.build_dependencies) {
                base.build_dependencies = update.build_dependencies;
            }
            if base.patch_file.is_none() {
                base.patch_file = update.patch_file;
            }
            if base.patch_index.is_none() {
                base.patch_index = update.patch_index;
            }
            if base.patch_total.is_none() {
                base.patch_total = update.patch_total;
            }
            if base.revision.is_none() {
                base.revision = update.revision;
            }
            if base.worktree_name.is_none() {
                base.worktree_name = update.worktree_name;
            }
            if base.worktree_path.is_none() {
                base.worktree_path = update.worktree_path;
            }
            if base.worktree_owned.is_none() {
                base.worktree_owned = update.worktree_owned;
            }
            if base.agent_selector.is_none() {
                base.agent_selector = update.agent_selector;
            }
            if base.agent_backend.is_none() {
                base.agent_backend = update.agent_backend;
            }
            if base.agent_label.is_none() {
                base.agent_label = update.agent_label;
            }
            if is_empty_vec(&base.agent_command) {
                base.agent_command = update.agent_command;
            }
            if base.config_agent_selector.is_none() {
                base.config_agent_selector = update.config_agent_selector;
            }
            if base.config_backend.is_none() {
                base.config_backend = update.config_backend;
            }
            if base.config_agent_label.is_none() {
                base.config_agent_label = update.config_agent_label;
            }
            if is_empty_vec(&base.config_agent_command) {
                base.config_agent_command = update.config_agent_command;
            }
            if base.background_quiet.is_none() {
                base.background_quiet = update.background_quiet;
            }
            if update.agent_exit_code.is_some() {
                base.agent_exit_code = update.agent_exit_code;
            }
            if update.cancel_cleanup_status.is_some() {
                base.cancel_cleanup_status = update.cancel_cleanup_status;
            }
            if update.cancel_cleanup_error.is_some() {
                base.cancel_cleanup_error = update.cancel_cleanup_error;
            }
            if update.retry_cleanup_status.is_some() {
                base.retry_cleanup_status = update.retry_cleanup_status;
            }
            if update.retry_cleanup_error.is_some() {
                base.retry_cleanup_error = update.retry_cleanup_error;
            }
            Some(base)
        }
    }
}

fn persist_record(paths: &JobPaths, record: &JobRecord) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = paths.record_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let nonce = RECORD_TMP_NONCE.fetch_add(1, AtomicOrdering::Relaxed);
    let epoch_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = paths.record_path.with_extension(format!(
        "json.tmp.{}.{}.{}",
        std::process::id(),
        epoch_nanos,
        nonce
    ));
    let contents = serde_json::to_vec_pretty(record)?;
    fs::write(&tmp, contents)?;
    match fs::rename(&tmp, &paths.record_path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            Err(Box::new(err))
        }
    }
}

fn load_record(paths: &JobPaths) -> Result<JobRecord, Box<dyn std::error::Error>> {
    let mut buf = String::new();
    File::open(&paths.record_path)?.read_to_string(&mut buf)?;
    let record: JobRecord = serde_json::from_str(&buf)?;
    Ok(record)
}

fn find_after_cycle(graph: &HashMap<String, Vec<String>>) -> Option<Vec<String>> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum VisitState {
        Visiting,
        Visited,
    }

    fn dfs(
        node: &str,
        graph: &HashMap<String, Vec<String>>,
        states: &mut HashMap<String, VisitState>,
        stack: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        states.insert(node.to_string(), VisitState::Visiting);
        stack.push(node.to_string());

        if let Some(dependencies) = graph.get(node) {
            for dependency in dependencies {
                if let Some(pos) = stack.iter().position(|value| value == dependency) {
                    let mut cycle = stack[pos..].to_vec();
                    cycle.push(dependency.clone());
                    return Some(cycle);
                }

                if !matches!(states.get(dependency), Some(VisitState::Visited))
                    && let Some(cycle) = dfs(dependency, graph, states, stack)
                {
                    return Some(cycle);
                }
            }
        }

        stack.pop();
        states.insert(node.to_string(), VisitState::Visited);
        None
    }

    let mut states = HashMap::new();
    let mut nodes = graph.keys().cloned().collect::<Vec<_>>();
    nodes.sort();
    nodes.dedup();

    for node in nodes {
        if matches!(states.get(&node), Some(VisitState::Visited)) {
            continue;
        }
        let mut stack = Vec::new();
        if let Some(cycle) = dfs(&node, graph, &mut states, &mut stack) {
            return Some(cycle);
        }
    }

    None
}

pub fn resolve_after_dependencies_for_enqueue(
    jobs_root: &Path,
    new_job_id: &str,
    requested_after: &[String],
) -> Result<Vec<JobAfterDependency>, Box<dyn std::error::Error>> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();

    for value in requested_after {
        let dependency = value.trim().to_string();
        if dependency.is_empty() {
            return Err("unknown --after job id: <empty>".into());
        }
        if seen.insert(dependency.clone()) {
            deduped.push(dependency);
        }
    }

    if deduped.iter().any(|dependency| dependency == new_job_id) {
        return Err(format!("invalid --after self dependency: {}", new_job_id).into());
    }

    for dependency in &deduped {
        let paths = paths_for(jobs_root, dependency);
        if !paths.record_path.exists() {
            return Err(format!("unknown --after job id: {}", dependency).into());
        }
        if let Err(err) = load_record(&paths) {
            return Err(
                format!("cannot read job record for --after {}: {}", dependency, err).into(),
            );
        }
    }

    let records = list_records(jobs_root)?;
    let mut after_graph: HashMap<String, Vec<String>> = HashMap::new();
    for record in records {
        let mut dependencies = record
            .schedule
            .as_ref()
            .map(|schedule| {
                schedule
                    .after
                    .iter()
                    .map(|dependency| dependency.job_id.trim().to_string())
                    .filter(|dependency| !dependency.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        dependencies.sort();
        dependencies.dedup();
        if !dependencies.is_empty() {
            after_graph.insert(record.id.clone(), dependencies);
        }
    }

    if !deduped.is_empty() {
        after_graph.insert(new_job_id.to_string(), deduped.clone());
    }

    if let Some(cycle) = find_after_cycle(&after_graph) {
        return Err(format!("invalid --after cycle: {}", cycle.join(" -> ")).into());
    }

    Ok(deduped
        .into_iter()
        .map(|job_id| JobAfterDependency {
            job_id,
            policy: AfterPolicy::Success,
        })
        .collect())
}

#[derive(Debug, Default)]
pub struct SchedulerOutcome {
    pub started: Vec<String>,
    pub updated: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
pub fn enqueue_job(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
    child_args: &[String],
    recorded_args: &[String],
    metadata: Option<JobMetadata>,
    config_snapshot: Option<serde_json::Value>,
    schedule: Option<JobSchedule>,
) -> Result<JobRecord, Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    fs::create_dir_all(&paths.job_dir)?;

    let _ = File::create(&paths.stdout_path)?;
    let _ = File::create(&paths.stderr_path)?;

    let now = Utc::now();
    let record = JobRecord {
        id: job_id.to_string(),
        status: JobStatus::Queued,
        command: recorded_args.to_vec(),
        child_args: child_args.to_vec(),
        created_at: now,
        started_at: None,
        finished_at: None,
        pid: None,
        exit_code: None,
        stdout_path: relative_path(project_root, &paths.stdout_path),
        stderr_path: relative_path(project_root, &paths.stderr_path),
        session_path: None,
        outcome_path: None,
        metadata,
        config_snapshot,
        schedule,
    };

    persist_record(&paths, &record)?;
    Ok(record)
}

#[allow(dead_code, clippy::too_many_arguments)]
pub fn launch_background_job(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    job_id: &str,
    child_args: &[String],
    recorded_args: &[String],
    metadata: Option<JobMetadata>,
    config_snapshot: Option<serde_json::Value>,
    schedule: Option<JobSchedule>,
) -> Result<JobRecord, Box<dyn std::error::Error>> {
    enqueue_job(
        project_root,
        jobs_root,
        job_id,
        child_args,
        recorded_args,
        metadata,
        config_snapshot,
        schedule,
    )?;
    start_job(project_root, jobs_root, binary, job_id)
}

pub fn start_job(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    job_id: &str,
) -> Result<JobRecord, Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    let mut record = load_record(&paths)?;

    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.stdout_path)?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.stderr_path)?;

    let mut child = Command::new(binary);
    child
        .args(&record.child_args)
        .current_dir(project_root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));

    let child = child.spawn()?;
    record.status = JobStatus::Running;
    record.started_at = Some(Utc::now());
    record.pid = Some(child.id());
    persist_record(&paths, &record)?;

    Ok(record)
}

pub fn finalize_job(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
    status: JobStatus,
    exit_code: i32,
    session_path: Option<String>,
    metadata: Option<JobMetadata>,
) -> Result<JobRecord, Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    let mut record = load_record(&paths)?;

    record.status = status;
    record.exit_code = Some(exit_code);
    record.finished_at = Some(Utc::now());

    if let Some(session) = session_path {
        record.session_path = Some(relative_path(project_root, Path::new(&session)));
    }

    record.metadata = merge_metadata(record.metadata.take(), metadata);

    persist_record(&paths, &record)?;

    // Best-effort for partial runs where log paths were missing from older records.
    if record.stdout_path.is_empty() {
        record.stdout_path = relative_path(project_root, &paths.stdout_path);
    }
    if record.stderr_path.is_empty() {
        record.stderr_path = relative_path(project_root, &paths.stderr_path);
    }

    if record.outcome_path.is_none() {
        record.outcome_path = write_outcome_file(project_root, &paths, &record)?;
        persist_record(&paths, &record)?;
    }

    if let Some(schedule) = record.schedule.as_ref() {
        remove_custom_artifact_markers(project_root, job_id, &schedule.artifacts)?;
        if record.status == JobStatus::Succeeded {
            write_custom_artifact_markers(project_root, job_id, &schedule.artifacts)?;
        }
    }

    Ok(record)
}

fn write_outcome_file(
    project_root: &Path,
    paths: &JobPaths,
    record: &JobRecord,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let outcome = JobOutcome {
        id: record.id.clone(),
        status: record.status,
        command: record.command.clone(),
        child_args: record.child_args.clone(),
        exit_code: record.exit_code,
        created_at: record.created_at,
        started_at: record.started_at,
        finished_at: record.finished_at,
        stdout_path: record.stdout_path.clone(),
        stderr_path: record.stderr_path.clone(),
        session_path: record.session_path.clone(),
        metadata: record.metadata.clone(),
        config_snapshot: record.config_snapshot.clone(),
        schedule: record.schedule.clone(),
    };

    let path = paths.job_dir.join("outcome.json");
    serde_json::to_writer_pretty(File::create(&path)?, &outcome)?;
    Ok(Some(relative_path(project_root, &path)))
}

pub fn list_records(jobs_root: &Path) -> Result<Vec<JobRecord>, Box<dyn std::error::Error>> {
    let mut records = Vec::new();
    if !jobs_root.exists() {
        return Ok(records);
    }

    for entry in fs::read_dir(jobs_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let id = entry.file_name();
        let job_id = id.to_string_lossy();
        let paths = paths_for(jobs_root, &job_id);
        if !paths.record_path.exists() {
            continue;
        }

        match load_record(&paths) {
            Ok(record) => records.push(record),
            Err(err) => display::warn(format!("unable to load background job {}: {}", job_id, err)),
        }
    }

    records.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(records)
}

fn job_is_terminal(status: JobStatus) -> bool {
    matches!(
        status,
        JobStatus::Succeeded
            | JobStatus::Failed
            | JobStatus::Cancelled
            | JobStatus::BlockedByDependency
            | JobStatus::BlockedByApproval
    )
}

fn job_is_active(status: JobStatus) -> bool {
    matches!(
        status,
        JobStatus::Queued
            | JobStatus::WaitingOnDeps
            | JobStatus::WaitingOnApproval
            | JobStatus::WaitingOnLocks
            | JobStatus::Running
    )
}

fn artifact_exists(repo: &Repository, artifact: &JobArtifact) -> bool {
    match artifact {
        JobArtifact::PlanBranch { branch, .. } | JobArtifact::PlanCommits { branch, .. } => {
            repo.find_branch(branch, git2::BranchType::Local).is_ok()
        }
        JobArtifact::PlanDoc { slug, branch } => {
            let plan_path = crate::plan::plan_rel_path(slug);
            let Ok(branch_ref) = repo.find_branch(branch, git2::BranchType::Local) else {
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

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleArtifactState {
    Present,
    Missing,
}

impl ScheduleArtifactState {
    pub fn label(self) -> &'static str {
        match self {
            ScheduleArtifactState::Present => "present",
            ScheduleArtifactState::Missing => "missing",
        }
    }
}

pub const SCHEDULE_SNAPSHOT_VERSION: u32 = 1;
pub const SCHEDULE_SNAPSHOT_ORDERING: &str = "created_at_then_job_id";

#[derive(Debug, Clone, Serialize)]
pub struct ScheduleAfterEdge {
    pub policy: AfterPolicy,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScheduleEdge {
    pub from: String,
    pub to: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<ScheduleArtifactState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<ScheduleAfterEdge>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScheduleSnapshotJob {
    pub order: usize,
    pub job_id: String,
    pub slug: Option<String>,
    pub name: String,
    pub status: JobStatus,
    pub wait: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScheduleSnapshot {
    pub version: u32,
    pub ordering: &'static str,
    pub jobs: Vec<ScheduleSnapshotJob>,
    pub edges: Vec<ScheduleEdge>,
}

impl ScheduleSnapshot {
    pub fn empty() -> Self {
        Self {
            version: SCHEDULE_SNAPSHOT_VERSION,
            ordering: SCHEDULE_SNAPSHOT_ORDERING,
            jobs: Vec::new(),
            edges: Vec::new(),
        }
    }

    pub fn new(jobs: Vec<ScheduleSnapshotJob>, edges: Vec<ScheduleEdge>) -> Self {
        Self {
            version: SCHEDULE_SNAPSHOT_VERSION,
            ordering: SCHEDULE_SNAPSHOT_ORDERING,
            jobs,
            edges,
        }
    }
}

pub(crate) struct ScheduleGraph {
    records: HashMap<String, JobRecord>,
    after: HashMap<String, Vec<JobAfterDependency>>,
    after_dependents: HashMap<String, Vec<String>>,
    dependencies: HashMap<String, Vec<JobArtifact>>,
    artifacts: HashMap<String, Vec<JobArtifact>>,
    producers: HashMap<JobArtifact, Vec<String>>,
    consumers: HashMap<JobArtifact, Vec<String>>,
    job_order: Vec<String>,
}

impl ScheduleGraph {
    pub(crate) fn new(records: Vec<JobRecord>) -> Self {
        let mut records_map = HashMap::new();
        for record in records {
            records_map.insert(record.id.clone(), record);
        }

        let mut after = HashMap::new();
        let mut after_dependents: HashMap<String, Vec<String>> = HashMap::new();
        let mut dependencies = HashMap::new();
        let mut artifacts = HashMap::new();
        let mut producers: HashMap<JobArtifact, Vec<String>> = HashMap::new();
        let mut consumers: HashMap<JobArtifact, Vec<String>> = HashMap::new();

        for record in records_map.values() {
            let schedule = record.schedule.as_ref();
            let mut after_deps = schedule
                .map(|sched| sched.after.clone())
                .unwrap_or_default();
            sort_after_dependencies(&mut after_deps);
            after_deps.dedup();
            for dep in &after_deps {
                after_dependents
                    .entry(dep.job_id.clone())
                    .or_default()
                    .push(record.id.clone());
            }
            after.insert(record.id.clone(), after_deps);

            let mut deps = schedule
                .map(|sched| {
                    sched
                        .dependencies
                        .iter()
                        .map(|dep| dep.artifact.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            sort_artifacts(&mut deps);
            dependencies.insert(record.id.clone(), deps);

            let mut produced = schedule
                .map(|sched| sched.artifacts.clone())
                .unwrap_or_default();
            sort_artifacts(&mut produced);
            artifacts.insert(record.id.clone(), produced.clone());

            for artifact in &produced {
                producers
                    .entry(artifact.clone())
                    .or_default()
                    .push(record.id.clone());
            }

            if let Some(schedule) = schedule {
                for dep in &schedule.dependencies {
                    consumers
                        .entry(dep.artifact.clone())
                        .or_default()
                        .push(record.id.clone());
                }
            }
        }

        for list in producers.values_mut() {
            sort_job_ids(list, &records_map);
        }
        for list in consumers.values_mut() {
            sort_job_ids(list, &records_map);
        }
        for list in after_dependents.values_mut() {
            sort_job_ids(list, &records_map);
        }

        let mut job_order = records_map.keys().cloned().collect::<Vec<_>>();
        sort_job_ids(&mut job_order, &records_map);

        Self {
            records: records_map,
            after,
            after_dependents,
            dependencies,
            artifacts,
            producers,
            consumers,
            job_order,
        }
    }

    pub(crate) fn record(&self, job_id: &str) -> Option<&JobRecord> {
        self.records.get(job_id)
    }

    pub(crate) fn job_ids_sorted(&self) -> Vec<String> {
        self.job_order.clone()
    }

    pub(crate) fn dependencies_for(&self, job_id: &str) -> Vec<JobArtifact> {
        self.dependencies.get(job_id).cloned().unwrap_or_default()
    }

    pub(crate) fn after_for(&self, job_id: &str) -> Vec<JobAfterDependency> {
        self.after.get(job_id).cloned().unwrap_or_default()
    }

    pub(crate) fn after_dependents_for(&self, job_id: &str) -> Vec<String> {
        self.after_dependents
            .get(job_id)
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn artifacts_for(&self, job_id: &str) -> Vec<JobArtifact> {
        self.artifacts.get(job_id).cloned().unwrap_or_default()
    }

    pub(crate) fn producers_for(&self, artifact: &JobArtifact) -> Vec<String> {
        self.producers.get(artifact).cloned().unwrap_or_default()
    }

    pub(crate) fn consumers_for(&self, artifact: &JobArtifact) -> Vec<String> {
        self.consumers.get(artifact).cloned().unwrap_or_default()
    }

    pub(crate) fn artifact_state(
        &self,
        repo: &Repository,
        artifact: &JobArtifact,
    ) -> ScheduleArtifactState {
        if artifact_exists(repo, artifact) {
            ScheduleArtifactState::Present
        } else {
            ScheduleArtifactState::Missing
        }
    }

    pub(crate) fn collect_focus_jobs(&self, focus: &str, max_depth: usize) -> HashSet<String> {
        if !self.records.contains_key(focus) {
            return HashSet::new();
        }

        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        seen.insert(focus.to_string());
        queue.push_back((focus.to_string(), 0usize));

        while let Some((job_id, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            let mut neighbors = Vec::new();
            for dep in self.after_for(&job_id) {
                neighbors.push(dep.job_id);
            }
            neighbors.extend(self.after_dependents_for(&job_id));
            for dep in self.dependencies_for(&job_id) {
                neighbors.extend(self.producers_for(&dep));
            }
            for artifact in self.artifacts_for(&job_id) {
                neighbors.extend(self.consumers_for(&artifact));
            }
            neighbors.sort_by(|a, b| self.job_compare(a, b));
            neighbors.dedup();
            for neighbor in neighbors {
                if seen.insert(neighbor.clone()) {
                    queue.push_back((neighbor, depth + 1));
                }
            }
        }

        seen
    }

    pub(crate) fn snapshot_edges(
        &self,
        repo: &Repository,
        roots: &[String],
        max_depth: usize,
    ) -> Vec<ScheduleEdge> {
        let mut edges = Vec::new();

        for root in roots {
            if !self.records.contains_key(root) {
                continue;
            }
            let mut path = HashSet::new();
            path.insert(root.clone());
            self.collect_edges(repo, root, max_depth, &mut path, &mut edges);
        }

        edges
    }

    fn collect_edges(
        &self,
        repo: &Repository,
        job_id: &str,
        depth_remaining: usize,
        path: &mut HashSet<String>,
        edges: &mut Vec<ScheduleEdge>,
    ) {
        if depth_remaining == 0 {
            return;
        }

        for after in self.after_for(job_id) {
            edges.push(ScheduleEdge {
                from: job_id.to_string(),
                to: after.job_id.clone(),
                artifact: None,
                state: None,
                after: Some(ScheduleAfterEdge {
                    policy: after.policy,
                }),
            });
            if depth_remaining > 1 && !path.contains(&after.job_id) {
                path.insert(after.job_id.clone());
                self.collect_edges(repo, &after.job_id, depth_remaining - 1, path, edges);
                path.remove(&after.job_id);
            }
        }

        for dependency in self.dependencies_for(job_id) {
            let artifact_label = format_artifact(&dependency);
            let producers = self.producers_for(&dependency);
            if producers.is_empty() {
                let state = self.artifact_state(repo, &dependency);
                edges.push(ScheduleEdge {
                    from: job_id.to_string(),
                    to: format!("artifact:{artifact_label}"),
                    artifact: Some(artifact_label),
                    state: Some(state),
                    after: None,
                });
                continue;
            }

            for producer_id in producers {
                edges.push(ScheduleEdge {
                    from: job_id.to_string(),
                    to: producer_id.clone(),
                    artifact: Some(artifact_label.clone()),
                    state: None,
                    after: None,
                });
                if depth_remaining > 1 && !path.contains(&producer_id) {
                    path.insert(producer_id.clone());
                    self.collect_edges(repo, &producer_id, depth_remaining - 1, path, edges);
                    path.remove(&producer_id);
                }
            }
        }
    }

    fn job_compare(&self, a: &str, b: &str) -> Ordering {
        match (self.records.get(a), self.records.get(b)) {
            (Some(left), Some(right)) => {
                let order = left.created_at.cmp(&right.created_at);
                if order == Ordering::Equal {
                    left.id.cmp(&right.id)
                } else {
                    order
                }
            }
            _ => a.cmp(b),
        }
    }
}

fn artifact_sort_key(artifact: &JobArtifact) -> (u8, &str, &str) {
    match artifact {
        JobArtifact::PlanBranch { slug, branch } => (0, slug, branch),
        JobArtifact::PlanDoc { slug, branch } => (1, slug, branch),
        JobArtifact::PlanCommits { slug, branch } => (2, slug, branch),
        JobArtifact::TargetBranch { name } => (3, name, ""),
        JobArtifact::MergeSentinel { slug } => (4, slug, ""),
        JobArtifact::CommandPatch { job_id } => (5, job_id, ""),
        JobArtifact::Custom { type_id, key } => (6, type_id, key),
    }
}

fn after_policy_sort_key(policy: AfterPolicy) -> u8 {
    match policy {
        AfterPolicy::Success => 0,
    }
}

fn sort_after_dependencies(dependencies: &mut [JobAfterDependency]) {
    dependencies.sort_by(|left, right| {
        let left_key = (left.job_id.as_str(), after_policy_sort_key(left.policy));
        let right_key = (right.job_id.as_str(), after_policy_sort_key(right.policy));
        left_key.cmp(&right_key)
    });
}

fn sort_artifacts(artifacts: &mut [JobArtifact]) {
    artifacts.sort_by(|left, right| artifact_sort_key(left).cmp(&artifact_sort_key(right)));
}

fn sort_job_ids(job_ids: &mut [String], records: &HashMap<String, JobRecord>) {
    job_ids.sort_by(
        |left, right| match (records.get(left), records.get(right)) {
            (Some(left_record), Some(right_record)) => {
                let order = left_record.created_at.cmp(&right_record.created_at);
                if order == Ordering::Equal {
                    left_record.id.cmp(&right_record.id)
                } else {
                    order
                }
            }
            _ => left.cmp(right),
        },
    );
}

fn pinned_head_matches(repo: &Repository, pinned: &PinnedHead) -> Result<bool, git2::Error> {
    let branch_ref = repo.find_branch(&pinned.branch, git2::BranchType::Local)?;
    let commit = branch_ref.into_reference().peel_to_commit()?;
    let expected = Oid::from_str(&pinned.oid).ok();
    Ok(Some(commit.id()) == expected)
}

fn is_ephemeral_vizier_path(path: &str) -> bool {
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

fn clean_worktree_matches(repo: &Repository) -> Result<bool, git2::Error> {
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

fn branch_from_locks(locks: &[JobLock]) -> Option<String> {
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

fn resolve_branch_precondition_target(
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

fn evaluate_precondition(
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

fn resolve_after_dependency_state(
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

fn build_scheduler_facts(
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

fn scheduler_tick_locked(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
) -> Result<SchedulerOutcome, Box<dyn std::error::Error>> {
    let mut records = list_records(jobs_root)?;
    let mut outcome = SchedulerOutcome::default();

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

    Ok(outcome)
}

pub fn scheduler_tick(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
) -> Result<SchedulerOutcome, Box<dyn std::error::Error>> {
    let _lock = SchedulerLock::acquire(jobs_root)?;
    scheduler_tick_locked(project_root, jobs_root, binary)
}

fn note_waited(waited_on: &mut Vec<JobWaitKind>, kind: JobWaitKind) {
    if !waited_on.contains(&kind) {
        waited_on.push(kind);
    }
}

fn require_approval_mut(
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

fn ensure_approval_transitionable(record: &JobRecord) -> Result<(), Box<dyn std::error::Error>> {
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
    let _lock = SchedulerLock::acquire(jobs_root)?;
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

    let scheduler_outcome = scheduler_tick_locked(project_root, jobs_root, binary)?;
    let retry_lookup = retry_set.iter().cloned().collect::<HashSet<_>>();
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

fn collect_retry_predecessors(graph: &ScheduleGraph, job_id: &str) -> Vec<String> {
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

fn collect_retry_set(graph: &ScheduleGraph, retry_root: &str) -> Vec<String> {
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

fn collect_merge_retry_slugs(record: &JobRecord) -> HashSet<String> {
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

fn ensure_retry_git_state_safe(project_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repository::discover(project_root)?;
    let git_dir = repo.path();
    let merge_head = git_dir.join("MERGE_HEAD");
    let cherry_pick_head = git_dir.join("CHERRY_PICK_HEAD");
    if merge_head.exists() || cherry_pick_head.exists() {
        return Err("cannot retry merge-related jobs while Git has an in-progress merge/cherry-pick; run `git merge --abort` or `git cherry-pick --abort` (or resolve/commit), then retry".into());
    }
    Ok(())
}

fn remove_merge_sentinel_files(
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

fn remove_file_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn truncate_log(path: &Path) -> io::Result<()> {
    File::create(path).map(|_| ())
}

fn rewind_job_record_for_retry(
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
        }
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

pub fn read_record(
    jobs_root: &Path,
    job_id: &str,
) -> Result<JobRecord, Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    if !paths.record_path.exists() {
        return Err(format!("no background job {}", job_id).into());
    }

    load_record(&paths)
}

pub fn update_job_record<F>(
    jobs_root: &Path,
    job_id: &str,
    updater: F,
) -> Result<JobRecord, Box<dyn std::error::Error>>
where
    F: FnOnce(&mut JobRecord),
{
    let paths = paths_for(jobs_root, job_id);
    if !paths.record_path.exists() {
        return Err(format!("no background job {}", job_id).into());
    }

    let mut record = load_record(&paths)?;
    updater(&mut record);
    persist_record(&paths, &record)?;
    Ok(record)
}

pub fn status_label(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Queued => "queued",
        JobStatus::WaitingOnDeps => "waiting_on_deps",
        JobStatus::WaitingOnApproval => "waiting_on_approval",
        JobStatus::WaitingOnLocks => "waiting_on_locks",
        JobStatus::Running => "running",
        JobStatus::Succeeded => "succeeded",
        JobStatus::Failed => "failed",
        JobStatus::Cancelled => "cancelled",
        JobStatus::BlockedByDependency => "blocked_by_dependency",
        JobStatus::BlockedByApproval => "blocked_by_approval",
    }
}

fn emit_log(path: &Path, offset: u64, label: &str, labeled: bool) -> io::Result<u64> {
    if !path.exists() {
        return Ok(offset);
    }

    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buffer = String::new();
    file.read_to_string(&mut buffer)?;
    let new_offset = file.stream_position()?;

    if !buffer.is_empty() {
        if labeled {
            for line in buffer.lines() {
                println!("[{label}] {line}");
            }
        } else {
            print!("{buffer}");
        }
        let _ = std::io::stdout().flush();
    }

    Ok(new_offset)
}

fn read_log_tail(path: &Path, tail_bytes: usize) -> io::Result<Option<Vec<u8>>> {
    if !path.exists() {
        return Ok(None);
    }

    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(tail_bytes as u64);
    file.seek(SeekFrom::Start(start))?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    Ok(Some(buffer))
}

fn latest_non_empty_line(path: &Path, tail_bytes: usize) -> io::Result<Option<String>> {
    let Some(buffer) = read_log_tail(path, tail_bytes)? else {
        return Ok(None);
    };
    if buffer.is_empty() {
        return Ok(None);
    }

    let text = String::from_utf8_lossy(&buffer);
    let line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.to_string());
    Ok(line)
}

fn latest_log_mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
}

pub fn latest_job_log_line(
    jobs_root: &Path,
    job_id: &str,
    tail_bytes: usize,
) -> io::Result<Option<LatestLogLine>> {
    let paths = paths_for(jobs_root, job_id);
    let stdout_line = latest_non_empty_line(&paths.stdout_path, tail_bytes)?;
    let stderr_line = latest_non_empty_line(&paths.stderr_path, tail_bytes)?;

    match (stdout_line, stderr_line) {
        (Some(line), None) => Ok(Some(LatestLogLine {
            stream: LatestLogStream::Stdout,
            line,
        })),
        (None, Some(line)) => Ok(Some(LatestLogLine {
            stream: LatestLogStream::Stderr,
            line,
        })),
        (Some(stdout), Some(stderr)) => {
            let stdout_mtime = latest_log_mtime(&paths.stdout_path);
            let stderr_mtime = latest_log_mtime(&paths.stderr_path);
            let prefer_stderr = match (stdout_mtime, stderr_mtime) {
                (Some(out), Some(err)) => err >= out,
                (None, Some(_)) => true,
                _ => false,
            };
            if prefer_stderr {
                Ok(Some(LatestLogLine {
                    stream: LatestLogStream::Stderr,
                    line: stderr,
                }))
            } else {
                Ok(Some(LatestLogLine {
                    stream: LatestLogStream::Stdout,
                    line: stdout,
                }))
            }
        }
        (None, None) => Ok(None),
    }
}

fn follow_poll_delay(advanced: bool, idle_polls: &mut u32) -> StdDuration {
    if advanced {
        *idle_polls = 0;
        return StdDuration::from_millis(15);
    }

    *idle_polls = idle_polls.saturating_add(1);
    let millis = match *idle_polls {
        1 => 40,
        2 => 80,
        3 => 160,
        _ => 240,
    };
    StdDuration::from_millis(millis)
}

pub fn tail_job_logs(
    jobs_root: &Path,
    job_id: &str,
    stream: LogStream,
    follow: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    let mut stdout_offset = 0u64;
    let mut stderr_offset = 0u64;
    let mut idle_polls = 0u32;

    let label_stdout = matches!(stream, LogStream::Both);
    let label_stderr = matches!(stream, LogStream::Both);

    loop {
        let mut advanced = false;
        if matches!(stream, LogStream::Stdout | LogStream::Both) {
            let next = emit_log(&paths.stdout_path, stdout_offset, "stdout", label_stdout)?;
            if next != stdout_offset {
                advanced = true;
                stdout_offset = next;
            }
        }

        if matches!(stream, LogStream::Stderr | LogStream::Both) {
            let next = emit_log(&paths.stderr_path, stderr_offset, "stderr", label_stderr)?;
            if next != stderr_offset {
                advanced = true;
                stderr_offset = next;
            }
        }

        if !follow {
            break;
        }

        let record = read_record(jobs_root, job_id)?;
        let running = job_is_active(record.status);
        if !running && !advanced {
            break;
        }

        thread::sleep(follow_poll_delay(advanced, &mut idle_polls));
    }

    Ok(())
}

fn read_log_chunk(path: &Path, offset: u64) -> io::Result<(u64, Vec<u8>)> {
    if !path.exists() {
        return Ok((offset, Vec::new()));
    }

    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let new_offset = file.stream_position()?;
    Ok((new_offset, buffer))
}

pub fn follow_job_logs_raw(
    jobs_root: &Path,
    job_id: &str,
) -> Result<i32, Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    let mut stdout_offset = 0u64;
    let mut stderr_offset = 0u64;
    let mut idle_polls = 0u32;

    loop {
        let mut advanced = false;

        let (next_stdout, stdout_buf) = read_log_chunk(&paths.stdout_path, stdout_offset)?;
        if !stdout_buf.is_empty() {
            io::stdout().write_all(&stdout_buf)?;
            io::stdout().flush()?;
            advanced = true;
        }
        stdout_offset = next_stdout;

        let (next_stderr, stderr_buf) = read_log_chunk(&paths.stderr_path, stderr_offset)?;
        if !stderr_buf.is_empty() {
            io::stderr().write_all(&stderr_buf)?;
            io::stderr().flush()?;
            advanced = true;
        }
        stderr_offset = next_stderr;

        let record = read_record(jobs_root, job_id)?;
        let running = job_is_active(record.status);
        if !running && !advanced {
            return Ok(record.exit_code.unwrap_or(1));
        }

        thread::sleep(follow_poll_delay(advanced, &mut idle_polls));
    }
}

pub struct CancelJobOutcome {
    pub record: JobRecord,
    pub cleanup: CancelCleanupResult,
}

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

fn attempt_cancel_cleanup(
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

fn attempt_retry_cleanup(project_root: &Path, record: &JobRecord) -> RetryCleanupResult {
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

fn resolve_recorded_path(project_root: &Path, recorded: &str) -> PathBuf {
    let path = PathBuf::from(recorded);
    if path.is_absolute() {
        path
    } else {
        project_root.join(path)
    }
}

fn worktree_safe_to_remove(
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

fn cleanup_worktree(
    project_root: &Path,
    worktree_path: &Path,
    worktree_name: Option<&str>,
) -> Result<(), String> {
    let repo = Repository::open(project_root).map_err(|err| err.to_string())?;
    let mut errors = Vec::new();

    if let Some(name) = worktree_name
        .map(|value| value.to_string())
        .or_else(|| find_worktree_name_by_path(&repo, worktree_path))
        && let Err(err) = prune_worktree(&repo, &name)
    {
        let prune_error = err.to_string();
        if let Err(fallback_err) = run_git_worktree_cleanup_fallback(project_root, worktree_path) {
            if prune_error_mentions_missing_shallow(&prune_error) {
                errors.push(format!(
                    "libgit2 prune for worktree {} could not stat .git/shallow; fallback cleanup failed: {}",
                    name, fallback_err
                ));
            } else {
                errors.push(format!(
                    "failed to prune worktree {}: {}; fallback cleanup failed: {}",
                    name, prune_error, fallback_err
                ));
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

fn prune_worktree(repo: &Repository, worktree_name: &str) -> Result<(), git2::Error> {
    let worktree = repo.find_worktree(worktree_name)?;
    let mut opts = WorktreePruneOptions::new();
    opts.valid(true).locked(true).working_tree(true);
    worktree.prune(Some(&mut opts))
}

fn prune_error_mentions_missing_shallow(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains(".git/shallow")
        && (lower.contains("could not find")
            || lower.contains("couldn't find")
            || lower.contains("no such file")
            || lower.contains("to stat"))
}

fn run_git_worktree_cleanup_fallback(
    project_root: &Path,
    worktree_path: &Path,
) -> Result<(), String> {
    let mut errors = Vec::new();
    if let Err(err) = run_git_cleanup_command(
        project_root,
        &["worktree", "remove", "--force"],
        Some(worktree_path),
    ) && worktree_path.exists()
    {
        errors.push(err);
    }

    if let Err(err) = run_git_cleanup_command(
        project_root,
        &["worktree", "prune", "--expire", "now"],
        None,
    ) {
        errors.push(err);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn run_git_cleanup_command(
    project_root: &Path,
    args: &[&str],
    path_arg: Option<&Path>,
) -> Result<(), String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(project_root).args(args);
    if let Some(path) = path_arg {
        cmd.arg(path);
    }

    let output = cmd.output().map_err(|err| {
        format!(
            "failed to execute `git {}`: {}",
            format_git_subcommand(args, path_arg),
            err
        )
    })?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("exit status {}", output.status)
    };
    Err(format!(
        "`git {}` failed: {}",
        format_git_subcommand(args, path_arg),
        detail
    ))
}

fn format_git_subcommand(args: &[&str], path_arg: Option<&Path>) -> String {
    let mut parts = args
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    if let Some(path) = path_arg {
        parts.push(path.display().to_string());
    }
    parts.join(" ")
}

fn find_worktree_name_by_path(repo: &Repository, worktree_path: &Path) -> Option<String> {
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

fn wait_for_exit(pid: u32, timeout: StdDuration) -> bool {
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

fn pid_is_running(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use git2::{BranchType, Signature};
    use std::path::Path;
    use std::sync::{Arc, Barrier};
    use tempfile::TempDir;

    fn init_repo(temp: &TempDir) -> Result<Repository, git2::Error> {
        let repo = Repository::init(temp.path())?;
        Ok(repo)
    }

    fn seed_repo(repo: &Repository) -> Result<Oid, Box<dyn std::error::Error>> {
        let workdir = repo.workdir().ok_or("missing workdir")?;
        let readme = workdir.join("README.md");
        fs::write(&readme, "seed")?;
        let mut index = repo.index()?;
        index.add_path(Path::new("README.md"))?;
        let tree_id = index.write_tree()?;
        let tree = repo.find_tree(tree_id)?;
        let sig = Signature::now("vizier", "vizier@example.com")?;
        let oid = repo.commit(Some("HEAD"), &sig, &sig, "seed", &tree, &[])?;
        Ok(oid)
    }

    fn ensure_branch(repo: &Repository, name: &str) -> Result<(), Box<dyn std::error::Error>> {
        if repo.find_branch(name, BranchType::Local).is_ok() {
            return Ok(());
        }
        let head = repo.head()?.peel_to_commit()?;
        repo.branch(name, &head, false)?;
        Ok(())
    }

    fn commit_plan_doc(
        repo: &Repository,
        slug: &str,
        branch: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let workdir = repo.workdir().ok_or("missing workdir")?;
        let plan_path = crate::plan::plan_rel_path(slug);
        let full_path = workdir.join(&plan_path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&full_path, format!("# plan {}\n", slug))?;

        let mut index = repo.index()?;
        index.add_path(&plan_path)?;
        let tree_id = index.write_tree()?;
        let tree = repo.find_tree(tree_id)?;
        let sig = Signature::now("vizier", "vizier@example.com")?;
        let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
        let parents = parent.iter().collect::<Vec<_>>();
        let refname = format!("refs/heads/{branch}");
        repo.commit(
            Some(refname.as_str()),
            &sig,
            &sig,
            "plan doc",
            &tree,
            &parents,
        )?;
        Ok(())
    }

    fn ensure_artifact_exists(
        repo: &Repository,
        jobs_root: &Path,
        artifact: &JobArtifact,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match artifact {
            JobArtifact::PlanBranch { branch, .. } | JobArtifact::PlanCommits { branch, .. } => {
                ensure_branch(repo, branch)?;
            }
            JobArtifact::PlanDoc { slug, branch } => {
                commit_plan_doc(repo, slug, branch)?;
            }
            JobArtifact::TargetBranch { name } => {
                ensure_branch(repo, name)?;
            }
            JobArtifact::MergeSentinel { slug } => {
                let path = repo
                    .path()
                    .join(".vizier/tmp/merge-conflicts")
                    .join(format!("{slug}.json"));
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(path, "{}")?;
            }
            JobArtifact::CommandPatch { job_id } => {
                let path = command_patch_path(jobs_root, job_id);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(path, "patch")?;
            }
            JobArtifact::Custom { .. } => {
                let project_root = repo.path().parent().ok_or("missing repo root")?;
                write_custom_artifact_markers(
                    project_root,
                    "fixture-artifact-producer",
                    std::slice::from_ref(artifact),
                )?;
            }
        }
        Ok(())
    }

    #[test]
    fn follow_poll_delay_uses_short_backoff_and_resets_on_activity() {
        let mut idle_polls = 0u32;

        assert_eq!(
            follow_poll_delay(false, &mut idle_polls),
            StdDuration::from_millis(40)
        );
        assert_eq!(
            follow_poll_delay(false, &mut idle_polls),
            StdDuration::from_millis(80)
        );
        assert_eq!(
            follow_poll_delay(false, &mut idle_polls),
            StdDuration::from_millis(160)
        );
        assert_eq!(
            follow_poll_delay(false, &mut idle_polls),
            StdDuration::from_millis(240)
        );
        assert_eq!(
            follow_poll_delay(false, &mut idle_polls),
            StdDuration::from_millis(240)
        );

        assert_eq!(
            follow_poll_delay(true, &mut idle_polls),
            StdDuration::from_millis(15)
        );
        assert_eq!(
            follow_poll_delay(false, &mut idle_polls),
            StdDuration::from_millis(40)
        );
    }

    #[test]
    fn latest_job_log_line_returns_stdout_when_only_stdout_has_content() {
        let temp = TempDir::new().expect("temp dir");
        let jobs_root = temp.path().join(".vizier/jobs");
        let paths = paths_for(&jobs_root, "job-stdout-only");
        fs::create_dir_all(&paths.job_dir).expect("create job dir");
        fs::write(&paths.stdout_path, "first line\nlatest stdout\n").expect("write stdout log");

        let latest = latest_job_log_line(&jobs_root, "job-stdout-only", 8 * 1024)
            .expect("resolve latest log line")
            .expect("expected stdout line");
        assert_eq!(latest.stream, LatestLogStream::Stdout);
        assert_eq!(latest.line, "latest stdout");
    }

    #[test]
    fn latest_job_log_line_returns_stderr_when_only_stderr_has_content() {
        let temp = TempDir::new().expect("temp dir");
        let jobs_root = temp.path().join(".vizier/jobs");
        let paths = paths_for(&jobs_root, "job-stderr-only");
        fs::create_dir_all(&paths.job_dir).expect("create job dir");
        fs::write(&paths.stderr_path, "latest stderr\n").expect("write stderr log");

        let latest = latest_job_log_line(&jobs_root, "job-stderr-only", 8 * 1024)
            .expect("resolve latest log line")
            .expect("expected stderr line");
        assert_eq!(latest.stream, LatestLogStream::Stderr);
        assert_eq!(latest.line, "latest stderr");
    }

    #[test]
    fn latest_job_log_line_prefers_newer_stream_when_both_have_content() {
        let temp = TempDir::new().expect("temp dir");
        let jobs_root = temp.path().join(".vizier/jobs");
        let paths = paths_for(&jobs_root, "job-both-streams");
        fs::create_dir_all(&paths.job_dir).expect("create job dir");

        fs::write(&paths.stdout_path, "old stdout\n").expect("write stdout log");
        thread::sleep(StdDuration::from_millis(20));
        fs::write(&paths.stderr_path, "new stderr\n").expect("write stderr log");

        let latest = latest_job_log_line(&jobs_root, "job-both-streams", 8 * 1024)
            .expect("resolve latest log line")
            .expect("expected latest line");
        assert_eq!(latest.stream, LatestLogStream::Stderr);
        assert_eq!(latest.line, "new stderr");
    }

    #[test]
    fn latest_job_log_line_returns_none_for_missing_or_empty_logs() {
        let temp = TempDir::new().expect("temp dir");
        let jobs_root = temp.path().join(".vizier/jobs");
        let paths = paths_for(&jobs_root, "job-empty");
        fs::create_dir_all(&paths.job_dir).expect("create job dir");
        fs::write(&paths.stdout_path, "\n\n").expect("write stdout log");
        fs::write(&paths.stderr_path, "   \n").expect("write stderr log");

        let latest =
            latest_job_log_line(&jobs_root, "job-empty", 8 * 1024).expect("resolve latest line");
        assert!(latest.is_none(), "expected no latest line for empty logs");

        let missing =
            latest_job_log_line(&jobs_root, "job-missing", 8 * 1024).expect("resolve missing");
        assert!(
            missing.is_none(),
            "expected no latest line for missing logs"
        );
    }

    #[test]
    fn persist_record_handles_concurrent_writers_without_tmp_collisions() {
        let temp = TempDir::new().expect("temp dir");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");
        fs::create_dir_all(&jobs_root).expect("create jobs root");

        enqueue_job(
            project_root,
            &jobs_root,
            "race-job",
            &["save".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            None,
        )
        .expect("enqueue race job");

        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();

        for worker in 0..2u32 {
            let jobs_root = jobs_root.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || -> Result<(), String> {
                barrier.wait();
                for attempt in 0..200u32 {
                    let paths = paths_for(&jobs_root, "race-job");
                    let mut record = load_record(&paths).map_err(|err| err.to_string())?;
                    let metadata = record.metadata.get_or_insert_with(JobMetadata::default);
                    metadata.patch_index = Some((worker * 1000 + attempt) as usize);
                    persist_record(&paths, &record).map_err(|err| err.to_string())?;
                }
                Ok(())
            }));
        }

        barrier.wait();
        for handle in handles {
            handle
                .join()
                .expect("concurrent writer should not panic")
                .expect("concurrent writer should not fail");
        }

        let record = read_record(&jobs_root, "race-job").expect("read final race record");
        assert_eq!(record.id, "race-job");
    }

    fn write_job_with_status(
        project_root: &Path,
        jobs_root: &Path,
        job_id: &str,
        status: JobStatus,
        schedule: JobSchedule,
        child_args: &[String],
    ) -> Result<JobRecord, Box<dyn std::error::Error>> {
        enqueue_job(
            project_root,
            jobs_root,
            job_id,
            child_args,
            &["vizier".to_string()],
            None,
            None,
            Some(schedule.clone()),
        )?;
        let paths = paths_for(jobs_root, job_id);
        let mut record = load_record(&paths)?;
        record.status = status;
        record.schedule = Some(schedule);
        persist_record(&paths, &record)?;
        Ok(record)
    }

    fn update_job_record<F: FnOnce(&mut JobRecord)>(
        jobs_root: &Path,
        job_id: &str,
        updater: F,
    ) -> Result<JobRecord, Box<dyn std::error::Error>> {
        let paths = paths_for(jobs_root, job_id);
        let mut record = load_record(&paths)?;
        updater(&mut record);
        persist_record(&paths, &record)?;
        Ok(record)
    }

    #[derive(Clone, Copy)]
    enum ArtifactKind {
        PlanBranch,
        PlanDoc,
        PlanCommits,
        TargetBranch,
        MergeSentinel,
        CommandPatch,
        Custom,
    }

    fn artifact_for(kind: ArtifactKind, suffix: &str) -> JobArtifact {
        match kind {
            ArtifactKind::PlanBranch => JobArtifact::PlanBranch {
                slug: format!("plan-{suffix}"),
                branch: format!("draft/plan-{suffix}"),
            },
            ArtifactKind::PlanDoc => JobArtifact::PlanDoc {
                slug: format!("doc-{suffix}"),
                branch: format!("draft/doc-{suffix}"),
            },
            ArtifactKind::PlanCommits => JobArtifact::PlanCommits {
                slug: format!("commits-{suffix}"),
                branch: format!("draft/commits-{suffix}"),
            },
            ArtifactKind::TargetBranch => JobArtifact::TargetBranch {
                name: format!("target-{suffix}"),
            },
            ArtifactKind::MergeSentinel => JobArtifact::MergeSentinel {
                slug: format!("merge-{suffix}"),
            },
            ArtifactKind::CommandPatch => JobArtifact::CommandPatch {
                job_id: format!("job-{suffix}"),
            },
            ArtifactKind::Custom => JobArtifact::Custom {
                type_id: "acme.execution".to_string(),
                key: format!("key-{suffix}"),
            },
        }
    }

    fn make_record(
        job_id: &str,
        status: JobStatus,
        created_at: DateTime<Utc>,
        schedule: Option<JobSchedule>,
    ) -> JobRecord {
        JobRecord {
            id: job_id.to_string(),
            status,
            command: Vec::new(),
            child_args: Vec::new(),
            created_at,
            started_at: None,
            finished_at: None,
            pid: None,
            exit_code: None,
            stdout_path: String::new(),
            stderr_path: String::new(),
            session_path: None,
            outcome_path: None,
            metadata: None,
            config_snapshot: None,
            schedule,
        }
    }

    fn after_dependency(job_id: &str) -> JobAfterDependency {
        JobAfterDependency {
            job_id: job_id.to_string(),
            policy: AfterPolicy::Success,
        }
    }

    #[test]
    fn resolve_after_dependencies_rejects_unknown_job_id() {
        let temp = TempDir::new().expect("temp dir");
        let jobs_root = temp.path().join(".vizier/jobs");
        fs::create_dir_all(&jobs_root).expect("jobs root");

        let err = resolve_after_dependencies_for_enqueue(
            &jobs_root,
            "job-new",
            &["missing-job".to_string()],
        )
        .expect_err("expected unknown dependency to fail");
        assert!(
            err.to_string()
                .contains("unknown --after job id: missing-job"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn resolve_after_dependencies_rejects_self_dependency() {
        let temp = TempDir::new().expect("temp dir");
        let jobs_root = temp.path().join(".vizier/jobs");
        fs::create_dir_all(&jobs_root).expect("jobs root");

        let err = resolve_after_dependencies_for_enqueue(
            &jobs_root,
            "job-self",
            &["job-self".to_string()],
        )
        .expect_err("expected self dependency to fail");
        assert!(
            err.to_string()
                .contains("invalid --after self dependency: job-self"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn resolve_after_dependencies_dedupes_repeated_ids() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-a",
            &["--help".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            None,
        )
        .expect("enqueue job-a");

        let after = resolve_after_dependencies_for_enqueue(
            &jobs_root,
            "job-new",
            &["job-a".to_string(), "job-a".to_string()],
        )
        .expect("resolve after");
        assert_eq!(
            after,
            vec![JobAfterDependency {
                job_id: "job-a".to_string(),
                policy: AfterPolicy::Success,
            }]
        );
    }

    #[test]
    fn resolve_after_dependencies_rejects_cycles() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-a",
            &["--help".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            Some(JobSchedule {
                after: vec![after_dependency("job-b")],
                ..JobSchedule::default()
            }),
        )
        .expect("enqueue job-a");
        enqueue_job(
            project_root,
            &jobs_root,
            "job-b",
            &["--help".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            Some(JobSchedule {
                after: vec![after_dependency("job-c")],
                ..JobSchedule::default()
            }),
        )
        .expect("enqueue job-b");

        let err =
            resolve_after_dependencies_for_enqueue(&jobs_root, "job-c", &["job-a".to_string()])
                .expect_err("expected cycle to fail");
        assert!(
            err.to_string().contains("invalid --after cycle:"),
            "unexpected error: {err}"
        );
        assert!(
            err.to_string().contains("job-a")
                && err.to_string().contains("job-b")
                && err.to_string().contains("job-c"),
            "expected cycle path in error: {err}"
        );
    }

    #[test]
    fn resolve_after_dependencies_rejects_malformed_records() {
        let temp = TempDir::new().expect("temp dir");
        let jobs_root = temp.path().join(".vizier/jobs");
        let job_dir = jobs_root.join("job-bad");
        fs::create_dir_all(&job_dir).expect("create bad job dir");
        fs::write(job_dir.join("job.json"), "{ not json }").expect("write malformed json");

        let err =
            resolve_after_dependencies_for_enqueue(&jobs_root, "job-new", &["job-bad".to_string()])
                .expect_err("expected malformed record to fail");
        assert!(
            err.to_string()
                .contains("cannot read job record for --after job-bad"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn scheduler_lock_busy_returns_error() {
        let temp = TempDir::new().expect("temp dir");
        let jobs_root = temp.path().join(".vizier/jobs");
        fs::create_dir_all(&jobs_root).expect("create jobs root");
        fs::write(jobs_root.join("scheduler.lock"), "locked").expect("write lock");

        let err = SchedulerLock::acquire(&jobs_root)
            .err()
            .expect("expected scheduler lock error");
        assert!(
            err.to_string().contains("scheduler is busy"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn scheduler_tick_marks_blocked_by_dependency() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        let schedule = JobSchedule {
            dependencies: vec![JobDependency {
                artifact: JobArtifact::PlanDoc {
                    slug: "alpha".to_string(),
                    branch: "draft/alpha".to_string(),
                },
            }],
            ..JobSchedule::default()
        };

        enqueue_job(
            project_root,
            &jobs_root,
            "blocked-job",
            &["save".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            Some(schedule),
        )
        .expect("enqueue job");

        let binary = std::env::current_exe().expect("current exe");
        scheduler_tick(project_root, &jobs_root, &binary).expect("scheduler tick");

        let record = read_record(&jobs_root, "blocked-job").expect("read record");
        assert_eq!(record.status, JobStatus::BlockedByDependency);
        let wait_reason = record
            .schedule
            .as_ref()
            .and_then(|sched| sched.wait_reason.as_ref())
            .expect("wait reason");
        assert_eq!(wait_reason.kind, JobWaitKind::Dependencies);
        let detail = wait_reason.detail.as_deref().unwrap_or("");
        assert!(
            detail.contains("missing plan_doc:alpha (draft/alpha)"),
            "unexpected wait detail: {detail}"
        );
        let waited_on = record
            .schedule
            .as_ref()
            .map(|sched| sched.waited_on.clone())
            .unwrap_or_default();
        assert!(
            waited_on.contains(&JobWaitKind::Dependencies),
            "expected waited_on to include dependencies"
        );
    }

    #[test]
    fn scheduler_tick_waits_on_after_dependency() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "dep-running",
            &["--help".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            None,
        )
        .expect("enqueue dep");
        update_job_record(&jobs_root, "dep-running", |record| {
            record.status = JobStatus::Running;
        })
        .expect("set dep status");

        enqueue_job(
            project_root,
            &jobs_root,
            "dependent",
            &["--help".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            Some(JobSchedule {
                after: vec![after_dependency("dep-running")],
                ..JobSchedule::default()
            }),
        )
        .expect("enqueue dependent");

        let binary = std::env::current_exe().expect("current exe");
        scheduler_tick(project_root, &jobs_root, &binary).expect("tick");

        let record = read_record(&jobs_root, "dependent").expect("read dependent");
        assert_eq!(record.status, JobStatus::WaitingOnDeps);
        let wait = record
            .schedule
            .as_ref()
            .and_then(|schedule| schedule.wait_reason.as_ref())
            .and_then(|reason| reason.detail.as_deref());
        assert_eq!(wait, Some("waiting on job dep-running"));
    }

    #[test]
    fn scheduler_tick_blocks_on_after_data_errors() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        let bad_dir = jobs_root.join("bad-predecessor");
        fs::create_dir_all(&bad_dir).expect("bad dir");
        fs::write(bad_dir.join("job.json"), "{ invalid }").expect("malformed job json");

        enqueue_job(
            project_root,
            &jobs_root,
            "dependent",
            &["--help".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            Some(JobSchedule {
                after: vec![after_dependency("bad-predecessor")],
                ..JobSchedule::default()
            }),
        )
        .expect("enqueue dependent");

        let binary = std::env::current_exe().expect("current exe");
        scheduler_tick(project_root, &jobs_root, &binary).expect("tick");

        let record = read_record(&jobs_root, "dependent").expect("read dependent");
        assert_eq!(record.status, JobStatus::BlockedByDependency);
        let wait = record
            .schedule
            .as_ref()
            .and_then(|schedule| schedule.wait_reason.as_ref())
            .and_then(|reason| reason.detail.clone())
            .unwrap_or_default();
        assert!(
            wait.contains("scheduler data error for job dependency bad-predecessor"),
            "unexpected wait detail: {wait}"
        );
    }

    #[test]
    fn scheduler_tick_errors_on_missing_binary() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "spawn-failure",
            &["save".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            None,
        )
        .expect("enqueue job");

        let missing_binary = project_root.join("does-not-exist");
        let result = scheduler_tick(project_root, &jobs_root, &missing_binary);
        assert!(result.is_err(), "expected scheduler tick to fail");
    }

    #[cfg(unix)]
    #[test]
    fn scheduler_tick_errors_on_persist_failure() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "persist-failure",
            &[],
            &["vizier".to_string()],
            None,
            None,
            None,
        )
        .expect("enqueue job");

        let paths = paths_for(&jobs_root, "persist-failure");
        let original = fs::metadata(&paths.job_dir)
            .expect("metadata")
            .permissions();
        let mut read_only = original.clone();
        read_only.set_mode(0o555);
        fs::set_permissions(&paths.job_dir, read_only).expect("set perms");

        let binary = project_root.join("missing-binary");
        let result = scheduler_tick(project_root, &jobs_root, &binary);

        fs::set_permissions(&paths.job_dir, original).expect("restore perms");
        assert!(result.is_err(), "expected scheduler tick to fail");
    }

    #[test]
    fn scheduler_tick_handles_graph_shapes() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");
        fs::create_dir_all(&jobs_root).expect("jobs root");

        let artifact_a = JobArtifact::CommandPatch {
            job_id: "a-artifact".to_string(),
        };
        let artifact_b = JobArtifact::CommandPatch {
            job_id: "b-artifact".to_string(),
        };
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-a",
            JobStatus::Running,
            JobSchedule {
                artifacts: vec![artifact_a.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("job a");
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-b",
            JobStatus::Queued,
            JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: artifact_a.clone(),
                }],
                artifacts: vec![artifact_b.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("job b");
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-c",
            JobStatus::Queued,
            JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: artifact_b.clone(),
                }],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("job c");

        let fan_artifact = JobArtifact::CommandPatch {
            job_id: "fan-root".to_string(),
        };
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-fan-root",
            JobStatus::Running,
            JobSchedule {
                artifacts: vec![fan_artifact.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("fan root");
        for job_id in ["job-fan-left", "job-fan-right"] {
            write_job_with_status(
                project_root,
                &jobs_root,
                job_id,
                JobStatus::Queued,
                JobSchedule {
                    dependencies: vec![JobDependency {
                        artifact: fan_artifact.clone(),
                    }],
                    ..JobSchedule::default()
                },
                &["--help".to_string()],
            )
            .expect("fan job");
        }

        let fan_in_left = JobArtifact::CommandPatch {
            job_id: "fanin-left".to_string(),
        };
        let fan_in_right = JobArtifact::CommandPatch {
            job_id: "fanin-right".to_string(),
        };
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-fanin-left",
            JobStatus::Running,
            JobSchedule {
                artifacts: vec![fan_in_left.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("fanin left");
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-fanin-right",
            JobStatus::Running,
            JobSchedule {
                artifacts: vec![fan_in_right.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("fanin right");
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-fanin",
            JobStatus::Queued,
            JobSchedule {
                dependencies: vec![
                    JobDependency {
                        artifact: fan_in_left.clone(),
                    },
                    JobDependency {
                        artifact: fan_in_right.clone(),
                    },
                ],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("fanin");

        let diamond_root = JobArtifact::CommandPatch {
            job_id: "diamond-root".to_string(),
        };
        let diamond_left = JobArtifact::CommandPatch {
            job_id: "diamond-left".to_string(),
        };
        let diamond_right = JobArtifact::CommandPatch {
            job_id: "diamond-right".to_string(),
        };
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-diamond-root",
            JobStatus::Running,
            JobSchedule {
                artifacts: vec![diamond_root.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("diamond root");
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-diamond-left",
            JobStatus::Queued,
            JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: diamond_root.clone(),
                }],
                artifacts: vec![diamond_left.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("diamond left");
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-diamond-right",
            JobStatus::Queued,
            JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: diamond_root.clone(),
                }],
                artifacts: vec![diamond_right.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("diamond right");
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-diamond-leaf",
            JobStatus::Queued,
            JobSchedule {
                dependencies: vec![
                    JobDependency {
                        artifact: diamond_left.clone(),
                    },
                    JobDependency {
                        artifact: diamond_right.clone(),
                    },
                ],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("diamond leaf");

        let disjoint_artifact = JobArtifact::CommandPatch {
            job_id: "disjoint-root".to_string(),
        };
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-disjoint-root",
            JobStatus::Running,
            JobSchedule {
                artifacts: vec![disjoint_artifact.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("disjoint root");
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-disjoint-leaf",
            JobStatus::Queued,
            JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: disjoint_artifact.clone(),
                }],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("disjoint leaf");

        let binary = std::env::current_exe().expect("current exe");
        scheduler_tick(project_root, &jobs_root, &binary).expect("scheduler tick");

        let record_b = read_record(&jobs_root, "job-b").expect("read job b");
        assert_eq!(record_b.status, JobStatus::WaitingOnDeps);
        let detail_b = record_b
            .schedule
            .as_ref()
            .and_then(|sched| sched.wait_reason.as_ref())
            .and_then(|reason| reason.detail.clone())
            .unwrap_or_default();
        assert!(
            detail_b.contains("waiting on command_patch:a-artifact"),
            "unexpected wait detail for job-b: {detail_b}"
        );

        let record_c = read_record(&jobs_root, "job-c").expect("read job c");
        let detail_c = record_c
            .schedule
            .as_ref()
            .and_then(|sched| sched.wait_reason.as_ref())
            .and_then(|reason| reason.detail.clone())
            .unwrap_or_default();
        assert!(
            detail_c.contains("waiting on command_patch:b-artifact"),
            "unexpected wait detail for job-c: {detail_c}"
        );

        for job_id in ["job-fan-left", "job-fan-right"] {
            let record = read_record(&jobs_root, job_id).expect("read fan job");
            assert_eq!(record.status, JobStatus::WaitingOnDeps);
        }

        let record_fanin = read_record(&jobs_root, "job-fanin").expect("read fanin job");
        let detail_fanin = record_fanin
            .schedule
            .as_ref()
            .and_then(|sched| sched.wait_reason.as_ref())
            .and_then(|reason| reason.detail.clone())
            .unwrap_or_default();
        assert!(
            detail_fanin.contains("waiting on command_patch:fanin-left"),
            "unexpected fan-in detail: {detail_fanin}"
        );

        let record_diamond = read_record(&jobs_root, "job-diamond-leaf").expect("read diamond");
        let detail_diamond = record_diamond
            .schedule
            .as_ref()
            .and_then(|sched| sched.wait_reason.as_ref())
            .and_then(|reason| reason.detail.clone())
            .unwrap_or_default();
        assert!(
            detail_diamond.contains("waiting on command_patch:diamond-left"),
            "unexpected diamond detail: {detail_diamond}"
        );

        let record_disjoint = read_record(&jobs_root, "job-disjoint-leaf").expect("read disjoint");
        assert_eq!(record_disjoint.status, JobStatus::WaitingOnDeps);
    }

    #[test]
    fn scheduler_tick_waited_on_accumulates_and_stabilizes() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");
        fs::create_dir_all(&jobs_root).expect("jobs root");

        let artifact = JobArtifact::CommandPatch {
            job_id: "dep-ready".to_string(),
        };
        write_job_with_status(
            project_root,
            &jobs_root,
            "dep-producer",
            JobStatus::Running,
            JobSchedule {
                artifacts: vec![artifact.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("dep producer");
        write_job_with_status(
            project_root,
            &jobs_root,
            "lock-holder",
            JobStatus::Running,
            JobSchedule {
                locks: vec![JobLock {
                    key: "lock-a".to_string(),
                    mode: LockMode::Exclusive,
                }],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("lock holder");

        write_job_with_status(
            project_root,
            &jobs_root,
            "waiting-job",
            JobStatus::Queued,
            JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: artifact.clone(),
                }],
                locks: vec![JobLock {
                    key: "lock-a".to_string(),
                    mode: LockMode::Exclusive,
                }],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("waiting job");

        let binary = std::env::current_exe().expect("current exe");
        let outcome = scheduler_tick(project_root, &jobs_root, &binary).expect("tick 1");
        assert!(outcome.updated.contains(&"waiting-job".to_string()));

        let record = read_record(&jobs_root, "waiting-job").expect("read waiting job");
        assert_eq!(record.status, JobStatus::WaitingOnDeps);
        let waited_on = record
            .schedule
            .as_ref()
            .map(|sched| sched.waited_on.clone())
            .unwrap_or_default();
        assert_eq!(waited_on, vec![JobWaitKind::Dependencies]);

        ensure_artifact_exists(
            &Repository::discover(project_root).expect("repo"),
            &jobs_root,
            &artifact,
        )
        .expect("create artifact");
        let outcome = scheduler_tick(project_root, &jobs_root, &binary).expect("tick 2");
        assert!(outcome.updated.contains(&"waiting-job".to_string()));

        let record = read_record(&jobs_root, "waiting-job").expect("read waiting job");
        assert_eq!(record.status, JobStatus::WaitingOnLocks);
        let waited_on = record
            .schedule
            .as_ref()
            .map(|sched| sched.waited_on.clone())
            .unwrap_or_default();
        assert_eq!(
            waited_on,
            vec![JobWaitKind::Dependencies, JobWaitKind::Locks]
        );

        let outcome = scheduler_tick(project_root, &jobs_root, &binary).expect("tick 3");
        assert!(
            !outcome.updated.contains(&"waiting-job".to_string()),
            "expected no-op tick to avoid updates"
        );

        update_job_record(&jobs_root, "lock-holder", |record| {
            record.status = JobStatus::Succeeded;
        })
        .expect("release lock");

        scheduler_tick(project_root, &jobs_root, &binary).expect("tick 4");
        let record = read_record(&jobs_root, "waiting-job").expect("read waiting job");
        assert_eq!(record.status, JobStatus::Running);
        let wait_reason = record
            .schedule
            .as_ref()
            .and_then(|sched| sched.wait_reason.as_ref());
        assert!(wait_reason.is_none(), "wait reason should clear on start");
    }

    #[test]
    fn scheduler_tick_missing_child_args_fails() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "missing-child-args",
            &[],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            None,
        )
        .expect("enqueue");

        let binary = std::env::current_exe().expect("current exe");
        scheduler_tick(project_root, &jobs_root, &binary).expect("tick");

        let record = read_record(&jobs_root, "missing-child-args").expect("record");
        assert_eq!(record.status, JobStatus::Failed);
        assert_eq!(record.exit_code, Some(1));
        assert!(
            record.finished_at.is_some(),
            "expected finished_at to be set"
        );
        let outcome_path = record.outcome_path.as_deref().expect("outcome path");
        assert!(
            project_root.join(outcome_path).exists(),
            "expected outcome file to exist"
        );
        let wait_reason = record
            .schedule
            .as_ref()
            .and_then(|sched| sched.wait_reason.as_ref())
            .expect("wait reason");
        assert_eq!(wait_reason.kind, JobWaitKind::Dependencies);
        assert_eq!(wait_reason.detail.as_deref(), Some("missing child args"));
    }

    #[test]
    fn scheduler_tick_starts_with_empty_schedule() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "empty-schedule",
            &["--help".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            None,
        )
        .expect("enqueue");

        let binary = std::env::current_exe().expect("current exe");
        scheduler_tick(project_root, &jobs_root, &binary).expect("tick");

        let record = read_record(&jobs_root, "empty-schedule").expect("record");
        assert_eq!(record.status, JobStatus::Running);
        let wait_reason = record
            .schedule
            .as_ref()
            .and_then(|sched| sched.wait_reason.as_ref());
        assert!(wait_reason.is_none(), "wait reason should be cleared");
    }

    #[test]
    fn scheduler_facts_collect_artifact_existence() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");
        fs::create_dir_all(&jobs_root).expect("jobs root");

        let kinds = [
            ArtifactKind::PlanBranch,
            ArtifactKind::PlanDoc,
            ArtifactKind::PlanCommits,
            ArtifactKind::TargetBranch,
            ArtifactKind::MergeSentinel,
            ArtifactKind::CommandPatch,
            ArtifactKind::Custom,
        ];

        for (idx, kind) in kinds.iter().enumerate() {
            let exists = artifact_for(*kind, &format!("exists-{idx}"));
            let missing = artifact_for(*kind, &format!("missing-{idx}"));
            ensure_artifact_exists(&repo, &jobs_root, &exists).expect("create artifact");

            write_job_with_status(
                project_root,
                &jobs_root,
                &format!("job-{idx}"),
                JobStatus::Queued,
                JobSchedule {
                    dependencies: vec![
                        JobDependency {
                            artifact: exists.clone(),
                        },
                        JobDependency {
                            artifact: missing.clone(),
                        },
                    ],
                    ..JobSchedule::default()
                },
                &["--help".to_string()],
            )
            .expect("write job");
        }

        let mut records = list_records(&jobs_root).expect("list records");
        records.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        let facts = build_scheduler_facts(&repo, &jobs_root, &records).expect("facts");

        for (idx, kind) in kinds.iter().enumerate() {
            let exists = artifact_for(*kind, &format!("exists-{idx}"));
            let missing = artifact_for(*kind, &format!("missing-{idx}"));
            assert!(
                facts.artifact_exists.contains(&exists),
                "expected artifact to exist: {exists:?}"
            );
            assert!(
                !facts.artifact_exists.contains(&missing),
                "expected artifact to be missing: {missing:?}"
            );
        }
    }

    #[test]
    fn finalize_job_writes_custom_artifact_markers() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        let artifact = JobArtifact::Custom {
            type_id: "acme.execution".to_string(),
            key: "final".to_string(),
        };
        enqueue_job(
            project_root,
            &jobs_root,
            "custom-producer",
            &["--help".to_string()],
            &["vizier".to_string(), "__workflow-node".to_string()],
            None,
            None,
            Some(JobSchedule {
                artifacts: vec![artifact.clone()],
                ..JobSchedule::default()
            }),
        )
        .expect("enqueue custom producer");
        finalize_job(
            project_root,
            &jobs_root,
            "custom-producer",
            JobStatus::Succeeded,
            0,
            None,
            None,
        )
        .expect("finalize custom producer");

        let marker =
            custom_artifact_marker_path(project_root, "custom-producer", "acme.execution", "final");
        assert!(
            marker.exists(),
            "expected custom artifact marker {}",
            marker.display()
        );
        assert!(
            artifact_exists(&repo, &artifact),
            "custom artifact should be externally discoverable after finalize"
        );
    }

    #[test]
    fn scheduler_facts_collect_producer_statuses() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");
        fs::create_dir_all(&jobs_root).expect("jobs root");

        let artifact = JobArtifact::CommandPatch {
            job_id: "artifact".to_string(),
        };
        write_job_with_status(
            project_root,
            &jobs_root,
            "producer-running",
            JobStatus::Running,
            JobSchedule {
                artifacts: vec![artifact.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("producer running");
        write_job_with_status(
            project_root,
            &jobs_root,
            "producer-succeeded",
            JobStatus::Succeeded,
            JobSchedule {
                artifacts: vec![artifact.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("producer succeeded");

        let mut records = list_records(&jobs_root).expect("list records");
        records.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        let facts = build_scheduler_facts(
            &Repository::discover(project_root).expect("repo"),
            &jobs_root,
            &records,
        )
        .expect("facts");
        let statuses = facts
            .producer_statuses
            .get(&artifact)
            .expect("producer statuses");
        assert!(statuses.contains(&JobStatus::Running));
        assert!(statuses.contains(&JobStatus::Succeeded));
    }

    #[test]
    fn scheduler_facts_record_pinned_head_status() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");
        fs::create_dir_all(&jobs_root).expect("jobs root");

        let head = repo.head().expect("head");
        let branch = head.shorthand().expect("branch").to_string();
        let oid = head.target().map(|id| id.to_string()).expect("head oid");

        write_job_with_status(
            project_root,
            &jobs_root,
            "pinned-ok",
            JobStatus::Queued,
            JobSchedule {
                pinned_head: Some(PinnedHead {
                    branch: branch.clone(),
                    oid: oid.clone(),
                }),
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("pinned ok");
        write_job_with_status(
            project_root,
            &jobs_root,
            "pinned-bad",
            JobStatus::Queued,
            JobSchedule {
                pinned_head: Some(PinnedHead {
                    branch: branch.clone(),
                    oid: "deadbeef".to_string(),
                }),
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("pinned bad");

        let mut records = list_records(&jobs_root).expect("list records");
        records.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        let facts = build_scheduler_facts(&repo, &jobs_root, &records).expect("facts");

        let ok = facts.pinned_heads.get("pinned-ok").expect("pinned ok fact");
        assert!(ok.matches);
        assert_eq!(ok.branch, branch);

        let bad = facts
            .pinned_heads
            .get("pinned-bad")
            .expect("pinned bad fact");
        assert!(!bad.matches);
        assert_eq!(bad.branch, branch);
    }

    #[test]
    fn scheduler_facts_track_running_locks() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");
        fs::create_dir_all(&jobs_root).expect("jobs root");

        let lock = JobLock {
            key: "lock-a".to_string(),
            mode: LockMode::Exclusive,
        };
        write_job_with_status(
            project_root,
            &jobs_root,
            "lock-holder",
            JobStatus::Running,
            JobSchedule {
                locks: vec![lock.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("lock holder");

        let mut records = list_records(&jobs_root).expect("list records");
        records.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        let facts = build_scheduler_facts(
            &Repository::discover(project_root).expect("repo"),
            &jobs_root,
            &records,
        )
        .expect("facts");
        assert!(
            !facts.lock_state.can_acquire(&lock),
            "expected lock to be held"
        );
    }

    #[test]
    fn schedule_graph_orders_dependencies_by_artifact_key() {
        let deps = vec![
            JobDependency {
                artifact: JobArtifact::TargetBranch {
                    name: "main".to_string(),
                },
            },
            JobDependency {
                artifact: JobArtifact::CommandPatch {
                    job_id: "job-z".to_string(),
                },
            },
            JobDependency {
                artifact: JobArtifact::PlanDoc {
                    slug: "alpha".to_string(),
                    branch: "draft/alpha".to_string(),
                },
            },
        ];

        let record = make_record(
            "job-1",
            JobStatus::Queued,
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                dependencies: deps,
                ..JobSchedule::default()
            }),
        );

        let graph = ScheduleGraph::new(vec![record]);
        let ordered = graph.dependencies_for("job-1");
        let expected = vec![
            JobArtifact::PlanDoc {
                slug: "alpha".to_string(),
                branch: "draft/alpha".to_string(),
            },
            JobArtifact::TargetBranch {
                name: "main".to_string(),
            },
            JobArtifact::CommandPatch {
                job_id: "job-z".to_string(),
            },
        ];
        assert_eq!(ordered, expected);
    }

    #[test]
    fn schedule_graph_orders_producers_by_created_at_then_id() {
        let artifact = JobArtifact::CommandPatch {
            job_id: "shared".to_string(),
        };
        let record_a = make_record(
            "job-a",
            JobStatus::Queued,
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                artifacts: vec![artifact.clone()],
                ..JobSchedule::default()
            }),
        );
        let record_b = make_record(
            "job-b",
            JobStatus::Queued,
            Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                artifacts: vec![artifact.clone()],
                ..JobSchedule::default()
            }),
        );

        let graph = ScheduleGraph::new(vec![record_b, record_a]);
        let producers = graph.producers_for(&artifact);
        assert_eq!(producers, vec!["job-a".to_string(), "job-b".to_string()]);
    }

    #[test]
    fn schedule_graph_collect_focus_includes_after_neighbors() {
        let focused = make_record(
            "job-focused",
            JobStatus::Queued,
            Utc.with_ymd_and_hms(2026, 1, 3, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                after: vec![after_dependency("job-parent")],
                ..JobSchedule::default()
            }),
        );
        let parent = make_record(
            "job-parent",
            JobStatus::Succeeded,
            Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
            Some(JobSchedule::default()),
        );
        let child = make_record(
            "job-child",
            JobStatus::Queued,
            Utc.with_ymd_and_hms(2026, 1, 4, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                after: vec![after_dependency("job-focused")],
                ..JobSchedule::default()
            }),
        );

        let graph = ScheduleGraph::new(vec![focused, parent, child]);
        let focus = graph.collect_focus_jobs("job-focused", 1);
        assert!(focus.contains("job-focused"));
        assert!(focus.contains("job-parent"));
        assert!(focus.contains("job-child"));
    }

    #[test]
    fn schedule_snapshot_includes_after_edges() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");

        let predecessor = make_record(
            "job-predecessor",
            JobStatus::Succeeded,
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            Some(JobSchedule::default()),
        );
        let dependent = make_record(
            "job-dependent",
            JobStatus::Queued,
            Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                after: vec![after_dependency("job-predecessor")],
                ..JobSchedule::default()
            }),
        );

        let graph = ScheduleGraph::new(vec![dependent, predecessor]);
        let edges = graph.snapshot_edges(&repo, &["job-dependent".to_string()], 2);
        assert!(
            edges.iter().any(|edge| {
                edge.from == "job-dependent"
                    && edge.to == "job-predecessor"
                    && edge.after.as_ref().map(|after| after.policy) == Some(AfterPolicy::Success)
            }),
            "expected snapshot to include explicit after edge"
        );
    }

    #[test]
    fn schedule_graph_reports_artifact_state() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        ensure_branch(&repo, "present").expect("ensure branch");

        let graph = ScheduleGraph::new(Vec::new());
        let present = JobArtifact::TargetBranch {
            name: "present".to_string(),
        };
        let missing = JobArtifact::TargetBranch {
            name: "missing".to_string(),
        };
        assert_eq!(
            graph.artifact_state(&repo, &present),
            ScheduleArtifactState::Present
        );
        assert_eq!(
            graph.artifact_state(&repo, &missing),
            ScheduleArtifactState::Missing
        );
    }

    #[test]
    fn schedule_snapshot_respects_depth_limit() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");

        let artifact_b = JobArtifact::CommandPatch {
            job_id: "b".to_string(),
        };
        let artifact_c = JobArtifact::CommandPatch {
            job_id: "c".to_string(),
        };

        let job_c = make_record(
            "job-c",
            JobStatus::Succeeded,
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                artifacts: vec![artifact_c.clone()],
                ..JobSchedule::default()
            }),
        );
        let job_b = make_record(
            "job-b",
            JobStatus::Succeeded,
            Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: artifact_c.clone(),
                }],
                artifacts: vec![artifact_b.clone()],
                ..JobSchedule::default()
            }),
        );
        let job_a = make_record(
            "job-a",
            JobStatus::Queued,
            Utc.with_ymd_and_hms(2026, 1, 3, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: artifact_b.clone(),
                }],
                ..JobSchedule::default()
            }),
        );

        let graph = ScheduleGraph::new(vec![job_a, job_b, job_c]);
        let roots = vec!["job-a".to_string()];

        let edges = graph.snapshot_edges(&repo, &roots, 1);
        assert!(
            edges
                .iter()
                .any(|edge| edge.from == "job-a" && edge.to == "job-b"),
            "expected job-a -> job-b edge"
        );
        assert!(
            edges.iter().all(|edge| edge.from != "job-b"),
            "expected depth=1 to skip job-b dependencies"
        );

        let deeper = graph.snapshot_edges(&repo, &roots, 2);
        assert!(
            deeper
                .iter()
                .any(|edge| edge.from == "job-b" && edge.to == "job-c"),
            "expected depth=2 to include job-b -> job-c edge"
        );
    }

    #[test]
    fn retry_set_includes_downstream_dependents_only() {
        let predecessor = make_record(
            "job-predecessor",
            JobStatus::Succeeded,
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                artifacts: vec![JobArtifact::CommandPatch {
                    job_id: "pred-artifact".to_string(),
                }],
                ..JobSchedule::default()
            }),
        );
        let root_artifact = JobArtifact::CommandPatch {
            job_id: "root-artifact".to_string(),
        };
        let root = make_record(
            "job-root",
            JobStatus::Failed,
            Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                after: vec![after_dependency("job-predecessor")],
                artifacts: vec![root_artifact.clone()],
                ..JobSchedule::default()
            }),
        );
        let dependent_after = make_record(
            "job-dependent-after",
            JobStatus::BlockedByDependency,
            Utc.with_ymd_and_hms(2026, 1, 3, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                after: vec![after_dependency("job-root")],
                ..JobSchedule::default()
            }),
        );
        let dependent_artifact = make_record(
            "job-dependent-artifact",
            JobStatus::BlockedByDependency,
            Utc.with_ymd_and_hms(2026, 1, 4, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: root_artifact,
                }],
                ..JobSchedule::default()
            }),
        );

        let graph =
            ScheduleGraph::new(vec![dependent_artifact, dependent_after, predecessor, root]);
        let retry_set = collect_retry_set(&graph, "job-root");
        assert_eq!(
            retry_set,
            vec![
                "job-root".to_string(),
                "job-dependent-after".to_string(),
                "job-dependent-artifact".to_string(),
            ]
        );
    }

    #[test]
    fn rewind_job_record_for_retry_clears_runtime_and_artifacts() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-retry",
            &["--help".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue");

        let worktree_path = project_root.join(".vizier/tmp-worktrees/retry-cleanup");
        fs::create_dir_all(&worktree_path).expect("create worktree path");

        let paths = paths_for(&jobs_root, "job-retry");
        fs::write(&paths.stdout_path, "stdout").expect("write stdout");
        fs::write(&paths.stderr_path, "stderr").expect("write stderr");
        let outcome_path = paths.job_dir.join("outcome.json");
        fs::write(&outcome_path, "{}").expect("write outcome");
        let ask_patch = command_patch_path(&jobs_root, "job-retry");
        let save_patch = save_input_patch_path(&jobs_root, "job-retry");
        fs::write(&ask_patch, "ask patch").expect("write ask patch");
        fs::write(&save_patch, "save patch").expect("write save patch");
        let custom_artifact = JobArtifact::Custom {
            type_id: "acme.execution".to_string(),
            key: "retry-node".to_string(),
        };
        write_custom_artifact_markers(
            project_root,
            "job-retry",
            std::slice::from_ref(&custom_artifact),
        )
        .expect("write custom marker");
        let custom_marker =
            custom_artifact_marker_path(project_root, "job-retry", "acme.execution", "retry-node");

        let mut record = update_job_record(&jobs_root, "job-retry", |record| {
            record.status = JobStatus::Failed;
            let now = Utc::now();
            record.started_at = Some(now);
            record.finished_at = Some(now);
            record.pid = Some(4242);
            record.exit_code = Some(1);
            record.session_path = Some(".vizier/sessions/s1/session.json".to_string());
            record.outcome_path = Some(".vizier/jobs/job-retry/outcome.json".to_string());
            record.schedule = Some(JobSchedule {
                wait_reason: Some(JobWaitReason {
                    kind: JobWaitKind::Dependencies,
                    detail: Some("waiting on old state".to_string()),
                }),
                waited_on: vec![JobWaitKind::Dependencies],
                artifacts: vec![custom_artifact.clone()],
                ..record.schedule.clone().unwrap_or_default()
            });
            record.metadata = Some(JobMetadata {
                worktree_owned: Some(true),
                worktree_path: Some(".vizier/tmp-worktrees/retry-cleanup".to_string()),
                agent_exit_code: Some(12),
                cancel_cleanup_status: Some(CancelCleanupStatus::Failed),
                cancel_cleanup_error: Some("old error".to_string()),
                ..JobMetadata::default()
            });
        })
        .expect("set runtime fields");

        rewind_job_record_for_retry(project_root, &jobs_root, &mut record).expect("rewind record");
        persist_record(&paths, &record).expect("persist rewinded record");

        assert_eq!(record.status, JobStatus::Queued);
        assert!(record.started_at.is_none());
        assert!(record.finished_at.is_none());
        assert!(record.pid.is_none());
        assert!(record.exit_code.is_none());
        assert!(record.session_path.is_none());
        assert!(record.outcome_path.is_none());

        let schedule = record.schedule.as_ref().expect("schedule");
        assert!(schedule.wait_reason.is_none(), "wait reason should clear");
        assert!(schedule.waited_on.is_empty(), "waited_on should clear");

        let metadata = record.metadata.as_ref().expect("metadata");
        assert!(metadata.worktree_name.is_none());
        assert!(metadata.worktree_path.is_none());
        assert!(metadata.worktree_owned.is_none());
        assert!(metadata.agent_exit_code.is_none());
        assert!(metadata.cancel_cleanup_status.is_none());
        assert!(metadata.cancel_cleanup_error.is_none());
        assert_eq!(
            metadata.retry_cleanup_status,
            Some(RetryCleanupStatus::Done)
        );
        assert!(metadata.retry_cleanup_error.is_none());

        assert!(
            !outcome_path.exists(),
            "expected outcome file to be removed during rewind"
        );
        assert!(
            !ask_patch.exists(),
            "expected ask-save patch to be removed during rewind"
        );
        assert!(
            !save_patch.exists(),
            "expected save-input patch to be removed during rewind"
        );
        assert!(
            !custom_marker.exists(),
            "expected custom artifact marker to be removed during rewind"
        );
        assert!(
            !worktree_path.exists(),
            "expected retry-owned worktree to be removed during rewind"
        );
        let stdout = fs::read_to_string(&paths.stdout_path).expect("read stdout");
        let stderr = fs::read_to_string(&paths.stderr_path).expect("read stderr");
        assert!(stdout.is_empty(), "expected stdout log truncation");
        assert!(stderr.is_empty(), "expected stderr log truncation");
    }

    #[test]
    fn rewind_job_record_for_retry_retains_worktree_metadata_when_cleanup_degrades() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-retry-degraded",
            &["--help".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue");

        let worktree_rel = ".vizier/tmp-worktrees/retry-degraded";
        let worktree_path = project_root.join(worktree_rel);
        fs::create_dir_all(&worktree_path).expect("create worktree path");

        let mut record = update_job_record(&jobs_root, "job-retry-degraded", |record| {
            record.status = JobStatus::Failed;
            record.metadata = Some(JobMetadata {
                worktree_name: Some("missing-retry-worktree".to_string()),
                worktree_owned: Some(true),
                worktree_path: Some(worktree_rel.to_string()),
                ..JobMetadata::default()
            });
        })
        .expect("set runtime fields");

        rewind_job_record_for_retry(project_root, &jobs_root, &mut record).expect("rewind record");

        let metadata = record.metadata.as_ref().expect("metadata");
        assert_eq!(
            metadata.worktree_name.as_deref(),
            Some("missing-retry-worktree")
        );
        assert_eq!(metadata.worktree_path.as_deref(), Some(worktree_rel));
        assert_eq!(metadata.worktree_owned, Some(true));
        assert_eq!(
            metadata.retry_cleanup_status,
            Some(RetryCleanupStatus::Degraded)
        );
        let detail = metadata.retry_cleanup_error.as_deref().unwrap_or("");
        assert!(
            detail.contains("fallback cleanup failed"),
            "expected fallback failure detail, got: {detail}"
        );
    }

    #[test]
    fn rewind_job_record_for_retry_clears_worktree_metadata_when_fallback_succeeds() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-retry-fallback",
            &["--help".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue");

        let worktree_rel = ".vizier/tmp-worktrees/retry-fallback";
        let worktree_path = project_root.join(worktree_rel);
        if let Some(parent) = worktree_path.parent() {
            fs::create_dir_all(parent).expect("create worktree parent");
        }
        let add_status = Command::new("git")
            .arg("-C")
            .arg(project_root)
            .arg("worktree")
            .arg("add")
            .arg("--detach")
            .arg(&worktree_path)
            .status()
            .expect("run git worktree add");
        assert!(
            add_status.success(),
            "expected git worktree add to succeed (status={add_status})"
        );

        let mut record = update_job_record(&jobs_root, "job-retry-fallback", |record| {
            record.status = JobStatus::Failed;
            record.metadata = Some(JobMetadata {
                worktree_name: Some("wrong-worktree-name".to_string()),
                worktree_owned: Some(true),
                worktree_path: Some(worktree_rel.to_string()),
                ..JobMetadata::default()
            });
        })
        .expect("set runtime fields");

        rewind_job_record_for_retry(project_root, &jobs_root, &mut record).expect("rewind record");

        let metadata = record.metadata.as_ref().expect("metadata");
        assert!(metadata.worktree_name.is_none());
        assert!(metadata.worktree_path.is_none());
        assert!(metadata.worktree_owned.is_none());
        assert_eq!(
            metadata.retry_cleanup_status,
            Some(RetryCleanupStatus::Done)
        );
        assert!(metadata.retry_cleanup_error.is_none());
        assert!(
            !worktree_path.exists(),
            "expected fallback cleanup to remove worktree path"
        );
    }

    #[test]
    fn prune_error_mentions_missing_shallow_detects_known_message() {
        let sample = "could not find '/tmp/repo/.git/shallow' to stat";
        assert!(
            prune_error_mentions_missing_shallow(sample),
            "expected missing shallow detection"
        );
        assert!(
            !prune_error_mentions_missing_shallow("failed to prune worktree"),
            "unexpected shallow detection for unrelated error"
        );
    }

    #[test]
    fn retry_job_clears_merge_sentinel_when_git_state_is_clean() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-merge-retry",
            &["--help".to_string()],
            &["vizier".to_string(), "merge".to_string()],
            Some(JobMetadata {
                scope: Some("merge".to_string()),
                plan: Some("retry-merge".to_string()),
                ..JobMetadata::default()
            }),
            None,
            Some(JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: JobArtifact::TargetBranch {
                        name: "missing-target".to_string(),
                    },
                }],
                locks: vec![JobLock {
                    key: "merge_sentinel:retry-merge".to_string(),
                    mode: LockMode::Exclusive,
                }],
                ..JobSchedule::default()
            }),
        )
        .expect("enqueue merge retry");
        update_job_record(&jobs_root, "job-merge-retry", |record| {
            record.status = JobStatus::Failed;
            record.exit_code = Some(1);
        })
        .expect("set failed status");

        let sentinel = project_root
            .join(".vizier/tmp/merge-conflicts")
            .join("retry-merge.json");
        if let Some(parent) = sentinel.parent() {
            fs::create_dir_all(parent).expect("create merge-conflict parent");
        }
        fs::write(&sentinel, "{}").expect("write sentinel");

        let binary = std::env::current_exe().expect("current exe");
        retry_job(project_root, &jobs_root, &binary, "job-merge-retry").expect("retry merge job");

        assert!(
            !sentinel.exists(),
            "expected merge sentinel cleanup during retry"
        );
    }

    #[test]
    fn retry_job_rejects_running_jobs_in_retry_set() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-root",
            &["--help".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            None,
        )
        .expect("enqueue root");
        update_job_record(&jobs_root, "job-root", |record| {
            record.status = JobStatus::Failed;
            record.exit_code = Some(1);
        })
        .expect("mark root failed");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-dependent",
            &["--help".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            Some(JobSchedule {
                after: vec![after_dependency("job-root")],
                ..JobSchedule::default()
            }),
        )
        .expect("enqueue dependent");
        update_job_record(&jobs_root, "job-dependent", |record| {
            record.status = JobStatus::Running;
        })
        .expect("mark dependent running");

        let binary = std::env::current_exe().expect("current exe");
        let err = retry_job(project_root, &jobs_root, &binary, "job-root")
            .expect_err("expected running dependent to block retry");
        assert!(
            err.to_string().contains("job-dependent (running)"),
            "unexpected retry active-set error: {err}"
        );
    }

    #[test]
    fn retry_job_allows_waiting_jobs_in_retry_set() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        let root_artifact = JobArtifact::CommandPatch {
            job_id: "job-root".to_string(),
        };

        enqueue_job(
            project_root,
            &jobs_root,
            "job-root",
            &["--help".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            Some(JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: JobArtifact::TargetBranch {
                        name: "missing-retry-target".to_string(),
                    },
                }],
                artifacts: vec![root_artifact.clone()],
                ..JobSchedule::default()
            }),
        )
        .expect("enqueue root");
        update_job_record(&jobs_root, "job-root", |record| {
            record.status = JobStatus::Failed;
            record.exit_code = Some(1);
            if let Some(schedule) = record.schedule.as_mut() {
                schedule.wait_reason = Some(JobWaitReason {
                    kind: JobWaitKind::Dependencies,
                    detail: Some("dependency failed for previous attempt".to_string()),
                });
                schedule.waited_on = vec![JobWaitKind::Dependencies];
            }
        })
        .expect("mark root failed");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-dependent",
            &["--help".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            Some(JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: root_artifact,
                }],
                wait_reason: Some(JobWaitReason {
                    kind: JobWaitKind::Dependencies,
                    detail: Some("waiting on command_patch:job-root".to_string()),
                }),
                waited_on: vec![JobWaitKind::Dependencies],
                ..JobSchedule::default()
            }),
        )
        .expect("enqueue dependent");
        update_job_record(&jobs_root, "job-dependent", |record| {
            record.status = JobStatus::WaitingOnDeps;
        })
        .expect("mark dependent waiting");

        let binary = std::env::current_exe().expect("current exe");
        let outcome = retry_job(project_root, &jobs_root, &binary, "job-root")
            .expect("waiting jobs in retry set should not block retry");
        assert_eq!(
            outcome.retry_set,
            vec!["job-root".to_string(), "job-dependent".to_string()]
        );
        assert_eq!(
            outcome.reset,
            vec!["job-root".to_string(), "job-dependent".to_string()]
        );
    }

    #[test]
    fn gc_jobs_preserves_terminal_records_referenced_by_active_after_dependencies() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-predecessor",
            &["--help".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            None,
        )
        .expect("enqueue predecessor");
        update_job_record(&jobs_root, "job-predecessor", |record| {
            record.status = JobStatus::Succeeded;
            let old = Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap();
            record.created_at = old;
            record.started_at = Some(old);
            record.finished_at = Some(old);
            record.exit_code = Some(0);
        })
        .expect("mark predecessor terminal");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-dependent",
            &["--help".to_string()],
            &["vizier".to_string(), "save".to_string()],
            None,
            None,
            Some(JobSchedule {
                after: vec![after_dependency("job-predecessor")],
                ..JobSchedule::default()
            }),
        )
        .expect("enqueue dependent");
        update_job_record(&jobs_root, "job-dependent", |record| {
            record.status = JobStatus::Queued;
        })
        .expect("ensure active status");

        let removed = gc_jobs(project_root, &jobs_root, Duration::days(7)).expect("gc");
        assert_eq!(removed, 0, "expected predecessor to be retained");
        assert!(
            paths_for(&jobs_root, "job-predecessor").job_dir.exists(),
            "expected referenced predecessor to remain after GC"
        );
    }
}
