use std::{ffi::OsString, path::PathBuf, process::Stdio, sync::Arc};

use serde_json::Value;
use tokio::{
    io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::Mutex,
};

use crate::{
    agent::{
        AgentDisplayAdapter, AgentError, AgentEvent, AgentFuture, AgentOutputMode, AgentRequest,
        AgentResponse, AgentRunner, ProgressHook as AgentProgressHook, humanize_event_type,
    },
    auditor::TokenUsage,
    config,
    display::{self, ProgressEvent, ProgressKind},
};

pub struct GeminiRunner;

impl AgentRunner for GeminiRunner {
    fn backend_name(&self) -> &'static str {
        "gemini"
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
pub struct GeminiDisplayAdapter;

impl AgentDisplayAdapter for GeminiDisplayAdapter {
    fn adapt(&self, event: &AgentEvent, _scope: Option<config::CommandScope>) -> ProgressEvent {
        let payload = &event.payload;
        let phase = first_string(payload, &[&["phase"], &["type"], &["event"]])
            .map(|value| humanize_event_type(&value))
            .or_else(|| Some(humanize_event_type(&event.kind)));

        let label = first_string(
            payload,
            &[
                &["label"],
                &["model"],
                &["message", "role"],
                &["role"],
                &["tool", "name"],
                &["data", "tool", "name"],
            ],
        );

        let message = first_string(
            payload,
            &[
                &["message"],
                &["message", "content"],
                &["message", "text"],
                &["data", "message", "content"],
                &["data", "message", "text"],
                &["text"],
                &["content"],
                &["result", "message"],
                &["result", "text"],
                &["result", "summary"],
                &["result"],
                &["error", "message"],
                &["data", "error", "message"],
            ],
        );

        let detail = first_string(
            payload,
            &[
                &["detail"],
                &["session_id"],
                &["session"],
                &["result", "id"],
                &["tool", "id"],
                &["data", "tool", "id"],
            ],
        );

        let path = first_string(
            payload,
            &[&["path"], &["file"], &["data", "path"], &["data", "file"]],
        );

        let progress = find_float(
            payload,
            &[
                &["progress"],
                &["result", "progress"],
                &["data", "progress"],
            ],
        );

        let status = first_string(
            payload,
            &[
                &["status"],
                &["result", "status"],
                &["message", "status"],
                &["error", "type"],
            ],
        );

        let timestamp = first_string(payload, &[&["timestamp"], &["result", "timestamp"]]);

        ProgressEvent {
            kind: ProgressKind::Agent,
            source: Some("[gemini]".to_string()),
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

fn build_agent_args(req: &AgentRequest) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("--output-format"),
        OsString::from("stream-json"),
    ];

    if let Some(profile) = &req.profile {
        if !profile.is_empty() {
            args.push(OsString::from("--profile"));
            args.push(OsString::from(profile));
        }
    }

    for extra in &req.extra_args {
        args.push(OsString::from(extra));
    }

    args
}

fn prepare_agent_command(req: &AgentRequest) -> Result<Command, AgentError> {
    let (program, base_args) = req
        .command
        .split_first()
        .ok_or(AgentError::MissingCommand)?;

    let mut command = Command::new(program);
    command.args(base_args);
    command.current_dir(&req.repo_root);
    for arg in build_agent_args(req) {
        command.arg(arg);
    }
    Ok(command)
}

pub async fn run_exec(
    req: AgentRequest,
    adapter: Arc<dyn AgentDisplayAdapter>,
    progress: Option<AgentProgressHook>,
) -> Result<AgentResponse, AgentError> {
    let scope = req.scope;
    let mut command = prepare_agent_command(&req)?;
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                let missing = req
                    .command
                    .first()
                    .map(|s| PathBuf::from(s))
                    .unwrap_or_else(|| PathBuf::from("agent"));
                return Err(AgentError::BinaryNotFound(missing));
            }

            return Err(AgentError::Spawn(err));
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(req.prompt.as_bytes()).await?;
        stdin.shutdown().await?;
    }

    let passthrough = matches!(req.output_mode, AgentOutputMode::PassthroughHuman);
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
    let mut assistant_fragments = Vec::new();
    let mut usage: Option<TokenUsage> = None;

