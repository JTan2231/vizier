use std::path::{Path, PathBuf};

use crate::scheduler::JobArtifact;
use crate::scheduler::spec::{SchedulerDecision, SchedulerFacts};

#[derive(Debug, Clone)]
pub struct OriginRef {
    pub owner: String,
    pub repo: String,
}

#[derive(Debug, Clone)]
pub struct RepoStatus {
    pub branch: String,
    pub clean: bool,
    pub ahead: usize,
    pub behind: usize,
}

pub trait FsPort {
    type Error: std::error::Error + Send + Sync + 'static;

    fn read_to_string(&self, path: &Path) -> Result<String, Self::Error>;
    fn write_string(&self, path: &Path, contents: &str) -> Result<(), Self::Error>;
    fn list_dir(&self, path: &Path) -> Result<Vec<PathBuf>, Self::Error>;
    fn exists(&self, path: &Path) -> Result<bool, Self::Error>;
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, Self::Error>;
}

pub trait VcsPort {
    type Error: std::error::Error + Send + Sync + 'static;

    fn diff(&self, base: &str, head: &str, paths: Option<&[String]>)
    -> Result<String, Self::Error>;
    fn log(&self, max: usize) -> Result<Vec<String>, Self::Error>;
    fn status(&self) -> Result<RepoStatus, Self::Error>;
    fn head(&self) -> Result<String, Self::Error>;
    fn origin(&self) -> Result<Option<OriginRef>, Self::Error>;
    fn create_worktree(&self, name: &str, path: &Path, branch: &str) -> Result<(), Self::Error>;
    fn commit(&self, message: &str, paths: &[String]) -> Result<String, Self::Error>;
}

pub trait ClockPort {
    fn now_rfc3339(&self) -> String;
    fn monotonic_ms(&self) -> u128;
}

#[derive(Debug, Clone)]
pub struct AgentRequest {
    pub prompt: String,
    pub scope: String,
    pub timeout_ms: Option<u64>,
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AgentResponse {
    pub stdout: String,
    pub stderr: Vec<String>,
    pub exit_code: i32,
    pub duration_ms: u128,
}

pub trait AgentPort {
    type Error: std::error::Error + Send + Sync + 'static;

    fn run(
        &self,
        request: AgentRequest,
        events: &dyn EventSink,
    ) -> Result<AgentResponse, Self::Error>;
}

pub trait SchedulerStore {
    type Error: std::error::Error + Send + Sync + 'static;

    fn load_facts(&self) -> Result<SchedulerFacts, Self::Error>;
    fn persist_decision(
        &self,
        job_id: &str,
        decision: &SchedulerDecision,
    ) -> Result<(), Self::Error>;
    fn record_artifact(&self, artifact: &JobArtifact) -> Result<(), Self::Error>;
}

pub trait EventSink {
    fn info(&self, message: &str);
    fn warn(&self, message: &str);
    fn progress(&self, message: &str);
}
