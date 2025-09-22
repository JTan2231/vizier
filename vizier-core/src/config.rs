use lazy_static::lazy_static;
use std::sync::RwLock;

use crate::{COMMIT_PROMPT, EDITOR_PROMPT, SYSTEM_PROMPT_BASE, tools, tree};

lazy_static! {
    static ref CONFIG: RwLock<Config> = RwLock::new(Config::default());
}

#[derive(Clone, PartialEq, Eq, Hash, serde::Deserialize)]
pub enum SystemPrompt {
    Base,
    Editor,
    Commit,
}

#[derive(Clone)]
pub struct Config {
    pub provider: wire::api::API,
    pub commit_confirmation: bool,
    prompt_store: std::collections::HashMap<SystemPrompt, String>,
}

impl Config {
    pub fn default() -> Self {
        let prompt_directory = std::path::PathBuf::from(tools::get_todo_dir());

        Self {
            provider: wire::api::API::OpenAI(wire::api::OpenAIModel::GPT5),
            commit_confirmation: false,
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
            ]),
        }
    }

    /// NOTE: Only supports prompts for now
    pub fn from_json(filepath: std::path::PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let file_config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(filepath)?)?;

        let prompt_directory = std::path::PathBuf::from(tools::get_todo_dir());

        let mut config = Self::default();
        config.prompt_store = std::collections::HashMap::from([
            (
                SystemPrompt::Base,
                if let Some(serde_json::Value::String(prompt)) =
                    file_config.get("BASE_SYSTEM_PROMPT")
                {
                    prompt.clone()
                } else {
                    match std::fs::read_to_string(prompt_directory.join("BASE_SYSTEM_PROMPT.md")) {
                        Ok(s) => s,
                        Err(_) => SYSTEM_PROMPT_BASE.to_string(),
                    }
                },
            ),
            (
                SystemPrompt::Editor,
                if let Some(serde_json::Value::String(prompt)) = file_config.get("EDITOR_PROMPT") {
                    prompt.clone()
                } else {
                    match std::fs::read_to_string(prompt_directory.join("EDITOR_PROMPT.md")) {
                        Ok(s) => s,
                        Err(_) => EDITOR_PROMPT.to_string(),
                    }
                },
            ),
            (
                SystemPrompt::Commit,
                if let Some(serde_json::Value::String(prompt)) = file_config.get("COMMIT_PROMPT") {
                    prompt.clone()
                } else {
                    match std::fs::read_to_string(prompt_directory.join("COMMIT_PROMPT.md")) {
                        Ok(s) => s,
                        Err(_) => COMMIT_PROMPT.to_string(),
                    }
                },
            ),
        ]);

        Ok(config)
    }

    pub fn get_prompt(&self, prompt: SystemPrompt) -> String {
        self.prompt_store.get(&prompt).unwrap().to_string()
    }
}

pub fn set_config(new_config: Config) {
    *CONFIG.write().unwrap() = new_config;
}

pub fn get_config() -> Config {
    CONFIG.read().unwrap().clone()
}

pub fn get_system_prompt_with_meta() -> Result<String, Box<dyn std::error::Error>> {
    let mut prompt = get_config().get_prompt(SystemPrompt::Base);

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
}
