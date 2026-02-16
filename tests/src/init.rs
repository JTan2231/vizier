use crate::fixtures::*;

const REQUIRED_IGNORE_RULES: [&str; 5] = [
    ".vizier/tmp-worktrees/",
    ".vizier/tmp/",
    ".vizier/sessions/",
    ".vizier/jobs/",
    ".vizier/implementation-plans",
];

const REQUIRED_PROMPT_FILES: [(&str, &str); 4] = [
    (
        ".vizier/prompts/DRAFT_PROMPTS.md",
        "vizier-cli/templates/init/prompts/DRAFT_PROMPTS.md",
    ),
    (
        ".vizier/prompts/APPROVE_PROMPTS.md",
        "vizier-cli/templates/init/prompts/APPROVE_PROMPTS.md",
    ),
    (
        ".vizier/prompts/MERGE_PROMPTS.md",
        "vizier-cli/templates/init/prompts/MERGE_PROMPTS.md",
    ),
    (
        ".vizier/prompts/COMMIT_PROMPTS.md",
        "vizier-cli/templates/init/prompts/COMMIT_PROMPTS.md",
    ),
];

fn assert_matches_repo_template(
    repo: &IntegrationRepo,
    actual_rel: &str,
    template_rel: &str,
) -> TestResult {
    let expected = fs::read_to_string(repo_root().join(template_rel))?;
    let actual = repo.read(actual_rel)?;
    assert_eq!(
        actual, expected,
        "{actual_rel} should match template {template_rel}"
    );
    Ok(())
}

