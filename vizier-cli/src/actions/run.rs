use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::thread;
use std::time::Duration;

use serde_json::json;
use uuid::Uuid;
use vizier_core::display;

use crate::actions::shared::format_block;
use crate::cli::args::{RunCmd, RunFormatArg};
use crate::jobs;
use crate::workflow_templates::{self, ResolvedWorkflowSource, WorkflowTemplateInputSpec};

pub(crate) fn run_workflow(
    project_root: &Path,
    jobs_root: &Path,
    cmd: RunCmd,
) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = vizier_core::config::get_config();
    let source = workflow_templates::resolve_workflow_source(project_root, &cmd.flow, &cfg)?;
    let input_spec = workflow_templates::load_template_input_spec(&source)?;
    let mut set_overrides = parse_set_overrides(&cmd.set)?;
    apply_named_input_aliases(&source, &input_spec, &mut set_overrides)?;
    apply_positional_inputs(&source, &input_spec, &cmd.inputs, &mut set_overrides)?;
    let template = workflow_templates::load_template_with_params(&source, &set_overrides)?;
    validate_entrypoint_input_requirements(&source, &input_spec, &cmd.inputs, &template)?;

    let run_id = format!("run_{}", Uuid::new_v4().simple());
    let enqueue = jobs::enqueue_workflow_run(
        project_root,
        jobs_root,
        &run_id,
        &source.selector,
        &template,
        &std::env::args().collect::<Vec<_>>(),
        None,
    )?;

    let mut job_ids = enqueue.job_ids.values().cloned().collect::<Vec<_>>();
    job_ids.sort();
    let mut root_jobs = resolve_root_jobs(jobs_root, &job_ids)?;

    if let Some(alias) = source.command_alias.as_ref() {
        annotate_alias_metadata(jobs_root, &job_ids, alias.as_str())?;
    }

    if !cmd.after.is_empty() {
        for root in &root_jobs {
            let dependencies =
                jobs::resolve_after_dependencies_for_enqueue(jobs_root, root, &cmd.after)?;
            apply_after_dependencies(jobs_root, root, &dependencies)?;
        }
    }

    let approval_override = if cmd.require_approval {
        Some(true)
    } else if cmd.no_require_approval {
        Some(false)
    } else {
        None
    };
    if let Some(required) = approval_override {
        for root in &root_jobs {
            apply_approval_override(jobs_root, root, required)?;
        }
    }

    // Trigger initial scheduling once after enqueue and root-level overrides.
    let binary = std::env::current_exe()?;
    let _ = jobs::scheduler_tick(project_root, jobs_root, &binary)?;

    root_jobs.sort();
    if !cmd.follow {
        emit_enqueue_summary(cmd.format, &source, &enqueue, &root_jobs)?;
        return Ok(());
    }

    let terminal = follow_run(
        project_root,
        jobs_root,
        &binary,
        &run_id,
        &job_ids,
        cmd.format,
    )?;
    emit_follow_summary(cmd.format, &source, &enqueue, &root_jobs, &terminal)?;

    if terminal.exit_code == 0 {
        Ok(())
    } else {
        std::process::exit(terminal.exit_code)
    }
}

fn parse_set_overrides(
    values: &[String],
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let mut out = BTreeMap::new();
    for raw in values {
        let trimmed = raw.trim();
        let Some((key, value)) = trimmed.split_once('=') else {
            return Err(format!("invalid --set value `{raw}`; expected KEY=VALUE").into());
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(format!("invalid --set value `{raw}`; key cannot be empty").into());
        }
        out.insert(key.to_string(), value.to_string());
    }
    Ok(out)
}

