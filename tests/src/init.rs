use crate::fixtures::*;

const REQUIRED_IGNORE_RULES: [&str; 4] = [
    ".vizier/tmp/",
    ".vizier/tmp-worktrees/",
    ".vizier/jobs/",
    ".vizier/sessions/",
];

#[test]
fn test_init_creates_durable_markers_and_required_ignore_rules() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let narrative_dir = repo.path().join(".vizier/narrative");
    if narrative_dir.exists() {
        fs::remove_dir_all(&narrative_dir)?;
    }
    repo.write(".gitignore", "/target\nCargo.lock\n")?;

    let output = repo.vizier_output_no_follow(&["init"])?;
    assert!(
        output.status.success(),
        "vizier init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("newly initialized"),
        "expected newly initialized outcome, got: {stdout}"
    );

    assert!(
        repo.path().join(".vizier/narrative/snapshot.md").is_file(),
        "init should create snapshot marker"
    );
    assert!(
        repo.path().join(".vizier/narrative/glossary.md").is_file(),
        "init should create glossary marker"
    );

    let gitignore = repo.read(".gitignore")?;
    assert!(
        gitignore.starts_with("/target\nCargo.lock\n"),
        "existing .gitignore content should remain at top: {gitignore}"
    );
    for rule in REQUIRED_IGNORE_RULES {
        assert!(
            gitignore.contains(rule),
            "expected required ignore rule {rule} in .gitignore:\n{gitignore}"
        );
        assert_eq!(
            gitignore.matches(rule).count(),
            1,
            "required ignore rule {rule} should appear once:\n{gitignore}"
        );
    }
    Ok(())
}

#[test]
fn test_init_partial_repo_only_adds_missing_pieces() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let snapshot_before = repo.read(".vizier/narrative/snapshot.md")?;
    fs::remove_file(repo.path().join(".vizier/narrative/glossary.md"))?;

    let mut gitignore = repo.read(".gitignore")?;
    gitignore = gitignore
        .lines()
        .filter(|line| line.trim() != ".vizier/sessions/")
        .collect::<Vec<_>>()
        .join("\n");
    gitignore.push('\n');
    repo.write(".gitignore", &gitignore)?;

    let output = repo.vizier_output_no_follow(&["init"])?;
    assert!(
        output.status.success(),
        "vizier init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let snapshot_after = repo.read(".vizier/narrative/snapshot.md")?;
    assert_eq!(
        snapshot_before, snapshot_after,
        "init should not overwrite existing snapshot content"
    );
    assert!(
        repo.path().join(".vizier/narrative/glossary.md").is_file(),
        "init should restore missing glossary marker"
    );

    let gitignore_after = repo.read(".gitignore")?;
    assert!(
        gitignore_after.contains(".vizier/sessions/"),
        "init should append missing sessions ignore rule:\n{gitignore_after}"
    );
    assert_eq!(
        gitignore_after.matches(".vizier/sessions/").count(),
        1,
        "sessions ignore rule should not be duplicated:\n{gitignore_after}"
    );
    Ok(())
}

#[test]
fn test_init_is_noop_when_already_satisfied() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let snapshot_before = repo.read(".vizier/narrative/snapshot.md")?;
    let glossary_before = repo.read(".vizier/narrative/glossary.md")?;
    let gitignore_before = repo.read(".gitignore")?;

    let output = repo.vizier_output_no_follow(&["init"])?;
    assert!(
        output.status.success(),
        "vizier init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("already satisfied"),
        "expected already satisfied outcome, got: {stdout}"
    );

    assert_eq!(
        snapshot_before,
        repo.read(".vizier/narrative/snapshot.md")?,
        "snapshot should remain unchanged when init is already satisfied"
    );
    assert_eq!(
        glossary_before,
        repo.read(".vizier/narrative/glossary.md")?,
        "glossary should remain unchanged when init is already satisfied"
    );
    assert_eq!(
        gitignore_before,
        repo.read(".gitignore")?,
        ".gitignore should remain unchanged when init is already satisfied"
    );
    Ok(())
}

#[test]
fn test_init_check_reports_missing_and_exits_non_zero() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    fs::remove_file(repo.path().join(".vizier/narrative/glossary.md"))?;
    let mut gitignore = repo.read(".gitignore")?;
    gitignore = gitignore
        .lines()
        .filter(|line| line.trim() != ".vizier/jobs/")
        .collect::<Vec<_>>()
        .join("\n");
    gitignore.push('\n');
    repo.write(".gitignore", &gitignore)?;

    let output = repo.vizier_output_no_follow(&["init", "--check"])?;
    assert!(
        !output.status.success(),
        "vizier init --check should fail when items are missing"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("missing required items"),
        "check mode should report missing items: {stdout}"
    );
    assert!(
        stdout.contains("missing: .vizier/narrative/glossary.md"),
        "check output should include missing glossary marker: {stdout}"
    );
    assert!(
        stdout.contains("missing: .gitignore: .vizier/jobs/"),
        "check output should include missing ignore rule: {stdout}"
    );
    Ok(())
}

#[test]
fn test_init_check_is_non_mutating_on_uninitialized_repo() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let narrative_dir = repo.path().join(".vizier/narrative");
    if narrative_dir.exists() {
        fs::remove_dir_all(&narrative_dir)?;
    }
    let jobs_dir = repo.path().join(".vizier/jobs");
    if jobs_dir.exists() {
        fs::remove_dir_all(&jobs_dir)?;
    }
    let sessions_dir = repo.path().join(".vizier/sessions");
    if sessions_dir.exists() {
        fs::remove_dir_all(&sessions_dir)?;
    }
    repo.write(".gitignore", "/target\n")?;

    let output = repo.vizier_output_no_follow(&["init", "--check"])?;
    assert!(
        !output.status.success(),
        "vizier init --check should fail on an uninitialized repo"
    );
    assert!(
        !repo.path().join(".vizier/jobs").exists(),
        "check mode should not create .vizier/jobs"
    );
    assert!(
        !repo.path().join(".vizier/sessions").exists(),
        "check mode should not create .vizier/sessions"
    );
    Ok(())
}

#[test]
fn test_init_fails_outside_git_repo() -> TestResult {
    let temp = TempDir::new()?;
    let output = Command::new(vizier_binary())
        .current_dir(temp.path())
        .arg("init")
        .output()?;

    assert!(
        !output.status.success(),
        "vizier init should fail outside a git repository"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not a git repository"),
        "expected explicit non-git error, got: {stderr}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn test_init_reports_path_specific_permission_error() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let gitignore_path = repo.path().join(".gitignore");
    let mut gitignore = repo.read(".gitignore")?;
    gitignore = gitignore
        .lines()
        .filter(|line| line.trim() != ".vizier/jobs/")
        .collect::<Vec<_>>()
        .join("\n");
    gitignore.push('\n');
    repo.write(".gitignore", &gitignore)?;

    let mut perms = fs::metadata(&gitignore_path)?.permissions();
    perms.set_mode(0o444);
    fs::set_permissions(&gitignore_path, perms)?;

    let output = repo.vizier_output_no_follow(&["init"])?;
    assert!(
        !output.status.success(),
        "vizier init should fail when .gitignore is not writable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(".gitignore"),
        "permission failure should include .gitignore path: {stderr}"
    );
    Ok(())
}
