use crate::fixtures::*;

fn write_agent_script(path: &Path, output: &str) -> TestResult {
    fs::write(path, format!("#!/bin/sh\ncat >/dev/null\necho {output}\n"))?;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

#[test]
fn test_ask_creates_single_combined_commit() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;

    let output = repo.vizier_output(&["ask", "single commit check"])?;
    assert!(
        output.status.success(),
        "vizier ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after = count_commits_from_head(&repo.repo())?;
    assert_eq!(after - before, 1, "ask should create one combined commit");
    let files = files_changed_in_commit(&repo.repo(), "HEAD")?;
    assert!(
        files.contains(".vizier/narrative/snapshot.md")
            && files.contains(".vizier/narrative/glossary.md")
            && files.contains("a"),
        "ask commit should include code and narrative assets, got {files:?}"
    );
    Ok(())
}

#[test]
fn test_ask_commit_generation_stays_ask_scoped() -> TestResult {
    let repo = IntegrationRepo::new_without_mock()?;
    let scripts = tempfile::tempdir()?;
    let ask_script = scripts.path().join("ask-agent.sh");
    let save_script = scripts.path().join("save-agent.sh");
    write_agent_script(&ask_script, "ask-scope-commit-body")?;
    write_agent_script(&save_script, "save-scope-commit-body")?;

    let config_dir = tempfile::tempdir()?;
    let config_path = config_dir.path().join("scope-config.toml");
    let ask_cmd = ask_script.to_string_lossy().replace('\\', "\\\\");
    let save_cmd = save_script.to_string_lossy().replace('\\', "\\\\");
    fs::write(
        &config_path,
        format!(
            r#"
[agents.ask.agent]
label = "ask-test"
command = ["{ask_cmd}"]

[agents.save.agent]
label = "save-test"
command = ["{save_cmd}"]
"#
        ),
    )?;

    let output = repo
        .vizier_cmd_with_config(&config_path)
        .args(["ask", "scope regression check"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let commit = Command::new("git")
        .args([
            "-C",
            repo.path().to_str().unwrap(),
            "log",
            "-1",
            "--pretty=%B",
        ])
        .output()?;
    let message = String::from_utf8_lossy(&commit.stdout);
    assert!(
        message.contains("ask-scope-commit-body"),
        "ask commit should use ask-scoped runtime output, got:\n{message}"
    );
    assert!(
        !message.contains("save-scope-commit-body"),
        "ask commit should not use save-scoped runtime output, got:\n{message}"
    );
    Ok(())
}
#[test]
fn test_ask_reports_token_usage_progress() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo.vizier_output(&["ask", "token usage integration smoke"])?;
    assert!(
        output.status.success(),
        "vizier ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[codex:ask] agent — mock agent running"),
        "expected agent progress line, stderr was:\n{}",
        stderr
    );
    assert!(
        !stderr.to_ascii_lowercase().contains("token usage"),
        "token usage progress should not be emitted anymore:\n{}",
        stderr
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Agent run:"),
        "ask stdout should include agent run summary:\n{}",
        stdout
    );

    let quiet_repo = IntegrationRepo::new()?;
    let quiet = quiet_repo.vizier_output(&["-q", "ask", "quiet usage check"])?;
    assert!(
        quiet.status.success(),
        "quiet vizier ask failed: {}",
        String::from_utf8_lossy(&quiet.stderr)
    );
    let quiet_stderr = String::from_utf8_lossy(&quiet.stderr);
    assert!(
        quiet_stderr.is_empty(),
        "quiet mode should suppress agent progress but printed:\n{}",
        quiet_stderr
    );
    Ok(())
}
