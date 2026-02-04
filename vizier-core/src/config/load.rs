use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::{display, tools, tree};

use super::*;

use lazy_static::lazy_static;

#[derive(Copy, Clone)]
enum FileFormat {
    Json,
    Toml,
}

const MODEL_KEY_PATHS: &[&[&str]] = &[
    &["model"],
    &["provider"],
    &["provider", "model"],
    &["provider", "name"],
];
const BACKEND_KEY_PATHS: &[&[&str]] = &[&["backend"], &["provider", "backend"]];
const FALLBACK_BACKEND_KEY_PATHS: &[&[&str]] = &[&["fallback_backend"], &["fallback-backend"]];
const FALLBACK_BACKEND_DEPRECATION_MESSAGE: &str =
    "fallback_backend entries are unsupported; remove them from your config.";
const REASONING_EFFORT_KEY_PATHS: &[&[&str]] = &[
    &["reasoning_effort"],
    &["reasoning-effort"],
    &["thinking_level"],
    &["thinking-level"],
    &["provider", "reasoning_effort"],
    &["provider", "reasoning-effort"],
    &["provider", "thinking_level"],
    &["provider", "thinking-level"],
    &["flags", "reasoning_effort"],
    &["flags", "reasoning-effort"],
    &["flags", "thinking_level"],
    &["flags", "thinking-level"],
];
const MODEL_CONFIG_REMOVED_MESSAGE: &str =
    "model overrides are no longer supported now that the wire backend has been removed.";
const REASONING_CONFIG_REMOVED_MESSAGE: &str = "reasoning-effort overrides are no longer supported now that the wire backend has been removed.";

lazy_static! {
    static ref CONFIG: RwLock<Config> = RwLock::new(default_config_with_repo_prompts());
}

fn value_at_path<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    let mut current = value;

    for segment in path {
        match current {
            serde_json::Value::Object(map) => {
                current = map.get(*segment)?;
            }
            _ => return None,
        }
    }

    Some(current)
}

fn find_string(value: &serde_json::Value, paths: &[&[&str]]) -> Option<String> {
    for path in paths {
        if let Some(serde_json::Value::String(s)) = value_at_path(value, path)
            && !s.is_empty()
        {
            return Some(s.clone());
        }
    }

    None
}

fn parse_agent_overrides(
    value: &serde_json::Value,
    allow_prompt_children: bool,
    base_dir: Option<&Path>,
) -> Result<Option<AgentOverrides>, Box<dyn std::error::Error>> {
    if !value.is_object() {
        return Ok(None);
    }

    if find_string(value, FALLBACK_BACKEND_KEY_PATHS).is_some() {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidInput,
            FALLBACK_BACKEND_DEPRECATION_MESSAGE,
        )));
    }

    let mut overrides = AgentOverrides::default();

    if find_string(value, MODEL_KEY_PATHS).is_some() {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidInput,
            MODEL_CONFIG_REMOVED_MESSAGE,
        )));
    }

    if find_string(value, REASONING_EFFORT_KEY_PATHS).is_some() {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidInput,
            REASONING_CONFIG_REMOVED_MESSAGE,
        )));
    }

    if let Some(agent_value) = value.get("agent") {
        if let Some(raw) = agent_value.as_str() {
            if overrides.selector.is_none()
                && let Some(selector) = normalize_selector_value(raw)
            {
                overrides.selector = Some(selector);
            }
        } else if let Some(parsed) = parse_agent_runtime_override(agent_value)? {
            overrides.agent_runtime = Some(parsed);
        }
    }

    if find_string(value, BACKEND_KEY_PATHS).is_some() {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidInput,
            "backend entries are unsupported; use agent selectors instead",
        )));
    }

    if allow_prompt_children {
        if let Some(doc_settings) = parse_documentation_settings(value)? {
            overrides.documentation = doc_settings;
        }

        if let Some(prompts_value) = value_at_path(value, &["prompts"]) {
            overrides.prompt_overrides =
                parse_prompt_override_table(prompts_value, base_dir)?.unwrap_or_default();
        }
    }

    if overrides.is_empty() {
        Ok(None)
    } else {
        Ok(Some(overrides))
    }
}

