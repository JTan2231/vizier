use crate::scheduler::{AfterPolicy, JobAfterDependency, JobArtifact, JobLock, format_artifact};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowTemplate {
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    #[serde(default)]
    pub policy: WorkflowTemplatePolicy,
    #[serde(default)]
    pub artifact_contracts: Vec<WorkflowArtifactContract>,
    #[serde(default)]
    pub nodes: Vec<WorkflowNode>,
}

impl WorkflowTemplate {
    pub fn policy_snapshot(&self) -> WorkflowPolicySnapshot {
        let mut artifact_contracts = self.artifact_contracts.clone();
        artifact_contracts.sort_by(|left, right| left.id.cmp(&right.id));

        let mut nodes = self
            .nodes
            .iter()
            .map(WorkflowPolicySnapshotNode::from_node)
            .collect::<Vec<_>>();
        nodes.sort_by(|left, right| left.id.cmp(&right.id));

        WorkflowPolicySnapshot {
            template_id: self.id.clone(),
            template_version: self.version.clone(),
            failure_mode: self.policy.failure_mode,
            resume: self.policy.resume.clone(),
            artifact_contracts,
            nodes,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowTemplatePolicy {
    #[serde(default)]
    pub failure_mode: WorkflowFailureMode,
    #[serde(default)]
    pub resume: WorkflowResumePolicy,
}

impl Default for WorkflowTemplatePolicy {
    fn default() -> Self {
        Self {
            failure_mode: WorkflowFailureMode::BlockDownstream,
            resume: WorkflowResumePolicy::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowFailureMode {
    BlockDownstream,
    ContinueIndependent,
}

impl Default for WorkflowFailureMode {
    fn default() -> Self {
        Self::BlockDownstream
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowResumePolicy {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub reuse_mode: WorkflowResumeReuseMode,
}

impl Default for WorkflowResumePolicy {
    fn default() -> Self {
        Self {
            key: "default".to_string(),
            reuse_mode: WorkflowResumeReuseMode::Strict,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowResumeReuseMode {
    Strict,
    Compatible,
}

impl Default for WorkflowResumeReuseMode {
    fn default() -> Self {
        Self::Strict
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowArtifactContract {
    pub id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowNode {
    pub id: String,
    #[serde(default)]
    pub kind: WorkflowNodeKind,
    pub uses: String,
    #[serde(default)]
    pub args: BTreeMap<String, String>,
    #[serde(default)]
    pub after: Vec<WorkflowAfterDependency>,
    #[serde(default)]
    pub needs: Vec<JobArtifact>,
    #[serde(default)]
    pub produces: WorkflowOutcomeArtifacts,
    #[serde(default)]
    pub locks: Vec<JobLock>,
    #[serde(default)]
    pub preconditions: Vec<WorkflowPrecondition>,
    #[serde(default)]
    pub gates: Vec<WorkflowGate>,
    #[serde(default)]
    pub retry: WorkflowRetryPolicy,
    #[serde(default)]
    pub on: WorkflowOutcomeEdges,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkflowCapability {
    GitSaveWorktreePatch,
    PlanGenerateDraftPlan,
    PlanApplyOnce,
    ReviewCritiqueOrFix,
    GitIntegratePlanBranch,
    PatchExecutePipeline,
    BuildMaterializeStep,
    GateStopCondition,
    GateConflictResolution,
    GateCicd,
    RemediationCicdAutoFix,
    ExecCustomCommand,
    ReviewApplyFixesOnly,
    InternalTerminalSink,
}

impl WorkflowCapability {
    pub fn id(self) -> &'static str {
        match self {
            Self::GitSaveWorktreePatch => "cap.git.save_worktree_patch",
            Self::PlanGenerateDraftPlan => "cap.plan.generate_draft_plan",
            Self::PlanApplyOnce => "cap.plan.apply_once",
            Self::ReviewCritiqueOrFix => "cap.review.critique_or_fix",
            Self::GitIntegratePlanBranch => "cap.git.integrate_plan_branch",
            Self::PatchExecutePipeline => "cap.patch.execute_pipeline",
            Self::BuildMaterializeStep => "cap.build.materialize_step",
            Self::GateStopCondition => "cap.gate.stop_condition",
            Self::GateConflictResolution => "cap.gate.conflict_resolution",
            Self::GateCicd => "cap.gate.cicd",
            Self::RemediationCicdAutoFix => "cap.remediation.cicd_auto_fix",
            Self::ExecCustomCommand => "cap.exec.custom_command",
            Self::ReviewApplyFixesOnly => "cap.review.apply_fixes_only",
            Self::InternalTerminalSink => "cap.internal.terminal_sink",
        }
    }

    pub fn from_id(value: &str) -> Option<Self> {
        match value {
            "cap.git.save_worktree_patch" => Some(Self::GitSaveWorktreePatch),
            "cap.plan.generate_draft_plan" => Some(Self::PlanGenerateDraftPlan),
            "cap.plan.apply_once" => Some(Self::PlanApplyOnce),
            "cap.review.critique_or_fix" => Some(Self::ReviewCritiqueOrFix),
            "cap.git.integrate_plan_branch" => Some(Self::GitIntegratePlanBranch),
            "cap.patch.execute_pipeline" => Some(Self::PatchExecutePipeline),
            "cap.build.materialize_step" => Some(Self::BuildMaterializeStep),
            "cap.gate.stop_condition" => Some(Self::GateStopCondition),
            "cap.gate.conflict_resolution" => Some(Self::GateConflictResolution),
            "cap.gate.cicd" => Some(Self::GateCicd),
            "cap.remediation.cicd_auto_fix" => Some(Self::RemediationCicdAutoFix),
            "cap.exec.custom_command" => Some(Self::ExecCustomCommand),
            "cap.review.apply_fixes_only" => Some(Self::ReviewApplyFixesOnly),
            "cap.internal.terminal_sink" => Some(Self::InternalTerminalSink),
            _ => None,
        }
    }

    pub fn from_uses_label(value: &str) -> Option<Self> {
        match value {
            "cap.git.save_worktree_patch" => Some(Self::GitSaveWorktreePatch),
            "cap.plan.generate_draft_plan" => Some(Self::PlanGenerateDraftPlan),
            "cap.plan.apply_once" => Some(Self::PlanApplyOnce),
            "cap.review.critique_or_fix" => Some(Self::ReviewCritiqueOrFix),
            "cap.git.integrate_plan_branch" => Some(Self::GitIntegratePlanBranch),
            "cap.patch.execute_pipeline" => Some(Self::PatchExecutePipeline),
            "cap.build.materialize_step" => Some(Self::BuildMaterializeStep),
            "cap.gate.stop_condition" => Some(Self::GateStopCondition),
            "cap.gate.conflict_resolution" => Some(Self::GateConflictResolution),
            "cap.gate.cicd" => Some(Self::GateCicd),
            "cap.remediation.cicd_auto_fix" => Some(Self::RemediationCicdAutoFix),
            "cap.exec.custom_command" => Some(Self::ExecCustomCommand),
            "cap.review.apply_fixes_only" => Some(Self::ReviewApplyFixesOnly),
            "cap.internal.terminal_sink" => Some(Self::InternalTerminalSink),
            "vizier.save.apply" => Some(Self::GitSaveWorktreePatch),
            "vizier.draft.generate_plan" => Some(Self::PlanGenerateDraftPlan),
            "vizier.approve.apply_once" => Some(Self::PlanApplyOnce),
            "vizier.review.critique" => Some(Self::ReviewCritiqueOrFix),
            "vizier.merge.integrate" => Some(Self::GitIntegratePlanBranch),
            "vizier.patch.execute" => Some(Self::PatchExecutePipeline),
            "vizier.build.materialize" => Some(Self::BuildMaterializeStep),
            "vizier.approve.stop_condition" => Some(Self::GateStopCondition),
            "vizier.merge.conflict_resolution" => Some(Self::GateConflictResolution),
            "vizier.merge.cicd_gate" => Some(Self::GateCicd),
            "vizier.merge.cicd_auto_fix" => Some(Self::RemediationCicdAutoFix),
            "vizier.review.apply" => Some(Self::ReviewApplyFixesOnly),
            "vizier.approve.terminal" | "vizier.merge.terminal" => Some(Self::InternalTerminalSink),
            _ => None,
        }
    }
}

pub fn workflow_node_capability(node: &WorkflowNode) -> Option<WorkflowCapability> {
    let uses = node.uses.trim();
    WorkflowCapability::from_id(uses).or_else(|| WorkflowCapability::from_uses_label(uses))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodeKind {
    Builtin,
    Agent,
    Shell,
    Gate,
    Custom,
}

impl Default for WorkflowNodeKind {
    fn default() -> Self {
        Self::Builtin
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodeClass {
    Executor,
    Control,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowExecutorClass {
    EnvironmentBuiltin,
    EnvironmentShell,
    Agent,
}

impl WorkflowExecutorClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EnvironmentBuiltin => "environment_builtin",
            Self::EnvironmentShell => "environment_shell",
            Self::Agent => "agent",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowDiagnosticLevel {
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowDiagnostic {
    pub level: WorkflowDiagnosticLevel,
    pub node_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowNodeIdentity {
    pub node_class: WorkflowNodeClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_class: Option<WorkflowExecutorClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_operation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_capability_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<WorkflowDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowAfterDependency {
    pub node_id: String,
    #[serde(default)]
    pub policy: AfterPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WorkflowOutcomeArtifacts {
    #[serde(default)]
    pub succeeded: Vec<JobArtifact>,
    #[serde(default)]
    pub failed: Vec<JobArtifact>,
    #[serde(default)]
    pub blocked: Vec<JobArtifact>,
    #[serde(default)]
    pub cancelled: Vec<JobArtifact>,
}

impl WorkflowOutcomeArtifacts {
    pub fn all(&self) -> Vec<JobArtifact> {
        let mut combined = Vec::new();
        combined.extend(self.succeeded.iter().cloned());
        combined.extend(self.failed.iter().cloned());
        combined.extend(self.blocked.iter().cloned());
        combined.extend(self.cancelled.iter().cloned());
        dedup_artifacts(combined)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WorkflowOutcomeEdges {
    #[serde(default)]
    pub succeeded: Vec<String>,
    #[serde(default)]
    pub failed: Vec<String>,
    #[serde(default)]
    pub blocked: Vec<String>,
    #[serde(default)]
    pub cancelled: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkflowPrecondition {
    PinnedHead,
    CleanWorktree,
    BranchExists,
    Custom {
        id: String,
        #[serde(default)]
        args: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkflowGate {
    Approval {
        #[serde(default)]
        required: bool,
        #[serde(default)]
        policy: WorkflowGatePolicy,
    },
    Script {
        script: String,
        #[serde(default)]
        policy: WorkflowGatePolicy,
    },
    Cicd {
        script: String,
        #[serde(default)]
        auto_resolve: bool,
        #[serde(default)]
        policy: WorkflowGatePolicy,
    },
    Custom {
        id: String,
        #[serde(default)]
        policy: WorkflowGatePolicy,
        #[serde(default)]
        args: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowGatePolicy {
    Block,
    Warn,
    Retry,
}

impl Default for WorkflowGatePolicy {
    fn default() -> Self {
        Self::Block
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowRetryPolicy {
    #[serde(default)]
    pub mode: WorkflowRetryMode,
    #[serde(default)]
    pub budget: u32,
}

impl Default for WorkflowRetryPolicy {
    fn default() -> Self {
        Self {
            mode: WorkflowRetryMode::Never,
            budget: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRetryMode {
    Never,
    OnFailure,
    UntilGate,
}

impl Default for WorkflowRetryMode {
    fn default() -> Self {
        Self::Never
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowPolicySnapshot {
    pub template_id: String,
    pub template_version: String,
    pub failure_mode: WorkflowFailureMode,
    pub resume: WorkflowResumePolicy,
    pub artifact_contracts: Vec<WorkflowArtifactContract>,
    pub nodes: Vec<WorkflowPolicySnapshotNode>,
}

impl WorkflowPolicySnapshot {
    pub fn stable_hash_hex(&self) -> Result<String, serde_json::Error> {
        let bytes = serde_json::to_vec(self)?;
        let digest = Sha256::digest(bytes);
        Ok(format!("{digest:x}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowPolicySnapshotNode {
    pub id: String,
    pub kind: WorkflowNodeKind,
    pub uses: String,
    pub args: BTreeMap<String, String>,
    pub after: Vec<WorkflowAfterDependency>,
    pub needs: Vec<String>,
    pub produces: WorkflowPolicySnapshotArtifacts,
    pub locks: Vec<WorkflowPolicySnapshotLock>,
    pub preconditions: Vec<String>,
    pub gates: Vec<String>,
    pub retry: WorkflowRetryPolicy,
    pub on: WorkflowOutcomeEdges,
}

impl WorkflowPolicySnapshotNode {
    fn from_node(node: &WorkflowNode) -> Self {
        let mut after = node.after.clone();
        after.sort_by(|left, right| {
            left.node_id
                .cmp(&right.node_id)
                .then_with(|| format!("{:?}", left.policy).cmp(&format!("{:?}", right.policy)))
        });

        let mut needs = node.needs.iter().map(format_artifact).collect::<Vec<_>>();
        needs.sort();
        needs.dedup();

        let mut locks = node
            .locks
            .iter()
            .map(|lock| WorkflowPolicySnapshotLock {
                key: lock.key.clone(),
                mode: format!("{:?}", lock.mode).to_ascii_lowercase(),
            })
            .collect::<Vec<_>>();
        locks.sort_by(|left, right| left.key.cmp(&right.key).then(left.mode.cmp(&right.mode)));
        locks.dedup();

        let mut preconditions = node
            .preconditions
            .iter()
            .map(workflow_precondition_label)
            .collect::<Vec<_>>();
        preconditions.sort();
        preconditions.dedup();

        let mut gates = node
            .gates
            .iter()
            .map(workflow_gate_label)
            .collect::<Vec<_>>();
        gates.sort();
        gates.dedup();

        Self {
            id: node.id.clone(),
            kind: node.kind,
            uses: node.uses.clone(),
            args: node.args.clone(),
            after,
            needs,
            produces: WorkflowPolicySnapshotArtifacts::from_outcomes(&node.produces),
            locks,
            preconditions,
            gates,
            retry: node.retry.clone(),
            on: normalize_outcome_edges(&node.on),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowPolicySnapshotArtifacts {
    pub succeeded: Vec<String>,
    pub failed: Vec<String>,
    pub blocked: Vec<String>,
    pub cancelled: Vec<String>,
}

impl WorkflowPolicySnapshotArtifacts {
    fn from_outcomes(artifacts: &WorkflowOutcomeArtifacts) -> Self {
        Self {
            succeeded: sorted_artifact_labels(&artifacts.succeeded),
            failed: sorted_artifact_labels(&artifacts.failed),
            blocked: sorted_artifact_labels(&artifacts.blocked),
            cancelled: sorted_artifact_labels(&artifacts.cancelled),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorkflowPolicySnapshotLock {
    pub key: String,
    pub mode: String,
}

const LEGACY_CAPABILITY_COMPATIBILITY_END_DATE: &str = "2026-06-01";
pub const PROMPT_ARTIFACT_TYPE_ID: &str = "prompt_text";

#[derive(Debug, Clone)]
struct WorkflowNodeResolution {
    identity: WorkflowNodeIdentity,
    capability: Option<WorkflowCapability>,
}

pub fn workflow_template_diagnostics(
    template: &WorkflowTemplate,
) -> Result<Vec<WorkflowDiagnostic>, String> {
    let mut diagnostics = Vec::new();
    for node in &template.nodes {
        let resolved = classify_template_node(template, node)?;
        diagnostics.extend(resolved.identity.diagnostics.clone());
    }
    Ok(diagnostics)
}

fn classify_template_node(
    template: &WorkflowTemplate,
    node: &WorkflowNode,
) -> Result<WorkflowNodeResolution, String> {
    let uses = node.uses.trim();
    let mut diagnostics = Vec::new();

    let mut resolved = if uses.is_empty() {
        if !matches!(node.kind, WorkflowNodeKind::Gate) {
            return Err(format!(
                "template {}@{} node `{}` must declare an explicit executor or control uses id",
                template.id, template.version, node.id
            ));
        }
        let inferred_policy = infer_gate_policy(node).ok_or_else(|| {
            format!(
                "template {}@{} node `{}` has empty uses and cannot infer a gate policy",
                template.id, template.version, node.id
            )
        })?;
        diagnostics.push(WorkflowDiagnostic {
            level: WorkflowDiagnosticLevel::Warning,
            node_id: node.id.clone(),
            message: format!(
                "gate policy inferred as `{inferred_policy}` from gate configuration; declare uses explicitly before {}",
                LEGACY_CAPABILITY_COMPATIBILITY_END_DATE
            ),
        });
        WorkflowNodeResolution {
            identity: WorkflowNodeIdentity {
                node_class: WorkflowNodeClass::Control,
                executor_class: None,
                executor_operation: None,
                control_policy: Some(inferred_policy.to_string()),
                legacy_capability_id: None,
                diagnostics: Vec::new(),
            },
            capability: None,
        }
    } else if let Some((executor_class, executor_operation)) = canonical_executor_operation(uses) {
        WorkflowNodeResolution {
            identity: WorkflowNodeIdentity {
                node_class: WorkflowNodeClass::Executor,
                executor_class: Some(executor_class),
                executor_operation: Some(executor_operation.to_string()),
                control_policy: None,
                legacy_capability_id: None,
                diagnostics: Vec::new(),
            },
            capability: None,
        }
    } else if let Some(control_policy) = canonical_control_policy(uses) {
        WorkflowNodeResolution {
            identity: WorkflowNodeIdentity {
                node_class: WorkflowNodeClass::Control,
                executor_class: None,
                executor_operation: None,
                control_policy: Some(control_policy.to_string()),
                legacy_capability_id: None,
                diagnostics: Vec::new(),
            },
            capability: None,
        }
    } else if let Some((executor_class, executor_operation, capability)) =
        legacy_executor_alias(uses)
    {
        diagnostics.push(WorkflowDiagnostic {
            level: WorkflowDiagnosticLevel::Warning,
            node_id: node.id.clone(),
            message: format!(
                "legacy uses label `{uses}` is deprecated; migrate to explicit executor ids before {}",
                LEGACY_CAPABILITY_COMPATIBILITY_END_DATE
            ),
        });
        WorkflowNodeResolution {
            identity: WorkflowNodeIdentity {
                node_class: WorkflowNodeClass::Executor,
                executor_class: Some(executor_class),
                executor_operation: Some(executor_operation.to_string()),
                control_policy: None,
                legacy_capability_id: capability.map(|capability| capability.id().to_string()),
                diagnostics: Vec::new(),
            },
            capability,
        }
    } else if let Some((control_policy, capability)) = legacy_control_alias(uses) {
        diagnostics.push(WorkflowDiagnostic {
            level: WorkflowDiagnosticLevel::Warning,
            node_id: node.id.clone(),
            message: format!(
                "legacy uses label `{uses}` is deprecated; migrate to explicit control ids before {}",
                LEGACY_CAPABILITY_COMPATIBILITY_END_DATE
            ),
        });
        WorkflowNodeResolution {
            identity: WorkflowNodeIdentity {
                node_class: WorkflowNodeClass::Control,
                executor_class: None,
                executor_operation: None,
                control_policy: Some(control_policy.to_string()),
                legacy_capability_id: capability.map(|capability| capability.id().to_string()),
                diagnostics: Vec::new(),
            },
            capability,
        }
    } else {
        return Err(format!(
            "template {}@{} node `{}` uses unknown label `{uses}`; declare one of cap.env.builtin.*, cap.env.shell.*, cap.agent.*, or control.*",
            template.id, template.version, node.id
        ));
    };

    match resolved.identity.node_class {
        WorkflowNodeClass::Executor => {
            let Some(executor_class) = resolved.identity.executor_class else {
                return Err(format!(
                    "template {}@{} node `{}` executor classification missing executor class",
                    template.id, template.version, node.id
                ));
            };
            let expected = match node.kind {
                WorkflowNodeKind::Builtin => Some(WorkflowExecutorClass::EnvironmentBuiltin),
                WorkflowNodeKind::Shell | WorkflowNodeKind::Custom => {
                    Some(WorkflowExecutorClass::EnvironmentShell)
                }
                WorkflowNodeKind::Agent => Some(WorkflowExecutorClass::Agent),
                WorkflowNodeKind::Gate => None,
            };
            let Some(expected_class) = expected else {
                return Err(format!(
                    "template {}@{} node `{}` is a gate node and cannot declare executor operation `{}`",
                    template.id,
                    template.version,
                    node.id,
                    resolved
                        .identity
                        .executor_operation
                        .as_deref()
                        .unwrap_or("<unknown>")
                ));
            };
            if executor_class != expected_class {
                return Err(format!(
                    "template {}@{} node `{}` declares executor class `{}` but kind `{:?}` requires `{}`",
                    template.id,
                    template.version,
                    node.id,
                    executor_class.as_str(),
                    node.kind,
                    expected_class.as_str()
                ));
            }
        }
        WorkflowNodeClass::Control => {
            if !matches!(node.kind, WorkflowNodeKind::Gate) {
                return Err(format!(
                    "template {}@{} node `{}` declares control policy `{}` but kind `{:?}` is not gate",
                    template.id,
                    template.version,
                    node.id,
                    resolved
                        .identity
                        .control_policy
                        .as_deref()
                        .unwrap_or("unknown"),
                    node.kind
                ));
            }
            if has_nonempty_arg(node, "command") || has_nonempty_arg(node, "script") {
                return Err(format!(
                    "template {}@{} node `{}` declares control policy `{}` but also sets command/script args",
                    template.id,
                    template.version,
                    node.id,
                    resolved
                        .identity
                        .control_policy
                        .as_deref()
                        .unwrap_or("unknown")
                ));
            }
        }
    }

    resolved.identity.diagnostics = diagnostics;
    Ok(resolved)
}

fn canonical_executor_operation(value: &str) -> Option<(WorkflowExecutorClass, &'static str)> {
    match value {
        "cap.env.builtin.prompt.resolve" => {
            Some((WorkflowExecutorClass::EnvironmentBuiltin, "prompt.resolve"))
        }
        "cap.env.builtin.worktree.prepare" => Some((
            WorkflowExecutorClass::EnvironmentBuiltin,
            "worktree.prepare",
        )),
        "cap.env.builtin.worktree.cleanup" => Some((
            WorkflowExecutorClass::EnvironmentBuiltin,
            "worktree.cleanup",
        )),
        "cap.env.builtin.plan.persist" => {
            Some((WorkflowExecutorClass::EnvironmentBuiltin, "plan.persist"))
        }
        "cap.env.builtin.git.stage_commit" => Some((
            WorkflowExecutorClass::EnvironmentBuiltin,
            "git.stage_commit",
        )),
        "cap.env.builtin.git.integrate_plan_branch" => Some((
            WorkflowExecutorClass::EnvironmentBuiltin,
            "git.integrate_plan_branch",
        )),
        "cap.env.builtin.git.save_worktree_patch" => Some((
            WorkflowExecutorClass::EnvironmentBuiltin,
            "git.save_worktree_patch",
        )),
        "cap.env.builtin.patch.pipeline_prepare" => Some((
            WorkflowExecutorClass::EnvironmentBuiltin,
            "patch.pipeline_prepare",
        )),
        "cap.env.builtin.patch.pipeline_finalize" => Some((
            WorkflowExecutorClass::EnvironmentBuiltin,
            "patch.pipeline_finalize",
        )),
        "cap.env.builtin.patch.execute_pipeline" => Some((
            WorkflowExecutorClass::EnvironmentBuiltin,
            "patch.execute_pipeline",
        )),
        "cap.env.builtin.build.materialize_step" => Some((
            WorkflowExecutorClass::EnvironmentBuiltin,
            "build.materialize_step",
        )),
        "cap.env.builtin.merge.sentinel.write" => Some((
            WorkflowExecutorClass::EnvironmentBuiltin,
            "merge.sentinel.write",
        )),
        "cap.env.builtin.merge.sentinel.clear" => Some((
            WorkflowExecutorClass::EnvironmentBuiltin,
            "merge.sentinel.clear",
        )),
        "cap.env.shell.prompt.resolve" => {
            Some((WorkflowExecutorClass::EnvironmentShell, "prompt.resolve"))
        }
        "cap.env.shell.command.run" => {
            Some((WorkflowExecutorClass::EnvironmentShell, "command.run"))
        }
        "cap.env.shell.cicd.run" => Some((WorkflowExecutorClass::EnvironmentShell, "cicd.run")),
        "cap.agent.invoke" => Some((WorkflowExecutorClass::Agent, "agent.invoke")),
        _ => None,
    }
}

fn canonical_control_policy(value: &str) -> Option<&'static str> {
    match value {
        "control.gate.stop_condition" => Some("gate.stop_condition"),
        "control.gate.conflict_resolution" => Some("gate.conflict_resolution"),
        "control.gate.cicd" => Some("gate.cicd"),
        "control.gate.approval" => Some("gate.approval"),
        "control.terminal" => Some("terminal"),
        _ => None,
    }
}

fn legacy_executor_alias(
    value: &str,
) -> Option<(
    WorkflowExecutorClass,
    &'static str,
    Option<WorkflowCapability>,
)> {
    match value {
        "cap.agent.plan.generate_draft" => {
            Some((WorkflowExecutorClass::Agent, "agent.invoke", None))
        }
        "cap.agent.plan.apply" => Some((WorkflowExecutorClass::Agent, "agent.invoke", None)),
        "cap.agent.review.critique_or_fix" => {
            Some((WorkflowExecutorClass::Agent, "agent.invoke", None))
        }
        "cap.agent.review.apply_fixes" => {
            Some((WorkflowExecutorClass::Agent, "agent.invoke", None))
        }
        "cap.agent.remediation.cicd_fix" => {
            Some((WorkflowExecutorClass::Agent, "agent.invoke", None))
        }
        "cap.agent.merge.resolve_conflict" => {
            Some((WorkflowExecutorClass::Agent, "agent.invoke", None))
        }
        "cap.git.save_worktree_patch" | "vizier.save.apply" => Some((
            WorkflowExecutorClass::EnvironmentBuiltin,
            "git.save_worktree_patch",
            Some(WorkflowCapability::GitSaveWorktreePatch),
        )),
        "cap.plan.generate_draft_plan" | "vizier.draft.generate_plan" => Some((
            WorkflowExecutorClass::EnvironmentBuiltin,
            "plan.generate_draft_plan",
            Some(WorkflowCapability::PlanGenerateDraftPlan),
        )),
        "cap.plan.apply_once" | "vizier.approve.apply_once" => Some((
            WorkflowExecutorClass::EnvironmentBuiltin,
            "plan.apply_once",
            Some(WorkflowCapability::PlanApplyOnce),
        )),
        "cap.review.critique_or_fix" | "vizier.review.critique" => Some((
            WorkflowExecutorClass::Agent,
            "agent.invoke",
            Some(WorkflowCapability::ReviewCritiqueOrFix),
        )),
        "cap.git.integrate_plan_branch" | "vizier.merge.integrate" => Some((
            WorkflowExecutorClass::EnvironmentBuiltin,
            "git.integrate_plan_branch",
            Some(WorkflowCapability::GitIntegratePlanBranch),
        )),
        "cap.patch.execute_pipeline" | "vizier.patch.execute" => Some((
            WorkflowExecutorClass::EnvironmentBuiltin,
            "patch.execute_pipeline",
            Some(WorkflowCapability::PatchExecutePipeline),
        )),
        "cap.build.materialize_step" | "vizier.build.materialize" => Some((
            WorkflowExecutorClass::EnvironmentBuiltin,
            "build.materialize_step",
            Some(WorkflowCapability::BuildMaterializeStep),
        )),
        "cap.remediation.cicd_auto_fix" | "vizier.merge.cicd_auto_fix" => Some((
            WorkflowExecutorClass::Agent,
            "agent.invoke",
            Some(WorkflowCapability::RemediationCicdAutoFix),
        )),
        "cap.exec.custom_command" => Some((
            WorkflowExecutorClass::EnvironmentShell,
            "command.run",
            Some(WorkflowCapability::ExecCustomCommand),
        )),
        "cap.review.apply_fixes_only" | "vizier.review.apply" => Some((
            WorkflowExecutorClass::Agent,
            "agent.invoke",
            Some(WorkflowCapability::ReviewApplyFixesOnly),
        )),
        _ => None,
    }
}

fn legacy_control_alias(value: &str) -> Option<(&'static str, Option<WorkflowCapability>)> {
    match value {
        "cap.gate.stop_condition" | "vizier.approve.stop_condition" => Some((
            "gate.stop_condition",
            Some(WorkflowCapability::GateStopCondition),
        )),
        "cap.gate.conflict_resolution" | "vizier.merge.conflict_resolution" => Some((
            "gate.conflict_resolution",
            Some(WorkflowCapability::GateConflictResolution),
        )),
        "cap.gate.cicd" | "vizier.merge.cicd_gate" => {
            Some(("gate.cicd", Some(WorkflowCapability::GateCicd)))
        }
        "cap.internal.terminal_sink" | "vizier.approve.terminal" | "vizier.merge.terminal" => {
            Some(("terminal", Some(WorkflowCapability::InternalTerminalSink)))
        }
        _ => None,
    }
}

fn infer_gate_policy(node: &WorkflowNode) -> Option<&'static str> {
    let has_script = script_gate_count(node) > 0;
    let has_cicd = cicd_gate_count(node) > 0;
    let has_approval = approval_gate_count(node) > 0;
    let active_kinds = [has_script, has_cicd, has_approval]
        .iter()
        .filter(|entry| **entry)
        .count();
    if active_kinds > 1 {
        return None;
    }
    if has_cicd {
        Some("gate.cicd")
    } else if has_script {
        Some("gate.stop_condition")
    } else if has_approval {
        Some("gate.approval")
    } else {
        Some("terminal")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledWorkflowNode {
    pub template_id: String,
    pub template_version: String,
    pub node_id: String,
    pub capability: Option<WorkflowCapability>,
    pub node_class: WorkflowNodeClass,
    pub executor_class: Option<WorkflowExecutorClass>,
    pub executor_operation: Option<String>,
    pub control_policy: Option<String>,
    pub diagnostics: Vec<WorkflowDiagnostic>,
    pub policy_snapshot_hash: String,
    pub policy_snapshot: WorkflowPolicySnapshot,
    pub after: Vec<JobAfterDependency>,
    pub dependencies: Vec<JobArtifact>,
    pub locks: Vec<JobLock>,
    pub artifacts: Vec<JobArtifact>,
    pub preconditions: Vec<WorkflowPrecondition>,
    pub gates: Vec<WorkflowGate>,
    pub retry: WorkflowRetryPolicy,
    pub on: WorkflowOutcomeEdges,
}

pub fn compile_workflow_node(
    template: &WorkflowTemplate,
    node_id: &str,
    resolved_after: &BTreeMap<String, String>,
) -> Result<CompiledWorkflowNode, String> {
    let node = template
        .nodes
        .iter()
        .find(|entry| entry.id == node_id)
        .ok_or_else(|| {
            format!(
                "template {}@{} does not define node `{}`",
                template.id, template.version, node_id
            )
        })?;
    validate_node_artifact_contracts(template, node)?;
    validate_node_outcome_edges(template, node)?;
    let resolved = classify_template_node(template, node)?;

    let mut after = Vec::new();
    for dependency in &node.after {
        if !template
            .nodes
            .iter()
            .any(|entry| entry.id == dependency.node_id)
        {
            return Err(format!(
                "template {}@{} node `{}` references unknown after node `{}`",
                template.id, template.version, node.id, dependency.node_id
            ));
        }
        let Some(job_id) = resolved_after.get(&dependency.node_id) else {
            return Err(format!(
                "template {}@{} node `{}` requires unresolved after node `{}`",
                template.id, template.version, node.id, dependency.node_id
            ));
        };
        after.push(JobAfterDependency {
            job_id: job_id.clone(),
            policy: dependency.policy,
        });
    }

    let policy_snapshot = template.policy_snapshot();
    let policy_snapshot_hash = policy_snapshot
        .stable_hash_hex()
        .map_err(|err| format!("serialize policy snapshot for {}: {}", template.id, err))?;

    Ok(CompiledWorkflowNode {
        template_id: template.id.clone(),
        template_version: template.version.clone(),
        node_id: node.id.clone(),
        capability: resolved
            .capability
            .or_else(|| workflow_node_capability(node)),
        node_class: resolved.identity.node_class,
        executor_class: resolved.identity.executor_class,
        executor_operation: resolved.identity.executor_operation,
        control_policy: resolved.identity.control_policy,
        diagnostics: resolved.identity.diagnostics,
        policy_snapshot_hash,
        policy_snapshot,
        after,
        dependencies: dedup_artifacts(node.needs.clone()),
        locks: dedup_locks(node.locks.clone()),
        artifacts: node.produces.all(),
        preconditions: dedup_preconditions(node.preconditions.clone()),
        gates: dedup_gates(node.gates.clone()),
        retry: node.retry.clone(),
        on: normalize_outcome_edges(&node.on),
    })
}

pub fn validate_workflow_capability_contracts(template: &WorkflowTemplate) -> Result<(), String> {
    let mut by_id = BTreeMap::new();
    let mut node_resolutions = BTreeMap::new();
    for node in &template.nodes {
        if by_id.insert(node.id.clone(), node).is_some() {
            return Err(format!(
                "template {}@{} defines duplicate node id `{}`",
                template.id, template.version, node.id
            ));
        }
        let resolution = classify_template_node(template, node)?;
        node_resolutions.insert(node.id.clone(), resolution);
    }

    let cicd_gate_nodes = node_resolutions
        .iter()
        .filter(|(_id, resolution)| {
            matches!(
                resolution.identity.control_policy.as_deref(),
                Some("gate.cicd")
            )
        })
        .map(|(id, _)| id.as_str())
        .collect::<Vec<_>>();
    if cicd_gate_nodes.len() > 1 {
        return Err(format!(
            "template {}@{} capability `{}` nodes [{}] violate single-gate cardinality",
            template.id,
            template.version,
            "gate.cicd",
            cicd_gate_nodes.join(", ")
        ));
    }

    for node in &template.nodes {
        let resolution = node_resolutions.get(&node.id).ok_or_else(|| {
            format!(
                "template {}@{} missing classifier state for node `{}`",
                template.id, template.version, node.id
            )
        })?;

        match resolution.identity.executor_operation.as_deref() {
            Some("agent.invoke") => {
                let enforce_prompt_dependency = node.uses.trim() == "cap.agent.invoke";
                validate_agent_invoke_contract(template, node, enforce_prompt_dependency)?;
            }
            Some("prompt.resolve") => {
                if let Some(executor_class) = resolution.identity.executor_class {
                    validate_prompt_resolve_contract(template, node, executor_class)?;
                }
            }
            _ => {}
        }

        let capability = resolution
            .capability
            .or_else(|| workflow_node_capability(node));
        let Some(capability) = capability else {
            continue;
        };

        match capability {
            WorkflowCapability::PlanApplyOnce => {
                validate_plan_apply_once_contract(template, &by_id, node)?;
            }
            WorkflowCapability::GateStopCondition => {
                validate_stop_condition_contract(template, &by_id, node)?;
            }
            WorkflowCapability::GitIntegratePlanBranch => {
                validate_integrate_plan_branch_contract(template, &by_id, node)?;
            }
            WorkflowCapability::GateConflictResolution => {
                validate_conflict_resolution_contract(template, &by_id, node)?;
            }
            WorkflowCapability::GateCicd => {
                validate_cicd_gate_contract(template, &by_id, node)?;
            }
            WorkflowCapability::RemediationCicdAutoFix => {
                validate_cicd_remediation_contract(template, &by_id, node)?;
            }
            WorkflowCapability::ReviewCritiqueOrFix | WorkflowCapability::ReviewApplyFixesOnly => {
                validate_review_contract(template, node, capability)?;
            }
            WorkflowCapability::ExecCustomCommand => {
                validate_exec_custom_command_contract(template, node)?;
            }
            WorkflowCapability::GitSaveWorktreePatch => {
                validate_save_worktree_patch_contract(template, node)?;
            }
            WorkflowCapability::PlanGenerateDraftPlan => {
                validate_generate_draft_plan_contract(template, node)?;
            }
            WorkflowCapability::PatchExecutePipeline => {
                validate_patch_execute_contract(template, node)?;
            }
            WorkflowCapability::BuildMaterializeStep => {
                validate_build_materialize_contract(template, node)?;
            }
            WorkflowCapability::InternalTerminalSink => {}
        }
    }

    Ok(())
}

fn validate_agent_invoke_contract(
    template: &WorkflowTemplate,
    node: &WorkflowNode,
    enforce_prompt_dependency: bool,
) -> Result<(), String> {
    if !matches!(node.kind, WorkflowNodeKind::Agent) {
        return Err(executor_contract_error(
            template,
            "agent.invoke",
            node,
            &format!("uses kind {:?}; expected agent", node.kind),
        ));
    }
    if has_nonempty_arg(node, "command") || has_nonempty_arg(node, "script") {
        return Err(executor_contract_error(
            template,
            "agent.invoke",
            node,
            "must not declare args.command or args.script; runtime command comes from config",
        ));
    }
    if !enforce_prompt_dependency {
        return Ok(());
    }

    let prompt_dependency_count = node
        .needs
        .iter()
        .filter(|artifact| is_prompt_artifact(artifact))
        .count();
    if prompt_dependency_count != 1 || node.needs.len() != 1 {
        return Err(executor_contract_error(
            template,
            "agent.invoke",
            node,
            "requires exactly one prompt artifact dependency (`custom:prompt_text:<key>`)",
        ));
    }

    Ok(())
}

fn validate_prompt_resolve_contract(
    template: &WorkflowTemplate,
    node: &WorkflowNode,
    executor_class: WorkflowExecutorClass,
) -> Result<(), String> {
    match executor_class {
        WorkflowExecutorClass::EnvironmentBuiltin => {
            if !matches!(node.kind, WorkflowNodeKind::Builtin) {
                return Err(executor_contract_error(
                    template,
                    "prompt.resolve",
                    node,
                    &format!("uses kind {:?}; expected builtin", node.kind),
                ));
            }
            if has_nonempty_arg(node, "command") || has_nonempty_arg(node, "script") {
                return Err(executor_contract_error(
                    template,
                    "prompt.resolve",
                    node,
                    "builtin prompt resolve must not declare args.command or args.script",
                ));
            }
            validate_prompt_resolve_output_contract(template, node)
        }
        WorkflowExecutorClass::EnvironmentShell => {
            if !matches!(
                node.kind,
                WorkflowNodeKind::Shell | WorkflowNodeKind::Custom
            ) {
                return Err(executor_contract_error(
                    template,
                    "prompt.resolve",
                    node,
                    &format!("uses kind {:?}; expected shell/custom", node.kind),
                ));
            }
            let has_command = has_nonempty_arg(node, "command");
            let has_script = has_nonempty_arg(node, "script");
            if has_command == has_script {
                return Err(executor_contract_error(
                    template,
                    "prompt.resolve",
                    node,
                    "shell prompt resolve requires exactly one of args.command or args.script",
                ));
            }
            validate_prompt_resolve_output_contract(template, node)
        }
        WorkflowExecutorClass::Agent => Err(executor_contract_error(
            template,
            "prompt.resolve",
            node,
            "agent executor class is invalid for prompt.resolve",
        )),
    }
}

fn validate_prompt_resolve_output_contract(
    template: &WorkflowTemplate,
    node: &WorkflowNode,
) -> Result<(), String> {
    let artifacts = node.produces.all();
    if artifacts.len() != 1 || !is_prompt_artifact(&artifacts[0]) {
        return Err(executor_contract_error(
            template,
            "prompt.resolve",
            node,
            "must produce exactly one prompt artifact (`custom:prompt_text:<key>`)",
        ));
    }
    Ok(())
}

fn is_prompt_artifact(artifact: &JobArtifact) -> bool {
    matches!(
        artifact,
        JobArtifact::Custom { type_id, .. } if type_id == PROMPT_ARTIFACT_TYPE_ID
    )
}

fn validate_plan_apply_once_contract(
    template: &WorkflowTemplate,
    by_id: &BTreeMap<String, &WorkflowNode>,
    node: &WorkflowNode,
) -> Result<(), String> {
    if script_gate_count(node) > 1 {
        return Err(capability_error(
            template,
            WorkflowCapability::PlanApplyOnce,
            node,
            "defines multiple script gates; expected at most one",
        ));
    }

    let stop_targets =
        resolve_outcome_targets(template, by_id, node, "succeeded", &node.on.succeeded)?
            .into_iter()
            .filter(|target| {
                target.id == "approve_gate_stop_condition"
                    || workflow_node_capability(target)
                        == Some(WorkflowCapability::GateStopCondition)
                    || (matches!(target.kind, WorkflowNodeKind::Gate)
                        && script_gate_count(target) > 0)
            })
            .collect::<Vec<_>>();

    if stop_targets.len() > 1 {
        let ids = stop_targets
            .iter()
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(capability_error(
            template,
            WorkflowCapability::PlanApplyOnce,
            node,
            &format!("has ambiguous stop-condition targets on on.succeeded: {ids}"),
        ));
    }

    if !node.on.succeeded.is_empty() && stop_targets.is_empty() {
        return Err(capability_error(
            template,
            WorkflowCapability::PlanApplyOnce,
            node,
            "has on.succeeded targets but none resolve to a stop-condition gate",
        ));
    }

    let Some(stop_node) = stop_targets.first().copied() else {
        return Ok(());
    };
    if !matches!(stop_node.kind, WorkflowNodeKind::Gate) {
        return Err(capability_error(
            template,
            WorkflowCapability::PlanApplyOnce,
            node,
            &format!(
                "targets stop-condition node `{}` with kind {:?}; expected gate",
                stop_node.id, stop_node.kind
            ),
        ));
    }

    if script_gate_count(stop_node) > 1 {
        return Err(capability_error(
            template,
            WorkflowCapability::PlanApplyOnce,
            node,
            &format!(
                "stop-condition node `{}` defines multiple script gates",
                stop_node.id
            ),
        ));
    }

    if script_gate_count(stop_node) == 1
        && matches!(stop_node.retry.mode, WorkflowRetryMode::UntilGate)
        && !stop_node.on.failed.iter().any(|target| target == &node.id)
    {
        return Err(capability_error(
            template,
            WorkflowCapability::PlanApplyOnce,
            node,
            &format!(
                "stop-condition node `{}` has until_gate retry but on.failed does not route back to `{}`",
                stop_node.id, node.id
            ),
        ));
    }

    Ok(())
}

fn validate_stop_condition_contract(
    template: &WorkflowTemplate,
    by_id: &BTreeMap<String, &WorkflowNode>,
    node: &WorkflowNode,
) -> Result<(), String> {
    if !matches!(node.kind, WorkflowNodeKind::Gate) {
        return Err(capability_error(
            template,
            WorkflowCapability::GateStopCondition,
            node,
            &format!("uses kind {:?}; expected gate", node.kind),
        ));
    }
    if script_gate_count(node) > 1 {
        return Err(capability_error(
            template,
            WorkflowCapability::GateStopCondition,
            node,
            "defines multiple script gates; expected at most one",
        ));
    }

    let parents = template
        .nodes
        .iter()
        .filter(|candidate| {
            workflow_node_capability(candidate) == Some(WorkflowCapability::PlanApplyOnce)
                && candidate
                    .on
                    .succeeded
                    .iter()
                    .any(|target| target == &node.id)
        })
        .collect::<Vec<_>>();
    if parents.is_empty() {
        return Err(capability_error(
            template,
            WorkflowCapability::GateStopCondition,
            node,
            "is not targeted from any cap.plan.apply_once on.succeeded edge",
        ));
    }
    if parents.len() > 1 {
        let ids = parents
            .iter()
            .map(|parent| parent.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(capability_error(
            template,
            WorkflowCapability::GateStopCondition,
            node,
            &format!("is targeted by multiple apply nodes: {ids}"),
        ));
    }

    if script_gate_count(node) == 1
        && matches!(node.retry.mode, WorkflowRetryMode::UntilGate)
        && !node.on.failed.iter().any(|target| target == &parents[0].id)
    {
        return Err(capability_error(
            template,
            WorkflowCapability::GateStopCondition,
            node,
            &format!(
                "has until_gate retry but on.failed does not route back to `{}`",
                parents[0].id
            ),
        ));
    }

    let _ = resolve_outcome_targets(template, by_id, node, "failed", &node.on.failed)?;
    Ok(())
}

fn validate_integrate_plan_branch_contract(
    template: &WorkflowTemplate,
    by_id: &BTreeMap<String, &WorkflowNode>,
    node: &WorkflowNode,
) -> Result<(), String> {
    if cicd_gate_count(node) > 1 {
        return Err(capability_error(
            template,
            WorkflowCapability::GitIntegratePlanBranch,
            node,
            "defines multiple cicd gates; expected at most one",
        ));
    }

    let conflict_targets =
        resolve_outcome_targets(template, by_id, node, "blocked", &node.on.blocked)?
            .into_iter()
            .filter(|target| {
                target.id == "merge_conflict_resolution"
                    || workflow_node_capability(target)
                        == Some(WorkflowCapability::GateConflictResolution)
                    || node_has_custom_gate(target, "conflict_resolution")
            })
            .collect::<Vec<_>>();
    if conflict_targets.len() > 1 {
        let ids = conflict_targets
            .iter()
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(capability_error(
            template,
            WorkflowCapability::GitIntegratePlanBranch,
            node,
            &format!("has ambiguous conflict-resolution targets on on.blocked: {ids}"),
        ));
    }
    if let Some(conflict_target) = conflict_targets.first().copied()
        && !matches!(conflict_target.kind, WorkflowNodeKind::Gate)
    {
        return Err(capability_error(
            template,
            WorkflowCapability::GitIntegratePlanBranch,
            node,
            &format!(
                "targets conflict-resolution node `{}` with kind {:?}; expected gate",
                conflict_target.id, conflict_target.kind
            ),
        ));
    }

    let cicd_targets =
        resolve_outcome_targets(template, by_id, node, "succeeded", &node.on.succeeded)?
            .into_iter()
            .filter(|target| {
                target.id == "merge_gate_cicd"
                    || workflow_node_capability(target) == Some(WorkflowCapability::GateCicd)
                    || cicd_gate_count(target) > 0
            })
            .collect::<Vec<_>>();
    if cicd_targets.len() > 1 {
        let ids = cicd_targets
            .iter()
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(capability_error(
            template,
            WorkflowCapability::GitIntegratePlanBranch,
            node,
            &format!("has ambiguous cicd gate targets on on.succeeded: {ids}"),
        ));
    }
    if let Some(gate_target) = cicd_targets.first().copied() {
        if !matches!(gate_target.kind, WorkflowNodeKind::Gate) {
            return Err(capability_error(
                template,
                WorkflowCapability::GitIntegratePlanBranch,
                node,
                &format!(
                    "targets cicd node `{}` with kind {:?}; expected gate",
                    gate_target.id, gate_target.kind
                ),
            ));
        }
        if cicd_gate_count(gate_target) != 1 {
            return Err(capability_error(
                template,
                WorkflowCapability::GitIntegratePlanBranch,
                node,
                &format!(
                    "cicd target node `{}` must define exactly one cicd gate",
                    gate_target.id
                ),
            ));
        }
        if let Some((_script, auto_resolve)) = single_cicd_gate(gate_target)
            && auto_resolve
            && !gate_retry_loop_closes(template, by_id, gate_target)?
        {
            return Err(capability_error(
                template,
                WorkflowCapability::GitIntegratePlanBranch,
                node,
                &format!(
                    "cicd gate node `{}` enables auto_resolve but on.failed does not return to the gate",
                    gate_target.id
                ),
            ));
        }
    }

    Ok(())
}

fn validate_conflict_resolution_contract(
    template: &WorkflowTemplate,
    by_id: &BTreeMap<String, &WorkflowNode>,
    node: &WorkflowNode,
) -> Result<(), String> {
    if !matches!(node.kind, WorkflowNodeKind::Gate) {
        return Err(capability_error(
            template,
            WorkflowCapability::GateConflictResolution,
            node,
            &format!("uses kind {:?}; expected gate", node.kind),
        ));
    }

    let integrate_parents = template
        .nodes
        .iter()
        .filter(|candidate| {
            workflow_node_capability(candidate) == Some(WorkflowCapability::GitIntegratePlanBranch)
                && candidate.on.blocked.iter().any(|target| target == &node.id)
        })
        .collect::<Vec<_>>();
    if integrate_parents.is_empty() {
        return Err(capability_error(
            template,
            WorkflowCapability::GateConflictResolution,
            node,
            "is not targeted from any cap.git.integrate_plan_branch on.blocked edge",
        ));
    }
    if integrate_parents.len() > 1 {
        let ids = integrate_parents
            .iter()
            .map(|parent| parent.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(capability_error(
            template,
            WorkflowCapability::GateConflictResolution,
            node,
            &format!("is targeted by multiple integrate nodes: {ids}"),
        ));
    }

    let auto_resolve = conflict_auto_resolve_enabled(template, node)?;
    if auto_resolve
        && !node
            .on
            .succeeded
            .iter()
            .any(|target| target == &integrate_parents[0].id)
    {
        return Err(capability_error(
            template,
            WorkflowCapability::GateConflictResolution,
            node,
            &format!(
                "enables auto_resolve but on.succeeded does not route back to `{}`",
                integrate_parents[0].id
            ),
        ));
    }

    let _ = resolve_outcome_targets(template, by_id, node, "succeeded", &node.on.succeeded)?;
    Ok(())
}

fn validate_cicd_gate_contract(
    template: &WorkflowTemplate,
    by_id: &BTreeMap<String, &WorkflowNode>,
    node: &WorkflowNode,
) -> Result<(), String> {
    if !matches!(node.kind, WorkflowNodeKind::Gate) {
        return Err(capability_error(
            template,
            WorkflowCapability::GateCicd,
            node,
            &format!("uses kind {:?}; expected gate", node.kind),
        ));
    }
    if cicd_gate_count(node) != 1 {
        return Err(capability_error(
            template,
            WorkflowCapability::GateCicd,
            node,
            "must define exactly one cicd gate",
        ));
    }

    if let Some((_script, auto_resolve)) = single_cicd_gate(node)
        && auto_resolve
        && !gate_retry_loop_closes(template, by_id, node)?
    {
        return Err(capability_error(
            template,
            WorkflowCapability::GateCicd,
            node,
            "enables auto_resolve but on.failed does not return to the gate",
        ));
    }

    Ok(())
}

fn validate_cicd_remediation_contract(
    template: &WorkflowTemplate,
    by_id: &BTreeMap<String, &WorkflowNode>,
    node: &WorkflowNode,
) -> Result<(), String> {
    let targets = resolve_outcome_targets(template, by_id, node, "succeeded", &node.on.succeeded)?
        .into_iter()
        .filter(|target| {
            target.id == "merge_gate_cicd"
                || workflow_node_capability(target) == Some(WorkflowCapability::GateCicd)
        })
        .collect::<Vec<_>>();
    if targets.is_empty() {
        return Err(capability_error(
            template,
            WorkflowCapability::RemediationCicdAutoFix,
            node,
            "must route on.succeeded back to exactly one cicd gate node",
        ));
    }
    if targets.len() > 1 {
        let ids = targets
            .iter()
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(capability_error(
            template,
            WorkflowCapability::RemediationCicdAutoFix,
            node,
            &format!("routes on.succeeded to multiple cicd gate nodes: {ids}"),
        ));
    }
    if !matches!(targets[0].kind, WorkflowNodeKind::Gate) {
        return Err(capability_error(
            template,
            WorkflowCapability::RemediationCicdAutoFix,
            node,
            &format!(
                "routes on.succeeded to `{}` with kind {:?}; expected gate",
                targets[0].id, targets[0].kind
            ),
        ));
    }
    Ok(())
}

fn validate_review_contract(
    template: &WorkflowTemplate,
    node: &WorkflowNode,
    capability: WorkflowCapability,
) -> Result<(), String> {
    if cicd_gate_count(node) > 1 {
        return Err(capability_error(
            template,
            capability,
            node,
            "defines multiple cicd gates; expected at most one",
        ));
    }
    Ok(())
}

fn validate_exec_custom_command_contract(
    template: &WorkflowTemplate,
    node: &WorkflowNode,
) -> Result<(), String> {
    let has_command = has_nonempty_arg(node, "command") || has_nonempty_arg(node, "script");
    if matches!(
        node.kind,
        WorkflowNodeKind::Shell | WorkflowNodeKind::Custom
    ) {
        if !has_command {
            return Err(capability_error(
                template,
                WorkflowCapability::ExecCustomCommand,
                node,
                "requires args.command or args.script",
            ));
        }
        return Ok(());
    }

    if !has_command {
        return Err(capability_error(
            template,
            WorkflowCapability::ExecCustomCommand,
            node,
            "requires args.command or args.script for non-shell/custom node kinds",
        ));
    }
    Ok(())
}

fn validate_save_worktree_patch_contract(
    template: &WorkflowTemplate,
    node: &WorkflowNode,
) -> Result<(), String> {
    if !matches!(node.kind, WorkflowNodeKind::Builtin) {
        return Err(capability_error(
            template,
            WorkflowCapability::GitSaveWorktreePatch,
            node,
            &format!(
                "uses kind {:?}; expected builtin for scheduled save-worktree execution",
                node.kind
            ),
        ));
    }
    Ok(())
}

fn validate_generate_draft_plan_contract(
    template: &WorkflowTemplate,
    node: &WorkflowNode,
) -> Result<(), String> {
    let has_spec_text = has_nonempty_arg(node, "spec_text");
    let has_spec_file = has_nonempty_arg(node, "spec_file");
    if !has_spec_text && !has_spec_file {
        return Err(capability_error(
            template,
            WorkflowCapability::PlanGenerateDraftPlan,
            node,
            "requires spec_text or spec_file",
        ));
    }

    let source = node
        .args
        .get("spec_source")
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_else(|| "inline".to_string());
    match source.as_str() {
        "inline" | "stdin" => Ok(()),
        "file" => {
            if has_spec_file {
                Ok(())
            } else {
                Err(capability_error(
                    template,
                    WorkflowCapability::PlanGenerateDraftPlan,
                    node,
                    "has spec_source=file but no spec_file argument",
                ))
            }
        }
        _ => Err(capability_error(
            template,
            WorkflowCapability::PlanGenerateDraftPlan,
            node,
            &format!("has unsupported spec_source `{source}`"),
        )),
    }
}

fn validate_patch_execute_contract(
    template: &WorkflowTemplate,
    node: &WorkflowNode,
) -> Result<(), String> {
    let files_json = node.args.get("files_json").ok_or_else(|| {
        capability_error(
            template,
            WorkflowCapability::PatchExecutePipeline,
            node,
            "requires files_json",
        )
    })?;
    let files: Vec<String> = serde_json::from_str(files_json).map_err(|err| {
        capability_error(
            template,
            WorkflowCapability::PatchExecutePipeline,
            node,
            &format!("has invalid files_json payload: {err}"),
        )
    })?;
    if files.is_empty() {
        return Err(capability_error(
            template,
            WorkflowCapability::PatchExecutePipeline,
            node,
            "requires at least one patch file in files_json",
        ));
    }
    Ok(())
}

fn validate_build_materialize_contract(
    template: &WorkflowTemplate,
    node: &WorkflowNode,
) -> Result<(), String> {
    let required = ["build_id", "step_key", "slug", "branch", "target"];
    let present = required
        .iter()
        .filter(|key| has_nonempty_arg(node, key))
        .copied()
        .collect::<Vec<_>>();
    if present.is_empty() {
        return Ok(());
    }
    if present.len() == required.len() {
        return Ok(());
    }

    let missing = required
        .iter()
        .filter(|key| !present.contains(key))
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    Err(capability_error(
        template,
        WorkflowCapability::BuildMaterializeStep,
        node,
        &format!("provides partial runtime arguments; missing required keys: {missing}"),
    ))
}

fn has_nonempty_arg(node: &WorkflowNode, key: &str) -> bool {
    node.args
        .get(key)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn executor_contract_error(
    template: &WorkflowTemplate,
    operation: &str,
    node: &WorkflowNode,
    detail: &str,
) -> String {
    format!(
        "template {}@{} executor `{}` node `{}` {}",
        template.id, template.version, operation, node.id, detail
    )
}

fn capability_error(
    template: &WorkflowTemplate,
    capability: WorkflowCapability,
    node: &WorkflowNode,
    detail: &str,
) -> String {
    format!(
        "template {}@{} capability `{}` node `{}` {}",
        template.id,
        template.version,
        capability.id(),
        node.id,
        detail
    )
}

fn resolve_outcome_targets<'a>(
    template: &WorkflowTemplate,
    by_id: &BTreeMap<String, &'a WorkflowNode>,
    node: &WorkflowNode,
    outcome: &str,
    targets: &[String],
) -> Result<Vec<&'a WorkflowNode>, String> {
    let mut resolved = Vec::new();
    for target in targets {
        let Some(target_node) = by_id.get(target).copied() else {
            return Err(format!(
                "template {}@{} node `{}` references unknown on.{} target `{}`",
                template.id, template.version, node.id, outcome, target
            ));
        };
        resolved.push(target_node);
    }
    Ok(resolved)
}

fn script_gate_count(node: &WorkflowNode) -> usize {
    node.gates
        .iter()
        .filter(|gate| matches!(gate, WorkflowGate::Script { .. }))
        .count()
}

fn cicd_gate_count(node: &WorkflowNode) -> usize {
    node.gates
        .iter()
        .filter(|gate| matches!(gate, WorkflowGate::Cicd { .. }))
        .count()
}

fn approval_gate_count(node: &WorkflowNode) -> usize {
    node.gates
        .iter()
        .filter(|gate| matches!(gate, WorkflowGate::Approval { .. }))
        .count()
}

fn single_cicd_gate(node: &WorkflowNode) -> Option<(String, bool)> {
    let gates = node
        .gates
        .iter()
        .filter_map(|gate| match gate {
            WorkflowGate::Cicd {
                script,
                auto_resolve,
                ..
            } => Some((script.clone(), *auto_resolve)),
            _ => None,
        })
        .collect::<Vec<_>>();
    if gates.len() == 1 {
        gates.into_iter().next()
    } else {
        None
    }
}

fn node_has_custom_gate(node: &WorkflowNode, gate_id: &str) -> bool {
    node.gates
        .iter()
        .any(|gate| matches!(gate, WorkflowGate::Custom { id, .. } if id == gate_id))
}

fn conflict_auto_resolve_enabled(
    template: &WorkflowTemplate,
    node: &WorkflowNode,
) -> Result<bool, String> {
    if let Some(raw) = node.args.get("auto_resolve")
        && let Some(value) = parse_bool_like(raw)
    {
        return Ok(value);
    }
    let matches = node
        .gates
        .iter()
        .filter_map(|gate| match gate {
            WorkflowGate::Custom { id, args, .. } if id == "conflict_resolution" => {
                Some(args.get("auto_resolve").cloned())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Err(capability_error(
            template,
            WorkflowCapability::GateConflictResolution,
            node,
            "defines multiple custom gates `conflict_resolution`; expected at most one",
        ));
    }
    Ok(matches
        .into_iter()
        .next()
        .flatten()
        .as_deref()
        .and_then(parse_bool_like)
        .unwrap_or(false))
}

fn parse_bool_like(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn gate_retry_loop_closes(
    template: &WorkflowTemplate,
    by_id: &BTreeMap<String, &WorkflowNode>,
    gate_node: &WorkflowNode,
) -> Result<bool, String> {
    if gate_node
        .on
        .failed
        .iter()
        .any(|target| target == &gate_node.id)
    {
        return Ok(true);
    }
    let failed_targets =
        resolve_outcome_targets(template, by_id, gate_node, "failed", &gate_node.on.failed)?;
    Ok(failed_targets
        .into_iter()
        .any(|target| target.on.succeeded.iter().any(|next| next == &gate_node.id)))
}

fn validate_node_artifact_contracts(
    template: &WorkflowTemplate,
    node: &WorkflowNode,
) -> Result<(), String> {
    let mut declared = BTreeMap::new();
    for contract in &template.artifact_contracts {
        let compiled_schema = if let Some(schema) = contract.schema.as_ref() {
            Some(compile_artifact_schema(schema).map_err(|err| {
                format!(
                    "template {}@{} artifact contract `{}` has invalid schema: {}",
                    template.id, template.version, contract.id, err
                )
            })?)
        } else {
            None
        };
        declared.insert(contract.id.clone(), compiled_schema);
    }

    for artifact in node
        .needs
        .iter()
        .chain(node.produces.succeeded.iter())
        .chain(node.produces.failed.iter())
        .chain(node.produces.blocked.iter())
        .chain(node.produces.cancelled.iter())
    {
        let contract_id = artifact_contract_id(artifact);
        let Some(compiled_schema) = declared.get(contract_id.as_str()) else {
            return Err(format!(
                "template {}@{} node `{}` references artifact `{}` without declared artifact contract `{}`",
                template.id,
                template.version,
                node.id,
                format_artifact(artifact),
                contract_id
            ));
        };

        if let Some(schema) = compiled_schema {
            let payload = artifact_contract_payload(artifact);
            if let Err(detail) = validate_artifact_schema(schema, &payload, "$") {
                return Err(format!(
                    "template {}@{} node `{}` artifact `{}` violates schema for contract `{}`: {}",
                    template.id,
                    template.version,
                    node.id,
                    format_artifact(artifact),
                    contract_id,
                    detail
                ));
            }
        }
    }
    Ok(())
}

fn validate_node_outcome_edges(
    template: &WorkflowTemplate,
    node: &WorkflowNode,
) -> Result<(), String> {
    let known = template
        .nodes
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<BTreeSet<_>>();

    for (outcome, targets) in [
        ("succeeded", &node.on.succeeded),
        ("failed", &node.on.failed),
        ("blocked", &node.on.blocked),
        ("cancelled", &node.on.cancelled),
    ] {
        for target in targets {
            if known.contains(target.as_str()) {
                continue;
            }
            return Err(format!(
                "template {}@{} node `{}` references unknown on.{} target `{}`",
                template.id, template.version, node.id, outcome, target
            ));
        }
    }

    Ok(())
}

fn artifact_contract_id(artifact: &JobArtifact) -> String {
    match artifact {
        JobArtifact::PlanBranch { .. } => "plan_branch".to_string(),
        JobArtifact::PlanDoc { .. } => "plan_doc".to_string(),
        JobArtifact::PlanCommits { .. } => "plan_commits".to_string(),
        JobArtifact::TargetBranch { .. } => "target_branch".to_string(),
        JobArtifact::MergeSentinel { .. } => "merge_sentinel".to_string(),
        JobArtifact::CommandPatch { .. } => "command_patch".to_string(),
        JobArtifact::Custom { type_id, .. } => type_id.clone(),
    }
}

fn artifact_contract_payload(artifact: &JobArtifact) -> JsonValue {
    match artifact {
        JobArtifact::PlanBranch { slug, branch }
        | JobArtifact::PlanDoc { slug, branch }
        | JobArtifact::PlanCommits { slug, branch } => serde_json::json!({
            "slug": slug,
            "branch": branch,
        }),
        JobArtifact::TargetBranch { name } => serde_json::json!({
            "name": name,
        }),
        JobArtifact::MergeSentinel { slug } => serde_json::json!({
            "slug": slug,
        }),
        JobArtifact::CommandPatch { job_id } => serde_json::json!({
            "job_id": job_id,
        }),
        JobArtifact::Custom { type_id, key } => serde_json::json!({
            "type_id": type_id,
            "key": key,
        }),
    }
}

#[derive(Debug, Clone)]
enum CompiledArtifactSchema {
    Bool(bool),
    Object(CompiledArtifactSchemaObject),
}

#[derive(Debug, Clone, Default)]
struct CompiledArtifactSchemaObject {
    expected_type: Option<ArtifactSchemaType>,
    required: Vec<String>,
    properties: BTreeMap<String, CompiledArtifactSchema>,
    additional_properties: Option<bool>,
    const_value: Option<JsonValue>,
    enum_values: Option<Vec<JsonValue>>,
    pattern: Option<ArtifactSchemaPattern>,
}

#[derive(Debug, Clone, Copy)]
enum ArtifactSchemaType {
    Object,
    Array,
    String,
    Number,
    Integer,
    Boolean,
    Null,
}

impl ArtifactSchemaType {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "object" => Some(Self::Object),
            "array" => Some(Self::Array),
            "string" => Some(Self::String),
            "number" => Some(Self::Number),
            "integer" => Some(Self::Integer),
            "boolean" => Some(Self::Boolean),
            "null" => Some(Self::Null),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::Array => "array",
            Self::String => "string",
            Self::Number => "number",
            Self::Integer => "integer",
            Self::Boolean => "boolean",
            Self::Null => "null",
        }
    }

    fn matches(self, value: &JsonValue) -> bool {
        match self {
            Self::Object => value.is_object(),
            Self::Array => value.is_array(),
            Self::String => value.is_string(),
            Self::Number => value.is_number(),
            Self::Integer => value.as_i64().is_some() || value.as_u64().is_some(),
            Self::Boolean => value.is_boolean(),
            Self::Null => value.is_null(),
        }
    }
}

#[derive(Debug, Clone)]
enum ArtifactSchemaPattern {
    Exact(String),
    OneOf(Vec<String>),
}

impl ArtifactSchemaPattern {
    fn compile(raw: &str) -> Result<Self, String> {
        if raw.starts_with("^(") && raw.ends_with(")$") {
            let body = &raw[2..raw.len() - 2];
            if body.is_empty() {
                return Err("keyword `pattern` cannot have an empty alternation".to_string());
            }
            let mut options = Vec::new();
            for token in body.split('|') {
                if token.is_empty() {
                    return Err("keyword `pattern` contains an empty alternation token".to_string());
                }
                if contains_pattern_meta(token) {
                    return Err(
                        "keyword `pattern` only supports plain literal alternation tokens"
                            .to_string(),
                    );
                }
                options.push(token.to_string());
            }
            return Ok(Self::OneOf(options));
        }

        if raw.starts_with('^') && raw.ends_with('$') {
            let literal = &raw[1..raw.len() - 1];
            if contains_pattern_meta(literal) {
                return Err(
                    "keyword `pattern` only supports anchored plain literals or anchored literal alternation"
                        .to_string(),
                );
            }
            return Ok(Self::Exact(literal.to_string()));
        }

        Err(
            "keyword `pattern` only supports anchored plain literals or anchored literal alternation"
                .to_string(),
        )
    }

    fn matches(&self, value: &str) -> bool {
        match self {
            Self::Exact(expected) => value == expected,
            Self::OneOf(options) => options.iter().any(|option| option == value),
        }
    }
}

fn contains_pattern_meta(value: &str) -> bool {
    value.chars().any(|ch| {
        matches!(
            ch,
            '.' | '*' | '+' | '?' | '[' | ']' | '{' | '}' | '(' | ')' | '|' | '\\' | '^' | '$'
        )
    })
}

fn compile_artifact_schema(schema: &JsonValue) -> Result<CompiledArtifactSchema, String> {
    match schema {
        JsonValue::Bool(value) => Ok(CompiledArtifactSchema::Bool(*value)),
        JsonValue::Object(map) => {
            let expected_type = match map.get("type") {
                None => None,
                Some(JsonValue::String(raw)) => Some(
                    ArtifactSchemaType::parse(raw)
                        .ok_or_else(|| format!("keyword `type` has unsupported value `{raw}`"))?,
                ),
                Some(_) => return Err("keyword `type` must be a string".to_string()),
            };

            let required = match map.get("required") {
                None => Vec::new(),
                Some(JsonValue::Array(entries)) => {
                    let mut required = Vec::new();
                    for entry in entries {
                        let field = entry.as_str().ok_or_else(|| {
                            "keyword `required` must be an array of strings".to_string()
                        })?;
                        required.push(field.to_string());
                    }
                    required
                }
                Some(_) => return Err("keyword `required` must be an array".to_string()),
            };

            let properties = match map.get("properties") {
                None => BTreeMap::new(),
                Some(JsonValue::Object(entries)) => {
                    let mut compiled = BTreeMap::new();
                    for (name, child_schema) in entries {
                        compiled.insert(name.clone(), compile_artifact_schema(child_schema)?);
                    }
                    compiled
                }
                Some(_) => return Err("keyword `properties` must be an object".to_string()),
            };

            let additional_properties = match map.get("additionalProperties") {
                None => None,
                Some(JsonValue::Bool(value)) => Some(*value),
                Some(_) => {
                    return Err("keyword `additionalProperties` must be a boolean".to_string());
                }
            };

            let const_value = map.get("const").cloned();

            let enum_values = match map.get("enum") {
                None => None,
                Some(JsonValue::Array(values)) => Some(values.clone()),
                Some(_) => return Err("keyword `enum` must be an array".to_string()),
            };

            let pattern = match map.get("pattern") {
                None => None,
                Some(JsonValue::String(raw)) => Some(ArtifactSchemaPattern::compile(raw)?),
                Some(_) => return Err("keyword `pattern` must be a string".to_string()),
            };

            Ok(CompiledArtifactSchema::Object(
                CompiledArtifactSchemaObject {
                    expected_type,
                    required,
                    properties,
                    additional_properties,
                    const_value,
                    enum_values,
                    pattern,
                },
            ))
        }
        _ => Err("schema must be an object or boolean".to_string()),
    }
}

fn validate_artifact_schema(
    schema: &CompiledArtifactSchema,
    value: &JsonValue,
    path: &str,
) -> Result<(), String> {
    match schema {
        CompiledArtifactSchema::Bool(true) => Ok(()),
        CompiledArtifactSchema::Bool(false) => Err(format!("{path} is rejected by boolean schema")),
        CompiledArtifactSchema::Object(schema) => {
            if let Some(expected_type) = schema.expected_type
                && !expected_type.matches(value)
            {
                return Err(format!("{path} expected type `{}`", expected_type.label()));
            }

            if let Some(const_value) = schema.const_value.as_ref()
                && value != const_value
            {
                return Err(format!("{path} does not match `const` constraint"));
            }

            if let Some(enum_values) = schema.enum_values.as_ref()
                && !enum_values.iter().any(|candidate| candidate == value)
            {
                return Err(format!("{path} does not match any `enum` value"));
            }

            if let Some(pattern) = schema.pattern.as_ref()
                && let Some(text) = value.as_str()
                && !pattern.matches(text)
            {
                return Err(format!("{path} does not satisfy `pattern` constraint"));
            }

            if !schema.required.is_empty() {
                let object = value
                    .as_object()
                    .ok_or_else(|| format!("{path} must be an object for `required` validation"))?;
                for field in &schema.required {
                    if !object.contains_key(field) {
                        return Err(format!("{path} missing required property `{field}`"));
                    }
                }
            }

            if !schema.properties.is_empty() || matches!(schema.additional_properties, Some(false))
            {
                let object = value
                    .as_object()
                    .ok_or_else(|| format!("{path} must be an object for property validation"))?;

                for (name, child_schema) in &schema.properties {
                    if let Some(child_value) = object.get(name) {
                        let child_path = format!("{path}.{name}");
                        validate_artifact_schema(child_schema, child_value, &child_path)?;
                    }
                }

                if matches!(schema.additional_properties, Some(false)) {
                    for field in object.keys() {
                        if !schema.properties.contains_key(field) {
                            return Err(format!(
                                "{path} contains undeclared property `{field}` while `additionalProperties` is false"
                            ));
                        }
                    }
                }
            }

            Ok(())
        }
    }
}

fn sorted_artifact_labels(artifacts: &[JobArtifact]) -> Vec<String> {
    let mut values = artifacts.iter().map(format_artifact).collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn dedup_artifacts(artifacts: Vec<JobArtifact>) -> Vec<JobArtifact> {
    let mut by_label = artifacts
        .into_iter()
        .map(|artifact| (format_artifact(&artifact), artifact))
        .collect::<Vec<_>>();
    by_label.sort_by(|left, right| left.0.cmp(&right.0));
    by_label.dedup_by(|left, right| left.0 == right.0);
    by_label.into_iter().map(|(_, artifact)| artifact).collect()
}

fn dedup_locks(locks: Vec<JobLock>) -> Vec<JobLock> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for lock in locks {
        let key = format!("{}:{:?}", lock.key, lock.mode);
        if seen.insert(key) {
            deduped.push(lock);
        }
    }
    deduped
}

fn dedup_preconditions(preconditions: Vec<WorkflowPrecondition>) -> Vec<WorkflowPrecondition> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for precondition in preconditions {
        let key = workflow_precondition_label(&precondition);
        if seen.insert(key) {
            deduped.push(precondition);
        }
    }
    deduped
}

fn dedup_gates(gates: Vec<WorkflowGate>) -> Vec<WorkflowGate> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for gate in gates {
        let key = workflow_gate_label(&gate);
        if seen.insert(key) {
            deduped.push(gate);
        }
    }
    deduped
}

fn normalize_outcome_edges(edges: &WorkflowOutcomeEdges) -> WorkflowOutcomeEdges {
    WorkflowOutcomeEdges {
        succeeded: normalize_node_ids(&edges.succeeded),
        failed: normalize_node_ids(&edges.failed),
        blocked: normalize_node_ids(&edges.blocked),
        cancelled: normalize_node_ids(&edges.cancelled),
    }
}

fn normalize_node_ids(values: &[String]) -> Vec<String> {
    let mut node_ids = values.to_vec();
    node_ids.sort();
    node_ids.dedup();
    node_ids
}

fn workflow_precondition_label(precondition: &WorkflowPrecondition) -> String {
    match precondition {
        WorkflowPrecondition::PinnedHead => "pinned_head".to_string(),
        WorkflowPrecondition::CleanWorktree => "clean_worktree".to_string(),
        WorkflowPrecondition::BranchExists => "branch_exists".to_string(),
        WorkflowPrecondition::Custom { id, args } => {
            let mut pairs = args
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>();
            pairs.sort();
            if pairs.is_empty() {
                format!("custom:{id}")
            } else {
                format!("custom:{id}:{}", pairs.join(","))
            }
        }
    }
}

fn workflow_gate_label(gate: &WorkflowGate) -> String {
    match gate {
        WorkflowGate::Approval { required, policy } => {
            format!("approval:{required}:{policy:?}")
        }
        WorkflowGate::Script { script, policy } => format!("script:{script}:{policy:?}"),
        WorkflowGate::Cicd {
            script,
            auto_resolve,
            policy,
        } => {
            format!("cicd:{script}:{auto_resolve}:{policy:?}")
        }
        WorkflowGate::Custom { id, policy, args } => {
            let mut pairs = args
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>();
            pairs.sort();
            if pairs.is_empty() {
                format!("custom:{id}:{policy:?}")
            } else {
                format!("custom:{id}:{policy:?}:{}", pairs.join(","))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::{LockMode, format_artifact};

    fn prompt_artifact(key: &str) -> JobArtifact {
        JobArtifact::Custom {
            type_id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
            key: key.to_string(),
        }
    }

    fn sample_template() -> WorkflowTemplate {
        WorkflowTemplate {
            id: "template.review".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::from([("slug".to_string(), "alpha".to_string())]),
            policy: WorkflowTemplatePolicy {
                failure_mode: WorkflowFailureMode::BlockDownstream,
                resume: WorkflowResumePolicy {
                    key: "review".to_string(),
                    reuse_mode: WorkflowResumeReuseMode::Strict,
                },
            },
            artifact_contracts: vec![
                WorkflowArtifactContract {
                    id: "plan_branch".to_string(),
                    version: "v1".to_string(),
                    schema: None,
                },
                WorkflowArtifactContract {
                    id: "plan_doc".to_string(),
                    version: "v1".to_string(),
                    schema: None,
                },
                WorkflowArtifactContract {
                    id: "plan_commits".to_string(),
                    version: "v1".to_string(),
                    schema: None,
                },
                WorkflowArtifactContract {
                    id: "command_patch".to_string(),
                    version: "v1".to_string(),
                    schema: None,
                },
            ],
            nodes: vec![
                WorkflowNode {
                    id: "review_apply".to_string(),
                    kind: WorkflowNodeKind::Agent,
                    uses: "vizier.review.apply".to_string(),
                    args: BTreeMap::new(),
                    after: vec![WorkflowAfterDependency {
                        node_id: "review_critique".to_string(),
                        policy: AfterPolicy::Success,
                    }],
                    needs: vec![JobArtifact::PlanDoc {
                        slug: "alpha".to_string(),
                        branch: "draft/alpha".to_string(),
                    }],
                    produces: WorkflowOutcomeArtifacts {
                        succeeded: vec![JobArtifact::PlanCommits {
                            slug: "alpha".to_string(),
                            branch: "draft/alpha".to_string(),
                        }],
                        ..Default::default()
                    },
                    locks: vec![JobLock {
                        key: "branch:draft/alpha".to_string(),
                        mode: LockMode::Exclusive,
                    }],
                    preconditions: vec![WorkflowPrecondition::BranchExists],
                    gates: vec![],
                    retry: WorkflowRetryPolicy {
                        mode: WorkflowRetryMode::Never,
                        budget: 0,
                    },
                    on: WorkflowOutcomeEdges::default(),
                },
                WorkflowNode {
                    id: "review_critique".to_string(),
                    kind: WorkflowNodeKind::Agent,
                    uses: "vizier.review.critique".to_string(),
                    args: BTreeMap::new(),
                    after: vec![],
                    needs: vec![
                        JobArtifact::PlanBranch {
                            slug: "alpha".to_string(),
                            branch: "draft/alpha".to_string(),
                        },
                        JobArtifact::PlanDoc {
                            slug: "alpha".to_string(),
                            branch: "draft/alpha".to_string(),
                        },
                    ],
                    produces: WorkflowOutcomeArtifacts {
                        succeeded: vec![JobArtifact::CommandPatch {
                            job_id: "review-critique".to_string(),
                        }],
                        ..Default::default()
                    },
                    locks: vec![JobLock {
                        key: "branch:draft/alpha".to_string(),
                        mode: LockMode::Exclusive,
                    }],
                    preconditions: vec![WorkflowPrecondition::PinnedHead],
                    gates: vec![WorkflowGate::Script {
                        script: "./cicd.sh".to_string(),
                        policy: WorkflowGatePolicy::Warn,
                    }],
                    retry: WorkflowRetryPolicy {
                        mode: WorkflowRetryMode::OnFailure,
                        budget: 1,
                    },
                    on: WorkflowOutcomeEdges::default(),
                },
            ],
        }
    }

    fn custom_artifact_template() -> WorkflowTemplate {
        WorkflowTemplate {
            id: "template.custom".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![WorkflowArtifactContract {
                id: "acme.diff".to_string(),
                version: "v1".to_string(),
                schema: None,
            }],
            nodes: vec![WorkflowNode {
                id: "custom_node".to_string(),
                kind: WorkflowNodeKind::Custom,
                uses: "cap.env.shell.command.run".to_string(),
                args: BTreeMap::from([(
                    "command".to_string(),
                    "printf custom-workflow".to_string(),
                )]),
                after: Vec::new(),
                needs: vec![JobArtifact::Custom {
                    type_id: "acme.diff".to_string(),
                    key: "input".to_string(),
                }],
                produces: WorkflowOutcomeArtifacts {
                    succeeded: vec![JobArtifact::Custom {
                        type_id: "acme.diff".to_string(),
                        key: "output".to_string(),
                    }],
                    ..Default::default()
                },
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: WorkflowRetryPolicy::default(),
                on: WorkflowOutcomeEdges::default(),
            }],
        }
    }

    #[test]
    fn policy_snapshot_hash_is_stable_for_equivalent_templates() {
        let left = sample_template();
        let mut right = sample_template();
        right.nodes.reverse();
        right.artifact_contracts.reverse();

        let left_hash = left
            .policy_snapshot()
            .stable_hash_hex()
            .expect("hash left snapshot");
        let right_hash = right
            .policy_snapshot()
            .stable_hash_hex()
            .expect("hash right snapshot");

        assert_eq!(
            left_hash, right_hash,
            "equivalent snapshots must hash equally"
        );
    }

    #[test]
    fn compile_node_maps_edges_and_artifacts() {
        let template = sample_template();
        let resolved_after =
            BTreeMap::from([("review_critique".to_string(), "job-17".to_string())]);

        let compiled = compile_workflow_node(&template, "review_apply", &resolved_after)
            .expect("compile review_apply");

        assert_eq!(compiled.template_id, "template.review");
        assert_eq!(compiled.template_version, "v1");
        assert_eq!(compiled.node_id, "review_apply");
        assert_eq!(compiled.node_class, WorkflowNodeClass::Executor);
        assert_eq!(compiled.executor_class, Some(WorkflowExecutorClass::Agent));
        assert_eq!(compiled.executor_operation.as_deref(), Some("agent.invoke"));
        assert_eq!(
            compiled.control_policy, None,
            "executor nodes should not expose control policy"
        );
        assert_eq!(
            compiled.diagnostics.len(),
            1,
            "legacy uses aliases should emit compile diagnostics"
        );
        assert_eq!(compiled.after.len(), 1);
        assert_eq!(compiled.after[0].job_id, "job-17");
        assert_eq!(compiled.dependencies.len(), 1);
        assert_eq!(
            format_artifact(&compiled.dependencies[0]),
            "plan_doc:alpha (draft/alpha)"
        );
        assert_eq!(compiled.artifacts.len(), 1);
        assert_eq!(
            format_artifact(&compiled.artifacts[0]),
            "plan_commits:alpha (draft/alpha)"
        );
    }

    #[test]
    fn compile_node_requires_resolved_after_nodes() {
        let template = sample_template();
        let error = compile_workflow_node(&template, "review_apply", &BTreeMap::new())
            .expect_err("missing after dependency should fail");
        assert!(
            error.contains("unresolved after node `review_critique`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn compile_node_requires_declared_artifact_contracts() {
        let mut template = sample_template();
        template.artifact_contracts.clear();
        let resolved_after =
            BTreeMap::from([("review_critique".to_string(), "job-17".to_string())]);
        let error = compile_workflow_node(&template, "review_apply", &resolved_after)
            .expect_err("missing artifact contract should fail");
        assert!(
            error.contains("without declared artifact contract `plan_doc`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn compile_node_rejects_unknown_after_node_references() {
        let mut template = sample_template();
        template.nodes[0].after = vec![WorkflowAfterDependency {
            node_id: "not_defined".to_string(),
            policy: AfterPolicy::Success,
        }];
        let resolved_after = BTreeMap::from([("not_defined".to_string(), "job-17".to_string())]);
        let error = compile_workflow_node(&template, "review_apply", &resolved_after)
            .expect_err("unknown after node should fail");
        assert!(
            error.contains("references unknown after node `not_defined`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn compile_node_rejects_unknown_on_targets() {
        let mut template = sample_template();
        template.nodes[0].on = WorkflowOutcomeEdges {
            succeeded: vec!["not_defined".to_string()],
            ..Default::default()
        };
        let resolved_after =
            BTreeMap::from([("review_critique".to_string(), "job-17".to_string())]);
        let error = compile_workflow_node(&template, "review_apply", &resolved_after)
            .expect_err("unknown on target should fail");
        assert!(
            error.contains("references unknown on.succeeded target `not_defined`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn compile_node_accepts_multiple_on_targets_for_single_outcome() {
        let mut template = sample_template();
        template.nodes[0].on = WorkflowOutcomeEdges {
            failed: vec!["review_critique".to_string(), "review_apply".to_string()],
            ..Default::default()
        };
        let resolved_after =
            BTreeMap::from([("review_critique".to_string(), "job-17".to_string())]);
        let compiled = compile_workflow_node(&template, "review_apply", &resolved_after)
            .expect("multiple on targets should compile");
        assert_eq!(
            compiled.on.failed,
            vec!["review_apply".to_string(), "review_critique".to_string()],
            "outcome targets should be normalized deterministically"
        );
    }

    #[test]
    fn compile_node_accepts_custom_artifact_contracts() {
        let template = custom_artifact_template();
        let compiled = compile_workflow_node(&template, "custom_node", &BTreeMap::new())
            .expect("custom artifact contracts should compile");

        assert_eq!(compiled.dependencies.len(), 1);
        assert_eq!(
            format_artifact(&compiled.dependencies[0]),
            "custom:acme.diff:input"
        );
        assert_eq!(compiled.artifacts.len(), 1);
        assert_eq!(
            format_artifact(&compiled.artifacts[0]),
            "custom:acme.diff:output"
        );
    }

    #[test]
    fn compile_node_rejects_custom_artifacts_without_declared_contracts() {
        let mut template = custom_artifact_template();
        template.artifact_contracts.clear();
        let error = compile_workflow_node(&template, "custom_node", &BTreeMap::new())
            .expect_err("missing custom artifact contract should fail");
        assert!(
            error.contains("without declared artifact contract `acme.diff`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn compile_node_enforces_artifact_contract_schema() {
        let mut template = custom_artifact_template();
        template.artifact_contracts[0].schema = Some(serde_json::json!({
            "type": "object",
            "required": ["type_id", "key"],
            "properties": {
                "type_id": { "const": "acme.diff" },
                "key": { "type": "string", "pattern": "^(input|output)$" }
            },
            "additionalProperties": false
        }));
        compile_workflow_node(&template, "custom_node", &BTreeMap::new())
            .expect("schema-compatible custom artifact should compile");

        template.artifact_contracts[0].schema = Some(serde_json::json!({
            "type": "object",
            "required": ["key"],
            "properties": {
                "key": { "const": "result" }
            }
        }));
        let error = compile_workflow_node(&template, "custom_node", &BTreeMap::new())
            .expect_err("schema-mismatched custom artifact should fail");
        assert!(
            error.contains("violates schema for contract `acme.diff`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn compile_node_rejects_invalid_artifact_contract_schema() {
        let mut template = custom_artifact_template();
        template.artifact_contracts[0].schema = Some(serde_json::json!({
            "type": 7
        }));
        let error = compile_workflow_node(&template, "custom_node", &BTreeMap::new())
            .expect_err("invalid contract schema should fail");
        assert!(
            error.contains("has invalid schema"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn workflow_capability_maps_known_labels_and_ids() {
        assert_eq!(
            WorkflowCapability::from_uses_label("vizier.merge.cicd_gate"),
            Some(WorkflowCapability::GateCicd)
        );
        assert_eq!(
            WorkflowCapability::from_id("cap.gate.cicd"),
            Some(WorkflowCapability::GateCicd)
        );
        assert_eq!(
            WorkflowCapability::GateCicd.id(),
            "cap.gate.cicd",
            "capability id should be stable for docs/runtime"
        );
    }

    #[test]
    fn workflow_capability_maps_non_vizier_labels_to_custom_command() {
        assert_eq!(WorkflowCapability::from_uses_label("acme.shell.note"), None);
        assert_eq!(
            WorkflowCapability::from_uses_label("custom.namespace.step"),
            None
        );
        assert_eq!(
            WorkflowCapability::from_uses_label("vizier.unknown.label"),
            None,
            "unknown vizier labels should remain unmapped so runtime can reject/handle explicitly"
        );
    }

    #[test]
    fn classify_template_node_reports_legacy_alias_warnings() {
        let template = WorkflowTemplate {
            id: "template.legacy".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![],
            nodes: vec![WorkflowNode {
                id: "legacy_merge".to_string(),
                kind: WorkflowNodeKind::Builtin,
                uses: "vizier.merge.integrate".to_string(),
                args: BTreeMap::new(),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: WorkflowRetryPolicy::default(),
                on: WorkflowOutcomeEdges::default(),
            }],
        };

        let diagnostics = workflow_template_diagnostics(&template)
            .expect("legacy aliases should classify with warnings");
        assert_eq!(
            diagnostics.len(),
            1,
            "expected one warning: {diagnostics:?}"
        );
        assert!(
            diagnostics[0].message.contains("deprecated"),
            "unexpected warning: {:?}",
            diagnostics[0]
        );
        assert!(
            diagnostics[0]
                .message
                .contains(LEGACY_CAPABILITY_COMPATIBILITY_END_DATE),
            "warning should include compatibility window end date: {:?}",
            diagnostics[0]
        );
    }

    #[test]
    fn validate_capability_contracts_rejects_unknown_uses_labels() {
        let template = WorkflowTemplate {
            id: "template.unknown".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![],
            nodes: vec![WorkflowNode {
                id: "unknown".to_string(),
                kind: WorkflowNodeKind::Custom,
                uses: "acme.custom.step".to_string(),
                args: BTreeMap::from([("command".to_string(), "echo nope".to_string())]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: WorkflowRetryPolicy::default(),
                on: WorkflowOutcomeEdges::default(),
            }],
        };

        let error = validate_workflow_capability_contracts(&template)
            .expect_err("unknown uses labels should be rejected");
        assert!(
            error.contains("uses unknown label"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn compile_node_classifies_executor_first_model() {
        let template = WorkflowTemplate {
            id: "template.executor_first".to_string(),
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
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts {
                        succeeded: vec![prompt_artifact("resolve_prompt")],
                        ..Default::default()
                    },
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: Vec::new(),
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges {
                        succeeded: vec!["apply_plan".to_string()],
                        ..Default::default()
                    },
                },
                WorkflowNode {
                    id: "apply_plan".to_string(),
                    kind: WorkflowNodeKind::Agent,
                    uses: "cap.agent.invoke".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: vec![prompt_artifact("resolve_prompt")],
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: Vec::new(),
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges {
                        succeeded: vec!["stop_gate".to_string()],
                        ..Default::default()
                    },
                },
                WorkflowNode {
                    id: "stop_gate".to_string(),
                    kind: WorkflowNodeKind::Gate,
                    uses: "control.gate.stop_condition".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: vec![WorkflowGate::Script {
                        script: "./stop.sh".to_string(),
                        policy: WorkflowGatePolicy::Retry,
                    }],
                    retry: WorkflowRetryPolicy {
                        mode: WorkflowRetryMode::UntilGate,
                        budget: 2,
                    },
                    on: WorkflowOutcomeEdges {
                        failed: vec!["apply_plan".to_string()],
                        ..Default::default()
                    },
                },
            ],
        };

        validate_workflow_capability_contracts(&template)
            .expect("executor-first split template should validate");
        let compiled = compile_workflow_node(&template, "resolve_prompt", &BTreeMap::new())
            .expect("executor-first node should compile");
        assert_eq!(compiled.node_class, WorkflowNodeClass::Executor);
        assert_eq!(
            compiled.executor_class,
            Some(WorkflowExecutorClass::EnvironmentBuiltin)
        );
        assert_eq!(
            compiled.executor_operation.as_deref(),
            Some("prompt.resolve")
        );
        assert!(
            compiled.diagnostics.is_empty(),
            "canonical ids should not emit legacy warnings: {:?}",
            compiled.diagnostics
        );

        let invoke = compile_workflow_node(&template, "apply_plan", &BTreeMap::new())
            .expect("invoke node should compile");
        assert_eq!(invoke.node_class, WorkflowNodeClass::Executor);
        assert_eq!(invoke.executor_class, Some(WorkflowExecutorClass::Agent));
        assert_eq!(invoke.executor_operation.as_deref(), Some("agent.invoke"));
        assert!(
            invoke.diagnostics.is_empty(),
            "canonical invoke should not emit legacy warnings: {:?}",
            invoke.diagnostics
        );
    }

    #[test]
    fn validate_capability_contracts_rejects_agent_invoke_without_prompt_dependency() {
        let template = WorkflowTemplate {
            id: "template.invoke".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![WorkflowArtifactContract {
                id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
                version: "v1".to_string(),
                schema: None,
            }],
            nodes: vec![WorkflowNode {
                id: "invoke".to_string(),
                kind: WorkflowNodeKind::Agent,
                uses: "cap.agent.invoke".to_string(),
                args: BTreeMap::new(),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: WorkflowRetryPolicy::default(),
                on: WorkflowOutcomeEdges::default(),
            }],
        };

        let error = validate_workflow_capability_contracts(&template)
            .expect_err("invoke without prompt dependency should fail");
        assert!(
            error.contains("requires exactly one prompt artifact dependency"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn validate_capability_contracts_rejects_agent_invoke_with_multiple_prompt_dependencies() {
        let template = WorkflowTemplate {
            id: "template.invoke".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![WorkflowArtifactContract {
                id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
                version: "v1".to_string(),
                schema: None,
            }],
            nodes: vec![WorkflowNode {
                id: "invoke".to_string(),
                kind: WorkflowNodeKind::Agent,
                uses: "cap.agent.invoke".to_string(),
                args: BTreeMap::new(),
                after: Vec::new(),
                needs: vec![prompt_artifact("one"), prompt_artifact("two")],
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: WorkflowRetryPolicy::default(),
                on: WorkflowOutcomeEdges::default(),
            }],
        };

        let error = validate_workflow_capability_contracts(&template)
            .expect_err("invoke with multiple prompt dependencies should fail");
        assert!(
            error.contains("requires exactly one prompt artifact dependency"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn validate_capability_contracts_rejects_shell_prompt_resolve_without_command_or_script() {
        let template = WorkflowTemplate {
            id: "template.prompt".to_string(),
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
                kind: WorkflowNodeKind::Shell,
                uses: "cap.env.shell.prompt.resolve".to_string(),
                args: BTreeMap::new(),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts {
                    succeeded: vec![prompt_artifact("resolve_prompt")],
                    ..Default::default()
                },
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: WorkflowRetryPolicy::default(),
                on: WorkflowOutcomeEdges::default(),
            }],
        };

        let error = validate_workflow_capability_contracts(&template)
            .expect_err("shell prompt resolve without command/script should fail");
        assert!(
            error.contains("requires exactly one of args.command or args.script"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn validate_capability_contracts_allow_intent_as_non_critical_metadata() {
        let template = WorkflowTemplate {
            id: "template.invoke".to_string(),
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
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts {
                        succeeded: vec![prompt_artifact("invoke")],
                        ..Default::default()
                    },
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: Vec::new(),
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges::default(),
                },
                WorkflowNode {
                    id: "invoke".to_string(),
                    kind: WorkflowNodeKind::Agent,
                    uses: "cap.agent.invoke".to_string(),
                    args: BTreeMap::from([("intent".to_string(), "optional".to_string())]),
                    after: Vec::new(),
                    needs: vec![prompt_artifact("invoke")],
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: Vec::new(),
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges::default(),
                },
            ],
        };

        validate_workflow_capability_contracts(&template)
            .expect("intent should remain optional metadata and not part of invoke routing");
    }

    #[test]
    fn validate_capability_contracts_rejects_approve_stop_gate_missing_retry_back_edge() {
        let template = WorkflowTemplate {
            id: "template.approve".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![WorkflowArtifactContract {
                id: "plan_doc".to_string(),
                version: "v1".to_string(),
                schema: None,
            }],
            nodes: vec![
                WorkflowNode {
                    id: "approve_apply".to_string(),
                    kind: WorkflowNodeKind::Builtin,
                    uses: "vizier.approve.apply_once".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: vec![JobArtifact::PlanDoc {
                        slug: "slug".to_string(),
                        branch: "draft/slug".to_string(),
                    }],
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: Vec::new(),
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges {
                        succeeded: vec!["approve_stop".to_string()],
                        ..Default::default()
                    },
                },
                WorkflowNode {
                    id: "approve_stop".to_string(),
                    kind: WorkflowNodeKind::Gate,
                    uses: "vizier.approve.stop_condition".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: vec![WorkflowGate::Script {
                        script: "./scripts/stop.sh".to_string(),
                        policy: WorkflowGatePolicy::Retry,
                    }],
                    retry: WorkflowRetryPolicy {
                        mode: WorkflowRetryMode::UntilGate,
                        budget: 2,
                    },
                    on: WorkflowOutcomeEdges::default(),
                },
            ],
        };

        let error = validate_workflow_capability_contracts(&template)
            .expect_err("stop-condition gate without retry edge should fail");
        assert!(
            error.contains("cap.gate.stop_condition") || error.contains("cap.plan.apply_once"),
            "unexpected error: {error}"
        );
        assert!(error.contains("on.failed"), "unexpected error: {error}");
    }

    #[test]
    fn validate_capability_contracts_rejects_merge_cicd_auto_resolve_without_retry_loop() {
        let template = WorkflowTemplate {
            id: "template.merge".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![],
            nodes: vec![
                WorkflowNode {
                    id: "merge_gate".to_string(),
                    kind: WorkflowNodeKind::Gate,
                    uses: "vizier.merge.cicd_gate".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: vec![WorkflowGate::Cicd {
                        script: "./cicd.sh".to_string(),
                        auto_resolve: true,
                        policy: WorkflowGatePolicy::Retry,
                    }],
                    retry: WorkflowRetryPolicy {
                        mode: WorkflowRetryMode::UntilGate,
                        budget: 2,
                    },
                    on: WorkflowOutcomeEdges {
                        failed: vec!["fix".to_string()],
                        ..Default::default()
                    },
                },
                WorkflowNode {
                    id: "fix".to_string(),
                    kind: WorkflowNodeKind::Agent,
                    uses: "vizier.merge.cicd_auto_fix".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: Vec::new(),
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges::default(),
                },
            ],
        };

        let error = validate_workflow_capability_contracts(&template)
            .expect_err("auto-resolve cicd gate without retry closure should fail");
        assert!(error.contains("cap.gate.cicd"), "unexpected error: {error}");
        assert!(error.contains("on.failed"), "unexpected error: {error}");
    }

    #[test]
    fn validate_capability_contracts_allows_merge_inline_and_explicit_cicd_gate_paths() {
        let template = WorkflowTemplate {
            id: "template.merge".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![],
            nodes: vec![
                WorkflowNode {
                    id: "merge_integrate".to_string(),
                    kind: WorkflowNodeKind::Builtin,
                    uses: "vizier.merge.integrate".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: vec![WorkflowGate::Cicd {
                        script: "./cicd.sh".to_string(),
                        auto_resolve: false,
                        policy: WorkflowGatePolicy::Retry,
                    }],
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges {
                        succeeded: vec!["merge_gate_cicd".to_string()],
                        ..Default::default()
                    },
                },
                WorkflowNode {
                    id: "merge_gate_cicd".to_string(),
                    kind: WorkflowNodeKind::Gate,
                    uses: "vizier.merge.cicd_gate".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: vec![WorkflowGate::Cicd {
                        script: "./cicd.sh".to_string(),
                        auto_resolve: false,
                        policy: WorkflowGatePolicy::Retry,
                    }],
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges::default(),
                },
            ],
        };
        validate_workflow_capability_contracts(&template)
            .expect("compat inline+explicit merge gate paths should remain valid");
    }

    #[test]
    fn validate_capability_contracts_rejects_review_node_with_multiple_cicd_gates() {
        let template = WorkflowTemplate {
            id: "template.review".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![],
            nodes: vec![WorkflowNode {
                id: "review_main".to_string(),
                kind: WorkflowNodeKind::Agent,
                uses: "vizier.review.critique".to_string(),
                args: BTreeMap::new(),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: vec![
                    WorkflowGate::Cicd {
                        script: "./first.sh".to_string(),
                        auto_resolve: false,
                        policy: WorkflowGatePolicy::Warn,
                    },
                    WorkflowGate::Cicd {
                        script: "./second.sh".to_string(),
                        auto_resolve: false,
                        policy: WorkflowGatePolicy::Warn,
                    },
                ],
                retry: WorkflowRetryPolicy::default(),
                on: WorkflowOutcomeEdges::default(),
            }],
        };
        let error = validate_workflow_capability_contracts(&template)
            .expect_err("multiple review cicd gates should fail");
        assert!(
            error.contains("cap.review.critique_or_fix"),
            "unexpected error: {error}"
        );
        assert!(
            error.contains("multiple cicd gates"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn validate_capability_contracts_accepts_review_node_without_canonical_id() {
        let template = WorkflowTemplate {
            id: "template.review".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![],
            nodes: vec![WorkflowNode {
                id: "custom_review".to_string(),
                kind: WorkflowNodeKind::Agent,
                uses: "vizier.review.critique".to_string(),
                args: BTreeMap::new(),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: WorkflowRetryPolicy::default(),
                on: WorkflowOutcomeEdges::default(),
            }],
        };
        validate_workflow_capability_contracts(&template)
            .expect("semantic review capability should pass without canonical node id");
    }

    #[test]
    fn validate_capability_contracts_rejects_patch_execute_without_files() {
        let template = WorkflowTemplate {
            id: "template.patch".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![],
            nodes: vec![WorkflowNode {
                id: "patch".to_string(),
                kind: WorkflowNodeKind::Builtin,
                uses: "vizier.patch.execute".to_string(),
                args: BTreeMap::new(),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: WorkflowRetryPolicy::default(),
                on: WorkflowOutcomeEdges::default(),
            }],
        };
        let error = validate_workflow_capability_contracts(&template)
            .expect_err("patch node without files_json should fail");
        assert!(
            error.contains("cap.patch.execute_pipeline"),
            "unexpected error: {error}"
        );
        assert!(error.contains("files_json"), "unexpected error: {error}");
    }
}
