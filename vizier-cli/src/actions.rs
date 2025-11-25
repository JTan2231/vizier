use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::Arc;
use std::time::{Duration, Instant};

use git2::build::CheckoutBuilder;
use git2::{BranchType, Oid, Repository, RepositoryState};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tempfile::{Builder, NamedTempFile, TempPath};
use tokio::{sync::mpsc, task::JoinHandle};

use vizier_core::agent::{
    AgentError, AgentRequest, AgentResponse, ProgressHook, ReviewCheckContext, ReviewGateContext,
    ReviewGateStatus,
};
use vizier_core::vcs::{
    AttemptOutcome, CherryPickOutcome, CredentialAttempt, MergePreparation, MergeReady,
    PushErrorKind, RemoteScheme, add_worktree_for_branch, amend_head_commit,
    apply_cherry_pick_sequence, build_squash_plan, commit_in_progress_cherry_pick,
    commit_in_progress_merge, commit_in_progress_squash, commit_paths_in_repo, commit_ready_merge,
    commit_soft_squash, commit_squashed_merge, create_branch_from, delete_branch,
    detect_primary_branch, list_conflicted_paths, prepare_merge, remove_worktree, repo_root,
};
use vizier_core::{
    agent_prompt, auditor,
    auditor::{AgentRunRecord, Auditor, CommitMessageBuilder, CommitMessageType, Message},
    bootstrap,
    bootstrap::{BootstrapOptions, IssuesProvider},
    config,
    display::{self, LogLevel, ProgressEvent, Verbosity, format_label_value_block, format_number},
    file_tracking, vcs,
};

use crate::plan;

fn clip_message(msg: &str) -> String {
    const LIMIT: usize = 90;
    let mut clipped = String::new();
    for (idx, ch) in msg.chars().enumerate() {
        if idx >= LIMIT {
            clipped.push('…');
            break;
        }
        clipped.push(ch);
    }
    clipped
}

fn format_block(rows: Vec<(String, String)>) -> String {
    format_label_value_block(&rows, 0)
}

fn format_block_with_indent(rows: Vec<(String, String)>, indent: usize) -> String {
    format_label_value_block(&rows, indent)
}

#[derive(Debug, Serialize)]
struct ConfigReport {
    backend: String,
    agent_backend: Option<String>,
    no_session: bool,
    workflow: WorkflowReport,
    merge: MergeReport,
    review: ReviewReport,
    agent_runtime_default: Option<AgentRuntimeReport>,
    scopes: BTreeMap<String, ScopeReport>,
}

#[derive(Debug, Serialize)]
struct ScopeReport {
    backend: String,
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
    progress: String,
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
    base.resolve_agent_settings(config::CommandScope::Ask, cli_override)
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
        backend: agent.backend.to_string(),
        documentation: documentation_report(&agent.documentation),
        agent_runtime: runtime,
    }
}

fn build_config_report(
    cfg: &config::Config,
    cli_override: Option<&config::AgentOverrides>,
) -> Result<ConfigReport, Box<dyn std::error::Error>> {
    let default_agent = resolve_default_agent_settings(cfg, cli_override)?;
    let agent_backend = Some(default_agent.backend.to_string());
    let agent_runtime_default = Some(runtime_report(&default_agent.agent_runtime));

    let mut scopes = BTreeMap::new();
    for scope in config::CommandScope::all() {
        let agent = cfg.resolve_agent_settings(*scope, cli_override)?;
        scopes.insert(scope.as_str().to_string(), scope_report(&agent));
    }

    Ok(ConfigReport {
        backend: cfg.backend.to_string(),
        agent_backend,
        no_session: cfg.no_session,
        workflow: WorkflowReport {
            no_commit_default: cfg.workflow.no_commit_default,
            background: BackgroundReport {
                enabled: cfg.workflow.background.enabled,
                quiet: cfg.workflow.background.quiet,
                progress: match cfg.workflow.background.progress {
                    display::ProgressMode::Auto => "auto".to_string(),
                    display::ProgressMode::Never => "never".to_string(),
                    display::ProgressMode::Always => "always".to_string(),
                },
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
    let mut rows = vec![("Backend".to_string(), scope.backend.clone())];

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
        ("Backend".to_string(), report.backend.clone()),
        (
            "Agent backend".to_string(),
            value_or_unset(report.agent_backend.clone(), "unset"),
        ),
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
        (
            "Background progress".to_string(),
            report.workflow.background.progress.clone(),
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

pub fn run_plan_summary(
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

fn format_agent_value() -> Option<String> {
    auditor::Auditor::latest_agent_context().map(|context| {
        let mut parts = vec![format!("backend {}", context.backend)];
        if !context.backend_label.is_empty() {
            parts.push(format!("runtime {}", context.backend_label));
        }
        parts.push(format!("scope {}", context.scope.as_str()));
        if let Some(code) = context.exit_code {
            parts.push(format!("exit {}", code));
        }
        if let Some(duration) = context.duration_ms {
            parts.push(format!("elapsed {:.2}s", duration as f64 / 1000.0));
        }
        parts.join(" • ")
    })
}

fn latest_agent_rows() -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if let Some(agent) = format_agent_value() {
        rows.push(("Agent".to_string(), agent));
    }

    if let Some(run) = Auditor::latest_agent_run() {
        rows.extend(run.to_rows());
    }

    rows
}

fn copy_session_log_to_repo_root(repo_root: &Path, artifact: &auditor::SessionArtifact) {
    let dest_dir = repo_root
        .join(".vizier")
        .join("sessions")
        .join(&artifact.id);
    let dest_path = dest_dir.join("session.json");

    if artifact.path == dest_path {
        return;
    }

    if let Err(err) = fs::create_dir_all(&dest_dir) {
        display::debug(format!(
            "unable to prepare session log directory {}: {}",
            dest_dir.display(),
            err
        ));
        return;
    }

    if let Err(err) = fs::copy(&artifact.path, &dest_path) {
        display::debug(format!(
            "unable to copy session log from {} to {}: {}",
            artifact.path.display(),
            dest_path.display(),
            err
        ));
    }
}

fn append_agent_rows(rows: &mut Vec<(String, String)>, verbosity: Verbosity) {
    if matches!(verbosity, Verbosity::Quiet) {
        return;
    }

    let agent_rows = latest_agent_rows();
    if !agent_rows.is_empty() {
        rows.extend(agent_rows);
    }
}

fn current_verbosity() -> Verbosity {
    display::get_display_config().verbosity
}

fn format_credential_attempt(attempt: &CredentialAttempt) -> String {
    let label = attempt.strategy.label();
    match &attempt.outcome {
        AttemptOutcome::Success => format!("{label}=ok"),
        AttemptOutcome::Failure(message) => {
            format!("{label}=failed({})", clip_message(message))
        }
        AttemptOutcome::Skipped(message) => {
            format!("{label}=skipped({})", clip_message(message))
        }
    }
}

fn render_push_auth_failure(
    remote: &str,
    url: &str,
    scheme: &RemoteScheme,
    attempts: &[CredentialAttempt],
) {
    let scheme_label = scheme.label();
    display::emit(
        LogLevel::Error,
        format!("Push to {remote} failed ({scheme_label} {url})"),
    );

    if !attempts.is_empty() {
        let summary = attempts
            .iter()
            .map(format_credential_attempt)
            .collect::<Vec<_>>()
            .join("; ");
        display::emit(LogLevel::Error, format!("Credential strategies: {summary}"));
    }

    if matches!(scheme, RemoteScheme::Ssh) {
        display::emit(
            LogLevel::Error,
            "Hint: start ssh-agent and `ssh-add ~/.ssh/id_ed25519`, or switch the remote to HTTPS.",
        );
    }
}

fn push_origin_if_requested(should_push: bool) -> Result<(), Box<dyn std::error::Error>> {
    if !should_push {
        return Ok(());
    }

    display::info("Pushing current branch to origin...");
    match vcs::push_current_branch("origin") {
        Ok(_) => {
            display::info("Push to origin completed.");
            Ok(())
        }
        Err(err) => {
            match err.kind() {
                PushErrorKind::Auth {
                    remote,
                    url,
                    scheme,
                    attempts,
                } => {
                    render_push_auth_failure(remote, url, scheme, attempts);
                }
                PushErrorKind::General(message) => {
                    display::emit(
                        LogLevel::Error,
                        format!("Error pushing to origin: {message}"),
                    );
                }
            }

            Err(Box::<dyn std::error::Error>::from(err))
        }
    }
}

pub fn print_agent_summary() {
    let verbosity = display::get_display_config().verbosity;
    if matches!(verbosity, Verbosity::Quiet) {
        return;
    }

    let rows = latest_agent_rows();

    let block = format_block_with_indent(rows, 2);
    if block.is_empty() {
        return;
    }

    println!("Agent run:");
    println!("{block}");
}

fn prompt_selection<'a>(
    agent: &'a config::AgentSettings,
) -> Result<&'a config::PromptSelection, Box<dyn std::error::Error>> {
    agent.prompt_selection().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Other,
            format!(
                "agent for `{}` is missing a resolved prompt; call AgentSettings::for_prompt first",
                agent.scope.as_str()
            ),
        )
        .into()
    })
}

fn require_agent_backend(
    agent: &config::AgentSettings,
    prompt: config::PromptKind,
    error_message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let derived = agent.for_prompt(prompt)?;
    if !derived.backend.requires_agent_runner() {
        return Err(error_message.into());
    }
    Ok(())
}

fn build_agent_request(
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
        scope: Some(agent.scope),
        metadata,
        timeout: Some(Duration::from_secs(9000)),
    }
}

fn spawn_plain_progress_logger(mut rx: mpsc::Receiver<ProgressEvent>) -> Option<JoinHandle<()>> {
    let cfg = display::get_display_config();
    if matches!(cfg.verbosity, Verbosity::Quiet) {
        return None;
    }

    let verbosity = cfg.verbosity;
    Some(tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            for line in display::render_progress_event(&event, verbosity) {
                eprintln!("{}", line);
            }
        }
    }))
}

