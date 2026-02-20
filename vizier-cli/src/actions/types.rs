use vizier_core::config;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitMode {
    AutoCommit,
    HoldForReview,
}

impl CommitMode {
    #[allow(dead_code)]
    pub fn should_commit(self) -> bool {
        matches!(self, CommitMode::AutoCommit)
    }

    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            CommitMode::AutoCommit => "auto",
            CommitMode::HoldForReview => "manual",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ListOptions {
    pub target: Option<String>,
    pub format: Option<config::ListFormat>,
    pub fields: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct CdOptions {
    pub slug: String,
    pub branch: String,
    pub path_only: bool,
}

#[derive(Debug, Clone)]
pub struct CleanOptions {
    pub job_id: String,
    pub assume_yes: bool,
    pub format: CleanOutputFormat,
    pub keep_branches: bool,
    pub force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanOutputFormat {
    Text,
    Json,
}
