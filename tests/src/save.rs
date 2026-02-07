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
fn test_save() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;

    let output = repo.vizier_cmd().arg("save").output()?;
    assert!(
        output.status.success(),
        "vizier save exited with {:?}",
        output.status
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    let after = count_commits_from_head(&repo.repo())?;
    assert_eq!(after - before, 1, "save should create a single commit");

    let files = files_changed_in_commit(&repo.repo(), "HEAD")?;
    assert!(
        files.contains("a")
            && files.contains(".vizier/narrative/snapshot.md")
            && files.contains(".vizier/narrative/glossary.md"),
        "combined commit should include code + narrative files, got {files:?}"
    );

    let snapshot = repo.read(".vizier/narrative/snapshot.md")?;
    assert!(
        snapshot.contains("some snapshot change"),
        "expected mock backend snapshot update"
    );

    let session_log = session_log_contents_from_output(&repo, &stdout)?;
    assert!(
        session_log
            .to_ascii_lowercase()
            .contains("mock agent response"),
        "session log missing backend response"
    );
    Ok(())
}

#[test]
fn test_save_commit_generation_respects_save_scope_override() -> TestResult {
    let repo = IntegrationRepo::new_without_mock()?;
    let scripts = tempfile::tempdir()?;
    let default_script = scripts.path().join("default-agent.sh");
    let save_script = scripts.path().join("save-agent.sh");
    write_agent_script(&default_script, "default-scope-commit-body")?;
    write_agent_script(&save_script, "save-scope-commit-body")?;

    let config_dir = tempfile::tempdir()?;
    let config_path = config_dir.path().join("scope-config.toml");
    let default_cmd = default_script.to_string_lossy().replace('\\', "\\\\");
    let save_cmd = save_script.to_string_lossy().replace('\\', "\\\\");
    fs::write(
        &config_path,
        format!(
            r#"
[agents.default.agent]
label = "default-test"
command = ["{default_cmd}"]

[agents.save.agent]
label = "save-test"
command = ["{save_cmd}"]
"#
        ),
    )?;

    let output = repo
        .vizier_cmd_with_config(&config_path)
        .args(["save"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier save failed: {}",
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
    let mut parts = message.splitn(2, "\nNarrative updates:\n");
    let commit_section = parts.next().unwrap_or_default();
    let narrative_section = parts.next().unwrap_or_default();
    assert!(
        commit_section.contains("save-scope-commit-body"),
        "save commit should use save-scoped runtime output, got:\n{message}"
    );
    assert!(
        !commit_section.contains("default-scope-commit-body"),
        "save commit should not use default-scoped runtime output in the main commit body, got:\n{message}"
    );
    assert!(
        narrative_section.contains("default-scope-commit-body"),
        "narrative update section should still reflect default-scoped runtime output, got:\n{message}"
    );
    Ok(())
}
#[test]
fn test_save_with_staged_files() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;
    repo.write("b", "this is an integration test")?;
    add_all(&repo.repo(), &["."])?;

    let status = repo.vizier_cmd().arg("save").status()?;
    assert!(status.success(), "vizier save exited with {status:?}");

    let repo_handle = repo.repo();
    let after = count_commits_from_head(&repo_handle)?;
    assert_eq!(
        after - before,
        1,
        "save should still create a single combined commit when files are pre-staged"
    );
    let files = files_changed_in_commit(&repo_handle, "HEAD")?;
    assert!(
        files.contains("b")
            && files.contains(".vizier/narrative/snapshot.md")
            && files.contains(".vizier/narrative/glossary.md"),
        "combined commit should include staged code and narrative files, got {files:?}"
    );
    Ok(())
}
#[test]
fn test_save_with_staged_change_and_unstaged_deletion() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;

    repo.write("b", "staged change")?;
    repo.git(&["add", "b"])?;
    fs::remove_file(repo.path().join("c"))?;

    let status = repo.vizier_cmd().arg("save").status()?;
    assert!(status.success(), "vizier save exited with {status:?}");

    let repo_handle = repo.repo();
    let after = count_commits_from_head(&repo_handle)?;
    assert_eq!(
        after - before,
        1,
        "save should create a single combined commit with deletion"
    );
    let files = files_changed_in_commit(&repo_handle, "HEAD")?;
    assert!(
        files.contains("b") && files.contains("c"),
        "expected commit to include staged change + deletion, got {files:?}"
    );
    Ok(())
}
#[test]
fn test_save_without_code_changes() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;

    let mut cmd = repo.vizier_cmd();
    cmd.arg("save");
    cmd.env("VIZIER_IT_SKIP_CODE_CHANGE", "1");
    let output = cmd.output()?;

    assert!(
        output.status.success(),
        "vizier save failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let session_log = session_log_contents_from_output(&repo, &stdout)?;
    assert!(
        session_log
            .to_ascii_lowercase()
            .contains("mock agent response"),
        "session log missing backend response"
    );

    let after = count_commits_from_head(&repo.repo())?;
    assert_eq!(after - before, 1, "should create a single commit");
    let files = files_changed_in_commit(&repo.repo(), "HEAD")?;
    assert!(
        files.contains(".vizier/narrative/snapshot.md")
            && files.contains(".vizier/narrative/glossary.md")
            && !files.contains("a"),
        "expected commit to contain only narrative assets when code changes are skipped, got {files:?}"
    );
    Ok(())
}
#[test]
fn test_save_with_deleted_narrative_file() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;

    fs::remove_file(repo.path().join(".vizier/narrative/threads/demo.md"))?;

    let mut cmd = repo.vizier_cmd();
    cmd.arg("save");
    cmd.env("VIZIER_IT_SKIP_VIZIER_CHANGE", "1");
    let status = cmd.status()?;
    assert!(status.success(), "vizier save exited with {status:?}");

    let repo_handle = repo.repo();
    let after = count_commits_from_head(&repo_handle)?;
    assert_eq!(after - before, 1, "save should create a single commit");

    let files = files_changed_in_commit(&repo_handle, "HEAD")?;
    assert!(
        files.contains(".vizier/narrative/threads/demo.md"),
        "expected commit to include deleted narrative file, got {files:?}"
    );
    Ok(())
}
#[test]
fn test_save_allows_snapshot_without_glossary_update() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let mut cmd = repo.vizier_cmd();
    cmd.arg("save");
    cmd.env("VIZIER_IT_SKIP_GLOSSARY_CHANGE", "1");
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "save should succeed even when snapshot updates omit glossary updates"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stderr}\n{stdout}");
    assert!(
        !combined
            .to_ascii_lowercase()
            .contains("snapshot updates must include a glossary update"),
        "unexpected glossary enforcement message, got: {combined}"
    );
    Ok(())
}
#[test]
fn test_save_no_commit_leaves_pending_changes() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;

    let output = repo.vizier_cmd().args(["--no-commit", "save"]).output()?;
    assert!(
        output.status.success(),
        "vizier save --no-commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after = count_commits_from_head(&repo.repo())?;
    assert_eq!(
        after, before,
        "no-commit save should not create new commits"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Mode") && stdout.to_ascii_lowercase().contains("manual"),
        "expected manual mode indicator in output but saw: {stdout}"
    );

    let status = Command::new("git")
        .args([
            "-C",
            repo.path().to_str().unwrap(),
            "status",
            "--short",
            ".vizier/narrative/snapshot.md",
        ])
        .output()?;
    let status_stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        status_stdout.contains(".vizier/narrative/snapshot.md"),
        "expected .vizier/narrative/snapshot.md to be dirty after --no-commit save, git status was: {status_stdout}"
    );
    let glossary_status = Command::new("git")
        .args([
            "-C",
            repo.path().to_str().unwrap(),
            "status",
            "--short",
            ".vizier/narrative/glossary.md",
        ])
        .output()?;
    let glossary_stdout = String::from_utf8_lossy(&glossary_status.stdout);
    assert!(
        glossary_stdout.contains(".vizier/narrative/glossary.md"),
        "expected .vizier/narrative/glossary.md to be dirty after --no-commit save, git status was: {glossary_stdout}"
    );

    let code_status = Command::new("git")
        .args([
            "-C",
            repo.path().to_str().unwrap(),
            "status",
            "--short",
            "a",
        ])
        .output()?;
    let code_stdout = String::from_utf8_lossy(&code_status.stdout);
    assert!(
        code_stdout.contains("a"),
        "expected code changes to remain unstaged after --no-commit save, git status was: {code_stdout}"
    );
    Ok(())
}
