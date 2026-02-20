use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;
use git2::{ErrorCode, Oid, Repository, RepositoryState, ResetType, build::CheckoutBuilder};
use vizier_core::{
    config,
    vcs::{self, ReleaseBump, ReleaseCommit, ReleaseNotes, ReleaseTag, ReleaseVersion},
};

use super::shared::format_block;
use crate::cli::args::ReleaseCmd;
use crate::cli::prompt::prompt_yes_no;

struct ReleasePlan {
    last_tag: Option<ReleaseTag>,
    base_version: ReleaseVersion,
    auto_bump: ReleaseBump,
    selected_bump: ReleaseBump,
    forced_bump: Option<ReleaseBump>,
    next_version: ReleaseVersion,
    target_tag: String,
    commits: Vec<ReleaseCommit>,
    notes: ReleaseNotes,
}

#[derive(Clone, Debug)]
struct ReleaseScript {
    command: String,
    source: ReleaseScriptSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReleaseScriptSource {
    CliOverride,
    Config,
}

impl ReleaseScriptSource {
    fn label(&self) -> &'static str {
        match self {
            Self::CliOverride => "--release-script",
            Self::Config => "[release.gate].script",
        }
    }
}

#[derive(Clone, Debug)]
struct ReleaseStartState {
    repo_root: PathBuf,
    start_head: Oid,
    branch_name: String,
}

#[derive(Clone, Debug)]
struct ReleaseTransaction {
    repo_root: PathBuf,
    start_head: Oid,
    branch_name: String,
    created_commit: Oid,
    created_tag: Option<String>,
}

#[derive(Default)]
struct RollbackOutcome {
    tag_removed: Option<bool>,
    branch_restored: bool,
    worktree_restored: bool,
    errors: Vec<String>,
}

impl RollbackOutcome {
    fn succeeded(&self) -> bool {
        self.errors.is_empty()
            && self.tag_removed.unwrap_or(true)
            && self.branch_restored
            && self.worktree_restored
    }
}

#[derive(Debug)]
enum ReleaseScriptFailure {
    Spawn(String),
    Exit(i32),
    Signal,
}

impl ReleaseScriptFailure {
    fn summary(&self) -> String {
        match self {
            Self::Spawn(detail) => format!("failed to start ({detail})"),
            Self::Exit(code) => format!("exit {code}"),
            Self::Signal => "terminated by signal".to_string(),
        }
    }
}

pub(crate) fn run_release(cmd: ReleaseCmd) -> Result<(), Box<dyn std::error::Error>> {
    if cmd.max_commits == 0 {
        return Err("--max-commits must be at least 1".into());
    }

    ensure_release_preconditions()?;

    let release_script = resolve_release_script(&cmd)?;

    let last_tag = vcs::latest_reachable_release_tag()?;
    let commits = vcs::commits_since_release_tag(last_tag.as_ref())?;
    let auto_bump = vcs::derive_release_bump(&commits);
    let forced_bump = forced_bump_from_flags(&cmd);
    let selected_bump = forced_bump.unwrap_or(auto_bump);
    let base_version = last_tag
        .as_ref()
        .map(|tag| tag.version)
        .unwrap_or(ReleaseVersion::ZERO);
    let next_version = base_version.bump(selected_bump);
    let target_tag = format!("v{next_version}");

    let notes = vcs::build_release_notes(&commits, cmd.max_commits);

    let plan = ReleasePlan {
        last_tag,
        base_version,
        auto_bump,
        selected_bump,
        forced_bump,
        next_version,
        target_tag,
        commits,
        notes,
    };

    if plan.selected_bump == ReleaseBump::None {
        print_noop(&plan);
        return Ok(());
    }

    if vcs::release_tag_exists(&plan.target_tag)? {
        return Err(format!(
            "target release tag {} already exists; choose a different bump or remove the tag",
            plan.target_tag
        )
        .into());
    }

    if cmd.dry_run {
        print_dry_run(&plan, cmd.no_tag);
        return Ok(());
    }

    confirm_release_if_needed(&cmd, &plan)?;

    let start_state = capture_release_start_state()?;

    let commit_message = build_release_commit_message(&plan);
    let commit_oid = vcs::add_and_commit(None, &commit_message, true)?;

    let mut tag_created = false;
    if !cmd.no_tag {
        if vcs::release_tag_exists(&plan.target_tag)? {
            return Err(format!(
                "target release tag {} already exists; release commit created but tagging aborted",
                plan.target_tag
            )
            .into());
        }

        let annotation = build_tag_annotation(&plan);
        vcs::create_annotated_release_tag(&plan.target_tag, &annotation)?;
        tag_created = true;
    }

    let transaction = ReleaseTransaction {
        repo_root: start_state.repo_root.clone(),
        start_head: start_state.start_head,
        branch_name: start_state.branch_name,
        created_commit: commit_oid,
        created_tag: if tag_created {
            Some(plan.target_tag.clone())
        } else {
            None
        },
    };

    let mut script_status = None;

    if let Some(script) = release_script.as_ref() {
        print_release_script_invocation(script);
        match run_release_script(
            script,
            &start_state.repo_root,
            &plan,
            commit_oid,
            tag_created,
        ) {
            Ok(()) => {
                script_status = Some("passed (exit 0)".to_string());
            }
            Err(failure) => {
                let rollback = rollback_release_transaction(&transaction);
                print_release_failure(&plan, script, &failure, &transaction, &rollback);
                if rollback.succeeded() {
                    return Err(format!(
                        "release script failed ({}); release commit/tag rolled back",
                        failure.summary()
                    )
                    .into());
                }
                return Err(format!(
                    "release script failed ({}); rollback incomplete; see output for recovery instructions",
                    failure.summary()
                )
                .into());
            }
        }
    }

    print_release_outcome(&plan, commit_oid, tag_created, script_status.as_deref());
    Ok(())
}

