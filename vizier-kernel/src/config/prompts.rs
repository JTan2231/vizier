use crate::prompts::{
    COMMIT_PROMPT, DOCUMENTATION_PROMPT, IMPLEMENTATION_PLAN_PROMPT, MERGE_CONFLICT_PROMPT,
    REVIEW_PROMPT,
};
use std::path::PathBuf;

use super::CommandScope;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Deserialize)]
pub enum PromptKind {
    Documentation,
    Commit,
    ImplementationPlan,
    Review,
    MergeConflict,
}

/// Alias for prompt variants that feed the system prompt builder.
pub type SystemPrompt = PromptKind;

impl PromptKind {
    pub fn all() -> &'static [PromptKind] {
        const ALL: &[PromptKind] = &[
            PromptKind::Documentation,
            PromptKind::Commit,
            PromptKind::ImplementationPlan,
            PromptKind::Review,
            PromptKind::MergeConflict,
        ];
        ALL
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            PromptKind::Documentation => "documentation",
            PromptKind::Commit => "commit",
            PromptKind::ImplementationPlan => "implementation_plan",
            PromptKind::Review => "review",
            PromptKind::MergeConflict => "merge_conflict",
        }
    }

    pub fn filename_candidates(&self) -> &'static [&'static str] {
        match self {
            PromptKind::Documentation => &["DOCUMENTATION_PROMPT.md"],
            PromptKind::Commit => &["COMMIT_PROMPT.md"],
            PromptKind::ImplementationPlan => &["IMPLEMENTATION_PLAN_PROMPT.md"],
            PromptKind::Review => &["REVIEW_PROMPT.md"],
            PromptKind::MergeConflict => &["MERGE_CONFLICT_PROMPT.md"],
        }
    }

    pub(crate) fn default_template(&self) -> &'static str {
        match self {
            PromptKind::Documentation => DOCUMENTATION_PROMPT,
            PromptKind::Commit => COMMIT_PROMPT,
            PromptKind::ImplementationPlan => IMPLEMENTATION_PLAN_PROMPT,
            PromptKind::Review => REVIEW_PROMPT,
            PromptKind::MergeConflict => MERGE_CONFLICT_PROMPT,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptOrigin {
    ScopedConfig { scope: CommandScope },
    RepoFile { path: PathBuf },
    Default,
}

impl PromptOrigin {
    pub fn label(&self) -> &'static str {
        match self {
            PromptOrigin::ScopedConfig { .. } => "scoped-config",
            PromptOrigin::RepoFile { .. } => "repo-file",
            PromptOrigin::Default => "default",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PromptSelection {
    pub text: String,
    pub kind: PromptKind,
    pub requested_scope: CommandScope,
    pub origin: PromptOrigin,
    pub source_path: Option<PathBuf>,
}
