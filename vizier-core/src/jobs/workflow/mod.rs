use super::*;

mod compile;
mod control;
mod executor;
mod runtime;

#[allow(unused_imports)]
pub(crate) use compile::*;
#[allow(unused_imports)]
pub(crate) use control::*;
#[allow(unused_imports)]
pub(crate) use executor::*;
#[allow(unused_imports)]
pub(crate) use runtime::*;

pub use compile::{
    WorkflowRunEnqueueOptions, audit_workflow_run_template, enqueue_workflow_run,
    enqueue_workflow_run_with_options, validate_workflow_run_template,
};
pub use runtime::run_workflow_node_command;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkflowNodeOutcome {
    Succeeded,
    Failed,
    Blocked,
    Cancelled,
}

impl WorkflowNodeOutcome {
    pub(crate) fn as_str(self) -> &'static str {
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stderr_lines: Vec<String>,
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
            stdout_text: None,
            stderr_lines: Vec::new(),
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
            stdout_text: None,
            stderr_lines: Vec::new(),
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
            stdout_text: None,
            stderr_lines: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WorkflowOperationOutputPayload {
    schema: String,
    run_id: String,
    job_id: String,
    node_id: String,
    uses: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    executor_operation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    control_policy: Option<String>,
    outcome: String,
    exit_code: i32,
    stdout_text: String,
    stderr_lines: Vec<String>,
    started_at: String,
    finished_at: String,
    duration_ms: i64,
    result: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkflowRouteMode {
    PropagateContext,
    RetryJob,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct WorkflowRouteTarget {
    pub(crate) node_id: String,
    pub(crate) mode: WorkflowRouteMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct WorkflowRouteTargets {
    #[serde(default)]
    pub(crate) succeeded: Vec<WorkflowRouteTarget>,
    #[serde(default)]
    pub(crate) failed: Vec<WorkflowRouteTarget>,
    #[serde(default)]
    pub(crate) blocked: Vec<WorkflowRouteTarget>,
    #[serde(default)]
    pub(crate) cancelled: Vec<WorkflowRouteTarget>,
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
pub(crate) struct WorkflowRuntimeNodeManifest {
    pub(crate) node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    pub(crate) job_id: String,
    pub(crate) uses: String,
    pub(crate) kind: WorkflowNodeKind,
    pub(crate) args: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) executor_operation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) control_policy: Option<String>,
    #[serde(default)]
    pub(crate) gates: Vec<WorkflowGate>,
    pub(crate) retry: crate::workflow_template::WorkflowRetryPolicy,
    pub(crate) routes: WorkflowRouteTargets,
    #[serde(default)]
    pub(crate) artifacts_by_outcome: WorkflowOutcomeArtifactsByOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct WorkflowOutcomeArtifactsByOutcome {
    #[serde(default)]
    pub(crate) succeeded: Vec<JobArtifact>,
    #[serde(default)]
    pub(crate) failed: Vec<JobArtifact>,
    #[serde(default)]
    pub(crate) blocked: Vec<JobArtifact>,
    #[serde(default)]
    pub(crate) cancelled: Vec<JobArtifact>,
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
pub(crate) struct WorkflowRunManifest {
    pub(crate) run_id: String,
    pub(crate) template_selector: String,
    pub(crate) template_id: String,
    pub(crate) template_version: String,
    pub(crate) policy_snapshot_hash: String,
    #[serde(default)]
    pub(crate) ephemeral: bool,
    #[serde(default)]
    pub(crate) ephemeral_cleanup_requested: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ephemeral_cleanup_state: Option<EphemeralCleanupState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ephemeral_cleanup_detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ephemeral_baseline: Option<EphemeralRunBaseline>,
    pub(crate) nodes: BTreeMap<String, WorkflowRuntimeNodeManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnqueueWorkflowRunResult {
    pub run_id: String,
    pub template_selector: String,
    pub template_id: String,
    pub template_version: String,
    pub policy_snapshot_hash: String,
    #[serde(default)]
    pub ephemeral: bool,
    pub job_ids: BTreeMap<String, String>,
}
