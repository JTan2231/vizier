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
            _ if !value.trim().is_empty() && !value.starts_with("vizier.") => {
                Some(Self::ExecCustomCommand)
            }
            _ => None,
        }
    }
}

pub fn workflow_node_capability(node: &WorkflowNode) -> Option<WorkflowCapability> {
    WorkflowCapability::from_uses_label(&node.uses)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledWorkflowNode {
    pub template_id: String,
    pub template_version: String,
    pub node_id: String,
    pub capability: Option<WorkflowCapability>,
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
        capability: workflow_node_capability(node),
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
                uses: "acme.custom.step".to_string(),
                args: BTreeMap::new(),
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
        assert_eq!(
            WorkflowCapability::from_uses_label("acme.shell.note"),
            Some(WorkflowCapability::ExecCustomCommand)
        );
        assert_eq!(
            WorkflowCapability::from_uses_label("custom.namespace.step"),
            Some(WorkflowCapability::ExecCustomCommand)
        );
        assert_eq!(
            WorkflowCapability::from_uses_label("vizier.unknown.label"),
            None,
            "unknown vizier labels should remain unmapped so runtime can reject/handle explicitly"
        );
    }
}
