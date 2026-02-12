use std::future::Future;
use std::path::{Path, PathBuf};

use vizier_core::{
    display,
    vcs::repo_root,
    workflow_template::{
        WorkflowCapability, WorkflowGate, WorkflowNode, WorkflowNodeKind, WorkflowOutcomeEdges,
        WorkflowRetryMode, WorkflowTemplate, workflow_node_capability,
    },
};

use super::gates::{
    CicdScriptResult, StopConditionScriptResult, clip_log, log_cicd_result,
    log_stop_condition_result, record_stop_condition_attempt, run_cicd_script,
    run_stop_condition_script,
};

const APPROVE_APPLY_NODE_ID: &str = "approve_apply_once";
const APPROVE_STOP_GATE_NODE_ID: &str = "approve_gate_stop_condition";
const MERGE_INTEGRATE_NODE_ID: &str = "merge_integrate";
const MERGE_CONFLICT_NODE_ID: &str = "merge_conflict_resolution";
const MERGE_GATE_NODE_ID: &str = "merge_gate_cicd";
#[cfg(test)]
const MERGE_AUTO_FIX_NODE_ID: &str = "merge_cicd_auto_fix";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkflowNodeOutcome {
    Succeeded,
    Failed,
    Blocked,
}

fn outcome_targets(
    outcome_edges: &WorkflowOutcomeEdges,
    outcome: WorkflowNodeOutcome,
) -> &[String] {
    match outcome {
        WorkflowNodeOutcome::Succeeded => &outcome_edges.succeeded,
        WorkflowNodeOutcome::Failed => &outcome_edges.failed,
        WorkflowNodeOutcome::Blocked => &outcome_edges.blocked,
    }
}

fn outcome_contains_target(
    node: &WorkflowNode,
    outcome: WorkflowNodeOutcome,
    target: &str,
) -> bool {
    outcome_targets(&node.on, outcome)
        .iter()
        .any(|value| value == target)
}

fn find_template_node<'a>(
    template: &'a WorkflowTemplate,
    node_id: &str,
) -> Result<&'a WorkflowNode, Box<dyn std::error::Error>> {
    template
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .ok_or_else(|| {
            format!(
                "template {}@{} does not define node `{}`",
                template.id, template.version, node_id
            )
            .into()
        })
}

fn find_template_nodes_by_capability(
    template: &WorkflowTemplate,
    capability: WorkflowCapability,
) -> Vec<&WorkflowNode> {
    template
        .nodes
        .iter()
        .filter(|node| workflow_node_capability(node) == Some(capability))
        .collect::<Vec<_>>()
}

fn find_unique_template_node_by_capability<'a>(
    template: &'a WorkflowTemplate,
    capability: WorkflowCapability,
    scope: &str,
) -> Result<Option<&'a WorkflowNode>, Box<dyn std::error::Error>> {
    let matches = find_template_nodes_by_capability(template, capability);
    if matches.len() > 1 {
        return Err(format!(
            "{scope} defines multiple `{}` nodes; expected exactly one",
            capability.id()
        )
        .into());
    }
    Ok(matches.into_iter().next())
}

fn find_required_template_node<'a>(
    template: &'a WorkflowTemplate,
    canonical_node_id: &str,
    fallback_capability: WorkflowCapability,
    scope: &str,
) -> Result<&'a WorkflowNode, Box<dyn std::error::Error>> {
    if let Some(node) = template
        .nodes
        .iter()
        .find(|node| node.id == canonical_node_id)
    {
        return Ok(node);
    }
    if let Some(node) =
        find_unique_template_node_by_capability(template, fallback_capability, scope)?
    {
        return Ok(node);
    }
    Err(format!(
        "{scope} does not define node `{canonical_node_id}` or a `{}` node",
        fallback_capability.id()
    )
    .into())
}

fn outcome_target_nodes<'a>(
    template: &'a WorkflowTemplate,
    node: &WorkflowNode,
    outcome: WorkflowNodeOutcome,
) -> Result<Vec<&'a WorkflowNode>, Box<dyn std::error::Error>> {
    let mut targets = Vec::new();
    for target_id in outcome_targets(&node.on, outcome) {
        targets.push(find_template_node(template, target_id)?);
    }
    Ok(targets)
}

fn select_unique_outcome_target<'a, F>(
    template: &'a WorkflowTemplate,
    node: &WorkflowNode,
    outcome: WorkflowNodeOutcome,
    description: &str,
    mut predicate: F,
) -> Result<Option<&'a WorkflowNode>, Box<dyn std::error::Error>>
where
    F: FnMut(&WorkflowNode) -> bool,
{
    let mut matches = outcome_target_nodes(template, node, outcome)?
        .into_iter()
        .filter(|target| predicate(target))
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        let ids = matches
            .into_iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "workflow node `{}` has ambiguous {} targets: {}",
            node.id, description, ids
        )
        .into());
    }
    Ok(matches.pop())
}

fn node_has_script_gate(node: &WorkflowNode) -> bool {
    node.gates
        .iter()
        .any(|gate| matches!(gate, WorkflowGate::Script { .. }))
}

fn node_has_custom_gate(node: &WorkflowNode, gate_id: &str) -> bool {
    node.gates
        .iter()
        .any(|gate| matches!(gate, WorkflowGate::Custom { id, .. } if id == gate_id))
}