fn parse_prompt_override_table(
    value: &serde_json::Value,
    base_dir: Option<&Path>,
) -> Result<Option<HashMap<PromptKind, PromptOverrides>>, Box<dyn std::error::Error>> {
    let table = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(None),
    };

    let mut overrides = HashMap::new();

    for (key, entry) in table {
        let Some(kind) = prompt_kind_from_key(key) else {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown prompt kind `{key}`"),
            )));
        };

        let mut prompt_override = PromptOverrides::default();

        match entry {
            serde_json::Value::String(text) => {
                if !text.trim().is_empty() {
                    prompt_override.text = Some(text.clone());
                }
            }
            serde_json::Value::Object(_) => {
                if let Some(path) = parse_prompt_path(entry, base_dir)? {
                    prompt_override.source_path = Some(path.clone());
                    prompt_override.text = Some(std::fs::read_to_string(&path)?);
                } else if let Some(text) =
                    parse_inline_prompt_text(entry).map(|text| text.to_string())
                {
                    prompt_override.text = Some(text);
                }

                if let Some(agent) = parse_agent_overrides(entry, false, base_dir)? {
                    prompt_override.agent = Some(Box::new(agent));
                }
            }
            _ => continue,
        }

        if prompt_override.text.is_none() && prompt_override.agent.is_none() {
            continue;
        }

        overrides.insert(kind, prompt_override);
    }

    if overrides.is_empty() {
        Ok(None)
    } else {
        Ok(Some(overrides))
    }
}

fn parse_prompt_path(
    entry: &serde_json::Value,
    base_dir: Option<&Path>,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    let Some(object) = entry.as_object() else {
        return Ok(None);
    };

    let path_value = object
        .get("path")
        .or_else(|| object.get("file"))
        .and_then(|value| value.as_str());

    let Some(raw_path) = path_value
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    let mut resolved = PathBuf::from(raw_path);
    if resolved.is_relative()
        && let Some(base) = base_dir
    {
        resolved = base.join(resolved);
    }

    Ok(Some(resolved))
}

fn parse_inline_prompt_text(entry: &serde_json::Value) -> Option<&str> {
    let object = entry.as_object()?;
    for key in ["text", "prompt", "template", "inline"] {
        if let Some(value) = object.get(key)
            && let Some(text) = value.as_str()
        {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    None
}

fn parse_command_value(value: &serde_json::Value) -> Option<Vec<String>> {
    match value {
        serde_json::Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(vec![trimmed.to_string()])
            }
        }
        serde_json::Value::Array(entries) => {
            let mut parts = Vec::new();
            for entry in entries {
                if let Some(text) = entry.as_str().map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    parts.push(text.to_string());
                }
            }
            if parts.is_empty() { None } else { Some(parts) }
        }
        _ => None,
    }
}

fn parse_agent_sections_into_layer(
    layer: &mut ConfigLayer,
    agents_value: &serde_json::Value,
    base_dir: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let table = agents_value
        .as_object()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "[agents] must be a table"))?;

    for (key, value) in table.iter() {
        let Some(overrides) = parse_agent_overrides(value, true, base_dir)? else {
            continue;
        };

        if key.eq_ignore_ascii_case("default") {
            layer.agent_defaults = Some(overrides);
            continue;
        }

        let scope = key.parse::<CommandScope>().map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown [agents.{key}] section: {err}"),
            )
        })?;
        layer.agent_scopes.insert(scope, overrides);
    }

    Ok(())
}

pub fn load_config_layer_from_json(
    filepath: PathBuf,
) -> Result<ConfigLayer, Box<dyn std::error::Error>> {
    load_config_layer_from_reader(filepath.as_path(), FileFormat::Json)
}

pub fn load_config_layer_from_toml(
    filepath: PathBuf,
) -> Result<ConfigLayer, Box<dyn std::error::Error>> {
    load_config_layer_from_reader(filepath.as_path(), FileFormat::Toml)
}

pub fn load_config_layer_from_path<P: AsRef<Path>>(
    filepath: P,
) -> Result<ConfigLayer, Box<dyn std::error::Error>> {
    let path = filepath.as_ref();

    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());

    match ext.as_deref() {
        Some("json") => load_config_layer_from_reader(path, FileFormat::Json),
        Some("toml") => load_config_layer_from_reader(path, FileFormat::Toml),
        _ => load_config_layer_from_reader(path, FileFormat::Toml)
            .or_else(|_| load_config_layer_from_reader(path, FileFormat::Json)),
    }
}

fn load_config_layer_from_reader(
    path: &Path,
    format: FileFormat,
) -> Result<ConfigLayer, Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(path)?;
    let base_dir = path.parent();
    load_config_layer_from_str(&contents, format, base_dir)
}

fn load_config_layer_from_str(
    contents: &str,
    format: FileFormat,
    base_dir: Option<&Path>,
) -> Result<ConfigLayer, Box<dyn std::error::Error>> {
    let file_config: serde_json::Value = match format {
        FileFormat::Json => serde_json::from_str(contents)?,
        FileFormat::Toml => toml::from_str(contents)?,
    };

    load_config_layer_from_value(file_config, base_dir)
}

