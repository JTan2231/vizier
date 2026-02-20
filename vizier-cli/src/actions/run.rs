use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;
use vizier_core::display;

use crate::actions::shared::format_block;
use crate::actions::workflow_preflight::prepare_workflow_template;
use crate::cli::args::{RunCmd, RunFormatArg};
use crate::jobs;
use crate::workflow_templates::ResolvedWorkflowSource;

const RUN_AFTER_PREFIX: &str = "run:";
const RUN_ID_PREFIX: &str = "run_";

#[derive(Debug)]
enum AfterReference {
    JobId(String),
    RunId(String),
}

#[derive(Debug, Deserialize, Default)]
struct AfterDependencyRunManifest {
    #[serde(default)]
    nodes: BTreeMap<String, AfterDependencyRunNode>,
}

#[derive(Debug, Deserialize, Default)]
struct AfterDependencyRunNode {
    #[serde(default)]
    job_id: String,
    #[serde(default)]
    routes: AfterDependencyRunRoutes,
}

#[derive(Debug, Deserialize, Default)]
struct AfterDependencyRunRoutes {
    #[serde(default)]
    succeeded: Vec<serde_json::Value>,
}

#[derive(Debug, Clone)]
struct EnqueuedRunSummary {
    index: u32,
    run_id: String,
    enqueue: jobs::EnqueueWorkflowRunResult,
    job_ids: Vec<String>,
    root_jobs: Vec<String>,
}

#[derive(Debug, Clone)]
struct FollowedRunSummary {
    index: u32,
    run_id: String,
    terminal: FollowResult,
}

pub(crate) fn run_workflow(
    project_root: &Path,
    jobs_root: &Path,
    cmd: RunCmd,
) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = vizier_core::config::get_config();
    let prepared = prepare_workflow_template(project_root, &cmd.flow, &cmd.inputs, &cmd.set, &cfg)?;
    let source = prepared.source;
    let template = prepared.template;

    if cmd.check {
        jobs::validate_workflow_run_template(&template)?;
        emit_validation_summary(cmd.format, &source, &template)?;
        return Ok(());
    }

    let repeat = cmd.repeat.get();
    let approval_override = if cmd.require_approval {
        Some(true)
    } else if cmd.no_require_approval {
        Some(false)
    } else {
        None
    };
    let binary = std::env::current_exe()?;
    let invocation_args = std::env::args().collect::<Vec<_>>();

    if repeat == 1 {
        let run_id = format!("run_{}", Uuid::new_v4().simple());
        let enqueue = jobs::enqueue_workflow_run(
            project_root,
            jobs_root,
            &run_id,
            &source.selector,
            &template,
            &invocation_args,
            None,
        )?;

        let mut job_ids = enqueue.job_ids.values().cloned().collect::<Vec<_>>();
        job_ids.sort();
        let mut root_jobs = resolve_root_jobs(jobs_root, &job_ids)?;

        if let Some(alias) = source.command_alias.as_ref() {
            annotate_alias_metadata(jobs_root, &job_ids, alias.as_str())?;
        }

        let normalized_after = normalize_after_dependencies(jobs_root, &cmd.after)?;
        if !normalized_after.is_empty() {
            for root in &root_jobs {
                let dependencies = jobs::resolve_after_dependencies_for_enqueue(
                    jobs_root,
                    root,
                    &normalized_after,
                )?;
                apply_after_dependencies(jobs_root, root, &dependencies)?;
            }
        }

        if let Some(required) = approval_override {
            for root in &root_jobs {
                apply_approval_override(jobs_root, root, required)?;
            }
        }

        // Trigger initial scheduling once after enqueue and root-level overrides.
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
            return Ok(());
        }
        std::process::exit(terminal.exit_code);
    }

    let mut summaries = Vec::<EnqueuedRunSummary>::with_capacity(repeat as usize);
    let mut previous_run_id = None::<String>;
    for index in 1..=repeat {
        let run_id = format!("run_{}", Uuid::new_v4().simple());
        let enqueue = jobs::enqueue_workflow_run(
            project_root,
            jobs_root,
            &run_id,
            &source.selector,
            &template,
            &invocation_args,
            None,
        )?;

        let mut job_ids = enqueue.job_ids.values().cloned().collect::<Vec<_>>();
        job_ids.sort();
        let mut root_jobs = resolve_root_jobs(jobs_root, &job_ids)?;

        if let Some(alias) = source.command_alias.as_ref() {
            annotate_alias_metadata(jobs_root, &job_ids, alias.as_str())?;
        }

        let mut requested_after = cmd.after.clone();
        if let Some(previous) = previous_run_id.as_ref() {
            requested_after.push(format!("{RUN_AFTER_PREFIX}{previous}"));
        }
        let normalized_after = normalize_after_dependencies(jobs_root, &requested_after)?;
        if !normalized_after.is_empty() {
            for root in &root_jobs {
                let dependencies = jobs::resolve_after_dependencies_for_enqueue(
                    jobs_root,
                    root,
                    &normalized_after,
                )?;
                apply_after_dependencies(jobs_root, root, &dependencies)?;
            }
        }

        if let Some(required) = approval_override {
            for root in &root_jobs {
                apply_approval_override(jobs_root, root, required)?;
            }
        }

        // Keep deterministic startup by applying per-iteration root overrides before ticking.
        let _ = jobs::scheduler_tick(project_root, jobs_root, &binary)?;

        root_jobs.sort();
        summaries.push(EnqueuedRunSummary {
            index,
            run_id: run_id.clone(),
            enqueue,
            job_ids,
            root_jobs,
        });
        previous_run_id = Some(run_id);
    }

    if !cmd.follow {
        emit_repeat_enqueue_summary(cmd.format, &source, repeat, &summaries)?;
        return Ok(());
    }

    let mut followed_runs = Vec::<FollowedRunSummary>::new();
    for summary in &summaries {
        let terminal = follow_run(
            project_root,
            jobs_root,
            &binary,
            &summary.run_id,
            &summary.job_ids,
            cmd.format,
        )?;
        let should_stop = terminal.exit_code != 0;
        followed_runs.push(FollowedRunSummary {
            index: summary.index,
            run_id: summary.run_id.clone(),
            terminal,
        });
        if should_stop {
            break;
        }
    }

    let aggregate = followed_runs
        .last()
        .map(|entry| {
            (
                entry.terminal.terminal_state.clone(),
                entry.terminal.exit_code,
            )
        })
        .unwrap_or_else(|| ("succeeded".to_string(), 0));

    emit_repeat_follow_summary(
        cmd.format,
        &source,
        repeat,
        &summaries,
        &followed_runs,
        aggregate.0.as_str(),
        aggregate.1,
    )?;

    if aggregate.1 == 0 {
        Ok(())
    } else {
        std::process::exit(aggregate.1)
    }
}

