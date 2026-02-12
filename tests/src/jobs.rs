use crate::fixtures::*;
use std::thread;

fn schedule_record(job_id: &str, status: &str, created_at: &str, schedule: Value) -> Value {
    json!({
        "id": job_id,
        "status": status,
        "command": ["vizier", "save", "schedule"],
        "created_at": created_at,
        "started_at": created_at,
        "finished_at": null,
        "pid": null,
        "exit_code": null,
        "stdout_path": format!(".vizier/jobs/{job_id}/stdout.log"),
        "stderr_path": format!(".vizier/jobs/{job_id}/stderr.log"),
        "session_path": null,
        "outcome_path": null,
        "metadata": null,
        "config_snapshot": null,
        "schedule": schedule
    })
}

fn schedule_summary_rows(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed
                .chars()
                .next()
                .map(|ch| ch.is_ascii_digit())
                .unwrap_or(false)
        })
        .collect()
}

#[test]
fn test_jobs_tail_follow_uses_global_flag() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo.vizier_cmd_background().args(["save"]).output()?;
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
fn test_background_save_job_dual_writes_scope_alias_and_template_selector() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo.vizier_cmd_background().args(["save"]).output()?;
    assert!(
        output.status.success(),
        "background save failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let job_id = extract_job_id(&stdout).ok_or("expected job id in background save output")?;
    wait_for_job_completion(&repo, &job_id, Duration::from_secs(20))?;

    let record = read_job_record(&repo, &job_id)?;
    assert_eq!(
        record.pointer("/metadata/scope").and_then(Value::as_str),
        Some("save"),
        "save metadata should continue dual-writing legacy scope"
    );
    assert_eq!(
        record
            .pointer("/metadata/command_alias")
            .and_then(Value::as_str),
        Some("save"),
        "save metadata should write command_alias"
    );
    assert_eq!(
        record
            .pointer("/metadata/workflow_template_selector")
            .and_then(Value::as_str),
        Some("template.save@v1"),
        "save metadata should write resolved workflow_template_selector"
    );

    Ok(())
}

#[test]
fn test_jobs_status_output() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-status";
    write_job_record(
        &repo,
        job_id,
        json!({
            "id": job_id,
            "status": "failed",
            "command": ["vizier", "save", "status"],
            "created_at": "2026-01-30T02:00:00Z",
            "started_at": "2026-01-30T02:00:01Z",
            "finished_at": "2026-01-30T02:00:02Z",
            "pid": 1234,
            "exit_code": 1,
            "stdout_path": format!(".vizier/jobs/{job_id}/stdout.log"),
            "stderr_path": format!(".vizier/jobs/{job_id}/stderr.log"),
            "session_path": null,
            "outcome_path": null,
            "metadata": null,
            "config_snapshot": null
        }),
    )?;

    let output = repo.vizier_output(&["jobs", "status", job_id])?;
    assert!(
        output.status.success(),
        "vizier jobs status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(job_id),
        "status output missing job id:\n{stdout}"
    );
    assert!(
        stdout.contains("[failed]"),
        "status output missing status label:\n{stdout}"
    );
    assert!(
        stdout.contains("exit=1"),
        "status output missing exit:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!("stdout=.vizier/jobs/{job_id}/stdout.log")),
        "status output missing stdout path:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!("stderr=.vizier/jobs/{job_id}/stderr.log")),
        "status output missing stderr path:\n{stdout}"
    );

    let output = repo
        .vizier_cmd_background()
        .args(["--json", "jobs", "status", job_id])
        .output()?;
    assert!(
        output.status.success(),
        "vizier --json jobs status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        json.get("job").and_then(Value::as_str),
        Some(job_id),
        "job id mismatch in JSON status: {json}"
    );
    assert_eq!(
        json.get("status").and_then(Value::as_str),
        Some("failed"),
        "status mismatch in JSON status: {json}"
    );
    assert_eq!(
        json.get("exit_code").and_then(Value::as_i64),
        Some(1),
        "exit_code mismatch in JSON status: {json}"
    );
    let expected_stdout = format!(".vizier/jobs/{job_id}/stdout.log");
    let expected_stderr = format!(".vizier/jobs/{job_id}/stderr.log");
    assert_eq!(
        json.get("stdout").and_then(Value::as_str),
        Some(expected_stdout.as_str()),
        "stdout mismatch in JSON status: {json}"
    );
    assert_eq!(
        json.get("stderr").and_then(Value::as_str),
        Some(expected_stderr.as_str()),
        "stderr mismatch in JSON status: {json}"
    );
    Ok(())
}

#[test]
fn test_jobs_approve_advances_waiting_approval_job() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-approve-gate";
    write_job_record(
        &repo,
        job_id,
        json!({
            "id": job_id,
            "status": "waiting_on_approval",
            "command": ["vizier", "save", "approval"],
            "child_args": ["--help"],
            "created_at": "2026-02-07T12:00:00Z",
            "started_at": null,
            "finished_at": null,
            "pid": null,
            "exit_code": null,
            "stdout_path": format!(".vizier/jobs/{job_id}/stdout.log"),
            "stderr_path": format!(".vizier/jobs/{job_id}/stderr.log"),
            "session_path": null,
            "outcome_path": null,
            "metadata": { "scope": "approve", "plan": "approval-plan" },
            "config_snapshot": null,
            "schedule": {
                "dependencies": [
                    { "artifact": { "target_branch": { "name": "missing-approval-target" } } }
                ],
                "approval": {
                    "required": true,
                    "state": "pending",
                    "requested_at": "2026-02-07T12:00:00Z",
                    "requested_by": "tester"
                }
            }
        }),
    )?;

    let output = repo.vizier_output(&["jobs", "approve", job_id])?;
    assert!(
        output.status.success(),
        "vizier jobs approve failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Job approval granted"),
        "expected approval outcome block:\n{stdout}"
    );

    let record = read_job_record(&repo, job_id)?;
    let approval_state = record
        .pointer("/schedule/approval/state")
        .and_then(Value::as_str);
    assert_eq!(
        approval_state,
        Some("approved"),
        "expected approval state to be approved: {record}"
    );
    assert_eq!(
        record.get("status").and_then(Value::as_str),
        Some("blocked_by_dependency"),
        "expected approved job to move past approval and settle on dependency status: {record}"
    );
    Ok(())
}

#[test]
fn test_jobs_reject_marks_blocked_by_approval_and_records_reason() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-reject-gate";
    write_job_record(
        &repo,
        job_id,
        json!({
            "id": job_id,
            "status": "waiting_on_approval",
            "command": ["vizier", "approve", "reject-plan"],
            "child_args": ["approve", "reject-plan", "--yes"],
            "created_at": "2026-02-07T12:30:00Z",
            "started_at": null,
            "finished_at": null,
            "pid": null,
            "exit_code": null,
            "stdout_path": format!(".vizier/jobs/{job_id}/stdout.log"),
            "stderr_path": format!(".vizier/jobs/{job_id}/stderr.log"),
            "session_path": null,
            "outcome_path": null,
            "metadata": { "scope": "approve", "plan": "reject-plan" },
            "config_snapshot": null,
            "schedule": {
                "approval": {
                    "required": true,
                    "state": "pending",
                    "requested_at": "2026-02-07T12:30:00Z",
                    "requested_by": "tester"
                }
            }
        }),
    )?;

    let reason = "needs architecture sign-off";
    let output = repo.vizier_output(&["jobs", "reject", job_id, "--reason", reason])?;
    assert!(
        output.status.success(),
        "vizier jobs reject failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Job approval rejected"),
        "expected reject outcome block:\n{stdout}"
    );

    let record = read_job_record(&repo, job_id)?;
    assert_eq!(
        record.get("status").and_then(Value::as_str),
        Some("blocked_by_approval"),
        "expected blocked_by_approval status: {record}"
    );
    assert_eq!(
        record
            .pointer("/schedule/approval/state")
            .and_then(Value::as_str),
        Some("rejected"),
        "expected rejected approval state: {record}"
    );
    assert_eq!(
        record
            .pointer("/schedule/approval/reason")
            .and_then(Value::as_str),
        Some(reason),
        "expected rejection reason to be recorded: {record}"
    );
    assert_eq!(
        record
            .pointer("/schedule/wait_reason/kind")
            .and_then(Value::as_str),
        Some("approval"),
        "expected approval wait reason: {record}"
    );
    assert!(
        record
            .pointer("/schedule/waited_on")
            .and_then(Value::as_array)
            .map(|values| values
                .iter()
                .any(|value| value.as_str() == Some("approval")))
            .unwrap_or(false),
        "expected waited_on to include approval: {record}"
    );
    let outcome_path = repo
        .path()
        .join(".vizier/jobs")
        .join(job_id)
        .join("outcome.json");
    assert!(
        outcome_path.exists(),
        "expected reject flow to write outcome.json"
    );
    Ok(())
}

