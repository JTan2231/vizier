use git2::build::CheckoutBuilder;
use git2::{
    Commit, Error, ErrorCode, Index, IndexAddOption, ObjectType, Oid, Repository, Signature, Sort,
    Status, StatusOptions,
};
use std::path::Path;

use super::normalize_pathspec;

/// Stage changes (index-only), mirroring `git add` / `git add -u` (no commit).
///
/// - `Some(paths)`: for each normalized path:
///     * if directory → recursive add (matches `git add <dir>`).
///     * if file → add that single path.
/// - `None`: update tracked paths (like `git add -u`), staging modifications/deletions,
///   but NOT newly untracked files.
fn stage_impl(repo: &Repository, paths: Option<Vec<&str>>) -> Result<(), Error> {
    let mut index = repo.index()?;

    match paths {
        Some(list) => {
            for raw in list {
                let norm = normalize_pathspec(raw);
                let p = std::path::Path::new(&norm);
                if p.is_dir() {
                    index.add_all([p], IndexAddOption::DEFAULT, None)?;
                } else {
                    index.add_path(p)?;
                }
            }

            index.write()?;
        }
        None => {
            index.update_all(["."], None)?;
            index.write()?;
        }
    }

    Ok(())
}

fn remove_path_allow_missing(index: &mut Index, path: &Path) -> Result<(), Error> {
    match index.remove_path(path) {
        Ok(()) => Ok(()),
        Err(err) if err.code() == ErrorCode::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn stage_paths_allow_missing_impl(repo: &Repository, paths: &[&str]) -> Result<(), Error> {
    if paths.is_empty() {
        return Ok(());
    }

    let mut index = repo.index()?;

    for raw in paths {
        let norm = normalize_pathspec(raw);
        let p = std::path::Path::new(&norm);
        if p.is_dir() {
            match index.add_all([p], IndexAddOption::DEFAULT, None) {
                Ok(()) => {}
                Err(err) if err.code() == ErrorCode::NotFound => {
                    remove_path_allow_missing(&mut index, p)?;
                }
                Err(err) => return Err(err),
            }
        } else {
            match index.add_path(p) {
                Ok(()) => {}
                Err(err) if err.code() == ErrorCode::NotFound => {
                    remove_path_allow_missing(&mut index, p)?;
                }
                Err(err) => return Err(err),
            }
        }
    }

    index.write()?;
    Ok(())
}

pub fn stage(paths: Option<Vec<&str>>) -> Result<(), Error> {
    let repo = Repository::open(".")?;
    stage_impl(&repo, paths)
}

pub fn stage_paths_allow_missing(paths: &[&str]) -> Result<(), Error> {
    let repo = Repository::open(".")?;
    stage_paths_allow_missing_impl(&repo, paths)
}

pub fn stage_in<P: AsRef<Path>>(repo_path: P, paths: Option<Vec<&str>>) -> Result<(), Error> {
    let repo = Repository::open(repo_path)?;
    stage_impl(&repo, paths)
}

pub fn stage_paths_allow_missing_in<P: AsRef<Path>>(
    repo_path: P,
    paths: &[&str],
) -> Result<(), Error> {
    let repo = Repository::open(repo_path)?;
    stage_paths_allow_missing_impl(&repo, paths)
}

fn stage_all_impl(repo: &Repository) -> Result<(), Error> {
    if repo.workdir().is_none() {
        return Ok(());
    }

    let mut index = repo.index()?;
    index.add_all(["."], IndexAddOption::DEFAULT, None)?;
    index.write()?;
    Ok(())
}

/// Stage tracked + untracked changes and deletions, mirroring `git add -A`.
pub fn stage_all() -> Result<(), Error> {
    let repo = Repository::open(".")?;
    stage_all_impl(&repo)
}

/// Stage tracked + untracked changes and deletions, mirroring `git add -A`.
pub fn stage_all_in<P: AsRef<Path>>(repo_path: P) -> Result<(), Error> {
    let repo = Repository::open(repo_path)?;
    stage_all_impl(&repo)
}

// TODO: Remove the `add` portion from this
/// Stage changes and create a commit in the current repository, returning the new commit `Oid`.
///
/// Assumptions:
/// - If `paths` is `Some`, each path is normalized and added:
///   - Directories → `git add <dir>` (recursive).
///   - Files → `git add <file>`.
/// - If `paths` is `None` and `allow_empty` is `false`, behaves like `git add -u`
///   (updates tracked files, removes deleted).
/// - If `allow_empty` is `false`, and the resulting tree matches the parent’s, returns an error.
/// - If no parent exists (unborn branch), commit has no parents.
/// - Commit metadata uses repo config signature if available, else falls back to
///   `"Vizier <vizier@local>"`.
fn add_and_commit_impl(
    repo: &Repository,
    paths: Option<Vec<&str>>,
    message: &str,
    allow_empty: bool,
) -> Result<Oid, git2::Error> {
    let mut index = repo.index()?;

    match paths {
        Some(paths) => {
            for raw in paths {
                let norm = normalize_pathspec(raw);
                let p = std::path::Path::new(&norm);
                if p.is_dir() {
                    index.add_all([p], git2::IndexAddOption::DEFAULT, None)?;
                } else {
                    index.add_path(p)?;
                }
            }
        }
        None => {
            if !allow_empty {
                // git add -u (update tracked, remove deleted)
                index.update_all(["."], None)?;
            }
        }
    }

    index.write()?;
    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;

    // Prefer config-driven signature if available
    let signature = repo
        .signature()
        .or_else(|_| Signature::now("Vizier", "vizier@local"))?;

    // Parent(s)
    let parent_commit = repo.head().ok().and_then(|h| h.peel_to_commit().ok());

    if !allow_empty
        && parent_commit
            .as_ref()
            .is_some_and(|parent| parent.tree_id() == tree_id)
    {
        return Err(git2::Error::from_str("nothing to commit"));
    }

    let parents: Vec<&git2::Commit> = match parent_commit.as_ref() {
        Some(p) => vec![p],
        None => vec![],
    };

    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parents,
    )
}

pub fn add_and_commit(
    paths: Option<Vec<&str>>,
    message: &str,
    allow_empty: bool,
) -> Result<Oid, git2::Error> {
    let repo = Repository::open(".")?;
    add_and_commit_impl(&repo, paths, message, allow_empty)
}

pub fn add_and_commit_in<P: AsRef<Path>>(
    repo_path: P,
    paths: Option<Vec<&str>>,
    message: &str,
    allow_empty: bool,
) -> Result<Oid, git2::Error> {
    let repo = Repository::open(repo_path)?;
    add_and_commit_impl(&repo, paths, message, allow_empty)
}

fn commit_staged_impl(
    repo: &Repository,
    message: &str,
    allow_empty: bool,
) -> Result<Oid, git2::Error> {
    let mut index = repo.index()?;
    index.write()?;
    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;

    let signature = repo
        .signature()
        .or_else(|_| Signature::now("Vizier", "vizier@local"))?;
    let parent_commit = repo.head().ok().and_then(|h| h.peel_to_commit().ok());

    if !allow_empty {
        if let Some(ref parent) = parent_commit {
            if parent.tree_id() == tree_id {
                return Err(git2::Error::from_str("nothing to commit"));
            }
        } else if index.is_empty() {
            return Err(git2::Error::from_str("nothing to commit"));
        }
    }

    let parents: Vec<&git2::Commit> = match parent_commit.as_ref() {
        Some(p) => vec![p],
        None => vec![],
    };

    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parents,
    )
}

