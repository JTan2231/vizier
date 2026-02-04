use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::config::AgentOutputHandling;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

impl MessageRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageRole::System => "System",
            MessageRole::User => "User",
            MessageRole::Assistant => "Assistant",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AgentRunRecord {
    pub command: Vec<String>,
    pub output: AgentOutputHandling,
    pub progress_filter: Option<Vec<String>>,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: Vec<String>,
    pub duration_ms: u128,
}

impl AgentRunRecord {
    pub fn to_rows(&self) -> Vec<(String, String)> {
        let mut rows = Vec::new();
        rows.push(("Exit code".to_string(), self.exit_code.to_string()));
        rows.push((
            "Duration".to_string(),
            format!("{:.2}s", self.duration_ms as f64 / 1000.0),
        ));
        rows
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitDisposition {
    Auto,
    Hold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditState {
    Clean,
    Committed,
    Pending,
}

#[derive(Clone, Debug)]
pub struct NarrativeChangeSet {
    pub paths: Vec<String>,
    pub summary: Option<String>,
}

impl NarrativeChangeSet {
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct SessionArtifact {
    pub id: String,
    pub path: PathBuf,
    relative_path: Option<String>,
}

impl SessionArtifact {
    pub fn new(id: &str, path: PathBuf, project_root: &Path) -> Self {
        let relative = path
            .strip_prefix(project_root)
            .ok()
            .map(|value| value.to_string_lossy().to_string());

        Self {
            id: id.to_string(),
            path,
            relative_path: relative,
        }
    }

    pub fn display_path(&self) -> String {
        self.relative_path
            .clone()
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

#[derive(Clone, Debug)]
pub struct AuditResult {
    pub session_artifact: Option<SessionArtifact>,
    pub state: AuditState,
    pub narrative_changes: Option<NarrativeChangeSet>,
}

impl AuditResult {
    pub fn session_display(&self) -> Option<String> {
        self.session_artifact
            .as_ref()
            .map(|artifact| artifact.display_path())
    }

    pub fn narrative_changes(&self) -> Option<&NarrativeChangeSet> {
        self.narrative_changes.as_ref()
    }

    pub fn committed(&self) -> bool {
        matches!(self.state, AuditState::Committed)
    }

    pub fn pending(&self) -> bool {
        matches!(self.state, AuditState::Pending)
    }

    pub fn is_committed(&self) -> bool {
        self.committed()
    }

    pub fn is_pending(&self) -> bool {
        self.pending()
    }
}
