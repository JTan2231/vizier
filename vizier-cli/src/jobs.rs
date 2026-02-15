#![allow(dead_code)]

use chrono::{DateTime, Duration, Utc};
use git2::{Oid, Repository, WorktreePruneOptions};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
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
use vizier_core::scheduler::spec::{
    self, AfterDependencyState, JobAfterDependencyStatus, JobPreconditionFact,
    JobPreconditionState, PinnedHeadFact, SchedulerAction, SchedulerFacts,
};
#[allow(unused_imports)]
pub use vizier_core::scheduler::{
    AfterPolicy, JobAfterDependency, JobApprovalFact, JobApprovalState, JobArtifact, JobLock,
    JobPrecondition, JobStatus, JobWaitKind, JobWaitReason, LockMode, PinnedHead, format_artifact,
};
use vizier_core::workflow_template::{
    CompiledWorkflowNode, PROMPT_ARTIFACT_TYPE_ID, WorkflowGate, WorkflowNodeKind,
    WorkflowOutcomeEdges, WorkflowPrecondition, WorkflowRetryMode, WorkflowTemplate,
    compile_workflow_node, validate_workflow_capability_contracts,
};
use vizier_core::{
    agent::{AgentError, AgentRequest, DEFAULT_AGENT_TIMEOUT},
    config, display,
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
    pub workflow_run_id: Option<String>,
    pub workflow_node_attempt: Option<u32>,
    pub workflow_node_outcome: Option<String>,
    pub workflow_payload_refs: Option<Vec<String>>,
    pub workflow_template_selector: Option<String>,
    pub workflow_template_id: Option<String>,
    pub workflow_template_version: Option<String>,
    pub workflow_node_id: Option<String>,
    #[serde(default, skip_serializing)]
    pub workflow_capability_id: Option<String>,
    pub workflow_executor_class: Option<String>,
    pub workflow_executor_operation: Option<String>,
    pub workflow_control_policy: Option<String>,
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
    pub execution_root: Option<String>,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodeOutcome {
    Succeeded,
    Failed,
    Blocked,
    Cancelled,
}

impl WorkflowNodeOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowNodeResult {
    pub outcome: WorkflowNodeOutcome,
    #[serde(default)]
    pub artifacts_written: Vec<JobArtifact>,
    #[serde(default)]
    pub payload_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JobMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

impl WorkflowNodeResult {
    fn succeeded(summary: impl Into<String>) -> Self {
        Self {
            outcome: WorkflowNodeOutcome::Succeeded,
            artifacts_written: Vec::new(),
            payload_refs: Vec::new(),
            metadata: None,
            summary: Some(summary.into()),
            exit_code: Some(0),
        }
    }

    fn failed(summary: impl Into<String>, exit_code: Option<i32>) -> Self {
        Self {
            outcome: WorkflowNodeOutcome::Failed,
            artifacts_written: Vec::new(),
            payload_refs: Vec::new(),
            metadata: None,
            summary: Some(summary.into()),
            exit_code,
        }
    }

    fn blocked(summary: impl Into<String>, exit_code: Option<i32>) -> Self {
        Self {
            outcome: WorkflowNodeOutcome::Blocked,
            artifacts_written: Vec::new(),
            payload_refs: Vec::new(),
            metadata: None,
            summary: Some(summary.into()),
            exit_code,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum WorkflowRouteMode {
    PropagateContext,
    RetryJob,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct WorkflowRouteTarget {
    node_id: String,
    mode: WorkflowRouteMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct WorkflowRouteTargets {
    #[serde(default)]
    succeeded: Vec<WorkflowRouteTarget>,
    #[serde(default)]
    failed: Vec<WorkflowRouteTarget>,
    #[serde(default)]
    blocked: Vec<WorkflowRouteTarget>,
    #[serde(default)]
    cancelled: Vec<WorkflowRouteTarget>,
}

impl WorkflowRouteTargets {
    fn for_outcome(&self, outcome: WorkflowNodeOutcome) -> &[WorkflowRouteTarget] {
        match outcome {
            WorkflowNodeOutcome::Succeeded => &self.succeeded,
            WorkflowNodeOutcome::Failed => &self.failed,
            WorkflowNodeOutcome::Blocked => &self.blocked,
            WorkflowNodeOutcome::Cancelled => &self.cancelled,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct WorkflowRuntimeNodeManifest {
    node_id: String,
    job_id: String,
    uses: String,
    kind: WorkflowNodeKind,
    args: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    executor_operation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    control_policy: Option<String>,
    #[serde(default)]
    gates: Vec<WorkflowGate>,
    retry: vizier_core::workflow_template::WorkflowRetryPolicy,
    routes: WorkflowRouteTargets,
    #[serde(default)]
    artifacts_by_outcome: WorkflowOutcomeArtifactsByOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct WorkflowOutcomeArtifactsByOutcome {
    #[serde(default)]
    succeeded: Vec<JobArtifact>,
    #[serde(default)]
    failed: Vec<JobArtifact>,
    #[serde(default)]
    blocked: Vec<JobArtifact>,
    #[serde(default)]
    cancelled: Vec<JobArtifact>,
}

impl WorkflowOutcomeArtifactsByOutcome {
    fn for_outcome(&self, outcome: WorkflowNodeOutcome) -> &[JobArtifact] {
        match outcome {
            WorkflowNodeOutcome::Succeeded => &self.succeeded,
            WorkflowNodeOutcome::Failed => &self.failed,
            WorkflowNodeOutcome::Blocked => &self.blocked,
            WorkflowNodeOutcome::Cancelled => &self.cancelled,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct WorkflowRunManifest {
    run_id: String,
    template_selector: String,
    template_id: String,
    template_version: String,
    policy_snapshot_hash: String,
    nodes: BTreeMap<String, WorkflowRuntimeNodeManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnqueueWorkflowRunResult {
    pub run_id: String,
    pub template_selector: String,
    pub template_id: String,
    pub template_version: String,
    pub policy_snapshot_hash: String,
    pub job_ids: BTreeMap<String, String>,
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

fn workflow_run_manifest_path(project_root: &Path, run_id: &str) -> PathBuf {
    project_root
        .join(".vizier/jobs/runs")
        .join(format!("{run_id}.json"))
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

fn custom_artifact_payload_dir(project_root: &Path, type_id: &str, key: &str) -> PathBuf {
    project_root
        .join(".vizier/jobs/artifacts/data")
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

fn custom_artifact_payload_path(
    project_root: &Path,
    job_id: &str,
    type_id: &str,
    key: &str,
) -> PathBuf {
    custom_artifact_payload_dir(project_root, type_id, key).join(format!("{job_id}.json"))
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

fn write_custom_artifact_payload(
    project_root: &Path,
    job_id: &str,
    type_id: &str,
    key: &str,
    payload: &serde_json::Value,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = custom_artifact_payload_path(project_root, job_id, type_id, key);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_vec_pretty(payload)?)?;
    Ok(path)
}

type CustomArtifactPayloadRead = (String, serde_json::Value, PathBuf);

fn read_latest_custom_artifact_payload(
    project_root: &Path,
    type_id: &str,
    key: &str,
) -> Result<Option<CustomArtifactPayloadRead>, Box<dyn std::error::Error>> {
    let marker_dir = custom_artifact_marker_dir(project_root, type_id, key);
    if !marker_dir.is_dir() {
        return Ok(None);
    }

    let mut candidates = Vec::new();
    for entry in fs::read_dir(&marker_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let job_id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_string();
        if job_id.trim().is_empty() {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        candidates.push((modified, job_id));
    }

    if candidates.is_empty() {
        return Ok(None);
    }
    candidates.sort();
    let (_modified, job_id) = candidates
        .last()
        .cloned()
        .ok_or_else(|| "missing payload candidate".to_string())?;
    let payload_path = custom_artifact_payload_path(project_root, &job_id, type_id, key);
    if !payload_path.exists() {
        return Ok(None);
    }

    let payload = serde_json::from_slice::<serde_json::Value>(&fs::read(&payload_path)?)?;
    Ok(Some((job_id, payload, payload_path)))
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
    remove_custom_artifact_markers_with_payload_policy(project_root, job_id, artifacts, &[])
}

fn remove_custom_artifact_markers_with_payload_policy(
    project_root: &Path,
    job_id: &str,
    artifacts: &[JobArtifact],
    keep_payload_for: &[JobArtifact],
) -> Result<(), Box<dyn std::error::Error>> {
    for artifact in artifacts {
        let JobArtifact::Custom { type_id, key } = artifact else {
            continue;
        };
        let marker = custom_artifact_marker_path(project_root, job_id, type_id, key);
        remove_file_if_exists(&marker)?;
        let preserve_payload = keep_payload_for.iter().any(|entry| entry == artifact);
        if !preserve_payload {
            let payload = custom_artifact_payload_path(project_root, job_id, type_id, key);
            remove_file_if_exists(&payload)?;
        }
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
        let payload_key_dir = custom_artifact_payload_dir(project_root, type_id, key);
        if payload_key_dir.is_dir() && fs::read_dir(&payload_key_dir)?.next().is_none() {
            let _ = fs::remove_dir(&payload_key_dir);
        }
        if let Some(type_dir) = payload_key_dir.parent()
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
            if base.workflow_run_id.is_none() {
                base.workflow_run_id = update.workflow_run_id;
            }
            if base.workflow_node_attempt.is_none() {
                base.workflow_node_attempt = update.workflow_node_attempt;
            }
            if base.workflow_node_outcome.is_none() {
                base.workflow_node_outcome = update.workflow_node_outcome;
            }
            if is_empty_vec(&base.workflow_payload_refs) {
                base.workflow_payload_refs = update.workflow_payload_refs;
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
            if base.workflow_executor_class.is_none() {
                base.workflow_executor_class = update.workflow_executor_class;
            }
            if base.workflow_executor_operation.is_none() {
                base.workflow_executor_operation = update.workflow_executor_operation;
            }
            if base.workflow_control_policy.is_none() {
                base.workflow_control_policy = update.workflow_control_policy;
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
            if update.execution_root.is_some() {
                base.execution_root = update.execution_root;
            }
            if update.worktree_owned == Some(false) {
                base.worktree_name = None;
                base.worktree_path = None;
                base.worktree_owned = None;
            } else {
                if update.worktree_name.is_some() {
                    base.worktree_name = update.worktree_name;
                }
                if update.worktree_path.is_some() {
                    base.worktree_path = update.worktree_path;
                }
                if update.worktree_owned.is_some() {
                    base.worktree_owned = update.worktree_owned;
                }
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

fn sanitize_workflow_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    while out.starts_with('-') {
        out.remove(0);
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "node".to_string()
    } else {
        out
    }
}

fn workflow_job_id(run_id: &str, node_id: &str) -> String {
    format!(
        "wf-{}-{}",
        sanitize_workflow_component(run_id),
        sanitize_workflow_component(node_id)
    )
}

fn workflow_node_artifacts_by_outcome(
    template: &WorkflowTemplate,
    node_id: &str,
) -> Result<WorkflowOutcomeArtifactsByOutcome, String> {
    let node = template
        .nodes
        .iter()
        .find(|entry| entry.id == node_id)
        .ok_or_else(|| format!("template node `{node_id}` is missing"))?;
    Ok(WorkflowOutcomeArtifactsByOutcome {
        succeeded: node.produces.succeeded.clone(),
        failed: node.produces.failed.clone(),
        blocked: node.produces.blocked.clone(),
        cancelled: node.produces.cancelled.clone(),
    })
}

fn to_job_preconditions(
    node_id: &str,
    preconditions: &[WorkflowPrecondition],
) -> Result<Vec<JobPrecondition>, Box<dyn std::error::Error>> {
    let mut converted = Vec::new();
    for precondition in preconditions {
        match precondition {
            WorkflowPrecondition::CleanWorktree => converted.push(JobPrecondition::CleanWorktree),
            WorkflowPrecondition::BranchExists => {
                converted.push(JobPrecondition::BranchExists { branch: None })
            }
            WorkflowPrecondition::Custom { id, args } => converted.push(JobPrecondition::Custom {
                id: id.clone(),
                args: args.clone(),
            }),
            WorkflowPrecondition::PinnedHead => {
                return Err(format!(
                    "workflow node `{node_id}` uses unsupported scheduler precondition `pinned_head`"
                )
                .into());
            }
        }
    }
    Ok(converted)
}

fn to_schedule_approval(
    gates: &[vizier_core::workflow_template::WorkflowGate],
) -> Option<JobApproval> {
    for gate in gates {
        if let vizier_core::workflow_template::WorkflowGate::Approval { required, .. } = gate
            && *required
        {
            return Some(pending_job_approval());
        }
    }
    None
}

fn workflow_gate_summary(gate: &vizier_core::workflow_template::WorkflowGate) -> String {
    match gate {
        vizier_core::workflow_template::WorkflowGate::Approval { required, .. } => {
            format!("approval(required={required})")
        }
        vizier_core::workflow_template::WorkflowGate::Script { script, .. } => {
            format!("script({script})")
        }
        vizier_core::workflow_template::WorkflowGate::Cicd {
            script,
            auto_resolve,
            ..
        } => format!("cicd(script={script}, auto_resolve={auto_resolve})"),
        vizier_core::workflow_template::WorkflowGate::Custom { id, .. } => {
            format!("custom({id})")
        }
    }
}

fn append_unique_after(after: &mut Vec<JobAfterDependency>, dependency: JobAfterDependency) {
    if after
        .iter()
        .any(|entry| entry.job_id == dependency.job_id && entry.policy == dependency.policy)
    {
        return;
    }
    after.push(dependency);
}

fn write_workflow_run_manifest(
    project_root: &Path,
    manifest: &WorkflowRunManifest,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = workflow_run_manifest_path(project_root, &manifest.run_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(manifest)?)?;
    Ok(())
}

fn load_workflow_run_manifest(
    project_root: &Path,
    run_id: &str,
) -> Result<WorkflowRunManifest, Box<dyn std::error::Error>> {
    let path = workflow_run_manifest_path(project_root, run_id);
    let bytes = fs::read(path)?;
    Ok(serde_json::from_slice::<WorkflowRunManifest>(&bytes)?)
}

fn compiled_node_lookup(
    template: &WorkflowTemplate,
) -> Result<BTreeMap<String, CompiledWorkflowNode>, Box<dyn std::error::Error>> {
    let mut resolved_after = BTreeMap::new();
    for node in &template.nodes {
        resolved_after.insert(node.id.clone(), workflow_job_id(&template.id, &node.id));
    }

    let mut compiled = BTreeMap::new();
    for node in &template.nodes {
        let node_compiled = compile_workflow_node(template, &node.id, &resolved_after)?;
        compiled.insert(node.id.clone(), node_compiled);
    }
    Ok(compiled)
}

fn convert_outcome_edges_to_routes(edges: &WorkflowOutcomeEdges) -> WorkflowRouteTargets {
    let to_targets = |values: &[String], mode: WorkflowRouteMode| {
        values
            .iter()
            .map(|target| WorkflowRouteTarget {
                node_id: target.clone(),
                mode,
            })
            .collect::<Vec<_>>()
    };

    WorkflowRouteTargets {
        succeeded: to_targets(&edges.succeeded, WorkflowRouteMode::PropagateContext),
        failed: to_targets(&edges.failed, WorkflowRouteMode::RetryJob),
        blocked: to_targets(&edges.blocked, WorkflowRouteMode::RetryJob),
        cancelled: to_targets(&edges.cancelled, WorkflowRouteMode::RetryJob),
    }
}

pub fn enqueue_workflow_run(
    project_root: &Path,
    jobs_root: &Path,
    run_id: &str,
    template_selector: &str,
    template: &WorkflowTemplate,
    recorded_args: &[String],
    config_snapshot: Option<serde_json::Value>,
) -> Result<EnqueueWorkflowRunResult, Box<dyn std::error::Error>> {
    validate_workflow_capability_contracts(template)?;
    if template.nodes.is_empty() {
        return Err("workflow template has no nodes".into());
    }

    let mut node_to_job_id = BTreeMap::new();
    for node in &template.nodes {
        let job_id = workflow_job_id(run_id, &node.id);
        if node_to_job_id.insert(node.id.clone(), job_id).is_some() {
            return Err(format!("duplicate workflow node id `{}`", node.id).into());
        }
    }

    let mut resolved_after = BTreeMap::new();
    for (node_id, job_id) in &node_to_job_id {
        resolved_after.insert(node_id.clone(), job_id.clone());
    }

    let mut incoming_success = BTreeMap::<String, Vec<String>>::new();
    for source in &template.nodes {
        for target in &source.on.succeeded {
            incoming_success
                .entry(target.clone())
                .or_default()
                .push(source.id.clone());
        }
    }

    for (target, parents) in &incoming_success {
        if parents.len() > 1 {
            let list = parents.join(", ");
            return Err(format!(
                "template {}@{} node `{}` has multiple on.succeeded parents ({list}); runtime bridge currently requires a single parent",
                template.id, template.version, target
            )
            .into());
        }
    }

    let policy_snapshot_hash = template.policy_snapshot().stable_hash_hex()?;
    let mut manifest_nodes = BTreeMap::new();

    for node in &template.nodes {
        let compiled = compile_workflow_node(template, &node.id, &resolved_after)?;
        let job_id = node_to_job_id
            .get(&node.id)
            .cloned()
            .ok_or_else(|| format!("missing job id for node {}", node.id))?;

        let mut after = compiled.after.clone();
        if let Some(parents) = incoming_success.get(&node.id)
            && let Some(parent) = parents.first()
            && let Some(parent_job_id) = node_to_job_id.get(parent)
        {
            append_unique_after(
                &mut after,
                JobAfterDependency {
                    job_id: parent_job_id.clone(),
                    policy: AfterPolicy::Success,
                },
            );
        }
        sort_after_dependencies(&mut after);
        after.dedup();

        let preconditions = to_job_preconditions(&node.id, &compiled.preconditions)?;
        let approval = to_schedule_approval(&compiled.gates);
        let dependencies = compiled
            .dependencies
            .iter()
            .map(|artifact| JobDependency {
                artifact: artifact.clone(),
            })
            .collect::<Vec<_>>();

        let schedule = JobSchedule {
            after,
            dependencies,
            locks: compiled.locks.clone(),
            artifacts: compiled.artifacts.clone(),
            pinned_head: None,
            preconditions,
            approval,
            wait_reason: None,
            waited_on: Vec::new(),
        };

        let metadata = JobMetadata {
            workflow_run_id: Some(run_id.to_string()),
            workflow_node_attempt: Some(1),
            workflow_template_selector: Some(template_selector.to_string()),
            workflow_template_id: Some(compiled.template_id.clone()),
            workflow_template_version: Some(compiled.template_version.clone()),
            workflow_node_id: Some(compiled.node_id.clone()),
            workflow_executor_class: compiled
                .executor_class
                .map(|value| value.as_str().to_string()),
            workflow_executor_operation: compiled.executor_operation.clone(),
            workflow_control_policy: compiled.control_policy.clone(),
            workflow_policy_snapshot_hash: Some(compiled.policy_snapshot_hash.clone()),
            workflow_gates: if compiled.gates.is_empty() {
                None
            } else {
                Some(
                    compiled
                        .gates
                        .iter()
                        .map(workflow_gate_summary)
                        .collect::<Vec<_>>(),
                )
            },
            ..JobMetadata::default()
        };

        let child_args = vec![
            "__workflow-node".to_string(),
            "--job-id".to_string(),
            job_id.clone(),
        ];
        let command = if recorded_args.is_empty() {
            vec![
                "vizier".to_string(),
                "__workflow-node".to_string(),
                "--job-id".to_string(),
                job_id.clone(),
            ]
        } else {
            recorded_args.to_vec()
        };
        enqueue_job(
            project_root,
            jobs_root,
            &job_id,
            &child_args,
            &command,
            Some(metadata),
            config_snapshot.clone(),
            Some(schedule),
        )?;

        manifest_nodes.insert(
            node.id.clone(),
            WorkflowRuntimeNodeManifest {
                node_id: node.id.clone(),
                job_id: job_id.clone(),
                uses: node.uses.clone(),
                kind: node.kind,
                args: node.args.clone(),
                executor_operation: compiled.executor_operation.clone(),
                control_policy: compiled.control_policy.clone(),
                gates: compiled.gates.clone(),
                retry: compiled.retry.clone(),
                routes: convert_outcome_edges_to_routes(&compiled.on),
                artifacts_by_outcome: workflow_node_artifacts_by_outcome(template, &node.id)?,
            },
        );
    }

    write_workflow_run_manifest(
        project_root,
        &WorkflowRunManifest {
            run_id: run_id.to_string(),
            template_selector: template_selector.to_string(),
            template_id: template.id.clone(),
            template_version: template.version.clone(),
            policy_snapshot_hash: policy_snapshot_hash.clone(),
            nodes: manifest_nodes,
        },
    )?;

    Ok(EnqueueWorkflowRunResult {
        run_id: run_id.to_string(),
        template_selector: template_selector.to_string(),
        template_id: template.id.clone(),
        template_version: template.version.clone(),
        policy_snapshot_hash,
        job_ids: node_to_job_id,
    })
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
    finalize_job_with_artifacts(
        project_root,
        jobs_root,
        job_id,
        status,
        exit_code,
        session_path,
        metadata,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn finalize_job_with_artifacts(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
    status: JobStatus,
    exit_code: i32,
    session_path: Option<String>,
    metadata: Option<JobMetadata>,
    artifacts_written: Option<&[JobArtifact]>,
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
        let artifacts = if let Some(artifacts) = artifacts_written {
            artifacts
        } else if record.status == JobStatus::Succeeded {
            &schedule.artifacts[..]
        } else {
            &[]
        };
        remove_custom_artifact_markers_with_payload_policy(
            project_root,
            job_id,
            &schedule.artifacts,
            artifacts,
        )?;
        if !artifacts.is_empty() {
            write_custom_artifact_markers(project_root, job_id, artifacts)?;
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

fn dedup_job_artifacts(mut artifacts: Vec<JobArtifact>) -> Vec<JobArtifact> {
    sort_artifacts(&mut artifacts);
    artifacts.dedup();
    artifacts
}

fn map_workflow_outcome_to_job_status(
    outcome: WorkflowNodeOutcome,
    exit_code: Option<i32>,
) -> (JobStatus, i32) {
    match outcome {
        WorkflowNodeOutcome::Succeeded => (JobStatus::Succeeded, exit_code.unwrap_or(0)),
        WorkflowNodeOutcome::Failed => (JobStatus::Failed, exit_code.unwrap_or(1)),
        WorkflowNodeOutcome::Blocked => (JobStatus::BlockedByDependency, exit_code.unwrap_or(10)),
        WorkflowNodeOutcome::Cancelled => (JobStatus::Cancelled, exit_code.unwrap_or(143)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkflowExecutionContext {
    execution_root: Option<String>,
    worktree_path: Option<String>,
    worktree_name: Option<String>,
    worktree_owned: Option<bool>,
}

fn normalized_metadata_value(value: Option<&String>) -> Option<String> {
    value
        .map(|entry| entry.trim())
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.to_string())
}

fn workflow_execution_context_from_metadata(
    metadata: Option<&JobMetadata>,
) -> Option<WorkflowExecutionContext> {
    let metadata = metadata?;
    let context = WorkflowExecutionContext {
        execution_root: normalized_metadata_value(metadata.execution_root.as_ref()),
        worktree_path: normalized_metadata_value(metadata.worktree_path.as_ref()),
        worktree_name: normalized_metadata_value(metadata.worktree_name.as_ref()),
        worktree_owned: metadata.worktree_owned,
    };
    if context.execution_root.is_none() && context.worktree_path.is_none() {
        None
    } else {
        Some(context)
    }
}

fn run_shell_text_command(
    execution_root: &Path,
    script: &str,
) -> Result<(i32, String, String), Box<dyn std::error::Error>> {
    let output = Command::new("sh")
        .arg("-lc")
        .arg(script)
        .current_dir(execution_root)
        .output()?;
    let status = output.status.code().unwrap_or(1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok((status, stdout, stderr))
}

fn parse_bool_like(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn bool_arg(args: &BTreeMap<String, String>, key: &str) -> Option<bool> {
    args.get(key).and_then(|value| parse_bool_like(value))
}

fn resolve_execution_root(
    project_root: &Path,
    record: &JobRecord,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let canonical_project_root = project_root.canonicalize().map_err(|err| {
        format!(
            "cannot resolve repository root {}: {}",
            project_root.display(),
            err
        )
    })?;
    let Some(metadata) = record.metadata.as_ref() else {
        return Ok(canonical_project_root);
    };

    if let Some(execution_root) = normalized_metadata_value(metadata.execution_root.as_ref()) {
        return resolve_execution_root_candidate(
            project_root,
            &canonical_project_root,
            &execution_root,
            "execution_root",
        );
    }

    if let Some(worktree_path) = normalized_metadata_value(metadata.worktree_path.as_ref()) {
        return resolve_execution_root_candidate(
            project_root,
            &canonical_project_root,
            &worktree_path,
            "worktree_path",
        );
    }

    Ok(canonical_project_root)
}

fn resolve_execution_root_candidate(
    project_root: &Path,
    canonical_project_root: &Path,
    recorded: &str,
    field_name: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let resolved = resolve_recorded_path(project_root, recorded);
    let canonical = resolved.canonicalize().map_err(|err| {
        format!(
            "workflow metadata.{field_name} path {} is invalid: {}",
            resolved.display(),
            err
        )
    })?;
    if !canonical.starts_with(canonical_project_root) {
        return Err(format!(
            "workflow metadata.{field_name} path {} is outside repository root {}",
            canonical.display(),
            canonical_project_root.display()
        )
        .into());
    }
    Ok(canonical)
}

fn resolve_path_in_execution_root(execution_root: &Path, path: &str) -> PathBuf {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        candidate
    } else {
        execution_root.join(candidate)
    }
}

fn first_non_empty_arg(args: &BTreeMap<String, String>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = args.get(*key)
            && !value.trim().is_empty()
        {
            return Some(value.trim().to_string());
        }
    }
    None
}

fn resolve_node_shell_script(
    node: &WorkflowRuntimeNodeManifest,
    fallback: Option<String>,
) -> Option<String> {
    first_non_empty_arg(&node.args, &["command", "script"]).or(fallback)
}

fn script_gate_script(node: &WorkflowRuntimeNodeManifest) -> Option<String> {
    node.gates.iter().find_map(|gate| match gate {
        WorkflowGate::Script { script, .. } => {
            if script.trim().is_empty() {
                None
            } else {
                Some(script.trim().to_string())
            }
        }
        _ => None,
    })
}

fn cicd_gate_config(node: &WorkflowRuntimeNodeManifest) -> Option<(String, bool)> {
    node.gates.iter().find_map(|gate| match gate {
        WorkflowGate::Cicd {
            script,
            auto_resolve,
            ..
        } if !script.trim().is_empty() => Some((script.trim().to_string(), *auto_resolve)),
        _ => None,
    })
}

fn conflict_auto_resolve_from_gate(node: &WorkflowRuntimeNodeManifest) -> Option<bool> {
    for gate in &node.gates {
        if let WorkflowGate::Custom { id, args, .. } = gate
            && id == "conflict_resolution"
            && let Some(value) = args
                .get("auto_resolve")
                .and_then(|raw| parse_bool_like(raw))
        {
            return Some(value);
        }
    }
    None
}

fn workflow_slug_from_record(record: &JobRecord, node: &WorkflowRuntimeNodeManifest) -> String {
    if let Some(value) = first_non_empty_arg(&node.args, &["slug", "plan"]) {
        return value;
    }
    if let Some(value) = record
        .metadata
        .as_ref()
        .and_then(|meta| meta.plan.as_ref())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        return value.to_string();
    }
    if let Some(value) = record
        .metadata
        .as_ref()
        .and_then(|meta| meta.branch.as_ref())
        .and_then(|branch| branch.strip_prefix("draft/"))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        return value.to_string();
    }
    sanitize_workflow_component(&record.id)
}

fn merge_sentinel_path(project_root: &Path, slug: &str) -> PathBuf {
    project_root
        .join(".vizier/tmp/merge-conflicts")
        .join(format!("{slug}.json"))
}

fn ensure_local_branch(
    execution_root: &Path,
    branch: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let exists = Command::new("git")
        .arg("-C")
        .arg(execution_root)
        .arg("show-ref")
        .arg("--verify")
        .arg("--quiet")
        .arg(format!("refs/heads/{branch}"))
        .status()?;
    if exists.success() {
        return Ok(());
    }

    let created = Command::new("git")
        .arg("-C")
        .arg(execution_root)
        .arg("branch")
        .arg(branch)
        .status()?;
    if created.success() {
        Ok(())
    } else {
        Err(format!("unable to create local branch `{branch}`").into())
    }
}

fn current_branch_name(execution_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(execution_root)
        .arg("rev-parse")
        .arg("--abbrev-ref")
        .arg("HEAD")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() || name == "HEAD" {
        None
    } else {
        Some(name)
    }
}

fn has_unmerged_paths(execution_root: &Path) -> bool {
    let output = Command::new("git")
        .arg("-C")
        .arg(execution_root)
        .arg("ls-files")
        .arg("-u")
        .output();
    match output {
        Ok(output) if output.status.success() => {
            !String::from_utf8_lossy(&output.stdout).trim().is_empty()
        }
        _ => false,
    }
}

fn parse_files_json(
    node: &WorkflowRuntimeNodeManifest,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let Some(raw) = node.args.get("files_json") else {
        return Err("patch pipeline operation requires args.files_json".into());
    };
    let files = serde_json::from_str::<Vec<String>>(raw)
        .map_err(|err| format!("invalid files_json payload: {err}"))?;
    if files.is_empty() {
        return Err("patch pipeline operation requires at least one file".into());
    }
    Ok(files)
}

fn patch_pipeline_manifest_path(jobs_root: &Path, job_id: &str) -> PathBuf {
    jobs_root.join(job_id).join("patch-pipeline.json")
}

fn patch_pipeline_finalize_path(jobs_root: &Path, job_id: &str) -> PathBuf {
    jobs_root.join(job_id).join("patch-pipeline.finalize.json")
}

fn resolve_workflow_agent_settings(
    record: &JobRecord,
) -> Result<config::AgentSettings, Box<dyn std::error::Error>> {
    let cfg = config::get_config();
    let scope_alias = record
        .metadata
        .as_ref()
        .and_then(|meta| meta.command_alias.clone().or(meta.scope.clone()));
    let template_selector = record
        .metadata
        .as_ref()
        .and_then(|meta| meta.workflow_template_selector.clone());

    if let Some(raw_alias) = scope_alias {
        if let Some(alias) = config::CommandAlias::parse(&raw_alias) {
            if let Some(raw_template) = template_selector
                && let Some(selector) = config::TemplateSelector::parse(&raw_template)
            {
                return config::resolve_agent_settings_for_alias_template(
                    &cfg,
                    &alias,
                    Some(&selector),
                    None,
                );
            }
            return config::resolve_agent_settings_for_alias(&cfg, &alias, None);
        }
        if let Ok(scope) = raw_alias.parse::<config::CommandScope>() {
            return config::resolve_agent_settings(&cfg, scope, None);
        }
    }

    config::resolve_default_agent_settings(&cfg, None)
}

fn build_workflow_agent_request(
    agent: &config::AgentSettings,
    prompt: String,
    repo_root: PathBuf,
) -> AgentRequest {
    let mut metadata = BTreeMap::new();
    metadata.insert("agent_backend".to_string(), agent.backend.to_string());
    metadata.insert("agent_label".to_string(), agent.agent_runtime.label.clone());
    metadata.insert(
        "agent_command".to_string(),
        agent.agent_runtime.command.join(" "),
    );
    metadata.insert(
        "agent_output".to_string(),
        agent.agent_runtime.output.as_str().to_string(),
    );
    if let Some(alias) = agent.command_alias.as_ref() {
        metadata.insert("command_alias".to_string(), alias.to_string());
    }
    if let Some(selector) = agent.template_selector.as_ref() {
        metadata.insert("template_selector".to_string(), selector.to_string());
    }
    if let Some(filter) = agent.agent_runtime.progress_filter.as_ref() {
        metadata.insert("agent_progress_filter".to_string(), filter.join(" "));
    }
    match &agent.agent_runtime.resolution {
        config::AgentRuntimeResolution::BundledShim { path, .. } => {
            metadata.insert(
                "agent_command_source".to_string(),
                "bundled-shim".to_string(),
            );
            metadata.insert("agent_shim_path".to_string(), path.display().to_string());
        }
        config::AgentRuntimeResolution::ProvidedCommand => {
            metadata.insert("agent_command_source".to_string(), "configured".to_string());
        }
    }

    AgentRequest {
        prompt,
        repo_root,
        command: agent.agent_runtime.command.clone(),
        progress_filter: agent.agent_runtime.progress_filter.clone(),
        output: agent.agent_runtime.output,
        allow_script_wrapper: agent.agent_runtime.enable_script_wrapper,
        scope: agent.scope,
        metadata,
        timeout: Some(DEFAULT_AGENT_TIMEOUT),
    }
}

fn execute_agent_request_blocking(
    runner: std::sync::Arc<dyn vizier_core::agent::AgentRunner>,
    request: AgentRequest,
) -> Result<vizier_core::agent::AgentResponse, AgentError> {
    let handle = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| AgentError::Io(io::Error::other(format!("tokio runtime: {err}"))))?;
        runtime.block_on(runner.execute(request, None))
    });

    match handle.join() {
        Ok(result) => result,
        Err(_) => Err(AgentError::Io(io::Error::other(
            "agent worker thread panicked",
        ))),
    }
}

fn workflow_prompt_text_from_record(
    execution_root: &Path,
    record: &JobRecord,
    node: &WorkflowRuntimeNodeManifest,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(text) = node.args.get("prompt_text")
        && !text.trim().is_empty()
    {
        return Ok(text.clone());
    }
    if let Some(path) = node.args.get("prompt_file")
        && !path.trim().is_empty()
    {
        let abs = resolve_path_in_execution_root(execution_root, path);
        return Ok(fs::read_to_string(abs)?);
    }
    if let Some(command) = node.args.get("command")
        && !command.trim().is_empty()
    {
        let (status, stdout, stderr) = run_shell_text_command(execution_root, command)?;
        if status != 0 {
            return Err(format!(
                "prompt.resolve command failed (exit {status}): {}",
                stderr.trim()
            )
            .into());
        }
        return Ok(stdout);
    }
    if let Some(script) = node.args.get("script")
        && !script.trim().is_empty()
    {
        let (status, stdout, stderr) = run_shell_text_command(execution_root, script)?;
        if status != 0 {
            return Err(format!(
                "prompt.resolve script failed (exit {status}): {}",
                stderr.trim()
            )
            .into());
        }
        return Ok(stdout);
    }

    let from_config = record
        .config_snapshot
        .as_ref()
        .and_then(|snapshot| {
            snapshot
                .pointer("/workflow/prompt_text")
                .and_then(|value| value.as_str())
                .or_else(|| {
                    snapshot
                        .pointer("/workflow_runtime/prompt_text")
                        .and_then(|value| value.as_str())
                })
        })
        .map(|value| value.to_string());
    if let Some(text) = from_config {
        return Ok(text);
    }

    if let Ok(value) = std::env::var("VIZIER_WORKFLOW_PROMPT_TEXT")
        && !value.trim().is_empty()
    {
        return Ok(value);
    }

    Err("prompt.resolve missing prompt_text source (args.prompt_text/prompt_file/command/script, config workflow.prompt_text, or VIZIER_WORKFLOW_PROMPT_TEXT)".into())
}

fn prompt_output_artifact(node: &WorkflowRuntimeNodeManifest) -> Option<JobArtifact> {
    let mut all = node.artifacts_by_outcome.succeeded.clone();
    all.extend(node.artifacts_by_outcome.failed.iter().cloned());
    all.extend(node.artifacts_by_outcome.blocked.iter().cloned());
    all.extend(node.artifacts_by_outcome.cancelled.iter().cloned());
    all.into_iter().find(|artifact| {
        matches!(
            artifact,
            JobArtifact::Custom { type_id, .. } if type_id == PROMPT_ARTIFACT_TYPE_ID
        )
    })
}

fn resolve_prompt_payload_text(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("text")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .or_else(|| {
            payload
                .pointer("/payload/text")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
        })
}

fn execute_workflow_executor(
    project_root: &Path,
    jobs_root: &Path,
    record: &JobRecord,
    node: &WorkflowRuntimeNodeManifest,
) -> Result<WorkflowNodeResult, Box<dyn std::error::Error>> {
    let execution_root = resolve_execution_root(project_root, record)?;
    match node.executor_operation.as_deref() {
        Some("worktree.prepare") => {
            let branch = first_non_empty_arg(&node.args, &["branch"])
                .or_else(|| {
                    record
                        .metadata
                        .as_ref()
                        .and_then(|meta| meta.branch.as_ref().cloned())
                })
                .or_else(|| {
                    first_non_empty_arg(&node.args, &["slug", "plan"])
                        .map(|slug| crate::plan::default_branch_for_slug(&slug))
                })
                .or_else(|| {
                    record
                        .metadata
                        .as_ref()
                        .and_then(|meta| meta.plan.as_ref())
                        .map(|slug| crate::plan::default_branch_for_slug(slug))
                });
            let Some(branch) = branch else {
                return Ok(WorkflowNodeResult::failed(
                    "worktree.prepare could not determine branch (set branch or slug/plan)",
                    Some(1),
                ));
            };
            if let Err(err) = ensure_local_branch(project_root, &branch) {
                return Ok(WorkflowNodeResult::failed(
                    format!("worktree.prepare could not ensure branch `{branch}`: {err}"),
                    Some(1),
                ));
            }

            let purpose = first_non_empty_arg(&node.args, &["purpose"])
                .unwrap_or_else(|| sanitize_workflow_component(&node.node_id));
            let dir_name = format!("{}-{}", sanitize_workflow_component(&purpose), record.id);
            let worktree_path = project_root.join(".vizier/tmp-worktrees").join(&dir_name);
            if let Some(parent) = worktree_path.parent() {
                fs::create_dir_all(parent)?;
            }

            if worktree_path.exists() {
                let mut result =
                    WorkflowNodeResult::succeeded("worktree already exists for this node");
                result.payload_refs = vec![relative_path(project_root, &worktree_path)];
                result.metadata = Some(JobMetadata {
                    execution_root: Some(relative_path(project_root, &worktree_path)),
                    worktree_owned: Some(true),
                    worktree_path: Some(relative_path(project_root, &worktree_path)),
                    worktree_name: find_worktree_name_by_path(
                        &Repository::open(project_root)?,
                        &worktree_path,
                    ),
                    ..JobMetadata::default()
                });
                return Ok(result);
            }

            let add = Command::new("git")
                .arg("-C")
                .arg(project_root)
                .arg("worktree")
                .arg("add")
                .arg(&worktree_path)
                .arg(&branch)
                .output()?;
            if !add.status.success() {
                let detail = String::from_utf8_lossy(&add.stderr).trim().to_string();
                let reason = if detail.is_empty() {
                    "worktree.prepare failed to add worktree".to_string()
                } else {
                    format!("worktree.prepare failed to add worktree: {detail}")
                };
                return Ok(WorkflowNodeResult::failed(
                    reason,
                    Some(add.status.code().unwrap_or(1)),
                ));
            }

            let mut result = WorkflowNodeResult::succeeded("worktree prepared");
            result.payload_refs = vec![relative_path(project_root, &worktree_path)];
            let worktree_name =
                find_worktree_name_by_path(&Repository::open(project_root)?, &worktree_path);
            result.metadata = Some(JobMetadata {
                branch: Some(branch),
                execution_root: Some(relative_path(project_root, &worktree_path)),
                worktree_owned: Some(true),
                worktree_path: Some(relative_path(project_root, &worktree_path)),
                worktree_name,
                ..JobMetadata::default()
            });
            Ok(result)
        }
        Some("worktree.cleanup") => {
            let Some(metadata) = record.metadata.as_ref() else {
                return Ok(WorkflowNodeResult::succeeded(
                    "worktree cleanup skipped (no worktree metadata)",
                ));
            };
            if metadata.worktree_owned != Some(true) {
                return Ok(WorkflowNodeResult::succeeded(
                    "worktree cleanup skipped (worktree not marked as job-owned)",
                ));
            }
            let Some(recorded_path) = metadata.worktree_path.as_ref() else {
                return Ok(WorkflowNodeResult::failed(
                    "worktree cleanup cannot run: missing worktree_path metadata",
                    Some(1),
                ));
            };

            let worktree_path = resolve_recorded_path(project_root, recorded_path);
            let worktree_name = metadata.worktree_name.as_deref();
            if !worktree_safe_to_remove(project_root, &worktree_path, worktree_name) {
                return Ok(WorkflowNodeResult::failed(
                    format!(
                        "refusing to cleanup unsafe worktree path {}",
                        worktree_path.display()
                    ),
                    Some(1),
                ));
            }

            let mut result =
                if let Err(err) = cleanup_worktree(project_root, &worktree_path, worktree_name) {
                    display::warn(format!(
                        "workflow worktree cleanup degraded for job {}: {}",
                        record.id, err
                    ));
                    let mut degraded = WorkflowNodeResult::succeeded(
                        "worktree cleanup degraded (manual prune may be needed)",
                    );
                    degraded.metadata = Some(JobMetadata {
                        retry_cleanup_status: Some(RetryCleanupStatus::Degraded),
                        retry_cleanup_error: Some(err),
                        ..JobMetadata::default()
                    });
                    degraded
                } else {
                    let mut cleaned = WorkflowNodeResult::succeeded("worktree cleaned");
                    cleaned.metadata = Some(JobMetadata {
                        execution_root: Some(".".to_string()),
                        worktree_owned: Some(false),
                        retry_cleanup_status: Some(RetryCleanupStatus::Done),
                        retry_cleanup_error: None,
                        ..JobMetadata::default()
                    });
                    cleaned
                };
            result.payload_refs = vec![relative_path(project_root, &worktree_path)];
            Ok(result)
        }
        Some("prompt.resolve") => {
            let prompt_artifact = prompt_output_artifact(node).ok_or_else(|| {
                "prompt.resolve node is missing prompt artifact output".to_string()
            })?;
            let (type_id, key) = match prompt_artifact.clone() {
                JobArtifact::Custom { type_id, key } => (type_id, key),
                _ => {
                    return Err("prompt.resolve output artifact must be custom prompt_text".into());
                }
            };

            let prompt_text = workflow_prompt_text_from_record(&execution_root, record, node)?;
            let payload = serde_json::json!({
                "type_id": type_id,
                "key": key,
                "text": prompt_text,
                "written_at": Utc::now().to_rfc3339(),
            });
            let path =
                write_custom_artifact_payload(project_root, &record.id, &type_id, &key, &payload)?;
            Ok(WorkflowNodeResult {
                outcome: WorkflowNodeOutcome::Succeeded,
                artifacts_written: vec![prompt_artifact],
                payload_refs: vec![relative_path(project_root, &path)],
                metadata: None,
                summary: Some("prompt resolved".to_string()),
                exit_code: Some(0),
            })
        }
        Some("agent.invoke") => {
            let prompt_dependency = record
                .schedule
                .as_ref()
                .and_then(|schedule| {
                    schedule
                        .dependencies
                        .iter()
                        .find_map(|dependency| match &dependency.artifact {
                            JobArtifact::Custom { type_id, key }
                                if type_id == PROMPT_ARTIFACT_TYPE_ID =>
                            {
                                Some((type_id.clone(), key.clone()))
                            }
                            _ => None,
                        })
                })
                .ok_or_else(|| {
                    "agent.invoke requires a custom:prompt_text dependency".to_string()
                })?;
            let (type_id, key) = prompt_dependency;
            let (_producer_job, payload, payload_path) =
                read_latest_custom_artifact_payload(project_root, &type_id, &key)?.ok_or_else(
                    || {
                        format!(
                            "agent.invoke could not find prompt payload for custom:{}:{}",
                            type_id, key
                        )
                    },
                )?;
            let prompt_text = resolve_prompt_payload_text(&payload)
                .ok_or_else(|| "prompt payload missing text field".to_string())?;

            let agent_settings = match resolve_workflow_agent_settings(record) {
                Ok(settings) => settings,
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("agent.invoke could not resolve agent settings: {err}"),
                        Some(1),
                    ));
                }
            };
            let runner = match agent_settings.agent_runner() {
                Ok(runner) => runner.clone(),
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("agent.invoke requires agent backend runner: {err}"),
                        Some(1),
                    ));
                }
            };
            let request = build_workflow_agent_request(
                &agent_settings,
                prompt_text,
                execution_root.to_path_buf(),
            );
            let response = execute_agent_request_blocking(runner, request);
            match response {
                Ok(response) => {
                    if !response.assistant_text.is_empty() {
                        print!("{}", response.assistant_text);
                        let _ = io::stdout().flush();
                    }
                    for line in response.stderr {
                        eprintln!("{line}");
                    }

                    let mut result = WorkflowNodeResult::succeeded(
                        "agent.invoke completed via configured runner",
                    );
                    result.payload_refs = vec![relative_path(project_root, &payload_path)];
                    result.metadata = Some(JobMetadata {
                        agent_selector: Some(agent_settings.selector.clone()),
                        agent_backend: Some(agent_settings.backend.to_string()),
                        agent_label: Some(agent_settings.agent_runtime.label.clone()),
                        agent_command: Some(agent_settings.agent_runtime.command.clone()),
                        config_backend: Some(agent_settings.backend.to_string()),
                        config_agent_selector: Some(agent_settings.selector.clone()),
                        config_agent_label: Some(agent_settings.agent_runtime.label.clone()),
                        config_agent_command: Some(agent_settings.agent_runtime.command.clone()),
                        agent_exit_code: Some(response.exit_code),
                        ..JobMetadata::default()
                    });
                    Ok(result)
                }
                Err(AgentError::NonZeroExit(code, lines)) => {
                    for line in lines {
                        eprintln!("{line}");
                    }
                    Ok(WorkflowNodeResult::failed(
                        format!("agent.invoke failed (exit {code})"),
                        Some(code),
                    ))
                }
                Err(AgentError::Timeout(secs)) => Ok(WorkflowNodeResult::failed(
                    format!("agent.invoke timed out after {secs}s"),
                    Some(124),
                )),
                Err(err) => Ok(WorkflowNodeResult::failed(
                    format!("agent.invoke failed: {err}"),
                    Some(1),
                )),
            }
        }
        Some("plan.persist") => {
            let spec_source = node
                .args
                .get("spec_source")
                .map(|value| value.trim().to_ascii_lowercase())
                .unwrap_or_else(|| "inline".to_string());
            let spec_text_arg = node
                .args
                .get("spec_text")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            let spec_file = node
                .args
                .get("spec_file")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            let spec_text = match spec_source.as_str() {
                "inline" | "stdin" => {
                    if let Some(text) = spec_text_arg {
                        text
                    } else if let Some(path) = spec_file {
                        fs::read_to_string(resolve_path_in_execution_root(&execution_root, &path))?
                    } else {
                        return Ok(WorkflowNodeResult::failed(
                            "plan.persist requires spec_text or spec_file",
                            Some(1),
                        ));
                    }
                }
                "file" => {
                    let Some(path) = spec_file else {
                        return Ok(WorkflowNodeResult::failed(
                            "plan.persist has spec_source=file but no spec_file",
                            Some(1),
                        ));
                    };
                    fs::read_to_string(resolve_path_in_execution_root(&execution_root, &path))?
                }
                other => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("plan.persist has unsupported spec_source `{other}`"),
                        Some(1),
                    ));
                }
            };

            let requested_slug = first_non_empty_arg(&node.args, &["name_override", "slug"])
                .or_else(|| record.metadata.as_ref().and_then(|meta| meta.plan.clone()))
                .unwrap_or_else(|| crate::plan::slug_from_spec(&spec_text));
            let slug = match crate::plan::sanitize_name_override(&requested_slug) {
                Ok(value) => value,
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("plan.persist invalid slug `{requested_slug}`: {err}"),
                        Some(1),
                    ));
                }
            };
            let branch = first_non_empty_arg(&node.args, &["branch"])
                .or_else(|| {
                    record
                        .metadata
                        .as_ref()
                        .and_then(|meta| meta.branch.clone())
                })
                .unwrap_or_else(|| crate::plan::default_branch_for_slug(&slug));
            if let Err(err) = ensure_local_branch(&execution_root, &branch) {
                return Ok(WorkflowNodeResult::failed(
                    format!("plan.persist could not ensure branch `{branch}`: {err}"),
                    Some(1),
                ));
            }

            let plan_id = first_non_empty_arg(&node.args, &["plan_id"])
                .unwrap_or_else(crate::plan::new_plan_id);
            let plan_body = first_non_empty_arg(&node.args, &["plan_body", "plan_text", "content"])
                .unwrap_or_else(|| spec_text.clone());
            let doc_contents =
                crate::plan::render_plan_document(&plan_id, &slug, &branch, &spec_text, &plan_body);
            let plan_rel = crate::plan::plan_rel_path(&slug);
            let plan_abs = execution_root.join(&plan_rel);
            if let Err(err) = crate::plan::write_plan_file(&plan_abs, &doc_contents) {
                return Ok(WorkflowNodeResult::failed(
                    format!("plan.persist failed to write {}: {err}", plan_rel.display()),
                    Some(1),
                ));
            }

            let now = Utc::now().to_rfc3339();
            let summary = spec_text
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(|line| line.chars().take(160).collect::<String>());
            let state_rel = crate::plan::upsert_plan_record(
                &execution_root,
                crate::plan::PlanRecordUpsert {
                    plan_id: plan_id.clone(),
                    slug: Some(slug.clone()),
                    branch: Some(branch.clone()),
                    source: Some(spec_source),
                    intent: first_non_empty_arg(&node.args, &["intent"]),
                    target_branch: first_non_empty_arg(&node.args, &["target_branch"]).or_else(
                        || {
                            record
                                .metadata
                                .as_ref()
                                .and_then(|meta| meta.target.clone())
                        },
                    ),
                    work_ref: Some(format!("workflow-job:{}", record.id)),
                    status: Some("proposed".to_string()),
                    summary,
                    updated_at: now.clone(),
                    created_at: Some(now),
                    job_ids: Some(HashMap::from([("persist".to_string(), record.id.clone())])),
                },
            )?;

            let mut result = WorkflowNodeResult::succeeded("plan persisted");
            result.artifacts_written = vec![
                JobArtifact::PlanBranch {
                    slug: slug.clone(),
                    branch: branch.clone(),
                },
                JobArtifact::PlanDoc {
                    slug: slug.clone(),
                    branch: branch.clone(),
                },
            ];
            result.payload_refs = vec![
                relative_path(project_root, &plan_abs),
                relative_path(project_root, &execution_root.join(state_rel)),
            ];
            result.metadata = Some(JobMetadata {
                plan: Some(slug),
                branch: Some(branch),
                ..JobMetadata::default()
            });
            Ok(result)
        }
        Some("git.stage_commit") => {
            let add = Command::new("git")
                .arg("-C")
                .arg(&execution_root)
                .arg("add")
                .arg("-A")
                .status()?;
            if !add.success() {
                return Ok(WorkflowNodeResult::failed(
                    "git add -A failed during git.stage_commit",
                    Some(add.code().unwrap_or(1)),
                ));
            }

            let diff = Command::new("git")
                .arg("-C")
                .arg(&execution_root)
                .arg("diff")
                .arg("--cached")
                .arg("--quiet")
                .status()?;
            if diff.success() {
                return Ok(WorkflowNodeResult::succeeded(
                    "git.stage_commit: no staged changes",
                ));
            }

            let message = node
                .args
                .get("message")
                .filter(|value| !value.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| "chore: workflow stage commit".to_string());
            let commit = Command::new("git")
                .arg("-C")
                .arg(&execution_root)
                .arg("commit")
                .arg("-m")
                .arg(&message)
                .status()?;
            if commit.success() {
                Ok(WorkflowNodeResult::succeeded(
                    "git.stage_commit committed changes",
                ))
            } else {
                Ok(WorkflowNodeResult::failed(
                    "git commit failed during git.stage_commit",
                    Some(commit.code().unwrap_or(1)),
                ))
            }
        }
        Some("git.integrate_plan_branch") => {
            let source_branch =
                first_non_empty_arg(&node.args, &["branch", "source_branch", "plan_branch"])
                    .or_else(|| {
                        first_non_empty_arg(&node.args, &["slug", "plan"])
                            .map(|slug| crate::plan::default_branch_for_slug(&slug))
                    })
                    .or_else(|| {
                        record
                            .metadata
                            .as_ref()
                            .and_then(|meta| meta.branch.clone())
                    })
                    .or_else(|| {
                        record
                            .metadata
                            .as_ref()
                            .and_then(|meta| meta.plan.as_ref())
                            .map(|slug| crate::plan::default_branch_for_slug(slug))
                    });
            let Some(source_branch) = source_branch else {
                return Ok(WorkflowNodeResult::failed(
                    "git.integrate_plan_branch requires a source branch",
                    Some(1),
                ));
            };
            let target_branch = first_non_empty_arg(&node.args, &["target", "target_branch"])
                .or_else(|| {
                    record
                        .metadata
                        .as_ref()
                        .and_then(|meta| meta.target.clone())
                });
            let squash = bool_arg(&node.args, "squash").unwrap_or(true);
            let delete_branch = bool_arg(&node.args, "delete_branch").unwrap_or(false);
            let slug = workflow_slug_from_record(record, node);
            let sentinel = merge_sentinel_path(project_root, &slug);

            if let Some(target) = target_branch.as_ref() {
                let current = current_branch_name(&execution_root);
                if current.as_deref() != Some(target.as_str()) {
                    let checkout = Command::new("git")
                        .arg("-C")
                        .arg(&execution_root)
                        .arg("checkout")
                        .arg(target)
                        .output()?;
                    if !checkout.status.success() {
                        let detail = String::from_utf8_lossy(&checkout.stderr).trim().to_string();
                        return Ok(WorkflowNodeResult::failed(
                            format!(
                                "git.integrate_plan_branch failed checkout `{target}`: {detail}"
                            ),
                            Some(checkout.status.code().unwrap_or(1)),
                        ));
                    }
                }
            }

            let merge_output = if squash {
                Command::new("git")
                    .arg("-C")
                    .arg(&execution_root)
                    .arg("merge")
                    .arg("--squash")
                    .arg(&source_branch)
                    .output()?
            } else {
                Command::new("git")
                    .arg("-C")
                    .arg(&execution_root)
                    .arg("merge")
                    .arg("--no-ff")
                    .arg("--no-edit")
                    .arg(&source_branch)
                    .output()?
            };
            if !merge_output.status.success() {
                let detail = String::from_utf8_lossy(&merge_output.stderr)
                    .trim()
                    .to_string();
                if has_unmerged_paths(&execution_root) {
                    if let Some(parent) = sentinel.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    let payload = serde_json::json!({
                        "slug": slug,
                        "source_branch": source_branch,
                        "target_branch": target_branch,
                        "job_id": record.id,
                        "node_id": node.node_id,
                        "created_at": Utc::now().to_rfc3339(),
                    });
                    fs::write(&sentinel, serde_json::to_vec_pretty(&payload)?)?;

                    let mut result = WorkflowNodeResult::blocked(
                        "git.integrate_plan_branch detected merge conflicts",
                        Some(10),
                    );
                    result.artifacts_written = vec![JobArtifact::MergeSentinel {
                        slug: workflow_slug_from_record(record, node),
                    }];
                    result.payload_refs = vec![relative_path(project_root, &sentinel)];
                    return Ok(result);
                }
                let summary = if detail.is_empty() {
                    "git.integrate_plan_branch failed".to_string()
                } else {
                    format!("git.integrate_plan_branch failed: {detail}")
                };
                return Ok(WorkflowNodeResult::failed(
                    summary,
                    Some(merge_output.status.code().unwrap_or(1)),
                ));
            }

            if squash {
                let diff = Command::new("git")
                    .arg("-C")
                    .arg(&execution_root)
                    .arg("diff")
                    .arg("--cached")
                    .arg("--quiet")
                    .status()?;
                if !diff.success() {
                    let message =
                        first_non_empty_arg(&node.args, &["message"]).unwrap_or_else(|| {
                            format!(
                                "feat: merge plan {}",
                                workflow_slug_from_record(record, node)
                            )
                        });
                    let commit = Command::new("git")
                        .arg("-C")
                        .arg(&execution_root)
                        .arg("commit")
                        .arg("-m")
                        .arg(&message)
                        .status()?;
                    if !commit.success() {
                        return Ok(WorkflowNodeResult::failed(
                            "git.integrate_plan_branch squash commit failed",
                            Some(commit.code().unwrap_or(1)),
                        ));
                    }
                }
            }

            let _ = remove_file_if_exists(&sentinel);
            if delete_branch
                && current_branch_name(&execution_root).as_deref() != Some(source_branch.as_str())
            {
                let _ = Command::new("git")
                    .arg("-C")
                    .arg(&execution_root)
                    .arg("branch")
                    .arg("-D")
                    .arg(&source_branch)
                    .status();
            }

            Ok(WorkflowNodeResult::succeeded(
                "git.integrate_plan_branch merged source branch",
            ))
        }
        Some("git.save_worktree_patch") => {
            let output = Command::new("git")
                .arg("-C")
                .arg(&execution_root)
                .arg("diff")
                .arg("--binary")
                .arg("HEAD")
                .output()?;
            if !output.status.success() {
                return Ok(WorkflowNodeResult::failed(
                    "git.save_worktree_patch could not produce patch",
                    Some(output.status.code().unwrap_or(1)),
                ));
            }
            let patch_path = command_patch_path(jobs_root, &record.id);
            if let Some(parent) = patch_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&patch_path, output.stdout)?;
            let mut result = WorkflowNodeResult::succeeded("saved worktree patch");
            result.artifacts_written = vec![JobArtifact::CommandPatch {
                job_id: record.id.clone(),
            }];
            result.payload_refs = vec![relative_path(project_root, &patch_path)];
            result.metadata = Some(JobMetadata {
                patch_file: Some(relative_path(project_root, &patch_path)),
                patch_index: Some(1),
                patch_total: Some(1),
                ..JobMetadata::default()
            });
            Ok(result)
        }
        Some("patch.pipeline_prepare") => {
            let files = match parse_files_json(node) {
                Ok(files) => files,
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("patch.pipeline_prepare: {err}"),
                        Some(1),
                    ));
                }
            };
            for file in &files {
                let resolved = resolve_path_in_execution_root(&execution_root, file);
                if !resolved.exists() {
                    return Ok(WorkflowNodeResult::failed(
                        format!(
                            "patch.pipeline_prepare missing patch file {}",
                            resolved.display()
                        ),
                        Some(1),
                    ));
                }
            }

            let manifest_path = patch_pipeline_manifest_path(jobs_root, &record.id);
            if let Some(parent) = manifest_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let manifest = serde_json::json!({
                "job_id": record.id,
                "node_id": node.node_id,
                "files": files,
                "prepared_at": Utc::now().to_rfc3339(),
            });
            fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;

            let total = manifest["files"]
                .as_array()
                .map(|items| items.len())
                .unwrap_or(0);
            let mut result = WorkflowNodeResult::succeeded("patch pipeline prepared");
            result.payload_refs = vec![relative_path(project_root, &manifest_path)];
            result.metadata = Some(JobMetadata {
                patch_file: Some(relative_path(project_root, &manifest_path)),
                patch_index: Some(0),
                patch_total: Some(total),
                ..JobMetadata::default()
            });
            Ok(result)
        }
        Some("patch.execute_pipeline") => {
            let files = match parse_files_json(node) {
                Ok(files) => files,
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("patch.execute_pipeline: {err}"),
                        Some(1),
                    ));
                }
            };
            for file in &files {
                let path = resolve_path_in_execution_root(&execution_root, file);
                let apply = Command::new("git")
                    .arg("-C")
                    .arg(&execution_root)
                    .arg("apply")
                    .arg("--index")
                    .arg(&path)
                    .output()?;
                if !apply.status.success() {
                    let detail = String::from_utf8_lossy(&apply.stderr).trim().to_string();
                    let summary = if detail.is_empty() {
                        format!("patch.execute_pipeline failed applying {}", path.display())
                    } else {
                        format!(
                            "patch.execute_pipeline failed applying {}: {}",
                            path.display(),
                            detail
                        )
                    };
                    return Ok(WorkflowNodeResult::failed(
                        summary,
                        Some(apply.status.code().unwrap_or(1)),
                    ));
                }
            }
            let mut result = WorkflowNodeResult::succeeded("patch pipeline executed");
            result.metadata = Some(JobMetadata {
                patch_index: Some(files.len()),
                patch_total: Some(files.len()),
                ..JobMetadata::default()
            });
            Ok(result)
        }
        Some("patch.pipeline_finalize") => {
            let output = Command::new("git")
                .arg("-C")
                .arg(&execution_root)
                .arg("diff")
                .arg("--binary")
                .arg("HEAD")
                .output()?;
            if !output.status.success() {
                return Ok(WorkflowNodeResult::failed(
                    "patch.pipeline_finalize could not capture patch",
                    Some(output.status.code().unwrap_or(1)),
                ));
            }
            let patch_path = command_patch_path(jobs_root, &record.id);
            if let Some(parent) = patch_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&patch_path, &output.stdout)?;

            let finalize_path = patch_pipeline_finalize_path(jobs_root, &record.id);
            let summary = serde_json::json!({
                "job_id": record.id,
                "node_id": node.node_id,
                "finalized_at": Utc::now().to_rfc3339(),
                "patch_path": relative_path(project_root, &patch_path),
            });
            fs::write(&finalize_path, serde_json::to_vec_pretty(&summary)?)?;

            let mut result = WorkflowNodeResult::succeeded("patch pipeline finalized");
            result.artifacts_written = vec![JobArtifact::CommandPatch {
                job_id: record.id.clone(),
            }];
            result.payload_refs = vec![
                relative_path(project_root, &patch_path),
                relative_path(project_root, &finalize_path),
            ];
            result.metadata = Some(JobMetadata {
                patch_file: Some(relative_path(project_root, &patch_path)),
                ..JobMetadata::default()
            });
            Ok(result)
        }
        Some("build.materialize_step") => {
            let build_id = first_non_empty_arg(&node.args, &["build_id"])
                .or_else(|| {
                    record
                        .metadata
                        .as_ref()
                        .and_then(|meta| meta.workflow_run_id.clone())
                })
                .unwrap_or_else(|| "workflow".to_string());
            let step_key = first_non_empty_arg(&node.args, &["step_key"])
                .unwrap_or_else(|| sanitize_workflow_component(&node.node_id));
            let slug = first_non_empty_arg(&node.args, &["slug", "plan"]);
            let branch = first_non_empty_arg(&node.args, &["branch"]);
            let target = first_non_empty_arg(&node.args, &["target", "target_branch"]);

            let step_path = execution_root
                .join(".vizier/implementation-plans/builds")
                .join(&build_id)
                .join("steps")
                .join(&step_key)
                .join("materialized.json");
            if let Some(parent) = step_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let payload = serde_json::json!({
                "build_id": build_id,
                "step_key": step_key,
                "job_id": record.id,
                "node_id": node.node_id,
                "args": node.args,
                "materialized_at": Utc::now().to_rfc3339(),
            });
            fs::write(&step_path, serde_json::to_vec_pretty(&payload)?)?;

            let mut artifacts = Vec::new();
            if let (Some(slug), Some(branch)) = (slug.as_ref(), branch.as_ref()) {
                let _ = ensure_local_branch(&execution_root, branch);
                artifacts.push(JobArtifact::PlanBranch {
                    slug: slug.clone(),
                    branch: branch.clone(),
                });
                let plan_abs = execution_root.join(crate::plan::plan_rel_path(slug));
                if !plan_abs.exists() {
                    let doc = crate::plan::render_plan_document(
                        &crate::plan::new_plan_id(),
                        slug,
                        branch,
                        "Generated by build.materialize_step",
                        "Build materialization placeholder.",
                    );
                    let _ = crate::plan::write_plan_file(&plan_abs, &doc);
                }
                if plan_abs.exists() {
                    artifacts.push(JobArtifact::PlanDoc {
                        slug: slug.clone(),
                        branch: branch.clone(),
                    });
                }
            }
            if let Some(target_branch) = target.as_ref() {
                let _ = ensure_local_branch(&execution_root, target_branch);
                artifacts.push(JobArtifact::TargetBranch {
                    name: target_branch.clone(),
                });
            }

            let mut result = WorkflowNodeResult::succeeded("build step materialized");
            result.artifacts_written = artifacts;
            result.payload_refs = vec![relative_path(project_root, &step_path)];
            result.metadata = Some(JobMetadata {
                build_pipeline: first_non_empty_arg(&node.args, &["pipeline"]),
                build_target: target,
                plan: slug,
                branch,
                ..JobMetadata::default()
            });
            Ok(result)
        }
        Some("merge.sentinel.write") => {
            let slug = workflow_slug_from_record(record, node);
            let sentinel = merge_sentinel_path(project_root, &slug);
            if let Some(parent) = sentinel.parent() {
                fs::create_dir_all(parent)?;
            }
            let payload = serde_json::json!({
                "slug": slug,
                "job_id": record.id,
                "node_id": node.node_id,
                "run_id": record.metadata.as_ref().and_then(|meta| meta.workflow_run_id.clone()),
                "source_branch": first_non_empty_arg(&node.args, &["branch", "source_branch"]).or_else(|| record.metadata.as_ref().and_then(|meta| meta.branch.clone())),
                "target_branch": first_non_empty_arg(&node.args, &["target", "target_branch"]).or_else(|| record.metadata.as_ref().and_then(|meta| meta.target.clone())),
                "written_at": Utc::now().to_rfc3339(),
            });
            fs::write(&sentinel, serde_json::to_vec_pretty(&payload)?)?;
            let mut result = WorkflowNodeResult::succeeded("merge sentinel written");
            result.artifacts_written = vec![JobArtifact::MergeSentinel {
                slug: workflow_slug_from_record(record, node),
            }];
            result.payload_refs = vec![relative_path(project_root, &sentinel)];
            Ok(result)
        }
        Some("merge.sentinel.clear") => {
            let slug = workflow_slug_from_record(record, node);
            let sentinel = merge_sentinel_path(project_root, &slug);
            remove_file_if_exists(&sentinel)?;
            if let Some(parent) = sentinel.parent()
                && parent.exists()
                && fs::read_dir(parent)?.next().is_none()
            {
                let _ = fs::remove_dir(parent);
            }
            let mut result = WorkflowNodeResult::succeeded("merge sentinel cleared");
            result.payload_refs = vec![relative_path(project_root, &sentinel)];
            Ok(result)
        }
        Some("command.run") => {
            let Some(script) = resolve_node_shell_script(node, None) else {
                return Ok(WorkflowNodeResult::failed(
                    "command.run requires args.command or args.script",
                    Some(1),
                ));
            };
            let (status, stdout, stderr) = run_shell_text_command(&execution_root, &script)?;
            if !stdout.is_empty() {
                print!("{stdout}");
                let _ = io::stdout().flush();
            }
            if !stderr.is_empty() {
                eprint!("{stderr}");
                let _ = io::stderr().flush();
            }
            if status == 0 {
                Ok(WorkflowNodeResult::succeeded("command.run succeeded"))
            } else {
                Ok(WorkflowNodeResult::failed(
                    format!("command.run failed (exit {status})"),
                    Some(status),
                ))
            }
        }
        Some("cicd.run") => {
            let default_gate_script = cicd_gate_config(node).map(|(script, _)| script);
            let Some(script) = resolve_node_shell_script(node, default_gate_script) else {
                return Ok(WorkflowNodeResult::failed(
                    "cicd.run requires args.command/args.script or a cicd gate script",
                    Some(1),
                ));
            };
            let (status, stdout, stderr) = run_shell_text_command(&execution_root, &script)?;
            if !stdout.is_empty() {
                print!("{stdout}");
                let _ = io::stdout().flush();
            }
            if !stderr.is_empty() {
                eprint!("{stderr}");
                let _ = io::stderr().flush();
            }
            if status == 0 {
                Ok(WorkflowNodeResult::succeeded("cicd.run passed"))
            } else {
                Ok(WorkflowNodeResult::failed(
                    format!("cicd.run failed (exit {status})"),
                    Some(status),
                ))
            }
        }
        Some(other) => Ok(WorkflowNodeResult::failed(
            format!("unsupported executor operation `{other}`"),
            Some(1),
        )),
        None => Ok(WorkflowNodeResult::failed(
            "missing executor operation in workflow metadata",
            Some(1),
        )),
    }
}

fn execute_workflow_control(
    project_root: &Path,
    record: &JobRecord,
    node: &WorkflowRuntimeNodeManifest,
) -> Result<WorkflowNodeResult, Box<dyn std::error::Error>> {
    let execution_root = resolve_execution_root(project_root, record)?;
    match node.control_policy.as_deref() {
        Some("terminal") => {
            let has_routes = !node.routes.succeeded.is_empty()
                || !node.routes.failed.is_empty()
                || !node.routes.blocked.is_empty()
                || !node.routes.cancelled.is_empty();
            if has_routes {
                Ok(WorkflowNodeResult::failed(
                    "terminal policy node must not declare outgoing routes",
                    Some(1),
                ))
            } else {
                Ok(WorkflowNodeResult::succeeded("terminal sink reached"))
            }
        }
        Some("gate.stop_condition") => {
            let script = first_non_empty_arg(&node.args, &["script"])
                .or_else(|| script_gate_script(node))
                .unwrap_or_default();
            if script.is_empty() {
                return Ok(WorkflowNodeResult::succeeded(
                    "stop-condition gate skipped (no script configured)",
                ));
            }

            let (status, _stdout, stderr) = run_shell_text_command(&execution_root, &script)?;
            if status == 0 {
                return Ok(WorkflowNodeResult::succeeded("stop-condition gate passed"));
            }

            let attempt = record
                .metadata
                .as_ref()
                .and_then(|meta| meta.workflow_node_attempt)
                .unwrap_or(1);
            let retry_budget = node.retry.budget.saturating_add(1);
            if matches!(node.retry.mode, WorkflowRetryMode::UntilGate) && attempt > retry_budget {
                return Ok(WorkflowNodeResult {
                    outcome: WorkflowNodeOutcome::Blocked,
                    artifacts_written: Vec::new(),
                    payload_refs: Vec::new(),
                    metadata: None,
                    summary: Some(format!(
                        "stop-condition failed on attempt {attempt}; retry budget exhausted ({})",
                        node.retry.budget
                    )),
                    exit_code: Some(10),
                });
            }

            let detail = stderr.trim();
            let summary = if detail.is_empty() {
                format!("stop-condition failed on attempt {attempt}")
            } else {
                format!("stop-condition failed on attempt {attempt}: {detail}")
            };
            Ok(WorkflowNodeResult::failed(summary, Some(status)))
        }
        Some("gate.conflict_resolution") => {
            let slug = workflow_slug_from_record(record, node);
            let sentinel = merge_sentinel_path(project_root, &slug);
            if !sentinel.exists() {
                return Ok(WorkflowNodeResult::succeeded(
                    "conflict-resolution gate skipped (no merge sentinel)",
                ));
            }

            let mut conflicts_present = has_unmerged_paths(&execution_root);
            let auto_resolve = bool_arg(&node.args, "auto_resolve")
                .or_else(|| conflict_auto_resolve_from_gate(node))
                .unwrap_or(false);
            if conflicts_present && auto_resolve {
                if let Some(script) = resolve_node_shell_script(node, None) {
                    let (status, stdout, stderr) =
                        run_shell_text_command(&execution_root, &script)?;
                    if !stdout.is_empty() {
                        print!("{stdout}");
                        let _ = io::stdout().flush();
                    }
                    if !stderr.is_empty() {
                        eprint!("{stderr}");
                        let _ = io::stderr().flush();
                    }
                    if status != 0 {
                        return Ok(WorkflowNodeResult::failed(
                            format!("conflict auto-resolve script failed (exit {status})"),
                            Some(status),
                        ));
                    }
                }
                conflicts_present = has_unmerged_paths(&execution_root);
            }

            if conflicts_present {
                let mut blocked = WorkflowNodeResult::blocked(
                    format!("merge conflicts remain for slug `{slug}`"),
                    Some(10),
                );
                blocked.artifacts_written = vec![JobArtifact::MergeSentinel { slug }];
                blocked.payload_refs = vec![relative_path(project_root, &sentinel)];
                return Ok(blocked);
            }

            remove_file_if_exists(&sentinel)?;
            Ok(WorkflowNodeResult::succeeded(
                "merge conflicts resolved and sentinel cleared",
            ))
        }
        Some("gate.cicd") => {
            let gate_cfg = cicd_gate_config(node);
            let script = resolve_node_shell_script(
                node,
                gate_cfg.as_ref().map(|(script, _)| script.clone()),
            );
            let Some(script) = script else {
                return Ok(WorkflowNodeResult::succeeded(
                    "cicd gate skipped (no script configured)",
                ));
            };
            let auto_resolve = bool_arg(&node.args, "auto_resolve")
                .or_else(|| gate_cfg.as_ref().map(|(_, auto)| *auto))
                .unwrap_or(false);
            let attempt = record
                .metadata
                .as_ref()
                .and_then(|meta| meta.workflow_node_attempt)
                .unwrap_or(1);

            let (status, stdout, stderr) = run_shell_text_command(&execution_root, &script)?;
            if !stdout.is_empty() {
                print!("{stdout}");
                let _ = io::stdout().flush();
            }
            if !stderr.is_empty() {
                eprint!("{stderr}");
                let _ = io::stderr().flush();
            }
            if status == 0 {
                return Ok(WorkflowNodeResult::succeeded(format!(
                    "cicd gate passed on attempt {attempt}"
                )));
            }

            if auto_resolve
                && let Some(fix_script) = first_non_empty_arg(
                    &node.args,
                    &["auto_resolve_command", "auto_resolve_script"],
                )
            {
                let (fix_status, fix_stdout, fix_stderr) =
                    run_shell_text_command(&execution_root, &fix_script)?;
                if !fix_stdout.is_empty() {
                    print!("{fix_stdout}");
                    let _ = io::stdout().flush();
                }
                if !fix_stderr.is_empty() {
                    eprint!("{fix_stderr}");
                    let _ = io::stderr().flush();
                }
                if fix_status == 0 {
                    let (retry_status, retry_stdout, retry_stderr) =
                        run_shell_text_command(&execution_root, &script)?;
                    if !retry_stdout.is_empty() {
                        print!("{retry_stdout}");
                        let _ = io::stdout().flush();
                    }
                    if !retry_stderr.is_empty() {
                        eprint!("{retry_stderr}");
                        let _ = io::stderr().flush();
                    }
                    if retry_status == 0 {
                        return Ok(WorkflowNodeResult::succeeded(format!(
                            "cicd gate passed after auto-resolve on attempt {attempt}"
                        )));
                    }
                }
            }

            Ok(WorkflowNodeResult::failed(
                format!("cicd gate failed on attempt {attempt} (exit {status})"),
                Some(status),
            ))
        }
        Some("gate.approval") => {
            let required = bool_arg(&node.args, "required").unwrap_or(true);
            if !required {
                return Ok(WorkflowNodeResult::succeeded(
                    "approval gate bypassed (required=false)",
                ));
            }

            let approval = record
                .schedule
                .as_ref()
                .and_then(|schedule| schedule.approval.as_ref());
            let Some(approval) = approval else {
                return Ok(WorkflowNodeResult::blocked(
                    "approval gate blocked: no approval state present",
                    Some(10),
                ));
            };
            match approval.state {
                JobApprovalState::Approved => {
                    Ok(WorkflowNodeResult::succeeded("approval gate passed"))
                }
                JobApprovalState::Pending => Ok(WorkflowNodeResult::blocked(
                    "approval gate pending human decision",
                    Some(10),
                )),
                JobApprovalState::Rejected => {
                    let reason = approval
                        .reason
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or("approval rejected");
                    Ok(WorkflowNodeResult::failed(
                        format!("approval gate rejected: {reason}"),
                        Some(10),
                    ))
                }
            }
        }
        Some(other) => Ok(WorkflowNodeResult::failed(
            format!("unsupported control policy `{other}`"),
            Some(1),
        )),
        None => Ok(WorkflowNodeResult::failed(
            "missing control policy in workflow metadata",
            Some(1),
        )),
    }
}

fn apply_workflow_routes(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    source_record: &JobRecord,
    node: &WorkflowRuntimeNodeManifest,
    run_manifest: &WorkflowRunManifest,
    outcome: WorkflowNodeOutcome,
) {
    let source_context = workflow_execution_context_from_metadata(source_record.metadata.as_ref());
    for route in node.routes.for_outcome(outcome) {
        let Some(target) = run_manifest.nodes.get(&route.node_id) else {
            display::warn(format!(
                "workflow route target `{}` missing from run manifest {}",
                route.node_id, run_manifest.run_id
            ));
            continue;
        };

        match route.mode {
            WorkflowRouteMode::PropagateContext => {
                let Some(context) = source_context.as_ref() else {
                    continue;
                };
                match apply_workflow_execution_context(jobs_root, &target.job_id, context, true) {
                    Ok(true) => display::debug(format!(
                        "workflow route {} -> {} propagated execution context",
                        node.node_id, target.node_id
                    )),
                    Ok(false) => display::debug(format!(
                        "workflow route {} -> {} skipped execution-context propagation (active or unchanged target)",
                        node.node_id, target.node_id
                    )),
                    Err(err) => display::warn(format!(
                        "workflow route {} -> {} context propagation failed: {}",
                        node.node_id, target.node_id, err
                    )),
                }
            }
            WorkflowRouteMode::RetryJob => {
                let target_record = read_record(jobs_root, &target.job_id);
                if let Ok(record) = target_record
                    && job_is_active(record.status)
                {
                    continue;
                }
                if let Err(err) = retry_job_internal(
                    project_root,
                    jobs_root,
                    binary,
                    &target.job_id,
                    source_context.as_ref(),
                ) {
                    display::warn(format!(
                        "workflow route {} -> {} retry failed: {}",
                        node.node_id, target.node_id, err
                    ));
                }
            }
        }
    }
}

fn apply_workflow_execution_context(
    jobs_root: &Path,
    job_id: &str,
    context: &WorkflowExecutionContext,
    skip_active_targets: bool,
) -> Result<bool, Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    if !paths.record_path.exists() {
        return Err(format!("no background job {}", job_id).into());
    }
    let mut record = load_record(&paths)?;
    if skip_active_targets && record.status == JobStatus::Running {
        return Ok(false);
    }

    let mut metadata = record.metadata.take().unwrap_or_default();
    let changed = metadata.execution_root != context.execution_root
        || metadata.worktree_path != context.worktree_path
        || metadata.worktree_name != context.worktree_name
        || metadata.worktree_owned != context.worktree_owned;
    if !changed {
        record.metadata = Some(metadata);
        return Ok(false);
    }

    metadata.execution_root = context.execution_root.clone();
    metadata.worktree_path = context.worktree_path.clone();
    metadata.worktree_name = context.worktree_name.clone();
    metadata.worktree_owned = context.worktree_owned;
    record.metadata = Some(metadata);
    persist_record(&paths, &record)?;
    Ok(true)
}

fn execute_workflow_node_job(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
) -> Result<i32, Box<dyn std::error::Error>> {
    let record = read_record(jobs_root, job_id)?;
    let metadata = record
        .metadata
        .as_ref()
        .ok_or_else(|| format!("workflow node job {} is missing metadata", job_id))?;
    let run_id = metadata
        .workflow_run_id
        .as_deref()
        .ok_or_else(|| format!("workflow node job {} missing workflow_run_id", job_id))?;
    let node_id = metadata
        .workflow_node_id
        .as_deref()
        .ok_or_else(|| format!("workflow node job {} missing workflow_node_id", job_id))?;
    let manifest = load_workflow_run_manifest(project_root, run_id)?;
    let node_manifest = manifest
        .nodes
        .get(node_id)
        .ok_or_else(|| format!("workflow run {} missing node {}", run_id, node_id))?;

    set_current_job_id(Some(job_id.to_string()));
    let result = match (
        node_manifest.executor_operation.as_deref(),
        node_manifest.control_policy.as_deref(),
    ) {
        (Some(_), _) => execute_workflow_executor(project_root, jobs_root, &record, node_manifest),
        (None, Some(_)) => execute_workflow_control(project_root, &record, node_manifest),
        _ => Ok(WorkflowNodeResult::failed(
            format!("workflow node {} has no runtime operation/policy", node_id),
            Some(1),
        )),
    };
    set_current_job_id(None);
    let result = result?;

    let mut artifacts_written = node_manifest
        .artifacts_by_outcome
        .for_outcome(result.outcome)
        .to_vec();
    artifacts_written.extend(result.artifacts_written.clone());
    artifacts_written = dedup_job_artifacts(artifacts_written);

    let (status, exit_code) = map_workflow_outcome_to_job_status(result.outcome, result.exit_code);
    let metadata_update = JobMetadata {
        workflow_node_outcome: Some(result.outcome.as_str().to_string()),
        workflow_payload_refs: if result.payload_refs.is_empty() {
            None
        } else {
            Some(result.payload_refs.clone())
        },
        ..JobMetadata::default()
    };
    let metadata_update = merge_metadata(Some(metadata_update), result.metadata.clone());
    let finalized_record = finalize_job_with_artifacts(
        project_root,
        jobs_root,
        job_id,
        status,
        exit_code,
        None,
        metadata_update,
        Some(&artifacts_written),
    )?;

    let binary = std::env::current_exe()?;
    apply_workflow_routes(
        project_root,
        jobs_root,
        &binary,
        &finalized_record,
        node_manifest,
        &manifest,
        result.outcome,
    );
    let _ = scheduler_tick(project_root, jobs_root, &binary)?;
    Ok(exit_code)
}

pub fn run_workflow_node_command(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    match execute_workflow_node_job(project_root, jobs_root, job_id) {
        Ok(code) => {
            if code == 0 {
                Ok(())
            } else {
                Err(format!("workflow node {job_id} failed (exit {code})").into())
            }
        }
        Err(err) => {
            if let Err(finalize_err) =
                finalize_failed_workflow_node_if_active(project_root, jobs_root, job_id)
            {
                display::warn(format!(
                    "unable to finalize failed workflow node job {} after runtime error: {}",
                    job_id, finalize_err
                ));
            }
            Err(err)
        }
    }
}

fn finalize_failed_workflow_node_if_active(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let record = read_record(jobs_root, job_id)?;
    if !job_is_active(record.status) {
        return Ok(());
    }

    let metadata = JobMetadata {
        workflow_node_outcome: Some(WorkflowNodeOutcome::Failed.as_str().to_string()),
        ..JobMetadata::default()
    };
    let _ = finalize_job(
        project_root,
        jobs_root,
        job_id,
        JobStatus::Failed,
        1,
        None,
        Some(metadata),
    )?;
    Ok(())
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
    retry_job_internal(project_root, jobs_root, binary, requested_job_id, None)
}

fn retry_job_internal(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    requested_job_id: &str,
    propagated_context: Option<&WorkflowExecutionContext>,
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

    if let Some(context) = propagated_context {
        let _ = apply_workflow_execution_context(jobs_root, requested_job_id, context, true)?;
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
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::{Arc, Barrier};
    use tempfile::TempDir;
    use vizier_core::workflow_template::{
        WorkflowArtifactContract, WorkflowNode, WorkflowNodeKind, WorkflowOutcomeArtifacts,
        WorkflowOutcomeEdges, WorkflowRetryMode, WorkflowTemplate, WorkflowTemplatePolicy,
    };

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

    fn prompt_invoke_template() -> WorkflowTemplate {
        WorkflowTemplate {
            id: "template.runtime.prompt_invoke".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![WorkflowArtifactContract {
                id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
                version: "v1".to_string(),
                schema: None,
            }],
            nodes: vec![
                WorkflowNode {
                    id: "resolve_prompt".to_string(),
                    kind: WorkflowNodeKind::Builtin,
                    uses: "cap.env.builtin.prompt.resolve".to_string(),
                    args: BTreeMap::from([("prompt_text".to_string(), "hello world".to_string())]),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts {
                        succeeded: vec![JobArtifact::Custom {
                            type_id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
                            key: "approve_prompt".to_string(),
                        }],
                        ..WorkflowOutcomeArtifacts::default()
                    },
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: Vec::new(),
                    retry: Default::default(),
                    on: WorkflowOutcomeEdges {
                        succeeded: vec!["invoke_agent".to_string()],
                        ..WorkflowOutcomeEdges::default()
                    },
                },
                WorkflowNode {
                    id: "invoke_agent".to_string(),
                    kind: WorkflowNodeKind::Agent,
                    uses: "cap.agent.invoke".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: vec![JobArtifact::Custom {
                        type_id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
                        key: "approve_prompt".to_string(),
                    }],
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: Vec::new(),
                    retry: Default::default(),
                    on: WorkflowOutcomeEdges::default(),
                },
            ],
        }
    }

    fn runtime_executor_node(
        node_id: &str,
        job_id: &str,
        uses: &str,
        operation: &str,
        args: BTreeMap<String, String>,
    ) -> WorkflowRuntimeNodeManifest {
        WorkflowRuntimeNodeManifest {
            node_id: node_id.to_string(),
            job_id: job_id.to_string(),
            uses: uses.to_string(),
            kind: WorkflowNodeKind::Builtin,
            args,
            executor_operation: Some(operation.to_string()),
            control_policy: None,
            gates: Vec::new(),
            retry: vizier_core::workflow_template::WorkflowRetryPolicy::default(),
            routes: WorkflowRouteTargets::default(),
            artifacts_by_outcome: WorkflowOutcomeArtifactsByOutcome::default(),
        }
    }

    fn runtime_control_node(
        node_id: &str,
        job_id: &str,
        uses: &str,
        policy: &str,
        args: BTreeMap<String, String>,
    ) -> WorkflowRuntimeNodeManifest {
        WorkflowRuntimeNodeManifest {
            node_id: node_id.to_string(),
            job_id: job_id.to_string(),
            uses: uses.to_string(),
            kind: WorkflowNodeKind::Gate,
            args,
            executor_operation: None,
            control_policy: Some(policy.to_string()),
            gates: Vec::new(),
            retry: vizier_core::workflow_template::WorkflowRetryPolicy::default(),
            routes: WorkflowRouteTargets::default(),
            artifacts_by_outcome: WorkflowOutcomeArtifactsByOutcome::default(),
        }
    }

    fn git_status(project_root: &Path, args: &[&str]) -> std::process::ExitStatus {
        Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .status()
            .expect("run git")
    }

    fn git_output(project_root: &Path, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .expect("run git output")
    }

    fn git_commit_all(project_root: &Path, message: &str) {
        let add = git_status(project_root, &["add", "-A"]);
        assert!(add.success(), "git add failed: {add:?}");
        let commit = git_status(
            project_root,
            &[
                "-c",
                "user.name=vizier",
                "-c",
                "user.email=vizier@example.com",
                "commit",
                "-m",
                message,
            ],
        );
        assert!(commit.success(), "git commit failed: {commit:?}");
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
        let custom_payload = write_custom_artifact_payload(
            project_root,
            "job-retry",
            "acme.execution",
            "retry-node",
            &serde_json::json!({"text": "payload"}),
        )
        .expect("write custom payload");

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
                execution_root: Some(".vizier/tmp-worktrees/retry-cleanup".to_string()),
                worktree_owned: Some(true),
                worktree_path: Some(".vizier/tmp-worktrees/retry-cleanup".to_string()),
                workflow_node_attempt: Some(4),
                workflow_node_outcome: Some("failed".to_string()),
                workflow_payload_refs: Some(vec!["payload.json".to_string()]),
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
        assert_eq!(metadata.execution_root.as_deref(), Some("."));
        assert_eq!(metadata.workflow_node_attempt, Some(5));
        assert!(metadata.workflow_node_outcome.is_none());
        assert!(metadata.workflow_payload_refs.is_none());
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
            !custom_payload.exists(),
            "expected custom artifact payload to be removed during rewind"
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
                execution_root: Some(worktree_rel.to_string()),
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
        assert_eq!(metadata.execution_root.as_deref(), Some(worktree_rel));
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
                execution_root: Some(worktree_rel.to_string()),
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
        assert_eq!(metadata.execution_root.as_deref(), Some("."));
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
    fn retry_job_internal_applies_propagated_execution_context_before_scheduler_tick() {
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
            Some(JobMetadata {
                execution_root: Some(".".to_string()),
                ..JobMetadata::default()
            }),
            None,
            Some(JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: JobArtifact::TargetBranch {
                        name: "missing-retry-target".to_string(),
                    },
                }],
                ..JobSchedule::default()
            }),
        )
        .expect("enqueue root");
        update_job_record(&jobs_root, "job-root", |record| {
            record.status = JobStatus::Failed;
            record.exit_code = Some(1);
        })
        .expect("mark root failed");

        let propagated = WorkflowExecutionContext {
            execution_root: Some(".vizier/tmp-worktrees/propagated".to_string()),
            worktree_path: Some(".vizier/tmp-worktrees/propagated".to_string()),
            worktree_name: Some("propagated".to_string()),
            worktree_owned: Some(true),
        };
        let binary = std::env::current_exe().expect("current exe");
        retry_job_internal(
            project_root,
            &jobs_root,
            &binary,
            "job-root",
            Some(&propagated),
        )
        .expect("retry with propagated context");

        let root = read_record(&jobs_root, "job-root").expect("root record");
        let metadata = root.metadata.as_ref().expect("root metadata");
        assert_eq!(metadata.execution_root, propagated.execution_root);
        assert_eq!(metadata.worktree_path, propagated.worktree_path);
        assert_eq!(metadata.worktree_name, propagated.worktree_name);
        assert_eq!(metadata.worktree_owned, propagated.worktree_owned);
    }

    #[test]
    fn enqueue_workflow_run_materializes_runtime_node_jobs() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        let template = prompt_invoke_template();
        let result = enqueue_workflow_run(
            project_root,
            &jobs_root,
            "run-runtime",
            "template.runtime.prompt_invoke@v1",
            &template,
            &[
                "vizier".to_string(),
                "jobs".to_string(),
                "schedule".to_string(),
            ],
            None,
        )
        .expect("enqueue workflow run");

        assert_eq!(result.job_ids.len(), 2);
        let resolve_job = result
            .job_ids
            .get("resolve_prompt")
            .expect("resolve job id")
            .clone();
        let invoke_job = result
            .job_ids
            .get("invoke_agent")
            .expect("invoke job id")
            .clone();

        let resolve_record = read_record(&jobs_root, &resolve_job).expect("resolve record");
        assert_eq!(
            resolve_record.child_args,
            vec![
                "__workflow-node".to_string(),
                "--job-id".to_string(),
                resolve_job.clone()
            ]
        );
        let resolve_meta = resolve_record.metadata.as_ref().expect("resolve metadata");
        assert_eq!(resolve_meta.workflow_run_id.as_deref(), Some("run-runtime"));
        assert_eq!(resolve_meta.workflow_node_attempt, Some(1));
        assert_eq!(
            resolve_meta.workflow_executor_operation.as_deref(),
            Some("prompt.resolve")
        );

        let invoke_record = read_record(&jobs_root, &invoke_job).expect("invoke record");
        let after = invoke_record
            .schedule
            .as_ref()
            .map(|schedule| schedule.after.clone())
            .unwrap_or_default();
        assert!(
            after
                .iter()
                .any(|dependency| dependency.job_id == resolve_job),
            "expected invoke node to depend on resolve node via on.succeeded routing"
        );

        let manifest =
            load_workflow_run_manifest(project_root, "run-runtime").expect("workflow run manifest");
        assert_eq!(manifest.nodes.len(), 2);
        assert!(manifest.nodes.contains_key("resolve_prompt"));
        assert!(manifest.nodes.contains_key("invoke_agent"));
        let resolve_manifest = manifest
            .nodes
            .get("resolve_prompt")
            .expect("resolve manifest");
        assert!(
            resolve_manifest.routes.succeeded.iter().any(|target| {
                target.node_id == "invoke_agent"
                    && matches!(target.mode, WorkflowRouteMode::PropagateContext)
            }),
            "expected success edge to materialize as context-propagation route"
        );
    }

    #[test]
    fn workflow_runtime_prompt_payload_roundtrip() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        let template = prompt_invoke_template();
        let result = enqueue_workflow_run(
            project_root,
            &jobs_root,
            "run-prompt",
            "template.runtime.prompt_invoke@v1",
            &template,
            &[
                "vizier".to_string(),
                "jobs".to_string(),
                "schedule".to_string(),
            ],
            None,
        )
        .expect("enqueue workflow run");
        let manifest =
            load_workflow_run_manifest(project_root, "run-prompt").expect("workflow manifest");

        let resolve_job = result
            .job_ids
            .get("resolve_prompt")
            .expect("resolve job id")
            .clone();
        let resolve_record = read_record(&jobs_root, &resolve_job).expect("resolve record");
        let resolve_node = manifest
            .nodes
            .get("resolve_prompt")
            .expect("resolve node manifest");
        let resolve_result =
            execute_workflow_executor(project_root, &jobs_root, &resolve_record, resolve_node)
                .expect("execute prompt.resolve");
        assert_eq!(resolve_result.outcome, WorkflowNodeOutcome::Succeeded);
        assert_eq!(resolve_result.payload_refs.len(), 1);
        let payload_ref = project_root.join(resolve_result.payload_refs[0].as_str());
        assert!(payload_ref.exists(), "expected payload file to exist");
        let (status, exit_code) =
            map_workflow_outcome_to_job_status(resolve_result.outcome, resolve_result.exit_code);
        let _ = finalize_job_with_artifacts(
            project_root,
            &jobs_root,
            &resolve_job,
            status,
            exit_code,
            None,
            Some(JobMetadata::default()),
            Some(&resolve_result.artifacts_written),
        )
        .expect("finalize resolve");

        let invoke_job = result
            .job_ids
            .get("invoke_agent")
            .expect("invoke job id")
            .clone();
        let invoke_record = read_record(&jobs_root, &invoke_job).expect("invoke record");
        let invoke_node = manifest
            .nodes
            .get("invoke_agent")
            .expect("invoke node manifest");
        let invoke_result =
            execute_workflow_executor(project_root, &jobs_root, &invoke_record, invoke_node)
                .expect("execute agent.invoke");
        assert_eq!(invoke_result.outcome, WorkflowNodeOutcome::Succeeded);
        assert_eq!(invoke_result.payload_refs.len(), 1);
    }

    #[test]
    fn run_workflow_node_command_finalizes_job_when_executor_errors() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        let template = WorkflowTemplate {
            id: "template.runtime.prompt_missing_file".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![WorkflowArtifactContract {
                id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
                version: "v1".to_string(),
                schema: None,
            }],
            nodes: vec![WorkflowNode {
                id: "resolve_prompt".to_string(),
                kind: WorkflowNodeKind::Builtin,
                uses: "cap.env.builtin.prompt.resolve".to_string(),
                args: BTreeMap::from([(
                    "prompt_file".to_string(),
                    "__missing_prompt__.md".to_string(),
                )]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts {
                    succeeded: vec![JobArtifact::Custom {
                        type_id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
                        key: "draft_main".to_string(),
                    }],
                    ..WorkflowOutcomeArtifacts::default()
                },
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges::default(),
            }],
        };

        let enqueue = enqueue_workflow_run(
            project_root,
            &jobs_root,
            "run-runtime-error",
            "template.runtime.prompt_missing_file@v1",
            &template,
            &["vizier".to_string(), "__workflow-node".to_string()],
            None,
        )
        .expect("enqueue workflow run");
        let resolve_job = enqueue
            .job_ids
            .get("resolve_prompt")
            .expect("resolve job id")
            .clone();

        update_job_record(&jobs_root, &resolve_job, |record| {
            record.status = JobStatus::Running;
            record.started_at = Some(Utc::now());
            record.pid = Some(std::process::id());
        })
        .expect("mark running");

        let err = run_workflow_node_command(project_root, &jobs_root, &resolve_job)
            .expect_err("missing prompt file should fail workflow node");
        let err_text = err.to_string();
        assert!(
            err_text.contains("No such file or directory")
                || err_text.contains("__missing_prompt__.md")
                || err_text.contains("prompt.resolve"),
            "expected executor error details: {err_text}"
        );

        let record = read_record(&jobs_root, &resolve_job).expect("resolve record");
        assert_eq!(record.status, JobStatus::Failed);
        assert_eq!(record.exit_code, Some(1));
        assert!(record.finished_at.is_some());
        let metadata = record.metadata.as_ref().expect("metadata");
        assert_eq!(metadata.workflow_node_outcome.as_deref(), Some("failed"));
    }

    #[test]
    fn workflow_runtime_worktree_prepare_and_cleanup_manage_owned_paths() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-worktree-runtime",
            &["--help".to_string()],
            &["vizier".to_string(), "__workflow-node".to_string()],
            None,
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue");
        let record = read_record(&jobs_root, "job-worktree-runtime").expect("record");

        let prepare = runtime_executor_node(
            "prepare",
            "job-worktree-runtime",
            "cap.env.builtin.worktree.prepare",
            "worktree.prepare",
            BTreeMap::from([("branch".to_string(), "draft/worktree-runtime".to_string())]),
        );
        let prepare_result = execute_workflow_executor(project_root, &jobs_root, &record, &prepare)
            .expect("prepare");
        assert_eq!(prepare_result.outcome, WorkflowNodeOutcome::Succeeded);
        let prepare_meta = prepare_result.metadata.clone().expect("worktree metadata");
        assert_eq!(
            prepare_meta.execution_root.as_deref(),
            prepare_meta.worktree_path.as_deref()
        );
        let worktree_rel = prepare_meta
            .worktree_path
            .as_deref()
            .expect("worktree path metadata");
        let worktree_abs = resolve_recorded_path(project_root, worktree_rel);
        assert!(worktree_abs.exists(), "expected worktree path to exist");

        let mut cleanup_record = record.clone();
        cleanup_record.metadata = Some(prepare_meta);
        let cleanup = runtime_executor_node(
            "cleanup",
            "job-worktree-runtime",
            "cap.env.builtin.worktree.cleanup",
            "worktree.cleanup",
            BTreeMap::new(),
        );
        let cleanup_result =
            execute_workflow_executor(project_root, &jobs_root, &cleanup_record, &cleanup)
                .expect("cleanup");
        assert_eq!(cleanup_result.outcome, WorkflowNodeOutcome::Succeeded);
        let cleanup_meta = cleanup_result.metadata.clone().expect("cleanup metadata");
        assert_eq!(cleanup_meta.execution_root.as_deref(), Some("."));
        assert_eq!(cleanup_meta.worktree_owned, Some(false));
        assert!(
            !worktree_abs.exists(),
            "expected owned worktree directory to be removed"
        );
    }

    #[test]
    fn workflow_runtime_worktree_prepare_derives_branch_from_slug_when_branch_missing() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-worktree-slug-derived",
            &["--help".to_string()],
            &["vizier".to_string(), "__workflow-node".to_string()],
            None,
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue");
        let record = read_record(&jobs_root, "job-worktree-slug-derived").expect("record");

        let prepare = runtime_executor_node(
            "prepare",
            "job-worktree-slug-derived",
            "cap.env.builtin.worktree.prepare",
            "worktree.prepare",
            BTreeMap::from([("slug".to_string(), "worktree-slug-derived".to_string())]),
        );
        let prepare_result = execute_workflow_executor(project_root, &jobs_root, &record, &prepare)
            .expect("prepare");
        assert_eq!(prepare_result.outcome, WorkflowNodeOutcome::Succeeded);
        let prepare_meta = prepare_result.metadata.clone().expect("worktree metadata");
        assert_eq!(
            prepare_meta.branch.as_deref(),
            Some("draft/worktree-slug-derived")
        );

        let worktree_rel = prepare_meta
            .worktree_path
            .as_deref()
            .expect("worktree path metadata");
        let worktree_abs = resolve_recorded_path(project_root, worktree_rel);
        assert!(worktree_abs.exists(), "expected worktree path to exist");

        let mut cleanup_record = record.clone();
        cleanup_record.metadata = Some(prepare_meta);
        let cleanup = runtime_executor_node(
            "cleanup",
            "job-worktree-slug-derived",
            "cap.env.builtin.worktree.cleanup",
            "worktree.cleanup",
            BTreeMap::new(),
        );
        let cleanup_result =
            execute_workflow_executor(project_root, &jobs_root, &cleanup_record, &cleanup)
                .expect("cleanup");
        assert_eq!(cleanup_result.outcome, WorkflowNodeOutcome::Succeeded);
        assert!(
            !worktree_abs.exists(),
            "expected owned worktree directory to be removed"
        );
    }

    #[test]
    fn workflow_runtime_worktree_prepare_fails_without_branch_or_slug() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-worktree-no-branch",
            &["--help".to_string()],
            &["vizier".to_string(), "__workflow-node".to_string()],
            None,
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue");
        let record = read_record(&jobs_root, "job-worktree-no-branch").expect("record");

        let prepare = runtime_executor_node(
            "prepare",
            "job-worktree-no-branch",
            "cap.env.builtin.worktree.prepare",
            "worktree.prepare",
            BTreeMap::new(),
        );
        let prepare_result = execute_workflow_executor(project_root, &jobs_root, &record, &prepare)
            .expect("prepare");
        assert_eq!(prepare_result.outcome, WorkflowNodeOutcome::Failed);
        assert_eq!(
            prepare_result.summary.as_deref(),
            Some("worktree.prepare could not determine branch (set branch or slug/plan)")
        );
    }

    #[test]
    fn resolve_execution_root_prefers_execution_root_and_validates_repo_bounds() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let canonical_root = project_root.canonicalize().expect("canonical repo root");

        let worktree_rel = ".vizier/tmp-worktrees/root-precedence";
        let worktree_abs = project_root.join(worktree_rel);
        fs::create_dir_all(&worktree_abs).expect("create worktree path");
        let canonical_worktree = worktree_abs
            .canonicalize()
            .expect("canonical worktree root");

        let mut record = JobRecord {
            id: "job-root-precedence".to_string(),
            status: JobStatus::Queued,
            command: vec!["vizier".to_string(), "__workflow-node".to_string()],
            child_args: Vec::new(),
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            pid: None,
            exit_code: None,
            stdout_path: ".vizier/jobs/job-root-precedence/stdout.log".to_string(),
            stderr_path: ".vizier/jobs/job-root-precedence/stderr.log".to_string(),
            session_path: None,
            outcome_path: None,
            metadata: Some(JobMetadata {
                execution_root: Some(".".to_string()),
                worktree_path: Some(worktree_rel.to_string()),
                ..JobMetadata::default()
            }),
            config_snapshot: None,
            schedule: None,
        };

        let resolved =
            resolve_execution_root(project_root, &record).expect("resolve from explicit root");
        assert_eq!(resolved, canonical_root);

        if let Some(metadata) = record.metadata.as_mut() {
            metadata.execution_root = Some(worktree_rel.to_string());
            metadata.worktree_path = Some(".".to_string());
        }
        let resolved =
            resolve_execution_root(project_root, &record).expect("resolve from execution_root");
        assert_eq!(resolved, canonical_worktree);

        if let Some(metadata) = record.metadata.as_mut() {
            metadata.execution_root = None;
            metadata.worktree_path = Some(worktree_rel.to_string());
        }
        let resolved =
            resolve_execution_root(project_root, &record).expect("resolve from worktree_path");
        assert_eq!(resolved, canonical_worktree);

        if let Some(metadata) = record.metadata.as_mut() {
            metadata.execution_root = Some("..".to_string());
            metadata.worktree_path = Some(worktree_rel.to_string());
        }
        let err = resolve_execution_root(project_root, &record)
            .expect_err("expected repo-boundary rejection");
        assert!(
            err.to_string().contains("outside repository root"),
            "expected out-of-repo rejection, got: {err}"
        );

        if let Some(metadata) = record.metadata.as_mut() {
            metadata.execution_root = Some("missing-root".to_string());
            metadata.worktree_path = Some(worktree_rel.to_string());
        }
        let err = resolve_execution_root(project_root, &record)
            .expect_err("expected missing-root failure");
        assert!(
            err.to_string().contains("metadata.execution_root"),
            "expected explicit field validation error, got: {err}"
        );
    }

    #[test]
    fn merge_metadata_clears_worktree_fields_when_cleanup_resets_root() {
        let existing = Some(JobMetadata {
            execution_root: Some(".vizier/tmp-worktrees/workflow".to_string()),
            worktree_name: Some("workflow-node".to_string()),
            worktree_path: Some(".vizier/tmp-worktrees/workflow".to_string()),
            worktree_owned: Some(true),
            ..JobMetadata::default()
        });
        let update = Some(JobMetadata {
            execution_root: Some(".".to_string()),
            worktree_owned: Some(false),
            retry_cleanup_status: Some(RetryCleanupStatus::Done),
            ..JobMetadata::default()
        });
        let merged = merge_metadata(existing, update).expect("merged metadata");
        assert_eq!(merged.execution_root.as_deref(), Some("."));
        assert!(merged.worktree_name.is_none());
        assert!(merged.worktree_path.is_none());
        assert!(merged.worktree_owned.is_none());
        assert_eq!(merged.retry_cleanup_status, Some(RetryCleanupStatus::Done));
    }

    #[test]
    fn apply_workflow_execution_context_is_idempotent_and_skips_active_targets() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-target",
            &["--help".to_string()],
            &["vizier".to_string(), "__workflow-node".to_string()],
            None,
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue target");
        let context = WorkflowExecutionContext {
            execution_root: Some(".vizier/tmp-worktrees/ctx-a".to_string()),
            worktree_path: Some(".vizier/tmp-worktrees/ctx-a".to_string()),
            worktree_name: Some("ctx-a".to_string()),
            worktree_owned: Some(true),
        };

        let first = apply_workflow_execution_context(&jobs_root, "job-target", &context, true)
            .expect("first propagation");
        assert!(first, "expected first propagation to update metadata");
        let second = apply_workflow_execution_context(&jobs_root, "job-target", &context, true)
            .expect("second propagation");
        assert!(!second, "expected unchanged propagation to be idempotent");

        update_job_record(&jobs_root, "job-target", |record| {
            record.status = JobStatus::Running;
        })
        .expect("mark target running");
        let changed_context = WorkflowExecutionContext {
            execution_root: Some(".vizier/tmp-worktrees/ctx-b".to_string()),
            worktree_path: Some(".vizier/tmp-worktrees/ctx-b".to_string()),
            worktree_name: Some("ctx-b".to_string()),
            worktree_owned: Some(true),
        };
        let active =
            apply_workflow_execution_context(&jobs_root, "job-target", &changed_context, true)
                .expect("active target propagation");
        assert!(
            !active,
            "expected propagation to skip active target metadata"
        );

        let target = read_record(&jobs_root, "job-target").expect("target record");
        let metadata = target.metadata.expect("target metadata");
        assert_eq!(metadata.execution_root, context.execution_root);
        assert_eq!(metadata.worktree_path, context.worktree_path);
        assert_eq!(metadata.worktree_name, context.worktree_name);
        assert_eq!(metadata.worktree_owned, context.worktree_owned);
    }

    #[test]
    fn workflow_runtime_command_run_uses_worktree_then_repo_after_cleanup_reset() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-exec-root",
            &["--help".to_string()],
            &["vizier".to_string(), "__workflow-node".to_string()],
            None,
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue");
        let record = read_record(&jobs_root, "job-exec-root").expect("record");

        let prepare = runtime_executor_node(
            "prepare",
            "job-exec-root",
            "cap.env.builtin.worktree.prepare",
            "worktree.prepare",
            BTreeMap::from([(
                "branch".to_string(),
                "draft/execution-root-runtime".to_string(),
            )]),
        );
        let prepare_result = execute_workflow_executor(project_root, &jobs_root, &record, &prepare)
            .expect("prepare");
        assert_eq!(prepare_result.outcome, WorkflowNodeOutcome::Succeeded);
        let prepare_meta = prepare_result.metadata.clone().expect("prepare metadata");
        let worktree_rel = prepare_meta
            .worktree_path
            .as_deref()
            .expect("worktree path metadata");
        let worktree_abs = resolve_recorded_path(project_root, worktree_rel);

        let mut in_worktree_record = record.clone();
        in_worktree_record.metadata = Some(prepare_meta.clone());
        let in_worktree = runtime_executor_node(
            "in-worktree",
            "job-exec-root",
            "cap.env.shell.command.run",
            "command.run",
            BTreeMap::from([(
                "script".to_string(),
                "echo from-worktree > marker-in-worktree.txt".to_string(),
            )]),
        );
        let in_worktree_result =
            execute_workflow_executor(project_root, &jobs_root, &in_worktree_record, &in_worktree)
                .expect("in-worktree command");
        assert_eq!(in_worktree_result.outcome, WorkflowNodeOutcome::Succeeded);
        assert!(
            worktree_abs.join("marker-in-worktree.txt").exists(),
            "expected marker in propagated worktree root"
        );
        assert!(
            !project_root.join("marker-in-worktree.txt").exists(),
            "worktree command should not write marker in repository root"
        );

        let cleanup = runtime_executor_node(
            "cleanup",
            "job-exec-root",
            "cap.env.builtin.worktree.cleanup",
            "worktree.cleanup",
            BTreeMap::new(),
        );
        let cleanup_result =
            execute_workflow_executor(project_root, &jobs_root, &in_worktree_record, &cleanup)
                .expect("cleanup");
        assert_eq!(cleanup_result.outcome, WorkflowNodeOutcome::Succeeded);
        let merged_meta = merge_metadata(Some(prepare_meta), cleanup_result.metadata.clone())
            .expect("merged cleanup metadata");
        assert_eq!(merged_meta.execution_root.as_deref(), Some("."));

        let mut repo_root_record = record.clone();
        repo_root_record.metadata = Some(merged_meta);
        let in_repo = runtime_executor_node(
            "in-repo",
            "job-exec-root",
            "cap.env.shell.command.run",
            "command.run",
            BTreeMap::from([(
                "script".to_string(),
                "echo from-repo > marker-in-repo.txt".to_string(),
            )]),
        );
        let in_repo_result =
            execute_workflow_executor(project_root, &jobs_root, &repo_root_record, &in_repo)
                .expect("repo command");
        assert_eq!(in_repo_result.outcome, WorkflowNodeOutcome::Succeeded);
        assert!(
            project_root.join("marker-in-repo.txt").exists(),
            "expected marker in repository root after cleanup reset"
        );
    }

    #[test]
    fn workflow_runtime_plan_persist_writes_plan_doc_and_state() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-plan-persist",
            &["--help".to_string()],
            &["vizier".to_string(), "__workflow-node".to_string()],
            None,
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue");
        let record = read_record(&jobs_root, "job-plan-persist").expect("record");
        let node = runtime_executor_node(
            "persist",
            "job-plan-persist",
            "cap.env.builtin.plan.persist",
            "plan.persist",
            BTreeMap::from([
                ("name_override".to_string(), "runtime-plan".to_string()),
                ("spec_source".to_string(), "inline".to_string()),
                (
                    "spec_text".to_string(),
                    "Runtime operation completion spec".to_string(),
                ),
            ]),
        );
        let result =
            execute_workflow_executor(project_root, &jobs_root, &record, &node).expect("persist");
        assert_eq!(result.outcome, WorkflowNodeOutcome::Succeeded);
        assert!(
            result
                .artifacts_written
                .iter()
                .any(|artifact| matches!(artifact, JobArtifact::PlanBranch { .. })),
            "expected plan branch artifact"
        );
        assert!(
            result
                .artifacts_written
                .iter()
                .any(|artifact| matches!(artifact, JobArtifact::PlanDoc { .. })),
            "expected plan doc artifact"
        );
        let plan_doc = project_root.join(".vizier/implementation-plans/runtime-plan.md");
        assert!(plan_doc.exists(), "expected persisted plan doc");
        let state_ref = result
            .payload_refs
            .iter()
            .find(|entry| entry.contains(".vizier/state/plans/"))
            .cloned()
            .expect("plan state payload ref");
        assert!(
            project_root.join(state_ref).exists(),
            "expected persisted plan state"
        );
    }

    #[test]
    fn workflow_runtime_integrate_plan_branch_blocks_on_conflict_and_writes_sentinel() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");
        let target = current_branch_name(project_root).expect("target branch");

        fs::write(project_root.join("conflict.txt"), "base\n").expect("write base");
        git_commit_all(project_root, "base conflict");

        let checkout = git_status(project_root, &["checkout", "-b", "draft/runtime-conflict"]);
        assert!(checkout.success(), "create draft branch");
        fs::write(project_root.join("conflict.txt"), "draft\n").expect("write draft");
        git_commit_all(project_root, "draft conflict");

        let checkout_target = git_status(project_root, &["checkout", &target]);
        assert!(checkout_target.success(), "checkout target");
        fs::write(project_root.join("conflict.txt"), "target\n").expect("write target");
        git_commit_all(project_root, "target conflict");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-integrate-conflict",
            &["--help".to_string()],
            &["vizier".to_string(), "__workflow-node".to_string()],
            Some(JobMetadata {
                plan: Some("runtime-conflict".to_string()),
                branch: Some("draft/runtime-conflict".to_string()),
                target: Some(target.clone()),
                ..JobMetadata::default()
            }),
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue");
        let record = read_record(&jobs_root, "job-integrate-conflict").expect("record");
        let node = runtime_executor_node(
            "integrate",
            "job-integrate-conflict",
            "cap.env.builtin.git.integrate_plan_branch",
            "git.integrate_plan_branch",
            BTreeMap::from([
                ("branch".to_string(), "draft/runtime-conflict".to_string()),
                ("target_branch".to_string(), target),
                ("squash".to_string(), "false".to_string()),
            ]),
        );
        let result =
            execute_workflow_executor(project_root, &jobs_root, &record, &node).expect("integrate");
        assert_eq!(result.outcome, WorkflowNodeOutcome::Blocked);
        assert!(
            result
                .artifacts_written
                .iter()
                .any(|artifact| matches!(artifact, JobArtifact::MergeSentinel { .. })),
            "expected merge sentinel artifact"
        );
        let sentinel = project_root.join(".vizier/tmp/merge-conflicts/runtime-conflict.json");
        assert!(sentinel.exists(), "expected merge sentinel file");
    }

    #[test]
    fn workflow_runtime_integrate_plan_branch_derives_branch_from_slug_when_branch_missing() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");
        let target = current_branch_name(project_root).expect("target branch");

        let checkout = git_status(
            project_root,
            &["checkout", "-b", "draft/runtime-slug-merge"],
        );
        assert!(checkout.success(), "create draft branch");
        fs::write(project_root.join("slug-merge.txt"), "from slug source\n")
            .expect("write source file");
        git_commit_all(project_root, "feat: slug merge source");
        let checkout_target = git_status(project_root, &["checkout", &target]);
        assert!(checkout_target.success(), "checkout target");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-integrate-slug-derived",
            &["--help".to_string()],
            &["vizier".to_string(), "__workflow-node".to_string()],
            None,
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue");
        let record = read_record(&jobs_root, "job-integrate-slug-derived").expect("record");
        let node = runtime_executor_node(
            "integrate",
            "job-integrate-slug-derived",
            "cap.env.builtin.git.integrate_plan_branch",
            "git.integrate_plan_branch",
            BTreeMap::from([
                ("slug".to_string(), "runtime-slug-merge".to_string()),
                ("target_branch".to_string(), target),
                ("squash".to_string(), "false".to_string()),
            ]),
        );
        let result =
            execute_workflow_executor(project_root, &jobs_root, &record, &node).expect("integrate");
        assert_eq!(result.outcome, WorkflowNodeOutcome::Succeeded);
        assert_eq!(
            fs::read_to_string(project_root.join("slug-merge.txt"))
                .ok()
                .as_deref(),
            Some("from slug source\n"),
            "expected merge to include source branch changes"
        );
    }

    #[test]
    fn workflow_runtime_git_save_worktree_patch_writes_command_patch() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");
        fs::write(project_root.join("README.md"), "updated\n").expect("update readme");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-save-patch",
            &["--help".to_string()],
            &["vizier".to_string(), "__workflow-node".to_string()],
            None,
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue");
        let record = read_record(&jobs_root, "job-save-patch").expect("record");
        let node = runtime_executor_node(
            "save_patch",
            "job-save-patch",
            "cap.env.builtin.git.save_worktree_patch",
            "git.save_worktree_patch",
            BTreeMap::new(),
        );
        let result = execute_workflow_executor(project_root, &jobs_root, &record, &node)
            .expect("save patch");
        assert_eq!(result.outcome, WorkflowNodeOutcome::Succeeded);
        let patch_path = command_patch_path(&jobs_root, "job-save-patch");
        assert!(patch_path.exists(), "expected command patch output");
    }

    #[test]
    fn workflow_runtime_patch_pipeline_prepare_execute_and_finalize() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        fs::write(project_root.join("sample.txt"), "before\n").expect("seed sample");
        git_commit_all(project_root, "seed sample");
        fs::write(project_root.join("sample.txt"), "after\n").expect("edit sample");
        let patch_path = project_root.join("sample.patch");
        let diff = git_output(project_root, &["diff", "--binary", "HEAD"]);
        assert!(diff.status.success(), "build patch diff");
        fs::write(&patch_path, diff.stdout).expect("write patch file");
        let restore = git_status(project_root, &["checkout", "--", "sample.txt"]);
        assert!(restore.success(), "restore sample");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-patch-pipeline",
            &["--help".to_string()],
            &["vizier".to_string(), "__workflow-node".to_string()],
            None,
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue");
        let record = read_record(&jobs_root, "job-patch-pipeline").expect("record");
        let files_json = serde_json::to_string(&vec![patch_path.display().to_string()])
            .expect("serialize files");

        let prepare = runtime_executor_node(
            "patch_prepare",
            "job-patch-pipeline",
            "cap.env.builtin.patch.pipeline_prepare",
            "patch.pipeline_prepare",
            BTreeMap::from([("files_json".to_string(), files_json.clone())]),
        );
        let prepare_result = execute_workflow_executor(project_root, &jobs_root, &record, &prepare)
            .expect("prepare");
        assert_eq!(prepare_result.outcome, WorkflowNodeOutcome::Succeeded);
        assert!(
            patch_pipeline_manifest_path(&jobs_root, "job-patch-pipeline").exists(),
            "expected pipeline manifest"
        );

        let execute = runtime_executor_node(
            "patch_execute",
            "job-patch-pipeline",
            "cap.env.builtin.patch.execute_pipeline",
            "patch.execute_pipeline",
            BTreeMap::from([("files_json".to_string(), files_json)]),
        );
        let execute_result = execute_workflow_executor(project_root, &jobs_root, &record, &execute)
            .expect("execute");
        assert_eq!(execute_result.outcome, WorkflowNodeOutcome::Succeeded);
        let staged = git_output(project_root, &["diff", "--cached", "--name-only"]);
        assert!(
            String::from_utf8_lossy(&staged.stdout).contains("sample.txt"),
            "expected patch application to stage sample.txt"
        );

        let finalize = runtime_executor_node(
            "patch_finalize",
            "job-patch-pipeline",
            "cap.env.builtin.patch.pipeline_finalize",
            "patch.pipeline_finalize",
            BTreeMap::new(),
        );
        let finalize_result =
            execute_workflow_executor(project_root, &jobs_root, &record, &finalize)
                .expect("finalize");
        assert_eq!(finalize_result.outcome, WorkflowNodeOutcome::Succeeded);
        assert!(
            command_patch_path(&jobs_root, "job-patch-pipeline").exists(),
            "expected finalized command patch"
        );
        assert!(
            patch_pipeline_finalize_path(&jobs_root, "job-patch-pipeline").exists(),
            "expected finalize marker"
        );
    }

    #[test]
    fn workflow_runtime_build_materialize_step_emits_artifacts() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-build-materialize",
            &["--help".to_string()],
            &["vizier".to_string(), "__workflow-node".to_string()],
            None,
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue");
        let record = read_record(&jobs_root, "job-build-materialize").expect("record");
        let node = runtime_executor_node(
            "materialize",
            "job-build-materialize",
            "cap.env.builtin.build.materialize_step",
            "build.materialize_step",
            BTreeMap::from([
                ("build_id".to_string(), "build-runtime".to_string()),
                ("step_key".to_string(), "s1".to_string()),
                ("slug".to_string(), "runtime-build".to_string()),
                ("branch".to_string(), "draft/runtime-build".to_string()),
                ("target".to_string(), "main".to_string()),
            ]),
        );
        let result = execute_workflow_executor(project_root, &jobs_root, &record, &node)
            .expect("materialize");
        assert_eq!(result.outcome, WorkflowNodeOutcome::Succeeded);
        assert!(
            result
                .artifacts_written
                .iter()
                .any(|artifact| matches!(artifact, JobArtifact::PlanBranch { .. })),
            "expected plan branch artifact"
        );
        assert!(
            project_root
                .join(
                    ".vizier/implementation-plans/builds/build-runtime/steps/s1/materialized.json"
                )
                .exists(),
            "expected build step materialized payload"
        );
    }

    #[test]
    fn workflow_runtime_merge_sentinel_write_and_clear() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-sentinel",
            &["--help".to_string()],
            &["vizier".to_string(), "__workflow-node".to_string()],
            Some(JobMetadata {
                plan: Some("runtime-sentinel".to_string()),
                ..JobMetadata::default()
            }),
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue");
        let record = read_record(&jobs_root, "job-sentinel").expect("record");

        let write_node = runtime_executor_node(
            "write_sentinel",
            "job-sentinel",
            "cap.env.builtin.merge.sentinel.write",
            "merge.sentinel.write",
            BTreeMap::new(),
        );
        let write_result =
            execute_workflow_executor(project_root, &jobs_root, &record, &write_node)
                .expect("write");
        assert_eq!(write_result.outcome, WorkflowNodeOutcome::Succeeded);
        let sentinel = project_root.join(".vizier/tmp/merge-conflicts/runtime-sentinel.json");
        assert!(sentinel.exists(), "expected sentinel written");

        let clear_node = runtime_executor_node(
            "clear_sentinel",
            "job-sentinel",
            "cap.env.builtin.merge.sentinel.clear",
            "merge.sentinel.clear",
            BTreeMap::new(),
        );
        let clear_result =
            execute_workflow_executor(project_root, &jobs_root, &record, &clear_node)
                .expect("clear");
        assert_eq!(clear_result.outcome, WorkflowNodeOutcome::Succeeded);
        assert!(!sentinel.exists(), "expected sentinel cleared");
    }

    #[test]
    fn workflow_runtime_command_and_cicd_shell_ops_respect_exit_status() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-shell-op",
            &["--help".to_string()],
            &["vizier".to_string(), "__workflow-node".to_string()],
            None,
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue");
        let record = read_record(&jobs_root, "job-shell-op").expect("record");

        let command_ok = runtime_executor_node(
            "command_ok",
            "job-shell-op",
            "cap.env.shell.command.run",
            "command.run",
            BTreeMap::from([("script".to_string(), "printf ok".to_string())]),
        );
        let command_ok_result =
            execute_workflow_executor(project_root, &jobs_root, &record, &command_ok)
                .expect("command ok");
        assert_eq!(command_ok_result.outcome, WorkflowNodeOutcome::Succeeded);

        let command_fail = runtime_executor_node(
            "command_fail",
            "job-shell-op",
            "cap.env.shell.command.run",
            "command.run",
            BTreeMap::from([("script".to_string(), "exit 9".to_string())]),
        );
        let command_fail_result =
            execute_workflow_executor(project_root, &jobs_root, &record, &command_fail)
                .expect("command fail");
        assert_eq!(command_fail_result.outcome, WorkflowNodeOutcome::Failed);
        assert_eq!(command_fail_result.exit_code, Some(9));

        let cicd = runtime_executor_node(
            "cicd_ok",
            "job-shell-op",
            "cap.env.shell.cicd.run",
            "cicd.run",
            BTreeMap::from([("script".to_string(), "exit 0".to_string())]),
        );
        let cicd_result =
            execute_workflow_executor(project_root, &jobs_root, &record, &cicd).expect("cicd");
        assert_eq!(cicd_result.outcome, WorkflowNodeOutcome::Succeeded);
    }

    #[test]
    fn workflow_runtime_conflict_cicd_approval_and_terminal_gates() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        fs::write(project_root.join("gate-conflict.txt"), "base\n").expect("write base");
        git_commit_all(project_root, "gate conflict base");
        let target = current_branch_name(project_root).expect("target branch");
        let checkout = git_status(project_root, &["checkout", "-b", "draft/gate-conflict"]);
        assert!(checkout.success(), "create draft gate branch");
        fs::write(project_root.join("gate-conflict.txt"), "draft\n").expect("write draft");
        git_commit_all(project_root, "gate draft");
        let checkout_target = git_status(project_root, &["checkout", &target]);
        assert!(checkout_target.success(), "checkout target");
        fs::write(project_root.join("gate-conflict.txt"), "target\n").expect("write target");
        git_commit_all(project_root, "gate target");
        let merge = git_status(project_root, &["merge", "--no-ff", "draft/gate-conflict"]);
        assert!(
            !merge.success(),
            "expected deliberate merge conflict for gate coverage"
        );

        let sentinel = project_root.join(".vizier/tmp/merge-conflicts/gate-conflict.json");
        if let Some(parent) = sentinel.parent() {
            fs::create_dir_all(parent).expect("create sentinel dir");
        }
        fs::write(&sentinel, "{}").expect("write sentinel");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-gates",
            &["--help".to_string()],
            &["vizier".to_string(), "__workflow-node".to_string()],
            Some(JobMetadata {
                plan: Some("gate-conflict".to_string()),
                workflow_node_attempt: Some(2),
                ..JobMetadata::default()
            }),
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue");
        let mut record = read_record(&jobs_root, "job-gates").expect("record");

        let conflict_gate = runtime_control_node(
            "conflict",
            "job-gates",
            "control.gate.conflict_resolution",
            "gate.conflict_resolution",
            BTreeMap::new(),
        );
        let conflict_result =
            execute_workflow_control(project_root, &record, &conflict_gate).expect("conflict gate");
        assert_eq!(conflict_result.outcome, WorkflowNodeOutcome::Blocked);

        let mut cicd_gate = runtime_control_node(
            "cicd",
            "job-gates",
            "control.gate.cicd",
            "gate.cicd",
            BTreeMap::new(),
        );
        cicd_gate.gates = vec![WorkflowGate::Cicd {
            script: "exit 7".to_string(),
            auto_resolve: false,
            policy: vizier_core::workflow_template::WorkflowGatePolicy::Retry,
        }];
        let cicd_result =
            execute_workflow_control(project_root, &record, &cicd_gate).expect("cicd gate");
        assert_eq!(cicd_result.outcome, WorkflowNodeOutcome::Failed);
        assert_eq!(cicd_result.exit_code, Some(7));

        let approval_gate = runtime_control_node(
            "approval",
            "job-gates",
            "control.gate.approval",
            "gate.approval",
            BTreeMap::new(),
        );
        record.schedule = Some(JobSchedule {
            approval: Some(pending_job_approval()),
            ..JobSchedule::default()
        });
        let approval_pending = execute_workflow_control(project_root, &record, &approval_gate)
            .expect("approval pending");
        assert_eq!(approval_pending.outcome, WorkflowNodeOutcome::Blocked);

        if let Some(schedule) = record.schedule.as_mut()
            && let Some(approval) = schedule.approval.as_mut()
        {
            approval.state = JobApprovalState::Approved;
        }
        let approval_ok =
            execute_workflow_control(project_root, &record, &approval_gate).expect("approval ok");
        assert_eq!(approval_ok.outcome, WorkflowNodeOutcome::Succeeded);

        if let Some(schedule) = record.schedule.as_mut()
            && let Some(approval) = schedule.approval.as_mut()
        {
            approval.state = JobApprovalState::Rejected;
            approval.reason = Some("manual reject".to_string());
        }
        let approval_rejected = execute_workflow_control(project_root, &record, &approval_gate)
            .expect("approval rejected");
        assert_eq!(approval_rejected.outcome, WorkflowNodeOutcome::Failed);
        assert_eq!(approval_rejected.exit_code, Some(10));

        let mut terminal = runtime_control_node(
            "terminal",
            "job-gates",
            "control.terminal",
            "terminal",
            BTreeMap::new(),
        );
        terminal.routes.failed.push(WorkflowRouteTarget {
            node_id: "unexpected".to_string(),
            mode: WorkflowRouteMode::RetryJob,
        });
        let invalid_terminal =
            execute_workflow_control(project_root, &record, &terminal).expect("terminal invalid");
        assert_eq!(invalid_terminal.outcome, WorkflowNodeOutcome::Failed);

        terminal.routes = WorkflowRouteTargets::default();
        let valid_terminal =
            execute_workflow_control(project_root, &record, &terminal).expect("terminal valid");
        assert_eq!(valid_terminal.outcome, WorkflowNodeOutcome::Succeeded);
    }

    #[test]
    fn stop_condition_runtime_blocks_when_retry_budget_is_exhausted() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "job-stop-gate",
            &["--help".to_string()],
            &["vizier".to_string(), "__workflow-node".to_string()],
            Some(JobMetadata {
                workflow_node_attempt: Some(3),
                ..JobMetadata::default()
            }),
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue");
        let record = read_record(&jobs_root, "job-stop-gate").expect("record");

        let node = WorkflowRuntimeNodeManifest {
            node_id: "gate".to_string(),
            job_id: "job-stop-gate".to_string(),
            uses: "control.gate.stop_condition".to_string(),
            kind: WorkflowNodeKind::Gate,
            args: BTreeMap::from([("script".to_string(), "exit 1".to_string())]),
            executor_operation: None,
            control_policy: Some("gate.stop_condition".to_string()),
            gates: Vec::new(),
            retry: vizier_core::workflow_template::WorkflowRetryPolicy {
                mode: WorkflowRetryMode::UntilGate,
                budget: 1,
            },
            routes: WorkflowRouteTargets::default(),
            artifacts_by_outcome: WorkflowOutcomeArtifactsByOutcome::default(),
        };

        let result = execute_workflow_control(project_root, &record, &node)
            .expect("execute stop-condition gate");
        assert_eq!(result.outcome, WorkflowNodeOutcome::Blocked);
        assert!(
            result
                .summary
                .as_deref()
                .unwrap_or("")
                .contains("retry budget exhausted"),
            "expected budget summary, got {:?}",
            result.summary
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
