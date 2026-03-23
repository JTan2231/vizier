use super::*;

pub(crate) fn execute_workflow_control(
    project_root: &Path,
    record: &JobRecord,
    node: &WorkflowRuntimeNodeManifest,
) -> Result<WorkflowNodeResult, Box<dyn std::error::Error>> {
    let execution_root = resolve_execution_root(project_root, record)?;
    match node.control_policy.as_deref() {
        Some("terminal") => {
            let has_routes = !node.routes.succeeded.is_empty()
                || !node.routes.failed.is_empty()
                || !node.routes.blocked.is_empty()
                || !node.routes.cancelled.is_empty();
            if has_routes {
                Ok(WorkflowNodeResult::failed(
                    "terminal policy node must not declare outgoing routes",
                    Some(1),
                ))
            } else {
                Ok(WorkflowNodeResult::succeeded("terminal sink reached"))
            }
        }
        Some("gate.stop_condition") => {
            let script = first_non_empty_arg(&node.args, &["script"])
                .or_else(|| script_gate_script(node))
                .unwrap_or_default();
            if script.is_empty() {
                return Ok(WorkflowNodeResult::succeeded(
                    "stop-condition gate skipped (no script configured)",
                ));
            }

            let (status, stdout, stderr) = run_shell_text_command(&execution_root, &script)?;
            print_stdout_text(&stdout);
            print_stderr_text(&stderr);
            let stderr_lines = stderr_lines_from_text(&stderr);
            if status == 0 {
                let mut result = WorkflowNodeResult::succeeded("stop-condition gate passed");
                if !stdout.is_empty() {
                    result.stdout_text = Some(stdout);
                }
                result.stderr_lines = stderr_lines;
                return Ok(result);
            }

            let attempt = record
                .metadata
                .as_ref()
                .and_then(|meta| meta.workflow_node_attempt)
                .unwrap_or(1);
            let retry_budget = node.retry.budget.saturating_add(1);
            if matches!(node.retry.mode, WorkflowRetryMode::UntilGate) && attempt > retry_budget {
                let result = WorkflowNodeResult {
                    outcome: WorkflowNodeOutcome::Blocked,
                    artifacts_written: Vec::new(),
                    payload_refs: Vec::new(),
                    metadata: None,
                    summary: Some(format!(
                        "stop-condition failed on attempt {attempt}; retry budget exhausted ({})",
                        node.retry.budget
                    )),
                    exit_code: Some(10),
                    stdout_text: if stdout.is_empty() {
                        None
                    } else {
                        Some(stdout)
                    },
                    stderr_lines,
                };
                return Ok(result);
            }

            let detail = stderr.trim();
            let summary = if detail.is_empty() {
                format!("stop-condition failed on attempt {attempt}")
            } else {
                format!("stop-condition failed on attempt {attempt}: {detail}")
            };
            let mut result = WorkflowNodeResult::failed(summary, Some(status));
            if !stdout.is_empty() {
                result.stdout_text = Some(stdout);
            }
            result.stderr_lines = stderr_lines;
            Ok(result)
        }
        Some("gate.conflict_resolution") => {
            let slug = workflow_slug_from_record(record, node);
            let sentinel = merge_sentinel_path(project_root, &slug);
            if !sentinel.exists() {
                return Ok(WorkflowNodeResult::succeeded(
                    "conflict-resolution gate skipped (no merge sentinel)",
                ));
            }

            let mut conflict_paths = list_unmerged_paths(&execution_root);
            let mut conflicts_present = !conflict_paths.is_empty();
            let auto_resolve = bool_arg(&node.args, "auto_resolve")
                .or_else(|| conflict_auto_resolve_from_gate(node))
                .unwrap_or(false);
            let mut stdout_text = String::new();
            let mut stderr_lines = Vec::new();
            let mut auto_resolve_warning: Option<String> = None;
            if conflicts_present && auto_resolve {
                if let Some(script) = resolve_node_shell_script(node, None) {
                    let (status, stdout, stderr) =
                        run_shell_text_command(&execution_root, &script)?;
                    print_stdout_text(&stdout);
                    print_stderr_text(&stderr);
                    if !stdout.is_empty() {
                        stdout_text.push_str(&stdout);
                    }
                    stderr_lines.extend(stderr_lines_from_text(&stderr));
                    if status != 0 {
                        let mut result = WorkflowNodeResult::failed(
                            format!("conflict auto-resolve script failed (exit {status})"),
                            Some(status),
                        );
                        if !stdout_text.is_empty() {
                            result.stdout_text = Some(stdout_text);
                        }
                        result.stderr_lines = stderr_lines;
                        return Ok(result);
                    }
                } else if let Some(detail) = run_merge_conflict_auto_resolve_agent(
                    &execution_root,
                    record,
                    node,
                    &sentinel,
                    &slug,
                    &conflict_paths,
                ) {
                    display::warn(detail.clone());
                    auto_resolve_warning = Some(detail);
                    if let Some(detail) = auto_resolve_warning.as_ref() {
                        stderr_lines.push(detail.clone());
                    }
                }
                conflict_paths = list_unmerged_paths(&execution_root);
                conflicts_present = !conflict_paths.is_empty();
            }

            if conflicts_present {
                let summary = if let Some(detail) = auto_resolve_warning {
                    format!("merge conflicts remain for slug `{slug}` ({detail})")
                } else {
                    format!("merge conflicts remain for slug `{slug}`")
                };
                let mut blocked = WorkflowNodeResult::blocked(summary, Some(10));
                blocked.artifacts_written = vec![JobArtifact::MergeSentinel { slug }];
                blocked.payload_refs = vec![relative_path(project_root, &sentinel)];
                if !stdout_text.is_empty() {
                    blocked.stdout_text = Some(stdout_text);
                }
                blocked.stderr_lines = stderr_lines;
                return Ok(blocked);
            }

            remove_file_if_exists(&sentinel)?;
            let mut result =
                WorkflowNodeResult::succeeded("merge conflicts resolved and sentinel cleared");
            if !stdout_text.is_empty() {
                result.stdout_text = Some(stdout_text);
            }
            result.stderr_lines = stderr_lines;
            Ok(result)
        }
        Some("gate.cicd") => {
            let gate_cfg = cicd_gate_config(node);
            let script = resolve_node_shell_script(
                node,
                gate_cfg.as_ref().map(|(script, _)| script.clone()),
            );
            let Some(script) = script else {
                return Ok(WorkflowNodeResult::succeeded(
                    "cicd gate skipped (no script configured)",
                ));
            };
            let auto_resolve = bool_arg(&node.args, "auto_resolve")
                .or_else(|| gate_cfg.as_ref().map(|(_, auto)| *auto))
                .unwrap_or(false);
            let attempt = record
                .metadata
                .as_ref()
                .and_then(|meta| meta.workflow_node_attempt)
                .unwrap_or(1);
            let mut stdout_text = String::new();
            let mut stderr_lines = Vec::new();

            let (status, stdout, stderr) = run_shell_text_command(&execution_root, &script)?;
            print_stdout_text(&stdout);
            print_stderr_text(&stderr);
            if !stdout.is_empty() {
                stdout_text.push_str(&stdout);
            }
            stderr_lines.extend(stderr_lines_from_text(&stderr));
            if status == 0 {
                let mut result =
                    WorkflowNodeResult::succeeded(format!("cicd gate passed on attempt {attempt}"));
                if !stdout_text.is_empty() {
                    result.stdout_text = Some(stdout_text);
                }
                result.stderr_lines = stderr_lines;
                return Ok(result);
            }

            if auto_resolve
                && let Some(fix_script) = first_non_empty_arg(
                    &node.args,
                    &["auto_resolve_command", "auto_resolve_script"],
                )
            {
                let (fix_status, fix_stdout, fix_stderr) =
                    run_shell_text_command(&execution_root, &fix_script)?;
                print_stdout_text(&fix_stdout);
                print_stderr_text(&fix_stderr);
                if !fix_stdout.is_empty() {
                    stdout_text.push_str(&fix_stdout);
                }
                stderr_lines.extend(stderr_lines_from_text(&fix_stderr));
                if fix_status == 0 {
                    let (retry_status, retry_stdout, retry_stderr) =
                        run_shell_text_command(&execution_root, &script)?;
                    print_stdout_text(&retry_stdout);
                    print_stderr_text(&retry_stderr);
                    if !retry_stdout.is_empty() {
                        stdout_text.push_str(&retry_stdout);
                    }
                    stderr_lines.extend(stderr_lines_from_text(&retry_stderr));
                    if retry_status == 0 {
                        let mut result = WorkflowNodeResult::succeeded(format!(
                            "cicd gate passed after auto-resolve on attempt {attempt}"
                        ));
                        if !stdout_text.is_empty() {
                            result.stdout_text = Some(stdout_text);
                        }
                        result.stderr_lines = stderr_lines;
                        return Ok(result);
                    }
                }
            }

            let mut result = WorkflowNodeResult::failed(
                format!("cicd gate failed on attempt {attempt} (exit {status})"),
                Some(status),
            );
            if !stdout_text.is_empty() {
                result.stdout_text = Some(stdout_text);
            }
            result.stderr_lines = stderr_lines;
            Ok(result)
        }
        Some("gate.approval") => {
            let required = bool_arg(&node.args, "required").unwrap_or(true);
            if !required {
                return Ok(WorkflowNodeResult::succeeded(
                    "approval gate bypassed (required=false)",
                ));
            }

            let approval = record
                .schedule
                .as_ref()
                .and_then(|schedule| schedule.approval.as_ref());
            let Some(approval) = approval else {
                return Ok(WorkflowNodeResult::blocked(
                    "approval gate blocked: no approval state present",
                    Some(10),
                ));
            };
            match approval.state {
                JobApprovalState::Approved => {
                    Ok(WorkflowNodeResult::succeeded("approval gate passed"))
                }
                JobApprovalState::Pending => Ok(WorkflowNodeResult::blocked(
                    "approval gate pending human decision",
                    Some(10),
                )),
                JobApprovalState::Rejected => {
                    let reason = approval
                        .reason
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or("approval rejected");
                    Ok(WorkflowNodeResult::failed(
                        format!("approval gate rejected: {reason}"),
                        Some(10),
                    ))
                }
            }
        }
        Some(other) => Ok(WorkflowNodeResult::failed(
            format!("unsupported control policy `{other}`"),
            Some(1),
        )),
        None => Ok(WorkflowNodeResult::failed(
            "missing control policy in workflow metadata",
            Some(1),
        )),
    }
}
