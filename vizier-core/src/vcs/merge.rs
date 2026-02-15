use git2::build::CheckoutBuilder;
use git2::{
    BranchType, CherrypickOptions, Error, ErrorCode, FileFavor, Index,
    MergeOptions as GitMergeOptions, Oid, Repository, RepositoryState, ResetType, Sort,
};
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct MergeReady {
    pub head_oid: Oid,
    pub source_oid: Oid,
    pub tree_oid: Oid,
}

#[derive(Debug, Clone)]
pub struct MergeConflict {
    pub head_oid: Oid,
    pub source_oid: Oid,
    pub files: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum MergePreparation {
    Ready(MergeReady),
    Conflicted(MergeConflict),
}

#[derive(Debug, Clone)]
pub struct MergeCommitSummary {
    pub oid: Oid,
    pub parents: Vec<Oid>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SquashPlan {
    pub target_head: Oid,
    pub source_tip: Oid,
    pub merge_base: Oid,
    pub commits_to_apply: Vec<Oid>,
    pub merge_commits: Vec<MergeCommitSummary>,
    pub inferred_mainline: Option<u32>,
    pub mainline_ambiguous: bool,
}

#[derive(Debug, Clone)]
pub struct CherryPickApply {
    pub applied: Vec<Oid>,
}

#[derive(Debug, Clone)]
pub struct CherryPickApplyConflict {
    pub applied: Vec<Oid>,
    pub remaining: Vec<Oid>,
    pub files: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum CherryPickOutcome {
    Completed(CherryPickApply),
    Conflicted(CherryPickApplyConflict),
}

pub fn prepare_merge(source_branch: &str) -> Result<MergePreparation, Error> {
    prepare_merge_in(".", source_branch)
}

pub fn prepare_merge_in<P: AsRef<Path>>(
    repo_path: P,
    source_branch: &str,
) -> Result<MergePreparation, Error> {
    let repo = Repository::open(repo_path)?;
    prepare_merge_impl(&repo, source_branch)
}

fn prepare_merge_impl(repo: &Repository, source_branch: &str) -> Result<MergePreparation, Error> {
    if repo.state() != RepositoryState::Clean {
        return Err(Error::from_str(
            "cannot start a merge while another git operation is in progress",
        ));
    }

    let head_ref = repo.head()?;
    if !head_ref.is_branch() {
        return Err(Error::from_str(
            "cannot merge into detached HEAD; checkout a branch first",
        ));
    }

    let head_commit = head_ref.peel_to_commit()?;
    let source_ref = repo.find_branch(source_branch, BranchType::Local)?;
    let source_commit = source_ref.get().peel_to_commit()?;

    let mut index = repo.merge_commits(&head_commit, &source_commit, None)?;
    if index.has_conflicts() {
        let conflicts = collect_conflict_paths(&mut index);
        materialize_conflicts(repo, source_branch)?;
        return Ok(MergePreparation::Conflicted(MergeConflict {
            head_oid: head_commit.id(),
            source_oid: source_commit.id(),
            files: conflicts,
        }));
    }

    let tree_oid = index.write_tree_to(repo)?;
    Ok(MergePreparation::Ready(MergeReady {
        head_oid: head_commit.id(),
        source_oid: source_commit.id(),
        tree_oid,
    }))
}

pub fn build_squash_plan(source_branch: &str) -> Result<SquashPlan, Error> {
    build_squash_plan_in(".", source_branch)
}

pub fn build_squash_plan_in<P: AsRef<Path>>(
    repo_path: P,
    source_branch: &str,
) -> Result<SquashPlan, Error> {
    let repo = Repository::open(repo_path)?;
    build_squash_plan_impl(&repo, source_branch)
}

fn build_squash_plan_impl(repo: &Repository, source_branch: &str) -> Result<SquashPlan, Error> {
    let head_ref = repo.head()?;
    if !head_ref.is_branch() {
        return Err(Error::from_str(
            "cannot merge into detached HEAD; checkout a branch first",
        ));
    }

    let head_commit = head_ref.peel_to_commit()?;
    let source_ref = repo.find_branch(source_branch, BranchType::Local)?;
    let source_commit = source_ref.get().peel_to_commit()?;
    let merge_base = repo.merge_base(head_commit.id(), source_commit.id())?;
    let commits_to_apply = collect_commits_from_base(repo, merge_base, source_commit.id())?;
    let mut merge_commits = Vec::new();
    let mut possible_mainlines: Option<HashSet<u32>> = None;
    let mut ambiguous = false;

    for oid in &commits_to_apply {
        let commit = repo.find_commit(*oid)?;
        let parent_count = commit.parent_count();
        if parent_count > 1 {
            let mut parents = Vec::with_capacity(parent_count);
            for idx in 0..parent_count {
                parents.push(commit.parent_id(idx)?);
            }
            merge_commits.push(MergeCommitSummary {
                oid: *oid,
                parents: parents.clone(),
                summary: commit.summary().map(|s| s.to_string()),
            });

            if parent_count > 2 {
                ambiguous = true;
                continue;
            }

            let mut candidates = HashSet::new();
            for (idx, parent) in parents.iter().enumerate() {
                if repo.graph_descendant_of(head_commit.id(), *parent)? {
                    candidates.insert((idx + 1) as u32);
                }
            }

            if candidates.is_empty() {
                ambiguous = true;
                continue;
            }

            possible_mainlines = Some(match possible_mainlines {
                None => candidates,
                Some(existing) => existing.intersection(&candidates).copied().collect(),
            });
            if matches!(possible_mainlines, Some(ref set) if set.is_empty()) {
                ambiguous = true;
            }
        }
    }

    let inferred_mainline = if !ambiguous {
        possible_mainlines.as_ref().and_then(|set| {
            if set.len() == 1 {
                set.iter().copied().next()
            } else {
                None
            }
        })
    } else {
        None
    };

    let mainline_ambiguous = ambiguous
        || matches!(possible_mainlines, Some(ref set) if !set.is_empty() && set.len() != 1);

    Ok(SquashPlan {
        target_head: head_commit.id(),
        source_tip: source_commit.id(),
        merge_base,
        commits_to_apply,
        merge_commits,
        inferred_mainline,
        mainline_ambiguous,
    })
}

pub fn apply_cherry_pick_sequence(
    start_head: Oid,
    commits: &[Oid],
    file_favor: Option<FileFavor>,
    mainline: Option<u32>,
) -> Result<CherryPickOutcome, Error> {
    let repo = Repository::discover(".")?;
    let current_head = repo.head()?.peel_to_commit()?.id();
    if current_head != start_head {
        return Err(Error::from_str(
            "HEAD moved since the squash plan was prepared; aborting",
        ));
    }

    let mut applied = Vec::new();
    for (idx, oid) in commits.iter().enumerate() {
        let commit = repo.find_commit(*oid)?;
        let mut opts = CherrypickOptions::new();
        let parent_count = commit.parent_count();
        if parent_count > 1 {
            let Some(mainline_parent) = mainline else {
                return Err(Error::from_str(
                    "plan branch includes merge commits; rerun vizier merge with --squash-mainline <parent> or --no-squash",
                ));
            };
            if mainline_parent == 0 || mainline_parent as usize > parent_count {
                return Err(Error::from_str(&format!(
                    "squash mainline parent {} is out of range for merge commit {}",
                    mainline_parent,
                    commit.id()
                )));
            }
            opts.mainline(mainline_parent);
        }
        let mut checkout = CheckoutBuilder::new();
        checkout
            .allow_conflicts(true)
            .conflict_style_merge(true)
            .force();
        opts.checkout_builder(checkout);
        let mut merge_opts = GitMergeOptions::new();
        merge_opts.fail_on_conflict(false);
        if let Some(favor) = file_favor {
            merge_opts.file_favor(favor);
        }
        opts.merge_opts(merge_opts);
        let result = repo.cherrypick(&commit, Some(&mut opts));
        if let Err(err) = result
            && err.code() != ErrorCode::MergeConflict
        {
            return Err(err);
        }

        let mut index = repo.index()?;
        if index.has_conflicts() {
            let files = collect_conflict_paths(&mut index);
            return Ok(CherryPickOutcome::Conflicted(CherryPickApplyConflict {
                applied: applied.clone(),
                remaining: commits[idx..].to_vec(),
                files,
            }));
        }

        index.write()?;
        let tree_oid = index.write_tree()?;
        let tree = repo.find_tree(tree_oid)?;
        let sig = repo.signature()?;
        let parent_commit = repo.head()?.peel_to_commit()?;
        let message = commit.summary().unwrap_or("Apply plan commit");
        let new_oid = repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent_commit])?;

