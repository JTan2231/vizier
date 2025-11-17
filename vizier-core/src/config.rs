use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use lazy_static::lazy_static;
use wire::{
    api::Prompt,
    config::{ClientOptions, ThinkingLevel},
    new_client_with_options, openai,
};

use crate::{
    COMMIT_PROMPT, IMPLEMENTATION_PLAN_PROMPT, MERGE_CONFLICT_PROMPT, REVIEW_PROMPT,
    SYSTEM_PROMPT_BASE, tools, tree,
};

pub const DEFAULT_MODEL: &str = "gpt-5";

lazy_static! {
    static ref CONFIG: RwLock<Config> = RwLock::new(Config::default());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Deserialize)]
pub enum PromptKind {
    Base,
    Commit,
    ImplementationPlan,
    Review,
    MergeConflict,
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
    prompt_store: std::collections::HashMap<PromptKind, String>,
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
            PromptKind::Base,
            load_prompt("BASE_SYSTEM_PROMPT.md", SYSTEM_PROMPT_BASE),
        );
        prompt_store.insert(
            PromptKind::Commit,
            load_prompt("COMMIT_PROMPT.md", COMMIT_PROMPT),
        );
        prompt_store.insert(
            PromptKind::ImplementationPlan,
            load_prompt("IMPLEMENTATION_PLAN_PROMPT.md", IMPLEMENTATION_PLAN_PROMPT),
        );
        prompt_store.insert(
            PromptKind::Review,
            load_prompt("REVIEW_PROMPT.md", REVIEW_PROMPT),
        );
        prompt_store.insert(
            PromptKind::MergeConflict,
            load_prompt("MERGE_CONFLICT_PROMPT.md", MERGE_CONFLICT_PROMPT),
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
            config.set_prompt(PromptKind::Base, prompt);
        }

        if let Some(prompt) = find_string(&file_config, COMMIT_PROMPT_KEY_PATHS) {
            config.set_prompt(PromptKind::Commit, prompt);
        }

        if let Some(prompt) = find_string(&file_config, IMPLEMENTATION_PLAN_PROMPT_KEY_PATHS) {
            config.set_prompt(PromptKind::ImplementationPlan, prompt);
        }

        if let Some(prompt) = find_string(&file_config, REVIEW_PROMPT_KEY_PATHS) {
            config.set_prompt(PromptKind::Review, prompt);
        }

        if let Some(prompt) = find_string(&file_config, MERGE_CONFLICT_PROMPT_KEY_PATHS) {
            config.set_prompt(PromptKind::MergeConflict, prompt);
        }

        Ok(config)
    }

    pub fn set_prompt<S: Into<String>>(&mut self, prompt: PromptKind, value: S) {
        self.prompt_store.insert(prompt, value.into());
    }

    pub fn get_prompt(&self, prompt: PromptKind) -> String {
        self.prompt_store.get(&prompt).unwrap().to_string()
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
const IMPLEMENTATION_PLAN_PROMPT_KEY_PATHS: &[&[&str]] = &[
    &["IMPLEMENTATION_PLAN_PROMPT"],
    &["implementation_plan_prompt"],
    &["prompts", "IMPLEMENTATION_PLAN_PROMPT"],
    &["prompts", "implementation_plan"],
    &["prompts", "implementation_plan_prompt"],
];
const REVIEW_PROMPT_KEY_PATHS: &[&[&str]] = &[
    &["REVIEW_PROMPT"],
    &["review_prompt"],
    &["prompts", "REVIEW_PROMPT"],
    &["prompts", "review"],
    &["prompts", "review_prompt"],
];
const MERGE_CONFLICT_PROMPT_KEY_PATHS: &[&[&str]] = &[
    &["MERGE_CONFLICT_PROMPT"],
    &["merge_conflict_prompt"],
    &["prompts", "MERGE_CONFLICT_PROMPT"],
    &["prompts", "merge_conflict"],
    &["prompts", "merge_conflict_prompt"],
];

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
    prompt_kind: Option<PromptKind>,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut prompt = if let Some(prompt) = prompt_kind {
        get_config().get_prompt(prompt)
    } else {
        get_config().get_prompt(PromptKind::Base)
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
            "COMMIT_PROMPT": "commit override",
            "IMPLEMENTATION_PLAN_PROMPT": "plan override",
            "REVIEW_PROMPT": "review override",
            "MERGE_CONFLICT_PROMPT": "merge override"
        }
        "#;
        let file = write_json_file(json);

        let cfg = Config::from_json(file.path().to_path_buf()).expect("should parse JSON config");

        assert_eq!(cfg.get_prompt(PromptKind::Base), "base override");
        assert_eq!(cfg.get_prompt(PromptKind::Commit), "commit override");
        assert_eq!(
            cfg.get_prompt(PromptKind::ImplementationPlan),
            "plan override"
        );
        assert_eq!(cfg.get_prompt(PromptKind::Review), "review override");
        assert_eq!(cfg.get_prompt(PromptKind::MergeConflict), "merge override");
    }

    #[test]
    fn test_from_json_partial_override() {
        let json = r#"{ "COMMIT_PROMPT": "only commit override" }"#;
        let file = write_json_file(json);

        let cfg = Config::from_json(file.path().to_path_buf()).expect("should parse JSON config");

        let default_cfg = Config::default();

        assert_eq!(cfg.get_prompt(PromptKind::Commit), "only commit override");
        assert_eq!(
            cfg.get_prompt(PromptKind::Base),
            default_cfg.get_prompt(PromptKind::Base)
        );
    }

    #[test]
    fn test_from_toml_prompts_table() {
        let toml = r#"
model = "gpt-5"

[prompts]
base = "toml base override"
commit = "toml commit override"
implementation_plan = "toml plan override"
review = "toml review override"
merge_conflict = "toml merge override"
"#;

        let mut file = NamedTempFile::new().expect("failed to create temp toml file");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let cfg = Config::from_toml(file.path().to_path_buf()).expect("should parse TOML config");

        assert_eq!(cfg.get_prompt(PromptKind::Base), "toml base override");
        assert_eq!(cfg.get_prompt(PromptKind::Commit), "toml commit override");
        assert_eq!(
            cfg.get_prompt(PromptKind::ImplementationPlan),
            "toml plan override"
        );
        assert_eq!(cfg.get_prompt(PromptKind::Review), "toml review override");
        assert_eq!(
            cfg.get_prompt(PromptKind::MergeConflict),
            "toml merge override"
        );
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
