use crate::fixtures::*;

#[test]
fn test_test_display_smoke_is_clean() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let output = repo.vizier_output(&["test-display"])?;
    assert!(
        output.status.success(),
        "test-display should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Agent display test succeeded"),
        "stdout missing success summary: {stdout}"
    );

    let status = Command::new("git")
        .args(["-C", repo.path().to_str().unwrap(), "status", "--porcelain"])
        .output()?;
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "test-display should not touch the repo: {}",
        String::from_utf8_lossy(&status.stdout)
    );
    Ok(())
}
#[test]
fn test_test_display_propagates_agent_exit_code() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let mut cmd = repo.vizier_cmd();
    cmd.arg("test-display");
    cmd.env("VIZIER_FORCE_AGENT_ERROR", "true");
    let output = cmd.output()?;
    assert_eq!(
        output.status.code(),
        Some(42),
        "expected test-display to exit with the agent status"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("status 42") || stderr.contains("agent"),
        "stderr should mention agent failure: {stderr}"
    );

    let status = Command::new("git")
        .args(["-C", repo.path().to_str().unwrap(), "status", "--porcelain"])
        .output()?;
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "failure path should leave the repo untouched: {}",
        String::from_utf8_lossy(&status.stdout)
    );
    Ok(())
}
#[test]
fn test_test_display_raw_and_quiet_modes() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let raw = repo.vizier_output(&["test-display", "--raw"])?;
    assert!(
        raw.status.success(),
        "raw run failed: {}",
        String::from_utf8_lossy(&raw.stderr)
    );
    let raw_stdout = String::from_utf8_lossy(&raw.stdout);
    assert!(
        raw_stdout.contains("mock agent response"),
        "raw output should include captured stdout: {raw_stdout}"
    );
    let raw_stderr = String::from_utf8_lossy(&raw.stderr);
    assert!(
        raw_stderr.contains("mock agent running") || raw_stderr.contains("mock stderr"),
        "raw stderr should surface progress or captured stderr: {raw_stderr}"
    );

    let quiet = repo.vizier_output(&["-q", "test-display"])?;
    assert!(
        quiet.status.success(),
        "quiet run failed: {}",
        String::from_utf8_lossy(&quiet.stderr)
    );
    assert!(
        String::from_utf8_lossy(&quiet.stdout).trim().is_empty(),
        "quiet mode should suppress stdout summary: {}",
        String::from_utf8_lossy(&quiet.stdout)
    );
    Ok(())
}
#[test]
fn test_test_display_can_write_session_when_opted_in() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let before_logs = gather_session_logs(&repo)?;
    let output = repo.vizier_output(&["test-display", "--session"])?;
    assert!(
        output.status.success(),
        "session-enabled run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let after_logs = gather_session_logs(&repo)?;
    let new_log = new_session_log(&before_logs, &after_logs)
        .ok_or("expected test-display to write a session log when --session is set")?;
    let contents = fs::read_to_string(new_log)?;
    let json: Value = serde_json::from_str(&contents)?;
    assert_eq!(
        json.get("model")
            .and_then(|model| model.get("scope"))
            .and_then(Value::as_str),
        Some("template.save.v1"),
        "session log should record the resolved template scope for the default command alias"
    );
    Ok(())
}

#[test]
fn test_test_display_legacy_scope_flag_matches_command_alias() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let before_logs = gather_session_logs(&repo)?;
    let command_output = repo.vizier_output(&["test-display", "--command", "save", "--session"])?;
    assert!(
        command_output.status.success(),
        "--command run failed: {}",
        String::from_utf8_lossy(&command_output.stderr)
    );
    let after_command_logs = gather_session_logs(&repo)?;
    let command_log = new_session_log(&before_logs, &after_command_logs)
        .ok_or("expected test-display --command to produce a session log")?;
    let command_json: Value = serde_json::from_str(&fs::read_to_string(command_log)?)?;
    let command_scope = command_json
        .get("model")
        .and_then(|model| model.get("scope"))
        .and_then(Value::as_str);
    assert_eq!(
        command_scope,
        Some("template.save.v1"),
        "--command should resolve through template.save.v1 by default"
    );

    let scope_output = repo.vizier_output(&["test-display", "--scope", "save", "--session"])?;
    assert!(
        scope_output.status.success(),
        "--scope run failed: {}",
        String::from_utf8_lossy(&scope_output.stderr)
    );
    let after_scope_logs = gather_session_logs(&repo)?;
    let scope_log = new_session_log(&after_command_logs, &after_scope_logs)
        .ok_or("expected test-display --scope to produce a session log")?;
    let scope_json: Value = serde_json::from_str(&fs::read_to_string(scope_log)?)?;
    let scope_scope = scope_json
        .get("model")
        .and_then(|model| model.get("scope"))
        .and_then(Value::as_str);
    assert_eq!(
        scope_scope, command_scope,
        "legacy --scope should resolve to the same profile as --command"
    );
    Ok(())
}
