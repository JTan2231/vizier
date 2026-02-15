use git2::{
    ApplyLocation, BranchType, Diff, DiffDelta, DiffFindOptions, DiffFormat, DiffLine, DiffOptions,
    DiffStatsFormat, Error, ErrorCode, Oid, Repository, RepositoryState, Status, StatusEntry,
    StatusOptions, StatusShow, Tree,
};
use std::fs;
use std::path::{Path, PathBuf};

use super::normalize_pathspec;

fn append_patch_line(buf: &mut Vec<u8>, line: DiffLine<'_>) {
    let origin = line.origin();
    if matches!(origin, '+' | '-' | ' ') {
        buf.push(origin as u8);
    }
    buf.extend_from_slice(line.content());
}

fn configure_diff_options(pathspec: Option<&str>) -> DiffOptions {
    configure_diff_options_with_binary(pathspec, false)
}

fn configure_diff_options_with_binary(pathspec: Option<&str>, show_binary: bool) -> DiffOptions {
    let mut opts = DiffOptions::new();

    opts.ignore_submodules(true)
        .id_abbrev(40)
        .show_binary(show_binary);

    if let Some(spec) = pathspec {
        opts.pathspec(spec);
    }

    opts
}

fn diff_tree_to_workdir_tolerant<'repo>(
    repo: &'repo Repository,
    base: Option<&Tree<'repo>>,
    pathspec: Option<&str>,
) -> Result<Diff<'repo>, Error> {
    diff_tree_to_workdir_tolerant_with_binary(repo, base, pathspec, false)
}

fn diff_tree_to_workdir_tolerant_with_binary<'repo>(
    repo: &'repo Repository,
    base: Option<&Tree<'repo>>,
    pathspec: Option<&str>,
    show_binary: bool,
) -> Result<Diff<'repo>, Error> {
    let mut opts = configure_diff_options_with_binary(pathspec, show_binary);

    match repo.diff_tree_to_workdir_with_index(base, Some(&mut opts)) {
        Ok(diff) => Ok(diff),
        Err(err) if err.code() == ErrorCode::NotFound => {
            let mut staged_opts = configure_diff_options_with_binary(pathspec, show_binary);
            let mut workdir_opts = configure_diff_options_with_binary(pathspec, show_binary);

            let index = repo.index()?;
            let mut staged_diff =
                repo.diff_tree_to_index(base, Some(&index), Some(&mut staged_opts))?;
            let workdir_diff = repo.diff_index_to_workdir(Some(&index), Some(&mut workdir_opts))?;

            staged_diff.merge(&workdir_diff)?;
            Ok(staged_diff)
        }
        Err(err) => Err(err),
    }
}

/// Return a unified diff (`git diff`-style patch) for the repository at `repo_path`,
/// formatted newest → oldest changes where applicable.
///
/// Assumptions:
/// - If `target` is `None`, compare HEAD (or empty tree if unborn) to working dir + index.
/// - If `target` is a single rev, compare that commit tree to working dir + index.
/// - If `target` is `<from>..<to>`, compare commit `<from>` to `<to>`.
/// - If `target` does not resolve to a rev, treat it as a path and restrict the diff there.
/// - If `exclude` is given, exclude those pathspecs (normalized) from the diff.
pub fn get_diff(
    repo_path: &str,
    target: Option<&str>, // commit/range or directory path
    // NOTE: This shouldn't match the git pathspec format, it should rather just be
    //       std::path::Pathbuf-convertable strings
    exclude: Option<&[&str]>,
) -> Result<String, Error> {
    let repo = Repository::open(repo_path)?;

    let diff = match target {
        Some(spec) if spec.contains("..") => {
            let parts: Vec<_> = spec.split("..").collect();
            if parts.len() != 2 {
                return Err(Error::from_str("Invalid double-dot range"));
            }

            let from = repo.revparse_single(parts[0])?.peel_to_tree()?;
            let to = repo.revparse_single(parts[1])?.peel_to_tree()?;

            let mut opts = configure_diff_options(None);
            repo.diff_tree_to_tree(Some(&from), Some(&to), Some(&mut opts))?
        }
        Some(spec) => {
            // Try as rev first
            match repo.revparse_single(spec) {
                Ok(obj) => {
                    let base = obj.peel_to_tree()?;
                    diff_tree_to_workdir_tolerant(&repo, Some(&base), None)?
                }
                Err(_) => {
                    // Treat as a directory/file path
                    let normalized = normalize_pathspec(spec);
                    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());

                    diff_tree_to_workdir_tolerant(
                        &repo,
                        head_tree.as_ref(),
                        Some(normalized.as_str()),
                    )?
                }
            }
        }
        None => {
            // HEAD vs working dir (with index); handle unborn HEAD
            let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());

            diff_tree_to_workdir_tolerant(&repo, head_tree.as_ref(), None)?
        }
    };

    // Excluding files from the diff with our exclude vector
    // Originally tried adding things to the pathspec, but libgit2 didn't appreciate that and
    // instead decided to ignore all possible paths when putting together the diff.
    // So, we're left with this hack.
    let mut buf = Vec::new();
    let exclude = if let Some(e) = exclude {
        e.iter().map(|p| p.to_string()).collect()
    } else {
        Vec::new()
    };

    diff.print(
        DiffFormat::Patch,
        |delta: DiffDelta<'_>, _, line: DiffLine<'_>| {
            let file_path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .and_then(|p| p.to_str());

            if let Some(path) = file_path {
                let diff_path = std::path::Path::new(path);
                if !exclude.iter().any(|excluded| {
                    let exclude_path = std::path::Path::new(excluded);

                    diff_path.starts_with(exclude_path)
                }) {
                    append_patch_line(&mut buf, line);
                }
            }
            true
        },
    )?;

    Ok(String::from_utf8_lossy(&buf).into_owned())
}