#[derive(Debug, Clone)]
pub struct SnapshotInitOptions {
    pub force: bool,
    pub depth: Option<usize>,
    pub paths: Vec<String>,
    pub exclude: Vec<String>,
    pub issues: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SpecSource {
    Inline,
    File(PathBuf),
    Stdin,
}

impl SpecSource {
    pub fn as_metadata_value(&self) -> String {
        match self {
            SpecSource::Inline => "inline".to_string(),
            SpecSource::File(path) => format!("file:{}", path.display()),
            SpecSource::Stdin => "stdin".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitMode {
    AutoCommit,
    HoldForReview,
}

impl CommitMode {
    pub fn should_commit(self) -> bool {
        matches!(self, CommitMode::AutoCommit)
    }

    pub fn label(self) -> &'static str {
        match self {
            CommitMode::AutoCommit => "auto",
            CommitMode::HoldForReview => "manual",
        }
    }
}

fn audit_disposition(mode: CommitMode) -> auditor::CommitDisposition {
    match mode {
        CommitMode::AutoCommit => auditor::CommitDisposition::Auto,
        CommitMode::HoldForReview => auditor::CommitDisposition::Hold,
    }
}

#[derive(Debug, Clone)]
pub struct DraftArgs {
    pub spec_text: String,
    pub spec_source: SpecSource,
    pub name_override: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ListOptions {
    pub target: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ApproveOptions {
    pub plan: Option<String>,
    pub list_only: bool,
    pub target: Option<String>,
    pub branch_override: Option<String>,
    pub assume_yes: bool,
    pub push_after: bool,
}

#[derive(Debug, Clone)]
pub struct ReviewOptions {
    pub plan: String,
    pub target: Option<String>,
    pub branch_override: Option<String>,
    pub assume_yes: bool,
    pub review_only: bool,
    pub skip_checks: bool,
    pub cicd_gate: CicdGateOptions,
    pub auto_resolve_requested: bool,
    pub push_after: bool,
}

#[derive(Debug, Clone)]
pub struct MergeOptions {
    pub plan: String,
    pub target: Option<String>,
    pub branch_override: Option<String>,
    pub assume_yes: bool,
    pub delete_branch: bool,
    pub note: Option<String>,
    pub push_after: bool,
    pub conflict_auto_resolve: ConflictAutoResolveSetting,
    pub conflict_strategy: MergeConflictStrategy,
    pub complete_conflict: bool,
    pub cicd_gate: CicdGateOptions,
    pub squash: bool,
    pub squash_mainline: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct TestDisplayOptions {
    pub scope: config::CommandScope,
    pub prompt_override: Option<String>,
    pub raw_output: bool,
    pub timeout: Option<Duration>,
    pub disable_wrapper: bool,
    pub record_session: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeConflictStrategy {
    Manual,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictAutoResolveSource {
    Default,
    Config,
    FlagEnable,
    FlagDisable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConflictAutoResolveSetting {
    enabled: bool,
    source: ConflictAutoResolveSource,
}

impl ConflictAutoResolveSetting {
    pub fn new(enabled: bool, source: ConflictAutoResolveSource) -> Self {
        Self { enabled, source }
    }

    pub fn enabled(self) -> bool {
        self.enabled
    }

    #[allow(dead_code)]
    pub fn source(self) -> ConflictAutoResolveSource {
        self.source
    }

    fn source_description(self) -> &'static str {
        match self.source {
            ConflictAutoResolveSource::Default => "default",
            ConflictAutoResolveSource::Config => "merge.conflicts.auto_resolve",
            ConflictAutoResolveSource::FlagEnable => "--auto-resolve-conflicts",
            ConflictAutoResolveSource::FlagDisable => "--no-auto-resolve-conflicts",
        }
    }

    fn status_line(self) -> String {
        let origin = self.source_description();
        if self.enabled() {
            format!("Conflict auto-resolution enabled via {origin}.")
        } else {
            format!(
                "Conflict auto-resolution disabled via {origin}; conflicts will require manual resolution unless overridden."
            )
        }
    }
}

#[derive(Debug, Clone)]
pub struct CicdGateOptions {
    pub script: Option<PathBuf>,
    pub auto_resolve: bool,
    pub retries: u32,
}

impl CicdGateOptions {
    pub fn from_config(config: &config::MergeCicdGateConfig) -> Self {
        Self {
            script: config.script.clone(),
            auto_resolve: config.auto_resolve,
            retries: config.retries,
        }
    }

    #[allow(dead_code)]
    pub fn disabled() -> Self {
        Self {
            script: None,
            auto_resolve: false,
            retries: 1,
        }
    }
}

#[derive(Debug, Clone)]
struct CicdGateOutcome {
    script: PathBuf,
    attempts: u32,
    fixes: Vec<CicdFixRecord>,
}

#[derive(Debug, Clone)]
enum CicdFixRecord {
    Commit(String),
    Amend(String),
}

impl CicdFixRecord {
    fn describe(&self) -> String {
        match self {
            CicdFixRecord::Commit(oid) => format!("commit:{oid}"),
            CicdFixRecord::Amend(oid) => format!("amend:{oid}"),
        }
    }
}

#[derive(Debug)]
struct MergeExecutionResult {
    merge_oid: Oid,
    source_oid: Oid,
    gate: Option<CicdGateOutcome>,
    squashed: bool,
    implementation_oid: Option<Oid>,
}

#[derive(Debug)]
enum MergeConflictResolution {
    MergeCommitted {
        merge_oid: Oid,
        source_oid: Oid,
    },
    SquashImplementationCommitted {
        source_oid: Oid,
        implementation_oid: Oid,
    },
}

#[derive(Debug)]
struct CicdScriptResult {
    status: ExitStatus,
    duration: Duration,
    stdout: String,
    stderr: String,
}

impl CicdScriptResult {
    fn success(&self) -> bool {
        self.status.success()
    }

    fn status_label(&self) -> String {
        match self.status.code() {
            Some(code) => format!("exit={code}"),
            None => "terminated".to_string(),
        }
    }
}

#[derive(Debug)]
enum PendingMergeStatus {
    None,
    Ready {
        merge_oid: Oid,
        source_oid: Oid,
    },
    SquashReady {
        source_oid: Oid,
        merge_message: String,
    },
    Blocked(PendingMergeBlocker),
}

#[derive(Debug)]
enum PendingMergeBlocker {
    WrongCheckout { expected_branch: String },
    NotInMerge { target_branch: String },
    Conflicts { files: Vec<String> },
}

#[derive(Debug)]
struct PendingMergeError {
    slug: String,
    detail: PendingMergeBlocker,
}

impl PendingMergeError {
    fn new(slug: impl Into<String>, detail: PendingMergeBlocker) -> Self {
        PendingMergeError {
            slug: slug.into(),
            detail,
        }
    }
}

impl std::fmt::Display for PendingMergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.detail {
            PendingMergeBlocker::WrongCheckout { expected_branch } => write!(
                f,
                "Pending Vizier merge for plan {} is tied to {}; checkout that branch and rerun `vizier merge {} --complete-conflict` to finalize the conflict resolution.",
                self.slug, expected_branch, self.slug
            ),
            PendingMergeBlocker::NotInMerge { target_branch } => write!(
                f,
                "Vizier has merge metadata for plan {} but Git is no longer merging/cherry-picking on {}; rerun `vizier merge {}` (without --complete-conflict) to start a new merge if needed.",
                self.slug, target_branch, self.slug
            ),
            PendingMergeBlocker::Conflicts { files } => {
                if files.is_empty() {
                    write!(
                        f,
                        "Merge conflicts for plan {} are still unresolved; fix them, stage the results, then rerun `vizier merge {} --complete-conflict`.",
                        self.slug, self.slug
                    )
                } else {
                    let preview = files.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
                    let more = if files.len() > 3 {
                        format!(" (+{} more)", files.len() - 3)
                    } else {
                        String::new()
                    };
                    write!(
                        f,
                        "Merge conflicts for plan {} remain ({preview}{more}); resolve and stage them, then rerun `vizier merge {} --complete-conflict`.",
                        self.slug, self.slug
                    )
                }
            }
        }
    }
}

impl std::error::Error for PendingMergeError {}

pub async fn run_snapshot_init(
    opts: SnapshotInitOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let depth_preview = bootstrap::preview_history_depth(opts.depth)?;

    display::info(format!(
        "Bootstrapping snapshot (history depth target: {})",
        depth_preview
    ));
    if !opts.paths.is_empty() {
        display::info(format!("Scope includes: {}", opts.paths.join(", ")));
    }
    if !opts.exclude.is_empty() {
        display::info(format!("Scope excludes: {}", opts.exclude.join(", ")));
    }

    let issues_provider = if let Some(provider) = opts.issues {
        Some(provider.parse::<IssuesProvider>()?)
    } else {
        None
    };

    let report = bootstrap::bootstrap_snapshot(BootstrapOptions {
        force: opts.force,
        depth: opts.depth,
        paths: opts.paths.clone(),
        exclude: opts.exclude.clone(),
        issues_provider,
    })
    .await?;

    if !report.warnings.is_empty() {
        for note in &report.warnings {
            display::warn(format!("Warning: {}", note));
        }
    }

    let verbosity = current_verbosity();
    if !report.summary.trim().is_empty() {
        display::info(format!("Snapshot summary: {}", report.summary.trim()));
    }

    let files_updated = report.files_touched.len();
    let mut rows = vec![(
        "Outcome".to_string(),
        "Snapshot bootstrap complete".to_string(),
    )];
    rows.push(("Depth used".to_string(), format_number(report.depth_used)));
    rows.push((
        "Files".to_string(),
        if files_updated == 0 {
            "no .vizier changes".to_string()
        } else {
            format!("updated {}", format_number(files_updated))
        },
    ));

    if matches!(verbosity, Verbosity::Info | Verbosity::Debug) {
        rows.push(("Analyzed at".to_string(), report.analysis_timestamp.clone()));
        rows.push((
            "Branch".to_string(),
            report
                .branch
                .as_deref()
                .unwrap_or("<detached HEAD>")
                .to_string(),
        ));
        rows.push((
            "Head".to_string(),
            report
                .head_commit
                .as_deref()
                .map(short_hash)
                .unwrap_or_else(|| "<no HEAD commit>".to_string()),
        ));
        rows.push((
            "Working tree".to_string(),
            if report.dirty { "dirty" } else { "clean" }.to_string(),
        ));
        if !report.scope_includes.is_empty() {
            rows.push(("Includes".to_string(), report.scope_includes.join(", ")));
        }
        if !report.scope_excludes.is_empty() {
            rows.push(("Excludes".to_string(), report.scope_excludes.join(", ")));
        }
        if let Some(provider) = report.issues_provider.as_ref() {
            rows.push(("Issues provider".to_string(), provider.to_string()));
        }
        if !report.issues.is_empty() {
            rows.push(("Issues".to_string(), report.issues.join(", ")));
        }
    }

    append_agent_rows(&mut rows, verbosity);

    let outcome = format_block(rows);
    if !outcome.is_empty() {
        println!("{}", outcome);
    }

    Ok(())
}

pub async fn run_save(
    commit_ref: &str,
    exclude: &[&str],
    commit_message: Option<String>,
    use_editor: bool,
    commit_mode: CommitMode,
    push_after_commit: bool,
    agent: &config::AgentSettings,
) -> Result<(), Box<dyn std::error::Error>> {
    match vcs::get_diff(".", Some(commit_ref), Some(exclude)) {
        Ok(diff) => match save(
            diff,
            commit_message,
            use_editor,
            commit_mode,
            push_after_commit,
            agent,
        )
        .await
        {
            Ok(outcome) => {
                println!("{}", format_save_outcome(&outcome, current_verbosity()));
                Ok(())
            }
            Err(e) => {
                display::emit(LogLevel::Error, format!("Error running --save: {e}"));
                Err(Box::<dyn std::error::Error>::from(e))
            }
        },
        Err(e) => {
            display::emit(
                LogLevel::Error,
                format!("Error generating diff for {commit_ref}: {e}"),
            );
            Err(Box::<dyn std::error::Error>::from(e))
        }
    }
}

#[derive(Debug)]
pub struct SaveOutcome {
    pub session_log: Option<String>,
    pub code_commit: Option<String>,
    pub pushed: bool,
    pub audit_state: auditor::AuditState,
    pub commit_mode: CommitMode,
}

fn format_save_outcome(outcome: &SaveOutcome, verbosity: Verbosity) -> String {
    let mut rows = vec![("Outcome".to_string(), "Save complete".to_string())];

    match &outcome.session_log {
        Some(path) if !path.is_empty() => rows.push(("Session".to_string(), path.clone())),
        _ => rows.push(("Session".to_string(), "none".to_string())),
    }

    match &outcome.code_commit {
        Some(hash) if !hash.is_empty() => {
            rows.push(("Code commit".to_string(), short_hash(hash)));
        }
        _ => rows.push(("Code commit".to_string(), "none".to_string())),
    }

    rows.push(("Mode".to_string(), outcome.commit_mode.label().to_string()));
    rows.push((
        "Narrative".to_string(),
        match outcome.audit_state {
            auditor::AuditState::Committed => "committed",
            auditor::AuditState::Pending => "pending",
            auditor::AuditState::Clean => "clean",
        }
        .to_string(),
    ));

    if outcome.pushed {
        rows.push(("Push".to_string(), "pushed".to_string()));
    }

    append_agent_rows(&mut rows, verbosity);
    format_block(rows)
}

fn short_hash(hash: &str) -> String {
    const MAX: usize = 8;
    if hash.len() <= MAX {
        hash.to_string()
    } else {
        hash.chars().take(MAX).collect()
    }
}

fn narrative_change_set(result: &auditor::AuditResult) -> (Vec<String>, Option<String>) {
    result
        .narrative_changes()
        .map(|changes| (changes.paths.clone(), changes.summary.clone()))
        .unwrap_or_else(|| (Vec::new(), None))
}

fn stage_narrative_paths(paths: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if paths.is_empty() {
        return Ok(());
    }

    let refs: Vec<&str> = paths.iter().map(|p| p.as_str()).collect();
    vcs::stage(Some(refs))?;
    Ok(())
}

fn trim_staged_vizier_paths(allowed: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let allowlist: HashSet<&str> = allowed.iter().map(|p| p.as_str()).collect();
    let staged = vcs::snapshot_staged(".")?;
    let mut to_unstage: Vec<String> = Vec::new();

    for item in staged {
        match &item.kind {
            vcs::StagedKind::Renamed { from, to } => {
                if to.starts_with(".vizier/") && !allowlist.contains(to.as_str()) {
                    to_unstage.push(from.clone());
                    to_unstage.push(to.clone());
                }
            }
            _ => {
                if item.path.starts_with(".vizier/") && !allowlist.contains(item.path.as_str()) {
                    to_unstage.push(item.path.clone());
                }
            }
        }
    }

    if !to_unstage.is_empty() {
        to_unstage.sort();
        to_unstage.dedup();
        let refs: Vec<&str> = to_unstage.iter().map(|p| p.as_str()).collect();
        vcs::unstage(Some(refs))?;
    }

    Ok(())
}

fn clear_narrative_tracker(paths: &[String]) {
    file_tracking::FileTracker::clear_tracked(paths);
}

fn build_save_instruction(note: Option<&str>) -> String {
    let mut instruction =
        "<instruction>Update the snapshot and supporting narrative docs as needed</instruction>"
            .to_string();

    if let Some(text) = note {
        instruction.push_str(&format!(
            "<change_author_note>{}</change_author_note>",
            text
        ));
    }

    instruction
}

async fn save(
    _initial_diff: String,
    // NOTE: These two should never be Some(...) && true
    user_message: Option<String>,
    use_message_editor: bool,
    commit_mode: CommitMode,
    push_after_commit: bool,
    agent: &config::AgentSettings,
) -> Result<SaveOutcome, Box<dyn std::error::Error>> {
    let provided_note = if let Some(message) = user_message {
        Some(message)
    } else if use_message_editor {
        if let Ok(edited) = get_editor_message() {
            Some(edited)
        } else {
            None
        }
    } else {
        None
    };

    let save_instruction = build_save_instruction(provided_note.as_deref());
    let prompt_agent = agent.for_prompt(config::PromptKind::Documentation)?;

    let system_prompt = agent_prompt::build_documentation_prompt(
        prompt_agent.prompt_selection(),
        &save_instruction,
        &prompt_agent.documentation,
    )
    .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;

    let response = Auditor::llm_request_with_tools(
        &prompt_agent,
        Some(config::PromptKind::Documentation),
        system_prompt,
        save_instruction,
        None,
    )
    .await?;

    let audit_result = auditor::Auditor::finalize(audit_disposition(commit_mode)).await?;
    let session_display = audit_result.session_display();
    let (narrative_paths, narrative_summary) = narrative_change_set(&audit_result);
    let has_narrative_changes = !narrative_paths.is_empty();

    let mut summary_rows = vec![(
        "Assistant summary".to_string(),
        response.content.trim().to_string(),
    )];
    append_agent_rows(&mut summary_rows, current_verbosity());
    let summary_block = format_block(summary_rows);
    if !summary_block.is_empty() {
        for line in summary_block.lines() {
            display::info(line.to_string());
        }
    }

    let post_tool_diff = vcs::get_diff(".", Some("HEAD"), Some(&[".vizier/"]))?;
    let has_code_changes = !post_tool_diff.trim().is_empty();
    let mut code_commit = None;

    if commit_mode.should_commit() {
        if has_code_changes || has_narrative_changes {
            let commit_body = if has_code_changes {
                Auditor::llm_request(
                    config::get_config().get_prompt(config::PromptKind::Commit),
                    post_tool_diff.clone(),
                )
                .await?
                .content
            } else {
                narrative_summary
                    .clone()
                    .unwrap_or_else(|| "Update snapshot and narrative docs".to_string())
            };

            let mut message_builder = CommitMessageBuilder::new(commit_body);
            message_builder
                .set_header(if has_code_changes {
                    CommitMessageType::CodeChange
                } else {
                    CommitMessageType::NarrativeChange
                })
                .with_session_log_path(session_display.clone());

            if has_code_changes {
                message_builder.with_narrative_summary(narrative_summary.clone());
            }

            if let Some(note) = provided_note.as_ref() {
                message_builder.with_author_note(note.clone());
            }

            stage_narrative_paths(&narrative_paths)?;
            if has_code_changes {
                vcs::stage(Some(vec!["."]))?;
            } else {
                vcs::stage(None)?;
            }
            trim_staged_vizier_paths(&narrative_paths)?;

            let commit_message = message_builder.build();

            display::info("Committing combined changes...");
            let commit_oid = vcs::commit_staged(&commit_message, false)?;
            display::info(format!(
                "Changes committed with message: {}",
                commit_message
            ));

            clear_narrative_tracker(&narrative_paths);
            code_commit = Some(commit_oid.to_string());
        } else {
            display::info("No code or narrative changes detected; skipping commit.");
        }
    } else {
        if has_narrative_changes {
            display::info(
                "Snapshot/narrative updates left pending (--no-commit); review and commit when ready.",
            );
        }
        if has_code_changes {
            display::info(
                "Code changes detected but --no-commit is active; leaving them staged/dirty.",
            );
        } else if provided_note.is_some() {
            display::info(
                "Author note provided but no code changes detected; skipping code commit.",
            );
        }
    }

    let mut pushed = false;
    if commit_mode.should_commit() && push_after_commit {
        if code_commit.is_some() {
            push_origin_if_requested(true)?;
            pushed = true;
        } else {
            display::info("Push skipped because no commit was created.");
        }
    } else if push_after_commit {
        display::info("Push skipped because --no-commit is active.");
    }

    Ok(SaveOutcome {
        session_log: session_display,
        code_commit,
        pushed,
        audit_state: audit_result.state,
        commit_mode,
    })
}

enum Shell {
    Bash,
    Zsh,
    Fish,
    Other,
}

impl Shell {
    fn from_path(shell_path: &str) -> Self {
        let shell_name = std::path::PathBuf::from(shell_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_lowercase();

        match shell_name.as_str() {
            "bash" => Shell::Bash,
            "zsh" => Shell::Zsh,
            "fish" => Shell::Fish,
            _ => Shell::Other,
        }
    }

    fn get_rc_source_command(&self) -> String {
        match self {
            Shell::Bash => ". ~/.bashrc".to_string(),
            Shell::Zsh => ". ~/.zshrc".to_string(),
            Shell::Fish => "source ~/.config/fish/config.fish".to_string(),
            Shell::Other => "".to_string(),
        }
    }

    fn get_interactive_args(&self) -> Vec<String> {
        match self {
            Shell::Fish => vec!["-C".to_string()],
            _ => vec!["-i".to_string(), "-c".to_string()],
        }
    }
}

fn get_editor_message() -> Result<String, Box<dyn std::error::Error>> {
    let temp_file = Builder::new()
        .prefix("tllm_input")
        .suffix(".md")
        .tempfile()?;

    let temp_path: TempPath = temp_file.into_temp_path();

    match std::fs::write(temp_path.to_path_buf(), "") {
        Ok(_) => {}
        Err(e) => {
            display::emit(LogLevel::Error, "Error writing to temp file");
            return Err(Box::new(e));
        }
    };

    let shell_path = env::var("SHELL").unwrap_or_else(|_| "bash".to_string());
    let editor = env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
    let shell = Shell::from_path(&shell_path);

    let command = format!("{} {}", editor, temp_path.to_str().unwrap());
    let rc_source = shell.get_rc_source_command();
    let full_command = if rc_source.is_empty() {
        command
    } else {
        format!("{} && {}", rc_source, command)
    };

    let status = Command::new(shell_path)
        .args(shell.get_interactive_args())
        .arg("-c")
        .arg(&full_command)
        .status()?;

    if !status.success() {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Editor command failed",
        )));
    }

    let user_message = match fs::read_to_string(&temp_path) {
        Ok(contents) => {
            if contents.is_empty() {
                return Ok(String::new());
            }

            contents
        }
        Err(e) => {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Error reading file: {}", e),
            )));
        }
    };

    Ok(user_message)
}

/// NOTE: Filters out hidden entries; every visible file in `.vizier/` is treated as part of the narrative surface.
///
/// This is `vizier ask`
pub async fn inline_command(
    user_message: String,
    push_after_commit: bool,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
) -> Result<(), Box<dyn std::error::Error>> {
    let prompt_agent = agent.for_prompt(config::PromptKind::Documentation)?;
    let system_prompt = match agent_prompt::build_documentation_prompt(
        prompt_agent.prompt_selection(),
        &user_message,
        &prompt_agent.documentation,
    ) {
        Ok(prompt) => prompt,
        Err(e) => {
            display::emit(
                LogLevel::Error,
                format!("Error building agent prompt: {}", e),
            );
            return Err(Box::<dyn std::error::Error>::from(e));
        }
    };

    let response = match Auditor::llm_request_with_tools(
        &prompt_agent,
        Some(config::PromptKind::Documentation),
        system_prompt,
        user_message,
        None,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            display::emit(LogLevel::Error, format!("Error during LLM request: {e}"));
            return Err(Box::<dyn std::error::Error>::from(e));
        }
    };

    let audit_result = match auditor::Auditor::finalize(audit_disposition(commit_mode)).await {
        Ok(outcome) => outcome,
        Err(e) => {
            display::emit(LogLevel::Error, format!("Error finalizing audit: {e}"));
            return Err(Box::<dyn std::error::Error>::from(e));
        }
    };
    let session_display = audit_result.session_display();
    let (narrative_paths, narrative_summary) = narrative_change_set(&audit_result);
    let has_narrative_changes = !narrative_paths.is_empty();

    let post_tool_diff = vcs::get_diff(".", Some("HEAD"), Some(&[".vizier/"]))?;
    let has_code_changes = !post_tool_diff.trim().is_empty();
    let mut commit_oid: Option<String> = None;

    if commit_mode.should_commit() {
        if has_code_changes || has_narrative_changes {
            let commit_body = if has_code_changes {
                Auditor::llm_request(
                    config::get_config().get_prompt(config::PromptKind::Commit),
                    post_tool_diff.clone(),
                )
                .await?
                .content
            } else {
                narrative_summary
                    .clone()
                    .unwrap_or_else(|| "Update snapshot and narrative docs".to_string())
            };

            let mut builder = CommitMessageBuilder::new(commit_body);
            builder
                .set_header(if has_code_changes {
                    CommitMessageType::CodeChange
                } else {
                    CommitMessageType::NarrativeChange
                })
                .with_session_log_path(session_display.clone());

            if has_code_changes {
                builder.with_narrative_summary(narrative_summary.clone());
            }

            stage_narrative_paths(&narrative_paths)?;
            if has_code_changes {
                vcs::stage(Some(vec!["."]))?;
            } else {
                vcs::stage(None)?;
            }
            trim_staged_vizier_paths(&narrative_paths)?;

            let commit_message = builder.build();
            let oid = vcs::commit_staged(&commit_message, false)?;
            clear_narrative_tracker(&narrative_paths);
            commit_oid = Some(oid.to_string());
        } else {
            display::info("No code or narrative changes detected; skipping commit.");
        }
    } else {
        if has_narrative_changes {
            display::info(
                "Held .vizier changes for manual review (--no-commit active); commit them when ready.",
            );
        }
        if has_code_changes {
            display::info(
                "Code changes detected but --no-commit is active; leaving them staged/dirty.",
            );
        }
    }

    if commit_mode.should_commit() {
        if commit_oid.is_some() {
            push_origin_if_requested(push_after_commit)?;
        } else if push_after_commit {
            display::info("Push skipped because no commit was created.");
        }
    } else if push_after_commit {
        display::info("Push skipped because --no-commit is active.");
    }

    println!("{}", response.content.trim_end());
    print_agent_summary();

    Ok(())
}

