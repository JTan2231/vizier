#[cfg(not(feature = "mock_llm"))]
use std::time::Instant;
use std::{
    collections::BTreeMap,
    fmt,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    time::Duration,
};

#[cfg(not(feature = "mock_llm"))]
use std::process::Stdio;

use tokio::{
    io::{self, AsyncBufRead, AsyncBufReadExt},
    sync::mpsc,
};

#[cfg(not(feature = "mock_llm"))]
use tokio::{
    io::{AsyncWriteExt, BufReader},
    process::Command,
    time,
};

use crate::{
    config,
    display::{self, ProgressEvent, ProgressKind, Status},
};

#[derive(Debug, Clone)]
pub struct AgentRequest {
    pub prompt: String,
    pub repo_root: PathBuf,
    pub command: Vec<String>,
    pub progress_filter: Option<Vec<String>>,
    pub output: config::AgentOutputHandling,
    pub allow_script_wrapper: bool,
    pub scope: Option<config::CommandScope>,
    pub metadata: BTreeMap<String, String>,
    pub timeout: Option<Duration>,
}

impl AgentRequest {
    pub fn new(prompt: String, repo_root: PathBuf) -> Self {
        Self {
            prompt,
            repo_root,
            command: Vec::new(),
            progress_filter: None,
            output: config::AgentOutputHandling::Wrapped,
            allow_script_wrapper: false,
            scope: None,
            metadata: BTreeMap::new(),
            timeout: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentResponse {
    pub assistant_text: String,
    pub stderr: Vec<String>,
    pub exit_code: i32,
    pub duration_ms: u128,
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

#[derive(Debug, Clone, Copy)]
pub enum ReviewGateStatus {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone)]
pub struct ReviewGateContext {
    pub script: Option<String>,
    pub status: ReviewGateStatus,
    pub attempts: u32,
    pub duration_ms: Option<u128>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub auto_resolve_enabled: bool,
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
    MissingCommand,
    Spawn(std::io::Error),
    Io(std::io::Error),
    NonZeroExit(i32, Vec<String>),
    Timeout(u64),
    BoundsRead(PathBuf, std::io::Error),
    MissingPrompt(config::PromptKind),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::BinaryNotFound(path) => {
                write!(f, "agent command not found at {}", path.display())
            }
            AgentError::MissingCommand => write!(f, "agent command was not provided"),
            AgentError::Spawn(e) => write!(f, "failed spawning agent command: {}", e),
            AgentError::Io(e) => write!(f, "I/O error: {}", e),
            AgentError::NonZeroExit(code, lines) => {
                write!(
                    f,
                    "agent command exited with status {code}; stderr: {}",
                    lines.join("; ")
                )
            }
            AgentError::Timeout(secs) => {
                write!(f, "agent command exceeded timeout after {secs}s")
            }
            AgentError::BoundsRead(path, err) => {
                write!(
                    f,
                    "failed to read agent bounds prompt at {}: {}",
                    path.display(),
                    err
                )
            }
            AgentError::MissingPrompt(kind) => {
                write!(
                    f,
                    "no prompt template was resolved for kind `{}`",
                    kind.as_str()
                )
            }
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

    fn execute(&self, request: AgentRequest, progress_hook: Option<ProgressHook>) -> AgentFuture;
}

pub struct ScriptRunner;

impl ScriptRunner {
    #[cfg(all(unix, not(feature = "mock_llm")))]
    fn should_use_stdbuf() -> bool {
        let disabled = std::env::var("VIZIER_DISABLE_STDBUF")
            .ok()
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes"
                )
            })
            .unwrap_or(false);

        !disabled
    }

    #[cfg(any(feature = "mock_llm", not(unix)))]
    #[allow(dead_code)]
    fn should_use_stdbuf() -> bool {
        false
    }

    #[cfg(all(unix, not(feature = "mock_llm")))]
    fn buffering_wrappers(allow_script: bool) -> Vec<Vec<String>> {
        let mut wrappers = vec![
            vec!["stdbuf".to_string(), "-oL".to_string(), "-eL".to_string()],
            vec!["unbuffer".to_string(), "-p".to_string()],
        ];
        if allow_script && display::get_display_config().stdout_is_tty {
            wrappers.push(vec![
                "script".to_string(),
                "-q".to_string(),
                "/dev/null".to_string(),
            ]);
        }
        wrappers
    }