fn apply_named_input_aliases(
    source: &ResolvedWorkflowSource,
    input_spec: &WorkflowTemplateInputSpec,
    set_overrides: &mut BTreeMap<String, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if input_spec.named.is_empty() {
        return Ok(());
    }

    let declared_params = input_spec
        .params
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let aliases = input_spec
        .named
        .iter()
        .map(|(alias, target)| (alias.trim().replace('-', "_"), target.trim().to_string()))
        .collect::<Vec<_>>();

    for (alias, target) in &aliases {
        if alias.is_empty() {
            return Err(format!(
                "workflow `{}` has an empty [cli].named alias key",
                source.selector
            )
            .into());
        }
        if target.is_empty() {
            return Err(format!(
                "workflow `{}` alias `{alias}` has an empty [cli].named target",
                source.selector
            )
            .into());
        }
        if !declared_params.contains(target.as_str()) {
            return Err(format!(
                "workflow `{}` alias `{alias}` maps to unknown parameter `{target}`",
                source.selector
            )
            .into());
        }
    }

    for (alias, target) in aliases {
        if alias == target {
            continue;
        }

        let Some(value) = set_overrides.remove(&alias) else {
            continue;
        };

        if set_overrides.contains_key(&target) {
            return Err(format!(
                "workflow parameter `{target}` was provided multiple ways (`--{}` alias and explicit override)",
                alias.replace('_', "-")
            )
            .into());
        }
        set_overrides.insert(target, value);
    }

    Ok(())
}

fn apply_positional_inputs(
    source: &ResolvedWorkflowSource,
    input_spec: &WorkflowTemplateInputSpec,
    positional_values: &[String],
    set_overrides: &mut BTreeMap<String, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if positional_values.is_empty() {
        return Ok(());
    }

    if input_spec.positional.is_empty() {
        return Err(format!(
            "workflow `{}` does not define positional inputs; use named flags (for example `--param value`) or `--set key=value`",
            source.selector
        )
        .into());
    }

    if positional_values.len() > input_spec.positional.len() {
        return Err(format!(
            "workflow `{}` accepts at most {} positional input(s): {}",
            source.selector,
            input_spec.positional.len(),
            input_spec.positional.join(", ")
        )
        .into());
    }

    let declared_params = input_spec
        .params
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    for key in &input_spec.positional {
        if key.trim().is_empty() {
            return Err(format!(
                "workflow `{}` has an empty positional mapping entry",
                source.selector
            )
            .into());
        }
        if !declared_params.contains(key.as_str()) {
            return Err(format!(
                "workflow `{}` positional input `{key}` is not declared in [params]",
                source.selector
            )
            .into());
        }
    }

    for (index, value) in positional_values.iter().enumerate() {
        let key = &input_spec.positional[index];
        if set_overrides.contains_key(key) {
            return Err(format!(
                "workflow parameter `{key}` was provided multiple ways (positional input {} and named override)",
                index + 1
            )
            .into());
        }
        set_overrides.insert(key.clone(), value.clone());
    }

    Ok(())
}

