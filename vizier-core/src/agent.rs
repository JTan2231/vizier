use std::{collections::BTreeMap, fmt, future::Future, path::PathBuf, pin::Pin, sync::Arc};

use serde_json::Value;
use tokio::sync::mpsc;

use crate::{
    auditor::TokenUsage,
    config,
    display::{ProgressEvent, ProgressKind, Status},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentOutputMode {
    EventsJson,
    PassthroughHuman,
}

impl Default for AgentOutputMode {
    fn default() -> Self {
        AgentOutputMode::EventsJson
    }
}

#[derive(Debug, Clone)]
pub struct AgentRequest {
    pub prompt: String,
    pub repo_root: PathBuf,
    pub profile: Option<String>,
    pub bin: PathBuf,
    pub extra_args: Vec<String>,
    pub model: Option<String>,
    pub output_mode: AgentOutputMode,
    pub scope: Option<config::CommandScope>,
    pub metadata: BTreeMap<String, String>,
}

impl AgentRequest {
    pub fn new(prompt: String, repo_root: PathBuf) -> Self {
        Self {
            prompt,
            repo_root,
            profile: None,
            bin: PathBuf::from("codex"),
            extra_args: Vec::new(),
            model: None,
            output_mode: AgentOutputMode::default(),
            scope: None,
            metadata: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentResponse {
    pub assistant_text: String,
    pub usage: Option<TokenUsage>,
    pub events: Vec<AgentEvent>,
}

#[derive(Debug, Clone)]
pub struct ReviewCheckContext {
    pub command: String,
    pub status_code: Option<i32>,
    pub success: bool,
    pub duration_ms: u128,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone)]
pub struct AgentEvent {
    pub kind: String,
    pub payload: Value,
}

pub trait AgentDisplayAdapter: Send + Sync {
    fn adapt(&self, event: &AgentEvent, scope: Option<config::CommandScope>) -> ProgressEvent;
}

#[derive(Default)]
pub struct FallbackDisplayAdapter;

impl AgentDisplayAdapter for FallbackDisplayAdapter {
    fn adapt(&self, event: &AgentEvent, scope: Option<config::CommandScope>) -> ProgressEvent {
        let message = event
            .payload
            .get("message")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .or_else(|| {
                event
                    .payload
                    .get("detail")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
            })
            .or_else(|| Some(event.payload.to_string()));

        let source = scope
            .map(|s| format!("[wire:{}]", s.as_str()))
            .unwrap_or_else(|| "[wire]".to_string());

        ProgressEvent {
            kind: ProgressKind::Agent,
            source: Some(source),
            phase: Some(humanize_event_type(&event.kind)),
            label: None,
            message,
            detail: None,
            path: None,
            progress: None,
            status: None,
            timestamp: None,
            raw: Some(event.payload.to_string()),
        }
    }
}

pub fn humanize_event_type(kind: &str) -> String {
    let mut out = String::new();
    for part in kind.split(|c| c == '.' || c == '_') {
        if part.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(part);
    }

    if out.is_empty() {
        kind.to_string()
    } else {
        out
    }
}

#[derive(Clone)]
pub enum ProgressHook {
    Display(mpsc::Sender<Status>),
    Plain(mpsc::Sender<ProgressEvent>),
}

impl ProgressHook {
    pub async fn send_event(&self, event: ProgressEvent) {
        match self {
            ProgressHook::Display(tx) => {
                let _ = tx.send(Status::Event(event)).await;
            }
            ProgressHook::Plain(tx) => {
                let _ = tx.send(event).await;
            }
        }
    }
}

#[derive(Debug)]
pub enum AgentError {
    BinaryNotFound(PathBuf),
    Spawn(std::io::Error),
    Io(std::io::Error),
    NonZeroExit(i32, Vec<String>),
    ProfileAuth(String),
    MalformedEvent(String),
    MissingAssistantMessage,
    BoundsRead(PathBuf, std::io::Error),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::BinaryNotFound(path) => {
                write!(f, "agent backend binary not found at {}", path.display())
            }
            AgentError::Spawn(e) => write!(f, "failed spawning agent backend: {}", e),
            AgentError::Io(e) => write!(f, "I/O error: {}", e),
            AgentError::NonZeroExit(code, lines) => {
                write!(
                    f,
                    "agent backend exited with status {code}; stderr: {}",
                    lines.join("; ")
                )
            }
            AgentError::ProfileAuth(msg) => write!(f, "agent profile/auth failure: {}", msg),
            AgentError::MalformedEvent(line) => {
                write!(f, "agent backend emitted malformed JSON event: {}", line)
            }
            AgentError::MissingAssistantMessage => {
                write!(
                    f,
                    "agent backend completed without producing an assistant message"
                )
            }
            AgentError::BoundsRead(path, err) => write!(
                f,
                "failed to read agent bounds prompt at {}: {}",
                path.display(),
                err
            ),
        }
    }
}

impl std::error::Error for AgentError {}

impl From<std::io::Error> for AgentError {
    fn from(value: std::io::Error) -> Self {
        AgentError::Io(value)
    }
}

pub type AgentFuture = Pin<Box<dyn Future<Output = Result<AgentResponse, AgentError>> + Send>>;

pub trait AgentRunner: Send + Sync {
    fn backend_name(&self) -> &'static str;

    fn execute(
        &self,
        request: AgentRequest,
        adapter: Arc<dyn AgentDisplayAdapter>,
        progress_hook: Option<ProgressHook>,
    ) -> AgentFuture;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::CommandScope, display::Status};
    use serde_json::json;

    #[test]
    fn fallback_adapter_scopes_wire_events_and_humanizes_kind() {
        let adapter = FallbackDisplayAdapter::default();
        let event = AgentEvent {
            kind: "thread.started".to_string(),
            payload: json!({"message": "hello from wire"}),
        };

        let progress = adapter.adapt(&event, Some(CommandScope::Review));

        assert_eq!(progress.source.as_deref(), Some("[wire:review]"));
        assert_eq!(progress.phase.as_deref(), Some("thread started"));
        assert_eq!(progress.message.as_deref(), Some("hello from wire"));
        assert!(
            progress.raw.as_ref().is_some(),
            "raw payload should be preserved"
        );
    }

    #[test]
    fn fallback_adapter_uses_detail_when_message_missing() {
        let adapter = FallbackDisplayAdapter::default();
        let event = AgentEvent {
            kind: "item.completed".to_string(),
            payload: json!({"detail": "fallback detail"}),
        };

        let progress = adapter.adapt(&event, Some(CommandScope::Ask));

        assert_eq!(progress.message.as_deref(), Some("fallback detail"));
        assert!(progress.raw.as_ref().unwrap().contains("fallback detail"));
    }

    #[tokio::test]
    async fn progress_hook_forwards_plain_events() {
        let (tx, mut rx) = mpsc::channel(4);
        let hook = ProgressHook::Plain(tx);
        let adapter = FallbackDisplayAdapter::default();
        let event = AgentEvent {
            kind: "progress.update".to_string(),
            payload: json!({"message": "plain route"}),
        };
        let rendered = adapter.adapt(&event, Some(CommandScope::Approve));

        hook.send_event(rendered.clone()).await;
        let received = rx.recv().await.expect("event should arrive");

        assert_eq!(received.source, rendered.source);
        assert_eq!(received.message, Some("plain route".to_string()));
    }

    #[tokio::test]
    async fn progress_hook_forwards_display_status_events() {
        let (tx, mut rx) = mpsc::channel(4);
        let hook = ProgressHook::Display(tx);
        let adapter = FallbackDisplayAdapter::default();
        let event = AgentEvent {
            kind: "progress.update".to_string(),
            payload: json!({"message": "display route"}),
        };
        let rendered = adapter.adapt(&event, Some(CommandScope::Merge));

        hook.send_event(rendered.clone()).await;
        let received = rx.recv().await.expect("status should arrive");

        if let Status::Event(progress) = received {
            assert_eq!(progress.source, rendered.source);
            assert_eq!(progress.message, Some("display route".to_string()));
        } else {
            panic!("expected Status::Event");
        }
    }
}
