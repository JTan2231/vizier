use std::path::{Path, PathBuf};
use std::{fs, process::Command};

use git2::{BranchType, Oid, Repository};
use uuid::Uuid;
use vizier_core::{auditor, config, display, vcs};

use crate::actions::shared::{TempWorktree, push_origin_if_requested};
use crate::actions::{CommitMode, run_save_in_worktree};
use crate::cli::args::{BuildPipelineArg, Commands, ResolvedInput, SaveCmd};
use crate::cli::resolve::resolve_prompt_input;
use crate::cli::util::flag_present;
use crate::jobs;

fn command_scope_for(command: &Commands) -> Option<config::CommandScope> {
    match command {
        Commands::Draft(_) => Some(config::CommandScope::Draft),
        Commands::Patch(_) => Some(config::CommandScope::Draft),
        Commands::Approve(_) => Some(config::CommandScope::Approve),
        Commands::Review(_) => Some(config::CommandScope::Review),
        Commands::Merge(_) => Some(config::CommandScope::Merge),
        Commands::Save(_) => Some(config::CommandScope::Save),
        _ => None,
    }
}

fn job_is_active(status: jobs::JobStatus) -> bool {
    matches!(
        status,
        jobs::JobStatus::Queued
            | jobs::JobStatus::WaitingOnDeps
            | jobs::JobStatus::WaitingOnApproval
            | jobs::JobStatus::WaitingOnLocks
            | jobs::JobStatus::Running
    )
}

pub(crate) fn scheduler_supported(command: &Commands) -> bool {
    matches!(
        command,
        Commands::Save(_)
            | Commands::Draft(_)
            | Commands::Patch(_)
            | Commands::Approve(_)
            | Commands::Review(_)
            | Commands::Merge(_)
    )
}

pub(crate) fn resolve_pinned_head(
    project_root: &Path,
) -> Result<jobs::PinnedHead, Box<dyn std::error::Error>> {
    let repo = Repository::discover(project_root)?;
    let head = repo.head()?;
    if !head.is_branch() {
        return Err("pinned head requires a branch checkout (detached HEAD)".into());
    }
    let branch = head
        .shorthand()
        .ok_or("unable to resolve HEAD branch name")?
        .to_string();
    let oid = head
        .target()
        .ok_or("unable to resolve HEAD commit")?
        .to_string();
    Ok(jobs::PinnedHead { branch, oid })
}

fn load_job_pinned_head(
    jobs_root: &Path,
    job_id: &str,
) -> Result<jobs::PinnedHead, Box<dyn std::error::Error>> {
    let record = jobs::read_record(jobs_root, job_id)?;
    let schedule = record
        .schedule
        .as_ref()
        .ok_or("job schedule missing pinned head details")?;
    schedule
        .pinned_head
        .clone()
        .ok_or("job schedule missing pinned head details".into())
}

fn pinned_head_matches(repo: &Repository, pinned: &jobs::PinnedHead) -> Result<bool, git2::Error> {
    let branch_ref = repo.find_branch(&pinned.branch, BranchType::Local)?;
    let commit = branch_ref.into_reference().peel_to_commit()?;
    let expected = Oid::from_str(&pinned.oid).ok();
    Ok(Some(commit.id()) == expected)
}

fn current_branch_name(repo: &Repository) -> Result<Option<String>, git2::Error> {
    let head = repo.head()?;
    if !head.is_branch() {
        return Ok(None);
    }
    Ok(head.shorthand().map(|name| name.to_string()))
}

fn ensure_branch_matches(
    project_root: &Path,
    pinned: &jobs::PinnedHead,
) -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repository::discover(project_root)?;
    let current = current_branch_name(&repo)?;
    if current.as_deref() != Some(pinned.branch.as_str()) {
        return Err(format!(
            "scheduled job expects branch {} to be checked out; current branch is {:?}",
            pinned.branch, current
        )
        .into());
    }
    if !pinned_head_matches(&repo, pinned)? {
        return Err(format!(
            "pinned head mismatch for {} (expected {})",
            pinned.branch, pinned.oid
        )
        .into());
    }
    Ok(())
}