        repo.cleanup_state().ok();
        let mut checkout = CheckoutBuilder::new();
        checkout.force();
        repo.checkout_head(Some(&mut checkout))?;

        applied.push(new_oid);
    }

    Ok(CherryPickOutcome::Completed(CherryPickApply { applied }))
}

pub fn commit_soft_squash(message: &str, base_oid: Oid, expected_head: Oid) -> Result<Oid, Error> {
    let repo = Repository::discover(".")?;
    if repo.head()?.peel_to_commit()?.id() != expected_head {
        return Err(Error::from_str(
            "HEAD moved after applying the plan; aborting squash",
        ));
    }

    let base_obj = repo.find_object(base_oid, None)?;
    repo.reset(&base_obj, ResetType::Soft, None)?;

    let mut index = repo.index()?;
    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;
    let sig = repo.signature()?;
    let parent = repo.find_commit(base_oid)?;

    let oid = repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;

    let mut checkout = CheckoutBuilder::new();
    checkout.force();
    repo.checkout_head(Some(&mut checkout))?;

    Ok(oid)
}

fn commit_in_progress_cherry_pick_repo(
    repo: &Repository,
    message: &str,
    expected_parent: Oid,
) -> Result<Oid, Error> {
    if repo.state() != RepositoryState::CherryPick {
        return Err(Error::from_str("no cherry-pick in progress to finalize"));
    }

    let parent_commit = repo.head()?.peel_to_commit()?;
    if parent_commit.id() != expected_parent {
        return Err(Error::from_str(
            "HEAD no longer points to the expected cherry-pick parent",
        ));
    }

    let mut index = repo.index()?;
    if index.has_conflicts() {
        return Err(Error::from_str(
            "cannot finalize cherry-pick until all conflicts are resolved",
        ));
    }

    index.write()?;
    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;
    let sig = repo.signature()?;

    let oid = repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent_commit])?;

    repo.cleanup_state()?;
    let mut checkout = CheckoutBuilder::new();
    checkout.force();
    repo.checkout_head(Some(&mut checkout))?;

    Ok(oid)
}

