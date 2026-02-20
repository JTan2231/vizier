use std::path::Path;

use serde_json::json;

use crate::actions::shared::format_block;
use crate::actions::workflow_preflight::prepare_workflow_template;
use crate::cli::args::{AuditCmd, AuditFormatArg};
use crate::jobs;

fn lock_mode_label(mode: vizier_core::scheduler::LockMode) -> &'static str {
    match mode {
        vizier_core::scheduler::LockMode::Shared => "shared",
        vizier_core::scheduler::LockMode::Exclusive => "exclusive",
    }
}

pub(crate) fn run_workflow_audit(
    project_root: &Path,
    cmd: AuditCmd,
) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = vizier_core::config::get_config();
    let prepared = prepare_workflow_template(project_root, &cmd.flow, &cmd.inputs, &cmd.set, &cfg)?;
    let report = jobs::audit_workflow_run_template(&prepared.template)?;
    emit_audit_summary(cmd.format, &prepared.source, &prepared.template, &report)?;

    if cmd.strict && !report.untethered_inputs.is_empty() {
        std::process::exit(10);
    }

    Ok(())
}

fn emit_audit_summary(
    format: AuditFormatArg,
    source: &crate::workflow_templates::ResolvedWorkflowSource,
    template: &vizier_core::workflow_template::WorkflowTemplate,
    report: &vizier_core::workflow_audit::WorkflowAuditReport,
) -> Result<(), Box<dyn std::error::Error>> {
    if matches!(format, AuditFormatArg::Json) {
        let payload = json!({
            "outcome": "workflow_audit_completed",
            "workflow_template_selector": source.selector,
            "workflow_template_id": template.id,
            "workflow_template_version": template.version,
            "node_count": template.nodes.len(),
            "output_artifacts": report.output_artifacts,
            "output_artifacts_by_outcome": {
                "succeeded": report.output_artifacts_by_outcome.succeeded,
                "failed": report.output_artifacts_by_outcome.failed,
                "blocked": report.output_artifacts_by_outcome.blocked,
                "cancelled": report.output_artifacts_by_outcome.cancelled,
            },
            "untethered_inputs": report.untethered_inputs.iter().map(|entry| {
                json!({
                    "artifact": entry.artifact,
                    "consumers": entry.consumers,
                })
            }).collect::<Vec<_>>(),
            "effective_locks": report.effective_locks.iter().map(|entry| {
                json!({
                    "node_id": entry.node_id,
                    "locks": entry.locks.iter().map(|lock| {
                        json!({
                            "key": lock.key,
                            "mode": lock_mode_label(lock.mode),
                        })
                    }).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
            "summary": {
                "untethered_count": report.untethered_inputs.len(),
                "has_untethered": !report.untethered_inputs.is_empty(),
            }
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!(
        "{}",
        format_block(vec![
            (
                "Outcome".to_string(),
                "Workflow audit completed".to_string()
            ),
            ("Selector".to_string(), source.selector.clone()),
            (
                "Template".to_string(),
                format!("{}@{}", template.id, template.version),
            ),
            ("Nodes".to_string(), template.nodes.len().to_string()),
        ])
    );
    println!();
    println!("Output artifacts:");
    if report.output_artifacts.is_empty() {
        println!("- none");
    } else {
        for artifact in &report.output_artifacts {
            println!("- {artifact}");
        }
    }
    println!();
    println!("Untethered inputs:");
    if report.untethered_inputs.is_empty() {
        println!("- none");
    } else {
        for entry in &report.untethered_inputs {
            let consumers = entry.consumers.join(", ");
            println!("- {} (consumed by: {consumers})", entry.artifact);
        }
    }
    println!();
    println!("Effective locks:");
    if report.effective_locks.is_empty() {
        println!("- none");
    } else {
        for entry in &report.effective_locks {
            if entry.locks.is_empty() {
                println!("- {}: none", entry.node_id);
                continue;
            }
            let locks = entry
                .locks
                .iter()
                .map(|lock| format!("{} ({})", lock.key, lock_mode_label(lock.mode)))
                .collect::<Vec<_>>()
                .join(", ");
            println!("- {}: {}", entry.node_id, locks);
        }
    }

    Ok(())
}