fn reset_worktree_clean(project_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["reset", "--hard"])
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!("failed to reset worktree before applying changes: {stderr}").into())
}

fn write_patch_from_worktree(
    worktree_path: &Path,
    base_oid: Oid,
    head_oid: Option<Oid>,
    patch_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = patch_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(worktree_path);
    cmd.args(["diff", "--binary", "--no-color"]);
    if let Some(head) = head_oid {
        cmd.arg(format!("{base}..{head}", base = base_oid, head = head));
    } else {
        cmd.arg(base_oid.to_string());
    }
    let output = cmd.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("failed to generate patch from worktree: {stderr}").into());
    }
    fs::write(patch_path, output.stdout)?;
    Ok(())
}

pub(crate) fn capture_save_input_patch(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let patch_path = jobs::save_input_patch_path(jobs_root, job_id);
    if let Some(parent) = patch_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["diff", "--binary", "--no-color", "HEAD"])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("failed to capture save input patch: {stderr}").into());
    }
    fs::write(patch_path, output.stdout)?;
    Ok(())
}

fn patch_is_empty(patch_path: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    Ok(match fs::metadata(patch_path) {
        Ok(meta) => meta.len() == 0,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => true,
        Err(err) => return Err(Box::new(err)),
    })
}

fn apply_patch_to_repo(
    project_root: &Path,
    patch_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if patch_is_empty(patch_path)? {
        return Ok(());
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["apply", "--binary"])
        .arg(patch_path)
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!("failed to apply scheduled changes: {stderr}").into())
}

fn head_commit_oid(path: &Path) -> Result<Oid, Box<dyn std::error::Error>> {
    let repo = Repository::discover(path)?;
    let head = repo.head()?;
    let commit = head.peel_to_commit()?;
    Ok(commit.id())
}

fn cherry_pick_commit(project_root: &Path, commit: Oid) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .arg("cherry-pick")
        .arg(commit.to_string())
        .output()?;
    if output.status.success() {
        return Ok(());
    }

    let _ = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["cherry-pick", "--abort"])
        .status();

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let message = format!(
        "failed to apply scheduled changes via cherry-pick: {}{}",
        stdout,
        if stderr.is_empty() {
            "".to_string()
        } else {
            format!("\n{stderr}")
        }
    );
    Err(message.into())
}

fn write_job_input(
    project_root: &Path,
    job_id: &str,
    text: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = project_root.join(".vizier/tmp/job-inputs");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{job_id}.txt"));
    std::fs::write(&path, text)?;
    Ok(path)
}

pub(crate) fn prepare_prompt_input(
    positional: Option<&str>,
    file: Option<&Path>,
    project_root: &Path,
    job_id: &str,
) -> Result<(ResolvedInput, Option<PathBuf>), Box<dyn std::error::Error>> {
    let resolved = resolve_prompt_input(positional, file)?;
    if matches!(resolved.origin, crate::cli::args::InputOrigin::Stdin) {
        let path = write_job_input(project_root, job_id, &resolved.text)?;
        return Ok((resolved, Some(path)));
    }
    Ok((resolved, None))
}

pub(crate) fn strip_stdin_marker(raw_args: &[String]) -> Vec<String> {
    raw_args
        .iter()
        .filter(|arg| arg.as_str() != "-")
        .cloned()
        .collect()
}

pub(crate) fn has_active_plan_job(
    jobs_root: &Path,
    slug: &str,
    scope: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    let records = jobs::list_records(jobs_root)?;
    Ok(records.iter().any(|record| {
        job_is_active(record.status)
            && record
                .metadata
                .as_ref()
                .and_then(|meta| meta.scope.as_deref())
                == Some(scope)
            && record
                .metadata
                .as_ref()
                .and_then(|meta| meta.plan.as_deref())
                == Some(slug)
    }))
}

