use std::error::Error;
use std::io::{IsTerminal, stderr};

use crossterm::{
    cursor::MoveToColumn,
    execute,
    terminal::{Clear, ClearType},
};
use lazy_static::lazy_static;
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
    Done,
    Error(String),
}

struct DisplayRuntime {
    show_spinner: bool,
    show_line_updates: bool,
    line_once: bool,
    verbosity: Verbosity,
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
        }
    }
}

async fn render_spinner_runtime(mut rx: Receiver<Status>) {
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
        render_spinner_runtime(rx).await;
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
