use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use lazy_static::lazy_static;
use wire::{
    api::Prompt,
    config::{ClientOptions, ThinkingLevel},
    new_client_with_options, openai,
};

use crate::{COMMIT_PROMPT, SYSTEM_PROMPT_BASE, tools, tree};

pub const DEFAULT_MODEL: &str = "gpt-5";

lazy_static! {
    static ref CONFIG: RwLock<Config> = RwLock::new(Config::default());
}

#[derive(Clone, PartialEq, Eq, Hash, serde::Deserialize)]
pub enum SystemPrompt {
    Base,
    Commit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    Codex,
    Wire,
}

impl BackendKind {
    pub fn from_str(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "codex" => Some(Self::Codex),
            "wire" => Some(Self::Wire),
            _ => None,
        }
    }
}

impl std::fmt::Display for BackendKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendKind::Codex => write!(f, "codex"),
            BackendKind::Wire => write!(f, "wire"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandScope {
    Ask,
    Save,
    Draft,
    Approve,
    Review,
    Merge,
}

impl CommandScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            CommandScope::Ask => "ask",
            CommandScope::Save => "save",
            CommandScope::Draft => "draft",
            CommandScope::Approve => "approve",
            CommandScope::Review => "review",
            CommandScope::Merge => "merge",
        }
    }

    pub fn all() -> &'static [CommandScope] {
        &[
            CommandScope::Ask,
            CommandScope::Save,
            CommandScope::Draft,
            CommandScope::Approve,
            CommandScope::Review,
            CommandScope::Merge,
        ]
    }
}

impl std::str::FromStr for CommandScope {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "ask" => Ok(CommandScope::Ask),
            "save" => Ok(CommandScope::Save),
            "draft" => Ok(CommandScope::Draft),
            "approve" => Ok(CommandScope::Approve),
            "review" => Ok(CommandScope::Review),
            "merge" => Ok(CommandScope::Merge),
            other => Err(format!("unknown command scope `{other}`")),
        }
    }
}

impl std::fmt::Display for CommandScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodexOverride {
    pub binary_path: Option<PathBuf>,
    pub profile: Option<Option<String>>,
    pub bounds_prompt_path: Option<PathBuf>,
    pub extra_args: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentOverrides {
    pub backend: Option<BackendKind>,
    pub fallback_backend: Option<BackendKind>,
    pub model: Option<String>,
    pub reasoning_effort: Option<ThinkingLevel>,
    pub codex: Option<CodexOverride>,
}

impl AgentOverrides {
    pub fn is_empty(&self) -> bool {
        self.backend.is_none()
            && self.fallback_backend.is_none()
            && self.model.is_none()
            && self.reasoning_effort.is_none()
            && self.codex.is_none()
    }
}

#[derive(Clone)]
pub struct AgentSettings {
    pub scope: CommandScope,
    pub backend: BackendKind,
    pub fallback_backend: Option<BackendKind>,
    pub provider: Arc<dyn Prompt>,
    pub provider_model: String,
    pub reasoning_effort: Option<ThinkingLevel>,
    pub codex: CodexOptions,
}

#[derive(Clone, Debug)]
pub struct CodexOptions {
    pub binary_path: PathBuf,
    pub profile: Option<String>,
    pub bounds_prompt_path: Option<PathBuf>,
    pub extra_args: Vec<String>,
}

impl Default for CodexOptions {
    fn default() -> Self {
        Self {
            binary_path: PathBuf::from("codex"),
            profile: None,
            bounds_prompt_path: None,
            extra_args: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct Config {
    pub provider: Arc<dyn Prompt>,
    pub provider_model: String,
    pub reasoning_effort: Option<ThinkingLevel>,
    pub no_session: bool,
    pub backend: BackendKind,
    pub fallback_backend: Option<BackendKind>,
    pub codex: CodexOptions,
    pub review: ReviewConfig,
    pub agent_defaults: AgentOverrides,
    pub agent_scopes: HashMap<CommandScope, AgentOverrides>,
    prompt_store: std::collections::HashMap<SystemPrompt, String>,
}

#[derive(Clone)]
pub struct ReviewConfig {
    pub checks: ReviewChecksConfig,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            checks: ReviewChecksConfig::default(),
        }
    }
}

#[derive(Clone)]
pub struct ReviewChecksConfig {
    pub commands: Vec<String>,
}

impl Default for ReviewChecksConfig {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
        }
    }
}

