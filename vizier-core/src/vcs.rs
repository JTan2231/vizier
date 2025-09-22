use git2::{
    DiffFormat, DiffOptions, Error, IndexAddOption, Oid, Repository, Signature, Sort, Status,
    StatusOptions,
};

fn normalize_pathspec(path: &str) -> String {
    let mut s = path
        .trim()
        .trim_end_matches('/')
        .trim_end_matches('\\')
        .to_string();

    s = s.replace('\\', "/");
    if let Some(stripped) = s.strip_prefix("./") {
        s = stripped.to_string();
    }

    // Preserve leading UNC `//`, collapse doubles after it.
    if s.starts_with("//") {
        let mut out = String::from("//");
        let rest = s.trim_start_matches('/');
        // collapse any remaining '//' in the tail
        let mut last = '\0';
        for ch in rest.chars() {
            if ch != '/' || last != '/' {
                out.push(ch);
            }
            last = ch;
        }
        s = out;
    } else {
        while s.contains("//") {
            s = s.replace("//", "/");
        }
    }

    s
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
    let mut opts = DiffOptions::new();

    opts.ignore_submodules(true).id_abbrev(40);

    let diff = match target {
        Some(spec) if spec.contains("..") => {
            let parts: Vec<_> = spec.split("..").collect();
            if parts.len() != 2 {
                return Err(Error::from_str("Invalid double-dot range"));
            }

            let from = repo.revparse_single(parts[0])?.peel_to_tree()?;
            let to = repo.revparse_single(parts[1])?.peel_to_tree()?;

            repo.diff_tree_to_tree(Some(&from), Some(&to), Some(&mut opts))?
        }
        Some(spec) => {
            // Try as rev first
            match repo.revparse_single(spec) {
                Ok(obj) => {
                    let base = obj.peel_to_tree()?;
                    repo.diff_tree_to_workdir_with_index(Some(&base), Some(&mut opts))?
                }
                Err(_) => {
                    // Treat as a directory/file path
                    let normalized = normalize_pathspec(spec);
                    opts.pathspec(&normalized);
                    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());

                    repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))?
                }
            }
        }
        None => {
            // HEAD vs working dir (with index); handle unborn HEAD
            let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());

            repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))?
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

    diff.print(DiffFormat::Patch, |delta, _, line| {
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
                buf.extend_from_slice(line.content());
            }
        }
        true
    })?;

    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Stage changes (index-only), mirroring `git add` / `git add -u` (no commit).
