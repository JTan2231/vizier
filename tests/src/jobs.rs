use crate::fixtures::*;

#[test]
fn test_jobs_tail_follow_uses_global_flag() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo
        .vizier_cmd_background()
        .args(["ask", "jobs tail follow"])
        .output()?;
    assert!(
        output.status.success(),
        "background ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let job_id = extract_job_id(&stdout).ok_or("expected job id in output")?;
    wait_for_job_completion(&repo, &job_id, Duration::from_secs(20))?;

    let output = repo
        .vizier_cmd_background()
        .args(["jobs", "tail", "--follow", &job_id])
        .output()?;
    assert!(
        output.status.success(),
        "vizier jobs tail --follow failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[stdout]") || stdout.contains("mock agent response"),
        "expected jobs tail output to include stdout log content:\n{stdout}"
    );
    Ok(())
}

#[test]
fn test_jobs_list_hides_succeeded_by_default() -> TestResult {
    let repo = IntegrationRepo::new()?;
    write_job_record_simple(
        &repo,
        "job-running",
        "running",
        "2026-01-30T02:00:00Z",
        None,
        &["vizier", "ask", "running"],
    )?;
    write_job_record_simple(
        &repo,
        "job-failed",
        "failed",
        "2026-01-30T03:00:00Z",
        Some("2026-01-30T03:30:00Z"),
        &["vizier", "ask", "failed"],
    )?;
    write_job_record_simple(
        &repo,
        "job-succeeded",
        "succeeded",
        "2026-01-29T23:00:00Z",
        Some("2026-01-29T23:15:00Z"),
        &["vizier", "ask", "succeeded"],
    )?;

    let output = repo.vizier_output(&["jobs", "list"])?;
    assert!(
        output.status.success(),
        "vizier jobs list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("job-running"),
        "expected running job listed:\n{stdout}"
    );
    assert!(
        stdout.contains("job-failed"),
        "expected failed job listed:\n{stdout}"
    );
    assert!(
        !stdout.contains("job-succeeded"),
        "succeeded jobs should be hidden by default:\n{stdout}"
    );
    assert!(
        stdout.contains("Hidden : 1 succeeded (use --all to include)"),
        "expected hidden succeeded hint:\n{stdout}"
    );
    assert!(
        stdout.contains("Created: 2026-01-30T02:00:00"),
        "expected created timestamp for running job:\n{stdout}"
    );
    assert!(
        stdout.contains("Failed : 2026-01-30T03:30:00"),
        "expected failed timestamp for failed job:\n{stdout}"
    );
    assert_eq!(
        stdout.matches("Failed :").count(),
        1,
        "failed timestamp should appear once:\n{stdout}"
    );
    assert_eq!(
        stdout.matches("Created:").count(),
        2,
        "created timestamp should appear for each listed job:\n{stdout}"
    );
    Ok(())
}

#[test]
fn test_jobs_list_all_includes_succeeded() -> TestResult {
    let repo = IntegrationRepo::new()?;
    write_job_record_simple(
        &repo,
        "job-running",
        "running",
        "2026-01-30T02:00:00Z",
        None,
        &["vizier", "ask", "running"],
    )?;
    write_job_record_simple(
        &repo,
        "job-failed",
        "failed",
        "2026-01-30T03:00:00Z",
        Some("2026-01-30T03:30:00Z"),
        &["vizier", "ask", "failed"],
    )?;
    write_job_record_simple(
        &repo,
        "job-succeeded",
        "succeeded",
        "2026-01-29T23:00:00Z",
        Some("2026-01-29T23:15:00Z"),
        &["vizier", "ask", "succeeded"],
    )?;

    let output = repo.vizier_output(&["jobs", "list", "--all"])?;
    assert!(
        output.status.success(),
        "vizier jobs list --all failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("job-succeeded"),
        "expected succeeded job listed with --all:\n{stdout}"
    );
    assert_eq!(
        stdout.matches("Created:").count(),
        3,
        "created timestamp should appear for each listed job:\n{stdout}"
    );
    Ok(())
}