pub fn commit_in_progress_cherry_pick(message: &str, expected_parent: Oid) -> Result<Oid, Error> {
    let repo = Repository::discover(".")?;
    commit_in_progress_cherry_pick_repo(&repo, message, expected_parent)
}

pub fn commit_in_progress_cherry_pick_in<P: AsRef<Path>>(
    repo_path: P,
    message: &str,
    expected_parent: Oid,
) -> Result<Oid, Error> {
    let repo = Repository::open(repo_path)?;
    commit_in_progress_cherry_pick_repo(&repo, message, expected_parent)
}

pub fn commit_ready_merge(message: &str, ready: MergeReady) -> Result<Oid, Error> {
    commit_ready_merge_in(".", message, ready)
}

pub fn commit_ready_merge_in<P: AsRef<Path>>(
    repo_path: P,
    message: &str,
    ready: MergeReady,
) -> Result<Oid, Error> {
    let repo = Repository::open(repo_path)?;
    let mut checkout = CheckoutBuilder::new();
    let head_commit = repo.find_commit(ready.head_oid)?;
    let source_commit = repo.find_commit(ready.source_oid)?;
    let tree = repo.find_tree(ready.tree_oid)?;
    let sig = repo.signature()?;

    let oid = repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        message,
        &tree,
        &[&head_commit, &source_commit],
    )?;

    checkout.force();
    repo.checkout_head(Some(&mut checkout))?;

    Ok(oid)
}

pub fn commit_squashed_merge(message: &str, ready: MergeReady) -> Result<Oid, Error> {
    commit_squashed_merge_in(".", message, ready)
}