fn normalize_after_dependencies(
    jobs_root: &Path,
    requested_after: &[String],
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut deduped = Vec::new();
    let mut seen = HashSet::new();

    for raw in requested_after {
        match parse_after_reference(raw)? {
            AfterReference::JobId(job_id) => {
                if seen.insert(job_id.clone()) {
                    deduped.push(job_id);
                }
            }
            AfterReference::RunId(run_id) => {
                let expanded = expand_run_after_reference(jobs_root, &run_id)?;
                for job_id in expanded {
                    if seen.insert(job_id.clone()) {
                        deduped.push(job_id);
                    }
                }
            }
        }
    }

    Ok(deduped)
}

fn parse_after_reference(raw: &str) -> Result<AfterReference, Box<dyn std::error::Error>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("unknown --after job id: <empty>".into());
    }

    if let Some(run_id) = trimmed.strip_prefix(RUN_AFTER_PREFIX) {
        let run_id = run_id.trim();
        if run_id.is_empty() {
            return Err(
                "invalid --after run reference `run:`; expected `run:<run_id>`"
                    .to_string()
                    .into(),
            );
        }
        return Ok(AfterReference::RunId(run_id.to_string()));
    }

    if trimmed.starts_with(RUN_ID_PREFIX) {
        return Err(format!(
            "invalid --after reference `{trimmed}`; use `run:{trimmed}` for run dependencies"
        )
        .into());
    }

    Ok(AfterReference::JobId(trimmed.to_string()))
}