pub fn commit_staged(message: &str, allow_empty: bool) -> Result<Oid, git2::Error> {
    let repo = Repository::open(".")?;
    commit_staged_impl(&repo, message, allow_empty)
}

pub fn commit_staged_in<P: AsRef<Path>>(
    repo_path: P,
    message: &str,
    allow_empty: bool,
) -> Result<Oid, git2::Error> {
    let repo = Repository::open(repo_path)?;
    commit_staged_impl(&repo, message, allow_empty)
}

/// Return up to `depth` commits whose messages match any of the `filters` (OR),
/// Returns up to `depth` commits (newest -> oldest) whose *full* messages
/// contain ANY of the provided `filters` (case-insensitive).
/// The returned String contains each commit's entire message (subject + body),
/// with original newlines preserved. Between commits, a simple header demarcates entries.
pub fn get_log(depth: usize, filters: Option<Vec<String>>) -> Result<Vec<String>, Error> {
    let repo = Repository::discover(".")?;

    let mut walk = repo.revwalk()?;
    walk.push_head()?;
    walk.set_sorting(Sort::TIME)?; // newest -> oldest by committer time

    let needles: Vec<String> = filters
        .unwrap_or_default()
        .into_iter()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect();
    let use_filters = !needles.is_empty();

    let mut out = Vec::new();
    let mut kept = 0usize;

    for oid_res in walk {
        let oid = oid_res?;
        let commit = repo.find_commit(oid)?;

        let msg = commit
            .message()
            .map(|s| s.to_owned())
            .unwrap_or_else(|| String::from_utf8_lossy(commit.message_bytes()).into_owned());

        let keep = if use_filters {
            let msg_lc = msg.to_lowercase();
            needles.iter().any(|n| msg_lc.contains(n))
        } else {
            true
        };

        if !keep {
            continue;
        }

        let sha = oid.to_string();
        let short_sha = &sha[..7.min(sha.len())];
        let author = commit.author().name().unwrap_or("<unknown>").to_string();

        let mut out_msg = String::new();

        out_msg.push_str(&format!("commit {short_sha} — {author}\n"));
        out_msg.push_str(&msg);
        if !msg.ends_with('\n') {
            out_msg.push('\n');
        }

        out_msg.push('\n');

        out.push(out_msg);

        kept += 1;
        if kept >= depth {
            break;
        }
    }

    Ok(out)
}

