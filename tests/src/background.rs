use crate::fixtures::*;

#[test]
fn test_background_default_spawns_job() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo
        .vizier_cmd_background()
        .args(["ask", "background default"])
        .output()?;
    assert!(
        output.status.success(),
        "background ask failed: {}",
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
        "expected background job record at {}",
        job_path.display()
    );
    wait_for_job_completion(&repo, &job_id, Duration::from_secs(20))?;
    Ok(())
}
#[test]
fn test_background_default_is_not_quiet() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo
        .vizier_cmd_background()
        .args(["ask", "background stderr"])
        .output()?;
    assert!(
        output.status.success(),
        "background ask failed: {}",
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
        "expected background stderr log to include progress output when quiet is not injected:\n{stderr_log}"
    );
    Ok(())
}
#[test]
fn test_background_follow_streams_logs() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo
        .vizier_cmd_background()
        .args(["--follow", "ask", "background follow"])
        .output()?;
    assert!(
        output.status.success(),
        "background follow ask failed: {}",
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
fn test_background_stdin_forces_foreground() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let mut cmd = repo.vizier_cmd_background();
    cmd.args(["ask"]);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(b"background stdin prompt\n")?;
    }
    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "stdin ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("Background job started"),
        "stdin ask should run in the foreground when backgrounding by default:\n{stdout}"
    );
    assert!(
        stdout.contains("Agent run:"),
        "stdin ask should include the agent summary:\n{stdout}"
    );
    Ok(())
}
#[test]
fn test_background_rejects_json_without_no_background() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let blocked = repo
        .vizier_cmd_background()
        .args(["ask", "json background", "--json"])
        .output()?;
    assert!(
        !blocked.status.success(),
        "expected ask --json to fail when backgrounding by default"
    );
    let stderr = String::from_utf8_lossy(&blocked.stderr);
    assert!(
        stderr.contains("--json cannot be used with background execution"),
        "unexpected stderr for --json background error:\n{stderr}"
    );

    let allowed = repo
        .vizier_cmd()
        .args(["ask", "json foreground", "--json"])
        .output()?;
    assert!(
        allowed.status.success(),
        "ask --json with --no-background should succeed: {}",
        String::from_utf8_lossy(&allowed.stderr)
    );
    Ok(())
}
#[test]
fn test_explicit_background_requires_noninteractive_flags() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let cases = [
        (
            vec!["approve", "--background", "plan-a"],
            "--background for vizier approve requires --yes",
        ),
        (
            vec!["merge", "--background", "plan-b"],
            "--background for vizier merge requires --yes",
        ),
        (
            vec!["review", "--background", "plan-c"],
            "--background for vizier review requires --yes",
        ),
    ];

    for (args, expected) in cases {
        let output = repo.vizier_cmd_background().args(&args).output()?;
        assert!(
            !output.status.success(),
            "expected background safety gate to fail for {:?}",
            args
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(expected),
            "missing safety gate message in stderr:\n{stderr}"
        );
    }
    Ok(())
}
#[test]
fn test_background_prompts_for_approve() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let slug = "bg-approve";
    let draft = repo
        .vizier_cmd()
        .args(["draft", "--name", slug, "approve background prompt"])
        .output()?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    let mut approve_cmd = repo.vizier_cmd_background();
    approve_cmd.args(["approve", slug]);
    approve_cmd.stdin(Stdio::piped());
    approve_cmd.stdout(Stdio::piped());
    approve_cmd.stderr(Stdio::piped());
    let mut child = approve_cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(b"y\n")?;
    }
    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "background approve failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Implement plan now? [y/N]"),
        "expected approve prompt in stdout:\n{stdout}"
    );
    let job_id = extract_job_id(&stdout).ok_or("expected job id from approve")?;
    wait_for_job_completion(&repo, &job_id, Duration::from_secs(30))?;
    Ok(())
}
#[test]
fn test_background_prompts_for_review() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let slug = "bg-review";
    let draft = repo
        .vizier_cmd()
        .args(["draft", "--name", slug, "review background prompt"])
        .output()?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    let mut review_cmd = repo.vizier_cmd_background();
    review_cmd.args(["review", slug, "--skip-checks"]);
    review_cmd.stdin(Stdio::piped());
    review_cmd.stdout(Stdio::piped());
    review_cmd.stderr(Stdio::piped());
    let mut child = review_cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(b"n\n")?;
    }
    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "background review failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Apply suggested fixes"),
        "expected review prompt in stdout:\n{stdout}"
    );
    let job_id = extract_job_id(&stdout).ok_or("expected job id from review")?;
    wait_for_job_completion(&repo, &job_id, Duration::from_secs(40))?;
    Ok(())
}
#[test]
fn test_background_prompts_for_merge() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let slug = "bg-merge";
    let draft = repo
        .vizier_cmd()
        .args(["draft", "--name", slug, "merge background prompt"])
        .output()?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    let mut merge_cmd = repo.vizier_cmd_background();
    merge_cmd.args(["merge", slug]);
    merge_cmd.stdin(Stdio::piped());
    merge_cmd.stdout(Stdio::piped());
    merge_cmd.stderr(Stdio::piped());
    let mut child = merge_cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(b"y\n")?;
    }
    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "background merge failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Merge this plan? [y/N]"),
        "expected merge prompt in stdout:\n{stdout}"
    );
    let job_id = extract_job_id(&stdout).ok_or("expected job id from merge")?;
    wait_for_job_completion(&repo, &job_id, Duration::from_secs(40))?;
    Ok(())
}
#[test]
fn test_background_disabled_forces_foreground() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let config_path = repo.path().join(".vizier/tmp/background-off.toml");
    fs::create_dir_all(config_path.parent().unwrap())?;
    fs::write(
        &config_path,
        r#"
[workflow.background]
enabled = false
"#,
    )?;

    let output = repo
        .vizier_cmd_background()
        .args([
            "--config-file",
            config_path.to_str().unwrap(),
            "ask",
            "foreground fallback",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "foreground ask failed with background disabled: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("Background job started"),
        "background should be disabled but stdout was:\n{stdout}"
    );

    let background = repo
        .vizier_cmd_background()
        .args([
            "--config-file",
            config_path.to_str().unwrap(),
            "--background",
            "ask",
            "should fail",
        ])
        .output()?;
    assert!(
        !background.status.success(),
        "expected --background to fail when disabled"
    );
    let stderr = String::from_utf8_lossy(&background.stderr);
    assert!(
        stderr.contains("background execution disabled"),
        "unexpected stderr for disabled background:\n{stderr}"
    );

    let follow = repo
        .vizier_cmd_background()
        .args([
            "--config-file",
            config_path.to_str().unwrap(),
            "--follow",
            "ask",
            "should fail",
        ])
        .output()?;
    assert!(
        !follow.status.success(),
        "expected --follow to fail when disabled"
    );
    let stderr = String::from_utf8_lossy(&follow.stderr);
    assert!(
        stderr.contains("background execution disabled"),
        "unexpected stderr for disabled follow:\n{stderr}"
    );
    Ok(())
}
