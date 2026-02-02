use crate::fixtures::*;

use std::io::Write;
use std::process::Stdio;

#[test]
fn test_scheduler_default_spawns_job() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo
        .vizier_cmd_background()
        .args(["ask", "scheduler default"])
        .output()?;
    assert!(
        output.status.success(),
        "scheduled ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let job_id = extract_job_id(&stdout).ok_or("expected job id in output")?;
    let job_path = repo
        .path()
        .join(".vizier/jobs")
        .join(&job_id)
        .join("job.json");
    assert!(
        job_path.exists(),
        "expected job record at {}",
        job_path.display()
    );
    wait_for_job_completion(&repo, &job_id, Duration::from_secs(20))?;
    Ok(())
}

#[test]
fn test_scheduler_default_is_not_quiet() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo
        .vizier_cmd_background()
        .args(["ask", "scheduler stderr"])
        .output()?;
    assert!(
        output.status.success(),
        "scheduled ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let job_id = extract_job_id(&stdout).ok_or("expected job id in output")?;
    wait_for_job_completion(&repo, &job_id, Duration::from_secs(20))?;

    let stderr_path = repo
        .path()
        .join(".vizier/jobs")
        .join(&job_id)
        .join("stderr.log");
    let stderr_log = fs::read_to_string(&stderr_path)?;
    assert!(
        stderr_log.contains("mock agent running"),
        "expected scheduler stderr log to include progress output:\n{stderr_log}"
    );
    Ok(())
}

#[test]
fn test_scheduler_follow_streams_logs() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo
        .vizier_cmd_background()
        .args(["--follow", "ask", "scheduler follow"])
        .output()?;
    assert!(
        output.status.success(),
        "scheduled ask --follow failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Agent run:"),
        "expected follow stdout to include agent summary:\n{stdout}"
    );
    Ok(())
}

#[test]
fn test_scheduler_stdin_is_supported() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let mut cmd = repo.vizier_cmd_background();
    cmd.args(["ask"]);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(b"scheduler stdin prompt\n")?;
    }
    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "stdin ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let job_id = extract_job_id(&stdout).ok_or("expected job id for stdin ask")?;
    wait_for_job_completion(&repo, &job_id, Duration::from_secs(20))?;
    Ok(())
}

#[test]
fn test_scheduler_rejects_json_output() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let blocked = repo
        .vizier_cmd_background()
        .args(["ask", "json scheduler", "--json"])
        .output()?;
    assert!(
        !blocked.status.success(),
        "expected ask --json to fail under scheduler"
    );
    let stderr = String::from_utf8_lossy(&blocked.stderr);
    assert!(
        stderr.contains("--json cannot be used"),
        "unexpected stderr for --json scheduler error:\n{stderr}"
    );
    Ok(())
}

#[test]
fn test_scheduler_rejects_json_for_additional_commands() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let cases = [
        vec!["--json", "draft", "json draft spec"],
        vec!["--json", "save"],
    ];

    for args in cases {
        let output = repo.vizier_cmd_background().args(&args).output()?;
        assert!(
            !output.status.success(),
            "expected --json rejection for {:?}",
            args
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("--json cannot be used"),
            "missing --json rejection message:\n{stderr}"
        );
    }
    Ok(())
}

#[test]
fn test_scheduler_requires_noninteractive_flags() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let cases = [
        (vec!["approve", "plan-a"], "vizier approve requires --yes"),
        (vec!["merge", "plan-b"], "vizier merge requires --yes"),
        (vec!["review", "plan-c"], "vizier review requires --yes"),
    ];

    for (args, expected) in cases {
        let output = repo.vizier_cmd_background().args(&args).output()?;
        assert!(
            !output.status.success(),
            "expected scheduler safety gate to fail for {:?}",
            args
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(expected),
            "missing scheduler safety gate message in stderr:\n{stderr}"
        );
    }
    Ok(())
}

