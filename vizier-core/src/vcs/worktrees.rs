use git2::{Error, Repository, WorktreeAddOptions, WorktreePruneOptions};
use std::path::Path;

pub fn add_worktree_for_branch(
    worktree_name: &str,
    path: &Path,
    branch_name: &str,
) -> Result<(), Error> {
    let repo = Repository::discover(".")?;
    let mut opts = WorktreeAddOptions::new();
    let reference = repo.find_reference(&format!("refs/heads/{branch_name}"))?;
    opts.reference(Some(&reference));
    repo.worktree(worktree_name, path, Some(&opts))?;
    Ok(())
}

pub fn remove_worktree(worktree_name: &str, remove_dir: bool) -> Result<(), Error> {
    let repo = Repository::discover(".")?;
    let worktree = repo.find_worktree(worktree_name)?;
    let mut opts = WorktreePruneOptions::new();
    opts.valid(true).locked(true).working_tree(remove_dir);
    worktree.prune(Some(&mut opts))
}