pub async fn run_draft(
    args: DraftArgs,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
) -> Result<(), Box<dyn std::error::Error>> {
    require_agent_backend(
        agent,
        config::PromptKind::ImplementationPlan,
        "vizier draft requires an agent-style backend; update [agents.draft] or pass --backend agent|gemini",
    )?;

    let DraftArgs {
        spec_text,
        spec_source,
        name_override,
    } = args;

    let repo_root = repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let plan_dir_main = repo_root.join(".vizier/implementation-plans");
    let branch_prefix = "draft/";

    let base_slug = if let Some(name) = name_override {
        plan::sanitize_name_override(&name)?
    } else {
        plan::slug_from_spec(&spec_text)
    };

    let slug = plan::ensure_unique_slug(&base_slug, &plan_dir_main, branch_prefix)?;
    let branch_name = format!("{branch_prefix}{slug}");
    let plan_rel_path = plan::plan_rel_path(&slug);
    let plan_display = plan_rel_path.to_string_lossy().to_string();

    let tmp_root = repo_root.join(".vizier/tmp-worktrees");
    fs::create_dir_all(&tmp_root)?;
    let worktree_suffix = plan::short_suffix();
    let worktree_name = format!("vizier-draft-{slug}-{worktree_suffix}");
    let worktree_path = tmp_root.join(format!("{slug}-{worktree_suffix}"));
    let plan_in_worktree = worktree_path.join(&plan_rel_path);
    let spec_source_label = spec_source.as_metadata_value();
    display::debug(format!(
        "Drafting plan {slug} from spec source: {spec_source_label}"
    ));

    let mut plan_document_preview: Option<String> = None;
    let mut session_artifact: Option<auditor::SessionArtifact> = None;
    let primary_branch = detect_primary_branch()
        .ok_or_else(|| "unable to detect a primary branch (tried origin/HEAD, main, master)")?;

    let mut branch_created = false;
    let mut worktree_created = false;
    let mut plan_committed = false;

    let plan_result: Result<(), Box<dyn std::error::Error>> = async {
        create_branch_from(&primary_branch, &branch_name).map_err(|err| {
            Box::<dyn std::error::Error>::from(format!(
                "create_branch_from({}<-{}): {}",
                branch_name, primary_branch, err
            ))
        })?;
        branch_created = true;

        add_worktree_for_branch(&worktree_name, &worktree_path, &branch_name).map_err(|err| {
            display::emit(
                LogLevel::Debug,
                format!(
                    "failed adding worktree {} at {}: {}",
                    worktree_name,
                    worktree_path.display(),
                    err
                ),
            );
            Box::<dyn std::error::Error>::from(format!(
                "add_worktree({}, {}): {}",
                worktree_name,
                worktree_path.display(),
                err
            ))
        })?;
        worktree_created = true;
        let _cwd_guard = WorkdirGuard::enter(&worktree_path)?;

        let prompt_agent = agent.for_prompt(config::PromptKind::ImplementationPlan)?;
        let selection = prompt_selection(&prompt_agent)?;
        let prompt = agent_prompt::build_implementation_plan_prompt(
            selection,
            &slug,
            &branch_name,
            &spec_text,
            &prompt_agent.documentation,
        )
        .map_err(|err| -> Box<dyn std::error::Error> {
            Box::from(format!("build_prompt: {err}"))
        })?;

        let llm_response = Auditor::llm_request_with_tools(
            &prompt_agent,
            Some(config::PromptKind::ImplementationPlan),
            prompt,
            spec_text.clone(),
            Some(worktree_path.clone()),
        )
        .await
        .map_err(|err| Box::<dyn std::error::Error>::from(format!("Agent backend: {err}")))?;

        let plan_body = llm_response.content;
        let document = plan::render_plan_document(
            &slug,
            &branch_name,
            &spec_text,
            &plan_body,
        );
        plan_document_preview = Some(document.clone());
        plan::write_plan_file(&plan_in_worktree, &document).map_err(
            |err| -> Box<dyn std::error::Error> {
                Box::from(format!(
                    "write_plan_file({}): {err}",
                    plan_in_worktree.display()
                ))
            },
        )?;

        if commit_mode.should_commit() {
            let plan_rel = plan_rel_path.as_path();
            commit_paths_in_repo(
                &worktree_path,
                &[plan_rel],
                &format!("docs: add implementation plan {}", slug),
            )
            .map_err(|err| -> Box<dyn std::error::Error> {
                Box::from(format!("commit_plan({}): {err}", worktree_path.display()))
            })?;
            plan_committed = true;
            if let Some(artifact) = Auditor::persist_session_log() {
                session_artifact = Some(artifact);
                Auditor::clear_messages();
            }
        } else {
            display::info(
                "Plan document generated with --no-commit; leaving worktree dirty for manual review.",
            );
        }

        Ok(())
    }
    .await;

    match plan_result {
        Ok(()) => {
            let plan_to_print = plan_document_preview
                .clone()
                .or_else(|| fs::read_to_string(&plan_in_worktree).ok());

            if let Some(artifact) = session_artifact.as_ref() {
                copy_session_log_to_repo_root(&repo_root, artifact);
            }

            if worktree_created && commit_mode.should_commit() {
                if let Err(err) = remove_worktree(&worktree_name, true) {
                    display::warn(format!(
                        "temporary worktree cleanup failed ({}); remove manually with `git worktree prune`",
                        err
                    ));
                }
                if worktree_path.exists() {
                    let _ = fs::remove_dir_all(&worktree_path);
                }
            }

            if commit_mode.should_commit() {
                display::info(format!(
                    "View with: git checkout {branch_name} && $EDITOR {plan_display}"
                ));

                let mut rows = vec![
                    ("Outcome".to_string(), "Draft ready".to_string()),
                    ("Plan".to_string(), plan_display.clone()),
                    ("Branch".to_string(), branch_name.clone()),
                ];
                append_agent_rows(&mut rows, current_verbosity());
                println!("{}", format_block(rows));
            } else {
                let mut rows = vec![
                    (
                        "Outcome".to_string(),
                        "Draft pending (manual commit)".to_string(),
                    ),
                    ("Branch".to_string(), branch_name.clone()),
                    ("Worktree".to_string(), worktree_path.display().to_string()),
                    ("Plan".to_string(), plan_display.clone()),
                ];
                append_agent_rows(&mut rows, current_verbosity());
                println!("{}", format_block(rows));
                display::info(format!(
                    "Review and commit manually: git -C {} status",
                    worktree_path.display()
                ));
            }

            if let Some(plan_text) = plan_to_print {
                println!();
                println!("{plan_text}");
            }
            Ok(())
        }
        Err(err) => {
            if worktree_created {
                let _ = remove_worktree(&worktree_name, true);
                if worktree_path.exists() {
                    let _ = fs::remove_dir_all(&worktree_path);
                }
            }
            if branch_created && !plan_committed {
                let _ = delete_branch(&branch_name);
            } else if plan_committed {
                display::info(format!(
                    "Draft artifacts preserved on {branch_name}; inspect with `git checkout {branch_name}`"
                ));
            }
            Err(err)
        }
    }
}

pub fn run_list(opts: ListOptions) -> Result<(), Box<dyn std::error::Error>> {
    list_pending_plans(opts.target)
}

const DEFAULT_TEST_PROMPT: &str = "Smoke-test the configured agent: emit a few progress updates (no writes, keep it short) and a final response.";

pub async fn run_test_display(
    opts: TestDisplayOptions,
    agent: &config::AgentSettings,
) -> Result<(), Box<dyn std::error::Error>> {
    if !agent.backend.requires_agent_runner() {
        return Err(format!(
            "vizier test-display requires an agent-capable backend; `{}` is configured for scope `{}`",
            agent.backend,
            agent.scope.as_str()
        )
        .into());
    }

    Auditor::record_agent_context(agent, None);
    let runner = Arc::clone(agent.agent_runner()?);
    let repo_root = repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let prompt = opts
        .prompt_override
        .clone()
        .unwrap_or_else(|| DEFAULT_TEST_PROMPT.to_string());

    let mut request = build_agent_request(agent, prompt.clone(), repo_root);
    if opts.disable_wrapper {
        request.allow_script_wrapper = false;
    }
    if let Some(timeout) = opts.timeout {
        request.timeout = Some(timeout);
    }

    let (progress_tx, progress_rx) = mpsc::channel(64);
    let progress_handle = spawn_plain_progress_logger(progress_rx);
    let result = runner
        .execute(request, Some(ProgressHook::Plain(progress_tx)))
        .await;
    if let Some(handle) = progress_handle {
        let _ = handle.await;
    }

    match result {
        Ok(response) => {
            let session_path = if opts.record_session {
                record_test_display_session(agent, &prompt, &response)
            } else {
                None
            };
            emit_test_display_summary(agent, &response, opts.raw_output, session_path);
            Ok(())
        }
        Err(AgentError::NonZeroExit(code, stderr)) => {
            render_test_display_failure(agent, code, &stderr);
            if opts.raw_output
                && !stderr.is_empty()
                && !matches!(current_verbosity(), Verbosity::Quiet)
            {
                for line in &stderr {
                    eprintln!("{line}");
                }
            }
            if opts.record_session {
                let _ = record_test_display_failure(agent, &prompt, code, &stderr);
            }
            std::process::exit(if code == 0 { 1 } else { code });
        }
        Err(AgentError::Timeout(secs)) => {
            render_test_display_timeout(agent, secs);
            if opts.record_session {
                let _ = record_test_display_failure(
                    agent,
                    &prompt,
                    124,
                    &vec![format!("timeout after {secs}s")],
                );
            }
            std::process::exit(124);
        }
        Err(err) => Err(Box::new(err)),
    }
}

fn emit_test_display_summary(
    agent: &config::AgentSettings,
    response: &AgentResponse,
    raw_output: bool,
    session_path: Option<String>,
) {
    if matches!(current_verbosity(), Verbosity::Quiet) {
        return;
    }

    let mut rows = vec![
        (
            "Outcome".to_string(),
            "Agent display test succeeded".to_string(),
        ),
        ("Scope".to_string(), agent.scope.as_str().to_string()),
        ("Backend".to_string(), agent.backend.to_string()),
        ("Exit code".to_string(), response.exit_code.to_string()),
        (
            "Duration".to_string(),
            format!("{:.2}s", response.duration_ms as f64 / 1000.0),
        ),
    ];

    if let Some(path) = session_path {
        rows.push(("Session".to_string(), path));
    }

    if !raw_output {
        let stdout_snippet = response
            .assistant_text
            .trim()
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        rows.push((
            "Stdout".to_string(),
            if stdout_snippet.is_empty() {
                "<empty>".to_string()
            } else {
                clip_message(&stdout_snippet)
            },
        ));
        if let Some(last_stderr) = response.stderr.last() {
            rows.push(("Stderr".to_string(), clip_message(last_stderr)));
        }
    }

    println!("{}", format_block(rows));

    if raw_output {
        if !response.assistant_text.is_empty() {
            println!("{}", response.assistant_text.trim_end());
        }
        if !response.stderr.is_empty() {
            for line in &response.stderr {
                eprintln!("{line}");
            }
        }
    }
}

fn render_test_display_failure(agent: &config::AgentSettings, code: i32, stderr: &[String]) {
    let mut message = format!(
        "agent for `{}` exited with status {code}",
        agent.scope.as_str()
    );
    if let Some(line) = stderr.last() {
        message.push_str(&format!("; stderr: {line}"));
    }
    display::emit(LogLevel::Error, message);
    if matches!(current_verbosity(), Verbosity::Debug) && !stderr.is_empty() {
        for line in stderr {
            display::debug(format!("stderr: {line}"));
        }
    }
}

fn render_test_display_timeout(agent: &config::AgentSettings, secs: u64) {
    display::emit(
        LogLevel::Error,
        format!(
            "agent for `{}` timed out after {secs}s",
            agent.scope.as_str()
        ),
    );
}

fn record_test_display_session(
    agent: &config::AgentSettings,
    prompt: &str,
    response: &AgentResponse,
) -> Option<String> {
    auditor::Auditor::add_message(Message::user(prompt.to_string()));
    auditor::Auditor::add_message(Message::assistant(response.assistant_text.clone()));
    auditor::Auditor::record_agent_run(AgentRunRecord {
        command: agent.agent_runtime.command.clone(),
        output: agent.agent_runtime.output,
        progress_filter: agent.agent_runtime.progress_filter.clone(),
        exit_code: response.exit_code,
        stdout: response.assistant_text.clone(),
        stderr: response.stderr.clone(),
        duration_ms: response.duration_ms,
    });
    persist_session_log_with_notice()
}

fn record_test_display_failure(
    agent: &config::AgentSettings,
    prompt: &str,
    exit_code: i32,
    stderr: &[String],
) -> Option<String> {
    auditor::Auditor::add_message(Message::user(prompt.to_string()));
    let assistant_text = if stderr.is_empty() {
        format!("agent exited with status {exit_code}")
    } else {
        stderr.join("\n")
    };
    auditor::Auditor::add_message(Message::assistant(assistant_text));
    auditor::Auditor::record_agent_run(AgentRunRecord {
        command: agent.agent_runtime.command.clone(),
        output: agent.agent_runtime.output,
        progress_filter: agent.agent_runtime.progress_filter.clone(),
        exit_code,
        stdout: String::new(),
        stderr: stderr.to_vec(),
        duration_ms: 0,
    });
    persist_session_log_with_notice()
}

fn persist_session_log_with_notice() -> Option<String> {
    match auditor::Auditor::persist_session_log() {
        Some(artifact) => {
            auditor::Auditor::clear_messages();
            Some(artifact.display_path())
        }
        None => {
            if config::get_config().no_session {
                display::info("Session logging disabled (--no-session); no session file written.");
            }
            None
        }
    }
}

pub async fn run_approve(
    opts: ApproveOptions,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
) -> Result<(), Box<dyn std::error::Error>> {
    if opts.list_only {
        display::warn("`vizier approve --list` is deprecated; use `vizier list` instead.");
        return list_pending_plans(opts.target.clone());
    }

    require_agent_backend(
        agent,
        config::PromptKind::Documentation,
        "vizier approve requires an agent-style backend; update [agents.approve] or pass --backend agent|gemini",
    )?;

    let spec = plan::PlanBranchSpec::resolve(
        opts.plan.as_deref(),
        opts.branch_override.as_deref(),
        opts.target.as_deref(),
    )?;

    vcs::ensure_clean_worktree().map_err(|err| {
        Box::<dyn std::error::Error>::from(format!(
            "clean working tree required before approval: {err}"
        ))
    })?;

    let repo = Repository::discover(".")?;
    let source_ref = repo
        .find_branch(&spec.branch, BranchType::Local)
        .map_err(|_| format!("draft branch {} not found", spec.branch))?;
    let source_commit = source_ref.get().peel_to_commit()?;
    let source_oid = source_commit.id();

    let target_ref = repo
        .find_branch(&spec.target_branch, BranchType::Local)
        .map_err(|_| format!("target branch {} not found", spec.target_branch))?;
    let target_commit = target_ref.into_reference().peel_to_commit()?;
    let target_oid = target_commit.id();

    if repo.graph_descendant_of(target_oid, source_oid)? {
        let rows = vec![
            ("Outcome".to_string(), "Plan already merged".to_string()),
            ("Plan".to_string(), spec.slug.clone()),
            ("Target".to_string(), spec.target_branch.clone()),
            (
                "Latest commit".to_string(),
                short_hash(&source_oid.to_string()),
            ),
        ];
        println!("{}", format_block(rows));
        return Ok(());
    }

    if !repo.graph_descendant_of(source_oid, target_oid)? {
        display::warn(format!(
            "{} does not include the latest {} commits; merge may require manual resolution.",
            spec.branch, spec.target_branch
        ));
    }

    let plan_meta = spec.load_metadata()?;
    if plan_meta.branch != spec.branch {
        display::warn(format!(
            "Plan metadata references branch {} but CLI resolved to {}",
            plan_meta.branch, spec.branch
        ));
    }

    if !opts.assume_yes {
        spec.show_preview(&plan_meta);
        if !prompt_for_confirmation("Implement plan now? [y/N] ")? {
            println!("Approval cancelled; no changes were made.");
            return Ok(());
        }
    }

    let worktree = plan::PlanWorktree::create(&spec.slug, &spec.branch, "approve")?;
    let worktree_path = worktree.path().to_path_buf();
    let plan_path = worktree.plan_path(&spec.slug);
    let mut worktree = Some(worktree);

    let approval = apply_plan_in_worktree(
        &spec,
        &plan_meta,
        &worktree_path,
        &plan_path,
        opts.push_after,
        commit_mode,
        agent,
    )
    .await;

    match approval {
        Ok(result) => {
            if commit_mode.should_commit() {
                if let Some(tree) = worktree.take() {
                    if let Err(err) = tree.cleanup() {
                        display::warn(format!(
                            "temporary worktree cleanup failed ({}); remove manually with `git worktree prune`",
                            err
                        ));
                    }
                }

                let mut rows = vec![
                    ("Outcome".to_string(), "Plan implemented".to_string()),
                    ("Plan".to_string(), spec.slug.clone()),
                    ("Branch".to_string(), spec.branch.clone()),
                    ("Review".to_string(), spec.diff_command()),
                ];
                if let Some(commit_oid) = result.commit_oid.as_ref() {
                    rows.push(("Latest commit".to_string(), short_hash(commit_oid)));
                }
                append_agent_rows(&mut rows, current_verbosity());
                println!("{}", format_block(rows));
            } else {
                display::info(format!(
                    "Plan worktree preserved at {}; inspect branch {} for pending changes.",
                    worktree_path.display(),
                    spec.branch
                ));
                let mut rows = vec![
                    (
                        "Outcome".to_string(),
                        "Plan pending manual commit".to_string(),
                    ),
                    ("Plan".to_string(), spec.slug.clone()),
                    ("Branch".to_string(), spec.branch.clone()),
                    ("Worktree".to_string(), worktree_path.display().to_string()),
                    ("Review".to_string(), spec.diff_command()),
                ];
                append_agent_rows(&mut rows, current_verbosity());
                println!("{}", format_block(rows));
            }
            Ok(())
        }
        Err(err) => {
            if let Some(tree) = worktree.take() {
                display::warn(format!(
                    "Plan worktree preserved at {}; inspect branch {} for partial changes.",
                    tree.path().display(),
                    spec.branch
                ));
            }
            Err(err)
        }
    }
}

