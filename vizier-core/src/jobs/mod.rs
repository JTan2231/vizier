#![allow(dead_code)]

use crate::scheduler::spec::{
    self, AfterDependencyState, JobAfterDependencyStatus, JobPreconditionFact,
    JobPreconditionState, PinnedHeadFact, SchedulerAction, SchedulerFacts,
};
#[allow(unused_imports)]
pub use crate::scheduler::{
    AfterPolicy, JobAfterDependency, JobApprovalFact, JobApprovalState, JobArtifact, JobLock,
    JobPrecondition, JobStatus, JobWaitKind, JobWaitReason, LockMode, MissingProducerPolicy,
    PinnedHead, format_artifact,
};
use crate::workflow_audit::{WorkflowAuditReport, analyze_workflow_template_with_effective_locks};
use crate::workflow_template::{
    CompiledWorkflowNode, OPERATION_OUTPUT_ARTIFACT_TYPE_ID, PROMPT_ARTIFACT_TYPE_ID, WorkflowGate,
    WorkflowNodeKind, WorkflowOutcomeEdges, WorkflowPrecondition, WorkflowRetryMode,
    WorkflowTemplate, compile_workflow_node, validate_workflow_capability_contracts,
    workflow_operation_output_artifact,
};
use crate::{
    agent::{AgentError, AgentRequest, DEFAULT_AGENT_TIMEOUT},
    config, display,
};
use chrono::{DateTime, Duration, Utc};
use git2::{ErrorCode, Oid, Repository, WorktreePruneOptions};
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

const PLAN_TEXT_ARTIFACT_TYPE_ID: &str = "plan_text";
const OPERATION_OUTPUT_SCHEMA_ID: &str = "vizier.operation_output.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobDependency {
    pub artifact: JobArtifact,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct JobDependenciesPolicy {
    #[serde(default)]
    pub missing_producer: MissingProducerPolicy,
}

fn is_default_dependency_policy(policy: &JobDependenciesPolicy) -> bool {
    policy == &JobDependenciesPolicy::default()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobSchedule {
    #[serde(default)]
    pub after: Vec<JobAfterDependency>,
    #[serde(default)]
    pub dependencies: Vec<JobDependency>,
    #[serde(default, skip_serializing_if = "is_default_dependency_policy")]
    pub dependency_policy: JobDependenciesPolicy,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EphemeralCleanupState {
    Pending,
    Deferred,
    Completed,
    Degraded,
}

impl EphemeralCleanupState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Deferred => "deferred",
            Self::Completed => "completed",
            Self::Degraded => "degraded",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct EphemeralRunBaseline {
    #[serde(default)]
    pub vizier_root_existed: bool,
    #[serde(default)]
    pub preexisting_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobMetadata {
    pub command_alias: Option<String>,
    pub scope: Option<String>,
    pub target: Option<String>,
    pub plan: Option<String>,
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral_run: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral_cleanup_requested: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral_cleanup_state: Option<EphemeralCleanupState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral_cleanup_detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral_owned_branches: Option<Vec<String>>,
    pub workflow_run_id: Option<String>,
    pub workflow_node_name: Option<String>,
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
    pub process_liveness_state: Option<ProcessLivenessState>,
    pub process_liveness_checked_at: Option<DateTime<Utc>>,
    pub process_liveness_failure_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessLivenessState {
    Alive,
    StaleMissingPid,
    StaleNotRunning,
    StaleIdentityMismatch,
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

mod cleanup;
mod graph;
mod logs;
mod monitor;
mod scheduler;
#[cfg(test)]
mod tests;
mod workflow;

#[allow(unused_imports)]
use cleanup::*;
#[allow(unused_imports)]
use graph::*;
#[allow(unused_imports)]
use logs::*;
#[allow(unused_imports)]
use monitor::*;
#[allow(unused_imports)]
use scheduler::*;
#[allow(unused_imports)]
use workflow::*;

pub use cleanup::{
    CancelJobOutcome, CleanJobError, CleanJobErrorKind, CleanJobOptions, CleanJobOutcome,
    CleanRemovedCounts, CleanScope, CleanSkippedItems, RetryOutcome, approve_job,
    cancel_job_with_cleanup, clean_job_scope, gc_jobs, record_current_job_worktree,
    record_job_worktree, reject_job, retry_job,
};
pub use graph::ScheduleGraph;
pub use logs::{follow_job_logs_raw, latest_job_log_line, tail_job_logs};
pub use monitor::*;
pub use scheduler::{
    EphemeralRunCleanupEvent, SchedulerOutcome, scheduler_tick,
    scheduler_tick_without_ephemeral_cleanup,
};
pub use workflow::{
    EnqueueWorkflowRunResult, WorkflowRunEnqueueOptions, audit_workflow_run_template,
    enqueue_workflow_run, enqueue_workflow_run_with_options, run_workflow_node_command,
    validate_workflow_run_template,
};

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
            if base.ephemeral_run.is_none() {
                base.ephemeral_run = update.ephemeral_run;
            }
            if update.ephemeral_cleanup_requested.is_some() {
                base.ephemeral_cleanup_requested = update.ephemeral_cleanup_requested;
            }
            if update.ephemeral_cleanup_state.is_some() {
                base.ephemeral_cleanup_state = update.ephemeral_cleanup_state;
                base.ephemeral_cleanup_detail = update.ephemeral_cleanup_detail;
            } else if update.ephemeral_cleanup_detail.is_some() {
                base.ephemeral_cleanup_detail = update.ephemeral_cleanup_detail;
            }
            if is_empty_vec(&base.ephemeral_owned_branches) {
                base.ephemeral_owned_branches = update.ephemeral_owned_branches;
            }
            if base.workflow_run_id.is_none() {
                base.workflow_run_id = update.workflow_run_id;
            }
            if base.workflow_node_name.is_none() {
                base.workflow_node_name = update.workflow_node_name;
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
            if update.process_liveness_state.is_some() {
                base.process_liveness_state = update.process_liveness_state;
            }
            if update.process_liveness_checked_at.is_some() {
                base.process_liveness_checked_at = update.process_liveness_checked_at;
            }
            if update.process_liveness_failure_reason.is_some() {
                base.process_liveness_failure_reason = update.process_liveness_failure_reason;
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
