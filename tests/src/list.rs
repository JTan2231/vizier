use crate::fixtures::*;

#[test]
fn test_list_outputs_prettified_blocks() -> TestResult {
    let repo = IntegrationRepo::new()?;

    let empty = repo.vizier_output(&["list"])?;
    assert!(
        empty.status.success(),
        "vizier list (empty) failed: {}",
        String::from_utf8_lossy(&empty.stderr)
    );
    let empty_stdout = String::from_utf8_lossy(&empty.stdout);
    assert!(
        empty_stdout.contains("Outcome: No pending draft branches"),
        "empty list output missing outcome: {empty_stdout}"
    );

    let draft_alpha = repo.vizier_output(&["draft", "--name", "alpha", "Alpha spec line"])?;
    assert!(
        draft_alpha.status.success(),
        "vizier draft alpha failed: {}",
        String::from_utf8_lossy(&draft_alpha.stderr)
    );
    let draft_beta = repo.vizier_output(&["draft", "--name", "beta", "Beta spec line"])?;
    assert!(
        draft_beta.status.success(),
        "vizier draft beta failed: {}",
        String::from_utf8_lossy(&draft_beta.stderr)
    );

    let list = repo.vizier_output(&["list"])?;
    assert!(
        list.status.success(),
        "vizier list failed: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(
        stdout.contains("Outcome: 2 pending draft branches"),
        "list header missing pending count: {stdout}"
    );
    let lines: Vec<&str> = stdout.lines().collect();
    let alpha_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("Plan") && line.contains("alpha"))
        .expect("list output missing plan alpha");
    let beta_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("Plan") && line.contains("beta"))
        .expect("list output missing plan beta");
    assert!(
        beta_idx > alpha_idx,
        "expected beta entry after alpha entry: {stdout}"
    );
    assert!(
        lines[alpha_idx + 1..beta_idx]
            .iter()
            .any(|line| line.trim().is_empty()),
        "list output should separate entries with whitespace: {stdout}"
    );
    for (slug, summary) in [("alpha", "Alpha spec line"), ("beta", "Beta spec line")] {
        assert!(
            lines
                .iter()
                .any(|line| line.trim_start().starts_with("Plan") && line.contains(slug)),
            "list output missing plan {slug}: {stdout}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.trim_start().starts_with("Branch") && line.contains(slug)),
            "list output missing branch for {slug}: {stdout}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.trim_start().starts_with("Summary") && line.contains(summary)),
            "list output missing summary for {slug}: {stdout}"
        );
    }

    Ok(())
}
#[test]
fn test_list_includes_inline_job_commands() -> TestResult {
    let repo = IntegrationRepo::new()?;

    let draft_alpha = repo.vizier_output(&["draft", "--name", "alpha", "Alpha spec line"])?;
    assert!(
        draft_alpha.status.success(),
        "vizier draft alpha failed: {}",
        String::from_utf8_lossy(&draft_alpha.stderr)
    );

    let job_id = "inline-job-alpha";
    write_job_record(
        &repo,
        job_id,
        json!({
            "id": job_id,
            "status": "running",
            "command": ["vizier", "approve"],
            "created_at": "2026-01-27T18:42:03Z",
            "started_at": "2026-01-27T18:42:03Z",
            "stdout_path": "stdout.log",
            "stderr_path": "stderr.log",
            "metadata": {
                "plan": "alpha",
                "branch": "draft/alpha",
                "command_alias": "approve"
            }
        }),
    )?;

    let list = repo.vizier_output(&["list"])?;
    assert!(
        list.status.success(),
        "vizier list failed: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(
        stdout.contains(job_id),
        "list output missing job id: {stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.contains("Job status") && line.contains("running")),
        "list output missing job status line: {stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.contains("Job scope") && line.contains("approve")),
        "list output missing job scope line: {stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.contains("Job started") && line.contains("2026-01-27T18:42:03")),
        "list output missing job started line: {stdout}"
    );
    assert!(
        stdout.contains(&format!("vizier jobs status {job_id}")),
        "list output missing status command: {stdout}"
    );
    assert!(
        stdout.contains(&format!("vizier jobs tail --follow {job_id}")),
        "list output missing follow logs command: {stdout}"
    );
    assert!(
        stdout.contains(&format!("vizier jobs attach {job_id}")),
        "list output missing attach command: {stdout}"
    );

    Ok(())
}

#[test]
fn test_list_table_format_from_config() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let config_path = repo.path().join(".vizier").join("config.toml");
    fs::write(
        &config_path,
        r#"
[display.lists.list]
format = "table"
entry_fields = ["Plan", "Summary"]
job_fields = []
command_fields = []
summary_max_len = 80
"#,
    )?;

    let draft_alpha = repo.vizier_output(&["draft", "--name", "alpha", "Alpha spec line"])?;
    assert!(
        draft_alpha.status.success(),
        "vizier draft alpha failed: {}",
        String::from_utf8_lossy(&draft_alpha.stderr)
    );
    let list = repo.vizier_output(&["list"])?;
    assert!(
        list.status.success(),
        "vizier list failed: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    let stdout = String::from_utf8_lossy(&list.stdout);
    let header = stdout
        .lines()
        .find(|line| line.contains("Plan") && line.contains("Summary"))
        .unwrap_or("");
    assert!(
        !header.is_empty(),
        "expected table header with Plan and Summary: {stdout}"
    );
    assert!(
        stdout.contains("alpha"),
        "expected plan slug in table output: {stdout}"
    );
    assert!(
        !stdout.contains("Branch"),
        "table output should omit Branch when entry_fields excludes it: {stdout}"
    );
    Ok(())
}

#[test]
fn test_list_fields_and_format_overrides() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let draft_alpha = repo.vizier_output(&["draft", "--name", "alpha", "Alpha spec line"])?;
    assert!(
        draft_alpha.status.success(),
        "vizier draft alpha failed: {}",
        String::from_utf8_lossy(&draft_alpha.stderr)
    );

    let list = repo.vizier_output(&["list", "--format", "json", "--fields", "Plan,Summary"])?;
    assert!(
        list.status.success(),
        "vizier list --format json failed: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    let json: Value = serde_json::from_slice(&list.stdout)?;
    let entries = json
        .get("entries")
        .and_then(|value| value.as_array())
        .ok_or("expected entries array in JSON output")?;
    let entry = entries.first().ok_or("expected at least one entry")?;
    assert!(entry.get("plan").is_some(), "expected plan field: {json}");
    assert!(
        entry.get("summary").is_some(),
        "expected summary field: {json}"
    );
    assert!(
        entry.get("branch").is_none(),
        "branch should be omitted when fields override: {json}"
    );
    Ok(())
}