pub async fn run_review(
    opts: ReviewOptions,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
) -> Result<(), Box<dyn std::error::Error>> {
    require_agent_backend(
        agent,
        config::PromptKind::Review,
        "vizier review requires an agent-style backend; update [agents.review] or pass --backend agent|gemini",
    )?;

    let spec = plan::PlanBranchSpec::resolve(
        Some(opts.plan.as_str()),
        opts.branch_override.as_deref(),
        opts.target.as_deref(),
    )?;

    vcs::ensure_clean_worktree().map_err(|err| {
        Box::<dyn std::error::Error>::from(format!(
            "clean working tree required before review: {err}"
        ))
    })?;

    let repo = Repository::discover(".")?;
    let source_ref = repo
        .find_branch(&spec.branch, BranchType::Local)
        .map_err(|_| format!("draft branch {} not found", spec.branch))?;
    let source_commit = source_ref.get().peel_to_commit()?;
    let source_oid = source_commit.id();

    let target_ref = repo
        .find_branch(&spec.target_branch, BranchType::Local)
        .map_err(|_| format!("target branch {} not found", spec.target_branch))?;
    let target_commit = target_ref.into_reference().peel_to_commit()?;
    let target_oid = target_commit.id();

    if repo.graph_descendant_of(target_oid, source_oid)? {
        let rows = vec![
            ("Outcome".to_string(), "Plan already merged".to_string()),
            ("Plan".to_string(), spec.slug.clone()),
            ("Target".to_string(), spec.target_branch.clone()),
            (
                "Latest commit".to_string(),
                short_hash(&source_oid.to_string()),
            ),
        ];
        println!("{}", format_block(rows));
        return Ok(());
    }

    if !repo.graph_descendant_of(source_oid, target_oid)? {
        display::warn(format!(
            "{} does not include the latest {} commits; review may miss upstream changes.",
            spec.branch, spec.target_branch
        ));
    }

    let plan_meta = spec.load_metadata()?;
    let worktree = plan::PlanWorktree::create(&spec.slug, &spec.branch, "review")?;
    let plan_path = worktree.plan_path(&spec.slug);
    let worktree_path = worktree.path().to_path_buf();
    let mut worktree = Some(worktree);

    let review_result = perform_review_workflow(
        &spec,
        &plan_meta,
        &worktree_path,
        &plan_path,
        ReviewExecution {
            assume_yes: opts.assume_yes,
            review_only: opts.review_only,
            skip_checks: opts.skip_checks,
            cicd_gate: opts.cicd_gate.clone(),
            auto_resolve_requested: opts.auto_resolve_requested,
        },
        commit_mode,
        agent,
    )
    .await;

    match review_result {
        Ok(outcome) => {
            if commit_mode.should_commit() {
                if let Some(tree) = worktree.take() {
                    if let Err(err) = tree.cleanup() {
                        display::warn(format!(
                            "temporary worktree cleanup failed ({}); remove manually with `git worktree prune`",
                            err
                        ));
                    }
                }
            } else if let Some(tree) = worktree.take() {
                display::info(format!(
                    "Review worktree preserved at {}; inspect branch {} for pending critique/fix artifacts.",
                    tree.path().display(),
                    spec.branch
                ));
            }

            if outcome.branch_mutated && opts.push_after && commit_mode.should_commit() {
                push_origin_if_requested(true)?;
            } else if outcome.branch_mutated && opts.push_after {
                display::info("Push skipped because --no-commit left review changes pending.");
            }

            if let Some(commit) = outcome.fix_commit.as_ref() {
                display::info(format!(
                    "Fixes addressing review feedback committed at {} on {}",
                    commit, spec.branch
                ));
            }

            let repo_root = repo_root().ok();
            let mut rows = vec![
                ("Outcome".to_string(), "Review complete".to_string()),
                ("Plan".to_string(), spec.slug.clone()),
                ("Branch".to_string(), spec.branch.clone()),
                ("Critique".to_string(), outcome.critique_label.to_string()),
                (
                    "CI/CD gate".to_string(),
                    outcome.cicd_gate.summary_label(repo_root.as_deref()),
                ),
                (
                    "Checks".to_string(),
                    format!(
                        "{}/{}",
                        format_number(outcome.checks_passed),
                        format_number(outcome.checks_total)
                    ),
                ),
                ("Diff".to_string(), outcome.diff_command.clone()),
                (
                    "Session".to_string(),
                    outcome
                        .session_path
                        .clone()
                        .unwrap_or_else(|| "<unknown>".to_string()),
                ),
            ];
            let verbosity = current_verbosity();
            append_agent_rows(&mut rows, verbosity);
            println!("{}", format_block(rows));
            Ok(())
        }
        Err(err) => {
            if let Some(tree) = worktree.take() {
                display::warn(format!(
                    "Plan worktree preserved at {}; inspect branch {} for partial changes.",
                    tree.path().display(),
                    spec.branch
                ));
            }
            Err(err)
        }
    }
}

pub async fn run_merge(
    opts: MergeOptions,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
) -> Result<(), Box<dyn std::error::Error>> {
    if !commit_mode.should_commit() {
        return Err("--no-commit is not supported for vizier merge; rerun without the flag once you are ready to finalize the merge."
            .into());
    }
    let spec = plan::PlanBranchSpec::resolve(
        Some(opts.plan.as_str()),
        opts.branch_override.as_deref(),
        opts.target.as_deref(),
    )?;

    if matches!(opts.conflict_strategy, MergeConflictStrategy::Agent) {
        require_agent_backend(
            agent,
            config::PromptKind::MergeConflict,
            "Agent-based conflict resolution requires an agent-style backend; update [agents.merge] or rerun with --backend agent|gemini",
        )?;
    }
    display::warn(opts.conflict_auto_resolve.status_line());

    if opts.cicd_gate.auto_resolve && opts.cicd_gate.script.is_some() {
        let review_agent = agent.for_prompt(config::PromptKind::Review)?;
        if !review_agent.backend.requires_agent_runner() {
            display::warn(
                "CI/CD auto-remediation requested but [agents.merge] is not set to an agent-style backend; gate failures will abort without auto fixes.",
            );
        }
    }

    let repo = Repository::discover(".")?;
    let source_ref = repo
        .find_branch(&spec.branch, BranchType::Local)
        .map_err(|_| format!("draft branch {} not found", spec.branch))?;
    let source_commit = source_ref.get().peel_to_commit()?;
    let source_oid = source_commit.id();

    let target_ref = repo
        .find_branch(&spec.target_branch, BranchType::Local)
        .map_err(|_| format!("target branch {} not found", spec.target_branch))?;
    let target_commit = target_ref.into_reference().peel_to_commit()?;
    let target_oid = target_commit.id();

    if repo.graph_descendant_of(target_oid, source_oid)? {
        let rows = vec![
            ("Outcome".to_string(), "Plan already merged".to_string()),
            ("Plan".to_string(), spec.slug.clone()),
            ("Target".to_string(), spec.target_branch.clone()),
            (
                "Latest commit".to_string(),
                short_hash(&source_oid.to_string()),
            ),
        ];
        println!("{}", format_block(rows));
        return Ok(());
    }

    match try_complete_pending_merge(
        &spec,
        opts.conflict_strategy,
        opts.conflict_auto_resolve,
        agent,
    )
    .await?
    {
        PendingMergeStatus::Ready {
            merge_oid,
            source_oid,
        } => {
            let gate_summary = run_cicd_gate_for_merge(&spec, &opts, agent).await?;
            let execution = MergeExecutionResult {
                merge_oid,
                source_oid,
                gate: gate_summary,
                squashed: false,
                implementation_oid: None,
            };
            finalize_merge(&spec, execution, opts.delete_branch, opts.push_after)?;
            return Ok(());
        }
        PendingMergeStatus::SquashReady {
            source_oid,
            merge_message,
        } => {
            let execution = finalize_squashed_merge_from_head(
                &spec,
                &merge_message,
                source_oid,
                None,
                &opts,
                agent,
            )
            .await?;
            finalize_merge(&spec, execution, opts.delete_branch, opts.push_after)?;
            return Ok(());
        }
        PendingMergeStatus::Blocked(blocker) => {
            return Err(Box::new(PendingMergeError::new(spec.slug.clone(), blocker)));
        }
        PendingMergeStatus::None => {
            if opts.complete_conflict {
                return Err(Box::<dyn std::error::Error>::from(format!(
                    "No Vizier-managed merge is awaiting completion for plan {}; rerun `vizier merge {}` without --complete-conflict to start a merge.",
                    spec.slug, spec.slug
                )));
            }
        }
    }

    vcs::ensure_clean_worktree().map_err(|err| {
        Box::<dyn std::error::Error>::from(format!(
            "clean working tree required before merge: {err}"
        ))
    })?;

    let plan_meta = spec.load_metadata()?;

    if !opts.assume_yes {
        spec.show_preview(&plan_meta);
        if !prompt_for_confirmation("Merge this plan? [y/N] ")? {
            println!("Merge cancelled; no changes were made.");
            return Ok(());
        }
    }

    let worktree = plan::PlanWorktree::create(&spec.slug, &spec.branch, "merge")?;
    let worktree_path = worktree.path().to_path_buf();
    let plan_path = worktree.plan_path(&spec.slug);
    let mut worktree = Some(worktree);
    let plan_document = fs::read_to_string(&plan_path).ok();

    if plan_path.exists() {
        display::info(format!(
            "Removing {} from the plan branch before merge",
            spec.plan_rel_path().display()
        ));
        if let Err(err) = fs::remove_file(&plan_path) {
            return Err(Box::<dyn std::error::Error>::from(format!(
                "failed to remove {} before merge: {}",
                plan_path.display(),
                err
            )));
        }
    }

    if let Err(err) =
        refresh_plan_branch(&spec, &plan_meta, &worktree_path, opts.push_after, agent).await
    {
        display::warn(format!(
            "Plan worktree preserved at {}; inspect {} for unresolved narrative changes.",
            worktree.as_ref().unwrap().path().display(),
            spec.branch
        ));
        return Err(err);
    }

    if let Some(tree) = worktree.take() {
        if let Err(err) = tree.cleanup() {
            display::warn(format!(
                "temporary worktree cleanup failed ({}); remove manually with `git worktree prune`",
                err
            ));
        }
    }

    let current_branch = current_branch_name(&repo)?;
    if current_branch.as_deref() != Some(spec.target_branch.as_str()) {
        display::info(format!(
            "Checking out {} before merge...",
            spec.target_branch
        ));
        vcs::checkout_branch(&spec.target_branch)?;
    }

    let implementation_message = build_implementation_commit_message(&spec, &plan_meta);
    let merge_message = build_merge_commit_message(
        &spec,
        &plan_meta,
        plan_document.as_deref(),
        opts.note.as_deref(),
    );
    let mut squash_plan = None;
    if opts.squash {
        squash_plan = Some(resolve_squash_plan_and_mainline(&spec, &opts)?);
    }

    let execution = if opts.squash {
        let (plan, mainline) = squash_plan.expect("missing squash plan despite squash=true");
        execute_squashed_merge(
            &spec,
            &implementation_message,
            &merge_message,
            &opts,
            plan,
            mainline,
            agent,
        )
        .await?
    } else {
        let preparation = prepare_merge(&spec.branch)?;
        execute_legacy_merge(&spec, &merge_message, preparation, &opts, agent).await?
    };

    finalize_merge(&spec, execution, opts.delete_branch, opts.push_after)?;
    Ok(())
}

fn resolve_squash_plan_and_mainline(
    spec: &plan::PlanBranchSpec,
    opts: &MergeOptions,
) -> Result<(vcs::SquashPlan, Option<u32>), Box<dyn std::error::Error>> {
    let plan = build_squash_plan(&spec.branch)?;
    if plan.merge_commits.is_empty() {
        return Ok((plan, opts.squash_mainline));
    }

    display::warn(
        "Plan branch contains merge commits; squash mode requires choosing a mainline parent.",
    );
    for merge in &plan.merge_commits {
        let parents = merge
            .parents
            .iter()
            .map(|oid| short_hash(&oid.to_string()))
            .collect::<Vec<_>>()
            .join(", ");
        let summary = merge.summary.as_deref().unwrap_or("no subject");
        display::warn(format!(
            "  - {} (parents: {}) - {}",
            short_hash(&merge.oid.to_string()),
            parents,
            summary
        ));
    }

    if plan
        .merge_commits
        .iter()
        .any(|merge| merge.parents.len() > 2)
    {
        return Err(format!(
            "Plan branch {} contains octopus merges; rerun vizier merge {} with --no-squash or rewrite the branch history.",
            spec.slug, spec.slug
        )
        .into());
    }

    if let Some(mainline) = opts.squash_mainline {
        validate_squash_mainline(mainline, &plan.merge_commits)?;
        if plan.mainline_ambiguous {
            display::warn(
                "Merge history is ambiguous; proceeding with the provided --squash-mainline value.",
            );
        } else if let Some(inferred) = plan.inferred_mainline {
            if inferred != mainline {
                display::warn(format!(
                    "Inferred mainline {} differs from provided {}; continuing with the provided value.",
                    inferred, mainline
                ));
            }
        }
        return Ok((plan, Some(mainline)));
    }

    let hint = plan
        .inferred_mainline
        .map(|hint| format!(" (suggested mainline: {hint})"))
        .unwrap_or_default();
    let mut guidance = format!(
        "Plan branch {} includes merge commits; rerun with --squash-mainline <parent index>{hint} or use --no-squash to keep the branch history.",
        spec.slug
    );
    if plan.mainline_ambiguous {
        guidance.push_str(" Merge history appears ambiguous; --no-squash is safest.");
    }

    Err(guidance.into())
}

fn validate_squash_mainline(
    mainline: u32,
    merges: &[vcs::MergeCommitSummary],
) -> Result<(), Box<dyn std::error::Error>> {
    if mainline == 0 {
        return Err("squash mainline parent index must be at least 1".into());
    }

    for merge in merges {
        if mainline as usize > merge.parents.len() {
            return Err(format!(
                "squash mainline parent {} is out of range for merge commit {}",
                mainline,
                short_hash(&merge.oid.to_string())
            )
            .into());
        }
    }

    Ok(())
}

async fn execute_legacy_merge(
    spec: &plan::PlanBranchSpec,
    merge_message: &str,
    preparation: MergePreparation,
    opts: &MergeOptions,
    agent: &config::AgentSettings,
) -> Result<MergeExecutionResult, Box<dyn std::error::Error>> {
    let (merge_oid, source_oid) = match preparation {
        MergePreparation::Ready(ready) => {
            let source_tip = ready.source_oid;
            let oid = commit_ready_merge(merge_message, ready)?;
            (oid, source_tip)
        }
        MergePreparation::Conflicted(conflict) => {
            match handle_merge_conflict(
                spec,
                merge_message,
                conflict,
                opts.conflict_auto_resolve,
                opts.conflict_strategy,
                false,
                None,
                agent,
            )
            .await?
            {
                MergeConflictResolution::MergeCommitted {
                    merge_oid,
                    source_oid,
                } => (merge_oid, source_oid),
                MergeConflictResolution::SquashImplementationCommitted { .. } => {
                    return Err(
                        "internal error: squashed merge resolution returned for legacy path".into(),
                    );
                }
            }
        }
    };

    let gate = run_cicd_gate_for_merge(spec, opts, agent).await?;
    Ok(MergeExecutionResult {
        merge_oid,
        source_oid,
        gate,
        squashed: false,
        implementation_oid: None,
    })
}

async fn execute_squashed_merge(
    spec: &plan::PlanBranchSpec,
    implementation_message: &str,
    merge_message: &str,
    opts: &MergeOptions,
    plan: vcs::SquashPlan,
    squash_mainline: Option<u32>,
    agent: &config::AgentSettings,
) -> Result<MergeExecutionResult, Box<dyn std::error::Error>> {
    match apply_cherry_pick_sequence(
        plan.target_head,
        &plan.commits_to_apply,
        None,
        squash_mainline,
    )? {
        CherryPickOutcome::Completed(result) => {
            let expected_head = result.applied.last().copied().unwrap_or(plan.target_head);
            let implementation_oid =
                commit_soft_squash(implementation_message, plan.target_head, expected_head)?;
            finalize_squashed_merge_from_head(
                spec,
                merge_message,
                plan.source_tip,
                Some(implementation_oid),
                opts,
                agent,
            )
            .await
        }
        CherryPickOutcome::Conflicted(conflict) => {
            match handle_squash_apply_conflict(
                spec,
                merge_message,
                implementation_message,
                &plan,
                squash_mainline,
                conflict,
                opts.conflict_auto_resolve,
                opts.conflict_strategy,
                agent,
            )
            .await?
            {
                MergeConflictResolution::SquashImplementationCommitted {
                    source_oid,
                    implementation_oid,
                } => {
                    finalize_squashed_merge_from_head(
                        spec,
                        merge_message,
                        source_oid,
                        Some(implementation_oid),
                        opts,
                        agent,
                    )
                    .await
                }
                MergeConflictResolution::MergeCommitted { .. } => Err(
                    "internal error: legacy merge conflict resolution triggered while squashing"
                        .into(),
                ),
            }
        }
    }
}

