use git2::build::CheckoutBuilder;
use git2::{BranchType, Error, ErrorCode, Repository};
use std::path::Path;

/// Determine the repository's primary branch by preferring origin/HEAD, then main/master, then
/// the most recently updated local branch.
pub fn detect_primary_branch() -> Option<String> {
    let repo = Repository::discover(".").ok()?;
    detect_primary_branch_in_repo(&repo)
}

pub fn detect_primary_branch_in<P: AsRef<Path>>(repo_path: P) -> Option<String> {
    let repo = Repository::open(repo_path).ok()?;
    detect_primary_branch_in_repo(&repo)
}

fn detect_primary_branch_in_repo(repo: &Repository) -> Option<String> {
    if let Ok(ref_remote_head) = repo.find_reference("refs/remotes/origin/HEAD")
        && let Some(symbolic) = ref_remote_head.symbolic_target()
        && let Some(name) = symbolic.strip_prefix("refs/remotes/origin/")
        && repo.find_branch(name, BranchType::Local).is_ok()
    {
        return Some(name.to_string());
    }

    for candidate in ["main", "master"] {
        if repo.find_branch(candidate, BranchType::Local).is_ok() {
            return Some(candidate.to_string());
        }
    }

    let mut newest: Option<(String, i64)> = None;
    if let Ok(branches) = repo.branches(Some(BranchType::Local)) {
        for (branch, _) in branches.flatten() {
            if let Ok(commit) = branch.get().peel_to_commit()
                && let Ok(Some(name)) = branch.name()
            {
                let seconds = commit.time().seconds();
                match newest {
                    Some((_, current)) if current >= seconds => {}
                    _ => {
                        newest = Some((name.to_string(), seconds));
                    }
                }
            }
        }
    }

    newest.map(|(name, _)| name)
}

pub fn branch_exists(name: &str) -> Result<bool, Error> {
    let repo = Repository::discover(".")?;
    match repo.find_branch(name, BranchType::Local) {
        Ok(_) => Ok(true),
        Err(err) if err.code() == ErrorCode::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

pub fn branch_exists_in<P: AsRef<Path>>(repo_path: P, name: &str) -> Result<bool, Error> {
    let repo = Repository::open(repo_path)?;
    match repo.find_branch(name, BranchType::Local) {
        Ok(_) => Ok(true),
        Err(err) if err.code() == ErrorCode::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

pub fn create_branch_from(base: &str, new_branch: &str) -> Result<(), Error> {
    create_branch_from_in(".", base, new_branch)
}

pub fn create_branch_from_in<P: AsRef<Path>>(
    repo_path: P,
    base: &str,
    new_branch: &str,
) -> Result<(), Error> {
    let repo = Repository::open(repo_path)?;
    let base_branch = repo.find_branch(base, BranchType::Local)?;
    let commit = base_branch.into_reference().peel_to_commit()?;
    repo.branch(new_branch, &commit, false)?;
    Ok(())
}

pub fn create_branch_from_head_in<P: AsRef<Path>>(
    repo_path: P,
    new_branch: &str,
) -> Result<(), Error> {
    let repo = Repository::open(repo_path)?;
    let head_commit = repo.head()?.peel_to_commit()?;
    repo.branch(new_branch, &head_commit, false)?;
    Ok(())
}

pub fn delete_branch(name: &str) -> Result<(), Error> {
    delete_branch_in(".", name)
}

pub fn delete_branch_in<P: AsRef<Path>>(repo_path: P, name: &str) -> Result<(), Error> {
    let repo = Repository::open(repo_path)?;
    match repo.find_branch(name, BranchType::Local) {
        Ok(mut branch) => branch.delete(),
        Err(err) if err.code() == ErrorCode::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

pub fn checkout_branch(name: &str) -> Result<(), Error> {
    checkout_branch_in(".", name)
}

pub fn checkout_branch_in<P: AsRef<Path>>(repo_path: P, name: &str) -> Result<(), Error> {
    let repo = Repository::open(repo_path)?;
    let mut checkout = CheckoutBuilder::new();
    checkout.force();
    let target_ref = format!("refs/heads/{name}");
    if let Err(err) = repo.set_head(&target_ref) {
        if !branch_checked_out_in_linked_repo(&err) {
            return Err(err);
        }
        // libgit2 blocks switching symbolic HEAD directly when the target
        // branch is the HEAD of a linked worktree. Detach first, then attach.
        let head_oid = repo
            .head()?
            .target()
            .ok_or_else(|| Error::from_str("cannot detach HEAD without a direct target OID"))?;
        repo.set_head_detached(head_oid)?;
        repo.set_head(&target_ref)?;
    }
    repo.checkout_head(Some(&mut checkout))
}

fn branch_checked_out_in_linked_repo(err: &Error) -> bool {
    err.message()
        .contains("current HEAD of a linked repository")
}

pub fn current_branch_name_in<P: AsRef<Path>>(repo_path: P) -> Result<Option<String>, Error> {
    let repo = Repository::open(repo_path)?;
    let head = repo.head()?;
    if !head.is_branch() {
        return Ok(None);
    }
    Ok(head
        .shorthand()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string()))
}