pub fn diff_binary_against_head_in<P: AsRef<Path>>(repo_path: P) -> Result<Vec<u8>, Error> {
    let repo = Repository::open(repo_path)?;
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let diff = diff_tree_to_workdir_tolerant_with_binary(&repo, head_tree.as_ref(), None, true)?;

    let mut patch = Vec::new();
    diff.print(DiffFormat::Patch, |_, _, line| {
        append_patch_line(&mut patch, line);
        true
    })?;

    Ok(patch)
}

pub fn apply_patch_with_index_in<P: AsRef<Path>>(repo_path: P, patch: &[u8]) -> Result<(), Error> {
    let repo = Repository::open(repo_path)?;
    let diff = Diff::from_buffer(patch)?;
    repo.apply(&diff, ApplyLocation::Both, None)
}

pub fn apply_patch_file_with_index_in<P: AsRef<Path>, Q: AsRef<Path>>(
    repo_path: P,
    patch_path: Q,
) -> Result<(), Error> {
    let patch = fs::read(patch_path).map_err(|err| Error::from_str(&err.to_string()))?;
    apply_patch_with_index_in(repo_path, &patch)
}

fn repo_state_label(state: RepositoryState) -> Option<&'static str> {
    match state {
        RepositoryState::Clean => None,
        RepositoryState::Merge => Some("merge in progress"),
        RepositoryState::Revert | RepositoryState::RevertSequence => Some("revert in progress"),
        RepositoryState::CherryPick | RepositoryState::CherryPickSequence => {
            Some("cherry-pick in progress")
        }
        RepositoryState::Bisect => Some("bisecting"),
        RepositoryState::Rebase
        | RepositoryState::RebaseInteractive
        | RepositoryState::RebaseMerge => Some("rebase in progress"),
        RepositoryState::ApplyMailbox | RepositoryState::ApplyMailboxOrRebase => {
            Some("apply mailbox in progress")
        }
    }
}

fn short_oid(oid: Oid) -> String {
    let text = oid.to_string();
    text.chars().take(7).collect()
}