async fn finalize_squashed_merge_from_head(
    spec: &plan::PlanBranchSpec,
    merge_message: &str,
    source_oid: Oid,
    expected_implementation: Option<Oid>,
    opts: &MergeOptions,
    agent: &config::AgentSettings,
) -> Result<MergeExecutionResult, Box<dyn std::error::Error>> {
    let gate = run_cicd_gate_for_merge(spec, opts, agent).await?;
    let ready = merge_ready_from_head(source_oid)?;
    if let Some(expected) = expected_implementation {
        if expected != ready.head_oid {
            display::warn(format!(
                "HEAD moved after recording the implementation commit (expected {}, saw {}); finalizing merge from the current HEAD state.",
                short_hash(&expected.to_string()),
                short_hash(&ready.head_oid.to_string())
            ));
        }
    }
    let implementation_head = ready.head_oid;
    let merge_oid = commit_squashed_merge(merge_message, ready)?;
    Ok(MergeExecutionResult {
        merge_oid,
        source_oid,
        gate,
        squashed: true,
        implementation_oid: Some(implementation_head),
    })
}

async fn handle_squash_apply_conflict(
    spec: &plan::PlanBranchSpec,
    merge_message: &str,
    implementation_message: &str,
    plan: &vcs::SquashPlan,
    mainline: Option<u32>,
    conflict: vcs::CherryPickApplyConflict,
    setting: ConflictAutoResolveSetting,
    strategy: MergeConflictStrategy,
    agent: &config::AgentSettings,
) -> Result<MergeConflictResolution, Box<dyn std::error::Error>> {
    let files = conflict.files.clone();
    let replay_state = MergeReplayState {
        merge_base_oid: plan.merge_base.to_string(),
        start_oid: plan.target_head.to_string(),
        source_commits: plan
            .commits_to_apply
            .iter()
            .map(|oid| oid.to_string())
            .collect(),
        applied_commits: conflict.applied.iter().map(|oid| oid.to_string()).collect(),
        squash_mainline: mainline,
    };
    let state = MergeConflictState {
        slug: spec.slug.clone(),
        source_branch: spec.branch.clone(),
        target_branch: spec.target_branch.clone(),
        head_oid: plan.target_head.to_string(),
        source_oid: plan.source_tip.to_string(),
        merge_message: merge_message.to_string(),
        squash: true,
        implementation_message: Some(implementation_message.to_string()),
        replay: Some(replay_state),
        squash_mainline: mainline,
    };

    let state_path = write_conflict_state(&state)?;
    display::warn("Cherry-picking the plan commits onto the target branch produced conflicts.");
    emit_conflict_instructions(&spec.slug, &files, &state_path);

    match strategy {
        MergeConflictStrategy::Manual => {
            display::warn(setting.status_line());
            Err("merge blocked by conflicts; resolve them and rerun vizier merge with --complete-conflict".into())
        }
        MergeConflictStrategy::Agent => {
            display::warn(format!(
                "Auto-resolving merge conflicts via {}...",
                setting.source_description()
            ));
            match try_auto_resolve_conflicts(spec, &state, &files, agent).await {
                Ok(resolution) => Ok(resolution),
                Err(err) => {
                    display::warn(format!(
                        "Backend auto-resolution failed: {err}. Falling back to manual resolution."
                    ));
                    emit_conflict_instructions(&spec.slug, &files, &state_path);
                    Err("merge blocked by conflicts; resolve them and rerun vizier merge".into())
                }
            }
        }
    }
}

fn merge_ready_from_head(source_oid: Oid) -> Result<MergeReady, Box<dyn std::error::Error>> {
    let repo = Repository::discover(".")?;
    let head = repo.head()?;
    if !head.is_branch() {
        return Err(
            "cannot finalize merge while HEAD is detached; checkout the target branch first".into(),
        );
    }
    let head_commit = head.peel_to_commit()?;
    Ok(MergeReady {
        head_oid: head_commit.id(),
        source_oid,
        tree_oid: head_commit.tree_id(),
    })
}

async fn run_cicd_gate_for_merge(
    spec: &plan::PlanBranchSpec,
    opts: &MergeOptions,
    agent: &config::AgentSettings,
) -> Result<Option<CicdGateOutcome>, Box<dyn std::error::Error>> {
    let Some(script) = opts.cicd_gate.script.as_ref() else {
        return Ok(None);
    };

    let repo_root = repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let mut attempts: u32 = 0;
    let mut fix_attempts: u32 = 0;
    let mut fix_commits: Vec<CicdFixRecord> = Vec::new();

    loop {
        attempts += 1;
        let result = run_cicd_script(script, &repo_root)?;
        log_cicd_result(script, &result, attempts);

        if result.success() {
            return Ok(Some(CicdGateOutcome {
                script: script.clone(),
                attempts,
                fixes: fix_commits,
            }));
        }

        if !opts.cicd_gate.auto_resolve {
            return Err(cicd_gate_failure_error(script, &result));
        }

        if !agent.backend.requires_agent_runner() {
            display::warn(
                "CI/CD gate auto-remediation requires an agent-style backend; skipping automatic fixes.",
            );
            return Err(cicd_gate_failure_error(script, &result));
        }

        if fix_attempts >= opts.cicd_gate.retries {
            display::warn(format!(
                "CI/CD auto-remediation exhausted its retry budget ({} attempt(s)).",
                opts.cicd_gate.retries
            ));
            return Err(cicd_gate_failure_error(script, &result));
        }

        fix_attempts += 1;
        display::info(format!(
            "CI/CD gate failed; attempting backend remediation ({}/{})...",
            fix_attempts, opts.cicd_gate.retries
        ));
        let truncated_stdout = clip_log(result.stdout.as_bytes());
        let truncated_stderr = clip_log(result.stderr.as_bytes());
        if let Some(record) = attempt_cicd_auto_fix(
            spec,
            script,
            fix_attempts,
            opts.cicd_gate.retries,
            result.status.code(),
            &truncated_stdout,
            &truncated_stderr,
            agent,
            opts.squash,
        )
        .await?
        {
            match &record {
                CicdFixRecord::Commit(oid) => {
                    display::info(format!("Remediation attempt committed at {}.", oid));
                }
                CicdFixRecord::Amend(oid) => {
                    display::info(format!(
                        "Remediation attempt amended the implementation commit ({}).",
                        oid
                    ));
                }
            }
            fix_commits.push(record);
        } else {
            display::info("Backend remediation reported no file changes.");
        }
    }
}

async fn attempt_cicd_auto_fix(
    spec: &plan::PlanBranchSpec,
    script: &Path,
    attempt: u32,
    max_attempts: u32,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    agent: &config::AgentSettings,
    amend_head: bool,
) -> Result<Option<CicdFixRecord>, Box<dyn std::error::Error>> {
    let fix_agent = agent.for_prompt(config::PromptKind::Review)?;
    let prompt = agent_prompt::build_cicd_failure_prompt(
        &spec.slug,
        &spec.branch,
        &spec.target_branch,
        script,
        attempt,
        max_attempts,
        exit_code,
        stdout,
        stderr,
        &fix_agent.documentation,
    )
    .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let instruction = format!(
        "CI/CD gate script {} failed while merging plan {} (attempt {attempt}/{max_attempts}). Apply fixes so the script succeeds.",
        script.display(),
        spec.slug
    );

    let repo_root = repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let request_root = repo_root.clone();
    let (event_tx, event_rx) = mpsc::channel(64);
    let progress_handle = spawn_plain_progress_logger(event_rx);
    let (text_tx, _text_rx) = mpsc::channel(1);
    let response = Auditor::llm_request_with_tools_no_display(
        &fix_agent,
        Some(config::PromptKind::Review),
        prompt,
        instruction,
        auditor::RequestStream::Status {
            text: text_tx,
            events: Some(event_tx),
        },
        Some(request_root),
    )
    .await?;
    if let Some(handle) = progress_handle {
        let _ = handle.await;
    }

    #[cfg(feature = "mock_llm")]
    {
        mock_cicd_remediation(&repo_root)?;
    }

    let audit_result = Auditor::commit_audit().await?;
    let session_path = audit_result.session_display();
    let (narrative_paths, narrative_summary) = narrative_change_set(&audit_result);

    let diff = vcs::get_diff(".", Some("HEAD"), None)?;
    if diff.trim().is_empty() {
        return Ok(None);
    }

    let mut summary = response.content.trim().to_string();
    if summary.is_empty() {
        summary = format!(
            "Fix CI/CD gate failure for plan {} (attempt {attempt}/{max_attempts})",
            spec.slug
        );
    }
    let exit_label = exit_code
        .map(|code| code.to_string())
        .unwrap_or_else(|| "signal".to_string());
    stage_narrative_paths(&narrative_paths)?;
    vcs::stage(Some(vec!["."]))?;
    trim_staged_vizier_paths(&narrative_paths)?;
    let record = if amend_head {
        let commit_oid = amend_head_commit(None)?;
        CicdFixRecord::Amend(commit_oid.to_string())
    } else {
        let mut builder = CommitMessageBuilder::new(summary);
        builder
            .set_header(CommitMessageType::CodeChange)
            .with_session_log_path(session_path.clone())
            .with_narrative_summary(narrative_summary.clone())
            .with_author_note(format!(
                "CI/CD script: {} (exit={})",
                script.display(),
                exit_label
            ));
        let message = builder.build();
        let commit_oid = vcs::commit_staged(&message, false)?;
        CicdFixRecord::Commit(commit_oid.to_string())
    };
    clear_narrative_tracker(&narrative_paths);

    Ok(Some(record))
}

fn finalize_merge(
    spec: &plan::PlanBranchSpec,
    execution: MergeExecutionResult,
    delete_branch: bool,
    push_after: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let MergeExecutionResult {
        merge_oid,
        source_oid,
        gate,
        squashed,
        implementation_oid,
    } = execution;

    let repo = Repository::discover(".")?;
    if delete_branch {
        let mut should_delete = true;
        if squashed {
            let merge_commit = repo.find_commit(merge_oid)?;
            if merge_commit.parent_count() != 1 {
                display::warn(format!(
                    "Skipping deletion of {}; expected a single-parent merge but found {} parent(s).",
                    spec.branch,
                    merge_commit.parent_count()
                ));
                should_delete = false;
            } else if let Some(expected_impl) = implementation_oid {
                let parent = merge_commit.parent(0)?;
                if parent.id() != expected_impl {
                    display::warn(format!(
                        "Skipping deletion of {}; merge parent {} did not match the recorded implementation commit {}.",
                        spec.branch,
                        short_hash(&parent.id().to_string()),
                        short_hash(&expected_impl.to_string())
                    ));
                    should_delete = false;
                }
            }
        } else if !repo.graph_descendant_of(merge_oid, source_oid)? {
            display::warn(format!(
                "Skipping deletion of {}; merge commit did not include the branch tip.",
                spec.branch
            ));
            should_delete = false;
        }

        if should_delete {
            vcs::delete_branch(&spec.branch)?;
            display::info(format!("Deleted {} after merge", spec.branch));
        }
    } else {
        display::info(format!(
            "Keeping {} after merge (branch deletion disabled).",
            spec.branch
        ));
    }

    push_origin_if_requested(push_after)?;
    let mut rows = vec![
        ("Outcome".to_string(), "Merge complete".to_string()),
        ("Plan".to_string(), spec.slug.clone()),
        ("Target".to_string(), spec.target_branch.clone()),
        (
            "Merge commit".to_string(),
            short_hash(&merge_oid.to_string()),
        ),
    ];

    if let Some(summary) = gate.as_ref() {
        let script_label = repo_root()
            .ok()
            .and_then(|root| {
                summary
                    .script
                    .strip_prefix(&root)
                    .ok()
                    .map(|p| p.display().to_string())
            })
            .unwrap_or_else(|| summary.script.display().to_string());
        rows.push(("CI/CD script".to_string(), script_label));
        rows.push((
            "Gate attempts".to_string(),
            format_number(summary.attempts as usize),
        ));
        if !summary.fixes.is_empty() {
            let labels = summary
                .fixes
                .iter()
                .map(|record| record.describe())
                .collect::<Vec<_>>()
                .join(", ");
            if !labels.is_empty() {
                rows.push(("Gate fixes".to_string(), labels));
            }
        }
    }

    let verbosity = current_verbosity();
    append_agent_rows(&mut rows, verbosity);
    println!("{}", format_block(rows));
    Ok(())
}