    if let Some(stdout) = child.stdout.take() {
        let mut reader = BufReader::new(stdout).lines();
        let mut passthrough_writer = if passthrough {
            Some(io::stderr())
        } else {
            None
        };

        while let Some(line) = reader.next_line().await? {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Some(writer) = passthrough_writer.as_mut() {
                let mut outbound = line.clone();
                if !outbound.ends_with('\n') {
                    outbound.push('\n');
                }
                let _ = writer.write_all(outbound.as_bytes()).await;
                let _ = writer.flush().await;
            }

            let payload: Value = serde_json::from_str(trimmed)
                .map_err(|_| AgentError::MalformedEvent(trimmed.to_string()))?;

            let kind = payload
                .get("type")
                .and_then(Value::as_str)
                .or_else(|| payload.get("event").and_then(Value::as_str))
                .unwrap_or("unknown")
                .to_string();

            let event = AgentEvent {
                kind: kind.clone(),
                payload: payload.clone(),
            };

            if let Some(ref hook) = progress {
                let rendered = adapter.adapt(&event, scope);
                hook.send_event(rendered).await;
            }

            if usage.is_none() {
                usage = extract_usage(&payload);
            }

            if let Some(text) = extract_assistant_text(&event) {
                assistant_fragments.push(text);
            }

            events.push(event);
        }
    }

    let status = child.wait().await?;
    if let Some(handle) = stderr_handle {
        let _ = handle.await;
    }

    let stderr_summary = stderr_lines.lock().await.clone();

    if usage.is_none() {
        for line in &stderr_summary {
            if let Ok(value) = serde_json::from_str::<Value>(line) {
                usage = extract_usage(&value);
                if usage.is_some() {
                    break;
                }
            }
        }
    }

    if !status.success() {
        return Err(AgentError::NonZeroExit(
            status.code().unwrap_or(-1),
            stderr_summary,
        ));
    }

    let assistant_text = if assistant_fragments.is_empty() {
        String::new()
    } else {
        assistant_fragments.join("")
    };

    if assistant_text.trim().is_empty() {
        return Err(AgentError::MissingAssistantMessage);
    }

    Ok(AgentResponse {
        assistant_text,
        usage,
        events,
    })
}

fn extract_usage(payload: &Value) -> Option<TokenUsage> {
    let stats = payload
        .get("usage")
        .or_else(|| payload.get("stats"))
        .or_else(|| payload.pointer("/result/stats"))
        .or_else(|| payload.pointer("/data/stats"))?;

    parse_usage(stats)
}

fn parse_usage(stats: &Value) -> Option<TokenUsage> {
    let input = find_number(
        stats,
        &[
            &["input_tokens"],
            &["inputTokens"],
            &["prompt_tokens"],
            &["promptTokens"],
            &["input", "total_tokens"],
            &["input", "totalTokens"],
        ],
    );
    let output = find_number(
        stats,
        &[
            &["output_tokens"],
            &["outputTokens"],
            &["completion_tokens"],
            &["completionTokens"],
            &["output", "total_tokens"],
            &["output", "totalTokens"],
        ],
    );
    let cached = find_number(
        stats,
        &[
            &["cached_input_tokens"],
            &["cachedInputTokens"],
            &["cache_tokens"],
            &["cacheTokens"],
            &["input", "cached_input_tokens"],
            &["input", "cachedInputTokens"],
            &["input", "cachedTokens"],
        ],
    );
    let reasoning = find_number(
        stats,
        &[
            &["reasoning_output_tokens"],
            &["reasoningOutputTokens"],
            &["reasoning_tokens"],
            &["reasoningTokens"],
            &["output", "reasoning_output_tokens"],
            &["output", "reasoningOutputTokens"],
            &["output", "reasoningTokens"],
        ],
    );
    let total = find_number(
        stats,
        &[
            &["total_tokens"],
            &["totalTokens"],
            &["billed_tokens"],
            &["billedTokens"],
        ],
    )
    .or_else(|| {
        input
            .zip(output)
            .map(|(input_tokens, output_tokens)| input_tokens + output_tokens)
    });

    let has_data = input.is_some()
        || output.is_some()
        || cached.is_some()
        || reasoning.is_some()
        || total.is_some();

    has_data.then_some(TokenUsage {
        input_tokens: input.unwrap_or(0),
        cached_input_tokens: cached.unwrap_or(0),
        output_tokens: output.unwrap_or(0),
        reasoning_output_tokens: reasoning.unwrap_or(0),
        total_tokens: total.unwrap_or_else(|| input.unwrap_or(0) + output.unwrap_or(0)),
        known: true,
    })
}

fn extract_assistant_text(event: &AgentEvent) -> Option<String> {
    let payload = &event.payload;
    let role = first_string(payload, &[&["message", "role"], &["role"]]);
    let content = first_string(
        payload,
        &[
            &["message", "content"],
            &["message", "text"],
            &["data", "message", "content"],
            &["data", "message", "text"],
            &["result", "message"],
            &["result", "text"],
            &["result"],
            &["text"],
            &["content"],
            &["message"],
        ],
    );

    match event.kind.as_str() {
        "message" => {
            if role.as_deref() == Some("user") {
                return None;
            }
            return content;
        }
        "result" => {
            return content;
        }
        _ => {
            if role.as_deref() == Some("assistant") {
                return content;
            }
        }
    }

    None
}

