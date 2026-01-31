use std::path::Path;
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

use serde_json::json;

use vizier_core::{auditor::Auditor, display, vcs::repo_root};

#[derive(Debug, Clone)]
pub(crate) struct CicdScriptResult {
    pub(crate) status: ExitStatus,
    pub(crate) duration: Duration,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

pub(crate) type StopConditionScriptResult = CicdScriptResult;

impl CicdScriptResult {
    pub(crate) fn success(&self) -> bool {
        self.status.success()
    }

    pub(crate) fn status_label(&self) -> String {
        match self.status.code() {
            Some(code) => format!("exit={code}"),
            None => "terminated".to_string(),
        }
    }
}

pub(crate) fn clip_log(bytes: &[u8]) -> String {
    const LIMIT: usize = 8_192;
    if bytes.is_empty() {
        return String::new();
    }
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= LIMIT {
        text.to_string()
    } else {
        let mut clipped = text[..LIMIT].to_string();
        clipped.push_str("\n… output truncated …");
        clipped
    }
}

fn log_cicd_stream(label: &str, content: &str) {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return;
    }
    let snippet: String = trimmed
        .lines()
        .take(12)
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    display::warn(format!("  {label}:\n{snippet}"));
}

pub(crate) fn run_cicd_script(
    script: &Path,
    repo_root: &Path,
) -> Result<CicdScriptResult, Box<dyn std::error::Error>> {
    let start = Instant::now();
    let output = Command::new("sh")
        .arg(script)
        .current_dir(repo_root)
        .output()
        .map_err(|err| format!("failed to run CI/CD script {}: {err}", script.display()))?;
    Ok(CicdScriptResult {
        status: output.status,
        duration: start.elapsed(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

pub(crate) fn log_cicd_result(script: &Path, result: &CicdScriptResult, attempt: u32) {
    let label = if result.success() { "passed" } else { "failed" };
    let status = result.status_label();
    let duration = format!("{:.2}s", result.duration.as_secs_f64());
    let message = format!(
        "CI/CD gate `{}` {label} ({status}; {duration}) [attempt {attempt}]",
        script.display()
    );
    if result.success() {
        display::info(message);
    } else {
        display::warn(message);
        log_cicd_stream("stdout", &result.stdout);
        log_cicd_stream("stderr", &result.stderr);
    }
}

pub(crate) fn run_stop_condition_script(
    script: &Path,
    worktree_root: &Path,
) -> Result<StopConditionScriptResult, Box<dyn std::error::Error>> {
    let start = Instant::now();
    let output = Command::new("sh")
        .arg(script)
        .current_dir(worktree_root)
        .output()
        .map_err(|err| {
            format!(
                "failed to run approve stop-condition script {}: {err}",
                script.display()
            )
        })?;
    Ok(StopConditionScriptResult {
        status: output.status,
        duration: start.elapsed(),
        stdout: clip_log(&output.stdout),
        stderr: clip_log(&output.stderr),
    })
}

pub(crate) fn log_stop_condition_result(
    script: &Path,
    result: &StopConditionScriptResult,
    attempt: u32,
) {
    let label = if result.success() { "passed" } else { "failed" };
    let status = result.status_label();
    let duration = format!("{:.2}s", result.duration.as_secs_f64());
    let message = format!(
        "Approve stop-condition `{}` {label} ({status}; {duration}) [attempt {attempt}]",
        script.display()
    );
    if result.success() {
        display::info(message);
    } else {
        display::warn(message);
        log_cicd_stream("stdout", &result.stdout);
        log_cicd_stream("stderr", &result.stderr);
    }
}

pub(crate) fn stop_condition_script_label(script: &Path, repo_root: Option<&Path>) -> String {
    repo_root
        .and_then(|root| script.strip_prefix(root).ok())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| script.display().to_string())
}

pub(crate) fn record_stop_condition_summary(
    scope: &str,
    script: Option<&Path>,
    status: &str,
    attempts: u32,
    last_result: Option<&StopConditionScriptResult>,
) {
    let repo_root = repo_root().ok();
    let script_label = script.map(|path| stop_condition_script_label(path, repo_root.as_deref()));
    let (exit_code, duration_ms, stdout, stderr) = if let Some(result) = last_result {
        (
            result.status.code(),
            Some(result.duration.as_millis()),
            result.stdout.clone(),
            result.stderr.clone(),
        )
    } else {
        (None, None, String::new(), String::new())
    };

    Auditor::record_operation(
        "approve_stop_condition",
        json!({
            "scope": scope,
            "script": script_label,
            "status": status,
            "attempts": attempts,
            "exit_code": exit_code,
            "duration_ms": duration_ms,
            "stdout": stdout,
            "stderr": stderr,
        }),
    );
}

pub(crate) fn record_stop_condition_attempt(
    scope: &str,
    script: &Path,
    attempt: u32,
    result: &StopConditionScriptResult,
) {
    let repo_root = repo_root().ok();
    let script_label = stop_condition_script_label(script, repo_root.as_deref());
    let status = if result.success() { "passed" } else { "failed" };
    Auditor::record_operation(
        "approve_stop_condition_attempt",
        json!({
            "scope": scope,
            "script": script_label,
            "attempt": attempt,
            "status": status,
            "exit_code": result.status.code(),
            "duration_ms": result.duration.as_millis(),
            "stdout": result.stdout.clone(),
            "stderr": result.stderr.clone(),
        }),
    );
}