fn load_config_layer_from_value(
    file_config: serde_json::Value,
    base_dir: Option<&Path>,
) -> Result<ConfigLayer, Box<dyn std::error::Error>> {
    let mut layer = ConfigLayer::default();

    if find_string(&file_config, MODEL_KEY_PATHS).is_some() {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidInput,
            MODEL_CONFIG_REMOVED_MESSAGE,
        )));
    }

    if find_string(&file_config, REASONING_EFFORT_KEY_PATHS).is_some() {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidInput,
            REASONING_CONFIG_REMOVED_MESSAGE,
        )));
    }

    if find_string(&file_config, FALLBACK_BACKEND_KEY_PATHS).is_some() {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidInput,
            FALLBACK_BACKEND_DEPRECATION_MESSAGE,
        )));
    }

    if let Some(agent_value) = value_at_path(&file_config, &["agent"]) {
        if let Some(raw) = agent_value.as_str() {
            if let Some(selector) = normalize_selector_value(raw) {
                layer.agent_selector = Some(selector);
            }
        } else if let Some(parsed) = parse_agent_runtime_override(agent_value)? {
            layer.agent_runtime = Some(parsed);
        }
    }

    if find_string(&file_config, BACKEND_KEY_PATHS).is_some() {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidInput,
            "backend entries are unsupported; use agent selectors instead",
        )));
    }

    if let Some(commands) = parse_string_array(value_at_path(
        &file_config,
        &["review", "checks", "commands"],
    )) {
        layer.review.checks = Some(commands);
    } else if let Some(commands) =
        parse_string_array(value_at_path(&file_config, &["review", "checks"]))
    {
        layer.review.checks = Some(commands);
    }

    if let Some(stop_condition) = value_at_path(&file_config, &["approve", "stop_condition"]) {
        if let Some(script) = stop_condition
            .get("script")
            .and_then(|value| value.as_str())
            && !script.trim().is_empty()
        {
            layer.approve.stop_condition.script = Some(PathBuf::from(script.trim()));
        }

        if let Some(retries) = parse_u32(
            stop_condition
                .get("retries")
                .or_else(|| stop_condition.get("max_attempts"))
                .or_else(|| stop_condition.get("max-attempts")),
        ) {
            layer.approve.stop_condition.retries = Some(retries);
        }
    }

    if let Some(merge_table) = value_at_path(&file_config, &["merge"]) {
        if let Some(squash) = merge_table.get("squash").and_then(|value| value.as_bool()) {
            layer.merge.squash_default = Some(squash);
        }

        if let Some(mainline) = merge_table
            .get("squash_mainline")
            .or_else(|| merge_table.get("squash-mainline"))
            .and_then(|value| value.as_i64())
            && mainline > 0
        {
            layer.merge.squash_mainline = Some(mainline as u32);
        }

        if let Some(gate) = merge_table
            .get("cicd_gate")
            .or_else(|| merge_table.get("cicd-gate"))
        {
            parse_merge_cicd_gate(gate, base_dir, &mut layer.merge.cicd_gate)?;
        }

        if let Some(conflicts) = merge_table
            .get("conflicts")
            .or_else(|| merge_table.get("conflict"))
            && let Some(auto_resolve) = conflicts
                .get("auto_resolve")
                .or_else(|| conflicts.get("auto-resolve"))
                .and_then(|value| value.as_bool())
        {
            layer.merge.conflicts.auto_resolve = Some(auto_resolve);
        }
    }

    if let Some(commits_table) = value_at_path(&file_config, &["commits"]) {
        parse_commit_table(commits_table, &mut layer.commits)?;
    }

    if let Some(display_table) = value_at_path(&file_config, &["display"]) {
        parse_display_table(display_table, &mut layer.display)?;
    }

    if let Some(jobs_table) = value_at_path(&file_config, &["jobs"]) {
        parse_jobs_table(jobs_table, &mut layer.jobs)?;
    }

    if let Some(workflow_table) = value_at_path(&file_config, &["workflow"]) {
        parse_workflow_table(workflow_table, &mut layer.workflow)?;
    }

    if let Some(agents_value) = value_at_path(&file_config, &["agents"]) {
        parse_agent_sections_into_layer(&mut layer, agents_value, base_dir)?;
    }

    Ok(layer)
}

pub fn load_config_from_json(filepath: PathBuf) -> Result<Config, Box<dyn std::error::Error>> {
    load_config_from_layer(load_config_layer_from_json(filepath)?)
}

pub fn load_config_from_toml(filepath: PathBuf) -> Result<Config, Box<dyn std::error::Error>> {
    load_config_from_layer(load_config_layer_from_toml(filepath)?)
}

pub fn load_config_from_path<P: AsRef<Path>>(
    filepath: P,
) -> Result<Config, Box<dyn std::error::Error>> {
    load_config_from_layer(load_config_layer_from_path(filepath)?)
}

fn load_config_from_layer(layer: ConfigLayer) -> Result<Config, Box<dyn std::error::Error>> {
    let mut config = Config::from_layers(&[layer]);
    attach_repo_prompts(&mut config);
    Ok(config)
}

