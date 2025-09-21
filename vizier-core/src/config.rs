use lazy_static::lazy_static;
use std::sync::RwLock;

use crate::{COMMIT_PROMPT, EDITOR_PROMPT, SYSTEM_PROMPT_BASE, tools, tree};

lazy_static! {
    static ref CONFIG: RwLock<Config> = RwLock::new(Config::default());
}

#[derive(Clone)]
pub struct Config {
    pub provider: wire::api::API,
    pub commit_confirmation: bool,
}

impl Config {
    pub fn default() -> Self {
        Self {
            provider: wire::api::API::OpenAI(wire::api::OpenAIModel::GPT5),
            commit_confirmation: false,
        }
    }
}

pub fn set_config(new_config: Config) {
    *CONFIG.write().unwrap() = new_config;
}

pub fn get_config() -> Config {
    CONFIG.read().unwrap().clone()
}

// TODO: There's probably a much better way of organizing this

pub fn get_base_system_prompt() -> String {
    let prompt_directory = std::path::PathBuf::from(tools::get_todo_dir());

    match std::fs::read_to_string(prompt_directory.join("BASE_SYSTEM_PROMPT.md")) {
        Ok(s) => s,
        Err(_) => SYSTEM_PROMPT_BASE.to_string(),
    }
}

pub fn get_editor_prompt() -> String {
    let prompt_directory = std::path::PathBuf::from(tools::get_todo_dir());

    match std::fs::read_to_string(prompt_directory.join("EDITOR_PROMPT.md")) {
        Ok(s) => s,
        Err(_) => EDITOR_PROMPT.to_string(),
    }
}

pub fn get_commit_prompt() -> String {
    let prompt_directory = std::path::PathBuf::from(tools::get_todo_dir());

    match std::fs::read_to_string(prompt_directory.join("COMMIT_PROMPT.md")) {
        Ok(s) => s,
        Err(_) => COMMIT_PROMPT.to_string(),
    }
}

pub fn get_system_prompt_with_meta() -> Result<String, Box<dyn std::error::Error>> {
    let mut prompt = get_base_system_prompt();

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
