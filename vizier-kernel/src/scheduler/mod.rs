use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

pub mod spec;

pub type JobId = String;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    WaitingOnDeps,
    WaitingOnApproval,
    WaitingOnLocks,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    BlockedByDependency,
    BlockedByApproval,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum JobArtifact {
    PlanBranch {
        slug: String,
        branch: String,
    },
    PlanDoc {
        slug: String,
        branch: String,
    },
    PlanCommits {
        slug: String,
        branch: String,
    },
    TargetBranch {
        name: String,
    },
    MergeSentinel {
        slug: String,
    },
    #[serde(rename = "command_patch")]
    CommandPatch {
        job_id: String,
    },
    Custom {
        type_id: String,
        key: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AfterPolicy {
    Success,
}

impl Default for AfterPolicy {
    fn default() -> Self {
        Self::Success
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissingProducerPolicy {
    Block,
    Wait,
}

impl Default for MissingProducerPolicy {
    fn default() -> Self {
        Self::Block
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobAfterDependency {
    pub job_id: String,
    #[serde(default)]
    pub policy: AfterPolicy,
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
    Approval,
    Locks,
    PinnedHead,
    Preconditions,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobApprovalState {
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobApprovalFact {
    #[serde(default)]
    pub required: bool,
    pub state: JobApprovalState,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobWaitReason {
    pub kind: JobWaitKind,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobPrecondition {
    CleanWorktree,
    BranchExists {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
    },
    Custom {
        id: String,
        #[serde(default)]
        args: BTreeMap<String, String>,
    },
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
        JobArtifact::CommandPatch { job_id } => format!("command_patch:{job_id}"),
        JobArtifact::Custom { type_id, key } => format!("custom:{type_id}:{key}"),
    }
}
