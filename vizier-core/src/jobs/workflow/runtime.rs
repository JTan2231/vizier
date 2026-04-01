use super::*;

pub(crate) fn map_workflow_outcome_to_job_status(
    outcome: WorkflowNodeOutcome,
    exit_code: Option<i32>,
) -> (JobStatus, i32) {
    match outcome {
        WorkflowNodeOutcome::Succeeded => (JobStatus::Succeeded, exit_code.unwrap_or(0)),
        WorkflowNodeOutcome::Failed => (JobStatus::Failed, exit_code.unwrap_or(1)),
        WorkflowNodeOutcome::Blocked => (JobStatus::BlockedByDependency, exit_code.unwrap_or(10)),
        WorkflowNodeOutcome::Cancelled => (JobStatus::Cancelled, exit_code.unwrap_or(143)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkflowExecutionContext {
    pub(crate) execution_root: Option<String>,
    pub(crate) worktree_path: Option<String>,
    pub(crate) worktree_name: Option<String>,
    pub(crate) worktree_owned: Option<bool>,
}

pub(crate) fn normalized_metadata_value(value: Option<&String>) -> Option<String> {
    value
        .map(|entry| entry.trim())
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.to_string())
}

pub(crate) fn workflow_execution_context_from_metadata(
    metadata: Option<&JobMetadata>,
) -> Option<WorkflowExecutionContext> {
    let metadata = metadata?;
    let context = WorkflowExecutionContext {
        execution_root: normalized_metadata_value(metadata.execution_root.as_ref()),
        worktree_path: normalized_metadata_value(metadata.worktree_path.as_ref()),
        worktree_name: normalized_metadata_value(metadata.worktree_name.as_ref()),
        worktree_owned: metadata.worktree_owned,
    };
    if context.execution_root.is_none() && context.worktree_path.is_none() {
        None
    } else {
        Some(context)
    }
}

pub(crate) fn run_shell_text_command(
    execution_root: &Path,
    script: &str,
) -> Result<(i32, String, String), Box<dyn std::error::Error>> {
    let output = Command::new("sh")
        .arg("-lc")
        .arg(script)
        .current_dir(execution_root)
        .output()?;
    let status = output.status.code().unwrap_or(1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok((status, stdout, stderr))
}

pub(crate) fn parse_bool_like(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

pub(crate) fn bool_arg(args: &BTreeMap<String, String>, key: &str) -> Option<bool> {
    args.get(key).and_then(|value| parse_bool_like(value))
}

pub(crate) fn resolve_execution_root(
    project_root: &Path,
    record: &JobRecord,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let canonical_project_root = project_root.canonicalize().map_err(|err| {
        format!(
            "cannot resolve repository root {}: {}",
            project_root.display(),
            err
        )
    })?;
    let Some(metadata) = record.metadata.as_ref() else {
        return Ok(canonical_project_root);
    };

    if let Some(execution_root) = normalized_metadata_value(metadata.execution_root.as_ref()) {
        return resolve_execution_root_candidate(
            project_root,
            &canonical_project_root,
            &execution_root,
            "execution_root",
        );
    }

    if let Some(worktree_path) = normalized_metadata_value(metadata.worktree_path.as_ref()) {
        return resolve_execution_root_candidate(
            project_root,
            &canonical_project_root,
            &worktree_path,
            "worktree_path",
        );
    }

    Ok(canonical_project_root)
}

pub(crate) fn resolve_execution_root_candidate(
    project_root: &Path,
    canonical_project_root: &Path,
    recorded: &str,
    field_name: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let resolved = resolve_recorded_path(project_root, recorded);
    let canonical = resolved.canonicalize().map_err(|err| {
        format!(
            "workflow metadata.{field_name} path {} is invalid: {}",
            resolved.display(),
            err
        )
    })?;
    if !canonical.starts_with(canonical_project_root) {
        return Err(format!(
            "workflow metadata.{field_name} path {} is outside repository root {}",
            canonical.display(),
            canonical_project_root.display()
        )
        .into());
    }
    Ok(canonical)
}

pub(crate) fn resolve_path_in_execution_root(execution_root: &Path, path: &str) -> PathBuf {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        candidate
    } else {
        execution_root.join(candidate)
    }
}

pub(crate) fn first_non_empty_arg(
    args: &BTreeMap<String, String>,
    keys: &[&str],
) -> Option<String> {
    for key in keys {
        if let Some(value) = args.get(*key)
            && !value.trim().is_empty()
        {
            return Some(value.trim().to_string());
        }
    }
    None
}

pub(crate) fn resolve_node_shell_script(
    node: &WorkflowRuntimeNodeManifest,
    fallback: Option<String>,
) -> Option<String> {
    first_non_empty_arg(&node.args, &["command", "script"]).or(fallback)
}

pub(crate) fn script_gate_script(node: &WorkflowRuntimeNodeManifest) -> Option<String> {
    node.gates.iter().find_map(|gate| match gate {
        WorkflowGate::Script { script, .. } => {
            if script.trim().is_empty() {
                None
            } else {
                Some(script.trim().to_string())
            }
        }
        _ => None,
    })
}

pub(crate) fn cicd_gate_config(node: &WorkflowRuntimeNodeManifest) -> Option<(String, bool)> {
    node.gates.iter().find_map(|gate| match gate {
        WorkflowGate::Cicd {
            script,
            auto_resolve,
            ..
        } if !script.trim().is_empty() => Some((script.trim().to_string(), *auto_resolve)),
        _ => None,
    })
}

pub(crate) fn conflict_auto_resolve_from_gate(node: &WorkflowRuntimeNodeManifest) -> Option<bool> {
    for gate in &node.gates {
        if let WorkflowGate::Custom { id, args, .. } = gate
            && id == "conflict_resolution"
            && let Some(value) = args
                .get("auto_resolve")
                .and_then(|raw| parse_bool_like(raw))
        {
            return Some(value);
        }
    }
    None
}

pub(crate) fn workflow_slug_from_record(
    record: &JobRecord,
    node: &WorkflowRuntimeNodeManifest,
) -> String {
    if let Some(value) = first_non_empty_arg(&node.args, &["slug", "plan"]) {
        return value;
    }
    if let Some(value) = record
        .metadata
        .as_ref()
        .and_then(|meta| meta.plan.as_ref())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        return value.to_string();
    }
    if let Some(value) = record
        .metadata
        .as_ref()
        .and_then(|meta| meta.branch.as_ref())
        .and_then(|branch| branch.strip_prefix("draft/"))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        return value.to_string();
    }
    sanitize_workflow_component(&record.id)
}

pub(crate) fn merge_sentinel_path(project_root: &Path, slug: &str) -> PathBuf {
    project_root
        .join(".vizier/tmp/merge-conflicts")
        .join(format!("{slug}.json"))
}

pub(crate) fn ensure_local_branch(
    execution_root: &Path,
    branch: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if crate::vcs::branch_exists_in(execution_root, branch)? {
        return Ok(());
    }

    crate::vcs::create_branch_from_head_in(execution_root, branch)
        .map_err(|err| format!("unable to create local branch `{branch}`: {err}").into())
}

pub(crate) fn current_branch_name(execution_root: &Path) -> Option<String> {
    crate::vcs::current_branch_name_in(execution_root)
        .ok()
        .flatten()
}

pub(crate) fn has_unmerged_paths(execution_root: &Path) -> bool {
    !list_unmerged_paths(execution_root).is_empty()
}

pub(crate) fn list_unmerged_paths(execution_root: &Path) -> Vec<String> {
    crate::vcs::list_conflicted_paths_in(execution_root).unwrap_or_default()
}

pub(crate) fn parse_non_empty_json_string_field(
    value: &serde_json::Value,
    key: &str,
) -> Option<String> {
    value
        .get(key)
        .and_then(|raw| raw.as_str())
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
        .map(|raw| raw.to_string())
}

pub(crate) fn read_merge_sentinel_branches(sentinel: &Path) -> (Option<String>, Option<String>) {
    let Ok(raw) = fs::read_to_string(sentinel) else {
        return (None, None);
    };
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return (None, None);
    };
    (
        parse_non_empty_json_string_field(&payload, "source_branch"),
        parse_non_empty_json_string_field(&payload, "target_branch"),
    )
}

