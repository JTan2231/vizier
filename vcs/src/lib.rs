use git2::{DiffOptions, Error, Repository, Signature};

pub fn get_diff(
    repo_path: &str,
    target: Option<&str>, // commit/range or directory path
    exclude: Option<&[&str]>,
) -> Result<String, Error> {
    let repo = Repository::open(repo_path)?;
    let mut opts = DiffOptions::new();

    // Add excludes
    if let Some(excludes) = exclude {
        for ex in excludes {
            opts.pathspec(&format!(":!{}", ex));
        }
    }

    // Handle different target types
    let diff = match target {
        Some(spec) if spec.contains("..") => {
            // Commit range (e.g., "main..feature")
            let parts: Vec<_> = spec.split("..").collect();
            let from = repo.revparse_single(parts[0])?.peel_to_tree()?;
            let to = repo.revparse_single(parts[1])?.peel_to_tree()?;
            repo.diff_tree_to_tree(Some(&from), Some(&to), Some(&mut opts))?
        }
        Some(spec) => {
            // Try as commit first, fall back to directory
            if let Ok(obj) = repo.revparse_single(spec) {
                let tree = obj.peel_to_tree()?;
                repo.diff_tree_to_workdir_with_index(Some(&tree), Some(&mut opts))?
            } else {
                // Treat as directory path
                opts.pathspec(spec);
                let head = repo.head()?.peel_to_tree()?;
                repo.diff_tree_to_workdir_with_index(Some(&head), Some(&mut opts))?
            }
        }
        None => {
            // Default: HEAD vs working dir
            let head = repo.head()?.peel_to_tree()?;
            repo.diff_tree_to_workdir_with_index(Some(&head), Some(&mut opts))?
        }
    };

    let mut buf = Vec::new();
    diff.print(git2::DiffFormat::Patch, |_, _, line| {
        buf.extend_from_slice(line.content());
        true
    })?;

    String::from_utf8(buf).map_err(|_| Error::from_str("Invalid UTF-8 in diff"))
}

pub fn add_and_commit(
    paths: Option<Vec<&str>>,
    message: &str,
    allow_empty: bool,
) -> Result<git2::Oid, git2::Error> {
    let repo = Repository::open(".")?;
    let mut index = repo.index()?;

    match paths {
        Some(paths) => {
            for path in paths {
                index.add_path(std::path::Path::new(path))?;
            }
        }
        None => {
            // equivalent to `git add -u`
            index.update_all(["*"].iter(), None)?;
        }
    }

    index.write()?;
    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;

    let signature = Signature::now("Vizier", "vizier@local")?;
    let parent = match repo.head() {
        Ok(head) => Some(head.peel_to_commit()?),
        Err(_) => None,
    };

    let parent_refs: Vec<&git2::Commit> = parent.iter().collect();

    if !allow_empty && parent_refs.len() == 1 {
        if parent_refs[0].tree_id() == tree_id {
            return Err(git2::Error::from_str("nothing to commit"));
        }
    }

    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parent_refs,
    )
}
