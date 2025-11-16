use chrono::Utc;
use git2::Repository;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokio::{
    sync::mpsc::{Sender, channel},
    task::JoinHandle,
};

use crate::{
    codex,
    config::{self, SystemPrompt},
    display, file_tracking, tools, vcs,
};

lazy_static! {
    static ref AUDITOR: Mutex<Auditor> = Mutex::new(Auditor::new());
}

#[derive(Clone, Debug)]
pub struct TokenUsage {
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub known: bool,
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
        }
    }

    /// Clones the message history in the auditor
    pub fn get_messages() -> Vec<wire::types::Message> {
        AUDITOR.lock().unwrap().messages.clone()
    }

    pub fn add_message(message: wire::types::Message) {
        AUDITOR.lock().unwrap().messages.push(message);
    }

    pub fn replace_messages(messages: &Vec<wire::types::Message>) {
        AUDITOR.lock().unwrap().messages = messages.clone();
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
        }
    }

    pub fn get_total_usage() -> TokenUsage {
        let auditor = AUDITOR.lock().unwrap();
        let mut usage = TokenUsage {
            input_tokens: 0,
            output_tokens: 0,
            known: !auditor.usage_unknown,
        };

        for message in auditor.messages.iter() {
            usage.input_tokens += message.input_tokens;
            usage.output_tokens += message.output_tokens;
        }

        usage
    }

    fn mark_usage_unknown() {
        if let Ok(mut auditor) = AUDITOR.lock() {
            auditor.usage_unknown = true;
        }
    }

    /// Persist the session log + commit narrative changes (if any).
    /// Returns the session artifact that now owns the transcript for downstream plumbing.
    pub async fn commit_audit() -> Result<Option<SessionArtifact>, Box<dyn std::error::Error>> {
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

        if !file_tracking::FileTracker::has_pending_changes() {
            return Ok(session_artifact);
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

        if let Some(commit_message) = diff_message {
            display::info("Committing TODO changes...");
            file_tracking::FileTracker::commit_changes(
                &CommitMessageBuilder::new(commit_message)
                    .set_header(CommitMessageType::NarrativeChange)
                    .with_session_log_path(
                        session_artifact
                            .as_ref()
                            .map(|artifact| artifact.display_path()),
                    )
                    .build(),
            )
            .await?;

            display::info("Committed TODO changes");
        }

        Ok(session_artifact)
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
            system_prompt: Self::prompt_info(project_root, &cfg),
            model: SessionModelInfo {
                provider: if cfg.backend == config::BackendKind::Codex {
                    "codex".to_string()
                } else {
                    "wire".to_string()
                },
                name: cfg.provider_model.clone(),
                reasoning_effort: cfg
                    .reasoning_effort
                    .as_ref()
                    .map(|level| format!("{level:?}")),
            },
            messages: self.messages.clone(),
            operations: Vec::new(),
            artifacts: Vec::new(),
            outcome: SessionOutcome {
                status: "completed".to_string(),
                summary: Self::summarize_assistant(&self.messages),
            },
        })
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
            "fallback_backend": cfg.fallback_backend.map(|kind| kind.to_string()),
            "reasoning_effort": cfg
                .reasoning_effort
                .as_ref()
                .map(|level| format!("{level:?}")),
            "codex": {
                "binary": cfg.codex.binary_path.display().to_string(),
                "profile": cfg.codex.profile.clone(),
                "bounds_prompt": cfg
                    .codex
                    .bounds_prompt_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
            },
        })
    }

    fn prompt_info(project_root: &Path, cfg: &config::Config) -> SessionPromptInfo {
        let prompt_path = project_root.join(".vizier").join("BASE_SYSTEM_PROMPT.md");
        let prompt_hash = {
            let prompt = cfg.get_prompt(config::SystemPrompt::Base);
            let digest = Sha256::digest(prompt.as_bytes());
            format!("{:x}", digest)
        };

        SessionPromptInfo {
            path: prompt_path
                .exists()
                .then(|| prompt_path.strip_prefix(project_root).ok())
                .flatten()
                .map(|relative| relative.to_string_lossy().to_string()),
            hash: prompt_hash,
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
        Self::add_message(
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

            tokio::spawn(async move {
                while let Some(msg) = request_rx.recv().await {
                    tx.send(display::Status::Working(msg)).await.unwrap();
                }
            });

            Ok(prompt_wire_with_tools(
                &*crate::config::get_config().provider,
                request_tx.clone(),
                &system_prompt,
                messages.clone(),
                vec![],
            )
            .await?)
        })
        .await
        .map_err(|e| e as Box<dyn std::error::Error>)?;

        Self::replace_messages(&response);

        Ok(response.last().unwrap().clone())
    }

    /// Basic LLM request with tool usage
    /// NOTE: Returns the _entire_ conversation, up to date with the LLM's responses
    pub async fn llm_request_with_tools(
        prompt_variant: Option<SystemPrompt>,
        system_prompt: String,
        user_message: String,
        tools: Vec<wire::types::Tool>,
        codex_model: Option<codex::CodexModel>,
        repo_root_override: Option<PathBuf>,
    ) -> Result<wire::types::Message, Box<dyn std::error::Error>> {
        let cfg = crate::config::get_config();
        let backend = cfg.backend;
        let fallback_backend = cfg.fallback_backend;
        let provider = cfg.provider.clone();
        let codex_opts = cfg.codex.clone();
        let resolved_codex_model = codex_model.unwrap_or_default();

        Self::add_message(provider.new_message(user_message).as_user().build());

        let messages = AUDITOR.lock().unwrap().messages.clone();

        match backend {
            config::BackendKind::Wire => {
                let response =
                    run_wire_with_status(system_prompt, messages.clone(), tools.clone()).await?;
                Self::replace_messages(&response);
                Ok(response.last().unwrap().clone())
            }
            config::BackendKind::Codex => {
                simulate_integration_changes()?;
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
                        model: resolved_codex_model,
                        output_mode: codex::CodexOutputMode::EventsJson,
                    };

                    let response =
                        codex::run_exec(request, Some(codex::ProgressHook::Display(tx.clone())))
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
                    } else {
                        Auditor::mark_usage_unknown();
                    }
                    updated.push(assistant_message);
                    Ok(updated)
                })
                .await;

                match codex_run {
                    Ok(response) => {
                        Self::replace_messages(&response);
                        Ok(response.last().unwrap().clone())
                    }
                    Err(err) => {
                        if fallback_backend == Some(config::BackendKind::Wire) {
                            let codex_error = match err.downcast::<codex::CodexError>() {
                                Ok(e) => *e,
                                Err(other) => return Err(other as Box<dyn std::error::Error>),
                            };
                            display::warn(format!(
                                "Codex backend failed ({}); retrying with wire backend",
                                codex_error
                            ));
                            let fallback_prompt =
                                config::get_system_prompt_with_meta(prompt_variant)?;
                            let fallback_tools = if tools.is_empty() {
                                tools::get_tools()
                            } else {
                                tools.clone()
                            };
                            let response = run_wire_with_status(
                                fallback_prompt,
                                messages.clone(),
                                fallback_tools,
                            )
                            .await?;
                            Self::replace_messages(&response);
                            Ok(response.last().unwrap().clone())
                        } else {
                            Err(err as Box<dyn std::error::Error>)
                        }
                    }
                }
            }
        }
    }

    // TODO: Rectify this with the function above
    pub async fn llm_request_with_tools_no_display(
        prompt_variant: Option<SystemPrompt>,
        system_prompt: String,
        user_message: String,
        tools: Vec<wire::types::Tool>,
        stream: RequestStream,
        codex_model: Option<codex::CodexModel>,
        repo_root_override: Option<PathBuf>,
    ) -> Result<wire::types::Message, Box<dyn std::error::Error>> {
        let cfg = crate::config::get_config();
        let backend = cfg.backend;
        let fallback_backend = cfg.fallback_backend;
        let provider = cfg.provider.clone();
        let codex_opts = cfg.codex.clone();
        let resolved_codex_model = codex_model.unwrap_or_default();

        Self::add_message(provider.new_message(user_message).as_user().build());

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
                Self::add_message(last.clone());
                Ok(last)
            }
            config::BackendKind::Codex => {
                simulate_integration_changes()?;
                let repo_root = match repo_root_override {
                    Some(path) => path,
                    None => find_project_root()?.unwrap_or_else(|| PathBuf::from(".")),
                };
                let (output_mode, progress_hook) = match &stream {
                    RequestStream::Status { events, .. } => (
                        codex::CodexOutputMode::EventsJson,
                        events.clone().map(codex::ProgressHook::Plain),
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
                    model: resolved_codex_model,
                    output_mode,
                };

                match codex::run_exec(request, progress_hook).await {
                    Ok(response) => {
                        let mut assistant_message = provider
                            .new_message(response.assistant_text.clone())
                            .as_assistant()
                            .build();
                        if let Some(usage) = response.usage {
                            assistant_message.input_tokens = usage.input_tokens;
                            assistant_message.output_tokens = usage.output_tokens;
                        } else {
                            Auditor::mark_usage_unknown();
                        }
                        Self::add_message(assistant_message.clone());
                        Ok(assistant_message)
                    }
                    Err(err) => {
                        if fallback_backend == Some(config::BackendKind::Wire) {
                            display::warn(format!(
                                "Codex backend failed ({}); retrying with wire backend",
                                err
                            ));
                            let fallback_prompt =
                                config::get_system_prompt_with_meta(prompt_variant)?;
                            let fallback_tools = if tools.is_empty() {
                                tools::get_tools()
                            } else {
                                tools.clone()
                            };
                            let (fallback_tx, drain_handle) = channel_for_stream(&stream);
                            let response = prompt_wire_with_tools(
                                &*provider,
                                fallback_tx,
                                &fallback_prompt,
                                messages.clone(),
                                fallback_tools,
                            )
                            .await?;
                            if let Some(handle) = drain_handle {
                                let _ = handle.await;
                            }
                            let last = response.last().unwrap().clone();
                            Self::add_message(last.clone());
                            Ok(last)
                        } else {
                            Err(Box::new(err))
                        }
                    }
                }
            }
        }
    }
}