pub(crate) fn load_merge_conflict_companion_prompt(
    execution_root: &Path,
    node: &WorkflowRuntimeNodeManifest,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    if let Some(text) = first_non_empty_arg(&node.args, &["prompt_text"]) {
        return Ok(Some(text));
    }

    let Some(prompt_file) = first_non_empty_arg(&node.args, &["prompt_file"]) else {
        return Ok(None);
    };
    let abs = resolve_path_in_execution_root(execution_root, &prompt_file);
    let contents = fs::read_to_string(abs)?;
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

pub(crate) fn resolve_merge_conflict_branches(
    execution_root: &Path,
    record: &JobRecord,
    node: &WorkflowRuntimeNodeManifest,
    sentinel: &Path,
    slug: &str,
) -> (String, String) {
    let (sentinel_source, sentinel_target) = read_merge_sentinel_branches(sentinel);
    let source_branch = sentinel_source
        .or_else(|| first_non_empty_arg(&node.args, &["branch", "source_branch"]))
        .or_else(|| {
            record
                .metadata
                .as_ref()
                .and_then(|meta| meta.branch.clone())
        })
        .unwrap_or_else(|| format!("draft/{slug}"));
    let target_branch = sentinel_target
        .or_else(|| first_non_empty_arg(&node.args, &["target", "target_branch"]))
        .or_else(|| {
            record
                .metadata
                .as_ref()
                .and_then(|meta| meta.target.clone())
        })
        .or_else(|| current_branch_name(execution_root))
        .unwrap_or_else(|| "main".to_string());
    (source_branch, target_branch)
}

pub(crate) fn run_merge_conflict_auto_resolve_agent(
    execution_root: &Path,
    record: &JobRecord,
    node: &WorkflowRuntimeNodeManifest,
    sentinel: &Path,
    slug: &str,
    conflicts: &[String],
) -> Option<String> {
    let base_settings = match resolve_workflow_agent_settings(record) {
        Ok(settings) => settings,
        Err(err) => {
            return Some(format!("auto-resolve agent settings unavailable: {err}"));
        }
    };
    let prompt_settings = match base_settings.for_prompt(config::PromptKind::MergeConflict) {
        Ok(settings) => settings,
        Err(err) => {
            return Some(format!(
                "merge-conflict prompt profile unavailable for auto-resolve: {err}"
            ));
        }
    };
    let runner = match prompt_settings.agent_runner() {
        Ok(runner) => runner.clone(),
        Err(err) => {
            return Some(format!(
                "merge-conflict auto-resolve requires agent runner: {err}"
            ));
        }
    };

    let mut prompt_selection = prompt_settings
        .prompt_selection()
        .cloned()
        .unwrap_or_else(|| {
            config::get_config().prompt_for(
                config::CommandScope::Merge,
                config::PromptKind::MergeConflict,
            )
        });
    match load_merge_conflict_companion_prompt(execution_root, node) {
        Ok(Some(companion)) => {
            prompt_selection.text = format!("{companion}\n\n{}", prompt_selection.text.trim());
        }
        Ok(None) => {}
        Err(err) => {
            return Some(format!(
                "unable to load merge-conflict companion prompt: {err}"
            ));
        }
    }

    let (source_branch, target_branch) =
        resolve_merge_conflict_branches(execution_root, record, node, sentinel, slug);
    let merge_slug = merge_plan_slug_from_context(&source_branch, record, node)
        .unwrap_or_else(|| slug.to_string());
    let source_plan_document =
        match load_plan_document_for_merge_message(execution_root, &source_branch, &merge_slug) {
            Ok(document) => document,
            Err(err) => {
                eprintln!("merge-conflict source plan context unavailable: {err}");
                None
            }
        };
    let prompt = match crate::agent_prompt::build_merge_conflict_prompt(
        &prompt_selection,
        &target_branch,
        &source_branch,
        conflicts,
        source_plan_document.as_deref(),
        &prompt_settings.documentation,
    ) {
        Ok(prompt) => prompt,
        Err(err) => {
            return Some(format!("unable to build merge-conflict prompt: {err}"));
        }
    };

    let request =
        build_workflow_agent_request(&prompt_settings, prompt, execution_root.to_path_buf());
    match execute_agent_request_blocking(runner, request) {
        Ok(response) => {
            if !response.assistant_text.is_empty() {
                print!("{}", response.assistant_text);
                let _ = io::stdout().flush();
            }
            for line in response.stderr {
                eprintln!("{line}");
            }
            None
        }
        Err(AgentError::NonZeroExit(code, lines)) => {
            for line in lines {
                eprintln!("{line}");
            }
            Some(format!("merge-conflict auto-resolve agent exited {code}"))
        }
        Err(AgentError::Timeout(secs)) => Some(format!(
            "merge-conflict auto-resolve agent timed out after {secs}s"
        )),
        Err(err) => Some(format!("merge-conflict auto-resolve agent failed: {err}")),
    }
}

pub(crate) fn merge_plan_slug_from_context(
    source_branch: &str,
    record: &JobRecord,
    node: &WorkflowRuntimeNodeManifest,
) -> Option<String> {
    first_non_empty_arg(&node.args, &["slug", "plan"])
        .or_else(|| record.metadata.as_ref().and_then(|meta| meta.plan.clone()))
        .or_else(|| {
            source_branch
                .strip_prefix("draft/")
                .map(|value| value.to_string())
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn merge_commit_message_with_plan(subject: &str, plan_document: Option<&str>) -> String {
    let subject = subject.trim();
    match plan_document.map(str::trim) {
        Some(plan) if !plan.is_empty() => format!("{subject}\n\n{plan}"),
        _ => subject.to_string(),
    }
}

pub(crate) fn git_blob_exists_at_revision(
    execution_root: &Path,
    revision: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    Ok(crate::vcs::blob_exists_at_revision_in(
        execution_root,
        revision,
    )?)
}

pub(crate) fn git_show_blob_at_revision(
    execution_root: &Path,
    revision: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    Ok(crate::vcs::read_blob_at_revision_in(
        execution_root,
        revision,
    )?)
}

pub(crate) fn load_plan_document_for_merge_message(
    execution_root: &Path,
    source_branch: &str,
    slug: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let plan_rel = crate::plan::plan_rel_path(slug)
        .to_string_lossy()
        .replace('\\', "/");
    let tip_revision = format!("{source_branch}:{plan_rel}");
    if git_blob_exists_at_revision(execution_root, &tip_revision)? {
        return Ok(Some(git_show_blob_at_revision(
            execution_root,
            &tip_revision,
        )?));
    }

    let revisions =
        crate::vcs::revisions_touching_path_in(execution_root, source_branch, &plan_rel).map_err(
            |err| format!("unable to inspect revisions for `{source_branch}` `{plan_rel}`: {err}"),
        )?;
    for oid in revisions {
        let revision = format!("{oid}:{plan_rel}");
        if !git_blob_exists_at_revision(execution_root, &revision)? {
            continue;
        }
        return Ok(Some(git_show_blob_at_revision(execution_root, &revision)?));
    }
    Ok(None)
}

pub(crate) fn ensure_source_plan_doc_removed_before_merge(
    execution_root: &Path,
    source_branch: &str,
    target_branch: Option<&str>,
    slug: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let plan_rel = crate::plan::plan_rel_path(slug)
        .to_string_lossy()
        .replace('\\', "/");
    let tip_revision = format!("{source_branch}:{plan_rel}");
    if !git_blob_exists_at_revision(execution_root, &tip_revision)? {
        return Ok(());
    }

    let starting_branch = current_branch_name(execution_root);
    if starting_branch.as_deref() != Some(source_branch) {
        crate::vcs::checkout_branch_in(execution_root, source_branch).map_err(|err| {
            format!("unable to checkout `{source_branch}` while preparing plan doc cleanup: {err}")
        })?;
    }

    let restore_branch = target_branch
        .filter(|target| *target != source_branch)
        .map(|target| target.to_string())
        .or_else(|| {
            starting_branch
                .as_ref()
                .filter(|name| name.as_str() != source_branch)
                .cloned()
        });

    let cleanup_result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let plan_abs = execution_root.join(&plan_rel);
        if plan_abs.exists() {
            fs::remove_file(&plan_abs).map_err(|err| {
                format!("failed removing `{plan_rel}` on `{source_branch}`: {err}")
            })?;
        }

        crate::vcs::stage_paths_allow_missing_in(execution_root, &[plan_rel.as_str()])
            .map_err(|err| format!("failed to stage plan cleanup for `{plan_rel}`: {err}"))?;
        crate::vcs::commit_staged_in(
            execution_root,
            &format!("chore: remove implementation plan doc {slug}"),
            false,
        )
        .map_err(|err| format!("failed to commit plan cleanup on `{source_branch}`: {err}"))?;

        Ok(())
    })();

    if let Some(branch) = restore_branch.as_ref() {
        crate::vcs::checkout_branch_in(execution_root, branch).map_err(|err| {
            format!("unable to checkout `{branch}` after plan doc cleanup: {err}")
        })?;
    }

    cleanup_result
}

pub(crate) fn parse_string_list_json_arg(
    node: &WorkflowRuntimeNodeManifest,
    arg_key: &str,
    operation: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let Some(raw) = node.args.get(arg_key) else {
        return Err(format!("{operation} requires args.{arg_key}").into());
    };
    let values = serde_json::from_str::<Vec<String>>(raw)
        .map_err(|err| format!("{operation} has invalid {arg_key} payload: {err}"))?;
    if values.is_empty() {
        return Err(format!("{operation} requires at least one value in args.{arg_key}").into());
    }
    Ok(values)
}

pub(crate) fn parse_files_json(
    node: &WorkflowRuntimeNodeManifest,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    parse_string_list_json_arg(node, "files_json", "patch pipeline operation")
}

pub(crate) fn parse_stage_files_json(
    node: &WorkflowRuntimeNodeManifest,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    parse_string_list_json_arg(node, "files_json", "git.stage")
}

pub(crate) fn patch_pipeline_manifest_path(jobs_root: &Path, job_id: &str) -> PathBuf {
    jobs_root.join(job_id).join("patch-pipeline.json")
}

pub(crate) fn patch_pipeline_finalize_path(jobs_root: &Path, job_id: &str) -> PathBuf {
    jobs_root.join(job_id).join("patch-pipeline.finalize.json")
}

pub(crate) fn resolve_workflow_agent_settings(
    record: &JobRecord,
) -> Result<config::AgentSettings, Box<dyn std::error::Error>> {
    let cfg = config::get_config();
    let scope_alias = record
        .metadata
        .as_ref()
        .and_then(|meta| meta.command_alias.clone().or(meta.scope.clone()));
    let template_selector = record
        .metadata
        .as_ref()
        .and_then(|meta| meta.workflow_template_selector.clone());

    if let Some(raw_alias) = scope_alias {
        if let Some(alias) = config::CommandAlias::parse(&raw_alias) {
            if let Some(raw_template) = template_selector
                && let Some(selector) = config::TemplateSelector::parse(&raw_template)
            {
                return config::resolve_agent_settings_for_alias_template(
                    &cfg,
                    &alias,
                    Some(&selector),
                    None,
                );
            }
            return config::resolve_agent_settings_for_alias(&cfg, &alias, None);
        }
        if let Ok(scope) = raw_alias.parse::<config::CommandScope>() {
            return config::resolve_agent_settings(&cfg, scope, None);
        }
    }

    config::resolve_default_agent_settings(&cfg, None)
}

pub(crate) fn build_workflow_agent_request(
    agent: &config::AgentSettings,
    prompt: String,
    repo_root: PathBuf,
) -> AgentRequest {
    let mut metadata = BTreeMap::new();
    metadata.insert("agent_backend".to_string(), agent.backend.to_string());
    metadata.insert("agent_label".to_string(), agent.agent_runtime.label.clone());
    metadata.insert(
        "agent_command".to_string(),
        agent.agent_runtime.command.join(" "),
    );
    metadata.insert(
        "agent_output".to_string(),
        agent.agent_runtime.output.as_str().to_string(),
    );
    if let Some(alias) = agent.command_alias.as_ref() {
        metadata.insert("command_alias".to_string(), alias.to_string());
    }
    if let Some(selector) = agent.template_selector.as_ref() {
        metadata.insert("template_selector".to_string(), selector.to_string());
    }
    if let Some(filter) = agent.agent_runtime.progress_filter.as_ref() {
        metadata.insert("agent_progress_filter".to_string(), filter.join(" "));
    }
    match &agent.agent_runtime.resolution {
        config::AgentRuntimeResolution::BundledShim { path, .. } => {
            metadata.insert(
                "agent_command_source".to_string(),
                "bundled-shim".to_string(),
            );
            metadata.insert("agent_shim_path".to_string(), path.display().to_string());
        }
        config::AgentRuntimeResolution::ProvidedCommand => {
            metadata.insert("agent_command_source".to_string(), "configured".to_string());
        }
    }

    AgentRequest {
        prompt,
        repo_root,
        command: agent.agent_runtime.command.clone(),
        progress_filter: agent.agent_runtime.progress_filter.clone(),
        output: agent.agent_runtime.output,
        allow_script_wrapper: agent.agent_runtime.enable_script_wrapper,
        scope: agent.scope,
        metadata,
        timeout: Some(DEFAULT_AGENT_TIMEOUT),
    }
}

pub(crate) fn execute_agent_request_blocking(
    runner: std::sync::Arc<dyn crate::agent::AgentRunner>,
    request: AgentRequest,
) -> Result<crate::agent::AgentResponse, AgentError> {
    let handle = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| AgentError::Io(io::Error::other(format!("tokio runtime: {err}"))))?;
        runtime.block_on(runner.execute(request, None))
    });

    match handle.join() {
        Ok(result) => result,
        Err(_) => Err(AgentError::Io(io::Error::other(
            "agent worker thread panicked",
        ))),
    }
}

