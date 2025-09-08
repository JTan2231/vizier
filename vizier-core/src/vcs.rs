use git2::{DiffFormat, DiffOptions, Error, Oid, Repository, Signature, Sort};

// TODO: Please god write a testing harness for this, or at least the flows using this

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
pub fn get_log(depth: usize, filters: Option<Vec<String>>) -> Result<String, Error> {
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

    let mut out = String::new();
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

        out.push_str(&format!("commit {short_sha} — {author}\n"));
        out.push_str(&msg);
        if !msg.ends_with('\n') {
            out.push('\n');
        }

        out.push('\n');

        kept += 1;
        if kept >= depth {
            break;
        }
    }

    Ok(out)
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

    // --- get_log: deterministic ordering ------------------------------------

    #[test]
    fn log_depth_and_filters() {
        use std::{thread, time::Duration};
        let (td, repo) = init_temp_repo();
        let _cwd = CwdGuard::enter(td.path()).unwrap();

        write(Path::new("f.txt"), "a\n");
        raw_commit(&repo, "feat: add a");
        thread::sleep(Duration::from_secs(1));

        append(Path::new("f.txt"), "b\n");
        raw_commit(&repo, "chore: touch b");
        thread::sleep(Duration::from_secs(1));

        append(Path::new("f.txt"), "c\n");
        raw_commit(&repo, "FEAT: big C change\n\nDetails...");

        let out1 = get_log(1, Some(vec!["feat".into()])).expect("log");
        assert!(out1.to_lowercase().contains("feat: big c change"));

        let out2 = get_log(2, Some(vec!["feat".into(), "chore".into()])).expect("log2");
        let l2 = out2.to_lowercase();
        assert!(l2.contains("feat: big c change"));
        assert!(l2.contains("chore: touch b"));
        assert!(out2.contains("commit "));
        assert!(out2.contains(" — "));
    }
}
