use std::collections::HashSet;
use std::path::Path;

use git2::Repository;
use serde_json::{Map, Value, json};
use vizier_core::{
    config,
    display::{format_label_value_block, format_number},
};

use crate::actions::shared::format_table;
use crate::cli::args::{
    JobsAction, JobsCmd, JobsListField, JobsScheduleFormatArg, JobsShowField, normalize_labels,
    parse_fields, resolve_label,
};
use crate::jobs::{self, JobStatus};

fn join_or_none(items: Vec<String>) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

fn format_wait_reason(reason: &jobs::JobWaitReason) -> String {
    let detail = reason
        .detail
        .clone()
        .unwrap_or_else(|| "waiting".to_string());
    format!("{:?}: {detail}", reason.kind).to_lowercase()
}

fn format_after_policy(policy: jobs::AfterPolicy) -> &'static str {
    match policy {
        jobs::AfterPolicy::Success => "success",
    }
}

fn format_after_dependencies(after: &[jobs::JobAfterDependency]) -> String {
    join_or_none(
        after
            .iter()
            .map(|dependency| {
                format!(
                    "{} ({})",
                    dependency.job_id,
                    format_after_policy(dependency.policy)
                )
            })
            .collect(),
    )
}

fn format_waited_on(waited_on: &[jobs::JobWaitKind]) -> String {
    join_or_none(
        waited_on
            .iter()
            .map(|kind| format!("{:?}", kind).to_lowercase())
            .collect(),
    )
}

#[derive(Clone, Copy, Debug)]
enum ScheduleFormat {
    Dag,
    Json,
}

fn schedule_status_visible(status: JobStatus) -> bool {
    matches!(
        status,
        JobStatus::Queued
            | JobStatus::WaitingOnDeps
            | JobStatus::WaitingOnLocks
            | JobStatus::Running
            | JobStatus::BlockedByDependency
    )
}

fn schedule_roots(
    graph: &jobs::ScheduleGraph,
    include_all: bool,
    focus: Option<&str>,
    max_depth: usize,
) -> Vec<String> {
    let mut roots = Vec::new();
    let job_order = graph.job_ids_sorted();

    if let Some(focus_id) = focus {
        let focus_set = graph.collect_focus_jobs(focus_id, max_depth);
        if focus_set.is_empty() {
            return roots;
        }
        for job_id in job_order {
            if !focus_set.contains(&job_id) {
                continue;
            }
            let record = graph.record(&job_id);
            let visible = include_all
                || record
                    .map(|record| schedule_status_visible(record.status))
                    .unwrap_or(false);
            if visible || job_id == focus_id {
                roots.push(job_id);
            }
        }
        if let Some(index) = roots.iter().position(|job_id| job_id == focus_id) {
            let focus = roots.remove(index);
            roots.insert(0, focus);
        }
        return roots;
    }

    for job_id in job_order {
        let record = graph.record(&job_id);
        if include_all
            || record
                .map(|record| schedule_status_visible(record.status))
                .unwrap_or(false)
        {
            roots.push(job_id);
        }
    }

    roots
}

fn format_schedule_job_line(record: &jobs::JobRecord) -> String {
    let mut parts = vec![format!(
        "{} {}",
        record.id,
        jobs::status_label(record.status)
    )];

    let mut metadata_parts = Vec::new();
    if let Some(metadata) = record.metadata.as_ref() {
        if let Some(scope) = metadata.scope.as_ref() {
            metadata_parts.push(scope.clone());
        }
        if let Some(plan) = metadata.plan.as_ref() {
            metadata_parts.push(plan.clone());
        }
        if let Some(target) = metadata.target.as_ref() {
            metadata_parts.push(target.clone());
        }
    }
    if !metadata_parts.is_empty() {
        parts.push(format!("[{}]", metadata_parts.join("/")));
    }

    if let Some(wait_reason) = record
        .schedule
        .as_ref()
        .and_then(|sched| sched.wait_reason.as_ref())
    {
        let detail = wait_reason
            .detail
            .clone()
            .unwrap_or_else(|| "waiting".to_string());
        parts.push(format!("[wait: {detail}]"));
    }

    if let Some(locks) = record.schedule.as_ref().map(|sched| &sched.locks)
        && !locks.is_empty()
    {
        let formatted = join_or_none(
            locks
                .iter()
                .map(|lock| format!("{}:{:?}", lock.key, lock.mode).to_lowercase())
                .collect(),
        );
        parts.push(format!("[locks: {formatted}]"));
    }

    if let Some(pinned) = record
        .schedule
        .as_ref()
        .and_then(|sched| sched.pinned_head.as_ref())
    {
        parts.push(format!("[pinned: {}@{}]", pinned.branch, pinned.oid));
    }

    parts.join(" ")
}

