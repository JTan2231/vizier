use crate::fixtures::*;

#[test]
fn test_approve_cancel_exits_nonzero() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let slug = "cancel-approve";
    let draft = repo
        .vizier_cmd()
        .args(["draft", "--name", slug, "approve cancel prompt"])
        .output()?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    let mut approve_cmd = repo.vizier_cmd();
    approve_cmd.args(["approve", slug]);
    approve_cmd.stdin(Stdio::piped());
    approve_cmd.stdout(Stdio::piped());
    approve_cmd.stderr(Stdio::piped());
    let mut child = approve_cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(b"n\n")?;
    }
    let output = child.wait_with_output()?;
    assert!(
        !output.status.success(),
        "expected approve cancellation to exit non-zero"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Approval cancelled"),
        "expected approval cancellation message:\n{stdout}"
    );
    Ok(())
}
#[test]
fn test_approve_merges_plan() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let draft = repo.vizier_output(&[
        "draft",
        "--name",
        "approve-smoke",
        "approval smoke test spec",
    ])?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    let list_before = repo.vizier_output(&["list"])?;
    assert!(
        list_before.status.success(),
        "vizier list failed: {}",
        String::from_utf8_lossy(&list_before.stderr)
    );
    let stdout_before = String::from_utf8_lossy(&list_before.stdout);
    assert!(
        stdout_before.contains("Plan   : approve-smoke"),
        "pending plans missing approve-smoke: {}",
        stdout_before
    );
    assert!(
        stdout_before.contains("Branch : draft/approve-smoke"),
        "pending plans missing branch detail: {}",
        stdout_before
    );

    clean_workdir(&repo)?;

    {
        let repo_handle = repo.repo();
        let mut checkout = CheckoutBuilder::new();
        checkout.force();
        repo_handle.checkout_head(Some(&mut checkout))?;
    }

    let approve = repo.vizier_output(&["approve", "approve-smoke", "--yes"])?;
    assert!(
        approve.status.success(),
        "vizier approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );
    let approve_stderr = String::from_utf8_lossy(&approve.stderr);
    assert!(
        approve_stderr.contains("[codex:approve] agent — mock agent running"),
        "Agent progress log missing expected line: {}",
        approve_stderr
    );

    let repo_handle = repo.repo();
    let branch = repo_handle
        .find_branch("draft/approve-smoke", BranchType::Local)
        .expect("draft branch exists after approval");
    let merge_commit = branch.get().peel_to_commit()?;
    let tree = merge_commit.tree()?;
    let entry = tree.get_path(Path::new(".vizier/implementation-plans/approve-smoke.md"))?;
    let blob = repo_handle.find_blob(entry.id())?;
    let contents = std::str::from_utf8(blob.content())?;
    assert!(
        contents.contains("approve-smoke"),
        "plan document missing slug content"
    );

    Ok(())
}
#[test]
fn test_approve_creates_single_combined_commit() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "single-commit-approve", "spec"])?;

    let repo_handle = repo.repo();
    let draft_branch = repo_handle.find_branch("draft/single-commit-approve", BranchType::Local)?;
    let before_commit = draft_branch.get().peel_to_commit()?.id();

    clean_workdir(&repo)?;
    let approve = repo.vizier_output(&["approve", "single-commit-approve", "--yes"])?;
    assert!(
        approve.status.success(),
        "vizier approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );

    let repo_handle = repo.repo();
    let branch = repo_handle.find_branch("draft/single-commit-approve", BranchType::Local)?;
    let commit = branch.get().peel_to_commit()?;
    assert_eq!(
        commit.parent(0)?.id(),
        before_commit,
        "approve should add exactly one commit"
    );

    let files = files_changed_in_commit(&repo_handle, &commit.id().to_string())?;
    assert!(
        files.contains(".vizier/narrative/snapshot.md")
            && files.contains(".vizier/narrative/glossary.md")
            && files.contains("a"),
        "approve commit should include code and narrative assets, got {files:?}"
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
fn test_cli_backend_override_rejected_for_approve() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo
        .vizier_cmd()
        .args(["--backend", "codex", "approve", "example"])
        .output()?;
    assert!(
        !output.status.success(),
        "vizier should reject deprecated --backend flag"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    assert!(
        stderr.contains("--backend") && stderr.contains("unexpected"),
        "stderr should mention the rejected --backend flag, got: {stderr}"
    );
    Ok(())
}
#[test]
fn test_approve_requires_plan_slug() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo.vizier_cmd().args(["approve"]).output()?;
    assert!(
        !output.status.success(),
        "vizier approve should fail without a plan slug"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    assert!(
        stderr.contains("plan") && stderr.contains("required"),
        "stderr should mention the missing plan argument, got: {stderr}"
    );
    Ok(())
}
#[test]
fn test_approve_list_flag_rejected() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo.vizier_cmd().args(["approve", "--list"]).output()?;
    assert!(
        !output.status.success(),
        "vizier approve --list should be rejected"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    assert!(
        stderr.contains("--list") && stderr.contains("unexpected"),
        "stderr should mention the rejected --list flag, got: {stderr}"
    );
    Ok(())
}
#[test]
fn test_approve_fails_when_codex_errors() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let draft = repo
        .vizier_cmd()
        .args(["draft", "--name", "codex-approve", "spec"])
        .output()?;
    assert!(
        draft.status.success(),
        "vizier draft failed unexpectedly: {}",
        String::from_utf8_lossy(&draft.stderr)
    );
    let repo_handle = repo.repo();
    let before_commit = repo_handle
        .find_branch("draft/codex-approve", BranchType::Local)?
        .get()
        .peel_to_commit()?;

    let mut approve = repo.vizier_cmd();
    approve.env("VIZIER_FORCE_AGENT_ERROR", "1");
    approve.args(["approve", "codex-approve", "--yes"]);
    let output = approve.output()?;
    assert!(
        !output.status.success(),
        "vizier approve should fail when the backend errors"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("agent backend"),
        "stderr should mention backend error, got: {stderr}"
    );

    let repo_handle = repo.repo();
    let after_commit = repo_handle
        .find_branch("draft/codex-approve", BranchType::Local)?
        .get()
        .peel_to_commit()?;
    assert_eq!(
        before_commit.id(),
        after_commit.id(),
        "backend failure should not add commits to the plan branch"
    );
    Ok(())
}
#[test]
fn test_approve_stop_condition_passes_on_first_attempt() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "stop-pass", "stop condition pass spec"])?;
    clean_workdir(&repo)?;

    let log_path = repo.path().join("approve-stop-pass.log");
    let script_path = write_cicd_script(
        &repo,
        "approve-stop-pass.sh",
        &format!(
            "#!/bin/sh\nset -eu\necho \"stop-called\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    )?;
    let script_flag = script_path.to_string_lossy().to_string();

    let before_logs = gather_session_logs(&repo)?;
    let approve = repo.vizier_output(&[
        "approve",
        "stop-pass",
        "--yes",
        "--stop-condition-script",
        &script_flag,
    ])?;
    assert!(
        approve.status.success(),
        "vizier approve with passing stop-condition should succeed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );

    assert!(
        log_path.exists(),
        "stop-condition script should run at least once"
    );
    let contents = fs::read_to_string(&log_path)?;
    let lines: Vec<_> = contents.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "stop-condition script should run exactly once when it passes on the first attempt, got {} lines",
        lines.len()
    );

    let after_logs = gather_session_logs(&repo)?;
    let new_log = new_session_log(&before_logs, &after_logs)
        .ok_or_else(|| "expected vizier approve to create a session log".to_string())?;
    let contents = fs::read_to_string(new_log)?;
    let json: Value = serde_json::from_str(&contents)?;
    let operations = json
        .get("operations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let attempt_ops: Vec<_> = operations
        .iter()
        .filter(|entry| {
            entry
                .get("kind")
                .and_then(Value::as_str)
                .map(|kind| kind == "approve_stop_condition_attempt")
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(
        attempt_ops.len(),
        1,
        "expected exactly one stop-condition attempt record"
    );
    let attempt_details = attempt_ops[0]
        .get("details")
        .and_then(Value::as_object)
        .ok_or_else(|| "approve_stop_condition_attempt missing details".to_string())?;
    assert_eq!(
        attempt_details.get("attempt").and_then(Value::as_u64),
        Some(1),
        "attempt record should mark the first run"
    );
    assert_eq!(
        attempt_details.get("status").and_then(Value::as_str),
        Some("passed"),
        "attempt record should show passed status: {:?}",
        attempt_details
    );
    let stop_op = operations
        .iter()
        .find(|entry| entry.get("kind").and_then(Value::as_str) == Some("approve_stop_condition"))
        .cloned()
        .ok_or_else(|| "expected approve_stop_condition operation in session log".to_string())?;
    let details = stop_op
        .get("details")
        .and_then(Value::as_object)
        .ok_or_else(|| "approve_stop_condition operation missing details".to_string())?;
    assert_eq!(
        details.get("status").and_then(Value::as_str),
        Some("passed"),
        "stop-condition status should be passed: {details:?}"
    );
    assert_eq!(
        details.get("attempts").and_then(Value::as_u64),
        Some(1),
        "stop-condition attempts should be 1 when it passes on the first run: {details:?}"
    );
    Ok(())
}
#[test]
fn test_approve_stop_condition_retries_then_passes() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "stop-retry", "stop condition retry spec"])?;
    clean_workdir(&repo)?;

    let counter_path = repo.path().join("approve-stop-count.txt");
    let log_path = repo.path().join("approve-stop-retry.log");
    let script_path = write_cicd_script(
        &repo,
        "approve-stop-retry.sh",
        &format!(
            "#!/bin/sh\nset -eu\nCOUNT_FILE=\"{}\"\nif [ -f \"$COUNT_FILE\" ]; then\n  n=$(cat \"$COUNT_FILE\")\nelse\n  n=0\nfi\nn=$((n+1))\necho \"$n\" > \"$COUNT_FILE\"\necho \"run $n\" >> \"{}\"\nif [ \"$n\" -lt 2 ]; then\n  exit 1\nfi\nexit 0\n",
            counter_path.display(),
            log_path.display()
        ),
    )?;
    let script_flag = script_path.to_string_lossy().to_string();

    let before_logs = gather_session_logs(&repo)?;
    let approve = repo.vizier_output(&[
        "approve",
        "stop-retry",
        "--yes",
        "--stop-condition-script",
        &script_flag,
        "--stop-condition-retries",
        "3",
    ])?;
    assert!(
        approve.status.success(),
        "vizier approve with retrying stop-condition should succeed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );

    let contents = fs::read_to_string(&counter_path)?;
    assert_eq!(
        contents.trim(),
        "2",
        "stop-condition script should have run twice before passing, got counter contents: {contents}"
    );

    let after_logs = gather_session_logs(&repo)?;
    let new_log = new_session_log(&before_logs, &after_logs)
        .ok_or_else(|| "expected vizier approve to create a session log".to_string())?;
    let contents = fs::read_to_string(new_log)?;
    let json: Value = serde_json::from_str(&contents)?;
    let operations = json
        .get("operations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let attempt_ops: Vec<_> = operations
        .iter()
        .filter(|entry| {
            entry
                .get("kind")
                .and_then(Value::as_str)
                .map(|kind| kind == "approve_stop_condition_attempt")
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(
        attempt_ops.len(),
        2,
        "expected two stop-condition attempt records when a retry occurs"
    );
    let attempt_statuses: Vec<_> = attempt_ops
        .iter()
        .filter_map(|entry| {
            entry
                .get("details")
                .and_then(Value::as_object)
                .and_then(|details| details.get("status"))
                .and_then(Value::as_str)
        })
        .collect();
    assert_eq!(
        attempt_statuses,
        vec!["failed", "passed"],
        "attempt records should capture the failed then passed sequence: {:?}",
        attempt_statuses
    );
    let stop_op = operations
        .iter()
        .find(|entry| entry.get("kind").and_then(Value::as_str) == Some("approve_stop_condition"))
        .cloned()
        .ok_or_else(|| "expected approve_stop_condition operation in session log".to_string())?;
    let details = stop_op
        .get("details")
        .and_then(Value::as_object)
        .ok_or_else(|| "approve_stop_condition operation missing details".to_string())?;
    assert_eq!(
        details.get("status").and_then(Value::as_str),
        Some("passed"),
        "stop-condition status should be passed after retries: {details:?}"
    );
    assert_eq!(
        details.get("attempts").and_then(Value::as_u64),
        Some(2),
        "stop-condition attempts should be 2 when it fails once then passes: {details:?}"
    );
    Ok(())
}
#[test]
fn test_approve_stop_condition_exhausts_retries_and_fails() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&[
        "draft",
        "--name",
        "stop-fail",
        "stop condition failure spec",
    ])?;
    clean_workdir(&repo)?;

    let log_path = repo.path().join("approve-stop-fail.log");
    let script_path = write_cicd_script(
        &repo,
        "approve-stop-fail.sh",
        &format!(
            "#!/bin/sh\nset -eu\necho \"fail\" >> \"{}\"\nexit 1\n",
            log_path.display()
        ),
    )?;
    let script_flag = script_path.to_string_lossy().to_string();

    let before_logs = gather_session_logs(&repo)?;
    let approve = repo.vizier_output(&[
        "approve",
        "stop-fail",
        "--yes",
        "--stop-condition-script",
        &script_flag,
        "--stop-condition-retries",
        "2",
    ])?;
    assert!(
        !approve.status.success(),
        "vizier approve should fail when the stop-condition never passes"
    );
    let stderr = String::from_utf8_lossy(&approve.stderr);
    assert!(
        stderr.contains("Plan worktree preserved at"),
        "stderr should mention preserved worktree for failed stop-condition: {stderr}"
    );

    let contents = fs::read_to_string(&log_path)?;
    let attempts = contents.lines().count();
    assert!(
        attempts >= 3,
        "stop-condition script should run at least three times when retries are exhausted (saw {attempts} runs)"
    );

    let after_logs = gather_session_logs(&repo)?;
    let new_log = new_session_log(&before_logs, &after_logs)
        .ok_or_else(|| "expected vizier approve to create a session log".to_string())?;
    let contents = fs::read_to_string(new_log)?;
    let json: Value = serde_json::from_str(&contents)?;
    let operations = json
        .get("operations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let attempt_ops: Vec<_> = operations
        .iter()
        .filter(|entry| {
            entry
                .get("kind")
                .and_then(Value::as_str)
                .map(|kind| kind == "approve_stop_condition_attempt")
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(
        attempt_ops.len(),
        3,
        "expected three stop-condition attempt records when retries are exhausted"
    );
    assert!(
        attempt_ops.iter().all(|entry| {
            entry
                .get("details")
                .and_then(Value::as_object)
                .and_then(|details| details.get("status"))
                .and_then(Value::as_str)
                == Some("failed")
        }),
        "all attempt records should be failed when the stop condition never passes: {:?}",
        attempt_ops
    );
    let stop_op = operations
        .iter()
        .find(|entry| entry.get("kind").and_then(Value::as_str) == Some("approve_stop_condition"))
        .cloned()
        .ok_or_else(|| "expected approve_stop_condition operation in session log".to_string())?;
    let details = stop_op
        .get("details")
        .and_then(Value::as_object)
        .ok_or_else(|| "approve_stop_condition operation missing details".to_string())?;
    assert_eq!(
        details.get("status").and_then(Value::as_str),
        Some("failed"),
        "stop-condition status should be failed when retries are exhausted: {details:?}"
    );
    assert_eq!(
        details.get("attempts").and_then(Value::as_u64),
        Some(3),
        "stop-condition attempts should be 3 when retries=2 and the script never passes: {details:?}"
    );
    Ok(())
}
