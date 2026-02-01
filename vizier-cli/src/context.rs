use std::path::PathBuf;

use vizier_core::{display, vcs};

#[derive(Clone)]
pub struct CliContext {
    pub repo_root: PathBuf,
    pub verbosity: display::Verbosity,
}

impl CliContext {
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let repo_root =
            vcs::repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
        Ok(Self {
            repo_root,
            verbosity: display::get_display_config().verbosity,
        })
    }
}
