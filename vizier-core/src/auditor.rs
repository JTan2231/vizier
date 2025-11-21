use chrono::Utc;
use git2::Repository;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::{
    sync::mpsc::{Sender, channel},
    task::JoinHandle,
};
use wire::config::ThinkingLevel;

use crate::{
    agent::ProgressHook,
    codex,
    config::{self, PromptOrigin, SystemPrompt},
    display::{self, Verbosity},
    file_tracking, tools, vcs,
};

lazy_static! {
    static ref AUDITOR: Mutex<Auditor> = Mutex::new(Auditor::new());
}

#[derive(Clone, Debug)]
pub struct TokenUsage {
    pub input_tokens: usize,
    pub cached_input_tokens: usize,
    pub output_tokens: usize,
    pub reasoning_output_tokens: usize,
    pub total_tokens: usize,
    pub known: bool,
}

#[derive(Clone, Debug, Default)]
struct BackendUsageTotals {
    cached_input_total: usize,
    reasoning_output_total: usize,
    billed_total: usize,
    has_data: bool,
}

impl BackendUsageTotals {
    fn record(&mut self, usage: &TokenUsage) {
        self.cached_input_total = self
            .cached_input_total
            .saturating_add(usage.cached_input_tokens);
        self.reasoning_output_total = self
            .reasoning_output_total
            .saturating_add(usage.reasoning_output_tokens);
        self.billed_total = self.billed_total.saturating_add(
            usage
                .total_tokens
                .max(usage.input_tokens + usage.output_tokens),
        );
        self.has_data = true;
    }

    fn clear(&mut self) {
        *self = BackendUsageTotals::default();
    }
}

#[derive(Clone, Debug)]
pub struct TokenUsageReport {
    pub prompt_total: usize,
    pub completion_total: usize,
    pub prompt_delta: usize,
    pub completion_delta: usize,
    pub cached_input_total: Option<usize>,
    pub cached_input_delta: Option<usize>,
    pub reasoning_output_total: Option<usize>,
    pub reasoning_output_delta: Option<usize>,
    pub backend_total: Option<usize>,
    pub backend_delta: Option<usize>,
    pub known: bool,
    pub timestamp: String,
}

impl TokenUsageReport {
    pub fn total(&self) -> usize {
        self.backend_total
            .or_else(|| Some(self.prompt_total + self.completion_total))
            .unwrap_or(0)
    }

    pub fn delta_total(&self) -> usize {
        self.backend_delta
            .or_else(|| Some(self.prompt_delta + self.completion_delta))
            .unwrap_or(0)
    }

    fn to_progress_event(&self) -> display::ProgressEvent {
        let summary = if self.known {
            let mut parts = vec![
                format!("prompt={} (+{})", self.prompt_total, self.prompt_delta),
                format!(
                    "completion={} (+{})",
                    self.completion_total, self.completion_delta
                ),
                format!("total={} (+{})", self.total(), self.delta_total()),
            ];
            if let Some(cached_total) = self.cached_input_total {
                let delta = self.cached_input_delta.unwrap_or(0);
                parts.push(format!("cached_input={} (+{})", cached_total, delta));
            }
            if let Some(reasoning_total) = self.reasoning_output_total {
                let delta = self.reasoning_output_delta.unwrap_or(0);
                parts.push(format!("reasoning_output={} (+{})", reasoning_total, delta));
            }
            parts.join(" ")
        } else {
            "unknown usage (backend omitted token counts)".to_string()
        };

        display::ProgressEvent {
            kind: display::ProgressKind::TokenUsage,
            source: None,
            phase: None,
            label: Some("token-usage".to_string()),
            message: Some(summary),
            detail: None,
            path: None,
            progress: None,
            status: None,
            timestamp: Some(self.timestamp.clone()),
            raw: None,
        }
    }
}

fn emit_token_usage_line(report: &TokenUsageReport) {
    let cfg = display::get_display_config();
    if matches!(cfg.verbosity, Verbosity::Quiet) {
        return;
    }

    for line in display::render_progress_event(&report.to_progress_event(), cfg.verbosity) {
        eprintln!("{}", line);
    }
}

async fn forward_usage_report(report: &TokenUsageReport, stream: &RequestStream) {
    match stream {
        RequestStream::Status {
            events: Some(events),
            ..
        } => {
            let _ = events.send(report.to_progress_event()).await;
        }
        _ => emit_token_usage_line(report),
    }
}

#[derive(Clone)]
pub enum RequestStream {
    Status {
        text: Sender<String>,
        events: Option<Sender<display::ProgressEvent>>,
    },
    PassthroughStderr,
}

pub struct AuditorCleanup {
    pub debug: bool,
    pub print_json: bool,
}

#[derive(Clone, Debug)]
pub struct AgentInvocationContext {
    pub backend: config::BackendKind,
    pub scope: config::CommandScope,
    pub model: String,
    pub reasoning_effort: Option<ThinkingLevel>,
    pub prompt_kind: SystemPrompt,
}

impl Drop for AuditorCleanup {
    fn drop(&mut self) {
        if let Some(artifact) = Auditor::persist_session_log() {
            display::info(format!("Session saved to {}", artifact.display_path()));

            if self.print_json {
                if let Ok(contents) = fs::read_to_string(&artifact.path) {
                    println!("{}", contents);
                }
            }
        }
    }
}

