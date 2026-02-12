use crate::fixtures::*;
use std::collections::HashMap;

fn write_narrative_only_approve_agent(repo: &IntegrationRepo, name: &str) -> TestResult<PathBuf> {
    let script_dir = repo.path().join(".vizier/tmp/bin");
    fs::create_dir_all(&script_dir)?;
    let script_path = script_dir.join(format!("{name}.sh"));
    fs::write(
        &script_path,
        "#!/bin/sh\nset -eu\ncat >/dev/null\nmkdir -p .vizier/narrative/threads\nprintf '%s\\n' 'staged snapshot update' > .vizier/narrative/snapshot.md\nprintf '%s\\n' 'staged glossary update' > .vizier/narrative/glossary.md\nprintf '%s\\n' 'staged thread update' > .vizier/narrative/threads/approve-staged-only.md\nprintf '%s\\n' 'noise = true' > .vizier/config.toml\ngit add .vizier/narrative/snapshot.md .vizier/narrative/glossary.md .vizier/narrative/threads/approve-staged-only.md .vizier/config.toml\nprintf '%s\\n' 'staged narrative-only approve update'\n",
    )?;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&script_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms)?;
    }
    Ok(script_path)
}

fn load_job_records(repo: &IntegrationRepo) -> TestResult<Vec<Value>> {
    let jobs_dir = repo.path().join(".vizier/jobs");
    let mut records = Vec::new();
    if !jobs_dir.is_dir() {
        return Ok(records);
    }
    for entry in fs::read_dir(jobs_dir)? {
        let entry = entry?;
        let path = entry.path().join("job.json");
        if !path.is_file() {
            continue;
        }
        let record: Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
        records.push(record);
    }
    Ok(records)
}

fn workflow_node_jobs_for_plan(
    records: &[Value],
    template_id: &str,
    plan: &str,
) -> HashMap<String, String> {
    records
        .iter()
        .filter(|record| {
            record
                .pointer("/metadata/workflow_template_id")
                .and_then(Value::as_str)
                == Some(template_id)
                && record.pointer("/metadata/plan").and_then(Value::as_str) == Some(plan)
        })
        .filter_map(|record| {
            let node = record
                .pointer("/metadata/workflow_node_id")
                .and_then(Value::as_str)?;
            let job = record.get("id").and_then(Value::as_str)?;
            Some((node.to_string(), job.to_string()))
        })
        .collect::<HashMap<_, _>>()
}

#[test]
fn test_approve_requires_yes() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo
        .vizier_cmd_background()
        .args(["approve", "missing-plan"])
        .output()?;
    assert!(
        !output.status.success(),
        "expected approve without --yes to fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("requires --yes"),
        "expected scheduler guard to mention --yes requirement:
{stderr}"
    );
    Ok(())
}

#[test]
fn test_scheduled_approve_records_workflow_template_metadata() -> TestResult {
    let repo = IntegrationRepo::new()?;
    schedule_job_and_expect_status(
        &repo,
        &[
            "draft",
            "--name",
            "approve-template-meta",
            "approve template metadata",
        ],
        "succeeded",
        Duration::from_secs(40),
    )?;

    let stop_script = write_cicd_script(&repo, "approve-stop-pass.sh", "#!/bin/sh\nset -eu\n")?;
    let stop_script_flag = stop_script.to_string_lossy().to_string();
    let (_output, record) = schedule_job_and_wait(
        &repo,
        &[
            "approve",
            "approve-template-meta",
            "--yes",
            "--stop-condition-script",
            &stop_script_flag,
            "--stop-condition-retries",
            "2",
        ],
        Duration::from_secs(40),
    )?;

    assert_eq!(
        record.get("status").and_then(Value::as_str),
        Some("succeeded"),
        "scheduled approve should succeed: {record}"
    );
    assert_eq!(
        record
            .pointer("/metadata/workflow_template_id")
            .and_then(Value::as_str),
        Some("template.approve"),
        "approve jobs should persist workflow template id"
    );
    assert_eq!(
        record
            .pointer("/metadata/workflow_template_version")
            .and_then(Value::as_str),
        Some("v1"),
        "approve jobs should persist workflow template version"
    );
    assert_eq!(
        record
            .pointer("/metadata/workflow_node_id")
            .and_then(Value::as_str),
        Some("approve_apply_once"),
        "approve jobs should persist workflow node id"
    );
    assert_eq!(
        record
            .pointer("/metadata/workflow_capability_id")
            .and_then(Value::as_str),
        Some("cap.plan.apply_once"),
        "approve jobs should persist workflow capability id"
    );
    let hash = record
        .pointer("/metadata/workflow_policy_snapshot_hash")
        .and_then(Value::as_str)
        .ok_or("approve workflow policy snapshot hash missing")?;
    assert_eq!(
        hash.len(),
        64,
        "approve workflow hash should be a sha256 hex string: {hash}"
    );
    let gate_labels = record
        .pointer("/metadata/workflow_gates")
        .and_then(Value::as_array)
        .ok_or("approve workflow gates missing")?
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    assert!(
        gate_labels.iter().any(|label| label.contains("approval(")),
        "approve workflow gates should include approval gate: {gate_labels:?}"
    );
    assert!(
        gate_labels.iter().any(|label| label.contains("script(")),
        "approve workflow gates should include stop-condition script gate: {gate_labels:?}"
    );
    Ok(())
}

