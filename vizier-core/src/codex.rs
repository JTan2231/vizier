use std::{
    ffi::{OsStr, OsString},
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
    auditor::TokenUsage,
    config,
    display::{self, ProgressEvent, ProgressKind, Status},
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
            CodexModel::Gpt5 => "gpt-5.1",
            CodexModel::Gpt5Codex => "gpt-5.1-codex",
        }
    }
}

impl Default for CodexModel {
    fn default() -> Self {
        CodexModel::Gpt5
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexOutputMode {
    EventsJson,
    PassthroughHuman,
}

impl Default for CodexOutputMode {
    fn default() -> Self {
        CodexOutputMode::EventsJson
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
    pub output_mode: CodexOutputMode,
}

#[derive(Debug, Clone)]
pub struct CodexResponse {
    pub assistant_text: String,
    pub usage: Option<TokenUsage>,
    pub events: Vec<CodexEvent>,
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
pub struct CodexEvent {
    pub kind: String,
    pub payload: Value,
}

impl CodexEvent {
    /// Converts Codex events into the metadata that the CLI renderer relies on.
    ///
    /// Vizier currently renders `phase`, `label`, `message`, `detail`, `data.path`/`data.file`,
    /// `progress`, `status`, and `timestamp`. Keep this list updated whenever the Codex event
    /// schema grows to ensure CLI history stays stable.
    fn to_progress_event(&self) -> ProgressEvent {
        let payload = &self.payload;
        let phase = value_from(payload, "phase")
            .or_else(|| pointer_value(payload, "/data/phase"))
            .or_else(|| Some(humanize_event_type(&self.kind)));
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
            kind: ProgressKind::Codex,
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

fn humanize_event_type(kind: &str) -> String {
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
    Display(tokio::sync::mpsc::Sender<Status>),
    Plain(tokio::sync::mpsc::Sender<ProgressEvent>),
}

impl ProgressHook {
    async fn send_event(&self, event: ProgressEvent) {
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
    prompt_selection: &config::PromptSelection,
    snapshot: &str,
    threads: &[ThreadArtifact],
    user_input: &str,
    bounds_override: Option<&Path>,
) -> Result<String, CodexError> {
    let bounds = load_bounds_prompt(bounds_override)?;

    let mut prompt = String::new();
    prompt.push_str(&prompt_selection.text);
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

pub fn build_prompt_for_codex(
    prompt_selection: &config::PromptSelection,
    user_input: &str,
    bounds_override: Option<&Path>,
) -> Result<String, CodexError> {
    let context = gather_prompt_context()?;
    build_prompt(
        prompt_selection,
        &context.snapshot,
        &context.threads,
        user_input,
        bounds_override,
    )
}

pub fn build_implementation_plan_prompt(
    prompt_selection: &config::PromptSelection,
    plan_slug: &str,
    branch_name: &str,
    operator_spec: &str,
    bounds_override: Option<&Path>,
) -> Result<String, CodexError> {
    let context = gather_prompt_context()?;
    let bounds = load_bounds_prompt(bounds_override)?;

    let mut prompt = String::new();
    prompt.push_str(&prompt_selection.text);
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

pub fn build_review_prompt(
    prompt_selection: &config::PromptSelection,
    plan_slug: &str,
    branch_name: &str,
    target_branch: &str,
    plan_document: &str,
    diff_summary: &str,
    check_results: &[ReviewCheckContext],
    bounds_override: Option<&Path>,
) -> Result<String, CodexError> {
    let context = gather_prompt_context()?;
    let bounds = load_bounds_prompt(bounds_override)?;

    let mut prompt = String::new();
    prompt.push_str(&prompt_selection.text);
    prompt.push_str("\n\n<codexBounds>\n");
    prompt.push_str(&bounds);
    prompt.push_str("\n</codexBounds>\n\n");

    prompt.push_str("<planMetadata>\n");
    prompt.push_str(&format!(
        "plan_slug: {plan_slug}\nbranch: {branch_name}\ntarget_branch: {target_branch}\nplan_file: .vizier/implementation-plans/{plan_slug}.md\n"
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

    prompt.push_str("<planDocument>\n");
    if plan_document.trim().is_empty() {
        prompt.push_str("(plan document appears empty)\n");
    } else {
        prompt.push_str(plan_document.trim());
        prompt.push('\n');
    }
    prompt.push_str("</planDocument>\n\n");

    prompt.push_str("<diffSummary>\n");
    if diff_summary.trim().is_empty() {
        prompt.push_str("(diff between plan branch and target branch was empty or unavailable)\n");
    } else {
        prompt.push_str(diff_summary.trim());
        prompt.push('\n');
    }
    prompt.push_str("</diffSummary>\n\n");

    prompt.push_str("<checkResults>\n");
    if check_results.is_empty() {
        prompt.push_str("No review checks were executed before this critique.\n");
    } else {
        for check in check_results {
            let status_label = if check.success { "success" } else { "failure" };
            let status_code = check
                .status_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_string());
            prompt.push_str(&format!(
                "### Command: {}\nstatus: {} (code={})\nduration_ms: {}\nstdout:\n{}\n\nstderr:\n{}\n\n",
                check.command.trim(),
                status_label,
                status_code,
                check.duration_ms,
                check.stdout.trim(),
                check.stderr.trim(),
            ));
        }
    }
    prompt.push_str("</checkResults>\n");

    Ok(prompt)
}

pub fn build_merge_conflict_prompt(
    prompt_selection: &config::PromptSelection,
    target_branch: &str,
    source_branch: &str,
    conflicts: &[String],
    bounds_override: Option<&Path>,
) -> Result<String, CodexError> {
    let context = gather_prompt_context()?;
    let bounds = load_bounds_prompt(bounds_override)?;

    let mut prompt = String::new();
    prompt.push_str(&prompt_selection.text);
    prompt.push_str("\n\n<codexBounds>\n");
    prompt.push_str(&bounds);
    prompt.push_str("\n</codexBounds>\n\n");

    prompt.push_str("<mergeContext>\n");
    prompt.push_str(&format!(
        "target_branch: {target_branch}\nsource_branch: {source_branch}\n"
    ));
    prompt.push_str("conflict_files:\n");
    if conflicts.is_empty() {
        prompt.push_str("- (conflicts were detected but no file list was provided)\n");
    } else {
        for file in conflicts {
            prompt.push_str(&format!("- {file}\n"));
        }
    }
    prompt.push_str("</mergeContext>\n\n");

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
    prompt.push_str("</todoThreads>\n");

    Ok(prompt)
}

pub fn build_cicd_failure_prompt(
    plan_slug: &str,
    plan_branch: &str,
    target_branch: &str,
    script_path: &Path,
    attempt: u32,
    max_attempts: u32,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    bounds_override: Option<&Path>,
) -> Result<String, CodexError> {
    let context = gather_prompt_context()?;
    let bounds = load_bounds_prompt(bounds_override)?;

    let mut prompt = String::new();
    prompt.push_str("You are assisting after `vizier merge` ran the repository's CI/CD gate script and it failed. Diagnose the failure using the captured output, make the minimal scoped edits needed for the script to pass, update `.vizier/.snapshot` plus TODO threads when behavior changes, and never delete or bypass the gate. Provide a concise summary of the fixes you applied.\n\n");

    prompt.push_str("<codexBounds>\n");
    prompt.push_str(&bounds);
    prompt.push_str("\n</codexBounds>\n\n");

    prompt.push_str("<planMetadata>\n");
    prompt.push_str(&format!(
        "plan_slug: {plan_slug}\nplan_branch: {plan_branch}\ntarget_branch: {target_branch}\n"
    ));
    prompt.push_str("</planMetadata>\n\n");

    prompt.push_str("<cicdContext>\n");
    prompt.push_str(&format!(
        "script_path: {}\nattempt: {}\nmax_attempts: {}\nexit_code: {}\n",
        script_path.display(),
        attempt,
        max_attempts,
        exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string())
    ));
    prompt.push_str("</cicdContext>\n\n");

    prompt.push_str("<gateOutput>\nstdout:\n");
    if stdout.trim().is_empty() {
        prompt.push_str("(stdout was empty)\n");
    } else {
        prompt.push_str(stdout.trim());
        prompt.push('\n');
    }
    prompt.push_str("\nstderr:\n");
    if stderr.trim().is_empty() {
        prompt.push_str("(stderr was empty)\n");
    } else {
        prompt.push_str(stderr.trim());
        prompt.push('\n');
    }
    prompt.push_str("</gateOutput>\n\n");

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

    Ok(prompt)
}

fn load_bounds_prompt(bounds_override: Option<&Path>) -> Result<String, CodexError> {
    if let Some(path) = bounds_override {
        let contents = std::fs::read_to_string(path)
            .map_err(|err| CodexError::BoundsRead(path.to_path_buf(), err))?;
        return Ok(contents);
    }

    if let Some(path) = &config::get_config().codex.bounds_prompt_path {
        let contents = std::fs::read_to_string(path)
            .map_err(|err| CodexError::BoundsRead(path.clone(), err))?;
        Ok(contents)
    } else {
        Ok(DEFAULT_BOUNDS.to_string())
    }
}

fn build_exec_args(req: &CodexRequest, output_path: &Path) -> Vec<OsString> {
    let mut args = Vec::new();
    args.push(OsString::from("exec"));
    args.push(OsString::from("--model"));
    args.push(OsString::from(req.model.as_model_name()));
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
    progress: Option<ProgressHook>,
) -> Result<CodexResponse, CodexError> {
    #[cfg(feature = "mock_llm")]
    {
        if mock_codex_failure_requested() {
            return Err(CodexError::NonZeroExit(
                42,
                vec!["forced mock Codex failure".to_string()],
            ));
        }
        let response = mock_codex_response();
        if let Some(progress_hook) = progress {
            for event in &response.events {
                progress_hook.send_event(event.to_progress_event()).await;
            }
        }
        let _ = req;
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
                display::debug(format!("[codex] {trimmed}"));
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
                        hook.send_event(event.to_progress_event()).await;
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
        assistant_text: "mock codex response".to_string(),
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
    std::env::var("VIZIER_FORCE_CODEX_ERROR")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{self, CommandScope, PromptKind};
    use std::sync::Mutex;

    fn base_request(mode: CodexOutputMode) -> CodexRequest {
        CodexRequest {
            prompt: "prompt".to_string(),
            repo_root: PathBuf::from("/tmp/repo"),
            profile: None,
            bin: PathBuf::from("/bin/codex"),
            extra_args: vec!["--foo".to_string()],
            model: CodexModel::Gpt5,
            output_mode: mode,
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
    fn progress_event_uses_event_type_as_phase_fallback() {
        let payload = serde_json::json!({
            "type": "thread.started",
            "thread_id": "abc123"
        });
        let event = CodexEvent {
            kind: "thread.started".to_string(),
            payload,
        };

        let progress = event.to_progress_event();
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

        let progress = event.to_progress_event();
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

    static CONFIG_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn implementation_plan_prompt_respects_override() {
        let _guard = CONFIG_LOCK.lock().unwrap();
        let original = config::get_config();
        let mut cfg = original.clone();
        cfg.set_prompt(PromptKind::ImplementationPlan, "custom plan".to_string());
        config::set_config(cfg);

        let selection =
            config::get_config().prompt_for(CommandScope::Draft, PromptKind::ImplementationPlan);
        let prompt =
            build_implementation_plan_prompt(&selection, "slug", "draft/slug", "spec", None)
                .unwrap();

        assert!(prompt.starts_with("custom plan"));
        assert!(prompt.contains("<codexBounds>"));

        config::set_config(original);
    }

    #[test]
    fn review_prompt_respects_override() {
        let _guard = CONFIG_LOCK.lock().unwrap();
        let original = config::get_config();
        let mut cfg = original.clone();
        cfg.set_prompt(PromptKind::Review, "custom review".to_string());
        config::set_config(cfg);

        let selection = config::get_config().prompt_for(CommandScope::Review, PromptKind::Review);
        let prompt = build_review_prompt(
            &selection,
            "slug",
            "draft/slug",
            "main",
            "plan",
            "diff",
            &[],
            None,
        )
        .unwrap();

        assert!(prompt.starts_with("custom review"));
        assert!(prompt.contains("<planDocument>"));

        config::set_config(original);
    }

    #[test]
    fn merge_conflict_prompt_respects_override() {
        let _guard = CONFIG_LOCK.lock().unwrap();
        let original = config::get_config();
        let mut cfg = original.clone();
        cfg.set_prompt(PromptKind::MergeConflict, "custom merge".to_string());
        config::set_config(cfg);

        let conflicts = vec!["src/lib.rs".to_string()];
        let selection =
            config::get_config().prompt_for(CommandScope::Merge, PromptKind::MergeConflict);
        let prompt =
            build_merge_conflict_prompt(&selection, "main", "draft/slug", &conflicts, None)
                .unwrap();

        assert!(prompt.starts_with("custom merge"));
        assert!(prompt.contains("<mergeContext>"));

        config::set_config(original);
    }
}
