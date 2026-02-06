use crate::fixtures::*;

#[test]
fn test_load_session_ignores_legacy_config_dir() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let session_id = "legacy-session";
    let config_root = TempDir::new()?;
    let legacy_dir = config_root.path().join("vizier");
    fs::create_dir_all(&legacy_dir)?;
    fs::write(
        legacy_dir.join(format!("{session_id}.json")),
        r#"[{"role":"User","content":"legacy"}]"#,
    )?;

    let output = repo
        .vizier_cmd()
        .env("VIZIER_CONFIG_DIR", config_root.path())
        .env("XDG_CONFIG_HOME", config_root.path())
        .args(["--load-session", session_id, "plan"])
        .output()?;
    assert!(
        !output.status.success(),
        "vizier --load-session should require repo-local sessions"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("could not find session file"),
        "stderr should mention missing repo session file, got: {stderr}"
    );
    Ok(())
}

#[test]
fn test_session_log_uses_v1_schema_and_repo_local_path() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = gather_session_logs(&repo)?;

    let output = repo.vizier_output(&["save"])?;
    assert!(
        output.status.success(),
        "vizier save failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after = gather_session_logs(&repo)?;
    let session_path = new_session_log(&before, &after)
        .ok_or_else(|| io::Error::other("missing new session log"))?
        .clone();

    let relative = session_path
        .strip_prefix(repo.path())
        .map_err(|_| io::Error::other("session log path not repo-local"))?;
    let relative_str = relative.to_string_lossy();
    assert!(
        relative_str.starts_with(".vizier/sessions/"),
        "session log path should live under .vizier/sessions/, got: {relative_str}"
    );
    assert!(
        relative_str.ends_with("/session.json"),
        "session log filename should be session.json, got: {relative_str}"
    );

    let contents = fs::read_to_string(&session_path)?;
    let session_json: Value = serde_json::from_str(&contents)?;
    assert_eq!(
        session_json.get("schema").and_then(Value::as_str),
        Some("vizier.session.v1"),
        "session logs should carry the v1 schema marker"
    );

    Ok(())
}

#[test]
fn test_session_log_captures_token_usage_totals() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo.vizier_cmd().arg("save").output()?;
    assert!(
        output.status.success(),
        "vizier save failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[codex:save] agent — mock agent running"),
        "stderr missing agent progress lines:\n{}",
        stderr
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let contents = session_log_contents_from_output(&repo, &stdout)?;
    let session_json: Value = serde_json::from_str(&contents)?;
    let agent = session_json
        .get("agent")
        .ok_or_else(|| io::Error::other("session log missing agent run data"))?;
    assert_eq!(
        agent.get("exit_code").and_then(Value::as_i64),
        Some(0),
        "session log should record agent exit status"
    );
    let stderr_lines = agent
        .get("stderr")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        !stderr_lines.is_empty(),
        "agent stderr should be captured in session log"
    );
    let stdout_value = agent
        .get("stdout")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        !stdout_value.trim().is_empty(),
        "agent stdout should be captured in session log"
    );
    Ok(())
}
#[test]
fn test_session_log_handles_unknown_token_usage() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = gather_session_logs(&repo)?;

    let mut cmd = repo.vizier_cmd();
    cmd.args(["-q", "ask", "suppress usage event"]);
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "vizier ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.trim().is_empty(),
        "quiet ask should not emit stderr: {stderr}"
    );

    let after = gather_session_logs(&repo)?;
    let session_path = new_session_log(&before, &after)
        .ok_or_else(|| io::Error::other("missing new session log"))?
        .clone();
    let contents = fs::read_to_string(&session_path)?;
    let session_json: Value = serde_json::from_str(&contents)?;
    let agent = session_json
        .get("agent")
        .ok_or_else(|| io::Error::other("session log missing agent run data"))?;
    assert_eq!(
        agent.get("exit_code").and_then(Value::as_i64),
        Some(0),
        "session log should still record agent exit even when output is quiet"
    );
    Ok(())
}
#[test]
fn test_script_runner_session_logs_io_across_commands() -> TestResult {
    let repo = IntegrationRepo::new_without_mock()?;

    let capture_agent_log =
        |args: &[&str], label: &str| -> Result<Value, Box<dyn std::error::Error>> {
            let before = gather_session_logs(&repo)?;
            let mut cmd = repo.vizier_cmd();
            cmd.env("OPENAI_API_KEY", "test-key");
            cmd.env("ANTHROPIC_API_KEY", "test-key");
            cmd.args(args);
            let output = cmd.output()?;
            assert!(
                output.status.success(),
                "vizier {label} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let after = gather_session_logs(&repo)?;
            let session_path = new_session_log(&before, &after)
                .ok_or_else(|| format!("missing session log for {label}"))?
                .clone();
            let contents = fs::read_to_string(session_path)?;
            let json: Value = serde_json::from_str(&contents)?;
            Ok(json)
        };

    let assert_agent_io = |json: &Value, label: &str| {
        let agent = json
            .get("agent")
            .unwrap_or_else(|| panic!("session log missing agent run for {label}"));
        let command: Vec<String> = agent
            .get("command")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            command.iter().any(|entry| entry.contains("codex")),
            "{label} session log should capture agent command, got {command:?}"
        );
        assert!(
            agent
                .get("stdout")
                .and_then(Value::as_str)
                .unwrap_or("")
                .contains("mock agent response"),
            "{label} session log should persist agent stdout"
        );
        let stderr_lines: Vec<String> = agent
            .get("stderr")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            stderr_lines
                .iter()
                .any(|line| line.contains("mock agent running")),
            "{label} session log should capture agent stderr, found {stderr_lines:?}"
        );
        assert!(
            agent
                .get("duration_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 0,
            "{label} session log should record duration"
        );
    };

    let ask = capture_agent_log(&["ask", "script runner smoke"], "ask")?;
    assert_agent_io(&ask, "ask");

    let save = capture_agent_log(&["save"], "save")?;
    assert_agent_io(&save, "save");

    let draft = capture_agent_log(
        &["draft", "--name", "script-runner", "script runner plan"],
        "draft",
    )?;
    assert_agent_io(&draft, "draft");

    clean_workdir(&repo)?;

    let approve = capture_agent_log(&["approve", "script-runner", "--yes"], "approve")?;
    assert_agent_io(&approve, "approve");

    clean_workdir(&repo)?;

    let review = capture_agent_log(
        &["review", "script-runner", "--review-only", "--skip-checks"],
        "review",
    )?;
    assert_agent_io(&review, "review");

    clean_workdir(&repo)?;

    let mut merge_cmd = repo.vizier_cmd();
    merge_cmd.env("OPENAI_API_KEY", "test-key");
    merge_cmd.env("ANTHROPIC_API_KEY", "test-key");
    merge_cmd.args(["merge", "script-runner", "--yes"]);
    let merge = merge_cmd.output()?;
    assert!(
        merge.status.success(),
        "vizier merge failed with real script runner: {}",
        String::from_utf8_lossy(&merge.stderr)
    );

    Ok(())
}