fn parse_agent_runtime_override(
    value: &serde_json::Value,
) -> Result<Option<AgentRuntimeOverride>, Box<dyn std::error::Error>> {
    let object = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(None),
    };

    let mut overrides = AgentRuntimeOverride::default();

    let allowed_keys = [
        "label",
        "command",
        "progress_filter",
        "output",
        "enable_script_wrapper",
    ];
    for key in object.keys() {
        if !allowed_keys.contains(&key.as_str()) {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                "agent runtime supports only label, command, progress_filter, output, and enable_script_wrapper",
            )));
        }
    }

    if let Some(label) = object
        .get("label")
        .and_then(|value| value.as_str().map(|s| s.trim()).filter(|s| !s.is_empty()))
    {
        overrides.label = Some(label.to_ascii_lowercase());
    }

    if let Some(command) = object.get("command") {
        if let Some(parsed) = parse_command_value(command) {
            overrides.command = Some(parsed);
        } else {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                "`agent.command` must be a non-empty string or array",
            )));
        }
    }

    if let Some(filter) = object.get("progress_filter") {
        if let Some(parsed) = parse_command_value(filter) {
            overrides.progress_filter = Some(parsed);
        } else {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                "`agent.progress_filter` must be a non-empty string or array",
            )));
        }
    }

    if let Some(output) = object.get("output") {
        if let Some(value) = output.as_str() {
            let normalized = value.trim().to_ascii_lowercase();
            overrides.output = match normalized.as_str() {
                "" | "auto" | "wrapped" | "wrapped-json" => Some(AgentOutputMode::Auto),
                other => {
                    return Err(Box::new(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "unknown agent.output value `{other}` (expected auto|wrapped-json)"
                        ),
                    )));
                }
            };
        } else {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                "`agent.output` must be a string (auto|wrapped-json)",
            )));
        }
    }

    if let Some(enable_script) = parse_bool(object.get("enable_script_wrapper")) {
        overrides.enable_script_wrapper = Some(enable_script);
    }

    if overrides == AgentRuntimeOverride::default() {
        Ok(None)
    } else {
        Ok(Some(overrides))
    }
}

fn parse_documentation_settings(
    value: &serde_json::Value,
) -> Result<Option<DocumentationSettingsOverride>, Box<dyn std::error::Error>> {
    let Some(table) = value_at_path(value, &["documentation"]).and_then(|v| v.as_object()) else {
        return Ok(None);
    };

    let mut overrides = DocumentationSettingsOverride::default();

    if let Some(enabled) = parse_bool(
        table
            .get("enabled")
            .or_else(|| table.get("enable"))
            .or_else(|| table.get("use_prompt"))
            .or_else(|| table.get("use-prompt"))
            .or_else(|| table.get("use_documentation_prompt"))
            .or_else(|| table.get("use-documentation-prompt")),
    ) {
        overrides.use_documentation_prompt = Some(enabled);
    }

    if let Some(include_snapshot) = parse_bool(
        table
            .get("include_snapshot")
            .or_else(|| table.get("include-snapshot"))
            .or_else(|| table.get("snapshot")),
    ) {
        overrides.include_snapshot = Some(include_snapshot);
    }

    if let Some(include_docs) = parse_bool(
        table
            .get("include_narrative_docs")
            .or_else(|| table.get("include-narrative-docs"))
            .or_else(|| table.get("include_narrative")),
    ) {
        overrides.include_narrative_docs = Some(include_docs);
    }

    if overrides.is_empty() {
        Ok(None)
    } else {
        Ok(Some(overrides))
    }
}

fn prompt_kind_from_key(key: &str) -> Option<PromptKind> {
    let normalized = key.trim().to_ascii_lowercase().replace('-', "_");

    match normalized.as_str() {
        "documentation" => Some(PromptKind::Documentation),
        "commit" => Some(PromptKind::Commit),
        "implementation_plan" => Some(PromptKind::ImplementationPlan),
        "review" => Some(PromptKind::Review),
        "merge_conflict" => Some(PromptKind::MergeConflict),
        _ => None,
    }
}

fn parse_string_array(value: Option<&serde_json::Value>) -> Option<Vec<String>> {
    let array = value?.as_array()?;
    let mut entries = Vec::new();
    for entry in array {
        if let Some(text) = entry.as_str().map(|s| s.trim()).filter(|s| !s.is_empty()) {
            entries.push(text.to_string());
        }
    }
    if entries.is_empty() {
        None
    } else {
        Some(entries)
    }
}

fn parse_string_array_allow_empty(value: Option<&serde_json::Value>) -> Option<Vec<String>> {
    let array = value?.as_array()?;
    let mut entries = Vec::new();
    for entry in array {
        if let Some(text) = entry.as_str().map(|s| s.trim()).filter(|s| !s.is_empty()) {
            entries.push(text.to_string());
        }
    }
    Some(entries)
}

