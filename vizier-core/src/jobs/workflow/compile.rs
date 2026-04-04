use super::*;

pub(crate) fn sanitize_workflow_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    while out.starts_with('-') {
        out.remove(0);
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "node".to_string()
    } else {
        out
    }
}

pub(crate) fn workflow_job_id(run_id: &str, node_id: &str) -> String {
    format!(
        "wf-{}-{}",
        sanitize_workflow_component(run_id),
        sanitize_workflow_component(node_id)
    )
}

pub(crate) fn dedup_artifacts_preserve_order(artifacts: Vec<JobArtifact>) -> Vec<JobArtifact> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for artifact in artifacts {
        if seen.insert(artifact.clone()) {
            deduped.push(artifact);
        }
    }
    deduped
}

pub(crate) fn emit_workflow_node_lifecycle_line(
    node: &WorkflowRuntimeNodeManifest,
    stage: &str,
    message: impl AsRef<str>,
) -> String {
    let operation = node
        .executor_operation
        .as_deref()
        .or(node.control_policy.as_deref())
        .unwrap_or("unknown");
    let line = format!(
        "[workflow-node] {stage} node={} uses={} op={} {}",
        node.node_id,
        node.uses,
        operation,
        message.as_ref()
    );
    eprintln!("{line}");
    line
}

pub(crate) fn stderr_lines_from_text(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect()
}

pub(crate) fn print_stdout_text(text: &str) {
    if !text.is_empty() {
        print!("{text}");
        let _ = io::stdout().flush();
    }
}

pub(crate) fn print_stderr_text(text: &str) {
    if !text.is_empty() {
        eprint!("{text}");
        let _ = io::stderr().flush();
    }
}

pub(crate) fn workflow_node_artifacts_by_outcome(
    template: &WorkflowTemplate,
    node_id: &str,
) -> Result<WorkflowOutcomeArtifactsByOutcome, String> {
    let node = template
        .nodes
        .iter()
        .find(|entry| entry.id == node_id)
        .ok_or_else(|| format!("template node `{node_id}` is missing"))?;
    let operation_output = workflow_operation_output_artifact(node_id);

    let mut succeeded = node.produces.succeeded.clone();
    succeeded.push(operation_output.clone());
    let mut failed = node.produces.failed.clone();
    failed.push(operation_output.clone());
    let mut blocked = node.produces.blocked.clone();
    blocked.push(operation_output.clone());
    let mut cancelled = node.produces.cancelled.clone();
    cancelled.push(operation_output);

    Ok(WorkflowOutcomeArtifactsByOutcome {
        succeeded: dedup_artifacts_preserve_order(succeeded),
        failed: dedup_artifacts_preserve_order(failed),
        blocked: dedup_artifacts_preserve_order(blocked),
        cancelled: dedup_artifacts_preserve_order(cancelled),
    })
}

pub(crate) fn to_job_preconditions(
    node_id: &str,
    preconditions: &[WorkflowPrecondition],
) -> Result<Vec<JobPrecondition>, Box<dyn std::error::Error>> {
    let mut converted = Vec::new();
    for precondition in preconditions {
        match precondition {
            WorkflowPrecondition::CleanWorktree => converted.push(JobPrecondition::CleanWorktree),
            WorkflowPrecondition::BranchExists => {
                converted.push(JobPrecondition::BranchExists { branch: None })
            }
            WorkflowPrecondition::Custom { id, args } => converted.push(JobPrecondition::Custom {
                id: id.clone(),
                args: args.clone(),
            }),
            WorkflowPrecondition::PinnedHead => {
                return Err(format!(
                    "workflow node `{node_id}` uses unsupported scheduler precondition `pinned_head`"
                )
                .into());
            }
        }
    }
    Ok(converted)
}

pub(crate) fn to_schedule_approval(
    gates: &[crate::workflow_template::WorkflowGate],
) -> Option<JobApproval> {
    for gate in gates {
        if let crate::workflow_template::WorkflowGate::Approval { required, .. } = gate
            && *required
        {
            return Some(pending_job_approval());
        }
    }
    None
}

pub(crate) fn workflow_gate_summary(gate: &crate::workflow_template::WorkflowGate) -> String {
    match gate {
        crate::workflow_template::WorkflowGate::Approval { required, .. } => {
            format!("approval(required={required})")
        }
        crate::workflow_template::WorkflowGate::Script { script, .. } => {
            format!("script({script})")
        }
        crate::workflow_template::WorkflowGate::Cicd {
            script,
            auto_resolve,
            ..
        } => format!("cicd(script={script}, auto_resolve={auto_resolve})"),
        crate::workflow_template::WorkflowGate::Custom { id, .. } => {
            format!("custom({id})")
        }
    }
}