impl Config {
    pub fn default() -> Self {
        let prompt_directory = tools::try_get_todo_dir().map(std::path::PathBuf::from);

        let mut prompt_store = std::collections::HashMap::new();
        let load_prompt = |filename: &str, fallback: &str| {
            prompt_directory
                .as_ref()
                .and_then(|dir| std::fs::read_to_string(dir.join(filename)).ok())
                .unwrap_or_else(|| fallback.to_string())
        };

        prompt_store.insert(
            SystemPrompt::Base,
            load_prompt("BASE_SYSTEM_PROMPT.md", SYSTEM_PROMPT_BASE),
        );
        prompt_store.insert(
            SystemPrompt::Commit,
            load_prompt("COMMIT_PROMPT.md", COMMIT_PROMPT),
        );

        Self {
            provider: Arc::new(openai::OpenAIClient::new(DEFAULT_MODEL)),
            provider_model: DEFAULT_MODEL.to_owned(),
            reasoning_effort: None,
            no_session: false,
            backend: BackendKind::Codex,
            fallback_backend: Some(BackendKind::Wire),
            codex: CodexOptions::default(),
            review: ReviewConfig::default(),
            agent_defaults: AgentOverrides::default(),
            agent_scopes: HashMap::new(),
            prompt_store,
        }
    }