pub(crate) async fn run_scheduled_save(
    job_id: &str,
    cmd: &SaveCmd,
    push_after: bool,
    commit_mode: CommitMode,
    agent: &config::AgentSettings,
    project_root: &Path,
    jobs_root: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let pinned = load_job_pinned_head(jobs_root, job_id)?;
    let repo = Repository::discover(project_root)?;
    if !pinned_head_matches(&repo, &pinned)? {
        return Err(format!(
            "pinned head mismatch for {} (expected {})",
            pinned.branch, pinned.oid
        )
        .into());
    }

    let temp_branch = format!("vizier-job-{job_id}");
    if vcs::branch_exists(&temp_branch)? {
        vcs::delete_branch(&temp_branch)?;
    }
    vcs::create_branch_from(&pinned.branch, &temp_branch)?;

    let worktree = TempWorktree::create(job_id, &temp_branch, "save")?;
    let worktree_path = worktree.path().to_path_buf();
    let base_oid = Oid::from_str(&pinned.oid)?;
    let input_patch = jobs::save_input_patch_path(jobs_root, job_id);
    if !input_patch.exists() {
        return Err("scheduled save is missing the captured input patch; rerun the command".into());
    }
    apply_patch_to_repo(&worktree_path, &input_patch)?;

    let result = run_save_in_worktree(
        &cmd.rev_or_range,
        &[".vizier/"],
        cmd.commit_message.clone(),
        cmd.commit_message_editor,
        commit_mode,
        false,
        agent,
        &worktree_path,
    )
    .await;
    if let Err(err) = result {
        display::warn(format!(
            "Save worktree preserved at {}; inspect branch {} for partial changes.",
            worktree.path().display(),
            temp_branch
        ));
        return Err(err);
    }

    let head_oid = head_commit_oid(&worktree_path)?;
    let patch_path = jobs::command_patch_path(jobs_root, job_id);
    let patch_head = if commit_mode.should_commit() {
        Some(head_oid)
    } else {
        None
    };
    write_patch_from_worktree(&worktree_path, base_oid, patch_head, &patch_path)?;

    if commit_mode.should_commit() {
        if head_oid != base_oid {
            ensure_branch_matches(project_root, &pinned)?;
            reset_worktree_clean(project_root)?;
            cherry_pick_commit(project_root, head_oid)?;
            if push_after {
                push_origin_if_requested(true)?;
            }
        } else {
            display::info("Save produced no commit; nothing to apply.");
        }
    } else {
        ensure_branch_matches(project_root, &pinned)?;
        reset_worktree_clean(project_root)?;
        if patch_is_empty(&patch_path)? {
            display::info("Save produced no changes; nothing to apply.");
        } else {
            apply_patch_to_repo(project_root, &patch_path)?;
        }
    }

    worktree.cleanup()?;
    vcs::delete_branch(&temp_branch)?;
    Ok(())
}

pub(crate) fn background_config_snapshot(cfg: &config::Config) -> serde_json::Value {
    serde_json::json!({
        "agent_selector": cfg.agent_selector,
        "agent": {
            "label": cfg.agent_runtime.label,
            "command": cfg.agent_runtime.command,
        },
        "workflow": {
            "no_commit_default": cfg.workflow.no_commit_default,
            "background": {
                "enabled": cfg.workflow.background.enabled,
                "quiet": cfg.workflow.background.quiet,
            },
        },
    })
}