fn parse_nonempty_string(value: Option<&serde_json::Value>) -> Option<String> {
    value?
        .as_str()
        .map(|text| text.trim())
        .filter(|text| !text.is_empty())
        .map(|text| text.to_string())
}

fn parse_string_map(value: Option<&serde_json::Value>) -> Option<HashMap<String, String>> {
    let object = value?.as_object()?;
    let mut values = HashMap::new();
    for (key, raw_value) in object {
        if let Some(text) = raw_value
            .as_str()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            values.insert(key.clone(), text.to_string());
        }
    }
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

fn parse_usize(value: Option<&serde_json::Value>) -> Option<usize> {
    let raw = value?;
    if let Some(num) = raw.as_u64() {
        return usize::try_from(num).ok();
    }
    if let Some(text) = raw.as_str() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Ok(parsed) = trimmed.parse::<usize>() {
            return Some(parsed);
        }
    }
    None
}

fn parse_commit_meta_fields(values: &[String]) -> Vec<CommitMetaField> {
    let mut fields = Vec::new();
    for value in values {
        if let Some(field) = CommitMetaField::parse(value) {
            fields.push(field);
        } else {
            display::warn(format!(
                "unknown commits.meta.include field `{}`; ignoring",
                value
            ));
        }
    }
    fields
}

fn parse_commit_implementation_fields(values: &[String]) -> Vec<CommitImplementationField> {
    let mut fields = Vec::new();
    for value in values {
        if let Some(field) = CommitImplementationField::parse(value) {
            fields.push(field);
        } else {
            display::warn(format!(
                "unknown commits.implementation.fields entry `{}`; ignoring",
                value
            ));
        }
    }
    fields
}

fn parse_bool(value: Option<&serde_json::Value>) -> Option<bool> {
    let raw = value?;
    match raw {
        serde_json::Value::Bool(inner) => Some(*inner),
        serde_json::Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.eq_ignore_ascii_case("true") {
                Some(true)
            } else if trimmed.eq_ignore_ascii_case("false") {
                Some(false)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn parse_u32(value: Option<&serde_json::Value>) -> Option<u32> {
    let raw = value?;
    if let Some(num) = raw.as_u64() {
        return u32::try_from(num).ok();
    }
    if let Some(text) = raw.as_str() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Ok(parsed) = trimmed.parse::<u32>() {
            return Some(parsed);
        }
    }
    None
}

fn parse_merge_cicd_gate(
    value: &serde_json::Value,
    _base_dir: Option<&Path>,
    gate: &mut MergeCicdGateLayer,
) -> Result<(), Box<dyn std::error::Error>> {
    let table = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(()),
    };

    if let Some(script) = parse_nonempty_string(
        table
            .get("script")
            .or_else(|| table.get("path"))
            .or_else(|| table.get("file")),
    ) {
        gate.script = Some(PathBuf::from(script));
    }

    if let Some(auto_resolve) = parse_bool(
        table
            .get("auto_resolve")
            .or_else(|| table.get("auto-resolve"))
            .or_else(|| table.get("auto_fix"))
            .or_else(|| table.get("auto-fix"))
            .or_else(|| table.get("auto_resolve"))
            .or_else(|| table.get("auto-resolve")),
    ) {
        gate.auto_resolve = Some(auto_resolve);
    }

    if let Some(retries) = parse_u32(
        table
            .get("retries")
            .or_else(|| table.get("max_attempts"))
            .or_else(|| table.get("max-attempts")),
    ) {
        gate.retries = Some(retries);
    }

    Ok(())
}

fn parse_commit_table(
    value: &serde_json::Value,
    layer: &mut CommitLayer,
) -> Result<(), Box<dyn std::error::Error>> {
    let table = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(()),
    };

    if let Some(meta_table) = table.get("meta") {
        parse_commit_meta_table(meta_table, &mut layer.meta)?;
    }

    if let Some(fallback) = table
        .get("fallback_subjects")
        .or_else(|| table.get("fallback-subjects"))
    {
        parse_commit_fallback_subjects(fallback, &mut layer.fallback_subjects)?;
    }

    if let Some(implementation) = table.get("implementation") {
        parse_commit_implementation_table(implementation, &mut layer.implementation)?;
    }

    if let Some(merge_table) = table.get("merge") {
        parse_commit_merge_table(merge_table, &mut layer.merge)?;
    }

    Ok(())
}

