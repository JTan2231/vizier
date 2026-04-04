use super::*;

pub(crate) fn execute_workflow_executor(
    project_root: &Path,
    jobs_root: &Path,
    record: &JobRecord,
    node: &WorkflowRuntimeNodeManifest,
) -> Result<WorkflowNodeResult, Box<dyn std::error::Error>> {
    let execution_root = resolve_execution_root(project_root, record)?;
    match node.executor_operation.as_deref() {
        Some("worktree.prepare") => {
            let branch = first_non_empty_arg(&node.args, &["branch"])
                .or_else(|| {
                    record
                        .metadata
                        .as_ref()
                        .and_then(|meta| meta.branch.as_ref().cloned())
                })
                .or_else(|| {
                    first_non_empty_arg(&node.args, &["slug", "plan"])
                        .map(|slug| crate::plan::default_branch_for_slug(&slug))
                })
                .or_else(|| {
                    record
                        .metadata
                        .as_ref()
                        .and_then(|meta| meta.plan.as_ref())
                        .map(|slug| crate::plan::default_branch_for_slug(slug))
                });
            let Some(branch) = branch else {
                return Ok(WorkflowNodeResult::failed(
                    "worktree.prepare could not determine branch (set branch or slug/plan)",
                    Some(1),
                ));
            };
            let created_branch = match ensure_local_branch_with_ownership(project_root, &branch) {
                Ok(created) => created,
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("worktree.prepare could not ensure branch `{branch}`: {err}"),
                        Some(1),
                    ));
                }
            };

            let purpose = first_non_empty_arg(&node.args, &["purpose"])
                .unwrap_or_else(|| sanitize_workflow_component(&node.node_id));
            let dir_name = format!("{}-{}", sanitize_workflow_component(&purpose), record.id);
            let worktree_path = project_root.join(".vizier/tmp-worktrees").join(&dir_name);
            if let Some(parent) = worktree_path.parent() {
                fs::create_dir_all(parent)?;
            }

            if worktree_path.exists() {
                let mut result =
                    WorkflowNodeResult::succeeded("worktree already exists for this node");
                result.payload_refs = vec![relative_path(project_root, &worktree_path)];
                result.metadata = Some(JobMetadata {
                    execution_root: Some(relative_path(project_root, &worktree_path)),
                    worktree_owned: Some(true),
                    worktree_path: Some(relative_path(project_root, &worktree_path)),
                    worktree_name: find_worktree_name_by_path(
                        &Repository::open(project_root)?,
                        &worktree_path,
                    ),
                    ..JobMetadata::default()
                });
                return Ok(result);
            }

            if let Err(err) = crate::vcs::add_worktree_for_branch_in(
                project_root,
                &dir_name,
                &worktree_path,
                &branch,
            ) {
                let reason = format!("worktree.prepare failed to add worktree: {err}");
                return Ok(WorkflowNodeResult::failed(reason, Some(1)));
            }

            let mut result = WorkflowNodeResult::succeeded("worktree prepared");
            result.payload_refs = vec![relative_path(project_root, &worktree_path)];
            let worktree_name =
                find_worktree_name_by_path(&Repository::open(project_root)?, &worktree_path);
            result.metadata = Some(JobMetadata {
                branch: Some(branch.clone()),
                ephemeral_owned_branches: created_branch.then_some(vec![branch.clone()]),
                execution_root: Some(relative_path(project_root, &worktree_path)),
                worktree_owned: Some(true),
                worktree_path: Some(relative_path(project_root, &worktree_path)),
                worktree_name,
                ..JobMetadata::default()
            });
            Ok(result)
        }
        Some("worktree.cleanup") => {
            let Some(metadata) = record.metadata.as_ref() else {
                return Ok(WorkflowNodeResult::succeeded(
                    "worktree cleanup skipped (no worktree metadata)",
                ));
            };
            if metadata.worktree_owned != Some(true) {
                return Ok(WorkflowNodeResult::succeeded(
                    "worktree cleanup skipped (worktree not marked as job-owned)",
                ));
            }
            let Some(recorded_path) = metadata.worktree_path.as_ref() else {
                return Ok(WorkflowNodeResult::failed(
                    "worktree cleanup cannot run: missing worktree_path metadata",
                    Some(1),
                ));
            };

            let worktree_path = resolve_recorded_path(project_root, recorded_path);
            let worktree_name = metadata.worktree_name.as_deref();
            if !worktree_safe_to_remove(project_root, &worktree_path, worktree_name) {
                return Ok(WorkflowNodeResult::failed(
                    format!(
                        "refusing to cleanup unsafe worktree path {}",
                        worktree_path.display()
                    ),
                    Some(1),
                ));
            }

            let mut result =
                if let Err(err) = cleanup_worktree(project_root, &worktree_path, worktree_name) {
                    display::warn(format!(
                        "workflow worktree cleanup degraded for job {}: {}",
                        record.id, err
                    ));
                    let mut degraded = WorkflowNodeResult::succeeded(
                        "worktree cleanup degraded (manual prune may be needed)",
                    );
                    degraded.metadata = Some(JobMetadata {
                        retry_cleanup_status: Some(RetryCleanupStatus::Degraded),
                        retry_cleanup_error: Some(err),
                        ..JobMetadata::default()
                    });
                    degraded
                } else {
                    let mut cleaned = WorkflowNodeResult::succeeded("worktree cleaned");
                    cleaned.metadata = Some(JobMetadata {
                        execution_root: Some(".".to_string()),
                        worktree_owned: Some(false),
                        retry_cleanup_status: Some(RetryCleanupStatus::Done),
                        retry_cleanup_error: None,
                        ..JobMetadata::default()
                    });
                    cleaned
                };
            result.payload_refs = vec![relative_path(project_root, &worktree_path)];
            Ok(result)
        }
        Some("prompt.resolve") => {
            let prompt_artifact = prompt_output_artifact(node).ok_or_else(|| {
                "prompt.resolve node is missing prompt artifact output".to_string()
            })?;
            let (type_id, key) = match prompt_artifact.clone() {
                JobArtifact::Custom { type_id, key } => (type_id, key),
                _ => {
                    return Err("prompt.resolve output artifact must be custom prompt_text".into());
                }
            };

            let (prompt_text, stderr_lines) =
                workflow_prompt_text_from_record(project_root, &execution_root, record, node)?;
            let payload = serde_json::json!({
                "type_id": type_id,
                "key": key,
                "text": prompt_text,
                "written_at": Utc::now().to_rfc3339(),
            });
            let path =
                write_custom_artifact_payload(project_root, &record.id, &type_id, &key, &payload)?;
            Ok(WorkflowNodeResult {
                outcome: WorkflowNodeOutcome::Succeeded,
                artifacts_written: vec![prompt_artifact],
                payload_refs: vec![relative_path(project_root, &path)],
                metadata: None,
                summary: Some("prompt resolved".to_string()),
                exit_code: Some(0),
                stdout_text: None,
                stderr_lines,
            })
        }
        Some("agent.invoke") => {
            let prompt_dependency = record
                .schedule
                .as_ref()
                .and_then(|schedule| {
                    schedule
                        .dependencies
                        .iter()
                        .find_map(|dependency| match &dependency.artifact {
                            JobArtifact::Custom { type_id, key }
                                if type_id == PROMPT_ARTIFACT_TYPE_ID =>
                            {
                                Some((type_id.clone(), key.clone()))
                            }
                            _ => None,
                        })
                })
                .ok_or_else(|| {
                    "agent.invoke requires a custom:prompt_text dependency".to_string()
                })?;
            let (type_id, key) = prompt_dependency;
            let (_producer_job, payload, payload_path) =
                read_latest_custom_artifact_payload(project_root, &type_id, &key)?.ok_or_else(
                    || {
                        format!(
                            "agent.invoke could not find prompt payload for custom:{}:{}",
                            type_id, key
                        )
                    },
                )?;
            let prompt_text = resolve_prompt_payload_text(&payload)
                .ok_or_else(|| "prompt payload missing text field".to_string())?;

            let agent_settings = match resolve_workflow_agent_settings(record) {
                Ok(settings) => settings,
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("agent.invoke could not resolve agent settings: {err}"),
                        Some(1),
                    ));
                }
            };
            let runner = match agent_settings.agent_runner() {
                Ok(runner) => runner.clone(),
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("agent.invoke requires agent backend runner: {err}"),
                        Some(1),
                    ));
                }
            };
            let request = build_workflow_agent_request(
                &agent_settings,
                prompt_text,
                execution_root.to_path_buf(),
            );
            let response = execute_agent_request_blocking(runner, request);
            match response {
                Ok(response) => {
                    let assistant_text = response.assistant_text.clone();
                    let stderr_lines = response.stderr.clone();
                    print_stdout_text(&assistant_text);
                    for line in &stderr_lines {
                        eprintln!("{line}");
                    }

                    let mut result = WorkflowNodeResult::succeeded(
                        "agent.invoke completed via configured runner",
                    );
                    if !assistant_text.is_empty() {
                        result.stdout_text = Some(assistant_text.clone());
                    }
                    result.stderr_lines = stderr_lines.clone();
                    result.payload_refs = vec![relative_path(project_root, &payload_path)];
                    let mut produced_custom = HashSet::new();
                    for artifact in &node.artifacts_by_outcome.succeeded {
                        if let JobArtifact::Custom { type_id, key } = artifact
                            && type_id != OPERATION_OUTPUT_ARTIFACT_TYPE_ID
                        {
                            produced_custom.insert((type_id.clone(), key.clone()));
                        }
                    }
                    for (type_id, key) in produced_custom {
                        let artifact_payload = serde_json::json!({
                            "type_id": type_id,
                            "key": key,
                            "text": assistant_text.clone(),
                            "stderr": stderr_lines.clone(),
                            "exit_code": response.exit_code,
                            "duration_ms": response.duration_ms,
                            "written_at": Utc::now().to_rfc3339(),
                        });
                        let artifact_path = write_custom_artifact_payload(
                            project_root,
                            &record.id,
                            &type_id,
                            &key,
                            &artifact_payload,
                        )?;
                        result
                            .payload_refs
                            .push(relative_path(project_root, &artifact_path));
                        result
                            .artifacts_written
                            .push(JobArtifact::Custom { type_id, key });
                    }
                    result.metadata = Some(JobMetadata {
                        agent_selector: Some(agent_settings.selector.clone()),
                        agent_backend: Some(agent_settings.backend.to_string()),
                        agent_label: Some(agent_settings.agent_runtime.label.clone()),
                        agent_command: Some(agent_settings.agent_runtime.command.clone()),
                        config_backend: Some(agent_settings.backend.to_string()),
                        config_agent_selector: Some(agent_settings.selector.clone()),
                        config_agent_label: Some(agent_settings.agent_runtime.label.clone()),
                        config_agent_command: Some(agent_settings.agent_runtime.command.clone()),
                        agent_exit_code: Some(response.exit_code),
                        ..JobMetadata::default()
                    });
                    Ok(result)
                }
                Err(AgentError::NonZeroExit(code, lines)) => {
                    for line in &lines {
                        eprintln!("{line}");
                    }
                    let mut result = WorkflowNodeResult::failed(
                        format!("agent.invoke failed (exit {code})"),
                        Some(code),
                    );
                    result.stderr_lines = lines;
                    Ok(result)
                }
                Err(AgentError::Timeout(secs)) => Ok(WorkflowNodeResult::failed(
                    format!("agent.invoke timed out after {secs}s"),
                    Some(124),
                )),
                Err(err) => Ok(WorkflowNodeResult::failed(
                    format!("agent.invoke failed: {err}"),
                    Some(1),
                )),
            }
        }
        Some("plan.persist") => {
            let spec_source = node
                .args
                .get("spec_source")
                .map(|value| value.trim().to_ascii_lowercase())
                .unwrap_or_else(|| "inline".to_string());
            let spec_text_arg = node
                .args
                .get("spec_text")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            let spec_file = node
                .args
                .get("spec_file")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            let spec_text = match spec_source.as_str() {
                "inline" | "stdin" => {
                    if let Some(text) = spec_text_arg {
                        text
                    } else if let Some(path) = spec_file {
                        fs::read_to_string(resolve_path_in_execution_root(&execution_root, &path))?
                    } else {
                        return Ok(WorkflowNodeResult::failed(
                            "plan.persist requires spec_text or spec_file",
                            Some(1),
                        ));
                    }
                }
                "file" => {
                    let Some(path) = spec_file else {
                        return Ok(WorkflowNodeResult::failed(
                            "plan.persist has spec_source=file but no spec_file",
                            Some(1),
                        ));
                    };
                    fs::read_to_string(resolve_path_in_execution_root(&execution_root, &path))?
                }
                other => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("plan.persist has unsupported spec_source `{other}`"),
                        Some(1),
                    ));
                }
            };

            let requested_slug = first_non_empty_arg(&node.args, &["name_override", "slug"])
                .or_else(|| record.metadata.as_ref().and_then(|meta| meta.plan.clone()))
                .unwrap_or_else(|| crate::plan::slug_from_spec(&spec_text));
            let slug = match crate::plan::sanitize_name_override(&requested_slug) {
                Ok(value) => value,
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("plan.persist invalid slug `{requested_slug}`: {err}"),
                        Some(1),
                    ));
                }
            };
            let branch = first_non_empty_arg(&node.args, &["branch"])
                .or_else(|| {
                    record
                        .metadata
                        .as_ref()
                        .and_then(|meta| meta.branch.clone())
                })
                .unwrap_or_else(|| crate::plan::default_branch_for_slug(&slug));
            let created_branch = match ensure_local_branch_with_ownership(&execution_root, &branch)
            {
                Ok(created) => created,
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("plan.persist could not ensure branch `{branch}`: {err}"),
                        Some(1),
                    ));
                }
            };

            let plan_id = first_non_empty_arg(&node.args, &["plan_id"])
                .unwrap_or_else(crate::plan::new_plan_id);
            let mut plan_payload_ref: Option<PathBuf> = None;
            let plan_body_from_dependency = record.schedule.as_ref().and_then(|schedule| {
                schedule.dependencies.iter().find_map(|dependency| {
                    let JobArtifact::Custom { type_id, key } = &dependency.artifact else {
                        return None;
                    };
                    if type_id == PLAN_TEXT_ARTIFACT_TYPE_ID {
                        Some((type_id.clone(), key.clone()))
                    } else {
                        None
                    }
                })
            });
            let plan_body_from_dependency =
                if let Some((type_id, key)) = plan_body_from_dependency {
                    let (_producer_job, payload, payload_path) =
                        read_latest_custom_artifact_payload(project_root, &type_id, &key)?
                            .ok_or_else(|| {
                                format!(
                                    "plan.persist could not find plan payload for custom:{}:{}",
                                    type_id, key
                                )
                            })?;
                    let Some(text) = resolve_custom_payload_text(&payload) else {
                        return Ok(WorkflowNodeResult::failed(
                            format!(
                                "plan.persist plan payload missing text field for custom:{}:{}",
                                type_id, key
                            ),
                            Some(1),
                        ));
                    };
                    plan_payload_ref = Some(payload_path);
                    Some(text)
                } else {
                    None
                };
            let plan_body = first_non_empty_arg(&node.args, &["plan_body", "plan_text", "content"])
                .or(plan_body_from_dependency)
                .unwrap_or_else(|| spec_text.clone());
            let doc_contents =
                crate::plan::render_plan_document(&plan_id, &slug, &branch, &spec_text, &plan_body);
            let plan_rel = crate::plan::plan_rel_path(&slug);
            let plan_abs = execution_root.join(&plan_rel);
            if let Err(err) = crate::plan::write_plan_file(&plan_abs, &doc_contents) {
                return Ok(WorkflowNodeResult::failed(
                    format!("plan.persist failed to write {}: {err}", plan_rel.display()),
                    Some(1),
                ));
            }

            let now = Utc::now().to_rfc3339();
            let summary = spec_text
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(|line| line.chars().take(160).collect::<String>());
            let state_rel = crate::plan::upsert_plan_record(
                &execution_root,
                crate::plan::PlanRecordUpsert {
                    plan_id: plan_id.clone(),
                    slug: Some(slug.clone()),
                    branch: Some(branch.clone()),
                    source: Some(spec_source),
                    intent: first_non_empty_arg(&node.args, &["intent"]),
                    target_branch: first_non_empty_arg(&node.args, &["target_branch"]).or_else(
                        || {
                            record
                                .metadata
                                .as_ref()
                                .and_then(|meta| meta.target.clone())
                        },
                    ),
                    work_ref: Some(format!("workflow-job:{}", record.id)),
                    status: Some("proposed".to_string()),
                    summary,
                    updated_at: now.clone(),
                    created_at: Some(now),
                    job_ids: Some(HashMap::from([("persist".to_string(), record.id.clone())])),
                },
            )?;

            let mut result = WorkflowNodeResult::succeeded("plan persisted");
            result.artifacts_written = vec![
                JobArtifact::PlanBranch {
                    slug: slug.clone(),
                    branch: branch.clone(),
                },
                JobArtifact::PlanDoc {
                    slug: slug.clone(),
                    branch: branch.clone(),
                },
            ];
            result.payload_refs = vec![
                relative_path(project_root, &plan_abs),
                relative_path(project_root, &execution_root.join(state_rel)),
            ];
            if let Some(payload_path) = plan_payload_ref {
                result
                    .payload_refs
                    .push(relative_path(project_root, &payload_path));
            }
            result.metadata = Some(JobMetadata {
                plan: Some(slug),
                branch: Some(branch.clone()),
                ephemeral_owned_branches: created_branch.then_some(vec![branch.clone()]),
                ..JobMetadata::default()
            });
            Ok(result)
        }
        Some("git.stage") => {
            let files = match parse_stage_files_json(node) {
                Ok(files) => files,
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("git.stage {err}"),
                        Some(1),
                    ));
                }
            };
            let file_refs = files.iter().map(String::as_str).collect::<Vec<_>>();
            if let Err(err) = crate::vcs::stage_in(&execution_root, Some(file_refs)) {
                return Ok(WorkflowNodeResult::failed(
                    format!("git.stage failed to stage changes: {err}"),
                    Some(1),
                ));
            }
            if let Err(err) = stage_declared_plan_docs(&execution_root, node) {
                return Ok(WorkflowNodeResult::failed(
                    format!("git.stage {err}"),
                    Some(1),
                ));
            }
            Ok(WorkflowNodeResult::succeeded("git.stage staged changes"))
        }
        Some("git.commit") => {
            let staged = match crate::vcs::snapshot_staged(&execution_root.to_string_lossy()) {
                Ok(staged) => staged,
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("git.commit could not inspect staged changes: {err}"),
                        Some(1),
                    ));
                }
            };
            if staged.is_empty() {
                return Ok(WorkflowNodeResult::succeeded(
                    "git.commit: no staged changes",
                ));
            }

            let message = match resolve_commit_message(project_root, record, node) {
                Ok(message) => message,
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("git.commit could not resolve commit message: {err}"),
                        Some(1),
                    ));
                }
            };
            match crate::vcs::commit_staged_in(&execution_root, &message, false) {
                Ok(_) => Ok(WorkflowNodeResult::succeeded(
                    "git.commit committed changes",
                )),
                Err(err) => Ok(WorkflowNodeResult::failed(
                    format!("git.commit failed to create commit: {err}"),
                    Some(1),
                )),
            }
        }
        Some("git.stage_commit") => {
            if let Err(err) = crate::vcs::stage_all_in(&execution_root) {
                return Ok(WorkflowNodeResult::failed(
                    format!("git.stage_commit failed to stage changes: {err}"),
                    Some(1),
                ));
            }
            if let Err(err) = stage_declared_plan_docs(&execution_root, node) {
                return Ok(WorkflowNodeResult::failed(
                    format!("git.stage_commit {err}"),
                    Some(1),
                ));
            }

            let staged = match crate::vcs::snapshot_staged(&execution_root.to_string_lossy()) {
                Ok(staged) => staged,
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("git.stage_commit could not inspect staged changes: {err}"),
                        Some(1),
                    ));
                }
            };
            if staged.is_empty() {
                return Ok(WorkflowNodeResult::succeeded(
                    "git.stage_commit: no staged changes",
                ));
            }

            let message = match resolve_commit_message(project_root, record, node) {
                Ok(message) => message,
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("git.stage_commit could not resolve commit message: {err}"),
                        Some(1),
                    ));
                }
            };
            match crate::vcs::commit_staged_in(&execution_root, &message, false) {
                Ok(_) => Ok(WorkflowNodeResult::succeeded(
                    "git.stage_commit committed changes",
                )),
                Err(err) => Ok(WorkflowNodeResult::failed(
                    format!("git.stage_commit failed to create commit: {err}"),
                    Some(1),
                )),
            }
        }
        Some("git.integrate_plan_branch") => {
            let source_branch =
                first_non_empty_arg(&node.args, &["branch", "source_branch", "plan_branch"])
                    .or_else(|| {
                        first_non_empty_arg(&node.args, &["slug", "plan"])
                            .map(|slug| crate::plan::default_branch_for_slug(&slug))
                    })
                    .or_else(|| {
                        record
                            .metadata
                            .as_ref()
                            .and_then(|meta| meta.branch.clone())
                    })
                    .or_else(|| {
                        record
                            .metadata
                            .as_ref()
                            .and_then(|meta| meta.plan.as_ref())
                            .map(|slug| crate::plan::default_branch_for_slug(slug))
                    });
            let Some(source_branch) = source_branch else {
                return Ok(WorkflowNodeResult::failed(
                    "git.integrate_plan_branch requires a source branch",
                    Some(1),
                ));
            };
            let target_branch = first_non_empty_arg(&node.args, &["target", "target_branch"])
                .or_else(|| {
                    record
                        .metadata
                        .as_ref()
                        .and_then(|meta| meta.target.clone())
                });
            let squash = bool_arg(&node.args, "squash").unwrap_or(true);
            let delete_branch = bool_arg(&node.args, "delete_branch").unwrap_or(false);
            let slug = workflow_slug_from_record(record, node);
            let sentinel = merge_sentinel_path(project_root, &slug);
            let merge_slug =
                merge_plan_slug_from_context(&source_branch, record, node).unwrap_or(slug.clone());
            let merge_subject = first_non_empty_arg(&node.args, &["message"])
                .unwrap_or_else(|| format!("feat: merge plan {merge_slug}"));

            if let Some(target) = target_branch.as_ref() {
                let current = current_branch_name(&execution_root);
                if current.as_deref() != Some(target.as_str())
                    && let Err(err) = crate::vcs::checkout_branch_in(&execution_root, target)
                {
                    return Ok(WorkflowNodeResult::failed(
                        format!("git.integrate_plan_branch failed checkout `{target}`: {err}"),
                        Some(1),
                    ));
                }
            }

            let plan_document = match load_plan_document_for_merge_message(
                &execution_root,
                &source_branch,
                &merge_slug,
            ) {
                Ok(doc) => doc,
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("git.integrate_plan_branch could not load plan document: {err}"),
                        Some(1),
                    ));
                }
            };
            if plan_document.is_some()
                && let Err(err) = ensure_source_plan_doc_removed_before_merge(
                    &execution_root,
                    &source_branch,
                    target_branch.as_deref(),
                    &merge_slug,
                )
            {
                return Ok(WorkflowNodeResult::failed(
                    format!(
                        "git.integrate_plan_branch failed removing plan doc from source branch: {err}"
                    ),
                    Some(1),
                ));
            }
            let merge_message =
                merge_commit_message_with_plan(&merge_subject, plan_document.as_deref());

            let finalize_in_progress = match Repository::open(&execution_root) {
                Ok(repo) if repo.state() == git2::RepositoryState::Merge => match repo.index() {
                    Ok(index) if !index.has_conflicts() => {
                        let head_oid = match repo.head().and_then(|head| head.peel_to_commit()) {
                            Ok(head_commit) => head_commit.id(),
                            Err(err) => {
                                return Ok(WorkflowNodeResult::failed(
                                    format!(
                                        "git.integrate_plan_branch could not inspect merge head: {err}"
                                    ),
                                    Some(1),
                                ));
                            }
                        };
                        let source_oid = match repo
                            .find_branch(&source_branch, git2::BranchType::Local)
                            .and_then(|branch| branch.get().peel_to_commit())
                        {
                            Ok(source_commit) => source_commit.id(),
                            Err(err) => {
                                return Ok(WorkflowNodeResult::failed(
                                    format!(
                                        "git.integrate_plan_branch could not inspect source branch `{source_branch}`: {err}"
                                    ),
                                    Some(1),
                                ));
                            }
                        };
                        Some((head_oid, source_oid))
                    }
                    Ok(_) => None,
                    Err(err) => {
                        return Ok(WorkflowNodeResult::failed(
                            format!(
                                "git.integrate_plan_branch could not inspect merge index: {err}"
                            ),
                            Some(1),
                        ));
                    }
                },
                Ok(_) => None,
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("git.integrate_plan_branch could not open repository: {err}"),
                        Some(1),
                    ));
                }
            };

            if let Some((head_oid, source_oid)) = finalize_in_progress {
                let commit_result = if squash {
                    crate::vcs::commit_in_progress_squash_in(
                        &execution_root,
                        &merge_message,
                        head_oid,
                    )
                } else {
                    crate::vcs::commit_in_progress_merge_in(
                        &execution_root,
                        &merge_message,
                        head_oid,
                        source_oid,
                    )
                };
                if let Err(err) = commit_result {
                    let summary = if squash {
                        format!("git.integrate_plan_branch finalize squash merge failed: {err}")
                    } else {
                        format!("git.integrate_plan_branch finalize merge failed: {err}")
                    };
                    return Ok(WorkflowNodeResult::failed(summary, Some(1)));
                }

                let _ = remove_file_if_exists(&sentinel);
                if delete_branch
                    && current_branch_name(&execution_root).as_deref()
                        != Some(source_branch.as_str())
                {
                    let _ = crate::vcs::delete_branch_in(&execution_root, &source_branch);
                }

                return Ok(WorkflowNodeResult::succeeded(
                    "git.integrate_plan_branch finalized resolved merge",
                ));
            }

            let merge_ready = match crate::vcs::prepare_merge_in(&execution_root, &source_branch) {
                Ok(crate::vcs::MergePreparation::Ready(ready)) => ready,
                Ok(crate::vcs::MergePreparation::Conflicted(_conflict)) => {
                    if let Some(parent) = sentinel.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    let payload = serde_json::json!({
                        "slug": slug,
                        "source_branch": source_branch,
                        "target_branch": target_branch,
                        "job_id": record.id,
                        "node_id": node.node_id,
                        "created_at": Utc::now().to_rfc3339(),
                    });
                    fs::write(&sentinel, serde_json::to_vec_pretty(&payload)?)?;

                    let mut result = WorkflowNodeResult::blocked(
                        "git.integrate_plan_branch detected merge conflicts",
                        Some(10),
                    );
                    result.artifacts_written = vec![JobArtifact::MergeSentinel {
                        slug: workflow_slug_from_record(record, node),
                    }];
                    result.payload_refs = vec![relative_path(project_root, &sentinel)];
                    return Ok(result);
                }
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("git.integrate_plan_branch failed: {err}"),
                        Some(1),
                    ));
                }
            };

            let head_tree_matches = match Repository::open(&execution_root) {
                Ok(repo) => match repo.head().and_then(|head| head.peel_to_commit()) {
                    Ok(head_commit) => head_commit.tree_id() == merge_ready.tree_oid,
                    Err(err) => {
                        return Ok(WorkflowNodeResult::failed(
                            format!(
                                "git.integrate_plan_branch could not inspect merge result: {err}"
                            ),
                            Some(1),
                        ));
                    }
                },
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("git.integrate_plan_branch could not open repository: {err}"),
                        Some(1),
                    ));
                }
            };

            if !head_tree_matches {
                let commit_result = if squash {
                    crate::vcs::commit_squashed_merge_in(
                        &execution_root,
                        &merge_message,
                        merge_ready,
                    )
                } else {
                    crate::vcs::commit_ready_merge_in(&execution_root, &merge_message, merge_ready)
                };
                if let Err(err) = commit_result {
                    let summary = if squash {
                        format!("git.integrate_plan_branch squash commit failed: {err}")
                    } else {
                        format!("git.integrate_plan_branch merge commit failed: {err}")
                    };
                    return Ok(WorkflowNodeResult::failed(summary, Some(1)));
                }
            }

            let _ = remove_file_if_exists(&sentinel);
            if delete_branch
                && current_branch_name(&execution_root).as_deref() != Some(source_branch.as_str())
            {
                let _ = crate::vcs::delete_branch_in(&execution_root, &source_branch);
            }

            Ok(WorkflowNodeResult::succeeded(
                "git.integrate_plan_branch merged source branch",
            ))
        }
        Some("git.save_worktree_patch") => {
            let patch = match crate::vcs::diff_binary_against_head_in(&execution_root) {
                Ok(bytes) => bytes,
                Err(_) => {
                    return Ok(WorkflowNodeResult::failed(
                        "git.save_worktree_patch could not produce patch",
                        Some(1),
                    ));
                }
            };
            let patch_path = command_patch_path(jobs_root, &record.id);
            if let Some(parent) = patch_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&patch_path, patch)?;
            let mut result = WorkflowNodeResult::succeeded("saved worktree patch");
            result.artifacts_written = vec![JobArtifact::CommandPatch {
                job_id: record.id.clone(),
            }];
            result.payload_refs = vec![relative_path(project_root, &patch_path)];
            result.metadata = Some(JobMetadata {
                patch_file: Some(relative_path(project_root, &patch_path)),
                patch_index: Some(1),
                patch_total: Some(1),
                ..JobMetadata::default()
            });
            Ok(result)
        }
        Some("patch.pipeline_prepare") => {
            let files = match parse_files_json(node) {
                Ok(files) => files,
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("patch.pipeline_prepare: {err}"),
                        Some(1),
                    ));
                }
            };
            for file in &files {
                let resolved = resolve_path_in_execution_root(&execution_root, file);
                if !resolved.exists() {
                    return Ok(WorkflowNodeResult::failed(
                        format!(
                            "patch.pipeline_prepare missing patch file {}",
                            resolved.display()
                        ),
                        Some(1),
                    ));
                }
            }

            let manifest_path = patch_pipeline_manifest_path(jobs_root, &record.id);
            if let Some(parent) = manifest_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let manifest = serde_json::json!({
                "job_id": record.id,
                "node_id": node.node_id,
                "files": files,
                "prepared_at": Utc::now().to_rfc3339(),
            });
            fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;

            let total = manifest["files"]
                .as_array()
                .map(|items| items.len())
                .unwrap_or(0);
            let mut result = WorkflowNodeResult::succeeded("patch pipeline prepared");
            result.payload_refs = vec![relative_path(project_root, &manifest_path)];
            result.metadata = Some(JobMetadata {
                patch_file: Some(relative_path(project_root, &manifest_path)),
                patch_index: Some(0),
                patch_total: Some(total),
                ..JobMetadata::default()
            });
            Ok(result)
        }
        Some("patch.execute_pipeline") => {
            let files = match parse_files_json(node) {
                Ok(files) => files,
                Err(err) => {
                    return Ok(WorkflowNodeResult::failed(
                        format!("patch.execute_pipeline: {err}"),
                        Some(1),
                    ));
                }
            };
            for file in &files {
                let path = resolve_path_in_execution_root(&execution_root, file);
                if let Err(err) = crate::vcs::apply_patch_file_with_index_in(&execution_root, &path)
                {
                    let summary = format!(
                        "patch.execute_pipeline failed applying {}: {}",
                        path.display(),
                        err
                    );
                    return Ok(WorkflowNodeResult::failed(summary, Some(1)));
                }
            }
            let mut result = WorkflowNodeResult::succeeded("patch pipeline executed");
            result.metadata = Some(JobMetadata {
                patch_index: Some(files.len()),
                patch_total: Some(files.len()),
                ..JobMetadata::default()
            });
            Ok(result)
        }
        Some("patch.pipeline_finalize") => {
            let patch = match crate::vcs::diff_binary_against_head_in(&execution_root) {
                Ok(bytes) => bytes,
                Err(_) => {
                    return Ok(WorkflowNodeResult::failed(
                        "patch.pipeline_finalize could not capture patch",
                        Some(1),
                    ));
                }
            };
            let patch_path = command_patch_path(jobs_root, &record.id);
            if let Some(parent) = patch_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&patch_path, &patch)?;

            let finalize_path = patch_pipeline_finalize_path(jobs_root, &record.id);
            let summary = serde_json::json!({
                "job_id": record.id,
                "node_id": node.node_id,
                "finalized_at": Utc::now().to_rfc3339(),
                "patch_path": relative_path(project_root, &patch_path),
            });
            fs::write(&finalize_path, serde_json::to_vec_pretty(&summary)?)?;

            let mut result = WorkflowNodeResult::succeeded("patch pipeline finalized");
            result.artifacts_written = vec![JobArtifact::CommandPatch {
                job_id: record.id.clone(),
            }];
            result.payload_refs = vec![
                relative_path(project_root, &patch_path),
                relative_path(project_root, &finalize_path),
            ];
            result.metadata = Some(JobMetadata {
                patch_file: Some(relative_path(project_root, &patch_path)),
                ..JobMetadata::default()
            });
            Ok(result)
        }
        Some("build.materialize_step") => {
            let build_id = first_non_empty_arg(&node.args, &["build_id"])
                .or_else(|| {
                    record
                        .metadata
                        .as_ref()
                        .and_then(|meta| meta.workflow_run_id.clone())
                })
                .unwrap_or_else(|| "workflow".to_string());
            let step_key = first_non_empty_arg(&node.args, &["step_key"])
                .unwrap_or_else(|| sanitize_workflow_component(&node.node_id));
            let slug = first_non_empty_arg(&node.args, &["slug", "plan"]);
            let branch = first_non_empty_arg(&node.args, &["branch"]);
            let target = first_non_empty_arg(&node.args, &["target", "target_branch"]);

            let step_path = execution_root
                .join(".vizier/implementation-plans/builds")
                .join(&build_id)
                .join("steps")
                .join(&step_key)
                .join("materialized.json");
            if let Some(parent) = step_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let payload = serde_json::json!({
                "build_id": build_id,
                "step_key": step_key,
                "job_id": record.id,
                "node_id": node.node_id,
                "args": node.args,
                "materialized_at": Utc::now().to_rfc3339(),
            });
            fs::write(&step_path, serde_json::to_vec_pretty(&payload)?)?;

            let mut artifacts = Vec::new();
            let mut owned_branches = Vec::new();
            if let (Some(slug), Some(branch)) = (slug.as_ref(), branch.as_ref()) {
                if ensure_local_branch_with_ownership(&execution_root, branch).unwrap_or(false) {
                    owned_branches.push(branch.clone());
                }
                artifacts.push(JobArtifact::PlanBranch {
                    slug: slug.clone(),
                    branch: branch.clone(),
                });
                let plan_abs = execution_root.join(crate::plan::plan_rel_path(slug));
                if !plan_abs.exists() {
                    let doc = crate::plan::render_plan_document(
                        &crate::plan::new_plan_id(),
                        slug,
                        branch,
                        "Generated by build.materialize_step",
                        "Build materialization placeholder.",
                    );
                    let _ = crate::plan::write_plan_file(&plan_abs, &doc);
                }
                if plan_abs.exists() {
                    artifacts.push(JobArtifact::PlanDoc {
                        slug: slug.clone(),
                        branch: branch.clone(),
                    });
                }
            }
            if let Some(target_branch) = target.as_ref() {
                if ensure_local_branch_with_ownership(&execution_root, target_branch)
                    .unwrap_or(false)
                {
                    owned_branches.push(target_branch.clone());
                }
                artifacts.push(JobArtifact::TargetBranch {
                    name: target_branch.clone(),
                });
            }
            owned_branches.sort();
            owned_branches.dedup();

            let mut result = WorkflowNodeResult::succeeded("build step materialized");
            result.artifacts_written = artifacts;
            result.payload_refs = vec![relative_path(project_root, &step_path)];
            result.metadata = Some(JobMetadata {
                build_pipeline: first_non_empty_arg(&node.args, &["pipeline"]),
                build_target: target,
                plan: slug,
                branch,
                ephemeral_owned_branches: (!owned_branches.is_empty()).then_some(owned_branches),
                ..JobMetadata::default()
            });
            Ok(result)
        }
        Some("merge.sentinel.write") => {
            let slug = workflow_slug_from_record(record, node);
            let sentinel = merge_sentinel_path(project_root, &slug);
            if let Some(parent) = sentinel.parent() {
                fs::create_dir_all(parent)?;
            }
            let payload = serde_json::json!({
                "slug": slug,
                "job_id": record.id,
                "node_id": node.node_id,
                "run_id": record.metadata.as_ref().and_then(|meta| meta.workflow_run_id.clone()),
                "source_branch": first_non_empty_arg(&node.args, &["branch", "source_branch"]).or_else(|| record.metadata.as_ref().and_then(|meta| meta.branch.clone())),
                "target_branch": first_non_empty_arg(&node.args, &["target", "target_branch"]).or_else(|| record.metadata.as_ref().and_then(|meta| meta.target.clone())),
                "written_at": Utc::now().to_rfc3339(),
            });
            fs::write(&sentinel, serde_json::to_vec_pretty(&payload)?)?;
            let mut result = WorkflowNodeResult::succeeded("merge sentinel written");
            result.artifacts_written = vec![JobArtifact::MergeSentinel {
                slug: workflow_slug_from_record(record, node),
            }];
            result.payload_refs = vec![relative_path(project_root, &sentinel)];
            Ok(result)
        }
        Some("merge.sentinel.clear") => {
            let slug = workflow_slug_from_record(record, node);
            let sentinel = merge_sentinel_path(project_root, &slug);
            remove_file_if_exists(&sentinel)?;
            if let Some(parent) = sentinel.parent()
                && parent.exists()
                && fs::read_dir(parent)?.next().is_none()
            {
                let _ = fs::remove_dir(parent);
            }
            let mut result = WorkflowNodeResult::succeeded("merge sentinel cleared");
            result.payload_refs = vec![relative_path(project_root, &sentinel)];
            Ok(result)
        }
        Some("command.run") => {
            let Some(script) = resolve_node_shell_script(node, None) else {
                return Ok(WorkflowNodeResult::failed(
                    "command.run requires args.command or args.script",
                    Some(1),
                ));
            };
            let (status, stdout, stderr) = run_shell_text_command(&execution_root, &script)?;
            print_stdout_text(&stdout);
            print_stderr_text(&stderr);
            let stderr_lines = stderr_lines_from_text(&stderr);
            if status == 0 {
                let mut result = WorkflowNodeResult::succeeded("command.run succeeded");
                if !stdout.is_empty() {
                    result.stdout_text = Some(stdout);
                }
                result.stderr_lines = stderr_lines;
                Ok(result)
            } else {
                let mut result = WorkflowNodeResult::failed(
                    format!("command.run failed (exit {status})"),
                    Some(status),
                );
                if !stdout.is_empty() {
                    result.stdout_text = Some(stdout);
                }
                result.stderr_lines = stderr_lines;
                Ok(result)
            }
        }
        Some("cicd.run") => {
            let default_gate_script = cicd_gate_config(node).map(|(script, _)| script);
            let Some(script) = resolve_node_shell_script(node, default_gate_script) else {
                return Ok(WorkflowNodeResult::failed(
                    "cicd.run requires args.command/args.script or a cicd gate script",
                    Some(1),
                ));
            };
            let (status, stdout, stderr) = run_shell_text_command(&execution_root, &script)?;
            print_stdout_text(&stdout);
            print_stderr_text(&stderr);
            let stderr_lines = stderr_lines_from_text(&stderr);
            if status == 0 {
                let mut result = WorkflowNodeResult::succeeded("cicd.run passed");
                if !stdout.is_empty() {
                    result.stdout_text = Some(stdout);
                }
                result.stderr_lines = stderr_lines;
                Ok(result)
            } else {
                let mut result = WorkflowNodeResult::failed(
                    format!("cicd.run failed (exit {status})"),
                    Some(status),
                );
                if !stdout.is_empty() {
                    result.stdout_text = Some(stdout);
                }
                result.stderr_lines = stderr_lines;
                Ok(result)
            }
        }
        Some(other) => Ok(WorkflowNodeResult::failed(
            format!("unsupported executor operation `{other}`"),
            Some(1),
        )),
        None => Ok(WorkflowNodeResult::failed(
            "missing executor operation in workflow metadata",
            Some(1),
        )),
    }
}
