use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tokio::{sync::mpsc, task::JoinHandle};

use vizier_core::{
    agent::{AgentRequest, DEFAULT_AGENT_TIMEOUT},
    auditor::{self, Auditor},
    config,
    display::{self, LogLevel, ProgressEvent, Verbosity, format_label_value_block},
    vcs::{self, AttemptOutcome, CredentialAttempt, PushErrorKind, RemoteScheme},
};

use super::types::CommitMode;
use crate::jobs;

pub(crate) fn clip_message(msg: &str) -> String {
    const LIMIT: usize = 90;
    let mut clipped = String::new();
    for (idx, ch) in msg.chars().enumerate() {
        if idx >= LIMIT {
            clipped.push('…');
            break;
        }
        clipped.push(ch);
    }
    clipped
}

pub(crate) fn format_block(rows: Vec<(String, String)>) -> String {
    format_label_value_block(&rows, 0)
}

pub(crate) fn format_block_with_indent(rows: Vec<(String, String)>, indent: usize) -> String {
    format_label_value_block(&rows, indent)
}

pub(crate) fn format_table(rows: &[Vec<String>], indent: usize) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let column_count = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    if column_count == 0 {
        return String::new();
    }

    let mut widths = vec![0usize; column_count];
    for row in rows {
        for (idx, cell) in row.iter().enumerate() {
            widths[idx] = widths[idx].max(cell.chars().count());
        }
    }

    let padding = " ".repeat(indent);
    let mut out = String::new();
    for (row_idx, row) in rows.iter().enumerate() {
        if row_idx > 0 {
            out.push('\n');
        }
        out.push_str(&padding);
        for (col, col_width) in widths.iter().enumerate() {
            let cell = row.get(col).cloned().unwrap_or_default();
            let padding_width = col_width.saturating_sub(cell.chars().count());
            out.push_str(&cell);
            if col + 1 < column_count {
                out.push_str(&" ".repeat(padding_width + 2));
            }
        }
    }

    out
}

fn format_agent_value() -> Option<String> {
    auditor::Auditor::latest_agent_context().map(|context| {
        let mut parts = vec![format!("agent {}", context.selector)];
        parts.push(format!("backend {}", context.backend));
        if !context.backend_label.is_empty() {
            parts.push(format!("runtime {}", context.backend_label));
        }
        parts.push(format!("scope {}", context.scope.as_str()));
        if let Some(code) = context.exit_code {
            parts.push(format!("exit {}", code));
        }
        if let Some(duration) = context.duration_ms {
            parts.push(format!("elapsed {:.2}s", duration as f64 / 1000.0));
        }
        parts.join(" • ")
    })
}

fn latest_agent_rows() -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if let Some(agent) = format_agent_value() {
        rows.push(("Agent".to_string(), agent));
    }

    if let Some(run) = Auditor::latest_agent_run() {
        rows.extend(run.to_rows());
    }

    rows
}

pub(crate) fn copy_session_log_to_repo_root(repo_root: &Path, artifact: &auditor::SessionArtifact) {
    let dest_dir = repo_root
        .join(".vizier")
        .join("sessions")
        .join(&artifact.id);
    let dest_path = dest_dir.join("session.json");

    if artifact.path == dest_path {
        return;
    }

    if let Err(err) = fs::create_dir_all(&dest_dir) {
        display::debug(format!(
            "unable to prepare session log directory {}: {}",
            dest_dir.display(),
            err
        ));
        return;
    }

    if let Err(err) = fs::copy(&artifact.path, &dest_path) {
        display::debug(format!(
            "unable to copy session log from {} to {}: {}",
            artifact.path.display(),
            dest_path.display(),
            err
        ));
    }
}

pub(crate) fn append_agent_rows(rows: &mut Vec<(String, String)>, verbosity: Verbosity) {
    if matches!(verbosity, Verbosity::Quiet) {
        return;
    }

    let agent_rows = latest_agent_rows();
    if !agent_rows.is_empty() {
        rows.extend(agent_rows);
    }
}

pub(crate) fn current_verbosity() -> Verbosity {
    display::get_display_config().verbosity
}

fn format_credential_attempt(attempt: &CredentialAttempt) -> String {
    let label = attempt.strategy.label();
    match &attempt.outcome {
        AttemptOutcome::Success => format!("{label}=ok"),
        AttemptOutcome::Failure(message) => {
            format!("{label}=failed({})", clip_message(message))
        }
        AttemptOutcome::Skipped(message) => {
            format!("{label}=skipped({})", clip_message(message))
        }
    }
}