fn expand_run_after_reference(
    jobs_root: &Path,
    run_id: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let manifest_path = jobs_root.join("runs").join(format!("{run_id}.json"));
    let bytes = fs::read(&manifest_path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            format!(
                "invalid --after run reference `run:{run_id}`: manifest not found at `{}`",
                manifest_path.display()
            )
        } else {
            format!(
                "invalid --after run reference `run:{run_id}`: unable to read manifest `{}`: {err}",
                manifest_path.display()
            )
        }
    })?;
    let manifest = serde_json::from_slice::<AfterDependencyRunManifest>(&bytes).map_err(|err| {
        format!(
            "invalid --after run reference `run:{run_id}`: unable to parse manifest `{}`: {err}",
            manifest_path.display()
        )
    })?;

    let mut sink_job_ids = Vec::new();
    let mut sink_job_to_node = HashMap::<String, String>::new();
    for (node_id, node) in &manifest.nodes {
        if !node.routes.succeeded.is_empty() {
            continue;
        }

        let sink_job_id = node.job_id.trim();
        if sink_job_id.is_empty() {
            return Err(format!(
                "invalid --after run reference `run:{run_id}`: sink node `{node_id}` has an empty job_id"
            )
            .into());
        }

        if let Some(existing_node_id) =
            sink_job_to_node.insert(sink_job_id.to_string(), node_id.clone())
        {
            return Err(format!(
                "invalid --after run reference `run:{run_id}`: duplicate sink job_id `{sink_job_id}` across nodes `{existing_node_id}` and `{node_id}`"
            )
            .into());
        }

        sink_job_ids.push(sink_job_id.to_string());
    }

    if sink_job_ids.is_empty() {
        return Err(format!(
            "invalid --after run reference `run:{run_id}`: manifest has no success-terminal sink nodes"
        )
        .into());
    }

    Ok(sink_job_ids)
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

