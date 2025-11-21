use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use serde_json::Value;
#[cfg(feature = "mock_llm")]
use serde_json::json;
use tempfile::NamedTempFile;
use tokio::{
    io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::Mutex,
    task::JoinHandle,
};

use crate::{
    agent::ProgressHook as AgentProgressHook,
    agent::{
        AgentDisplayAdapter, AgentError, AgentEvent, AgentFuture, AgentOutputMode, AgentRequest,
        AgentResponse, AgentRunner, humanize_event_type,
    },
    auditor::TokenUsage,
    config,
    display::{self, ProgressEvent, ProgressKind},
};

pub type CodexOutputMode = AgentOutputMode;
pub type CodexRequest = AgentRequest;
pub type CodexResponse = AgentResponse;
pub type CodexEvent = AgentEvent;
pub type CodexError = AgentError;
pub use crate::agent::ProgressHook;

pub struct CodexRunner;

impl AgentRunner for CodexRunner {
    fn backend_name(&self) -> &'static str {
        "codex"
    }

    fn execute(
        &self,
        request: AgentRequest,
        adapter: Arc<dyn AgentDisplayAdapter>,
        progress_hook: Option<AgentProgressHook>,
    ) -> AgentFuture {
        Box::pin(run_exec(request, adapter, progress_hook))
    }
}

#[derive(Default)]
pub struct CodexDisplayAdapter;

impl AgentDisplayAdapter for CodexDisplayAdapter {
    fn adapt(&self, event: &AgentEvent, _scope: Option<config::CommandScope>) -> ProgressEvent {
        let payload = &event.payload;
        let phase = value_from(payload, "phase")
            .or_else(|| pointer_value(payload, "/data/phase"))
            .or_else(|| Some(humanize_event_type(&event.kind)));
        let label = value_from(payload, "label").or_else(|| pointer_value(payload, "/item/type"));
        let message = value_from(payload, "message")
            .or_else(|| pointer_value(payload, "/data/message"))
            .or_else(|| pointer_value(payload, "/item/text"));
        let detail = value_from(payload, "detail")
            .or_else(|| pointer_value(payload, "/data/detail"))
            .or_else(|| pointer_value(payload, "/item/id"));
        let path = pointer_value(payload, "/data/path")
            .or_else(|| pointer_value(payload, "/data/file"))
            .or_else(|| pointer_value(payload, "/data/target"));
        let progress = payload
            .get("progress")
            .and_then(Value::as_f64)
            .or_else(|| payload.pointer("/data/progress").and_then(Value::as_f64));
        let status =
            value_from(payload, "status").or_else(|| pointer_value(payload, "/data/status"));
        let timestamp = value_from(payload, "timestamp");

        ProgressEvent {
            kind: ProgressKind::Agent,
            source: Some("[codex]".to_string()),
            phase,
            label,
            message,
            detail,
            path,
            progress,
            status,
            timestamp,
            raw: Some(payload.to_string()),
        }
    }
}

fn build_exec_args(req: &CodexRequest, output_path: &Path) -> Vec<OsString> {
    let mut args = Vec::new();
    args.push(OsString::from("exec"));
    if let Some(model) = req.model.as_deref() {
        args.push(OsString::from("--model"));
        args.push(OsString::from(model));
    }
    if matches!(req.output_mode, CodexOutputMode::EventsJson) {
        args.push(OsString::from("--json"));
    }
    args.push(OsString::from("--output-last-message"));
    args.push(output_path.as_os_str().to_os_string());
    args.push(OsString::from("--cd"));
    args.push(req.repo_root.clone().into_os_string());

    if let Some(profile) = &req.profile {
        args.push(OsString::from("-p"));
        args.push(OsString::from(profile));
    }

    for extra in &req.extra_args {
        args.push(OsString::from(extra));
    }

    args.push(OsString::from("-"));
    args
}