struct ScheduleEntry {
    label: String,
    producer: Option<String>,
}

fn render_schedule_dependencies(
    graph: &jobs::ScheduleGraph,
    repo: &Repository,
    job_id: &str,
    depth_remaining: usize,
    prefix: &str,
    path: &mut HashSet<String>,
) {
    if depth_remaining == 0 {
        return;
    }

    let mut entries = Vec::new();
    for dependency in graph.after_for(job_id) {
        let status = graph
            .record(&dependency.job_id)
            .map(|record| jobs::status_label(record.status))
            .unwrap_or("missing");
        entries.push(ScheduleEntry {
            label: format!(
                "after:{} -> {} {}",
                format_after_policy(dependency.policy),
                dependency.job_id,
                status
            ),
            producer: Some(dependency.job_id),
        });
    }
    for dependency in graph.dependencies_for(job_id) {
        let artifact_label = jobs::format_artifact(&dependency);
        let producers = graph.producers_for(&dependency);
        if producers.is_empty() {
            let state = graph.artifact_state(repo, &dependency);
            entries.push(ScheduleEntry {
                label: format!("{artifact_label} -> [{}]", state.label()),
                producer: None,
            });
        } else {
            for producer_id in producers {
                let status = graph
                    .record(&producer_id)
                    .map(|record| jobs::status_label(record.status))
                    .unwrap_or("unknown");
                entries.push(ScheduleEntry {
                    label: format!("{artifact_label} -> {producer_id} {status}"),
                    producer: Some(producer_id),
                });
            }
        }
    }

    for (index, entry) in entries.iter().enumerate() {
        let last = index + 1 == entries.len();
        let branch = if last { "`-- " } else { "|-- " };
        println!("{prefix}{branch}{}", entry.label);
        if let Some(producer_id) = entry.producer.as_ref()
            && depth_remaining > 1
            && !path.contains(producer_id)
        {
            path.insert(producer_id.clone());
            let child_prefix = if last {
                format!("{prefix}    ")
            } else {
                format!("{prefix}|   ")
            };
            render_schedule_dependencies(
                graph,
                repo,
                producer_id,
                depth_remaining - 1,
                &child_prefix,
                path,
            );
            path.remove(producer_id);
        }
    }
}

fn render_schedule_dag(
    graph: &jobs::ScheduleGraph,
    repo: &Repository,
    roots: &[String],
    max_depth: usize,
) {
    println!("Schedule (DAG)");
    for (index, job_id) in roots.iter().enumerate() {
        if let Some(record) = graph.record(job_id) {
            if index > 0 {
                println!();
            }
            println!("{}", format_schedule_job_line(record));
            let mut path = HashSet::new();
            path.insert(job_id.clone());
            render_schedule_dependencies(graph, repo, job_id, max_depth, "", &mut path);
        }
    }
}