fn branch_status_line(repo: &Repository) -> String {
    let mut line = match repo.head() {
        Ok(head) if head.is_branch() => {
            let name = head.shorthand().unwrap_or("HEAD");
            let mut out = format!("## {name}");
            if let Ok(branch) = repo.find_branch(name, BranchType::Local)
                && let Ok(upstream) = branch.upstream()
            {
                if let Ok(Some(up_name)) = upstream.name() {
                    out.push_str("...");
                    out.push_str(up_name);
                }

                if let (Some(local_oid), Some(upstream_oid)) =
                    (head.target(), upstream.get().target())
                    && let Ok((ahead, behind)) = repo.graph_ahead_behind(local_oid, upstream_oid)
                    && (ahead > 0 || behind > 0)
                {
                    out.push_str(&format!(" [ahead {ahead}, behind {behind}]"));
                }
            }
            out
        }
        Ok(head) => {
            let desc = head
                .target()
                .map(short_oid)
                .map(|oid| format!("detached {oid}"))
                .unwrap_or_else(|| "detached".to_string());
            format!("## HEAD ({desc})")
        }
        Err(_) => "## HEAD (no branch)".to_string(),
    };

    if let Some(label) = repo_state_label(repo.state()) {
        line.push_str(&format!(" ({label})"));
    }

    line
}

fn format_status_entry(entry: StatusEntry) -> Option<String> {
    let status = entry.status();
    if status.contains(Status::IGNORED) || status.is_empty() {
        return None;
    }

    let path = entry
        .head_to_index()
        .or_else(|| entry.index_to_workdir())
        .and_then(|d| d.new_file().path().or(d.old_file().path()))
        .and_then(|p| p.to_str())
        .or_else(|| entry.path())
        .unwrap_or_default()
        .to_string();

    if status.contains(Status::CONFLICTED) {
        return Some(format!("UU {path}"));
    }

    if status.contains(Status::WT_NEW) && !status.intersects(Status::INDEX_NEW) {
        return Some(format!("?? {path}"));
    }

    let index_code = if status.contains(Status::INDEX_NEW) {
        'A'
    } else if status.contains(Status::INDEX_MODIFIED) {
        'M'
    } else if status.contains(Status::INDEX_DELETED) {
        'D'
    } else if status.contains(Status::INDEX_RENAMED) {
        'R'
    } else if status.contains(Status::INDEX_TYPECHANGE) {
        'T'
    } else {
        ' '
    };

    let mut worktree_code = if status.contains(Status::WT_MODIFIED) {
        'M'
    } else if status.contains(Status::WT_DELETED) {
        'D'
    } else if status.contains(Status::WT_RENAMED) {
        'R'
    } else if status.contains(Status::WT_TYPECHANGE) {
        'T'
    } else {
        ' '
    };

    if status.contains(Status::WT_NEW) {
        worktree_code = '?';
    }

    let path_display = if status.contains(Status::INDEX_RENAMED) {
        let from = entry
            .head_to_index()
            .and_then(|d| d.old_file().path())
            .and_then(|p| p.to_str())
            .unwrap_or(path.as_str());
        let to = entry
            .head_to_index()
            .and_then(|d| d.new_file().path())
            .and_then(|p| p.to_str())
            .unwrap_or(path.as_str());
        format!("{from} -> {to}")
    } else if status.contains(Status::WT_RENAMED) {
        let from = entry
            .index_to_workdir()
            .and_then(|d| d.old_file().path())
            .and_then(|p| p.to_str())
            .unwrap_or(path.as_str());
        let to = entry
            .index_to_workdir()
            .and_then(|d| d.new_file().path())
            .and_then(|p| p.to_str())
            .unwrap_or(path.as_str());
        format!("{from} -> {to}")
    } else {
        path
    };

    Some(format!("{index_code}{worktree_code} {path_display}"))
}

/// Summarize repository status in a `git status --short --branch`-style format.
pub fn status_with_branch<P: AsRef<Path>>(repo_path: P) -> Result<String, Error> {
    let repo = Repository::discover(repo_path)?;
    if repo.workdir().is_none() {
        return Ok("## status unavailable (bare repository)".to_string());
    }

    let mut opts = StatusOptions::new();
    opts.show(StatusShow::IndexAndWorkdir)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_unmodified(false)
        .include_ignored(false)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true);

    let statuses = repo.statuses(Some(&mut opts))?;
    let mut entries: Vec<String> = statuses.iter().filter_map(format_status_entry).collect();
    entries.sort();

    let mut lines = vec![branch_status_line(&repo)];
    if !entries.is_empty() {
        lines.extend(entries);
    }

    Ok(lines.join("\n"))
}

#[derive(Debug, Clone)]
pub struct DiffSummary {
    pub stats: String,
    pub name_status: String,
}