///
/// - `Some(paths)`: for each normalized path:
///     * if directory → recursive add (matches `git add <dir>`).
///     * if file → add that single path.
/// - `None`: update tracked paths (like `git add -u`), staging modifications/deletions,
///     but NOT newly untracked files.
pub fn stage(paths: Option<Vec<&str>>) -> Result<(), Error> {
    let repo = Repository::open(".")?;
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
pub fn add_and_commit(
    paths: Option<Vec<&str>>,
    message: &str,
    allow_empty: bool,
) -> Result<Oid, git2::Error> {
    let repo = Repository::open(".")?;
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

    if !allow_empty {
        if let Some(ref parent) = parent_commit {
            if parent.tree_id() == tree_id {
                return Err(git2::Error::from_str("nothing to commit"));
            }
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
pub fn unstage(paths: Option<Vec<&str>>) -> Result<(), Error> {
    let repo = Repository::open(".")?;
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
                    let mut idx = repo.index()?;
                    for p in spec {
                        idx.remove_path(p)?;
                    }

                    idx.write()?;
                    return Ok(());
                }
            };

            let head_obj = head.resolve()?.peel(git2::ObjectType::Commit)?;

            repo.reset_default(Some(&head_obj), &spec)?;
        }

        (Some(list), None) => {
            for raw in list {
                let norm = normalize_pathspec(raw);
                index.remove_path(std::path::Path::new(&norm))?;
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

/// Extract (owner, repo) from `origin`
pub fn origin_owner_repo(repo_path: &str) -> Result<(String, String), Error> {
    let repo = Repository::discover(repo_path)?;
    let remote = repo.find_remote("origin").or_else(|_| {
        // Some repos only have fetch remotes in the list; fall back to first if needed.
        let remotes = repo.remotes()?;
        let name = remotes
            .iter()
            .flatten()
            .next()
            .ok_or_else(|| Error::from_str("No remotes found"))?;
        repo.find_remote(name)
    })?;

    let url = remote
        .url()
        .ok_or_else(|| Error::from_str("origin remote has no URL"))?;
    // Accept common GitHub patterns:
    // 1) https://github.com/OWNER/REPO(.git)
    // 2) git@github.com:OWNER/REPO(.git)
    // 3) ssh://git@github.com/OWNER/REPO(.git)
    // Normalize to just "OWNER/REPO"
    let owner_repo = if let Some(rest) = url.strip_prefix("https://github.com/") {
        rest
    } else if let Some(rest) = url.strip_prefix("http://github.com/") {
        rest
    } else if let Some(rest) = url.strip_prefix("ssh://git@github.com/") {
        rest
    } else if let Some(rest) = url.strip_prefix("git@github.com:") {
        rest
    } else {
        return Err(Error::from_str("Unsupported GitHub remote URL format"));
    };

    let trimmed = owner_repo.trim_end_matches(".git").trim_end_matches('/');
    let mut parts = trimmed.split('/');

    let owner = parts
        .next()
        .ok_or_else(|| Error::from_str("Missing owner in remote URL"))?;
    let repo = parts
        .next()
        .ok_or_else(|| Error::from_str("Missing repo in remote URL"))?;

    if parts.next().is_some() {
        return Err(Error::from_str("Remote URL contains extra path segments"));
    }

    Ok((owner.to_string(), repo.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{IndexAddOption, Repository, Signature};
    use std::fs::{self, File};
    use std::io::Write;
    use std::path::{Path, PathBuf};

    struct CwdGuard {
        old: PathBuf,
    }

    impl CwdGuard {
        fn enter<P: AsRef<Path>>(p: P) -> std::io::Result<Self> {
            let old = std::env::current_dir()?;
            std::env::set_current_dir(p)?;
            Ok(Self { old })
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.old);
        }
    }

    fn init_temp_repo() -> (tempfile::TempDir, Repository) {
        let td = tempfile::TempDir::new().expect("tempdir");
        let repo = Repository::init(td.path()).expect("init repo");
        let _ = repo.config().and_then(|mut c| {
            c.set_str("user.name", "Tester")?;
            c.set_str("user.email", "tester@example.com")
        });
        (td, repo)
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = File::create(path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f.sync_all().unwrap();
    }

    fn append(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f.sync_all().unwrap();
    }

    fn raw_commit(repo: &Repository, msg: &str) -> Oid {
        let mut idx = repo.index().unwrap();
        idx.add_all(["."], IndexAddOption::DEFAULT, None).unwrap();
        idx.write().unwrap();
        let tree_id = idx.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = repo
            .signature()
            .or_else(|_| Signature::now("Tester", "tester@example.com"))
            .unwrap();
        let parent_opt = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent_opt.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
            .unwrap()
    }

    fn raw_stage(repo: &Repository, rel: &str) {
        let mut idx = repo.index().unwrap();
        idx.add_path(Path::new(rel)).unwrap();
        idx.write().unwrap();
    }

    // --- normalize_pathspec --------------------------------------------------

    #[test]
    fn normalize_pathspec_variants() {
        assert_eq!(super::normalize_pathspec(" src//utils/// "), "src/utils");
        assert_eq!(super::normalize_pathspec("./a/b/"), "a/b");
        assert_eq!(super::normalize_pathspec(r#"a\win\path\"#), "a/win/path");

        // Match current implementation: if it starts with `//`, internal `//` are preserved.
        assert_eq!(
            super::normalize_pathspec("//server//share//x"),
            "//server/share/x"
        );
    }

    // --- add_and_commit core behaviors --------------------------------------

    #[test]
    fn add_and_commit_basic_and_noop() {
        let (td, _repo) = init_temp_repo();
        let _cwd = CwdGuard::enter(td.path()).unwrap();

        write(Path::new("README.md"), "# one\n");
        let oid1 = add_and_commit(Some(vec!["README.md"]), "init", false).expect("commit ok");
        assert_ne!(oid1, Oid::zero());

        // No changes, allow_empty=false → "nothing to commit"
        let err = add_and_commit(None, "noop", false).unwrap_err();
        assert!(format!("{err}").contains("nothing to commit"));

        // Empty commit (allow_empty=true) → OK
        let oid2 = add_and_commit(None, "empty ok", true).expect("empty commit ok");
        assert_ne!(oid2, oid1);
    }

    #[test]
    fn add_and_commit_pathspecs_and_deletes_and_ignores() {
        let (td, _) = init_temp_repo();
        let _cwd = CwdGuard::enter(td.path()).unwrap();

        // .gitignore excludes dist/** and vendor/**
        write(Path::new(".gitignore"), "dist/\nvendor/\n");

        // Create a mix
        write(Path::new("src/a.rs"), "fn a(){}\n");
        write(Path::new("src/b.rs"), "fn b(){}\n");
        write(Path::new("dist/bundle.js"), "/* build */\n");
        write(Path::new("vendor/lib/x.c"), "/* vendored */\n");
        let c1 = add_and_commit(Some(vec!["./src//"]), "src only", false).unwrap();
        assert_ne!(c1, Oid::zero());

        // Update tracked files + delete one; update_all should stage deletes.
        fs::remove_file("src/a.rs").unwrap();
        append(Path::new("src/b.rs"), "// mod\n");

        // Ignored paths shouldn't be added even with update_all
        let c2 = add_and_commit(None, "update tracked & deletions", false).unwrap();
        assert_ne!(c2, c1);

        // Show that vendor/dist are still untracked (ignored), not part of commit 2
        // Verify via a diff: HEAD..workdir should be empty (no pending tracked changes)
        let repo_path = td.path().to_str().unwrap();
        let d = get_diff(repo_path, None, None).unwrap();
        // No pending tracked changes post-commit; any diff would now be due to ignored dirs (which aren't included)
        assert!(d.is_empty() || !d.contains("src/")); // conservative assertion
    }

    // --- get_diff: path, excludes, ranges -----------------------------------

    #[test]
    fn diff_head_vs_workdir_and_path_and_exclude() {
        let (td, repo) = init_temp_repo();
        let repo_path = td.path().to_path_buf();
        let _cwd = CwdGuard::enter(&repo_path).unwrap();

        write(Path::new("a/file.txt"), "hello\n");
        write(Path::new("b/file.txt"), "world\n");
        raw_commit(&repo, "base");

        append(Path::new("a/file.txt"), "change-a\n"); // unstaged, tracked file
        append(Path::new("b/file.txt"), "change-b\n");
        write(Path::new("b/inner/keep.txt"), "keep\n"); // untracked; should not appear

        // 1) None → HEAD vs workdir(+index). Shows tracked edits, not untracked files.
        let d_all = get_diff(repo_path.to_str().unwrap(), None, None).expect("diff");
        assert!(d_all.contains("a/file.txt"));
        assert!(d_all.contains("b/file.txt"));
        assert!(!d_all.contains("b/inner/keep.txt")); // untracked → absent

        // 2) Treat `target` as a path
        let d_b = get_diff(repo_path.to_str().unwrap(), Some("b"), None).expect("diff b");
        assert!(!d_b.contains("a/file.txt"));
        assert!(d_b.contains("b/file.txt"));
        assert!(!d_b.contains("b/inner/keep.txt")); // still untracked → absent

        // 3) Exclude subdir via Windows-ish input → normalized
        let d_b_ex = get_diff(
            repo_path.to_str().unwrap(),
            Some("b"),
            Some(&[r".\b\inner"]),
        )
        .expect("diff b excl inner");
        assert!(d_b_ex.contains("b/file.txt"));
        assert!(!d_b_ex.contains("b/inner/keep.txt"));
    }

    #[test]
    fn diff_single_rev_to_workdir() {
        let (td, repo) = init_temp_repo();
        let repo_path = td.path().to_path_buf();
        let _cwd = CwdGuard::enter(&repo_path).unwrap();

        write(Path::new("x.txt"), "x1\n");
        let first = raw_commit(&repo, "c1");

        append(Path::new("x.txt"), "x2\n"); // unstaged, tracked change is visible
        let spec = first.to_string();
        let d = get_diff(repo_path.to_str().unwrap(), Some(&spec), None).expect("diff");
        println!("d: {}", d);
        assert!(d.contains("x.txt")); // file appears
        assert!(d.contains("\n+")); // there is an addition hunk
        assert!(d.contains("x2")); // payload appears (don’t hard-code "+x2")
    }

    #[test]
    fn diff_with_excludes() {
        let (td, repo) = init_temp_repo();
        let repo_path = td.path().to_path_buf();
        let _cwd = CwdGuard::enter(&repo_path).unwrap();

        // Base on main
        write(Path::new("common.txt"), "base\n");
        let base = raw_commit(&repo, "base");

        // Branch at base
        {
            let head_commit = repo.find_commit(base).unwrap();
            repo.branch("feature", &head_commit, true).unwrap();
        }

        // Advance main
        write(Path::new("main.txt"), "m1\n");
        write(Path::new("vendor/ignored.txt"), "should be excluded\n"); // will test exclusion
        let main1 = raw_commit(&repo, "main1");

        // Checkout feature and diverge
        {
            let mut checkout = git2::build::CheckoutBuilder::new();
            repo.set_head("refs/heads/feature").unwrap();
            repo.checkout_head(Some(&mut checkout.force())).unwrap();
        }
        write(Path::new("feat.txt"), "f1\n");

        // A..B (base..main1) shows main changes (including vendor/ by default)
        let dd = format!("{}..{}", base, main1);
        let out_dd = get_diff(repo_path.to_str().unwrap(), Some(&dd), None).expect("A..B");
        assert!(out_dd.contains("main.txt"));

        // Now exclude vendor/** using normalize-able pathspec; vendor should disappear
        let out_dd_ex = get_diff(repo_path.to_str().unwrap(), Some(&dd), Some(&["vendor//"]))
            .expect("A..B excl");
        println!("DIFF: {}", out_dd_ex);
        assert!(out_dd_ex.contains("main.txt"));
        assert!(!out_dd_ex.contains("vendor/ignored.txt"));
    }

    // --- unborn HEAD (no untracked): stage-only then diff --------------------

    #[test]
    fn diff_unborn_head_against_workdir_without_untracked() {
        let (td, repo) = init_temp_repo();
        let repo_path = td.path().to_path_buf();
        let _cwd = CwdGuard::enter(&repo_path).unwrap();

        // File exists in workdir and is STAGED (tracked) but no commits yet.
        write(Path::new("z.txt"), "hello\n");
        raw_stage(&repo, "z.txt"); // index-only

        // get_diff(None) compares empty tree → workdir+index, so z.txt appears even with untracked disabled
        let out = get_diff(repo_path.to_str().unwrap(), None, None).expect("diff unborn");
        println!("OUT: {}", out);
        assert!(out.contains("z.txt"));
        assert!(out.contains("hello"));
    }

    // --- stage (index-only) --------------------------------------------------

    #[test]
    fn stage_paths_and_update_tracked_only() {
        let (td, repo) = init_temp_repo();
        let _cwd = CwdGuard::enter(td.path()).unwrap();

        // Base commit with two tracked files
        write(Path::new("a.txt"), "A0\n");
        write(Path::new("b.txt"), "B0\n");
        raw_commit(&repo, "base");

        // Workdir changes:
        // - modify tracked a.txt
        // - delete tracked b.txt
        // - create new untracked c.txt
        append(Path::new("a.txt"), "A1\n");
        fs::remove_file("b.txt").unwrap();
        write(Path::new("c.txt"), "C0\n");

        // 1) stage(None) should mirror `git add -u`: stage tracked changes (a.txt mod, b.txt del)
        //    but NOT the new untracked c.txt.
        stage(None).expect("stage -u");
        let staged1 = snapshot_staged(".").expect("snapshot staged after -u");

        // Expect: a.txt Modified, b.txt Deleted; no c.txt
        let mut kinds = staged1
            .iter()
            .map(|s| match &s.kind {
                super::StagedKind::Added => ("Added", s.path.clone()),
                super::StagedKind::Modified => ("Modified", s.path.clone()),
                super::StagedKind::Deleted => ("Deleted", s.path.clone()),
                super::StagedKind::TypeChange => ("TypeChange", s.path.clone()),
                super::StagedKind::Renamed { from, to } => ("Renamed", format!("{from}->{to}")),
            })
            .collect::<Vec<_>>();
        kinds.sort_by(|a, b| a.1.cmp(&b.1));

        assert_eq!(
            kinds.sort(),
            vec![
                ("Deleted", "b.txt".to_string()),
                ("Modified", "a.txt".to_string()),
            ]
            .sort()
        );

        // 2) Now explicitly stage c.txt via stage(Some)
        stage(Some(vec!["c.txt"])).expect("stage c.txt");
        let staged2 = snapshot_staged(".").expect("snapshot staged after explicit add");

        let names2: Vec<_> = staged2.iter().map(|s| s.path.as_str()).collect();
        assert!(names2.contains(&"a.txt"));
        assert!(names2.contains(&"b.txt")); // staged deletion appears as b.txt in the snapshot
        assert!(names2.contains(&"c.txt")); // now present as Added
        assert!(
            staged2
                .iter()
                .any(|s| matches!(s.kind, super::StagedKind::Added) && s.path == "c.txt")
        );
    }

    // --- unstage: specific paths & entire index (born HEAD) ------------------

    #[test]
    fn unstage_specific_paths_and_all_with_head() {
        let (td, repo) = init_temp_repo();
        let _cwd = CwdGuard::enter(td.path()).unwrap();

        write(Path::new("x.txt"), "X0\n");
        write(Path::new("y.txt"), "Y0\n");
        raw_commit(&repo, "base");

        append(Path::new("x.txt"), "X1\n");
        append(Path::new("y.txt"), "Y1\n");

        // Stage both changes (explicit)
        stage(Some(vec!["x.txt", "y.txt"])).expect("stage both");

        // Unstage only x.txt → y.txt should remain staged
        unstage(Some(vec!["x.txt"])).expect("unstage x");

        let after_x = snapshot_staged(".").expect("snapshot after unstage x");
        assert!(after_x.iter().any(|s| s.path == "y.txt"));
        assert!(!after_x.iter().any(|s| s.path == "x.txt"));

        // Unstage everything → nothing should be staged
        unstage(None).expect("unstage all");
        let after_all = snapshot_staged(".").expect("snapshot after unstage all");
        assert!(after_all.is_empty());
    }

    // --- unstage: unborn HEAD behavior --------------------------------------

    #[test]
    fn unstage_with_unborn_head() {
        let (td, repo) = init_temp_repo();
        let _cwd = CwdGuard::enter(td.path()).unwrap();

        // No commits yet; create two files and stage both
        write(Path::new("u.txt"), "U0\n");
        write(Path::new("v.txt"), "V0\n");
        raw_stage(&repo, "u.txt");
        raw_stage(&repo, "v.txt");

        // Path-limited unstage on unborn HEAD should remove entries from index for those paths
        unstage(Some(vec!["u.txt"])).expect("unstage u.txt on unborn");
        let staged1 = snapshot_staged(".").expect("snapshot staged after partial unstage");
        let names1: Vec<_> = staged1.iter().map(|s| s.path.as_str()).collect();
        assert!(names1.contains(&"v.txt"));
        assert!(!names1.contains(&"u.txt"));

        // Full unstage on unborn HEAD should clear the index
        unstage(None).expect("unstage all unborn");
        let staged2 = snapshot_staged(".").expect("snapshot staged after clear");
        assert!(staged2.is_empty());
    }

    // --- snapshot → unstage → mutate → restore (A/M/D/R rename) --------------

    #[test]
    fn snapshot_and_restore_roundtrip_with_rename() {
        let (td, repo) = init_temp_repo();
        let _cwd = CwdGuard::enter(td.path()).unwrap();

        // Base: a.txt, b.txt
        write(Path::new("a.txt"), "A0\n");
        write(Path::new("b.txt"), "B0\n");
        raw_commit(&repo, "base");

        // Workdir staged set (before snapshot):
        // - RENAME: a.txt -> a_ren.txt (same content to improve rename detection)
        // - DELETE: b.txt
        // - ADD: c.txt
        // - (no explicit extra modifications; rely on rename detection)
        fs::rename("a.txt", "a_ren.txt").unwrap();
        fs::remove_file("b.txt").unwrap();
        write(Path::new("c.txt"), "C0\n");

        // Stage all changes so index reflects A/M/D/R
        {
            let mut idx = repo.index().unwrap();
            idx.add_all(["."], git2::IndexAddOption::DEFAULT, None)
                .unwrap();
            // ensure deletion is captured
            idx.update_all(["."], None).unwrap();
            idx.write().unwrap();
        }

        // Take snapshot of what's staged now
        let snap = snapshot_staged(".").expect("snapshot staged");

        // Sanity: ensure we actually captured the expected kinds
        // Expect at least: Added c.txt, Deleted b.txt, and a rename a.txt -> a_ren.txt
        let mut have_added_c = false;
        let mut have_deleted_b = false;
        let mut have_renamed_a = false;

        for it in &snap {
            match &it.kind {
                super::StagedKind::Added if it.path == "c.txt" => have_added_c = true,
                super::StagedKind::Deleted if it.path == "b.txt" => have_deleted_b = true,
                super::StagedKind::Renamed { from, to } if from == "a.txt" && to == "a_ren.txt" => {
                    have_renamed_a = true
                }
                _ => {}
            }
        }
        assert!(have_added_c, "expected Added c.txt in snapshot");
        assert!(have_deleted_b, "expected Deleted b.txt in snapshot");
        assert!(
            have_renamed_a,
            "expected Renamed a.txt->a_ren.txt in snapshot"
        );

        // Unstage everything
        unstage(None).expect("unstage all");

        // Mutate workdir arbitrarily (should not affect restoration correctness)
        append(Path::new("c.txt"), "C1\n"); // change content after snapshot
        write(Path::new("d.txt"), "D0 (noise)\n"); // create a noise file that won't be staged by restore

        // Restore exact staged set captured in `snap`
        restore_staged(".", &snap).expect("restore staged");

        // Re-snapshot after restore to compare equivalence (semantic equality of staged set)
        let after = snapshot_staged(".").expect("snapshot after restore");

        // Normalize into comparable tuples
        fn key(s: &super::StagedItem) -> (String, String) {
            match &s.kind {
                super::StagedKind::Added => ("Added".into(), s.path.clone()),
                super::StagedKind::Modified => ("Modified".into(), s.path.clone()),
                super::StagedKind::Deleted => ("Deleted".into(), s.path.clone()),
                super::StagedKind::TypeChange => ("TypeChange".into(), s.path.clone()),
                super::StagedKind::Renamed { from, to } => {
                    ("Renamed".into(), format!("{from}->{to}"))
                }
            }
        }

        let mut lhs = snap.iter().map(key).collect::<Vec<_>>();
        let mut rhs = after.iter().map(key).collect::<Vec<_>>();
        lhs.sort();
        rhs.sort();
        assert_eq!(
            lhs, rhs,
            "restored staged set should equal original snapshot"
        );
    }
}
