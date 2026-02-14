use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::thread;
use std::time::Duration;

use serde_json::json;
use uuid::Uuid;
use vizier_core::display;

use crate::actions::shared::format_block;
use crate::cli::args::{RunCmd, RunFormatArg};
use crate::jobs;
use crate::workflow_templates::{self, ResolvedWorkflowSource};

pub(crate) fn run_workflow(
    project_root: &Path,
    jobs_root: &Path,
    cmd: RunCmd,
) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = vizier_core::config::get_config();
    let source = workflow_templates::resolve_workflow_source(project_root, &cmd.flow, &cfg)?;
    let set_overrides = parse_set_overrides(&cmd.set)?;
    let template = workflow_templates::load_template_with_params(&source, &set_overrides)?;

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
}