fn validate_entrypoint_input_requirements(
    source: &ResolvedWorkflowSource,
    input_spec: &WorkflowTemplateInputSpec,
    positional_values: &[String],
    template: &vizier_core::workflow_template::WorkflowTemplate,
) -> Result<(), Box<dyn std::error::Error>> {
    if template.nodes.is_empty() {
        return Ok(());
    }

    let mut incoming_success = HashSet::<String>::new();
    for node in &template.nodes {
        for target in &node.on.succeeded {
            incoming_success.insert(target.clone());
        }
    }

    let mut resolved_after = BTreeMap::new();
    for node in &template.nodes {
        resolved_after.insert(node.id.clone(), format!("preflight-{}", node.id));
    }

    let declared_params = input_spec
        .params
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();

    for node in &template.nodes {
        if !node.after.is_empty() || incoming_success.contains(&node.id) {
            continue;
        }

        let compiled = vizier_core::workflow_template::compile_workflow_node(
            template,
            &node.id,
            &resolved_after,
        )
        .map_err(|err| {
            format!(
                "workflow `{}` failed entry-node preflight for `{}`: {err}",
                source.selector, node.id
            )
        })?;

        let Some(operation) = compiled.executor_operation.as_deref() else {
            continue;
        };
        let Some(required_arg_keys) =
            vizier_core::workflow_template::executor_non_empty_any_of_arg_keys(operation)
        else {
            continue;
        };

        let has_required_value = required_arg_keys.iter().any(|key| {
            node.args
                .get(*key)
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
        });
        if has_required_value {
            continue;
        }

        let node_arg_keys = required_arg_keys
            .iter()
            .filter(|key| node.args.contains_key(**key))
            .map(|key| (*key).to_string())
            .collect::<Vec<_>>();

        let mut required_inputs = node_arg_keys
            .iter()
            .filter(|key| declared_params.contains(key.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if required_inputs.is_empty() {
            required_inputs = if node_arg_keys.is_empty() {
                required_arg_keys
                    .iter()
                    .map(|key| (*key).to_string())
                    .collect::<Vec<_>>()
            } else {
                node_arg_keys
            };
        }
        required_inputs.sort_by_key(|key| {
            input_spec
                .positional
                .iter()
                .position(|entry| entry == key)
                .unwrap_or(usize::MAX)
        });

        return Err(build_entrypoint_input_error(
            source,
            input_spec,
            positional_values,
            &node.id,
            operation,
            &required_inputs,
        )
        .into());
    }

    Ok(())
}

fn build_entrypoint_input_error(
    source: &ResolvedWorkflowSource,
    input_spec: &WorkflowTemplateInputSpec,
    positional_values: &[String],
    node_id: &str,
    operation: &str,
    required_inputs: &[String],
) -> String {
    let flow_label = source
        .command_alias
        .as_ref()
        .map(|alias| alias.as_str().to_string())
        .unwrap_or_else(|| source.selector.clone());
    let required_flags = required_inputs
        .iter()
        .map(|param| {
            if let Some(alias) = preferred_cli_alias_for_param(input_spec, param)
                && alias != param
            {
                return format!(
                    "`--{}` (maps to `{}`)",
                    kebab_case_key(alias),
                    kebab_case_key(param)
                );
            }
            format!("`--{}`", kebab_case_key(param))
        })
        .collect::<Vec<_>>();

    let mut lines = vec![format!(
        "workflow `{}` entry node `{}` (`{operation}`) requires at least one non-empty input: {}",
        flow_label,
        node_id,
        required_flags.join(", ")
    )];

    if !positional_values.is_empty() && !input_spec.positional.is_empty() {
        let mapped = positional_values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let key = input_spec
                    .positional
                    .get(index)
                    .map(String::as_str)
                    .unwrap_or("extra_input");
                format!("{}=`{value}`", cli_label_for_param(input_spec, key))
            })
            .collect::<Vec<_>>();
        lines.push(format!("received positional inputs: {}", mapped.join(", ")));
    }

    let named_param = required_inputs
        .iter()
        .min_by_key(|param| {
            input_spec
                .positional
                .iter()
                .position(|entry| entry == *param)
                .unwrap_or(usize::MAX)
        })
        .or_else(|| required_inputs.first());

    let named_example = named_param
        .map(|param| {
            format!(
                "vizier run {} --{} {}",
                flow_label,
                kebab_case_key(&cli_label_for_param(input_spec, param)),
                cli_placeholder_for_param(input_spec, param)
            )
        })
        .unwrap_or_else(|| format!("vizier run {flow_label}"));

    let positional_example = required_inputs
        .iter()
        .filter_map(|param| {
            input_spec
                .positional
                .iter()
                .position(|entry| entry == param)
                .map(|index| (param, index))
        })
        .min_by_key(|(_, index)| *index)
        .map(|(_, index)| {
            let placeholders = input_spec.positional[..=index]
                .iter()
                .map(|param| cli_placeholder_for_param(input_spec, param))
                .collect::<Vec<_>>();
            format!("vizier run {} {}", flow_label, placeholders.join(" "))
        });

    lines.push("examples:".to_string());
    lines.push(format!("  {named_example}"));
    if let Some(example) = positional_example
        && example != named_example
    {
        lines.push(format!("  {example}"));
    }

    lines.join("\n")
}

