use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod spec;

pub type JobId = String;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    WaitingOnDeps,
    WaitingOnLocks,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    BlockedByDependency,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum JobArtifact {
    PlanBranch { slug: String, branch: String },
    PlanDoc { slug: String, branch: String },
    PlanCommits { slug: String, branch: String },
    TargetBranch { name: String },
    MergeSentinel { slug: String },
    AskSavePatch { job_id: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LockMode {
    Shared,
    Exclusive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobLock {
    pub key: String,
    pub mode: LockMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PinnedHead {
    pub branch: String,
    pub oid: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobWaitKind {
    Dependencies,
    Locks,
    PinnedHead,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobWaitReason {
    pub kind: JobWaitKind,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct LockState {
    exclusive: HashMap<String, usize>,
    shared: HashMap<String, usize>,
}

impl LockState {
    pub fn can_acquire(&self, lock: &JobLock) -> bool {
        match lock.mode {
            LockMode::Exclusive => {
                !self.exclusive.contains_key(&lock.key) && !self.shared.contains_key(&lock.key)
            }
            LockMode::Shared => !self.exclusive.contains_key(&lock.key),
        }
    }

    pub fn can_acquire_all(&self, locks: &[JobLock]) -> bool {
        locks.iter().all(|lock| self.can_acquire(lock))
    }

    pub fn acquire(&mut self, locks: &[JobLock]) {
        for lock in locks {
            match lock.mode {
                LockMode::Exclusive => {
                    *self.exclusive.entry(lock.key.clone()).or_insert(0) += 1;
                }
                LockMode::Shared => {
                    *self.shared.entry(lock.key.clone()).or_insert(0) += 1;
                }
            }
        }
    }
}

pub fn format_artifact(artifact: &JobArtifact) -> String {
    match artifact {
        JobArtifact::PlanBranch { slug, branch } => format!("plan_branch:{slug} ({branch})"),
        JobArtifact::PlanDoc { slug, branch } => format!("plan_doc:{slug} ({branch})"),
        JobArtifact::PlanCommits { slug, branch } => format!("plan_commits:{slug} ({branch})"),
        JobArtifact::TargetBranch { name } => format!("target_branch:{name}"),
        JobArtifact::MergeSentinel { slug } => format!("merge_sentinel:{slug}"),
        JobArtifact::AskSavePatch { job_id } => format!("ask_save_patch:{job_id}"),
    }
}
