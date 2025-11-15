use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use serde_json::Value;
#[cfg(feature = "mock_llm")]
use serde_json::json;
use tempfile::NamedTempFile;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::Mutex,
};

use crate::{
    IMPLEMENTATION_PLAN_PROMPT,
    auditor::TokenUsage,
    config::{self, SystemPrompt},
    display::{self, Status},
    tools,
};

const DEFAULT_BOUNDS: &str = r#"You are operating inside the current Git repository working tree.
- Edit files directly (especially `.vizier/.snapshot` and TODO artifacts) instead of calling Vizier CLI commands.
- Do not invoke Vizier tools; you have full shell/file access already.
- Stay within the repo boundaries; never access parent directories or network resources unless the prompt explicitly authorizes it.
- Aggressively make changes--the story is continuously evolving.
- Every run must end with a brief summary of the narrative changes you made."#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexModel {
    Gpt5,
    Gpt5Codex,
}

impl CodexModel {
    fn as_model_name(self) -> &'static str {
        match self {
            CodexModel::Gpt5 => "gpt-5",
            CodexModel::Gpt5Codex => "gpt-5-codex",
        }
    }
}

impl Default for CodexModel {
    fn default() -> Self {
        CodexModel::Gpt5
    }
}

#[derive(Debug, Clone)]
pub struct CodexRequest {
    pub prompt: String,
    pub repo_root: PathBuf,
    pub profile: Option<String>,
    pub bin: PathBuf,
    pub extra_args: Vec<String>,
    pub model: CodexModel,
}

#[derive(Debug, Clone)]
pub struct CodexResponse {
    pub assistant_text: String,
    pub usage: Option<TokenUsage>,
    pub events: Vec<CodexEvent>,
}

#[derive(Debug, Clone)]
pub struct CodexEvent {
    pub kind: String,
    pub payload: Value,
}

#[derive(Clone)]
pub enum ProgressHook {
    Display(tokio::sync::mpsc::Sender<Status>),
    Plain(tokio::sync::mpsc::Sender<String>),
}

impl ProgressHook {
    async fn send(&self, message: String) {
        match self {
            ProgressHook::Display(tx) => {
                let _ = tx.send(Status::Working(message)).await;
            }
            ProgressHook::Plain(tx) => {
                let _ = tx.send(message).await;
            }
        }
    }
}

#[derive(Debug)]
pub enum CodexError {
    BinaryNotFound(PathBuf),
    Spawn(std::io::Error),
    Io(std::io::Error),
    NonZeroExit(i32, Vec<String>),
    ProfileAuth(String),
    MalformedEvent(String),
    MissingAssistantMessage,
    BoundsRead(PathBuf, std::io::Error),
}

impl std::fmt::Display for CodexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodexError::BinaryNotFound(path) => {
                write!(f, "Codex binary not found at {}", path.display())
            }
            CodexError::Spawn(e) => write!(f, "failed spawning Codex: {}", e),
            CodexError::Io(e) => write!(f, "I/O error: {}", e),
            CodexError::NonZeroExit(code, lines) => {
                write!(
                    f,
                    "Codex exited with status {code}; stderr: {}",
                    lines.join("; ")
                )
            }
            CodexError::ProfileAuth(msg) => write!(f, "Codex profile/auth failure: {}", msg),
            CodexError::MalformedEvent(line) => {
                write!(f, "Codex emitted malformed JSON event: {}", line)
            }
            CodexError::MissingAssistantMessage => {
                write!(f, "Codex completed without producing an assistant message")
            }
            CodexError::BoundsRead(path, err) => write!(
                f,
                "failed to read Codex bounds prompt at {}: {}",
                path.display(),
                err
            ),
        }
    }
}

impl std::error::Error for CodexError {}

impl From<std::io::Error> for CodexError {
    fn from(value: std::io::Error) -> Self {
        CodexError::Io(value)
    }
}