#[test]
fn test_scheduler_dependency_waits_and_unblocks() -> TestResult {
    let repo = IntegrationRepo::new_without_mock()?;
    let draft_agent_path = write_sleeping_agent(&repo, "sleepy-draft", 2)?;
    let draft_config = write_agent_config(
        &repo,
        "config-sleepy-draft.toml",
        "draft",
        &draft_agent_path,
    )?;
    let approve_agent_path = write_sleeping_agent(&repo, "fast-approve", 0)?;
    let approve_config = write_agent_config(
        &repo,
        "config-fast-approve.toml",
        "approve",
        &approve_agent_path,
    )?;

    let output = repo
        .vizier_cmd_background_with_config(&draft_config)
        .args(["draft", "--name", "dep-wait", "dependency wait spec"])
        .output()?;
    assert!(
        output.status.success(),
        "scheduled draft failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let draft_stdout = String::from_utf8_lossy(&output.stdout);
    let draft_job_id = extract_job_id(&draft_stdout).ok_or("expected draft job id")?;
    wait_for_job_status(&repo, &draft_job_id, "running", Duration::from_secs(5))?;

    let approve = repo
        .vizier_cmd_background_with_config(&approve_config)
        .args(["approve", "dep-wait", "--yes"])
        .output()?;
    assert!(
        approve.status.success(),
        "scheduled approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );
    let approve_stdout = String::from_utf8_lossy(&approve.stdout);
    let approve_job_id = extract_job_id(&approve_stdout).ok_or("expected approve job id")?;

    wait_for_job_status(
        &repo,
        &approve_job_id,
        "waiting_on_deps",
        Duration::from_secs(5),
    )?;
    let approve_record = read_job_record(&repo, &approve_job_id)?;
    let wait_kind = approve_record
        .get("schedule")
        .and_then(|s| s.get("wait_reason"))
        .and_then(|r| r.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert_eq!(
        wait_kind, "dependencies",
        "expected approve to wait on dependencies, got {wait_kind}"
    );

    wait_for_job_completion(&repo, &draft_job_id, Duration::from_secs(30))?;
    let draft_record = read_job_record(&repo, &draft_job_id)?;
    let draft_status = draft_record
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("");
    if draft_status != "succeeded" {
        let stdout_path = draft_record
            .get("stdout_path")
            .and_then(Value::as_str)
            .map(|rel| repo.path().join(rel));
        let stderr_path = draft_record
            .get("stderr_path")
            .and_then(Value::as_str)
            .map(|rel| repo.path().join(rel));
        let stdout_log = stdout_path
            .as_ref()
            .and_then(|path| fs::read_to_string(path).ok())
            .unwrap_or_default();
        let stderr_log = stderr_path
            .as_ref()
            .and_then(|path| fs::read_to_string(path).ok())
            .unwrap_or_default();
        return Err(format!(
            "draft job failed (status={draft_status}). stdout:\n{stdout_log}\nstderr:\n{stderr_log}"
        )
        .into());
    }

    wait_for_job_completion(&repo, &approve_job_id, Duration::from_secs(30))?;
    let approve_record = read_job_record(&repo, &approve_job_id)?;
    let status = approve_record
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert_eq!(
        status, "succeeded",
        "expected approve to complete after dependency unblocks"
    );
    let waited_on = approve_record
        .get("schedule")
        .and_then(|s| s.get("waited_on"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        waited_on
            .iter()
            .any(|value| value.as_str() == Some("dependencies")),
        "expected waited_on to include dependencies: {waited_on:?}"
    );
    Ok(())
}

#[test]
fn test_scheduler_lock_contention_waits() -> TestResult {
    let repo = IntegrationRepo::new_without_mock()?;
    let draft_agent_path = write_sleeping_agent(&repo, "fast-draft", 0)?;
    let draft_config =
        write_agent_config(&repo, "config-fast-draft.toml", "draft", &draft_agent_path)?;
    let draft = repo
        .vizier_cmd_with_config(&draft_config)
        .args(["draft", "--name", "lock-wait", "lock contention spec"])
        .output()?;
    assert!(
        draft.status.success(),
        "draft setup failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    let approve_agent_path = write_sleeping_agent(&repo, "sleepy-approve", 2)?;
    let approve_config = write_agent_config(
        &repo,
        "config-sleepy-approve.toml",
        "approve",
        &approve_agent_path,
    )?;
    let review_agent_path = write_sleeping_agent(&repo, "fast-review", 0)?;
    let review_config = write_agent_config(
        &repo,
        "config-fast-review.toml",
        "review",
        &review_agent_path,
    )?;

    let approve = repo
        .vizier_cmd_background_with_config(&approve_config)
        .args(["approve", "lock-wait", "--yes"])
        .output()?;
    assert!(
        approve.status.success(),
        "scheduled approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );
    let approve_stdout = String::from_utf8_lossy(&approve.stdout);
    let approve_job_id = extract_job_id(&approve_stdout).ok_or("expected approve job id")?;
    wait_for_job_status(&repo, &approve_job_id, "running", Duration::from_secs(5))?;

    let review = repo
        .vizier_cmd_background_with_config(&review_config)
        .args(["review", "lock-wait", "--review-only"])
        .output()?;
    assert!(
        review.status.success(),
        "scheduled review failed: {}",
        String::from_utf8_lossy(&review.stderr)
    );
    let review_stdout = String::from_utf8_lossy(&review.stdout);
    let review_job_id = extract_job_id(&review_stdout).ok_or("expected review job id")?;

    wait_for_job_status(
        &repo,
        &review_job_id,
        "waiting_on_locks",
        Duration::from_secs(5),
    )?;
    let review_record = read_job_record(&repo, &review_job_id)?;
    let wait_kind = review_record
        .get("schedule")
        .and_then(|s| s.get("wait_reason"))
        .and_then(|r| r.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert_eq!(
        wait_kind, "locks",
        "expected review to wait on locks, got {wait_kind}"
    );

    wait_for_job_completion(&repo, &approve_job_id, Duration::from_secs(30))?;
    wait_for_job_completion(&repo, &review_job_id, Duration::from_secs(30))?;
    Ok(())
}

#[test]
fn test_scheduler_pinned_head_mismatch_waits() -> TestResult {
    let repo = IntegrationRepo::new_without_mock()?;
    let agent_path = write_sleeping_agent(&repo, "sleepy-ask", 2)?;
    let config_path = write_agent_config(&repo, "config-sleepy-ask.toml", "ask", &agent_path)?;

    let first = repo
        .vizier_cmd_background_with_config(&config_path)
        .args(["ask", "pinned head first"])
        .output()?;
    assert!(
        first.status.success(),
        "scheduled first ask failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_stdout = String::from_utf8_lossy(&first.stdout);
    let first_job_id = extract_job_id(&first_stdout).ok_or("expected first ask job id")?;
    wait_for_job_status(&repo, &first_job_id, "running", Duration::from_secs(5))?;

    let second = repo
        .vizier_cmd_background_with_config(&config_path)
        .args(["ask", "pinned head second"])
        .output()?;
    assert!(
        second.status.success(),
        "scheduled second ask failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_stdout = String::from_utf8_lossy(&second.stdout);
    let second_job_id = extract_job_id(&second_stdout).ok_or("expected second ask job id")?;
    wait_for_job_status(
        &repo,
        &second_job_id,
        "waiting_on_locks",
        Duration::from_secs(5),
    )?;

    wait_for_job_completion(&repo, &first_job_id, Duration::from_secs(30))?;
    wait_for_job_status(
        &repo,
        &second_job_id,
        "waiting_on_deps",
        Duration::from_secs(10),
    )?;

    let second_record = read_job_record(&repo, &second_job_id)?;
    let wait_kind = second_record
        .get("schedule")
        .and_then(|s| s.get("wait_reason"))
        .and_then(|r| r.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert_eq!(
        wait_kind, "pinned_head",
        "expected pinned head wait reason, got {wait_kind}"
    );
    let waited_on = second_record
        .get("schedule")
        .and_then(|s| s.get("waited_on"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        waited_on
            .iter()
            .any(|value| value.as_str() == Some("pinned_head")),
        "expected waited_on to include pinned_head: {waited_on:?}"
    );

    let cancel = repo
        .vizier_cmd_background()
        .args(["jobs", "cancel", &second_job_id])
        .output()?;
    assert!(
        cancel.status.success(),
        "failed to cancel pinned-head job: {}",
        String::from_utf8_lossy(&cancel.stderr)
    );
    Ok(())
}

#[test]
fn test_scheduler_background_ask_applies_single_commit() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;

    let output = repo
        .vizier_cmd_background()
        .args(["ask", "background ask single commit"])
        .output()?;
    assert!(
        output.status.success(),
        "scheduled ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let job_id = extract_job_id(&stdout).ok_or("expected job id")?;
    wait_for_job_completion(&repo, &job_id, Duration::from_secs(20))?;

    let after = count_commits_from_head(&repo.repo())?;
    assert_eq!(after - before, 1, "ask should create one commit");
    let files = files_changed_in_commit(&repo.repo(), "HEAD")?;
    assert!(
        files.contains("a")
            && files.contains(".vizier/narrative/snapshot.md")
            && files.contains(".vizier/narrative/glossary.md"),
        "ask commit should include code + narrative files, got {files:?}"
    );
    Ok(())
}

#[test]
fn test_scheduler_background_save_applies_single_commit() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;

    let output = repo.vizier_cmd_background().args(["save"]).output()?;
    assert!(
        output.status.success(),
        "scheduled save failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let job_id = extract_job_id(&stdout).ok_or("expected job id")?;
    wait_for_job_completion(&repo, &job_id, Duration::from_secs(20))?;

    let after = count_commits_from_head(&repo.repo())?;
    assert_eq!(after - before, 1, "save should create one commit");
    let files = files_changed_in_commit(&repo.repo(), "HEAD")?;
    assert!(
        files.contains("a")
            && files.contains(".vizier/narrative/snapshot.md")
            && files.contains(".vizier/narrative/glossary.md"),
        "save commit should include code + narrative files, got {files:?}"
    );
    Ok(())
}

#[test]
fn test_scheduler_background_ask_fails_on_pinned_head_mismatch() -> TestResult {
    let repo = IntegrationRepo::new_without_mock()?;
    let agent_path = write_sleeping_agent(&repo, "sleepy-ask", 2)?;
    let config_path = write_agent_config(&repo, "config-sleepy-ask.toml", "ask", &agent_path)?;

    let output = repo
        .vizier_cmd_background_with_config(&config_path)
        .args(["ask", "pinned mismatch"])
        .output()?;
    assert!(
        output.status.success(),
        "scheduled ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let job_id = extract_job_id(&stdout).ok_or("expected ask job id")?;
    wait_for_job_status(&repo, &job_id, "running", Duration::from_secs(5))?;

    repo.write("a", "pinned head mismatch\n")?;
    repo.git(&["add", "a"])?;
    repo.git(&["commit", "-m", "pinned head mismatch"])?;

    wait_for_job_completion(&repo, &job_id, Duration::from_secs(30))?;
    let record = read_job_record(&repo, &job_id)?;
    let status = record.get("status").and_then(Value::as_str).unwrap_or("");
    assert_eq!(
        status, "failed",
        "expected ask to fail on pinned head mismatch"
    );
    let stderr_path = repo
        .path()
        .join(".vizier/jobs")
        .join(&job_id)
        .join("stderr.log");
    let stderr_log = fs::read_to_string(&stderr_path).unwrap_or_default();
    assert!(
        stderr_log.contains("pinned head mismatch"),
        "expected pinned head mismatch error in stderr:\n{stderr_log}"
    );
    Ok(())
}

#[test]
fn test_scheduler_background_save_fails_when_input_patch_missing() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-save-missing-patch";
    let head_oid = oid_for_spec(&repo.repo(), "HEAD")?.to_string();
    let record = json!({
        "id": job_id,
        "status": "running",
        "command": ["vizier", "save"],
        "created_at": "2026-01-31T04:00:00Z",
        "started_at": "2026-01-31T04:00:01Z",
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
            "pinned_head": { "branch": "master", "oid": head_oid }
        }
    });
    write_job_record(&repo, job_id, record)?;

    let output = repo
        .vizier_cmd_background()
        .args(["--background-job-id", job_id, "save"])
        .output()?;
    assert!(
        !output.status.success(),
        "expected save to fail with missing input patch"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing the captured input patch"),
        "missing patch error not reported:\n{stderr}"
    );
    let save_record = read_job_record(&repo, job_id)?;
    let status = save_record
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert_eq!(
        status, "failed",
        "expected save to fail when input patch is missing"
    );
    Ok(())
}
