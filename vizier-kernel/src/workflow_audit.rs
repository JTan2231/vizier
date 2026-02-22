use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::scheduler::{JobArtifact, JobLock, LockMode, format_artifact};
use crate::workflow_template::{WorkflowTemplate, workflow_operation_output_artifact};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WorkflowAuditOutputArtifactsByOutcome {
    pub succeeded: Vec<String>,
    pub failed: Vec<String>,
    pub blocked: Vec<String>,
    pub cancelled: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowAuditUntetheredInput {
    pub artifact: String,
    pub consumers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowAuditEffectiveLocks {
    pub node_id: String,
    pub locks: Vec<JobLock>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WorkflowAuditReport {
    pub output_artifacts: Vec<String>,
    pub output_artifacts_by_outcome: WorkflowAuditOutputArtifactsByOutcome,
    pub untethered_inputs: Vec<WorkflowAuditUntetheredInput>,
    pub effective_locks: Vec<WorkflowAuditEffectiveLocks>,
}

pub fn analyze_workflow_template(template: &WorkflowTemplate) -> WorkflowAuditReport {
    analyze_workflow_template_with_effective_locks(template, &BTreeMap::new())
}

pub fn analyze_workflow_template_with_effective_locks(
    template: &WorkflowTemplate,
    effective_locks_by_node: &BTreeMap<String, Vec<JobLock>>,
) -> WorkflowAuditReport {
    let mut produced_by_artifact = BTreeMap::<String, BTreeSet<String>>::new();
    let mut consumed_by_artifact = BTreeMap::<String, BTreeSet<String>>::new();
    let mut produced_succeeded = BTreeSet::<String>::new();
    let mut produced_failed = BTreeSet::<String>::new();
    let mut produced_blocked = BTreeSet::<String>::new();
    let mut produced_cancelled = BTreeSet::<String>::new();

    for node in &template.nodes {
        let implicit_operation_output =
            format_artifact(&workflow_operation_output_artifact(&node.id));
        collect_produced_artifacts(
            &node.id,
            &node.produces.succeeded,
            &implicit_operation_output,
            &mut produced_succeeded,
            &mut produced_by_artifact,
        );
        collect_produced_artifacts(
            &node.id,
            &node.produces.failed,
            &implicit_operation_output,
            &mut produced_failed,
            &mut produced_by_artifact,
        );
        collect_produced_artifacts(
            &node.id,
            &node.produces.blocked,
            &implicit_operation_output,
            &mut produced_blocked,
            &mut produced_by_artifact,
        );
        collect_produced_artifacts(
            &node.id,
            &node.produces.cancelled,
            &implicit_operation_output,
            &mut produced_cancelled,
            &mut produced_by_artifact,
        );

        for artifact in &node.needs {
            consumed_by_artifact
                .entry(format_artifact(artifact))
                .or_default()
                .insert(node.id.clone());
        }
    }

    let output_artifacts = produced_by_artifact.keys().cloned().collect::<Vec<_>>();
    let output_artifacts_by_outcome = WorkflowAuditOutputArtifactsByOutcome {
        succeeded: produced_succeeded.into_iter().collect(),
        failed: produced_failed.into_iter().collect(),
        blocked: produced_blocked.into_iter().collect(),
        cancelled: produced_cancelled.into_iter().collect(),
    };
    let untethered_inputs = consumed_by_artifact
        .iter()
        .filter_map(|(artifact, consumers)| {
            if produced_by_artifact.contains_key(artifact) {
                None
            } else {
                Some(WorkflowAuditUntetheredInput {
                    artifact: artifact.clone(),
                    consumers: consumers.iter().cloned().collect(),
                })
            }
        })
        .collect::<Vec<_>>();
    let mut effective_locks = template
        .nodes
        .iter()
        .map(|node| WorkflowAuditEffectiveLocks {
            node_id: node.id.clone(),
            locks: normalize_locks(
                effective_locks_by_node
                    .get(&node.id)
                    .cloned()
                    .unwrap_or_default(),
            ),
        })
        .collect::<Vec<_>>();
    effective_locks.sort_by(|left, right| left.node_id.cmp(&right.node_id));

    WorkflowAuditReport {
        output_artifacts,
        output_artifacts_by_outcome,
        untethered_inputs,
        effective_locks,
    }
}

fn lock_mode_rank(mode: LockMode) -> u8 {
    match mode {
        LockMode::Shared => 0,
        LockMode::Exclusive => 1,
    }
}

fn normalize_locks(mut locks: Vec<JobLock>) -> Vec<JobLock> {
    locks.sort_by(|left, right| {
        left.key
            .cmp(&right.key)
            .then(lock_mode_rank(left.mode).cmp(&lock_mode_rank(right.mode)))
    });
    locks.dedup();
    locks
}

fn collect_produced_artifacts(
    node_id: &str,
    explicit_artifacts: &[JobArtifact],
    implicit_operation_output: &str,
    outcome_artifacts: &mut BTreeSet<String>,
    produced_by_artifact: &mut BTreeMap<String, BTreeSet<String>>,
) {
    for artifact in explicit_artifacts {
        let formatted = format_artifact(artifact);
        outcome_artifacts.insert(formatted.clone());
        produced_by_artifact
            .entry(formatted)
            .or_default()
            .insert(node_id.to_string());
    }

    outcome_artifacts.insert(implicit_operation_output.to_string());
    produced_by_artifact
        .entry(implicit_operation_output.to_string())
        .or_default()
        .insert(node_id.to_string());
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::scheduler::{JobArtifact, JobLock, LockMode};
    use crate::workflow_template::{WorkflowNode, WorkflowNodeKind, WorkflowTemplate};

    use super::{analyze_workflow_template, analyze_workflow_template_with_effective_locks};

    fn template_with_nodes(nodes: Vec<WorkflowNode>) -> WorkflowTemplate {
        WorkflowTemplate {
            id: "template.audit".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            node_lock_scope_contexts: BTreeMap::new(),
            policy: Default::default(),
            artifact_contracts: Vec::new(),
            nodes,
        }
    }

    fn shell_node(id: &str) -> WorkflowNode {
        WorkflowNode {
            id: id.to_string(),
            kind: WorkflowNodeKind::Shell,
            uses: "cap.env.shell.command.run".to_string(),
            args: BTreeMap::from([("script".to_string(), "true".to_string())]),
            after: Vec::new(),
            needs: Vec::new(),
            produces: Default::default(),
            locks: Vec::new(),
            preconditions: Vec::new(),
            gates: Vec::new(),
            retry: Default::default(),
            on: Default::default(),
        }
    }

    #[test]
    fn fully_wired_templates_report_no_untethered_inputs() {
        let mut producer = shell_node("producer");
        producer.produces.succeeded.push(JobArtifact::Custom {
            type_id: "prompt_text".to_string(),
            key: "main".to_string(),
        });

        let mut consumer = shell_node("consumer");
        consumer.needs.push(JobArtifact::Custom {
            type_id: "prompt_text".to_string(),
            key: "main".to_string(),
        });

        let report = analyze_workflow_template(&template_with_nodes(vec![producer, consumer]));
        assert!(
            report.untethered_inputs.is_empty(),
            "expected zero untethered inputs: {report:?}"
        );
    }

    #[test]
    fn untethered_inputs_include_consumer_node_ids() {
        let mut node_b = shell_node("node_b");
        node_b.needs.push(JobArtifact::Custom {
            type_id: "prompt_text".to_string(),
            key: "missing".to_string(),
        });

        let mut node_a = shell_node("node_a");
        node_a.needs.push(JobArtifact::Custom {
            type_id: "prompt_text".to_string(),
            key: "missing".to_string(),
        });

        let report = analyze_workflow_template(&template_with_nodes(vec![node_b, node_a]));
        assert_eq!(
            report.untethered_inputs,
            vec![super::WorkflowAuditUntetheredInput {
                artifact: "custom:prompt_text:missing".to_string(),
                consumers: vec!["node_a".to_string(), "node_b".to_string()],
            }]
        );
    }

    #[test]
    fn implicit_operation_output_artifacts_are_included() {
        let report = analyze_workflow_template(&template_with_nodes(vec![
            shell_node("first"),
            shell_node("second"),
        ]));

        for outcome in [
            &report.output_artifacts_by_outcome.succeeded,
            &report.output_artifacts_by_outcome.failed,
            &report.output_artifacts_by_outcome.blocked,
            &report.output_artifacts_by_outcome.cancelled,
        ] {
            assert_eq!(
                outcome,
                &vec![
                    "custom:operation_output:first".to_string(),
                    "custom:operation_output:second".to_string(),
                ],
                "expected implicit operation output artifacts for all outcomes"
            );
        }
    }

    #[test]
    fn output_artifacts_are_stable_deduped_and_partitioned_by_outcome() {
        let mut node_one = shell_node("node_one");
        node_one.produces.succeeded.push(JobArtifact::PlanDoc {
            slug: "alpha".to_string(),
            branch: "draft/alpha".to_string(),
        });
        node_one.produces.succeeded.push(JobArtifact::Custom {
            type_id: "stage_token".to_string(),
            key: "approve:alpha".to_string(),
        });
        node_one.produces.failed.push(JobArtifact::Custom {
            type_id: "stage_token".to_string(),
            key: "approve:alpha".to_string(),
        });

        let mut node_two = shell_node("node_two");
        node_two.produces.cancelled.push(JobArtifact::TargetBranch {
            name: "main".to_string(),
        });

        let report = analyze_workflow_template(&template_with_nodes(vec![node_one, node_two]));
        assert_eq!(
            report.output_artifacts,
            vec![
                "custom:operation_output:node_one".to_string(),
                "custom:operation_output:node_two".to_string(),
                "custom:stage_token:approve:alpha".to_string(),
                "plan_doc:alpha (draft/alpha)".to_string(),
                "target_branch:main".to_string(),
            ]
        );
        assert_eq!(
            report.output_artifacts_by_outcome.succeeded,
            vec![
                "custom:operation_output:node_one".to_string(),
                "custom:operation_output:node_two".to_string(),
                "custom:stage_token:approve:alpha".to_string(),
                "plan_doc:alpha (draft/alpha)".to_string(),
            ]
        );
        assert_eq!(
            report.output_artifacts_by_outcome.failed,
            vec![
                "custom:operation_output:node_one".to_string(),
                "custom:operation_output:node_two".to_string(),
                "custom:stage_token:approve:alpha".to_string(),
            ]
        );
        assert_eq!(
            report.output_artifacts_by_outcome.blocked,
            vec![
                "custom:operation_output:node_one".to_string(),
                "custom:operation_output:node_two".to_string(),
            ]
        );
        assert_eq!(
            report.output_artifacts_by_outcome.cancelled,
            vec![
                "custom:operation_output:node_one".to_string(),
                "custom:operation_output:node_two".to_string(),
                "target_branch:main".to_string(),
            ]
        );
    }

    #[test]
    fn report_includes_effective_locks_when_provided() {
        let template = template_with_nodes(vec![shell_node("node_b"), shell_node("node_a")]);
        let locks = BTreeMap::from([
            (
                "node_b".to_string(),
                vec![
                    JobLock {
                        key: "branch:main".to_string(),
                        mode: LockMode::Exclusive,
                    },
                    JobLock {
                        key: "branch:main".to_string(),
                        mode: LockMode::Exclusive,
                    },
                ],
            ),
            (
                "node_a".to_string(),
                vec![JobLock {
                    key: "repo_serial".to_string(),
                    mode: LockMode::Exclusive,
                }],
            ),
        ]);

        let report = analyze_workflow_template_with_effective_locks(&template, &locks);
        assert_eq!(
            report.effective_locks,
            vec![
                super::WorkflowAuditEffectiveLocks {
                    node_id: "node_a".to_string(),
                    locks: vec![JobLock {
                        key: "repo_serial".to_string(),
                        mode: LockMode::Exclusive,
                    }],
                },
                super::WorkflowAuditEffectiveLocks {
                    node_id: "node_b".to_string(),
                    locks: vec![JobLock {
                        key: "branch:main".to_string(),
                        mode: LockMode::Exclusive,
                    }],
                },
            ]
        );
    }
}
