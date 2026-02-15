use std::collections::HashSet;
use std::io::{self, Write};
use std::path::Path;
use std::thread;
use std::time::Duration as StdDuration;

use chrono::Local;
use git2::Repository;
use serde_json::{Map, Value, json};
use vizier_core::{
    config,
    display::{format_label_value_block, format_number},
};

use crate::actions::shared::format_table;
use crate::cli::args::{
    JobsAction, JobsActionFormatArg, JobsCmd, JobsListField, JobsScheduleFormatArg, JobsShowField,
    normalize_labels, parse_fields, resolve_label,
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
    Summary,
    Dag,
    Json,
}

fn schedule_status_visible(status: JobStatus) -> bool {
    matches!(
        status,
        JobStatus::Queued
            | JobStatus::WaitingOnDeps
            | JobStatus::WaitingOnApproval
            | JobStatus::WaitingOnLocks
            | JobStatus::Running
            | JobStatus::BlockedByDependency
            | JobStatus::BlockedByApproval
    )
}

fn dependency_blocked_status(status: JobStatus) -> bool {
    matches!(
        status,
        JobStatus::WaitingOnDeps | JobStatus::BlockedByDependency
    )
}

fn failed_job_blocks_dependents(graph: &jobs::ScheduleGraph, job_id: &str) -> bool {
    let Some(record) = graph.record(job_id) else {
        return false;
    };
    if record.status != JobStatus::Failed {
        return false;
    }

    let mut dependents = graph.after_dependents_for(job_id);
    for artifact in graph.artifacts_for(job_id) {
        dependents.extend(graph.consumers_for(&artifact));
    }
    dependents.sort();
    dependents.dedup();

    dependents.into_iter().any(|dependent_id| {
        graph
            .record(&dependent_id)
            .map(|dependent| dependency_blocked_status(dependent.status))
            .unwrap_or(false)
    })
}

fn schedule_job_visible(graph: &jobs::ScheduleGraph, include_all: bool, job_id: &str) -> bool {
    if include_all {
        return true;
    }
    let Some(record) = graph.record(job_id) else {
        return false;
    };
    schedule_status_visible(record.status) || failed_job_blocks_dependents(graph, job_id)
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
            let visible = schedule_job_visible(graph, include_all, &job_id);
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
        if schedule_job_visible(graph, include_all, &job_id) {
            roots.push(job_id);
        }
    }

    roots
}

#[derive(Clone)]
struct ScheduleSummaryRow {
    order: usize,
    slug: Option<String>,
    name: String,
    status: JobStatus,
    wait: Option<String>,
    job_id: String,
    created_at: String,
}

fn command_preview(command: &[String]) -> String {
    if command.is_empty() {
        return "<command unavailable>".to_string();
    }

    let preview_len = command.len().min(3);
    let mut preview = command[..preview_len].join(" ");
    if command.len() > preview_len {
        preview.push_str(" ...");
    }
    preview
}

fn resolve_schedule_slug(graph: &jobs::ScheduleGraph, job_id: &str) -> Option<String> {
    let record = graph.record(job_id)?;
    if let Some(plan) = record
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.plan.as_ref())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        return Some(plan.to_string());
    }

    for artifact in graph.artifacts_for(job_id) {
        match artifact {
            jobs::JobArtifact::PlanBranch { slug, .. }
            | jobs::JobArtifact::PlanDoc { slug, .. }
            | jobs::JobArtifact::PlanCommits { slug, .. }
            | jobs::JobArtifact::MergeSentinel { slug } => return Some(slug),
            jobs::JobArtifact::TargetBranch { .. }
            | jobs::JobArtifact::CommandPatch { .. }
            | jobs::JobArtifact::Custom { .. } => {}
        }
    }

    None
}