fn forced_bump_from_flags(cmd: &ReleaseCmd) -> Option<ReleaseBump> {
    if cmd.major {
        Some(ReleaseBump::Major)
    } else if cmd.minor {
        Some(ReleaseBump::Minor)
    } else if cmd.patch {
        Some(ReleaseBump::Patch)
    } else {
        None
    }
}

fn resolve_release_script(
    cmd: &ReleaseCmd,
) -> Result<Option<ReleaseScript>, Box<dyn std::error::Error>> {
    if cmd.no_release_script {
        return Ok(None);
    }

    if let Some(raw) = cmd.release_script.as_ref() {
        let command = raw.trim();
        if command.is_empty() {
            return Err("--release-script must be a non-empty command".into());
        }
        return Ok(Some(ReleaseScript {
            command: command.to_string(),
            source: ReleaseScriptSource::CliOverride,
        }));
    }

    let cfg = config::get_config();
    let Some(configured) = cfg.release.gate.script else {
        return Ok(None);
    };

    let command = configured.trim();
    if command.is_empty() {
        return Ok(None);
    }

    Ok(Some(ReleaseScript {
        command: command.to_string(),
        source: ReleaseScriptSource::Config,
    }))
}

fn capture_release_start_state() -> Result<ReleaseStartState, Box<dyn std::error::Error>> {
    let repo = Repository::discover(".")?;
    let repo_root = repo
        .workdir()
        .map(|path| path.to_path_buf())
        .ok_or("repository has no working directory")?;
    let head = repo.head()?;
    let start_head = head
        .target()
        .ok_or("HEAD does not point to a commit; cannot start release transaction")?;
    let branch_name = head
        .shorthand()
        .filter(|value| !value.trim().is_empty())
        .ok_or("cannot determine current branch for release transaction")?
        .to_string();

    Ok(ReleaseStartState {
        repo_root,
        start_head,
        branch_name,
    })
}

fn run_release_script(
    script: &ReleaseScript,
    repo_root: &Path,
    plan: &ReleasePlan,
    commit_oid: Oid,
    tag_created: bool,
) -> Result<(), ReleaseScriptFailure> {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(&script.command)
        .current_dir(repo_root)
        .env("VIZIER_RELEASE_VERSION", plan.next_version.to_string())
        .env(
            "VIZIER_RELEASE_TAG",
            if tag_created {
                plan.target_tag.as_str()
            } else {
                ""
            },
        )
        .env("VIZIER_RELEASE_COMMIT", commit_oid.to_string())
        .env(
            "VIZIER_RELEASE_RANGE",
            commit_range_label(plan.last_tag.as_ref()),
        );

    let status = command
        .status()
        .map_err(|err| ReleaseScriptFailure::Spawn(err.to_string()))?;

    if status.success() {
        Ok(())
    } else if let Some(code) = status.code() {
        Err(ReleaseScriptFailure::Exit(code))
    } else {
        Err(ReleaseScriptFailure::Signal)
    }
}