fn parse_commit_meta_table(
    value: &serde_json::Value,
    layer: &mut CommitMetaLayer,
) -> Result<(), Box<dyn std::error::Error>> {
    let table = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(()),
    };

    if let Some(enabled) = parse_bool(table.get("enabled").or_else(|| table.get("enable"))) {
        layer.enabled = Some(enabled);
    }

    if let Some(style) = parse_nonempty_string(table.get("style")) {
        if let Some(parsed) = CommitMetaStyle::parse(&style) {
            layer.style = Some(parsed);
        } else {
            display::warn(format!(
                "unknown commits.meta.style value `{}`; ignoring",
                style
            ));
        }
    }

    if let Some(include) = parse_string_array_allow_empty(table.get("include")) {
        layer.include = Some(parse_commit_meta_fields(&include));
    }

    if let Some(path_mode) = parse_nonempty_string(
        table
            .get("session_log_path")
            .or_else(|| table.get("session-log-path"))
            .or_else(|| table.get("session_log")),
    ) {
        if let Some(parsed) = CommitSessionLogPath::parse(&path_mode) {
            layer.session_log_path = Some(parsed);
        } else {
            display::warn(format!(
                "unknown commits.meta.session_log_path value `{}`; ignoring",
                path_mode
            ));
        }
    }

    if let Some(labels) = table.get("labels").and_then(|value| value.as_object()) {
        if let Some(value) = parse_nonempty_string(
            labels
                .get("session_id")
                .or_else(|| labels.get("session-id")),
        ) {
            layer.labels.session_id = Some(value);
        }
        if let Some(value) = parse_nonempty_string(
            labels
                .get("session_log")
                .or_else(|| labels.get("session-log")),
        ) {
            layer.labels.session_log = Some(value);
        }
        if let Some(value) = parse_nonempty_string(
            labels
                .get("author_note")
                .or_else(|| labels.get("author-note")),
        ) {
            layer.labels.author_note = Some(value);
        }
        if let Some(value) = parse_nonempty_string(
            labels
                .get("narrative_summary")
                .or_else(|| labels.get("narrative-summary")),
        ) {
            layer.labels.narrative_summary = Some(value);
        }
    }

    Ok(())
}

fn parse_commit_fallback_subjects(
    value: &serde_json::Value,
    layer: &mut CommitFallbackSubjectsLayer,
) -> Result<(), Box<dyn std::error::Error>> {
    let table = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(()),
    };

    if let Some(code_change) = parse_nonempty_string(
        table
            .get("code_change")
            .or_else(|| table.get("code-change")),
    ) {
        layer.code_change = Some(code_change);
    }

    if let Some(narrative_change) = parse_nonempty_string(
        table
            .get("narrative_change")
            .or_else(|| table.get("narrative-change")),
    ) {
        layer.narrative_change = Some(narrative_change);
    }

    if let Some(conversation) = parse_nonempty_string(
        table
            .get("conversation")
            .or_else(|| table.get("conversation_subject")),
    ) {
        layer.conversation = Some(conversation);
    }

    Ok(())
}

fn parse_commit_implementation_table(
    value: &serde_json::Value,
    layer: &mut CommitImplementationLayer,
) -> Result<(), Box<dyn std::error::Error>> {
    let table = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(()),
    };

    if let Some(subject) = parse_nonempty_string(table.get("subject")) {
        layer.subject = Some(subject);
    }

    if let Some(fields) = parse_string_array_allow_empty(table.get("fields")) {
        layer.fields = Some(parse_commit_implementation_fields(&fields));
    }

    Ok(())
}

fn parse_commit_merge_table(
    value: &serde_json::Value,
    layer: &mut CommitMergeLayer,
) -> Result<(), Box<dyn std::error::Error>> {
    let table = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(()),
    };

    if let Some(subject) = parse_nonempty_string(table.get("subject")) {
        layer.subject = Some(subject);
    }

    if let Some(include_note) = parse_bool(
        table
            .get("include_operator_note")
            .or_else(|| table.get("include-operator-note")),
    ) {
        layer.include_operator_note = Some(include_note);
    }

    if let Some(label) = parse_nonempty_string(
        table
            .get("operator_note_label")
            .or_else(|| table.get("operator-note-label")),
    ) {
        layer.operator_note_label = Some(label);
    }

    if let Some(plan_mode) =
        parse_nonempty_string(table.get("plan_mode").or_else(|| table.get("plan-mode")))
    {
        if let Some(parsed) = CommitMergePlanMode::parse(&plan_mode) {
            layer.plan_mode = Some(parsed);
        } else {
            display::warn(format!(
                "unknown commits.merge.plan_mode value `{}`; ignoring",
                plan_mode
            ));
        }
    }

    if let Some(label) =
        parse_nonempty_string(table.get("plan_label").or_else(|| table.get("plan-label")))
    {
        layer.plan_label = Some(label);
    }

    Ok(())
}

fn parse_display_table(
    value: &serde_json::Value,
    layer: &mut DisplayLayer,
) -> Result<(), Box<dyn std::error::Error>> {
    let table = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(()),
    };

    if let Some(lists) = table.get("lists") {
        parse_display_lists_table(lists, &mut layer.lists)?;
    }

    Ok(())
}