#[derive(Clone, Debug)]
pub struct ThreadArtifact {
    pub slug: String,
    pub body: String,
}

pub struct PromptContext {
    pub snapshot: String,
    pub threads: Vec<ThreadArtifact>,
}

pub fn gather_prompt_context() -> Result<PromptContext, CodexError> {
    let snapshot_path = PathBuf::from(format!("{}{}", tools::get_todo_dir(), ".snapshot"));
    let snapshot = std::fs::read_to_string(&snapshot_path).unwrap_or_default();
    let threads = read_thread_files(&PathBuf::from(tools::get_todo_dir()))?;

    Ok(PromptContext { snapshot, threads })
}

fn read_thread_files(dir: &Path) -> Result<Vec<ThreadArtifact>, CodexError> {
    let mut threads = Vec::new();

    if !dir.exists() {
        return Ok(threads);
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        if path
            .file_name()
            .and_then(OsStr::to_str)
            .map(|name| name == ".snapshot")
            .unwrap_or(false)
        {
            continue;
        }

        let slug = match path.file_name().and_then(OsStr::to_str) {
            Some(name) => name.to_string(),
            None => continue,
        };

        let body = std::fs::read_to_string(&path).unwrap_or_default();
        threads.push(ThreadArtifact { slug, body });
    }

    threads.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(threads)
}

pub fn build_prompt(
    snapshot: &str,
    threads: &[ThreadArtifact],
    user_input: &str,
) -> Result<String, CodexError> {
    let base_prompt = config::get_config().get_prompt(SystemPrompt::Base);
    let bounds = load_bounds_prompt()?;

    let mut prompt = String::new();
    prompt.push_str(&base_prompt);
    prompt.push_str("\n\n<codexBounds>\n");
    prompt.push_str(&bounds);
    prompt.push_str("\n</codexBounds>\n\n");

    prompt.push_str("<snapshot>\n");
    if snapshot.trim().is_empty() {
        prompt.push_str("(snapshot is currently empty)\n");
    } else {
        prompt.push_str(snapshot.trim());
        prompt.push('\n');
    }
    prompt.push_str("</snapshot>\n\n");

    prompt.push_str("<todoThreads>\n");
    if threads.is_empty() {
        prompt.push_str("(no active TODO threads)\n");
    } else {
        for thread in threads {
            prompt.push_str(&format!("### {}\n{}\n\n", thread.slug, thread.body.trim()));
        }
    }
    prompt.push_str("</todoThreads>\n\n");

    prompt.push_str("<task>\n");
    prompt.push_str(user_input.trim());
    prompt.push_str("\n</task>\n");

    Ok(prompt)
}

pub fn build_prompt_for_codex(user_input: &str) -> Result<String, CodexError> {
    let context = gather_prompt_context()?;
    build_prompt(&context.snapshot, &context.threads, user_input)
}

pub fn build_implementation_plan_prompt(
    plan_slug: &str,
    branch_name: &str,
    operator_spec: &str,
) -> Result<String, CodexError> {
    let context = gather_prompt_context()?;
    let bounds = load_bounds_prompt()?;

    let mut prompt = String::new();
    prompt.push_str(IMPLEMENTATION_PLAN_PROMPT);
    prompt.push_str("\n\n<codexBounds>\n");
    prompt.push_str(&bounds);
    prompt.push_str("\n</codexBounds>\n\n");

    prompt.push_str("<planMetadata>\n");
    prompt.push_str(&format!(
        "plan_slug: {plan_slug}\nbranch: {branch_name}\nplan_file: .vizier/implementation-plans/{plan_slug}.md\n"
    ));
    prompt.push_str("</planMetadata>\n\n");

    prompt.push_str("<snapshot>\n");
    if context.snapshot.trim().is_empty() {
        prompt.push_str("(snapshot is currently empty)\n");
    } else {
        prompt.push_str(context.snapshot.trim());
        prompt.push('\n');
    }
    prompt.push_str("</snapshot>\n\n");

    prompt.push_str("<todoThreads>\n");
    if context.threads.is_empty() {
        prompt.push_str("(no active TODO threads)\n");
    } else {
        for thread in &context.threads {
            prompt.push_str(&format!("### {}\n{}\n\n", thread.slug, thread.body.trim()));
        }
    }
    prompt.push_str("</todoThreads>\n\n");

    prompt.push_str("<operatorSpec>\n");
    prompt.push_str(operator_spec.trim());
    prompt.push('\n');
    prompt.push_str("</operatorSpec>\n");

    Ok(prompt)
}

