use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use vizier_core::{
    config,
    workflow_template::{
        WorkflowAfterDependency, WorkflowArtifactContract, WorkflowCapability, WorkflowFailureMode,
        WorkflowGate, WorkflowGatePolicy, WorkflowNode, WorkflowNodeKind, WorkflowOutcomeArtifacts,
        WorkflowOutcomeEdges, WorkflowPrecondition, WorkflowResumePolicy, WorkflowResumeReuseMode,
        WorkflowRetryMode, WorkflowRetryPolicy, WorkflowTemplate, WorkflowTemplatePolicy,
        compile_workflow_node, validate_workflow_capability_contracts, workflow_node_capability,
    },
};

use crate::jobs;

#[derive(Clone, Copy, Debug)]
pub(crate) enum TemplateScope {
    Save,
    Draft,
    Approve,
    Review,
    Merge,
    BuildExecute,
    Patch,
}

impl TemplateScope {
    fn config_value(self, cfg: &config::Config) -> &str {
        match self {
            Self::Save => &cfg.workflow.templates.save,
            Self::Draft => &cfg.workflow.templates.draft,
            Self::Approve => &cfg.workflow.templates.approve,
            Self::Review => &cfg.workflow.templates.review,
            Self::Merge => &cfg.workflow.templates.merge,
            Self::BuildExecute => &cfg.workflow.templates.build_execute,
            Self::Patch => &cfg.workflow.templates.patch,
        }
    }

    fn fallback_id(self) -> &'static str {
        match self {
            Self::Save => "template.save",
            Self::Draft => "template.draft",
            Self::Approve => "template.approve",
            Self::Review => "template.review",
            Self::Merge => "template.merge",
            Self::BuildExecute => "template.build_execute",
            Self::Patch => "template.patch",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::Draft => "draft",
            Self::Approve => "approve",
            Self::Review => "review",
            Self::Merge => "merge",
            Self::BuildExecute => "build_execute",
            Self::Patch => "patch",
        }
    }

    fn supported_versions(self) -> &'static [&'static str] {
        &["v1"]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowTemplateRef {
    pub id: String,
    pub version: String,
    pub source_path: Option<PathBuf>,
}

pub(crate) fn resolve_template_ref(
    cfg: &config::Config,
    scope: TemplateScope,
) -> WorkflowTemplateRef {
    parse_template_ref(scope.config_value(cfg), scope.fallback_id())
}

pub(crate) fn template_scope_for_alias(alias: &config::CommandAlias) -> Option<TemplateScope> {
    match alias.as_str() {
        "save" => Some(TemplateScope::Save),
        "draft" => Some(TemplateScope::Draft),
        "approve" => Some(TemplateScope::Approve),
        "review" => Some(TemplateScope::Review),
        "merge" => Some(TemplateScope::Merge),
        "build_execute" => Some(TemplateScope::BuildExecute),
        "patch" => Some(TemplateScope::Patch),
        _ => None,
    }
}

pub(crate) fn resolve_template_ref_for_alias(
    cfg: &config::Config,
    alias: &config::CommandAlias,
) -> Option<(TemplateScope, WorkflowTemplateRef)> {
    let scope = template_scope_for_alias(alias)?;
    let selector = cfg
        .template_selector_for_alias(alias)
        .map(|value| value.to_string())
        .unwrap_or_else(|| scope.config_value(cfg).to_string());
    Some((scope, parse_template_ref(&selector, scope.fallback_id())))
}

pub(crate) fn parse_template_ref(raw: &str, fallback_id: &str) -> WorkflowTemplateRef {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return WorkflowTemplateRef {
            id: fallback_id.to_string(),
            version: "v1".to_string(),
            source_path: None,
        };
    }

    if let Some(path) = parse_template_path_selector(trimmed) {
        return WorkflowTemplateRef {
            id: fallback_id.to_string(),
            version: "v1".to_string(),
            source_path: Some(path),
        };
    }

    if let Some((id, version)) = trimmed.rsplit_once('@')
        && !id.trim().is_empty()
        && !version.trim().is_empty()
    {
        return WorkflowTemplateRef {
            id: id.trim().to_string(),
            version: version.trim().to_string(),
            source_path: None,
        };
    }

    if let Some((id, suffix)) = trimmed.rsplit_once(".v")
        && !id.trim().is_empty()
        && !suffix.trim().is_empty()
        && suffix.chars().all(|ch| ch.is_ascii_digit())
    {
        return WorkflowTemplateRef {
            id: id.trim().to_string(),
            version: format!("v{}", suffix.trim()),
            source_path: None,
        };
    }

    WorkflowTemplateRef {
        id: trimmed.to_string(),
        version: "v1".to_string(),
        source_path: None,
    }
}

fn parse_template_path_selector(raw: &str) -> Option<PathBuf> {
    if let Some(path) = raw.strip_prefix("file:") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    if looks_like_template_path(raw) {
        return Some(PathBuf::from(raw));
    }

    None
}

fn is_builtin_template_ref(template_ref: &WorkflowTemplateRef, scope: TemplateScope) -> bool {
    template_ref.id == scope.fallback_id()
        && scope
            .supported_versions()
            .contains(&template_ref.version.as_str())
}

fn canonical_primary_node_id(scope: TemplateScope) -> &'static str {
    match scope {
        TemplateScope::Save => "save_apply_in_worktree",
        TemplateScope::Draft => "draft_generate_plan",
        TemplateScope::Approve => "approve_apply_once",
        TemplateScope::Review => "review_critique",
        TemplateScope::Merge => "merge_integrate",
        TemplateScope::BuildExecute => "materialize",
        TemplateScope::Patch => "patch_execute",
    }
}

fn canonical_primary_node_capability(scope: TemplateScope) -> WorkflowCapability {
    match scope {
        TemplateScope::Save => WorkflowCapability::GitSaveWorktreePatch,
        TemplateScope::Draft => WorkflowCapability::PlanGenerateDraftPlan,
        TemplateScope::Approve => WorkflowCapability::PlanApplyOnce,
        TemplateScope::Review => WorkflowCapability::ReviewCritiqueOrFix,
        TemplateScope::Merge => WorkflowCapability::GitIntegratePlanBranch,
        TemplateScope::BuildExecute => WorkflowCapability::BuildMaterializeStep,
        TemplateScope::Patch => WorkflowCapability::PatchExecutePipeline,
    }
}