    #[cfg(all(unix, not(feature = "mock_llm")))]
    fn wrapper_label(wrapper: &[String]) -> &'static str {
        match wrapper.first().map(String::as_str) {
            Some("stdbuf") => "stdbuf",
            Some("unbuffer") => "unbuffer",
            Some("script") => "script",
            _ => "wrapper",
        }
    }

    #[cfg(not(feature = "mock_llm"))]
    fn configure_stdio(cmd: &mut Command) {
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
    }

    fn render_source(
        scope: Option<config::CommandScope>,
        metadata: &BTreeMap<String, String>,
    ) -> String {
        let label = metadata
            .get("agent_label")
            .cloned()
            .or_else(|| metadata.get("agent_command").cloned())
            .unwrap_or_else(|| "agent".to_string());

        match scope {
            Some(scope) => format!("[{label}:{}]", scope.as_str()),
            None => format!("[{label}]"),
        }
    }

    #[cfg_attr(feature = "mock_llm", allow(dead_code))]
    fn command_label(command: &[String]) -> Option<String> {
        let program = command.first()?;
        let stem = Path::new(program)
            .file_stem()
            .map(|value| value.to_string_lossy().to_string())?;
        if stem.is_empty() { None } else { Some(stem) }
    }

    #[cfg_attr(feature = "mock_llm", allow(dead_code))]
    async fn read_progress_stream(
        reader: impl AsyncBufRead + Unpin,
        source: String,
        progress_hook: Option<ProgressHook>,
        capture_raw: bool,
    ) -> io::Result<(Vec<String>, String)> {
        let mut lines = Vec::new();
        let mut raw = String::new();
        let verbosity = display::get_display_config().verbosity;

        let mut stream = reader.lines();
        while let Some(line) = stream.next_line().await? {
            if capture_raw {
                raw.push_str(&line);
                raw.push('\n');
            }
            let trimmed = line.trim().to_string();
            if trimmed.is_empty() {
                continue;
            }

            if let Some(ref hook) = progress_hook {
                let event = ProgressEvent {
                    kind: ProgressKind::Agent,
                    source: Some(source.clone()),
                    phase: None,
                    label: None,
                    message: Some(trimmed.clone()),
                    detail: None,
                    path: None,
                    progress: None,
                    status: None,
                    timestamp: None,
                    raw: None,
                };
                hook.send_event(event).await;
            } else if !matches!(verbosity, display::Verbosity::Quiet) {
                let event = ProgressEvent {
                    kind: ProgressKind::Agent,
                    source: Some(source.clone()),
                    phase: None,
                    label: None,
                    message: Some(trimmed.clone()),
                    detail: None,
                    path: None,
                    progress: None,
                    status: None,
                    timestamp: None,
                    raw: None,
                };
                for line in display::render_progress_event(&event, verbosity) {
                    eprintln!("{line}");
                }
            }

            lines.push(trimmed);
        }

        Ok((lines, raw))
    }

    #[cfg_attr(feature = "mock_llm", allow(dead_code))]
    async fn read_stderr(
        reader: impl AsyncBufRead + Unpin,
        source: String,
        progress_hook: Option<ProgressHook>,
    ) -> io::Result<Vec<String>> {
        let (lines, _) = Self::read_progress_stream(reader, source, progress_hook, false).await?;
        Ok(lines)
    }

    #[cfg_attr(feature = "mock_llm", allow(dead_code))]
    async fn read_filter_stdout(
        reader: impl AsyncBufRead + Unpin,
        source: String,
        progress_hook: Option<ProgressHook>,
    ) -> io::Result<(Vec<String>, String)> {
        Self::read_progress_stream(reader, source, progress_hook, true).await
    }
}

