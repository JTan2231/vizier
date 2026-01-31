use crate::fixtures::*;

#[test]
fn test_cd_creates_and_reuses_workspace() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.git(&["checkout", "-b", "draft/workspace-check"])?;
    repo.git(&["checkout", "master"])?;

    let first = repo.vizier_output(&["cd", "workspace-check"])?;
    assert!(
        first.status.success(),
        "vizier cd failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let stdout_first = String::from_utf8_lossy(&first.stdout);
    let path_first = stdout_first.lines().next().unwrap_or("").trim().to_string();
    assert!(
        !path_first.is_empty(),
        "cd should print the workspace path on the first line:\n{stdout_first}"
    );
    assert!(
        Path::new(&path_first).exists(),
        "workspace path should exist after vizier cd: {}",
        path_first
    );
    let repo_handle = repo.repo();
    let worktrees = list_worktree_names(&repo_handle)?;
    assert!(
        worktrees
            .iter()
            .any(|name| name == "vizier-workspace-workspace-check"),
        "worktree list should include the workspace name after cd: {worktrees:?}"
    );

    let second = repo.vizier_output(&["cd", "workspace-check"])?;
    assert!(
        second.status.success(),
        "vizier cd (reuse) failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let stdout_second = String::from_utf8_lossy(&second.stdout);
    let path_second = stdout_second
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    assert_eq!(
        path_first, path_second,
        "second cd should reuse the same workspace path"
    );

    Ok(())
}
#[test]
fn test_cd_fails_when_branch_missing() -> TestResult {
    let repo = IntegrationRepo::new()?;

    let output = repo.vizier_output(&["cd", "missing-branch"])?;
    assert!(
        !output.status.success(),
        "vizier cd should fail when the branch is missing"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("branch draft/missing-branch does not exist"),
        "stderr should explain missing branch: {stderr}"
    );
    assert!(
        stderr.contains("vizier draft missing-branch"),
        "stderr should hint at drafting the plan before cd, got: {stderr}"
    );

    Ok(())
}
#[test]
fn test_clean_prunes_requested_workspaces() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.git(&["checkout", "-b", "draft/workspace-alpha"])?;
    repo.git(&["checkout", "master"])?;
    repo.git(&["checkout", "-b", "draft/workspace-beta"])?;
    repo.git(&["checkout", "master"])?;

    let alpha = repo.vizier_output(&["cd", "workspace-alpha"])?;
    assert!(
        alpha.status.success(),
        "vizier cd alpha failed: {}",
        String::from_utf8_lossy(&alpha.stderr)
    );
    let alpha_path = PathBuf::from(
        String::from_utf8_lossy(&alpha.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim(),
    );
    let beta = repo.vizier_output(&["cd", "workspace-beta"])?;
    assert!(
        beta.status.success(),
        "vizier cd beta failed: {}",
        String::from_utf8_lossy(&beta.stderr)
    );
    let beta_path = PathBuf::from(
        String::from_utf8_lossy(&beta.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim(),
    );

    let clean_one = repo.vizier_output(&["clean", "workspace-alpha", "--yes"])?;
    assert!(
        clean_one.status.success(),
        "vizier clean alpha failed: {}",
        String::from_utf8_lossy(&clean_one.stderr)
    );
    assert!(
        !alpha_path.exists(),
        "targeted clean should remove the requested workspace directory"
    );
    let after_one = list_worktree_names(&repo.repo())?;
    assert!(
        !after_one
            .iter()
            .any(|name| name == "vizier-workspace-workspace-alpha"),
        "targeted clean should drop the alpha worktree registration"
    );
    assert!(
        after_one
            .iter()
            .any(|name| name == "vizier-workspace-workspace-beta"),
        "targeted clean should leave other workspaces in place"
    );
    assert!(
        beta_path.exists(),
        "targeted clean should not remove unrelated workspace paths"
    );

    let clean_all = repo.vizier_output(&["clean", "--yes"])?;
    assert!(
        clean_all.status.success(),
        "vizier clean --yes failed: {}",
        String::from_utf8_lossy(&clean_all.stderr)
    );
    let after_all = list_worktree_names(&repo.repo())?;
    assert!(
        !after_all
            .iter()
            .any(|name| name.starts_with("vizier-workspace-")),
        "global clean should remove all vizier-managed workspaces"
    );
    assert!(
        !beta_path.exists(),
        "global clean should remove remaining workspace directories"
    );

    Ok(())
}