fn jobs_list_field_value(field: JobsListField, record: &jobs::JobRecord) -> Option<String> {
    let schedule = record.schedule.as_ref();
    match field {
        JobsListField::Job => Some(record.id.clone()),
        JobsListField::Status => Some(jobs::status_label(record.status).to_string()),
        JobsListField::Created => Some(record.created_at.to_rfc3339()),
        JobsListField::After => schedule.map(|sched| format_after_dependencies(&sched.after)),
        JobsListField::Dependencies => schedule.map(|sched| {
            join_or_none(
                sched
                    .dependencies
                    .iter()
                    .map(|dep| jobs::format_artifact(&dep.artifact))
                    .collect(),
            )
        }),
        JobsListField::Locks => schedule.map(|sched| {
            join_or_none(
                sched
                    .locks
                    .iter()
                    .map(|lock| format!("{}:{:?}", lock.key, lock.mode).to_lowercase())
                    .collect(),
            )
        }),
        JobsListField::Wait => {
            schedule.and_then(|sched| sched.wait_reason.as_ref().map(format_wait_reason))
        }
        JobsListField::WaitedOn => schedule.map(|sched| format_waited_on(&sched.waited_on)),
        JobsListField::PinnedHead => schedule.and_then(|sched| {
            sched
                .pinned_head
                .as_ref()
                .map(|pinned| format!("{}@{}", pinned.branch, pinned.oid))
        }),
        JobsListField::Artifacts => schedule
            .map(|sched| join_or_none(sched.artifacts.iter().map(jobs::format_artifact).collect())),
        JobsListField::Failed => {
            if record.status == JobStatus::Failed {
                record
                    .finished_at
                    .map(|value| value.to_rfc3339())
                    .or_else(|| Some("unknown".to_string()))
            } else {
                None
            }
        }
        JobsListField::Command => {
            if record.command.is_empty() {
                Some("<command unavailable>".to_string())
            } else {
                Some(record.command.join(" "))
            }
        }
    }
}

fn jobs_show_field_value(field: JobsShowField, record: &jobs::JobRecord) -> Option<String> {
    let metadata = record.metadata.as_ref();
    let schedule = record.schedule.as_ref();
    match field {
        JobsShowField::Job => Some(record.id.clone()),
        JobsShowField::Status => Some(jobs::status_label(record.status).to_string()),
        JobsShowField::Pid => record.pid.map(|pid| pid.to_string()),
        JobsShowField::Started => record.started_at.map(|value| value.to_rfc3339()),
        JobsShowField::Finished => record.finished_at.map(|value| value.to_rfc3339()),
        JobsShowField::ExitCode => record.exit_code.map(|code| code.to_string()),
        JobsShowField::Stdout => Some(record.stdout_path.clone()),
        JobsShowField::Stderr => Some(record.stderr_path.clone()),
        JobsShowField::Session => record.session_path.clone(),
        JobsShowField::Outcome => record.outcome_path.clone(),
        JobsShowField::Scope => metadata.and_then(|meta| meta.scope.clone()),
        JobsShowField::Plan => metadata.and_then(|meta| meta.plan.clone()),
        JobsShowField::Target => metadata.and_then(|meta| meta.target.clone()),
        JobsShowField::Branch => metadata.and_then(|meta| meta.branch.clone()),
        JobsShowField::Revision => metadata.and_then(|meta| meta.revision.clone()),
        JobsShowField::After => schedule.map(|sched| format_after_dependencies(&sched.after)),
        JobsShowField::Dependencies => schedule.map(|sched| {
            join_or_none(
                sched
                    .dependencies
                    .iter()
                    .map(|dep| jobs::format_artifact(&dep.artifact))
                    .collect(),
            )
        }),
        JobsShowField::Locks => schedule.map(|sched| {
            join_or_none(
                sched
                    .locks
                    .iter()
                    .map(|lock| format!("{}:{:?}", lock.key, lock.mode).to_lowercase())
                    .collect(),
            )
        }),
        JobsShowField::Wait => {
            schedule.and_then(|sched| sched.wait_reason.as_ref().map(format_wait_reason))
        }
        JobsShowField::WaitedOn => schedule.map(|sched| format_waited_on(&sched.waited_on)),
        JobsShowField::PinnedHead => schedule.and_then(|sched| {
            sched
                .pinned_head
                .as_ref()
                .map(|pinned| format!("{}@{}", pinned.branch, pinned.oid))
        }),
        JobsShowField::Artifacts => schedule
            .map(|sched| join_or_none(sched.artifacts.iter().map(jobs::format_artifact).collect())),
        JobsShowField::Worktree => metadata.and_then(|meta| meta.worktree_path.clone()),
        JobsShowField::WorktreeName => metadata.and_then(|meta| meta.worktree_name.clone()),
        JobsShowField::AgentBackend => metadata.and_then(|meta| meta.agent_backend.clone()),
        JobsShowField::AgentLabel => metadata.and_then(|meta| meta.agent_label.clone()),
        JobsShowField::AgentCommand => {
            metadata.and_then(|meta| meta.agent_command.as_ref().map(|cmd| cmd.join(" ")))
        }
        JobsShowField::AgentExit => {
            metadata.and_then(|meta| meta.agent_exit_code.map(|code| code.to_string()))
        }
        JobsShowField::CancelCleanup => metadata.and_then(|meta| {
            meta.cancel_cleanup_status
                .map(|status| status.label().to_string())
        }),
        JobsShowField::CancelCleanupError => {
            metadata.and_then(|meta| meta.cancel_cleanup_error.clone())
        }
        JobsShowField::ConfigSnapshot => record
            .config_snapshot
            .as_ref()
            .map(|value| value.to_string()),
        JobsShowField::Command => {
            if record.command.is_empty() {
                None
            } else {
                Some(record.command.join(" "))
            }
        }
    }
}

