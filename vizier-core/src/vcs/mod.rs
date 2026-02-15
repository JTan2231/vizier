mod branches;
mod commits;
mod merge;
mod release;
mod remotes;
mod status;
mod worktrees;

pub use branches::{
    branch_exists, branch_exists_in, checkout_branch, checkout_branch_in, create_branch_from,
    create_branch_from_head_in, create_branch_from_in, current_branch_name_in, delete_branch,
    delete_branch_in, detect_primary_branch, detect_primary_branch_in,
};
pub use commits::{
    StagedItem, StagedKind, add_and_commit, add_and_commit_in, amend_head_commit,
    blob_exists_at_revision, blob_exists_at_revision_in, commit_paths_in_repo, commit_staged,
    commit_staged_in, get_log, read_blob_at_revision, read_blob_at_revision_in, restore_staged,
    revisions_touching_path, revisions_touching_path_in, snapshot_staged, stage, stage_all,
    stage_all_in, stage_in, stage_paths_allow_missing, stage_paths_allow_missing_in, unstage,
    unstage_in,
};
pub use merge::{
    CherryPickApply, CherryPickApplyConflict, CherryPickOutcome, MergeCommitSummary, MergeConflict,
    MergePreparation, MergeReady, SquashPlan, apply_cherry_pick_sequence, build_squash_plan,
    build_squash_plan_in, commit_in_progress_cherry_pick, commit_in_progress_cherry_pick_in,
    commit_in_progress_merge, commit_in_progress_squash, commit_ready_merge, commit_ready_merge_in,
    commit_soft_squash, commit_squashed_merge, commit_squashed_merge_in, list_conflicted_paths,
    list_conflicted_paths_in, prepare_merge, prepare_merge_in,
};
pub use release::{
    ReleaseBump, ReleaseCommit, ReleaseNoteEntry, ReleaseNotes, ReleaseSectionKind, ReleaseTag,
    ReleaseVersion, build_release_notes, classify_commit, commits_since_release_tag,
    create_annotated_release_tag, derive_release_bump, is_conventional_commit_subject,
    latest_reachable_release_tag, parse_release_version_tag, release_tag_exists,
};
pub use remotes::{
    AttemptOutcome, CredentialAttempt, CredentialStrategy, HelperScope, PushError, PushErrorKind,
    RemoteScheme, SshKeyKind, origin_owner_repo, push_current_branch, push_current_branch_in,
};
pub use status::{
    DiffSummary, apply_patch_file_with_index_in, apply_patch_with_index_in,
    diff_binary_against_head_in, diff_summary_against_target, ensure_clean_worktree, get_diff,
    repo_root, status_with_branch,
};
pub use worktrees::{
    add_worktree_for_branch, add_worktree_for_branch_in, find_worktree_name_by_path_in,
    remove_worktree, remove_worktree_in,
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

#[cfg(test)]
mod tests;
