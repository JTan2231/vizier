use crate::fixtures::*;

#[test]
fn test_repo_config_overrides_env_config() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let repo_config = repo.path().join(".vizier").join("config.toml");
    fs::write(
        &repo_config,
        r#"
[agents.default]
agent = "codex"
"#,
    )?;

    let env_config = repo.path().join("env-config.toml");
    fs::write(
        &env_config,
        r#"
[agents.default]
agent = "gemini"
"#,
    )?;

    let before_logs = gather_session_logs(&repo)?;
    let isolated_config = TempDir::new()?;
    let mut cmd = repo.vizier_cmd();
    cmd.env("VIZIER_CONFIG_FILE", env_config.as_os_str());
    cmd.env("VIZIER_CONFIG_DIR", isolated_config.path());
    cmd.env("XDG_CONFIG_HOME", isolated_config.path());
    cmd.args(["ask", "repo config should win over env"]);
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "vizier ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after_logs = gather_session_logs(&repo)?;
    let new_log = new_session_log(&before_logs, &after_logs)
        .ok_or("expected vizier ask to produce a new session log")?;
    let contents = fs::read_to_string(new_log)?;
    let json: Value = serde_json::from_str(&contents)?;
    assert_eq!(
        json.get("model")
            .and_then(|model| model.get("provider"))
            .and_then(Value::as_str),
        Some("agent"),
        "repo config should force ask onto the configured backend despite env overrides"
    );
    Ok(())
}
#[test]
fn test_env_config_used_when_repo_config_missing() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let repo_toml = repo.path().join(".vizier").join("config.toml");
    if repo_toml.exists() {
        fs::remove_file(&repo_toml)?;
    }
    let repo_json = repo.path().join(".vizier").join("config.json");
    if repo_json.exists() {
        fs::remove_file(&repo_json)?;
    }

    let env_config = repo.path().join("env-config.toml");
    fs::write(
        &env_config,
        r#"
[agents.default]
agent = "codex"
"#,
    )?;

    let before_logs = gather_session_logs(&repo)?;
    let isolated_config = TempDir::new()?;
    let mut cmd = repo.vizier_cmd();
    cmd.env("VIZIER_CONFIG_FILE", env_config.as_os_str());
    cmd.env("VIZIER_CONFIG_DIR", isolated_config.path());
    cmd.env("XDG_CONFIG_HOME", isolated_config.path());
    cmd.args(["ask", "env config selection"]);
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "vizier ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after_logs = gather_session_logs(&repo)?;
    let new_log = new_session_log(&before_logs, &after_logs)
        .ok_or("expected vizier ask to create a session log")?;
    let contents = fs::read_to_string(new_log)?;
    let json: Value = serde_json::from_str(&contents)?;
    assert_eq!(
        json.get("model")
            .and_then(|model| model.get("provider"))
            .and_then(Value::as_str),
        Some("agent"),
        "env config should take effect when no repo config exists"
    );
    Ok(())
}
#[test]
fn test_global_review_checks_fill_repo_defaults() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let repo_config = repo.path().join(".vizier").join("config.toml");
    fs::write(
        &repo_config,
        r#"
[merge.cicd_gate]
script = "./cicd.sh"
auto_resolve = true
retries = 2
"#,
    )?;

    let config_root = TempDir::new()?;
    let global_dir = config_root.path().join("vizier");
    fs::create_dir_all(&global_dir)?;
    let check_marker = repo.path().join("global-review-check.txt");
    fs::write(
        global_dir.join("config.toml"),
        format!(
            r#"
[review.checks]
commands = ["echo global-review-check >> \"{}\""]
"#,
            check_marker.display()
        ),
    )?;

    let mut draft_cmd = repo.vizier_cmd();
    draft_cmd.env("VIZIER_CONFIG_DIR", config_root.path());
    draft_cmd.env("XDG_CONFIG_HOME", config_root.path());
    draft_cmd.args([
        "draft",
        "--name",
        "global-review-check",
        "global review check spec",
    ]);
    let draft = draft_cmd.output()?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    clean_workdir(&repo)?;

    let mut approve_cmd = repo.vizier_cmd();
    approve_cmd.env("VIZIER_CONFIG_DIR", config_root.path());
    approve_cmd.env("XDG_CONFIG_HOME", config_root.path());
    approve_cmd.args(["approve", "global-review-check", "--yes"]);
    let approve = approve_cmd.output()?;
    assert!(
        approve.status.success(),
        "vizier approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );

    clean_workdir(&repo)?;

    let mut review_cmd = repo.vizier_cmd();
    review_cmd.env("VIZIER_CONFIG_DIR", config_root.path());
    review_cmd.env("XDG_CONFIG_HOME", config_root.path());
    review_cmd.args(["review", "global-review-check", "--review-only"]);
    let review = review_cmd.output()?;
    assert!(
        review.status.success(),
        "vizier review failed: {}",
        String::from_utf8_lossy(&review.stderr)
    );

    assert!(
        check_marker.exists(),
        "global review check command should have created the marker file"
    );
    let contents = fs::read_to_string(&check_marker)?;
    assert!(
        contents.contains("global-review-check"),
        "marker file should include the check output, found: {contents}"
    );

    Ok(())
}