pub(crate) fn workflow_prompt_text_from_record(
    project_root: &Path,
    execution_root: &Path,
    record: &JobRecord,
    node: &WorkflowRuntimeNodeManifest,
) -> Result<String, Box<dyn std::error::Error>> {
    let raw_prompt_text = if let Some(text) = node.args.get("prompt_text")
        && !text.trim().is_empty()
    {
        text.clone()
    } else if let Some(path) = node.args.get("prompt_file")
        && !path.trim().is_empty()
    {
        let abs = resolve_path_in_execution_root(execution_root, path);
        fs::read_to_string(abs)?
    } else if let Some(command) = node.args.get("command")
        && !command.trim().is_empty()
    {
        let (status, stdout, stderr) = run_shell_text_command(execution_root, command)?;
        if status != 0 {
            return Err(format!(
                "prompt.resolve command failed (exit {status}): {}",
                stderr.trim()
            )
            .into());
        }
        stdout
    } else if let Some(script) = node.args.get("script")
        && !script.trim().is_empty()
    {
        let (status, stdout, stderr) = run_shell_text_command(execution_root, script)?;
        if status != 0 {
            return Err(format!(
                "prompt.resolve script failed (exit {status}): {}",
                stderr.trim()
            )
            .into());
        }
        stdout
    } else if let Some(text) = record
        .config_snapshot
        .as_ref()
        .and_then(|snapshot| {
            snapshot
                .pointer("/workflow/prompt_text")
                .and_then(|value| value.as_str())
                .or_else(|| {
                    snapshot
                        .pointer("/workflow_runtime/prompt_text")
                        .and_then(|value| value.as_str())
                })
        })
        .map(|value| value.to_string())
    {
        text
    } else if let Ok(value) = std::env::var("VIZIER_WORKFLOW_PROMPT_TEXT")
        && !value.trim().is_empty()
    {
        value
    } else {
        return Err("prompt.resolve missing prompt_text source (args.prompt_text/prompt_file/command/script, config workflow.prompt_text, or VIZIER_WORKFLOW_PROMPT_TEXT)".into());
    };

    let variables = collect_prompt_template_variables(project_root, execution_root, record, node)?;
    render_prompt_template(&raw_prompt_text, &variables, execution_root)
}

