use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use lazy_static::lazy_static;
use wire::{
    api::Prompt,
    config::{ClientOptions, ThinkingLevel},
    new_client_with_options, openai,
};

use crate::{CHAT_PROMPT, COMMIT_PROMPT, EDITOR_PROMPT, SYSTEM_PROMPT_BASE, tools, tree};

pub const DEFAULT_MODEL: &str = "gpt-5";

lazy_static! {
    static ref CONFIG: RwLock<Config> = RwLock::new(Config::default());
}

#[derive(Clone, PartialEq, Eq, Hash, serde::Deserialize)]
pub enum SystemPrompt {
    Base,
    Editor,
    Commit,
    Chat,
}

#[derive(Clone)]
pub struct Config {
    pub provider: Arc<dyn Prompt>,
    pub provider_model: String,
    pub reasoning_effort: Option<ThinkingLevel>,
    pub commit_confirmation: bool,
    pub no_session: bool,
    prompt_store: std::collections::HashMap<SystemPrompt, String>,
}

impl Config {
    pub fn default() -> Self {
        let prompt_directory = std::path::PathBuf::from(tools::get_todo_dir());

        Self {
            provider: Arc::new(openai::OpenAIClient::new(DEFAULT_MODEL)),
            provider_model: DEFAULT_MODEL.to_owned(),
            reasoning_effort: None,
            commit_confirmation: false,
            no_session: false,
            prompt_store: std::collections::HashMap::from([
                (
                    SystemPrompt::Base,
                    match std::fs::read_to_string(prompt_directory.join("BASE_SYSTEM_PROMPT.md")) {
                        Ok(s) => s,
                        Err(_) => SYSTEM_PROMPT_BASE.to_string(),
                    },
                ),
                (
                    SystemPrompt::Editor,
                    match std::fs::read_to_string(prompt_directory.join("EDITOR_PROMPT.md")) {
                        Ok(s) => s,
                        Err(_) => EDITOR_PROMPT.to_string(),
                    },
                ),
                (
                    SystemPrompt::Commit,
                    match std::fs::read_to_string(prompt_directory.join("COMMIT_PROMPT.md")) {
                        Ok(s) => s,
                        Err(_) => COMMIT_PROMPT.to_string(),
                    },
                ),
                (
                    SystemPrompt::Chat,
                    match std::fs::read_to_string(prompt_directory.join("CHAT_PROMPT.md")) {
                        Ok(s) => s,
                        Err(_) => CHAT_PROMPT.to_string(),
                    },
                ),
            ]),
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

        if let Some(commit_confirmation) = find_bool(&file_config, COMMIT_CONFIRMATION_KEY_PATHS) {
            config.commit_confirmation = commit_confirmation;
        }

        if let Some(prompt) = find_string(&file_config, BASE_PROMPT_KEY_PATHS) {
            config.prompt_store.insert(SystemPrompt::Base, prompt);
        }

        if let Some(prompt) = find_string(&file_config, EDITOR_PROMPT_KEY_PATHS) {
            config.prompt_store.insert(SystemPrompt::Editor, prompt);
        }

        if let Some(prompt) = find_string(&file_config, COMMIT_PROMPT_KEY_PATHS) {
            config.prompt_store.insert(SystemPrompt::Commit, prompt);
        }

        if let Some(prompt) = find_string(&file_config, CHAT_PROMPT_KEY_PATHS) {
            config.prompt_store.insert(SystemPrompt::Chat, prompt);
        }

        Ok(config)
    }

    pub fn get_prompt(&self, prompt: SystemPrompt) -> String {
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
const COMMIT_CONFIRMATION_KEY_PATHS: &[&[&str]] = &[
    &["commit_confirmation"],
    &["require_confirmation"],
    &["prompts", "commit_confirmation"],
    &["flags", "require_confirmation"],
];
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
const EDITOR_PROMPT_KEY_PATHS: &[&[&str]] = &[
    &["EDITOR_PROMPT"],
    &["editor_prompt"],
    &["prompts", "EDITOR_PROMPT"],
    &["prompts", "editor"],
    &["prompts", "editor_prompt"],
];
const COMMIT_PROMPT_KEY_PATHS: &[&[&str]] = &[
    &["COMMIT_PROMPT"],
    &["commit_prompt"],
    &["prompts", "COMMIT_PROMPT"],
    &["prompts", "commit"],
    &["prompts", "commit_prompt"],
];
const CHAT_PROMPT_KEY_PATHS: &[&[&str]] = &[
    &["CHAT_PROMPT"],
    &["chat_prompt"],
    &["prompts", "CHAT_PROMPT"],
    &["prompts", "chat"],
    &["prompts", "chat_prompt"],
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

fn find_bool(value: &serde_json::Value, paths: &[&[&str]]) -> Option<bool> {
    for path in paths {
        if let Some(serde_json::Value::Bool(b)) = value_at_path(value, path) {
            return Some(*b);
        }
    }

    None
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
            "EDITOR_PROMPT": "editor override",
            "COMMIT_PROMPT": "commit override"
        }
        "#;
        let file = write_json_file(json);

        let cfg = Config::from_json(file.path().to_path_buf()).expect("should parse JSON config");

        assert_eq!(cfg.get_prompt(SystemPrompt::Base), "base override");
        assert_eq!(cfg.get_prompt(SystemPrompt::Editor), "editor override");
        assert_eq!(cfg.get_prompt(SystemPrompt::Commit), "commit override");
    }

    #[test]
    fn test_from_json_partial_override() {
        let json = r#"{ "EDITOR_PROMPT": "only editor override" }"#;
        let file = write_json_file(json);

        let cfg = Config::from_json(file.path().to_path_buf()).expect("should parse JSON config");

        let default_cfg = Config::default();

        assert_eq!(cfg.get_prompt(SystemPrompt::Editor), "only editor override");
        assert_eq!(
            cfg.get_prompt(SystemPrompt::Base),
            default_cfg.get_prompt(SystemPrompt::Base)
        );
        assert_eq!(
            cfg.get_prompt(SystemPrompt::Commit),
            default_cfg.get_prompt(SystemPrompt::Commit)
        );
    }

    #[test]
    fn test_from_toml_prompts_table() {
        let toml = r#"
model = "gpt-5"

[prompts]
base = "toml base override"
editor = "toml editor override"
commit = "toml commit override"
"#;

        let mut file = NamedTempFile::new().expect("failed to create temp toml file");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let cfg = Config::from_toml(file.path().to_path_buf()).expect("should parse TOML config");

        assert_eq!(cfg.get_prompt(SystemPrompt::Base), "toml base override");
        assert_eq!(cfg.get_prompt(SystemPrompt::Editor), "toml editor override");
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
}
