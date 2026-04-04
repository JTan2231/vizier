use super::*;

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

pub const JOB_MONITOR_VERSION: u32 = 1;
pub const SCHEDULE_SNAPSHOT_VERSION: u32 = 1;
pub const SCHEDULE_SNAPSHOT_ORDERING: &str = "created_at_then_job_id";

#[derive(Debug, Clone, Serialize)]
pub struct JobMonitorWait {
    pub kind: JobWaitKind,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobMonitorSchedule {
    pub after: Vec<JobAfterDependency>,
    pub dependencies: Vec<JobDependency>,
    pub artifacts: Vec<JobArtifact>,
    pub locks: Vec<JobLock>,
    pub approval: Option<JobApproval>,
    pub pinned_head: Option<PinnedHead>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobMonitorWorkflow {
    pub run_id: Option<String>,
    pub node_id: Option<String>,
    pub node_name: Option<String>,
    pub executor_class: Option<String>,
    pub executor_operation: Option<String>,
    pub control_policy: Option<String>,
    pub template_selector: Option<String>,
    pub template_id: Option<String>,
    pub template_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral_run: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_state: Option<EphemeralCleanupState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobMonitorContext {
    pub command_alias: Option<String>,
    pub plan: Option<String>,
    pub target: Option<String>,
    pub branch: Option<String>,
    pub execution_root: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobMonitorRecord {
    pub job_id: String,
    pub status: JobStatus,
    pub command: Vec<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub stdout_path: String,
    pub stderr_path: String,
    pub session_path: Option<String>,
    pub outcome_path: Option<String>,
    pub wait: Option<JobMonitorWait>,
    pub waited_on: Vec<JobWaitKind>,
    pub schedule: Option<JobMonitorSchedule>,
    pub workflow: JobMonitorWorkflow,
    pub context: JobMonitorContext,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobMonitorScheduleRow {
    pub order: usize,
    pub job_id: String,
    pub slug: Option<String>,
    pub name: String,
    pub command: Vec<String>,
    pub status: JobStatus,
    pub wait: Option<JobMonitorWait>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobMonitorListEnvelope {
    pub version: u32,
    pub generated_at: String,
    pub jobs: Vec<JobMonitorRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobMonitorShowEnvelope {
    pub version: u32,
    pub generated_at: String,
    pub job: JobMonitorRecord,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobMonitorScheduleEnvelope {
    pub version: u32,
    pub generated_at: String,
    pub ordering: &'static str,
    pub jobs: Vec<JobMonitorScheduleRow>,
    pub edges: Vec<ScheduleEdge>,
}

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
    pub command: Vec<String>,
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

fn wait_kind_label(kind: JobWaitKind) -> &'static str {
    match kind {
        JobWaitKind::Dependencies => "dependencies",
        JobWaitKind::Approval => "approval",
        JobWaitKind::Locks => "locks",
        JobWaitKind::PinnedHead => "pinned_head",
        JobWaitKind::Preconditions => "preconditions",
    }
}

fn monitor_wait_reason(reason: &JobWaitReason) -> JobMonitorWait {
    let detail = reason
        .detail
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .unwrap_or_else(|| wait_kind_label(reason.kind).to_string());
    JobMonitorWait {
        kind: reason.kind,
        detail,
    }
}

fn monitor_schedule(schedule: Option<&JobSchedule>) -> Option<JobMonitorSchedule> {
    schedule.map(|schedule| JobMonitorSchedule {
        after: schedule.after.clone(),
        dependencies: schedule.dependencies.clone(),
        artifacts: schedule.artifacts.clone(),
        locks: schedule.locks.clone(),
        approval: schedule.approval.clone(),
        pinned_head: schedule.pinned_head.clone(),
    })
}

pub fn project_job_monitor_record(record: &JobRecord) -> JobMonitorRecord {
    let metadata = record.metadata.as_ref();
    let wait = record
        .schedule
        .as_ref()
        .and_then(|schedule| schedule.wait_reason.as_ref())
        .map(monitor_wait_reason);
    let waited_on = record
        .schedule
        .as_ref()
        .map(|schedule| schedule.waited_on.clone())
        .unwrap_or_default();

    JobMonitorRecord {
        job_id: record.id.clone(),
        status: record.status,
        command: record.command.clone(),
        created_at: record.created_at.to_rfc3339(),
        started_at: record.started_at.map(|value| value.to_rfc3339()),
        finished_at: record.finished_at.map(|value| value.to_rfc3339()),
        pid: record.pid,
        exit_code: record.exit_code,
        stdout_path: record.stdout_path.clone(),
        stderr_path: record.stderr_path.clone(),
        session_path: record.session_path.clone(),
        outcome_path: record.outcome_path.clone(),
        wait,
        waited_on,
        schedule: monitor_schedule(record.schedule.as_ref()),
        workflow: JobMonitorWorkflow {
            run_id: metadata.and_then(|meta| meta.workflow_run_id.clone()),
            node_id: metadata.and_then(|meta| meta.workflow_node_id.clone()),
            node_name: metadata.and_then(|meta| meta.workflow_node_name.clone()),
            executor_class: metadata.and_then(|meta| meta.workflow_executor_class.clone()),
            executor_operation: metadata.and_then(|meta| meta.workflow_executor_operation.clone()),
            control_policy: metadata.and_then(|meta| meta.workflow_control_policy.clone()),
            template_selector: metadata.and_then(|meta| meta.workflow_template_selector.clone()),
            template_id: metadata.and_then(|meta| meta.workflow_template_id.clone()),
            template_version: metadata.and_then(|meta| meta.workflow_template_version.clone()),
            ephemeral_run: metadata.and_then(|meta| meta.ephemeral_run),
            cleanup_state: metadata.and_then(|meta| meta.ephemeral_cleanup_state),
            cleanup_detail: metadata.and_then(|meta| meta.ephemeral_cleanup_detail.clone()),
        },
        context: JobMonitorContext {
            command_alias: metadata.and_then(|meta| meta.command_alias.clone()),
            plan: metadata.and_then(|meta| meta.plan.clone()),
            target: metadata.and_then(|meta| meta.target.clone()),
            branch: metadata.and_then(|meta| meta.branch.clone()),
            execution_root: metadata.and_then(|meta| meta.execution_root.clone()),
        },
    }
}

pub fn project_job_monitor_schedule_row(
    order: usize,
    slug: Option<String>,
    name: String,
    record: &JobRecord,
) -> JobMonitorScheduleRow {
    let wait = record
        .schedule
        .as_ref()
        .and_then(|schedule| schedule.wait_reason.as_ref())
        .map(monitor_wait_reason);

    JobMonitorScheduleRow {
        order,
        job_id: record.id.clone(),
        slug,
        name,
        command: record.command.clone(),
        status: record.status,
        wait,
        created_at: record.created_at.to_rfc3339(),
    }
}

pub fn build_job_monitor_list_envelope(records: &[JobRecord]) -> JobMonitorListEnvelope {
    JobMonitorListEnvelope {
        version: JOB_MONITOR_VERSION,
        generated_at: Utc::now().to_rfc3339(),
        jobs: records.iter().map(project_job_monitor_record).collect(),
    }
}

pub fn build_job_monitor_show_envelope(record: &JobRecord) -> JobMonitorShowEnvelope {
    JobMonitorShowEnvelope {
        version: JOB_MONITOR_VERSION,
        generated_at: Utc::now().to_rfc3339(),
        job: project_job_monitor_record(record),
    }
}

pub fn build_job_monitor_schedule_envelope(
    jobs: Vec<JobMonitorScheduleRow>,
    edges: Vec<ScheduleEdge>,
) -> JobMonitorScheduleEnvelope {
    JobMonitorScheduleEnvelope {
        version: JOB_MONITOR_VERSION,
        generated_at: Utc::now().to_rfc3339(),
        ordering: SCHEDULE_SNAPSHOT_ORDERING,
        jobs,
        edges,
    }
}