#[test]
fn test_jobs_cancel_without_cleanup_preserves_worktree() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-cancel-no-cleanup";
    let worktree_path = repo
        .path()
        .join(".vizier/tmp-worktrees")
        .join("cancel-cleanup");
    fs::create_dir_all(&worktree_path)?;
    let pid = spawn_detached_sleep(60)?;
    let record = json!({
        "id": job_id,
        "status": "running",
        "command": ["vizier", "draft"],
        "created_at": "2026-01-31T00:00:00Z",
        "started_at": "2026-01-31T00:00:01Z",
        "finished_at": null,
        "pid": pid,
        "exit_code": null,
        "stdout_path": "stdout.log",
        "stderr_path": "stderr.log",
        "session_path": null,
        "outcome_path": null,
        "metadata": {
            "worktree_path": ".vizier/tmp-worktrees/cancel-cleanup",
            "worktree_owned": true
        },
        "config_snapshot": null
    });
    write_job_record(&repo, job_id, record)?;

    let cancel = repo
        .vizier_cmd()
        .args(["jobs", "cancel", job_id])
        .output()?;
    let cancel_ok = cancel.status.success();
    terminate_pid(pid);
    assert!(
        cancel_ok,
        "vizier jobs cancel failed: {}",
        String::from_utf8_lossy(&cancel.stderr)
    );
    let cancel_stdout = String::from_utf8_lossy(&cancel.stdout);
    assert!(
        cancel_stdout.contains("cleanup=skipped"),
        "expected cancel output to note cleanup skipped:\n{cancel_stdout}"
    );
    assert!(
        worktree_path.exists(),
        "expected worktree to remain after cancel without cleanup: {}",
        worktree_path.display()
    );

    let record = read_job_record(&repo, job_id)?;
    let cleanup_status = record
        .get("metadata")
        .and_then(|meta| meta.get("cancel_cleanup_status"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert_eq!(
        cleanup_status, "skipped",
        "expected cancel cleanup status to be skipped"
    );
    Ok(())
}

#[test]
fn test_jobs_cancel_with_cleanup_removes_worktree() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-cancel-cleanup";
    let worktree_path = repo
        .path()
        .join(".vizier/tmp-worktrees")
        .join("cancel-cleanup");
    fs::create_dir_all(&worktree_path)?;
    let pid = spawn_detached_sleep(60)?;
    let record = json!({
        "id": job_id,
        "status": "running",
        "command": ["vizier", "draft"],
        "created_at": "2026-01-31T00:00:00Z",
        "started_at": "2026-01-31T00:00:01Z",
        "finished_at": null,
        "pid": pid,
        "exit_code": null,
        "stdout_path": "stdout.log",
        "stderr_path": "stderr.log",
        "session_path": null,
        "outcome_path": null,
        "metadata": {
            "worktree_path": ".vizier/tmp-worktrees/cancel-cleanup",
            "worktree_owned": true
        },
        "config_snapshot": null
    });
    write_job_record(&repo, job_id, record)?;

    let cancel = repo
        .vizier_cmd()
        .args(["jobs", "cancel", "--cleanup-worktree", job_id])
        .output()?;
    let cancel_ok = cancel.status.success();
    terminate_pid(pid);
    assert!(
        cancel_ok,
        "vizier jobs cancel --cleanup-worktree failed: {}",
        String::from_utf8_lossy(&cancel.stderr)
    );
    let cancel_stdout = String::from_utf8_lossy(&cancel.stdout);
    assert!(
        cancel_stdout.contains("cleanup=done"),
        "expected cancel output to note cleanup done:\n{cancel_stdout}"
    );
    assert!(
        !worktree_path.exists(),
        "expected worktree to be removed after cleanup: {}",
        worktree_path.display()
    );

    let record = read_job_record(&repo, job_id)?;
    let cleanup_status = record
        .get("metadata")
        .and_then(|meta| meta.get("cancel_cleanup_status"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert_eq!(
        cleanup_status, "done",
        "expected cancel cleanup status to be done"
    );
    Ok(())
}

#[test]
fn test_job_failure_does_not_run_cancel_cleanup() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-failed-cleanup";
    let worktree_path = repo
        .path()
        .join(".vizier/tmp-worktrees")
        .join("failed-cleanup");
    fs::create_dir_all(&worktree_path)?;
    let config_path = repo.path().join(".vizier/tmp/cancel-cleanup.toml");
    fs::create_dir_all(config_path.parent().unwrap())?;
    fs::write(
        &config_path,
        r#"
[jobs.cancel]
cleanup_worktree = true
"#,
    )?;
    let record = json!({
        "id": job_id,
        "status": "failed",
        "command": ["vizier", "approve"],
        "created_at": "2026-01-31T00:00:00Z",
        "started_at": "2026-01-31T00:00:01Z",
        "finished_at": "2026-01-31T00:00:03Z",
        "pid": null,
        "exit_code": 1,
        "stdout_path": "stdout.log",
        "stderr_path": "stderr.log",
        "session_path": null,
        "outcome_path": null,
        "metadata": {
            "worktree_path": ".vizier/tmp-worktrees/failed-cleanup",
            "worktree_owned": true
        },
        "config_snapshot": null
    });
    write_job_record(&repo, job_id, record)?;

    let output = repo
        .vizier_cmd()
        .args([
            "--config-file",
            config_path.to_str().unwrap(),
            "jobs",
            "cancel",
            "--cleanup-worktree",
            job_id,
        ])
        .output()?;
    assert!(
        !output.status.success(),
        "expected cancel on failed job to exit non-zero"
    );

    let record = read_job_record(&repo, job_id)?;
    let cleanup_field = record
        .get("metadata")
        .and_then(|meta| meta.get("cancel_cleanup_status"));
    assert!(
        cleanup_field.is_none() || cleanup_field == Some(&Value::Null),
        "expected cancel cleanup status to be absent on failure"
    );
    assert!(
        worktree_path.exists(),
        "expected failed job worktree to remain: {}",
        worktree_path.display()
    );
    Ok(())
}