/// Compute diff stats and name-status between `target...HEAD`, mirroring `git diff --stat` /
/// `--name-status` while relying solely on libgit2.
pub fn diff_summary_against_target<P: AsRef<Path>>(
    repo_path: P,
    target: &str,
) -> Result<DiffSummary, Error> {
    let repo = Repository::discover(repo_path)?;
    let head = repo.head()?.peel_to_commit()?;
    let target_commit = repo.revparse_single(target)?.peel_to_commit()?;
    let base_oid = repo.merge_base(target_commit.id(), head.id())?;
    let base_tree = repo.find_commit(base_oid)?.tree()?;
    let head_tree = head.tree()?;

    let mut opts = DiffOptions::new();
    opts.id_abbrev(40);

    let mut diff = repo.diff_tree_to_tree(Some(&base_tree), Some(&head_tree), Some(&mut opts))?;
    let mut find_opts = DiffFindOptions::new();
    diff.find_similar(Some(&mut find_opts))?;

    let stats_buf = diff.stats()?.to_buf(DiffStatsFormat::FULL, 80)?;
    let stats = String::from_utf8_lossy(stats_buf.as_ref())
        .trim_end()
        .to_string();

    let mut entries = Vec::new();
    for delta in diff.deltas() {
        use git2::Delta::*;
        let code = match delta.status() {
            Added => "A",
            Copied => "C",
            Deleted => "D",
            Modified => "M",
            Renamed => "R",
            Typechange => "T",
            Conflicted => "U",
            Untracked => "A",
            Unmodified | Ignored | Unreadable => continue,
        };

        let old_path = delta
            .old_file()
            .path()
            .and_then(|p| p.to_str())
            .unwrap_or_default();
        let new_path = delta
            .new_file()
            .path()
            .and_then(|p| p.to_str())
            .unwrap_or(old_path);

        let line = match delta.status() {
            Renamed | Copied => format!("{code}\t{old_path}\t{new_path}"),
            _ => format!("{code}\t{new_path}"),
        };
        entries.push(line);
    }

    entries.sort();

    Ok(DiffSummary {
        stats,
        name_status: entries.join("\n").trim().to_string(),
    })
}

pub fn repo_root() -> Result<PathBuf, Error> {
    let repo = Repository::discover(".")?;
    repo.workdir()
        .map(|dir| dir.to_path_buf())
        .ok_or_else(|| Error::from_str("repository has no working directory"))
}

pub fn ensure_clean_worktree() -> Result<(), Error> {
    let repo = Repository::discover(".")?;
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .exclude_submodules(true);
    let statuses = repo.statuses(Some(&mut opts))?;
    let has_relevant_changes = statuses.iter().any(|entry| {
        let Some(path) = entry.path() else {
            return true;
        };
        !is_ephemeral_vizier_path(path)
    });
    if has_relevant_changes {
        Err(Error::from_str(
            "working tree has uncommitted or untracked changes",
        ))
    } else {
        Ok(())
    }
}

fn is_ephemeral_vizier_path(path: &str) -> bool {
    const EPHEMERAL_PREFIXES: [&str; 4] = [
        ".vizier/jobs",
        ".vizier/sessions",
        ".vizier/tmp",
        ".vizier/tmp-worktrees",
    ];
    EPHEMERAL_PREFIXES
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(&format!("{}/", prefix)))
}

#[cfg(test)]
mod tests {
    use super::is_ephemeral_vizier_path;

    #[test]
    fn ephemeral_vizier_paths_are_excluded_from_clean_worktree_checks() {
        for path in [
            ".vizier/jobs",
            ".vizier/jobs/job-1/job.json",
            ".vizier/sessions/session-1/session.json",
            ".vizier/tmp/merge-conflicts/alpha.json",
            ".vizier/tmp-worktrees/plan-123/README.md",
        ] {
            assert!(
                is_ephemeral_vizier_path(path),
                "expected `{path}` to be treated as ephemeral"
            );
        }
    }

    #[test]
    fn non_ephemeral_paths_are_not_excluded() {
        for path in [
            ".vizier/config.toml",
            ".vizier/narrative/snapshot.md",
            ".vizier/implementation-plans/alpha.md",
            "src/main.rs",
        ] {
            assert!(
                !is_ephemeral_vizier_path(path),
                "did not expect `{path}` to be treated as ephemeral"
            );
        }
    }
}