fn render_push_auth_failure(
    remote: &str,
    url: &str,
    scheme: &RemoteScheme,
    attempts: &[CredentialAttempt],
) {
    let scheme_label = scheme.label();
    display::emit(
        LogLevel::Error,
        format!("Push to {remote} failed ({scheme_label} {url})"),
    );

    if !attempts.is_empty() {
        let summary = attempts
            .iter()
            .map(format_credential_attempt)
            .collect::<Vec<_>>()
            .join("; ");
        display::emit(LogLevel::Error, format!("Credential strategies: {summary}"));
    }

    if matches!(scheme, RemoteScheme::Ssh) {
        display::emit(
            LogLevel::Error,
            "Hint: start ssh-agent and `ssh-add ~/.ssh/id_ed25519`, or switch the remote to HTTPS.",
        );
    }
}

pub(crate) fn push_origin_if_requested(
    should_push: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if !should_push {
        return Ok(());
    }

    display::info("Pushing current branch to origin...");
    match vcs::push_current_branch("origin") {
        Ok(_) => {
            display::info("Push to origin completed.");
            Ok(())
        }
        Err(err) => {
            match err.kind() {
                PushErrorKind::Auth {
                    remote,
                    url,
                    scheme,
                    attempts,
                } => {
                    render_push_auth_failure(remote, url, scheme, attempts);
                }
                PushErrorKind::General(message) => {
                    display::emit(
                        LogLevel::Error,
                        format!("Error pushing to origin: {message}"),
                    );
                }
            }

            Err(Box::<dyn std::error::Error>::from(err))
        }
    }
}

pub(crate) fn print_agent_summary() {
    let verbosity = display::get_display_config().verbosity;
    if matches!(verbosity, Verbosity::Quiet) {
        return;
    }

    let rows = latest_agent_rows();

    let block = format_block_with_indent(rows, 2);
    if block.is_empty() {
        return;
    }

    println!("Agent run:");
    println!("{block}");
}

pub(crate) fn prompt_selection(
    agent: &config::AgentSettings,
) -> Result<&config::PromptSelection, Box<dyn std::error::Error>> {
    agent.prompt_selection().ok_or_else(|| {
        io::Error::other(format!(
            "agent for `{}` is missing a resolved prompt; call AgentSettings::for_prompt first",
            agent.profile_scope.as_str()
        ))
        .into()
    })
}

pub(crate) fn require_agent_backend(
    agent: &config::AgentSettings,
    prompt: config::PromptKind,
    error_message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let derived = agent.for_prompt(prompt)?;
    if !derived.backend.requires_agent_runner() {
        return Err(error_message.into());
    }
    Ok(())
}

pub(crate) fn build_agent_request(
    agent: &config::AgentSettings,
    prompt: String,
    repo_root: PathBuf,
) -> AgentRequest {
    let mut metadata = BTreeMap::new();
    metadata.insert("agent_backend".to_string(), agent.backend.to_string());
    metadata.insert("agent_label".to_string(), agent.agent_runtime.label.clone());
    metadata.insert(
        "agent_command".to_string(),
        agent.agent_runtime.command.join(" "),
    );
    metadata.insert(
        "agent_output".to_string(),
        agent.agent_runtime.output.as_str().to_string(),
    );
    if let Some(filter) = agent.agent_runtime.progress_filter.as_ref() {
        metadata.insert("agent_progress_filter".to_string(), filter.join(" "));
    }
    match &agent.agent_runtime.resolution {
        config::AgentRuntimeResolution::BundledShim { path, .. } => {
            metadata.insert(
                "agent_command_source".to_string(),
                "bundled-shim".to_string(),
            );
            metadata.insert("agent_shim_path".to_string(), path.display().to_string());
        }
        config::AgentRuntimeResolution::ProvidedCommand => {
            metadata.insert("agent_command_source".to_string(), "configured".to_string());
        }
    }

    AgentRequest {
        prompt,
        repo_root,
        command: agent.agent_runtime.command.clone(),
        progress_filter: agent.agent_runtime.progress_filter.clone(),
        output: agent.agent_runtime.output,
        allow_script_wrapper: agent.agent_runtime.enable_script_wrapper,
        scope: agent.scope,
        metadata,
        timeout: Some(DEFAULT_AGENT_TIMEOUT),
    }
}

