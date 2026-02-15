use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use vizier_core::{
    config,
    scheduler::{JobArtifact, JobLock},
    workflow_template::{
        WorkflowAfterDependency, WorkflowArtifactContract, WorkflowGate, WorkflowGatePolicy,
        WorkflowNode, WorkflowNodeKind, WorkflowOutcomeArtifacts, WorkflowOutcomeEdges,
        WorkflowPrecondition, WorkflowRetryMode, WorkflowRetryPolicy, WorkflowTemplate,
        WorkflowTemplatePolicy,
    },
};

#[derive(Debug, Clone)]
pub(crate) struct ResolvedWorkflowSource {
    pub selector: String,
    pub path: PathBuf,
    pub command_alias: Option<config::CommandAlias>,
}

#[derive(Debug, Clone, Deserialize)]
struct WorkflowTemplateFile {
    id: String,
    version: String,
    #[serde(default)]
    params: BTreeMap<String, String>,
    #[serde(default)]
    policy: WorkflowTemplatePolicy,
    #[serde(default)]
    artifact_contracts: Vec<WorkflowArtifactContract>,
    #[serde(default)]
    nodes: Vec<WorkflowNodeFile>,
    #[serde(default)]
    imports: Vec<WorkflowTemplateImport>,
    #[serde(default)]
    links: Vec<WorkflowTemplateLink>,
    #[serde(default)]
    cli: WorkflowTemplateCli,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct WorkflowTemplateCli {
    #[serde(default)]
    positional: Vec<String>,
    #[serde(default)]
    named: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
struct WorkflowTemplateImport {
    name: String,
    path: String,
}

#[derive(Debug, Clone, Deserialize)]
struct WorkflowTemplateLink {
    from: String,
    to: String,
}

#[derive(Debug, Clone, Deserialize)]
struct WorkflowNodeFile {
    id: String,
    #[serde(default)]
    kind: WorkflowNodeKind,
    uses: String,
    #[serde(default)]
    args: BTreeMap<String, String>,
    #[serde(default)]
    after: Vec<WorkflowAfterDependency>,
    #[serde(default)]
    needs: Vec<JobArtifact>,
    #[serde(default)]
    produces: WorkflowOutcomeArtifacts,
    #[serde(default)]
    locks: Vec<JobLock>,
    #[serde(default)]
    preconditions: Vec<WorkflowPrecondition>,
    #[serde(default)]
    gates: Vec<WorkflowGateFile>,
    #[serde(default)]
    retry: WorkflowRetryPolicyFile,
    #[serde(default)]
    on: WorkflowOutcomeEdges,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WorkflowGateFile {
    Approval {
        #[serde(default)]
        required: TemplateBoolValue,
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
        auto_resolve: TemplateBoolValue,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum TemplateBoolValue {
    Bool(bool),
    String(String),
    I64(i64),
    U64(u64),
    F64(f64),
}

impl Default for TemplateBoolValue {
    fn default() -> Self {
        Self::Bool(false)
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
struct WorkflowRetryPolicyFile {
    #[serde(default)]
    mode: WorkflowRetryModeValue,
    #[serde(default)]
    budget: WorkflowRetryBudgetValue,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum WorkflowRetryModeValue {
    Mode(WorkflowRetryMode),
    String(String),
}

impl Default for WorkflowRetryModeValue {
    fn default() -> Self {
        Self::Mode(WorkflowRetryMode::Never)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum WorkflowRetryBudgetValue {
    U32(u32),
    U64(u64),
    I64(i64),
    String(String),
    F64(f64),
}

impl Default for WorkflowRetryBudgetValue {
    fn default() -> Self {
        Self::U32(0)
    }
}

#[derive(Debug, Clone)]
struct ComposedWorkflowTemplate {
    id: String,
    version: String,
    params: BTreeMap<String, String>,
    policy: WorkflowTemplatePolicy,
    artifact_contracts: Vec<WorkflowArtifactContract>,
    nodes: Vec<WorkflowNodeFile>,
    cli: WorkflowTemplateCli,
}

#[derive(Debug, Clone)]
struct ImportedStage {
    params: BTreeMap<String, String>,
    artifact_contracts: Vec<WorkflowArtifactContract>,
    nodes: Vec<WorkflowNodeFile>,
    terminal_nodes: Vec<String>,
    entry_nodes: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkflowTemplateInputSpec {
    pub params: Vec<String>,
    pub positional: Vec<String>,
    pub named: BTreeMap<String, String>,
}

pub(crate) fn resolve_workflow_source(
    project_root: &Path,
    flow: &str,
    cfg: &config::Config,
) -> Result<ResolvedWorkflowSource, Box<dyn std::error::Error>> {
    let flow = flow.trim();
    if flow.is_empty() {
        return Err("FLOW cannot be empty".into());
    }

    if is_explicit_file_source(flow) {
        let path = resolve_source_path(project_root, flow)?;
        return Ok(ResolvedWorkflowSource {
            selector: normalize_selector_from_path(project_root, flow, &path),
            path,
            command_alias: None,
        });
    }

    if let Some(alias) = config::CommandAlias::parse(flow)
        && let Some(selector) = cfg.template_selector_for_alias(&alias)
    {
        let selector_text = selector.to_string();
        let path = resolve_selector_path(project_root, &selector_text)?.ok_or_else(|| {
            format!(
                "alias `{}` resolves to `{}` but no readable template source was found",
                alias, selector_text
            )
        })?;
        return Ok(ResolvedWorkflowSource {
            selector: selector_text,
            path,
            command_alias: Some(alias),
        });
    }

    if let Some(path) = resolve_selector_path(project_root, flow)? {
        return Ok(ResolvedWorkflowSource {
            selector: flow.to_string(),
            path,
            command_alias: None,
        });
    }

    if let Some(path) = resolve_repo_fallback_path(project_root, flow) {
        return Ok(ResolvedWorkflowSource {
            selector: format!(
                "file:{}",
                path.strip_prefix(project_root)
                    .unwrap_or(&path)
                    .to_string_lossy()
            ),
            path,
            command_alias: None,
        });
    }

    Err(format!(
        "unable to resolve FLOW `{flow}`; pass file:<path>, a direct .toml/.json path, a configured [commands] alias, or a known template selector"
    )
    .into())
}

pub(crate) fn load_template_with_params(
    source: &ResolvedWorkflowSource,
    set_overrides: &BTreeMap<String, String>,
) -> Result<WorkflowTemplate, Box<dyn std::error::Error>> {
    let mut stack = Vec::<PathBuf>::new();
    let template = load_template_recursive(&source.path, &mut stack)?;
    apply_parameter_expansion(template, set_overrides)
}

pub(crate) fn load_template_input_spec(
    source: &ResolvedWorkflowSource,
) -> Result<WorkflowTemplateInputSpec, Box<dyn std::error::Error>> {
    let mut stack = Vec::<PathBuf>::new();
    let template = load_template_recursive(&source.path, &mut stack)?;
    let WorkflowTemplateCli {
        positional,
        named: raw_named,
    } = template.cli;
    let mut named = BTreeMap::new();
    for (alias, target) in raw_named {
        named.insert(alias.trim().replace('-', "_"), target.trim().to_string());
    }
    Ok(WorkflowTemplateInputSpec {
        params: template.params.keys().cloned().collect(),
        positional,
        named,
    })
}

fn load_template_recursive(
    path: &Path,
    stack: &mut Vec<PathBuf>,
) -> Result<ComposedWorkflowTemplate, Box<dyn std::error::Error>> {
    let canonical = fs::canonicalize(path)
        .map_err(|err| format!("unable to read template source {}: {err}", path.display()))?;

    if let Some(pos) = stack.iter().position(|entry| entry == &canonical) {
        let mut cycle = stack[pos..]
            .iter()
            .map(|entry| entry.display().to_string())
            .collect::<Vec<_>>();
        cycle.push(canonical.display().to_string());
        return Err(format!("workflow import cycle detected: {}", cycle.join(" -> ")).into());
    }

    stack.push(canonical.clone());
    let parsed = parse_template_file(&canonical)?;
    let result = if parsed.imports.is_empty() {
        Ok(ComposedWorkflowTemplate {
            id: parsed.id,
            version: parsed.version,
            params: parsed.params,
            policy: parsed.policy,
            artifact_contracts: parsed.artifact_contracts,
            nodes: parsed.nodes,
            cli: parsed.cli,
        })
    } else {
        compose_template(&canonical, parsed, stack)
    };
    stack.pop();
    result
}

fn compose_template(
    source_path: &Path,
    parsed: WorkflowTemplateFile,
    stack: &mut Vec<PathBuf>,
) -> Result<ComposedWorkflowTemplate, Box<dyn std::error::Error>> {
    let mut imported = HashMap::<String, ImportedStage>::new();
    let mut import_order = Vec::<String>::new();
    let mut link_graph = HashMap::<String, Vec<String>>::new();

    for import in &parsed.imports {
        let name = import.name.trim();
        if name.is_empty() {
            return Err(format!(
                "template {}@{} has import with empty name",
                parsed.id, parsed.version
            )
            .into());
        }
        if imported.contains_key(name) {
            return Err(format!(
                "template {}@{} has duplicate import name `{name}`",
                parsed.id, parsed.version
            )
            .into());
        }

        let import_path = source_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(import.path.trim());
        let stage_template = load_template_recursive(&import_path, stack)?;
        let stage = prefix_stage(name, &stage_template)?;

        import_order.push(name.to_string());
        imported.insert(name.to_string(), stage);
        link_graph.entry(name.to_string()).or_default();
    }

    for link in &parsed.links {
        let from = link.from.trim();
        let to = link.to.trim();
        if from.is_empty() || to.is_empty() {
            return Err(format!(
                "template {}@{} has link with empty endpoint",
                parsed.id, parsed.version
            )
            .into());
        }
        if !imported.contains_key(from) {
            return Err(format!(
                "template {}@{} link references unknown stage `{from}`",
                parsed.id, parsed.version
            )
            .into());
        }
        if !imported.contains_key(to) {
            return Err(format!(
                "template {}@{} link references unknown stage `{to}`",
                parsed.id, parsed.version
            )
            .into());
        }
        link_graph
            .entry(from.to_string())
            .or_default()
            .push(to.to_string());
    }

    if let Some(cycle) = find_cycle(&link_graph) {
        return Err(format!("workflow link cycle detected: {}", cycle.join(" -> ")).into());
    }

    let mut params = parsed.params.clone();
    let mut artifact_contracts = parsed.artifact_contracts.clone();
    let mut nodes = parsed.nodes.clone();

    for name in &import_order {
        let stage = imported
            .get(name)
            .ok_or_else(|| format!("missing stage state for import `{name}`"))?;
        merge_params(&mut params, &stage.params, name)?;
        merge_artifact_contracts(&mut artifact_contracts, &stage.artifact_contracts, name)?;
        nodes.extend(stage.nodes.clone());
    }

    for link in &parsed.links {
        let from = link.from.trim();
        let to = link.to.trim();
        let source_stage = imported
            .get(from)
            .ok_or_else(|| format!("missing source stage `{from}`"))?;
        let target_stage = imported
            .get(to)
            .ok_or_else(|| format!("missing target stage `{to}`"))?;

        if source_stage.terminal_nodes.is_empty() {
            return Err(format!("stage `{from}` has no terminal nodes to link from").into());
        }
        if target_stage.entry_nodes.is_empty() {
            return Err(format!("stage `{to}` has no entry nodes to link to").into());
        }

        let target_ids = target_stage.entry_nodes.clone();
        for node in &mut nodes {
            if source_stage.terminal_nodes.iter().any(|id| id == &node.id) {
                for target in &target_ids {
                    if !node.on.succeeded.iter().any(|existing| existing == target) {
                        node.on.succeeded.push(target.clone());
                    }
                }
            }
        }
    }

    Ok(ComposedWorkflowTemplate {
        id: parsed.id,
        version: parsed.version,
        params,
        policy: parsed.policy,
        artifact_contracts,
        nodes,
        cli: parsed.cli,
    })
}

fn prefix_stage(
    stage_name: &str,
    template: &ComposedWorkflowTemplate,
) -> Result<ImportedStage, Box<dyn std::error::Error>> {
    if template.nodes.is_empty() {
        return Err(format!("imported stage `{stage_name}` has no nodes").into());
    }

    let mut remapped = Vec::with_capacity(template.nodes.len());
    let mut id_map = HashMap::<String, String>::new();

    for node in &template.nodes {
        let prefixed = prefixed_node_id(stage_name, &node.id);
        if id_map.insert(node.id.clone(), prefixed.clone()).is_some() {
            return Err(format!("imported stage `{stage_name}` has duplicate node ids").into());
        }
    }

    for node in &template.nodes {
        let mut cloned = node.clone();
        cloned.id = id_map.get(&node.id).cloned().ok_or_else(|| {
            format!(
                "missing remapped id for stage `{stage_name}` node `{}`",
                node.id
            )
        })?;

        for dependency in &mut cloned.after {
            dependency.node_id = id_map.get(&dependency.node_id).cloned().ok_or_else(|| {
                format!(
                    "stage `{stage_name}` node `{}` references unknown after node `{}`",
                    node.id, dependency.node_id
                )
            })?;
        }

        remap_targets(
            &mut cloned.on.succeeded,
            &id_map,
            stage_name,
            &node.id,
            "succeeded",
        )?;
        remap_targets(
            &mut cloned.on.failed,
            &id_map,
            stage_name,
            &node.id,
            "failed",
        )?;
        remap_targets(
            &mut cloned.on.blocked,
            &id_map,
            stage_name,
            &node.id,
            "blocked",
        )?;
        remap_targets(
            &mut cloned.on.cancelled,
            &id_map,
            stage_name,
            &node.id,
            "cancelled",
        )?;

        remapped.push(cloned);
    }

    let terminal_nodes = remapped
        .iter()
        .filter(|node| node.on.succeeded.is_empty())
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();

    let mut incoming_success = HashSet::<String>::new();
    for node in &remapped {
        for target in &node.on.succeeded {
            incoming_success.insert(target.clone());
        }
    }

    let mut entry_nodes = remapped
        .iter()
        .filter(|node| !incoming_success.contains(&node.id) && node.after.is_empty())
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();

    if entry_nodes.is_empty() {
        entry_nodes = remapped
            .iter()
            .filter(|node| !incoming_success.contains(&node.id))
            .map(|node| node.id.clone())
            .collect::<Vec<_>>();
    }

    Ok(ImportedStage {
        params: template.params.clone(),
        artifact_contracts: template.artifact_contracts.clone(),
        nodes: remapped,
        terminal_nodes,
        entry_nodes,
    })
}

fn merge_params(
    destination: &mut BTreeMap<String, String>,
    stage_params: &BTreeMap<String, String>,
    stage_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    for (key, value) in stage_params {
        if let Some(existing) = destination.get(key)
            && existing != value
        {
            return Err(format!(
                "conflicting parameter default for `{key}` while composing stage `{stage_name}` (`{existing}` vs `{value}`)"
            )
            .into());
        }
        destination.insert(key.clone(), value.clone());
    }
    Ok(())
}

fn merge_artifact_contracts(
    destination: &mut Vec<WorkflowArtifactContract>,
    stage_contracts: &[WorkflowArtifactContract],
    stage_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    for contract in stage_contracts {
        if let Some(existing) = destination.iter().find(|value| value.id == contract.id) {
            if existing != contract {
                return Err(format!(
                    "conflicting artifact contract `{}` while composing stage `{stage_name}`",
                    contract.id
                )
                .into());
            }
            continue;
        }
        destination.push(contract.clone());
    }
    Ok(())
}

fn remap_targets(
    targets: &mut Vec<String>,
    id_map: &HashMap<String, String>,
    stage_name: &str,
    node_id: &str,
    outcome: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    for target in targets {
        *target = id_map.get(target).cloned().ok_or_else(|| {
            format!(
                "stage `{stage_name}` node `{node_id}` references unknown `{outcome}` target `{target}`"
            )
        })?;
    }
    Ok(())
}

fn prefixed_node_id(stage_name: &str, node_id: &str) -> String {
    let prefix = sanitize_stage_name(stage_name);
    format!("{prefix}__{node_id}")
}

fn sanitize_stage_name(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches('_').to_string()
}

fn find_cycle(graph: &HashMap<String, Vec<String>>) -> Option<Vec<String>> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum VisitState {
        Visiting,
        Visited,
    }

    fn dfs(
        node: &str,
        graph: &HashMap<String, Vec<String>>,
        state: &mut HashMap<String, VisitState>,
        stack: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        state.insert(node.to_string(), VisitState::Visiting);
        stack.push(node.to_string());

        if let Some(next) = graph.get(node) {
            for target in next {
                if let Some(pos) = stack.iter().position(|value| value == target) {
                    let mut cycle = stack[pos..].to_vec();
                    cycle.push(target.clone());
                    return Some(cycle);
                }
                if !matches!(state.get(target), Some(VisitState::Visited))
                    && let Some(cycle) = dfs(target, graph, state, stack)
                {
                    return Some(cycle);
                }
            }
        }

        stack.pop();
        state.insert(node.to_string(), VisitState::Visited);
        None
    }

    let mut state = HashMap::<String, VisitState>::new();
    let mut nodes = graph.keys().cloned().collect::<Vec<_>>();
    nodes.sort();
    nodes.dedup();

    for node in nodes {
        if matches!(state.get(&node), Some(VisitState::Visited)) {
            continue;
        }
        let mut stack = Vec::new();
        if let Some(cycle) = dfs(&node, graph, &mut state, &mut stack) {
            return Some(cycle);
        }
    }

    None
}

fn parse_template_file(path: &Path) -> Result<WorkflowTemplateFile, Box<dyn std::error::Error>> {
    let contents = fs::read_to_string(path)
        .map_err(|err| format!("unable to read template source {}: {err}", path.display()))?;

    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("toml") => toml::from_str::<WorkflowTemplateFile>(&contents)
            .map_err(|err| format!("invalid TOML template {}: {err}", path.display()).into()),
        Some("json") => serde_json::from_str::<WorkflowTemplateFile>(&contents)
            .map_err(|err| format!("invalid JSON template {}: {err}", path.display()).into()),
        _ => Err(format!(
            "unsupported template source {}; expected .toml or .json",
            path.display()
        )
        .into()),
    }
}

fn apply_parameter_expansion(
    mut template: ComposedWorkflowTemplate,
    set_overrides: &BTreeMap<String, String>,
) -> Result<WorkflowTemplate, Box<dyn std::error::Error>> {
    let mut params = template.params.clone();
    for (key, value) in set_overrides {
        params.insert(key.clone(), value.clone());
    }

    for (index, contract) in template.artifact_contracts.iter_mut().enumerate() {
        let id_path = format!("artifact_contracts[{index}].id");
        contract.id = expand_string_value(&contract.id, &id_path, &params)?;
        let version_path = format!("artifact_contracts[{index}].version");
        contract.version = expand_string_value(&contract.version, &version_path, &params)?;
    }

    let mut expanded_nodes = Vec::with_capacity(template.nodes.len());
    for node in template.nodes {
        expanded_nodes.push(expand_node(node, &params)?);
    }

    Ok(WorkflowTemplate {
        id: template.id,
        version: template.version,
        params,
        policy: template.policy,
        artifact_contracts: template.artifact_contracts,
        nodes: expanded_nodes,
    })
}

fn expand_node(
    mut node: WorkflowNodeFile,
    params: &BTreeMap<String, String>,
) -> Result<WorkflowNode, Box<dyn std::error::Error>> {
    let node_id = node.id.clone();

    for (arg_key, arg_value) in &mut node.args {
        let path = format!("nodes[{node_id}].args.{arg_key}");
        *arg_value = expand_string_value(arg_value, &path, params)?;
    }

    expand_artifact_list(&mut node.needs, &format!("nodes[{node_id}].needs"), params)?;
    expand_artifact_list(
        &mut node.produces.succeeded,
        &format!("nodes[{node_id}].produces.succeeded"),
        params,
    )?;
    expand_artifact_list(
        &mut node.produces.failed,
        &format!("nodes[{node_id}].produces.failed"),
        params,
    )?;
    expand_artifact_list(
        &mut node.produces.blocked,
        &format!("nodes[{node_id}].produces.blocked"),
        params,
    )?;
    expand_artifact_list(
        &mut node.produces.cancelled,
        &format!("nodes[{node_id}].produces.cancelled"),
        params,
    )?;

    for (index, lock) in node.locks.iter_mut().enumerate() {
        let path = format!("nodes[{node_id}].locks[{index}].key");
        lock.key = expand_string_value(&lock.key, &path, params)?;
    }

    for (index, precondition) in node.preconditions.iter_mut().enumerate() {
        if let WorkflowPrecondition::Custom { args, .. } = precondition {
            for (arg_key, arg_value) in args.iter_mut() {
                let path = format!("nodes[{node_id}].preconditions[{index}].custom.args.{arg_key}");
                *arg_value = expand_string_value(arg_value, &path, params)?;
            }
        }
    }

    let mut gates = Vec::with_capacity(node.gates.len());
    for (index, gate) in node.gates.into_iter().enumerate() {
        gates.push(expand_gate(
            gate,
            &format!("nodes[{node_id}].gates[{index}]"),
            params,
        )?);
    }

    let retry = expand_retry_policy(node.retry, &format!("nodes[{node_id}].retry"), params)?;

    Ok(WorkflowNode {
        id: node.id,
        kind: node.kind,
        uses: node.uses,
        args: node.args,
        after: node.after,
        needs: node.needs,
        produces: node.produces,
        locks: node.locks,
        preconditions: node.preconditions,
        gates,
        retry,
        on: node.on,
    })
}

fn expand_artifact_list(
    artifacts: &mut [JobArtifact],
    path_prefix: &str,
    params: &BTreeMap<String, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    for (index, artifact) in artifacts.iter_mut().enumerate() {
        expand_artifact(artifact, &format!("{path_prefix}[{index}]"), params)?;
    }
    Ok(())
}

fn expand_artifact(
    artifact: &mut JobArtifact,
    path_prefix: &str,
    params: &BTreeMap<String, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    match artifact {
        JobArtifact::PlanBranch { slug, branch } => {
            let slug_path = format!("{path_prefix}.plan_branch.slug");
            *slug = expand_string_value(slug, &slug_path, params)?;
            let branch_path = format!("{path_prefix}.plan_branch.branch");
            *branch = expand_string_value(branch, &branch_path, params)?;
        }
        JobArtifact::PlanDoc { slug, branch } => {
            let slug_path = format!("{path_prefix}.plan_doc.slug");
            *slug = expand_string_value(slug, &slug_path, params)?;
            let branch_path = format!("{path_prefix}.plan_doc.branch");
            *branch = expand_string_value(branch, &branch_path, params)?;
        }
        JobArtifact::PlanCommits { slug, branch } => {
            let slug_path = format!("{path_prefix}.plan_commits.slug");
            *slug = expand_string_value(slug, &slug_path, params)?;
            let branch_path = format!("{path_prefix}.plan_commits.branch");
            *branch = expand_string_value(branch, &branch_path, params)?;
        }
        JobArtifact::TargetBranch { name } => {
            let name_path = format!("{path_prefix}.target_branch.name");
            *name = expand_string_value(name, &name_path, params)?;
        }
        JobArtifact::MergeSentinel { slug } => {
            let slug_path = format!("{path_prefix}.merge_sentinel.slug");
            *slug = expand_string_value(slug, &slug_path, params)?;
        }
        JobArtifact::CommandPatch { job_id } => {
            let path = format!("{path_prefix}.command_patch.job_id");
            *job_id = expand_string_value(job_id, &path, params)?;
        }
        JobArtifact::Custom { type_id, key } => {
            let type_path = format!("{path_prefix}.custom.type_id");
            *type_id = expand_string_value(type_id, &type_path, params)?;
            let key_path = format!("{path_prefix}.custom.key");
            *key = expand_string_value(key, &key_path, params)?;
        }
    }

    Ok(())
}

fn expand_gate(
    gate: WorkflowGateFile,
    path_prefix: &str,
    params: &BTreeMap<String, String>,
) -> Result<WorkflowGate, Box<dyn std::error::Error>> {
    match gate {
        WorkflowGateFile::Approval { required, policy } => Ok(WorkflowGate::Approval {
            required: coerce_bool(
                required,
                &format!("{path_prefix}.approval.required"),
                params,
            )?,
            policy,
        }),
        WorkflowGateFile::Script { script, policy } => Ok(WorkflowGate::Script {
            script: expand_string_value(&script, &format!("{path_prefix}.script"), params)?,
            policy,
        }),
        WorkflowGateFile::Cicd {
            script,
            auto_resolve,
            policy,
        } => Ok(WorkflowGate::Cicd {
            script: expand_string_value(&script, &format!("{path_prefix}.cicd.script"), params)?,
            auto_resolve: coerce_bool(
                auto_resolve,
                &format!("{path_prefix}.cicd.auto_resolve"),
                params,
            )?,
            policy,
        }),
        WorkflowGateFile::Custom {
            id,
            policy,
            mut args,
        } => {
            let id = expand_string_value(&id, &format!("{path_prefix}.custom.id"), params)?;
            for (arg_key, arg_value) in &mut args {
                let path = format!("{path_prefix}.custom.args.{arg_key}");
                *arg_value = expand_string_value(arg_value, &path, params)?;
            }
            Ok(WorkflowGate::Custom { id, policy, args })
        }
    }
}

fn expand_retry_policy(
    retry: WorkflowRetryPolicyFile,
    path_prefix: &str,
    params: &BTreeMap<String, String>,
) -> Result<WorkflowRetryPolicy, Box<dyn std::error::Error>> {
    let mode = coerce_retry_mode(retry.mode, &format!("{path_prefix}.mode"), params)?;
    let budget = coerce_retry_budget(retry.budget, &format!("{path_prefix}.budget"), params)?;
    Ok(WorkflowRetryPolicy { mode, budget })
}

fn coerce_bool(
    value: TemplateBoolValue,
    field_path: &str,
    params: &BTreeMap<String, String>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let raw_value = match value {
        TemplateBoolValue::Bool(value) => return Ok(value),
        TemplateBoolValue::String(value) => expand_string_value(&value, field_path, params)?,
        TemplateBoolValue::I64(value) => value.to_string(),
        TemplateBoolValue::U64(value) => value.to_string(),
        TemplateBoolValue::F64(value) => value.to_string(),
    };

    let normalized = raw_value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(format!("invalid bool value `{raw_value}` for {field_path}").into()),
    }
}

fn coerce_retry_mode(
    value: WorkflowRetryModeValue,
    field_path: &str,
    params: &BTreeMap<String, String>,
) -> Result<WorkflowRetryMode, Box<dyn std::error::Error>> {
    match value {
        WorkflowRetryModeValue::Mode(mode) => Ok(mode),
        WorkflowRetryModeValue::String(raw) => {
            let expanded = expand_string_value(&raw, field_path, params)?;
            parse_retry_mode(&expanded, field_path)
        }
    }
}

fn parse_retry_mode(
    value: &str,
    field_path: &str,
) -> Result<WorkflowRetryMode, Box<dyn std::error::Error>> {
    let raw_value = value.trim();
    serde_json::from_value::<WorkflowRetryMode>(serde_json::Value::String(raw_value.to_string()))
        .map_err(|_| format!("invalid retry mode value `{raw_value}` for {field_path}").into())
}

fn coerce_retry_budget(
    value: WorkflowRetryBudgetValue,
    field_path: &str,
    params: &BTreeMap<String, String>,
) -> Result<u32, Box<dyn std::error::Error>> {
    match value {
        WorkflowRetryBudgetValue::U32(value) => Ok(value),
        WorkflowRetryBudgetValue::U64(value) => u32::try_from(value)
            .map_err(|_| format!("invalid u32 value `{value}` for {field_path}").into()),
        WorkflowRetryBudgetValue::I64(value) => {
            if value < 0 {
                return Err(format!("invalid u32 value `{value}` for {field_path}").into());
            }
            u32::try_from(value)
                .map_err(|_| format!("invalid u32 value `{value}` for {field_path}").into())
        }
        WorkflowRetryBudgetValue::String(raw) => {
            let expanded = expand_string_value(&raw, field_path, params)?;
            let trimmed = expanded.trim();
            if trimmed.is_empty() || !trimmed.chars().all(|ch| ch.is_ascii_digit()) {
                return Err(format!("invalid u32 value `{expanded}` for {field_path}").into());
            }
            trimmed
                .parse::<u32>()
                .map_err(|_| format!("invalid u32 value `{expanded}` for {field_path}").into())
        }
        WorkflowRetryBudgetValue::F64(value) => {
            Err(format!("invalid u32 value `{value}` for {field_path}").into())
        }
    }
}

fn expand_string_value(
    value: &str,
    field_path: &str,
    params: &BTreeMap<String, String>,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0usize;

    while let Some(start_rel) = value[cursor..].find("${") {
        let start = cursor + start_rel;
        out.push_str(&value[cursor..start]);

        let body_start = start + 2;
        let Some(end_rel) = value[body_start..].find('}') else {
            return Err(format!("unclosed parameter placeholder in {field_path}").into());
        };
        let end = body_start + end_rel;
        let key = value[body_start..end].trim();
        if key.is_empty() {
            return Err(format!("empty parameter placeholder in {field_path}").into());
        }

        let replacement = params
            .get(key)
            .ok_or_else(|| format!("unresolved parameter `{key}` in {field_path}"))?;
        out.push_str(replacement);
        cursor = end + 1;
    }

    out.push_str(&value[cursor..]);
    Ok(out)
}

fn resolve_selector_path(
    project_root: &Path,
    selector: &str,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    if selector.trim().is_empty() {
        return Ok(None);
    }

    if is_explicit_file_source(selector) {
        let path = resolve_source_path(project_root, selector)?;
        return Ok(Some(path));
    }

    if let Some((id, version)) = parse_selector_identity(selector) {
        let matches = find_selector_file_candidates(project_root, &id, &version)?;
        if matches.len() > 1 {
            return Err(format!(
                "template selector `{selector}` is ambiguous; matching files: {}",
                matches
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .into());
        }
        return Ok(matches.into_iter().next());
    }

    Ok(None)
}

fn parse_selector_identity(selector: &str) -> Option<(String, String)> {
    let (id, version) = selector.rsplit_once('@')?;
    let id = id.trim();
    let version = version.trim();
    if id.is_empty() || version.is_empty() {
        return None;
    }
    Some((id.to_string(), version.to_string()))
}

fn find_selector_file_candidates(
    project_root: &Path,
    id: &str,
    version: &str,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut matches = Vec::new();
    for path in candidate_template_paths(project_root)? {
        if let Some((candidate_id, candidate_version)) = read_template_identity(&path)?
            && candidate_id == id
            && candidate_version == version
        {
            matches.push(path);
        }
    }
    Ok(matches)
}

fn candidate_template_paths(
    project_root: &Path,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    for root in [
        project_root.join(".vizier"),
        project_root.join(".vizier/workflow"),
    ] {
        if !root.is_dir() {
            continue;
        }
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| value.to_ascii_lowercase());
            if matches!(ext.as_deref(), Some("toml") | Some("json")) {
                out.push(path);
            }
        }
    }
    Ok(out)
}

fn read_template_identity(
    path: &Path,
) -> Result<Option<(String, String)>, Box<dyn std::error::Error>> {
    let contents = fs::read_to_string(path)?;
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("toml") => {
            let value = toml::from_str::<toml::Value>(&contents)?;
            let id = value.get("id").and_then(toml::Value::as_str);
            let version = value.get("version").and_then(toml::Value::as_str);
            Ok(match (id, version) {
                (Some(id), Some(version)) => Some((id.to_string(), version.to_string())),
                _ => None,
            })
        }
        Some("json") => {
            let value = serde_json::from_str::<serde_json::Value>(&contents)?;
            let id = value.get("id").and_then(serde_json::Value::as_str);
            let version = value.get("version").and_then(serde_json::Value::as_str);
            Ok(match (id, version) {
                (Some(id), Some(version)) => Some((id.to_string(), version.to_string())),
                _ => None,
            })
        }
        _ => Ok(None),
    }
}

fn resolve_repo_fallback_path(project_root: &Path, flow: &str) -> Option<PathBuf> {
    let candidates = [
        project_root.join(format!(".vizier/{flow}.toml")),
        project_root.join(format!(".vizier/{flow}.json")),
        project_root.join(format!(".vizier/workflow/{flow}.toml")),
        project_root.join(format!(".vizier/workflow/{flow}.json")),
    ];

    candidates
        .into_iter()
        .find(|path| path.is_file())
        .and_then(|path| resolve_source_path(project_root, &path.to_string_lossy()).ok())
}

fn is_explicit_file_source(value: &str) -> bool {
    if value.starts_with("file:") {
        return true;
    }

    value.starts_with('.')
        || value.starts_with('/')
        || value.contains('/')
        || value.contains('\\')
        || value.ends_with(".toml")
        || value.ends_with(".json")
}

fn resolve_source_path(
    project_root: &Path,
    source: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let raw = source.strip_prefix("file:").unwrap_or(source).trim();
    if raw.is_empty() {
        return Err("file selector is missing a path".into());
    }

    let path = Path::new(raw);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };

    let canonical = fs::canonicalize(&candidate).map_err(|err| {
        format!(
            "workflow source `{}` does not resolve to a readable file: {err}",
            candidate.display()
        )
    })?;

    if !canonical.is_file() {
        return Err(format!("workflow source `{}` is not a file", canonical.display()).into());
    }
    let canonical_root =
        fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    if !canonical.starts_with(&canonical_root) && !canonical.starts_with(project_root) {
        return Err(format!(
            "workflow source `{}` is outside repository root {}",
            canonical.display(),
            project_root.display()
        )
        .into());
    }

    Ok(canonical)
}

fn normalize_selector_from_path(project_root: &Path, raw: &str, path: &Path) -> String {
    if raw.starts_with("file:") {
        return raw.to_string();
    }
    if Path::new(raw).is_absolute() {
        return raw.to_string();
    }

    let relative = path
        .strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy();
    format!("file:{relative}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        let mut file = fs::File::create(path).expect("create file");
        file.write_all(contents.as_bytes()).expect("write file");
    }

    #[test]
    fn resolves_alias_to_file_selector() {
        let root = tempfile::tempdir().expect("tempdir");
        write(
            &root.path().join(".vizier/config.toml"),
            "[commands]\ndevelop = \"file:.vizier/develop.toml\"\n",
        );
        write(
            &root.path().join(".vizier/develop.toml"),
            "id = \"template.develop\"\nversion = \"v1\"\n[[nodes]]\nid = \"n1\"\nkind = \"shell\"\nuses = \"cap.env.shell.command.run\"\n[nodes.args]\nscript = \"true\"\n",
        );

        let cfg = config::load_config_from_path(root.path().join(".vizier/config.toml"))
            .expect("load config");
        let resolved =
            resolve_workflow_source(root.path(), "develop", &cfg).expect("resolve source");

        assert_eq!(resolved.selector, "file:.vizier/develop.toml");
        assert_eq!(
            resolved.command_alias.as_ref().map(|alias| alias.as_str()),
            Some("develop")
        );
        assert!(resolved.path.ends_with(".vizier/develop.toml"));
    }

    #[test]
    fn composes_import_links_with_prefixed_nodes() {
        let root = tempfile::tempdir().expect("tempdir");
        write(
            &root.path().join(".vizier/workflow/a.toml"),
            "id = \"template.a\"\nversion = \"v1\"\n[[nodes]]\nid = \"a1\"\nkind = \"shell\"\nuses = \"cap.env.shell.command.run\"\n[nodes.args]\nscript = \"true\"\n",
        );
        write(
            &root.path().join(".vizier/workflow/b.toml"),
            "id = \"template.b\"\nversion = \"v1\"\n[[nodes]]\nid = \"b1\"\nkind = \"shell\"\nuses = \"cap.env.shell.command.run\"\n[nodes.args]\nscript = \"true\"\n",
        );
        write(
            &root.path().join(".vizier/develop.toml"),
            "id = \"template.develop\"\nversion = \"v1\"\n[[imports]]\nname = \"stage_a\"\npath = \"workflow/a.toml\"\n[[imports]]\nname = \"stage_b\"\npath = \"workflow/b.toml\"\n[[links]]\nfrom = \"stage_a\"\nto = \"stage_b\"\n",
        );

        let source = ResolvedWorkflowSource {
            selector: "file:.vizier/develop.toml".to_string(),
            path: root.path().join(".vizier/develop.toml"),
            command_alias: Some(config::CommandAlias::parse("develop").expect("alias")),
        };

        let template =
            load_template_with_params(&source, &BTreeMap::new()).expect("load composed template");
        assert_eq!(template.id, "template.develop");
        assert_eq!(template.nodes.len(), 2);
        assert!(template.nodes.iter().any(|node| node.id == "stage_a__a1"));
        assert!(template.nodes.iter().any(|node| node.id == "stage_b__b1"));

        let stage_a = template
            .nodes
            .iter()
            .find(|node| node.id == "stage_a__a1")
            .expect("stage_a node");
        assert_eq!(stage_a.on.succeeded, vec!["stage_b__b1".to_string()]);
    }

    #[test]
    fn parameter_expansion_fails_for_unresolved_values() {
        let root = tempfile::tempdir().expect("tempdir");
        write(
            &root.path().join(".vizier/flow.toml"),
            "id = \"template.flow\"\nversion = \"v1\"\n[[nodes]]\nid = \"n1\"\nkind = \"shell\"\nuses = \"cap.env.shell.command.run\"\n[nodes.args]\nscript = \"echo ${missing}\"\n",
        );

        let source = ResolvedWorkflowSource {
            selector: "file:.vizier/flow.toml".to_string(),
            path: root.path().join(".vizier/flow.toml"),
            command_alias: None,
        };

        let err = load_template_with_params(&source, &BTreeMap::new())
            .expect_err("expected unresolved param failure");
        assert!(
            err.to_string().contains("unresolved parameter `missing`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parameter_expansion_covers_phase_one_fields() {
        let root = tempfile::tempdir().expect("tempdir");
        write(
            &root.path().join(".vizier/flow.json"),
            "{\n\
  \"id\": \"template.flow\",\n\
  \"version\": \"v1\",\n\
  \"params\": {\n\
    \"slug\": \"alpha\",\n\
    \"branch\": \"draft/alpha\",\n\
    \"lock_key\": \"alpha\",\n\
    \"gate_script\": \"test -f README.md\",\n\
    \"retry_mode\": \"on_failure\",\n\
    \"retry_budget\": \"3\",\n\
    \"approval_required\": \"yes\",\n\
    \"auto_resolve\": \"off\",\n\
    \"custom_gate_id\": \"conflict_handler\",\n\
    \"contract_version\": \"v2\"\n\
  },\n\
  \"artifact_contracts\": [\n\
    {\"id\": \"custom:prompt_text:${slug}\", \"version\": \"${contract_version}\"}\n\
  ],\n\
  \"nodes\": [\n\
    {\n\
      \"id\": \"n1\",\n\
      \"kind\": \"shell\",\n\
      \"uses\": \"cap.env.shell.command.run\",\n\
      \"args\": {\"script\": \"echo ${slug}\"},\n\
      \"needs\": [\n\
        {\"plan_doc\": {\"slug\": \"${slug}\", \"branch\": \"${branch}\"}}\n\
      ],\n\
      \"produces\": {\n\
        \"succeeded\": [\n\
          {\"custom\": {\"type_id\": \"prompt_text\", \"key\": \"${slug}\"}}\n\
        ]\n\
      },\n\
      \"locks\": [\n\
        {\"key\": \"plan:${lock_key}\", \"mode\": \"exclusive\"}\n\
      ],\n\
      \"preconditions\": [\n\
        {\"kind\": \"custom\", \"id\": \"check_branch\", \"args\": {\"branch\": \"${branch}\"}}\n\
      ],\n\
      \"gates\": [\n\
        {\"kind\": \"script\", \"script\": \"${gate_script}\"},\n\
        {\"kind\": \"approval\", \"required\": \"${approval_required}\"},\n\
        {\"kind\": \"cicd\", \"script\": \"echo gate\", \"auto_resolve\": \"${auto_resolve}\"},\n\
        {\"kind\": \"custom\", \"id\": \"${custom_gate_id}\", \"args\": {\"branch\": \"${branch}\"}}\n\
      ],\n\
      \"retry\": {\"mode\": \"${retry_mode}\", \"budget\": \"${retry_budget}\"}\n\
    }\n\
  ]\n\
}\n",
        );

        let source = ResolvedWorkflowSource {
            selector: "file:.vizier/flow.json".to_string(),
            path: root.path().join(".vizier/flow.json"),
            command_alias: None,
        };

        let set_overrides = BTreeMap::from([
            ("slug".to_string(), "beta".to_string()),
            ("branch".to_string(), "draft/beta".to_string()),
            ("retry_budget".to_string(), "7".to_string()),
        ]);
        let template =
            load_template_with_params(&source, &set_overrides).expect("load expanded template");
        let node = template.nodes.first().expect("node");

        assert_eq!(
            node.args.get("script").map(String::as_str),
            Some("echo beta")
        );
        assert_eq!(template.artifact_contracts.len(), 1);
        assert_eq!(
            template.artifact_contracts[0].id,
            "custom:prompt_text:beta".to_string()
        );
        assert_eq!(template.artifact_contracts[0].version, "v2".to_string());

        match &node.needs[0] {
            JobArtifact::PlanDoc { slug, branch } => {
                assert_eq!(slug, "beta");
                assert_eq!(branch, "draft/beta");
            }
            other => panic!("unexpected needs artifact: {other:?}"),
        }
        match &node.produces.succeeded[0] {
            JobArtifact::Custom { type_id, key } => {
                assert_eq!(type_id, "prompt_text");
                assert_eq!(key, "beta");
            }
            other => panic!("unexpected produced artifact: {other:?}"),
        }
        assert_eq!(node.locks[0].key, "plan:alpha");

        match &node.preconditions[0] {
            WorkflowPrecondition::Custom { args, .. } => {
                assert_eq!(args.get("branch").map(String::as_str), Some("draft/beta"));
            }
            other => panic!("unexpected precondition: {other:?}"),
        }

        assert_eq!(node.gates.len(), 4);
        match &node.gates[0] {
            WorkflowGate::Script { script, .. } => assert_eq!(script, "test -f README.md"),
            other => panic!("unexpected gate[0]: {other:?}"),
        }
        match &node.gates[1] {
            WorkflowGate::Approval { required, .. } => assert!(*required),
            other => panic!("unexpected gate[1]: {other:?}"),
        }
        match &node.gates[2] {
            WorkflowGate::Cicd { auto_resolve, .. } => assert!(!auto_resolve),
            other => panic!("unexpected gate[2]: {other:?}"),
        }
        match &node.gates[3] {
            WorkflowGate::Custom { id, args, .. } => {
                assert_eq!(id, "conflict_handler");
                assert_eq!(args.get("branch").map(String::as_str), Some("draft/beta"));
            }
            other => panic!("unexpected gate[3]: {other:?}"),
        }

        assert_eq!(node.retry.mode, WorkflowRetryMode::OnFailure);
        assert_eq!(node.retry.budget, 7);
    }

    #[test]
    fn parameter_expansion_fails_for_unresolved_non_args_field() {
        let root = tempfile::tempdir().expect("tempdir");
        write(
            &root.path().join(".vizier/flow.json"),
            "{\n\
  \"id\": \"template.flow\",\n\
  \"version\": \"v1\",\n\
  \"nodes\": [\n\
    {\n\
      \"id\": \"n1\",\n\
      \"kind\": \"shell\",\n\
      \"uses\": \"cap.env.shell.command.run\",\n\
      \"args\": {\"script\": \"true\"},\n\
      \"needs\": [\n\
        {\"plan_doc\": {\"slug\": \"${missing}\", \"branch\": \"main\"}}\n\
      ]\n\
    }\n\
  ]\n\
}\n",
        );

        let source = ResolvedWorkflowSource {
            selector: "file:.vizier/flow.json".to_string(),
            path: root.path().join(".vizier/flow.json"),
            command_alias: None,
        };

        let err = load_template_with_params(&source, &BTreeMap::new())
            .expect_err("expected unresolved non-args placeholder");
        assert!(
            err.to_string().contains("nodes[n1].needs[0].plan_doc.slug"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parameter_expansion_rejects_invalid_bool_coercion() {
        let root = tempfile::tempdir().expect("tempdir");
        write(
            &root.path().join(".vizier/flow.toml"),
            "id = \"template.flow\"\n\
version = \"v1\"\n\
[params]\n\
required = \"maybe\"\n\
[[nodes]]\n\
id = \"gate\"\n\
kind = \"gate\"\n\
uses = \"control.gate.approval\"\n\
[[nodes.gates]]\n\
kind = \"approval\"\n\
required = \"${required}\"\n",
        );

        let source = ResolvedWorkflowSource {
            selector: "file:.vizier/flow.toml".to_string(),
            path: root.path().join(".vizier/flow.toml"),
            command_alias: None,
        };

        let err = load_template_with_params(&source, &BTreeMap::new())
            .expect_err("expected invalid bool coercion");
        assert!(
            err.to_string()
                .contains("invalid bool value `maybe` for nodes[gate].gates[0].approval.required"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parameter_expansion_rejects_invalid_retry_budget_coercion() {
        let root = tempfile::tempdir().expect("tempdir");
        write(
            &root.path().join(".vizier/flow.toml"),
            "id = \"template.flow\"\n\
version = \"v1\"\n\
[params]\n\
budget = \"-1\"\n\
[[nodes]]\n\
id = \"n1\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
script = \"true\"\n\
[nodes.retry]\n\
budget = \"${budget}\"\n",
        );

        let source = ResolvedWorkflowSource {
            selector: "file:.vizier/flow.toml".to_string(),
            path: root.path().join(".vizier/flow.toml"),
            command_alias: None,
        };

        let err = load_template_with_params(&source, &BTreeMap::new())
            .expect_err("expected invalid retry budget coercion");
        assert!(
            err.to_string()
                .contains("invalid u32 value `-1` for nodes[n1].retry.budget"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parameter_expansion_rejects_invalid_retry_mode_coercion() {
        let root = tempfile::tempdir().expect("tempdir");
        write(
            &root.path().join(".vizier/flow.toml"),
            "id = \"template.flow\"\n\
version = \"v1\"\n\
[params]\n\
mode = \"sometimes\"\n\
[[nodes]]\n\
id = \"n1\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
script = \"true\"\n\
[nodes.retry]\n\
mode = \"${mode}\"\n",
        );

        let source = ResolvedWorkflowSource {
            selector: "file:.vizier/flow.toml".to_string(),
            path: root.path().join(".vizier/flow.toml"),
            command_alias: None,
        };

        let err = load_template_with_params(&source, &BTreeMap::new())
            .expect_err("expected invalid retry mode coercion");
        assert!(
            err.to_string()
                .contains("invalid retry mode value `sometimes` for nodes[n1].retry.mode"),
            "unexpected error: {err}"
        );
    }
}