pub(crate) fn build_job_metadata(
    command: &Commands,
    cfg: &config::Config,
    cli_agent_override: Option<&config::AgentOverrides>,
) -> jobs::JobMetadata {
    let mut metadata = jobs::JobMetadata {
        background_quiet: Some(cfg.workflow.background.quiet),
        config_backend: Some(cfg.backend.to_string()),
        config_agent_selector: Some(cfg.agent_selector.clone()),
        config_agent_label: cfg.agent_runtime.label.clone(),
        ..Default::default()
    };
    if !cfg.agent_runtime.command.is_empty() {
        metadata.config_agent_command = Some(cfg.agent_runtime.command.clone());
    }

    if let Some(scope) = command_scope_for(command) {
        metadata.scope = Some(scope.as_str().to_string());
        if let Ok(agent) = config::resolve_agent_settings(cfg, scope, cli_agent_override) {
            metadata.agent_selector = Some(agent.selector.clone());
            metadata.agent_backend = Some(agent.backend.to_string());
            metadata.agent_label = Some(agent.agent_runtime.label.clone());
            if !agent.agent_runtime.command.is_empty() {
                metadata.agent_command = Some(agent.agent_runtime.command.clone());
            }
        }
    }

    match command {
        Commands::Patch(cmd) => {
            metadata.scope = Some("patch".to_string());
            metadata.target = cmd.target.clone();
            metadata.build_pipeline = Some(
                match cmd.pipeline {
                    Some(BuildPipelineArg::Approve) => "approve",
                    Some(BuildPipelineArg::ApproveReview) => "approve-review",
                    Some(BuildPipelineArg::ApproveReviewMerge) => "approve-review-merge",
                    None => "approve-review-merge",
                }
                .to_string(),
            );
        }
        Commands::Approve(cmd) => {
            metadata.plan = Some(cmd.plan.clone());
            metadata.target = cmd.target.clone();
            metadata.branch = cmd.branch.clone();
        }
        Commands::Review(cmd) => {
            metadata.plan = cmd.plan.clone();
            metadata.target = cmd.target.clone();
            metadata.branch = cmd.branch.clone();
        }
        Commands::Merge(cmd) => {
            metadata.plan = cmd.plan.clone();
            metadata.target = cmd.target.clone();
            metadata.branch = cmd.branch.clone();
        }
        Commands::Save(cmd) => {
            metadata.revision = Some(cmd.rev_or_range.clone());
        }
        _ => {}
    }

    metadata
}

pub(crate) fn runtime_job_metadata() -> Option<jobs::JobMetadata> {
    let mut metadata = jobs::JobMetadata::default();
    if let Some(context) = auditor::Auditor::latest_agent_context() {
        metadata.agent_selector = Some(context.selector);
        metadata.agent_backend = Some(context.backend.to_string());
        metadata.agent_label = Some(context.backend_label);
    }

    if let Some(run) = auditor::Auditor::latest_agent_run() {
        metadata.agent_exit_code = Some(run.exit_code);
        if !run.command.is_empty() {
            metadata.agent_command = Some(run.command.clone());
        }
    }

    if metadata.agent_selector.is_none()
        && metadata.agent_backend.is_none()
        && metadata.agent_label.is_none()
        && metadata.agent_exit_code.is_none()
        && metadata
            .agent_command
            .as_ref()
            .map(|value| value.is_empty())
            .unwrap_or(true)
    {
        None
    } else {
        Some(metadata)
    }
}

fn strip_background_flags(raw_args: &[String]) -> Vec<String> {
    let mut args = Vec::new();
    let mut skip_next = false;
    for arg in raw_args.iter().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }

        if arg == "--background" || arg.starts_with("--background=") {
            continue;
        }

        if arg == "--follow" || arg.starts_with("--follow=") {
            continue;
        }

        if arg == "--no-background" || arg.starts_with("--no-background=") {
            continue;
        }

        if arg == "--background-job-id" {
            skip_next = true;
            continue;
        }
        if arg.starts_with("--background-job-id=") {
            continue;
        }

        args.push(arg.clone());
    }

    args
}

pub(crate) fn user_friendly_args(raw_args: &[String]) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(binary) = raw_args.first() {
        args.push(binary.clone());
    }
    args.extend(strip_background_flags(raw_args));
    args
}

pub(crate) fn generate_job_id() -> String {
    Uuid::new_v4().simple().to_string()
}

