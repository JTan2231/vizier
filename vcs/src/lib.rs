use git2::{DiffFormat, DiffOptions, Error, Oid, Repository, Signature, Sort, Tree};

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
    while s.contains("//") && !s.starts_with("//") {
        s = s.replace("//", "/");
    }
    s
}

fn empty_tree(repo: &Repository) -> Result<Tree, Error> {
    let mut idx = repo.index()?;
    idx.clear()?;
    let oid = idx.write_tree_to(repo)?;
    repo.find_tree(oid)
}

/// Return a unified diff (`git diff`-style patch) for the repository at `repo_path`,
/// formatted newest → oldest changes where applicable.
///
/// Assumptions:
/// - If `target` is `None`, compare HEAD (or empty tree if unborn) to working dir + index.
/// - If `target` is a single rev, compare that commit tree to working dir + index.
/// - If `target` is `<from>..<to>`, compare commit `<from>` to `<to>`.
/// - If `target` is `<from>...<to>`, diff the merge-base of `<from>` and `<to>` against `<to>`.
/// - If `target` does not resolve to a rev, treat it as a path and restrict the diff there.
/// - If `exclude` is given, exclude those pathspecs (normalized) from the diff.
pub fn get_diff(
    repo_path: &str,
    target: Option<&str>, // commit/range or directory path
    exclude: Option<&[&str]>,
) -> Result<String, Error> {
    let repo = Repository::open(repo_path)?;
    let mut opts = DiffOptions::new();

    if let Some(excludes) = exclude {
        for ex in excludes {
            let normalized = normalize_pathspec(ex);
            opts.pathspec(&format!(":(exclude){}", normalized));
        }
    }

    opts.ignore_submodules(true)
        .recurse_untracked_dirs(true)
        .id_abbrev(40);

    let diff = match target {
        Some(spec) if spec.contains("...") => {
            let parts: Vec<_> = spec.split("...").collect();
            if parts.len() != 2 {
                return Err(Error::from_str("Invalid triple-dot range"));
            }

            let a = repo.revparse_single(parts[0])?;
            let b = repo.revparse_single(parts[1])?;
            let base = repo.merge_base(a.id(), b.id())?;
            let from = repo.find_commit(base)?.tree()?;
            let to = b.peel_to_tree()?;

            repo.diff_tree_to_tree(Some(&from), Some(&to), Some(&mut opts))?
        }
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
                    let head_tree = match repo.head().ok().and_then(|h| h.peel_to_tree().ok()) {
                        Some(t) => t,
                        None => empty_tree(&repo)?,
                    };

                    repo.diff_tree_to_workdir_with_index(Some(&head_tree), Some(&mut opts))?
                }
            }
        }
        None => {
            // HEAD vs working dir (with index); handle unborn HEAD
            let head_tree = match repo.head().ok().and_then(|h| h.peel_to_tree().ok()) {
                Some(t) => t,
                None => empty_tree(&repo)?,
            };

            repo.diff_tree_to_workdir_with_index(Some(&head_tree), Some(&mut opts))?
        }
    };

    let mut buf = Vec::new();
    diff.print(DiffFormat::Patch, |_, _, line| {
        buf.extend_from_slice(line.content());
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
