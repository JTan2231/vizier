use git2::{BranchType, Error, Repository, WorktreeAddOptions, WorktreePruneOptions};
use std::fs;
use std::path::Path;

pub fn add_worktree_for_branch(
    worktree_name: &str,
    path: &Path,
    branch_name: &str,
) -> Result<(), Error> {
    add_worktree_for_branch_in(".", worktree_name, path, branch_name)
}

pub fn add_worktree_for_branch_in<P: AsRef<Path>, Q: AsRef<Path>>(
    repo_path: P,
    worktree_name: &str,
    path: Q,
    branch_name: &str,
) -> Result<(), Error> {
    let repo = Repository::open(repo_path)?;
    let path = path.as_ref();
    let target_refname = format!("refs/heads/{branch_name}");
    match add_worktree_for_reference(&repo, worktree_name, path, &target_refname) {
        Ok(()) => Ok(()),
        Err(err) if branch_already_checked_out(&err) => {
            add_worktree_for_checked_out_branch(&repo, worktree_name, path, branch_name)
        }
        Err(err) => Err(err),
    }
}

fn add_worktree_for_reference(
    repo: &Repository,
    worktree_name: &str,
    path: &Path,
    reference_name: &str,
) -> Result<(), Error> {
    let mut opts = WorktreeAddOptions::new();
    opts.checkout_existing(true);
    let reference = repo.find_reference(reference_name)?;
    opts.reference(Some(&reference));
    repo.worktree(worktree_name, path, Some(&opts))?;
    Ok(())
}

fn add_worktree_for_checked_out_branch(
    repo: &Repository,
    worktree_name: &str,
    path: &Path,
    branch_name: &str,
) -> Result<(), Error> {
    let target_refname = format!("refs/heads/{branch_name}");
    let target_reference = repo.find_reference(&target_refname)?;
    let target_oid = target_reference.target().ok_or_else(|| {
        Error::from_str(&format!(
            "branch `{branch_name}` does not resolve to a direct commit target"
        ))
    })?;
    let target_commit = repo.find_commit(target_oid)?;

    let temp_branch_name = allocate_temp_branch_name(repo, worktree_name)?;
    repo.branch(&temp_branch_name, &target_commit, false)?;
    let temp_refname = format!("refs/heads/{temp_branch_name}");

    let add_result = add_worktree_for_reference(repo, worktree_name, path, &temp_refname);
    let mut cleanup_error: Option<Error> = None;

    if add_result.is_ok()
        && let Err(err) = repoint_worktree_head(path, &target_refname)
    {
        cleanup_error = Some(err);
    }

    if let Ok(mut temp_branch) = repo.find_branch(&temp_branch_name, BranchType::Local)
        && let Err(err) = temp_branch.delete()
        && cleanup_error.is_none()
    {
        cleanup_error = Some(err);
    }

    if let Some(err) = cleanup_error {
        return Err(err);
    }

    add_result
}

fn allocate_temp_branch_name(repo: &Repository, worktree_name: &str) -> Result<String, Error> {
    let sanitized = sanitize_branch_component(worktree_name);
    for attempt in 0..1000 {
        let candidate = format!("__vizier/worktree/{sanitized}/{attempt}");
        if repo
            .find_reference(&format!("refs/heads/{candidate}"))
            .is_err()
        {
            return Ok(candidate);
        }
    }
    Err(Error::from_str(
        "could not allocate unique temporary branch for worktree add",
    ))
}

fn sanitize_branch_component(value: &str) -> String {
    let mut sanitized = value
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => ch,
            _ => '-',
        })
        .collect::<String>();
    if sanitized.trim_matches('-').is_empty() {
        sanitized = "worktree".to_string();
    }
    sanitized.trim_matches('-').to_string()
}

fn repoint_worktree_head(worktree_path: &Path, target_refname: &str) -> Result<(), Error> {
    let worktree_repo = Repository::open(worktree_path)?;
    let head_path = worktree_repo.path().join("HEAD");
    fs::write(&head_path, format!("ref: {target_refname}\n"))
        .map_err(|err| Error::from_str(&format!("failed to update worktree HEAD: {err}")))
}

fn branch_already_checked_out(err: &Error) -> bool {
    err.message().contains("already checked out")
}

pub fn remove_worktree(worktree_name: &str, remove_dir: bool) -> Result<(), Error> {
    remove_worktree_in(".", worktree_name, remove_dir)
}

pub fn remove_worktree_in<P: AsRef<Path>>(
    repo_path: P,
    worktree_name: &str,
    remove_dir: bool,
) -> Result<(), Error> {
    let repo = Repository::open(repo_path)?;
    let worktree = repo.find_worktree(worktree_name)?;
    let mut opts = WorktreePruneOptions::new();
    opts.valid(true).locked(true).working_tree(remove_dir);
    worktree.prune(Some(&mut opts))
}

pub fn find_worktree_name_by_path_in<P: AsRef<Path>, Q: AsRef<Path>>(
    repo_path: P,
    worktree_path: Q,
) -> Result<Option<String>, Error> {
    let repo = Repository::open(repo_path)?;
    let target = match worktree_path.as_ref().canonicalize() {
        Ok(path) => path,
        Err(_) => return Ok(None),
    };
    let worktrees = repo.worktrees()?;
    for name in worktrees.iter().flatten() {
        if let Ok(worktree) = repo.find_worktree(name)
            && worktree.path().canonicalize().ok() == Some(target.clone())
        {
            return Ok(Some(name.to_string()));
        }
    }
    Ok(None)
}