#[cfg_attr(feature = "mock_llm", allow(unused_variables, unreachable_code))]
pub async fn run_exec(
    req: CodexRequest,
    adapter: Arc<dyn AgentDisplayAdapter>,
    progress: Option<AgentProgressHook>,
) -> Result<CodexResponse, CodexError> {
    let scope = req.scope;
    #[cfg(feature = "mock_llm")]
    {
        if mock_codex_failure_requested() {
            return Err(CodexError::NonZeroExit(
                42,
                vec!["forced mock agent failure".to_string()],
            ));
        }
        let response = mock_codex_response();
        if let Some(progress_hook) = progress {
            for event in &response.events {
                let rendered = adapter.adapt(event, scope);
                progress_hook.send_event(rendered).await;
            }
        }
        return Ok(response);
    }

    let _tempfile_guard = NamedTempFile::new()?;
    let output_path = _tempfile_guard.path().to_path_buf();

    let mut command = Command::new(&req.bin);
    for arg in build_exec_args(&req, &output_path) {
        command.arg(arg);
    }
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                return Err(CodexError::BinaryNotFound(req.bin.clone()));
            }

            return Err(CodexError::Spawn(err));
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(req.prompt.as_bytes()).await?;
        stdin.shutdown().await?;
    }

    let passthrough = matches!(req.output_mode, CodexOutputMode::PassthroughHuman);
    let stderr_lines = Arc::new(Mutex::new(Vec::new()));
    let stderr_handle = if let Some(stderr) = child.stderr.take() {
        let stderr_lines = stderr_lines.clone();
        Some(tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            let mut writer = if passthrough {
                Some(io::stderr())
            } else {
                None
            };
            while let Ok(Some(line)) = reader.next_line().await {
                if let Some(writer) = writer.as_mut() {
                    let mut outbound = line.clone();
                    outbound.push('\n');
                    let _ = writer.write_all(outbound.as_bytes()).await;
                    let _ = writer.flush().await;
                }

                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                display::debug(format!("[agent] {trimmed}"));
                stderr_lines.lock().await.push(trimmed);
            }
        }))
    } else {
        None
    };

    let mut events = Vec::new();
    let mut usage: Option<TokenUsage> = None;
    let mut stdout_passthrough: Option<JoinHandle<()>> = None;
    let mut stdout = child.stdout.take();

    match req.output_mode {
        CodexOutputMode::EventsJson => {
            if let Some(reader_stdout) = stdout.take() {
                let mut reader = BufReader::new(reader_stdout).lines();
                while let Some(line) = reader.next_line().await? {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    let payload: Value = serde_json::from_str(trimmed)
                        .map_err(|_| CodexError::MalformedEvent(trimmed.to_string()))?;

                    let kind = payload
                        .get("type")
                        .and_then(Value::as_str)
                        .ok_or_else(|| CodexError::MalformedEvent(trimmed.to_string()))?
                        .to_string();

                    let event = CodexEvent {
                        kind: kind.clone(),
                        payload: payload.clone(),
                    };

                    if let Some(ref hook) = progress {
                        let rendered = adapter.adapt(&event, scope);
                        hook.send_event(rendered).await;
                    }

                    if kind == "turn.completed" && usage.is_none() {
                        usage = extract_usage(&payload);
                    }

                    events.push(event);
                }
            }
        }
        CodexOutputMode::PassthroughHuman => {
            if let Some(mut stdout_stream) = stdout.take() {
                stdout_passthrough = Some(tokio::spawn(async move {
                    let mut stderr_writer = io::stderr();
                    let _ = io::copy(&mut stdout_stream, &mut stderr_writer).await;
                }));
            }
        }
    }

    let status = child.wait().await?;
    if let Some(handle) = stderr_handle {
        let _ = handle.await;
    }
    if let Some(handle) = stdout_passthrough {
        let _ = handle.await;
    }

    let stderr_summary = stderr_lines.lock().await.clone();

    if usage.is_none() {
        for line in &stderr_summary {
            if let Ok(value) = serde_json::from_str::<Value>(line) {
                if value
                    .get("type")
                    .and_then(Value::as_str)
                    .map(|kind| kind == "turn.completed")
                    .unwrap_or(false)
                {
                    usage = extract_usage(&value);
                    if usage.is_some() {
                        break;
                    }
                }
            }
        }
    }

    if !status.success() {
        if let Some(auth_message) = classify_profile_failure(&stderr_summary) {
            return Err(CodexError::ProfileAuth(auth_message));
        }

        return Err(CodexError::NonZeroExit(
            status.code().unwrap_or(-1),
            stderr_summary,
        ));
    }

    let assistant_text = std::fs::read_to_string(&output_path).unwrap_or_default();

    if assistant_text.trim().is_empty() {
        return Err(CodexError::MissingAssistantMessage);
    }

    Ok(CodexResponse {
        assistant_text,
        usage,
        events,
    })
}

fn classify_profile_failure(lines: &[String]) -> Option<String> {
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if lower.contains("profile") || lower.contains("auth") {
            return Some(line.clone());
        }
    }

    None
}