fn rollback_release_transaction(txn: &ReleaseTransaction) -> RollbackOutcome {
    let mut outcome = RollbackOutcome::default();

    let repo = match Repository::open(&txn.repo_root) {
        Ok(repo) => repo,
        Err(err) => {
            outcome
                .errors
                .push(format!("failed to open repository for rollback: {err}"));
            return outcome;
        }
    };

    if let Some(tag_name) = txn.created_tag.as_ref() {
        let reference_name = format!("refs/tags/{tag_name}");
        match repo.find_reference(&reference_name) {
            Ok(mut reference) => match reference.delete() {
                Ok(()) => outcome.tag_removed = Some(true),
                Err(err) => {
                    outcome.tag_removed = Some(false);
                    outcome
                        .errors
                        .push(format!("failed to delete tag `{tag_name}`: {err}"));
                }
            },
            Err(err) if err.code() == ErrorCode::NotFound => {
                outcome.tag_removed = Some(true);
            }
            Err(err) => {
                outcome.tag_removed = Some(false);
                outcome.errors.push(format!(
                    "failed to inspect created tag `{tag_name}` during rollback: {err}"
                ));
            }
        }
    }

    let branch_ref = format!("refs/heads/{}", txn.branch_name);
    match repo.find_reference(&branch_ref) {
        Ok(mut branch) => match branch.set_target(txn.start_head, "vizier release rollback") {
            Ok(_) => outcome.branch_restored = true,
            Err(err) => {
                outcome.errors.push(format!(
                    "failed to reset branch `{}` to {}: {err}",
                    txn.branch_name, txn.start_head
                ));
            }
        },
        Err(err) => {
            outcome.errors.push(format!(
                "failed to locate branch `{}` for rollback: {err}",
                txn.branch_name
            ));
        }
    }

    if let Err(err) = repo.set_head(&branch_ref) {
        outcome.errors.push(format!(
            "failed to checkout branch `{}` during rollback: {err}",
            txn.branch_name
        ));
    }

    match repo.find_object(txn.start_head, None) {
        Ok(start_object) => {
            let mut checkout = CheckoutBuilder::new();
            checkout.force().remove_untracked(true);
            match repo.reset(&start_object, ResetType::Hard, Some(&mut checkout)) {
                Ok(()) => outcome.worktree_restored = true,
                Err(err) => {
                    outcome.errors.push(format!(
                        "failed to restore worktree/index to {}: {err}",
                        txn.start_head
                    ));
                }
            }
        }
        Err(err) => {
            outcome.errors.push(format!(
                "failed to resolve start commit {} during rollback: {err}",
                txn.start_head
            ));
        }
    }

    outcome
}

fn print_release_script_invocation(script: &ReleaseScript) {
    let rows = vec![
        ("Release script".to_string(), script.command.clone()),
        (
            "Script source".to_string(),
            script.source.label().to_string(),
        ),
        ("Script status".to_string(), "running".to_string()),
    ];
    println!("{}", format_block(rows));
}

fn print_release_failure(
    plan: &ReleasePlan,
    script: &ReleaseScript,
    failure: &ReleaseScriptFailure,
    txn: &ReleaseTransaction,
    rollback: &RollbackOutcome,
) {
    let rows = vec![
        ("Outcome".to_string(), "Release failed".to_string()),
        ("Version".to_string(), format!("v{}", plan.next_version)),
        ("Release script".to_string(), script.command.clone()),
        (
            "Script source".to_string(),
            script.source.label().to_string(),
        ),
        ("Script status".to_string(), failure.summary()),
        (
            "Rollback".to_string(),
            if rollback.succeeded() {
                "complete".to_string()
            } else {
                "incomplete".to_string()
            },
        ),
        (
            "Tag rollback".to_string(),
            tag_rollback_label(txn, rollback),
        ),
        (
            "Branch rollback".to_string(),
            if rollback.branch_restored {
                format!(
                    "restored {} to {}",
                    txn.branch_name,
                    short_oid(txn.start_head)
                )
            } else {
                format!(
                    "FAILED to restore {} to {}",
                    txn.branch_name,
                    short_oid(txn.start_head)
                )
            },
        ),
        (
            "Worktree rollback".to_string(),
            if rollback.worktree_restored {
                format!("restored to {}", short_oid(txn.start_head))
            } else {
                format!("FAILED to restore to {}", short_oid(txn.start_head))
            },
        ),
    ];

    println!("{}", format_block(rows));

    if rollback.succeeded() {
        return;
    }

    println!("\nRollback recovery details:");
    println!("  start_head: {}", txn.start_head);
    println!("  created_commit: {}", txn.created_commit);
    println!("  branch: {}", txn.branch_name);
    if let Some(tag_name) = txn.created_tag.as_ref() {
        println!("  created_tag: {tag_name}");
    } else {
        println!("  created_tag: <none>");
    }

    if !rollback.errors.is_empty() {
        println!("  rollback_errors:");
        for err in &rollback.errors {
            println!("    - {err}");
        }
    }

    println!("\nManual recovery suggestions:");
    println!("  git checkout {}", txn.branch_name);
    println!("  git reset --hard {}", txn.start_head);
    if let Some(tag_name) = txn.created_tag.as_ref() {
        println!("  git tag -d {tag_name}");
    }
}