fn preferred_cli_alias_for_param<'a>(
    input_spec: &'a WorkflowTemplateInputSpec,
    param: &str,
) -> Option<&'a str> {
    input_spec.named.iter().find_map(|(alias, target)| {
        if target == param {
            Some(alias.as_str())
        } else {
            None
        }
    })
}

fn cli_label_for_param(input_spec: &WorkflowTemplateInputSpec, param: &str) -> String {
    preferred_cli_alias_for_param(input_spec, param)
        .unwrap_or(param)
        .to_string()
}

fn cli_placeholder_for_param(input_spec: &WorkflowTemplateInputSpec, param: &str) -> String {
    format!(
        "<{}>",
        kebab_case_key(&cli_label_for_param(input_spec, param))
    )
}

fn kebab_case_key(value: &str) -> String {
    value.trim().replace('_', "-")
}

fn resolve_root_jobs(
    jobs_root: &Path,
    job_ids: &[String],
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut roots = Vec::new();
    for job_id in job_ids {
        let record = jobs::read_record(jobs_root, job_id)?;
        let schedule = record.schedule.unwrap_or_default();
        if schedule.after.is_empty() {
            roots.push(job_id.clone());
        }
    }
    roots.sort();
    roots.dedup();
    Ok(roots)
}

fn annotate_alias_metadata(
    jobs_root: &Path,
    job_ids: &[String],
    alias: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    for job_id in job_ids {
        jobs::update_job_record(jobs_root, job_id, |record| {
            let metadata = record.metadata.get_or_insert_with(Default::default);
            metadata.command_alias = Some(alias.to_string());
            metadata.scope = Some(alias.to_string());
        })?;
    }
    Ok(())
}

fn apply_after_dependencies(
    jobs_root: &Path,
    job_id: &str,
    dependencies: &[jobs::JobAfterDependency],
) -> Result<(), Box<dyn std::error::Error>> {
    jobs::update_job_record(jobs_root, job_id, |record| {
        let schedule = record.schedule.get_or_insert_with(Default::default);
        for dependency in dependencies {
            if schedule.after.iter().any(|existing| {
                existing.job_id == dependency.job_id && existing.policy == dependency.policy
            }) {
                continue;
            }
            schedule.after.push(dependency.clone());
        }
    })?;
    Ok(())
}

fn apply_approval_override(
    jobs_root: &Path,
    job_id: &str,
    required: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    jobs::update_job_record(jobs_root, job_id, |record| {
        let schedule = record.schedule.get_or_insert_with(Default::default);
        if required {
            schedule.approval = Some(jobs::pending_job_approval());
        } else {
            schedule.approval = None;
        }
    })?;
    Ok(())
}

