use crate::fixtures::*;

use std::io::Write;
use std::process::Stdio;

#[test]
fn test_scheduler_default_spawns_job() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo.vizier_cmd_background().args(["save"]).output()?;
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
    let output = repo.vizier_cmd_background().args(["save"]).output()?;
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
        .args(["--follow", "save"])
        .output()?;
    assert!(
        output.status.success(),
        "scheduled ask --follow failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Outcome    : Save complete") && stdout.contains("Agent      :"),
        "expected follow stdout to include agent summary:\n{stdout}"
    );
    Ok(())
}

#[test]
fn test_scheduler_stdin_is_supported() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let mut cmd = repo.vizier_cmd_background();
    cmd.args(["save"]);
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
        .args(["save", "--json"])
        .output()?;
    assert!(!blocked.status.success(), "expected save --json to fail");
    let stderr = String::from_utf8_lossy(&blocked.stderr);
    assert!(
        stderr.contains("global `--json` was removed"),
        "unexpected stderr for removed --json guidance:\n{stderr}"
    );
    Ok(())
}

#[test]
fn test_scheduler_rejects_json_for_additional_commands() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let cases = [
        vec!["--json", "draft", "json draft spec"],
        vec!["--json", "patch", "README.md", "--yes"],
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
            stderr.contains("global `--json` was removed"),
            "missing removed --json guidance:\n{stderr}"
        );
    }
    Ok(())
}

#[test]
fn test_scheduler_rejects_removed_background_globals() -> TestResult {
    let repo = IntegrationRepo::new()?;
    for flag in ["--background", "--no-background"] {
        let output = repo.vizier_cmd_background().args([flag, "save"]).output()?;
        assert!(
            !output.status.success(),
            "expected {flag} to fail with migration guidance"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("was removed"),
            "missing removed-flag guidance for {flag}:\n{stderr}"
        );
    }
    Ok(())
}

#[test]
fn test_scheduler_rejects_unknown_after_dependency() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo
        .vizier_cmd_background()
        .args(["save", "--after", "job-missing"])
        .output()?;
    assert!(
        !output.status.success(),
        "expected unknown --after dependency to fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown --after job id: job-missing"),
        "missing unknown --after error message:\n{stderr}"
    );
    Ok(())
}