fn first_string(payload: &Value, paths: &[&[&str]]) -> Option<String> {
    for path in paths {
        if let Some(value) = string_at(payload, path) {
            if !value.trim().is_empty() {
                return Some(value);
            }
        }
    }

    None
}

fn find_number(value: &Value, paths: &[&[&str]]) -> Option<usize> {
    for path in paths {
        if let Some(found) = at_path(value, path) {
            if let Some(num) = found.as_u64() {
                return Some(num as usize);
            } else if let Some(num) = found.as_i64() {
                if num >= 0 {
                    return Some(num as usize);
                }
            }
        }
    }

    None
}

fn find_float(value: &Value, paths: &[&[&str]]) -> Option<f64> {
    for path in paths {
        if let Some(found) = at_path(value, path) {
            if let Some(num) = found.as_f64() {
                return Some(num);
            } else if let Some(num) = found.as_u64() {
                return Some(num as f64);
            } else if let Some(num) = found.as_i64() {
                return Some(num as f64);
            }
        }
    }

    None
}

fn string_at(value: &Value, path: &[&str]) -> Option<String> {
    at_path(value, path).and_then(display::value_to_string)
}

fn at_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn adapter_maps_message_event() {
        let adapter = GeminiDisplayAdapter::default();
        let event = AgentEvent {
            kind: "message".to_string(),
            payload: json!({
                "type": "message",
                "message": {
                    "role": "assistant",
                    "content": "Hello from Gemini"
                },
                "timestamp": "2025-01-01T00:00:00Z"
            }),
        };

        let progress = adapter.adapt(&event, None);

        assert_eq!(progress.source.as_deref(), Some("[gemini]"));
        assert_eq!(progress.phase.as_deref(), Some("message"));
        assert_eq!(progress.message.as_deref(), Some("Hello from Gemini"));
        assert_eq!(progress.timestamp.as_deref(), Some("2025-01-01T00:00:00Z"));
        assert!(
            progress.raw.as_ref().unwrap().contains("Hello from Gemini"),
            "raw payload should be preserved"
        );
    }

    #[test]
    fn adapter_maps_tool_result_fields() {
        let adapter = GeminiDisplayAdapter::default();
        let event = AgentEvent {
            kind: "tool_result".to_string(),
            payload: json!({
                "type": "tool_result",
                "tool": {
                    "id": "call_1",
                    "name": "fs.read"
                },
                "result": "ok",
                "status": "success",
                "progress": 1.0
            }),
        };

        let progress = adapter.adapt(&event, None);
        let rendered =
            display::render_progress_event(&progress, display::Verbosity::Normal).join("\n");

        assert!(
            rendered.contains("[gemini]"),
            "rendered progress should include backend label"
        );
        assert_eq!(progress.detail.as_deref(), Some("call_1"));
        assert_eq!(progress.status.as_deref(), Some("success"));
        assert_eq!(progress.message.as_deref(), Some("ok"));
    }

    #[test]
    fn usage_parses_result_stats() {
        let payload = json!({
            "type": "result",
            "result": {
                "stats": {
                    "input_tokens": 10,
                    "output_tokens": 5,
                    "cached_input_tokens": 2,
                    "reasoning_output_tokens": 1,
                    "total_tokens": 16
                }
            }
        });

        let usage = extract_usage(&payload).expect("usage should be parsed");
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.cached_input_tokens, 2);
        assert_eq!(usage.reasoning_output_tokens, 1);
        assert_eq!(usage.total_tokens, 16);
        assert!(usage.known);
    }

    #[test]
    fn assistant_text_collects_from_message_events() {
        let event = AgentEvent {
            kind: "message".to_string(),
            payload: json!({
                "type": "message",
                "message": {
                    "role": "assistant",
                    "text": "partial ",
                }
            }),
        };
        let follow_up = AgentEvent {
            kind: "result".to_string(),
            payload: json!({
                "type": "result",
                "result": {
                    "text": "complete"
                }
            }),
        };

        let mut fragments = Vec::new();
        if let Some(text) = extract_assistant_text(&event) {
            fragments.push(text);
        }
        if let Some(text) = extract_assistant_text(&follow_up) {
            fragments.push(text);
        }

        assert_eq!(fragments.join(""), "partial complete");
    }
}