fn resolve_schedule_name(record: &jobs::JobRecord) -> String {
    let metadata = record.metadata.as_ref();
    let scope = metadata
        .and_then(|meta| meta.command_alias.as_deref().or(meta.scope.as_deref()))
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let plan = metadata
        .and_then(|meta| meta.plan.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let target = metadata
        .and_then(|meta| meta.target.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let (Some(scope), Some(plan), Some(target)) = (scope, plan, target) {
        return format!("{scope}/{plan}/{target}");
    }
    if let (Some(scope), Some(target)) = (scope, target) {
        return format!("{scope}/{target}");
    }
    if let Some(scope) = scope {
        return scope.to_string();
    }

    command_preview(&record.command)
}

fn resolve_schedule_wait(record: &jobs::JobRecord) -> Option<String> {
    record
        .schedule
        .as_ref()
        .and_then(|schedule| schedule.wait_reason.as_ref())
        .map(|reason| {
            reason
                .detail
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.to_string())
                .unwrap_or_else(|| format!("{:?}", reason.kind).to_lowercase())
        })
}

fn schedule_summary_rows(graph: &jobs::ScheduleGraph, roots: &[String]) -> Vec<ScheduleSummaryRow> {
    let mut rows = Vec::new();
    for (index, job_id) in roots.iter().enumerate() {
        let Some(record) = graph.record(job_id) else {
            continue;
        };

        rows.push(ScheduleSummaryRow {
            order: index + 1,
            slug: resolve_schedule_slug(graph, job_id),
            name: resolve_schedule_name(record),
            status: record.status,
            wait: resolve_schedule_wait(record),
            job_id: record.id.clone(),
            created_at: record.created_at.to_rfc3339(),
        });
    }
    rows
}

fn schedule_summary_table_rows(rows: &[ScheduleSummaryRow]) -> Vec<Vec<String>> {
    let mut table_rows = vec![vec![
        "#".to_string(),
        "Slug".to_string(),
        "Name".to_string(),
        "Status".to_string(),
        "Wait".to_string(),
        "Job".to_string(),
    ]];

    for row in rows {
        table_rows.push(vec![
            row.order.to_string(),
            row.slug.clone().unwrap_or_default(),
            row.name.clone(),
            jobs::status_label(row.status).to_string(),
            row.wait.clone().unwrap_or_default(),
            row.job_id.clone(),
        ]);
    }

    table_rows
}

fn render_schedule_summary(rows: &[ScheduleSummaryRow]) {
    println!("Schedule (Summary)");
    let table = format_table(&schedule_summary_table_rows(rows), 0);
    if !table.is_empty() {
        println!("{table}");
    }
}

fn schedule_snapshot_jobs(rows: &[ScheduleSummaryRow]) -> Vec<jobs::ScheduleSnapshotJob> {
    rows.iter()
        .map(|row| jobs::ScheduleSnapshotJob {
            order: row.order,
            job_id: row.job_id.clone(),
            slug: row.slug.clone(),
            name: row.name.clone(),
            status: row.status,
            wait: row.wait.clone(),
            created_at: row.created_at.clone(),
        })
        .collect()
}

fn format_schedule_job_line(record: &jobs::JobRecord) -> String {
    let mut parts = vec![format!(
        "{} {}",
        record.id,
        jobs::status_label(record.status)
    )];

    let mut metadata_parts = Vec::new();
    if let Some(metadata) = record.metadata.as_ref() {
        if let Some(scope) = metadata.command_alias.as_ref().or(metadata.scope.as_ref()) {
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
    println!("Schedule (DAG, verbose)");
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

const WATCH_LOG_TAIL_BYTES: usize = 16 * 1024;
const WATCH_ANSI_CLEAR_AND_HOME: &str = "\x1b[2J\x1b[H";

#[derive(Debug, Default)]
struct ScheduleStatusCounts {
    queued: usize,
    waiting: usize,
    running: usize,
    blocked: usize,
    terminal: usize,
}

fn schedule_status_counts(rows: &[ScheduleSummaryRow]) -> ScheduleStatusCounts {
    let mut counts = ScheduleStatusCounts::default();
    for row in rows {
        match row.status {
            JobStatus::Queued => counts.queued += 1,
            JobStatus::WaitingOnDeps | JobStatus::WaitingOnApproval | JobStatus::WaitingOnLocks => {
                counts.waiting += 1;
            }
            JobStatus::Running => counts.running += 1,
            JobStatus::BlockedByDependency | JobStatus::BlockedByApproval => counts.blocked += 1,
            JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled => {
                counts.terminal += 1;
            }
        }
    }
    counts
}

fn select_watch_running_job(
    rows: &[ScheduleSummaryRow],
    focused_job: Option<&str>,
    top: usize,
) -> Option<String> {
    if let Some(job_id) = focused_job
        && rows
            .iter()
            .any(|row| row.job_id == job_id && row.status == JobStatus::Running)
    {
        return Some(job_id.to_string());
    }

    rows.iter()
        .take(top)
        .find(|row| row.status == JobStatus::Running)
        .map(|row| row.job_id.clone())
}

fn render_schedule_watch(
    rows: &[ScheduleSummaryRow],
    top: usize,
    interval_ms: u64,
    selected_running_job: Option<&str>,
    latest_line: Option<&jobs::LatestLogLine>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Schedule Watch | refreshed {} | interval={}ms | top={}\n",
        Local::now().to_rfc3339(),
        interval_ms,
        top
    ));

    let counts = schedule_status_counts(rows);
    out.push_str(&format!(
        "State: queued={} waiting={} running={} blocked={} terminal={}\n\n",
        counts.queued, counts.waiting, counts.running, counts.blocked, counts.terminal
    ));

    out.push_str("Schedule (Summary)\n");
    let visible_rows = rows.iter().take(top).cloned().collect::<Vec<_>>();
    if visible_rows.is_empty() {
        out.push_str("(no scheduled jobs)\n");
    } else {
        let table_rows = schedule_summary_table_rows(&visible_rows);
        let table = format_table(&table_rows, 0);
        out.push_str(&table);
        out.push('\n');
    }

    out.push('\n');
    out.push_str("Running Job Output\n");
    if let Some(job_id) = selected_running_job {
        out.push_str(&format!("Running job: {job_id}\n"));
        if let Some(line) = latest_line {
            out.push_str(&format!(
                "Latest line: [{}] {}\n",
                line.stream.label(),
                line.line
            ));
        } else {
            out.push_str("Latest line: (no output yet)\n");
        }
    } else {
        out.push_str("No running job\n");
        out.push_str("Latest line: (none)\n");
    }

    out
}

fn ensure_watch_mode_allowed(no_ansi: bool) -> Result<(), Box<dyn std::error::Error>> {
    let display_cfg = vizier_core::display::get_display_config();
    if display_cfg.stdout_is_tty && display_cfg.stderr_is_tty && !no_ansi {
        return Ok(());
    }

    Err(
        "`--watch` requires an interactive TTY with ANSI enabled; rerun without `--watch` for static output."
            .into(),
    )
}

fn run_schedule_watch_loop(
    jobs_root: &Path,
    all: bool,
    focused_job: Option<&str>,
    max_depth: usize,
    top: usize,
    interval_ms: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        let records = jobs::list_records(jobs_root)?;
        let graph = jobs::ScheduleGraph::new(records);
        let roots = schedule_roots(&graph, all, focused_job, max_depth);
        let rows = schedule_summary_rows(&graph, &roots);
        let selected_running_job = select_watch_running_job(&rows, focused_job, top);
        let latest_line = selected_running_job
            .as_deref()
            .and_then(|job_id| {
                jobs::latest_job_log_line(jobs_root, job_id, WATCH_LOG_TAIL_BYTES).ok()
            })
            .flatten();

        let frame = render_schedule_watch(
            &rows,
            top,
            interval_ms,
            selected_running_job.as_deref(),
            latest_line.as_ref(),
        );
        print!("{WATCH_ANSI_CLEAR_AND_HOME}{frame}");
        io::stdout().flush()?;
        thread::sleep(StdDuration::from_millis(interval_ms));
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
        JobsListField::ApprovalRequired => schedule.and_then(|sched| {
            sched
                .approval
                .as_ref()
                .map(|approval| approval.required.to_string())
        }),
        JobsListField::ApprovalState => schedule.and_then(|sched| {
            sched
                .approval
                .as_ref()
                .map(|approval| jobs::approval_state_label(approval.state).to_string())
        }),
        JobsListField::ApprovalDecidedBy => schedule.and_then(|sched| {
            sched
                .approval
                .as_ref()
                .and_then(|approval| approval.decided_by.clone())
        }),
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
        JobsShowField::Scope => {
            metadata.and_then(|meta| meta.command_alias.clone().or(meta.scope.clone()))
        }
        JobsShowField::Plan => metadata.and_then(|meta| meta.plan.clone()),
        JobsShowField::Target => metadata.and_then(|meta| meta.target.clone()),
        JobsShowField::Branch => metadata.and_then(|meta| meta.branch.clone()),
        JobsShowField::BuildPipeline => metadata.and_then(|meta| meta.build_pipeline.clone()),
        JobsShowField::BuildTarget => metadata.and_then(|meta| meta.build_target.clone()),
        JobsShowField::BuildReviewMode => metadata.and_then(|meta| meta.build_review_mode.clone()),
        JobsShowField::BuildSkipChecks => {
            metadata.and_then(|meta| meta.build_skip_checks.map(|value| value.to_string()))
        }
        JobsShowField::BuildKeepBranch => {
            metadata.and_then(|meta| meta.build_keep_branch.map(|value| value.to_string()))
        }
        JobsShowField::BuildDependencies => metadata.and_then(|meta| {
            meta.build_dependencies.as_ref().map(|values| {
                if values.is_empty() {
                    "none".to_string()
                } else {
                    values.join(", ")
                }
            })
        }),
        JobsShowField::WorkflowRun => metadata.and_then(|meta| meta.workflow_run_id.clone()),
        JobsShowField::WorkflowTemplate => metadata.and_then(|meta| {
            meta.workflow_template_selector
                .clone()
                .or(meta.workflow_template_id.clone())
        }),
        JobsShowField::WorkflowTemplateVersion => {
            metadata.and_then(|meta| meta.workflow_template_version.clone())
        }
        JobsShowField::WorkflowNode => metadata.and_then(|meta| meta.workflow_node_id.clone()),
        JobsShowField::WorkflowNodeAttempt => {
            metadata.and_then(|meta| meta.workflow_node_attempt.map(|value| value.to_string()))
        }
        JobsShowField::WorkflowNodeOutcome => {
            metadata.and_then(|meta| meta.workflow_node_outcome.clone())
        }
        JobsShowField::WorkflowPayloadRefs => metadata.and_then(|meta| {
            meta.workflow_payload_refs.as_ref().map(|values| {
                if values.is_empty() {
                    "none".to_string()
                } else {
                    values.join(", ")
                }
            })
        }),
        JobsShowField::WorkflowExecutorClass => {
            metadata.and_then(|meta| meta.workflow_executor_class.clone())
        }
        JobsShowField::WorkflowExecutorOperation => {
            metadata.and_then(|meta| meta.workflow_executor_operation.clone())
        }
        JobsShowField::WorkflowControlPolicy => {
            metadata.and_then(|meta| meta.workflow_control_policy.clone())
        }
        JobsShowField::WorkflowPolicySnapshot => {
            metadata.and_then(|meta| meta.workflow_policy_snapshot_hash.clone())
        }
        JobsShowField::WorkflowGates => metadata.and_then(|meta| {
            meta.workflow_gates.as_ref().map(|gates| {
                if gates.is_empty() {
                    "none".to_string()
                } else {
                    gates.join(" | ")
                }
            })
        }),
        JobsShowField::PatchFile => metadata.and_then(|meta| meta.patch_file.clone()),
        JobsShowField::PatchIndex => {
            metadata.and_then(|meta| meta.patch_index.map(|value| value.to_string()))
        }
        JobsShowField::PatchTotal => {
            metadata.and_then(|meta| meta.patch_total.map(|value| value.to_string()))
        }
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
        JobsShowField::ApprovalRequired => schedule.and_then(|sched| {
            sched
                .approval
                .as_ref()
                .map(|approval| approval.required.to_string())
        }),
        JobsShowField::ApprovalState => schedule.and_then(|sched| {
            sched
                .approval
                .as_ref()
                .map(|approval| jobs::approval_state_label(approval.state).to_string())
        }),
        JobsShowField::ApprovalRequestedAt => schedule.and_then(|sched| {
            sched
                .approval
                .as_ref()
                .map(|approval| approval.requested_at.to_rfc3339())
        }),
        JobsShowField::ApprovalRequestedBy => schedule.and_then(|sched| {
            sched
                .approval
                .as_ref()
                .and_then(|approval| approval.requested_by.clone())
        }),
        JobsShowField::ApprovalDecidedAt => schedule.and_then(|sched| {
            sched
                .approval
                .as_ref()
                .and_then(|approval| approval.decided_at.as_ref().map(|value| value.to_rfc3339()))
        }),
        JobsShowField::ApprovalDecidedBy => schedule.and_then(|sched| {
            sched
                .approval
                .as_ref()
                .and_then(|approval| approval.decided_by.clone())
        }),
        JobsShowField::ApprovalReason => schedule.and_then(|sched| {
            sched
                .approval
                .as_ref()
                .and_then(|approval| approval.reason.clone())
        }),
        JobsShowField::PinnedHead => schedule.and_then(|sched| {
            sched
                .pinned_head
                .as_ref()
                .map(|pinned| format!("{}@{}", pinned.branch, pinned.oid))
        }),
        JobsShowField::Artifacts => schedule
            .map(|sched| join_or_none(sched.artifacts.iter().map(jobs::format_artifact).collect())),
        JobsShowField::ExecutionRoot => metadata.and_then(|meta| meta.execution_root.clone()),
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
        JobsShowField::RetryCleanup => metadata.and_then(|meta| {
            meta.retry_cleanup_status
                .map(|status| status.label().to_string())
        }),
        JobsShowField::RetryCleanupError => {
            metadata.and_then(|meta| meta.retry_cleanup_error.clone())
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
    no_ansi: bool,
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
            watch,
            top,
            interval_ms,
            max_depth,
        } => {
            let schedule_format = match format {
                Some(JobsScheduleFormatArg::Summary) => ScheduleFormat::Summary,
                Some(JobsScheduleFormatArg::Dag) => ScheduleFormat::Dag,
                Some(JobsScheduleFormatArg::Json) => ScheduleFormat::Json,
                None => ScheduleFormat::Summary,
            };

            if top == 0 {
                return Err("`--top` must be at least 1.".into());
            }
            if interval_ms < 100 {
                return Err("`--interval-ms` must be at least 100.".into());
            }

            if watch && !matches!(schedule_format, ScheduleFormat::Summary) {
                return Err(
                    "`--watch` only supports summary format; rerun without `--format dag|json`."
                        .into(),
                );
            }

            if watch {
                ensure_watch_mode_allowed(no_ansi)?;
                return run_schedule_watch_loop(
                    jobs_root,
                    all,
                    job.as_deref(),
                    max_depth,
                    top,
                    interval_ms,
                );
            }

            let records = jobs::list_records(jobs_root)?;
            let graph = jobs::ScheduleGraph::new(records);
            let roots = schedule_roots(&graph, all, job.as_deref(), max_depth);
            if roots.is_empty() {
                if matches!(schedule_format, ScheduleFormat::Json) {
                    let snapshot = jobs::ScheduleSnapshot::empty();
                    println!("{}", serde_json::to_string_pretty(&snapshot)?);
                } else {
                    println!("Outcome: No scheduled jobs");
                }
                return Ok(());
            }

            let rows = schedule_summary_rows(&graph, &roots);
            let repo = Repository::discover(project_root)?;
            match schedule_format {
                ScheduleFormat::Summary => render_schedule_summary(&rows),
                ScheduleFormat::Dag => render_schedule_dag(&graph, &repo, &roots, max_depth),
                ScheduleFormat::Json => {
                    let edges = graph.snapshot_edges(&repo, &roots, max_depth);
                    let snapshot =
                        jobs::ScheduleSnapshot::new(schedule_snapshot_jobs(&rows), edges);
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
        JobsAction::Status { job, format } => {
            let record = jobs::read_record(jobs_root, &job)?;
            let exit = record
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "-".to_string());
            if matches!(format, JobsActionFormatArg::Json) {
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
        JobsAction::Retry { job, format } => {
            let binary = std::env::current_exe()?;
            let outcome = jobs::retry_job(project_root, jobs_root, &binary, &job)?;
            if matches!(format, JobsActionFormatArg::Json) {
                let payload = json!({
                    "outcome": "Jobs retried",
                    "requested_job": outcome.requested_job,
                    "retry_root": outcome.retry_root,
                    "last_successful_point": outcome.last_successful_point,
                    "retry_set": outcome.retry_set,
                    "reset": outcome.reset,
                    "restarted": outcome.restarted,
                    "updated": outcome.updated,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                let mut rows = vec![
                    ("Outcome".to_string(), "Jobs retried".to_string()),
                    ("Requested".to_string(), outcome.requested_job),
                    ("Retry root".to_string(), outcome.retry_root),
                    (
                        "Last successful point".to_string(),
                        join_or_none(outcome.last_successful_point),
                    ),
                    ("Retry set".to_string(), join_or_none(outcome.retry_set)),
                    ("Reset".to_string(), join_or_none(outcome.reset)),
                    ("Restarted".to_string(), join_or_none(outcome.restarted)),
                ];
                if !outcome.updated.is_empty() {
                    rows.push(("Updated".to_string(), join_or_none(outcome.updated)));
                }
                println!("{}", format_label_value_block(&rows, 0));
            }
            Ok(())
        }
        JobsAction::Approve { job, format } => {
            let binary = std::env::current_exe()?;
            let outcome = jobs::approve_job(project_root, jobs_root, &binary, &job)?;
            let approval_state = outcome
                .record
                .schedule
                .as_ref()
                .and_then(|schedule| schedule.approval.as_ref())
                .map(|approval| jobs::approval_state_label(approval.state).to_string())
                .unwrap_or_else(|| "none".to_string());
            if matches!(format, JobsActionFormatArg::Json) {
                let payload = json!({
                    "outcome": "Job approval granted",
                    "job": outcome.record.id,
                    "status": jobs::status_label(outcome.record.status),
                    "approval_state": approval_state,
                    "started": outcome.started,
                    "updated": outcome.updated,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                let mut rows = vec![
                    ("Outcome".to_string(), "Job approval granted".to_string()),
                    ("Job".to_string(), outcome.record.id),
                    (
                        "Status".to_string(),
                        jobs::status_label(outcome.record.status).to_string(),
                    ),
                    ("Approval state".to_string(), approval_state),
                ];
                if !outcome.started.is_empty() {
                    rows.push(("Started".to_string(), join_or_none(outcome.started)));
                }
                if !outcome.updated.is_empty() {
                    rows.push(("Updated".to_string(), join_or_none(outcome.updated)));
                }
                println!("{}", format_label_value_block(&rows, 0));
            }
            Ok(())
        }
        JobsAction::Reject {
            job,
            reason,
            format,
        } => {
            let record = jobs::reject_job(project_root, jobs_root, &job, reason.as_deref())?;
            let approval_state = record
                .schedule
                .as_ref()
                .and_then(|schedule| schedule.approval.as_ref())
                .map(|approval| jobs::approval_state_label(approval.state).to_string())
                .unwrap_or_else(|| "none".to_string());
            let rejection_reason = record
                .schedule
                .as_ref()
                .and_then(|schedule| schedule.approval.as_ref())
                .and_then(|approval| approval.reason.clone())
                .unwrap_or_else(|| "approval rejected".to_string());
            if matches!(format, JobsActionFormatArg::Json) {
                let payload = json!({
                    "outcome": "Job approval rejected",
                    "job": record.id,
                    "status": jobs::status_label(record.status),
                    "approval_state": approval_state,
                    "reason": rejection_reason,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                let rows = vec![
                    ("Outcome".to_string(), "Job approval rejected".to_string()),
                    ("Job".to_string(), record.id),
                    (
                        "Status".to_string(),
                        jobs::status_label(record.status).to_string(),
                    ),
                    ("Approval state".to_string(), approval_state),
                    ("Reason".to_string(), rejection_reason),
                ];
                println!("{}", format_label_value_block(&rows, 0));
            }
            Ok(())
        }
        JobsAction::Tail {
            job,
            stream,
            follow,
        } => jobs::tail_job_logs(jobs_root, &job, stream.into(), follow),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(
        job_id: &str,
        status: JobStatus,
        created_at: chrono::DateTime<chrono::Utc>,
        schedule: Option<jobs::JobSchedule>,
    ) -> jobs::JobRecord {
        jobs::JobRecord {
            id: job_id.to_string(),
            status,
            command: vec![
                "vizier".to_string(),
                "jobs".to_string(),
                "schedule".to_string(),
            ],
            child_args: Vec::new(),
            created_at,
            started_at: None,
            finished_at: None,
            pid: None,
            exit_code: None,
            stdout_path: format!(".vizier/jobs/{job_id}/stdout.log"),
            stderr_path: format!(".vizier/jobs/{job_id}/stderr.log"),
            session_path: None,
            outcome_path: None,
            metadata: None,
            config_snapshot: None,
            schedule,
        }
    }

    fn after_dependency(job_id: &str) -> jobs::JobAfterDependency {
        jobs::JobAfterDependency {
            job_id: job_id.to_string(),
            policy: jobs::AfterPolicy::Success,
        }
    }

    fn row(order: usize, job_id: &str, status: JobStatus) -> ScheduleSummaryRow {
        ScheduleSummaryRow {
            order,
            slug: None,
            name: format!("job-{order}"),
            status,
            wait: None,
            job_id: job_id.to_string(),
            created_at: "2026-02-13T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn schedule_roots_include_failed_job_when_blocking_waiting_dependents() {
        let now = chrono::Utc::now();
        let failed = make_record(
            "job-failed",
            JobStatus::Failed,
            now,
            Some(jobs::JobSchedule::default()),
        );
        let waiting = make_record(
            "job-waiting",
            JobStatus::WaitingOnDeps,
            now + chrono::Duration::seconds(1),
            Some(jobs::JobSchedule {
                after: vec![after_dependency("job-failed")],
                ..jobs::JobSchedule::default()
            }),
        );
        let succeeded = make_record(
            "job-succeeded",
            JobStatus::Succeeded,
            now + chrono::Duration::seconds(2),
            Some(jobs::JobSchedule::default()),
        );
        let graph = jobs::ScheduleGraph::new(vec![succeeded, waiting, failed]);

        let roots = schedule_roots(&graph, false, None, 3);
        assert!(
            roots.iter().any(|job| job == "job-failed"),
            "failed blocker should stay visible: {roots:?}"
        );
        assert!(
            roots.iter().any(|job| job == "job-waiting"),
            "waiting dependent should stay visible: {roots:?}"
        );
        assert!(
            !roots.iter().any(|job| job == "job-succeeded"),
            "succeeded jobs should stay hidden by default: {roots:?}"
        );
    }

    #[test]
    fn schedule_roots_hide_unrelated_failed_jobs_without_all() {
        let now = chrono::Utc::now();
        let failed = make_record(
            "job-failed-unrelated",
            JobStatus::Failed,
            now,
            Some(jobs::JobSchedule::default()),
        );
        let running = make_record(
            "job-running",
            JobStatus::Running,
            now + chrono::Duration::seconds(1),
            Some(jobs::JobSchedule::default()),
        );
        let graph = jobs::ScheduleGraph::new(vec![running, failed]);

        let roots = schedule_roots(&graph, false, None, 3);
        assert!(
            !roots.iter().any(|job| job == "job-failed-unrelated"),
            "unrelated failed jobs should stay hidden without --all: {roots:?}"
        );
        assert!(
            roots.iter().any(|job| job == "job-running"),
            "active jobs should remain visible: {roots:?}"
        );
    }

    #[test]
    fn select_watch_running_job_prefers_focused_running_job() {
        let rows = vec![
            row(1, "job-a", JobStatus::Running),
            row(2, "job-b", JobStatus::Running),
        ];
        let selected = select_watch_running_job(&rows, Some("job-b"), 1);
        assert_eq!(selected.as_deref(), Some("job-b"));
    }

    #[test]
    fn select_watch_running_job_falls_back_to_first_visible_running() {
        let rows = vec![
            row(1, "job-queued", JobStatus::Queued),
            row(2, "job-running-1", JobStatus::Running),
            row(3, "job-running-2", JobStatus::Running),
        ];
        let selected = select_watch_running_job(&rows, None, 2);
        assert_eq!(selected.as_deref(), Some("job-running-1"));
    }

    #[test]
    fn select_watch_running_job_returns_none_when_no_visible_running_jobs() {
        let rows = vec![
            row(1, "job-queued", JobStatus::Queued),
            row(2, "job-running-hidden", JobStatus::Running),
        ];
        let selected = select_watch_running_job(&rows, None, 1);
        assert!(selected.is_none());
    }

    #[test]
    fn jobs_show_field_value_surfaces_executor_identity_metadata() {
        let record = jobs::JobRecord {
            id: "job-1".to_string(),
            status: JobStatus::Queued,
            command: vec!["vizier".to_string(), "jobs".to_string(), "show".to_string()],
            child_args: Vec::new(),
            created_at: chrono::Utc::now(),
            started_at: None,
            finished_at: None,
            pid: None,
            exit_code: None,
            stdout_path: ".vizier/jobs/job-1/stdout.log".to_string(),
            stderr_path: ".vizier/jobs/job-1/stderr.log".to_string(),
            session_path: None,
            outcome_path: None,
            metadata: Some(jobs::JobMetadata {
                execution_root: Some(".vizier/tmp-worktrees/job-1".to_string()),
                workflow_executor_class: Some("environment_builtin".to_string()),
                workflow_executor_operation: Some("plan.apply_once".to_string()),
                workflow_control_policy: Some("gate.stop_condition".to_string()),
                ..jobs::JobMetadata::default()
            }),
            config_snapshot: None,
            schedule: None,
        };

        assert_eq!(
            jobs_show_field_value(JobsShowField::WorkflowExecutorClass, &record).as_deref(),
            Some("environment_builtin")
        );
        assert_eq!(
            jobs_show_field_value(JobsShowField::WorkflowExecutorOperation, &record).as_deref(),
            Some("plan.apply_once")
        );
        assert_eq!(
            jobs_show_field_value(JobsShowField::WorkflowControlPolicy, &record).as_deref(),
            Some("gate.stop_condition")
        );
        assert_eq!(
            jobs_show_field_value(JobsShowField::ExecutionRoot, &record).as_deref(),
            Some(".vizier/tmp-worktrees/job-1")
        );
    }

    #[test]
    fn jobs_show_field_value_supports_canonical_agent_invoke_metadata() {
        let record = jobs::JobRecord {
            id: "job-2".to_string(),
            status: JobStatus::Queued,
            command: vec!["vizier".to_string(), "jobs".to_string(), "show".to_string()],
            child_args: Vec::new(),
            created_at: chrono::Utc::now(),
            started_at: None,
            finished_at: None,
            pid: None,
            exit_code: None,
            stdout_path: ".vizier/jobs/job-2/stdout.log".to_string(),
            stderr_path: ".vizier/jobs/job-2/stderr.log".to_string(),
            session_path: None,
            outcome_path: None,
            metadata: Some(jobs::JobMetadata {
                workflow_executor_class: Some("agent".to_string()),
                workflow_executor_operation: Some("agent.invoke".to_string()),
                ..jobs::JobMetadata::default()
            }),
            config_snapshot: None,
            schedule: None,
        };

        assert_eq!(
            jobs_show_field_value(JobsShowField::WorkflowExecutorClass, &record).as_deref(),
            Some("agent")
        );
        assert_eq!(
            jobs_show_field_value(JobsShowField::WorkflowExecutorOperation, &record).as_deref(),
            Some("agent.invoke")
        );
    }
}