fn emit_enqueue_summary(
    format: RunFormatArg,
    source: &ResolvedWorkflowSource,
    enqueue: &jobs::EnqueueWorkflowRunResult,
    root_jobs: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    if matches!(format, RunFormatArg::Json) {
        let payload = json!({
            "outcome": "workflow_run_enqueued",
            "run_id": enqueue.run_id,
            "workflow_template_selector": source.selector,
            "workflow_template_id": enqueue.template_id,
            "workflow_template_version": enqueue.template_version,
            "root_job_ids": root_jobs,
            "next": {
                "schedule": "vizier jobs schedule",
                "show": "vizier jobs show <job-id>",
                "tail": "vizier jobs tail <job-id> --follow"
            }
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let next_hint = if let Some(root) = root_jobs.first() {
        format!(
            "vizier jobs schedule --job {root}\nvizier jobs show {root}\nvizier jobs tail {root} --follow"
        )
    } else {
        "vizier jobs schedule".to_string()
    };

    println!(
        "{}",
        format_block(vec![
            ("Outcome".to_string(), "Workflow run enqueued".to_string()),
            ("Run".to_string(), enqueue.run_id.clone()),
            (
                "Template".to_string(),
                format!("{}@{}", enqueue.template_id, enqueue.template_version),
            ),
            ("Selector".to_string(), source.selector.clone(),),
            (
                "Root jobs".to_string(),
                if root_jobs.is_empty() {
                    "none".to_string()
                } else {
                    root_jobs.join(", ")
                },
            ),
            ("Next".to_string(), next_hint),
        ])
    );

    Ok(())
}

#[derive(Debug, Clone)]
struct FollowResult {
    exit_code: i32,
    terminal_state: String,
    succeeded: Vec<String>,
    failed: Vec<String>,
    blocked: Vec<String>,
    cancelled: Vec<String>,
}

fn follow_run(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    run_id: &str,
    job_ids: &[String],
    format: RunFormatArg,
) -> Result<FollowResult, Box<dyn std::error::Error>> {
    let stream_logs = matches!(format, RunFormatArg::Text);
    let mut last_status = HashMap::<String, jobs::JobStatus>::new();
    let mut last_log_line = HashMap::<String, String>::new();

    loop {
        let _ = jobs::scheduler_tick(project_root, jobs_root, binary)?;

        let mut succeeded = Vec::new();
        let mut failed = Vec::new();
        let mut blocked = Vec::new();
        let mut cancelled = Vec::new();

        for job_id in job_ids {
            let record = jobs::read_record(jobs_root, job_id)?;
            let status = record.status;

            if stream_logs {
                if last_status.get(job_id) != Some(&status) {
                    println!("[run:{run_id}] {job_id} => {}", jobs::status_label(status));
                    last_status.insert(job_id.clone(), status);
                }
                if let Some(line) = jobs::latest_job_log_line(jobs_root, job_id, 2048)? {
                    let marker = format!("{}:{}", line.stream.label(), line.line);
                    if last_log_line.get(job_id) != Some(&marker) {
                        println!("[{job_id}/{}] {}", line.stream.label(), line.line);
                        last_log_line.insert(job_id.clone(), marker);
                    }
                }
            }

            match status {
                jobs::JobStatus::Succeeded => succeeded.push(job_id.clone()),
                jobs::JobStatus::Failed => failed.push(job_id.clone()),
                jobs::JobStatus::Cancelled => cancelled.push(job_id.clone()),
                jobs::JobStatus::BlockedByDependency | jobs::JobStatus::BlockedByApproval => {
                    blocked.push(job_id.clone())
                }
                jobs::JobStatus::Queued
                | jobs::JobStatus::WaitingOnDeps
                | jobs::JobStatus::WaitingOnApproval
                | jobs::JobStatus::WaitingOnLocks
                | jobs::JobStatus::Running => {}
            }
        }

        let terminal_count = succeeded.len() + failed.len() + blocked.len() + cancelled.len();
        if terminal_count == job_ids.len() {
            succeeded.sort();
            failed.sort();
            blocked.sort();
            cancelled.sort();

            let (terminal_state, exit_code) = if !failed.is_empty() || !cancelled.is_empty() {
                ("failed".to_string(), 1)
            } else if !blocked.is_empty() {
                ("blocked".to_string(), 10)
            } else {
                ("succeeded".to_string(), 0)
            };

            return Ok(FollowResult {
                exit_code,
                terminal_state,
                succeeded,
                failed,
                blocked,
                cancelled,
            });
        }

        thread::sleep(Duration::from_millis(120));
    }
}

fn emit_follow_summary(
    format: RunFormatArg,
    source: &ResolvedWorkflowSource,
    enqueue: &jobs::EnqueueWorkflowRunResult,
    root_jobs: &[String],
    result: &FollowResult,
) -> Result<(), Box<dyn std::error::Error>> {
    if matches!(format, RunFormatArg::Json) {
        let payload = json!({
            "outcome": "workflow_run_terminal",
            "terminal_state": result.terminal_state,
            "exit_code": result.exit_code,
            "run_id": enqueue.run_id,
            "workflow_template_selector": source.selector,
            "workflow_template_id": enqueue.template_id,
            "workflow_template_version": enqueue.template_version,
            "root_job_ids": root_jobs,
            "succeeded": result.succeeded,
            "failed": result.failed,
            "blocked": result.blocked,
            "cancelled": result.cancelled,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let outcome = match result.terminal_state.as_str() {
        "succeeded" => "Workflow run succeeded",
        "blocked" => "Workflow run blocked",
        _ => "Workflow run failed",
    };

    let mut rows = vec![
        ("Outcome".to_string(), outcome.to_string()),
        ("Run".to_string(), enqueue.run_id.clone()),
        (
            "Template".to_string(),
            format!("{}@{}", enqueue.template_id, enqueue.template_version),
        ),
        ("Selector".to_string(), source.selector.clone()),
        (
            "Root jobs".to_string(),
            if root_jobs.is_empty() {
                "none".to_string()
            } else {
                root_jobs.join(", ")
            },
        ),
        ("Exit".to_string(), result.exit_code.to_string()),
    ];

    if !result.succeeded.is_empty() {
        rows.push(("Succeeded".to_string(), result.succeeded.join(", ")));
    }
    if !result.blocked.is_empty() {
        rows.push(("Blocked".to_string(), result.blocked.join(", ")));
    }
    if !result.failed.is_empty() {
        rows.push(("Failed".to_string(), result.failed.join(", ")));
    }
    if !result.cancelled.is_empty() {
        rows.push(("Cancelled".to_string(), result.cancelled.join(", ")));
    }

    println!("{}", format_block(rows));
    if result.exit_code == 10 {
        display::warn("run reached a blocked terminal state");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_set_overrides_accepts_last_write_wins() {
        let parsed = parse_set_overrides(&[
            "one=1".to_string(),
            "two=2".to_string(),
            "one=3".to_string(),
        ])
        .expect("parse overrides");

        assert_eq!(parsed.get("one"), Some(&"3".to_string()));
        assert_eq!(parsed.get("two"), Some(&"2".to_string()));
    }

    #[test]
    fn parse_set_overrides_rejects_missing_equals() {
        let err = parse_set_overrides(&["missing".to_string()]).expect_err("expected error");
        assert!(
            err.to_string().contains("expected KEY=VALUE"),
            "unexpected error: {err}"
        );
    }

    fn test_source() -> ResolvedWorkflowSource {
        ResolvedWorkflowSource {
            selector: "draft".to_string(),
            path: std::path::PathBuf::from(".vizier/workflow/draft.toml"),
            command_alias: None,
        }
    }

    #[test]
    fn apply_named_input_aliases_maps_alias_to_declared_param() {
        let source = test_source();
        let input_spec = WorkflowTemplateInputSpec {
            params: vec!["slug".to_string(), "spec_file".to_string()],
            positional: vec![],
            named: BTreeMap::from([
                ("name".to_string(), "slug".to_string()),
                ("file".to_string(), "spec_file".to_string()),
            ]),
        };
        let mut overrides = BTreeMap::from([
            ("name".to_string(), "my-change".to_string()),
            ("file".to_string(), "specs/DEFAULT.md".to_string()),
        ]);

        apply_named_input_aliases(&source, &input_spec, &mut overrides).expect("map named aliases");

        assert_eq!(overrides.get("slug"), Some(&"my-change".to_string()));
        assert_eq!(
            overrides.get("spec_file"),
            Some(&"specs/DEFAULT.md".to_string())
        );
        assert!(
            !overrides.contains_key("name") && !overrides.contains_key("file"),
            "aliases should be replaced by canonical params: {overrides:?}"
        );
    }

    #[test]
    fn apply_named_input_aliases_rejects_alias_plus_explicit_target() {
        let source = test_source();
        let input_spec = WorkflowTemplateInputSpec {
            params: vec!["slug".to_string()],
            positional: vec![],
            named: BTreeMap::from([("name".to_string(), "slug".to_string())]),
        };
        let mut overrides = BTreeMap::from([
            ("name".to_string(), "alpha".to_string()),
            ("slug".to_string(), "beta".to_string()),
        ]);

        let err =
            apply_named_input_aliases(&source, &input_spec, &mut overrides).expect_err("conflict");
        assert!(
            err.to_string().contains("provided multiple ways"),
            "unexpected error: {err}"
        );
    }
}
