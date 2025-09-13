use lazy_static::lazy_static;
use std::sync::RwLock;

use crate::{SYSTEM_PROMPT_BASE, tools, tree};

lazy_static! {
    static ref CONFIG: RwLock<Config> = RwLock::new(Config::default());
}

#[derive(Clone)]
pub struct Config {
    pub provider: wire::api::API,
    pub force_action: bool,
}

impl Config {
    pub fn default() -> Self {
        Self {
            provider: wire::api::API::OpenAI(wire::api::OpenAIModel::GPT5),
            force_action: false,
        }
    }
}

pub fn set_config(new_config: Config) {
    *CONFIG.write().unwrap() = new_config;
}

pub fn get_config() -> Config {
    CONFIG.read().unwrap().clone()
}

pub fn get_system_prompt() -> Result<String, Box<dyn std::error::Error>> {
    let mut prompt = SYSTEM_PROMPT_BASE.to_string();

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
