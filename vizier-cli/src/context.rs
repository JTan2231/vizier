use std::path::PathBuf;

use vizier_core::{config, display, vcs};

#[derive(Clone)]
pub struct CliContext {
    pub repo_root: PathBuf,
    #[allow(dead_code)]
    pub config: config::Config,
    pub verbosity: display::Verbosity,
}

impl CliContext {
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let repo_root =
            vcs::repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
        Ok(Self {
            repo_root,
            config: config::get_config(),
            verbosity: display::get_display_config().verbosity,
        })
    }
}
