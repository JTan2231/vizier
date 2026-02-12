use crate::fixtures::*;
use std::collections::HashMap;

fn run_scheduled_ok(repo: &IntegrationRepo, args: &[&str]) -> TestResult<Output> {
    schedule_job_and_expect_status(repo, args, "succeeded", Duration::from_secs(40))
}

fn merge_cicd_gate_operations(session_contents: &str) -> TestResult<Vec<Value>> {
    let session: Value = serde_json::from_str(session_contents)?;
    let operations = session
        .get("operations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(operations
        .into_iter()
        .filter(|entry| {
            entry.get("kind").and_then(Value::as_str) == Some("cicd_gate")
                && entry
                    .get("details")
                    .and_then(|details| details.get("scope"))
                    .and_then(Value::as_str)
                    == Some("merge")
        })
        .collect())
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
fn test_merge_requires_yes() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo
        .vizier_cmd_background()
        .args(["merge", "missing-plan"])
        .output()?;
    assert!(
        !output.status.success(),
        "expected merge without --yes to fail"
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
fn test_merge_queue_flag_is_rejected() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo
        .vizier_cmd_background()
        .args(["merge", "--queue", "queue-plan", "--yes"])
        .output()?;
    assert!(
        !output.status.success(),
        "expected merge --queue to be rejected"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stderr}\n{stdout}");
    assert!(
        combined.contains("--queue"),
        "expected error to mention --queue, got:\n{combined}"
    );
    Ok(())
}

#[test]
fn test_scheduled_merge_records_workflow_template_metadata() -> TestResult {
    let repo = IntegrationRepo::new()?;
    run_scheduled_ok(
        &repo,
        &[
            "draft",
            "--name",
            "merge-template-meta",
            "merge template metadata",
        ],
    )?;
    run_scheduled_ok(&repo, &["approve", "merge-template-meta", "--yes"])?;
    clean_workdir(&repo)?;

    let gate_script = write_cicd_script(&repo, "merge-template-gate.sh", "#!/bin/sh\nset -eu\n")?;
    let gate_script_flag = gate_script.to_string_lossy().to_string();
    let (_output, record) = schedule_job_and_wait(
        &repo,
        &[
            "merge",
            "merge-template-meta",
            "--yes",
            "--keep-branch",
            "--cicd-script",
            &gate_script_flag,
            "--auto-cicd-fix",
            "--cicd-retries",
            "4",
        ],
        Duration::from_secs(50),
    )?;

    assert_eq!(
        record.get("status").and_then(Value::as_str),
        Some("succeeded"),
        "scheduled merge should succeed: {record}"
    );
    assert_eq!(
        record
            .pointer("/metadata/workflow_template_id")
            .and_then(Value::as_str),
        Some("template.merge"),
        "merge jobs should persist workflow template id"
    );
    assert_eq!(
        record
            .pointer("/metadata/workflow_template_version")
            .and_then(Value::as_str),
        Some("v1"),
        "merge jobs should persist workflow template version"
    );
    assert_eq!(
        record
            .pointer("/metadata/workflow_node_id")
            .and_then(Value::as_str),
        Some("merge_integrate"),
        "merge jobs should persist workflow node id"
    );
    assert_eq!(
        record
            .pointer("/metadata/workflow_capability_id")
            .and_then(Value::as_str),
        Some("cap.git.integrate_plan_branch"),
        "merge jobs should persist workflow capability id"
    );
    let hash = record
        .pointer("/metadata/workflow_policy_snapshot_hash")
        .and_then(Value::as_str)
        .ok_or("merge workflow policy snapshot hash missing")?;
    assert_eq!(
        hash.len(),
        64,
        "merge workflow hash should be a sha256 hex string: {hash}"
    );
    let gate_labels = record
        .pointer("/metadata/workflow_gates")
        .and_then(Value::as_array)
        .ok_or("merge workflow gates missing")?
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    assert!(
        gate_labels.iter().any(|label| label.contains("cicd(")),
        "merge workflow gates should include cicd gate: {gate_labels:?}"
    );
    assert!(
        gate_labels
            .iter()
            .any(|label| label.contains("auto_resolve=true")),
        "merge workflow gates should preserve auto_resolve=true setting: {gate_labels:?}"
    );
    Ok(())
}

#[test]
fn test_scheduled_merge_builtin_template_enqueues_control_node_jobs() -> TestResult {
    let repo = IntegrationRepo::new()?;
    run_scheduled_ok(
        &repo,
        &[
            "draft",
            "--name",
            "merge-control-nodes",
            "merge control node scheduling",
        ],
    )?;
    run_scheduled_ok(&repo, &["approve", "merge-control-nodes", "--yes"])?;
    clean_workdir(&repo)?;

    let gate_script = write_cicd_script(&repo, "merge-control-pass.sh", "#!/bin/sh\nset -eu\n")?;
    let gate_script_flag = gate_script.to_string_lossy().to_string();
    let (_output, root_record) = schedule_job_and_wait(
        &repo,
        &[
            "merge",
            "merge-control-nodes",
            "--yes",
            "--keep-branch",
            "--cicd-script",
            &gate_script_flag,
            "--auto-cicd-fix",
            "--cicd-retries",
            "2",
        ],
        Duration::from_secs(60),
    )?;
    assert_eq!(
        root_record.get("status").and_then(Value::as_str),
        Some("succeeded"),
        "scheduled merge root should succeed: {root_record}"
    );

    let root_job_id = root_record
        .get("id")
        .and_then(Value::as_str)
        .ok_or("merge root job id missing")?
        .to_string();
    let expected_nodes = [
        "merge_integrate",
        "merge_conflict_resolution",
        "merge_gate_cicd",
        "merge_cicd_auto_fix",
    ];
    let mut node_jobs = HashMap::new();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(90) {
        let records = load_job_records(&repo)?;
        node_jobs = workflow_node_jobs_for_plan(&records, "template.merge", "merge-control-nodes");
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
        "expected merge control nodes to be queued, found {:?}",
        node_jobs
    );
    assert_eq!(
        node_jobs.get("merge_integrate"),
        Some(&root_job_id),
        "root merge job should still bind to merge_integrate"
    );
    for node in expected_nodes {
        let job_id = node_jobs
            .get(node)
            .ok_or_else(|| format!("missing job id for node {node}"))?;
        wait_for_job_status(&repo, job_id, "succeeded", Duration::from_secs(90))?;
    }

    Ok(())
}

#[test]
fn test_merge_auto_resolve_fails_when_codex_errors() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let draft = repo
        .vizier_cmd()
        .args(["draft", "--name", "codex-merge", "merge failure testcase"])
        .output()?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );
    let approve = repo
        .vizier_cmd()
        .args(["approve", "codex-merge", "--yes"])
        .output()?;
    assert!(
        approve.status.success(),
        "vizier approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );

    repo.write("a", "master conflicting change")?;
    repo.git(&["add", "a"])?;
    repo.git(&["commit", "-m", "master conflicting change"])?;

    let mut merge = repo.vizier_cmd();
    merge.env("VIZIER_FORCE_AGENT_ERROR", "1");
    merge.args(["merge", "codex-merge", "--yes", "--auto-resolve-conflicts"]);
    let output = merge.output()?;
    assert!(
        !output.status.success(),
        "merge should fail when backend auto-resolution errors"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Backend auto-resolution failed")
            || stderr.contains("forced mock agent failure")
            || stderr.contains("agent backend exited"),
        "stderr should mention backend failure, got: {stderr}"
    );

    repo.repo()
        .find_branch("draft/codex-merge", BranchType::Local)
        .expect("plan branch should remain after failure");
    Ok(())
}
#[test]
fn test_merge_removes_plan_document() -> TestResult {
    let repo = IntegrationRepo::new()?;
    run_scheduled_ok(
        &repo,
        &["draft", "--name", "remove-plan", "plan removal smoke"],
    )?;
    run_scheduled_ok(&repo, &["approve", "remove-plan", "--yes"])?;
    clean_workdir(&repo)?;
    let merge = run_scheduled_ok(&repo, &["merge", "remove-plan", "--yes"])?;
    assert!(
        merge.status.success(),
        "vizier merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    assert!(
        !repo
            .path()
            .join(".vizier/implementation-plans/remove-plan.md")
            .exists(),
        "plan document should be removed after vizier merge"
    );
    let _repo_handle = repo.repo();
    let head = _repo_handle.head()?.peel_to_commit()?;
    let message = head.message().unwrap_or_default().to_string();
    assert!(
        message.contains("Implementation Plan:"),
        "merge commit should inline plan metadata"
    );
    Ok(())
}