async fn run_wire_with_status(
    system_prompt: String,
    messages: Vec<wire::types::Message>,
    tools: Vec<wire::types::Tool>,
) -> Result<Vec<wire::types::Message>, Box<dyn std::error::Error>> {
    display::call_with_status(async move |tx| {
        tx.send(display::Status::Working("Thinking...".into()))
            .await?;

        let (request_tx, mut request_rx) = channel(10);
        tokio::spawn(async move {
            while let Some(msg) = request_rx.recv().await {
                tx.send(display::Status::Working(msg)).await.unwrap();
            }
        });

        simulate_integration_changes()?;

        Ok(prompt_wire_with_tools(
            &*crate::config::get_config().provider,
            request_tx.clone(),
            &system_prompt,
            messages.clone(),
            tools.clone(),
        )
        .await?)
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
    header: String,
    session_id: String,
    session_log_path: Option<String>,
    author_note: Option<String>,
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
    path: Option<String>,
    hash: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct SessionModelInfo {
    provider: String,
    name: String,
    reasoning_effort: Option<String>,
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
    use super::find_project_root;
    use std::fs;
    use tempfile::tempdir;

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
}

impl CommitMessageBuilder {
    pub fn new(body: String) -> Self {
        Self {
            header: "VIZIER".to_string(),
            session_id: AUDITOR.lock().unwrap().session_id.clone(),
            session_log_path: None,
            author_note: None,
            body,
        }
    }

    pub fn set_header(&mut self, message_type: CommitMessageType) -> &mut Self {
        match message_type {
            CommitMessageType::CodeChange => self.header = "VIZIER CODE CHANGE".to_string(),
            CommitMessageType::Conversation => self.header = "VIZIER CONVERSATION".to_string(),
            CommitMessageType::NarrativeChange => {
                self.header = "VIZIER NARRATIVE CHANGE".to_string()
            }
        };

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

    pub fn build(&self) -> String {
        let mut message = format!("{}\nSession ID: {}", self.header.clone(), self.session_id);

        if let Some(path) = &self.session_log_path {
            message = format!("{}\nSession Log: {}", message, path);
        }

        if let Some(an) = &self.author_note {
            message = format!("{}\nAuthor note: {}", message, an);
        }

        format!("{}\n\n{}", message, self.body)
    }
}
