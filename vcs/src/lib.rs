use git2::{DiffFormat, DiffOptions, Error, Oid, Repository, Signature, Tree};

// TODO: Please god write a testing harness for this, or at least the flows using this

fn normalize_pathspec(path: &str) -> String {
    let mut s = path
        .trim()
        .trim_end_matches('/')
        .trim_end_matches('\\')
        .to_string();
    // Convert backslashes to forward slashes for git pathspecs
    s = s.replace('\\', "/");
    // Remove leading ./ for consistency
    if let Some(stripped) = s.strip_prefix("./") {
        s = stripped.to_string();
    }
    // Collapse duplicate slashes (except possible leading // which could be UNC; keep it)
    while s.contains("//") && !s.starts_with("//") {
        s = s.replace("//", "/");
    }
    s
}

fn empty_tree(repo: &Repository) -> Result<Tree, Error> {
    // libgit2 convention: empty tree is the tree for the empty index
    let mut idx = repo.index()?;
    idx.clear()?;
    let oid = idx.write_tree_to(repo)?;
    repo.find_tree(oid)
}

pub fn get_diff(
    repo_path: &str,
    target: Option<&str>, // commit/range or directory path
    exclude: Option<&[&str]>,
) -> Result<String, Error> {
    let repo = Repository::open(repo_path)?;
    let mut opts = DiffOptions::new();

    // Turn excludes into proper pathspecs
    if let Some(excludes) = exclude {
        for ex in excludes {
            let normalized = normalize_pathspec(ex);
            // Use official pathspec magic
            opts.pathspec(&format!(":(exclude){}", normalized));
        }
    }

    // Helpful defaults
    opts.ignore_submodules(true)
        .recurse_untracked_dirs(true)
        .id_abbrev(40); // stable object ids in headers

    let diff = match target {
        Some(spec) if spec.contains("...") => {
            // Triple-dot: B vs merge-base(A,B)
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
            // Double-dot: B vs A
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

    // Prefer lossy to avoid failing on binary data; document that it’s lossy.
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

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
