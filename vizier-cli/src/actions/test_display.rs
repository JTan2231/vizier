use std::sync::Arc;

use tokio::sync::mpsc;

use vizier_core::{
    agent::{AgentError, AgentResponse, ProgressHook},
    auditor::{self, AgentRunRecord, Auditor, Message},
    config,
    display::{self, LogLevel, Verbosity},
    vcs::repo_root,
};

use super::shared::{
    build_agent_request, clip_message, current_verbosity, format_block, spawn_plain_progress_logger,
};
use super::types::TestDisplayOptions;

const DEFAULT_TEST_PROMPT: &str = "Smoke-test the configured agent: emit a few progress updates (no writes, keep it short) and a final response.";

pub(crate) async fn run_test_display(
    opts: TestDisplayOptions,
    agent: &config::AgentSettings,
) -> Result<(), Box<dyn std::error::Error>> {
    if !agent.backend.requires_agent_runner() {
        return Err(format!(
            "vizier test-display requires an agent-capable backend; `{}` is configured for scope `{}`",
            agent.backend,
            agent.scope.as_str()
        )
        .into());
    }

    Auditor::record_agent_context(agent, None);
    let runner = Arc::clone(agent.agent_runner()?);
    let repo_root = repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let prompt = opts
        .prompt_override
        .clone()
        .unwrap_or_else(|| DEFAULT_TEST_PROMPT.to_string());

    let mut request = build_agent_request(agent, prompt.clone(), repo_root);
    if opts.disable_wrapper {
        request.allow_script_wrapper = false;
    }
    if let Some(timeout) = opts.timeout {
        request.timeout = Some(timeout);
    }

    let (progress_tx, progress_rx) = mpsc::channel(64);
    let progress_handle = spawn_plain_progress_logger(progress_rx);
    let result = runner
        .execute(request, Some(ProgressHook::Plain(progress_tx)))
        .await;
    if let Some(handle) = progress_handle {
        let _ = handle.await;
    }

    match result {
        Ok(response) => {
            let session_path = if opts.record_session {
                record_test_display_session(agent, &prompt, &response)
            } else {
                None
            };
            emit_test_display_summary(agent, &response, opts.raw_output, session_path);
            Ok(())
        }
        Err(AgentError::NonZeroExit(code, stderr)) => {
            render_test_display_failure(agent, code, &stderr);
            if opts.raw_output
                && !stderr.is_empty()
                && !matches!(current_verbosity(), Verbosity::Quiet)
            {
                for line in &stderr {
                    eprintln!("{line}");
                }
            }
            if opts.record_session {
                let _ = record_test_display_failure(agent, &prompt, code, &stderr);
            }
            std::process::exit(if code == 0 { 1 } else { code });
        }
        Err(AgentError::Timeout(secs)) => {
            render_test_display_timeout(agent, secs);
            if opts.record_session {
                let _ = record_test_display_failure(
                    agent,
                    &prompt,
                    124,
                    &[format!("timeout after {secs}s")],
                );
            }
            std::process::exit(124);
        }
        Err(err) => Err(Box::new(err)),
    }
}

fn emit_test_display_summary(
    agent: &config::AgentSettings,
    response: &AgentResponse,
    raw_output: bool,
    session_path: Option<String>,
) {
    if matches!(current_verbosity(), Verbosity::Quiet) {
        return;
    }

    let mut rows = vec![
        (
            "Outcome".to_string(),
            "Agent display test succeeded".to_string(),
        ),
        ("Scope".to_string(), agent.scope.as_str().to_string()),
        ("Agent".to_string(), agent.selector.clone()),
        ("Backend".to_string(), agent.backend.to_string()),
        ("Exit code".to_string(), response.exit_code.to_string()),
        (
            "Duration".to_string(),
            format!("{:.2}s", response.duration_ms as f64 / 1000.0),
        ),
    ];

    if let Some(path) = session_path {
        rows.push(("Session".to_string(), path));
    }

    if !raw_output {
        let stdout_snippet = response
            .assistant_text
            .trim()
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        rows.push((
            "Stdout".to_string(),
            if stdout_snippet.is_empty() {
                "<empty>".to_string()
            } else {
                clip_message(&stdout_snippet)
            },
        ));
        if let Some(last_stderr) = response.stderr.last() {
            rows.push(("Stderr".to_string(), clip_message(last_stderr)));
        }
    }

    println!("{}", format_block(rows));

    if raw_output {
        if !response.assistant_text.is_empty() {
            println!("{}", response.assistant_text.trim_end());
        }
        if !response.stderr.is_empty() {
            for line in &response.stderr {
                eprintln!("{line}");
            }
        }
    }
}

fn render_test_display_failure(agent: &config::AgentSettings, code: i32, stderr: &[String]) {
    let mut message = format!(
        "agent for `{}` exited with status {code}",
        agent.scope.as_str()
    );
    if let Some(line) = stderr.last() {
        message.push_str(&format!("; stderr: {line}"));
    }
    display::emit(LogLevel::Error, message);
    if matches!(current_verbosity(), Verbosity::Debug) && !stderr.is_empty() {
        for line in stderr {
            display::debug(format!("stderr: {line}"));
        }
    }
}

fn render_test_display_timeout(agent: &config::AgentSettings, secs: u64) {
    display::emit(
        LogLevel::Error,
        format!(
            "agent for `{}` timed out after {secs}s",
            agent.scope.as_str()
        ),
    );
}

fn record_test_display_session(
    agent: &config::AgentSettings,
    prompt: &str,
    response: &AgentResponse,
) -> Option<String> {
    auditor::Auditor::add_message(Message::user(prompt.to_string()));
    auditor::Auditor::add_message(Message::assistant(response.assistant_text.clone()));
    auditor::Auditor::record_agent_run(AgentRunRecord {
        command: agent.agent_runtime.command.clone(),
        output: agent.agent_runtime.output,
        progress_filter: agent.agent_runtime.progress_filter.clone(),
        exit_code: response.exit_code,
        stdout: response.assistant_text.clone(),
        stderr: response.stderr.clone(),
        duration_ms: response.duration_ms,
    });
    persist_session_log_with_notice()
}

fn record_test_display_failure(
    agent: &config::AgentSettings,
    prompt: &str,
    exit_code: i32,
    stderr: &[String],
) -> Option<String> {
    auditor::Auditor::add_message(Message::user(prompt.to_string()));
    let assistant_text = if stderr.is_empty() {
        format!("agent exited with status {exit_code}")
    } else {
        stderr.join("\n")
    };
    auditor::Auditor::add_message(Message::assistant(assistant_text));
    auditor::Auditor::record_agent_run(AgentRunRecord {
        command: agent.agent_runtime.command.clone(),
        output: agent.agent_runtime.output,
        progress_filter: agent.agent_runtime.progress_filter.clone(),
        exit_code,
        stdout: String::new(),
        stderr: stderr.to_vec(),
        duration_ms: 0,
    });
    persist_session_log_with_notice()
}

fn persist_session_log_with_notice() -> Option<String> {
    match auditor::Auditor::persist_session_log() {
        Some(artifact) => {
            auditor::Auditor::clear_messages();
            Some(artifact.display_path())
        }
        None => {
            if config::get_config().no_session {
                display::info("Session logging disabled (--no-session); no session file written.");
            }
            None
        }
    }
}