pub(crate) fn run_jobs_command(
    project_root: &Path,
    jobs_root: &Path,
    cmd: JobsCmd,
    follow: bool,
    emit_json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match cmd.action {
        JobsAction::List {
            all,
            dismiss_failures,
            format,
        } => {
            let mut list_config = config::get_config().display.lists.jobs.clone();
            if let Some(fmt) = format {
                list_config.format = fmt.into();
            }
            if emit_json {
                list_config.format = config::ListFormat::Json;
            }

            let show_succeeded = if all {
                true
            } else {
                list_config.show_succeeded
            };
            let records = jobs::list_records(jobs_root)?;
            if records.is_empty() {
                if matches!(list_config.format, config::ListFormat::Json) {
                    let payload = json!({
                        "header": { "outcome": "No background jobs found" },
                        "jobs": [],
                    });
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                } else {
                    println!("Outcome: No background jobs found");
                }
                return Ok(());
            }

            let hidden_summary =
                |hidden_failed: usize, hidden_succeeded: usize| -> Option<String> {
                    let mut parts = Vec::new();
                    if hidden_failed > 0 {
                        parts.push(format!("{} failed", format_number(hidden_failed)));
                    }
                    if hidden_succeeded > 0 {
                        parts.push(format!("{} succeeded", format_number(hidden_succeeded)));
                    }
                    if parts.is_empty() {
                        None
                    } else {
                        Some(format!("{} (use --all to include)", parts.join(", ")))
                    }
                };

            let mut hidden_succeeded = 0usize;
            let mut hidden_failed = 0usize;
            let mut visible = Vec::new();
            for record in records {
                if !show_succeeded && record.status == JobStatus::Succeeded {
                    hidden_succeeded += 1;
                } else if !all && dismiss_failures && record.status == JobStatus::Failed {
                    hidden_failed += 1;
                } else {
                    visible.push(record);
                }
            }

            let hidden_label = hidden_summary(hidden_failed, hidden_succeeded);
            let outcome = if visible.is_empty() {
                let hidden_total = hidden_failed + hidden_succeeded;
                if hidden_total == 0 {
                    "No background jobs found".to_string()
                } else if hidden_failed > 0 {
                    "No visible background jobs".to_string()
                } else {
                    "No active background jobs".to_string()
                }
            } else {
                format!(
                    "{} background job{}",
                    format_number(visible.len()),
                    if visible.len() == 1 { "" } else { "s" }
                )
            };

            let fields = parse_fields(
                "display.lists.jobs.fields",
                &list_config.fields,
                JobsListField::parse,
            );
            let labels = normalize_labels(&list_config.labels);

            if matches!(list_config.format, config::ListFormat::Json) {
                let mut header = Map::new();
                header.insert("outcome".to_string(), Value::String(outcome));
                if let Some(hidden) = hidden_label.as_ref() {
                    header.insert("hidden".to_string(), Value::String(hidden.clone()));
                }
                let mut jobs_json = Vec::new();
                for record in &visible {
                    let mut obj = Map::new();
                    for field in &fields {
                        let value = jobs_list_field_value(*field, record);
                        if let Some(value) = value {
                            obj.insert(field.json_key().to_string(), Value::String(value));
                        }
                    }
                    jobs_json.push(Value::Object(obj));
                }
                let payload = json!({
                    "header": header,
                    "jobs": jobs_json,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
                return Ok(());
            }

            let mut header_rows = vec![(resolve_label(&labels, "Outcome"), outcome)];
            if let Some(hidden) = hidden_label.as_ref() {
                header_rows.push((resolve_label(&labels, "Hidden"), hidden.clone()));
            }
            let header_block = format_label_value_block(&header_rows, 0);
            if !header_block.is_empty() {
                println!("{header_block}");
            }

            if visible.is_empty() {
                return Ok(());
            }

            println!();

            match list_config.format {
                config::ListFormat::Table => {
                    let mut rows = Vec::new();
                    let header = fields
                        .iter()
                        .map(|field| resolve_label(&labels, field.label()))
                        .collect::<Vec<_>>();
                    if !header.is_empty() {
                        rows.push(header);
                    }

                    for record in &visible {
                        let mut row = Vec::new();
                        for field in &fields {
                            let value = jobs_list_field_value(*field, record);
                            row.push(value.unwrap_or_default());
                        }
                        rows.push(row);
                    }
                    let table = format_table(&rows, 0);
                    if !table.is_empty() {
                        println!("{table}");
                    }
                }
                _ => {
                    let mut blocks = Vec::new();
                    for record in &visible {
                        let mut rows = Vec::new();
                        for field in &fields {
                            let value = jobs_list_field_value(*field, record).unwrap_or_default();
                            rows.push((resolve_label(&labels, field.label()), value));
                        }
                        let block = format_label_value_block(&rows, 2);
                        if !block.is_empty() {
                            blocks.push(block);
                        }
                    }
                    if !blocks.is_empty() {
                        println!("{}", blocks.join("\n\n"));
                    }
                }
            }
            Ok(())
        }
        JobsAction::Schedule {
            all,
            job,
            format,
            max_depth,
        } => {
            let schedule_format = if emit_json {
                ScheduleFormat::Json
            } else {
                match format {
                    Some(JobsScheduleFormatArg::Json) => ScheduleFormat::Json,
                    _ => ScheduleFormat::Dag,
                }
            };

            let records = jobs::list_records(jobs_root)?;
            let graph = jobs::ScheduleGraph::new(records);
            let roots = schedule_roots(&graph, all, job.as_deref(), max_depth);
            if roots.is_empty() {
                if matches!(schedule_format, ScheduleFormat::Json) {
                    let snapshot = jobs::ScheduleSnapshot {
                        nodes: Vec::new(),
                        edges: Vec::new(),
                    };
                    println!("{}", serde_json::to_string_pretty(&snapshot)?);
                } else {
                    println!("Outcome: No scheduled jobs");
                }
                return Ok(());
            }

            let repo = Repository::discover(project_root)?;
            match schedule_format {
                ScheduleFormat::Dag => render_schedule_dag(&graph, &repo, &roots, max_depth),
                ScheduleFormat::Json => {
                    let snapshot = graph.snapshot(&repo, &roots, max_depth);
                    println!("{}", serde_json::to_string_pretty(&snapshot)?);
                }
            }
            Ok(())
        }
        JobsAction::Show { job, format } => {
            let record = jobs::read_record(jobs_root, &job)?;
            let mut show_config = config::get_config().display.lists.jobs_show.clone();
            if let Some(fmt) = format {
                show_config.format = fmt.into();
            }
            if emit_json {
                show_config.format = config::ListFormat::Json;
            }

            let fields = parse_fields(
                "display.lists.jobs_show.fields",
                &show_config.fields,
                JobsShowField::parse,
            );
            let labels = normalize_labels(&show_config.labels);
            match show_config.format {
                config::ListFormat::Json => {
                    let mut obj = Map::new();
                    for field in &fields {
                        let value = match field {
                            JobsShowField::ConfigSnapshot => record.config_snapshot.clone(),
                            _ => jobs_show_field_value(*field, &record).map(Value::String),
                        };
                        if let Some(value) = value {
                            obj.insert(field.json_key().to_string(), value);
                        }
                    }
                    println!("{}", serde_json::to_string_pretty(&Value::Object(obj))?);
                }
                config::ListFormat::Table => {
                    let mut rows = Vec::new();
                    for field in &fields {
                        if let Some(value) = jobs_show_field_value(*field, &record) {
                            let label = resolve_label(&labels, field.label());
                            rows.push(vec![label, value]);
                        }
                    }
                    let table = format_table(&rows, 0);
                    if !table.is_empty() {
                        println!("{table}");
                    }
                }
                _ => {
                    let mut lines = Vec::new();
                    for field in &fields {
                        if let Some(value) = jobs_show_field_value(*field, &record) {
                            let label = resolve_label(&labels, field.label());
                            if matches!(field, JobsShowField::Job) {
                                lines.push(format!("{label} {value}"));
                            } else {
                                lines.push(format!("{label}: {value}"));
                            }
                        }
                    }
                    if !lines.is_empty() {
                        println!("{}", lines.join("\n"));
                    }
                }
            }
            Ok(())
        }
        JobsAction::Status { job } => {
            let record = jobs::read_record(jobs_root, &job)?;
            let exit = record
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "-".to_string());
            if emit_json {
                let payload = json!({
                    "job": record.id,
                    "status": jobs::status_label(record.status),
                    "exit_code": record.exit_code,
                    "stdout": record.stdout_path,
                    "stderr": record.stderr_path,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!(
                    "{} [{}] exit={} stdout={} stderr={}",
                    record.id,
                    jobs::status_label(record.status),
                    exit,
                    record.stdout_path,
                    record.stderr_path
                );
            }
            Ok(())
        }
        JobsAction::Tail { job, stream } => {
            jobs::tail_job_logs(jobs_root, &job, stream.into(), follow)
        }
        JobsAction::Attach { job } => {
            jobs::tail_job_logs(jobs_root, &job, jobs::LogStream::Both, true)
        }
        JobsAction::Cancel {
            job,
            cleanup_worktree,
            no_cleanup_worktree,
        } => {
            let cleanup_override = if cleanup_worktree {
                Some(true)
            } else if no_cleanup_worktree {
                Some(false)
            } else {
                None
            };
            let cleanup_enabled = cleanup_override
                .unwrap_or_else(|| config::get_config().jobs.cancel.cleanup_worktree);
            let outcome =
                jobs::cancel_job_with_cleanup(project_root, jobs_root, &job, cleanup_enabled)?;
            if outcome.cleanup.status == jobs::CancelCleanupStatus::Failed
                && let Some(err) = outcome.cleanup.error.as_ref()
            {
                vizier_core::display::warn(format!(
                    "cleanup failed for job {}: {}",
                    outcome.record.id, err
                ));
            }
            println!(
                "Job {} marked cancelled (stdout: {}, stderr: {}, cleanup={})",
                outcome.record.id,
                outcome.record.stdout_path,
                outcome.record.stderr_path,
                outcome.cleanup.status.label()
            );
            Ok(())
        }
        JobsAction::Gc { days } => {
            let removed =
                jobs::gc_jobs(project_root, jobs_root, chrono::Duration::days(days as i64))?;
            println!("Outcome: removed {} job(s)", removed);
            Ok(())
        }
    }
}