pub(crate) fn spawn_plain_progress_logger(
    mut rx: mpsc::Receiver<ProgressEvent>,
) -> Option<JoinHandle<()>> {
    let cfg = display::get_display_config();
    if matches!(cfg.verbosity, Verbosity::Quiet) {
        return None;
    }

    let verbosity = cfg.verbosity;
    Some(tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            for line in display::render_progress_event(&event, verbosity) {
                eprintln!("{}", line);
            }
        }
    }))
}

pub(crate) fn audit_disposition(mode: CommitMode) -> auditor::CommitDisposition {
    match mode {
        CommitMode::AutoCommit => auditor::CommitDisposition::Auto,
        CommitMode::HoldForReview => auditor::CommitDisposition::Hold,
    }
}

pub(crate) fn short_hash(hash: &str) -> String {
    const MAX: usize = 8;
    if hash.len() <= MAX {
        hash.to_string()
    } else {
        hash.chars().take(MAX).collect()
    }
}

pub(crate) struct WorkdirGuard {
    previous: PathBuf,
}

impl WorkdirGuard {
    pub(crate) fn enter(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let previous = std::env::current_dir()?;
        std::env::set_current_dir(path)?;
        Ok(Self { previous })
    }
}

impl Drop for WorkdirGuard {
    fn drop(&mut self) {
        if let Err(err) = std::env::set_current_dir(&self.previous) {
            display::debug(format!("failed to restore working directory: {err}"));
        }
    }
}

pub(crate) struct TempWorktree {
    name: String,
    path: PathBuf,
}

impl TempWorktree {
    pub(crate) fn create(
        job_id: &str,
        branch: &str,
        purpose: &str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let repo_root = vcs::repo_root()
            .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?
            .to_path_buf();
        let tmp_root = repo_root.join(".vizier/tmp-worktrees");
        fs::create_dir_all(&tmp_root)?;
        let dir_name = format!("{purpose}-{job_id}");
        let worktree_path = tmp_root.join(&dir_name);
        let worktree_name = format!("vizier-{purpose}-{job_id}");

        vcs::add_worktree_for_branch(&worktree_name, &worktree_path, branch)?;
        jobs::record_current_job_worktree(&repo_root, Some(&worktree_name), &worktree_path);

        Ok(Self {
            name: worktree_name,
            path: worktree_path,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn cleanup(self) -> Result<(), Box<dyn std::error::Error>> {
        if let Err(err) = vcs::remove_worktree(&self.name, true) {
            display::warn(format!(
                "temporary worktree cleanup failed ({err}); remove manually with `git worktree prune`"
            ));
        }
        if self.path.exists() {
            let _ = fs::remove_dir_all(&self.path);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::build_agent_request;
    use std::path::PathBuf;
    use vizier_core::config::{self, CommandScope};

    #[test]
    fn build_agent_request_uses_agent_settings() {
        let mut cfg = config::Config::default();
        cfg.agent_runtime.command = vec![
            "/opt/backend".to_string(),
            "exec".to_string(),
            "--mode".to_string(),
        ];
        cfg.agent_runtime.label = Some("merge-script".to_string());

        let agent = config::resolve_agent_settings(&cfg, CommandScope::Merge, None)
            .expect("merge scope should resolve");

        let request = build_agent_request(
            &agent,
            "prompt body".to_string(),
            PathBuf::from("/repo/root"),
        );

        assert_eq!(
            request.command,
            vec![
                "/opt/backend".to_string(),
                "exec".to_string(),
                "--mode".to_string()
            ]
        );
        assert_eq!(
            request.scope,
            Some(config::CommandScope::Merge),
            "request should carry the originating scope"
        );
        assert_eq!(request.repo_root, PathBuf::from("/repo/root"));
        assert_eq!(
            request
                .metadata
                .get("agent_label")
                .map(|s| s.as_str())
                .unwrap_or(""),
            agent.agent_runtime.label
        );
        assert_eq!(
            request.metadata.get("agent_command").map(|s| s.as_str()),
            Some("/opt/backend exec --mode")
        );
        assert_eq!(
            request
                .metadata
                .get("agent_command_source")
                .map(|s| s.as_str()),
            Some("configured")
        );
    }
}