fn emit_validation_summary(
    format: RunFormatArg,
    source: &ResolvedWorkflowSource,
    template: &vizier_core::workflow_template::WorkflowTemplate,
) -> Result<(), Box<dyn std::error::Error>> {
    if matches!(format, RunFormatArg::Json) {
        let payload = json!({
            "outcome": "workflow_validation_passed",
            "workflow_template_selector": source.selector,
            "workflow_template_id": &template.id,
            "workflow_template_version": &template.version,
            "node_count": template.nodes.len(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!(
        "{}",
        format_block(vec![
            (
                "Outcome".to_string(),
                "Workflow validation passed".to_string(),
            ),
            ("Selector".to_string(), source.selector.clone()),
            (
                "Template".to_string(),
                format!("{}@{}", template.id, template.version),
            ),
            ("Nodes".to_string(), template.nodes.len().to_string()),
        ])
    );

    Ok(())
}

fn emit_repeat_enqueue_summary(
    format: RunFormatArg,
    source: &ResolvedWorkflowSource,
    repeat: u32,
    summaries: &[EnqueuedRunSummary],
) -> Result<(), Box<dyn std::error::Error>> {
    if matches!(format, RunFormatArg::Json) {
        let runs = summaries
            .iter()
            .map(|summary| {
                json!({
                    "index": summary.index,
                    "run_id": &summary.run_id,
                    "workflow_template_id": &summary.enqueue.template_id,
                    "workflow_template_version": &summary.enqueue.template_version,
                    "root_job_ids": &summary.root_jobs,
                })
            })
            .collect::<Vec<_>>();
        let payload = json!({
            "outcome": "workflow_runs_enqueued",
            "repeat": repeat,
            "workflow_template_selector": source.selector,
            "runs": runs,
            "next": {
                "schedule": "vizier jobs schedule",
                "show": "vizier jobs show <job-id>",
                "tail": "vizier jobs tail <job-id> --follow"
            }
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let run_ids = summaries
        .iter()
        .map(|summary| summary.run_id.clone())
        .collect::<Vec<_>>();
    let root_map = summaries
        .iter()
        .map(|summary| {
            let roots = if summary.root_jobs.is_empty() {
                "none".to_string()
            } else {
                summary.root_jobs.join(", ")
            };
            format!("{}: {roots}", summary.run_id)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let template = summaries
        .first()
        .map(|summary| {
            format!(
                "{}@{}",
                summary.enqueue.template_id, summary.enqueue.template_version
            )
        })
        .unwrap_or_else(|| "unknown".to_string());
    let next_hint = summaries
        .first()
        .and_then(|summary| summary.root_jobs.first())
        .map(|root| {
            format!(
                "vizier jobs schedule --job {root}\nvizier jobs show {root}\nvizier jobs tail {root} --follow"
            )
        })
        .unwrap_or_else(|| "vizier jobs schedule".to_string());

    println!(
        "{}",
        format_block(vec![
            ("Outcome".to_string(), "Workflow runs enqueued".to_string()),
            ("Repeat".to_string(), repeat.to_string()),
            ("Runs".to_string(), run_ids.join(", ")),
            ("Template".to_string(), template),
            ("Selector".to_string(), source.selector.clone()),
            ("Root jobs".to_string(), root_map),
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

fn emit_repeat_follow_summary(
    format: RunFormatArg,
    source: &ResolvedWorkflowSource,
    repeat: u32,
    summaries: &[EnqueuedRunSummary],
    followed_runs: &[FollowedRunSummary],
    terminal_state: &str,
    exit_code: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    if matches!(format, RunFormatArg::Json) {
        let runs = followed_runs
            .iter()
            .map(|entry| {
                json!({
                    "index": entry.index,
                    "run_id": &entry.run_id,
                    "terminal_state": &entry.terminal.terminal_state,
                    "exit_code": entry.terminal.exit_code,
                    "succeeded": &entry.terminal.succeeded,
                    "failed": &entry.terminal.failed,
                    "blocked": &entry.terminal.blocked,
                    "cancelled": &entry.terminal.cancelled,
                })
            })
            .collect::<Vec<_>>();
        let payload = json!({
            "outcome": "workflow_runs_terminal",
            "repeat": repeat,
            "terminal_state": terminal_state,
            "exit_code": exit_code,
            "workflow_template_selector": source.selector,
            "runs": runs,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let outcome = match terminal_state {
        "succeeded" => "Workflow runs succeeded",
        "blocked" => "Workflow runs blocked",
        _ => "Workflow runs failed",
    };
    let all_runs = summaries
        .iter()
        .map(|summary| summary.run_id.clone())
        .collect::<Vec<_>>();
    let followed = followed_runs
        .iter()
        .map(|entry| entry.run_id.clone())
        .collect::<Vec<_>>();
    let run_states = followed_runs
        .iter()
        .map(|entry| {
            format!(
                "#{} {} => {} ({})",
                entry.index, entry.run_id, entry.terminal.terminal_state, entry.terminal.exit_code
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let template = summaries
        .first()
        .map(|summary| {
            format!(
                "{}@{}",
                summary.enqueue.template_id, summary.enqueue.template_version
            )
        })
        .unwrap_or_else(|| "unknown".to_string());

    let mut rows = vec![
        ("Outcome".to_string(), outcome.to_string()),
        ("Repeat".to_string(), repeat.to_string()),
        ("Runs".to_string(), all_runs.join(", ")),
        (
            "Followed".to_string(),
            if followed.is_empty() {
                "none".to_string()
            } else {
                followed.join(", ")
            },
        ),
        ("Template".to_string(), template),
        ("Selector".to_string(), source.selector.clone()),
        ("Exit".to_string(), exit_code.to_string()),
    ];
    if !run_states.is_empty() {
        rows.push(("Run states".to_string(), run_states));
    }

    println!("{}", format_block(rows));
    if exit_code == 10 {
        display::warn("run reached a blocked terminal state");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_after_reference_rejects_bare_run_id() {
        let err = parse_after_reference("run_deadbeef").expect_err("expected run-id guidance");
        assert!(
            err.to_string().contains("use `run:run_deadbeef`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn normalize_after_dependencies_expands_run_sinks_and_dedupes() {
        let temp = TempDir::new().expect("temp dir");
        let runs_dir = temp.path().join("runs");
        std::fs::create_dir_all(&runs_dir).expect("create runs dir");
        std::fs::write(
            runs_dir.join("run_prev.json"),
            r#"{
  "nodes": {
    "node_a": {
      "job_id": "job-a",
      "routes": { "succeeded": [] }
    },
    "node_b": {
      "job_id": "job-b",
      "routes": { "succeeded": [] }
    },
    "node_c": {
      "job_id": "job-c",
      "routes": { "succeeded": [{ "node_id": "node_d", "mode": "propagate_context" }] }
    }
  }
}"#,
        )
        .expect("write run manifest");

        let dependencies = normalize_after_dependencies(
            temp.path(),
            &[
                "run:run_prev".to_string(),
                "manual-job".to_string(),
                "job-a".to_string(),
                "run:run_prev".to_string(),
            ],
        )
        .expect("resolve dependencies");
        assert_eq!(dependencies, vec!["job-a", "job-b", "manual-job"]);
    }

    #[test]
    fn expand_run_after_reference_rejects_manifests_without_success_sinks() {
        let temp = TempDir::new().expect("temp dir");
        let runs_dir = temp.path().join("runs");
        std::fs::create_dir_all(&runs_dir).expect("create runs dir");
        std::fs::write(
            runs_dir.join("run_prev.json"),
            r#"{
  "nodes": {
    "node_only": {
      "job_id": "job-only",
      "routes": { "succeeded": [{ "node_id": "node_only", "mode": "propagate_context" }] }
    }
  }
}"#,
        )
        .expect("write run manifest");

        let err = expand_run_after_reference(temp.path(), "run_prev")
            .expect_err("expected zero-sink error");
        assert!(
            err.to_string().contains("no success-terminal sink nodes"),
            "unexpected error: {err}"
        );
    }
}
