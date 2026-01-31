use crate::fixtures::*;

#[test]
fn test_plan_command_outputs_resolved_config() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before_logs = gather_session_logs(&repo)?;
    let output = repo.vizier_output(&["plan"])?;
    assert!(
        output.status.success(),
        "vizier plan failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let compact = stdout.replace(' ', "");
    assert!(
        stdout.contains("Resolved configuration:"),
        "plan should print a resolved config header:\n{stdout}"
    );
    assert!(
        compact.contains("Agent:codex"),
        "plan output should include the resolved agent selector:\n{stdout}"
    );
    assert!(
        compact.contains("Stop-conditionscript:unset"),
        "plan output should include approve.stop_condition.script status:\n{stdout}"
    );
    assert!(
        compact.contains("CI/CDscript:./cicd.sh"),
        "plan output should include merge.cicd_gate.script:\n{stdout}"
    );
    assert!(
        stdout.contains("bundled `codex` shim"),
        "plan output should describe agent runtime resolution:\n{stdout}"
    );
    assert!(
        stdout.contains("Per-scope agents:"),
        "plan output should render per-scope agent settings:\n{stdout}"
    );

    let after_logs = gather_session_logs(&repo)?;
    assert_eq!(
        before_logs.len(),
        after_logs.len(),
        "vizier plan should not create session logs"
    );
    Ok(())
}
#[test]
fn test_plan_json_respects_config_file_and_overrides() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let config_path = repo.path().join("custom-config.toml");
    fs::write(
        &config_path,
        r#"
agent = "codex"
[approve.stop_condition]
script = "./approve-stop.sh"
retries = 7
[merge.cicd_gate]
script = "./alt-cicd.sh"
auto_resolve = false
retries = 5
[review.checks]
commands = ["echo alt-review"]
[workflow]
no_commit_default = true
"#,
    )?;

    let output = repo
        .vizier_cmd_with_config(&config_path)
        .args(["--agent", "gemini", "plan", "--json"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier plan --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout)?;

    assert_eq!(
        json.get("agent").and_then(Value::as_str),
        Some("gemini"),
        "CLI agent override should win even when config file is provided"
    );
    assert_eq!(
        json.pointer("/workflow/no_commit_default")
            .and_then(Value::as_bool),
        Some(true),
        "workflow.no_commit_default from the config file should appear in the report"
    );
    assert_eq!(
        json.pointer("/workflow/background/enabled")
            .and_then(Value::as_bool),
        Some(true),
        "workflow.background.enabled should appear in the report"
    );
    assert_eq!(
        json.pointer("/workflow/background/quiet")
            .and_then(Value::as_bool),
        Some(false),
        "workflow.background.quiet should appear in the report"
    );
    assert_eq!(
        json.pointer("/merge/cicd_gate/script")
            .and_then(Value::as_str),
        Some("./alt-cicd.sh"),
        "merge.cicd_gate.script from the config file should appear in the report"
    );
    assert_eq!(
        json.pointer("/merge/cicd_gate/retries")
            .and_then(Value::as_u64),
        Some(5),
        "merge.cicd_gate.retries from the config file should appear in the report"
    );
    assert_eq!(
        json.pointer("/approve/stop_condition/script")
            .and_then(Value::as_str),
        Some("./approve-stop.sh"),
        "approve.stop_condition.script from the config file should appear in the report"
    );
    assert_eq!(
        json.pointer("/approve/stop_condition/retries")
            .and_then(Value::as_u64),
        Some(7),
        "approve.stop_condition.retries from the config file should appear in the report"
    );
    assert_eq!(
        json.pointer("/review/checks/0").and_then(Value::as_str),
        Some("echo alt-review"),
        "review checks from the config file should appear in the report"
    );
    assert_eq!(
        json.pointer("/scopes/ask/agent").and_then(Value::as_str),
        Some("gemini"),
        "per-scope agent selector should reflect CLI overrides"
    );
    Ok(())
}
#[test]
fn test_plan_reports_agent_command_override() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let bin_dir = repo.path().join("bin");
    fs::create_dir_all(&bin_dir)?;
    let custom_bin = bin_dir.join("codex-custom");
    fs::write(&custom_bin, "#!/bin/sh\nexit 0\n")?;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&custom_bin)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&custom_bin, perms)?;
    }

    let output = repo
        .vizier_cmd()
        .args(["--agent-command", custom_bin.to_str().unwrap(), "plan"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier plan with agent command override failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let compact = stdout.replace(' ', "");
    assert!(
        compact.contains(&format!("Command:{}", custom_bin.display())),
        "plan output should surface the overridden agent command:\n{stdout}"
    );
    assert!(
        compact.contains("Resolution:providedcommand"),
        "plan output should mark the agent runtime as a provided command when CLI overrides are supplied:\n{stdout}"
    );
    Ok(())
}