pub(crate) fn collect_prompt_template_variables(
    project_root: &Path,
    execution_root: &Path,
    record: &JobRecord,
    node: &WorkflowRuntimeNodeManifest,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let mut variables = BTreeMap::new();
    for (key, value) in &node.args {
        variables.insert(key.clone(), value.clone());
    }

    let local_namespace_prefix = node
        .node_id
        .rsplit_once("__")
        .map(|(namespace, _)| format!("{namespace}__"));

    if let Some(run_id) = record
        .metadata
        .as_ref()
        .and_then(|meta| meta.workflow_run_id.as_deref())
    {
        let manifest = load_workflow_run_manifest(project_root, run_id).map_err(|err| {
            format!("prompt.resolve could not load workflow run manifest `{run_id}`: {err}")
        })?;
        let mut composed_suffix_counts = BTreeMap::<&str, usize>::new();
        for runtime_node in manifest.nodes.values() {
            if let Some((_, suffix)) = runtime_node.node_id.rsplit_once("__")
                && !suffix.is_empty()
            {
                *composed_suffix_counts.entry(suffix).or_insert(0) += 1;
            }
        }

        for runtime_node in manifest.nodes.values() {
            let unique_suffix = runtime_node
                .node_id
                .rsplit_once("__")
                .map(|(_, suffix)| suffix)
                .filter(|suffix| !suffix.is_empty())
                .filter(|suffix| composed_suffix_counts.get(*suffix).copied() == Some(1));
            for (arg_key, arg_value) in &runtime_node.args {
                variables
                    .entry(format!("{}.{}", runtime_node.node_id, arg_key))
                    .or_insert_with(|| arg_value.clone());
                if let Some(prefix) = local_namespace_prefix.as_deref()
                    && let Some(local_node_id) = runtime_node.node_id.strip_prefix(prefix)
                    && !local_node_id.is_empty()
                {
                    variables
                        .entry(format!("{}.{}", local_node_id, arg_key))
                        .or_insert_with(|| arg_value.clone());
                }
                if let Some(suffix) = unique_suffix {
                    // Keep composed-template prompt placeholders stable for unique node-id suffixes.
                    variables
                        .entry(format!("{suffix}.{arg_key}"))
                        .or_insert_with(|| arg_value.clone());
                }
            }
        }
    }

    variables
        .entry("execution_root".to_string())
        .or_insert_with(|| execution_root.to_string_lossy().to_string());
    Ok(variables)
}