pub(crate) fn append_unique_after(
    after: &mut Vec<JobAfterDependency>,
    dependency: JobAfterDependency,
) {
    if after
        .iter()
        .any(|entry| entry.job_id == dependency.job_id && entry.policy == dependency.policy)
    {
        return;
    }
    after.push(dependency);
}

pub(crate) fn write_workflow_run_manifest(
    project_root: &Path,
    manifest: &WorkflowRunManifest,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = workflow_run_manifest_path(project_root, &manifest.run_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(manifest)?)?;
    Ok(())
}

pub(crate) fn load_workflow_run_manifest(
    project_root: &Path,
    run_id: &str,
) -> Result<WorkflowRunManifest, Box<dyn std::error::Error>> {
    let path = workflow_run_manifest_path(project_root, run_id);
    let bytes = fs::read(path)?;
    Ok(serde_json::from_slice::<WorkflowRunManifest>(&bytes)?)
}

fn collect_ephemeral_baseline_paths(
    project_root: &Path,
    root: &Path,
    paths: &mut Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !root.exists() {
        return Ok(());
    }

    paths.push(relative_path(project_root, root));
    if !root.is_dir() {
        return Ok(());
    }

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        collect_ephemeral_baseline_paths(project_root, &entry.path(), paths)?;
    }

    Ok(())
}

fn collect_ephemeral_run_baseline(
    project_root: &Path,
    vizier_root_existed_before_runtime: Option<bool>,
) -> Result<EphemeralRunBaseline, Box<dyn std::error::Error>> {
    let vizier_root = project_root.join(".vizier");
    let mut preexisting_paths = Vec::new();
    for rel in [
        ".vizier/narrative",
        ".vizier/implementation-plans",
        ".vizier/tmp",
    ] {
        collect_ephemeral_baseline_paths(
            project_root,
            &project_root.join(rel),
            &mut preexisting_paths,
        )?;
    }
    preexisting_paths.sort();
    preexisting_paths.dedup();
    Ok(EphemeralRunBaseline {
        vizier_root_existed: vizier_root_existed_before_runtime
            .unwrap_or_else(|| vizier_root.exists()),
        preexisting_paths,
    })
}

#[derive(Debug, Clone, Default)]
pub struct WorkflowRunEnqueueOptions {
    pub ephemeral: bool,
    pub vizier_root_existed_before_runtime: Option<bool>,
}

#[derive(Debug)]
pub(crate) struct WorkflowRunCompilation {
    incoming_success: BTreeMap<String, Vec<String>>,
    compiled_nodes: BTreeMap<String, CompiledWorkflowNode>,
}

pub(crate) fn compile_workflow_run_nodes_with_resolved_after(
    template: &WorkflowTemplate,
    resolved_after: &BTreeMap<String, String>,
) -> Result<WorkflowRunCompilation, Box<dyn std::error::Error>> {
    validate_workflow_capability_contracts(template)?;
    if template.nodes.is_empty() {
        return Err("workflow template has no nodes".into());
    }

    let mut incoming_success = BTreeMap::<String, Vec<String>>::new();
    for source in &template.nodes {
        for target in &source.on.succeeded {
            incoming_success
                .entry(target.clone())
                .or_default()
                .push(source.id.clone());
        }
    }

    for (target, parents) in &incoming_success {
        if parents.len() > 1 {
            let list = parents.join(", ");
            return Err(format!(
                "template {}@{} node `{}` has multiple on.succeeded parents ({list}); runtime bridge currently requires a single parent",
                template.id, template.version, target
            )
            .into());
        }
    }

    let mut compiled_nodes = BTreeMap::new();
    for node in &template.nodes {
        let node_compiled = compile_workflow_node(template, &node.id, resolved_after)?;
        compiled_nodes.insert(node.id.clone(), node_compiled);
    }
    Ok(WorkflowRunCompilation {
        incoming_success,
        compiled_nodes,
    })
}

pub(crate) fn compile_workflow_run_nodes_for_preflight(
    template: &WorkflowTemplate,
) -> Result<WorkflowRunCompilation, Box<dyn std::error::Error>> {
    let mut resolved_after = BTreeMap::new();
    for node in &template.nodes {
        resolved_after.insert(node.id.clone(), format!("preflight-{}", node.id));
    }
    compile_workflow_run_nodes_with_resolved_after(template, &resolved_after)
}

