pub use vizier_kernel::config::*;

mod driver;
mod load;
mod validate;

pub use driver::{
    AgentSettings, resolve_agent_settings, resolve_default_agent_settings,
    resolve_default_prompt_profile, resolve_prompt_profile,
};
pub use load::{
    base_config_dir, env_config_path, get_config, get_system_prompt_with_meta, global_config_path,
    load_config_from_json, load_config_from_path, load_config_from_toml,
    load_config_layer_from_json, load_config_layer_from_path, load_config_layer_from_toml,
    project_config_path, set_config,
};

#[cfg(test)]
use std::sync::{Mutex, OnceLock};

#[cfg(test)]
static CONFIG_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(test)]
pub fn test_config_lock() -> &'static Mutex<()> {
    CONFIG_TEST_LOCK.get_or_init(|| Mutex::new(()))
}