impl AgentRunner for ScriptRunner {
    fn backend_name(&self) -> &'static str {
        "script"
    }

    fn execute(&self, request: AgentRequest, progress_hook: Option<ProgressHook>) -> AgentFuture {
        Box::pin(async move {
            #[cfg(feature = "mock_llm")]
            {
                if std::env::var("VIZIER_FORCE_AGENT_ERROR")
                    .ok()
                    .map(|value| {
                        matches!(
                            value.trim().to_ascii_lowercase().as_str(),
                            "1" | "true" | "yes"
                        )
                    })
                    .unwrap_or(false)
                {
                    return Err(AgentError::NonZeroExit(
                        42,
                        vec!["forced mock agent failure".to_string()],
                    ));
                }

                if let Some(ref hook) = progress_hook {
                    let event = ProgressEvent {
                        kind: ProgressKind::Agent,
                        source: Some(Self::render_source(request.scope, &request.metadata)),
                        phase: None,
                        label: None,
                        message: Some("mock agent running".to_string()),
                        detail: None,
                        path: None,
                        progress: None,
                        status: None,
                        timestamp: None,
                        raw: None,
                    };
                    hook.send_event(event).await;
                }

                return Ok(AgentResponse {
                    assistant_text: "mock agent response".to_string(),
                    stderr: vec!["mock stderr".to_string()],
                    exit_code: 0,
                    duration_ms: 10,
                });
            }

            #[cfg(not(feature = "mock_llm"))]
            {
                let mut spawn_warnings = Vec::new();
                let (program, base_args) = request
                    .command
                    .split_first()
                    .ok_or(AgentError::MissingCommand)?;

                let mut command = Command::new(program);
                command.args(base_args);
                command.current_dir(&request.repo_root);
                Self::configure_stdio(&mut command);

                #[cfg(all(unix, not(feature = "mock_llm")))]
                let allow_script_wrapper = {
                    let needs_progress_streaming = progress_hook.is_some()
                        || matches!(request.output, config::AgentOutputHandling::Wrapped);
                    if request.allow_script_wrapper && needs_progress_streaming {
                        spawn_warnings.push(
                            "skipping script wrapper to preserve stderr for progress output"
                                .to_string(),
                        );
                        false
                    } else {
                        request.allow_script_wrapper
                    }
                };

                #[cfg(any(feature = "mock_llm", not(unix)))]
                let allow_script_wrapper = request.allow_script_wrapper;

                #[cfg(all(unix, not(feature = "mock_llm")))]
                let mut child = {
                    let mut spawned: Option<tokio::process::Child> = None;
                    if Self::should_use_stdbuf() {
                        for wrapper in Self::buffering_wrappers(allow_script_wrapper) {
                            let mut wrapped = Command::new(&wrapper[0]);
                            wrapped.args(&wrapper[1..]);
                            wrapped.args(&request.command);
                            wrapped.current_dir(&request.repo_root);
                            Self::configure_stdio(&mut wrapped);
                            match wrapped.spawn() {
                                Ok(child) => {
                                    spawned = Some(child);
                                    break;
                                }
                                Err(err) => {
                                    if err.kind() == std::io::ErrorKind::NotFound {
                                        spawn_warnings.push(format!(
                                            "{} not found; attempting next buffering wrapper",
                                            Self::wrapper_label(&wrapper)
                                        ));
                                        continue;
                                    }
                                    return Err(AgentError::Spawn(err));
                                }
                            }
                        }
                    }

                    match spawned {
                        Some(child) => child,
                        None => match command.spawn() {
                            Ok(child) => child,
                            Err(err) => {
                                if err.kind() == std::io::ErrorKind::NotFound {
                                    let missing = request
                                        .command
                                        .first()
                                        .map(PathBuf::from)
                                        .unwrap_or_else(|| PathBuf::from("agent"));
                                    return Err(AgentError::BinaryNotFound(missing));
                                }
                                return Err(AgentError::Spawn(err));
                            }
                        },
                    }
                };

                #[cfg(any(feature = "mock_llm", not(unix)))]
                let mut child = match command.spawn() {
                    Ok(child) => child,
                    Err(err) => {
                        if err.kind() == std::io::ErrorKind::NotFound {
                            let missing = request
                                .command
                                .first()
                                .map(PathBuf::from)
                                .unwrap_or_else(|| PathBuf::from("agent"));
                            return Err(AgentError::BinaryNotFound(missing));
                        }
                        return Err(AgentError::Spawn(err));
                    }
                };

                let start = Instant::now();
                let source = request
                    .metadata
                    .get("agent_label")
                    .cloned()
                    .or_else(|| Self::command_label(&request.command))
                    .map(|label| {
                        Self::render_source(request.scope, &{
                            let mut meta = request.metadata.clone();
                            meta.insert("agent_label".to_string(), label.clone());
                            meta
                        })
                    })
                    .unwrap_or_else(|| Self::render_source(request.scope, &request.metadata));

                let source_for_stderr = source.clone();
                let stderr_handle = if let Some(stderr) = child.stderr.take() {
                    let hook = progress_hook.clone();
                    Some(tokio::spawn(async move {
                        Self::read_stderr(BufReader::new(stderr), source_for_stderr, hook).await
                    }))
                } else {
                    None
                };

                // Optional progress filter pipeline; Rust treats filter stdout as final text when present.
                let mut filter_child = None;
                let mut filter_stdin: Option<tokio::process::ChildStdin> = None;
                let mut filter_stdout_handle = None;
                let mut filter_stderr_handle = None;

                if let Some(filter_cmd) = request.progress_filter.clone() {
                    if filter_cmd.is_empty() {
                        return Err(AgentError::Io(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "progress filter command cannot be empty",
                        )));
                    }

                    let (filter_program, filter_args) =
                        filter_cmd.split_first().ok_or(AgentError::MissingCommand)?;
                    let mut filter = Command::new(filter_program);
                    filter.args(filter_args);
                    filter.current_dir(&request.repo_root);
                    Self::configure_stdio(&mut filter);

                    #[cfg(all(unix, not(feature = "mock_llm")))]
                    let mut spawned_filter = {
                        let mut spawned: Option<tokio::process::Child> = None;
                        if Self::should_use_stdbuf() {
                            for wrapper in Self::buffering_wrappers(allow_script_wrapper) {
                                let mut wrapped = Command::new(&wrapper[0]);
                                wrapped.args(&wrapper[1..]);
                                wrapped.args(filter_cmd.clone());
                                wrapped.current_dir(&request.repo_root);
                                Self::configure_stdio(&mut wrapped);
                                match wrapped.spawn() {
                                    Ok(child) => {
                                        spawned = Some(child);
                                        break;
                                    }
                                    Err(err) => {
                                        if err.kind() == std::io::ErrorKind::NotFound {
                                            spawn_warnings.push(format!(
                                                "{} not found; attempting next buffering wrapper for progress filter",
                                                Self::wrapper_label(&wrapper)
                                            ));
                                            continue;
                                        }
                                        return Err(AgentError::Io(io::Error::new(
                                            io::ErrorKind::Other,
                                            format!(
                                                "failed to spawn progress filter `{}`: {}",
                                                filter_program, err
                                            ),
                                        )));
                                    }
                                }
                            }
                        }

                        match spawned {
                            Some(child) => child,
                            None => match filter.spawn() {
                                Ok(child) => child,
                                Err(err) => {
                                    return Err(AgentError::Io(io::Error::new(
                                        io::ErrorKind::Other,
                                        format!(
                                            "failed to spawn progress filter `{}`: {}",
                                            filter_program, err
                                        ),
                                    )));
                                }
                            },
                        }
                    };

                    #[cfg(any(feature = "mock_llm", not(unix)))]
                    let mut spawned_filter = match filter.spawn() {
                        Ok(child) => child,
                        Err(err) => {
                            return Err(AgentError::Io(io::Error::new(
                                io::ErrorKind::Other,
                                format!(
                                    "failed to spawn progress filter `{}`: {}",
                                    filter_program, err
                                ),
                            )));
                        }
                    };

                    let filter_source = source.clone();
                    if let Some(stdout) = spawned_filter.stdout.take() {
                        let hook = progress_hook.clone();
                        filter_stdout_handle = Some(tokio::spawn(async move {
                            Self::read_filter_stdout(
                                BufReader::new(stdout),
                                filter_source,
                                hook,
                            )
                            .await
                        }));
                    }

                    if let Some(stderr) = spawned_filter.stderr.take() {
                        let hook = progress_hook.clone();
                        let filter_source_err = source.clone();
                        filter_stderr_handle = Some(tokio::spawn(async move {
                            Self::read_stderr(BufReader::new(stderr), filter_source_err, hook).await
                        }));
                    }

                    filter_stdin = spawned_filter.stdin.take();
                    filter_child = Some(spawned_filter);
                }

                let stdout_handle = if let Some(stdout) = child.stdout.take() {
                    let mut writer = filter_stdin.take();
                    Some(tokio::spawn(async move {
                        let mut lines = BufReader::new(stdout).lines();
                        let mut buffer = String::new();

                        while let Some(line) = lines.next_line().await? {
                            if let Some(ref mut pipe) = writer {
                                pipe.write_all(line.as_bytes()).await?;
                                pipe.write_all(b"\n").await?;
                                let _ = pipe.flush().await;
                            }
                            buffer.push_str(&line);
                            buffer.push('\n');
                        }

                        if let Some(mut pipe) = writer {
                            let _ = pipe.shutdown().await;
                        }

                        Ok::<String, io::Error>(buffer)
                    }))
                } else {
                    None
                };

                if let Some(mut stdin) = child.stdin.take() {
                    stdin.write_all(request.prompt.as_bytes()).await?;
                    stdin.shutdown().await?;
                }

                let wait_future = child.wait();
                let status = if let Some(timeout) = request.timeout {
                    match time::timeout(timeout, wait_future).await {
                        Ok(result) => result?,
                        Err(_) => {
                            let _ = child.kill().await;
                            if let Some(mut filter) = filter_child.take() {
                                let _ = filter.kill().await;
                            }
                            let secs = timeout.as_secs();
                            return Err(AgentError::Timeout(secs));
                        }
                    }
                } else {
                    wait_future.await?
                };

                let duration_ms = start.elapsed().as_millis();

                let mut stderr_lines = Vec::new();
                if let Some(handle) = stderr_handle {
                    stderr_lines.extend(handle.await.unwrap_or_else(|_| Ok(Vec::new()))?);
                }

                let mut agent_stdout = String::new();
                if let Some(handle) = stdout_handle {
                    agent_stdout = handle.await.unwrap_or_else(|_| Ok(String::new()))?;
                }

                let mut filter_stdout_lines: Vec<String> = Vec::new();
                let mut filter_stdout_raw = String::new();
                if let Some(handle) = filter_stdout_handle {
                    (filter_stdout_lines, filter_stdout_raw) = handle
                        .await
                        .unwrap_or_else(|_| Ok((Vec::new(), String::new())))?;
                }

                if let Some(handle) = filter_stderr_handle {
                    stderr_lines.extend(handle.await.unwrap_or_else(|_| Ok(Vec::new()))?);
                }

                if let Some(mut filter) = filter_child {
                    let filter_status = filter.wait().await?;
                    if !filter_status.success() {
                        if !filter_stdout_lines.is_empty() {
                            stderr_lines.extend(filter_stdout_lines.clone());
                        } else if !filter_stdout_raw.is_empty() {
                            stderr_lines.extend(
                                filter_stdout_raw
                                    .lines()
                                    .map(|line| line.to_string())
                                    .collect::<Vec<_>>(),
                            );
                        }
                        return Err(AgentError::NonZeroExit(
                            filter_status.code().unwrap_or(-1),
                            stderr_lines,
                        ));
                    }
                }

                if !spawn_warnings.is_empty() {
                    stderr_lines.extend(spawn_warnings.clone());
                }

                if !status.success() {
                    return Err(AgentError::NonZeroExit(
                        status.code().unwrap_or(-1),
                        stderr_lines,
                    ));
                }

                let assistant_text = if !filter_stdout_raw.is_empty() {
                    filter_stdout_raw
                } else if !filter_stdout_lines.is_empty() {
                    filter_stdout_lines.join("\n")
                } else {
                    agent_stdout
                };

                Ok(AgentResponse {
                    assistant_text,
                    stderr: stderr_lines,
                    exit_code: status.code().unwrap_or(0),
                    duration_ms,
                })
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CommandScope;
    use tokio::sync::mpsc;

    #[cfg(not(feature = "mock_llm"))]
    #[tokio::test]
    async fn renders_progress_events_for_stderr() {
        let runner = ScriptRunner;
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("echo.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf 'line1\\n' 1>&2\nprintf 'done' > \"$1\"\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }

        let (tx, mut rx) = mpsc::channel(4);
        let request = AgentRequest {
            prompt: "ignored".to_string(),
            repo_root: tmp.path().to_path_buf(),
            command: vec![
                script.display().to_string(),
                tmp.path().join("out.txt").display().to_string(),
            ],
            progress_filter: None,
            output: config::AgentOutputHandling::Wrapped,
            allow_script_wrapper: false,
            scope: Some(CommandScope::Ask),
            metadata: BTreeMap::new(),
            timeout: Some(Duration::from_secs(5)),
        };

        let result = runner
            .execute(request, Some(ProgressHook::Plain(tx)))
            .await
            .expect("script should succeed");
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.assistant_text, "");
        assert!(rx.recv().await.is_some(), "expected progress event");
    }

    #[cfg(not(feature = "mock_llm"))]
    #[tokio::test]
    async fn streams_wrapped_json_with_progress_filter() {
        let runner = ScriptRunner;
        let tmp = tempfile::tempdir().unwrap();
        let agent = tmp.path().join("agent.sh");
        std::fs::write(
            &agent,
            r#"#!/bin/sh
echo '{"type":"item.started","item":{"type":"reasoning","text":"prep"}}'
sleep 0.3
echo '{"type":"item.completed","item":{"type":"agent_message","text":"final text"}}'
"#,
        )
        .unwrap();

        let filter = tmp.path().join("filter.sh");
        std::fs::write(
            &filter,
            r#"#!/bin/sh
last=""
while IFS= read -r line; do
  printf 'progress:%s\n' "$line" 1>&2
  last="$line"
done
printf '%s\n' "$last"
"#,
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for script in [&agent, &filter] {
                let mut perms = std::fs::metadata(script).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(script, perms).unwrap();
            }
        }

        let (tx, mut rx) = mpsc::channel(4);
        let request = AgentRequest {
            prompt: "prompt".to_string(),
            repo_root: tmp.path().to_path_buf(),
            command: vec![agent.display().to_string()],
            progress_filter: Some(vec![filter.display().to_string()]),
            output: config::AgentOutputHandling::Wrapped,
            allow_script_wrapper: true,
            scope: Some(CommandScope::Ask),
            metadata: BTreeMap::new(),
            timeout: Some(Duration::from_secs(2)),
        };

        let handle =
            tokio::spawn(
                async move { runner.execute(request, Some(ProgressHook::Plain(tx))).await },
            );

        let first_event = tokio::time::timeout(Duration::from_millis(1000), rx.recv())
            .await
            .expect("progress should arrive before completion");
        assert!(
            first_event.is_some(),
            "expected at least one progress event"
        );

        let response = handle.await.expect("task should run").expect("agent run");
        assert!(
            response.assistant_text.contains("final text"),
            "expected final text in stdout"
        );
        assert!(
            response
                .stderr
                .iter()
                .any(|line| line.contains("progress:")),
            "expected progress filter output to be captured"
        );
    }

    #[cfg(not(feature = "mock_llm"))]
    #[tokio::test]
    async fn preserves_blank_lines_from_filtered_output() {
        let runner = ScriptRunner;
        let tmp = tempfile::tempdir().unwrap();
        let agent = tmp.path().join("agent.sh");
        std::fs::write(
            &agent,
            "#!/bin/sh\nprintf 'chunk one\\nchunk two\\n'\n",
        )
        .unwrap();

        let filter = tmp.path().join("filter.sh");
        std::fs::write(
            &filter,
            r###"#!/bin/sh
content="## Section One

First section body.

## Section Two

Tail line."

while IFS= read -r line; do
  printf 'progress:%s\n' "$line" 1>&2
done

printf '%s\n' "$content"
"###,
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for script in [&agent, &filter] {
                let mut perms = std::fs::metadata(script).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(script, perms).unwrap();
            }
        }

        let (tx, mut rx) = mpsc::channel(8);
        let request = AgentRequest {
            prompt: "prompt".to_string(),
            repo_root: tmp.path().to_path_buf(),
            command: vec![agent.display().to_string()],
            progress_filter: Some(vec![filter.display().to_string()]),
            output: config::AgentOutputHandling::Wrapped,
            allow_script_wrapper: true,
            scope: Some(CommandScope::Ask),
            metadata: BTreeMap::new(),
            timeout: Some(Duration::from_secs(2)),
        };

        let response = runner
            .execute(request, Some(ProgressHook::Plain(tx)))
            .await
            .expect("agent run should succeed");

        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }

        let expected = "## Section One\n\nFirst section body.\n\n## Section Two\n\nTail line.\n";
        assert_eq!(response.assistant_text, expected);
        assert!(
            events
                .iter()
                .all(|event| event
                    .message
                    .as_deref()
                    .map(|msg| !msg.trim().is_empty())
                    .unwrap_or(false)),
            "progress events should remain trimmed and non-empty"
        );
        assert!(
            response
                .stderr
                .iter()
                .any(|line| line.contains("progress:chunk one")),
            "stderr should carry trimmed progress lines"
        );
    }

    #[cfg(all(not(feature = "mock_llm"), unix))]
    #[tokio::test]
    async fn stdbuf_wrapper_flushes_buffered_output() {
        if std::process::Command::new("stdbuf")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping stdbuf buffering test because stdbuf is unavailable");
            return;
        }

        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping stdbuf buffering test because python3 is unavailable");
            return;
        }

        let runner = ScriptRunner;
        let tmp = tempfile::tempdir().unwrap();
        let agent = tmp.path().join("buffered_agent.py");
        std::fs::write(
            &agent,
            r#"#!/usr/bin/env python3
import sys
import time
_ = sys.stdin.read()
sys.stdout.write('{"type":"item.started","item":{"type":"reasoning","text":"first"}}\n')
sys.stdout.flush()
time.sleep(1)
sys.stdout.write('{"type":"item.completed","item":{"type":"agent_message","text":"done"}}\n')
sys.stdout.flush()
"#,
        )
        .unwrap();

        let filter = tmp.path().join("filter.sh");
        std::fs::write(
            &filter,
            r#"#!/bin/sh
last=""
while IFS= read -r line; do
  printf 'progress:%s\n' "$line" 1>&2
  last="$line"
done
printf '%s\n' "$last"
"#,
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for script in [&agent, &filter] {
                let mut perms = std::fs::metadata(script).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(script, perms).unwrap();
            }
        }

        let (tx, mut rx) = mpsc::channel(4);
        let request = AgentRequest {
            prompt: "prompt".to_string(),
            repo_root: tmp.path().to_path_buf(),
            command: vec![agent.display().to_string()],
            progress_filter: Some(vec![filter.display().to_string()]),
            output: config::AgentOutputHandling::Wrapped,
            allow_script_wrapper: true,
            scope: Some(CommandScope::Ask),
            metadata: BTreeMap::new(),
            timeout: Some(Duration::from_secs(3)),
        };

        let handle =
            tokio::spawn(
                async move { runner.execute(request, Some(ProgressHook::Plain(tx))).await },
            );

        let first_event = tokio::time::timeout(Duration::from_millis(800), rx.recv())
            .await
            .expect("progress should arrive before agent completes");
        assert!(
            first_event.is_some(),
            "expected progress event before completion"
        );

        let response = handle.await.expect("task should run").expect("agent run");
        assert!(
            response.assistant_text.contains("done"),
            "expected final text in stdout"
        );
        assert!(
            response
                .stderr
                .iter()
                .any(|line| line.contains("progress:")),
            "expected buffered output to be visible via progress filter"
        );
    }

    #[cfg(not(feature = "mock_llm"))]
    #[tokio::test]
    async fn errors_on_missing_command() {
        let runner = ScriptRunner;
        let tmp = tempfile::tempdir().unwrap();
        let request = AgentRequest {
            prompt: "ignored".to_string(),
            repo_root: tmp.path().to_path_buf(),
            command: Vec::new(),
            progress_filter: None,
            output: config::AgentOutputHandling::Wrapped,
            allow_script_wrapper: false,
            scope: Some(CommandScope::Ask),
            metadata: BTreeMap::new(),
            timeout: Some(Duration::from_secs(1)),
        };

        let result = runner.execute(request, None).await;
        assert!(matches!(result, Err(AgentError::MissingCommand)));
    }

    #[cfg(not(feature = "mock_llm"))]
    #[tokio::test]
    async fn fails_on_non_executable_script() {
        let runner = ScriptRunner;
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("script.sh");
        std::fs::write(&script, "#!/bin/sh\necho hello\n").unwrap();
        // intentionally leave script non-executable

        let request = AgentRequest {
            prompt: "run".to_string(),
            repo_root: tmp.path().to_path_buf(),
            command: vec![script.display().to_string()],
            progress_filter: None,
            output: config::AgentOutputHandling::Wrapped,
            allow_script_wrapper: false,
            scope: Some(CommandScope::Ask),
            metadata: BTreeMap::new(),
            timeout: Some(Duration::from_secs(1)),
        };

        let result = runner.execute(request, None).await;
        match result {
            Err(AgentError::Spawn(err)) => {
                assert_eq!(
                    err.kind(),
                    std::io::ErrorKind::PermissionDenied,
                    "expected permission error for non-executable script, got {err:?}"
                );
            }
            Err(AgentError::NonZeroExit(code, lines)) => {
                assert_eq!(
                    code, 126,
                    "non-executable scripts should return 126 when wrapped"
                );
                assert!(
                    lines
                        .iter()
                        .any(|line| line.to_ascii_lowercase().contains("permission denied")),
                    "stderr should mention permission issue: {lines:?}"
                );
            }
            other => panic!("expected failure for non-executable script, got {other:?}"),
        }
    }

    #[cfg(not(feature = "mock_llm"))]
    #[tokio::test]
    async fn surfaces_non_zero_exit_and_stderr() {
        let runner = ScriptRunner;
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("fail.sh");
        std::fs::write(&script, "#!/bin/sh\necho \"failure detail\" 1>&2\nexit 3\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }

        let request = AgentRequest {
            prompt: "run".to_string(),
            repo_root: tmp.path().to_path_buf(),
            command: vec![script.display().to_string()],
            progress_filter: None,
            output: config::AgentOutputHandling::Wrapped,
            allow_script_wrapper: false,
            scope: Some(CommandScope::Ask),
            metadata: BTreeMap::new(),
            timeout: Some(Duration::from_secs(2)),
        };

        let result = runner.execute(request, None).await;
        match result {
            Err(AgentError::NonZeroExit(code, lines)) => {
                assert_eq!(code, 3);
                assert!(
                    lines.iter().any(|line| line.contains("failure detail")),
                    "stderr lines missing failure detail: {lines:?}"
                );
            }
            other => panic!("expected non-zero exit error, got {other:?}"),
        }
    }

    #[cfg(not(feature = "mock_llm"))]
    #[tokio::test]
    async fn times_out_when_script_runs_too_long() {
        let runner = ScriptRunner;
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("sleep.sh");
        std::fs::write(&script, "#!/bin/sh\nsleep 2\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }

        let request = AgentRequest {
            prompt: "timeout".to_string(),
            repo_root: tmp.path().to_path_buf(),
            command: vec![script.display().to_string()],
            progress_filter: None,
            output: config::AgentOutputHandling::Wrapped,
            allow_script_wrapper: false,
            scope: Some(CommandScope::Ask),
            metadata: BTreeMap::new(),
            timeout: Some(Duration::from_secs(1)),
        };

        let result = runner.execute(request, None).await;
        assert!(
            matches!(result, Err(AgentError::Timeout(secs)) if secs == 1),
            "expected timeout error, got {result:?}"
        );
    }
}