fn extract_script_gate(
    node: &WorkflowNode,
    scope: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let scripts = node
        .gates
        .iter()
        .filter_map(|gate| match gate {
            WorkflowGate::Script { script, .. } => Some(script.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if scripts.len() > 1 {
        return Err(format!(
            "{} node `{}` defines multiple script gates; expected at most one",
            scope, node.id
        )
        .into());
    }
    Ok(scripts.into_iter().next())
}

fn extract_cicd_gate(
    node: &WorkflowNode,
    scope: &str,
) -> Result<Option<(String, bool)>, Box<dyn std::error::Error>> {
    let gates = node
        .gates
        .iter()
        .filter_map(|gate| match gate {
            WorkflowGate::Cicd {
                script,
                auto_resolve,
                ..
            } => Some((script.clone(), *auto_resolve)),
            _ => None,
        })
        .collect::<Vec<_>>();
    if gates.len() > 1 {
        return Err(format!(
            "{} node `{}` defines multiple cicd gates; expected at most one",
            scope, node.id
        )
        .into());
    }
    Ok(gates.into_iter().next())
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn extract_custom_gate_bool_arg(
    node: &WorkflowNode,
    gate_id: &str,
    key: &str,
) -> Result<Option<bool>, Box<dyn std::error::Error>> {
    let gates = node
        .gates
        .iter()
        .filter_map(|gate| match gate {
            WorkflowGate::Custom { id, args, .. } if id == gate_id => Some(args.get(key).cloned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if gates.len() > 1 {
        return Err(format!(
            "workflow node `{}` defines multiple custom gates `{}`; expected at most one",
            node.id, gate_id
        )
        .into());
    }
    let value = gates.into_iter().next().flatten();
    Ok(value.as_deref().and_then(parse_bool))
}

#[derive(Debug, Clone)]
pub(crate) struct ApproveStopConditionPolicy {
    pub script: Option<PathBuf>,
    pub retries: u32,
    retry_path_enabled: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ApproveStopConditionReport {
    pub script: Option<PathBuf>,
    pub status: &'static str,
    pub attempts: u32,
    pub last_result: Option<StopConditionScriptResult>,
}

pub(crate) enum ApproveApplyOutcome<T> {
    Succeeded {
        value: T,
        report: ApproveStopConditionReport,
    },
    Failed {
        error: Box<dyn std::error::Error>,
        report: ApproveStopConditionReport,
    },
}

pub(crate) fn approve_stop_condition_policy(
    template: &WorkflowTemplate,
) -> Result<ApproveStopConditionPolicy, Box<dyn std::error::Error>> {
    let apply_node = find_required_template_node(
        template,
        APPROVE_APPLY_NODE_ID,
        WorkflowCapability::PlanApplyOnce,
        "approve template",
    )?;
    let legacy_script = extract_script_gate(apply_node, "approve template")?;

    let stop_node = select_unique_outcome_target(
        template,
        apply_node,
        WorkflowNodeOutcome::Succeeded,
        "approve stop-condition gate",
        |target| {
            target.id == APPROVE_STOP_GATE_NODE_ID
                || workflow_node_capability(target) == Some(WorkflowCapability::GateStopCondition)
                || (matches!(target.kind, WorkflowNodeKind::Gate) && node_has_script_gate(target))
        },
    )?;
    if let Some(stop_node) = stop_node {
        if !matches!(stop_node.kind, WorkflowNodeKind::Gate) {
            return Err(format!(
                "approve template node `{}` on.succeeded should target a gate node, found {:?}",
                apply_node.id, stop_node.kind
            )
            .into());
        }
        let script = extract_script_gate(stop_node, "approve stop-condition template")?
            .or(legacy_script)
            .map(PathBuf::from);
        let retries =
            if script.is_some() && matches!(stop_node.retry.mode, WorkflowRetryMode::UntilGate) {
                stop_node.retry.budget
            } else {
                0
            };
        let retry_path_enabled =
            outcome_contains_target(stop_node, WorkflowNodeOutcome::Failed, &apply_node.id);
        return Ok(ApproveStopConditionPolicy {
            script,
            retries,
            retry_path_enabled,
        });
    }

    if !outcome_targets(&apply_node.on, WorkflowNodeOutcome::Succeeded).is_empty() {
        return Err(format!(
            "approve template node `{}` has on.succeeded targets but no stop-condition gate was identified",
            apply_node.id
        )
        .into());
    }

    let script = legacy_script.map(PathBuf::from);
    let retries =
        if script.is_some() && matches!(apply_node.retry.mode, WorkflowRetryMode::UntilGate) {
            apply_node.retry.budget
        } else {
            0
        };
    let retry_path_enabled = script.is_some();
    Ok(ApproveStopConditionPolicy {
        script,
        retries,
        retry_path_enabled,
    })
}

pub(crate) async fn run_approve_apply_with_stop_condition<T, F, Fut>(
    worktree_path: &Path,
    policy: &ApproveStopConditionPolicy,
    mut apply_once: F,
) -> ApproveApplyOutcome<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Box<dyn std::error::Error>>>,
{
    let Some(script) = policy.script.as_ref() else {
        return match apply_once().await {
            Ok(value) => ApproveApplyOutcome::Succeeded {
                value,
                report: stop_condition_report(None, "none", 0, None),
            },
            Err(error) => ApproveApplyOutcome::Failed {
                error,
                report: stop_condition_report(None, "none", 0, None),
            },
        };
    };

    let mut attempts: u32 = 0;
    let mut remaining_retries = policy.retries;
    let mut last_result: Option<StopConditionScriptResult> = None;

    loop {
        let applied = match apply_once().await {
            Ok(value) => value,
            Err(error) => {
                return ApproveApplyOutcome::Failed {
                    error,
                    report: stop_condition_report(
                        Some(script),
                        "failed",
                        attempts,
                        last_result.clone(),
                    ),
                };
            }
        };

        let stop_result = match run_stop_condition_script(script, worktree_path) {
            Ok(result) => result,
            Err(error) => {
                return ApproveApplyOutcome::Failed {
                    error,
                    report: stop_condition_report(
                        Some(script),
                        "failed",
                        attempts,
                        last_result.clone(),
                    ),
                };
            }
        };

        attempts += 1;
        log_stop_condition_result(script, &stop_result, attempts);
        record_stop_condition_attempt("approve", script, attempts, &stop_result);
        let passed = stop_result.success();
        last_result = Some(stop_result);

        if passed {
            return ApproveApplyOutcome::Succeeded {
                value: applied,
                report: stop_condition_report(
                    Some(script),
                    "passed",
                    attempts,
                    last_result.clone(),
                ),
            };
        }

        if !policy.retry_path_enabled {
            let error = Box::<dyn std::error::Error>::from(format!(
                "Approve stop-condition `{}` failed and template on.failed does not route back to `{}`.",
                script.display(),
                APPROVE_APPLY_NODE_ID
            ));
            return ApproveApplyOutcome::Failed {
                error,
                report: stop_condition_report(
                    Some(script),
                    "failed",
                    attempts,
                    last_result.clone(),
                ),
            };
        }

        if remaining_retries == 0 {
            let error = Box::<dyn std::error::Error>::from(format!(
                "Approve stop-condition script `{}` did not succeed after {} attempt(s); inspect {} for partial changes and script logs.",
                script.display(),
                attempts,
                worktree_path.display()
            ));
            return ApproveApplyOutcome::Failed {
                error,
                report: stop_condition_report(
                    Some(script),
                    "failed",
                    attempts,
                    last_result.clone(),
                ),
            };
        }

        remaining_retries -= 1;
        display::info(format!(
            "Approve stop-condition not yet satisfied; retrying plan application ({} retries remaining).",
            remaining_retries
        ));
    }
}

fn stop_condition_report(
    script: Option<&Path>,
    status: &'static str,
    attempts: u32,
    last_result: Option<StopConditionScriptResult>,
) -> ApproveStopConditionReport {
    ApproveStopConditionReport {
        script: script.map(Path::to_path_buf),
        status,
        attempts,
        last_result,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ReviewCicdGatePolicy {
    pub script: Option<PathBuf>,
    pub auto_resolve: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ReviewCicdGateOutcome {
    pub script: PathBuf,
    pub result: CicdScriptResult,
}

pub(crate) fn review_cicd_gate_policy(
    gates: &[WorkflowGate],
) -> Result<ReviewCicdGatePolicy, Box<dyn std::error::Error>> {
    let cicd_gates = gates
        .iter()
        .filter_map(|gate| match gate {
            WorkflowGate::Cicd {
                script,
                auto_resolve,
                ..
            } => Some((script.clone(), *auto_resolve)),
            _ => None,
        })
        .collect::<Vec<_>>();
    if cicd_gates.len() > 1 {
        return Err(
            "review template node defines multiple cicd gates; expected at most one".into(),
        );
    }

    let (script, auto_resolve) = if let Some((script, auto_resolve)) = cicd_gates.into_iter().next()
    {
        (Some(PathBuf::from(script)), auto_resolve)
    } else {
        (None, false)
    };

    Ok(ReviewCicdGatePolicy {
        script,
        auto_resolve,
    })
}

pub(crate) fn run_review_cicd_gate(
    policy: &ReviewCicdGatePolicy,
) -> Result<Option<ReviewCicdGateOutcome>, Box<dyn std::error::Error>> {
    let Some(script) = policy.script.as_ref() else {
        return Ok(None);
    };

    let repo_root = repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let result = run_cicd_script(script, &repo_root)?;
    log_cicd_result(script, &result, 1);
    Ok(Some(ReviewCicdGateOutcome {
        script: script.clone(),
        result,
    }))
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct MergeConflictResolutionPolicy {
    pub auto_resolve: bool,
    pub retry_path_enabled: bool,
}

pub(crate) fn merge_conflict_resolution_policy(
    template: &WorkflowTemplate,
) -> Result<MergeConflictResolutionPolicy, Box<dyn std::error::Error>> {
    let integrate_node = find_required_template_node(
        template,
        MERGE_INTEGRATE_NODE_ID,
        WorkflowCapability::GitIntegratePlanBranch,
        "merge template",
    )?;
    let conflict_node = select_unique_outcome_target(
        template,
        integrate_node,
        WorkflowNodeOutcome::Blocked,
        "merge conflict-resolution gate",
        |target| {
            target.id == MERGE_CONFLICT_NODE_ID
                || workflow_node_capability(target)
                    == Some(WorkflowCapability::GateConflictResolution)
                || node_has_custom_gate(target, "conflict_resolution")
        },
    )?;

    let Some(conflict_node) = conflict_node else {
        return Ok(MergeConflictResolutionPolicy {
            auto_resolve: false,
            retry_path_enabled: false,
        });
    };
    if !matches!(conflict_node.kind, WorkflowNodeKind::Gate) {
        return Err(format!(
            "merge template node `{}` on.blocked should target a gate node, found {:?}",
            integrate_node.id, conflict_node.kind
        )
        .into());
    }

    let auto_resolve = if let Some(value) = conflict_node
        .args
        .get("auto_resolve")
        .and_then(|value| parse_bool(value))
    {
        value
    } else {
        extract_custom_gate_bool_arg(conflict_node, "conflict_resolution", "auto_resolve")?
            .unwrap_or_default()
    };
    let retry_path_enabled = outcome_contains_target(
        conflict_node,
        WorkflowNodeOutcome::Succeeded,
        &integrate_node.id,
    );

    Ok(MergeConflictResolutionPolicy {
        auto_resolve,
        retry_path_enabled,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct MergeCicdGatePolicy {
    pub script: Option<PathBuf>,
    pub auto_resolve: bool,
    pub retries: u32,
    retry_path_enabled: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct MergeCicdFixRequest {
    pub script: PathBuf,
    pub attempt: u32,
    pub max_attempts: u32,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone)]
pub(crate) struct MergeCicdGateOutcome<T> {
    pub script: PathBuf,
    pub attempts: u32,
    pub fixes: Vec<T>,
    pub result: CicdScriptResult,
}

#[derive(Debug, Clone)]
pub(crate) struct MergeCicdGateFailure<T> {
    pub script: PathBuf,
    pub attempts: u32,
    pub fixes: Vec<T>,
    pub result: CicdScriptResult,
}

#[derive(Debug, Clone)]
pub(crate) enum MergeCicdGateResult<T> {
    Skipped,
    Passed(MergeCicdGateOutcome<T>),
    Failed(MergeCicdGateFailure<T>),
}

pub(crate) fn merge_cicd_gate_policy(
    template: &WorkflowTemplate,
) -> Result<MergeCicdGatePolicy, Box<dyn std::error::Error>> {
    let mut gate_candidates = template
        .nodes
        .iter()
        .filter(|node| {
            node.id == MERGE_GATE_NODE_ID
                || workflow_node_capability(node) == Some(WorkflowCapability::GateCicd)
        })
        .collect::<Vec<_>>();
    gate_candidates.sort_by(|left, right| left.id.cmp(&right.id));
    gate_candidates.dedup_by(|left, right| left.id == right.id);
    if gate_candidates.len() > 1 {
        let ids = gate_candidates
            .iter()
            .map(|node| node.id.clone())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "merge template defines multiple CI/CD gate nodes ({ids}); expected one"
        )
        .into());
    }
    let (gate_node, explicit_gate_node) = if let Some(node) = gate_candidates.pop() {
        (node, true)
    } else {
        (
            find_required_template_node(
                template,
                MERGE_INTEGRATE_NODE_ID,
                WorkflowCapability::GitIntegratePlanBranch,
                "merge template",
            )?,
            false,
        )
    };
    let (script, auto_resolve) =
        if let Some((script, auto_resolve)) = extract_cicd_gate(gate_node, "merge template")? {
            (Some(PathBuf::from(script)), auto_resolve)
        } else {
            (None, false)
        };

    let retries =
        if script.is_some() && matches!(gate_node.retry.mode, WorkflowRetryMode::UntilGate) {
            gate_node.retry.budget
        } else {
            0
        };
    let retry_path_enabled = if script.is_none() {
        false
    } else if explicit_gate_node {
        if outcome_contains_target(gate_node, WorkflowNodeOutcome::Failed, &gate_node.id) {
            true
        } else {
            outcome_target_nodes(template, gate_node, WorkflowNodeOutcome::Failed)?
                .into_iter()
                .any(|target| {
                    outcome_contains_target(target, WorkflowNodeOutcome::Succeeded, &gate_node.id)
                })
        }
    } else {
        auto_resolve
    };

    Ok(MergeCicdGatePolicy {
        script,
        auto_resolve,
        retries,
        retry_path_enabled,
    })
}

pub(crate) async fn run_merge_cicd_gate<T, F, Fut>(
    policy: &MergeCicdGatePolicy,
    auto_fix_backend_ready: bool,
    mut attempt_auto_fix: F,
) -> Result<MergeCicdGateResult<T>, Box<dyn std::error::Error>>
where
    F: FnMut(MergeCicdFixRequest) -> Fut,
    Fut: Future<Output = Result<Option<T>, Box<dyn std::error::Error>>>,
{
    let repo_root = repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    run_merge_cicd_gate_at_repo(
        policy,
        auto_fix_backend_ready,
        &repo_root,
        &mut attempt_auto_fix,
    )
    .await
}

async fn run_merge_cicd_gate_at_repo<T, F, Fut>(
    policy: &MergeCicdGatePolicy,
    auto_fix_backend_ready: bool,
    repo_root: &Path,
    attempt_auto_fix: &mut F,
) -> Result<MergeCicdGateResult<T>, Box<dyn std::error::Error>>
where
    F: FnMut(MergeCicdFixRequest) -> Fut,
    Fut: Future<Output = Result<Option<T>, Box<dyn std::error::Error>>>,
{
    let Some(script) = policy.script.as_ref() else {
        return Ok(MergeCicdGateResult::Skipped);
    };

    let mut attempts: u32 = 0;
    let mut fix_attempts: u32 = 0;
    let mut fix_records = Vec::new();

    loop {
        attempts += 1;
        let result = run_cicd_script(script, repo_root)?;
        log_cicd_result(script, &result, attempts);

        if result.success() {
            return Ok(MergeCicdGateResult::Passed(MergeCicdGateOutcome {
                script: script.clone(),
                attempts,
                fixes: fix_records,
                result,
            }));
        }

        if !policy.auto_resolve || !policy.retry_path_enabled {
            if policy.auto_resolve && !policy.retry_path_enabled {
                display::warn(
                    "CI/CD gate auto-remediation is configured but template on.failed does not route through a node that returns to the gate.",
                );
            }
            return Ok(MergeCicdGateResult::Failed(MergeCicdGateFailure {
                script: script.clone(),
                attempts,
                fixes: fix_records,
                result,
            }));
        }

        if !auto_fix_backend_ready {
            display::warn(
                "CI/CD gate auto-remediation requires an agent-style backend; skipping automatic fixes.",
            );
            return Ok(MergeCicdGateResult::Failed(MergeCicdGateFailure {
                script: script.clone(),
                attempts,
                fixes: fix_records,
                result,
            }));
        }

        if fix_attempts >= policy.retries {
            display::warn(format!(
                "CI/CD auto-remediation exhausted its retry budget ({} attempt(s)).",
                policy.retries
            ));
            return Ok(MergeCicdGateResult::Failed(MergeCicdGateFailure {
                script: script.clone(),
                attempts,
                fixes: fix_records,
                result,
            }));
        }

        fix_attempts += 1;
        display::info(format!(
            "CI/CD gate failed; attempting backend remediation ({}/{})...",
            fix_attempts, policy.retries
        ));
        let request = MergeCicdFixRequest {
            script: script.clone(),
            attempt: fix_attempts,
            max_attempts: policy.retries,
            exit_code: result.status.code(),
            stdout: clip_log(result.stdout.as_bytes()),
            stderr: clip_log(result.stderr.as_bytes()),
        };
        match attempt_auto_fix(request).await? {
            Some(record) => fix_records.push(record),
            None => display::info("Backend remediation reported no file changes."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };
    use tempfile::TempDir;
    use vizier_core::workflow_template::{
        WorkflowGatePolicy, WorkflowNode, WorkflowOutcomeArtifacts, WorkflowRetryPolicy,
        WorkflowTemplate, WorkflowTemplatePolicy,
    };

    fn write_script(
        root: &Path,
        name: &str,
        body: &str,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let path = root.join(name);
        std::fs::write(&path, body)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms)?;
        }
        Ok(path)
    }

    fn approve_template_with_stop_gate(
        script: &str,
        retries: u32,
        retry_edge: bool,
    ) -> WorkflowTemplate {
        let mut stop_on = WorkflowOutcomeEdges::default();
        if retry_edge {
            stop_on.failed.push(APPROVE_APPLY_NODE_ID.to_string());
        }
        WorkflowTemplate {
            id: "template.approve".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: Vec::new(),
            nodes: vec![
                WorkflowNode {
                    id: APPROVE_APPLY_NODE_ID.to_string(),
                    kind: WorkflowNodeKind::Builtin,
                    uses: "vizier.approve.apply_once".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: vec![],
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges {
                        succeeded: vec![APPROVE_STOP_GATE_NODE_ID.to_string()],
                        ..Default::default()
                    },
                },
                WorkflowNode {
                    id: APPROVE_STOP_GATE_NODE_ID.to_string(),
                    kind: WorkflowNodeKind::Gate,
                    uses: "vizier.approve.stop_condition".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: vec![WorkflowGate::Script {
                        script: script.to_string(),
                        policy: WorkflowGatePolicy::Retry,
                    }],
                    retry: WorkflowRetryPolicy {
                        mode: WorkflowRetryMode::UntilGate,
                        budget: retries,
                    },
                    on: stop_on,
                },
            ],
        }
    }

    fn approve_template_with_custom_node_ids(script: &str, retries: u32) -> WorkflowTemplate {
        let apply_id = "apply_custom";
        let stop_id = "stop_custom";
        WorkflowTemplate {
            id: "template.approve".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: Vec::new(),
            nodes: vec![
                WorkflowNode {
                    id: apply_id.to_string(),
                    kind: WorkflowNodeKind::Builtin,
                    uses: "vizier.approve.apply_once".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: vec![],
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges {
                        succeeded: vec!["approve_terminal".to_string(), stop_id.to_string()],
                        ..Default::default()
                    },
                },
                WorkflowNode {
                    id: stop_id.to_string(),
                    kind: WorkflowNodeKind::Gate,
                    uses: "vizier.approve.stop_condition".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: vec![WorkflowGate::Script {
                        script: script.to_string(),
                        policy: WorkflowGatePolicy::Retry,
                    }],
                    retry: WorkflowRetryPolicy {
                        mode: WorkflowRetryMode::UntilGate,
                        budget: retries,
                    },
                    on: WorkflowOutcomeEdges {
                        failed: vec![apply_id.to_string()],
                        ..Default::default()
                    },
                },
                WorkflowNode {
                    id: "approve_terminal".to_string(),
                    kind: WorkflowNodeKind::Builtin,
                    uses: "vizier.approve.terminal".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: Vec::new(),
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges::default(),
                },
            ],
        }
    }

    fn merge_template_with_gate(
        script: &str,
        retries: u32,
        auto_resolve: bool,
        with_retry_path: bool,
    ) -> WorkflowTemplate {
        let mut gate_on = WorkflowOutcomeEdges::default();
        let mut auto_fix_nodes = Vec::new();
        if auto_resolve {
            gate_on.failed.push(MERGE_AUTO_FIX_NODE_ID.to_string());
            let auto_fix_on = if with_retry_path {
                WorkflowOutcomeEdges {
                    succeeded: vec![MERGE_GATE_NODE_ID.to_string()],
                    ..Default::default()
                }
            } else {
                WorkflowOutcomeEdges::default()
            };
            auto_fix_nodes.push(WorkflowNode {
                id: MERGE_AUTO_FIX_NODE_ID.to_string(),
                kind: WorkflowNodeKind::Agent,
                uses: "vizier.merge.cicd_auto_fix".to_string(),
                args: BTreeMap::new(),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: WorkflowRetryPolicy::default(),
                on: auto_fix_on,
            });
        }
        let mut nodes = vec![
            WorkflowNode {
                id: MERGE_INTEGRATE_NODE_ID.to_string(),
                kind: WorkflowNodeKind::Builtin,
                uses: "vizier.merge.integrate".to_string(),
                args: BTreeMap::new(),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: WorkflowRetryPolicy::default(),
                on: WorkflowOutcomeEdges {
                    succeeded: vec![MERGE_GATE_NODE_ID.to_string()],
                    ..Default::default()
                },
            },
            WorkflowNode {
                id: MERGE_GATE_NODE_ID.to_string(),
                kind: WorkflowNodeKind::Gate,
                uses: "vizier.merge.cicd_gate".to_string(),
                args: BTreeMap::new(),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: vec![WorkflowGate::Cicd {
                    script: script.to_string(),
                    auto_resolve,
                    policy: WorkflowGatePolicy::Retry,
                }],
                retry: WorkflowRetryPolicy {
                    mode: WorkflowRetryMode::UntilGate,
                    budget: retries,
                },
                on: gate_on,
            },
        ];
        nodes.extend(auto_fix_nodes);
        WorkflowTemplate {
            id: "template.merge".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: Vec::new(),
            nodes,
        }
    }

    fn merge_template_with_custom_gate_ids(script: &str, retries: u32) -> WorkflowTemplate {
        let gate_id = "gate_custom";
        WorkflowTemplate {
            id: "template.merge".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: Vec::new(),
            nodes: vec![
                WorkflowNode {
                    id: "integrate_custom".to_string(),
                    kind: WorkflowNodeKind::Builtin,
                    uses: "vizier.merge.integrate".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: Vec::new(),
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges {
                        succeeded: vec![gate_id.to_string()],
                        ..Default::default()
                    },
                },
                WorkflowNode {
                    id: gate_id.to_string(),
                    kind: WorkflowNodeKind::Gate,
                    uses: "vizier.merge.cicd_gate".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: vec![WorkflowGate::Cicd {
                        script: script.to_string(),
                        auto_resolve: true,
                        policy: WorkflowGatePolicy::Retry,
                    }],
                    retry: WorkflowRetryPolicy {
                        mode: WorkflowRetryMode::UntilGate,
                        budget: retries,
                    },
                    on: WorkflowOutcomeEdges {
                        failed: vec!["fix_custom".to_string()],
                        ..Default::default()
                    },
                },
                WorkflowNode {
                    id: "fix_custom".to_string(),
                    kind: WorkflowNodeKind::Agent,
                    uses: "vizier.merge.cicd_auto_fix".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: Vec::new(),
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges {
                        succeeded: vec![gate_id.to_string()],
                        ..Default::default()
                    },
                },
            ],
        }
    }

    fn merge_template_with_conflict_gate(auto_resolve: bool, retry_path: bool) -> WorkflowTemplate {
        let conflict_on = if retry_path {
            WorkflowOutcomeEdges {
                succeeded: vec![MERGE_INTEGRATE_NODE_ID.to_string()],
                ..Default::default()
            }
        } else {
            WorkflowOutcomeEdges::default()
        };
        WorkflowTemplate {
            id: "template.merge".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: Vec::new(),
            nodes: vec![
                WorkflowNode {
                    id: MERGE_INTEGRATE_NODE_ID.to_string(),
                    kind: WorkflowNodeKind::Builtin,
                    uses: "vizier.merge.integrate".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: Vec::new(),
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges {
                        blocked: vec![MERGE_CONFLICT_NODE_ID.to_string()],
                        ..Default::default()
                    },
                },
                WorkflowNode {
                    id: MERGE_CONFLICT_NODE_ID.to_string(),
                    kind: WorkflowNodeKind::Gate,
                    uses: "vizier.merge.conflict_resolution".to_string(),
                    args: BTreeMap::from([("auto_resolve".to_string(), auto_resolve.to_string())]),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: vec![WorkflowGate::Custom {
                        id: "conflict_resolution".to_string(),
                        policy: WorkflowGatePolicy::Block,
                        args: BTreeMap::from([(
                            "auto_resolve".to_string(),
                            auto_resolve.to_string(),
                        )]),
                    }],
                    retry: WorkflowRetryPolicy::default(),
                    on: conflict_on,
                },
            ],
        }
    }

    fn merge_template_with_custom_conflict_ids(auto_resolve: bool) -> WorkflowTemplate {
        let integrate_id = "merge_integrate_custom";
        let conflict_id = "merge_conflict_custom";
        WorkflowTemplate {
            id: "template.merge".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            policy: WorkflowTemplatePolicy::default(),
            artifact_contracts: Vec::new(),
            nodes: vec![
                WorkflowNode {
                    id: integrate_id.to_string(),
                    kind: WorkflowNodeKind::Builtin,
                    uses: "vizier.merge.integrate".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: Vec::new(),
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges {
                        blocked: vec![conflict_id.to_string(), "merge_terminal".to_string()],
                        ..Default::default()
                    },
                },
                WorkflowNode {
                    id: conflict_id.to_string(),
                    kind: WorkflowNodeKind::Gate,
                    uses: "vizier.merge.conflict_resolution".to_string(),
                    args: BTreeMap::from([("auto_resolve".to_string(), auto_resolve.to_string())]),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: vec![WorkflowGate::Custom {
                        id: "conflict_resolution".to_string(),
                        policy: WorkflowGatePolicy::Block,
                        args: BTreeMap::from([(
                            "auto_resolve".to_string(),
                            auto_resolve.to_string(),
                        )]),
                    }],
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges {
                        succeeded: vec![integrate_id.to_string()],
                        ..Default::default()
                    },
                },
                WorkflowNode {
                    id: "merge_terminal".to_string(),
                    kind: WorkflowNodeKind::Builtin,
                    uses: "vizier.merge.terminal".to_string(),
                    args: BTreeMap::new(),
                    after: Vec::new(),
                    needs: Vec::new(),
                    produces: WorkflowOutcomeArtifacts::default(),
                    locks: Vec::new(),
                    preconditions: Vec::new(),
                    gates: Vec::new(),
                    retry: WorkflowRetryPolicy::default(),
                    on: WorkflowOutcomeEdges::default(),
                },
            ],
        }
    }

    #[test]
    fn approve_stop_condition_policy_uses_until_gate_budget()
    -> Result<(), Box<dyn std::error::Error>> {
        let template = approve_template_with_stop_gate("./scripts/stop.sh", 4, true);
        let policy = approve_stop_condition_policy(&template)?;
        assert_eq!(
            policy
                .script
                .as_ref()
                .map(|path| path.display().to_string()),
            Some("./scripts/stop.sh".to_string())
        );
        assert_eq!(policy.retries, 4);
        assert!(policy.retry_path_enabled);
        Ok(())
    }

    #[test]
    fn approve_stop_condition_policy_disables_retry_when_on_failed_is_not_wired()
    -> Result<(), Box<dyn std::error::Error>> {
        let template = approve_template_with_stop_gate("./scripts/stop.sh", 4, false);
        let policy = approve_stop_condition_policy(&template)?;
        assert!(!policy.retry_path_enabled);
        Ok(())
    }

    #[test]
    fn approve_stop_condition_policy_uses_custom_node_ids_and_multi_target_edges()
    -> Result<(), Box<dyn std::error::Error>> {
        let template = approve_template_with_custom_node_ids("./scripts/stop.sh", 5);
        let policy = approve_stop_condition_policy(&template)?;
        assert_eq!(
            policy
                .script
                .as_ref()
                .map(|path| path.display().to_string()),
            Some("./scripts/stop.sh".to_string())
        );
        assert_eq!(policy.retries, 5);
        assert!(policy.retry_path_enabled);
        Ok(())
    }

    #[test]
    fn approve_runtime_retries_until_stop_condition_passes()
    -> Result<(), Box<dyn std::error::Error>> {
        let worktree = TempDir::new()?;
        let script = write_script(
            worktree.path(),
            "stop.sh",
            "#!/bin/sh\nset -eu\ncounter=\"$PWD/stop-counter\"\ncount=0\nif [ -f \"$counter\" ]; then\n  count=$(cat \"$counter\")\nfi\ncount=$((count + 1))\nprintf '%s\\n' \"$count\" > \"$counter\"\nif [ \"$count\" -lt 2 ]; then\n  exit 1\nfi\n",
        )?;
        let policy = ApproveStopConditionPolicy {
            script: Some(script),
            retries: 2,
            retry_path_enabled: true,
        };
        let apply_calls = Arc::new(AtomicU32::new(0));
        let apply_calls_ref = apply_calls.clone();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        let outcome = runtime.block_on(run_approve_apply_with_stop_condition(
            worktree.path(),
            &policy,
            move || {
                let apply_calls_ref = apply_calls_ref.clone();
                async move {
                    apply_calls_ref.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, Box<dyn std::error::Error>>("applied".to_string())
                }
            },
        ));

        match outcome {
            ApproveApplyOutcome::Succeeded { value, report } => {
                assert_eq!(value, "applied");
                assert_eq!(report.status, "passed");
                assert_eq!(report.attempts, 2);
            }
            ApproveApplyOutcome::Failed { error, .. } => {
                return Err(format!("expected stop-condition retry success, got: {error}").into());
            }
        }

        assert_eq!(apply_calls.load(Ordering::SeqCst), 2);
        Ok(())
    }

    #[test]
    fn review_cicd_gate_policy_extracts_single_gate() -> Result<(), Box<dyn std::error::Error>> {
        let gates = vec![WorkflowGate::Cicd {
            script: "./scripts/cicd.sh".to_string(),
            auto_resolve: true,
            policy: WorkflowGatePolicy::Warn,
        }];
        let policy = review_cicd_gate_policy(&gates)?;
        assert_eq!(
            policy
                .script
                .as_ref()
                .map(|path| path.display().to_string()),
            Some("./scripts/cicd.sh".to_string())
        );
        assert!(policy.auto_resolve);
        Ok(())
    }

    #[test]
    fn review_cicd_gate_policy_rejects_multiple_gates() {
        let gates = vec![
            WorkflowGate::Cicd {
                script: "./scripts/first.sh".to_string(),
                auto_resolve: false,
                policy: WorkflowGatePolicy::Warn,
            },
            WorkflowGate::Cicd {
                script: "./scripts/second.sh".to_string(),
                auto_resolve: false,
                policy: WorkflowGatePolicy::Warn,
            },
        ];
        let error =
            review_cicd_gate_policy(&gates).expect_err("expected policy validation failure");
        assert!(
            error.to_string().contains("multiple cicd gates"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn merge_runtime_retries_gate_after_auto_fix() -> Result<(), Box<dyn std::error::Error>> {
        let repo = TempDir::new()?;
        let script = write_script(
            repo.path(),
            "cicd.sh",
            "#!/bin/sh\nset -eu\nif [ -f \"$PWD/ci-fixed.txt\" ]; then\n  exit 0\nfi\necho \"gate failed\" >&2\nexit 1\n",
        )?;
        let template = merge_template_with_gate(&script.display().to_string(), 2, true, true);
        let policy = merge_cicd_gate_policy(&template)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        let repo_root = repo.path().to_path_buf();
        let fix_calls = Arc::new(AtomicU32::new(0));
        let fix_calls_ref = fix_calls.clone();
        let mut fix = move |_request: MergeCicdFixRequest| {
            let repo_root = repo_root.clone();
            let fix_calls_ref = fix_calls_ref.clone();
            async move {
                fix_calls_ref.fetch_add(1, Ordering::SeqCst);
                std::fs::write(repo_root.join("ci-fixed.txt"), "ok\n")?;
                Ok::<_, Box<dyn std::error::Error>>(Some("fixed".to_string()))
            }
        };

        let outcome = runtime.block_on(run_merge_cicd_gate_at_repo(
            &policy,
            true,
            repo.path(),
            &mut fix,
        ))?;
        let outcome = match outcome {
            MergeCicdGateResult::Passed(outcome) => outcome,
            other => return Err(format!("expected merge gate pass outcome, got {other:?}").into()),
        };
        assert_eq!(outcome.attempts, 2);
        assert_eq!(outcome.fixes, vec!["fixed".to_string()]);
        assert!(outcome.result.success(), "final gate result should succeed");
        assert_eq!(fix_calls.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn merge_policy_disables_retry_when_auto_fix_on_success_does_not_return_to_gate()
    -> Result<(), Box<dyn std::error::Error>> {
        let template = merge_template_with_gate("./scripts/cicd.sh", 2, true, false);
        let policy = merge_cicd_gate_policy(&template)?;
        assert!(policy.auto_resolve);
        assert!(!policy.retry_path_enabled);
        Ok(())
    }

    #[test]
    fn merge_policy_supports_custom_gate_and_auto_fix_node_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        let template = merge_template_with_custom_gate_ids("./scripts/cicd.sh", 3);
        let policy = merge_cicd_gate_policy(&template)?;
        assert!(policy.auto_resolve);
        assert_eq!(policy.retries, 3);
        assert!(policy.retry_path_enabled);
        Ok(())
    }

    #[test]
    fn merge_conflict_policy_extracts_auto_resolve_and_retry_edge()
    -> Result<(), Box<dyn std::error::Error>> {
        let template = merge_template_with_conflict_gate(true, true);
        let policy = merge_conflict_resolution_policy(&template)?;
        assert!(policy.auto_resolve);
        assert!(policy.retry_path_enabled);
        Ok(())
    }

    #[test]
    fn merge_conflict_policy_defaults_to_manual_without_conflict_gate()
    -> Result<(), Box<dyn std::error::Error>> {
        let template = merge_template_with_gate("./scripts/cicd.sh", 1, false, false);
        let policy = merge_conflict_resolution_policy(&template)?;
        assert!(!policy.auto_resolve);
        assert!(!policy.retry_path_enabled);
        Ok(())
    }

    #[test]
    fn merge_conflict_policy_supports_custom_node_ids_and_blocked_fanout()
    -> Result<(), Box<dyn std::error::Error>> {
        let template = merge_template_with_custom_conflict_ids(true);
        let policy = merge_conflict_resolution_policy(&template)?;
        assert!(policy.auto_resolve);
        assert!(policy.retry_path_enabled);
        Ok(())
    }
}