pub(crate) fn render_prompt_template(
    template: &str,
    variables: &BTreeMap<String, String>,
    execution_root: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut rendered = String::with_capacity(template.len());
    let mut cursor = 0usize;

    while let Some(open_rel) = template[cursor..].find("{{") {
        let open = cursor + open_rel;
        rendered.push_str(&template[cursor..open]);
        let key_start = open + 2;
        let Some(close_rel) = template[key_start..].find("}}") else {
            return Err("prompt.resolve found unclosed placeholder; expected `}}`".into());
        };
        let close = key_start + close_rel;
        let key = template[key_start..close].trim();
        if key.is_empty() {
            return Err("prompt.resolve found empty placeholder `{{}}`".into());
        }

        let replacement = resolve_prompt_template_placeholder(key, variables, execution_root)?;
        rendered.push_str(&replacement);
        cursor = close + 2;
    }

    rendered.push_str(&template[cursor..]);
    Ok(rendered)
}

pub(crate) fn resolve_prompt_template_placeholder(
    key: &str,
    variables: &BTreeMap<String, String>,
    execution_root: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(path) = key.strip_prefix("file:") {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err("prompt.resolve placeholder `file:` requires a non-empty path".into());
        }
        let abs = resolve_path_in_execution_root(execution_root, trimmed);
        return fs::read_to_string(&abs).map_err(|err| {
            format!(
                "prompt.resolve could not read placeholder file `{}`: {}",
                abs.display(),
                err
            )
            .into()
        });
    }

    if let Some(value) = variables.get(key) {
        return Ok(value.clone());
    }

    Err(format!("prompt.resolve unresolved placeholder `{{{{{key}}}}}`").into())
}

pub(crate) fn prompt_output_artifact(node: &WorkflowRuntimeNodeManifest) -> Option<JobArtifact> {
    let mut all = node.artifacts_by_outcome.succeeded.clone();
    all.extend(node.artifacts_by_outcome.failed.iter().cloned());
    all.extend(node.artifacts_by_outcome.blocked.iter().cloned());
    all.extend(node.artifacts_by_outcome.cancelled.iter().cloned());
    all.into_iter().find(|artifact| {
        matches!(
            artifact,
            JobArtifact::Custom { type_id, .. } if type_id == PROMPT_ARTIFACT_TYPE_ID
        )
    })
}

pub(crate) fn resolve_prompt_payload_text(payload: &serde_json::Value) -> Option<String> {
    resolve_custom_payload_text(payload)
}

pub(crate) fn resolve_custom_payload_text(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("text")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .or_else(|| {
            payload
                .get("stdout_text")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
        })
        .or_else(|| {
            payload
                .get("assistant_text")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
        })
        .or_else(|| {
            payload
                .pointer("/payload/text")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
        })
        .or_else(|| {
            payload
                .pointer("/payload/assistant_text")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
        })
}