fn parse_display_lists_table(
    value: &serde_json::Value,
    layer: &mut DisplayListsLayer,
) -> Result<(), Box<dyn std::error::Error>> {
    let table = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(()),
    };

    if let Some(list_table) = table.get("list") {
        parse_display_list_table(list_table, &mut layer.list)?;
    }

    if let Some(jobs_table) = table.get("jobs") {
        parse_display_jobs_list_table(jobs_table, &mut layer.jobs)?;
    }

    if let Some(jobs_show_table) = table.get("jobs_show").or_else(|| table.get("jobs-show")) {
        parse_display_jobs_show_table(jobs_show_table, &mut layer.jobs_show)?;
    }

    Ok(())
}

fn parse_display_list_table(
    value: &serde_json::Value,
    layer: &mut DisplayListLayer,
) -> Result<(), Box<dyn std::error::Error>> {
    let table = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(()),
    };

    if let Some(format) = parse_nonempty_string(table.get("format")) {
        if let Some(parsed) = ListFormat::parse(&format) {
            layer.format = Some(parsed);
        } else {
            display::warn(format!(
                "unknown display.lists.list.format value `{}`; ignoring",
                format
            ));
        }
    }

    if let Some(fields) = parse_string_array_allow_empty(
        table
            .get("header_fields")
            .or_else(|| table.get("header-fields")),
    ) {
        layer.header_fields = Some(fields);
    }

    if let Some(fields) = parse_string_array_allow_empty(
        table
            .get("entry_fields")
            .or_else(|| table.get("entry-fields")),
    ) {
        layer.entry_fields = Some(fields);
    }

    if let Some(fields) =
        parse_string_array_allow_empty(table.get("job_fields").or_else(|| table.get("job-fields")))
    {
        layer.job_fields = Some(fields);
    }

    if let Some(fields) = parse_string_array_allow_empty(
        table
            .get("command_fields")
            .or_else(|| table.get("command-fields")),
    ) {
        layer.command_fields = Some(fields);
    }

    if let Some(max_len) = parse_usize(
        table
            .get("summary_max_len")
            .or_else(|| table.get("summary-max-len")),
    ) {
        layer.summary_max_len = Some(max_len);
    }

    if let Some(single_line) = parse_bool(
        table
            .get("summary_single_line")
            .or_else(|| table.get("summary-single-line")),
    ) {
        layer.summary_single_line = Some(single_line);
    }

    if let Some(labels) = parse_string_map(table.get("labels")) {
        layer.labels = Some(labels);
    }

    Ok(())
}

fn parse_display_jobs_list_table(
    value: &serde_json::Value,
    layer: &mut DisplayJobsListLayer,
) -> Result<(), Box<dyn std::error::Error>> {
    let table = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(()),
    };

    if let Some(format) = parse_nonempty_string(table.get("format")) {
        if let Some(parsed) = ListFormat::parse(&format) {
            layer.format = Some(parsed);
        } else {
            display::warn(format!(
                "unknown display.lists.jobs.format value `{}`; ignoring",
                format
            ));
        }
    }

    if let Some(show) = parse_bool(
        table
            .get("show_succeeded")
            .or_else(|| table.get("show-succeeded")),
    ) {
        layer.show_succeeded = Some(show);
    }

    if let Some(fields) = parse_string_array_allow_empty(table.get("fields")) {
        layer.fields = Some(fields);
    }

    if let Some(labels) = parse_string_map(table.get("labels")) {
        layer.labels = Some(labels);
    }

    Ok(())
}

fn parse_display_jobs_show_table(
    value: &serde_json::Value,
    layer: &mut DisplayJobsShowLayer,
) -> Result<(), Box<dyn std::error::Error>> {
    let table = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(()),
    };

    if let Some(format) = parse_nonempty_string(table.get("format")) {
        if let Some(parsed) = ListFormat::parse(&format) {
            layer.format = Some(parsed);
        } else {
            display::warn(format!(
                "unknown display.lists.jobs_show.format value `{}`; ignoring",
                format
            ));
        }
    }

    if let Some(fields) = parse_string_array_allow_empty(table.get("fields")) {
        layer.fields = Some(fields);
    }

    if let Some(labels) = parse_string_map(table.get("labels")) {
        layer.labels = Some(labels);
    }

    Ok(())
}

fn parse_jobs_table(
    value: &serde_json::Value,
    layer: &mut JobsLayer,
) -> Result<(), Box<dyn std::error::Error>> {
    let table = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(()),
    };

    if let Some(cancel_table) = table.get("cancel")
        && let Some(cleanup_worktree) = parse_bool(
            cancel_table
                .get("cleanup_worktree")
                .or_else(|| cancel_table.get("cleanup-worktree")),
        )
    {
        layer.cancel.cleanup_worktree = Some(cleanup_worktree);
    }

    Ok(())
}

