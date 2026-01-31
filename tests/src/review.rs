use crate::fixtures::*;

#[test]
fn test_review_streams_critique() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let draft = repo.vizier_output(&["draft", "--name", "review-smoke", "review smoke spec"])?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    clean_workdir(&repo)?;

    let approve = repo.vizier_output(&["approve", "review-smoke", "--yes"])?;
    assert!(
        approve.status.success(),
        "vizier approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );

    clean_workdir(&repo)?;
    let repo_handle = repo.repo();
    let branch_before = repo_handle.find_branch("draft/review-smoke", BranchType::Local)?;
    let before_commit = branch_before.get().peel_to_commit()?.id();

    let review =
        repo.vizier_output(&["review", "review-smoke", "--review-only", "--skip-checks"])?;
    assert!(
        review.status.success(),
        "vizier review failed: {}",
        String::from_utf8_lossy(&review.stderr)
    );

    let stdout = String::from_utf8_lossy(&review.stdout);
    assert!(
        stdout.contains("--- Review critique for plan review-smoke ---"),
        "review output should stream the critique header but was:\n{}",
        stdout
    );

    let branch = repo_handle.find_branch("draft/review-smoke", BranchType::Local)?;
    let commit = branch.get().peel_to_commit()?;
    assert_eq!(
        commit.parent(0)?.id(),
        before_commit,
        "review should add exactly one commit"
    );
    let tree = commit.tree()?;
    assert!(
        tree.get_path(Path::new(".vizier/reviews/review-smoke.md"))
            .is_err(),
        "review artifacts should not be committed to the plan branch"
    );

    assert!(
        !repo.path().join(".vizier/reviews/review-smoke.md").exists(),
        "review directory should not exist after streaming critiques"
    );

    assert!(
        !repo
            .path()
            .join(".vizier/implementation-plans/review-smoke.md")
            .exists(),
        "plan document should remain confined to the draft branch"
    );

    let files = files_changed_in_commit(&repo_handle, &commit.id().to_string())?;
    assert!(
        files.contains(".vizier/narrative/snapshot.md")
            && files.contains(".vizier/narrative/glossary.md"),
        "critique commit should include narrative assets, got {files:?}"
    );
    assert!(
        !files
            .iter()
            .any(|path| path.contains("implementation-plans")),
        "plan documents should remain scratch, got {files:?}"
    );

    Ok(())
}
#[test]
fn test_review_writes_markdown_file() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let draft = repo.vizier_output(&["draft", "--name", "review-file", "review file spec"])?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    clean_workdir(&repo)?;
    let approve = repo.vizier_output(&["approve", "review-file", "--yes"])?;
    assert!(
        approve.status.success(),
        "vizier approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );

    clean_workdir(&repo)?;
    let review =
        repo.vizier_output(&["review", "review-file", "--review-file", "--skip-checks"])?;
    assert!(
        review.status.success(),
        "vizier review failed: {}",
        String::from_utf8_lossy(&review.stderr)
    );

    let review_path = repo.path().join("vizier-review.md");
    assert!(
        review_path.exists(),
        "expected vizier-review.md at repo root"
    );
    let contents = fs::read_to_string(&review_path)?;
    assert!(
        contents.contains("Review critique for plan review-file"),
        "review file should include the plan header, got:\n{contents}"
    );
    assert!(
        contents.contains("mock agent response"),
        "review file should include critique text, got:\n{contents}"
    );
    Ok(())
}
#[test]
fn test_review_summary_includes_token_suffix() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let draft = repo.vizier_output(&["draft", "--name", "token-suffix", "suffix spec"])?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    clean_workdir(&repo)?;
    let approve = repo.vizier_output(&["approve", "token-suffix", "--yes"])?;
    assert!(
        approve.status.success(),
        "vizier approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );

    clean_workdir(&repo)?;
    let review =
        repo.vizier_output(&["review", "token-suffix", "--review-only", "--skip-checks"])?;
    assert!(
        review.status.success(),
        "vizier review failed: {}",
        String::from_utf8_lossy(&review.stderr)
    );

    let stdout = String::from_utf8_lossy(&review.stdout);
    assert!(
        stdout.contains("Agent") && stdout.contains("codex"),
        "review summary should include agent details but was:\n{stdout}"
    );
    assert!(
        stdout.contains("Exit code"),
        "review summary should include agent exit code:\n{stdout}"
    );
    assert!(
        stdout.contains("mock agent response"),
        "review summary should surface the critique text:\n{stdout}"
    );
    Ok(())
}
#[test]
fn test_review_runs_cicd_gate_before_critique() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "review-gate-pass", "gate pass spec"])?;
    repo.vizier_output(&["approve", "review-gate-pass", "--yes"])?;
    clean_workdir(&repo)?;

    let gate_log = repo.path().join("review-gate.log");
    let script_path = write_cicd_script(
        &repo,
        "review-gate-pass.sh",
        &format!(
            "#!/bin/sh\nset -eu\necho \"gate ran\" > \"{}\"\n",
            gate_log.display()
        ),
    )?;
    let script_flag = script_path.to_string_lossy().to_string();
    let review = repo.vizier_output(&[
        "review",
        "review-gate-pass",
        "--review-only",
        "--skip-checks",
        "--cicd-script",
        &script_flag,
    ])?;
    assert!(
        review.status.success(),
        "vizier review failed: {}",
        String::from_utf8_lossy(&review.stderr)
    );

    assert!(
        gate_log.exists(),
        "CI/CD gate script should run before the critique"
    );

    let stdout = String::from_utf8_lossy(&review.stdout);
    assert!(
        stdout.contains("CI/CD gate") && stdout.contains("passed"),
        "review summary should report the passed CI/CD gate:\n{stdout}"
    );

    let contents = session_log_contents_from_output(&repo, &stdout)?;
    let json: Value = serde_json::from_str(&contents)?;
    let operations = json
        .get("operations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        operations.iter().any(|entry| {
            entry.get("kind").and_then(Value::as_str) == Some("cicd_gate")
                && entry
                    .get("details")
                    .and_then(|details| details.get("status"))
                    .and_then(Value::as_str)
                    == Some("passed")
        }),
        "session log should capture a passed CI/CD gate operation: {operations:?}"
    );

    Ok(())
}
#[test]
fn test_review_surfaces_failed_cicd_gate_and_continues() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "review-gate-fail", "gate fail spec"])?;
    repo.vizier_output(&["approve", "review-gate-fail", "--yes"])?;
    clean_workdir(&repo)?;

    let gate_log = repo.path().join("review-gate-fail.log");
    let script_path = write_cicd_script(
        &repo,
        "review-gate-fail.sh",
        &format!(
            "#!/bin/sh\nset -eu\necho \"broken gate\" > \"{}\"\nexit 1\n",
            gate_log.display()
        ),
    )?;
    let script_flag = script_path.to_string_lossy().to_string();
    let review = repo.vizier_output(&[
        "review",
        "review-gate-fail",
        "--review-only",
        "--skip-checks",
        "--cicd-script",
        &script_flag,
    ])?;
    assert!(
        review.status.success(),
        "vizier review should continue even when the gate fails: {}",
        String::from_utf8_lossy(&review.stderr)
    );

    let stdout = String::from_utf8_lossy(&review.stdout);
    assert!(
        stdout.contains("CI/CD gate") && stdout.contains("failed"),
        "review summary should report the failed CI/CD gate:\n{stdout}"
    );
    assert!(
        stdout.contains("--- Review critique for plan review-gate-fail ---"),
        "critique should still stream when the gate fails:\n{stdout}"
    );

    assert!(
        gate_log.exists(),
        "failed CI/CD gate should still run before the critique"
    );

    let contents = session_log_contents_from_output(&repo, &stdout)?;
    let json: Value = serde_json::from_str(&contents)?;
    let operations = json
        .get("operations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        operations.iter().any(|entry| {
            entry.get("kind").and_then(Value::as_str) == Some("cicd_gate")
                && entry
                    .get("details")
                    .and_then(|details| details.get("status"))
                    .and_then(Value::as_str)
                    == Some("failed")
        }),
        "session log should capture a failed CI/CD gate operation: {operations:?}"
    );
    assert!(
        operations.iter().any(|entry| {
            entry
                .get("details")
                .and_then(|details| details.get("exit_code"))
                .and_then(Value::as_i64)
                == Some(1)
        }),
        "failed gate operation should record exit code 1: {operations:?}"
    );

    Ok(())
}