pub fn commit_squashed_merge_in<P: AsRef<Path>>(
    repo_path: P,
    message: &str,
    ready: MergeReady,
) -> Result<Oid, Error> {
    let repo = Repository::open(repo_path)?;
    let mut checkout = CheckoutBuilder::new();
    let head_commit = repo.find_commit(ready.head_oid)?;
    let tree = repo.find_tree(ready.tree_oid)?;
    let sig = repo.signature()?;

    let oid = repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&head_commit])?;

    checkout.force();
    repo.checkout_head(Some(&mut checkout))?;

    Ok(oid)
}

pub fn commit_in_progress_merge(
    message: &str,
    head_oid: Oid,
    source_oid: Oid,
) -> Result<Oid, Error> {
    let repo = Repository::discover(".")?;
    if repo.state() != RepositoryState::Merge {
        return Err(Error::from_str("no merge in progress to finalize"));
    }

    let mut index = repo.index()?;
    if index.has_conflicts() {
        return Err(Error::from_str(
            "cannot finalize merge until all conflicts are resolved",
        ));
    }

    index.write()?;
    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;
    let head_commit = repo.find_commit(head_oid)?;
    let source_commit = repo.find_commit(source_oid)?;
    let sig = repo.signature()?;

    let oid = repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        message,
        &tree,
        &[&head_commit, &source_commit],
    )?;

    repo.cleanup_state()?;
    let mut checkout = CheckoutBuilder::new();
    checkout.force();
    repo.checkout_head(Some(&mut checkout))?;

    Ok(oid)
}

pub fn commit_in_progress_squash(message: &str, head_oid: Oid) -> Result<Oid, Error> {
    let repo = Repository::discover(".")?;
    if repo.state() != RepositoryState::Merge {
        return Err(Error::from_str("no merge in progress to finalize"));
    }

    let mut index = repo.index()?;
    if index.has_conflicts() {
        return Err(Error::from_str(
            "cannot finalize merge until all conflicts are resolved",
        ));
    }

    index.write()?;
    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;
    let head_commit = repo.find_commit(head_oid)?;
    let sig = repo.signature()?;

    let oid = repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&head_commit])?;

    repo.cleanup_state()?;
    let mut checkout = CheckoutBuilder::new();
    checkout.force();
    repo.checkout_head(Some(&mut checkout))?;

    Ok(oid)
}

fn collect_commits_from_base(
    repo: &Repository,
    merge_base: Oid,
    source_tip: Oid,
) -> Result<Vec<Oid>, Error> {
    let mut walk = repo.revwalk()?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::REVERSE)?;
    walk.push(source_tip)?;
    walk.hide(merge_base)?;

    let mut commits = Vec::new();
    for oid in walk {
        commits.push(oid?);
    }

    Ok(commits)
}

pub fn list_conflicted_paths() -> Result<Vec<String>, Error> {
    list_conflicted_paths_in(".")
}

pub fn list_conflicted_paths_in<P: AsRef<Path>>(repo_path: P) -> Result<Vec<String>, Error> {
    let repo = Repository::open(repo_path)?;
    let mut index = repo.index()?;
    if !index.has_conflicts() {
        return Ok(Vec::new());
    }
    Ok(collect_conflict_paths(&mut index))
}

fn collect_conflict_paths(index: &mut Index) -> Vec<String> {
    let mut files = Vec::new();
    if let Ok(mut conflicts) = index.conflicts() {
        for conflict in conflicts.by_ref().flatten() {
            let path_bytes = conflict
                .our
                .as_ref()
                .or(conflict.their.as_ref())
                .or(conflict.ancestor.as_ref())
                .map(|entry| entry.path.clone());
            if let Some(bytes) = path_bytes {
                let path = String::from_utf8_lossy(&bytes).to_string();
                files.push(path);
            }
        }
    }

    files.sort();
    files.dedup();
    files
}

fn materialize_conflicts(repo: &Repository, source_branch: &str) -> Result<(), Error> {
    let branch = repo.find_branch(source_branch, BranchType::Local)?;
    let reference = branch.into_reference();
    let mut checkout = CheckoutBuilder::new();
    checkout
        .allow_conflicts(true)
        .conflict_style_merge(true)
        .force();

    let annotated = repo.reference_to_annotated_commit(&reference)?;
    let mut merge_opts = GitMergeOptions::new();
    merge_opts.fail_on_conflict(false);

    repo.merge(&[&annotated], Some(&mut merge_opts), Some(&mut checkout))
}
