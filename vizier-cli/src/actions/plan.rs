use std::collections::BTreeMap;

use serde::Serialize;

use vizier_core::{
    config,
    display::{format_label_value_block, format_number},
};

#[derive(Debug, Serialize)]
struct ConfigReport {
    agent: String,
    no_session: bool,
    workflow: WorkflowReport,
    approve: ApproveReport,
    merge: MergeReport,
    review: ReviewReport,
    agent_runtime_default: Option<AgentRuntimeReport>,
    scopes: BTreeMap<String, ScopeReport>,
}

#[derive(Debug, Serialize)]
struct ScopeReport {
    agent: String,
    documentation: DocumentationReport,
    agent_runtime: Option<AgentRuntimeReport>,
}

#[derive(Debug, Serialize)]
struct AgentRuntimeReport {
    label: String,
    command: Vec<String>,
    resolution: RuntimeResolutionReport,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RuntimeResolutionReport {
    BundledShim { label: String, path: String },
    ProvidedCommand,
}

#[derive(Debug, Serialize)]
struct ApproveReport {
    stop_condition: ApproveStopConditionReport,
}

#[derive(Debug, Serialize)]
struct ApproveStopConditionReport {
    script: Option<String>,
    retries: u32,
}

#[derive(Debug, Serialize)]
struct MergeReport {
    squash_default: bool,
    squash_mainline: Option<u32>,
    cicd_gate: MergeGateReport,
}

#[derive(Debug, Serialize)]
struct MergeGateReport {
    script: Option<String>,
    auto_resolve: bool,
    retries: u32,
}

#[derive(Debug, Serialize)]
struct WorkflowReport {
    no_commit_default: bool,
    background: BackgroundReport,
}

#[derive(Debug, Serialize)]
struct ReviewReport {
    checks: Vec<String>,
    cicd_gate: MergeGateReport,
}

#[derive(Debug, Serialize)]
struct BackgroundReport {
    enabled: bool,
    quiet: bool,
}

#[derive(Debug, Serialize)]
struct DocumentationReport {
    enabled: bool,
    include_snapshot: bool,
    include_narrative_docs: bool,
}

fn resolve_default_agent_settings(
    cfg: &config::Config,
    cli_override: Option<&config::AgentOverrides>,
) -> Result<config::AgentSettings, Box<dyn std::error::Error>> {
    let mut base = cfg.clone();
    base.agent_scopes.clear();
    config::resolve_agent_settings(&base, config::CommandScope::Ask, cli_override)
}

fn runtime_report(runtime: &config::ResolvedAgentRuntime) -> AgentRuntimeReport {
    AgentRuntimeReport {
        label: runtime.label.clone(),
        command: runtime.command.clone(),
        resolution: match &runtime.resolution {
            config::AgentRuntimeResolution::BundledShim { label, path } => {
                RuntimeResolutionReport::BundledShim {
                    label: label.clone(),
                    path: path.display().to_string(),
                }
            }
            config::AgentRuntimeResolution::ProvidedCommand => {
                RuntimeResolutionReport::ProvidedCommand
            }
        },
    }
}

fn documentation_report(docs: &config::DocumentationSettings) -> DocumentationReport {
    DocumentationReport {
        enabled: docs.use_documentation_prompt,
        include_snapshot: docs.include_snapshot,
        include_narrative_docs: docs.include_narrative_docs,
    }
}

fn scope_report(agent: &config::AgentSettings) -> ScopeReport {
    let runtime = Some(runtime_report(&agent.agent_runtime));

    ScopeReport {
        agent: agent.selector.clone(),
        documentation: documentation_report(&agent.documentation),
        agent_runtime: runtime,
    }
}

fn format_approve_rows(report: &ApproveReport) -> Vec<(String, String)> {
    vec![
        (
            "Stop-condition script".to_string(),
            value_or_unset(report.stop_condition.script.clone(), "unset"),
        ),
        (
            "Stop-condition retries".to_string(),
            format_number(report.stop_condition.retries as usize),
        ),
    ]
}

fn build_config_report(
    cfg: &config::Config,
    cli_override: Option<&config::AgentOverrides>,
) -> Result<ConfigReport, Box<dyn std::error::Error>> {
    let default_agent = resolve_default_agent_settings(cfg, cli_override)?;
    let agent_runtime_default = Some(runtime_report(&default_agent.agent_runtime));

    let mut scopes = BTreeMap::new();
    for scope in config::CommandScope::all() {
        let agent = config::resolve_agent_settings(cfg, *scope, cli_override)?;
        scopes.insert(scope.as_str().to_string(), scope_report(&agent));
    }

    Ok(ConfigReport {
        agent: cfg.agent_selector.clone(),
        no_session: cfg.no_session,
        workflow: WorkflowReport {
            no_commit_default: cfg.workflow.no_commit_default,
            background: BackgroundReport {
                enabled: cfg.workflow.background.enabled,
                quiet: cfg.workflow.background.quiet,
            },
        },
        approve: ApproveReport {
            stop_condition: ApproveStopConditionReport {
                script: cfg
                    .approve
                    .stop_condition
                    .script
                    .as_ref()
                    .map(|path| path.display().to_string()),
                retries: cfg.approve.stop_condition.retries,
            },
        },
        merge: MergeReport {
            squash_default: cfg.merge.squash_default,
            squash_mainline: cfg.merge.squash_mainline,
            cicd_gate: MergeGateReport {
                script: cfg
                    .merge
                    .cicd_gate
                    .script
                    .as_ref()
                    .map(|path| path.display().to_string()),
                auto_resolve: cfg.merge.cicd_gate.auto_resolve,
                retries: cfg.merge.cicd_gate.retries,
            },
        },
        review: ReviewReport {
            checks: cfg.review.checks.commands.clone(),
            cicd_gate: MergeGateReport {
                script: cfg
                    .merge
                    .cicd_gate
                    .script
                    .as_ref()
                    .map(|path| path.display().to_string()),
                auto_resolve: cfg.merge.cicd_gate.auto_resolve,
                retries: cfg.merge.cicd_gate.retries,
            },
        },
        agent_runtime_default,
        scopes,
    })
}

fn value_or_unset(value: Option<String>, fallback: &str) -> String {
    value.unwrap_or_else(|| fallback.to_string())
}

fn format_runtime_resolution(resolution: &RuntimeResolutionReport) -> String {
    match resolution {
        RuntimeResolutionReport::BundledShim { label, path } => {
            format!("bundled `{label}` shim at {path}")
        }
        RuntimeResolutionReport::ProvidedCommand => "provided command".to_string(),
    }
}

fn runtime_rows(runtime: &AgentRuntimeReport) -> Vec<(String, String)> {
    vec![
        ("Runtime label".to_string(), runtime.label.clone()),
        (
            "Command".to_string(),
            runtime.command.join(" ").trim().to_string(),
        ),
        (
            "Resolution".to_string(),
            format_runtime_resolution(&runtime.resolution),
        ),
    ]
}

fn documentation_label(docs: &DocumentationReport) -> String {
    if !docs.enabled {
        return "disabled".to_string();
    }

    let mut parts = vec!["enabled".to_string()];
    parts.push(format!("snapshot={}", docs.include_snapshot));
    parts.push(format!("narrative_docs={}", docs.include_narrative_docs));
    parts.join(" ")
}

fn format_scope_rows(scope: &ScopeReport) -> Vec<(String, String)> {
    let mut rows = vec![("Agent".to_string(), scope.agent.clone())];

    rows.push((
        "Documentation prompt".to_string(),
        documentation_label(&scope.documentation),
    ));

    if let Some(runtime) = scope.agent_runtime.as_ref() {
        rows.extend(runtime_rows(runtime));
    }

    rows
}

fn format_review_rows(report: &ReviewReport) -> Vec<(String, String)> {
    let mut rows = vec![(
        "Checks".to_string(),
        if report.checks.is_empty() {
            "none".to_string()
        } else {
            report.checks.join(" | ")
        },
    )];
    rows.push((
        "CI/CD script".to_string(),
        value_or_unset(report.cicd_gate.script.clone(), "unset"),
    ));
    rows.push((
        "CI/CD auto-fix".to_string(),
        report.cicd_gate.auto_resolve.to_string(),
    ));
    rows.push((
        "CI/CD retries".to_string(),
        format_number(report.cicd_gate.retries as usize),
    ));
    rows
}

fn format_merge_rows(report: &MergeReport) -> Vec<(String, String)> {
    vec![
        (
            "Squash default".to_string(),
            report.squash_default.to_string(),
        ),
        (
            "Squash mainline".to_string(),
            report
                .squash_mainline
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unset".to_string()),
        ),
        (
            "CI/CD script".to_string(),
            value_or_unset(report.cicd_gate.script.clone(), "unset"),
        ),
        (
            "CI/CD auto-fix".to_string(),
            report.cicd_gate.auto_resolve.to_string(),
        ),
        (
            "CI/CD retries".to_string(),
            format_number(report.cicd_gate.retries as usize),
        ),
    ]
}

fn format_global_rows(report: &ConfigReport) -> Vec<(String, String)> {
    vec![
        ("Agent".to_string(), report.agent.clone()),
        ("No session".to_string(), report.no_session.to_string()),
        (
            "No-commit default".to_string(),
            report.workflow.no_commit_default.to_string(),
        ),
        (
            "Background enabled".to_string(),
            report.workflow.background.enabled.to_string(),
        ),
        (
            "Background quiet".to_string(),
            report.workflow.background.quiet.to_string(),
        ),
    ]
}

fn print_config_report(report: &ConfigReport) {
    println!("Resolved configuration:");

    let mut printed = false;
    let global_block = format_label_value_block(&format_global_rows(report), 2);
    if !global_block.is_empty() {
        println!("Global/Workflow:");
        println!("{global_block}");
        printed = true;
    }

    let approve_block = format_label_value_block(&format_approve_rows(&report.approve), 2);
    if !approve_block.is_empty() {
        if printed {
            println!();
        }
        println!("Approve:");
        println!("{approve_block}");
        printed = true;
    }

    let merge_block = format_label_value_block(&format_merge_rows(&report.merge), 2);
    if !merge_block.is_empty() {
        if printed {
            println!();
        }
        println!("Merge:");
        println!("{merge_block}");
        printed = true;
    }

    let review_block = format_label_value_block(&format_review_rows(&report.review), 2);
    if !review_block.is_empty() {
        if printed {
            println!();
        }
        println!("Review:");
        println!("{review_block}");
        printed = true;
    }

    if let Some(runtime) = report.agent_runtime_default.as_ref() {
        let runtime_block = format_label_value_block(&runtime_rows(runtime), 2);
        if !runtime_block.is_empty() {
            if printed {
                println!();
            }
            println!("Agent runtime (default):");
            println!("{runtime_block}");
            printed = true;
        }
    }

    if !report.scopes.is_empty() {
        if printed {
            println!();
        }
        println!("Per-scope agents:");
        for (scope, view) in report.scopes.iter() {
            println!("  {scope}:");
            println!("{}", format_label_value_block(&format_scope_rows(view), 4));
        }
    }
}

pub(crate) fn run_plan_summary(
    cli_override: Option<&config::AgentOverrides>,
    emit_json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = config::get_config();
    let report = build_config_report(&cfg, cli_override)?;

    if emit_json {
        let json = serde_json::to_string_pretty(&report)?;
        println!("{json}");
    } else {
        print_config_report(&report);
    }

    Ok(())
}