    pub fn provider_from_settings(
        model: &str,
        reasoning_effort: Option<ThinkingLevel>,
    ) -> Result<Arc<dyn Prompt>, Box<dyn std::error::Error>> {
        let mut options = ClientOptions::default();
        if let Some(level) = reasoning_effort {
            options = options.with_thinking_level(level);
        }

        Ok(Arc::from(new_client_with_options(model, options)?))
    }

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
        Self::from_str(&contents, format)
    }

    fn from_str(contents: &str, format: FileFormat) -> Result<Self, Box<dyn std::error::Error>> {
        let file_config: serde_json::Value = match format {
            FileFormat::Json => serde_json::from_str(contents)?,
            FileFormat::Toml => toml::from_str(contents)?,
        };

        Self::from_value(file_config)
    }

    fn from_value(file_config: serde_json::Value) -> Result<Self, Box<dyn std::error::Error>> {
        let mut config = Self::default();

        if let Some(model) = find_string(&file_config, MODEL_KEY_PATHS) {
            let model = model.trim();
            if !model.is_empty() {
                config.provider_model = model.to_owned();
            }
        }

        if let Some(level) = find_string(&file_config, REASONING_EFFORT_KEY_PATHS) {
            let level = level.trim();
            if !level.is_empty() {
                config.reasoning_effort = Some(ThinkingLevel::from_string(level)?);
            }
        }

        if let Some(backend) = find_string(&file_config, BACKEND_KEY_PATHS)
            .and_then(|value| BackendKind::from_str(value.trim()))
        {
            config.backend = backend;
        }

        if let Some(fallback) = find_string(&file_config, FALLBACK_BACKEND_KEY_PATHS)
            .and_then(|value| BackendKind::from_str(value.trim()))
        {
            config.fallback_backend = Some(fallback);
        }

        if let Some(codex_value) = value_at_path(&file_config, &["codex"]) {
            if let Some(codex_object) = codex_value.as_object() {
                if let Some(path_val) = codex_object
                    .get("binary")
                    .or_else(|| codex_object.get("binary_path"))
                {
                    if let Some(path) = path_val
                        .as_str()
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                    {
                        config.codex.binary_path = PathBuf::from(path);
                    }
                }

                if let Some(profile_val) = codex_object.get("profile") {
                    if profile_val.is_null() {
                        config.codex.profile = None;
                    } else if let Some(profile) = profile_val.as_str() {
                        let trimmed = profile.trim();
                        config.codex.profile = if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed.to_string())
                        };
                    }
                }

                if let Some(bounds_val) = codex_object
                    .get("bounds_prompt_path")
                    .or_else(|| codex_object.get("bounds_prompt"))
                {
                    if let Some(path) = bounds_val
                        .as_str()
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                    {
                        config.codex.bounds_prompt_path = Some(PathBuf::from(path));
                    }
                }

                if let Some(extra_val) = codex_object.get("extra_args") {
                    if let Some(array) = extra_val.as_array() {
                        let mut args = Vec::new();
                        for item in array {
                            if let Some(arg) = item.as_str() {
                                let trimmed = arg.trim();
                                if !trimmed.is_empty() {
                                    args.push(trimmed.to_string());
                                }
                            }
                        }
                        if !args.is_empty() {
                            config.codex.extra_args = args;
                        }
                    }
                }
            }
        }

        if let Some(commands) = parse_string_array(value_at_path(
            &file_config,
            &["review", "checks", "commands"],
        )) {
            config.review.checks.commands = commands;
        } else if let Some(commands) =
            parse_string_array(value_at_path(&file_config, &["review", "checks"]))
        {
            config.review.checks.commands = commands;
        }

        if let Some(prompt) = find_string(&file_config, BASE_PROMPT_KEY_PATHS) {
            config.prompt_store.insert(SystemPrompt::Base, prompt);
        }

        if let Some(prompt) = find_string(&file_config, COMMIT_PROMPT_KEY_PATHS) {
            config.prompt_store.insert(SystemPrompt::Commit, prompt);
        }

        if let Some(agent_value) = value_at_path(&file_config, &["agents"]) {
            config.parse_agent_sections(agent_value)?;
        }

        Ok(config)
    }

    fn parse_agent_sections(
        &mut self,
        agents_value: &serde_json::Value,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let table = agents_value.as_object().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "[agents] must be a table")
        })?;

        for (key, value) in table.iter() {
            let Some(overrides) = parse_agent_overrides(value)? else {
                continue;
            };

            if key.eq_ignore_ascii_case("default") {
                self.agent_defaults = overrides;
                continue;
            }

            let scope = key.parse::<CommandScope>().map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown [agents.{key}] section: {err}"),
                )
            })?;
            self.agent_scopes.insert(scope, overrides);
        }

        Ok(())
    }

    pub fn get_prompt(&self, prompt: SystemPrompt) -> String {
        self.prompt_store.get(&prompt).unwrap().to_string()
    }

    pub fn resolve_agent_settings(
        &self,
        scope: CommandScope,
        cli_override: Option<&AgentOverrides>,
    ) -> Result<AgentSettings, Box<dyn std::error::Error>> {
        let mut builder = AgentSettingsBuilder::new(self);

        if !self.agent_defaults.is_empty() {
            builder.apply(&self.agent_defaults);
        }

        if let Some(overrides) = self.agent_scopes.get(&scope) {
            builder.apply(overrides);
        }

        if let Some(overrides) = cli_override {
            if !overrides.is_empty() {
                builder.apply(overrides);
            }
        }

        let provider = if builder.provider_model == self.provider_model
            && builder.reasoning_effort == self.reasoning_effort
        {
            self.provider.clone()
        } else {
            Self::provider_from_settings(&builder.provider_model, builder.reasoning_effort)?
        };

        Ok(AgentSettings {
            scope,
            backend: builder.backend,
            fallback_backend: builder.fallback_backend,
            provider,
            provider_model: builder.provider_model,
            reasoning_effort: builder.reasoning_effort,
            codex: builder.codex,
        })
    }
}

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
const BASE_PROMPT_KEY_PATHS: &[&[&str]] = &[
    &["BASE_SYSTEM_PROMPT"],
    &["base_system_prompt"],
    &["prompts", "BASE_SYSTEM_PROMPT"],
    &["prompts", "base"],
    &["prompts", "base_system_prompt"],
];
const COMMIT_PROMPT_KEY_PATHS: &[&[&str]] = &[
    &["COMMIT_PROMPT"],
    &["commit_prompt"],
    &["prompts", "COMMIT_PROMPT"],
    &["prompts", "commit"],
    &["prompts", "commit_prompt"],
];

#[derive(Clone)]
struct AgentSettingsBuilder {
    backend: BackendKind,
    fallback_backend: Option<BackendKind>,
    provider_model: String,
    reasoning_effort: Option<ThinkingLevel>,
    codex: CodexOptions,
}

impl AgentSettingsBuilder {
    fn new(cfg: &Config) -> Self {
        Self {
            backend: cfg.backend,
            fallback_backend: cfg.fallback_backend,
            provider_model: cfg.provider_model.clone(),
            reasoning_effort: cfg.reasoning_effort,
            codex: cfg.codex.clone(),
        }
    }