pub fn find_project_root() -> std::io::Result<Option<PathBuf>> {
    let mut current_dir = std::env::current_dir()?;

    loop {
        let dot_git = current_dir.join(".git");
        if dot_git.is_dir() {
            return Ok(Some(current_dir));
        }

        if dot_git.is_file() {
            // Worktrees expose a .git file pointing at the real gitdir.
            if fs::read_to_string(&dot_git)
                .map(|contents| contents.trim_start().starts_with("gitdir:"))
                .unwrap_or(true)
            {
                return Ok(Some(current_dir));
            }
        }

        if let Some(parent) = current_dir.parent() {
            current_dir = parent.to_path_buf();
        } else {
            return Ok(None);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitDisposition {
    Auto,
    Hold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditState {
    Clean,
    Committed,
    Pending,
}

#[derive(Clone, Debug)]
pub struct NarrativeChangeSet {
    pub paths: Vec<String>,
    pub summary: Option<String>,
}

impl NarrativeChangeSet {
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct AuditResult {
    pub session_artifact: Option<SessionArtifact>,
    pub state: AuditState,
    pub narrative_changes: Option<NarrativeChangeSet>,
}

impl AuditResult {
    pub fn session_display(&self) -> Option<String> {
        self.session_artifact
            .as_ref()
            .map(|artifact| artifact.display_path())
    }

    pub fn narrative_changes(&self) -> Option<&NarrativeChangeSet> {
        self.narrative_changes.as_ref()
    }

    pub fn committed(&self) -> bool {
        matches!(self.state, AuditState::Committed)
    }

    pub fn pending(&self) -> bool {
        matches!(self.state, AuditState::Pending)
    }
}

async fn prompt_wire_with_tools(
    client: &dyn wire::api::Prompt,
    tx: tokio::sync::mpsc::Sender<String>,
    system_prompt: &str,
    messages: Vec<wire::types::Message>,
    tools: Vec<wire::types::Tool>,
) -> Result<Vec<wire::types::Message>, Box<dyn std::error::Error>> {
    // TODO: Mock server??? why did we even implement it if not to use it
    #[cfg(feature = "mock_llm")]
    {
        let _ = (&tx, system_prompt, &tools);
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
        let mut new_messages = messages.clone();
        new_messages.push(
            client
                .new_message("mock response".to_string())
                .as_assistant()
                .build(),
        );

        Ok(new_messages)
    }

    #[cfg(not(feature = "mock_llm"))]
    {
        client
            .prompt_with_tools_with_status(tx, &system_prompt, messages, tools)
            .await
    }
}

fn channel_for_stream(
    stream: &RequestStream,
) -> (tokio::sync::mpsc::Sender<String>, Option<JoinHandle<()>>) {
    match stream {
        RequestStream::Status { text, .. } => (text.clone(), None),
        RequestStream::PassthroughStderr => {
            let (silent_tx, mut silent_rx) = channel(32);
            let handle = tokio::spawn(async move { while silent_rx.recv().await.is_some() {} });
            (silent_tx, Some(handle))
        }
    }
}

/// _All_ LLM interactions need run through the auditor
/// This should hold every LLM interaction from the current session, in chronological order
#[derive(Debug, Serialize, Deserialize)]
pub struct Auditor {
    messages: Vec<wire::types::Message>,
    session_start: String,
    session_id: String,
    #[serde(skip)]
    usage_unknown: bool,
    #[serde(skip)]
    last_session_artifact: Option<SessionArtifact>,
    #[serde(skip)]
    last_usage_report: Option<TokenUsageReport>,
    #[serde(skip)]
    last_agent: Option<AgentInvocationContext>,
    #[serde(skip)]
    backend_usage_totals: BackendUsageTotals,
}

impl Auditor {
    pub fn new() -> Self {
        let now = Utc::now();

        Auditor {
            messages: Vec::new(),
            session_start: now.to_string(),
            session_id: uuid::Uuid::new_v4().to_string(),
            usage_unknown: false,
            last_session_artifact: None,
            last_usage_report: None,
            last_agent: None,
            backend_usage_totals: BackendUsageTotals::default(),
        }
    }

    /// Clones the message history in the auditor
    pub fn get_messages() -> Vec<wire::types::Message> {
        AUDITOR.lock().unwrap().messages.clone()
    }

    pub fn add_message(message: wire::types::Message) -> Option<TokenUsageReport> {
        let mut auditor = AUDITOR.lock().unwrap();
        let should_emit = message.message_type == wire::types::MessageType::Assistant;
        auditor.messages.push(message);
        if should_emit {
            return Auditor::capture_usage_report_locked(&mut auditor);
        }

        None
    }

    pub fn replace_messages(messages: &Vec<wire::types::Message>) -> Option<TokenUsageReport> {
        let mut auditor = AUDITOR.lock().unwrap();
        auditor.messages = messages.clone();
        Auditor::capture_usage_report_locked(&mut auditor)
    }

    fn record_agent(settings: &config::AgentSettings, prompt_kind: Option<SystemPrompt>) {
        if let Ok(mut auditor) = AUDITOR.lock() {
            auditor.last_agent = Some(AgentInvocationContext {
                backend: settings.backend,
                scope: settings.scope,
                model: settings.provider_model.clone(),
                reasoning_effort: settings.reasoning_effort,
                prompt_kind: prompt_kind.unwrap_or(SystemPrompt::Base),
            });
        }
    }

    fn record_backend_usage(usage: &TokenUsage) {
        if let Ok(mut auditor) = AUDITOR.lock() {
            auditor.backend_usage_totals.record(usage);
        }
    }

    pub fn latest_agent_context() -> Option<AgentInvocationContext> {
        AUDITOR
            .lock()
            .ok()
            .and_then(|auditor| auditor.last_agent.clone())
    }

    fn capture_usage_report_locked(auditor: &mut Auditor) -> Option<TokenUsageReport> {
        let mut totals = TokenUsage {
            input_tokens: 0,
            cached_input_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 0,
            known: !auditor.usage_unknown,
        };

        for message in auditor.messages.iter() {
            totals.input_tokens += message.input_tokens;
            totals.output_tokens += message.output_tokens;
        }
        totals.total_tokens = totals.input_tokens + totals.output_tokens;

        let (prev_prompt, prev_completion, prev_known, prev_cached, prev_reasoning, prev_billed) =
            auditor
                .last_usage_report
                .as_ref()
                .map(|report| {
                    (
                        report.prompt_total,
                        report.completion_total,
                        report.known,
                        report.cached_input_total.unwrap_or(0),
                        report.reasoning_output_total.unwrap_or(0),
                        report.total(),
                    )
                })
                .unwrap_or((0, 0, true, 0, 0, 0));

        if totals.input_tokens == prev_prompt
            && totals.output_tokens == prev_completion
            && totals.known == prev_known
        {
            return None;
        }

        let backend_totals = auditor.backend_usage_totals.clone();
        let cached_total = backend_totals
            .has_data
            .then_some(backend_totals.cached_input_total);
        let cached_delta = cached_total.map(|value| value.saturating_sub(prev_cached));
        let reasoning_total = backend_totals
            .has_data
            .then_some(backend_totals.reasoning_output_total);
        let reasoning_delta = reasoning_total.map(|value| value.saturating_sub(prev_reasoning));
        let billed_total = backend_totals
            .has_data
            .then_some(backend_totals.billed_total.max(totals.total_tokens));
        let billed_delta = billed_total.map(|value| value.saturating_sub(prev_billed));

        let report = TokenUsageReport {
            prompt_total: totals.input_tokens,
            completion_total: totals.output_tokens,
            prompt_delta: totals.input_tokens.saturating_sub(prev_prompt),
            completion_delta: totals.output_tokens.saturating_sub(prev_completion),
            cached_input_total: cached_total,
            cached_input_delta: cached_delta,
            reasoning_output_total: reasoning_total,
            reasoning_output_delta: reasoning_delta,
            backend_total: billed_total,
            backend_delta: billed_delta,
            known: totals.known,
            timestamp: Utc::now().to_rfc3339(),
        };

        auditor.last_usage_report = Some(report.clone());

        Some(report)
    }

    pub fn conversation_to_string() -> String {
        let messages = AUDITOR.lock().unwrap().messages.clone();
        let mut conversation = String::new();

        for message in messages
            .iter()
            .filter(|m| m.message_type != wire::types::MessageType::FunctionCall)
        {
            conversation.push_str(&format!(
                "{}: {}\n\n###\n\n",
                message.message_type.to_string(),
                message.content
            ));
        }

        conversation
    }

    pub fn clear_messages() {
        if let Ok(mut auditor) = AUDITOR.lock() {
            auditor.messages.clear();
            auditor.last_usage_report = None;
            auditor.backend_usage_totals.clear();
            auditor.usage_unknown = false;
        }
    }

    pub fn get_total_usage() -> TokenUsage {
        let auditor = AUDITOR.lock().unwrap();
        let mut usage = TokenUsage {
            input_tokens: 0,
            cached_input_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 0,
            known: !auditor.usage_unknown,
        };

        for message in auditor.messages.iter() {
            usage.input_tokens += message.input_tokens;
            usage.output_tokens += message.output_tokens;
        }

        usage.total_tokens = usage.input_tokens + usage.output_tokens;
        if auditor.backend_usage_totals.has_data {
            usage.cached_input_tokens = auditor.backend_usage_totals.cached_input_total;
            usage.reasoning_output_tokens = auditor.backend_usage_totals.reasoning_output_total;
            usage.total_tokens = auditor
                .backend_usage_totals
                .billed_total
                .max(usage.total_tokens);
        }

        usage
    }

    pub fn latest_usage_report() -> Option<TokenUsageReport> {
        AUDITOR
            .lock()
            .ok()
            .and_then(|auditor| auditor.last_usage_report.clone())
    }

    fn mark_usage_unknown() {
        if let Ok(mut auditor) = AUDITOR.lock() {
            auditor.usage_unknown = true;
        }
    }

    /// Persist the session log and return any pending narrative changes.
    pub async fn commit_audit() -> Result<AuditResult, Box<dyn std::error::Error>> {
        Self::finalize(CommitDisposition::Auto).await
    }

    pub async fn finalize(
        disposition: CommitDisposition,
    ) -> Result<AuditResult, Box<dyn std::error::Error>> {
        let session_artifact = Self::persist_session_log();
        let project_root = match find_project_root()? {
            Some(p) => p,
            None => std::path::PathBuf::from("."),
        };

        if let Err(err) = file_tracking::FileTracker::sync_vizier_changes(&project_root) {
            display::debug(format!(
                "Unable to auto-detect .vizier changes; continuing without sync ({})",
                err
            ));
        }

        let pending_paths = match file_tracking::FileTracker::pending_paths(&project_root) {
            Ok(paths) => paths,
            Err(err) => {
                display::debug(format!(
                    "Unable to enumerate pending .vizier changes; treating as clean ({})",
                    err
                ));
                Vec::new()
            }
        };

        if pending_paths.is_empty() {
            return Ok(AuditResult {
                session_artifact,
                state: AuditState::Clean,
                narrative_changes: None,
            });
        }

        let root = project_root.to_str().unwrap();

        let mut diff_message = None;
        if let Ok(diff) = vcs::get_diff(root, Some(&tools::get_todo_dir()), None) {
            display::info("Writing commit message for TODO changes...");
            diff_message = Some(
                Self::llm_request(
                    "Given a diff on a directory of TODO items, return a commit message for these changes."
                        .to_string(),
                    if diff.len() == 0 { "init".to_string() } else { diff },
                )
                .await?
                .content,
            );
        }

        if matches!(disposition, CommitDisposition::Hold) {
            return Ok(AuditResult {
                session_artifact,
                state: AuditState::Pending,
                narrative_changes: Some(NarrativeChangeSet {
                    paths: pending_paths,
                    summary: diff_message,
                }),
            });
        }

        Ok(AuditResult {
            session_artifact,
            state: AuditState::Committed,
            narrative_changes: Some(NarrativeChangeSet {
                paths: pending_paths,
                summary: diff_message,
            }),
        })
    }

    pub fn load_session_messages_from_path(
        path: &Path,
    ) -> Result<Vec<wire::types::Message>, Box<dyn std::error::Error>> {
        let contents = fs::read_to_string(path)?;
        Ok(Self::parse_session_messages(&contents)?)
    }

    fn parse_session_messages(
        contents: &str,
    ) -> Result<Vec<wire::types::Message>, serde_json::Error> {
        if let Ok(wrapper) = serde_json::from_str::<SessionLogWrapper>(contents) {
            return Ok(wrapper.messages);
        }

        serde_json::from_str(contents)
    }

    pub fn persist_session_log() -> Option<SessionArtifact> {
        if config::get_config().no_session {
            return None;
        }

        let project_root = match find_project_root().ok().flatten() {
            Some(root) => root,
            None => return None,
        };

        let log = {
            let auditor = AUDITOR.lock().ok()?;
            auditor.build_session_log(&project_root)?
        };

        match Self::write_session_file(&project_root, &log) {
            Ok(artifact) => {
                if let Ok(mut auditor) = AUDITOR.lock() {
                    auditor.last_session_artifact = Some(artifact.clone());
                }
                Some(artifact)
            }
            Err(err) => {
                display::warn(format!(
                    "Failed to write session log for {}: {}",
                    log.id, err
                ));
                None
            }
        }
    }

    fn build_session_log(&self, project_root: &Path) -> Option<SessionLog> {
        if self.messages.is_empty() {
            return None;
        }

        let cfg = config::get_config();
        Some(SessionLog {
            schema: "vizier.session.v1".to_string(),
            id: self.session_id.clone(),
            created_at: self.session_start.clone(),
            updated_at: Utc::now().to_rfc3339(),
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            workflow_type: Self::workflow_label(),
            mode: Self::mode_label(),
            repo: Self::repo_snapshot(project_root),
            config_effective: Self::config_snapshot(&cfg),
            system_prompt: Self::prompt_info(project_root, &cfg, self.last_agent.as_ref()),
            model: Self::model_snapshot(self.last_agent.as_ref(), &cfg),
            messages: self.messages.clone(),
            operations: Vec::new(),
            artifacts: Vec::new(),
            outcome: SessionOutcome {
                status: "completed".to_string(),
                summary: Self::summarize_assistant(&self.messages),
                token_usage: self.last_usage_report.as_ref().map(SessionTokenUsage::from),
            },
        })
    }

    fn model_snapshot(
        agent: Option<&AgentInvocationContext>,
        cfg: &config::Config,
    ) -> SessionModelInfo {
        match agent {
            Some(ctx) => SessionModelInfo {
                provider: ctx.backend.to_string(),
                name: ctx.model.clone(),
                reasoning_effort: ctx.reasoning_effort.map(|level| format!("{level:?}")),
                scope: Some(ctx.scope.as_str().to_string()),
            },
            None => SessionModelInfo {
                provider: cfg.backend.to_string(),
                name: cfg.provider_model.clone(),
                reasoning_effort: cfg
                    .reasoning_effort
                    .as_ref()
                    .map(|level| format!("{level:?}")),
                scope: None,
            },
        }
    }

    fn workflow_label() -> String {
        env::args()
            .nth(1)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "cli".to_string())
    }

    fn mode_label() -> String {
        env::var("VIZIER_MODE")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "default".to_string())
    }

    fn repo_snapshot(project_root: &Path) -> SessionRepoState {
        if let Ok(repo) = Repository::open(project_root) {
            let (branch, head) = match repo.head() {
                Ok(reference) if reference.is_branch() => {
                    let b = reference.shorthand().map(|s| s.to_string());
                    let h = reference
                        .peel_to_commit()
                        .ok()
                        .map(|commit| commit.id().to_string());
                    (b, h)
                }
                Ok(reference) => (
                    None,
                    reference
                        .peel_to_commit()
                        .ok()
                        .map(|commit| commit.id().to_string()),
                ),
                Err(_) => (None, None),
            };

            SessionRepoState {
                root: project_root.display().to_string(),
                branch,
                head,
            }
        } else {
            SessionRepoState {
                root: project_root.display().to_string(),
                branch: None,
                head: None,
            }
        }
    }

    fn config_snapshot(cfg: &config::Config) -> serde_json::Value {
        json!({
            "backend": cfg.backend.to_string(),
            "reasoning_effort": cfg
                .reasoning_effort
                .as_ref()
                .map(|level| format!("{level:?}")),
            "codex": {
                "binary": cfg.process.binary_path.display().to_string(),
                "profile": cfg.process.profile.clone(),
                "bounds_prompt": cfg
                    .process
                    .bounds_prompt_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
            },
        })
    }

    fn prompt_info(
        project_root: &Path,
        cfg: &config::Config,
        context: Option<&AgentInvocationContext>,
    ) -> SessionPromptInfo {
        let scope = context
            .map(|ctx| ctx.scope)
            .unwrap_or(config::CommandScope::Ask);
        let kind = context
            .map(|ctx| ctx.prompt_kind)
            .unwrap_or(SystemPrompt::Base);
        let selection = cfg.prompt_for(scope, kind);
        let origin = selection.origin.clone();
        let digest = Sha256::digest(selection.text.as_bytes());
        let hash = format!("{:x}", digest);

        let path = match &origin {
            PromptOrigin::RepoFile { path } => {
                let relative = path.strip_prefix(project_root).unwrap_or(path.as_path());
                Some(relative.to_string_lossy().to_string())
            }
            _ => None,
        };

        SessionPromptInfo {
            kind: kind.as_str().to_string(),
            scope: scope.as_str().to_string(),
            origin: origin.label().to_string(),
            path,
            hash,
        }
    }

    fn summarize_assistant(messages: &[wire::types::Message]) -> Option<String> {
        for message in messages.iter().rev() {
            if message.message_type == wire::types::MessageType::Assistant {
                return Some(message.content.clone());
            }
        }

        None
    }

    fn write_session_file(
        project_root: &Path,
        log: &SessionLog,
    ) -> Result<SessionArtifact, std::io::Error> {
        let sessions_dir = project_root.join(".vizier").join("sessions").join(&log.id);
        fs::create_dir_all(&sessions_dir)?;

        let session_path = sessions_dir.join("session.json");
        let tmp_path = sessions_dir.join("session.json.tmp");
        let buffer = serde_json::to_vec_pretty(log)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
        let mut tmp = File::create(&tmp_path)?;
        tmp.write_all(&buffer)?;
        tmp.sync_all()?;
        fs::rename(&tmp_path, &session_path)?;

        Ok(SessionArtifact::new(&log.id, session_path, project_root))
    }

    /// Basic LLM request without tool usage
    pub async fn llm_request(
        system_prompt: String,
        user_message: String,
    ) -> Result<wire::types::Message, Box<dyn std::error::Error>> {
        let _ = Self::add_message(
            crate::config::get_config()
                .provider
                .new_message(user_message)
                .as_user()
                .build(),
        );

        let messages = AUDITOR.lock().unwrap().messages.clone();

        let response = display::call_with_status(async move |tx| {
            tx.send(display::Status::Working("Thinking...".into()))
                .await?;

            let (request_tx, mut request_rx) = channel(10);
            let status_tx = tx.clone();

            tokio::spawn(async move {
                while let Some(msg) = request_rx.recv().await {
                    status_tx.send(display::Status::Working(msg)).await.unwrap();
                }
            });

            let response = prompt_wire_with_tools(
                &*crate::config::get_config().provider,
                request_tx.clone(),
                &system_prompt,
                messages.clone(),
                vec![],
            )
            .await?;

            if let Some(report) = Self::replace_messages(&response) {
                let _ = tx
                    .send(display::Status::Event(report.to_progress_event()))
                    .await;
            }

            Ok(response)
        })
        .await
        .map_err(|e| e as Box<dyn std::error::Error>)?;

        Ok(response.last().unwrap().clone())
    }

    /// Basic LLM request with tool usage
    /// NOTE: Returns the _entire_ conversation, up to date with the LLM's responses
    pub async fn llm_request_with_tools(
        agent: &config::AgentSettings,
        prompt_variant: Option<SystemPrompt>,
        system_prompt: String,
        user_message: String,
        tools: Vec<wire::types::Tool>,
        codex_model: Option<codex::CodexModel>,
        repo_root_override: Option<PathBuf>,
    ) -> Result<wire::types::Message, Box<dyn std::error::Error>> {
        Self::record_agent(agent, prompt_variant);

        let backend = agent.backend;
        let provider = agent.provider.clone();
        let codex_opts = agent.process.clone();
        let agent_scope = agent.scope;
        let resolved_codex_model = codex_model.unwrap_or_default();

        let _ = Self::add_message(provider.new_message(user_message).as_user().build());

        let messages = AUDITOR.lock().unwrap().messages.clone();

        match backend {
            config::BackendKind::Wire => {
                let response = run_wire_with_status(
                    provider.clone(),
                    system_prompt,
                    messages.clone(),
                    tools.clone(),
                    |resp| Self::replace_messages(resp),
                )
                .await?;
                Ok(response.last().unwrap().clone())
            }
            config::BackendKind::Process => {
                simulate_integration_changes()?;
                let runner = Arc::clone(agent.process_runner()?);
                let display_adapter = agent.display_adapter.clone();
                let repo_root = match repo_root_override {
                    Some(path) => path,
                    None => find_project_root()?.unwrap_or_else(|| PathBuf::from(".")),
                };
                let messages_clone = messages.clone();
                let provider_clone = provider.clone();
                let opts_clone = codex_opts.clone();
                let prompt_clone = system_prompt.clone();

                let codex_run = display::call_with_status(async move |tx| {
                    let request = codex::CodexRequest {
                        prompt: prompt_clone.clone(),
                        repo_root: repo_root.clone(),
                        profile: opts_clone.profile.clone(),
                        bin: opts_clone.binary_path.clone(),
                        extra_args: opts_clone.extra_args.clone(),
                        model: Some(resolved_codex_model.as_model_name().to_string()),
                        output_mode: codex::CodexOutputMode::EventsJson,
                        scope: Some(agent_scope),
                        metadata: BTreeMap::new(),
                    };

                    let response = runner
                        .execute(
                            request,
                            display_adapter.clone(),
                            Some(ProgressHook::Display(tx.clone())),
                        )
                        .await
                        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;

                    let mut updated = messages_clone.clone();
                    let mut assistant_message = provider_clone
                        .new_message(response.assistant_text.clone())
                        .as_assistant()
                        .build();
                    if let Some(usage) = response.usage {
                        assistant_message.input_tokens = usage.input_tokens;
                        assistant_message.output_tokens = usage.output_tokens;
                        Auditor::record_backend_usage(&usage);
                    } else {
                        Auditor::mark_usage_unknown();
                    }
                    updated.push(assistant_message);
                    if let Some(report) = Self::replace_messages(&updated) {
                        let _ = tx
                            .send(display::Status::Event(report.to_progress_event()))
                            .await;
                    }
                    Ok(updated)
                })
                .await;

                match codex_run {
                    Ok(response) => Ok(response.last().unwrap().clone()),
                    Err(err) => Err(err),
                }
            }
        }
    }

    // TODO: Rectify this with the function above
    pub async fn llm_request_with_tools_no_display(
        agent: &config::AgentSettings,
        prompt_variant: Option<SystemPrompt>,
        system_prompt: String,
        user_message: String,
        tools: Vec<wire::types::Tool>,
        stream: RequestStream,
        codex_model: Option<codex::CodexModel>,
        repo_root_override: Option<PathBuf>,
    ) -> Result<wire::types::Message, Box<dyn std::error::Error>> {
        Self::record_agent(agent, prompt_variant);

        let backend = agent.backend;
        let provider = agent.provider.clone();
        let codex_opts = agent.process.clone();
        let agent_scope = agent.scope;
        let resolved_codex_model = codex_model.unwrap_or_default();

        let _ = Self::add_message(provider.new_message(user_message).as_user().build());

        let messages = AUDITOR.lock().unwrap().messages.clone();

        match backend {
            config::BackendKind::Wire => {
                let (wire_tx, drain_handle) = channel_for_stream(&stream);
                let response = prompt_wire_with_tools(
                    &*provider,
                    wire_tx,
                    &system_prompt,
                    messages.clone(),
                    tools.clone(),
                )
                .await?;
                if let Some(handle) = drain_handle {
                    let _ = handle.await;
                }

                let last = response.last().unwrap().clone();
                if let Some(report) = Self::add_message(last.clone()) {
                    forward_usage_report(&report, &stream).await;
                }
                Ok(last)
            }
            config::BackendKind::Process => {
                simulate_integration_changes()?;
                let runner = Arc::clone(agent.process_runner()?);
                let display_adapter = agent.display_adapter.clone();
                let repo_root = match repo_root_override {
                    Some(path) => path,
                    None => find_project_root()?.unwrap_or_else(|| PathBuf::from(".")),
                };
                let (output_mode, progress_hook) = match &stream {
                    RequestStream::Status { events, .. } => (
                        codex::CodexOutputMode::EventsJson,
                        events.clone().map(ProgressHook::Plain),
                    ),
                    RequestStream::PassthroughStderr => {
                        (codex::CodexOutputMode::PassthroughHuman, None)
                    }
                };
                let request = codex::CodexRequest {
                    prompt: system_prompt.clone(),
                    repo_root,
                    profile: codex_opts.profile.clone(),
                    bin: codex_opts.binary_path.clone(),
                    extra_args: codex_opts.extra_args.clone(),
                    model: Some(resolved_codex_model.as_model_name().to_string()),
                    output_mode,
                    scope: Some(agent_scope),
                    metadata: BTreeMap::new(),
                };

                match runner
                    .execute(request, display_adapter.clone(), progress_hook)
                    .await
                {
                    Ok(response) => {
                        let mut assistant_message = provider
                            .new_message(response.assistant_text.clone())
                            .as_assistant()
                            .build();
                        if let Some(usage) = response.usage {
                            assistant_message.input_tokens = usage.input_tokens;
                            assistant_message.output_tokens = usage.output_tokens;
                            Auditor::record_backend_usage(&usage);
                        } else {
                            Auditor::mark_usage_unknown();
                        }
                        if let Some(report) = Self::add_message(assistant_message.clone()) {
                            forward_usage_report(&report, &stream).await;
                        }
                        Ok(assistant_message)
                    }
                    Err(err) => Err(Box::new(err)),
                }
            }
        }
    }
}

async fn run_wire_with_status<F>(
    provider: Arc<dyn wire::api::Prompt>,
    system_prompt: String,
    messages: Vec<wire::types::Message>,
    tools: Vec<wire::types::Tool>,
    mut on_response: F,
) -> Result<Vec<wire::types::Message>, Box<dyn std::error::Error>>
where
    F: FnMut(&Vec<wire::types::Message>) -> Option<TokenUsageReport> + Send + 'static,
{
    display::call_with_status(async move |tx| {
        tx.send(display::Status::Working("Thinking...".into()))
            .await?;

        let (request_tx, mut request_rx) = channel(10);
        let status_tx = tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = request_rx.recv().await {
                status_tx.send(display::Status::Working(msg)).await.unwrap();
            }
        });

        simulate_integration_changes()?;

        let response = prompt_wire_with_tools(
            &*provider,
            request_tx.clone(),
            &system_prompt,
            messages.clone(),
            tools.clone(),
        )
        .await?;

        if let Some(report) = on_response(&response) {
            let _ = tx
                .send(display::Status::Event(report.to_progress_event()))
                .await;
        }

        Ok(response)
    })
    .await
    .map_err(|e| e as Box<dyn std::error::Error>)
}

#[cfg(feature = "integration_testing")]
fn simulate_integration_changes() -> Result<(), Box<dyn std::error::Error>> {
    let skip_code_change = std::env::var("VIZIER_IT_SKIP_CODE_CHANGE").is_ok();
    let skip_vizier_change = std::env::var("VIZIER_IT_SKIP_VIZIER_CHANGE").is_ok();

    if !skip_code_change {
        crate::file_tracking::FileTracker::write("a", "some change")?;
    }

    if !skip_vizier_change {
        crate::file_tracking::FileTracker::write(".vizier/.snapshot", "some snapshot change")?;
        crate::file_tracking::FileTracker::write(".vizier/todo.md", "some todo change")?;
    }

    Ok(())
}

#[cfg(not(feature = "integration_testing"))]
fn simulate_integration_changes() -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

pub enum CommitMessageType {
    CodeChange,
    Conversation,
    NarrativeChange,
}

pub struct CommitMessageBuilder {
    message_type: Option<CommitMessageType>,
    session_id: String,
    session_log_path: Option<String>,
    author_note: Option<String>,
    narrative_summary: Option<String>,
    body: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct SessionRepoState {
    root: String,
    branch: Option<String>,
    head: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct SessionPromptInfo {
    kind: String,
    scope: String,
    origin: String,
    path: Option<String>,
    hash: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct SessionModelInfo {
    provider: String,
    name: String,
    reasoning_effort: Option<String>,
    scope: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct SessionTokenUsage {
    prompt_total: usize,
    completion_total: usize,
    total: usize,
    prompt_delta: usize,
    completion_delta: usize,
    delta_total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached_input_total: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached_input_delta: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_output_total: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_output_delta: Option<usize>,
    known: bool,
}

impl From<&TokenUsageReport> for SessionTokenUsage {
    fn from(report: &TokenUsageReport) -> Self {
        SessionTokenUsage {
            prompt_total: report.prompt_total,
            completion_total: report.completion_total,
            total: report.total(),
            prompt_delta: report.prompt_delta,
            completion_delta: report.completion_delta,
            delta_total: report.delta_total(),
            cached_input_total: report.cached_input_total,
            cached_input_delta: report.cached_input_delta,
            reasoning_output_total: report.reasoning_output_total,
            reasoning_output_delta: report.reasoning_output_delta,
            known: report.known,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct SessionOutcome {
    status: String,
    summary: Option<String>,
    token_usage: Option<SessionTokenUsage>,
}

#[derive(Serialize, Deserialize, Clone)]
struct SessionLog {
    schema: String,
    id: String,
    created_at: String,
    updated_at: String,
    tool_version: String,
    workflow_type: String,
    mode: String,
    repo: SessionRepoState,
    config_effective: serde_json::Value,
    system_prompt: SessionPromptInfo,
    model: SessionModelInfo,
    messages: Vec<wire::types::Message>,
    operations: Vec<serde_json::Value>,
    artifacts: Vec<String>,
    outcome: SessionOutcome,
}

#[derive(Deserialize)]
struct SessionLogWrapper {
    messages: Vec<wire::types::Message>,
}

#[derive(Clone, Debug)]
pub struct SessionArtifact {
    pub id: String,
    pub path: PathBuf,
    relative_path: Option<String>,
}

impl SessionArtifact {
    fn new(id: &str, path: PathBuf, project_root: &Path) -> Self {
        let relative = path
            .strip_prefix(project_root)
            .ok()
            .map(|value| value.to_string_lossy().to_string());

        Self {
            id: id.to_string(),
            path,
            relative_path: relative,
        }
    }

    pub fn display_path(&self) -> String {
        self.relative_path
            .clone()
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{AUDITOR, Auditor, CommitMessageBuilder, CommitMessageType, find_project_root};
    use std::fs;
    use tempfile::tempdir;
    use wire::{
        api::{API, OpenAIModel},
        types::MessageBuilder,
    };

    #[test]
    fn detects_worktree_root_with_git_file() {
        let tmp = tempdir().unwrap();
        let worktree = tmp.path().join("worktree");
        fs::create_dir(&worktree).unwrap();
        let nested = worktree.join("nested");
        fs::create_dir(&nested).unwrap();

        let git_dir = tmp.path().join("actual.git");
        fs::create_dir(&git_dir).unwrap();
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .unwrap();

        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&nested).unwrap();
        let detected = find_project_root().unwrap().expect("worktree root");
        assert_eq!(
            detected.canonicalize().unwrap(),
            worktree.canonicalize().unwrap()
        );
        std::env::set_current_dir(prev).unwrap();
    }

    #[test]
    fn usage_reports_accumulate_and_suppress_duplicates() {
        let mut auditor = Auditor::new();

        assert!(
            add_message_for_test(&mut auditor, user_message("hello")).is_none(),
            "user messages should not emit usage reports"
        );

        let first = add_message_for_test(&mut auditor, assistant_message("one", 10, 5))
            .expect("first assistant message should produce a report");
        assert_eq!(first.prompt_total, 10);
        assert_eq!(first.prompt_delta, 10);
        assert_eq!(first.completion_total, 5);
        assert_eq!(first.completion_delta, 5);
        assert!(first.known, "first report should mark usage as known");

        let second = add_message_for_test(&mut auditor, assistant_message("two", 3, 7))
            .expect("second assistant message should produce a report");
        assert_eq!(second.prompt_total, 13);
        assert_eq!(second.prompt_delta, 3);
        assert_eq!(second.completion_total, 12);
        assert_eq!(second.completion_delta, 7);

        let history = auditor.messages.clone();
        assert!(
            replay_messages(&mut auditor, &history).is_none(),
            "replaying the same messages should not emit duplicate reports"
        );
    }

    #[test]
    fn usage_reports_mark_unknown_usage() {
        let mut auditor = Auditor::new();
        auditor.usage_unknown = true;

        let report = add_message_for_test(&mut auditor, assistant_message("unknown", 0, 0))
            .expect("unknown usage should still create a report");
        assert!(
            !report.known,
            "report should mark usage as unknown when backend omits counts"
        );
        assert_eq!(report.prompt_total, 0);
        assert_eq!(report.completion_total, 0);

        let history = auditor.messages.clone();
        assert!(
            replay_messages(&mut auditor, &history).is_none(),
            "duplicate unknown-usage snapshots should be suppressed"
        );

        reset_auditor_state();
    }

    #[test]
    fn commit_builder_uses_summary_as_header() {
        let mut builder = CommitMessageBuilder::new(
            "feat: tighten CLI epilogue\n\n- ensure Outcome lines match Auditor facts".to_string(),
        );
        builder.set_header(CommitMessageType::CodeChange);

        let message = builder.build();
        assert!(
            message.starts_with("feat: tighten CLI epilogue"),
            "expected descriptive header, got '{}'",
            message
        );
        assert!(
            !message.contains("VIZIER CODE CHANGE"),
            "should not fall back to generic header when summary exists"
        );
        assert!(
            message.contains("\n\n- ensure Outcome lines match Auditor facts"),
            "original body should remain after metadata"
        );
    }

    #[test]
    fn commit_builder_falls_back_when_summary_missing() {
        let mut builder = CommitMessageBuilder::new("\n".to_string());
        builder.set_header(CommitMessageType::NarrativeChange);

        let message = builder.build();
        assert!(
            message.starts_with("VIZIER NARRATIVE CHANGE"),
            "missing summary should keep generic header"
        );
    }

    fn reset_auditor_state() {
        let mut auditor = AUDITOR.lock().unwrap();
        auditor.messages.clear();
        auditor.last_usage_report = None;
        auditor.usage_unknown = false;
        auditor.backend_usage_totals.clear();
    }

    fn assistant_message(content: &str, input: usize, output: usize) -> wire::types::Message {
        MessageBuilder::new(API::OpenAI(OpenAIModel::GPT5), content)
            .as_assistant()
            .with_usage(input, output)
            .build()
    }

    fn user_message(content: &str) -> wire::types::Message {
        MessageBuilder::new(API::OpenAI(OpenAIModel::GPT5), content)
            .as_user()
            .with_usage(0, 0)
            .build()
    }

    fn add_message_for_test(
        auditor: &mut Auditor,
        message: wire::types::Message,
    ) -> Option<super::TokenUsageReport> {
        let should_emit = message.message_type == wire::types::MessageType::Assistant;
        auditor.messages.push(message);
        if should_emit {
            Auditor::capture_usage_report_locked(auditor)
        } else {
            None
        }
    }

    fn replay_messages(
        auditor: &mut Auditor,
        messages: &[wire::types::Message],
    ) -> Option<super::TokenUsageReport> {
        auditor.messages = messages.to_vec();
        Auditor::capture_usage_report_locked(auditor)
    }
}

impl CommitMessageBuilder {
    pub fn new(body: String) -> Self {
        Self {
            message_type: None,
            session_id: AUDITOR.lock().unwrap().session_id.clone(),
            session_log_path: None,
            author_note: None,
            narrative_summary: None,
            body,
        }
    }

    pub fn set_header(&mut self, message_type: CommitMessageType) -> &mut Self {
        self.message_type = Some(message_type);
        self
    }

    pub fn with_author_note(&mut self, note: String) -> &mut Self {
        self.author_note = Some(note);

        self
    }

    pub fn with_session_log_path(&mut self, session_log_path: Option<String>) -> &mut Self {
        self.session_log_path = session_log_path;

        self
    }

    pub fn with_narrative_summary(&mut self, summary: Option<String>) -> &mut Self {
        self.narrative_summary = summary.and_then(|text| {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        });

        self
    }

    pub fn build(&self) -> String {
        let (derived_subject, remainder) = Self::split_subject_from_body(&self.body);
        let use_subject = derived_subject.is_some();
        let header = derived_subject
            .clone()
            .or_else(|| {
                self.message_type
                    .as_ref()
                    .map(|ty| ty.fallback_subject().to_string())
            })
            .unwrap_or_else(|| "VIZIER".to_string());

        let mut message = format!("{}\nSession ID: {}", header, self.session_id);

        if let Some(path) = &self.session_log_path {
            message = format!("{}\nSession Log: {}", message, path);
        }

        if let Some(an) = &self.author_note {
            message = format!("{}\nAuthor note: {}", message, an);
        }

        let body = if use_subject {
            remainder
        } else {
            self.body.clone()
        };

        let mut sections: Vec<String> = Vec::new();
        let body_trimmed = body.trim();
        if !body_trimmed.is_empty() {
            sections.push(body_trimmed.to_string());
        }

        if let Some(summary) = &self.narrative_summary {
            let trimmed = summary.trim();
            if !trimmed.is_empty() {
                sections.push(format!("Narrative updates:\n{}", trimmed));
            }
        }

        if sections.is_empty() {
            message
        } else {
            format!("{}\n\n{}", message, sections.join("\n\n"))
        }
    }

    fn split_subject_from_body(body: &str) -> (Option<String>, String) {
        let mut subject = None;
        let mut seen_subject = false;
        let mut remainder: Vec<&str> = Vec::new();

        for line in body.lines() {
            if !seen_subject {
                if line.trim().is_empty() {
                    continue;
                }
                subject = Some(line.trim().to_string());
                seen_subject = true;
                continue;
            }

            remainder.push(line);
        }

        if subject.is_none() {
            return (None, body.to_string());
        }

        (subject, remainder.join("\n"))
    }
}

impl CommitMessageType {
    fn fallback_subject(&self) -> &'static str {
        match self {
            CommitMessageType::CodeChange => "VIZIER CODE CHANGE",
            CommitMessageType::Conversation => "VIZIER CONVERSATION",
            CommitMessageType::NarrativeChange => "VIZIER NARRATIVE CHANGE",
        }
    }
}