#[test]
fn test_init_creates_required_scaffold_and_ignore_rules() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let narrative_dir = repo.path().join(".vizier/narrative");
    if narrative_dir.exists() {
        fs::remove_dir_all(&narrative_dir)?;
    }
    let config_path = repo.path().join(".vizier/config.toml");
    if config_path.exists() {
        fs::remove_file(&config_path)?;
    }
    let workflows_dir = repo.path().join(".vizier/workflows");
    if workflows_dir.exists() {
        fs::remove_dir_all(&workflows_dir)?;
    }
    let prompts_dir = repo.path().join(".vizier/prompts");
    if prompts_dir.exists() {
        fs::remove_dir_all(&prompts_dir)?;
    }
    let ci_path = repo.path().join("ci.sh");
    if ci_path.exists() {
        fs::remove_file(&ci_path)?;
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
    assert!(
        config_path.is_file(),
        "init should create .vizier/config.toml"
    );
    let config = repo.read(".vizier/config.toml")?;
    assert!(
        config.contains("script = \"./ci.sh\""),
        "init config should point merge ci gate to ./ci.sh:\n{config}"
    );
    assert!(
        config.contains("commit = \"file:.vizier/workflows/commit.toml\""),
        "init config should include the commit alias:\n{config}"
    );
    assert_matches_repo_template(
        &repo,
        ".vizier/workflows/draft.toml",
        ".vizier/workflows/draft.toml",
    )?;
    assert_matches_repo_template(
        &repo,
        ".vizier/workflows/approve.toml",
        ".vizier/workflows/approve.toml",
    )?;
    assert_matches_repo_template(
        &repo,
        ".vizier/workflows/merge.toml",
        ".vizier/workflows/merge.toml",
    )?;
    assert_matches_repo_template(
        &repo,
        ".vizier/workflows/commit.toml",
        ".vizier/workflows/commit.toml",
    )?;
    for (actual_rel, template_rel) in REQUIRED_PROMPT_FILES {
        assert_matches_repo_template(&repo, actual_rel, template_rel)?;
    }
    assert!(ci_path.is_file(), "init should create root ci.sh");
    let ci_contents = repo.read("ci.sh")?;
    assert!(
        ci_contents.contains("vizier ci stub"),
        "init should write the ci.sh stub:\n{ci_contents}"
    );
    #[cfg(unix)]
    {
        let mode = fs::metadata(&ci_path)?.permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "init should mark ci.sh executable");
    }

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
    let draft_before = repo.read(".vizier/workflows/draft.toml")?;
    let merge_prompt_before = repo.read(".vizier/prompts/MERGE_PROMPTS.md")?;
    fs::remove_file(repo.path().join(".vizier/narrative/glossary.md"))?;
    fs::remove_file(repo.path().join(".vizier/config.toml"))?;
    fs::remove_file(repo.path().join(".vizier/workflows/approve.toml"))?;
    fs::remove_file(repo.path().join(".vizier/prompts/APPROVE_PROMPTS.md"))?;
    let ci_path = repo.path().join("ci.sh");
    if ci_path.exists() {
        fs::remove_file(&ci_path)?;
    }

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
    assert!(
        repo.path().join(".vizier/config.toml").is_file(),
        "init should restore missing .vizier/config.toml"
    );
    assert!(
        repo.path().join(".vizier/workflows/approve.toml").is_file(),
        "init should restore missing approve workflow"
    );
    assert!(
        repo.path()
            .join(".vizier/prompts/APPROVE_PROMPTS.md")
            .is_file(),
        "init should restore missing approve prompt companion"
    );
    assert!(ci_path.is_file(), "init should restore missing ci.sh");
    assert_eq!(
        draft_before,
        repo.read(".vizier/workflows/draft.toml")?,
        "init should not overwrite existing workflow files"
    );
    assert_eq!(
        merge_prompt_before,
        repo.read(".vizier/prompts/MERGE_PROMPTS.md")?,
        "init should not overwrite existing prompt files"
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

    let bootstrap = repo.vizier_output_no_follow(&["init"])?;
    assert!(
        bootstrap.status.success(),
        "vizier init bootstrap failed: {}",
        String::from_utf8_lossy(&bootstrap.stderr)
    );

    let snapshot_before = repo.read(".vizier/narrative/snapshot.md")?;
    let glossary_before = repo.read(".vizier/narrative/glossary.md")?;
    let config_before = repo.read(".vizier/config.toml")?;
    let draft_before = repo.read(".vizier/workflows/draft.toml")?;
    let commit_before = repo.read(".vizier/workflows/commit.toml")?;
    let prompts_before = REQUIRED_PROMPT_FILES
        .iter()
        .map(|(actual_rel, _)| repo.read(actual_rel))
        .collect::<Result<Vec<_>, _>>()?;
    let ci_before = repo.read("ci.sh")?;
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
        config_before,
        repo.read(".vizier/config.toml")?,
        "config should remain unchanged when init is already satisfied"
    );
    assert_eq!(
        draft_before,
        repo.read(".vizier/workflows/draft.toml")?,
        "workflow templates should remain unchanged when init is already satisfied"
    );
    assert_eq!(
        commit_before,
        repo.read(".vizier/workflows/commit.toml")?,
        "workflow templates should remain unchanged when init is already satisfied"
    );
    for ((actual_rel, _), before) in REQUIRED_PROMPT_FILES.iter().zip(prompts_before.iter()) {
        assert_eq!(
            before,
            &repo.read(actual_rel)?,
            "prompt companion should remain unchanged when init is already satisfied"
        );
    }
    assert_eq!(
        ci_before,
        repo.read("ci.sh")?,
        "ci.sh should remain unchanged when init is already satisfied"
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

    let bootstrap = repo.vizier_output_no_follow(&["init"])?;
    assert!(
        bootstrap.status.success(),
        "vizier init bootstrap failed: {}",
        String::from_utf8_lossy(&bootstrap.stderr)
    );

    fs::remove_file(repo.path().join(".vizier/narrative/glossary.md"))?;
    fs::remove_file(repo.path().join(".vizier/prompts/DRAFT_PROMPTS.md"))?;
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
        stdout.contains("missing: .vizier/prompts/DRAFT_PROMPTS.md"),
        "check output should include missing prompt companion: {stdout}"
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
    let config = repo.path().join(".vizier/config.toml");
    if config.exists() {
        fs::remove_file(&config)?;
    }
    let workflows = repo.path().join(".vizier/workflows");
    if workflows.exists() {
        fs::remove_dir_all(&workflows)?;
    }
    let prompts = repo.path().join(".vizier/prompts");
    if prompts.exists() {
        fs::remove_dir_all(&prompts)?;
    }
    let ci_path = repo.path().join("ci.sh");
    if ci_path.exists() {
        fs::remove_file(&ci_path)?;
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
    assert!(
        !repo.path().join(".vizier/config.toml").exists(),
        "check mode should not create .vizier/config.toml"
    );
    assert!(
        !repo.path().join(".vizier/workflows").exists(),
        "check mode should not create .vizier/workflows"
    );
    assert!(
        !repo.path().join(".vizier/prompts").exists(),
        "check mode should not create .vizier/prompts"
    );
    assert!(
        !repo.path().join("ci.sh").exists(),
        "check mode should not create ci.sh"
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

    let bootstrap = repo.vizier_output_no_follow(&["init"])?;
    assert!(
        bootstrap.status.success(),
        "vizier init bootstrap failed: {}",
        String::from_utf8_lossy(&bootstrap.stderr)
    );

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
