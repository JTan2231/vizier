use std::io::{self, IsTerminal};

use chrono::Utc;
use git2::{Repository, RepositoryState};
use vizier_core::vcs::{
    self, ReleaseBump, ReleaseCommit, ReleaseNotes, ReleaseSection, ReleaseTag, ReleaseVersion,
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

pub(crate) fn run_release(cmd: ReleaseCmd) -> Result<(), Box<dyn std::error::Error>> {
    if cmd.max_commits == 0 {
        return Err("--max-commits must be at least 1".into());
    }

    ensure_release_preconditions()?;

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

    print_release_outcome(&plan, commit_oid, tag_created);
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

fn short_oid(oid: git2::Oid) -> String {
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

fn section_total(section: &ReleaseSection) -> usize {
    section.entries.len() + section.overflow
}

fn render_notes_preview(notes: &ReleaseNotes) -> String {
    let mut lines = Vec::new();

    for section in notes.sections(true) {
        if section.is_empty() {
            continue;
        }

        lines.push(format!("{}:", section.kind.title()));
        for entry in &section.entries {
            lines.push(format!("  - {} ({})", entry.subject, entry.short_sha));
        }
        if section.overflow > 0 {
            lines.push(format!("  - +{} more", section.overflow));
        }
        lines.push(String::new());
    }

    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }

    if lines.is_empty() {
        "(no release notes entries)".to_string()
    } else {
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

    let notes = plan.notes.render_markdown(true);
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
        "Release {}\n\nFrom: v{}\nBump: {}\nCommits scanned: {}\nReleasable commits: {}\nBreaking changes: {}\nFeatures: {}\nFixes/Performance: {}\nOther: {}",
        plan.target_tag,
        plan.base_version,
        plan.selected_bump,
        plan.commits.len(),
        releasable_commit_count(&plan.commits),
        section_total(&plan.notes.breaking),
        section_total(&plan.notes.features),
        section_total(&plan.notes.fixes_performance),
        section_total(&plan.notes.other),
    )
}

fn print_release_outcome(plan: &ReleasePlan, commit_oid: git2::Oid, tag_created: bool) {
    let rows = vec![
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

    println!("{}", format_block(rows));
}
