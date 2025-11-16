use std::error::Error;
use std::io::{IsTerminal, stderr};

use crossterm::{
    cursor::MoveToColumn,
    execute,
    terminal::{Clear, ClearType},
};
use lazy_static::lazy_static;
use serde_json::Value;
use tokio::sync::mpsc::{Receiver, Sender, channel};
use tokio::time::{self, Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verbosity {
    Quiet,
    Normal,
    Info,
    Debug,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressMode {
    Auto,
    Never,
    Always,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

#[derive(Clone, Copy, Debug)]
pub struct DisplayConfig {
    pub verbosity: Verbosity,
    pub progress: ProgressMode,
    pub ansi_enabled: bool,
    pub stdout_is_tty: bool,
    pub stderr_is_tty: bool,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        let stdout_is_tty = std::io::stdout().is_terminal();
        let stderr_is_tty = std::io::stderr().is_terminal();

        Self {
            verbosity: Verbosity::Normal,
            progress: ProgressMode::Auto,
            ansi_enabled: stdout_is_tty || stderr_is_tty,
            stdout_is_tty,
            stderr_is_tty,
        }
    }
}

lazy_static! {
    static ref CONFIG: std::sync::RwLock<DisplayConfig> =
        std::sync::RwLock::new(DisplayConfig::default());
}

pub fn set_display_config(config: DisplayConfig) {
    if let Ok(mut cfg) = CONFIG.write() {
        *cfg = config;
    }
}

pub fn get_display_config() -> DisplayConfig {
    CONFIG.read().map(|cfg| *cfg).unwrap_or_default()
}

impl Verbosity {
    fn allows(self, level: LogLevel) -> bool {
        match self {
            Verbosity::Quiet => matches!(level, LogLevel::Error),
            Verbosity::Normal => matches!(level, LogLevel::Error | LogLevel::Warn),
            Verbosity::Info => matches!(level, LogLevel::Error | LogLevel::Warn | LogLevel::Info),
            Verbosity::Debug => true,
        }
    }
}

pub fn emit(level: LogLevel, message: impl AsRef<str>) {
    let cfg = get_display_config();
    if level == LogLevel::Error || cfg.verbosity.allows(level) {
        eprintln!("{}", message.as_ref());
    }
}

pub fn warn(message: impl AsRef<str>) {
    emit(LogLevel::Warn, message);
}

pub fn info(message: impl AsRef<str>) {
    emit(LogLevel::Info, message);
}

pub fn debug(message: impl AsRef<str>) {
    emit(LogLevel::Debug, message);
}

pub enum Status {
    Working(String),
    Event(ProgressEvent),
    Done,
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressKind {
    Codex,
}

impl ProgressKind {
    fn prefix(self) -> &'static str {
        match self {
            ProgressKind::Codex => "[codex]",
        }
    }

    fn label(self) -> &'static str {
        match self {
            ProgressKind::Codex => "codex",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProgressEvent {
    pub kind: ProgressKind,
    pub phase: Option<String>,
    pub label: Option<String>,
    pub message: Option<String>,
    pub detail: Option<String>,
    pub path: Option<String>,
    pub progress: Option<f64>,
    pub status: Option<String>,
    pub timestamp: Option<String>,
    pub raw: Option<String>,
}

impl ProgressEvent {
    fn summarize(&self) -> (String, Option<String>) {
        let stage = self
            .phase
            .as_deref()
            .or_else(|| self.label.as_deref())
            .unwrap_or_else(|| self.kind.label())
            .to_string();
        let summary = self
            .message
            .as_ref()
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .or_else(|| {
                self.label
                    .as_ref()
                    .map(|s| s.as_str())
                    .filter(|s| !s.is_empty() && s != &&stage)
                    .map(|s| s.to_string())
            });

        (stage, summary)
    }

    fn scope(&self) -> Option<String> {
        let mut scopes = Vec::new();
        if let Some(detail) = self.detail.as_ref().filter(|s| !s.is_empty()) {
            scopes.push(detail.as_str());
        }
        if let Some(path) = self.path.as_ref().filter(|s| !s.is_empty()) {
            if !scopes.iter().any(|existing| existing == &path.as_str()) {
                scopes.push(path.as_str());
            }
        }

        if scopes.is_empty() {
            None
        } else {
            Some(scopes.join(" • "))
        }
    }
}

#[derive(Clone, Copy)]
struct DisplayRuntime {
    show_spinner: bool,
    show_line_updates: bool,
    line_once: bool,
    verbosity: Verbosity,
    log_events: bool,
}

impl DisplayRuntime {
    fn from_config(cfg: DisplayConfig) -> Self {
        let show_spinner = matches!(cfg.progress, ProgressMode::Auto | ProgressMode::Always)
            && cfg.stderr_is_tty
            && cfg.stdout_is_tty
            && cfg.ansi_enabled
            && !matches!(cfg.verbosity, Verbosity::Quiet);

        let show_line_updates = !show_spinner
            && (matches!(cfg.verbosity, Verbosity::Debug)
                || (matches!(cfg.verbosity, Verbosity::Info)
                    && matches!(cfg.progress, ProgressMode::Always)));

        let line_once = matches!(cfg.verbosity, Verbosity::Info)
            && matches!(cfg.progress, ProgressMode::Always)
            && !show_spinner;

        Self {
            show_spinner,
            show_line_updates,
            line_once,
            verbosity: cfg.verbosity,
            log_events: !matches!(cfg.verbosity, Verbosity::Quiet),
        }
    }
}

async fn render_spinner_runtime(mut rx: Receiver<Status>, runtime: DisplayRuntime) {
    let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut index = 0usize;
    let mut last_message = String::from("Working");
    let mut ticker = time::interval(Duration::from_millis(80));

    loop {
        tokio::select! {
            Some(status) = rx.recv() => {
                match status {
                    Status::Working(msg) => {
                        last_message = msg;
                        render_spinner_frame(spinner_frames[index % spinner_frames.len()], &last_message);
                        index = index.wrapping_add(1);
                    }
                    Status::Event(event) => {
                        if runtime.log_events {
                            clear_spinner_line();
                            emit_progress_event(&event, runtime);
                            render_spinner_frame(
                                spinner_frames[index % spinner_frames.len()],
                                &last_message,
                            );
                        }
                    }
                    Status::Done => {
                        clear_spinner_line();
                        break;
                    }
                    Status::Error(e) => {
                        clear_spinner_line();
                        emit(LogLevel::Error, format!("Error: {}", e));
                        break;
                    }
                }
            }
            _ = ticker.tick() => {
                render_spinner_frame(spinner_frames[index % spinner_frames.len()], &last_message);
                index = index.wrapping_add(1);
            }
        }
    }
}

fn render_spinner_frame(frame: &str, message: &str) {
    let _ = execute!(stderr(), MoveToColumn(0), Clear(ClearType::CurrentLine));
    eprint!("{} {}", frame, message);
}

fn clear_spinner_line() {
    let _ = execute!(stderr(), MoveToColumn(0), Clear(ClearType::CurrentLine));
}

async fn render_line_runtime(mut rx: Receiver<Status>, runtime: DisplayRuntime) {
    let mut printed_once = false;

    while let Some(status) = rx.recv().await {
        match status {
            Status::Working(msg) => {
                if runtime.verbosity == Verbosity::Debug {
                    debug(format!("Working: {}", msg));
                } else if runtime.line_once && !printed_once {
                    info(format!("Working: {}", msg));
                    printed_once = true;
                }
            }
            Status::Event(event) => {
                emit_progress_event(&event, runtime);
            }
            Status::Done => break,
            Status::Error(e) => {
                emit(LogLevel::Error, format!("Error: {}", e));
                break;
            }
        }
    }
}

async fn display_status(rx: Receiver<Status>, runtime: DisplayRuntime) {
    if runtime.show_spinner {
        render_spinner_runtime(rx, runtime).await;
    } else if runtime.show_line_updates {
        render_line_runtime(rx, runtime).await;
    } else {
        // Drain the channel to ensure all errors propagate even when quiet.
        render_line_runtime(rx, runtime).await;
    }
}

pub async fn call_with_status<F, Fut>(f: F) -> Result<Vec<wire::types::Message>, Box<dyn Error>>
where
    F: FnOnce(Sender<Status>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<Vec<wire::types::Message>, Box<dyn std::error::Error>>>
        + Send
        + 'static,
{
    let (tx, rx) = channel(10);
    let runtime = DisplayRuntime::from_config(get_display_config());
    let display_task = tokio::spawn(display_status(rx, runtime));

    let result = match f(tx.clone()).await {
        Ok(messages) => Ok(messages),
        Err(e) => {
            let _ = tx.send(Status::Error(e.to_string())).await;
            Err(e)
        }
    };

    let _ = tx.send(Status::Done).await;
    let _ = display_task.await;

    result
}

fn emit_progress_event(event: &ProgressEvent, runtime: DisplayRuntime) {
    if !runtime.log_events {
        return;
    }

    for line in render_progress_event(event, runtime.verbosity) {
        eprintln!("{}", line);
    }
}

fn format_progress_value(progress: f64) -> Option<String> {
    if !progress.is_finite() {
        return None;
    }

    let normalized = if (0.0..=1.0).contains(&progress) {
        progress * 100.0
    } else {
        progress
    };

    Some(format!("{:.0}%", normalized.clamp(0.0, 100.0)))
}

pub fn render_progress_event(event: &ProgressEvent, verbosity: Verbosity) -> Vec<String> {
    if matches!(verbosity, Verbosity::Quiet) {
        return Vec::new();
    }

    let (stage, summary) = event.summarize();
    let mut primary = format!("{} {}", event.kind.prefix(), stage);
    if let Some(progress) = event.progress.and_then(format_progress_value) {
        primary.push_str(&format!(" ({})", progress));
    }
    if let Some(status) = event.status.as_ref().filter(|s| !s.is_empty()) {
        primary.push_str(&format!(" [{}]", status));
    }
    if let Some(summary) = summary {
        primary.push_str(" — ");
        primary.push_str(&summary);
    }
    if let Some(scope) = event.scope() {
        primary.push_str(&format!(" ({})", scope));
    }

    let mut lines = vec![primary];

    if matches!(verbosity, Verbosity::Info | Verbosity::Debug) {
        if let Some(timestamp) = event.timestamp.as_ref().filter(|s| !s.is_empty()) {
            lines.push(format!("{} timestamp={}", event.kind.prefix(), timestamp));
        }
    }

    if matches!(verbosity, Verbosity::Debug) {
        if let Some(raw) = event.raw.as_ref().filter(|s| !s.is_empty()) {
            lines.push(format!("{} event={}", event.kind.prefix(), raw));
        }
    }

    lines
}

pub fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(num) => Some(num.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_basic_progress_line() {
        let event = ProgressEvent {
            kind: ProgressKind::Codex,
            phase: Some("apply plan".into()),
            label: None,
            message: Some("edit README".into()),
            detail: Some("README.md".into()),
            path: None,
            progress: Some(0.42),
            status: None,
            timestamp: None,
            raw: None,
        };

        let lines = render_progress_event(&event, Verbosity::Normal);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("[codex] apply plan"));
        assert!(lines[0].contains("42%"));
        assert!(lines[0].contains("edit README"));
        assert!(lines[0].contains("README.md"));
    }

    #[test]
    fn renders_debug_metadata() {
        let event = ProgressEvent {
            kind: ProgressKind::Codex,
            phase: Some("apply plan".into()),
            label: None,
            message: Some("edit README".into()),
            detail: None,
            path: None,
            progress: None,
            status: Some("running".into()),
            timestamp: Some("2024-01-01T00:00:00Z".into()),
            raw: Some("{\"type\":\"sample\"}".into()),
        };

        let lines = render_progress_event(&event, Verbosity::Debug);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("[running]"));
        assert!(lines[1].contains("timestamp=2024-01-01T00:00:00Z"));
        assert!(lines[2].contains("event={\"type\":\"sample\"}"));
    }
}