pub fn commit_paths_in_repo(
    repo_path: &Path,
    paths: &[&Path],
    message: &str,
) -> Result<Oid, Error> {
    if paths.is_empty() {
        return Err(Error::from_str("no paths provided for commit"));
    }

    let repo = Repository::discover(repo_path)?;
    let mut index = repo.index()?;
    let repo_root = repo.workdir().unwrap_or(repo_path);

    let mut rel_paths = Vec::with_capacity(paths.len());
    for path in paths {
        if path.is_absolute() {
            let relative = path
                .strip_prefix(repo_root)
                .map_err(|_| Error::from_str("absolute path is outside of the repository root"))?;
            rel_paths.push(relative.to_path_buf());
        } else {
            rel_paths.push(path.to_path_buf());
        }
    }

    for rel in &rel_paths {
        index.add_path(rel)?;
    }
    index.write()?;

    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;

    let signature = repo
        .signature()
        .or_else(|_| Signature::now("Vizier", "vizier@local"))?;

    let parent_commit = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
    let parent_commits: Vec<Commit> = parent_commit.into_iter().collect();
    let parent_refs: Vec<&Commit> = parent_commits.iter().collect();

    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parent_refs,
    )
}

/// Unstage changes (index-only), mirroring `git restore --staged` / `git reset -- <paths>`.
///
/// Behavior:
/// - If `paths` is `Some`, paths are normalized and only those paths are reset in the index:
///     - If `HEAD` exists, index entries for those paths become exactly `HEAD`’s versions.
///     - If `HEAD` is unborn, those paths are removed from the index (i.e., fully unstaged).
/// - If `paths` is `None`:
///     - If `HEAD` exists, the entire index is reset to `HEAD`’s tree (no working tree changes).
///     - If `HEAD` is unborn, the index is cleared.
/// - Never updates the working directory, and never moves `HEAD`.
fn unstage_impl(repo: &Repository, paths: Option<Vec<&str>>) -> Result<(), Error> {
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let mut index = repo.index()?;

    match (paths, head_tree) {
        (Some(list), Some(_head_tree)) => {
            // NOTE: reset_default requires &[&Path]
            let owned: Vec<std::path::PathBuf> = list
                .into_iter()
                .map(|p| std::path::PathBuf::from(normalize_pathspec(p)))
                .collect();

            let spec: Vec<&std::path::Path> = owned.iter().map(|p| p.as_path()).collect();
            let head = match repo.head() {
                Ok(h) => h,
                Err(_) => {
                    for p in &owned {
                        remove_path_allow_missing(&mut index, p.as_path())?;
                    }

                    index.write()?;
                    return Ok(());
                }
            };

            let head_obj = head.resolve()?.peel(ObjectType::Commit)?;

            match repo.reset_default(Some(&head_obj), &spec) {
                Ok(()) => {}
                Err(err) if err.code() == ErrorCode::NotFound => {
                    for p in &owned {
                        remove_path_allow_missing(&mut index, p.as_path())?;
                    }
                    index.write()?;
                }
                Err(err) => return Err(err),
            }
        }

        (Some(list), None) => {
            for raw in list {
                let norm = normalize_pathspec(raw);
                remove_path_allow_missing(&mut index, std::path::Path::new(&norm))?;
            }

            index.write()?;
        }

        (None, Some(head_tree)) => {
            index.read_tree(&head_tree)?;
            index.write()?;
        }

        (None, None) => {
            index.clear()?;
            index.write()?;
        }
    }

    Ok(())
}

pub fn unstage(paths: Option<Vec<&str>>) -> Result<(), Error> {
    let repo = Repository::open(".")?;
    unstage_impl(&repo, paths)
}

pub fn unstage_in<P: AsRef<Path>>(repo_path: P, paths: Option<Vec<&str>>) -> Result<(), Error> {
    let repo = Repository::open(repo_path)?;
    unstage_impl(&repo, paths)
}