pub(crate) fn emit_job_summary(record: &jobs::JobRecord) {
    let outcome = match record.status {
        jobs::JobStatus::Running => "Job started",
        jobs::JobStatus::Queued
        | jobs::JobStatus::WaitingOnDeps
        | jobs::JobStatus::WaitingOnApproval
        | jobs::JobStatus::WaitingOnLocks => "Job queued",
        jobs::JobStatus::BlockedByDependency | jobs::JobStatus::BlockedByApproval => "Job blocked",
        jobs::JobStatus::Succeeded => "Job complete",
        jobs::JobStatus::Failed => "Job failed",
        jobs::JobStatus::Cancelled => "Job cancelled",
    };
    println!("Outcome: {outcome}");
    println!("Job: {}", record.id);
    println!("Status: {}", jobs::status_label(record.status));
    if let Some(schedule) = record.schedule.as_ref()
        && let Some(reason) = schedule.wait_reason.as_ref()
    {
        let detail = reason
            .detail
            .clone()
            .unwrap_or_else(|| format!("{:?}", reason.kind).to_lowercase());
        println!("Wait: {detail}");
    }
    if let Some(approval) = record
        .schedule
        .as_ref()
        .and_then(|schedule| schedule.approval.as_ref())
        && approval.required
        && matches!(approval.state, jobs::JobApprovalState::Pending)
    {
        println!("Next: vizier jobs approve {}", record.id);
    }
    println!("Status: vizier jobs status {}", record.id);
    println!("Logs: vizier jobs tail --follow {}", record.id);
    println!("Attach: vizier jobs attach {}", record.id);
}

pub(crate) fn build_background_child_args(
    raw_args: &[String],
    job_id: &str,
    cfg: &config::BackgroundConfig,
    follow: bool,
    injected_args: &[String],
) -> Vec<String> {
    let mut args = build_background_child_args_base(raw_args, cfg, follow, injected_args);
    args.push("--background-job-id".to_string());
    args.push(job_id.to_string());
    args
}

fn build_background_child_args_base(
    raw_args: &[String],
    cfg: &config::BackgroundConfig,
    follow: bool,
    injected_args: &[String],
) -> Vec<String> {
    let mut args = strip_background_flags(raw_args);
    args.extend(injected_args.iter().cloned());

    if !flag_present(&args, None, "--no-ansi") {
        args.push("--no-ansi".to_string());
    }

    let quiet_flagged =
        flag_present(&args, Some('q'), "--quiet") || flag_present(&args, Some('v'), "--verbose");
    if cfg.quiet && !follow && !quiet_flagged && !flag_present(&args, Some('d'), "--debug") {
        args.push("--quiet".to_string());
    }

    if !flag_present(&args, None, "--no-pager") && !flag_present(&args, None, "--pager") {
        args.push("--no-pager".to_string());
    }

    args
}

#[cfg(test)]
mod tests {
    use super::{strip_background_flags, user_friendly_args};

    #[test]
    fn strip_background_flags_removes_background_controls() {
        let raw_args = vec![
            "vizier".to_string(),
            "save".to_string(),
            "--background".to_string(),
            "--background-job-id".to_string(),
            "abc123".to_string(),
            "--follow".to_string(),
            "--no-background".to_string(),
            "--background=1".to_string(),
            "--follow=1".to_string(),
            "--background-job-id=xyz".to_string(),
            "--other".to_string(),
            "value".to_string(),
        ];

        let stripped = strip_background_flags(&raw_args);
        assert_eq!(
            stripped,
            vec![
                "save".to_string(),
                "--other".to_string(),
                "value".to_string()
            ]
        );
    }

    #[test]
    fn user_friendly_args_keeps_binary_and_strips_background_flags() {
        let raw_args = vec![
            "vizier".to_string(),
            "save".to_string(),
            "--background".to_string(),
            "--background-job-id".to_string(),
            "abc123".to_string(),
            "--flag".to_string(),
        ];

        let args = user_friendly_args(&raw_args);
        assert_eq!(
            args,
            vec![
                "vizier".to_string(),
                "save".to_string(),
                "--flag".to_string()
            ]
        );
    }
}
