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
use std::time::Duration;
use tokio::sync::mpsc::Sender;

use crate::{
    agent::{AgentRequest, ProgressHook},
    config::{self, PromptOrigin, SystemPrompt},
    display, file_tracking, tools, vcs,
};

lazy_static! {
    static ref AUDITOR: Mutex<Auditor> = Mutex::new(Auditor::new());
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

impl MessageRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageRole::System => "System",
            MessageRole::User => "User",
            MessageRole::Assistant => "Assistant",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AgentRunRecord {
    pub command: Vec<String>,
    pub output: config::AgentOutputHandling,
    pub progress_filter: Option<Vec<String>>,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: Vec<String>,
    pub duration_ms: u128,
}

impl AgentRunRecord {
    pub fn to_rows(&self) -> Vec<(String, String)> {
        let mut rows = Vec::new();
        rows.push(("Exit code".to_string(), self.exit_code.to_string()));
        rows.push((
            "Duration".to_string(),
            format!("{:.2}s", self.duration_ms as f64 / 1000.0),
        ));
        rows
    }
}

#[derive(Clone)]
pub enum RequestStream {
    Status {
        text: Sender<String>,
        events: Option<Sender<display::ProgressEvent>>,
    },
}

pub struct AuditorCleanup {
    pub debug: bool,
    pub print_json: bool,
    pub persisted: bool,
}

#[derive(Clone, Debug)]
pub struct AgentInvocationContext {
    pub backend: config::BackendKind,
    pub backend_label: String,
    pub selector: String,
    pub scope: config::CommandScope,
    pub prompt_kind: SystemPrompt,
    pub command: Vec<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u128>,
}

impl Drop for AuditorCleanup {
    fn drop(&mut self) {
        if self.persisted {
            return;
        }

        if let Some(artifact) = Auditor::persist_session_log() {
            display::info(format!("Session saved to {}", artifact.display_path()));

            if self.print_json
                && let Ok(contents) = fs::read_to_string(&artifact.path)
            {
                println!("{}", contents);
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

/// _All_ LLM interactions need run through the auditor
/// This should hold every LLM interaction from the current session, in chronological order
#[derive(Debug, Serialize, Deserialize)]
pub struct Auditor {
    messages: Vec<Message>,
    session_start: String,
    session_id: String,
    #[serde(skip)]
    last_session_artifact: Option<SessionArtifact>,
    #[serde(skip)]
    last_agent: Option<AgentInvocationContext>,
    #[serde(skip)]
    last_run: Option<AgentRunRecord>,
    #[serde(default)]
    operations: Vec<serde_json::Value>,
}

impl Default for Auditor {
    fn default() -> Self {
        Self::new()
    }
}

impl Auditor {
    pub fn new() -> Self {
        let now = Utc::now();

        Auditor {
            messages: Vec::new(),
            session_start: now.to_string(),
            session_id: uuid::Uuid::new_v4().to_string(),
            last_session_artifact: None,
            last_agent: None,
            last_run: None,
            operations: Vec::new(),
        }
    }

    /// Clones the message history in the auditor
    pub fn get_messages() -> Vec<Message> {
        AUDITOR.lock().unwrap().messages.clone()
    }

    pub fn add_message(message: Message) {
        let mut auditor = AUDITOR.lock().unwrap();
        auditor.messages.push(message);
    }

    pub fn replace_messages(messages: &[Message]) {
        let mut auditor = AUDITOR.lock().unwrap();
        auditor.messages = messages.to_owned();
    }

    fn record_agent(settings: &config::AgentSettings, prompt_kind: Option<SystemPrompt>) {
        if let Ok(mut auditor) = AUDITOR.lock() {
            let backend_label = settings.agent_runtime.label.clone();
            auditor.last_agent = Some(AgentInvocationContext {
                backend: settings.backend,
                backend_label,
                selector: settings.selector.clone(),
                scope: settings.scope,
                prompt_kind: prompt_kind.unwrap_or(SystemPrompt::Documentation),
                command: settings.agent_runtime.command.clone(),
                exit_code: None,
                duration_ms: None,
            });
            auditor.last_run = None;
        }
    }

    pub fn record_agent_context(
        settings: &config::AgentSettings,
        prompt_kind: Option<SystemPrompt>,
    ) {
        Self::record_agent(settings, prompt_kind);
    }

    pub fn record_agent_run(run: AgentRunRecord) {
        if let Ok(mut auditor) = AUDITOR.lock() {
            auditor.last_run = Some(run.clone());
            if let Some(agent) = auditor.last_agent.as_mut() {
                agent.exit_code = Some(run.exit_code);
                agent.duration_ms = Some(run.duration_ms);
            }
        }
    }

    pub fn record_operation(kind: &str, details: serde_json::Value) {
        if let Ok(mut auditor) = AUDITOR.lock() {
            auditor
                .operations
                .push(json!({"kind": kind, "details": details}));
        }
    }

    pub fn latest_agent_context() -> Option<AgentInvocationContext> {
        AUDITOR
            .lock()
            .ok()
            .and_then(|auditor| auditor.last_agent.clone())
    }

    pub fn latest_agent_run() -> Option<AgentRunRecord> {
        AUDITOR
            .lock()
            .ok()
            .and_then(|auditor| auditor.last_run.clone())
    }

    pub fn conversation_to_string() -> String {
        let messages = AUDITOR.lock().unwrap().messages.clone();
        let mut conversation = String::new();

        for message in messages.iter() {
            conversation.push_str(&format!(
                "{}: {}\n\n###\n\n",
                message.role.as_str(),
                message.content
            ));
        }

        conversation
    }

    pub fn clear_messages() {
        if let Ok(mut auditor) = AUDITOR.lock() {
            auditor.messages.clear();
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

        let mut diff_chunks = Vec::new();
        for target in tools::story_diff_targets() {
            match vcs::get_diff(root, Some(&target), None) {
                Ok(diff) => diff_chunks.push(diff),
                Err(err) => display::debug(format!(
                    "Unable to compute narrative diff for {target}: {err}"
                )),
            }
        }

        let mut diff_message = None;
        if !diff_chunks.is_empty() {
            let combined = diff_chunks.join("");
            display::info("Writing commit message for narrative changes...");
            diff_message = Some(
                Self::llm_request(
                    "Given a diff on narrative artifacts, return a commit message for these changes."
                        .to_string(),
                    if combined.is_empty() {
                        "init".to_string()
                    } else {
                        combined
                    },
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
    ) -> Result<Vec<Message>, Box<dyn std::error::Error>> {
        let contents = fs::read_to_string(path)?;
        Ok(Self::parse_session_messages(&contents)?)
    }

    fn parse_session_messages(contents: &str) -> Result<Vec<Message>, serde_json::Error> {
        if let Ok(wrapper) = serde_json::from_str::<SessionLogWrapper>(contents) {
            return Ok(wrapper.messages);
        }

        serde_json::from_str(contents)
    }

    pub fn persist_session_log() -> Option<SessionArtifact> {
        if config::get_config().no_session {
            return None;
        }

        let project_root = find_project_root().ok().flatten()?;

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
            agent: self.last_run.as_ref().map(SessionAgentRun::from),
            operations: self.operations.clone(),
            artifacts: Vec::new(),
            outcome: SessionOutcome {
                status: "completed".to_string(),
                summary: Self::summarize_assistant(&self.messages),
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
                name: ctx.backend_label.clone(),
                reasoning_effort: None,
                scope: Some(ctx.scope.as_str().to_string()),
            },
            None => SessionModelInfo {
                provider: cfg.backend.to_string(),
                name: cfg.agent_selector.clone(),
                reasoning_effort: None,
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
            "agent": {
                "label": cfg.agent_runtime.label.clone(),
                "command": cfg.agent_runtime.command.clone(),
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
            .unwrap_or(SystemPrompt::Documentation);
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

    fn summarize_assistant(messages: &[Message]) -> Option<String> {
        for message in messages.iter().rev() {
            if message.role == MessageRole::Assistant {
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
        let buffer = serde_json::to_vec_pretty(log).map_err(std::io::Error::other)?;
        let mut tmp = File::create(&tmp_path)?;
        tmp.write_all(&buffer)?;
        tmp.sync_all()?;
        fs::rename(&tmp_path, &session_path)?;

        Ok(SessionArtifact::new(&log.id, session_path, project_root))
    }

    /// Basic LLM request without tool usage
    pub async fn llm_request(
        #[cfg_attr(feature = "integration_testing", allow(unused_variables))] system_prompt: String,
        user_message: String,
    ) -> Result<Message, Box<dyn std::error::Error>> {
        let agent = crate::config::get_config().resolve_prompt_profile(
            config::CommandScope::Ask,
            SystemPrompt::Commit,
            None,
        )?;
        Self::llm_request_with_tools(
            &agent,
            Some(SystemPrompt::Commit),
            system_prompt,
            user_message,
            None,
        )
        .await
    }

    /// Builds the prompt string sent to the agent runner, embedding the user
    /// message for commit flows so the diff is present in the request payload.
    fn render_prompt(
        system_prompt: &str,
        user_message: &str,
        prompt_variant: Option<SystemPrompt>,
    ) -> String {
        if user_message.trim().is_empty() {
            return system_prompt.to_string();
        }

        if !matches!(prompt_variant, Some(SystemPrompt::Commit)) {
            return system_prompt.to_string();
        }

        format!("{system_prompt}\n\n<userMessage>\n{user_message}\n</userMessage>\n")
    }

    /// Basic LLM request with tool usage
    /// NOTE: Returns the _entire_ conversation, up to date with the LLM's responses
    pub async fn llm_request_with_tools(
        agent: &config::AgentSettings,
        prompt_variant: Option<SystemPrompt>,
        system_prompt: String,
        user_message: String,
        repo_root_override: Option<PathBuf>,
    ) -> Result<Message, Box<dyn std::error::Error>> {
        Self::record_agent(agent, prompt_variant);
        let runtime_opts = agent.agent_runtime.clone();
        let agent_scope = agent.scope;
        let rendered_prompt = Self::render_prompt(&system_prompt, &user_message, prompt_variant);
        let mut messages = AUDITOR.lock().unwrap().messages.clone();
        messages.push(Message::user(user_message));
        Self::replace_messages(&messages);

        simulate_integration_changes()?;
        let runner = Arc::clone(agent.agent_runner()?);
        let repo_root = match repo_root_override {
            Some(path) => path,
            None => find_project_root()?.unwrap_or_else(|| PathBuf::from(".")),
        };
        let messages_clone = messages.clone();
        let opts_clone = runtime_opts.clone();
        let prompt_clone = rendered_prompt.clone();
        let mut metadata = BTreeMap::new();
        metadata.insert("agent_backend".to_string(), agent.backend.to_string());
        metadata.insert("agent_label".to_string(), opts_clone.label.clone());
        if !opts_clone.command.is_empty() {
            metadata.insert("agent_command".to_string(), opts_clone.command.join(" "));
        }
        metadata.insert(
            "agent_output".to_string(),
            opts_clone.output.as_str().to_string(),
        );
        if let Some(filter) = opts_clone.progress_filter.as_ref() {
            metadata.insert("agent_progress_filter".to_string(), filter.join(" "));
        }
        match &opts_clone.resolution {
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

        let codex_run = display::call_with_status(async move |tx| {
            let request = AgentRequest {
                prompt: prompt_clone.clone(),
                repo_root: repo_root.clone(),
                command: opts_clone.command.clone(),
                progress_filter: opts_clone.progress_filter.clone(),
                output: opts_clone.output,
                allow_script_wrapper: opts_clone.enable_script_wrapper,
                scope: Some(agent_scope),
                metadata,
                timeout: Some(Duration::from_secs(9000)),
            };

            let response = runner
                .execute(request, Some(ProgressHook::Display(tx.clone())))
                .await
                .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;

            let mut updated = messages_clone.clone();
            let message_text = if response.assistant_text.trim().is_empty() {
                " ".to_string()
            } else {
                response.assistant_text.clone()
            };
            let assistant_message = Message::assistant(message_text);
            updated.push(assistant_message.clone());
            Self::replace_messages(&updated);
            Auditor::record_agent_run(AgentRunRecord {
                command: opts_clone.command.clone(),
                output: opts_clone.output,
                progress_filter: opts_clone.progress_filter.clone(),
                exit_code: response.exit_code,
                stdout: response.assistant_text.clone(),
                stderr: response.stderr.clone(),
                duration_ms: response.duration_ms,
            });
            Ok(updated)
        })
        .await;

        match codex_run {
            Ok(response) => Ok(response.last().unwrap().clone()),
            Err(err) => Err(err),
        }
    }

    // TODO: Rectify this with the function above
    pub async fn llm_request_with_tools_no_display(
        agent: &config::AgentSettings,
        prompt_variant: Option<SystemPrompt>,
        system_prompt: String,
        user_message: String,
        stream: RequestStream,
        repo_root_override: Option<PathBuf>,
    ) -> Result<Message, Box<dyn std::error::Error>> {
        Self::record_agent(agent, prompt_variant);

        let runtime_opts = agent.agent_runtime.clone();
        let agent_scope = agent.scope;
        let rendered_prompt = Self::render_prompt(&system_prompt, &user_message, prompt_variant);
        let mut messages = AUDITOR.lock().unwrap().messages.clone();
        messages.push(Message::user(user_message));
        Self::replace_messages(&messages);

        simulate_integration_changes()?;
        let runner = Arc::clone(agent.agent_runner()?);
        let repo_root = match repo_root_override {
            Some(path) => path,
            None => find_project_root()?.unwrap_or_else(|| PathBuf::from(".")),
        };
        let progress_hook = match &stream {
            RequestStream::Status { events, .. } => events.clone().map(ProgressHook::Plain),
        };
        let mut metadata = BTreeMap::new();
        metadata.insert("agent_backend".to_string(), agent.backend.to_string());
        metadata.insert("agent_label".to_string(), runtime_opts.label.clone());
        if !runtime_opts.command.is_empty() {
            metadata.insert("agent_command".to_string(), runtime_opts.command.join(" "));
        }
        metadata.insert(
            "agent_output".to_string(),
            runtime_opts.output.as_str().to_string(),
        );
        if let Some(filter) = runtime_opts.progress_filter.as_ref() {
            metadata.insert("agent_progress_filter".to_string(), filter.join(" "));
        }
        match &runtime_opts.resolution {
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
        let request = AgentRequest {
            prompt: rendered_prompt.clone(),
            repo_root,
            command: runtime_opts.command.clone(),
            progress_filter: runtime_opts.progress_filter.clone(),
            output: runtime_opts.output,
            allow_script_wrapper: runtime_opts.enable_script_wrapper,
            scope: Some(agent_scope),
            metadata,
            timeout: Some(Duration::from_secs(9000)),
        };

        match runner.execute(request, progress_hook).await {
            Ok(response) => {
                let message_text = if response.assistant_text.trim().is_empty() {
                    " ".to_string()
                } else {
                    response.assistant_text.clone()
                };
                let assistant_message = Message::assistant(message_text);
                Self::add_message(assistant_message.clone());
                Auditor::record_agent_run(AgentRunRecord {
                    command: runtime_opts.command.clone(),
                    output: runtime_opts.output,
                    progress_filter: runtime_opts.progress_filter.clone(),
                    exit_code: response.exit_code,
                    stdout: response.assistant_text.clone(),
                    stderr: response.stderr.clone(),
                    duration_ms: response.duration_ms,
                });
                Ok(assistant_message)
            }
            Err(err) => Err(Box::new(err)),
        }
    }
}

#[cfg(feature = "integration_testing")]
fn simulate_integration_changes() -> Result<(), Box<dyn std::error::Error>> {
    let skip_code_change = std::env::var("VIZIER_IT_SKIP_CODE_CHANGE").is_ok();
    let skip_vizier_change = std::env::var("VIZIER_IT_SKIP_VIZIER_CHANGE").is_ok();
    let skip_glossary_change = std::env::var("VIZIER_IT_SKIP_GLOSSARY_CHANGE").is_ok();

    if !skip_code_change {
        crate::file_tracking::FileTracker::write("a", "some change")?;
    }

    if !skip_vizier_change {
        std::fs::create_dir_all(".vizier/narrative/threads")?;
        crate::file_tracking::FileTracker::write(
            ".vizier/narrative/snapshot.md",
            "some snapshot change",
        )?;
        if !skip_glossary_change {
            crate::file_tracking::FileTracker::write(
                ".vizier/narrative/glossary.md",
                "some glossary change",
            )?;
        }
        crate::file_tracking::FileTracker::write(
            ".vizier/narrative/threads/demo.md",
            "some narrative change",
        )?;
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
    session_artifact: Option<SessionArtifact>,
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
struct SessionAgentRun {
    command: Vec<String>,
    output: String,
    progress_filter: Option<Vec<String>>,
    exit_code: i32,
    stdout: String,
    stderr: Vec<String>,
    duration_ms: u128,
}

impl From<&AgentRunRecord> for SessionAgentRun {
    fn from(run: &AgentRunRecord) -> Self {
        SessionAgentRun {
            command: run.command.clone(),
            output: run.output.as_str().to_string(),
            progress_filter: run.progress_filter.clone(),
            exit_code: run.exit_code,
            stdout: run.stdout.clone(),
            stderr: run.stderr.clone(),
            duration_ms: run.duration_ms,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct SessionOutcome {
    status: String,
    summary: Option<String>,
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
    messages: Vec<Message>,
    agent: Option<SessionAgentRun>,
    operations: Vec<serde_json::Value>,
    artifacts: Vec<String>,
    outcome: SessionOutcome,
}

#[derive(Deserialize)]
struct SessionLogWrapper {
    messages: Vec<Message>,
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

fn meta_lines(
    meta: &config::CommitMetaConfig,
    session_id: &str,
    session_artifact: Option<&SessionArtifact>,
    author_note: Option<&String>,
) -> Vec<String> {
    let mut lines = Vec::new();

    for field in &meta.include {
        match field {
            config::CommitMetaField::SessionId => {
                lines.push(format!("{}: {}", meta.labels.session_id, session_id));
            }
            config::CommitMetaField::SessionLog => {
                if matches!(meta.session_log_path, config::CommitSessionLogPath::None) {
                    continue;
                }
                let path = session_artifact.and_then(|artifact| match meta.session_log_path {
                    config::CommitSessionLogPath::Relative => Some(artifact.display_path()),
                    config::CommitSessionLogPath::Absolute => {
                        Some(artifact.path.display().to_string())
                    }
                    config::CommitSessionLogPath::None => None,
                });
                if let Some(path) = path {
                    lines.push(format!("{}: {}", meta.labels.session_log, path));
                }
            }
            config::CommitMetaField::AuthorNote => {
                if let Some(note) = author_note
                    .map(|value| value.trim())
                    .filter(|value| !value.is_empty())
                {
                    lines.push(format!("{}: {}", meta.labels.author_note, note));
                }
            }
            config::CommitMetaField::NarrativeSummary => {}
        }
    }

    lines
}

impl CommitMessageBuilder {
    pub fn new(body: String) -> Self {
        Self {
            message_type: None,
            session_id: AUDITOR.lock().unwrap().session_id.clone(),
            session_artifact: None,
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

    pub fn with_session_artifact(
        &mut self,
        session_artifact: Option<SessionArtifact>,
    ) -> &mut Self {
        self.session_artifact = session_artifact;

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
        let cfg = config::get_config();
        let fallback_subject = self
            .message_type
            .as_ref()
            .map(|ty| ty.fallback_subject(&cfg.commits.fallback_subjects));
        let header = derived_subject
            .clone()
            .or_else(|| fallback_subject.map(|value| value.to_string()))
            .unwrap_or_else(|| "VIZIER".to_string());

        let meta = &cfg.commits.meta;
        let meta_active = meta.enabled && !matches!(meta.style, config::CommitMetaStyle::None);
        let meta_fields = meta_lines(
            meta,
            &self.session_id,
            self.session_artifact.as_ref(),
            self.author_note.as_ref(),
        );

        let mut message = header.clone();
        if meta_active
            && matches!(
                meta.style,
                config::CommitMetaStyle::Header | config::CommitMetaStyle::Both
            )
            && !meta_fields.is_empty()
        {
            message = format!("{}\n{}", message, meta_fields.join("\n"));
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

        if meta_active
            && meta
                .include
                .contains(&config::CommitMetaField::NarrativeSummary)
            && let Some(summary) = &self.narrative_summary
        {
            let trimmed = summary.trim();
            if !trimmed.is_empty() {
                sections.push(format!("{}:\n{}", meta.labels.narrative_summary, trimmed));
            }
        }

        if !sections.is_empty() {
            message = format!("{}\n\n{}", message, sections.join("\n\n"));
        }

        if meta_active
            && matches!(
                meta.style,
                config::CommitMetaStyle::Trailers | config::CommitMetaStyle::Both
            )
            && !meta_fields.is_empty()
        {
            message = format!("{}\n\n{}", message, meta_fields.join("\n"));
        }

        message
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
    fn fallback_subject<'a>(&self, fallback: &'a config::CommitFallbackSubjects) -> &'a str {
        match self {
            CommitMessageType::CodeChange => fallback.code_change.as_str(),
            CommitMessageType::Conversation => fallback.conversation.as_str(),
            CommitMessageType::NarrativeChange => fallback.narrative_change.as_str(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Auditor, CommitMessageBuilder, CommitMessageType, SessionArtifact, SystemPrompt,
        find_project_root,
    };
    use crate::config;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn with_config<F: FnOnce()>(cfg: config::Config, f: F) {
        let _guard = config::test_config_lock().lock().unwrap();
        let original = config::get_config();
        config::set_config(cfg);
        f();
        config::set_config(original);
    }

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

    #[test]
    fn commit_builder_respects_meta_trailers() {
        let mut cfg = config::Config::default();
        cfg.commits.meta.style = config::CommitMetaStyle::Trailers;
        cfg.commits.meta.include = vec![config::CommitMetaField::SessionId];

        with_config(cfg, || {
            let mut builder =
                CommitMessageBuilder::new("feat: footer test\n\nBody text".to_string());
            builder.set_header(CommitMessageType::CodeChange);
            let message = builder.build();
            let lines: Vec<&str> = message.lines().collect();
            assert_eq!(lines.first().copied(), Some("feat: footer test"));
            assert!(
                lines.last().unwrap_or(&"").starts_with("Session ID: "),
                "expected session id trailer, got {message}"
            );
        });
    }

    #[test]
    fn commit_builder_respects_meta_labels_and_includes() {
        let mut cfg = config::Config::default();
        cfg.commits.meta.include = vec![config::CommitMetaField::SessionId];
        cfg.commits.meta.labels.session_id = "Vizier-Session".to_string();
        cfg.commits.meta.session_log_path = config::CommitSessionLogPath::None;

        with_config(cfg, || {
            let mut builder = CommitMessageBuilder::new("feat: label test".to_string());
            builder.set_header(CommitMessageType::CodeChange);
            let message = builder.build();
            assert!(
                message.contains("Vizier-Session: "),
                "expected custom session label, got {message}"
            );
            assert!(
                !message.contains("Session Log:"),
                "session log line should be omitted"
            );
        });
    }

    #[test]
    fn commit_builder_uses_fallback_subject_override() {
        let mut cfg = config::Config::default();
        cfg.commits.fallback_subjects.narrative_change = "CUSTOM NARRATIVE".to_string();

        with_config(cfg, || {
            let mut builder = CommitMessageBuilder::new("\n".to_string());
            builder.set_header(CommitMessageType::NarrativeChange);
            let message = builder.build();
            assert!(
                message.starts_with("CUSTOM NARRATIVE"),
                "fallback subject should use config override, got {message}"
            );
        });
    }

    #[test]
    fn commit_builder_uses_absolute_session_log_path() {
        let mut cfg = config::Config::default();
        cfg.commits.meta.include = vec![config::CommitMetaField::SessionLog];
        cfg.commits.meta.session_log_path = config::CommitSessionLogPath::Absolute;

        with_config(cfg, || {
            let tmp = tempdir().unwrap();
            let path = tmp.path().join("session.json");
            fs::write(&path, "{}").unwrap();
            let artifact = SessionArtifact {
                id: "abc".to_string(),
                path: PathBuf::from(&path),
                relative_path: None,
            };
            let mut builder = CommitMessageBuilder::new("feat: path test".to_string());
            builder
                .set_header(CommitMessageType::CodeChange)
                .with_session_artifact(Some(artifact));
            let message = builder.build();
            assert!(
                message.contains(&path.display().to_string()),
                "expected absolute session log path, got {message}"
            );
        });
    }

    #[test]
    fn render_prompt_wraps_user_message() {
        let prompt = "base prompt";
        let user_msg = "diff --git a/file b/file";
        let rendered = Auditor::render_prompt(prompt, user_msg, Some(SystemPrompt::Commit));

        assert!(rendered.contains(prompt));
        assert!(rendered.contains("<userMessage>"));
        assert!(rendered.contains(user_msg));
        assert!(rendered.contains("</userMessage>"));
    }

    #[test]
    fn render_prompt_passthrough_on_empty_user_message() {
        let prompt = "base prompt";
        let rendered = Auditor::render_prompt(prompt, "", Some(SystemPrompt::Documentation));

        assert_eq!(rendered, prompt);
    }

    #[test]
    fn render_prompt_passthrough_for_non_commit_variant() {
        let prompt = "base prompt";
        let rendered =
            Auditor::render_prompt(prompt, "instructions", Some(SystemPrompt::Documentation));

        assert_eq!(rendered, prompt);
    }
}