pub(crate) fn resolve_primary_template_node_id(
    template: &WorkflowTemplate,
    scope: TemplateScope,
) -> Result<String, Box<dyn std::error::Error>> {
    let canonical_id = canonical_primary_node_id(scope);
    if template.nodes.iter().any(|node| node.id == canonical_id) {
        return Ok(canonical_id.to_string());
    }

    let expected_capability = canonical_primary_node_capability(scope);
    let matching = template
        .nodes
        .iter()
        .filter(|node| workflow_node_capability(node) == Some(expected_capability))
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    match matching.as_slice() {
        [node_id] => Ok(node_id.clone()),
        [] => Err(format!(
            "workflow template {}@{} for scope `{}` is missing canonical node `{}` and no node maps to capability `{}`",
            template.id,
            template.version,
            scope.label(),
            canonical_id,
            expected_capability.id()
        )
        .into()),
        _ => Err(format!(
            "workflow template {}@{} for scope `{}` declares multiple `{}` nodes for capability `{}`: {}",
            template.id,
            template.version,
            scope.label(),
            canonical_id,
            expected_capability.id(),
            matching.join(", ")
        )
        .into()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkflowTemplateNodeSchedule {
    pub order: Vec<String>,
    pub terminal_nodes: Vec<String>,
}

pub(crate) fn compile_template_node_schedule(
    template: &WorkflowTemplate,
) -> Result<WorkflowTemplateNodeSchedule, Box<dyn std::error::Error>> {
    let mut by_id = BTreeMap::new();
    for node in &template.nodes {
        if by_id.insert(node.id.clone(), node).is_some() {
            return Err(format!(
                "workflow template {}@{} defines duplicate node id `{}`",
                template.id, template.version, node.id
            )
            .into());
        }
    }
    validate_workflow_capability_contracts(template)
        .map_err(|err| -> Box<dyn std::error::Error> { err.into() })?;

    let mut indegree = by_id
        .keys()
        .map(|id| (id.clone(), 0usize))
        .collect::<BTreeMap<_, _>>();
    let mut outgoing = BTreeMap::<String, Vec<String>>::new();
    for node in &template.nodes {
        for dependency in &node.after {
            if !by_id.contains_key(&dependency.node_id) {
                return Err(format!(
                    "workflow template {}@{} node `{}` references unknown after node `{}`",
                    template.id, template.version, node.id, dependency.node_id
                )
                .into());
            }
            let count = indegree.entry(node.id.clone()).or_insert(0);
            *count += 1;
            outgoing
                .entry(dependency.node_id.clone())
                .or_default()
                .push(node.id.clone());
        }
    }

    let mut ready = indegree
        .iter()
        .filter_map(|(id, count)| if *count == 0 { Some(id.clone()) } else { None })
        .collect::<BTreeSet<_>>();
    let mut order = Vec::new();

    while let Some(node_id) = ready.iter().next().cloned() {
        ready.remove(&node_id);
        order.push(node_id.clone());
        if let Some(children) = outgoing.get(&node_id) {
            for child in children {
                let entry = indegree
                    .get_mut(child)
                    .ok_or_else(|| format!("unknown node `{child}` in workflow graph"))?;
                if *entry > 0 {
                    *entry -= 1;
                    if *entry == 0 {
                        ready.insert(child.clone());
                    }
                }
            }
        }
    }

    if order.len() != template.nodes.len() {
        return Err(format!(
            "workflow template {}@{} has an after-dependency cycle",
            template.id, template.version
        )
        .into());
    }

    let mut terminal = template
        .nodes
        .iter()
        .filter_map(|node| {
            let has_children = outgoing
                .get(&node.id)
                .map(|children| !children.is_empty())
                .unwrap_or(false);
            if has_children {
                None
            } else {
                Some(node.id.clone())
            }
        })
        .collect::<Vec<_>>();
    terminal.sort();
    terminal.dedup();

    Ok(WorkflowTemplateNodeSchedule {
        order,
        terminal_nodes: terminal,
    })
}

fn normalize_selector_component(value: &str) -> String {
    let mut normalized = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    while normalized.contains("__") {
        normalized = normalized.replace("__", "_");
    }
    normalized.trim_matches('_').to_string()
}

fn workflow_selector_candidate_paths(template_ref: &WorkflowTemplateRef) -> Vec<PathBuf> {
    let id = normalize_selector_component(&template_ref.id);
    let version = normalize_selector_component(&template_ref.version);
    if id.is_empty() || version.is_empty() {
        return Vec::new();
    }

    let selector = format!("{id}@{version}");
    vec![
        PathBuf::from(format!(".vizier/workflow/{selector}.json")),
        PathBuf::from(format!(".vizier/workflow/{selector}.toml")),
        PathBuf::from(format!(".vizier/workflow/templates/{selector}.json")),
        PathBuf::from(format!(".vizier/workflow/templates/{selector}.toml")),
        PathBuf::from(format!(".vizier/workflow/templates/{id}/{version}.json")),
        PathBuf::from(format!(".vizier/workflow/templates/{id}/{version}.toml")),
    ]
}

fn looks_like_template_path(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return false;
    }

    trimmed.starts_with('.')
        || trimmed.starts_with('~')
        || trimmed.starts_with('/')
        || trimmed.contains(std::path::MAIN_SEPARATOR)
        || trimmed.ends_with(".json")
        || trimmed.ends_with(".toml")
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledTemplateNode {
    pub schedule: jobs::JobSchedule,
    pub template_id: String,
    pub template_version: String,
    pub node_id: String,
    pub capability_id: Option<String>,
    pub policy_snapshot_hash: String,
    pub gate_labels: Vec<String>,
    pub gates: Vec<WorkflowGate>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct BuildExecuteGateConfig {
    pub review_cicd_script: Option<String>,
    pub merge_cicd_script: Option<String>,
    pub merge_cicd_auto_resolve: bool,
    pub merge_cicd_retries: u32,
}

impl BuildExecuteGateConfig {
    pub(crate) fn from_merge_config(gate: &config::MergeCicdGateConfig) -> Self {
        let script = gate.script.as_ref().map(|path| path.display().to_string());
        Self {
            review_cicd_script: script.clone(),
            merge_cicd_script: script,
            merge_cicd_auto_resolve: gate.auto_resolve,
            merge_cicd_retries: gate.retries,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MergeTemplateGateConfig<'a> {
    pub cicd_script: Option<&'a str>,
    pub cicd_auto_resolve: bool,
    pub cicd_retries: u32,
    pub conflict_auto_resolve: bool,
}

pub(crate) fn validate_template_agent_backends(
    template: &WorkflowTemplate,
    cfg: &config::Config,
    cli_override: Option<&config::AgentOverrides>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut required_aliases = BTreeSet::new();
    for node in &template.nodes {
        let Some(capability) = workflow_node_capability(node) else {
            continue;
        };
        match capability {
            WorkflowCapability::PlanGenerateDraftPlan => {
                required_aliases.insert("draft");
            }
            WorkflowCapability::PlanApplyOnce => {
                required_aliases.insert("approve");
            }
            WorkflowCapability::ReviewCritiqueOrFix | WorkflowCapability::ReviewApplyFixesOnly => {
                required_aliases.insert("review");
            }
            WorkflowCapability::RemediationCicdAutoFix => {
                required_aliases.insert("merge");
            }
            WorkflowCapability::GateConflictResolution => {
                if capability_auto_resolve_enabled(node) {
                    required_aliases.insert("merge");
                }
            }
            WorkflowCapability::GateCicd => {
                if node.gates.iter().any(|gate| {
                    matches!(
                        gate,
                        WorkflowGate::Cicd {
                            auto_resolve: true,
                            ..
                        }
                    )
                }) {
                    required_aliases.insert("merge");
                }
            }
            _ => {}
        }
    }

    for alias_name in required_aliases {
        let alias = alias_name
            .parse::<config::CommandAlias>()
            .map_err(|err| format!("invalid command alias `{alias_name}`: {err}"))?;
        let settings = config::resolve_agent_settings_for_alias(cfg, &alias, cli_override)?;
        if !settings.backend.requires_agent_runner() {
            return Err(format!(
                "workflow template {}@{} requires an agent-capable backend for alias `{}` (capability contract preflight)",
                template.id, template.version, alias_name
            )
            .into());
        }
    }

    Ok(())
}

fn capability_auto_resolve_enabled(node: &WorkflowNode) -> bool {
    if let Some(raw) = node.args.get("auto_resolve")
        && let Some(value) = parse_bool_like(raw)
    {
        return value;
    }
    node.gates.iter().any(|gate| {
        matches!(gate, WorkflowGate::Custom { id, args, .. } if id == "conflict_resolution"
            && args
                .get("auto_resolve")
                .and_then(|value| parse_bool_like(value))
                .unwrap_or(false))
    })
}

fn parse_bool_like(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

pub(crate) fn compile_template_node(
    template: &WorkflowTemplate,
    node_id: &str,
    resolved_after: &BTreeMap<String, String>,
    pinned_head: Option<jobs::PinnedHead>,
) -> Result<CompiledTemplateNode, Box<dyn std::error::Error>> {
    let compiled = compile_workflow_node(template, node_id, resolved_after)
        .map_err(|err| format!("compile workflow node {node_id}: {err}"))?;

    let mut schedule = jobs::JobSchedule {
        after: compiled.after,
        dependencies: compiled
            .dependencies
            .iter()
            .cloned()
            .map(|artifact| jobs::JobDependency { artifact })
            .collect(),
        locks: compiled.locks,
        artifacts: compiled.artifacts,
        pinned_head: None,
        preconditions: Vec::new(),
        approval: None,
        wait_reason: None,
        waited_on: Vec::new(),
    };

    for precondition in &compiled.preconditions {
        match precondition {
            WorkflowPrecondition::PinnedHead => {
                let pinned = pinned_head.clone().ok_or_else(|| {
                    format!(
                        "template {}@{} node `{}` requires a pinned_head precondition",
                        compiled.template_id, compiled.template_version, compiled.node_id
                    )
                })?;
                schedule.pinned_head = Some(pinned);
            }
            WorkflowPrecondition::CleanWorktree => {
                schedule
                    .preconditions
                    .push(jobs::JobPrecondition::CleanWorktree);
            }
            WorkflowPrecondition::BranchExists => {
                let branch = schedule
                    .pinned_head
                    .as_ref()
                    .map(|value| value.branch.clone())
                    .or_else(|| infer_branch_from_locks(&schedule.locks));
                schedule
                    .preconditions
                    .push(jobs::JobPrecondition::BranchExists { branch });
            }
            WorkflowPrecondition::Custom { id, args } => {
                schedule.preconditions.push(jobs::JobPrecondition::Custom {
                    id: id.clone(),
                    args: args.clone(),
                });
            }
        }
    }

    let mut gate_labels = Vec::new();
    for gate in &compiled.gates {
        gate_labels.push(workflow_gate_label(gate));
        if let WorkflowGate::Approval { required, policy } = gate
            && *required
            && matches!(policy, WorkflowGatePolicy::Block)
            && schedule.approval.is_none()
        {
            schedule.approval = Some(jobs::pending_job_approval());
        }
    }

    Ok(CompiledTemplateNode {
        schedule,
        template_id: compiled.template_id,
        template_version: compiled.template_version,
        node_id: compiled.node_id,
        capability_id: compiled.capability.map(|value| value.id().to_string()),
        policy_snapshot_hash: compiled.policy_snapshot_hash,
        gate_labels,
        gates: compiled.gates,
    })
}

fn infer_branch_from_locks(locks: &[jobs::JobLock]) -> Option<String> {
    let mut branches = locks
        .iter()
        .filter_map(|lock| lock.key.strip_prefix("branch:"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    branches.sort();
    branches.dedup();
    if branches.len() == 1 {
        branches.pop()
    } else {
        None
    }
}

fn render_template_string(value: &str, params: &BTreeMap<String, String>) -> String {
    let mut rendered = value.to_string();
    for (key, replacement) in params {
        let marker = format!("${{{key}}}");
        if rendered.contains(&marker) {
            rendered = rendered.replace(&marker, replacement);
        }
    }
    rendered
}

fn render_json_value(value: &mut serde_json::Value, params: &BTreeMap<String, String>) {
    match value {
        serde_json::Value::String(text) => {
            *text = render_template_string(text, params);
        }
        serde_json::Value::Array(values) => {
            for entry in values {
                render_json_value(entry, params);
            }
        }
        serde_json::Value::Object(map) => {
            for entry in map.values_mut() {
                render_json_value(entry, params);
            }
        }
        _ => {}
    }
}

fn render_artifact(artifact: &mut jobs::JobArtifact, params: &BTreeMap<String, String>) {
    match artifact {
        jobs::JobArtifact::PlanBranch { slug, branch }
        | jobs::JobArtifact::PlanDoc { slug, branch }
        | jobs::JobArtifact::PlanCommits { slug, branch } => {
            *slug = render_template_string(slug, params);
            *branch = render_template_string(branch, params);
        }
        jobs::JobArtifact::TargetBranch { name } => {
            *name = render_template_string(name, params);
        }
        jobs::JobArtifact::MergeSentinel { slug } => {
            *slug = render_template_string(slug, params);
        }
        jobs::JobArtifact::CommandPatch { job_id } => {
            *job_id = render_template_string(job_id, params);
        }
        jobs::JobArtifact::Custom { type_id, key } => {
            *type_id = render_template_string(type_id, params);
            *key = render_template_string(key, params);
        }
    }
}

fn render_template(template: &mut WorkflowTemplate, runtime_params: &BTreeMap<String, String>) {
    let mut params = template.params.clone();
    for (key, value) in runtime_params {
        params.insert(key.clone(), value.clone());
    }

    let keys = params.keys().cloned().collect::<Vec<_>>();
    for key in keys {
        if let Some(current) = params.get(&key).cloned() {
            params.insert(key, render_template_string(&current, &params));
        }
    }
    template.params = params.clone();

    template.id = render_template_string(&template.id, &params);
    template.version = render_template_string(&template.version, &params);
    template.policy.resume.key = render_template_string(&template.policy.resume.key, &params);

    for contract in &mut template.artifact_contracts {
        contract.id = render_template_string(&contract.id, &params);
        contract.version = render_template_string(&contract.version, &params);
        if let Some(schema) = contract.schema.as_mut() {
            render_json_value(schema, &params);
        }
    }

    for node in &mut template.nodes {
        node.id = render_template_string(&node.id, &params);
        node.uses = render_template_string(&node.uses, &params);
        for value in node.args.values_mut() {
            *value = render_template_string(value, &params);
        }
        for dependency in &mut node.after {
            dependency.node_id = render_template_string(&dependency.node_id, &params);
        }
        for artifact in &mut node.needs {
            render_artifact(artifact, &params);
        }
        for artifact in &mut node.produces.succeeded {
            render_artifact(artifact, &params);
        }
        for artifact in &mut node.produces.failed {
            render_artifact(artifact, &params);
        }
        for artifact in &mut node.produces.blocked {
            render_artifact(artifact, &params);
        }
        for artifact in &mut node.produces.cancelled {
            render_artifact(artifact, &params);
        }
        for lock in &mut node.locks {
            lock.key = render_template_string(&lock.key, &params);
        }
        for precondition in &mut node.preconditions {
            if let WorkflowPrecondition::Custom { id, args } = precondition {
                *id = render_template_string(id, &params);
                for value in args.values_mut() {
                    *value = render_template_string(value, &params);
                }
            }
        }
        for gate in &mut node.gates {
            match gate {
                WorkflowGate::Approval { .. } => {}
                WorkflowGate::Script { script, .. } | WorkflowGate::Cicd { script, .. } => {
                    *script = render_template_string(script, &params);
                }
                WorkflowGate::Custom { id, args, .. } => {
                    *id = render_template_string(id, &params);
                    for value in args.values_mut() {
                        *value = render_template_string(value, &params);
                    }
                }
            }
        }
        for node_id in &mut node.on.succeeded {
            *node_id = render_template_string(node_id, &params);
        }
        for node_id in &mut node.on.failed {
            *node_id = render_template_string(node_id, &params);
        }
        for node_id in &mut node.on.blocked {
            *node_id = render_template_string(node_id, &params);
        }
        for node_id in &mut node.on.cancelled {
            *node_id = render_template_string(node_id, &params);
        }
    }
}

fn parse_template_contents(
    path: &Path,
    contents: &str,
) -> Result<WorkflowTemplate, Box<dyn std::error::Error>> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match extension.as_str() {
        "json" => Ok(serde_json::from_str(contents)?),
        "toml" => Ok(toml::from_str(contents)?),
        _ => serde_json::from_str(contents)
            .or_else(|_| toml::from_str(contents))
            .map_err(|err| {
                format!(
                    "template {} is not valid JSON or TOML: {}",
                    path.display(),
                    err
                )
                .into()
            }),
    }
}

fn resolve_template_file(
    template_ref: &WorkflowTemplateRef,
    runtime_params: &BTreeMap<String, String>,
) -> Result<Option<WorkflowTemplate>, Box<dyn std::error::Error>> {
    let Some(path) = template_ref.source_path.as_ref() else {
        return Ok(None);
    };

    let raw = fs::read_to_string(path)
        .map_err(|err| format!("read workflow template file {}: {}", path.display(), err))?;
    let mut template = parse_template_contents(path, &raw)?;
    render_template(&mut template, runtime_params);
    if template.id.trim().is_empty() {
        template.id = template_ref.id.clone();
    }
    if template.version.trim().is_empty() {
        template.version = template_ref.version.clone();
    }
    Ok(Some(template))
}

fn resolve_selector_template_file_from_base(
    base_dir: &Path,
    template_ref: &WorkflowTemplateRef,
    runtime_params: &BTreeMap<String, String>,
) -> Result<Option<WorkflowTemplate>, Box<dyn std::error::Error>> {
    if template_ref.source_path.is_some() {
        return Ok(None);
    }

    for candidate in workflow_selector_candidate_paths(template_ref) {
        let path = base_dir.join(&candidate);
        if !path.exists() {
            continue;
        }

        let raw = fs::read_to_string(&path).map_err(|err| {
            format!(
                "read workflow template selector {}: {}",
                path.display(),
                err
            )
        })?;
        let mut template = parse_template_contents(&path, &raw)?;
        render_template(&mut template, runtime_params);
        if template.id.trim().is_empty() {
            template.id = template_ref.id.clone();
        }
        if template.version.trim().is_empty() {
            template.version = template_ref.version.clone();
        }
        return Ok(Some(template));
    }

    Ok(None)
}

fn resolve_selector_template_file(
    template_ref: &WorkflowTemplateRef,
    runtime_params: &BTreeMap<String, String>,
) -> Result<Option<WorkflowTemplate>, Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    resolve_selector_template_file_from_base(&cwd, template_ref, runtime_params)
}

fn resolve_template_instance(
    template_ref: &WorkflowTemplateRef,
    runtime_params: &BTreeMap<String, String>,
    scope: TemplateScope,
    builtin: WorkflowTemplate,
) -> Result<WorkflowTemplate, Box<dyn std::error::Error>> {
    if let Some(template) = resolve_template_file(template_ref, runtime_params)? {
        return Ok(template);
    }

    if is_builtin_template_ref(template_ref, scope) {
        return Ok(builtin);
    }

    if let Some(template) = resolve_selector_template_file(template_ref, runtime_params)? {
        return Ok(template);
    }

    let supported = scope
        .supported_versions()
        .iter()
        .map(|version| format!("{}@{}", scope.fallback_id(), version))
        .collect::<Vec<_>>()
        .join(", ");
    let searched = workflow_selector_candidate_paths(template_ref)
        .into_iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "workflow template selector `{}@{}` for scope `{}` did not match built-ins ({}) and no selector file was found (searched: {}); use file:<path> for explicit template files",
        template_ref.id,
        template_ref.version,
        scope.label(),
        supported,
        if searched.is_empty() { "none".to_string() } else { searched }
    )
    .into())
}

pub(crate) fn resolve_save_template(
    template_ref: &WorkflowTemplateRef,
    branch: &str,
    job_id: &str,
) -> Result<WorkflowTemplate, Box<dyn std::error::Error>> {
    let builtin = save_template(template_ref, branch, job_id);
    let mut params = builtin.params.clone();
    params.insert("target".to_string(), branch.to_string());
    resolve_template_instance(template_ref, &params, TemplateScope::Save, builtin)
}

pub(crate) fn resolve_draft_template(
    template_ref: &WorkflowTemplateRef,
    slug: &str,
    branch: &str,
    job_id: &str,
) -> Result<WorkflowTemplate, Box<dyn std::error::Error>> {
    let builtin = draft_template(template_ref, slug, branch, job_id);
    let mut params = builtin.params.clone();
    params.insert("job_id".to_string(), job_id.to_string());
    resolve_template_instance(template_ref, &params, TemplateScope::Draft, builtin)
}

pub(crate) fn resolve_approve_template(
    template_ref: &WorkflowTemplateRef,
    slug: &str,
    branch: &str,
    job_id: &str,
    require_human_approval: bool,
    stop_condition_script: Option<&str>,
    stop_condition_retries: u32,
) -> Result<WorkflowTemplate, Box<dyn std::error::Error>> {
    let builtin = approve_template(
        template_ref,
        slug,
        branch,
        job_id,
        require_human_approval,
        stop_condition_script,
        stop_condition_retries,
    );
    let mut params = builtin.params.clone();
    params.insert(
        "require_human_approval".to_string(),
        require_human_approval.to_string(),
    );
    params.insert(
        "stop_condition_retries".to_string(),
        stop_condition_retries.to_string(),
    );
    if let Some(script) = stop_condition_script {
        params.insert("stop_condition_script".to_string(), script.to_string());
    }
    resolve_template_instance(template_ref, &params, TemplateScope::Approve, builtin)
}

pub(crate) fn resolve_review_template(
    template_ref: &WorkflowTemplateRef,
    slug: &str,
    branch: &str,
    job_id: &str,
    cicd_probe_script: Option<&str>,
) -> Result<WorkflowTemplate, Box<dyn std::error::Error>> {
    let builtin = review_template(template_ref, slug, branch, job_id, cicd_probe_script);
    let mut params = builtin.params.clone();
    params.insert("job_id".to_string(), job_id.to_string());
    if let Some(script) = cicd_probe_script {
        params.insert("cicd_script".to_string(), script.to_string());
    }
    resolve_template_instance(template_ref, &params, TemplateScope::Review, builtin)
}

pub(crate) fn resolve_merge_template(
    template_ref: &WorkflowTemplateRef,
    slug: &str,
    branch: &str,
    target_branch: &str,
    gate_config: MergeTemplateGateConfig<'_>,
) -> Result<WorkflowTemplate, Box<dyn std::error::Error>> {
    let builtin = merge_template(template_ref, slug, branch, target_branch, gate_config);
    let mut params = builtin.params.clone();
    params.insert("target".to_string(), target_branch.to_string());
    params.insert(
        "cicd_auto_resolve".to_string(),
        gate_config.cicd_auto_resolve.to_string(),
    );
    params.insert(
        "cicd_retries".to_string(),
        gate_config.cicd_retries.to_string(),
    );
    params.insert(
        "conflict_auto_resolve".to_string(),
        gate_config.conflict_auto_resolve.to_string(),
    );
    if let Some(script) = gate_config.cicd_script {
        params.insert("cicd_script".to_string(), script.to_string());
    }
    resolve_template_instance(template_ref, &params, TemplateScope::Merge, builtin)
}

pub(crate) fn resolve_patch_template(
    template_ref: &WorkflowTemplateRef,
    pipeline: &str,
    target: Option<&str>,
    resume: bool,
) -> Result<WorkflowTemplate, Box<dyn std::error::Error>> {
    let builtin = patch_template(template_ref, pipeline, target, resume);
    let mut params = builtin.params.clone();
    if let Some(target_branch) = target {
        params.insert("target".to_string(), target_branch.to_string());
    }
    resolve_template_instance(template_ref, &params, TemplateScope::Patch, builtin)
}

pub(crate) fn resolve_build_execute_template(
    template_ref: &WorkflowTemplateRef,
    slug: &str,
    branch: &str,
    target_branch: &str,
    include_review: bool,
    include_merge: bool,
    gate_config: &BuildExecuteGateConfig,
) -> Result<WorkflowTemplate, Box<dyn std::error::Error>> {
    let builtin = build_execute_template(
        template_ref,
        slug,
        branch,
        target_branch,
        include_review,
        include_merge,
        gate_config,
    );
    let mut params = builtin.params.clone();
    params.insert("include_review".to_string(), include_review.to_string());
    params.insert("include_merge".to_string(), include_merge.to_string());
    params.insert("target".to_string(), target_branch.to_string());
    if let Some(script) = gate_config.review_cicd_script.as_ref() {
        params.insert("review_cicd_script".to_string(), script.clone());
    }
    if let Some(script) = gate_config.merge_cicd_script.as_ref() {
        params.insert("merge_cicd_script".to_string(), script.clone());
    }
    params.insert(
        "merge_cicd_auto_resolve".to_string(),
        gate_config.merge_cicd_auto_resolve.to_string(),
    );
    params.insert(
        "merge_cicd_retries".to_string(),
        gate_config.merge_cicd_retries.to_string(),
    );
    resolve_template_instance(template_ref, &params, TemplateScope::BuildExecute, builtin)
}

pub(crate) fn save_template(
    template_ref: &WorkflowTemplateRef,
    branch: &str,
    job_id: &str,
) -> WorkflowTemplate {
    WorkflowTemplate {
        id: template_ref.id.clone(),
        version: template_ref.version.clone(),
        params: BTreeMap::from([
            ("branch".to_string(), branch.to_string()),
            ("job_id".to_string(), job_id.to_string()),
        ]),
        policy: default_template_policy(),
        artifact_contracts: vec![artifact_contract("command_patch", "v1")],
        nodes: vec![WorkflowNode {
            id: "save_apply_in_worktree".to_string(),
            kind: WorkflowNodeKind::Builtin,
            uses: "vizier.save.apply".to_string(),
            args: BTreeMap::new(),
            after: Vec::new(),
            needs: Vec::new(),
            produces: WorkflowOutcomeArtifacts {
                succeeded: vec![jobs::JobArtifact::CommandPatch {
                    job_id: job_id.to_string(),
                }],
                ..Default::default()
            },
            locks: vec![
                jobs::JobLock {
                    key: "repo_serial".to_string(),
                    mode: jobs::LockMode::Exclusive,
                },
                jobs::JobLock {
                    key: format!("branch:{branch}"),
                    mode: jobs::LockMode::Exclusive,
                },
                jobs::JobLock {
                    key: format!("temp_worktree:{job_id}"),
                    mode: jobs::LockMode::Exclusive,
                },
            ],
            preconditions: vec![WorkflowPrecondition::PinnedHead],
            gates: Vec::new(),
            retry: WorkflowRetryPolicy::default(),
            on: Default::default(),
        }],
    }
}

pub(crate) fn draft_template(
    template_ref: &WorkflowTemplateRef,
    slug: &str,
    branch: &str,
    job_id: &str,
) -> WorkflowTemplate {
    WorkflowTemplate {
        id: template_ref.id.clone(),
        version: template_ref.version.clone(),
        params: BTreeMap::from([
            ("slug".to_string(), slug.to_string()),
            ("branch".to_string(), branch.to_string()),
        ]),
        policy: default_template_policy(),
        artifact_contracts: vec![
            artifact_contract("plan_branch", "v1"),
            artifact_contract("plan_doc", "v1"),
        ],
        nodes: vec![WorkflowNode {
            id: "draft_generate_plan".to_string(),
            kind: WorkflowNodeKind::Builtin,
            uses: "vizier.draft.generate_plan".to_string(),
            args: BTreeMap::new(),
            after: Vec::new(),
            needs: Vec::new(),
            produces: WorkflowOutcomeArtifacts {
                succeeded: vec![
                    jobs::JobArtifact::PlanBranch {
                        slug: slug.to_string(),
                        branch: branch.to_string(),
                    },
                    jobs::JobArtifact::PlanDoc {
                        slug: slug.to_string(),
                        branch: branch.to_string(),
                    },
                ],
                ..Default::default()
            },
            locks: vec![
                jobs::JobLock {
                    key: format!("branch:{branch}"),
                    mode: jobs::LockMode::Exclusive,
                },
                jobs::JobLock {
                    key: format!("temp_worktree:{job_id}"),
                    mode: jobs::LockMode::Exclusive,
                },
            ],
            preconditions: Vec::new(),
            gates: Vec::new(),
            retry: WorkflowRetryPolicy::default(),
            on: Default::default(),
        }],
    }
}

pub(crate) fn approve_template(
    template_ref: &WorkflowTemplateRef,
    slug: &str,
    branch: &str,
    job_id: &str,
    require_human_approval: bool,
    stop_condition_script: Option<&str>,
    stop_condition_retries: u32,
) -> WorkflowTemplate {
    let mut gates = vec![WorkflowGate::Approval {
        required: require_human_approval,
        policy: WorkflowGatePolicy::Block,
    }];
    if let Some(script) = stop_condition_script {
        gates.push(WorkflowGate::Script {
            script: script.to_string(),
            policy: WorkflowGatePolicy::Retry,
        });
    }
    let retry = if stop_condition_script.is_some() {
        WorkflowRetryPolicy {
            mode: WorkflowRetryMode::UntilGate,
            budget: stop_condition_retries,
        }
    } else {
        WorkflowRetryPolicy::default()
    };
    let mut apply_on = WorkflowOutcomeEdges::default();
    if stop_condition_script.is_some() {
        apply_on
            .succeeded
            .push("approve_gate_stop_condition".to_string());
    }

    let mut nodes = vec![WorkflowNode {
        id: "approve_apply_once".to_string(),
        kind: WorkflowNodeKind::Builtin,
        uses: "vizier.approve.apply_once".to_string(),
        args: BTreeMap::new(),
        after: Vec::new(),
        needs: vec![jobs::JobArtifact::PlanDoc {
            slug: slug.to_string(),
            branch: branch.to_string(),
        }],
        produces: WorkflowOutcomeArtifacts {
            succeeded: vec![jobs::JobArtifact::PlanCommits {
                slug: slug.to_string(),
                branch: branch.to_string(),
            }],
            ..Default::default()
        },
        locks: vec![
            jobs::JobLock {
                key: format!("branch:{branch}"),
                mode: jobs::LockMode::Exclusive,
            },
            jobs::JobLock {
                key: format!("temp_worktree:{job_id}"),
                mode: jobs::LockMode::Exclusive,
            },
        ],
        preconditions: Vec::new(),
        gates,
        retry,
        on: apply_on,
    }];

    if let Some(script) = stop_condition_script {
        nodes.push(WorkflowNode {
            id: "approve_gate_stop_condition".to_string(),
            kind: WorkflowNodeKind::Gate,
            uses: "vizier.approve.stop_condition".to_string(),
            args: BTreeMap::new(),
            after: vec![WorkflowAfterDependency {
                node_id: "approve_apply_once".to_string(),
                policy: jobs::AfterPolicy::Success,
            }],
            needs: Vec::new(),
            produces: WorkflowOutcomeArtifacts::default(),
            locks: Vec::new(),
            preconditions: Vec::new(),
            gates: vec![WorkflowGate::Script {
                script: script.to_string(),
                policy: WorkflowGatePolicy::Retry,
            }],
            retry: WorkflowRetryPolicy {
                mode: WorkflowRetryMode::UntilGate,
                budget: stop_condition_retries,
            },
            on: WorkflowOutcomeEdges {
                failed: vec!["approve_apply_once".to_string()],
                ..Default::default()
            },
        });
    }

    WorkflowTemplate {
        id: template_ref.id.clone(),
        version: template_ref.version.clone(),
        params: BTreeMap::from([
            ("slug".to_string(), slug.to_string()),
            ("branch".to_string(), branch.to_string()),
        ]),
        policy: default_template_policy(),
        artifact_contracts: vec![
            artifact_contract("plan_doc", "v1"),
            artifact_contract("plan_commits", "v1"),
        ],
        nodes,
    }
}

pub(crate) fn review_template(
    template_ref: &WorkflowTemplateRef,
    slug: &str,
    branch: &str,
    job_id: &str,
    cicd_probe_script: Option<&str>,
) -> WorkflowTemplate {
    let mut gates = Vec::new();
    if let Some(script) = cicd_probe_script {
        gates.push(WorkflowGate::Cicd {
            script: script.to_string(),
            auto_resolve: false,
            policy: WorkflowGatePolicy::Warn,
        });
    }

    WorkflowTemplate {
        id: template_ref.id.clone(),
        version: template_ref.version.clone(),
        params: BTreeMap::from([
            ("slug".to_string(), slug.to_string()),
            ("branch".to_string(), branch.to_string()),
        ]),
        policy: default_template_policy(),
        artifact_contracts: vec![
            artifact_contract("plan_branch", "v1"),
            artifact_contract("plan_doc", "v1"),
            artifact_contract("plan_commits", "v1"),
        ],
        nodes: vec![WorkflowNode {
            id: "review_critique".to_string(),
            kind: WorkflowNodeKind::Agent,
            uses: "vizier.review.critique".to_string(),
            args: BTreeMap::new(),
            after: Vec::new(),
            needs: vec![
                jobs::JobArtifact::PlanBranch {
                    slug: slug.to_string(),
                    branch: branch.to_string(),
                },
                jobs::JobArtifact::PlanDoc {
                    slug: slug.to_string(),
                    branch: branch.to_string(),
                },
            ],
            produces: WorkflowOutcomeArtifacts {
                succeeded: vec![jobs::JobArtifact::PlanCommits {
                    slug: slug.to_string(),
                    branch: branch.to_string(),
                }],
                ..Default::default()
            },
            locks: vec![
                jobs::JobLock {
                    key: format!("branch:{branch}"),
                    mode: jobs::LockMode::Exclusive,
                },
                jobs::JobLock {
                    key: format!("temp_worktree:{job_id}"),
                    mode: jobs::LockMode::Exclusive,
                },
            ],
            preconditions: Vec::new(),
            gates,
            retry: WorkflowRetryPolicy::default(),
            on: Default::default(),
        }],
    }
}

pub(crate) fn merge_template(
    template_ref: &WorkflowTemplateRef,
    slug: &str,
    branch: &str,
    target_branch: &str,
    gate_config: MergeTemplateGateConfig<'_>,
) -> WorkflowTemplate {
    let mut gates = Vec::new();
    if let Some(script) = gate_config.cicd_script {
        gates.push(WorkflowGate::Cicd {
            script: script.to_string(),
            auto_resolve: gate_config.cicd_auto_resolve,
            policy: WorkflowGatePolicy::Retry,
        });
    }
    let retry = if gate_config.cicd_script.is_some() {
        WorkflowRetryPolicy {
            mode: WorkflowRetryMode::UntilGate,
            budget: gate_config.cicd_retries,
        }
    } else {
        WorkflowRetryPolicy::default()
    };
    let mut integrate_on = WorkflowOutcomeEdges::default();
    if gate_config.cicd_script.is_some() {
        integrate_on.succeeded.push("merge_gate_cicd".to_string());
    }
    integrate_on
        .blocked
        .push("merge_conflict_resolution".to_string());

    let mut nodes = vec![WorkflowNode {
        id: "merge_integrate".to_string(),
        kind: WorkflowNodeKind::Builtin,
        uses: "vizier.merge.integrate".to_string(),
        args: BTreeMap::new(),
        after: Vec::new(),
        needs: vec![jobs::JobArtifact::PlanBranch {
            slug: slug.to_string(),
            branch: branch.to_string(),
        }],
        produces: WorkflowOutcomeArtifacts {
            succeeded: vec![jobs::JobArtifact::TargetBranch {
                name: target_branch.to_string(),
            }],
            blocked: vec![jobs::JobArtifact::MergeSentinel {
                slug: slug.to_string(),
            }],
            ..Default::default()
        },
        locks: vec![
            jobs::JobLock {
                key: format!("branch:{target_branch}"),
                mode: jobs::LockMode::Exclusive,
            },
            jobs::JobLock {
                key: format!("branch:{branch}"),
                mode: jobs::LockMode::Exclusive,
            },
            jobs::JobLock {
                key: format!("merge_sentinel:{slug}"),
                mode: jobs::LockMode::Exclusive,
            },
        ],
        preconditions: Vec::new(),
        gates,
        retry,
        on: integrate_on,
    }];
    nodes.push(WorkflowNode {
        id: "merge_conflict_resolution".to_string(),
        kind: WorkflowNodeKind::Gate,
        uses: "vizier.merge.conflict_resolution".to_string(),
        args: BTreeMap::from([(
            "auto_resolve".to_string(),
            gate_config.conflict_auto_resolve.to_string(),
        )]),
        after: vec![WorkflowAfterDependency {
            node_id: "merge_integrate".to_string(),
            policy: jobs::AfterPolicy::Success,
        }],
        needs: Vec::new(),
        produces: WorkflowOutcomeArtifacts::default(),
        locks: Vec::new(),
        preconditions: Vec::new(),
        gates: vec![WorkflowGate::Custom {
            id: "conflict_resolution".to_string(),
            policy: WorkflowGatePolicy::Block,
            args: BTreeMap::from([(
                "auto_resolve".to_string(),
                gate_config.conflict_auto_resolve.to_string(),
            )]),
        }],
        retry: WorkflowRetryPolicy::default(),
        on: WorkflowOutcomeEdges {
            succeeded: vec!["merge_integrate".to_string()],
            ..Default::default()
        },
    });

    if let Some(script) = gate_config.cicd_script {
        let mut gate_on = WorkflowOutcomeEdges::default();
        if gate_config.cicd_auto_resolve {
            gate_on.failed.push("merge_cicd_auto_fix".to_string());
        }
        nodes.push(WorkflowNode {
            id: "merge_gate_cicd".to_string(),
            kind: WorkflowNodeKind::Gate,
            uses: "vizier.merge.cicd_gate".to_string(),
            args: BTreeMap::new(),
            after: vec![WorkflowAfterDependency {
                node_id: "merge_integrate".to_string(),
                policy: jobs::AfterPolicy::Success,
            }],
            needs: Vec::new(),
            produces: WorkflowOutcomeArtifacts::default(),
            locks: Vec::new(),
            preconditions: Vec::new(),
            gates: vec![WorkflowGate::Cicd {
                script: script.to_string(),
                auto_resolve: gate_config.cicd_auto_resolve,
                policy: WorkflowGatePolicy::Retry,
            }],
            retry: WorkflowRetryPolicy {
                mode: WorkflowRetryMode::UntilGate,
                budget: gate_config.cicd_retries,
            },
            on: gate_on,
        });

        if gate_config.cicd_auto_resolve {
            nodes.push(WorkflowNode {
                id: "merge_cicd_auto_fix".to_string(),
                kind: WorkflowNodeKind::Agent,
                uses: "vizier.merge.cicd_auto_fix".to_string(),
                args: BTreeMap::new(),
                after: vec![WorkflowAfterDependency {
                    node_id: "merge_gate_cicd".to_string(),
                    policy: jobs::AfterPolicy::Success,
                }],
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: WorkflowRetryPolicy::default(),
                on: WorkflowOutcomeEdges {
                    succeeded: vec!["merge_gate_cicd".to_string()],
                    ..Default::default()
                },
            });
        }
    }

    WorkflowTemplate {
        id: template_ref.id.clone(),
        version: template_ref.version.clone(),
        params: BTreeMap::from([
            ("slug".to_string(), slug.to_string()),
            ("branch".to_string(), branch.to_string()),
            ("target".to_string(), target_branch.to_string()),
        ]),
        policy: default_template_policy(),
        artifact_contracts: vec![
            artifact_contract("plan_branch", "v1"),
            artifact_contract("target_branch", "v1"),
            artifact_contract("merge_sentinel", "v1"),
        ],
        nodes,
    }
}

pub(crate) fn patch_template(
    template_ref: &WorkflowTemplateRef,
    pipeline: &str,
    target: Option<&str>,
    resume: bool,
) -> WorkflowTemplate {
    let mut args = BTreeMap::new();
    args.insert("pipeline".to_string(), pipeline.to_string());
    args.insert("resume".to_string(), resume.to_string());
    if let Some(target_branch) = target {
        args.insert("target".to_string(), target_branch.to_string());
    }

    WorkflowTemplate {
        id: template_ref.id.clone(),
        version: template_ref.version.clone(),
        params: args.clone(),
        policy: default_template_policy(),
        artifact_contracts: Vec::new(),
        nodes: vec![WorkflowNode {
            id: "patch_execute".to_string(),
            kind: WorkflowNodeKind::Builtin,
            uses: "vizier.patch.execute".to_string(),
            args,
            after: Vec::new(),
            needs: Vec::new(),
            produces: WorkflowOutcomeArtifacts::default(),
            locks: Vec::new(),
            preconditions: Vec::new(),
            gates: Vec::new(),
            retry: WorkflowRetryPolicy::default(),
            on: Default::default(),
        }],
    }
}

pub(crate) fn build_execute_template(
    template_ref: &WorkflowTemplateRef,
    slug: &str,
    branch: &str,
    target_branch: &str,
    include_review: bool,
    include_merge: bool,
    gate_config: &BuildExecuteGateConfig,
) -> WorkflowTemplate {
    let mut nodes = Vec::new();
    let mut after_edges = Vec::new();

    nodes.push(WorkflowNode {
        id: "materialize".to_string(),
        kind: WorkflowNodeKind::Builtin,
        uses: "vizier.build.materialize".to_string(),
        args: BTreeMap::new(),
        after: Vec::new(),
        needs: Vec::new(),
        produces: WorkflowOutcomeArtifacts {
            succeeded: vec![
                jobs::JobArtifact::PlanBranch {
                    slug: slug.to_string(),
                    branch: branch.to_string(),
                },
                jobs::JobArtifact::PlanDoc {
                    slug: slug.to_string(),
                    branch: branch.to_string(),
                },
            ],
            ..Default::default()
        },
        locks: vec![
            jobs::JobLock {
                key: format!("branch:{branch}"),
                mode: jobs::LockMode::Exclusive,
            },
            jobs::JobLock {
                key: format!("temp_worktree:build-materialize-{slug}"),
                mode: jobs::LockMode::Exclusive,
            },
        ],
        preconditions: Vec::new(),
        gates: Vec::new(),
        retry: WorkflowRetryPolicy::default(),
        on: Default::default(),
    });

    after_edges.push(WorkflowAfterDependency {
        node_id: "materialize".to_string(),
        policy: jobs::AfterPolicy::Success,
    });
    nodes.push(WorkflowNode {
        id: "approve".to_string(),
        kind: WorkflowNodeKind::Builtin,
        uses: "vizier.approve.apply_once".to_string(),
        args: BTreeMap::new(),
        after: after_edges.clone(),
        needs: vec![jobs::JobArtifact::PlanDoc {
            slug: slug.to_string(),
            branch: branch.to_string(),
        }],
        produces: WorkflowOutcomeArtifacts {
            succeeded: vec![jobs::JobArtifact::PlanCommits {
                slug: slug.to_string(),
                branch: branch.to_string(),
            }],
            ..Default::default()
        },
        locks: vec![
            jobs::JobLock {
                key: format!("branch:{branch}"),
                mode: jobs::LockMode::Exclusive,
            },
            jobs::JobLock {
                key: format!("temp_worktree:build-approve-{slug}"),
                mode: jobs::LockMode::Exclusive,
            },
        ],
        preconditions: Vec::new(),
        gates: Vec::new(),
        retry: WorkflowRetryPolicy::default(),
        on: Default::default(),
    });

    let mut prior_node = "approve".to_string();
    if include_review {
        let mut gates = Vec::new();
        if let Some(script) = gate_config.review_cicd_script.as_ref() {
            gates.push(WorkflowGate::Cicd {
                script: script.clone(),
                auto_resolve: false,
                policy: WorkflowGatePolicy::Warn,
            });
        }
        nodes.push(WorkflowNode {
            id: "review".to_string(),
            kind: WorkflowNodeKind::Agent,
            uses: "vizier.review.critique".to_string(),
            args: BTreeMap::new(),
            after: vec![WorkflowAfterDependency {
                node_id: prior_node.clone(),
                policy: jobs::AfterPolicy::Success,
            }],
            needs: vec![
                jobs::JobArtifact::PlanBranch {
                    slug: slug.to_string(),
                    branch: branch.to_string(),
                },
                jobs::JobArtifact::PlanDoc {
                    slug: slug.to_string(),
                    branch: branch.to_string(),
                },
            ],
            produces: WorkflowOutcomeArtifacts {
                succeeded: vec![jobs::JobArtifact::PlanCommits {
                    slug: slug.to_string(),
                    branch: branch.to_string(),
                }],
                ..Default::default()
            },
            locks: vec![
                jobs::JobLock {
                    key: format!("branch:{branch}"),
                    mode: jobs::LockMode::Exclusive,
                },
                jobs::JobLock {
                    key: format!("temp_worktree:build-review-{slug}"),
                    mode: jobs::LockMode::Exclusive,
                },
            ],
            preconditions: Vec::new(),
            gates,
            retry: WorkflowRetryPolicy::default(),
            on: Default::default(),
        });
        prior_node = "review".to_string();
    }

    if include_merge {
        let mut gates = Vec::new();
        if let Some(script) = gate_config.merge_cicd_script.as_ref() {
            gates.push(WorkflowGate::Cicd {
                script: script.clone(),
                auto_resolve: gate_config.merge_cicd_auto_resolve,
                policy: WorkflowGatePolicy::Retry,
            });
        }
        let merge_retry = if gate_config.merge_cicd_script.is_some() {
            WorkflowRetryPolicy {
                mode: WorkflowRetryMode::UntilGate,
                budget: gate_config.merge_cicd_retries,
            }
        } else {
            WorkflowRetryPolicy::default()
        };
        nodes.push(WorkflowNode {
            id: "merge".to_string(),
            kind: WorkflowNodeKind::Builtin,
            uses: "vizier.merge.integrate".to_string(),
            args: BTreeMap::new(),
            after: vec![WorkflowAfterDependency {
                node_id: prior_node,
                policy: jobs::AfterPolicy::Success,
            }],
            needs: vec![jobs::JobArtifact::PlanBranch {
                slug: slug.to_string(),
                branch: branch.to_string(),
            }],
            produces: WorkflowOutcomeArtifacts {
                succeeded: vec![jobs::JobArtifact::TargetBranch {
                    name: target_branch.to_string(),
                }],
                blocked: vec![jobs::JobArtifact::MergeSentinel {
                    slug: slug.to_string(),
                }],
                ..Default::default()
            },
            locks: vec![
                jobs::JobLock {
                    key: format!("branch:{target_branch}"),
                    mode: jobs::LockMode::Exclusive,
                },
                jobs::JobLock {
                    key: format!("branch:{branch}"),
                    mode: jobs::LockMode::Exclusive,
                },
                jobs::JobLock {
                    key: format!("merge_sentinel:{slug}"),
                    mode: jobs::LockMode::Exclusive,
                },
            ],
            preconditions: Vec::new(),
            gates,
            retry: merge_retry,
            on: Default::default(),
        });
    }

    WorkflowTemplate {
        id: template_ref.id.clone(),
        version: template_ref.version.clone(),
        params: BTreeMap::from([
            ("slug".to_string(), slug.to_string()),
            ("branch".to_string(), branch.to_string()),
            ("target".to_string(), target_branch.to_string()),
        ]),
        policy: default_template_policy(),
        artifact_contracts: vec![
            artifact_contract("plan_branch", "v1"),
            artifact_contract("plan_doc", "v1"),
            artifact_contract("plan_commits", "v1"),
            artifact_contract("target_branch", "v1"),
            artifact_contract("merge_sentinel", "v1"),
            artifact_contract("command_patch", "v1"),
        ],
        nodes,
    }
}

fn default_template_policy() -> WorkflowTemplatePolicy {
    WorkflowTemplatePolicy {
        failure_mode: WorkflowFailureMode::BlockDownstream,
        resume: WorkflowResumePolicy {
            key: "default".to_string(),
            reuse_mode: WorkflowResumeReuseMode::Strict,
        },
    }
}

fn artifact_contract(id: &str, version: &str) -> WorkflowArtifactContract {
    WorkflowArtifactContract {
        id: id.to_string(),
        version: version.to_string(),
        schema: None,
    }
}

fn workflow_gate_label(gate: &WorkflowGate) -> String {
    match gate {
        WorkflowGate::Approval { required, policy } => {
            format!("approval(required={required}, policy={policy:?})")
        }
        WorkflowGate::Script { script, policy } => {
            format!("script(script={script}, policy={policy:?})")
        }
        WorkflowGate::Cicd {
            script,
            auto_resolve,
            policy,
        } => {
            format!("cicd(script={script}, auto_resolve={auto_resolve}, policy={policy:?})")
        }
        WorkflowGate::Custom { id, policy, .. } => format!("custom(id={id}, policy={policy:?})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use vizier_core::workflow_template::WorkflowGate;

    #[test]
    fn parse_template_ref_supports_at_version() {
        let parsed = parse_template_ref("template.save@v9", "template.save");
        assert_eq!(parsed.id, "template.save");
        assert_eq!(parsed.version, "v9");
        assert!(parsed.source_path.is_none());
    }

    #[test]
    fn parse_template_ref_supports_suffix_version() {
        let parsed = parse_template_ref("template.review.v3", "template.review");
        assert_eq!(parsed.id, "template.review");
        assert_eq!(parsed.version, "v3");
        assert!(parsed.source_path.is_none());
    }

    #[test]
    fn parse_template_ref_falls_back_to_v1() {
        let parsed = parse_template_ref("template.merge", "template.merge");
        assert_eq!(parsed.id, "template.merge");
        assert_eq!(parsed.version, "v1");
        assert!(parsed.source_path.is_none());
    }

    #[test]
    fn parse_template_ref_supports_file_selectors() {
        let parsed = parse_template_ref("file:.vizier/workflow/save.json", "template.save");
        assert_eq!(parsed.id, "template.save");
        assert_eq!(parsed.version, "v1");
        assert_eq!(
            parsed.source_path,
            Some(PathBuf::from(".vizier/workflow/save.json"))
        );
    }

    #[test]
    fn resolve_save_template_rejects_unknown_selector_without_file() {
        let template_ref = WorkflowTemplateRef {
            id: "template.custom".to_string(),
            version: "v9".to_string(),
            source_path: None,
        };
        let error = resolve_save_template(&template_ref, "draft/custom", "job-7")
            .expect_err("selector without matching file should fail");
        assert!(
            error.to_string().contains("did not match built-ins"),
            "unexpected error: {error}"
        );
        assert!(
            error.to_string().contains("no selector file was found"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn resolve_selector_template_file_loads_non_file_id_selector()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let selector_path = temp.path().join(".vizier/workflow/template.custom@v9.json");
        if let Some(parent) = selector_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            &selector_path,
            r#"{
  "id": "template.custom",
  "version": "v9",
  "artifact_contracts": [
    { "id": "command_patch", "version": "v1" }
  ],
  "nodes": [
    {
      "id": "save_apply_in_worktree",
      "kind": "builtin",
      "uses": "vizier.save.apply",
      "produces": {
        "succeeded": [
          { "command_patch": { "job_id": "${job_id}" } }
        ]
      },
      "locks": [
        { "key": "branch:${branch}", "mode": "exclusive" }
      ]
    }
  ]
}"#,
        )?;

        let template_ref = WorkflowTemplateRef {
            id: "template.custom".to_string(),
            version: "v9".to_string(),
            source_path: None,
        };
        let runtime_params = BTreeMap::from([
            ("branch".to_string(), "draft/custom".to_string()),
            ("job_id".to_string(), "job-7".to_string()),
        ]);
        let template =
            resolve_selector_template_file_from_base(temp.path(), &template_ref, &runtime_params)?
                .expect("selector template should load from .vizier/workflow");
        assert_eq!(template.id, "template.custom");
        assert_eq!(template.version, "v9");
        assert!(
            template.nodes.iter().any(|node| {
                node.locks
                    .iter()
                    .any(|lock| lock.key == "branch:draft/custom")
            }),
            "selector template should render runtime parameters"
        );
        Ok(())
    }

    #[test]
    fn resolve_save_template_loads_and_renders_file_template()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let template_path = temp.path().join("save-template.json");
        fs::write(
            &template_path,
            r#"{
  "id": "custom.save",
  "version": "v9",
  "artifact_contracts": [
    { "id": "command_patch", "version": "v1" }
  ],
  "nodes": [
    {
      "id": "save_apply_in_worktree",
      "kind": "builtin",
      "uses": "vizier.save.apply",
      "args": { "branch": "${branch}" },
      "produces": {
        "succeeded": [
          { "command_patch": { "job_id": "${job_id}" } }
        ]
      },
      "locks": [
        { "key": "repo_serial", "mode": "exclusive" },
        { "key": "branch:${branch}", "mode": "exclusive" },
        { "key": "temp_worktree:${job_id}", "mode": "exclusive" }
      ],
      "preconditions": [ { "kind": "pinned_head" } ]
    }
  ]
}"#,
        )?;
        let template_ref = parse_template_ref(
            &format!("file:{}", template_path.display()),
            "template.save",
        );
        let template = resolve_save_template(&template_ref, "draft/custom", "job-7")?;
        assert_eq!(template.id, "custom.save");
        assert_eq!(template.version, "v9");
        assert_eq!(
            template
                .nodes
                .iter()
                .find(|node| node.id == "save_apply_in_worktree")
                .and_then(|node| node.args.get("branch"))
                .map(|value| value.as_str()),
            Some("draft/custom")
        );
        let compiled = compile_template_node(
            &template,
            "save_apply_in_worktree",
            &BTreeMap::new(),
            Some(jobs::PinnedHead {
                branch: "draft/custom".to_string(),
                oid: "deadbeef".to_string(),
            }),
        )?;
        assert!(
            compiled
                .schedule
                .locks
                .iter()
                .any(|lock| lock.key == "branch:draft/custom"),
            "custom save template should render branch lock"
        );
        assert!(
            compiled
                .schedule
                .locks
                .iter()
                .any(|lock| lock.key == "temp_worktree:job-7"),
            "custom save template should render temp worktree lock"
        );
        Ok(())
    }

    #[test]
    fn resolve_primary_template_node_id_falls_back_to_semantic_capability() {
        let template = WorkflowTemplate {
            id: "custom.review".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: default_template_policy(),
            artifact_contracts: vec![
                artifact_contract("plan_branch", "v1"),
                artifact_contract("plan_doc", "v1"),
                artifact_contract("plan_commits", "v1"),
            ],
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
                gates: Vec::new(),
                retry: WorkflowRetryPolicy::default(),
                on: WorkflowOutcomeEdges::default(),
            }],
        };

        let primary = resolve_primary_template_node_id(&template, TemplateScope::Review)
            .expect("semantic capability fallback should resolve review node");
        assert_eq!(primary, "review_main");
    }

    #[test]
    fn compile_template_node_schedule_orders_nodes_and_finds_terminals() {
        let template = WorkflowTemplate {
            id: "custom.graph".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: default_template_policy(),
            artifact_contracts: vec![artifact_contract("command_patch", "v1")],
            nodes: vec![
                WorkflowNode {
                    id: "a".to_string(),
                    kind: WorkflowNodeKind::Shell,
                    uses: "acme.a".to_string(),
                    args: BTreeMap::from([("command".to_string(), "true".to_string())]),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: Vec::new(),
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges::default(),
                },
                WorkflowNode {
                    id: "b".to_string(),
                    kind: WorkflowNodeKind::Shell,
                    uses: "acme.b".to_string(),
                    args: BTreeMap::from([("command".to_string(), "true".to_string())]),
                    after: vec![WorkflowAfterDependency {
                        node_id: "a".to_string(),
                        policy: jobs::AfterPolicy::Success,
                    }],
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: Vec::new(),
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges::default(),
                },
                WorkflowNode {
                    id: "c".to_string(),
                    kind: WorkflowNodeKind::Shell,
                    uses: "acme.c".to_string(),
                    args: BTreeMap::from([("command".to_string(), "true".to_string())]),
                    after: vec![WorkflowAfterDependency {
                        node_id: "a".to_string(),
                        policy: jobs::AfterPolicy::Success,
                    }],
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

        let schedule = compile_template_node_schedule(&template)
            .expect("schedule should compile for an acyclic template");
        assert_eq!(schedule.order.first().map(String::as_str), Some("a"));
        assert_eq!(
            schedule.terminal_nodes,
            vec!["b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn build_execute_template_uses_gate_config_for_review_and_merge_nodes() {
        let template_ref = WorkflowTemplateRef {
            id: "template.build_execute".to_string(),
            version: "v1".to_string(),
            source_path: None,
        };
        let gate_config = BuildExecuteGateConfig {
            review_cicd_script: Some("./scripts/review-gate.sh".to_string()),
            merge_cicd_script: Some("./scripts/merge-gate.sh".to_string()),
            merge_cicd_auto_resolve: true,
            merge_cicd_retries: 3,
        };

        let template = build_execute_template(
            &template_ref,
            "sample",
            "draft/sample",
            "main",
            true,
            true,
            &gate_config,
        );

        let review = template
            .nodes
            .iter()
            .find(|node| node.id == "review")
            .expect("review node should be present");
        assert!(
            review
                .locks
                .iter()
                .any(|lock| lock.key == "branch:draft/sample"),
            "review node should include branch lock"
        );
        assert!(
            review
                .locks
                .iter()
                .any(|lock| lock.key == "temp_worktree:build-review-sample"),
            "review node should include review temp-worktree lock"
        );
        assert!(
            review
                .gates
                .iter()
                .any(|gate| matches!(gate, WorkflowGate::Cicd { script, auto_resolve, .. } if script == "./scripts/review-gate.sh" && !auto_resolve)),
            "review node should include warn-only cicd gate"
        );

        let merge = template
            .nodes
            .iter()
            .find(|node| node.id == "merge")
            .expect("merge node should be present");
        assert!(
            merge
                .locks
                .iter()
                .any(|lock| lock.key == "merge_sentinel:sample"),
            "merge node should include merge sentinel lock"
        );
        assert!(
            merge
                .gates
                .iter()
                .any(|gate| matches!(gate, WorkflowGate::Cicd { script, auto_resolve, .. } if script == "./scripts/merge-gate.sh" && *auto_resolve)),
            "merge node should include merge cicd gate with configured auto-resolve setting"
        );
        assert_eq!(merge.retry.mode, WorkflowRetryMode::UntilGate);
        assert_eq!(merge.retry.budget, 3);
    }

    #[test]
    fn build_execute_template_omits_merge_retry_without_gate_script() {
        let template_ref = WorkflowTemplateRef {
            id: "template.build_execute".to_string(),
            version: "v1".to_string(),
            source_path: None,
        };
        let gate_config = BuildExecuteGateConfig::default();
        let template = build_execute_template(
            &template_ref,
            "sample",
            "draft/sample",
            "main",
            true,
            true,
            &gate_config,
        );
        let merge = template
            .nodes
            .iter()
            .find(|node| node.id == "merge")
            .expect("merge node should be present");
        assert!(
            merge.gates.is_empty(),
            "merge gate should be empty without script"
        );
        assert_eq!(merge.retry.mode, WorkflowRetryMode::Never);
        assert_eq!(merge.retry.budget, 0);
    }

    #[test]
    fn approve_template_includes_stop_condition_gate_when_configured() {
        let template_ref = WorkflowTemplateRef {
            id: "template.approve".to_string(),
            version: "v1".to_string(),
            source_path: None,
        };
        let template = approve_template(
            &template_ref,
            "sample",
            "draft/sample",
            "job-42",
            true,
            Some("./scripts/approve-stop.sh"),
            3,
        );
        let node = template
            .nodes
            .iter()
            .find(|node| node.id == "approve_apply_once")
            .expect("approve node should be present");
        assert!(
            node.gates.iter().any(|gate| matches!(
                gate,
                WorkflowGate::Approval { required, .. } if *required
            )),
            "approve node should preserve approval gate"
        );
        assert!(
            node.gates.iter().any(|gate| matches!(
                gate,
                WorkflowGate::Script { script, .. } if script == "./scripts/approve-stop.sh"
            )),
            "approve node should include stop-condition script gate"
        );
        assert_eq!(node.retry.mode, WorkflowRetryMode::UntilGate);
        assert_eq!(node.retry.budget, 3);
        assert_eq!(
            node.on.succeeded,
            vec!["approve_gate_stop_condition".to_string()],
            "approve node should transition to stop-condition node on success"
        );
        let stop_node = template
            .nodes
            .iter()
            .find(|node| node.id == "approve_gate_stop_condition")
            .expect("approve stop-condition node should be present");
        assert!(
            stop_node.gates.iter().any(|gate| matches!(
                gate,
                WorkflowGate::Script { script, .. } if script == "./scripts/approve-stop.sh"
            )),
            "stop-condition node should run the stop-condition script"
        );
        assert_eq!(
            stop_node
                .after
                .iter()
                .map(|entry| entry.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["approve_apply_once"],
            "stop-condition node should run after approve_apply_once"
        );
        assert_eq!(
            stop_node.on.failed,
            vec!["approve_apply_once".to_string()],
            "stop-condition node should retry by returning to apply_once"
        );
    }

    #[test]
    fn merge_template_includes_cicd_gate_when_configured() {
        let template_ref = WorkflowTemplateRef {
            id: "template.merge".to_string(),
            version: "v1".to_string(),
            source_path: None,
        };
        let template = merge_template(
            &template_ref,
            "sample",
            "draft/sample",
            "main",
            MergeTemplateGateConfig {
                cicd_script: Some("./scripts/merge-gate.sh"),
                cicd_auto_resolve: true,
                cicd_retries: 4,
                conflict_auto_resolve: true,
            },
        );
        let node = template
            .nodes
            .iter()
            .find(|node| node.id == "merge_integrate")
            .expect("merge node should be present");
        assert!(
            node.gates.iter().any(|gate| matches!(
                gate,
                WorkflowGate::Cicd {
                    script,
                    auto_resolve,
                    ..
                } if script == "./scripts/merge-gate.sh" && *auto_resolve
            )),
            "merge node should include cicd gate"
        );
        assert_eq!(node.retry.mode, WorkflowRetryMode::UntilGate);
        assert_eq!(node.retry.budget, 4);
        assert_eq!(
            node.on.succeeded,
            vec!["merge_gate_cicd".to_string()],
            "merge integrate node should branch into the ci/cd gate node"
        );
        assert_eq!(
            node.on.blocked,
            vec!["merge_conflict_resolution".to_string()],
            "merge integrate node should route blocked outcomes to conflict resolution"
        );
        let gate_node = template
            .nodes
            .iter()
            .find(|node| node.id == "merge_gate_cicd")
            .expect("merge gate node should be present");
        assert_eq!(
            gate_node
                .after
                .iter()
                .map(|entry| entry.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["merge_integrate"],
            "merge gate should run after merge integration"
        );
        assert_eq!(gate_node.retry.mode, WorkflowRetryMode::UntilGate);
        assert_eq!(gate_node.retry.budget, 4);
        assert_eq!(
            gate_node.on.failed,
            vec!["merge_cicd_auto_fix".to_string()],
            "merge gate should route failed outcomes to auto-fix"
        );
        let auto_fix_node = template
            .nodes
            .iter()
            .find(|node| node.id == "merge_cicd_auto_fix")
            .expect("merge auto-fix node should be present");
        assert_eq!(
            auto_fix_node
                .after
                .iter()
                .map(|entry| entry.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["merge_gate_cicd"],
            "auto-fix should run after the CI/CD gate node"
        );
        assert_eq!(
            auto_fix_node.on.succeeded,
            vec!["merge_gate_cicd".to_string()],
            "auto-fix should return to the ci/cd gate node"
        );
        let conflict_node = template
            .nodes
            .iter()
            .find(|node| node.id == "merge_conflict_resolution")
            .expect("merge conflict node should be present");
        assert!(
            conflict_node
                .args
                .get("auto_resolve")
                .map(|value| value == "true")
                .unwrap_or(false),
            "merge conflict node should carry auto-resolve policy"
        );
        assert_eq!(
            conflict_node
                .after
                .iter()
                .map(|entry| entry.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["merge_integrate"],
            "merge conflict node should run after merge integration"
        );
        assert_eq!(
            conflict_node.on.succeeded,
            vec!["merge_integrate".to_string()],
            "merge conflict node should retry merge integration when conflicts are resolved"
        );
        assert!(
            conflict_node.gates.iter().any(|gate| matches!(
                gate,
                WorkflowGate::Custom { id, args, .. }
                if id == "conflict_resolution"
                    && args
                        .get("auto_resolve")
                        .map(|value| value == "true")
                        .unwrap_or(false)
            )),
            "merge conflict node should include explicit conflict-resolution gate arguments"
        );
    }

    #[test]
    fn patch_template_compiles_patch_execute_node() {
        let template_ref = WorkflowTemplateRef {
            id: "template.patch".to_string(),
            version: "v1".to_string(),
            source_path: None,
        };
        let template = patch_template(&template_ref, "approve-review-merge", Some("main"), false);
        let node = template
            .nodes
            .iter()
            .find(|node| node.id == "patch_execute")
            .expect("patch_execute node should be present");
        assert_eq!(node.uses, "vizier.patch.execute");
        assert!(
            matches!(node.kind, WorkflowNodeKind::Builtin),
            "patch_execute should be a builtin node"
        );
    }

    #[test]
    fn compile_template_node_schedule_rejects_invalid_approve_retry_wiring() {
        let template = WorkflowTemplate {
            id: "custom.approve".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: default_template_policy(),
            artifact_contracts: vec![artifact_contract("plan_doc", "v1")],
            nodes: vec![
                WorkflowNode {
                    id: "apply".to_string(),
                    kind: WorkflowNodeKind::Builtin,
                    uses: "vizier.approve.apply_once".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: vec![jobs::JobArtifact::PlanDoc {
                        slug: "slug".to_string(),
                        branch: "draft/slug".to_string(),
                    }],
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: Vec::new(),
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges {
                        succeeded: vec!["stop".to_string()],
                        ..Default::default()
                    },
                },
                WorkflowNode {
                    id: "stop".to_string(),
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
                        budget: 1,
                    },
                    on: WorkflowOutcomeEdges::default(),
                },
            ],
        };

        let error = compile_template_node_schedule(&template)
            .expect_err("invalid stop-condition retry wiring should fail schedule compile");
        assert!(
            error.to_string().contains("cap.gate.stop_condition")
                || error.to_string().contains("cap.plan.apply_once"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn compile_template_node_schedule_rejects_patch_without_files_json() {
        let template_ref = WorkflowTemplateRef {
            id: "template.patch".to_string(),
            version: "v1".to_string(),
            source_path: None,
        };
        let template = patch_template(&template_ref, "approve-review", Some("main"), false);
        let error = compile_template_node_schedule(&template)
            .expect_err("patch template without files_json should fail schedule compile");
        assert!(
            error.to_string().contains("cap.patch.execute_pipeline"),
            "unexpected error: {error}"
        );
    }
}
