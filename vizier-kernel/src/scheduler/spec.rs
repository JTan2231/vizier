use super::{
    AfterPolicy, JobArtifact, JobLock, JobStatus, JobWaitKind, JobWaitReason, LockState,
    format_artifact,
};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default)]
pub struct SchedulerFacts {
    pub job_statuses: HashMap<String, JobStatus>,
    pub job_after_dependencies: HashMap<String, Vec<JobAfterDependencyStatus>>,
    pub job_dependencies: HashMap<String, Vec<JobArtifact>>,
    pub producer_statuses: HashMap<JobArtifact, Vec<JobStatus>>,
    pub artifact_exists: HashSet<JobArtifact>,
    pub job_locks: HashMap<String, Vec<JobLock>>,
    pub pinned_heads: HashMap<String, PinnedHeadFact>,
    pub waited_on: HashMap<String, Vec<JobWaitKind>>,
    pub has_child_args: HashSet<String>,
    pub job_order: Vec<String>,
    pub lock_state: LockState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedulerAction {
    UpdateStatus,
    Start,
    FailMissingChildArgs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerDecision {
    pub next_status: JobStatus,
    pub wait_reason: Option<JobWaitReason>,
    pub waited_on: Vec<JobWaitKind>,
    pub action: SchedulerAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedHeadFact {
    pub branch: String,
    pub matches: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AfterDependencyState {
    Missing,
    Invalid { detail: String },
    Status(JobStatus),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobAfterDependencyStatus {
    pub job_id: String,
    pub policy: AfterPolicy,
    pub state: AfterDependencyState,
}

pub fn evaluate_job(facts: &SchedulerFacts, job_id: &str) -> SchedulerDecision {
    let mut lock_state = facts.lock_state.clone();
    evaluate_job_with_lock_state(facts, job_id, &mut lock_state)
}

pub fn evaluate_all(facts: &SchedulerFacts) -> HashMap<String, SchedulerDecision> {
    let mut decisions = HashMap::new();
    let mut lock_state = facts.lock_state.clone();

    for job_id in &facts.job_order {
        let status = match facts.job_statuses.get(job_id) {
            Some(status) => *status,
            None => continue,
        };
        if job_is_terminal(status) || status == JobStatus::Running {
            continue;
        }

        let decision = evaluate_job_with_lock_state(facts, job_id, &mut lock_state);
        decisions.insert(job_id.clone(), decision);
    }

    decisions
}

#[derive(Debug)]
enum DependencyState {
    Ready,
    Waiting { detail: String },
    Blocked { detail: String },
}

fn evaluate_job_with_lock_state(
    facts: &SchedulerFacts,
    job_id: &str,
    lock_state: &mut LockState,
) -> SchedulerDecision {
    match after_dependency_state(facts, job_id) {
        DependencyState::Blocked { detail } => {
            let mut waited_on = facts.waited_on.get(job_id).cloned().unwrap_or_default();
            note_waited(&mut waited_on, JobWaitKind::Dependencies);
            return SchedulerDecision {
                next_status: JobStatus::BlockedByDependency,
                wait_reason: Some(JobWaitReason {
                    kind: JobWaitKind::Dependencies,
                    detail: Some(detail),
                }),
                waited_on,
                action: SchedulerAction::UpdateStatus,
            };
        }
        DependencyState::Waiting { detail } => {
            let mut waited_on = facts.waited_on.get(job_id).cloned().unwrap_or_default();
            note_waited(&mut waited_on, JobWaitKind::Dependencies);
            return SchedulerDecision {
                next_status: JobStatus::WaitingOnDeps,
                wait_reason: Some(JobWaitReason {
                    kind: JobWaitKind::Dependencies,
                    detail: Some(detail),
                }),
                waited_on,
                action: SchedulerAction::UpdateStatus,
            };
        }
        DependencyState::Ready => {}
    }

    let dependencies = facts
        .job_dependencies
        .get(job_id)
        .cloned()
        .unwrap_or_default();
    let mut waited_on = facts.waited_on.get(job_id).cloned().unwrap_or_default();

    match dependency_state(facts, &dependencies) {
        DependencyState::Blocked { detail } => {
            note_waited(&mut waited_on, JobWaitKind::Dependencies);
            return SchedulerDecision {
                next_status: JobStatus::BlockedByDependency,
                wait_reason: Some(JobWaitReason {
                    kind: JobWaitKind::Dependencies,
                    detail: Some(detail),
                }),
                waited_on,
                action: SchedulerAction::UpdateStatus,
            };
        }
        DependencyState::Waiting { detail } => {
            note_waited(&mut waited_on, JobWaitKind::Dependencies);
            return SchedulerDecision {
                next_status: JobStatus::WaitingOnDeps,
                wait_reason: Some(JobWaitReason {
                    kind: JobWaitKind::Dependencies,
                    detail: Some(detail),
                }),
                waited_on,
                action: SchedulerAction::UpdateStatus,
            };
        }
        DependencyState::Ready => {}
    }

    if let Some(pinned) = facts.pinned_heads.get(job_id)
        && !pinned.matches
    {
        note_waited(&mut waited_on, JobWaitKind::PinnedHead);
        return SchedulerDecision {
            next_status: JobStatus::WaitingOnDeps,
            wait_reason: Some(JobWaitReason {
                kind: JobWaitKind::PinnedHead,
                detail: Some(format!("pinned head mismatch on {}", pinned.branch)),
            }),
            waited_on,
            action: SchedulerAction::UpdateStatus,
        };
    }

    let locks = facts.job_locks.get(job_id).cloned().unwrap_or_default();
    if !lock_state.can_acquire_all(&locks) {
        note_waited(&mut waited_on, JobWaitKind::Locks);
        return SchedulerDecision {
            next_status: JobStatus::WaitingOnLocks,
            wait_reason: Some(JobWaitReason {
                kind: JobWaitKind::Locks,
                detail: Some("waiting on locks".to_string()),
            }),
            waited_on,
            action: SchedulerAction::UpdateStatus,
        };
    }

    if !facts.has_child_args.contains(job_id) {
        return SchedulerDecision {
            next_status: JobStatus::Failed,
            wait_reason: Some(JobWaitReason {
                kind: JobWaitKind::Dependencies,
                detail: Some("missing child args".to_string()),
            }),
            waited_on,
            action: SchedulerAction::FailMissingChildArgs,
        };
    }

    lock_state.acquire(&locks);
    SchedulerDecision {
        next_status: JobStatus::Running,
        wait_reason: None,
        waited_on,
        action: SchedulerAction::Start,
    }
}

fn dependency_state(facts: &SchedulerFacts, deps: &[JobArtifact]) -> DependencyState {
    for artifact in deps {
        if facts.artifact_exists.contains(artifact) {
            continue;
        }
        match facts.producer_statuses.get(artifact) {
            Some(statuses) if statuses.iter().any(|status| job_is_active(*status)) => {
                return DependencyState::Waiting {
                    detail: format!("waiting on {}", format_artifact(artifact)),
                };
            }
            Some(statuses)
                if statuses
                    .iter()
                    .any(|status| matches!(status, JobStatus::Succeeded)) =>
            {
                return DependencyState::Blocked {
                    detail: format!("missing {}", format_artifact(artifact)),
                };
            }
            Some(_) => {
                return DependencyState::Blocked {
                    detail: format!("dependency failed for {}", format_artifact(artifact)),
                };
            }
            None => {
                return DependencyState::Blocked {
                    detail: format!("missing {}", format_artifact(artifact)),
                };
            }
        }
    }

    DependencyState::Ready
}

fn after_dependency_state(facts: &SchedulerFacts, job_id: &str) -> DependencyState {
    let deps = facts
        .job_after_dependencies
        .get(job_id)
        .cloned()
        .unwrap_or_default();

    for dep in deps {
        match dep.policy {
            AfterPolicy::Success => match dep.state {
                AfterDependencyState::Missing => {
                    return DependencyState::Blocked {
                        detail: format!("missing job dependency {}", dep.job_id),
                    };
                }
                AfterDependencyState::Invalid { detail } => {
                    return DependencyState::Blocked {
                        detail: format!(
                            "scheduler data error for job dependency {}: {}",
                            dep.job_id, detail
                        ),
                    };
                }
                AfterDependencyState::Status(JobStatus::Succeeded) => {}
                AfterDependencyState::Status(status) if job_is_active(status) => {
                    return DependencyState::Waiting {
                        detail: format!("waiting on job {}", dep.job_id),
                    };
                }
                AfterDependencyState::Status(status) => {
                    return DependencyState::Blocked {
                        detail: format!(
                            "dependency failed for job {} ({})",
                            dep.job_id,
                            status_label(status)
                        ),
                    };
                }
            },
        }
    }

    DependencyState::Ready
}

fn note_waited(waited_on: &mut Vec<JobWaitKind>, kind: JobWaitKind) {
    if !waited_on.contains(&kind) {
        waited_on.push(kind);
    }
}

fn job_is_terminal(status: JobStatus) -> bool {
    matches!(
        status,
        JobStatus::Succeeded
            | JobStatus::Failed
            | JobStatus::Cancelled
            | JobStatus::BlockedByDependency
    )
}

fn job_is_active(status: JobStatus) -> bool {
    matches!(
        status,
        JobStatus::Queued
            | JobStatus::WaitingOnDeps
            | JobStatus::WaitingOnLocks
            | JobStatus::Running
    )
}

fn status_label(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Queued => "queued",
        JobStatus::WaitingOnDeps => "waiting_on_deps",
        JobStatus::WaitingOnLocks => "waiting_on_locks",
        JobStatus::Running => "running",
        JobStatus::Succeeded => "succeeded",
        JobStatus::Failed => "failed",
        JobStatus::Cancelled => "cancelled",
        JobStatus::BlockedByDependency => "blocked_by_dependency",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::LockMode;

    fn base_facts(job_id: &str) -> SchedulerFacts {
        let mut facts = SchedulerFacts::default();
        facts.job_order.push(job_id.to_string());
        facts
            .job_statuses
            .insert(job_id.to_string(), JobStatus::Queued);
        facts.has_child_args.insert(job_id.to_string());
        facts
    }

    fn artifact_for(kind: usize, suffix: &str) -> JobArtifact {
        match kind {
            0 => JobArtifact::PlanBranch {
                slug: format!("plan-{suffix}"),
                branch: format!("draft/plan-{suffix}"),
            },
            1 => JobArtifact::PlanDoc {
                slug: format!("doc-{suffix}"),
                branch: format!("draft/doc-{suffix}"),
            },
            2 => JobArtifact::PlanCommits {
                slug: format!("commits-{suffix}"),
                branch: format!("draft/commits-{suffix}"),
            },
            3 => JobArtifact::TargetBranch {
                name: format!("target-{suffix}"),
            },
            4 => JobArtifact::MergeSentinel {
                slug: format!("merge-{suffix}"),
            },
            _ => JobArtifact::CommandPatch {
                job_id: format!("job-{suffix}"),
            },
        }
    }

    #[test]
    fn dependency_matrix_covers_artifacts() {
        let kinds = 0..6;

        for kind in kinds {
            let job_id = format!("job-{kind}");
            let missing = artifact_for(kind, &format!("missing-{kind}"));
            let exists = artifact_for(kind, &format!("exists-{kind}"));

            let mut facts = base_facts(&job_id);
            facts
                .job_dependencies
                .insert(job_id.clone(), vec![missing.clone()]);
            facts
                .producer_statuses
                .insert(missing.clone(), vec![JobStatus::Running]);
            let decision = evaluate_job(&facts, &job_id);
            assert_eq!(decision.next_status, JobStatus::WaitingOnDeps);
            assert_eq!(decision.action, SchedulerAction::UpdateStatus);
            let reason = decision.wait_reason.expect("wait reason");
            assert_eq!(reason.kind, JobWaitKind::Dependencies);
            assert_eq!(
                reason.detail.as_deref(),
                Some(format!("waiting on {}", format_artifact(&missing)).as_str())
            );

            let mut facts = base_facts(&job_id);
            facts
                .job_dependencies
                .insert(job_id.clone(), vec![missing.clone()]);
            facts
                .producer_statuses
                .insert(missing.clone(), vec![JobStatus::Succeeded]);
            let decision = evaluate_job(&facts, &job_id);
            assert_eq!(decision.next_status, JobStatus::BlockedByDependency);
            let reason = decision.wait_reason.expect("wait reason");
            assert_eq!(reason.kind, JobWaitKind::Dependencies);
            assert_eq!(
                reason.detail.as_deref(),
                Some(format!("missing {}", format_artifact(&missing)).as_str())
            );

            let mut facts = base_facts(&job_id);
            facts
                .job_dependencies
                .insert(job_id.clone(), vec![missing.clone()]);
            facts
                .producer_statuses
                .insert(missing.clone(), vec![JobStatus::Failed]);
            let decision = evaluate_job(&facts, &job_id);
            assert_eq!(decision.next_status, JobStatus::BlockedByDependency);
            let reason = decision.wait_reason.expect("wait reason");
            assert_eq!(reason.kind, JobWaitKind::Dependencies);
            assert_eq!(
                reason.detail.as_deref(),
                Some(format!("dependency failed for {}", format_artifact(&missing)).as_str())
            );

            let mut facts = base_facts(&job_id);
            facts
                .job_dependencies
                .insert(job_id.clone(), vec![missing.clone()]);
            let decision = evaluate_job(&facts, &job_id);
            assert_eq!(decision.next_status, JobStatus::BlockedByDependency);
            let reason = decision.wait_reason.expect("wait reason");
            assert_eq!(reason.kind, JobWaitKind::Dependencies);
            assert_eq!(
                reason.detail.as_deref(),
                Some(format!("missing {}", format_artifact(&missing)).as_str())
            );

            let mut facts = base_facts(&job_id);
            facts
                .job_dependencies
                .insert(job_id.clone(), vec![exists.clone()]);
            facts.artifact_exists.insert(exists);
            let decision = evaluate_job(&facts, &job_id);
            assert_eq!(decision.action, SchedulerAction::Start);
            assert_eq!(decision.next_status, JobStatus::Running);
        }
    }

    #[test]
    fn after_dependency_matrix_covers_statuses() {
        let job_id = "job-after";

        let mut facts = base_facts(job_id);
        facts.job_after_dependencies.insert(
            job_id.to_string(),
            vec![JobAfterDependencyStatus {
                job_id: "dep-running".to_string(),
                policy: AfterPolicy::Success,
                state: AfterDependencyState::Status(JobStatus::Running),
            }],
        );
        let decision = evaluate_job(&facts, job_id);
        assert_eq!(decision.next_status, JobStatus::WaitingOnDeps);
        assert_eq!(
            decision.wait_reason.and_then(|value| value.detail),
            Some("waiting on job dep-running".to_string())
        );

        let mut facts = base_facts(job_id);
        facts.job_after_dependencies.insert(
            job_id.to_string(),
            vec![JobAfterDependencyStatus {
                job_id: "dep-succeeded".to_string(),
                policy: AfterPolicy::Success,
                state: AfterDependencyState::Status(JobStatus::Succeeded),
            }],
        );
        let decision = evaluate_job(&facts, job_id);
        assert_eq!(decision.action, SchedulerAction::Start);
        assert_eq!(decision.next_status, JobStatus::Running);

        for status in [
            JobStatus::Failed,
            JobStatus::Cancelled,
            JobStatus::BlockedByDependency,
        ] {
            let mut facts = base_facts(job_id);
            facts.job_after_dependencies.insert(
                job_id.to_string(),
                vec![JobAfterDependencyStatus {
                    job_id: "dep-terminal".to_string(),
                    policy: AfterPolicy::Success,
                    state: AfterDependencyState::Status(status),
                }],
            );
            let decision = evaluate_job(&facts, job_id);
            assert_eq!(decision.next_status, JobStatus::BlockedByDependency);
            let detail = decision
                .wait_reason
                .and_then(|value| value.detail)
                .unwrap_or_default();
            assert!(
                detail.contains("dependency failed for job dep-terminal"),
                "unexpected detail for {status:?}: {detail}"
            );
            assert!(
                detail.contains(status_label(status)),
                "expected status label in detail for {status:?}: {detail}"
            );
        }

        let mut facts = base_facts(job_id);
        facts.job_after_dependencies.insert(
            job_id.to_string(),
            vec![JobAfterDependencyStatus {
                job_id: "dep-missing".to_string(),
                policy: AfterPolicy::Success,
                state: AfterDependencyState::Missing,
            }],
        );
        let decision = evaluate_job(&facts, job_id);
        assert_eq!(decision.next_status, JobStatus::BlockedByDependency);
        assert_eq!(
            decision.wait_reason.and_then(|value| value.detail),
            Some("missing job dependency dep-missing".to_string())
        );

        let mut facts = base_facts(job_id);
        facts.job_after_dependencies.insert(
            job_id.to_string(),
            vec![JobAfterDependencyStatus {
                job_id: "dep-invalid".to_string(),
                policy: AfterPolicy::Success,
                state: AfterDependencyState::Invalid {
                    detail: "invalid job status".to_string(),
                },
            }],
        );
        let decision = evaluate_job(&facts, job_id);
        assert_eq!(decision.next_status, JobStatus::BlockedByDependency);
        assert_eq!(
            decision.wait_reason.and_then(|value| value.detail),
            Some(
                "scheduler data error for job dependency dep-invalid: invalid job status"
                    .to_string()
            )
        );
    }

    #[test]
    fn after_dependencies_checked_before_artifacts() {
        let job_id = "job";
        let artifact = JobArtifact::CommandPatch {
            job_id: "shared".to_string(),
        };
        let mut facts = base_facts(job_id);
        facts.job_after_dependencies.insert(
            job_id.to_string(),
            vec![JobAfterDependencyStatus {
                job_id: "dep-running".to_string(),
                policy: AfterPolicy::Success,
                state: AfterDependencyState::Status(JobStatus::Running),
            }],
        );
        facts
            .job_dependencies
            .insert(job_id.to_string(), vec![artifact.clone()]);
        facts.producer_statuses.insert(
            artifact.clone(),
            vec![JobStatus::Failed, JobStatus::Succeeded],
        );

        let decision = evaluate_job(&facts, job_id);
        assert_eq!(decision.next_status, JobStatus::WaitingOnDeps);
        let reason = decision.wait_reason.expect("wait reason");
        assert_eq!(reason.kind, JobWaitKind::Dependencies);
        assert_eq!(reason.detail.as_deref(), Some("waiting on job dep-running"));
    }

    #[test]
    fn dependency_precedence_prefers_active_producer() {
        let job_id = "job";
        let artifact = JobArtifact::CommandPatch {
            job_id: "shared".to_string(),
        };
        let mut facts = base_facts(job_id);
        facts
            .job_dependencies
            .insert(job_id.to_string(), vec![artifact.clone()]);
        facts.producer_statuses.insert(
            artifact.clone(),
            vec![JobStatus::Succeeded, JobStatus::Running],
        );
        let decision = evaluate_job(&facts, job_id);
        assert_eq!(decision.next_status, JobStatus::WaitingOnDeps);
        let reason = decision.wait_reason.expect("wait reason");
        assert_eq!(reason.kind, JobWaitKind::Dependencies);
        assert_eq!(
            reason.detail.as_deref(),
            Some(format!("waiting on {}", format_artifact(&artifact)).as_str())
        );
    }

    #[test]
    fn pinned_head_checked_after_dependencies() {
        let job_id = "job";
        let artifact = JobArtifact::CommandPatch {
            job_id: "dep".to_string(),
        };
        let mut facts = base_facts(job_id);
        facts
            .job_dependencies
            .insert(job_id.to_string(), vec![artifact.clone()]);
        facts.pinned_heads.insert(
            job_id.to_string(),
            PinnedHeadFact {
                branch: "main".to_string(),
                matches: false,
            },
        );
        let decision = evaluate_job(&facts, job_id);
        assert_eq!(decision.next_status, JobStatus::BlockedByDependency);
        let reason = decision.wait_reason.expect("wait reason");
        assert_eq!(reason.kind, JobWaitKind::Dependencies);
    }

    #[test]
    fn pinned_head_blocks_before_locks() {
        let job_id = "job";
        let mut facts = base_facts(job_id);
        facts.pinned_heads.insert(
            job_id.to_string(),
            PinnedHeadFact {
                branch: "main".to_string(),
                matches: false,
            },
        );
        let lock = JobLock {
            key: "lock-a".to_string(),
            mode: LockMode::Exclusive,
        };
        facts.job_locks.insert(job_id.to_string(), vec![lock]);
        let decision = evaluate_job(&facts, job_id);
        assert_eq!(decision.next_status, JobStatus::WaitingOnDeps);
        let reason = decision.wait_reason.expect("wait reason");
        assert_eq!(reason.kind, JobWaitKind::PinnedHead);
    }

    #[test]
    fn lock_contention_honors_job_order() {
        let lock = JobLock {
            key: "lock-serial".to_string(),
            mode: LockMode::Exclusive,
        };
        let mut facts = SchedulerFacts::default();
        for job_id in ["job-early", "job-late"] {
            facts.job_order.push(job_id.to_string());
            facts
                .job_statuses
                .insert(job_id.to_string(), JobStatus::Queued);
            facts.has_child_args.insert(job_id.to_string());
            facts
                .job_locks
                .insert(job_id.to_string(), vec![lock.clone()]);
        }
        let decisions = evaluate_all(&facts);
        let early = decisions.get("job-early").expect("early decision");
        assert_eq!(early.action, SchedulerAction::Start);
        let late = decisions.get("job-late").expect("late decision");
        assert_eq!(late.next_status, JobStatus::WaitingOnLocks);
        let reason = late.wait_reason.as_ref().expect("wait reason");
        assert_eq!(reason.kind, JobWaitKind::Locks);
    }

    #[test]
    fn missing_child_args_fails_after_locks() {
        let job_id = "job";
        let mut facts = base_facts(job_id);
        facts.has_child_args.remove(job_id);
        let decision = evaluate_job(&facts, job_id);
        assert_eq!(decision.action, SchedulerAction::FailMissingChildArgs);
        assert_eq!(decision.next_status, JobStatus::Failed);
        let reason = decision.wait_reason.expect("wait reason");
        assert_eq!(reason.kind, JobWaitKind::Dependencies);
        assert_eq!(reason.detail.as_deref(), Some("missing child args"));
    }

    #[test]
    fn lock_wait_beats_missing_child_args() {
        let job_id = "job";
        let mut facts = SchedulerFacts::default();
        facts.job_order.push(job_id.to_string());
        facts
            .job_statuses
            .insert(job_id.to_string(), JobStatus::Queued);
        let lock = JobLock {
            key: "lock-a".to_string(),
            mode: LockMode::Exclusive,
        };
        facts
            .job_locks
            .insert(job_id.to_string(), vec![lock.clone()]);
        facts.lock_state.acquire(&[lock]);
        let decision = evaluate_job(&facts, job_id);
        assert_eq!(decision.next_status, JobStatus::WaitingOnLocks);
        let reason = decision.wait_reason.expect("wait reason");
        assert_eq!(reason.kind, JobWaitKind::Locks);
    }

    #[test]
    fn waited_on_accumulates_kinds() {
        let job_id = "job";
        let mut facts = base_facts(job_id);
        let lock = JobLock {
            key: "lock-a".to_string(),
            mode: LockMode::Exclusive,
        };
        facts
            .job_locks
            .insert(job_id.to_string(), vec![lock.clone()]);
        facts.lock_state.acquire(&[lock]);
        facts
            .waited_on
            .insert(job_id.to_string(), vec![JobWaitKind::Dependencies]);
        let decision = evaluate_job(&facts, job_id);
        assert_eq!(
            decision.waited_on,
            vec![JobWaitKind::Dependencies, JobWaitKind::Locks]
        );
    }
}