#[test]
fn test_scheduler_after_flag_is_repeatable_and_recorded() -> TestResult {
    let repo = IntegrationRepo::new()?;

    let first = repo.vizier_cmd_background().args(["save"]).output()?;
    assert!(
        first.status.success(),
        "first scheduled ask failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_job_id =
        extract_job_id(&String::from_utf8_lossy(&first.stdout)).ok_or("first job id missing")?;
    wait_for_job_completion(&repo, &first_job_id, Duration::from_secs(20))?;

    let second = repo.vizier_cmd_background().args(["save"]).output()?;
    assert!(
        second.status.success(),
        "second scheduled ask failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_job_id =
        extract_job_id(&String::from_utf8_lossy(&second.stdout)).ok_or("second job id missing")?;
    wait_for_job_completion(&repo, &second_job_id, Duration::from_secs(20))?;

    let third = repo
        .vizier_cmd_background()
        .args([
            "save",
            "after dependency third",
            "--after",
            &first_job_id,
            "--after",
            &first_job_id,
            "--after",
            &second_job_id,
        ])
        .output()?;
    assert!(
        third.status.success(),
        "third scheduled ask failed: {}",
        String::from_utf8_lossy(&third.stderr)
    );
    let third_job_id =
        extract_job_id(&String::from_utf8_lossy(&third.stdout)).ok_or("third job id missing")?;

    let record = read_job_record(&repo, &third_job_id)?;
    let after = record
        .get("schedule")
        .and_then(|value| value.get("after"))
        .and_then(Value::as_array)
        .ok_or("missing schedule.after for third job")?;
    let ids = after
        .iter()
        .filter_map(|value| value.get("job_id").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![first_job_id.as_str(), second_job_id.as_str()],
        "repeatable --after values were not deduplicated/preserved in order: {after:?}"
    );

    wait_for_job_completion(&repo, &third_job_id, Duration::from_secs(20))?;
    Ok(())
}

#[test]
fn test_scheduler_requires_noninteractive_flags() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let cases = [
        (vec!["approve", "plan-a"], "vizier approve requires --yes"),
        (vec!["patch", "README.md"], "vizier patch requires --yes"),
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
    let (draft_agent_path, draft_gate_path) =
        write_gated_agent(&repo, "gated-draft", "dep-wait-draft.ready")?;
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

    fs::write(&draft_gate_path, "release\n")?;
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
fn test_scheduler_after_dependency_waits_and_unblocks() -> TestResult {
    let repo = IntegrationRepo::new_without_mock()?;
    let (agent_path, gate_path) =
        write_gated_agent(&repo, "gated-draft-after", "after-wait-predecessor.ready")?;
    let config_path = write_agent_config(
        &repo,
        "config-sleepy-draft-after.toml",
        "draft",
        &agent_path,
    )?;

    let first = repo
        .vizier_cmd_background_with_config(&config_path)
        .args([
            "draft",
            "--name",
            "after-wait-predecessor",
            "after wait predecessor spec",
        ])
        .output()?;
    assert!(
        first.status.success(),
        "scheduled predecessor draft failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_job_id =
        extract_job_id(&String::from_utf8_lossy(&first.stdout)).ok_or("missing predecessor id")?;
    wait_for_job_status(&repo, &first_job_id, "running", Duration::from_secs(5))?;

    let second = repo
        .vizier_cmd_background_with_config(&config_path)
        .args([
            "draft",
            "--name",
            "after-wait-dependent",
            "--after",
            &first_job_id,
            "after wait dependent spec",
        ])
        .output()?;
    assert!(
        second.status.success(),
        "scheduled dependent draft failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_job_id =
        extract_job_id(&String::from_utf8_lossy(&second.stdout)).ok_or("missing dependent id")?;

    wait_for_job_status(
        &repo,
        &second_job_id,
        "waiting_on_deps",
        Duration::from_secs(5),
    )?;
    let record = read_job_record(&repo, &second_job_id)?;
    let detail = record
        .get("schedule")
        .and_then(|schedule| schedule.get("wait_reason"))
        .and_then(|reason| reason.get("detail"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert_eq!(
        detail,
        format!("waiting on job {}", first_job_id),
        "expected --after wait reason detail to mention predecessor"
    );

    fs::write(&gate_path, "release\n")?;
    wait_for_job_completion(&repo, &first_job_id, Duration::from_secs(30))?;
    wait_for_job_completion(&repo, &second_job_id, Duration::from_secs(30))?;
    Ok(())
}

#[test]
fn test_scheduler_require_approval_waits_until_jobs_approve() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let draft = repo.vizier_output(&[
        "draft",
        "--name",
        "approval-gated",
        "approval gate scheduler spec",
    ])?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    let queued = repo
        .vizier_cmd_background()
        .args(["approve", "approval-gated", "--yes", "--require-approval"])
        .output()?;
    assert!(
        queued.status.success(),
        "scheduled approve with --require-approval failed: {}",
        String::from_utf8_lossy(&queued.stderr)
    );
    let queued_stdout = String::from_utf8_lossy(&queued.stdout);
    assert!(
        queued_stdout.contains("Status: waiting_on_approval"),
        "expected queue summary to show waiting_on_approval:\n{queued_stdout}"
    );
    assert!(
        queued_stdout.contains("Next: vizier jobs approve"),
        "expected queue summary to show next approve action:\n{queued_stdout}"
    );
    let job_id = extract_job_id(&queued_stdout).ok_or("expected approve job id")?;

    wait_for_job_status(
        &repo,
        &job_id,
        "waiting_on_approval",
        Duration::from_secs(10),
    )?;
    let pending_record = read_job_record(&repo, &job_id)?;
    assert_eq!(
        pending_record
            .pointer("/schedule/approval/required")
            .and_then(Value::as_bool),
        Some(true),
        "expected approval.required=true on queued job: {pending_record}"
    );
    assert_eq!(
        pending_record
            .pointer("/schedule/approval/state")
            .and_then(Value::as_str),
        Some("pending"),
        "expected approval.state=pending before decision: {pending_record}"
    );

    let approval = repo
        .vizier_cmd_background()
        .args(["jobs", "approve", &job_id])
        .output()?;
    assert!(
        approval.status.success(),
        "vizier jobs approve failed: {}",
        String::from_utf8_lossy(&approval.stderr)
    );
    let approval_stdout = String::from_utf8_lossy(&approval.stdout);
    assert!(
        approval_stdout.contains("Job approval granted"),
        "expected approval outcome block:\n{approval_stdout}"
    );

    wait_for_job_completion(&repo, &job_id, Duration::from_secs(30))?;
    let final_record = read_job_record(&repo, &job_id)?;
    assert_ne!(
        final_record.get("status").and_then(Value::as_str),
        Some("waiting_on_approval"),
        "job should leave waiting_on_approval after approval: {final_record}"
    );
    assert_eq!(
        final_record
            .pointer("/schedule/approval/state")
            .and_then(Value::as_str),
        Some("approved"),
        "approval state should be approved after decision: {final_record}"
    );
    Ok(())
}

#[test]
fn test_scheduler_after_dependency_blocks_on_failed_predecessor() -> TestResult {
    let repo = IntegrationRepo::new_without_mock()?;

    let bin_dir = repo.path().join(".vizier/tmp/bin");
    fs::create_dir_all(&bin_dir)?;
    let failing_agent = bin_dir.join("failing-after.sh");
    fs::write(
        &failing_agent,
        "#!/bin/sh\nset -eu\ncat >/dev/null\necho 'intentional failure' 1>&2\nexit 1\n",
    )?;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&failing_agent)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&failing_agent, perms)?;
    }

    let failing_config =
        write_agent_config(&repo, "config-failing-after.toml", "save", &failing_agent)?;
    let fast_agent = write_sleeping_agent(&repo, "fast-after", 0)?;
    let fast_config = write_agent_config(&repo, "config-fast-after.toml", "save", &fast_agent)?;

    let predecessor = repo
        .vizier_cmd_background_with_config(&failing_config)
        .args(["save"])
        .output()?;
    assert!(
        predecessor.status.success(),
        "scheduling failing predecessor ask failed: {}",
        String::from_utf8_lossy(&predecessor.stderr)
    );
    let predecessor_job_id = extract_job_id(&String::from_utf8_lossy(&predecessor.stdout))
        .ok_or("missing predecessor job id")?;
    wait_for_job_completion(&repo, &predecessor_job_id, Duration::from_secs(20))?;
    let predecessor_record = read_job_record(&repo, &predecessor_job_id)?;
    assert_eq!(
        predecessor_record
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
        "failed",
        "expected predecessor to fail"
    );

    let dependent = repo
        .vizier_cmd_background_with_config(&fast_config)
        .args([
            "save",
            "after blocked dependent",
            "--after",
            &predecessor_job_id,
        ])
        .output()?;
    assert!(
        dependent.status.success(),
        "scheduling dependent ask failed: {}",
        String::from_utf8_lossy(&dependent.stderr)
    );
    let dependent_job_id = extract_job_id(&String::from_utf8_lossy(&dependent.stdout))
        .ok_or("missing dependent id")?;

    wait_for_job_status(
        &repo,
        &dependent_job_id,
        "blocked_by_dependency",
        Duration::from_secs(10),
    )?;
    let dependent_record = read_job_record(&repo, &dependent_job_id)?;
    let detail = dependent_record
        .get("schedule")
        .and_then(|schedule| schedule.get("wait_reason"))
        .and_then(|reason| reason.get("detail"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        detail.contains(&format!(
            "dependency failed for job {} (failed)",
            predecessor_job_id
        )),
        "expected failed predecessor detail, got: {detail}"
    );
    Ok(())
}

#[test]
fn test_scheduler_retry_unblocks_after_dependency_chain() -> TestResult {
    let repo = IntegrationRepo::new_without_mock()?;

    let bin_dir = repo.path().join(".vizier/tmp/bin");
    fs::create_dir_all(&bin_dir)?;
    let retry_agent = bin_dir.join("retry-after.sh");
    fs::write(
        &retry_agent,
        "#!/bin/sh\nset -eu\ncat >/dev/null\necho 'intentional failure' 1>&2\nexit 1\n",
    )?;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&retry_agent)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&retry_agent, perms)?;
    }

    let retry_config = write_agent_config(&repo, "config-retry-after.toml", "draft", &retry_agent)?;
    let fast_agent = write_sleeping_agent(&repo, "fast-after-retry", 0)?;
    let fast_config =
        write_agent_config(&repo, "config-fast-after-retry.toml", "draft", &fast_agent)?;

    let predecessor = repo
        .vizier_cmd_background_with_config(&retry_config)
        .args([
            "draft",
            "--name",
            "retry-predecessor",
            "retry predecessor spec",
        ])
        .output()?;
    assert!(
        predecessor.status.success(),
        "scheduling predecessor draft failed: {}",
        String::from_utf8_lossy(&predecessor.stderr)
    );
    let predecessor_job_id = extract_job_id(&String::from_utf8_lossy(&predecessor.stdout))
        .ok_or("missing predecessor job id")?;
    wait_for_job_completion(&repo, &predecessor_job_id, Duration::from_secs(20))?;
    assert_eq!(
        read_job_record(&repo, &predecessor_job_id)?
            .get("status")
            .and_then(Value::as_str),
        Some("failed"),
        "predecessor should fail before retry"
    );

    let dependent = repo
        .vizier_cmd_background_with_config(&fast_config)
        .args([
            "draft",
            "--name",
            "retry-dependent",
            "--after",
            &predecessor_job_id,
            "retry dependent spec",
        ])
        .output()?;
    assert!(
        dependent.status.success(),
        "scheduling dependent draft failed: {}",
        String::from_utf8_lossy(&dependent.stderr)
    );
    let dependent_job_id = extract_job_id(&String::from_utf8_lossy(&dependent.stdout))
        .ok_or("missing dependent job id")?;
    wait_for_job_status(
        &repo,
        &dependent_job_id,
        "blocked_by_dependency",
        Duration::from_secs(10),
    )?;

    // Flip the predecessor script to succeed so retry can advance the chain.
    fs::write(
        &retry_agent,
        "#!/bin/sh\nset -eu\ncat >/dev/null\necho 'retry success' 1>&2\necho 'mock agent response'\n",
    )?;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&retry_agent)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&retry_agent, perms)?;
    }

    let retry = repo
        .vizier_cmd_background()
        .args(["jobs", "retry", &predecessor_job_id])
        .output()?;
    assert!(
        retry.status.success(),
        "jobs retry failed: {}",
        String::from_utf8_lossy(&retry.stderr)
    );

    wait_for_job_completion(&repo, &predecessor_job_id, Duration::from_secs(30))?;
    wait_for_job_completion(&repo, &dependent_job_id, Duration::from_secs(30))?;

    let predecessor_record = read_job_record(&repo, &predecessor_job_id)?;
    assert_eq!(
        predecessor_record.get("status").and_then(Value::as_str),
        Some("succeeded"),
        "predecessor should succeed after retry"
    );
    let dependent_record = read_job_record(&repo, &dependent_job_id)?;
    assert_eq!(
        dependent_record.get("status").and_then(Value::as_str),
        Some("succeeded"),
        "dependent should succeed once predecessor retry passes"
    );
    Ok(())
}