#[test]
fn test_scheduled_approve_builtin_template_enqueues_control_node_jobs() -> TestResult {
    let repo = IntegrationRepo::new()?;
    schedule_job_and_expect_status(
        &repo,
        &[
            "draft",
            "--name",
            "approve-control-nodes",
            "approve control node scheduling",
        ],
        "succeeded",
        Duration::from_secs(40),
    )?;

    let stop_script = write_cicd_script(&repo, "approve-control-pass.sh", "#!/bin/sh\nset -eu\n")?;
    let stop_script_flag = stop_script.to_string_lossy().to_string();
    let (_output, root_record) = schedule_job_and_wait(
        &repo,
        &[
            "approve",
            "approve-control-nodes",
            "--yes",
            "--stop-condition-script",
            &stop_script_flag,
            "--stop-condition-retries",
            "2",
        ],
        Duration::from_secs(50),
    )?;

    let root_job_id = root_record
        .get("id")
        .and_then(Value::as_str)
        .ok_or("approve root job id missing")?
        .to_string();
    let expected_nodes = ["approve_apply_once", "approve_gate_stop_condition"];
    let mut node_jobs = HashMap::new();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(80) {
        let records = load_job_records(&repo)?;
        node_jobs =
            workflow_node_jobs_for_plan(&records, "template.approve", "approve-control-nodes");
        if expected_nodes
            .iter()
            .all(|node| node_jobs.contains_key(*node))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        expected_nodes
            .iter()
            .all(|node| node_jobs.contains_key(*node)),
        "expected approve control nodes to be queued, found {:?}",
        node_jobs
    );
    assert_eq!(
        node_jobs.get("approve_apply_once"),
        Some(&root_job_id),
        "root approve job should still bind to approve_apply_once"
    );

    for node in expected_nodes {
        let job_id = node_jobs
            .get(node)
            .ok_or_else(|| format!("missing job id for node {node}"))?;
        wait_for_job_status(&repo, job_id, "succeeded", Duration::from_secs(80))?;
    }
    Ok(())
}