#[derive(Debug, Clone)]
pub enum StagedKind {
    Added,                                // INDEX_NEW
    Modified,                             // INDEX_MODIFIED
    Deleted,                              // INDEX_DELETED
    TypeChange,                           // INDEX_TYPECHANGE
    Renamed { from: String, to: String }, // INDEX_RENAMED
}

#[derive(Debug, Clone)]
pub struct StagedItem {
    pub path: String, // primary path (for rename, the NEW path)
    pub kind: StagedKind,
}

/// Capture the current staged set (index vs HEAD), losslessly enough to restore.
pub fn snapshot_staged(repo_path: &str) -> Result<Vec<StagedItem>, Error> {
    let repo = Repository::open(repo_path)?;
    let mut opts = StatusOptions::new();
    // We want staged/index changes relative to HEAD:
    opts.include_untracked(false)
        .include_ignored(false)
        .renames_head_to_index(true)
        .renames_index_to_workdir(false)
        .update_index(false)
        .include_unmodified(false)
        .show(git2::StatusShow::Index);

    let statuses = repo.statuses(Some(&mut opts))?;
    let mut out = Vec::new();

    for entry in statuses.iter() {
        let s = entry.status();

        // Renames: libgit2 provides both paths when rename detection is enabled.
        if s.contains(Status::INDEX_RENAMED) {
            let from = entry
                .head_to_index()
                .and_then(|d| d.old_file().path())
                .and_then(|p| p.to_str())
                .unwrap_or_default()
                .to_string();

            let to = entry
                .head_to_index()
                .and_then(|d| d.new_file().path())
                .and_then(|p| p.to_str())
                .unwrap_or_default()
                .to_string();

            out.push(StagedItem {
                path: to.clone(),
                kind: StagedKind::Renamed { from, to },
            });
            continue;
        }

        let path = entry
            .head_to_index()
            .or_else(|| entry.index_to_workdir())
            .and_then(|d| d.new_file().path().or(d.old_file().path()))
            .and_then(|p| p.to_str())
            .unwrap_or_default()
            .to_string();

        let kind = if s.contains(Status::INDEX_NEW) {
            StagedKind::Added
        } else if s.contains(Status::INDEX_MODIFIED) {
            StagedKind::Modified
        } else if s.contains(Status::INDEX_DELETED) {
            StagedKind::Deleted
        } else if s.contains(Status::INDEX_TYPECHANGE) {
            StagedKind::TypeChange
        } else {
            // skip anything that isn't index-staged
            continue;
        };

        out.push(StagedItem { path, kind });
    }

    Ok(out)
}

/// Restore the staged set exactly as captured by `snapshot_staged`.
/// Index-only; does not modify worktree or HEAD.
pub fn restore_staged(repo_path: &str, staged: &[StagedItem]) -> Result<(), Error> {
    let repo = Repository::open(repo_path)?;
    let mut index = repo.index()?;

    for item in staged {
        match &item.kind {
            StagedKind::Added | StagedKind::Modified | StagedKind::TypeChange => {
                index.add_path(std::path::Path::new(&item.path))?;
            }
            StagedKind::Deleted => {
                index.remove_path(std::path::Path::new(&item.path))?;
            }
            StagedKind::Renamed { from, to } => {
                index.remove_path(std::path::Path::new(from))?;
                index.add_path(std::path::Path::new(to))?;
            }
        }
    }

    index.write()?;
    Ok(())
}

pub fn amend_head_commit(message: Option<&str>) -> Result<Oid, Error> {
    let repo = Repository::discover(".")?;
    let head = repo.head()?;
    if !head.is_branch() {
        return Err(Error::from_str("cannot amend detached HEAD"));
    }
    let mut index = repo.index()?;
    if index.has_conflicts() {
        return Err(Error::from_str(
            "cannot amend commit while conflicts remain",
        ));
    }
    index.write()?;
    let tree_oid = index.write_tree_to(&repo)?;
    let tree = repo.find_tree(tree_oid)?;
    let head_commit = head.peel_to_commit()?;
    let sig = repo.signature()?;
    let content = message
        .map(|msg| msg.to_string())
        .or_else(|| head_commit.message().map(|s| s.to_string()))
        .unwrap_or_else(|| "amended commit".to_string());
    let oid = head_commit.amend(
        Some("HEAD"),
        Some(&sig),
        Some(&sig),
        None,
        Some(&content),
        Some(&tree),
    )?;

    let mut checkout = CheckoutBuilder::new();
    checkout.force();
    repo.checkout_head(Some(&mut checkout))?;

    Ok(oid)
}