fn extract_usage(value: &Value) -> Option<TokenUsage> {
    let usage = value.get("usage")?;
    let input = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let output = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let cached_input = usage
        .get("cached_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let reasoning = usage
        .get("reasoning_output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let total = usage
        .get("total_tokens")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(input + output);

    Some(TokenUsage {
        input_tokens: input,
        cached_input_tokens: cached_input,
        output_tokens: output,
        reasoning_output_tokens: reasoning,
        total_tokens: total,
        known: true,
    })
}

fn value_from(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(|value| display::value_to_string(value))
}

fn pointer_value(payload: &Value, pointer: &str) -> Option<String> {
    payload
        .pointer(pointer)
        .and_then(|value| display::value_to_string(value))
}

#[cfg(feature = "mock_llm")]
fn mock_codex_response() -> CodexResponse {
    let suppress_usage = std::env::var("VIZIER_SUPPRESS_TOKEN_USAGE")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false);

    let usage = (!suppress_usage).then_some(TokenUsage {
        input_tokens: 10,
        cached_input_tokens: 5,
        output_tokens: 20,
        reasoning_output_tokens: 3,
        total_tokens: 30,
        known: true,
    });

    let turn_payload = if suppress_usage {
        json!({
            "type": "turn.completed",
        })
    } else {
        json!({
            "type": "turn.completed",
            "usage": {
                "input_tokens": 10,
                "cached_input_tokens": 5,
                "output_tokens": 20,
                "reasoning_output_tokens": 3,
                "total_tokens": 30
            }
        })
    };

    CodexResponse {
        assistant_text: "mock agent response".to_string(),
        usage,
        events: vec![
            CodexEvent {
                kind: "phase.update".to_string(),
                payload: json!({
                    "type": "phase.update",
                    "phase": "apply plan",
                    "message": "editing mock workspace",
                    "detail": "mock change",
                    "progress": 0.2,
                    "status": "running",
                    "timestamp": "2025-01-01T00:00:00Z",
                    "data": {
                        "path": "README.md"
                    }
                }),
            },
            CodexEvent {
                kind: "turn.completed".to_string(),
                payload: turn_payload,
            },
        ],
    }
}

#[cfg(feature = "mock_llm")]
fn mock_codex_failure_requested() -> bool {
    fn env_flag(name: &str) -> Option<bool> {
        std::env::var(name).ok().map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
    }

    env_flag("VIZIER_FORCE_AGENT_ERROR")
        .or_else(|| env_flag("VIZIER_FORCE_CODEX_ERROR"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CommandScope;
    use std::collections::BTreeMap;

    fn base_request(mode: CodexOutputMode) -> CodexRequest {
        CodexRequest {
            prompt: "prompt".to_string(),
            repo_root: PathBuf::from("/tmp/repo"),
            profile: None,
            bin: PathBuf::from("/bin/codex"),
            extra_args: vec!["--foo".to_string()],
            model: Some("gpt-5.1".to_string()),
            output_mode: mode,
            scope: None,
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn events_mode_includes_json_flag() {
        let req = base_request(CodexOutputMode::EventsJson);
        let args = build_exec_args(&req, Path::new("/tmp/out"));
        let rendered: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(rendered.iter().any(|arg| arg == "--json"));
    }

    #[test]
    fn passthrough_mode_skips_json_flag() {
        let req = base_request(CodexOutputMode::PassthroughHuman);
        let args = build_exec_args(&req, Path::new("/tmp/out"));
        let rendered: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(!rendered.iter().any(|arg| arg == "--json"));
    }

    #[test]
    fn omits_model_flag_when_model_not_set() {
        let mut req = base_request(CodexOutputMode::EventsJson);
        req.model = None;
        let args = build_exec_args(&req, Path::new("/tmp/out"));
        let rendered: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();

        assert!(
            !rendered.iter().any(|arg| arg == "--model"),
            "model flag should be absent when no model is configured: {rendered:?}"
        );
    }

    #[test]
    fn progress_event_uses_event_type_as_phase_fallback() {
        let payload = serde_json::json!({
            "type": "thread.started",
            "thread_id": "abc123"
        });
        let event = CodexEvent {
            kind: "thread.started".to_string(),
            payload,
        };

        let adapter = CodexDisplayAdapter::default();
        let progress = adapter.adapt(&event, Some(CommandScope::Ask));
        let lines =
            crate::display::render_progress_event(&progress, crate::display::Verbosity::Normal);

        assert!(
            lines[0].contains("[codex] thread started"),
            "unexpected progress line: {}",
            lines[0]
        );
    }

    #[test]
    fn progress_event_maps_item_completed_message() {
        let payload = serde_json::json!({
            "type": "item.completed",
            "item": {
                "id": "item_0",
                "type": "agent_message",
                "text": "Understood; I received your message."
            }
        });
        let event = CodexEvent {
            kind: "item.completed".to_string(),
            payload,
        };

        let adapter = CodexDisplayAdapter::default();
        let progress = adapter.adapt(&event, Some(CommandScope::Approve));
        let lines =
            crate::display::render_progress_event(&progress, crate::display::Verbosity::Normal);

        assert!(
            lines[0].contains("[codex] item completed"),
            "unexpected progress line: {}",
            lines[0]
        );
        assert!(
            lines[0].contains("Understood; I received your message."),
            "missing message in progress line: {}",
            lines[0]
        );
    }
}