async fn try_complete_pending_merge(
    spec: &plan::PlanBranchSpec,
    strategy: MergeConflictStrategy,
    setting: ConflictAutoResolveSetting,
    agent: &config::AgentSettings,
) -> Result<PendingMergeStatus, Box<dyn std::error::Error>> {
    let Some(state) = read_conflict_state(&spec.slug)? else {
        return Ok(PendingMergeStatus::None);
    };

    if state.source_branch != spec.branch {
        display::warn(format!(
            "Found stale merge-conflict metadata for {}; stored branch {}!=requested {}. Cleaning it up.",
            spec.slug, state.source_branch, spec.branch
        ));
        let _ = clear_conflict_state(&spec.slug);
        return Ok(PendingMergeStatus::None);
    }

    if state.target_branch != spec.target_branch {
        display::warn(format!(
            "Found stale merge-conflict metadata for {}; stored target {}!=requested {}. Cleaning it up.",
            spec.slug, state.target_branch, spec.target_branch
        ));
        let _ = clear_conflict_state(&spec.slug);
        return Ok(PendingMergeStatus::None);
    }

    let repo = Repository::discover(".")?;
    let current_branch = current_branch_name(&repo)?;
    if current_branch.as_deref() != Some(spec.target_branch.as_str()) {
        return Ok(PendingMergeStatus::Blocked(
            PendingMergeBlocker::WrongCheckout {
                expected_branch: spec.target_branch.clone(),
            },
        ));
    }

    if let Some(replay) = state.replay.as_ref() {
        let implementation_message = state
            .implementation_message
            .as_deref()
            .ok_or_else(|| "missing implementation commit message for squashed merge state")?;
        let start_oid = Oid::from_str(&replay.start_oid)?;
        let mut applied_commits: Vec<Oid> = replay
            .applied_commits
            .iter()
            .filter_map(|oid| Oid::from_str(oid).ok())
            .collect();
        let source_commits: Vec<Oid> = replay
            .source_commits
            .iter()
            .filter_map(|oid| Oid::from_str(oid).ok())
            .collect();

        let expected_head = applied_commits.last().copied().unwrap_or(start_oid);
        let head_commit = repo.head()?.peel_to_commit()?.id();
        if head_commit != expected_head {
            display::warn(format!(
                "Merge metadata for plan {} exists but HEAD moved; cleaning up the pending state.",
                spec.slug
            ));
            let _ = clear_conflict_state(&spec.slug);
            return Ok(PendingMergeStatus::Blocked(
                PendingMergeBlocker::NotInMerge {
                    target_branch: spec.target_branch.clone(),
                },
            ));
        }

        if let Err(err) = vcs::stage_all_in(repo.path()) {
            display::warn(format!(
                "Unable to stage merge replay changes via libgit2: {err}"
            ));
        }
        let mut conflict_paths = Vec::new();
        if let Ok(idx) = repo.index() {
            if let Ok(mut conflicts) = idx.conflicts() {
                while let Some(entry) = conflicts.next() {
                    if let Ok(conflict) = entry {
                        let path_bytes = conflict
                            .our
                            .as_ref()
                            .or(conflict.their.as_ref())
                            .or(conflict.ancestor.as_ref())
                            .map(|entry| entry.path.clone());
                        if let Some(bytes) = path_bytes {
                            conflict_paths.push(String::from_utf8_lossy(&bytes).to_string());
                        }
                    }
                }
            }
        }
        if !conflict_paths.is_empty() {
            let mut index = repo.index()?;
            for path in &conflict_paths {
                index.add_path(Path::new(path))?;
                index.conflict_remove(Path::new(path))?;
            }
            index.write()?;
        }
        let mut outstanding = Vec::new();
        {
            let mut idx = repo.index()?;
            let _ = idx.read(true);
            if idx.has_conflicts() {
                if let Ok(mut conflicts) = idx.conflicts() {
                    while let Some(entry) = conflicts.next() {
                        if let Ok(conflict) = entry {
                            let path_bytes = conflict
                                .our
                                .as_ref()
                                .or(conflict.their.as_ref())
                                .or(conflict.ancestor.as_ref())
                                .map(|entry| entry.path.clone());
                            if let Some(bytes) = path_bytes {
                                outstanding.push(String::from_utf8_lossy(&bytes).to_string());
                            }
                        }
                    }
                }
            }
        }
        if !outstanding.is_empty() {
            let mut index = repo.index()?;
            for path in &outstanding {
                index.add_path(Path::new(path))?;
                index.conflict_remove(Path::new(path))?;
            }
            index.write()?;
            outstanding = list_conflicted_paths()?;
            if !repo.index()?.has_conflicts() {
                outstanding.clear();
            }
        }
        if !outstanding.is_empty() {
            if let Some(status) = maybe_auto_resolve_pending_conflicts(
                spec,
                &state,
                &outstanding,
                strategy,
                setting,
                agent,
            )
            .await?
            {
                return Ok(status);
            }
        }
        display::info(format!(
            "Pending merge state: {:?}, index_conflicts={}, paths={outstanding:?}",
            repo.state(),
            repo.index().map(|idx| idx.has_conflicts()).unwrap_or(true)
        ));

        if repo.state() == RepositoryState::CherryPick {
            let current_index = applied_commits.len();
            let current_commit_oid = source_commits
                .get(current_index)
                .ok_or_else(|| "replay state missing the in-progress plan commit")?;
            let cherry_message = repo
                .find_commit(*current_commit_oid)?
                .summary()
                .unwrap_or("Apply plan commit")
                .to_string();
            match vcs::commit_in_progress_cherry_pick_in(
                repo.path(),
                &cherry_message,
                expected_head,
            ) {
                Ok(new_head) => {
                    applied_commits.push(new_head);
                }
                Err(err) => {
                    if !outstanding.is_empty() {
                        display::warn("Merge conflicts remain:");
                        for path in &outstanding {
                            display::warn(format!("  - {path}"));
                        }
                        display::info(format!(
                            "index reports conflicts: {}",
                            repo.index().map(|idx| idx.has_conflicts()).unwrap_or(true)
                        ));
                        let status = repo
                            .workdir()
                            .map(|wd| vcs::status_with_branch(wd).unwrap_or_default())
                            .unwrap_or_default();
                        if !status.is_empty() {
                            display::info(format!(
                                "Repository status before blocking merge:\n{status}"
                            ));
                        }
                        display::info(format!(
                            "Resolve the conflicts above, stage the files, then rerun `vizier merge {} --complete-conflict`.",
                            spec.slug
                        ));
                        return Ok(PendingMergeStatus::Blocked(
                            PendingMergeBlocker::Conflicts { files: outstanding },
                        ));
                    }

                    let fallback = (|| -> Result<Oid, git2::Error> {
                        let mut index = repo.index()?;
                        index.write()?;
                        let tree_oid = index.write_tree()?;
                        let tree = repo.find_tree(tree_oid)?;
                        let sig = repo.signature()?;
                        let parent_commit = repo.find_commit(expected_head)?;
                        let oid = repo.commit(
                            Some("HEAD"),
                            &sig,
                            &sig,
                            &cherry_message,
                            &tree,
                            &[&parent_commit],
                        )?;
                        repo.cleanup_state().ok();
                        let mut checkout = CheckoutBuilder::new();
                        checkout.force();
                        repo.checkout_head(Some(&mut checkout))?;
                        Ok(oid)
                    })();

                    match fallback {
                        Ok(oid) => applied_commits.push(oid),
                        Err(fallback_err) => {
                            display::info(format!(
                                "fallback cherry-pick commit failed: {fallback_err}"
                            ));
                            return Err(Box::new(err));
                        }
                    }
                }
            }
        } else if !outstanding.is_empty() {
            display::warn("Merge conflicts remain:");
            for path in &outstanding {
                display::warn(format!("  - {path}"));
            }
            display::info(format!(
                "index reports conflicts: {}",
                repo.index().map(|idx| idx.has_conflicts()).unwrap_or(true)
            ));
            let status = repo
                .workdir()
                .map(|wd| vcs::status_with_branch(wd).unwrap_or_default())
                .unwrap_or_default();
            if !status.is_empty() {
                display::info(format!(
                    "Repository status before blocking merge:\n{status}"
                ));
            }
            display::info(format!(
                "Resolve the conflicts above, stage the files, then rerun `vizier merge {} --complete-conflict`.",
                spec.slug
            ));
            return Ok(PendingMergeStatus::Blocked(
                PendingMergeBlocker::Conflicts { files: outstanding },
            ));
        }

        let remaining_commits = if source_commits.len() > applied_commits.len() {
            source_commits[applied_commits.len()..].to_vec()
        } else {
            Vec::new()
        };
        let replay_mainline = replay.squash_mainline.or(state.squash_mainline);

        match apply_cherry_pick_sequence(
            applied_commits.last().copied().unwrap_or(start_oid),
            &remaining_commits,
            Some(git2::FileFavor::Ours),
            replay_mainline,
        )? {
            CherryPickOutcome::Completed(result) => {
                applied_commits.extend(result.applied);
            }
            CherryPickOutcome::Conflicted(conflict) => {
                let replay_state = MergeReplayState {
                    merge_base_oid: replay.merge_base_oid.clone(),
                    start_oid: replay.start_oid.clone(),
                    source_commits: replay.source_commits.clone(),
                    applied_commits: applied_commits.iter().map(|oid| oid.to_string()).collect(),
                    squash_mainline: state.squash_mainline,
                };
                let next_state = MergeConflictState {
                    slug: state.slug.clone(),
                    source_branch: state.source_branch.clone(),
                    target_branch: state.target_branch.clone(),
                    head_oid: replay.start_oid.clone(),
                    source_oid: state.source_oid.clone(),
                    merge_message: state.merge_message.clone(),
                    squash: true,
                    implementation_message: Some(implementation_message.to_string()),
                    replay: Some(replay_state),
                    squash_mainline: state.squash_mainline,
                };
                if let Some(status) = maybe_auto_resolve_pending_conflicts(
                    spec,
                    &next_state,
                    &conflict.files,
                    strategy,
                    setting,
                    agent,
                )
                .await?
                {
                    return Ok(status);
                }
                let state_path = write_conflict_state(&next_state)?;
                emit_conflict_instructions(&spec.slug, &conflict.files, &state_path);
                return Ok(PendingMergeStatus::Blocked(
                    PendingMergeBlocker::Conflicts {
                        files: conflict.files.clone(),
                    },
                ));
            }
        }

        let expected_head = applied_commits.last().copied().unwrap_or(start_oid);
        let _ = commit_soft_squash(implementation_message, start_oid, expected_head)?;
        let _ = clear_conflict_state(&spec.slug);
        display::info("Conflicts resolved; implementation commit created for squashed merge.");
        let source_oid = Oid::from_str(&state.source_oid)?;
        return Ok(PendingMergeStatus::SquashReady {
            source_oid,
            merge_message: state.merge_message.clone(),
        });
    }

    if repo.state() != RepositoryState::Merge {
        display::warn(format!(
            "Merge metadata for plan {} exists but the repository is no longer in a merge state; assuming it was aborted.",
            spec.slug
        ));
        let _ = clear_conflict_state(&spec.slug);
        return Ok(PendingMergeStatus::Blocked(
            PendingMergeBlocker::NotInMerge {
                target_branch: spec.target_branch.clone(),
            },
        ));
    }

    let outstanding = list_conflicted_paths()?;
    if !outstanding.is_empty() {
        if let Some(status) = maybe_auto_resolve_pending_conflicts(
            spec,
            &state,
            &outstanding,
            strategy,
            setting,
            agent,
        )
        .await?
        {
            return Ok(status);
        }
        display::warn("Merge conflicts remain:");
        for path in &outstanding {
            display::warn(format!("  - {path}"));
        }
        display::info(format!(
            "Resolve the conflicts above, stage the files, then rerun `vizier merge {} --complete-conflict`.",
            spec.slug
        ));
        return Ok(PendingMergeStatus::Blocked(
            PendingMergeBlocker::Conflicts { files: outstanding },
        ));
    }

    let head_oid = Oid::from_str(&state.head_oid)?;
    let source_oid = Oid::from_str(&state.source_oid)?;
    if state.squash {
        let message = state
            .implementation_message
            .as_deref()
            .ok_or_else(|| "missing implementation commit message for squashed merge state")?;
        let _ = commit_in_progress_squash(message, head_oid)?;
        let _ = clear_conflict_state(&spec.slug);
        display::info("Conflicts resolved; implementation commit created for squashed merge.");
        return Ok(PendingMergeStatus::SquashReady {
            source_oid,
            merge_message: state.merge_message.clone(),
        });
    }

    let merge_oid = commit_in_progress_merge(&state.merge_message, head_oid, source_oid)?;
    let _ = clear_conflict_state(&spec.slug);
    display::info("Conflicts resolved; finalizing merge now.");
    Ok(PendingMergeStatus::Ready {
        merge_oid,
        source_oid,
    })
}

async fn maybe_auto_resolve_pending_conflicts(
    spec: &plan::PlanBranchSpec,
    state: &MergeConflictState,
    files: &[String],
    strategy: MergeConflictStrategy,
    setting: ConflictAutoResolveSetting,
    agent: &config::AgentSettings,
) -> Result<Option<PendingMergeStatus>, Box<dyn std::error::Error>> {
    if files.is_empty() {
        return Ok(None);
    }

    if !matches!(strategy, MergeConflictStrategy::Agent) {
        display::warn(setting.status_line());
        return Ok(None);
    }

    display::warn(format!(
        "Auto-resolving merge conflicts via {}...",
        setting.source_description()
    ));
    match try_auto_resolve_conflicts(spec, state, files, agent).await {
        Ok(MergeConflictResolution::MergeCommitted {
            merge_oid,
            source_oid,
        }) => Ok(Some(PendingMergeStatus::Ready {
            merge_oid,
            source_oid,
        })),
        Ok(MergeConflictResolution::SquashImplementationCommitted { source_oid, .. }) => {
            Ok(Some(PendingMergeStatus::SquashReady {
                source_oid,
                merge_message: state.merge_message.clone(),
            }))
        }
        Err(err) => {
            display::warn(format!(
                "Backend auto-resolution failed: {err}. Falling back to manual resolution."
            ));
            let state_path = merge_conflict_state_path(&state.slug)?;
            emit_conflict_instructions(&state.slug, files, &state_path);
            Err(err)
        }
    }
}

async fn handle_merge_conflict(
    spec: &plan::PlanBranchSpec,
    merge_message: &str,
    conflict: vcs::MergeConflict,
    setting: ConflictAutoResolveSetting,
    strategy: MergeConflictStrategy,
    squash: bool,
    implementation_message: Option<&str>,
    agent: &config::AgentSettings,
) -> Result<MergeConflictResolution, Box<dyn std::error::Error>> {
    let files = conflict.files.clone();
    let state = MergeConflictState {
        slug: spec.slug.clone(),
        source_branch: spec.branch.clone(),
        target_branch: spec.target_branch.clone(),
        head_oid: conflict.head_oid.to_string(),
        source_oid: conflict.source_oid.to_string(),
        merge_message: merge_message.to_string(),
        squash,
        implementation_message: implementation_message.map(|s| s.to_string()),
        replay: None,
        squash_mainline: None,
    };

    let state_path = write_conflict_state(&state)?;
    match strategy {
        MergeConflictStrategy::Manual => {
            display::warn(setting.status_line());
            emit_conflict_instructions(&spec.slug, &files, &state_path);
            Err("merge blocked by conflicts; resolve them and rerun vizier merge".into())
        }
        MergeConflictStrategy::Agent => {
            display::warn(format!(
                "Auto-resolving merge conflicts via {}...",
                setting.source_description()
            ));
            match try_auto_resolve_conflicts(spec, &state, &files, agent).await {
                Ok(result) => Ok(result),
                Err(err) => {
                    display::warn(format!(
                        "Backend auto-resolution failed: {err}. Falling back to manual resolution."
                    ));
                    emit_conflict_instructions(&spec.slug, &files, &state_path);
                    Err("merge blocked by conflicts; resolve them and rerun vizier merge".into())
                }
            }
        }
    }
}

fn emit_conflict_instructions(slug: &str, files: &[String], state_path: &Path) {
    if files.is_empty() {
        display::warn("Merge resulted in conflicts; run `git status` to inspect them.");
    } else {
        display::warn("Merge conflicts detected in:");
        for file in files {
            display::warn(format!("  - {file}"));
        }
    }

    display::info(format!(
        "Resolve the conflicts, stage the results, then rerun `vizier merge {slug} --complete-conflict` to finish the merge."
    ));
    display::info(format!(
        "Vizier stored merge metadata at {}; keep it until the merge completes.",
        state_path.display()
    ));
}

async fn try_auto_resolve_conflicts(
    spec: &plan::PlanBranchSpec,
    state: &MergeConflictState,
    files: &[String],
    agent: &config::AgentSettings,
) -> Result<MergeConflictResolution, Box<dyn std::error::Error>> {
    display::info("Attempting to resolve conflicts with the configured backend...");
    let prompt_agent = agent.for_prompt(config::PromptKind::MergeConflict)?;
    let selection = prompt_selection(&prompt_agent)?;
    let prompt = agent_prompt::build_merge_conflict_prompt(
        selection,
        &spec.target_branch,
        &spec.branch,
        files,
        &prompt_agent.documentation,
    )?;
    let repo_root = repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let request = build_agent_request(&prompt_agent, prompt, repo_root);

    let runner = Arc::clone(prompt_agent.agent_runner()?);
    let (progress_tx, progress_rx) = mpsc::channel(64);
    let progress_handle = spawn_plain_progress_logger(progress_rx);
    let result = runner
        .execute(request, Some(ProgressHook::Plain(progress_tx)))
        .await;
    if let Some(handle) = progress_handle {
        let _ = handle.await;
    }
    result.map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;

    #[cfg(feature = "mock_llm")]
    {
        mock_conflict_resolution(files)?;
    }

    if files.is_empty() {
        vcs::stage(None)?;
    } else {
        let paths: Vec<&str> = files.iter().map(|s| s.as_str()).collect();
        vcs::stage(Some(paths))?;
    }

    let remaining = list_conflicted_paths()?;
    if !remaining.is_empty() {
        return Err("conflicts remain after backend attempt".into());
    }

    let source_oid = Oid::from_str(&state.source_oid)?;
    if let Some(replay) = state.replay.as_ref() {
        let repo = Repository::discover(".")?;
        let implementation_message = state
            .implementation_message
            .as_deref()
            .ok_or_else(|| "missing implementation commit message for squashed merge state")?;
        let start_oid = Oid::from_str(&replay.start_oid)?;
        let mut applied_commits: Vec<Oid> = replay
            .applied_commits
            .iter()
            .filter_map(|oid| Oid::from_str(oid).ok())
            .collect();
        let source_commits: Vec<Oid> = replay
            .source_commits
            .iter()
            .filter_map(|oid| Oid::from_str(oid).ok())
            .collect();

        let current_index = applied_commits.len();
        let current_commit_oid = source_commits
            .get(current_index)
            .ok_or_else(|| "replay state missing the in-progress plan commit")?;
        let cherry_message = repo
            .find_commit(*current_commit_oid)?
            .summary()
            .unwrap_or("Apply plan commit")
            .to_string();
        let expected_parent = applied_commits.last().copied().unwrap_or(start_oid);
        let _ = commit_in_progress_cherry_pick(&cherry_message, expected_parent)?;

        let applied_head = Repository::discover(".")?.head()?.peel_to_commit()?.id();
        applied_commits.push(applied_head);

        let remaining_commits = if source_commits.len() > applied_commits.len() {
            source_commits[applied_commits.len()..].to_vec()
        } else {
            Vec::new()
        };
        let replay_mainline = replay.squash_mainline.or(state.squash_mainline);

        match apply_cherry_pick_sequence(
            applied_head,
            &remaining_commits,
            Some(git2::FileFavor::Ours),
            replay_mainline,
        )? {
            CherryPickOutcome::Completed(result) => {
                applied_commits.extend(result.applied);
            }
            CherryPickOutcome::Conflicted(next_conflict) => {
                let replay_state = MergeReplayState {
                    merge_base_oid: replay.merge_base_oid.clone(),
                    start_oid: replay.start_oid.clone(),
                    source_commits: replay.source_commits.clone(),
                    applied_commits: applied_commits.iter().map(|oid| oid.to_string()).collect(),
                    squash_mainline: state.squash_mainline,
                };
                let next_state = MergeConflictState {
                    slug: state.slug.clone(),
                    source_branch: state.source_branch.clone(),
                    target_branch: state.target_branch.clone(),
                    head_oid: replay.start_oid.clone(),
                    source_oid: state.source_oid.clone(),
                    merge_message: state.merge_message.clone(),
                    squash: true,
                    implementation_message: Some(implementation_message.to_string()),
                    replay: Some(replay_state),
                    squash_mainline: state.squash_mainline,
                };
                let state_path = write_conflict_state(&next_state)?;
                emit_conflict_instructions(&state.slug, &next_conflict.files, &state_path);
                return Err(
                    "merge blocked by conflicts; resolve them and rerun vizier merge".into(),
                );
            }
        }

        let expected_head = applied_commits.last().copied().unwrap_or(start_oid);
        let implementation_oid =
            commit_soft_squash(implementation_message, start_oid, expected_head)?;
        clear_conflict_state(&state.slug)?;
        display::info("Backend resolved the conflicts; implementation commit recorded.");
        return Ok(MergeConflictResolution::SquashImplementationCommitted {
            source_oid,
            implementation_oid,
        });
    }

    let head_oid = Oid::from_str(&state.head_oid)?;
    if state.squash {
        let message = state
            .implementation_message
            .as_deref()
            .ok_or_else(|| "missing implementation commit message for squashed merge state")?;
        let implementation_oid = commit_in_progress_squash(message, head_oid)?;
        clear_conflict_state(&state.slug)?;
        display::info("Backend resolved the conflicts; implementation commit recorded.");
        Ok(MergeConflictResolution::SquashImplementationCommitted {
            source_oid,
            implementation_oid,
        })
    } else {
        let merge_oid = commit_in_progress_merge(&state.merge_message, head_oid, source_oid)?;
        clear_conflict_state(&state.slug)?;
        display::info("Backend resolved the conflicts; finalizing merge.");
        Ok(MergeConflictResolution::MergeCommitted {
            merge_oid,
            source_oid,
        })
    }
}

#[cfg(feature = "mock_llm")]
fn mock_conflict_resolution(files: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    for rel in files {
        let path = Path::new(rel);
        if !path.exists() {
            continue;
        }

        let contents = std::fs::read_to_string(path)?;
        if let Some(resolved) = strip_conflict_markers(&contents) {
            std::fs::write(path, resolved)?;
        }
    }

    Ok(())
}

#[cfg(feature = "mock_llm")]
fn strip_conflict_markers(input: &str) -> Option<String> {
    if !input.contains("<<<<<<<") {
        return None;
    }

    let mut output = String::new();
    let mut remainder = input;

    while let Some(start) = remainder.find("<<<<<<<") {
        let (before, after_start) = remainder.split_at(start);
        output.push_str(before);

        let (_, after_marker) = after_start.split_once("<<<<<<<")?;
        let (_, after_left) = after_marker.split_once("=======")?;
        let (right, after_right) = after_left.split_once(">>>>>>>")?;
        let resolved = right.strip_prefix('\n').unwrap_or(right);
        output.push_str(resolved);

        if let Some(idx) = after_right.find('\n') {
            remainder = &after_right[idx + 1..];
        } else {
            remainder = "";
        }
    }

    output.push_str(remainder);
    Some(output)
}