fn tag_rollback_label(txn: &ReleaseTransaction, rollback: &RollbackOutcome) -> String {
    match txn.created_tag.as_ref() {
        None => "skipped (no tag created)".to_string(),
        Some(tag_name) => match rollback.tag_removed {
            Some(true) => format!("removed {tag_name}"),
            _ => format!("FAILED to remove {tag_name}"),
        },
    }
}

fn ensure_release_preconditions() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repository::discover(".")?;

    if let Some(message) = repository_state_message(repo.state()) {
        return Err(message.into());
    }

    let head = repo.head()?;
    if !head.is_branch() {
        return Err("cannot release from detached HEAD; checkout a branch first".into());
    }

    vcs::ensure_clean_worktree()?;
    Ok(())
}

fn repository_state_message(state: RepositoryState) -> Option<&'static str> {
    match state {
        RepositoryState::Clean => None,
        RepositoryState::Merge => Some("cannot release while a merge is in progress"),
        RepositoryState::CherryPick | RepositoryState::CherryPickSequence => {
            Some("cannot release while a cherry-pick is in progress")
        }
        RepositoryState::Rebase
        | RepositoryState::RebaseInteractive
        | RepositoryState::RebaseMerge
        | RepositoryState::ApplyMailbox
        | RepositoryState::ApplyMailboxOrRebase => {
            Some("cannot release while a rebase or apply-mailbox operation is in progress")
        }
        RepositoryState::Revert | RepositoryState::RevertSequence => {
            Some("cannot release while a revert is in progress")
        }
        RepositoryState::Bisect => Some("cannot release while a bisect is in progress"),
    }
}

fn short_oid(oid: Oid) -> String {
    let text = oid.to_string();
    text.chars().take(8).collect()
}

fn commit_range_label(last_tag: Option<&ReleaseTag>) -> String {
    match last_tag {
        Some(tag) => format!("{}..HEAD", tag.name),
        None => "<repo-root>..HEAD".to_string(),
    }
}

fn releasable_commit_count(commits: &[ReleaseCommit]) -> usize {
    commits
        .iter()
        .filter(|commit| {
            let (bump, _) = vcs::classify_commit(&commit.subject, &commit.message);
            bump != ReleaseBump::None
        })
        .count()
}

fn render_notes_preview(notes: &ReleaseNotes) -> String {
    if notes.is_empty() {
        "(no release notes entries)".to_string()
    } else {
        let mut lines = vec![format!("{}:", ReleaseNotes::SECTION_TITLE)];
        for entry in &notes.entries {
            lines.push(format!("  - {} ({})", entry.subject, entry.short_sha));
        }
        if notes.overflow > 0 {
            lines.push(format!("  - +{} more", notes.overflow));
        }
        lines.join("\n")
    }
}

fn print_noop(plan: &ReleasePlan) {
    let rows = vec![
        ("Outcome".to_string(), "No release created".to_string()),
        (
            "Reason".to_string(),
            "No releasable commits found (use --major/--minor/--patch to force a bump)".to_string(),
        ),
        (
            "Last tag".to_string(),
            plan.last_tag
                .as_ref()
                .map(|tag| tag.name.clone())
                .unwrap_or_else(|| "none".to_string()),
        ),
        (
            "Commit range".to_string(),
            commit_range_label(plan.last_tag.as_ref()),
        ),
        ("Computed bump".to_string(), plan.auto_bump.to_string()),
    ];

    println!("{}", format_block(rows));
}