    fn apply(&mut self, overrides: &AgentOverrides) {
        if let Some(backend) = overrides.backend {
            self.backend = backend;
        }

        if let Some(fallback) = overrides.fallback_backend {
            self.fallback_backend = Some(fallback);
        }

        if let Some(model) = overrides.model.as_ref() {
            self.provider_model = model.clone();
        }

        if let Some(level) = overrides.reasoning_effort {
            self.reasoning_effort = Some(level);
        }

        if let Some(codex) = overrides.codex.as_ref() {
            if let Some(path) = codex.binary_path.as_ref() {
                self.codex.binary_path = path.clone();
            }

            if let Some(profile) = codex.profile.as_ref() {
                self.codex.profile = profile.clone();
            }

            if let Some(bounds) = codex.bounds_prompt_path.as_ref() {
                self.codex.bounds_prompt_path = Some(bounds.clone());
            }

            if let Some(extra) = codex.extra_args.as_ref() {
                self.codex.extra_args = extra.clone();
            }
        }
    }
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
        if let Some(serde_json::Value::String(s)) = value_at_path(value, path) {
            if !s.is_empty() {
                return Some(s.clone());
            }
        }
    }

    None
}

fn parse_agent_overrides(
    value: &serde_json::Value,
) -> Result<Option<AgentOverrides>, Box<dyn std::error::Error>> {
    if !value.is_object() {
        return Ok(None);
    }

    let mut overrides = AgentOverrides::default();

    if let Some(backend) =
        find_string(value, BACKEND_KEY_PATHS).and_then(|text| BackendKind::from_str(text.trim()))
    {
        overrides.backend = Some(backend);
    }

    if let Some(fallback) = find_string(value, FALLBACK_BACKEND_KEY_PATHS)
        .and_then(|text| BackendKind::from_str(text.trim()))
    {
        overrides.fallback_backend = Some(fallback);
    }

    if let Some(model) = find_string(value, MODEL_KEY_PATHS) {
        let trimmed = model.trim();
        if !trimmed.is_empty() {
            overrides.model = Some(trimmed.to_string());
        }
    }

    if let Some(level) = find_string(value, REASONING_EFFORT_KEY_PATHS) {
        let trimmed = level.trim();
        if !trimmed.is_empty() {
            overrides.reasoning_effort = Some(ThinkingLevel::from_string(trimmed)?);
        }
    }

    if let Some(codex_value) = value_at_path(value, &["codex"]) {
        if let Some(parsed) = parse_codex_override(codex_value)? {
            overrides.codex = Some(parsed);
        }
    }

    if overrides.is_empty() {
        Ok(None)
    } else {
        Ok(Some(overrides))
    }
}

