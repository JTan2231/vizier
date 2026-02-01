use std::path::PathBuf;
use std::time::Duration;

use vizier_core::config;

#[derive(Debug, Clone)]
pub struct SnapshotInitOptions {
    pub force: bool,
    pub depth: Option<usize>,
    pub paths: Vec<String>,
    pub exclude: Vec<String>,
    pub issues: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SpecSource {
    Inline,
    File(PathBuf),
    Stdin,
}

impl SpecSource {
    pub fn as_metadata_value(&self) -> String {
        match self {
            SpecSource::Inline => "inline".to_string(),
            SpecSource::File(path) => format!("file:{}", path.display()),
            SpecSource::Stdin => "stdin".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitMode {
    AutoCommit,
    HoldForReview,
}

impl CommitMode {
    pub fn should_commit(self) -> bool {
        matches!(self, CommitMode::AutoCommit)
    }

    pub fn label(self) -> &'static str {
        match self {
            CommitMode::AutoCommit => "auto",
            CommitMode::HoldForReview => "manual",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DraftArgs {
    pub spec_text: String,
    pub spec_source: SpecSource,
    pub name_override: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ListOptions {
    pub target: Option<String>,
    pub format: Option<config::ListFormat>,
    pub fields: Option<Vec<String>>,
    pub emit_json: bool,
}

#[derive(Debug, Clone)]
pub struct CdOptions {
    pub slug: String,
    pub branch: String,
    pub path_only: bool,
}

#[derive(Debug, Clone)]
pub struct CleanOptions {
    pub slug: Option<String>,
    pub assume_yes: bool,
}

#[derive(Debug, Clone)]
pub struct ApproveStopCondition {
    pub script: Option<PathBuf>,
    pub retries: u32,
}

#[derive(Debug, Clone)]
pub struct ApproveOptions {
    pub plan: String,
    pub target: Option<String>,
    pub branch_override: Option<String>,
    pub assume_yes: bool,
    pub stop_condition: ApproveStopCondition,
    pub push_after: bool,
}

#[derive(Debug, Clone)]
pub struct ReviewOptions {
    pub plan: String,
    pub target: Option<String>,
    pub branch_override: Option<String>,
    pub assume_yes: bool,
    pub review_only: bool,
    pub review_file: bool,
    pub skip_checks: bool,
    pub cicd_gate: CicdGateOptions,
    pub auto_resolve_requested: bool,
    pub push_after: bool,
}

#[derive(Debug, Clone)]
pub struct MergeOptions {
    pub plan: String,
    pub target: Option<String>,
    pub branch_override: Option<String>,
    pub assume_yes: bool,
    pub delete_branch: bool,
    pub note: Option<String>,
    pub push_after: bool,
    pub conflict_auto_resolve: ConflictAutoResolveSetting,
    pub conflict_strategy: MergeConflictStrategy,
    pub complete_conflict: bool,
    pub cicd_gate: CicdGateOptions,
    pub squash: bool,
    pub squash_mainline: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct TestDisplayOptions {
    pub scope: config::CommandScope,
    pub prompt_override: Option<String>,
    pub raw_output: bool,
    pub timeout: Option<Duration>,
    pub disable_wrapper: bool,
    pub record_session: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeConflictStrategy {
    Manual,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictAutoResolveSource {
    Default,
    Config,
    FlagEnable,
    FlagDisable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConflictAutoResolveSetting {
    enabled: bool,
    source: ConflictAutoResolveSource,
}

impl ConflictAutoResolveSetting {
    pub fn new(enabled: bool, source: ConflictAutoResolveSource) -> Self {
        Self { enabled, source }
    }

    pub fn enabled(self) -> bool {
        self.enabled
    }

    pub(crate) fn source_description(self) -> &'static str {
        match self.source {
            ConflictAutoResolveSource::Default => "default",
            ConflictAutoResolveSource::Config => "merge.conflicts.auto_resolve",
            ConflictAutoResolveSource::FlagEnable => "--auto-resolve-conflicts",
            ConflictAutoResolveSource::FlagDisable => "--no-auto-resolve-conflicts",
        }
    }

    pub(crate) fn status_line(self) -> String {
        let origin = self.source_description();
        if self.enabled() {
            format!("Conflict auto-resolution enabled via {origin}.")
        } else {
            format!(
                "Conflict auto-resolution disabled via {origin}; conflicts will require manual resolution unless overridden."
            )
        }
    }
}

#[derive(Debug, Clone)]
pub struct CicdGateOptions {
    pub script: Option<PathBuf>,
    pub auto_resolve: bool,
    pub retries: u32,
}

impl CicdGateOptions {
    pub fn from_config(config: &config::MergeCicdGateConfig) -> Self {
        Self {
            script: config.script.clone(),
            auto_resolve: config.auto_resolve,
            retries: config.retries,
        }
    }
}