fn print_dry_run(plan: &ReleasePlan, no_tag: bool) {
    let bump_label = match plan.forced_bump {
        Some(forced) => format!("{} (forced)", forced),
        None => plan.selected_bump.to_string(),
    };

    let rows = vec![
        ("Outcome".to_string(), "Release dry run".to_string()),
        (
            "Last tag".to_string(),
            plan.last_tag
                .as_ref()
                .map(|tag| tag.name.clone())
                .unwrap_or_else(|| "none".to_string()),
        ),
        (
            "Commit range".to_string(),
            commit_range_label(plan.last_tag.as_ref()),
        ),
        ("Computed bump".to_string(), plan.auto_bump.to_string()),
        ("Selected bump".to_string(), bump_label),
        (
            "Next version".to_string(),
            format!("v{}", plan.next_version),
        ),
        (
            "Commits scanned".to_string(),
            plan.commits.len().to_string(),
        ),
        (
            "Releasable commits".to_string(),
            releasable_commit_count(&plan.commits).to_string(),
        ),
        (
            "Tag action".to_string(),
            if no_tag {
                "skip (--no-tag)".to_string()
            } else {
                format!("create annotated tag {}", plan.target_tag)
            },
        ),
    ];

    println!("{}", format_block(rows));
    println!(
        "\nRelease notes preview:\n{}",
        render_notes_preview(&plan.notes)
    );
}

fn confirm_release_if_needed(
    cmd: &ReleaseCmd,
    plan: &ReleasePlan,
) -> Result<(), Box<dyn std::error::Error>> {
    if cmd.assume_yes {
        return Ok(());
    }

    if !io::stdin().is_terminal() {
        return Err("vizier release requires --yes when stdin is not a TTY".into());
    }

    let prompt = format!(
        "Create release v{} ({} bump) from {}?",
        plan.next_version,
        plan.selected_bump,
        commit_range_label(plan.last_tag.as_ref())
    );
    let confirmed = prompt_yes_no(&prompt)?;
    if !confirmed {
        return Err("aborted by user".into());
    }

    Ok(())
}

fn build_release_commit_message(plan: &ReleasePlan) -> String {
    let mut body = vec![
        format!("Previous version: v{}", plan.base_version),
        format!("New version: v{}", plan.next_version),
        format!("Bump: {}", plan.selected_bump),
        format!(
            "Commit range: {}",
            commit_range_label(plan.last_tag.as_ref())
        ),
        format!("Commits scanned: {}", plan.commits.len()),
        format!(
            "Releasable commits: {}",
            releasable_commit_count(&plan.commits)
        ),
        String::new(),
        "Release Notes:".to_string(),
    ];

    let notes = plan.notes.render_markdown();
    if notes.is_empty() {
        body.push("- No release notes entries".to_string());
    } else {
        body.push(notes);
    }

    body.push(String::new());
    body.push("Generated-by: vizier release".to_string());
    body.push(format!("Generated-at: {}", Utc::now().to_rfc3339()));

    format!(
        "chore(release): v{}\n\n{}",
        plan.next_version,
        body.join("\n")
    )
}

fn build_tag_annotation(plan: &ReleasePlan) -> String {
    format!(
        "Release {}\n\nFrom: v{}\nBump: {}\nCommits scanned: {}\nReleasable commits: {}\nRelease note entries (Conventional Commits): {}",
        plan.target_tag,
        plan.base_version,
        plan.selected_bump,
        plan.commits.len(),
        releasable_commit_count(&plan.commits),
        plan.notes.total_entries(),
    )
}

fn print_release_outcome(
    plan: &ReleasePlan,
    commit_oid: Oid,
    tag_created: bool,
    script_status: Option<&str>,
) {
    let mut rows = vec![
        ("Outcome".to_string(), "Release complete".to_string()),
        ("Version".to_string(), format!("v{}", plan.next_version)),
        ("Commit".to_string(), short_oid(commit_oid)),
        (
            "Tag".to_string(),
            if tag_created {
                plan.target_tag.clone()
            } else {
                "skipped (--no-tag)".to_string()
            },
        ),
        (
            "Commit range".to_string(),
            commit_range_label(plan.last_tag.as_ref()),
        ),
    ];

    if let Some(status) = script_status {
        rows.push(("Release script".to_string(), status.to_string()));
    }

    println!("{}", format_block(rows));
}