pub fn validate_workflow_run_template(
    template: &WorkflowTemplate,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = compile_workflow_run_nodes_for_preflight(template)?;
    Ok(())
}

pub fn audit_workflow_run_template(
    template: &WorkflowTemplate,
) -> Result<WorkflowAuditReport, Box<dyn std::error::Error>> {
    let compilation = compile_workflow_run_nodes_for_preflight(template)?;
    let effective_locks = compilation
        .compiled_nodes
        .into_iter()
        .map(|(node_id, node)| (node_id, node.locks))
        .collect::<BTreeMap<_, _>>();
    Ok(analyze_workflow_template_with_effective_locks(
        template,
        &effective_locks,
    ))
}

pub(crate) fn convert_outcome_edges_to_routes(
    edges: &WorkflowOutcomeEdges,
) -> WorkflowRouteTargets {
    let to_targets = |values: &[String], mode: WorkflowRouteMode| {
        values
            .iter()
            .map(|target| WorkflowRouteTarget {
                node_id: target.clone(),
                mode,
            })
            .collect::<Vec<_>>()
    };

    WorkflowRouteTargets {
        succeeded: to_targets(&edges.succeeded, WorkflowRouteMode::PropagateContext),
        failed: to_targets(&edges.failed, WorkflowRouteMode::RetryJob),
        blocked: to_targets(&edges.blocked, WorkflowRouteMode::RetryJob),
        cancelled: to_targets(&edges.cancelled, WorkflowRouteMode::RetryJob),
    }
}