fn load_bounds_prompt() -> Result<String, CodexError> {
    if let Some(path) = &config::get_config().codex.bounds_prompt_path {
        let contents = std::fs::read_to_string(path)
            .map_err(|err| CodexError::BoundsRead(path.clone(), err))?;
        Ok(contents)
    } else {
        Ok(DEFAULT_BOUNDS.to_string())
    }
}

pub async fn run_exec(
    req: CodexRequest,
    progress: Option<ProgressHook>,
) -> Result<CodexResponse, CodexError> {
    #[cfg(feature = "mock_llm")]
    {
        return Ok(mock_codex_response());
    }

    let _tempfile_guard = NamedTempFile::new()?;
    let output_path = _tempfile_guard.path().to_path_buf();

    let mut command = Command::new(&req.bin);
    command
        .arg("exec")
        .arg("--dangerously-bypass-approvals-and-sandbox")
        .arg("--model")
        .arg(req.model.as_model_name())
        .arg("--json")
        .arg("--output-last-message")
        .arg(&output_path)
        .arg("--cd")
        .arg(&req.repo_root);

    if let Some(profile) = &req.profile {
        command.arg("-p").arg(profile);
    }

    for extra in &req.extra_args {
        command.arg(extra);
    }

    command.arg("-");
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

    let stderr_lines = Arc::new(Mutex::new(Vec::new()));
    let stderr_handle = if let Some(stderr) = child.stderr.take() {
        let stderr_lines = stderr_lines.clone();
        Some(tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                display::debug(format!("[codex] {trimmed}"));
                stderr_lines.lock().await.push(trimmed);
            }
        }))
    } else {
        None
    };

    let mut events = Vec::new();
    let mut usage: Option<TokenUsage> = None;

    if let Some(stdout) = child.stdout.take() {
        let mut reader = BufReader::new(stdout).lines();
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
                let status_line = summarize_event(&event);
                hook.send(status_line).await;
            }

            if kind == "turn.completed" && usage.is_none() {
                usage = extract_usage(&payload);
            }

            events.push(event);
        }
    }

    let status = child.wait().await?;
    if let Some(handle) = stderr_handle {
        let _ = handle.await;
    }

    let stderr_summary = stderr_lines.lock().await.clone();

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

fn summarize_event(event: &CodexEvent) -> String {
    if let Some(message) = event
        .payload
        .get("message")
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty())
    {
        format!("{}: {}", event.kind, message)
    } else if let Some(label) = event
        .payload
        .get("label")
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty())
    {
        format!("{}: {}", event.kind, label)
    } else {
        event.kind.clone()
    }
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

    Some(TokenUsage {
        input_tokens: input,
        output_tokens: output,
        known: true,
    })
}

#[cfg(feature = "mock_llm")]
fn mock_codex_response() -> CodexResponse {
    CodexResponse {
        assistant_text: "mock codex response".to_string(),
        usage: Some(TokenUsage {
            input_tokens: 10,
            output_tokens: 20,
            known: true,
        }),
        events: vec![CodexEvent {
            kind: "turn.completed".to_string(),
            payload: json!({
                "type": "turn.completed",
                "usage": { "input_tokens": 10, "output_tokens": 20 }
            }),
        }],
    }
}