#[test]
fn test_jobs_tail_follow_orders_streams() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-tail-order";
    write_job_record_simple(
        &repo,
        job_id,
        "running",
        "2026-01-30T02:00:00Z",
        None,
        &["vizier", "save", "follow-order"],
    )?;
    let job_dir = repo.path().join(".vizier/jobs").join(job_id);
    let stdout_path = job_dir.join("stdout.log");
    let stderr_path = job_dir.join("stderr.log");

    {
        let mut stdout_log = fs::OpenOptions::new().append(true).open(&stdout_path)?;
        writeln!(stdout_log, "start-out")?;
        let mut stderr_log = fs::OpenOptions::new().append(true).open(&stderr_path)?;
        writeln!(stderr_log, "start-err")?;
    }

    let mut cmd = repo.vizier_cmd_background();
    cmd.args(["jobs", "tail", "--follow", job_id]);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let child = cmd.spawn()?;

    thread::sleep(Duration::from_millis(200));
    {
        let mut stdout_log = fs::OpenOptions::new().append(true).open(&stdout_path)?;
        writeln!(stdout_log, "next-out")?;
        let mut stderr_log = fs::OpenOptions::new().append(true).open(&stderr_path)?;
        writeln!(stderr_log, "next-err")?;
    }
    thread::sleep(Duration::from_millis(200));
    update_job_record(&repo, job_id, |record| {
        record["status"] = Value::String("succeeded".to_string());
        record["finished_at"] = Value::String("2026-01-30T02:00:03Z".to_string());
        record["exit_code"] = Value::from(0);
    })?;

    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "vizier jobs tail --follow failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let start_out = stdout
        .find("[stdout] start-out")
        .ok_or("missing start stdout line")?;
    let start_err = stdout
        .find("[stderr] start-err")
        .ok_or("missing start stderr line")?;
    let next_out = stdout
        .find("[stdout] next-out")
        .ok_or("missing next stdout line")?;
    let next_err = stdout
        .find("[stderr] next-err")
        .ok_or("missing next stderr line")?;
    assert!(
        start_out < next_out && start_err < next_err,
        "unexpected per-stream log ordering:\n{stdout}"
    );
    Ok(())
}

#[test]
fn test_jobs_attach_streams_both_logs() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-attach";
    write_job_record_simple(
        &repo,
        job_id,
        "running",
        "2026-01-30T02:00:00Z",
        None,
        &["vizier", "save", "attach"],
    )?;
    let job_dir = repo.path().join(".vizier/jobs").join(job_id);
    let stdout_path = job_dir.join("stdout.log");
    let stderr_path = job_dir.join("stderr.log");

    let mut cmd = repo.vizier_cmd_background();
    cmd.args(["jobs", "attach", job_id]);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let child = cmd.spawn()?;

    thread::sleep(Duration::from_millis(200));
    {
        let mut stdout_log = fs::OpenOptions::new().append(true).open(&stdout_path)?;
        writeln!(stdout_log, "attach-out")?;
        let mut stderr_log = fs::OpenOptions::new().append(true).open(&stderr_path)?;
        writeln!(stderr_log, "attach-err")?;
    }
    thread::sleep(Duration::from_millis(200));
    update_job_record(&repo, job_id, |record| {
        record["status"] = Value::String("succeeded".to_string());
        record["finished_at"] = Value::String("2026-01-30T02:00:02Z".to_string());
        record["exit_code"] = Value::from(0);
    })?;

    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "vizier jobs attach failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[stdout] attach-out"),
        "attach output missing stdout line:\n{stdout}"
    );
    assert!(
        stdout.contains("[stderr] attach-err"),
        "attach output missing stderr line:\n{stdout}"
    );
    Ok(())
}

#[test]
fn test_jobs_tail_handles_missing_logs() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-missing-logs";
    write_job_record_simple(
        &repo,
        job_id,
        "succeeded",
        "2026-01-30T02:00:00Z",
        Some("2026-01-30T02:00:01Z"),
        &["vizier", "save", "missing-logs"],
    )?;
    let job_dir = repo.path().join(".vizier/jobs").join(job_id);
    let _ = fs::remove_file(job_dir.join("stdout.log"));
    let _ = fs::remove_file(job_dir.join("stderr.log"));

    let output = repo
        .vizier_cmd_background()
        .args(["jobs", "tail", job_id])
        .output()?;
    assert!(
        output.status.success(),
        "vizier jobs tail failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "expected no stdout when logs are missing"
    );
    Ok(())
}

#[test]
fn test_jobs_status_missing_job_returns_error() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let missing_dir = repo.path().join(".vizier/jobs").join("missing-job");
    fs::create_dir_all(&missing_dir)?;

    let output = repo
        .vizier_cmd_background()
        .args(["jobs", "status", "missing-job"])
        .output()?;
    assert!(
        !output.status.success(),
        "expected jobs status for missing job to fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no background job missing-job"),
        "missing job error not reported:\n{stderr}"
    );
    Ok(())
}

