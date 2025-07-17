use lazy_static::lazy_static;
use std::sync::RwLock;

lazy_static! {
    static ref CONFIG: RwLock<Config> = RwLock::new(Config::default());
}

#[derive(Clone)]
pub struct Config {
    pub provider: String,
}

impl Config {
    pub fn default() -> Self {
        Self {
            provider: "anthropic".to_string(),
        }
    }
}

pub fn set_config(new_config: Config) {
    *CONFIG.write().unwrap() = new_config;
}

pub fn get_config() -> Config {
    CONFIG.read().unwrap().clone()
}