#[test]
fn test_merge_uses_history_when_tip_plan_doc_is_missing() -> TestResult {
    let repo = IntegrationRepo::new()?;
    run_scheduled_ok(
        &repo,
        &["draft", "--name", "history-plan", "history fallback smoke"],
    )?;
    run_scheduled_ok(&repo, &["approve", "history-plan", "--yes"])?;
    repo.git(&["checkout", "draft/history-plan"])?;
    repo.git(&["rm", ".vizier/implementation-plans/history-plan.md"])?;
    repo.git(&["commit", "-m", "remove plan doc from tip"])?;
    repo.git(&["checkout", "master"])?;
    clean_workdir(&repo)?;

    let merge = run_scheduled_ok(&repo, &["merge", "history-plan", "--yes"])?;
    assert!(
        merge.status.success(),
        "vizier merge failed with tip-missing plan doc: {}",
        String::from_utf8_lossy(&merge.stderr)
    );

    let repo_handle = repo.repo();
    let head = repo_handle.head()?.peel_to_commit()?;
    let message = head.message().unwrap_or_default().to_string();
    assert!(
        message.contains("Implementation Plan:"),
        "merge commit should still include plan block from history"
    );
    assert!(
        message.contains("plan: history-plan"),
        "merge commit should inline recovered historical plan document"
    );
    assert!(
        !message.contains("Implementation plan document unavailable"),
        "merge commit should not fall back to unavailable placeholder when history recovery works"
    );
    Ok(())
}
#[test]
fn test_merge_default_squash_adds_implementation_commit() -> TestResult {
    let repo = IntegrationRepo::new()?;
    run_scheduled_ok(
        &repo,
        &["draft", "--name", "squash-default", "squash smoke"],
    )?;
    run_scheduled_ok(&repo, &["approve", "squash-default", "--yes"])?;
    clean_workdir(&repo)?;

    let repo_handle = repo.repo();
    let base_commit = repo_handle.head()?.peel_to_commit()?.id();
    let source_tip = repo_handle
        .find_branch("draft/squash-default", BranchType::Local)?
        .get()
        .peel_to_commit()?
        .id();

    let merge = run_scheduled_ok(&repo, &["merge", "squash-default", "--yes"])?;
    assert!(
        merge.status.success(),
        "vizier merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );

    let repo_handle = repo.repo();
    let head = repo_handle.head()?.peel_to_commit()?;
    assert_eq!(
        head.parent_count(),
        1,
        "squashed merge should produce a single-parent merge commit"
    );
    let implementation_commit = head.parent(0)?;
    assert_eq!(
        implementation_commit.parent_count(),
        1,
        "implementation commit should have a single parent"
    );
    assert_eq!(
        implementation_commit.parent(0)?.id(),
        base_commit,
        "implementation commit should descend from the previous master head"
    );
    assert!(
        !repo_handle.graph_descendant_of(head.id(), source_tip)?,
        "squashed merge should sever ancestry to the draft branch"
    );
    assert!(
        repo_handle
            .find_branch("draft/squash-default", BranchType::Local)
            .is_err(),
        "default squashed merge should delete the draft branch"
    );
    Ok(())
}
#[test]
fn test_merge_squash_replays_plan_history() -> TestResult {
    let repo = IntegrationRepo::new()?;
    run_scheduled_ok(
        &repo,
        &["draft", "--name", "squash-replay", "replay squash plan"],
    )?;

    repo.git(&["checkout", "draft/squash-replay"])?;
    repo.write("a", "first replay change\n")?;
    repo.git(&["commit", "-am", "first replay change"])?;
    repo.write("a", "second replay change\n")?;
    repo.git(&["commit", "-am", "second replay change"])?;

    let repo_handle = repo.repo();

    repo.git(&["checkout", "master"])?;
    clean_workdir(&repo)?;
    let plan_tip = repo_handle
        .find_branch("draft/squash-replay", BranchType::Local)?
        .get()
        .peel_to_commit()?
        .id();
    let base_commit = repo_handle.head()?.peel_to_commit()?.id();

    let merge = run_scheduled_ok(&repo, &["merge", "squash-replay", "--yes"])?;
    assert!(
        merge.status.success(),
        "vizier merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );

    let merge_commit = repo_handle.head()?.peel_to_commit()?;
    assert_eq!(
        merge_commit.parent_count(),
        1,
        "squashed merge should keep only the implementation commit as its parent"
    );
    let implementation_commit = merge_commit.parent(0)?;
    assert_eq!(
        implementation_commit.parent(0)?.id(),
        base_commit,
        "implementation commit should descend from the previous master head"
    );
    assert!(
        !repo_handle.graph_descendant_of(merge_commit.id(), plan_tip)?,
        "squashed merge should not keep the draft branch in the ancestry graph"
    );
    let contents = repo.read("a")?;
    assert!(
        contents.starts_with("second replay change\n"),
        "squashed merge should apply the plan branch edits to the target"
    );
    Ok(())
}
#[test]
fn test_merge_no_squash_matches_legacy_parentage() -> TestResult {
    let repo = IntegrationRepo::new()?;
    run_scheduled_ok(
        &repo,
        &["draft", "--name", "legacy-merge", "legacy merge spec"],
    )?;
    run_scheduled_ok(&repo, &["approve", "legacy-merge", "--yes"])?;
    clean_workdir(&repo)?;

    let repo_handle = repo.repo();
    let base_commit = repo_handle.head()?.peel_to_commit()?.id();

    let merge = run_scheduled_ok(&repo, &["merge", "legacy-merge", "--yes", "--no-squash"])?;
    assert!(
        merge.status.success(),
        "vizier merge --no-squash failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );

    let repo_handle = repo.repo();
    let head = repo_handle.head()?.peel_to_commit()?;
    assert_eq!(
        head.parent(0)?.id(),
        base_commit,
        "legacy merge should point directly to the previous master head"
    );
    Ok(())
}
#[test]
fn test_merge_squash_allows_zero_diff_range() -> TestResult {
    let repo = IntegrationRepo::new()?;
    run_scheduled_ok(
        &repo,
        &["draft", "--name", "zero-diff", "plan with no code changes"],
    )?;
    clean_workdir(&repo)?;

    let repo_handle = repo.repo();
    let base_commit = repo_handle.head()?.peel_to_commit()?;
    let source_tip = repo_handle
        .find_branch("draft/zero-diff", BranchType::Local)?
        .get()
        .peel_to_commit()?
        .id();

    let merge = run_scheduled_ok(&repo, &["merge", "zero-diff", "--yes"])?;
    assert!(
        merge.status.success(),
        "vizier merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );

    let head = repo_handle.head()?.peel_to_commit()?;
    assert_eq!(
        head.parent_count(),
        1,
        "squashed merge should keep only the implementation commit as its parent"
    );
    let implementation_commit = head.parent(0)?;
    assert_eq!(
        implementation_commit.parent(0)?.id(),
        base_commit.id(),
        "implementation commit should still descend from the previous master head"
    );
    assert!(
        !repo_handle.graph_descendant_of(head.id(), source_tip)?,
        "squashed merge should not retain the draft branch ancestry"
    );
    Ok(())
}
#[test]
fn test_merge_squash_replay_respects_manual_resolution_before_finishing_range() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "replay-conflict", "replay conflict plan"])?;

    repo.git(&["checkout", "draft/replay-conflict"])?;
    repo.write("a", "plan step one\n")?;
    repo.git(&["commit", "-am", "plan step one"])?;
    repo.write("a", "plan step two\n")?;
    repo.git(&["commit", "-am", "plan step two"])?;

    let plan_tip = repo
        .repo()
        .find_branch("draft/replay-conflict", BranchType::Local)?
        .get()
        .peel_to_commit()?
        .id();

    repo.git(&["checkout", "master"])?;
    clean_workdir(&repo)?;
    repo.write("a", "master diverges\n")?;
    repo.git(&["commit", "-am", "master divergence"])?;
    let base_commit = repo.repo().head()?.peel_to_commit()?.id();

    let merge = repo.vizier_output(&[
        "merge",
        "replay-conflict",
        "--yes",
        "--no-auto-resolve-conflicts",
    ])?;
    assert!(
        !merge.status.success(),
        "expected merge to surface cherry-pick conflict, got:\n{}",
        String::from_utf8_lossy(&merge.stderr)
    );

    let sentinel = repo
        .path()
        .join(".vizier/tmp/merge-conflicts/replay-conflict.json");
    assert!(
        sentinel.exists(),
        "merge conflict sentinel missing after initial failure"
    );

    repo.write("a", "manual resolution wins\n")?;

    let resume = repo.vizier_output(&[
        "merge",
        "replay-conflict",
        "--yes",
        "--no-auto-resolve-conflicts",
        "--complete-conflict",
    ])?;
    assert!(
        resume.status.success(),
        "vizier merge --complete-conflict failed after manual resolution: {}",
        String::from_utf8_lossy(&resume.stderr)
    );
    assert!(
        !sentinel.exists(),
        "sentinel should be removed after --complete-conflict succeeds"
    );

    let contents = repo.read("a")?;
    assert_eq!(
        contents, "manual resolution wins\n",
        "manual resolution should survive replaying the remaining plan commits"
    );

    let repo_handle = repo.repo();
    let head = repo_handle.head()?.peel_to_commit()?;
    assert_eq!(
        head.parent_count(),
        1,
        "squashed merge should keep only the implementation commit as its parent after replay"
    );
    let implementation_commit = head.parent(0)?;
    assert_eq!(
        implementation_commit.parent(0)?.id(),
        base_commit,
        "implementation commit should descend from the pre-merge target head"
    );
    assert!(
        !repo_handle.graph_descendant_of(head.id(), plan_tip)?,
        "squashed merge should not retain draft branch ancestry after manual conflict resolution"
    );
    Ok(())
}
fn prepare_plan_branch_with_merge_history(repo: &IntegrationRepo, slug: &str) -> TestResult {
    let plan_branch = format!("draft/{slug}");
    let side_branch = format!("{slug}-side");

    run_scheduled_ok(
        repo,
        &[
            "draft",
            "--name",
            slug,
            "plan branch includes merge history",
        ],
    )?;
    repo.git(&["checkout", &plan_branch])?;
    repo.write("a", "main path change\n")?;
    repo.git(&["commit", "-am", "main path change"])?;

    repo.git(&["checkout", "HEAD^", "-b", &side_branch])?;
    repo.write("b", "side path change\n")?;
    repo.git(&["commit", "-am", "side path change"])?;

    repo.git(&["checkout", &plan_branch])?;
    repo.git(&["merge", &side_branch])?;

    repo.git(&["checkout", "master"])?;
    clean_workdir(repo)?;
    Ok(())
}
#[test]
fn test_merge_squash_requires_mainline_for_merge_history() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_plan_branch_with_merge_history(&repo, "replay-merge-history")?;

    let merge = repo.vizier_output(&["merge", "replay-merge-history", "--yes"])?;
    assert!(
        !merge.status.success(),
        "expected merge to fail on plan branch with merge commits; got success"
    );
    let stderr = String::from_utf8_lossy(&merge.stderr);
    let requests_mainline = stderr.contains("--squash-mainline")
        || stderr.contains("requires choosing a mainline parent");
    assert!(
        requests_mainline && stderr.contains("merge commits"),
        "merge failure should request --squash-mainline when merge commits exist; stderr:\n{stderr}"
    );

    repo.git(&["reset", "--hard"])?;
    Ok(())
}
#[test]
fn test_merge_squash_mainline_replays_merge_history() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_plan_branch_with_merge_history(&repo, "replay-merge-history-mainline")?;

    let merge = run_scheduled_ok(
        &repo,
        &[
            "merge",
            "replay-merge-history-mainline",
            "--yes",
            "--squash-mainline",
            "1",
        ],
    )?;
    assert!(
        merge.status.success(),
        "expected merge to succeed when squash mainline is provided: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    assert!(
        repo.read("a")?.contains("main path change"),
        "target branch should include main path change after merge"
    );
    assert!(
        repo.read("b")?.contains("side path change"),
        "target branch should include side path change after merge"
    );
    Ok(())
}
#[test]
fn test_merge_no_squash_handles_merge_history() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_plan_branch_with_merge_history(&repo, "replay-merge-history-no-squash")?;

    let merge = run_scheduled_ok(
        &repo,
        &[
            "merge",
            "replay-merge-history-no-squash",
            "--yes",
            "--no-squash",
        ],
    )?;
    assert!(
        merge.status.success(),
        "expected --no-squash merge to succeed even when plan history contains merges: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    assert!(
        repo.read("a")?.contains("main path change"),
        "target branch should include main path change after legacy merge"
    );
    assert!(
        repo.read("b")?.contains("side path change"),
        "target branch should include side path change after legacy merge"
    );
    Ok(())
}
#[test]
fn test_merge_squash_rejects_octopus_merge_history() -> TestResult {
    let repo = IntegrationRepo::new()?;
    run_scheduled_ok(
        &repo,
        &["draft", "--name", "octopus", "octopus merge history"],
    )?;
    let plan_branch = "draft/octopus".to_string();
    let side_one = "octopus-side-1".to_string();
    let side_two = "octopus-side-2".to_string();

    repo.git(&["checkout", &plan_branch])?;
    repo.write("a", "base change\n")?;
    repo.git(&["commit", "-am", "base change"])?;
    let base_oid = oid_for_spec(&repo.repo(), "HEAD")?.to_string();

    repo.git(&["checkout", "-b", &side_one])?;
    repo.write("b", "side one\n")?;
    repo.git(&["commit", "-am", "side one change"])?;

    repo.git(&["checkout", "-b", &side_two, &base_oid])?;
    repo.write("c", "side two\n")?;
    repo.git(&["commit", "-am", "side two change"])?;

    repo.git(&["checkout", &plan_branch])?;
    repo.git(&["merge", &side_one, &side_two])?;
    repo.git(&["checkout", "master"])?;
    clean_workdir(&repo)?;

    let merge = repo.vizier_output(&["merge", "octopus", "--yes"])?;
    assert!(
        !merge.status.success(),
        "expected squash merge to abort on octopus history"
    );
    let stderr = String::from_utf8_lossy(&merge.stderr);
    let octopus_guidance = stderr.contains("octopus") && stderr.contains("--no-squash");
    let mainline_guidance = stderr.contains("squash mode requires choosing a mainline parent");
    assert!(
        octopus_guidance || mainline_guidance,
        "stderr should explain unsupported squash merge history: {stderr}"
    );

    Ok(())
}
#[test]
fn test_merge_cicd_gate_executes_script() -> TestResult {
    let repo = IntegrationRepo::new()?;
    run_scheduled_ok(&repo, &["draft", "--name", "cicd-pass", "cicd gate spec"])?;
    run_scheduled_ok(&repo, &["approve", "cicd-pass", "--yes"])?;
    clean_workdir(&repo)?;

    let script_path = write_cicd_script(
        &repo,
        "gate-pass.sh",
        "#!/bin/sh\nset -eu\nprintf \"gate ok\" > cicd-pass.log\n",
    )?;

    let script_flag = script_path.to_string_lossy().to_string();
    let sessions_before = gather_session_logs(&repo)?;
    let merge = run_scheduled_ok(
        &repo,
        &["merge", "cicd-pass", "--yes", "--cicd-script", &script_flag],
    )?;
    assert!(
        merge.status.success(),
        "vizier merge failed with CI/CD script: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let log = repo.read("cicd-pass.log")?;
    assert!(
        log.contains("gate ok"),
        "CI/CD script output missing expected line: {log}"
    );
    let sessions_after = gather_session_logs(&repo)?;
    let session_path = new_session_log(&sessions_before, &sessions_after)
        .ok_or("merge should create a new session log")?;
    let session = fs::read_to_string(session_path)?;
    let operations = merge_cicd_gate_operations(&session)?;
    let operation = operations
        .iter()
        .find(|entry| {
            entry
                .get("details")
                .and_then(|details| details.get("status"))
                .and_then(Value::as_str)
                == Some("passed")
        })
        .ok_or_else(|| format!("missing merge cicd_gate=passed operation: {operations:?}"))?;
    assert_eq!(
        operation
            .get("details")
            .and_then(|details| details.get("attempts"))
            .and_then(Value::as_u64),
        Some(1),
        "passed operation should record one attempt: {operation}"
    );
    assert!(
        operation
            .get("details")
            .and_then(|details| details.get("auto_resolve_enabled"))
            .and_then(Value::as_bool)
            .is_some(),
        "passed operation should record auto_resolve state: {operation}"
    );
    Ok(())
}
#[test]
fn test_merge_cicd_gate_failure_blocks_merge() -> TestResult {
    let repo = IntegrationRepo::new()?;
    run_scheduled_ok(&repo, &["draft", "--name", "cicd-fail", "cicd fail spec"])?;
    run_scheduled_ok(&repo, &["approve", "cicd-fail", "--yes"])?;
    clean_workdir(&repo)?;

    let script_path = write_cicd_script(
        &repo,
        "gate-fail.sh",
        "#!/bin/sh\necho \"gate failure\" >&2\nexit 1\n",
    )?;
    let script_flag = script_path.to_string_lossy().to_string();
    let sessions_before = gather_session_logs(&repo)?;
    let merge =
        repo.vizier_output(&["merge", "cicd-fail", "--yes", "--cicd-script", &script_flag])?;
    assert!(
        !merge.status.success(),
        "merge should fail when CI/CD gate exits non-zero"
    );
    let stderr = String::from_utf8_lossy(&merge.stderr);
    assert!(
        stderr.contains("CI/CD gate"),
        "stderr should mention CI/CD gate failure: {stderr}"
    );
    assert!(
        stderr.contains("gate failure"),
        "stderr should include script output: {stderr}"
    );
    let repo_handle = repo.repo();
    assert!(
        repo_handle
            .find_branch("draft/cicd-fail", BranchType::Local)
            .is_ok(),
        "draft branch should remain after CI/CD failure"
    );
    let sessions_after = gather_session_logs(&repo)?;
    let session_path = new_session_log(&sessions_before, &sessions_after)
        .ok_or("failed merge should still create a session log")?;
    let session = fs::read_to_string(session_path)?;
    let operations = merge_cicd_gate_operations(&session)?;
    let operation = operations
        .iter()
        .find(|entry| {
            entry
                .get("details")
                .and_then(|details| details.get("status"))
                .and_then(Value::as_str)
                == Some("failed")
        })
        .ok_or_else(|| format!("missing merge cicd_gate=failed operation: {operations:?}"))?;
    assert_eq!(
        operation
            .get("details")
            .and_then(|details| details.get("exit_code"))
            .and_then(Value::as_i64),
        Some(1),
        "failed operation should include exit code 1: {operation}"
    );
    Ok(())
}
#[test]
fn test_merge_cicd_gate_auto_fix_applies_changes() -> TestResult {
    let repo = IntegrationRepo::new()?;
    run_scheduled_ok(
        &repo,
        &["draft", "--name", "cicd-auto", "auto ci gate spec"],
    )?;
    run_scheduled_ok(&repo, &["approve", "cicd-auto", "--yes"])?;
    clean_workdir(&repo)?;

    repo.write(".vizier/tmp/mock_cicd_fix_path", "ci/fixed.txt\n")?;
    let script_path = write_cicd_script(
        &repo,
        "gate-auto.sh",
        "#!/bin/sh\nif [ -f \"ci/fixed.txt\" ]; then\n  exit 0\nfi\necho \"ci gate still failing\" >&2\nexit 1\n",
    )?;
    let script_flag = script_path.to_string_lossy().to_string();
    let sessions_before = gather_session_logs(&repo)?;
    let merge = repo.vizier_output(&[
        "merge",
        "cicd-auto",
        "--yes",
        "--cicd-script",
        &script_flag,
        "--auto-cicd-fix",
        "--cicd-retries",
        "2",
    ])?;
    assert!(
        merge.status.success(),
        "merge with auto CI/CD remediation should succeed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    assert!(
        repo.path().join("ci/fixed.txt").exists(),
        "auto remediation should create the expected fix file"
    );
    let stdout = String::from_utf8_lossy(&merge.stdout);
    assert!(
        stdout.contains("Gate fixes") && stdout.contains("amend:"),
        "merge summary should report the amended implementation commit: {stdout}"
    );
    let sessions_after = gather_session_logs(&repo)?;
    let session_path = new_session_log(&sessions_before, &sessions_after)
        .ok_or("merge should create a new session log")?;
    let session = fs::read_to_string(session_path)?;
    let operations = merge_cicd_gate_operations(&session)?;
    let operation = operations
        .iter()
        .find(|entry| {
            entry
                .get("details")
                .and_then(|details| details.get("status"))
                .and_then(Value::as_str)
                == Some("passed")
        })
        .ok_or_else(|| format!("missing merge cicd_gate=passed operation: {operations:?}"))?;
    assert_eq!(
        operation
            .get("details")
            .and_then(|details| details.get("attempts"))
            .and_then(Value::as_u64),
        Some(2),
        "autofix operation should capture two gate attempts: {operation}"
    );
    assert!(
        operation
            .get("details")
            .and_then(|details| details.get("fixes_count"))
            .and_then(Value::as_u64)
            .unwrap_or_default()
            >= 1,
        "autofix operation should report at least one remediation: {operation}"
    );
    assert!(
        operation
            .get("details")
            .and_then(|details| details.get("fixes"))
            .and_then(Value::as_array)
            .map(|labels| labels.iter().any(|label| {
                label
                    .as_str()
                    .map(|value| value.starts_with("amend:"))
                    .unwrap_or(false)
            }))
            .unwrap_or(false),
        "autofix operation should include amend label: {operation}"
    );
    Ok(())
}
#[test]
fn test_merge_conflict_auto_resolve() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_conflicting_plan(
        &repo,
        "conflict-auto",
        "master edits collide\n",
        "auto resolution should keep this line\n",
    )?;
    clean_workdir(&repo)?;

    let merge = repo.vizier_output(&[
        "merge",
        "conflict-auto",
        "--yes",
        "--auto-resolve-conflicts",
    ])?;
    assert!(
        merge.status.success(),
        "auto-resolve merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let stderr = String::from_utf8_lossy(&merge.stderr);
    assert!(
        stderr.contains("Auto-resolving merge conflicts via"),
        "stderr should mention config-driven conflict auto-resolution: {stderr}"
    );

    let sentinel = repo
        .path()
        .join(".vizier/tmp/merge-conflicts/conflict-auto.json");
    assert!(
        !sentinel.exists(),
        "sentinel should not remain after auto resolution"
    );

    let contents = repo.read("a")?;
    assert!(
        contents.contains("auto resolution should keep this line"),
        "file contents did not reflect plan branch after auto resolution: {}",
        contents
    );

    let status = Command::new("git")
        .args(["-C", repo.path().to_str().unwrap(), "status", "--porcelain"])
        .output()?;
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "working tree should be clean after auto resolution"
    );
    Ok(())
}
#[test]
fn test_merge_conflict_auto_resolve_reuses_setting_on_resume() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_conflicting_plan(
        &repo,
        "conflict-resume-auto",
        "master edits collide\n",
        "plan branch wins after resume\n",
    )?;
    clean_workdir(&repo)?;

    let mut first = repo.vizier_cmd();
    first.args([
        "merge",
        "conflict-resume-auto",
        "--yes",
        "--no-auto-resolve-conflicts",
    ]);
    let initial = first.output()?;
    assert!(
        !initial.status.success(),
        "initial merge should fail when auto-resolve is disabled"
    );
    let sentinel = repo
        .path()
        .join(".vizier/tmp/merge-conflicts/conflict-resume-auto.json");
    assert!(
        sentinel.exists(),
        "sentinel should remain after failed auto-resolution attempt"
    );

    let resume = repo.vizier_output(&[
        "merge",
        "conflict-resume-auto",
        "--yes",
        "--complete-conflict",
        "--auto-resolve-conflicts",
    ])?;
    assert!(
        resume.status.success(),
        "vizier merge --complete-conflict should reuse auto-resolve and succeed: {}",
        String::from_utf8_lossy(&resume.stderr)
    );
    let resume_stderr = String::from_utf8_lossy(&resume.stderr);
    assert!(
        resume_stderr.contains("Auto-resolving merge conflicts via")
            || resume_stderr.contains("Conflict auto-resolution enabled"),
        "resume should surface conflict auto-resolve status: {resume_stderr}"
    );
    assert!(
        !sentinel.exists(),
        "sentinel should be cleared after successful auto-resolve resume"
    );
    let contents = repo.read("a")?;
    assert!(
        contents.contains("plan branch wins after resume"),
        "auto-resolve resume should apply plan contents: {contents}"
    );
    let status = Command::new("git")
        .args(["-C", repo.path().to_str().unwrap(), "status", "--porcelain"])
        .output()?;
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "working tree should be clean after auto-resolve resume"
    );
    Ok(())
}
#[test]
fn test_merge_conflict_creates_sentinel() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_conflicting_plan(
        &repo,
        "conflict-manual",
        "master branch keeps its version\n",
        "plan branch prefers this text\n",
    )?;
    clean_workdir(&repo)?;

    let merge = repo.vizier_output(&[
        "merge",
        "conflict-manual",
        "--yes",
        "--no-auto-resolve-conflicts",
    ])?;
    assert!(
        !merge.status.success(),
        "expected merge to fail on conflicts"
    );

    let sentinel = repo
        .path()
        .join(".vizier/tmp/merge-conflicts/conflict-manual.json");
    assert!(sentinel.exists(), "conflict sentinel missing after failure");
    Ok(())
}
#[test]
fn test_merge_conflict_complete_flag() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_conflicting_plan(
        &repo,
        "conflict-complete",
        "master branch keeps its version\n",
        "plan branch prefers this text\n",
    )?;
    clean_workdir(&repo)?;

    let merge = repo.vizier_output(&[
        "merge",
        "conflict-complete",
        "--yes",
        "--no-auto-resolve-conflicts",
    ])?;
    assert!(
        !merge.status.success(),
        "expected merge to fail on conflicts"
    );

    repo.write("a", "manual resolution wins\n")?;
    repo.git(&["add", "a"])?;
    let status = Command::new("git")
        .args(["-C", repo.path().to_str().unwrap(), "status", "--porcelain"])
        .output()?;
    let status_out = String::from_utf8_lossy(&status.stdout);
    println!("status before resume:\n{status_out}");
    let idx_conflicts = repo.repo().index()?.has_conflicts();
    println!("index.has_conflicts before resume: {idx_conflicts}");
    let conflicts = Command::new("git")
        .args(["-C", repo.path().to_str().unwrap(), "ls-files", "-u"])
        .output()?;
    println!(
        "ls-files -u before resume:\n{}",
        String::from_utf8_lossy(&conflicts.stdout)
    );
    assert!(
        !status_out.contains("U "),
        "expected conflicts to be resolved before --complete-conflict, got:\n{status_out}"
    );

    let resume = repo.vizier_output(&[
        "merge",
        "conflict-complete",
        "--yes",
        "--no-auto-resolve-conflicts",
        "--complete-conflict",
    ])?;
    println!(
        "resume stderr:\n{}",
        String::from_utf8_lossy(&resume.stderr)
    );
    assert!(
        resume.status.success(),
        "vizier merge --complete-conflict failed after manual resolution: {}",
        String::from_utf8_lossy(&resume.stderr)
    );

    let sentinel = repo
        .path()
        .join(".vizier/tmp/merge-conflicts/conflict-complete.json");
    assert!(
        !sentinel.exists(),
        "sentinel should be removed after --complete-conflict succeeds"
    );
    Ok(())
}
#[test]
fn test_merge_conflict_complete_blocks_wrong_branch() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_conflicting_plan(
        &repo,
        "conflict-wrong-branch",
        "master branch keeps its version\n",
        "plan branch prefers this text\n",
    )?;
    clean_workdir(&repo)?;

    let merge = repo.vizier_output(&[
        "merge",
        "conflict-wrong-branch",
        "--yes",
        "--no-auto-resolve-conflicts",
    ])?;
    assert!(
        !merge.status.success(),
        "expected merge to fail on conflicts"
    );

    let sentinel = repo
        .path()
        .join(".vizier/tmp/merge-conflicts/conflict-wrong-branch.json");
    assert!(
        sentinel.exists(),
        "conflict sentinel missing after initial failure"
    );

    repo.git(&["cherry-pick", "--abort"])?;
    repo.git(&["checkout", "-b", "elsewhere"])?;

    let resume = repo.vizier_output(&[
        "merge",
        "conflict-wrong-branch",
        "--yes",
        "--no-auto-resolve-conflicts",
        "--complete-conflict",
    ])?;
    assert!(
        !resume.status.success(),
        "expected --complete-conflict to block when not on the target branch"
    );
    assert!(
        sentinel.exists(),
        "sentinel should remain when resume is blocked on wrong branch"
    );
    Ok(())
}
#[test]
fn test_merge_conflict_complete_flag_rejects_head_drift() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_conflicting_plan(
        &repo,
        "conflict-head-drift",
        "master branch keeps its version\n",
        "plan branch prefers this text\n",
    )?;
    clean_workdir(&repo)?;

    let merge = repo.vizier_output(&[
        "merge",
        "conflict-head-drift",
        "--yes",
        "--no-auto-resolve-conflicts",
    ])?;
    assert!(
        !merge.status.success(),
        "expected merge to fail on conflicts"
    );

    let sentinel = repo
        .path()
        .join(".vizier/tmp/merge-conflicts/conflict-head-drift.json");
    assert!(
        sentinel.exists(),
        "conflict sentinel missing after initial failure"
    );

    repo.git(&["cherry-pick", "--abort"])?;
    repo.write("a", "head moved after conflicts\n")?;
    repo.git(&["commit", "-am", "head drifted"])?;

    let resume = repo.vizier_output(&[
        "merge",
        "conflict-head-drift",
        "--yes",
        "--no-auto-resolve-conflicts",
        "--complete-conflict",
    ])?;
    assert!(
        !resume.status.success(),
        "expected --complete-conflict to block when HEAD moved"
    );
    assert!(
        !sentinel.exists(),
        "sentinel should be cleared when HEAD drift is detected"
    );
    Ok(())
}
#[test]
fn test_merge_complete_conflict_without_pending_state() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_conflicting_plan(
        &repo,
        "conflict-missing",
        "master has no conflicts yet\n",
        "plan branch prep work\n",
    )?;
    clean_workdir(&repo)?;

    let attempt =
        repo.vizier_output(&["merge", "conflict-missing", "--yes", "--complete-conflict"])?;
    assert!(
        !attempt.status.success(),
        "expected --complete-conflict to fail when no merge is pending"
    );
    let stdout = String::from_utf8_lossy(&attempt.stdout);
    let stderr = String::from_utf8_lossy(&attempt.stderr);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        combined.contains("No Vizier-managed merge is awaiting completion"),
        "missing helpful message in command output\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    let sentinel = repo
        .path()
        .join(".vizier/tmp/merge-conflicts/conflict-missing.json");
    assert!(
        !sentinel.exists(),
        "sentinel should not exist when the merge was never started"
    );
    Ok(())
}