#[test]
fn test_jobs_list_skips_malformed_records() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let bad_dir = repo.path().join(".vizier/jobs").join("bad-job");
    fs::create_dir_all(&bad_dir)?;
    fs::write(bad_dir.join("job.json"), "not-json")?;

    let output = repo.vizier_output(&["jobs", "list"])?;
    assert!(
        output.status.success(),
        "vizier jobs list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Outcome: No background jobs found"),
        "expected empty list outcome with malformed records:\n{stdout}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unable to load background job bad-job"),
        "expected warning for malformed job record:\n{stderr}"
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
        &["vizier", "save", "running"],
    )?;
    write_job_record_simple(
        &repo,
        "job-failed",
        "failed",
        "2026-01-30T03:00:00Z",
        Some("2026-01-30T03:30:00Z"),
        &["vizier", "save", "failed"],
    )?;
    write_job_record_simple(
        &repo,
        "job-succeeded",
        "succeeded",
        "2026-01-29T23:00:00Z",
        Some("2026-01-29T23:15:00Z"),
        &["vizier", "save", "succeeded"],
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
fn test_jobs_list_dismiss_failures_hides_failed() -> TestResult {
    let repo = IntegrationRepo::new()?;
    write_job_record_simple(
        &repo,
        "job-running",
        "running",
        "2026-01-30T02:00:00Z",
        None,
        &["vizier", "save", "running"],
    )?;
    write_job_record_simple(
        &repo,
        "job-failed",
        "failed",
        "2026-01-30T03:00:00Z",
        Some("2026-01-30T03:30:00Z"),
        &["vizier", "save", "failed"],
    )?;
    write_job_record_simple(
        &repo,
        "job-succeeded",
        "succeeded",
        "2026-01-29T23:00:00Z",
        Some("2026-01-29T23:15:00Z"),
        &["vizier", "save", "succeeded"],
    )?;

    let output = repo.vizier_output(&["jobs", "list", "--dismiss-failures"])?;
    assert!(
        output.status.success(),
        "vizier jobs list --dismiss-failures failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("job-running"),
        "expected running job listed:\n{stdout}"
    );
    assert!(
        !stdout.contains("job-failed"),
        "failed jobs should be hidden when dismissed:\n{stdout}"
    );
    assert!(
        !stdout.contains("job-succeeded"),
        "succeeded jobs should still be hidden by default:\n{stdout}"
    );
    assert!(
        stdout.contains("Hidden : 1 failed, 1 succeeded (use --all to include)"),
        "expected hidden failed/succeeded hint:\n{stdout}"
    );
    assert_eq!(
        stdout.matches("Failed :").count(),
        0,
        "failed timestamp should be hidden with dismiss-failures:\n{stdout}"
    );
    assert_eq!(
        stdout.matches("Created:").count(),
        1,
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
        &["vizier", "save", "running"],
    )?;
    write_job_record_simple(
        &repo,
        "job-failed",
        "failed",
        "2026-01-30T03:00:00Z",
        Some("2026-01-30T03:30:00Z"),
        &["vizier", "save", "failed"],
    )?;
    write_job_record_simple(
        &repo,
        "job-succeeded",
        "succeeded",
        "2026-01-29T23:00:00Z",
        Some("2026-01-29T23:15:00Z"),
        &["vizier", "save", "succeeded"],
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
fn test_jobs_list_all_overrides_dismiss_failures() -> TestResult {
    let repo = IntegrationRepo::new()?;
    write_job_record_simple(
        &repo,
        "job-running",
        "running",
        "2026-01-30T02:00:00Z",
        None,
        &["vizier", "save", "running"],
    )?;
    write_job_record_simple(
        &repo,
        "job-failed",
        "failed",
        "2026-01-30T03:00:00Z",
        Some("2026-01-30T03:30:00Z"),
        &["vizier", "save", "failed"],
    )?;
    write_job_record_simple(
        &repo,
        "job-succeeded",
        "succeeded",
        "2026-01-29T23:00:00Z",
        Some("2026-01-29T23:15:00Z"),
        &["vizier", "save", "succeeded"],
    )?;

    let output = repo.vizier_output(&["jobs", "list", "--dismiss-failures", "--all"])?;
    assert!(
        output.status.success(),
        "vizier jobs list --dismiss-failures --all failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("job-failed"),
        "expected failed job listed with --all override:\n{stdout}"
    );
    assert!(
        stdout.contains("job-succeeded"),
        "expected succeeded job listed with --all override:\n{stdout}"
    );
    assert!(
        !stdout.contains("Hidden :"),
        "expected no hidden summary with --all override:\n{stdout}"
    );
    assert_eq!(
        stdout.matches("Created:").count(),
        3,
        "created timestamp should appear for each listed job:\n{stdout}"
    );
    Ok(())
}

#[test]
fn test_jobs_schedule_empty_state() -> TestResult {
    let repo = IntegrationRepo::new()?;

    let output = repo
        .vizier_cmd_background()
        .args(["jobs", "schedule"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier jobs schedule failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Outcome: No scheduled jobs"),
        "expected empty schedule outcome:\n{stdout}"
    );

    let output = repo
        .vizier_cmd_background()
        .args(["jobs", "schedule", "--format", "dag"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier jobs schedule --format dag failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Outcome: No scheduled jobs"),
        "expected empty DAG schedule outcome:\n{stdout}"
    );

    let output = repo
        .vizier_cmd_background()
        .args(["jobs", "schedule", "--format", "json"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier jobs schedule --format json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        payload.get("version").and_then(Value::as_u64),
        Some(1),
        "expected JSON schedule version=1: {payload}"
    );
    assert_eq!(
        payload.get("ordering").and_then(Value::as_str),
        Some("created_at_then_job_id"),
        "expected JSON schedule ordering key: {payload}"
    );
    assert_eq!(
        payload
            .get("jobs")
            .and_then(Value::as_array)
            .map(|jobs| jobs.len()),
        Some(0),
        "expected empty jobs array: {payload}"
    );
    assert_eq!(
        payload
            .get("edges")
            .and_then(Value::as_array)
            .map(|edges| edges.len()),
        Some(0),
        "expected empty edges array: {payload}"
    );
    Ok(())
}

#[test]
fn test_jobs_schedule_dag_and_json_output() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.git(&["branch", "present-branch"])?;

    let artifact = json!({ "ask_save_patch": { "job_id": "producer-artifact" } });
    let producer_schedule = json!({
        "artifacts": [artifact.clone()]
    });
    let consumer_schedule = json!({
        "dependencies": [
            { "artifact": artifact.clone() },
            { "artifact": { "target_branch": { "name": "present-branch" } } },
            { "artifact": { "target_branch": { "name": "missing-branch" } } }
        ]
    });

    write_job_record(
        &repo,
        "job-producer",
        schedule_record(
            "job-producer",
            "running",
            "2026-02-01T00:00:00Z",
            producer_schedule,
        ),
    )?;
    write_job_record(
        &repo,
        "job-consumer",
        schedule_record(
            "job-consumer",
            "queued",
            "2026-02-01T01:00:00Z",
            consumer_schedule,
        ),
    )?;

    let output = repo.vizier_output(&["jobs", "schedule"])?;
    assert!(
        output.status.success(),
        "vizier jobs schedule failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Schedule (Summary)"),
        "expected summary header:\n{stdout}"
    );
    assert!(
        stdout.contains("Slug") && stdout.contains("Name") && stdout.contains("Status"),
        "expected summary columns:\n{stdout}"
    );
    assert_eq!(
        stdout.matches("job-producer").count(),
        1,
        "expected exactly one producer summary row:\n{stdout}"
    );
    assert_eq!(
        stdout.matches("job-consumer").count(),
        1,
        "expected exactly one consumer summary row:\n{stdout}"
    );

    let summary_rows = schedule_summary_rows(&stdout);
    let producer_index = summary_rows
        .iter()
        .position(|line| line.contains("job-producer"))
        .ok_or("missing producer summary row")?;
    let consumer_index = summary_rows
        .iter()
        .position(|line| line.contains("job-consumer"))
        .ok_or("missing consumer summary row")?;
    assert!(
        producer_index < consumer_index,
        "expected created_at ordering in summary rows:\n{stdout}"
    );

    let output = repo.vizier_output(&["jobs", "schedule", "--format", "dag"])?;
    assert!(
        output.status.success(),
        "vizier jobs schedule --format dag failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("command_patch:producer-artifact -> job-producer running"),
        "expected producer edge in DAG output:\n{stdout}"
    );
    assert!(
        stdout.contains("target_branch:present-branch -> [present]"),
        "expected present artifact state in DAG output:\n{stdout}"
    );
    assert!(
        stdout.contains("target_branch:missing-branch -> [missing]"),
        "expected missing artifact state in DAG output:\n{stdout}"
    );

    let output = repo
        .vizier_cmd_background()
        .args(["jobs", "schedule", "--format", "json"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier jobs schedule --format json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        payload.get("version").and_then(Value::as_u64),
        Some(1),
        "expected JSON schedule version=1: {payload}"
    );
    assert_eq!(
        payload.get("ordering").and_then(Value::as_str),
        Some("created_at_then_job_id"),
        "expected JSON ordering value: {payload}"
    );

    let jobs = payload
        .get("jobs")
        .and_then(Value::as_array)
        .ok_or("expected jobs array")?;
    assert!(
        jobs.iter()
            .any(|job| job.get("job_id") == Some(&Value::String("job-consumer".to_string()))),
        "expected job-consumer job entry: {payload}"
    );
    assert!(
        jobs.iter()
            .any(|job| job.get("job_id") == Some(&Value::String("job-producer".to_string()))),
        "expected job-producer job entry: {payload}"
    );
    for job in jobs {
        assert!(
            job.get("order").and_then(Value::as_u64).is_some(),
            "missing order in schedule JSON job entry: {job}"
        );
        assert!(
            job.get("job_id").and_then(Value::as_str).is_some(),
            "missing job_id in schedule JSON job entry: {job}"
        );
        assert!(
            job.get("name").and_then(Value::as_str).is_some(),
            "missing name in schedule JSON job entry: {job}"
        );
        assert!(
            job.get("status").and_then(Value::as_str).is_some(),
            "missing status in schedule JSON job entry: {job}"
        );
        assert!(
            job.get("created_at").and_then(Value::as_str).is_some(),
            "missing created_at in schedule JSON job entry: {job}"
        );
        assert!(
            job.get("slug").is_some(),
            "missing slug key in schedule JSON job entry: {job}"
        );
        assert!(
            job.get("wait").is_some(),
            "missing wait key in schedule JSON job entry: {job}"
        );
    }

    let edges = payload
        .get("edges")
        .and_then(Value::as_array)
        .ok_or("expected edges array")?;
    assert!(
        edges.iter().any(|edge| {
            edge.get("from") == Some(&Value::String("job-consumer".to_string()))
                && edge.get("to") == Some(&Value::String("job-producer".to_string()))
                && edge.get("artifact")
                    == Some(&Value::String(
                        "command_patch:producer-artifact".to_string(),
                    ))
        }),
        "expected producer edge in JSON: {payload}"
    );
    assert!(
        edges.iter().any(|edge| {
            edge.get("to")
                == Some(&Value::String(
                    "artifact:target_branch:present-branch".to_string(),
                ))
                && edge.get("state") == Some(&Value::String("present".to_string()))
        }),
        "expected present artifact edge in JSON: {payload}"
    );
    assert!(
        edges.iter().any(|edge| {
            edge.get("to")
                == Some(&Value::String(
                    "artifact:target_branch:missing-branch".to_string(),
                ))
                && edge.get("state") == Some(&Value::String("missing".to_string()))
        }),
        "expected missing artifact edge in JSON: {payload}"
    );
    Ok(())
}

#[test]
fn test_jobs_schedule_includes_after_edges() -> TestResult {
    let repo = IntegrationRepo::new()?;

    write_job_record(
        &repo,
        "job-predecessor",
        schedule_record(
            "job-predecessor",
            "succeeded",
            "2026-02-01T00:00:00Z",
            json!({}),
        ),
    )?;
    write_job_record(
        &repo,
        "job-dependent",
        schedule_record(
            "job-dependent",
            "queued",
            "2026-02-01T01:00:00Z",
            json!({
                "after": [
                    { "job_id": "job-predecessor", "policy": "success" }
                ]
            }),
        ),
    )?;

    let output = repo.vizier_output(&["jobs", "schedule"])?;
    assert!(
        output.status.success(),
        "vizier jobs schedule failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Schedule (Summary)"),
        "expected summary header:\n{stdout}"
    );
    assert!(
        stdout.contains("job-dependent"),
        "expected dependent row in summary output:\n{stdout}"
    );

    let output = repo.vizier_output(&["jobs", "schedule", "--format", "dag"])?;
    assert!(
        output.status.success(),
        "vizier jobs schedule --format dag failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("after:success -> job-predecessor succeeded"),
        "expected after edge in DAG output:\n{stdout}"
    );

    let output = repo.vizier_output(&["jobs", "schedule", "--format", "json"])?;
    assert!(
        output.status.success(),
        "vizier jobs schedule --format json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout)?;
    let edges = payload
        .get("edges")
        .and_then(Value::as_array)
        .ok_or("expected edges array")?;
    assert!(
        edges.iter().any(|edge| {
            edge.get("from") == Some(&Value::String("job-dependent".to_string()))
                && edge.get("to") == Some(&Value::String("job-predecessor".to_string()))
                && edge.get("after").and_then(|after| after.get("policy"))
                    == Some(&Value::String("success".to_string()))
        }),
        "expected after edge in JSON output: {payload}"
    );
    Ok(())
}

#[test]
fn test_jobs_schedule_filters_terminal_without_all() -> TestResult {
    let repo = IntegrationRepo::new()?;
    write_job_record_simple(
        &repo,
        "job-active",
        "queued",
        "2026-02-02T00:00:00Z",
        None,
        &["vizier", "save", "active"],
    )?;
    write_job_record_simple(
        &repo,
        "job-succeeded",
        "succeeded",
        "2026-02-02T00:10:00Z",
        Some("2026-02-02T00:11:00Z"),
        &["vizier", "save", "done"],
    )?;

    let output = repo.vizier_output(&["jobs", "schedule"])?;
    assert!(
        output.status.success(),
        "vizier jobs schedule failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("job-active"),
        "expected active job in schedule:\n{stdout}"
    );
    assert!(
        stdout.contains("queued"),
        "expected active status in schedule:\n{stdout}"
    );
    assert!(
        !stdout.contains("job-succeeded"),
        "did not expect succeeded job without --all:\n{stdout}"
    );

    let output = repo.vizier_output(&["jobs", "schedule", "--all"])?;
    assert!(
        output.status.success(),
        "vizier jobs schedule --all failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("job-succeeded"),
        "expected succeeded job with --all:\n{stdout}"
    );
    assert!(
        stdout.contains("succeeded"),
        "expected succeeded job with --all:\n{stdout}"
    );
    Ok(())
}

#[test]
fn test_jobs_schedule_summary_orders_by_created_at_then_job_id() -> TestResult {
    let repo = IntegrationRepo::new()?;
    write_job_record_simple(
        &repo,
        "job-b",
        "queued",
        "2026-02-02T00:00:01Z",
        None,
        &["vizier", "save", "job-b"],
    )?;
    write_job_record_simple(
        &repo,
        "job-a",
        "queued",
        "2026-02-02T00:00:01Z",
        None,
        &["vizier", "save", "job-a"],
    )?;
    write_job_record_simple(
        &repo,
        "job-early",
        "queued",
        "2026-02-02T00:00:00Z",
        None,
        &["vizier", "save", "job-early"],
    )?;

    let output = repo.vizier_output(&["jobs", "schedule"])?;
    assert!(
        output.status.success(),
        "vizier jobs schedule failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let rows = schedule_summary_rows(&stdout);
    assert!(
        rows.len() >= 3,
        "expected at least three summary rows:\n{stdout}"
    );
    assert!(
        rows[0].contains("job-early"),
        "expected earliest job first:\n{stdout}"
    );
    assert!(
        rows[1].contains("job-a"),
        "expected job-id tie-break for second row:\n{stdout}"
    );
    assert!(
        rows[2].contains("job-b"),
        "expected job-id tie-break for third row:\n{stdout}"
    );
    Ok(())
}

#[test]
fn test_jobs_schedule_job_focus_includes_neighbors() -> TestResult {
    let repo = IntegrationRepo::new()?;

    let artifact = json!({ "ask_save_patch": { "job_id": "root-artifact" } });
    let root_schedule = json!({ "artifacts": [artifact.clone()] });
    let consumer_schedule = json!({
        "dependencies": [
            { "artifact": artifact.clone() }
        ]
    });

    write_job_record(
        &repo,
        "job-root",
        schedule_record("job-root", "queued", "2026-02-03T00:00:00Z", root_schedule),
    )?;
    write_job_record(
        &repo,
        "job-consumer",
        schedule_record(
            "job-consumer",
            "queued",
            "2026-02-03T01:00:00Z",
            consumer_schedule,
        ),
    )?;
    write_job_record_simple(
        &repo,
        "job-unrelated",
        "queued",
        "2026-02-03T02:00:00Z",
        None,
        &["vizier", "save", "unrelated"],
    )?;

    let output = repo.vizier_output(&["jobs", "schedule", "--job", "job-root"])?;
    assert!(
        output.status.success(),
        "vizier jobs schedule --job failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("job-root"),
        "expected focus job in schedule:\n{stdout}"
    );
    assert!(
        stdout.contains("job-consumer"),
        "expected consumer in focused schedule:\n{stdout}"
    );
    assert!(
        !stdout.contains("job-unrelated"),
        "did not expect unrelated job in focused schedule:\n{stdout}"
    );
    let summary_rows = schedule_summary_rows(&stdout);
    let first_row = summary_rows.first().ok_or("missing focused summary row")?;
    assert!(
        first_row.contains("job-root"),
        "expected focused job pinned to summary row 1:\n{stdout}"
    );
    Ok(())
}

#[test]
fn test_jobs_schedule_max_depth_limits_expansion() -> TestResult {
    let repo = IntegrationRepo::new()?;

    let artifact_b = json!({ "ask_save_patch": { "job_id": "artifact-b" } });
    let artifact_c = json!({ "ask_save_patch": { "job_id": "artifact-c" } });

    let job_c_schedule = json!({ "artifacts": [artifact_c.clone()] });
    let job_b_schedule = json!({
        "dependencies": [ { "artifact": artifact_c.clone() } ],
        "artifacts": [artifact_b.clone()]
    });
    let job_a_schedule = json!({
        "dependencies": [ { "artifact": artifact_b.clone() } ]
    });

    write_job_record(
        &repo,
        "job-c",
        schedule_record("job-c", "succeeded", "2026-02-04T00:00:00Z", job_c_schedule),
    )?;
    write_job_record(
        &repo,
        "job-b",
        schedule_record("job-b", "succeeded", "2026-02-04T01:00:00Z", job_b_schedule),
    )?;
    write_job_record(
        &repo,
        "job-a",
        schedule_record("job-a", "queued", "2026-02-04T02:00:00Z", job_a_schedule),
    )?;

    let output =
        repo.vizier_output(&["jobs", "schedule", "--format", "dag", "--max-depth", "1"])?;
    assert!(
        output.status.success(),
        "vizier jobs schedule --format dag --max-depth 1 failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("job-a queued"),
        "expected root job in schedule:\n{stdout}"
    );
    assert!(
        stdout.contains("job-b succeeded"),
        "expected depth-1 producer edge:\n{stdout}"
    );
    assert!(
        !stdout.contains("job-c"),
        "did not expect depth-2 job with max-depth 1:\n{stdout}"
    );

    let output =
        repo.vizier_output(&["jobs", "schedule", "--format", "dag", "--max-depth", "2"])?;
    assert!(
        output.status.success(),
        "vizier jobs schedule --format dag --max-depth 2 failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("job-c"),
        "expected depth-2 job with max-depth 2:\n{stdout}"
    );
    Ok(())
}

#[test]
fn test_jobs_list_table_orders_by_created() -> TestResult {
    let repo = IntegrationRepo::new()?;
    write_job_record_simple(
        &repo,
        "job-oldest",
        "running",
        "2026-01-30T01:00:00Z",
        None,
        &["vizier", "save", "old"],
    )?;
    write_job_record_simple(
        &repo,
        "job-middle",
        "running",
        "2026-01-30T02:00:00Z",
        None,
        &["vizier", "save", "mid"],
    )?;
    write_job_record_simple(
        &repo,
        "job-newest",
        "running",
        "2026-01-30T03:00:00Z",
        None,
        &["vizier", "save", "new"],
    )?;

    let output = repo
        .vizier_cmd_background()
        .args(["jobs", "list", "--format", "table"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier jobs list --format table failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let newest = stdout
        .find("job-newest")
        .ok_or("missing newest job in table output")?;
    let middle = stdout
        .find("job-middle")
        .ok_or("missing middle job in table output")?;
    let oldest = stdout
        .find("job-oldest")
        .ok_or("missing oldest job in table output")?;
    assert!(
        newest < middle && middle < oldest,
        "jobs should be ordered newest to oldest:\n{stdout}"
    );
    Ok(())
}

#[test]
fn test_jobs_list_large_count_formats_outcome() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let config_path = repo.path().join(".vizier/config.toml");
    fs::write(
        &config_path,
        r#"
[display.lists.jobs]
format = "table"
fields = ["Job"]
show_succeeded = true
"#,
    )?;

    for idx in 0..1000 {
        let job_id = format!("job-{idx:04}");
        write_job_record_simple(
            &repo,
            &job_id,
            "succeeded",
            "2026-01-01T00:00:00Z",
            Some("2026-01-01T00:00:01Z"),
            &["vizier", "save", "bulk"],
        )?;
    }

    let output = repo
        .vizier_cmd_background()
        .args(["jobs", "list", "--format", "table"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier jobs list (large) failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Outcome: 1,000 background jobs"),
        "expected formatted job count in outcome:\n{stdout}"
    );
    Ok(())
}

#[test]
fn test_jobs_list_format_json() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-json-list";
    let record = json!({
        "id": job_id,
        "status": "failed",
        "command": ["vizier", "save", "json-list"],
        "created_at": "2026-01-31T01:00:00Z",
        "started_at": "2026-01-31T01:00:01Z",
        "finished_at": "2026-01-31T01:05:00Z",
        "pid": 9001,
        "exit_code": 1,
        "stdout_path": format!(".vizier/jobs/{job_id}/stdout.log"),
        "stderr_path": format!(".vizier/jobs/{job_id}/stderr.log"),
        "session_path": null,
        "outcome_path": null,
        "metadata": null,
        "config_snapshot": null,
        "schedule": {
            "dependencies": [
                { "artifact": { "plan_branch": { "slug": "alpha", "branch": "draft/alpha" } } }
            ],
            "locks": [
                { "key": "repo_serial", "mode": "exclusive" },
                { "key": "branch:draft/alpha", "mode": "shared" }
            ],
            "artifacts": [
                { "plan_commits": { "slug": "alpha", "branch": "draft/alpha" } }
            ],
            "pinned_head": { "branch": "main", "oid": "deadbeef" },
            "wait_reason": {
                "kind": "dependencies",
                "detail": "waiting on plan_branch:alpha (draft/alpha)"
            },
            "waited_on": ["dependencies"]
        }
    });
    write_job_record(&repo, job_id, record)?;

    let output = repo.vizier_output(&["jobs", "list", "--format", "json"])?;
    assert!(
        output.status.success(),
        "vizier jobs list --format json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout)?;
    let jobs = json
        .get("jobs")
        .and_then(|value| value.as_array())
        .ok_or("expected jobs array in JSON output")?;
    let job = jobs.first().ok_or("expected job entry in JSON output")?;
    assert_eq!(
        job.get("job").and_then(Value::as_str),
        Some(job_id),
        "job id mismatch in JSON output: {job}"
    );
    assert_eq!(
        job.get("status").and_then(Value::as_str),
        Some("failed"),
        "status mismatch in JSON output: {job}"
    );
    let created = job.get("created").and_then(Value::as_str).unwrap_or("");
    assert!(
        created.starts_with("2026-01-31T01:00:00"),
        "created timestamp mismatch: {created}"
    );
    assert_eq!(
        job.get("dependencies").and_then(Value::as_str),
        Some("plan_branch:alpha (draft/alpha)"),
        "dependencies mismatch: {job}"
    );
    assert_eq!(
        job.get("locks").and_then(Value::as_str),
        Some("repo_serial:exclusive, branch:draft/alpha:shared"),
        "locks mismatch: {job}"
    );
    assert_eq!(
        job.get("wait").and_then(Value::as_str),
        Some("dependencies: waiting on plan_branch:alpha (draft/alpha)"),
        "wait reason mismatch: {job}"
    );
    assert_eq!(
        job.get("waited_on").and_then(Value::as_str),
        Some("dependencies"),
        "waited_on mismatch: {job}"
    );
    assert_eq!(
        job.get("pinned_head").and_then(Value::as_str),
        Some("main@deadbeef"),
        "pinned head mismatch: {job}"
    );
    assert_eq!(
        job.get("artifacts").and_then(Value::as_str),
        Some("plan_commits:alpha (draft/alpha)"),
        "artifacts mismatch: {job}"
    );
    let failed = job.get("failed").and_then(Value::as_str).unwrap_or("");
    assert!(
        failed.starts_with("2026-01-31T01:05:00"),
        "failed timestamp mismatch: {failed}"
    );
    assert_eq!(
        job.get("command").and_then(Value::as_str),
        Some("vizier save json-list"),
        "command mismatch: {job}"
    );
    Ok(())
}

#[test]
fn test_jobs_list_and_show_json_include_after_field_when_configured() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-after-field";
    write_job_record(
        &repo,
        job_id,
        json!({
            "id": job_id,
            "status": "queued",
            "command": ["vizier", "save", "after"],
            "created_at": "2026-01-31T01:00:00Z",
            "started_at": null,
            "finished_at": null,
            "pid": null,
            "exit_code": null,
            "stdout_path": format!(".vizier/jobs/{job_id}/stdout.log"),
            "stderr_path": format!(".vizier/jobs/{job_id}/stderr.log"),
            "session_path": null,
            "outcome_path": null,
            "metadata": null,
            "config_snapshot": null,
            "schedule": {
                "after": [
                    { "job_id": "job-upstream", "policy": "success" }
                ]
            }
        }),
    )?;

    let config_path = repo.path().join(".vizier/tmp/jobs-after-fields.toml");
    fs::create_dir_all(config_path.parent().expect("config parent"))?;
    fs::write(
        &config_path,
        r#"
[display.lists.jobs]
format = "json"
show_succeeded = true
fields = ["Job", "After"]

[display.lists.jobs_show]
format = "json"
fields = ["Job", "After"]
"#,
    )?;

    let output = repo
        .vizier_cmd_background()
        .args([
            "--config-file",
            config_path.to_str().expect("config path utf8"),
            "jobs",
            "list",
            "--format",
            "json",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "vizier jobs list --format json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let list_json: Value = serde_json::from_slice(&output.stdout)?;
    let jobs = list_json
        .get("jobs")
        .and_then(Value::as_array)
        .ok_or("expected jobs array")?;
    let first = jobs.first().ok_or("expected one job entry")?;
    assert_eq!(
        first.get("after").and_then(Value::as_str),
        Some("job-upstream (success)"),
        "after field mismatch in jobs list JSON: {first}"
    );

    let output = repo
        .vizier_cmd_background()
        .args([
            "--config-file",
            config_path.to_str().expect("config path utf8"),
            "jobs",
            "show",
            job_id,
            "--format",
            "json",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "vizier jobs show --format json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let show_json: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        show_json.get("after").and_then(Value::as_str),
        Some("job-upstream (success)"),
        "after field mismatch in jobs show JSON: {show_json}"
    );
    Ok(())
}

#[test]
fn test_jobs_show_format_json() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-json-show";
    let record = json!({
        "id": job_id,
        "status": "failed",
        "command": ["vizier", "save", "show-json"],
        "created_at": "2026-01-31T02:00:00Z",
        "started_at": "2026-01-31T02:00:05Z",
        "finished_at": "2026-01-31T02:10:00Z",
        "pid": 4242,
        "exit_code": 42,
        "stdout_path": format!(".vizier/jobs/{job_id}/stdout.log"),
        "stderr_path": format!(".vizier/jobs/{job_id}/stderr.log"),
        "session_path": ".vizier/sessions/session.json",
        "outcome_path": format!(".vizier/jobs/{job_id}/outcome.json"),
        "metadata": {
            "scope": "save",
            "plan": "alpha",
            "target": "main",
            "branch": "draft/alpha",
            "build_pipeline": "approve-review-merge",
            "build_target": "build/alpha",
            "build_review_mode": "review_only",
            "build_skip_checks": true,
            "build_keep_branch": false,
            "build_dependencies": ["01", "02a"],
            "revision": "abc123",
            "worktree_name": "job-worktree",
            "worktree_path": ".vizier/tmp-worktrees/job-worktree",
            "agent_backend": "mock",
            "agent_label": "mock-agent",
            "agent_command": ["mock", "agent"],
            "agent_exit_code": 7,
            "cancel_cleanup_status": "done",
            "cancel_cleanup_error": "cleanup warning"
        },
        "config_snapshot": { "agent_selector": "codex", "workflow": { "background": { "quiet": false } } },
        "schedule": {
            "dependencies": [
                { "artifact": { "plan_doc": { "slug": "alpha", "branch": "draft/alpha" } } }
            ],
            "locks": [
                { "key": "repo_serial", "mode": "exclusive" }
            ],
            "artifacts": [
                { "plan_commits": { "slug": "alpha", "branch": "draft/alpha" } }
            ],
            "pinned_head": { "branch": "main", "oid": "feedface" },
            "wait_reason": { "kind": "locks", "detail": "waiting on locks" },
            "waited_on": ["locks"]
        }
    });
    write_job_record(&repo, job_id, record)?;

    let output = repo.vizier_output(&["jobs", "show", job_id, "--format", "json"])?;
    assert!(
        output.status.success(),
        "vizier jobs show --format json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        json.get("job").and_then(Value::as_str),
        Some(job_id),
        "job id mismatch: {json}"
    );
    assert_eq!(
        json.get("status").and_then(Value::as_str),
        Some("failed"),
        "status mismatch: {json}"
    );
    assert_eq!(
        json.get("pid").and_then(Value::as_str),
        Some("4242"),
        "pid mismatch: {json}"
    );
    let started = json.get("started").and_then(Value::as_str).unwrap_or("");
    assert!(
        started.starts_with("2026-01-31T02:00:05"),
        "started timestamp mismatch: {started}"
    );
    assert_eq!(
        json.get("exit_code").and_then(Value::as_str),
        Some("42"),
        "exit code mismatch: {json}"
    );
    assert_eq!(
        json.get("scope").and_then(Value::as_str),
        Some("save"),
        "scope mismatch: {json}"
    );
    assert_eq!(
        json.get("plan").and_then(Value::as_str),
        Some("alpha"),
        "plan mismatch: {json}"
    );
    assert_eq!(
        json.get("build_pipeline").and_then(Value::as_str),
        Some("approve-review-merge"),
        "build pipeline mismatch: {json}"
    );
    assert_eq!(
        json.get("build_target").and_then(Value::as_str),
        Some("build/alpha"),
        "build target mismatch: {json}"
    );
    assert_eq!(
        json.get("build_review_mode").and_then(Value::as_str),
        Some("review_only"),
        "build review mode mismatch: {json}"
    );
    assert_eq!(
        json.get("build_skip_checks").and_then(Value::as_str),
        Some("true"),
        "build skip checks mismatch: {json}"
    );
    assert_eq!(
        json.get("build_keep_branch").and_then(Value::as_str),
        Some("false"),
        "build keep branch mismatch: {json}"
    );
    assert_eq!(
        json.get("build_dependencies").and_then(Value::as_str),
        Some("01, 02a"),
        "build dependencies mismatch: {json}"
    );
    assert_eq!(
        json.get("dependencies").and_then(Value::as_str),
        Some("plan_doc:alpha (draft/alpha)"),
        "dependencies mismatch: {json}"
    );
    assert_eq!(
        json.get("locks").and_then(Value::as_str),
        Some("repo_serial:exclusive"),
        "locks mismatch: {json}"
    );
    assert_eq!(
        json.get("wait").and_then(Value::as_str),
        Some("locks: waiting on locks"),
        "wait mismatch: {json}"
    );
    assert_eq!(
        json.get("waited_on").and_then(Value::as_str),
        Some("locks"),
        "waited_on mismatch: {json}"
    );
    assert_eq!(
        json.get("pinned_head").and_then(Value::as_str),
        Some("main@feedface"),
        "pinned head mismatch: {json}"
    );
    assert_eq!(
        json.get("artifacts").and_then(Value::as_str),
        Some("plan_commits:alpha (draft/alpha)"),
        "artifacts mismatch: {json}"
    );
    assert_eq!(
        json.get("worktree").and_then(Value::as_str),
        Some(".vizier/tmp-worktrees/job-worktree"),
        "worktree mismatch: {json}"
    );
    assert_eq!(
        json.get("agent_command").and_then(Value::as_str),
        Some("mock agent"),
        "agent command mismatch: {json}"
    );
    assert_eq!(
        json.get("agent_exit").and_then(Value::as_str),
        Some("7"),
        "agent exit mismatch: {json}"
    );
    assert_eq!(
        json.get("cancel_cleanup").and_then(Value::as_str),
        Some("done"),
        "cancel cleanup mismatch: {json}"
    );
    assert_eq!(
        json.get("cancel_cleanup_error").and_then(Value::as_str),
        Some("cleanup warning"),
        "cancel cleanup error mismatch: {json}"
    );
    assert!(
        json.get("config_snapshot")
            .and_then(Value::as_object)
            .is_some(),
        "expected config_snapshot object: {json}"
    );
    assert_eq!(
        json.get("command").and_then(Value::as_str),
        Some("vizier save show-json"),
        "command mismatch: {json}"
    );
    Ok(())
}

#[test]
fn test_jobs_list_status_labels_for_waiting_and_blocked() -> TestResult {
    let repo = IntegrationRepo::new()?;
    write_job_record_simple(
        &repo,
        "job-wait-deps",
        "waiting_on_deps",
        "2026-01-31T03:00:00Z",
        None,
        &["vizier", "save", "wait-deps"],
    )?;
    write_job_record_simple(
        &repo,
        "job-wait-locks",
        "waiting_on_locks",
        "2026-01-31T03:01:00Z",
        None,
        &["vizier", "save", "wait-locks"],
    )?;
    write_job_record_simple(
        &repo,
        "job-wait-approval",
        "waiting_on_approval",
        "2026-01-31T03:01:30Z",
        None,
        &["vizier", "save", "wait-approval"],
    )?;
    write_job_record_simple(
        &repo,
        "job-blocked",
        "blocked_by_dependency",
        "2026-01-31T03:02:00Z",
        None,
        &["vizier", "save", "blocked"],
    )?;
    write_job_record_simple(
        &repo,
        "job-blocked-approval",
        "blocked_by_approval",
        "2026-01-31T03:02:30Z",
        None,
        &["vizier", "save", "blocked-approval"],
    )?;

    let output = repo.vizier_output(&["jobs", "list", "--format", "json"])?;
    assert!(
        output.status.success(),
        "vizier jobs list --format json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout)?;
    let jobs = json
        .get("jobs")
        .and_then(|value| value.as_array())
        .ok_or("expected jobs array in JSON output")?;

    let mut statuses = std::collections::HashMap::new();
    for job in jobs {
        if let (Some(id), Some(status)) = (
            job.get("job").and_then(Value::as_str),
            job.get("status").and_then(Value::as_str),
        ) {
            statuses.insert(id.to_string(), status.to_string());
        }
    }

    assert_eq!(
        statuses.get("job-wait-deps").map(String::as_str),
        Some("waiting_on_deps"),
        "waiting_on_deps label mismatch: {statuses:?}"
    );
    assert_eq!(
        statuses.get("job-wait-locks").map(String::as_str),
        Some("waiting_on_locks"),
        "waiting_on_locks label mismatch: {statuses:?}"
    );
    assert_eq!(
        statuses.get("job-wait-approval").map(String::as_str),
        Some("waiting_on_approval"),
        "waiting_on_approval label mismatch: {statuses:?}"
    );
    assert_eq!(
        statuses.get("job-blocked").map(String::as_str),
        Some("blocked_by_dependency"),
        "blocked_by_dependency label mismatch: {statuses:?}"
    );
    assert_eq!(
        statuses.get("job-blocked-approval").map(String::as_str),
        Some("blocked_by_approval"),
        "blocked_by_approval label mismatch: {statuses:?}"
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

#[test]
fn test_jobs_cancel_rejects_non_active_statuses() -> TestResult {
    let repo = IntegrationRepo::new()?;
    for (job_id, status) in [
        ("job-cancel-succeeded", "succeeded"),
        ("job-cancel-cancelled", "cancelled"),
    ] {
        write_job_record_simple(
            &repo,
            job_id,
            status,
            "2026-01-31T01:00:00Z",
            Some("2026-01-31T01:00:05Z"),
            &["vizier", "save", "done"],
        )?;
        let output = repo
            .vizier_cmd_background()
            .args(["jobs", "cancel", job_id])
            .output()?;
        assert!(
            !output.status.success(),
            "expected cancel to fail for status {status}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("is not active"),
            "expected non-active job error:\n{stderr}"
        );
    }
    Ok(())
}

#[test]
fn test_jobs_cancel_skips_unowned_worktree() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-cancel-unowned";
    let worktree_path = repo
        .path()
        .join(".vizier/tmp-worktrees")
        .join("unowned-worktree");
    fs::create_dir_all(&worktree_path)?;
    let pid = spawn_detached_sleep(10)?;
    let record = json!({
        "id": job_id,
        "status": "running",
        "command": ["vizier", "draft"],
        "created_at": "2026-01-31T02:00:00Z",
        "started_at": "2026-01-31T02:00:01Z",
        "finished_at": null,
        "pid": pid,
        "exit_code": null,
        "stdout_path": "stdout.log",
        "stderr_path": "stderr.log",
        "session_path": null,
        "outcome_path": null,
        "metadata": {
            "worktree_path": ".vizier/tmp-worktrees/unowned-worktree",
            "worktree_owned": false
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
        cancel_stdout.contains("cleanup=skipped"),
        "expected cleanup to be skipped for unowned worktree:\n{cancel_stdout}"
    );
    assert!(
        worktree_path.exists(),
        "expected unowned worktree to remain after cancel"
    );

    let record = read_job_record(&repo, job_id)?;
    let cleanup_status = record
        .get("metadata")
        .and_then(|meta| meta.get("cancel_cleanup_status"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert_eq!(
        cleanup_status, "skipped",
        "expected cleanup status to be skipped for unowned worktree"
    );
    Ok(())
}

#[test]
fn test_jobs_cancel_missing_worktree_reports_done() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-cancel-missing";
    let pid = spawn_detached_sleep(10)?;
    let record = json!({
        "id": job_id,
        "status": "running",
        "command": ["vizier", "draft"],
        "created_at": "2026-01-31T03:00:00Z",
        "started_at": "2026-01-31T03:00:01Z",
        "finished_at": null,
        "pid": pid,
        "exit_code": null,
        "stdout_path": "stdout.log",
        "stderr_path": "stderr.log",
        "session_path": null,
        "outcome_path": null,
        "metadata": {
            "worktree_path": ".vizier/tmp-worktrees/missing-worktree",
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
        "expected cleanup to be done for missing worktree:\n{cancel_stdout}"
    );
    let record = read_job_record(&repo, job_id)?;
    let cleanup_status = record
        .get("metadata")
        .and_then(|meta| meta.get("cancel_cleanup_status"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert_eq!(
        cleanup_status, "done",
        "expected cleanup status to be done for missing worktree"
    );
    Ok(())
}

#[test]
fn test_jobs_retry_rewinds_state_and_cleans_scheduler_artifacts() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-retry-cleanup";
    let worktree_rel = ".vizier/tmp-worktrees/retry-cleanup";
    fs::create_dir_all(repo.path().join(worktree_rel))?;

    write_job_record(
        &repo,
        job_id,
        json!({
            "id": job_id,
            "status": "failed",
            "command": ["vizier", "save", "retry cleanup"],
            "child_args": ["save", "retry cleanup"],
            "created_at": "2026-01-30T02:00:00Z",
            "started_at": "2026-01-30T02:00:01Z",
            "finished_at": "2026-01-30T02:00:02Z",
            "pid": 4321,
            "exit_code": 1,
            "stdout_path": format!(".vizier/jobs/{job_id}/stdout.log"),
            "stderr_path": format!(".vizier/jobs/{job_id}/stderr.log"),
            "session_path": ".vizier/sessions/retry/session.json",
            "outcome_path": format!(".vizier/jobs/{job_id}/outcome.json"),
            "metadata": {
                "scope": "save",
                "command_alias": "save",
                "workflow_template_selector": "template.save@v1",
                "worktree_owned": true,
                "worktree_path": worktree_rel,
                "agent_exit_code": 9,
                "cancel_cleanup_status": "failed",
                "cancel_cleanup_error": "stale error"
            },
            "config_snapshot": null,
            "schedule": {
                "dependencies": [
                    { "artifact": { "target_branch": { "name": "missing-retry-target" } } }
                ],
                "wait_reason": { "kind": "dependencies", "detail": "stale wait reason" },
                "waited_on": ["dependencies"]
            }
        }),
    )?;

    let job_dir = repo.path().join(".vizier/jobs").join(job_id);
    fs::write(job_dir.join("stdout.log"), "old stdout\n")?;
    fs::write(job_dir.join("stderr.log"), "old stderr\n")?;
    fs::write(job_dir.join("outcome.json"), "{}")?;
    fs::write(job_dir.join("ask-save.patch"), "old ask patch\n")?;
    fs::write(job_dir.join("save-input.patch"), "old save patch\n")?;

    let output = repo
        .vizier_cmd_background()
        .args(["jobs", "retry", job_id])
        .output()?;
    assert!(
        output.status.success(),
        "vizier jobs retry failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Outcome") && stdout.contains("Jobs retried"),
        "expected retry outcome block:\n{stdout}"
    );
    assert!(
        stdout.contains(job_id),
        "expected retry output to include job id:\n{stdout}"
    );

    let record = read_job_record(&repo, job_id)?;
    assert_eq!(
        record.get("status").and_then(Value::as_str),
        Some("blocked_by_dependency"),
        "expected retried job to re-enter scheduler as blocked by missing dependency: {record}"
    );
    assert_eq!(record.get("started_at"), Some(&Value::Null));
    assert_eq!(record.get("finished_at"), Some(&Value::Null));
    assert_eq!(record.get("pid"), Some(&Value::Null));
    assert_eq!(record.get("exit_code"), Some(&Value::Null));
    assert_eq!(record.get("session_path"), Some(&Value::Null));
    assert_eq!(record.get("outcome_path"), Some(&Value::Null));

    let metadata = record
        .get("metadata")
        .and_then(Value::as_object)
        .ok_or("expected metadata object")?;
    assert_eq!(metadata.get("worktree_path"), Some(&Value::Null));
    assert_eq!(metadata.get("worktree_owned"), Some(&Value::Null));
    assert_eq!(metadata.get("agent_exit_code"), Some(&Value::Null));
    assert_eq!(metadata.get("cancel_cleanup_status"), Some(&Value::Null));
    assert_eq!(metadata.get("cancel_cleanup_error"), Some(&Value::Null));
    assert_eq!(
        metadata.get("scope"),
        Some(&Value::String("save".to_string())),
        "retry should preserve legacy scope metadata"
    );
    assert_eq!(
        metadata.get("command_alias"),
        Some(&Value::String("save".to_string())),
        "retry should preserve command_alias metadata"
    );
    assert_eq!(
        metadata.get("workflow_template_selector"),
        Some(&Value::String("template.save@v1".to_string())),
        "retry should preserve workflow_template_selector metadata"
    );
    assert_eq!(
        metadata.get("retry_cleanup_status"),
        Some(&Value::String("done".to_string()))
    );
    assert_eq!(metadata.get("retry_cleanup_error"), Some(&Value::Null));

    assert!(
        !repo.path().join(worktree_rel).exists(),
        "expected retry cleanup to remove owned worktree"
    );
    assert!(
        !job_dir.join("outcome.json").exists(),
        "expected retry cleanup to remove stale outcome.json"
    );
    assert!(
        !job_dir.join("ask-save.patch").exists(),
        "expected retry cleanup to remove stale ask-save.patch"
    );
    assert!(
        !job_dir.join("save-input.patch").exists(),
        "expected retry cleanup to remove stale save-input.patch"
    );
    assert_eq!(fs::read_to_string(job_dir.join("stdout.log"))?, "");
    assert_eq!(fs::read_to_string(job_dir.join("stderr.log"))?, "");
    Ok(())
}

#[test]
fn test_jobs_retry_preserves_worktree_metadata_when_cleanup_degrades() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-retry-degraded";
    let worktree_rel = ".vizier/tmp-worktrees/retry-degraded";
    fs::create_dir_all(repo.path().join(worktree_rel))?;

    write_job_record(
        &repo,
        job_id,
        json!({
            "id": job_id,
            "status": "failed",
            "command": ["vizier", "save", "retry degraded"],
            "child_args": ["save", "retry degraded"],
            "created_at": "2026-01-30T03:00:00Z",
            "started_at": "2026-01-30T03:00:01Z",
            "finished_at": "2026-01-30T03:00:02Z",
            "pid": null,
            "exit_code": 1,
            "stdout_path": format!(".vizier/jobs/{job_id}/stdout.log"),
            "stderr_path": format!(".vizier/jobs/{job_id}/stderr.log"),
            "session_path": null,
            "outcome_path": null,
            "metadata": {
                "worktree_name": "missing-retry-worktree",
                "worktree_owned": true,
                "worktree_path": worktree_rel
            },
            "config_snapshot": null,
            "schedule": {
                "dependencies": [
                    { "artifact": { "target_branch": { "name": "missing-retry-degraded-target" } } }
                ]
            }
        }),
    )?;

    let output = repo
        .vizier_cmd_background()
        .args(["jobs", "retry", job_id])
        .output()?;
    assert!(
        output.status.success(),
        "vizier jobs retry (degraded cleanup) failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Jobs retried"),
        "expected retry outcome in stdout:\n{stdout}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("retry cleanup degraded") && stderr.contains("worktree metadata retained"),
        "expected actionable retry cleanup warning in stderr:\n{stderr}"
    );

    let record = read_job_record(&repo, job_id)?;
    assert_eq!(
        record.get("status").and_then(Value::as_str),
        Some("blocked_by_dependency"),
        "expected retried degraded job to remain blocked by missing dependency: {record}"
    );
    let metadata = record
        .get("metadata")
        .and_then(Value::as_object)
        .ok_or("expected metadata object")?;
    assert_eq!(
        metadata.get("worktree_name"),
        Some(&Value::String("missing-retry-worktree".to_string()))
    );
    assert_eq!(
        metadata.get("worktree_path"),
        Some(&Value::String(worktree_rel.to_string()))
    );
    assert_eq!(metadata.get("worktree_owned"), Some(&Value::Bool(true)));
    assert_eq!(
        metadata.get("retry_cleanup_status"),
        Some(&Value::String("degraded".to_string()))
    );
    let retry_cleanup_error = metadata
        .get("retry_cleanup_error")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        retry_cleanup_error.contains("fallback cleanup failed"),
        "expected fallback cleanup detail in metadata, got: {retry_cleanup_error}"
    );
    Ok(())
}

#[test]
fn test_jobs_retry_uses_fallback_cleanup_to_clear_worktree_metadata() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-retry-fallback";
    let worktree_rel = ".vizier/tmp-worktrees/retry-fallback";
    let worktree_path = repo.path().join(worktree_rel);
    if let Some(parent) = worktree_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let worktree_arg = worktree_path
        .to_str()
        .ok_or("worktree path is not valid utf-8")?
        .to_string();
    repo.git(&["worktree", "add", "--detach", &worktree_arg])?;

    write_job_record(
        &repo,
        job_id,
        json!({
            "id": job_id,
            "status": "failed",
            "command": ["vizier", "save", "retry fallback"],
            "child_args": ["save", "retry fallback"],
            "created_at": "2026-01-30T04:00:00Z",
            "started_at": "2026-01-30T04:00:01Z",
            "finished_at": "2026-01-30T04:00:02Z",
            "pid": null,
            "exit_code": 1,
            "stdout_path": format!(".vizier/jobs/{job_id}/stdout.log"),
            "stderr_path": format!(".vizier/jobs/{job_id}/stderr.log"),
            "session_path": null,
            "outcome_path": null,
            "metadata": {
                "worktree_name": "wrong-worktree-name",
                "worktree_owned": true,
                "worktree_path": worktree_rel
            },
            "config_snapshot": null,
            "schedule": {
                "dependencies": [
                    { "artifact": { "target_branch": { "name": "missing-retry-fallback-target" } } }
                ]
            }
        }),
    )?;

    let output = repo
        .vizier_cmd_background()
        .args(["jobs", "retry", job_id])
        .output()?;
    assert!(
        output.status.success(),
        "vizier jobs retry (fallback cleanup) failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let record = read_job_record(&repo, job_id)?;
    let metadata = record
        .get("metadata")
        .and_then(Value::as_object)
        .ok_or("expected metadata object")?;
    assert_eq!(metadata.get("worktree_name"), Some(&Value::Null));
    assert_eq!(metadata.get("worktree_path"), Some(&Value::Null));
    assert_eq!(metadata.get("worktree_owned"), Some(&Value::Null));
    assert_eq!(
        metadata.get("retry_cleanup_status"),
        Some(&Value::String("done".to_string()))
    );
    assert_eq!(metadata.get("retry_cleanup_error"), Some(&Value::Null));
    assert!(
        !worktree_path.exists(),
        "expected fallback cleanup to remove retry worktree"
    );
    Ok(())
}

#[test]
fn test_jobs_retry_json_output() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-retry-json";
    write_job_record(
        &repo,
        job_id,
        json!({
            "id": job_id,
            "status": "failed",
            "command": ["vizier", "save", "retry json"],
            "child_args": ["save", "retry json"],
            "created_at": "2026-01-30T02:00:00Z",
            "started_at": "2026-01-30T02:00:01Z",
            "finished_at": "2026-01-30T02:00:02Z",
            "pid": null,
            "exit_code": 1,
            "stdout_path": format!(".vizier/jobs/{job_id}/stdout.log"),
            "stderr_path": format!(".vizier/jobs/{job_id}/stderr.log"),
            "session_path": null,
            "outcome_path": null,
            "metadata": null,
            "config_snapshot": null,
            "schedule": {
                "dependencies": [
                    { "artifact": { "target_branch": { "name": "missing-retry-json-target" } } }
                ]
            }
        }),
    )?;

    let output = repo
        .vizier_cmd_background()
        .args(["--json", "jobs", "retry", job_id])
        .output()?;
    assert!(
        output.status.success(),
        "vizier --json jobs retry failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        payload.get("outcome").and_then(Value::as_str),
        Some("Jobs retried"),
        "unexpected retry JSON outcome: {payload}"
    );
    assert_eq!(
        payload.get("requested_job").and_then(Value::as_str),
        Some(job_id),
        "unexpected requested_job in retry JSON: {payload}"
    );
    assert_eq!(
        payload.get("retry_root").and_then(Value::as_str),
        Some(job_id),
        "unexpected retry_root in retry JSON: {payload}"
    );
    let retry_set = payload
        .get("retry_set")
        .and_then(Value::as_array)
        .ok_or("missing retry_set array")?;
    assert!(
        retry_set.iter().any(|value| value.as_str() == Some(job_id)),
        "retry_set should include requested job: {payload}"
    );
    let reset = payload
        .get("reset")
        .and_then(Value::as_array)
        .ok_or("missing reset array")?;
    assert!(
        reset.iter().any(|value| value.as_str() == Some(job_id)),
        "reset should include requested job: {payload}"
    );
    Ok(())
}

#[test]
fn test_jobs_gc_removes_old_jobs() -> TestResult {
    let repo = IntegrationRepo::new()?;
    write_job_record_simple(
        &repo,
        "job-old",
        "succeeded",
        "2000-01-01T00:00:00Z",
        Some("2000-01-01T00:00:01Z"),
        &["vizier", "save", "old"],
    )?;
    write_job_record_simple(
        &repo,
        "job-recent",
        "succeeded",
        "2099-01-01T00:00:00Z",
        Some("2099-01-01T00:00:01Z"),
        &["vizier", "save", "recent"],
    )?;
    write_job_record_simple(
        &repo,
        "job-running",
        "running",
        "2000-01-01T00:00:00Z",
        None,
        &["vizier", "save", "running"],
    )?;

    let output = repo
        .vizier_cmd_background()
        .args(["jobs", "gc", "--days", "7"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier jobs gc failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Outcome: removed 1 job(s)"),
        "unexpected gc outcome:\n{stdout}"
    );
    assert!(
        !repo.path().join(".vizier/jobs").join("job-old").exists(),
        "expected old job directory to be removed"
    );
    assert!(
        repo.path().join(".vizier/jobs").join("job-recent").exists(),
        "expected recent job to remain"
    );
    assert!(
        repo.path()
            .join(".vizier/jobs")
            .join("job-running")
            .exists(),
        "expected running job to remain"
    );
    Ok(())
}

#[test]
fn test_jobs_gc_preserves_terminal_jobs_referenced_by_active_after_dependencies() -> TestResult {
    let repo = IntegrationRepo::new()?;
    write_job_record(
        &repo,
        "job-old-predecessor",
        json!({
            "id": "job-old-predecessor",
            "status": "succeeded",
            "command": ["vizier", "save", "old"],
            "created_at": "2000-01-01T00:00:00Z",
            "started_at": "2000-01-01T00:00:00Z",
            "finished_at": "2000-01-01T00:00:01Z",
            "pid": null,
            "exit_code": 0,
            "stdout_path": ".vizier/jobs/job-old-predecessor/stdout.log",
            "stderr_path": ".vizier/jobs/job-old-predecessor/stderr.log",
            "session_path": null,
            "outcome_path": null,
            "metadata": null,
            "config_snapshot": null
        }),
    )?;
    write_job_record(
        &repo,
        "job-active-dependent",
        json!({
            "id": "job-active-dependent",
            "status": "queued",
            "command": ["vizier", "save", "queued"],
            "created_at": "2099-01-01T00:00:00Z",
            "started_at": null,
            "finished_at": null,
            "pid": null,
            "exit_code": null,
            "stdout_path": ".vizier/jobs/job-active-dependent/stdout.log",
            "stderr_path": ".vizier/jobs/job-active-dependent/stderr.log",
            "session_path": null,
            "outcome_path": null,
            "metadata": null,
            "config_snapshot": null,
            "schedule": {
                "after": [
                    { "job_id": "job-old-predecessor", "policy": "success" }
                ]
            }
        }),
    )?;

    let output = repo
        .vizier_cmd_background()
        .args(["jobs", "gc", "--days", "7"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier jobs gc failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Outcome: removed 0 job(s)"),
        "unexpected gc outcome:\n{stdout}"
    );
    assert!(
        repo.path()
            .join(".vizier/jobs")
            .join("job-old-predecessor")
            .exists(),
        "expected referenced predecessor job directory to remain"
    );
    Ok(())
}
