use crate::fixtures::*;

#[test]
fn test_draft_reports_token_usage() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let sessions_root = repo.path().join(".vizier/sessions");
    if sessions_root.exists() {
        fs::remove_dir_all(&sessions_root)?;
    }
    let before_logs = gather_session_logs(&repo)?;

    let output = repo.vizier_output(&[
        "draft",
        "--name",
        "token-usage",
        "capture usage for draft plans",
    ])?;
    assert!(
        output.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Agent") && stdout.contains("codex"),
        "draft summary should include agent metadata:\n{stdout}"
    );
    assert!(
        stdout.contains("Exit code"),
        "draft summary should include the agent exit code:\n{stdout}"
    );

    let after_logs = gather_session_logs(&repo)?;
    let session_path = new_session_log(&before_logs, &after_logs)
        .ok_or_else(|| io::Error::other("expected session log for draft"))?;
    let contents = fs::read_to_string(session_path)?;
    let session_json: Value = serde_json::from_str(&contents)?;
    let agent = session_json
        .get("agent")
        .ok_or_else(|| io::Error::other("session log missing agent run data"))?;
    let exit_code = agent
        .get("exit_code")
        .and_then(Value::as_i64)
        .ok_or_else(|| io::Error::other("agent.exit_code missing"))?;
    assert_eq!(exit_code, 0, "agent exit code should be recorded");
    let stderr = agent
        .get("stderr")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        !stderr.is_empty(),
        "session log should include agent stderr lines"
    );

    Ok(())
}
#[test]
fn test_draft_creates_branch_and_plan() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;
    let before_logs = gather_session_logs(&repo)?;

    let output = repo.vizier_output(&["draft", "--name", "smoke", "ship the draft flow"])?;
    assert!(
        output.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after_logs = gather_session_logs(&repo)?;
    let session_log = new_session_log(&before_logs, &after_logs)
        .ok_or("expected vizier draft to create a session log")?;
    assert!(
        session_log.exists(),
        "session log should exist at {}",
        session_log.display()
    );

    assert!(
        !repo
            .path()
            .join(".vizier/implementation-plans/smoke.md")
            .exists(),
        "plan should not appear in the operator’s working tree"
    );

    let repo_handle = repo.repo();
    let branch = repo_handle.find_branch("draft/smoke", BranchType::Local)?;
    let commit = branch.get().peel_to_commit()?;
    let tree = commit.tree()?;
    let entry = tree.get_path(Path::new(".vizier/implementation-plans/smoke.md"))?;
    let blob = repo_handle.find_blob(entry.id())?;
    let contents = std::str::from_utf8(blob.content())?;
    assert!(contents.contains("ship the draft flow"));
    assert!(contents.contains("## Implementation Plan"));
    assert!(
        contents.contains("plan: smoke"),
        "plan front matter should include slug"
    );
    assert!(
        contents.contains("branch: draft/smoke"),
        "plan front matter should include branch"
    );
    assert!(
        !contents.contains("status:"),
        "plan metadata should omit status fields"
    );

    let after = count_commits_from_head(&repo_handle)?;
    assert_eq!(after, before, "draft should not add commits to master");
    Ok(())
}
#[test]
fn test_draft_fails_when_codex_errors() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let mut cmd = repo.vizier_cmd();
    cmd.env("VIZIER_FORCE_AGENT_ERROR", "1");
    cmd.args(["draft", "--name", "codex-failure", "force failure"]);
    let output = cmd.output()?;
    assert!(
        !output.status.success(),
        "vizier draft should fail when the backend errors"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.to_ascii_lowercase().contains("agent command exited"),
        "stderr should mention agent command failure, got: {stderr}"
    );
    assert!(
        stderr.contains("42"),
        "stderr should include the exit status, got: {stderr}"
    );
    let plan_path = repo
        .path()
        .join(".vizier/implementation-plans/codex-failure.md");
    assert!(
        !plan_path.exists(),
        "failed draft should not leave a partially written plan"
    );
    Ok(())
}
