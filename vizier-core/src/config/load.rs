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

impl ConfigLayer {
    pub fn from_json(filepath: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        Self::from_reader(filepath.as_path(), FileFormat::Json)
    }

    pub fn from_toml(filepath: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        Self::from_reader(filepath.as_path(), FileFormat::Toml)
    }

    pub fn from_path<P: AsRef<Path>>(filepath: P) -> Result<Self, Box<dyn std::error::Error>> {
        let path = filepath.as_ref();

        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase());

        match ext.as_deref() {
            Some("json") => Self::from_reader(path, FileFormat::Json),
            Some("toml") => Self::from_reader(path, FileFormat::Toml),
            _ => Self::from_reader(path, FileFormat::Toml)
                .or_else(|_| Self::from_reader(path, FileFormat::Json)),
        }
    }

    fn from_reader(path: &Path, format: FileFormat) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)?;
        let base_dir = path.parent();
        Self::from_str(&contents, format, base_dir)
    }

    fn from_str(
        contents: &str,
        format: FileFormat,
        base_dir: Option<&Path>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let file_config: serde_json::Value = match format {
            FileFormat::Json => serde_json::from_str(contents)?,
            FileFormat::Toml => toml::from_str(contents)?,
        };

        Self::from_value(file_config, base_dir)
    }

    fn from_value(
        file_config: serde_json::Value,
        base_dir: Option<&Path>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
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

        if let Some(stop_condition) = value_at_path(&file_config, &["approve", "stop_condition"])
            && let Some(object) = stop_condition.as_object()
        {
            if let Some(script_value) = object
                .get("script")
                .and_then(|value| value.as_str().map(|s| s.trim()).filter(|s| !s.is_empty()))
            {
                layer.approve.stop_condition.script = Some(PathBuf::from(script_value));
            }

            if let Some(retries) = parse_u32(object.get("retries"))
                .or_else(|| parse_u32(object.get("max_retries")))
                .or_else(|| parse_u32(object.get("max_attempts")))
            {
                layer.approve.stop_condition.retries = Some(retries);
            }
        }

        if let Some(cicd_gate) = value_at_path(&file_config, &["merge", "cicd_gate"])
            && let Some(gate_object) = cicd_gate.as_object()
        {
            if let Some(script_value) = gate_object
                .get("script")
                .and_then(|value| value.as_str().map(|s| s.trim()).filter(|s| !s.is_empty()))
            {
                layer.merge.cicd_gate.script = Some(PathBuf::from(script_value));
            }

            if let Some(auto_value) = parse_bool(gate_object.get("auto_resolve")) {
                layer.merge.cicd_gate.auto_resolve = Some(auto_value);
            } else if let Some(auto_value) = parse_bool(gate_object.get("auto-fix")) {
                layer.merge.cicd_gate.auto_resolve = Some(auto_value);
            }

            if let Some(retries) = parse_u32(gate_object.get("retries")) {
                layer.merge.cicd_gate.retries = Some(retries);
            } else if let Some(retries) = parse_u32(gate_object.get("max_retries")) {
                layer.merge.cicd_gate.retries = Some(retries);
            } else if let Some(retries) = parse_u32(gate_object.get("max_attempts")) {
                layer.merge.cicd_gate.retries = Some(retries);
            }
        }

        if let Some(merge_table) = value_at_path(&file_config, &["merge"])
            && let Some(table) = merge_table.as_object()
        {
            if let Some(queue_value) = table.get("queue") {
                if let Some(queue_table) = queue_value.as_object() {
                    if let Some(enabled) = parse_bool(queue_table.get("enabled")) {
                        layer.merge.queue.enabled = Some(enabled);
                    }
                } else if let Some(enabled) = parse_bool(Some(queue_value)) {
                    layer.merge.queue.enabled = Some(enabled);
                }
            }

            if let Some(squash) = parse_bool(
                table
                    .get("squash")
                    .or_else(|| table.get("squash_default"))
                    .or_else(|| table.get("squash-default")),
            ) {
                layer.merge.squash_default = Some(squash);
            }

            if let Some(mainline) = parse_u32(
                table
                    .get("squash_mainline")
                    .or_else(|| table.get("squash-mainline")),
            ) {
                layer.merge.squash_mainline = Some(mainline);
            }

            if let Some(conflicts_value) = table.get("conflicts")
                && let Some(conflicts) = conflicts_value.as_object()
            {
                let auto_resolve = conflicts
                    .get("auto_resolve")
                    .or_else(|| conflicts.get("auto-resolve"))
                    .or_else(|| conflicts.get("auto_resolve_conflicts"))
                    .or_else(|| conflicts.get("auto-resolve-conflicts"))
                    .and_then(|value| parse_bool(Some(value)));
                if let Some(auto) = auto_resolve {
                    layer.merge.conflicts.auto_resolve = Some(auto);
                }
            }
        }

        if let Some(jobs_value) = value_at_path(&file_config, &["jobs"])
            && let Some(jobs_table) = jobs_value.as_object()
            && let Some(cancel_value) = jobs_table.get("cancel")
            && let Some(cancel_table) = cancel_value.as_object()
            && let Some(cleanup) = parse_bool(
                cancel_table
                    .get("cleanup_worktree")
                    .or_else(|| cancel_table.get("cleanup-worktree"))
                    .or_else(|| cancel_table.get("cleanup")),
            )
        {
            layer.jobs.cancel.cleanup_worktree = Some(cleanup);
        }

        if let Some(workflow_value) = value_at_path(&file_config, &["workflow"])
            && let Some(workflow_table) = workflow_value.as_object()
        {
            if let Some(no_commit) = parse_bool(workflow_table.get("no_commit_default"))
                .or_else(|| parse_bool(workflow_table.get("no-commit-default")))
            {
                layer.workflow.no_commit_default = Some(no_commit);
            }

            if let Some(background_value) = workflow_table.get("background")
                && let Some(background_table) = background_value.as_object()
            {
                if let Some(enabled) = parse_bool(
                    background_table
                        .get("enabled")
                        .or_else(|| background_table.get("allow")),
                ) {
                    layer.workflow.background.enabled = Some(enabled);
                }

                if let Some(quiet) = parse_bool(
                    background_table
                        .get("quiet")
                        .or_else(|| background_table.get("silent")),
                ) {
                    layer.workflow.background.quiet = Some(quiet);
                }
            }
        }

        if let Some(agent_value) = value_at_path(&file_config, &["agents"]) {
            parse_agent_sections_into_layer(&mut layer, agent_value, base_dir)?;
        }

        Ok(layer)
    }
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

pub fn set_config(new_config: Config) {
    *CONFIG.write().unwrap() = new_config;
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