#[cfg(feature = "mock_llm")]
fn mock_cicd_remediation(repo_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let instructions = repo_root.join(".vizier/tmp/mock_cicd_fix_path");
    if !instructions.exists() {
        return Ok(());
    }
    let rel = std::fs::read_to_string(&instructions)?;
    let trimmed = rel.trim();
    if trimmed.is_empty() {
        let _ = std::fs::remove_file(&instructions);
        return Ok(());
    }
    let target = repo_root.join(trimmed);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, "mock ci fix applied\n")?;
    let _ = std::fs::remove_file(&instructions);
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct MergeReplayState {
    merge_base_oid: String,
    start_oid: String,
    source_commits: Vec<String>,
    applied_commits: Vec<String>,
    #[serde(default)]
    squash_mainline: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct MergeConflictState {
    slug: String,
    source_branch: String,
    target_branch: String,
    head_oid: String,
    source_oid: String,
    merge_message: String,
    #[serde(default)]
    squash: bool,
    #[serde(default)]
    implementation_message: Option<String>,
    #[serde(default)]
    replay: Option<MergeReplayState>,
    #[serde(default)]
    squash_mainline: Option<u32>,
}

fn merge_conflict_state_path(slug: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let root = repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    Ok(root
        .join(".vizier/tmp/merge-conflicts")
        .join(format!("{slug}.json")))
}

fn write_conflict_state(state: &MergeConflictState) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = merge_conflict_state_path(&state.slug)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        let mut tmp = NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(tmp.as_file_mut(), state)?;
        tmp.as_file_mut().flush()?;
        tmp.persist(&path)?;
    } else {
        return Err("unable to determine merge-conflict metadata directory".into());
    }

    Ok(path)
}

fn read_conflict_state(
    slug: &str,
) -> Result<Option<MergeConflictState>, Box<dyn std::error::Error>> {
    let path = merge_conflict_state_path(slug)?;
    if !path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(&path)?;
    let state = serde_json::from_str::<MergeConflictState>(&contents)?;
    Ok(Some(state))
}