#[test]
fn test_scheduled_approve_custom_template_queues_full_graph_with_semantic_primary_node()
-> TestResult {
    let repo = IntegrationRepo::new()?;
    schedule_job_and_expect_status(
        &repo,
        &[
            "draft",
            "--name",
            "approve-custom-template",
            "approve custom template graph",
        ],
        "succeeded",
        Duration::from_secs(40),
    )?;

    repo.write(
        ".vizier/workflow/custom.approve@v1.json",
        r#"{
  "id": "custom.approve",
  "version": "v1",
  "policy": {
    "resume": {
      "key": "custom-approve",
      "reuse_mode": "strict"
    }
  },
  "artifact_contracts": [
    { "id": "plan_doc", "version": "v1" },
    { "id": "plan_commits", "version": "v1" }
  ],
  "nodes": [
    {
      "id": "custom_prepare",
      "kind": "shell",
      "uses": "acme.prepare",
      "args": {
        "command": "mkdir -p .vizier/tmp/custom-approve && printf '%s' '${slug}' > .vizier/tmp/custom-approve/${slug}.txt"
      }
    },
    {
      "id": "custom_apply",
      "kind": "builtin",
      "uses": "vizier.approve.apply_once",
      "after": [
        { "node_id": "custom_prepare", "policy": "success" }
      ],
      "needs": [
        { "plan_doc": { "slug": "${slug}", "branch": "${branch}" } }
      ],
      "produces": {
        "succeeded": [
          { "plan_commits": { "slug": "${slug}", "branch": "${branch}" } }
        ]
      }
    },
    {
      "id": "custom_finalize",
      "kind": "custom",
      "uses": "acme.finalize",
      "after": [
        { "node_id": "custom_apply", "policy": "success" }
      ],
      "args": {
        "command": "test -f .vizier/tmp/custom-approve/${slug}.txt"
      }
    }
  ]
}"#,
    )?;

    let config_path = repo.path().join(".vizier/tmp/custom-approve-config.toml");
    fs::create_dir_all(config_path.parent().ok_or("missing config parent")?)?;
    fs::write(
        &config_path,
        r#"
[workflow.templates]
approve = "custom.approve@v1"
"#,
    )?;

    let output = repo
        .vizier_cmd_background_with_config(&config_path)
        .args(["approve", "approve-custom-template", "--yes"])
        .output()?;
    assert!(
        output.status.success(),
        "custom-template approve queue failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let root_job_id = extract_job_id(&stdout).ok_or("missing queued job id for approve")?;
    wait_for_job_completion(&repo, &root_job_id, Duration::from_secs(80))?;

    let expected_nodes = ["custom_prepare", "custom_apply", "custom_finalize"];
    let mut node_jobs = HashMap::new();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(80) {
        let records = load_job_records(&repo)?;
        node_jobs =
            workflow_node_jobs_for_plan(&records, "custom.approve", "approve-custom-template");
        if expected_nodes
            .iter()
            .all(|node| node_jobs.contains_key(*node))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        expected_nodes
            .iter()
            .all(|node| node_jobs.contains_key(*node)),
        "expected custom approve graph nodes, found {:?}",
        node_jobs
    );

    for node in expected_nodes {
        let job_id = node_jobs
            .get(node)
            .ok_or_else(|| format!("missing job id for node {node}"))?;
        wait_for_job_status(&repo, job_id, "succeeded", Duration::from_secs(80))?;
    }

    let root_record = read_job_record(&repo, &root_job_id)?;
    assert_eq!(
        root_record
            .pointer("/metadata/workflow_node_id")
            .and_then(Value::as_str),
        Some("custom_apply"),
        "root approve job should bind to semantic primary node via uses"
    );
    let marker = repo
        .path()
        .join(".vizier/tmp/custom-approve/approve-custom-template.txt");
    assert!(
        marker.exists(),
        "custom pre-node marker should exist at {}",
        marker.display()
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
        stdout_before.contains("approve-smoke"),
        "pending plans missing approve-smoke: {}",
        stdout_before
    );
    assert!(
        stdout_before.contains("draft/approve-smoke"),
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
    let stderr_lower = stderr.to_ascii_lowercase();
    assert!(
        stderr_lower.contains("agent backend") || stderr_lower.contains("worktree preserved"),
        "stderr should mention backend error context or preserved worktree guidance, got: {stderr}"
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
fn test_approve_commits_staged_only_narrative_outputs() -> TestResult {
    let repo = IntegrationRepo::new_without_mock()?;
    let slug = "approve-staged-only-narrative";

    let mut draft = repo.vizier_cmd();
    draft.env("VIZIER_IT_SKIP_CODE_CHANGE", "1");
    draft.env("VIZIER_IT_SKIP_VIZIER_CHANGE", "1");
    draft.args([
        "draft",
        "--name",
        slug,
        "staged-only narrative approve test",
    ]);
    let draft_output = draft.output()?;
    assert!(
        draft_output.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft_output.stderr)
    );

    clean_workdir(&repo)?;

    let repo_handle = repo.repo();
    let branch = repo_handle.find_branch(&format!("draft/{slug}"), BranchType::Local)?;
    let before_commit = branch.get().peel_to_commit()?.id();

    let approve_agent_path = write_narrative_only_approve_agent(&repo, "approve-staged-only")?;
    let config_path = write_agent_config(
        &repo,
        "approve-staged-only.toml",
        "approve",
        &approve_agent_path,
    )?;

    let mut approve = repo.vizier_cmd_with_config(&config_path);
    approve.env("VIZIER_IT_SKIP_CODE_CHANGE", "1");
    approve.env("VIZIER_IT_SKIP_VIZIER_CHANGE", "1");
    approve.args(["approve", slug, "--yes"]);
    let approve_output = approve.output()?;
    assert!(
        approve_output.status.success(),
        "vizier approve failed: {}",
        String::from_utf8_lossy(&approve_output.stderr)
    );
    let approve_stderr = String::from_utf8_lossy(&approve_output.stderr);
    assert!(
        !approve_stderr.contains("nothing to commit"),
        "approve should not fail with nothing to commit:\n{approve_stderr}"
    );

    let repo_handle = repo.repo();
    let branch = repo_handle.find_branch(&format!("draft/{slug}"), BranchType::Local)?;
    let after_commit = branch.get().peel_to_commit()?;
    assert_eq!(
        after_commit.parent(0)?.id(),
        before_commit,
        "approve should add exactly one commit for staged-only narrative updates"
    );

    let files = files_changed_in_commit(&repo_handle, &after_commit.id().to_string())?;
    assert!(
        files.contains(".vizier/narrative/snapshot.md")
            && files.contains(".vizier/narrative/glossary.md")
            && files.contains(".vizier/narrative/threads/approve-staged-only.md"),
        "approve commit should include canonical narrative files, got {files:?}"
    );
    assert!(
        !files.contains(".vizier/config.toml"),
        "approve commit should trim non-canonical .vizier noise, got {files:?}"
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
