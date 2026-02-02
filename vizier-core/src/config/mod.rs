include!("schema.rs");
include!("prompts.rs");
include!("defaults.rs");
include!("merge.rs");
include!("load.rs");

#[cfg(test)]
use std::sync::{Mutex, OnceLock};

#[cfg(test)]
static CONFIG_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(test)]
pub fn test_config_lock() -> &'static Mutex<()> {
    CONFIG_TEST_LOCK.get_or_init(|| Mutex::new(()))
}

include!("validate.rs");