fn parse_workflow_table(
    value: &serde_json::Value,
    layer: &mut WorkflowLayer,
) -> Result<(), Box<dyn std::error::Error>> {
    let table = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(()),
    };

    if let Some(no_commit_default) = parse_bool(
        table
            .get("no_commit_default")
            .or_else(|| table.get("no-commit-default")),
    ) {
        layer.no_commit_default = Some(no_commit_default);
    }

    if let Some(background) = table.get("background").and_then(|value| value.as_object()) {
        if let Some(enabled) = parse_bool(background.get("enabled")) {
            layer.background.enabled = Some(enabled);
        }
        if let Some(quiet) = parse_bool(background.get("quiet")) {
            layer.background.quiet = Some(quiet);
        }
    }

    Ok(())
}

/// Returns the repo-local config path if `.vizier/config.toml` or `.vizier/config.json` exists.
///
/// Canonical search order (highest precedence first):
/// 1. CLI `--config-file` flag (handled in the CLI entrypoint)
/// 2. Repo-local `.vizier/config.toml` (falling back to `.vizier/config.json`)
/// 3. Global config under `$XDG_CONFIG_HOME`/platform default (`~/.config/vizier/config.toml`)
/// 4. `VIZIER_CONFIG_FILE` environment variable (lowest precedence)
pub fn project_config_path(project_root: &Path) -> Option<PathBuf> {
    let vizier_dir = project_root.join(".vizier");
    let toml_path = vizier_dir.join("config.toml");
    if toml_path.is_file() {
        return Some(toml_path);
    }
    let json_path = vizier_dir.join("config.json");
    if json_path.is_file() {
        Some(json_path)
    } else {
        None
    }
}

/// Returns the user-global config path (`~/.config/vizier/config.toml` on Unix).
pub fn global_config_path() -> Option<PathBuf> {
    let base_dir = base_config_dir()?;
    Some(base_dir.join("vizier").join("config.toml"))
}

/// Returns the config path provided via `VIZIER_CONFIG_FILE`, ignoring blank values.
pub fn env_config_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("VIZIER_CONFIG_FILE") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    None
}

pub fn base_config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("VIZIER_CONFIG_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    if let Ok(dir) = std::env::var("APPDATA") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed).join(".config"));
        }
    }

    if let Ok(profile) = std::env::var("USERPROFILE") {
        let trimmed = profile.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed).join("AppData").join("Roaming"));
        }
    }

    None
}

fn default_config_with_repo_prompts() -> Config {
    let mut config = Config::default();
    attach_repo_prompts(&mut config);
    config
}

fn attach_repo_prompts(config: &mut Config) {
    if !config.repo_prompts().is_empty() {
        return;
    }
    let prompts = load_repo_prompts();
    if !prompts.is_empty() {
        config.set_repo_prompts(prompts);
    }
}

fn load_repo_prompts() -> HashMap<SystemPrompt, PromptTemplate> {
    let prompt_directory = tools::try_get_vizier_dir().map(PathBuf::from);
    let mut repo_prompts = HashMap::new();

    if let Some(dir) = prompt_directory.as_ref() {
        for kind in PromptKind::all().iter().copied() {
            for filename in kind.filename_candidates() {
                let path = dir.join(filename);
                if let Ok(contents) = std::fs::read_to_string(&path) {
                    repo_prompts.insert(kind, PromptTemplate { path, contents });
                    break;
                }
            }
        }
    }

    repo_prompts
}

pub fn set_config(new_config: Config) {
    let mut config = new_config;
    attach_repo_prompts(&mut config);
    *CONFIG.write().unwrap() = config;
}

pub fn get_config() -> Config {
    CONFIG.read().unwrap().clone()
}

pub fn get_system_prompt_with_meta(
    scope: CommandScope,
    prompt_kind: Option<SystemPrompt>,
) -> Result<String, Box<dyn std::error::Error>> {
    let cfg = get_config();
    let mut prompt = if let Some(kind) = prompt_kind {
        cfg.prompt_for(scope, kind).text
    } else {
        cfg.prompt_for(scope, SystemPrompt::Documentation).text
    };

    prompt.push_str("<meta>");

    let file_tree = tree::build_tree()?;

    prompt.push_str(&format!(
        "<fileTree>{}</fileTree>",
        tree::tree_to_string(&file_tree, "")
    ));

    prompt.push_str(&format!(
        "<narrativeDocs>{}</narrativeDocs>",
        tools::list_narrative_docs()
    ));

    prompt.push_str(&format!(
        "<currentWorkingDirectory>{}</currentWorkingDirectory>",
        std::env::current_dir().unwrap().to_str().unwrap()
    ));

    prompt.push_str("</meta>");

    Ok(prompt)
}
