use git2::build::CheckoutBuilder;
use git2::{BranchType, Error, ErrorCode, Repository};

/// Determine the repository's primary branch by preferring origin/HEAD, then main/master, then
/// the most recently updated local branch.
pub fn detect_primary_branch() -> Option<String> {
    let repo = Repository::discover(".").ok()?;

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

pub fn create_branch_from(base: &str, new_branch: &str) -> Result<(), Error> {
    let repo = Repository::discover(".")?;
    let base_branch = repo.find_branch(base, BranchType::Local)?;
    let commit = base_branch.into_reference().peel_to_commit()?;
    repo.branch(new_branch, &commit, false)?;
    Ok(())
}

pub fn delete_branch(name: &str) -> Result<(), Error> {
    let repo = Repository::discover(".")?;
    match repo.find_branch(name, BranchType::Local) {
        Ok(mut branch) => branch.delete(),
        Err(err) if err.code() == ErrorCode::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

pub fn checkout_branch(name: &str) -> Result<(), Error> {
    let repo = Repository::discover(".")?;
    let mut checkout = CheckoutBuilder::new();
    checkout.force();
    repo.set_head(&format!("refs/heads/{name}"))?;
    repo.checkout_head(Some(&mut checkout))
}