pub(crate) fn parse_read_payload_selector(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let open = "read_payload(";
    if !trimmed.starts_with(open) || !trimmed.ends_with(')') {
        return None;
    }
    let inner = trimmed[open.len()..trimmed.len() - 1].trim();
    if inner.is_empty() {
        None
    } else {
        Some(inner.to_string())
    }
}

pub(crate) fn resolve_matching_custom_dependency(
    record: &JobRecord,
    selector: &str,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let custom_dependencies = record
        .schedule
        .as_ref()
        .map(|schedule| {
            schedule
                .dependencies
                .iter()
                .filter_map(|dependency| match &dependency.artifact {
                    JobArtifact::Custom { type_id, key } => Some((type_id.clone(), key.clone())),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if custom_dependencies.is_empty() {
        return Err(
            format!("read_payload({selector}) requires at least one custom dependency").into(),
        );
    }

    let mut matches = if let Some((type_id, key)) = selector.split_once(':') {
        custom_dependencies
            .into_iter()
            .filter(|(candidate_type, candidate_key)| {
                candidate_type == type_id && candidate_key == key
            })
            .collect::<Vec<_>>()
    } else {
        custom_dependencies
            .into_iter()
            .filter(|(type_id, key)| type_id == selector || key == selector)
            .collect::<Vec<_>>()
    };
    matches.sort();
    matches.dedup();
    if matches.is_empty() {
        return Err(format!("read_payload({selector}) did not match any custom dependency").into());
    }
    if matches.len() > 1 {
        let labels = matches
            .iter()
            .map(|(type_id, key)| format!("custom:{type_id}:{key}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(
            format!("read_payload({selector}) is ambiguous across dependencies: {labels}").into(),
        );
    }
    matches
        .into_iter()
        .next()
        .ok_or_else(|| "missing custom dependency match".into())
}

pub(crate) fn resolve_commit_message(
    project_root: &Path,
    record: &JobRecord,
    node: &WorkflowRuntimeNodeManifest,
) -> Result<String, Box<dyn std::error::Error>> {
    let fallback = "chore: workflow stage commit".to_string();
    let raw_message = first_non_empty_arg(&node.args, &["message", "commit_message"])
        .unwrap_or_else(|| fallback.clone());
    let message = if let Some(selector) = parse_read_payload_selector(raw_message.as_str()) {
        let (type_id, key) = resolve_matching_custom_dependency(record, selector.as_str())?;
        let (_producer_job, payload, _payload_path) = read_latest_custom_artifact_payload(
            project_root,
            &type_id,
            &key,
        )?
        .ok_or_else(|| {
            format!("read_payload({selector}) could not read payload for custom:{type_id}:{key}")
        })?;
        resolve_custom_payload_text(&payload).ok_or_else(|| {
            format!(
                "read_payload({selector}) payload missing text field for custom:{type_id}:{key}"
            )
        })?
    } else {
        raw_message
    };
    let normalized = message.trim();
    if normalized.is_empty() {
        return Err("git.commit resolved an empty commit message".into());
    }
    Ok(normalized.to_string())
}

pub(crate) fn stage_declared_plan_docs(
    execution_root: &Path,
    node: &WorkflowRuntimeNodeManifest,
) -> Result<(), String> {
    let plan_paths = plan_doc_paths_from_artifacts(&node.artifacts_by_outcome.succeeded);
    for plan_rel in &plan_paths {
        let plan_abs = execution_root.join(plan_rel);
        if !plan_abs.is_file() {
            return Err(format!(
                "expected plan doc `{}` for declared plan_doc artifact",
                plan_rel.display()
            ));
        }
    }

    if !plan_paths.is_empty() {
        let plan_path_strings = plan_paths
            .iter()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect::<Vec<_>>();
        let plan_path_refs = plan_path_strings
            .iter()
            .map(|path| path.as_str())
            .collect::<Vec<_>>();
        crate::vcs::stage_paths_allow_missing_in(execution_root, &plan_path_refs)
            .map_err(|err| format!("failed to stage plan docs: {err}"))?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_workflow_routes(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    source_record: &JobRecord,
    node: &WorkflowRuntimeNodeManifest,
    run_manifest: &WorkflowRunManifest,
    outcome: WorkflowNodeOutcome,
    scheduler_lock_held: bool,
) {
    let source_context = workflow_execution_context_from_metadata(source_record.metadata.as_ref());
    for route in node.routes.for_outcome(outcome) {
        let Some(target) = run_manifest.nodes.get(&route.node_id) else {
            display::warn(format!(
                "workflow route target `{}` missing from run manifest {}",
                route.node_id, run_manifest.run_id
            ));
            continue;
        };

        match route.mode {
            WorkflowRouteMode::PropagateContext => {
                let Some(context) = source_context.as_ref() else {
                    continue;
                };
                match apply_workflow_execution_context(jobs_root, &target.job_id, context, true) {
                    Ok(true) => display::debug(format!(
                        "workflow route {} -> {} propagated execution context",
                        node.node_id, target.node_id
                    )),
                    Ok(false) => display::debug(format!(
                        "workflow route {} -> {} skipped execution-context propagation (active or unchanged target)",
                        node.node_id, target.node_id
                    )),
                    Err(err) => display::warn(format!(
                        "workflow route {} -> {} context propagation failed: {}",
                        node.node_id, target.node_id, err
                    )),
                }
            }
            WorkflowRouteMode::RetryJob => {
                let target_record = read_record(jobs_root, &target.job_id);
                if let Ok(record) = target_record
                    && job_is_active(record.status)
                {
                    continue;
                }
                let retry_result = if scheduler_lock_held {
                    retry_job_internal_locked(
                        project_root,
                        jobs_root,
                        binary,
                        &target.job_id,
                        source_context.as_ref(),
                        false,
                    )
                } else {
                    retry_job_internal(
                        project_root,
                        jobs_root,
                        binary,
                        &target.job_id,
                        source_context.as_ref(),
                    )
                };
                if let Err(err) = retry_result {
                    display::warn(format!(
                        "workflow route {} -> {} retry failed: {}",
                        node.node_id, target.node_id, err
                    ));
                }
            }
        }
    }
}

pub(crate) fn apply_workflow_execution_context(
    jobs_root: &Path,
    job_id: &str,
    context: &WorkflowExecutionContext,
    skip_active_targets: bool,
) -> Result<bool, Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    if !paths.record_path.exists() {
        return Err(format!("no background job {}", job_id).into());
    }
    let mut record = load_record(&paths)?;
    if skip_active_targets && record.status == JobStatus::Running {
        return Ok(false);
    }

    let mut metadata = record.metadata.take().unwrap_or_default();
    let changed = metadata.execution_root != context.execution_root
        || metadata.worktree_path != context.worktree_path
        || metadata.worktree_name != context.worktree_name
        || metadata.worktree_owned != context.worktree_owned;
    if !changed {
        record.metadata = Some(metadata);
        return Ok(false);
    }

    metadata.execution_root = context.execution_root.clone();
    metadata.worktree_path = context.worktree_path.clone();
    metadata.worktree_name = context.worktree_name.clone();
    metadata.worktree_owned = context.worktree_owned;
    record.metadata = Some(metadata);
    persist_record(&paths, &record)?;
    Ok(true)
}

#[cfg(test)]
pub(crate) fn succeeded_completion_pause_barrier()
-> &'static Mutex<Option<std::sync::Arc<std::sync::Barrier>>> {
    static HOOK: OnceLock<Mutex<Option<std::sync::Arc<std::sync::Barrier>>>> = OnceLock::new();
    HOOK.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
pub(crate) fn set_succeeded_completion_pause_barrier(
    barrier: Option<std::sync::Arc<std::sync::Barrier>>,
) {
    let mut guard = succeeded_completion_pause_barrier()
        .lock()
        .expect("lock succeeded-completion pause barrier");
    *guard = barrier;
}

#[cfg(test)]
pub(crate) fn pause_succeeded_completion_if_configured() {
    let barrier = {
        let guard = succeeded_completion_pause_barrier()
            .lock()
            .expect("lock succeeded-completion pause barrier");
        guard.clone()
    };
    if let Some(barrier) = barrier {
        barrier.wait();
        barrier.wait();
    }
}

pub(crate) fn workflow_node_has_natural_stdout(node: &WorkflowRuntimeNodeManifest) -> bool {
    matches!(
        node.executor_operation.as_deref(),
        Some("agent.invoke" | "command.run" | "cicd.run")
    ) || matches!(
        node.control_policy.as_deref(),
        Some("gate.stop_condition" | "gate.conflict_resolution" | "gate.cicd")
    )
}

pub(crate) fn workflow_operation_result_payload(
    result: &WorkflowNodeResult,
    artifacts_written: &[JobArtifact],
) -> serde_json::Value {
    serde_json::json!({
        "summary": result.summary,
        "artifacts_written": artifacts_written
            .iter()
            .map(format_artifact)
            .collect::<Vec<_>>(),
        "payload_refs": result.payload_refs,
        "metadata": result.metadata,
    })
}

pub(crate) fn execute_workflow_node_job(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
) -> Result<i32, Box<dyn std::error::Error>> {
    let record = read_record(jobs_root, job_id)?;
    let metadata = record
        .metadata
        .as_ref()
        .ok_or_else(|| format!("workflow node job {} is missing metadata", job_id))?;
    let run_id = metadata
        .workflow_run_id
        .as_deref()
        .ok_or_else(|| format!("workflow node job {} missing workflow_run_id", job_id))?;
    let node_id = metadata
        .workflow_node_id
        .as_deref()
        .ok_or_else(|| format!("workflow node job {} missing workflow_node_id", job_id))?;
    let manifest = load_workflow_run_manifest(project_root, run_id)?;
    let node_manifest = manifest
        .nodes
        .get(node_id)
        .ok_or_else(|| format!("workflow run {} missing node {}", run_id, node_id))?;

    let started_at = Utc::now();
    let mut lifecycle_stderr_lines = Vec::new();
    lifecycle_stderr_lines.push(emit_workflow_node_lifecycle_line(
        node_manifest,
        "start",
        format!("run={run_id} job={job_id}"),
    ));
    lifecycle_stderr_lines.push(emit_workflow_node_lifecycle_line(
        node_manifest,
        "progress",
        "dispatching runtime handler",
    ));

    set_current_job_id(Some(job_id.to_string()));
    let result = match (
        node_manifest.executor_operation.as_deref(),
        node_manifest.control_policy.as_deref(),
    ) {
        (Some(_), _) => execute_workflow_executor(project_root, jobs_root, &record, node_manifest),
        (None, Some(_)) => execute_workflow_control(project_root, &record, node_manifest),
        _ => Ok(WorkflowNodeResult::failed(
            format!("workflow node {} has no runtime operation/policy", node_id),
            Some(1),
        )),
    };
    set_current_job_id(None);
    let mut result = result?;

    let mut artifacts_written = node_manifest
        .artifacts_by_outcome
        .for_outcome(result.outcome)
        .to_vec();
    artifacts_written.extend(result.artifacts_written.clone());
    artifacts_written.push(workflow_operation_output_artifact(node_id));
    artifacts_written = dedup_job_artifacts(artifacts_written);

    let (status, exit_code) = map_workflow_outcome_to_job_status(result.outcome, result.exit_code);
    lifecycle_stderr_lines.push(emit_workflow_node_lifecycle_line(
        node_manifest,
        "complete",
        format!("outcome={} exit_code={exit_code}", result.outcome.as_str()),
    ));

    let canonical_stdout = serde_json::json!({
        "schema": "vizier.operation_result.v1",
        "run_id": run_id,
        "job_id": job_id,
        "node_id": node_manifest.node_id.as_str(),
        "uses": node_manifest.uses.as_str(),
        "outcome": result.outcome.as_str(),
        "exit_code": exit_code,
        "summary": result.summary.clone(),
    });
    let mut canonical_stdout_text = None;
    if !workflow_node_has_natural_stdout(node_manifest) {
        let text = format!("{}\n", serde_json::to_string(&canonical_stdout)?);
        print_stdout_text(&text);
        canonical_stdout_text = Some(text);
    }

    let mut payload_stdout_text = result.stdout_text.clone().unwrap_or_default();
    if payload_stdout_text.is_empty()
        && let Some(canonical_text) = canonical_stdout_text.as_ref()
    {
        payload_stdout_text = canonical_text.clone();
    }

    let mut payload_stderr_lines = lifecycle_stderr_lines.clone();
    payload_stderr_lines.extend(result.stderr_lines.clone());

    let finished_at = Utc::now();
    let duration_ms = finished_at
        .signed_duration_since(started_at)
        .num_milliseconds()
        .max(0);
    let operation_output_payload = WorkflowOperationOutputPayload {
        schema: OPERATION_OUTPUT_SCHEMA_ID.to_string(),
        run_id: run_id.to_string(),
        job_id: job_id.to_string(),
        node_id: node_manifest.node_id.clone(),
        uses: node_manifest.uses.clone(),
        executor_operation: node_manifest.executor_operation.clone(),
        control_policy: node_manifest.control_policy.clone(),
        outcome: result.outcome.as_str().to_string(),
        exit_code,
        stdout_text: payload_stdout_text.clone(),
        stderr_lines: payload_stderr_lines,
        started_at: started_at.to_rfc3339(),
        finished_at: finished_at.to_rfc3339(),
        duration_ms,
        result: workflow_operation_result_payload(&result, &artifacts_written),
    };
    let operation_output_payload_value = serde_json::to_value(&operation_output_payload)?;
    let operation_output_path = write_custom_artifact_payload(
        project_root,
        job_id,
        OPERATION_OUTPUT_ARTIFACT_TYPE_ID,
        node_id,
        &operation_output_payload_value,
    )?;
    result
        .payload_refs
        .push(relative_path(project_root, &operation_output_path));
    result.payload_refs.sort();
    result.payload_refs.dedup();

    let metadata_update = JobMetadata {
        workflow_node_outcome: Some(result.outcome.as_str().to_string()),
        workflow_payload_refs: if result.payload_refs.is_empty() {
            None
        } else {
            Some(result.payload_refs.clone())
        },
        ..JobMetadata::default()
    };
    let metadata_update = merge_metadata(Some(metadata_update), result.metadata.clone());
    let binary = std::env::current_exe()?;
    if result.outcome == WorkflowNodeOutcome::Succeeded {
        display::debug(format!(
            "workflow node {} (run {}, job {}) acquiring scheduler lock for succeeded completion",
            node_id, run_id, job_id
        ));
        let _lock = SchedulerLock::acquire(jobs_root)?;
        display::debug(format!(
            "workflow node {} (run {}, job {}) acquired scheduler lock for succeeded completion",
            node_id, run_id, job_id
        ));

        let finalized_record = finalize_job_with_artifacts(
            project_root,
            jobs_root,
            job_id,
            status,
            exit_code,
            None,
            metadata_update,
            Some(&artifacts_written),
        )?;
        display::debug(format!(
            "workflow node {} (run {}, job {}) finalized succeeded source record",
            node_id, run_id, job_id
        ));

        #[cfg(test)]
        pause_succeeded_completion_if_configured();

        apply_workflow_routes(
            project_root,
            jobs_root,
            &binary,
            &finalized_record,
            node_manifest,
            &manifest,
            result.outcome,
            true,
        );
        display::debug(format!(
            "workflow node {} (run {}, job {}) applied succeeded routes",
            node_id, run_id, job_id
        ));

        let scheduler_outcome = scheduler_tick_locked(project_root, jobs_root, &binary)?;
        display::debug(format!(
            "workflow node {} (run {}, job {}) advanced scheduler tick under lock (started={}, updated={})",
            node_id,
            run_id,
            job_id,
            scheduler_outcome.started.len(),
            scheduler_outcome.updated.len()
        ));
        display::debug(format!(
            "workflow node {} (run {}, job {}) releasing scheduler lock after succeeded completion",
            node_id, run_id, job_id
        ));
    } else {
        let finalized_record = finalize_job_with_artifacts(
            project_root,
            jobs_root,
            job_id,
            status,
            exit_code,
            None,
            metadata_update,
            Some(&artifacts_written),
        )?;
        apply_workflow_routes(
            project_root,
            jobs_root,
            &binary,
            &finalized_record,
            node_manifest,
            &manifest,
            result.outcome,
            false,
        );
        let _ = scheduler_tick(project_root, jobs_root, &binary)?;
    }
    Ok(exit_code)
}

pub fn run_workflow_node_command(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    match execute_workflow_node_job(project_root, jobs_root, job_id) {
        Ok(code) => {
            if code == 0 {
                Ok(())
            } else {
                Err(format!("workflow node {job_id} failed (exit {code})").into())
            }
        }
        Err(err) => {
            if let Err(finalize_err) =
                finalize_failed_workflow_node_if_active(project_root, jobs_root, job_id)
            {
                display::warn(format!(
                    "unable to finalize failed workflow node job {} after runtime error: {}",
                    job_id, finalize_err
                ));
            }
            Err(err)
        }
    }
}

pub(crate) fn finalize_failed_workflow_node_if_active(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let record = read_record(jobs_root, job_id)?;
    if !job_is_active(record.status) {
        return Ok(());
    }

    let metadata = JobMetadata {
        workflow_node_outcome: Some(WorkflowNodeOutcome::Failed.as_str().to_string()),
        ..JobMetadata::default()
    };
    let _ = finalize_job(
        project_root,
        jobs_root,
        job_id,
        JobStatus::Failed,
        1,
        None,
        Some(metadata),
    )?;
    Ok(())
}