#[test]
fn test_scheduler_retry_merge_recovers_plan_doc_from_history() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&[
        "draft",
        "--name",
        "retry-merge-history",
        "retry merge history spec",
    ])?;
    repo.vizier_output(&["approve", "retry-merge-history", "--yes"])?;
    clean_workdir(&repo)?;

    let merge = repo
        .vizier_cmd_background()
        .args(["--push", "merge", "retry-merge-history", "--yes"])
        .output()?;
    assert!(
        merge.status.success(),
        "scheduling merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let merge_job_id =
        extract_job_id(&String::from_utf8_lossy(&merge.stdout)).ok_or("missing merge job id")?;
    wait_for_job_completion(&repo, &merge_job_id, Duration::from_secs(40))?;

    let failed_record = read_job_record(&repo, &merge_job_id)?;
    let failed_stderr_rel = failed_record
        .get("stderr_path")
        .and_then(Value::as_str)
        .ok_or("missing failed merge stderr path")?;
    let failed_stderr_log = fs::read_to_string(repo.path().join(failed_stderr_rel))?;
    assert_eq!(
        failed_record.get("status").and_then(Value::as_str),
        Some("failed"),
        "merge should fail the first time due to missing push remote:\n{failed_stderr_log}"
    );
    assert!(
        failed_stderr_log.contains("origin") || failed_stderr_log.contains("push"),
        "expected initial merge failure to involve push/origin:\n{failed_stderr_log}"
    );

    let repo_handle = repo.repo();
    let draft_tip = repo_handle
        .find_branch("draft/retry-merge-history", BranchType::Local)?
        .get()
        .peel_to_commit()?;
    assert!(
        draft_tip
            .tree()?
            .get_path(Path::new(
                ".vizier/implementation-plans/retry-merge-history.md"
            ))
            .is_err(),
        "merge prep should remove the plan doc from draft tip before retry"
    );

    let origin_dir = repo.path().join(".vizier/tmp/retry-merge-origin.git");
    fs::create_dir_all(origin_dir.parent().ok_or("origin parent missing")?)?;
    let init_status = Command::new("git")
        .args(["init", "--bare"])
        .arg(&origin_dir)
        .status()?;
    if !init_status.success() {
        return Err(format!(
            "failed to initialize bare origin at {} (status={init_status:?})",
            origin_dir.display()
        )
        .into());
    }
    let origin = origin_dir.to_string_lossy().to_string();
    repo.git(&["remote", "add", "origin", &origin])?;
    repo.git(&["push", "-u", "origin", "master"])?;
    repo.git(&["push", "origin", "draft/retry-merge-history"])?;

    let retry = repo
        .vizier_cmd_background()
        .args(["jobs", "retry", &merge_job_id])
        .output()?;
    assert!(
        retry.status.success(),
        "jobs retry failed: {}",
        String::from_utf8_lossy(&retry.stderr)
    );
    wait_for_job_completion(&repo, &merge_job_id, Duration::from_secs(40))?;

    let retried_record = read_job_record(&repo, &merge_job_id)?;
    let stderr_rel = retried_record
        .get("stderr_path")
        .and_then(Value::as_str)
        .ok_or("missing retry stderr path")?;
    let stderr_log = fs::read_to_string(repo.path().join(stderr_rel))?;
    assert_eq!(
        retried_record.get("status").and_then(Value::as_str),
        Some("succeeded"),
        "merge retry should succeed once push remote is configured:\n{stderr_log}"
    );
    assert!(
        !stderr_log.contains("MissingPlanFile"),
        "retry should not fail with MissingPlanFile when history has the plan doc:\n{stderr_log}"
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

    let (approve_agent_path, approve_gate_path) =
        write_gated_agent(&repo, "gated-approve", "lock-wait-approve.ready")?;
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

    fs::write(&approve_gate_path, "release\n")?;
    wait_for_job_completion(&repo, &approve_job_id, Duration::from_secs(30))?;
    wait_for_job_completion(&repo, &review_job_id, Duration::from_secs(30))?;
    Ok(())
}

#[test]
fn test_scheduler_pinned_head_mismatch_waits() -> TestResult {
    let repo = IntegrationRepo::new_without_mock()?;
    let (agent_path, gate_path) =
        write_gated_agent(&repo, "gated-ask", "pinned-mismatch-first.ready")?;
    let config_path = write_agent_config(&repo, "config-sleepy-ask.toml", "save", &agent_path)?;

    let first = repo
        .vizier_cmd_background_with_config(&config_path)
        .args(["save"])
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
        .args(["save"])
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

    fs::write(&gate_path, "release\n")?;
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
fn test_scheduler_pinned_head_mismatch_resolves_after_reset() -> TestResult {
    let repo = IntegrationRepo::new_without_mock()?;
    let (ask_agent_path, ask_gate_path) =
        write_gated_agent(&repo, "gated-ask", "pinned-resolve-first.ready")?;
    let draft_agent_path = write_sleeping_agent(&repo, "fast-draft", 0)?;

    let config_path = repo.path().join(".vizier/tmp/pinned-head-resolve.toml");
    fs::create_dir_all(config_path.parent().unwrap())?;
    let ask_agent = ask_agent_path.to_string_lossy().replace('\\', "\\\\");
    let draft_agent = draft_agent_path.to_string_lossy().replace('\\', "\\\\");
    fs::write(
        &config_path,
        format!(
            "[agents.save.agent]\nlabel = \"sleepy-ask\"\ncommand = [\"{ask_agent}\"]\n\n[agents.draft.agent]\nlabel = \"fast-draft\"\ncommand = [\"{draft_agent}\"]\n"
        ),
    )?;

    let first = repo
        .vizier_cmd_background_with_config(&config_path)
        .args(["save"])
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
        .args(["save"])
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

    fs::write(&ask_gate_path, "release\n")?;
    wait_for_job_completion(&repo, &first_job_id, Duration::from_secs(30))?;
    wait_for_job_status(
        &repo,
        &second_job_id,
        "waiting_on_deps",
        Duration::from_secs(10),
    )?;

    let second_record = read_job_record(&repo, &second_job_id)?;
    let pinned = second_record
        .get("schedule")
        .and_then(|s| s.get("pinned_head"))
        .ok_or("missing pinned_head")?;
    let branch = pinned
        .get("branch")
        .and_then(Value::as_str)
        .ok_or("missing pinned branch")?;
    let oid = pinned
        .get("oid")
        .and_then(Value::as_str)
        .ok_or("missing pinned oid")?;

    repo.git(&["checkout", branch])?;
    repo.git(&["reset", "--hard", oid])?;

    let draft = repo
        .vizier_cmd_background_with_config(&config_path)
        .args([
            "draft",
            "--name",
            "pinned-resolve",
            "pinned head resolve plan",
        ])
        .output()?;
    assert!(
        draft.status.success(),
        "scheduled draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );
    let draft_stdout = String::from_utf8_lossy(&draft.stdout);
    let draft_job_id = extract_job_id(&draft_stdout).ok_or("expected draft job id")?;

    wait_for_job_status(&repo, &second_job_id, "running", Duration::from_secs(10))?;
    wait_for_job_completion(&repo, &second_job_id, Duration::from_secs(30))?;
    wait_for_job_completion(&repo, &draft_job_id, Duration::from_secs(30))?;
    Ok(())
}

#[test]
fn test_scheduler_background_ask_applies_single_commit() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;

    let output = repo.vizier_cmd_background().args(["save"]).output()?;
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
    let (agent_path, gate_path) =
        write_gated_agent(&repo, "gated-ask-mismatch", "pinned-fail.ready")?;
    let config_path = write_agent_config(&repo, "config-sleepy-ask.toml", "save", &agent_path)?;

    let output = repo
        .vizier_cmd_background_with_config(&config_path)
        .args(["save"])
        .output()?;
    assert!(
        output.status.success(),
        "scheduled ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let job_id = extract_job_id(&stdout).ok_or("expected ask job id")?;
    wait_for_job_status(&repo, &job_id, "running", Duration::from_secs(10))?;

    repo.write("a", "pinned head mismatch\n")?;
    repo.git(&["add", "a"])?;
    repo.git(&["commit", "-m", "pinned head mismatch"])?;

    fs::write(&gate_path, "release\n")?;
    wait_for_job_completion(&repo, &job_id, Duration::from_secs(30))?;
    let record = read_job_record(&repo, &job_id)?;
    let status = record.get("status").and_then(Value::as_str).unwrap_or("");
    assert_eq!(
        status, "failed",
        "expected ask to fail on pinned head mismatch"
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
