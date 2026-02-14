use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use vizier_core::{
    config,
    workflow_template::{
        WorkflowArtifactContract, WorkflowNode, WorkflowTemplate, WorkflowTemplatePolicy,
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
    nodes: Vec<WorkflowNode>,
    #[serde(default)]
    imports: Vec<WorkflowTemplateImport>,
    #[serde(default)]
    links: Vec<WorkflowTemplateLink>,
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

#[derive(Debug, Clone)]
struct ImportedStage {
    params: BTreeMap<String, String>,
    artifact_contracts: Vec<WorkflowArtifactContract>,
    nodes: Vec<WorkflowNode>,
    terminal_nodes: Vec<String>,
    entry_nodes: Vec<String>,
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
    let mut template = load_template_recursive(&source.path, &mut stack)?;
    apply_parameter_expansion(&mut template, set_overrides)?;
    Ok(template)
}

fn load_template_recursive(
    path: &Path,
    stack: &mut Vec<PathBuf>,
) -> Result<WorkflowTemplate, Box<dyn std::error::Error>> {
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
        Ok(WorkflowTemplate {
            id: parsed.id,
            version: parsed.version,
            params: parsed.params,
            policy: parsed.policy,
            artifact_contracts: parsed.artifact_contracts,
            nodes: parsed.nodes,
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
) -> Result<WorkflowTemplate, Box<dyn std::error::Error>> {
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

    Ok(WorkflowTemplate {
        id: parsed.id,
        version: parsed.version,
        params,
        policy: parsed.policy,
        artifact_contracts,
        nodes,
    })
}

fn prefix_stage(
    stage_name: &str,
    template: &WorkflowTemplate,
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
    template: &mut WorkflowTemplate,
    set_overrides: &BTreeMap<String, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut params = template.params.clone();
    for (key, value) in set_overrides {
        params.insert(key.clone(), value.clone());
    }

    for node in &mut template.nodes {
        for (arg_key, arg_value) in &mut node.args {
            let expanded = expand_arg_value(&node.id, arg_key, arg_value, &params)?;
            *arg_value = expanded;
        }
    }

    template.params = params;
    Ok(())
}

fn expand_arg_value(
    node_id: &str,
    arg_key: &str,
    value: &str,
    params: &BTreeMap<String, String>,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0usize;

    while let Some(start_rel) = value[cursor..].find("${") {
        let start = cursor + start_rel;
        out.push_str(&value[cursor..start]);

        let body_start = start + 2;
        let Some(end_rel) = value[body_start..].find('}') else {
            return Err(format!(
                "unclosed parameter placeholder in node `{node_id}` arg `{arg_key}`"
            )
            .into());
        };
        let end = body_start + end_rel;
        let key = value[body_start..end].trim();
        if key.is_empty() {
            return Err(
                format!("empty parameter placeholder in node `{node_id}` arg `{arg_key}`").into(),
            );
        }

        let replacement = params.get(key).ok_or_else(|| {
            format!("unresolved parameter `{key}` in node `{node_id}` arg `{arg_key}`")
        })?;
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
}
