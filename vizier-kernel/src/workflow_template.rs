use crate::scheduler::{
    AfterPolicy, JobAfterDependency, JobArtifact, JobLock, MissingProducerPolicy, format_artifact,
};
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
            dependencies: self.policy.dependencies.clone(),
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
    #[serde(default)]
    pub dependencies: WorkflowDependenciesPolicy,
}

impl Default for WorkflowTemplatePolicy {
    fn default() -> Self {
        Self {
            failure_mode: WorkflowFailureMode::BlockDownstream,
            resume: WorkflowResumePolicy::default(),
            dependencies: WorkflowDependenciesPolicy::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WorkflowDependenciesPolicy {
    #[serde(default)]
    pub missing_producer: MissingProducerPolicy,
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
    pub dependencies: WorkflowDependenciesPolicy,
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

pub const PROMPT_ARTIFACT_TYPE_ID: &str = "prompt_text";

#[derive(Debug, Clone)]
struct WorkflowNodeResolution {
    identity: WorkflowNodeIdentity,
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
    if uses.is_empty() {
        return Err(format!(
            "template {}@{} node `{}` must declare an explicit canonical uses id",
            template.id, template.version, node.id
        ));
    }

    let resolved = if let Some((executor_class, executor_operation)) =
        canonical_executor_operation(uses)
    {
        WorkflowNodeResolution {
            identity: WorkflowNodeIdentity {
                node_class: WorkflowNodeClass::Executor,
                executor_class: Some(executor_class),
                executor_operation: Some(executor_operation.to_string()),
                control_policy: None,
                diagnostics: Vec::new(),
            },
        }
    } else if let Some(control_policy) = canonical_control_policy(uses) {
        WorkflowNodeResolution {
            identity: WorkflowNodeIdentity {
                node_class: WorkflowNodeClass::Control,
                executor_class: None,
                executor_operation: None,
                control_policy: Some(control_policy.to_string()),
                diagnostics: Vec::new(),
            },
        }
    } else {
        return Err(format!(
            "template {}@{} node `{}` uses unknown label `{uses}`; declare one of cap.env.builtin.*, cap.env.shell.*, cap.agent.invoke, or control.*",
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledWorkflowNode {
    pub template_id: String,
    pub template_version: String,
    pub node_id: String,
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

        validate_executor_arg_requirements(
            template,
            node,
            resolution.identity.executor_operation.as_deref(),
        )?;

        match resolution.identity.executor_operation.as_deref() {
            Some("agent.invoke") => validate_agent_invoke_contract(template, node)?,
            Some("prompt.resolve") => {
                if let Some(executor_class) = resolution.identity.executor_class {
                    validate_prompt_resolve_contract(template, node, executor_class)?;
                }
            }
            Some("command.run") => validate_exec_custom_command_contract(template, node)?,
            Some("cicd.run") => validate_cicd_run_contract(template, node)?,
            Some("git.save_worktree_patch") => {
                validate_save_worktree_patch_contract(template, node)?
            }
            Some("plan.persist") => validate_generate_draft_plan_contract(template, node)?,
            Some("patch.pipeline_prepare") => {
                validate_patch_files_contract(template, node, "patch.pipeline_prepare")?
            }
            Some("patch.execute_pipeline") => {
                validate_patch_files_contract(template, node, "patch.execute_pipeline")?
            }
            Some("build.materialize_step") => validate_build_materialize_contract(template, node)?,
            Some("git.stage_commit") => validate_plan_apply_once_contract(template, &by_id, node)?,
            Some("git.integrate_plan_branch") => {
                validate_integrate_plan_branch_contract(template, &by_id, node)?
            }
            _ => {}
        }

        match resolution.identity.control_policy.as_deref() {
            Some("gate.stop_condition") => {
                validate_stop_condition_contract(template, &by_id, node)?;
            }
            Some("gate.conflict_resolution") => {
                validate_conflict_resolution_contract(template, &by_id, node)?;
            }
            Some("gate.cicd") => {
                validate_cicd_gate_contract(template, &by_id, node)?;
            }
            _ => {}
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct NonEmptyAnyOfArgRequirement {
    operation: &'static str,
    keys: &'static [&'static str],
}

const NON_EMPTY_ANY_OF_ARG_REQUIREMENTS: &[NonEmptyAnyOfArgRequirement] = &[
    NonEmptyAnyOfArgRequirement {
        operation: "worktree.prepare",
        keys: &["branch", "slug", "plan"],
    },
    NonEmptyAnyOfArgRequirement {
        operation: "git.integrate_plan_branch",
        keys: &["branch", "source_branch", "plan_branch", "slug", "plan"],
    },
];

pub fn executor_non_empty_any_of_arg_keys(operation: &str) -> Option<&'static [&'static str]> {
    NON_EMPTY_ANY_OF_ARG_REQUIREMENTS
        .iter()
        .find(|entry| entry.operation == operation)
        .map(|entry| entry.keys)
}

fn validate_executor_arg_requirements(
    template: &WorkflowTemplate,
    node: &WorkflowNode,
    operation: Option<&str>,
) -> Result<(), String> {
    let Some(operation) = operation else {
        return Ok(());
    };

    if let Some(required_keys) = executor_non_empty_any_of_arg_keys(operation) {
        if required_keys.iter().any(|key| has_nonempty_arg(node, key)) {
            return Ok(());
        }
        let expected = required_keys
            .iter()
            .map(|key| format!("args.{key}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(executor_contract_error(
            template,
            operation,
            node,
            &format!("requires at least one non-empty argument: {expected}"),
        ));
    }

    Ok(())
}

fn validate_agent_invoke_contract(
    template: &WorkflowTemplate,
    node: &WorkflowNode,
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

fn node_has_executor_operation(node: &WorkflowNode, operation: &str) -> bool {
    canonical_executor_operation(node.uses.trim())
        .map(|(_class, executor_operation)| executor_operation == operation)
        .unwrap_or(false)
}

fn node_has_control_policy(node: &WorkflowNode, policy: &str) -> bool {
    canonical_control_policy(node.uses.trim())
        .map(|control_policy| control_policy == policy)
        .unwrap_or(false)
}

fn validate_plan_apply_once_contract(
    template: &WorkflowTemplate,
    by_id: &BTreeMap<String, &WorkflowNode>,
    node: &WorkflowNode,
) -> Result<(), String> {
    if script_gate_count(node) > 1 {
        return Err(contract_error(
            template,
            "git.stage_commit",
            node,
            "defines multiple script gates; expected at most one",
        ));
    }

    let stop_targets =
        resolve_outcome_targets(template, by_id, node, "succeeded", &node.on.succeeded)?
            .into_iter()
            .filter(|target| {
                target.id == "approve_gate_stop_condition"
                    || node_has_control_policy(target, "gate.stop_condition")
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
        return Err(contract_error(
            template,
            "git.stage_commit",
            node,
            &format!("has ambiguous stop-condition targets on on.succeeded: {ids}"),
        ));
    }

    if !node.on.succeeded.is_empty() && stop_targets.is_empty() {
        return Err(contract_error(
            template,
            "git.stage_commit",
            node,
            "has on.succeeded targets but none resolve to a stop-condition gate",
        ));
    }

    let Some(stop_node) = stop_targets.first().copied() else {
        return Ok(());
    };
    if !matches!(stop_node.kind, WorkflowNodeKind::Gate) {
        return Err(contract_error(
            template,
            "git.stage_commit",
            node,
            &format!(
                "targets stop-condition node `{}` with kind {:?}; expected gate",
                stop_node.id, stop_node.kind
            ),
        ));
    }

    if script_gate_count(stop_node) > 1 {
        return Err(contract_error(
            template,
            "git.stage_commit",
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
        return Err(contract_error(
            template,
            "git.stage_commit",
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
        return Err(contract_error(
            template,
            "gate.stop_condition",
            node,
            &format!("uses kind {:?}; expected gate", node.kind),
        ));
    }
    if script_gate_count(node) > 1 {
        return Err(contract_error(
            template,
            "gate.stop_condition",
            node,
            "defines multiple script gates; expected at most one",
        ));
    }

    let parents = template
        .nodes
        .iter()
        .filter(|candidate| {
            node_has_executor_operation(candidate, "git.stage_commit")
                && candidate
                    .on
                    .succeeded
                    .iter()
                    .any(|target| target == &node.id)
        })
        .collect::<Vec<_>>();
    if parents.is_empty() {
        return Err(contract_error(
            template,
            "gate.stop_condition",
            node,
            "is not targeted from any git.stage_commit on.succeeded edge",
        ));
    }
    if parents.len() > 1 {
        let ids = parents
            .iter()
            .map(|parent| parent.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(contract_error(
            template,
            "gate.stop_condition",
            node,
            &format!("is targeted by multiple apply nodes: {ids}"),
        ));
    }

    if script_gate_count(node) == 1
        && matches!(node.retry.mode, WorkflowRetryMode::UntilGate)
        && !node.on.failed.iter().any(|target| target == &parents[0].id)
    {
        return Err(contract_error(
            template,
            "gate.stop_condition",
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
        return Err(contract_error(
            template,
            "git.integrate_plan_branch",
            node,
            "defines multiple cicd gates; expected at most one",
        ));
    }

    let conflict_targets =
        resolve_outcome_targets(template, by_id, node, "blocked", &node.on.blocked)?
            .into_iter()
            .filter(|target| {
                target.id == "merge_conflict_resolution"
                    || node_has_control_policy(target, "gate.conflict_resolution")
                    || node_has_custom_gate(target, "conflict_resolution")
            })
            .collect::<Vec<_>>();
    if conflict_targets.len() > 1 {
        let ids = conflict_targets
            .iter()
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(contract_error(
            template,
            "git.integrate_plan_branch",
            node,
            &format!("has ambiguous conflict-resolution targets on on.blocked: {ids}"),
        ));
    }
    if let Some(conflict_target) = conflict_targets.first().copied()
        && !matches!(conflict_target.kind, WorkflowNodeKind::Gate)
    {
        return Err(contract_error(
            template,
            "git.integrate_plan_branch",
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
                    || node_has_control_policy(target, "gate.cicd")
                    || cicd_gate_count(target) > 0
            })
            .collect::<Vec<_>>();
    if cicd_targets.len() > 1 {
        let ids = cicd_targets
            .iter()
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(contract_error(
            template,
            "git.integrate_plan_branch",
            node,
            &format!("has ambiguous cicd gate targets on on.succeeded: {ids}"),
        ));
    }
    if let Some(gate_target) = cicd_targets.first().copied() {
        if !matches!(gate_target.kind, WorkflowNodeKind::Gate) {
            return Err(contract_error(
                template,
                "git.integrate_plan_branch",
                node,
                &format!(
                    "targets cicd node `{}` with kind {:?}; expected gate",
                    gate_target.id, gate_target.kind
                ),
            ));
        }
        if cicd_gate_count(gate_target) != 1 {
            return Err(contract_error(
                template,
                "git.integrate_plan_branch",
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
            return Err(contract_error(
                template,
                "git.integrate_plan_branch",
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
        return Err(contract_error(
            template,
            "gate.conflict_resolution",
            node,
            &format!("uses kind {:?}; expected gate", node.kind),
        ));
    }

    let integrate_parents = template
        .nodes
        .iter()
        .filter(|candidate| {
            node_has_executor_operation(candidate, "git.integrate_plan_branch")
                && candidate.on.blocked.iter().any(|target| target == &node.id)
        })
        .collect::<Vec<_>>();
    if integrate_parents.is_empty() {
        return Err(contract_error(
            template,
            "gate.conflict_resolution",
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
        return Err(contract_error(
            template,
            "gate.conflict_resolution",
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
        return Err(contract_error(
            template,
            "gate.conflict_resolution",
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
        return Err(contract_error(
            template,
            "gate.cicd",
            node,
            &format!("uses kind {:?}; expected gate", node.kind),
        ));
    }
    if cicd_gate_count(node) != 1 {
        return Err(contract_error(
            template,
            "gate.cicd",
            node,
            "must define exactly one cicd gate",
        ));
    }

    if let Some((_script, auto_resolve)) = single_cicd_gate(node)
        && auto_resolve
        && !gate_retry_loop_closes(template, by_id, node)?
    {
        return Err(contract_error(
            template,
            "gate.cicd",
            node,
            "enables auto_resolve but on.failed does not return to the gate",
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
            return Err(contract_error(
                template,
                "command.run",
                node,
                "requires args.command or args.script",
            ));
        }
        return Ok(());
    }

    if !has_command {
        return Err(contract_error(
            template,
            "command.run",
            node,
            "requires args.command or args.script for non-shell/custom node kinds",
        ));
    }
    Ok(())
}

fn validate_cicd_run_contract(
    template: &WorkflowTemplate,
    node: &WorkflowNode,
) -> Result<(), String> {
    let has_command = has_nonempty_arg(node, "command") || has_nonempty_arg(node, "script");
    let has_gate_script = node
        .gates
        .iter()
        .any(|gate| matches!(gate, WorkflowGate::Cicd { script, .. } if !script.trim().is_empty()));
    if has_command || has_gate_script {
        return Ok(());
    }
    Err(contract_error(
        template,
        "cicd.run",
        node,
        "requires args.command/args.script or a non-empty cicd gate script",
    ))
}

fn validate_save_worktree_patch_contract(
    template: &WorkflowTemplate,
    node: &WorkflowNode,
) -> Result<(), String> {
    if !matches!(node.kind, WorkflowNodeKind::Builtin) {
        return Err(contract_error(
            template,
            "git.save_worktree_patch",
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
        return Err(contract_error(
            template,
            "plan.persist",
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
                Err(contract_error(
                    template,
                    "plan.persist",
                    node,
                    "has spec_source=file but no spec_file argument",
                ))
            }
        }
        _ => Err(contract_error(
            template,
            "plan.persist",
            node,
            &format!("has unsupported spec_source `{source}`"),
        )),
    }
}

fn validate_patch_files_contract(
    template: &WorkflowTemplate,
    node: &WorkflowNode,
    operation: &str,
) -> Result<(), String> {
    let files_json = node
        .args
        .get("files_json")
        .ok_or_else(|| contract_error(template, operation, node, "requires files_json"))?;
    let files: Vec<String> = serde_json::from_str(files_json).map_err(|err| {
        contract_error(
            template,
            operation,
            node,
            &format!("has invalid files_json payload: {err}"),
        )
    })?;
    if files.is_empty() {
        return Err(contract_error(
            template,
            operation,
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
    Err(contract_error(
        template,
        "build.materialize_step",
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

fn contract_error(
    template: &WorkflowTemplate,
    contract: &str,
    node: &WorkflowNode,
    detail: &str,
) -> String {
    format!(
        "template {}@{} contract `{}` node `{}` {}",
        template.id, template.version, contract, node.id, detail
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
        return Err(contract_error(
            template,
            "gate.conflict_resolution",
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
    use crate::scheduler::{LockMode, MissingProducerPolicy, format_artifact};

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
                dependencies: WorkflowDependenciesPolicy::default(),
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
                    uses: "cap.agent.invoke".to_string(),
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
                    uses: "cap.agent.invoke".to_string(),
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
    fn policy_snapshot_tracks_dependency_policy() {
        let mut template = sample_template();
        let block_hash = template
            .policy_snapshot()
            .stable_hash_hex()
            .expect("hash block snapshot");
        assert_eq!(
            template.policy.dependencies.missing_producer,
            MissingProducerPolicy::Block
        );

        template.policy.dependencies.missing_producer = MissingProducerPolicy::Wait;
        let wait_snapshot = template.policy_snapshot();
        assert_eq!(
            wait_snapshot.dependencies.missing_producer,
            MissingProducerPolicy::Wait
        );
        let wait_hash = wait_snapshot.stable_hash_hex().expect("hash wait snapshot");
        assert_ne!(
            block_hash, wait_hash,
            "dependency policy changes should affect policy snapshot hash"
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
            0,
            "canonical uses should not emit classifier diagnostics"
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
    fn compile_node_validates_stage_token_custom_contracts() {
        let mut template = custom_artifact_template();
        template.nodes[0].needs = vec![JobArtifact::Custom {
            type_id: "stage_token".to_string(),
            key: "approve:alpha".to_string(),
        }];
        template.nodes[0].produces.succeeded = vec![JobArtifact::Custom {
            type_id: "stage_token".to_string(),
            key: "merge:alpha".to_string(),
        }];

        template.artifact_contracts.clear();
        let error = compile_workflow_node(&template, "custom_node", &BTreeMap::new())
            .expect_err("missing stage token contract should fail");
        assert!(
            error.contains("without declared artifact contract `stage_token`"),
            "unexpected error: {error}"
        );

        template.artifact_contracts.push(WorkflowArtifactContract {
            id: "stage_token".to_string(),
            version: "v1".to_string(),
            schema: None,
        });
        compile_workflow_node(&template, "custom_node", &BTreeMap::new())
            .expect("declared stage token contract should compile");
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
    fn validate_capability_contracts_rejects_legacy_vizier_uses_label() {
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

        let error = validate_workflow_capability_contracts(&template)
            .expect_err("legacy vizier uses labels should fail hard");
        assert!(
            error.contains("uses unknown label"),
            "unexpected error: {error}"
        );
        assert!(
            error.contains("vizier.merge.integrate"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn validate_capability_contracts_rejects_legacy_non_env_cap_label() {
        let template = WorkflowTemplate {
            id: "template.legacy".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![],
            nodes: vec![WorkflowNode {
                id: "legacy_merge".to_string(),
                kind: WorkflowNodeKind::Gate,
                uses: "cap.gate.cicd".to_string(),
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
            }],
        };

        let error = validate_workflow_capability_contracts(&template)
            .expect_err("legacy non-env cap labels should fail hard");
        assert!(
            error.contains("uses unknown label"),
            "unexpected error: {error}"
        );
        assert!(error.contains("cap.gate.cicd"), "unexpected error: {error}");
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
                        succeeded: vec!["stage_commit".to_string()],
                        ..Default::default()
                    },
                },
                WorkflowNode {
                    id: "stage_commit".to_string(),
                    kind: WorkflowNodeKind::Builtin,
                    uses: "cap.env.builtin.git.stage_commit".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
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
                        failed: vec!["stage_commit".to_string()],
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
    fn validate_capability_contracts_enforces_registered_non_empty_arg_requirements() {
        let template = WorkflowTemplate {
            id: "template.worktree".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![],
            nodes: vec![WorkflowNode {
                id: "worktree_prepare".to_string(),
                kind: WorkflowNodeKind::Builtin,
                uses: "cap.env.builtin.worktree.prepare".to_string(),
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
            .expect_err("worktree.prepare without args should fail queue-time validation");
        assert!(
            error.contains("executor `worktree.prepare`"),
            "unexpected error: {error}"
        );
        assert!(
            error.contains("requires at least one non-empty argument"),
            "unexpected error: {error}"
        );
        assert!(
            error.contains("args.branch")
                && error.contains("args.slug")
                && error.contains("args.plan"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn validate_capability_contracts_accepts_registered_non_empty_arg_requirements() {
        let template = WorkflowTemplate {
            id: "template.worktree".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![],
            nodes: vec![WorkflowNode {
                id: "worktree_prepare".to_string(),
                kind: WorkflowNodeKind::Builtin,
                uses: "cap.env.builtin.worktree.prepare".to_string(),
                args: BTreeMap::from([("slug".to_string(), "example-change".to_string())]),
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
            .expect("worktree.prepare with slug should satisfy arg requirement");
    }

    #[test]
    fn validate_capability_contracts_enforces_registered_non_empty_args_for_integrate_plan_branch()
    {
        let template = WorkflowTemplate {
            id: "template.merge".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![],
            nodes: vec![WorkflowNode {
                id: "merge_integrate".to_string(),
                kind: WorkflowNodeKind::Builtin,
                uses: "cap.env.builtin.git.integrate_plan_branch".to_string(),
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
            .expect_err("git.integrate_plan_branch without source args should fail");
        assert!(
            error.contains("executor `git.integrate_plan_branch`"),
            "unexpected error: {error}"
        );
        assert!(
            error.contains("requires at least one non-empty argument"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn validate_capability_contracts_rejects_cicd_run_without_command_or_gate_script() {
        let template = WorkflowTemplate {
            id: "template.cicd".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![],
            nodes: vec![WorkflowNode {
                id: "cicd_run".to_string(),
                kind: WorkflowNodeKind::Shell,
                uses: "cap.env.shell.cicd.run".to_string(),
                args: BTreeMap::new(),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: vec![WorkflowGate::Cicd {
                    script: "".to_string(),
                    auto_resolve: false,
                    policy: WorkflowGatePolicy::Retry,
                }],
                retry: WorkflowRetryPolicy::default(),
                on: WorkflowOutcomeEdges::default(),
            }],
        };
        let error = validate_workflow_capability_contracts(&template)
            .expect_err("cicd.run without script source should fail");
        assert!(
            error.contains("contract `cicd.run`"),
            "unexpected error: {error}"
        );
        assert!(
            error.contains("requires args.command/args.script or a non-empty cicd gate script"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn validate_capability_contracts_accepts_cicd_run_with_gate_script() {
        let template = WorkflowTemplate {
            id: "template.cicd".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![],
            nodes: vec![WorkflowNode {
                id: "cicd_run".to_string(),
                kind: WorkflowNodeKind::Shell,
                uses: "cap.env.shell.cicd.run".to_string(),
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
            }],
        };
        validate_workflow_capability_contracts(&template)
            .expect("cicd.run should accept non-empty cicd gate script");
    }

    #[test]
    fn validate_capability_contracts_rejects_patch_prepare_without_files() {
        let template = WorkflowTemplate {
            id: "template.patch".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: vec![],
            nodes: vec![WorkflowNode {
                id: "patch_prepare".to_string(),
                kind: WorkflowNodeKind::Builtin,
                uses: "cap.env.builtin.patch.pipeline_prepare".to_string(),
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
            .expect_err("patch prepare without files_json should fail");
        assert!(
            error.contains("patch.pipeline_prepare"),
            "unexpected error: {error}"
        );
        assert!(error.contains("files_json"), "unexpected error: {error}");
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
                    uses: "cap.env.builtin.git.stage_commit".to_string(),
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
                    uses: "control.gate.stop_condition".to_string(),
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
            error.contains("gate.stop_condition") || error.contains("git.stage_commit"),
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
                    uses: "control.gate.cicd".to_string(),
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
                },
            ],
        };

        let error = validate_workflow_capability_contracts(&template)
            .expect_err("auto-resolve cicd gate without retry closure should fail");
        assert!(error.contains("gate.cicd"), "unexpected error: {error}");
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
                    uses: "cap.env.builtin.git.integrate_plan_branch".to_string(),
                    args: BTreeMap::from([("slug".to_string(), "merge-plan".to_string())]),
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
                    uses: "control.gate.cicd".to_string(),
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
    fn validate_capability_contracts_accepts_review_node_with_canonical_invoke_contract() {
        let template = WorkflowTemplate {
            id: "template.review".to_string(),
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
                        succeeded: vec![prompt_artifact("review_main")],
                        ..Default::default()
                    },
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: Vec::new(),
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges::default(),
                },
                WorkflowNode {
                    id: "review_main".to_string(),
                    kind: WorkflowNodeKind::Agent,
                    uses: "cap.agent.invoke".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: vec![prompt_artifact("review_main")],
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
            .expect("canonical review invoke contract should validate");
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
                uses: "cap.env.builtin.patch.execute_pipeline".to_string(),
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
            error.contains("patch.execute_pipeline"),
            "unexpected error: {error}"
        );
        assert!(error.contains("files_json"), "unexpected error: {error}");
    }
}