fn clear_conflict_state(slug: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = merge_conflict_state_path(slug)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

struct ReviewExecution {
    assume_yes: bool,
    review_only: bool,
    skip_checks: bool,
    cicd_gate: CicdGateOptions,
    auto_resolve_requested: bool,
}

struct ReviewOutcome {
    critique_label: &'static str,
    session_path: Option<String>,
    checks_passed: usize,
    checks_total: usize,
    diff_command: String,
    branch_mutated: bool,
    fix_commit: Option<String>,
    cicd_gate: ReviewGateResult,
}

struct PlanApplyResult {
    commit_oid: Option<String>,
}

struct ReviewCheckResult {
    command: String,
    status_code: Option<i32>,
    success: bool,
    duration: Duration,
    stdout: String,
    stderr: String,
}

impl ReviewCheckResult {
    fn duration_label(&self) -> String {
        format!("{:.2}s", self.duration.as_secs_f64())
    }

    fn status_label(&self) -> String {
        match self.status_code {
            Some(code) => format!("exit={code}"),
            None => "terminated".to_string(),
        }
    }

    fn to_context(&self) -> ReviewCheckContext {
        ReviewCheckContext {
            command: self.command.clone(),
            status_code: self.status_code,
            success: self.success,
            duration_ms: self.duration.as_millis(),
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct ReviewGateResult {
    status: ReviewGateStatus,
    script: Option<PathBuf>,
    attempts: u32,
    exit_code: Option<i32>,
    duration: Option<Duration>,
    stdout: String,
    stderr: String,
    auto_resolve_enabled: bool,
}

impl ReviewGateResult {
    fn skipped() -> Self {
        ReviewGateResult {
            status: ReviewGateStatus::Skipped,
            script: None,
            attempts: 0,
            exit_code: None,
            duration: None,
            stdout: String::new(),
            stderr: String::new(),
            auto_resolve_enabled: false,
        }
    }

    fn script_label(&self, repo_root: Option<&Path>) -> String {
        let Some(script) = self.script.as_ref() else {
            return "unset".to_string();
        };

        repo_root
            .and_then(|root| script.strip_prefix(root).ok())
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| script.display().to_string())
    }

    fn exit_code_label(&self) -> String {
        self.exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string())
    }

    fn duration_ms(&self) -> Option<u128> {
        self.duration.map(|value| value.as_millis())
    }

    fn summary_label(&self, repo_root: Option<&Path>) -> String {
        match self.status {
            ReviewGateStatus::Skipped => "not configured".to_string(),
            ReviewGateStatus::Passed => format!("passed ({})", self.script_label(repo_root)),
            ReviewGateStatus::Failed => format!(
                "failed {} ({})",
                self.exit_code_label(),
                self.script_label(repo_root)
            ),
        }
    }

    fn to_prompt_context(&self, repo_root: Option<&Path>) -> Option<ReviewGateContext> {
        self.script.as_ref()?;
        Some(ReviewGateContext {
            script: Some(self.script_label(repo_root)),
            status: self.status,
            attempts: self.attempts,
            duration_ms: self.duration_ms(),
            exit_code: self.exit_code,
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
            auto_resolve_enabled: self.auto_resolve_enabled,
        })
    }
}

async fn perform_review_workflow(
    spec: &plan::PlanBranchSpec,
    plan_meta: &plan::PlanMetadata,
    worktree_path: &Path,
    plan_path: &Path,
    exec: ReviewExecution,
    commit_mode: CommitMode,
    agent: &config::AgentSettings,
) -> Result<ReviewOutcome, Box<dyn std::error::Error>> {
    let _cwd = WorkdirGuard::enter(worktree_path)?;
    let repo_root = repo_root().ok();

    if exec.auto_resolve_requested {
        display::warn(
            "CI/CD auto-remediation is disabled during review; rerun merge for gate auto-fixes.",
        );
    }

    let gate_result = run_cicd_gate_for_review(&exec.cicd_gate)?;
    record_gate_operation("review", &gate_result);
    if matches!(gate_result.status, ReviewGateStatus::Failed) {
        display::warn(
            "CI/CD gate failed before the review critique; continuing with failure context.",
        );
    }

    let commands = resolve_review_commands(worktree_path, exec.skip_checks);
    if commands.is_empty() && !exec.skip_checks {
        display::info("Review checks: none configured for this repository.");
    }

    let check_results = run_review_checks(&commands, worktree_path);
    let checks_passed = check_results.iter().filter(|res| res.success).count();
    let checks_total = check_results.len();

    let diff_summary = collect_diff_summary(spec, worktree_path)?;
    let plan_document = fs::read_to_string(plan_path)?;
    let check_contexts: Vec<_> = check_results
        .iter()
        .map(ReviewCheckResult::to_context)
        .collect();

    let critique_agent = agent.for_prompt(config::PromptKind::Review)?;
    let selection = prompt_selection(&critique_agent)?;
    let gate_context = gate_result.to_prompt_context(repo_root.as_deref());
    let prompt = agent_prompt::build_review_prompt(
        selection,
        &spec.slug,
        &spec.branch,
        &spec.target_branch,
        &plan_document,
        &diff_summary,
        &check_contexts,
        gate_context.as_ref(),
        &critique_agent.documentation,
    )
    .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;

    let user_message = format!(
        "Review plan {} ({}) against {}",
        spec.slug, spec.branch, spec.target_branch
    );
    let (event_tx, event_rx) = mpsc::channel(64);
    let progress_handle = spawn_plain_progress_logger(event_rx);
    let (text_tx, _text_rx) = mpsc::channel(1);

    let response = Auditor::llm_request_with_tools_no_display(
        &critique_agent,
        Some(config::PromptKind::Review),
        prompt,
        user_message,
        auditor::RequestStream::Status {
            text: text_tx,
            events: Some(event_tx),
        },
        Some(worktree_path.to_path_buf()),
    )
    .await?;
    if let Some(handle) = progress_handle {
        let _ = handle.await;
    }

    let audit_result = Auditor::finalize(audit_disposition(commit_mode)).await?;
    let session_path = audit_result.session_display();
    let (narrative_paths, narrative_summary) = narrative_change_set(&audit_result);

    let critique_text = response.content.trim().to_string();
    emit_review_critique(&spec.slug, &critique_text);

    let review_diff = vcs::get_diff(".", Some("HEAD"), None)?;
    let mut branch_mutated = !review_diff.trim().is_empty();
    if commit_mode.should_commit() {
        if branch_mutated {
            let mut summary = format!(
                "Recorded backend critique for plan {} (checks {}/{} passed).",
                spec.slug, checks_passed, checks_total
            );
            summary.push_str(&format!("\nDiff command: {}", spec.diff_command()));
            summary.push_str(&format!(
                "\nCI/CD gate: {}",
                gate_result.summary_label(repo_root.as_deref())
            ));

            let mut builder = CommitMessageBuilder::new(summary);
            builder
                .set_header(CommitMessageType::NarrativeChange)
                .with_session_log_path(session_path.clone())
                .with_narrative_summary(narrative_summary.clone())
                .with_author_note(format!(
                    "Review critique streamed to terminal; session: {}",
                    session_path.as_deref().unwrap_or("<session unavailable>")
                ));
            let commit_message = builder.build();
            stage_narrative_paths(&narrative_paths)?;
            vcs::stage(Some(vec!["."]))?;
            trim_staged_vizier_paths(&narrative_paths)?;
            let _review_commit = vcs::commit_staged(&commit_message, false)?;
            clear_narrative_tracker(&narrative_paths);
        } else {
            display::info("No file modifications produced during review; skipping commit.");
        }
    } else {
        let session_hint = session_path
            .as_deref()
            .unwrap_or("<session unavailable>")
            .to_string();
        if branch_mutated {
            display::info(format!(
                "Review critique not committed (--no-commit); consult the terminal output or session log {} before committing manually.",
                session_hint
            ));
            if !narrative_paths.is_empty() {
                display::info(
                    "Review critique artifacts held for manual review (--no-commit active).",
                );
            }
        } else {
            display::info(format!(
                "Review critique streamed (--no-commit); no file modifications generated. Session log: {}",
                session_hint
            ));
        }
    }

    let mut fix_commit: Option<String> = None;
    let diff_command = spec.diff_command();

    if exec.review_only {
        display::info("Review-only mode: skipped automatic fix prompt.");
    } else {
        let mut apply_fixes = exec.assume_yes;
        if !exec.assume_yes {
            apply_fixes = prompt_for_confirmation(&format!(
                "Apply suggested fixes on {}? [y/N] ",
                spec.branch
            ))?;
        }

        if apply_fixes {
            match apply_review_fixes(
                spec,
                plan_meta,
                worktree_path,
                &critique_text,
                commit_mode,
                agent,
            )
            .await?
            {
                Some(commit) => {
                    fix_commit = Some(commit);
                    branch_mutated = true;
                }
                None => {
                    let post_fix_diff = vcs::get_diff(".", Some("HEAD"), None)?;
                    let post_fix_changed = !post_fix_diff.trim().is_empty();
                    branch_mutated = branch_mutated || post_fix_changed;
                    if !post_fix_changed {
                        display::info(
                            "Backend reported no changes while addressing review feedback.",
                        );
                    }
                }
            }
        } else {
            display::info("Skipped automatic fixes; branch left untouched.");
        }
    }

    Ok(ReviewOutcome {
        critique_label: "terminal",
        session_path,
        checks_passed,
        checks_total,
        diff_command,
        branch_mutated,
        fix_commit,
        cicd_gate: gate_result,
    })
}

fn resolve_review_commands(worktree_path: &Path, skip_checks: bool) -> Vec<String> {
    if skip_checks {
        return Vec::new();
    }

    let cfg = config::get_config();
    if !cfg.review.checks.commands.is_empty() {
        return cfg.review.checks.commands.clone();
    }

    if worktree_path.join("Cargo.toml").exists() {
        return vec![
            "cargo check --all --all-targets".to_string(),
            "cargo test --all --all-targets".to_string(),
        ];
    }

    Vec::new()
}

fn run_review_checks(commands: &[String], worktree_path: &Path) -> Vec<ReviewCheckResult> {
    let mut results = Vec::new();
    for command in commands {
        display::info(format!("Running review check: `{}`", command));
        let start = Instant::now();
        match Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(worktree_path)
            .output()
        {
            Ok(output) => {
                let result = ReviewCheckResult {
                    command: command.clone(),
                    status_code: output.status.code(),
                    success: output.status.success(),
                    duration: start.elapsed(),
                    stdout: clip_log(&output.stdout),
                    stderr: clip_log(&output.stderr),
                };
                log_check_result(&result);
                results.push(result);
            }
            Err(err) => {
                let result = ReviewCheckResult {
                    command: command.clone(),
                    status_code: None,
                    success: false,
                    duration: start.elapsed(),
                    stdout: String::new(),
                    stderr: format!("failed to run command: {err}"),
                };
                log_check_result(&result);
                results.push(result);
            }
        }
    }
    results
}

fn log_check_result(result: &ReviewCheckResult) {
    let status_label = if result.success { "passed" } else { "failed" };
    let message = format!(
        "Review check `{}` {status_label} ({}; {})",
        result.command,
        result.status_label(),
        result.duration_label()
    );
    if result.success {
        display::info(message);
    } else {
        display::warn(message);
        let trimmed = result.stderr.trim();
        if !trimmed.is_empty() {
            let snippet: String = trimmed
                .lines()
                .take(6)
                .map(|line| format!("    {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            display::warn(format!("  stderr:\n{}", snippet));
        }
    }
}

fn clip_log(bytes: &[u8]) -> String {
    const LIMIT: usize = 8_192;
    if bytes.is_empty() {
        return String::new();
    }
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= LIMIT {
        text.to_string()
    } else {
        let mut clipped = text[..LIMIT].to_string();
        clipped.push_str("\n… output truncated …");
        clipped
    }
}

fn log_cicd_stream(label: &str, content: &str) {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return;
    }
    let snippet: String = trimmed
        .lines()
        .take(12)
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    display::warn(format!("  {label}:\n{snippet}"));
}

fn run_cicd_script(
    script: &Path,
    repo_root: &Path,
) -> Result<CicdScriptResult, Box<dyn std::error::Error>> {
    let start = Instant::now();
    let output = Command::new("sh")
        .arg(script)
        .current_dir(repo_root)
        .output()
        .map_err(|err| format!("failed to run CI/CD script {}: {err}", script.display()))?;
    Ok(CicdScriptResult {
        status: output.status,
        duration: start.elapsed(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

fn log_cicd_result(script: &Path, result: &CicdScriptResult, attempt: u32) {
    let label = if result.success() { "passed" } else { "failed" };
    let status = result.status_label();
    let duration = format!("{:.2}s", result.duration.as_secs_f64());
    let message = format!(
        "CI/CD gate `{}` {label} ({status}; {duration}) [attempt {attempt}]",
        script.display()
    );
    if result.success() {
        display::info(message);
    } else {
        display::warn(message);
        log_cicd_stream("stdout", &result.stdout);
        log_cicd_stream("stderr", &result.stderr);
    }
}

fn run_cicd_gate_for_review(
    gate_opts: &CicdGateOptions,
) -> Result<ReviewGateResult, Box<dyn std::error::Error>> {
    let Some(script) = gate_opts.script.as_ref() else {
        display::info("CI/CD gate: not configured for review; skipping.");
        return Ok(ReviewGateResult::skipped());
    };

    if gate_opts.auto_resolve {
        display::warn(
            "CI/CD gate auto-remediation is disabled during review; reporting status without applying fixes.",
        );
    }

    let repo_root = repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let result = run_cicd_script(script, &repo_root)?;
    log_cicd_result(script, &result, 1);

    let status = if result.success() {
        ReviewGateStatus::Passed
    } else {
        ReviewGateStatus::Failed
    };

    Ok(ReviewGateResult {
        status,
        script: Some(script.clone()),
        attempts: 1,
        exit_code: result.status.code(),
        duration: Some(result.duration),
        stdout: clip_log(result.stdout.as_bytes()),
        stderr: clip_log(result.stderr.as_bytes()),
        auto_resolve_enabled: false,
    })
}

fn record_gate_operation(scope: &str, gate: &ReviewGateResult) {
    let repo_root = repo_root().ok();
    let script = gate.script.as_ref().map(|path| {
        repo_root
            .as_ref()
            .and_then(|root| path.strip_prefix(root).ok())
            .map(|relative| relative.display().to_string())
            .unwrap_or_else(|| path.display().to_string())
    });
    Auditor::record_operation(
        "cicd_gate",
        json!({
            "scope": scope,
            "script": script,
            "status": match gate.status {
                ReviewGateStatus::Passed => "passed",
                ReviewGateStatus::Failed => "failed",
                ReviewGateStatus::Skipped => "skipped",
            },
            "attempts": gate.attempts,
            "exit_code": gate.exit_code,
            "duration_ms": gate.duration_ms(),
            "stdout": gate.stdout,
            "stderr": gate.stderr,
            "auto_resolve_enabled": gate.auto_resolve_enabled,
        }),
    );
}

fn cicd_gate_failure_error(script: &Path, result: &CicdScriptResult) -> Box<dyn std::error::Error> {
    let status = result.status_label();
    let message = format!(
        "CI/CD gate `{}` failed ({status}); inspect the output above and rerun `vizier merge` once resolved.",
        script.display()
    );
    Box::<dyn std::error::Error>::from(message)
}

fn collect_diff_summary(
    spec: &plan::PlanBranchSpec,
    worktree_path: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    match vcs::diff_summary_against_target(worktree_path, &spec.target_branch) {
        Ok(summary) => Ok(format!(
            "Diff command: {}\n\n{}\n\n{}",
            spec.diff_command(),
            summary.stats.trim(),
            summary.name_status.trim()
        )),
        Err(err) => Ok(format!(
            "Diff command: {}\n\nUnable to compute diff via libgit2: {err}",
            spec.diff_command()
        )),
    }
}

fn emit_review_critique(plan_slug: &str, critique: &str) {
    println!("--- Review critique for plan {plan_slug} ---");
    if critique.trim().is_empty() {
        println!("(Agent returned an empty critique.)");
    } else {
        println!("{}", critique.trim());
    }
    println!("--- End review critique ---");
}

async fn apply_review_fixes(
    spec: &plan::PlanBranchSpec,
    plan_meta: &plan::PlanMetadata,
    worktree_path: &Path,
    critique_text: &str,
    commit_mode: CommitMode,
    agent: &config::AgentSettings,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let plan_rel = spec.plan_rel_path();
    let prompt_agent = agent.for_prompt(config::PromptKind::Documentation)?;
    let mut instruction = format!(
        "<instruction>Read the implementation plan at {} and the review critique below. Address every Action Item without changing unrelated code.</instruction>",
        plan_rel.display()
    );
    instruction.push_str(&format!(
        "<planSummary>{}</planSummary>",
        plan::summarize_spec(plan_meta)
    ));
    instruction.push_str("<reviewCritique>\n");
    if critique_text.trim().is_empty() {
        instruction
            .push_str("(Review critique was empty; explain whether any fixes are necessary.)\n");
    } else {
        instruction.push_str(critique_text.trim());
        instruction.push('\n');
    }
    instruction.push_str("</reviewCritique>");
    instruction.push_str(
        "<note>Update `.vizier/narrative/snapshot.md` and any relevant narrative docs when behavior changes.</note>",
    );

    let system_prompt = agent_prompt::build_documentation_prompt(
        prompt_agent.prompt_selection(),
        &instruction,
        &prompt_agent.documentation,
    )
    .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;

    let (event_tx, event_rx) = mpsc::channel(64);
    let progress_handle = spawn_plain_progress_logger(event_rx);
    let (text_tx, _text_rx) = mpsc::channel(1);
    let response = Auditor::llm_request_with_tools_no_display(
        &prompt_agent,
        Some(config::PromptKind::Documentation),
        system_prompt,
        instruction.clone(),
        auditor::RequestStream::Status {
            text: text_tx,
            events: Some(event_tx),
        },
        Some(worktree_path.to_path_buf()),
    )
    .await?;
    if let Some(handle) = progress_handle {
        let _ = handle.await;
    }

    let audit_result = Auditor::finalize(audit_disposition(commit_mode)).await?;
    let session_path = audit_result.session_display();
    let (narrative_paths, narrative_summary) = narrative_change_set(&audit_result);

    let diff = vcs::get_diff(".", Some("HEAD"), None)?;
    if diff.trim().is_empty() {
        display::info("Backend reported no file modifications during fix-up.");
        return Ok(None);
    }

    if commit_mode.should_commit() {
        stage_narrative_paths(&narrative_paths)?;
        vcs::stage(Some(vec!["."]))?;
        trim_staged_vizier_paths(&narrative_paths)?;
        let mut summary = response.content.trim().to_string();
        if summary.is_empty() {
            summary = format!(
                "Addressed review feedback for plan {} based on the latest critique",
                spec.slug
            );
        }
        let mut builder = CommitMessageBuilder::new(summary);
        builder
            .set_header(CommitMessageType::CodeChange)
            .with_session_log_path(session_path.clone())
            .with_narrative_summary(narrative_summary.clone())
            .with_author_note(format!(
                "Review critique streamed to terminal; session: {}",
                session_path.as_deref().unwrap_or("<session unavailable>")
            ));
        let commit_message = builder.build();
        let commit_oid = vcs::commit_staged(&commit_message, false)?;
        clear_narrative_tracker(&narrative_paths);
        Ok(Some(commit_oid.to_string()))
    } else {
        display::info("Fixes left pending; commit manually once satisfied.");
        if !narrative_paths.is_empty() {
            display::info("Narrative updates left pending (--no-commit active).");
        }
        Ok(None)
    }
}

fn prompt_for_confirmation(prompt: &str) -> Result<bool, Box<dyn std::error::Error>> {
    use std::io::{self, Write};

    print!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let normalized = input.trim().to_ascii_lowercase();
    Ok(matches!(normalized.as_str(), "y" | "yes"))
}

fn list_pending_plans(target_override: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let entries = plan::PlanSlugInventory::collect(target_override.as_deref())?;
    if entries.is_empty() {
        let mut rows = vec![(
            "Outcome".to_string(),
            "No pending draft branches".to_string(),
        )];
        if let Some(target) = target_override {
            rows.push(("Target".to_string(), target));
        }
        println!("{}", format_block(rows));
        return Ok(());
    }

    let mut header_rows = vec![(
        "Outcome".to_string(),
        format!(
            "{} pending draft {}",
            format_number(entries.len()),
            if entries.len() == 1 {
                "branch"
            } else {
                "branches"
            }
        ),
    )];
    if let Some(target) = target_override {
        header_rows.push(("Target".to_string(), target));
    }
    println!("{}", format_block(header_rows));
    println!();

    for (idx, entry) in entries.iter().enumerate() {
        let summary = entry.summary.replace('"', "'").replace('\n', " ");
        let rows = vec![
            ("Plan".to_string(), entry.slug.clone()),
            ("Branch".to_string(), entry.branch.clone()),
            ("Summary".to_string(), summary),
        ];
        println!("{}", format_block_with_indent(rows, 2));
        if idx + 1 < entries.len() {
            println!();
        }
    }

    Ok(())
}

async fn apply_plan_in_worktree(
    spec: &plan::PlanBranchSpec,
    plan_meta: &plan::PlanMetadata,
    worktree_path: &Path,
    _plan_path: &Path,
    push_after: bool,
    commit_mode: CommitMode,
    agent: &config::AgentSettings,
) -> Result<PlanApplyResult, Box<dyn std::error::Error>> {
    let _cwd = WorkdirGuard::enter(worktree_path)?;
    let prompt_agent = agent.for_prompt(config::PromptKind::Documentation)?;

    let plan_rel = spec.plan_rel_path();
    let mut instruction = format!(
        "<instruction>Read the implementation plan at {} and implement its Execution Plan on this branch. Apply the listed steps, update `.vizier/narrative/snapshot.md` plus any narrative docs as needed, and stage the resulting edits for commit.</instruction>",
        plan_rel.display()
    );
    instruction.push_str(&format!(
        "<planSummary>{}</planSummary>",
        plan::summarize_spec(plan_meta)
    ));

    let system_prompt = agent_prompt::build_documentation_prompt(
        prompt_agent.prompt_selection(),
        &instruction,
        &prompt_agent.documentation,
    )
    .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;

    let (event_tx, event_rx) = mpsc::channel(64);
    let progress_handle = spawn_plain_progress_logger(event_rx);
    let (text_tx, _text_rx) = mpsc::channel(1);
    let response = Auditor::llm_request_with_tools_no_display(
        &prompt_agent,
        None,
        system_prompt,
        instruction.clone(),
        auditor::RequestStream::Status {
            text: text_tx,
            events: Some(event_tx),
        },
        Some(worktree_path.to_path_buf()),
    )
    .await
    .map_err(|err| -> Box<dyn std::error::Error> {
        Box::from(format!("agent backend error: {err}"))
    })?;
    if let Some(handle) = progress_handle {
        let _ = handle.await;
    }

    let audit_result = Auditor::finalize(audit_disposition(commit_mode)).await?;
    let session_path = audit_result.session_display();
    let (narrative_paths, narrative_summary) = narrative_change_set(&audit_result);

    let diff = vcs::get_diff(".", Some("HEAD"), None)?;
    if diff.trim().is_empty() {
        return Err("Agent completed without modifying files; nothing new to approve.".into());
    }

    let mut summary = response.content.trim().to_string();
    if summary.is_empty() {
        summary = format!(
            "Plan {} implemented on {}.\nSpec summary: {}",
            spec.slug,
            spec.branch,
            plan::summarize_spec(plan_meta)
        );
    }

    let mut builder = CommitMessageBuilder::new(summary);
    builder
        .set_header(CommitMessageType::CodeChange)
        .with_session_log_path(session_path.clone())
        .with_narrative_summary(narrative_summary.clone());

    let mut commit_oid: Option<String> = None;

    if commit_mode.should_commit() {
        stage_narrative_paths(&narrative_paths)?;
        vcs::stage(Some(vec!["."]))?;
        trim_staged_vizier_paths(&narrative_paths)?;
        let commit_message = builder.build();
        let oid = vcs::commit_staged(&commit_message, false)?;
        clear_narrative_tracker(&narrative_paths);
        commit_oid = Some(oid.to_string());

        if push_after {
            push_origin_if_requested(true)?;
        }
    } else if push_after {
        display::info("Push skipped because --no-commit left changes pending.");
    } else if !narrative_paths.is_empty() {
        display::info(
            "Narrative assets updated with --no-commit; changes left pending in the plan worktree.",
        );
    }

    Ok(PlanApplyResult { commit_oid })
}

async fn refresh_plan_branch(
    spec: &plan::PlanBranchSpec,
    plan_meta: &plan::PlanMetadata,
    worktree_path: &Path,
    push_after: bool,
    agent: &config::AgentSettings,
) -> Result<(), Box<dyn std::error::Error>> {
    let _cwd = WorkdirGuard::enter(worktree_path)?;
    let prompt_agent = agent.for_prompt(config::PromptKind::Documentation)?;

    let instruction = build_save_instruction(None);
    let system_prompt = agent_prompt::build_documentation_prompt(
        prompt_agent.prompt_selection(),
        &instruction,
        &prompt_agent.documentation,
    )
    .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;

    let response = Auditor::llm_request_with_tools(
        &prompt_agent,
        Some(config::PromptKind::Documentation),
        system_prompt,
        instruction,
        Some(worktree_path.to_path_buf()),
    )
    .await?;
    let audit_result = Auditor::commit_audit().await?;
    let session_path = audit_result.session_display();
    let (narrative_paths, narrative_summary) = narrative_change_set(&audit_result);
    let mut allowed_paths = narrative_paths.clone();
    let plan_rel = spec.plan_rel_path();
    if !worktree_path.join(&plan_rel).exists() {
        allowed_paths.push(plan_rel.to_string_lossy().replace('\\', "/"));
    }

    let diff = vcs::get_diff(".", Some("HEAD"), None)?;
    if diff.trim().is_empty() {
        display::info(format!(
            "Plan {} already has up-to-date narrative assets; no refresh needed.",
            spec.slug
        ));
        if push_after {
            push_origin_if_requested(true)?;
        }
        return Ok(());
    }

    let mut summary = response.content.trim().to_string();
    if summary.is_empty() {
        summary = format!(
            "Refreshed narrative assets before merging plan {}.\nSpec summary: {}",
            spec.slug,
            plan::summarize_spec(plan_meta)
        );
    }

    let mut builder = CommitMessageBuilder::new(summary);
    builder
        .set_header(CommitMessageType::NarrativeChange)
        .with_session_log_path(session_path.clone())
        .with_narrative_summary(narrative_summary.clone());
    let commit_message = builder.build();

    stage_narrative_paths(&narrative_paths)?;
    vcs::stage(Some(vec!["."]))?;
    trim_staged_vizier_paths(&allowed_paths)?;
    let commit_oid = vcs::commit_staged(&commit_message, false)?;
    clear_narrative_tracker(&narrative_paths);

    if push_after {
        push_origin_if_requested(true)?;
    }

    display::info(format!(
        "Refreshed {} at {}; ready to merge plan {}",
        spec.branch, commit_oid, spec.slug
    ));

    Ok(())
}

fn current_branch_name(repo: &Repository) -> Result<Option<String>, git2::Error> {
    let head = repo.head()?;
    if head.is_branch() {
        Ok(head.shorthand().map(|s| s.to_string()))
    } else {
        Ok(None)
    }
}

fn build_implementation_commit_message(
    spec: &plan::PlanBranchSpec,
    meta: &plan::PlanMetadata,
) -> String {
    let mut sections = Vec::new();
    sections.push(format!("Target branch: {}", spec.target_branch));
    sections.push(format!("Plan branch: {}", spec.branch));
    sections.push(format!("Summary: {}", plan::summarize_spec(meta)));

    format!("feat: apply plan {}\n\n{}", spec.slug, sections.join("\n"))
}

fn build_merge_commit_message(
    spec: &plan::PlanBranchSpec,
    _meta: &plan::PlanMetadata,
    plan_document: Option<&str>,
    note: Option<&str>,
) -> String {
    // Merge commits now keep a concise subject line and embed the stored plan
    // document directly so reviewers see the same content the backend implemented.
    let mut sections: Vec<String> = Vec::new();

    if let Some(note_text) = note.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }) {
        sections.push(format!("Operator Note: {}", note_text));
    }

    let plan_block = plan_document
        .and_then(|document| {
            let trimmed = document.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .unwrap_or_else(|| format!("Implementation plan document unavailable for {}", spec.slug));

    sections.push(format!("Implementation Plan:\n{}", plan_block));

    format!(
        "feat: merge plan {}\n\n{}",
        spec.slug,
        sections.join("\n\n")
    )
}

struct WorkdirGuard {
    previous: PathBuf,
}

impl WorkdirGuard {
    fn enter(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let previous = std::env::current_dir()?;
        std::env::set_current_dir(path)?;
        Ok(Self { previous })
    }
}

impl Drop for WorkdirGuard {
    fn drop(&mut self) {
        if let Err(err) = std::env::set_current_dir(&self.previous) {
            display::debug(format!("failed to restore working directory: {err}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{build_agent_request, build_merge_commit_message};
    use crate::plan::{PlanBranchSpec, PlanMetadata};
    use git2::Repository;
    use std::fs;
    use std::path::PathBuf;
    use vizier_core::config::{self, CommandScope};
    use vizier_core::vcs;

    fn sample_spec() -> PlanBranchSpec {
        PlanBranchSpec {
            slug: "merge-headers".to_string(),
            branch: "draft/merge-headers".to_string(),
            target_branch: "main".to_string(),
        }
    }

    fn sample_meta(spec: &PlanBranchSpec) -> PlanMetadata {
        PlanMetadata {
            slug: spec.slug.clone(),
            branch: spec.branch.clone(),
            spec_excerpt: None,
            spec_summary: Some("Trim redundant headers".to_string()),
        }
    }

    #[test]
    fn merge_commit_message_embeds_plan_document() {
        let spec = sample_spec();
        let meta = sample_meta(&spec);
        let plan_doc = r#"---
plan: merge-headers
branch: draft/merge-headers

## Operator Spec

Tidy merge message bodies.
"#;

        let message = build_merge_commit_message(&spec, &meta, Some(plan_doc), None);

        assert!(
            message.starts_with("feat: merge plan merge-headers\n\nImplementation Plan:\n---"),
            "Implementation Plan block missing: {message}"
        );
        assert!(
            !message.contains("\nPlan: merge-headers"),
            "old Plan header should not appear: {message}"
        );
        assert!(
            !message.contains("\nBranch: draft/merge-headers"),
            "old Branch header should not appear: {message}"
        );
        assert!(
            !message.contains("Spec source: inline"),
            "old Spec source header should not appear: {message}"
        );
    }

    #[test]
    fn merge_commit_message_handles_notes_and_missing_document() {
        let spec = sample_spec();
        let meta = sample_meta(&spec);
        let message =
            build_merge_commit_message(&spec, &meta, None, Some("  needs manual review  "));

        assert!(
            message.contains("Operator Note: needs manual review"),
            "note should be trimmed and rendered: {message}"
        );
        assert!(
            message.contains("Implementation plan document unavailable for merge-headers"),
            "missing plan placeholder should be present: {message}"
        );
    }

    #[test]
    fn build_agent_request_uses_agent_settings() {
        let mut cfg = config::Config::default();
        cfg.agent_runtime.command = vec![
            "/opt/backend".to_string(),
            "exec".to_string(),
            "--mode".to_string(),
        ];
        cfg.agent_runtime.label = Some("merge-script".to_string());

        let agent = cfg
            .resolve_agent_settings(CommandScope::Merge, None)
            .expect("merge scope should resolve");

        let request = build_agent_request(
            &agent,
            "prompt body".to_string(),
            PathBuf::from("/repo/root"),
        );

        assert_eq!(
            request.command,
            vec![
                "/opt/backend".to_string(),
                "exec".to_string(),
                "--mode".to_string()
            ]
        );
        assert_eq!(
            request.scope,
            Some(config::CommandScope::Merge),
            "request should carry the originating scope"
        );
        assert_eq!(request.repo_root, PathBuf::from("/repo/root"));
        assert_eq!(
            request
                .metadata
                .get("agent_label")
                .map(|s| s.as_str())
                .unwrap_or(""),
            agent.agent_runtime.label
        );
        assert_eq!(
            request.metadata.get("agent_command").map(|s| s.as_str()),
            Some("/opt/backend exec --mode")
        );
        assert_eq!(
            request
                .metadata
                .get("agent_command_source")
                .map(|s| s.as_str()),
            Some("configured")
        );
    }

    #[test]
    fn collect_diff_summary_reports_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = Repository::init(dir.path()).expect("init repo");

        fs::write(dir.path().join("one.txt"), "one\n").unwrap();
        fs::write(dir.path().join("two.txt"), "two\n").unwrap();
        vcs::add_and_commit_in(dir.path(), Some(vec!["."]), "init", false).expect("init commit");

        repo.branch(
            "target",
            &repo.head().unwrap().peel_to_commit().unwrap(),
            true,
        )
        .expect("create target branch");

        fs::write(dir.path().join("one.txt"), "one\nchanged\n").unwrap();
        fs::remove_file(dir.path().join("two.txt")).unwrap();
        fs::write(dir.path().join("three.txt"), "three\n").unwrap();
        vcs::stage_all_in(dir.path()).expect("stage all changes");
        vcs::commit_staged_in(dir.path(), "topic", false).expect("topic commit");

        let spec = PlanBranchSpec {
            slug: "diff-test".to_string(),
            branch: "draft/diff-test".to_string(),
            target_branch: "target".to_string(),
        };

        let summary = super::collect_diff_summary(&spec, dir.path()).expect("summary");
        assert!(
            summary.contains("Diff command:"),
            "summary should include diff command:\n{summary}"
        );
        assert!(
            summary.contains("one.txt"),
            "modified file should be mentioned:\n{summary}"
        );
        assert!(
            summary.contains("three.txt"),
            "added file should be mentioned:\n{summary}"
        );
        assert!(
            summary.contains("two.txt"),
            "deleted file should be mentioned:\n{summary}"
        );
    }
}