fn parse_codex_override(
    value: &serde_json::Value,
) -> Result<Option<CodexOverride>, Box<dyn std::error::Error>> {
    let object = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(None),
    };

    let mut overrides = CodexOverride::default();

    if let Some(path_val) = object.get("binary").or_else(|| object.get("binary_path")) {
        if let Some(path) = path_val
            .as_str()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            overrides.binary_path = Some(PathBuf::from(path));
        }
    }

    if let Some(profile_val) = object.get("profile") {
        if profile_val.is_null() {
            overrides.profile = Some(None);
        } else if let Some(profile) = profile_val.as_str() {
            let trimmed = profile.trim();
            overrides.profile = Some(if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            });
        }
    }

    if let Some(bounds_val) = object
        .get("bounds_prompt_path")
        .or_else(|| object.get("bounds_prompt"))
    {
        if let Some(path) = bounds_val
            .as_str()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            overrides.bounds_prompt_path = Some(PathBuf::from(path));
        }
    }

    if let Some(extra_val) = object.get("extra_args") {
        if let Some(array) = extra_val.as_array() {
            let mut args = Vec::new();
            for entry in array {
                if let Some(arg) = entry.as_str() {
                    let trimmed = arg.trim();
                    if !trimmed.is_empty() {
                        args.push(trimmed.to_string());
                    }
                }
            }
            if !args.is_empty() {
                overrides.extra_args = Some(args);
            }
        }
    }

    if overrides == CodexOverride::default() {
        Ok(None)
    } else {
        Ok(Some(overrides))
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

pub fn default_config_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("VIZIER_CONFIG_FILE") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    let base_dir = base_config_dir()?;
    Some(base_dir.join("vizier").join("config.toml"))
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
    base_prompt: Option<SystemPrompt>,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut prompt = if let Some(prompt) = base_prompt {
        get_config().get_prompt(prompt)
    } else {
        get_config().get_prompt(SystemPrompt::Base)
    };

    prompt.push_str("<meta>");

    let file_tree = tree::build_tree()?;

    prompt.push_str(&format!(
        "<fileTree>{}</fileTree>",
        tree::tree_to_string(&file_tree, "")
    ));

    prompt.push_str(&format!("<todos>{}</todos>", tools::list_todos()));

    prompt.push_str(&format!(
        "<currentWorkingDirectory>{}</currentWorkingDirectory>",
        std::env::current_dir().unwrap().to_str().unwrap()
    ));

    prompt.push_str("</meta>");

    Ok(prompt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;
    use wire::config::ThinkingLevel;

    fn write_json_file(contents: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("failed to create temp file");
        file.write_all(contents.as_bytes())
            .expect("failed to write temp file");
        file
    }

    #[test]
    fn test_from_json_overrides_prompts() {
        let json = r#"
        {
            "BASE_SYSTEM_PROMPT": "base override",
            "COMMIT_PROMPT": "commit override"
        }
        "#;
        let file = write_json_file(json);

        let cfg = Config::from_json(file.path().to_path_buf()).expect("should parse JSON config");

        assert_eq!(cfg.get_prompt(SystemPrompt::Base), "base override");
        assert_eq!(cfg.get_prompt(SystemPrompt::Commit), "commit override");
    }

    #[test]
    fn test_from_json_partial_override() {
        let json = r#"{ "COMMIT_PROMPT": "only commit override" }"#;
        let file = write_json_file(json);

        let cfg = Config::from_json(file.path().to_path_buf()).expect("should parse JSON config");

        let default_cfg = Config::default();

        assert_eq!(cfg.get_prompt(SystemPrompt::Commit), "only commit override");
        assert_eq!(
            cfg.get_prompt(SystemPrompt::Base),
            default_cfg.get_prompt(SystemPrompt::Base)
        );
    }

    #[test]
    fn test_from_toml_prompts_table() {
        let toml = r#"
model = "gpt-5"

[prompts]
base = "toml base override"
commit = "toml commit override"
"#;

        let mut file = NamedTempFile::new().expect("failed to create temp toml file");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let cfg = Config::from_toml(file.path().to_path_buf()).expect("should parse TOML config");

        assert_eq!(cfg.get_prompt(SystemPrompt::Base), "toml base override");
        assert_eq!(cfg.get_prompt(SystemPrompt::Commit), "toml commit override");
    }

    #[test]
    fn test_from_json_invalid_file() {
        let file = write_json_file("{ this is not valid json ");
        let result = Config::from_json(file.path().to_path_buf());
        assert!(result.is_err(), "expected error for invalid JSON");
    }

    #[test]
    fn test_from_json_missing_file() {
        let path = std::path::PathBuf::from("does_not_exist.json");
        let result = Config::from_json(path);
        assert!(result.is_err(), "expected error for missing file");
    }

    #[test]
    fn test_reasoning_effort_in_config_file() {
        let json = r#"{ "model": "gpt-5", "reasoning_effort": "medium" }"#;
        let file = write_json_file(json);

        let cfg =
            Config::from_json(file.path().to_path_buf()).expect("should parse reasoning effort");

        assert_eq!(cfg.provider_model, "gpt-5");
        assert_eq!(cfg.reasoning_effort, Some(ThinkingLevel::Medium));
    }

    #[test]
    fn test_reasoning_effort_without_model_uses_default() {
        let json = r#"{ "reasoning_effort": "high" }"#;
        let file = write_json_file(json);

        let cfg = Config::from_json(file.path().to_path_buf())
            .expect("should parse reasoning effort only");

        assert_eq!(cfg.provider_model, DEFAULT_MODEL);
        assert_eq!(cfg.reasoning_effort, Some(ThinkingLevel::High));
    }

    #[test]
    fn test_review_checks_table() {
        let toml = r#"
[review.checks]
commands = ["npm test", "cargo fmt -- --check"]
"#;
        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes()).unwrap();
        let cfg = Config::from_toml(file.path().to_path_buf()).expect("parse review config");
        assert_eq!(
            cfg.review.checks.commands,
            vec!["npm test", "cargo fmt -- --check"]
        );
    }
}