pub fn enqueue_workflow_run(
    project_root: &Path,
    jobs_root: &Path,
    run_id: &str,
    template_selector: &str,
    template: &WorkflowTemplate,
    recorded_args: &[String],
    config_snapshot: Option<serde_json::Value>,
) -> Result<EnqueueWorkflowRunResult, Box<dyn std::error::Error>> {
    enqueue_workflow_run_with_options(
        project_root,
        jobs_root,
        run_id,
        template_selector,
        template,
        recorded_args,
        config_snapshot,
        WorkflowRunEnqueueOptions::default(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn enqueue_workflow_run_with_options(
    project_root: &Path,
    jobs_root: &Path,
    run_id: &str,
    template_selector: &str,
    template: &WorkflowTemplate,
    recorded_args: &[String],
    config_snapshot: Option<serde_json::Value>,
    options: WorkflowRunEnqueueOptions,
) -> Result<EnqueueWorkflowRunResult, Box<dyn std::error::Error>> {
    let mut node_to_job_id = BTreeMap::new();
    for node in &template.nodes {
        let job_id = workflow_job_id(run_id, &node.id);
        if node_to_job_id.insert(node.id.clone(), job_id).is_some() {
            return Err(format!("duplicate workflow node id `{}`", node.id).into());
        }
    }

    let ephemeral_baseline = if options.ephemeral {
        Some(collect_ephemeral_run_baseline(
            project_root,
            options.vizier_root_existed_before_runtime,
        )?)
    } else {
        None
    };

    let mut resolved_after = BTreeMap::new();
    for (node_id, job_id) in &node_to_job_id {
        resolved_after.insert(node_id.clone(), job_id.clone());
    }

    let compilation = compile_workflow_run_nodes_with_resolved_after(template, &resolved_after)?;
    let incoming_success = &compilation.incoming_success;

    let policy_snapshot_hash = template.policy_snapshot().stable_hash_hex()?;
    let mut manifest_nodes = BTreeMap::new();

    for node in &template.nodes {
        let compiled = compilation
            .compiled_nodes
            .get(&node.id)
            .ok_or_else(|| format!("missing compiled node for `{}`", node.id))?;
        let job_id = node_to_job_id
            .get(&node.id)
            .cloned()
            .ok_or_else(|| format!("missing job id for node {}", node.id))?;

        let mut after = compiled.after.clone();
        if let Some(parents) = incoming_success.get(&node.id)
            && let Some(parent) = parents.first()
            && let Some(parent_job_id) = node_to_job_id.get(parent)
        {
            append_unique_after(
                &mut after,
                JobAfterDependency {
                    job_id: parent_job_id.clone(),
                    policy: AfterPolicy::Success,
                },
            );
        }
        sort_after_dependencies(&mut after);
        after.dedup();

        let preconditions = to_job_preconditions(&node.id, &compiled.preconditions)?;
        let approval = to_schedule_approval(&compiled.gates);
        let dependencies = compiled
            .dependencies
            .iter()
            .map(|artifact| JobDependency {
                artifact: artifact.clone(),
            })
            .collect::<Vec<_>>();
        let mut schedule_artifacts = compiled.artifacts.clone();
        schedule_artifacts.push(workflow_operation_output_artifact(&node.id));
        schedule_artifacts = dedup_job_artifacts(schedule_artifacts);

        let schedule = JobSchedule {
            after,
            dependencies,
            dependency_policy: JobDependenciesPolicy {
                missing_producer: template.policy.dependencies.missing_producer,
            },
            locks: compiled.locks.clone(),
            artifacts: schedule_artifacts,
            pinned_head: None,
            preconditions,
            approval,
            wait_reason: None,
            waited_on: Vec::new(),
        };

        let metadata = JobMetadata {
            ephemeral_run: options.ephemeral.then_some(true),
            ephemeral_cleanup_requested: options.ephemeral.then_some(true),
            ephemeral_cleanup_state: options.ephemeral.then_some(EphemeralCleanupState::Pending),
            workflow_run_id: Some(run_id.to_string()),
            workflow_node_name: compiled.name.clone(),
            workflow_node_attempt: Some(1),
            workflow_template_selector: Some(template_selector.to_string()),
            workflow_template_id: Some(compiled.template_id.clone()),
            workflow_template_version: Some(compiled.template_version.clone()),
            workflow_node_id: Some(compiled.node_id.clone()),
            workflow_executor_class: compiled
                .executor_class
                .map(|value| value.as_str().to_string()),
            workflow_executor_operation: compiled.executor_operation.clone(),
            workflow_control_policy: compiled.control_policy.clone(),
            workflow_policy_snapshot_hash: Some(compiled.policy_snapshot_hash.clone()),
            workflow_gates: if compiled.gates.is_empty() {
                None
            } else {
                Some(
                    compiled
                        .gates
                        .iter()
                        .map(workflow_gate_summary)
                        .collect::<Vec<_>>(),
                )
            },
            ..JobMetadata::default()
        };

        let child_args = vec![
            "__workflow-node".to_string(),
            "--job-id".to_string(),
            job_id.clone(),
        ];
        let command = if recorded_args.is_empty() {
            vec![
                "vizier".to_string(),
                "__workflow-node".to_string(),
                "--job-id".to_string(),
                job_id.clone(),
            ]
        } else {
            recorded_args.to_vec()
        };
        enqueue_job(
            project_root,
            jobs_root,
            &job_id,
            &child_args,
            &command,
            Some(metadata),
            config_snapshot.clone(),
            Some(schedule),
        )?;

        manifest_nodes.insert(
            node.id.clone(),
            WorkflowRuntimeNodeManifest {
                node_id: node.id.clone(),
                name: compiled.name.clone(),
                job_id: job_id.clone(),
                uses: node.uses.clone(),
                kind: node.kind,
                args: node.args.clone(),
                executor_operation: compiled.executor_operation.clone(),
                control_policy: compiled.control_policy.clone(),
                gates: compiled.gates.clone(),
                retry: compiled.retry.clone(),
                routes: convert_outcome_edges_to_routes(&compiled.on),
                artifacts_by_outcome: workflow_node_artifacts_by_outcome(template, &node.id)?,
            },
        );
    }

    write_workflow_run_manifest(
        project_root,
        &WorkflowRunManifest {
            run_id: run_id.to_string(),
            template_selector: template_selector.to_string(),
            template_id: template.id.clone(),
            template_version: template.version.clone(),
            policy_snapshot_hash: policy_snapshot_hash.clone(),
            ephemeral: options.ephemeral,
            ephemeral_cleanup_requested: options.ephemeral,
            ephemeral_cleanup_state: options.ephemeral.then_some(EphemeralCleanupState::Pending),
            ephemeral_cleanup_detail: None,
            ephemeral_baseline,
            nodes: manifest_nodes,
        },
    )?;

    Ok(EnqueueWorkflowRunResult {
        run_id: run_id.to_string(),
        template_selector: template_selector.to_string(),
        template_id: template.id.clone(),
        template_version: template.version.clone(),
        policy_snapshot_hash,
        ephemeral: options.ephemeral,
        job_ids: node_to_job_id,
    })
}

pub(crate) fn dedup_job_artifacts(mut artifacts: Vec<JobArtifact>) -> Vec<JobArtifact> {
    sort_artifacts(&mut artifacts);
    artifacts.dedup();
    artifacts
}
