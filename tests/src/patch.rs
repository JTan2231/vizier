use crate::fixtures::*;

fn read_branch_file(repo: &Repository, branch: &str, rel_path: &str) -> TestResult<String> {
    let branch_ref = repo.find_branch(branch, BranchType::Local)?;
    let commit = branch_ref.get().peel_to_commit()?;
    let tree = commit.tree()?;
    let entry = tree.get_path(Path::new(rel_path))?;
    let blob = repo.find_blob(entry.id())?;
    Ok(String::from_utf8(blob.content().to_vec())?)
}

fn execution_state(repo: &Repository, build_id: &str) -> TestResult<Value> {
    let branch = format!("build/{build_id}");
    let path = format!(".vizier/implementation-plans/builds/{build_id}/execution.json");
    let text = read_branch_file(repo, &branch, &path)?;
    Ok(serde_json::from_str(&text)?)
}

fn run_patch(repo: &IntegrationRepo, args: &[&str]) -> io::Result<Output> {
    let mut cmd = repo.vizier_cmd_base();
    cmd.args(args);
    cmd.output()
}

fn run_patch_follow(repo: &IntegrationRepo, args: &[&str]) -> io::Result<Output> {
    let mut cmd = repo.vizier_cmd();
    cmd.args(args);
    cmd.output()
}

fn load_job_records(repo: &IntegrationRepo) -> TestResult<Vec<Value>> {
    let jobs_dir = repo.path().join(".vizier/jobs");
    let mut records = Vec::new();
    if !jobs_dir.is_dir() {
        return Ok(records);
    }
    for entry in fs::read_dir(jobs_dir)? {
        let entry = entry?;
        let record_path = entry.path().join("job.json");
        if !record_path.is_file() {
            continue;
        }
        let record: Value = serde_json::from_str(&fs::read_to_string(&record_path)?)?;
        records.push(record);
    }
    Ok(records)
}

fn metadata_scope(record: &Value) -> Option<&str> {
    record.pointer("/metadata/scope").and_then(Value::as_str)
}

fn non_patch_phase_job_ids(records: &[Value]) -> HashSet<String> {
    records
        .iter()
        .filter(|record| {
            matches!(
                metadata_scope(record),
                Some("build_materialize") | Some("approve") | Some("review") | Some("merge")
            )
        })
        .filter_map(|record| record.get("id").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect()
}

fn wait_for_patch_execution_manifest(
    repo: &IntegrationRepo,
    patch_session: &str,
    timeout: Duration,
) -> TestResult {
    let build_branch = format!("build/{patch_session}");
    let execution_rel =
        format!(".vizier/implementation-plans/builds/{patch_session}/execution.json");
    let start = Instant::now();
    loop {
        if start.elapsed() > timeout {
            return Err(format!(
                "timed out waiting for build execution manifest on {build_branch}:{execution_rel}"
            )
            .into());
        }
        if read_branch_file(&repo.repo(), &build_branch, &execution_rel).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn test_patch_preflight_failure_fails_enqueued_root_job() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;
    repo.write("specs/ok.md", "Valid spec\n")?;
    repo.write("specs/empty.md", "   \n")?;

    let output = run_patch(
        &repo,
        &[
            "patch",
            "specs/ok.md",
            "specs/missing.md",
            "specs/empty.md",
            "--yes",
        ],
    )?;
    assert!(
        output.status.success(),
        "patch enqueue should succeed even when root job later fails: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let job_id = extract_job_id(&stdout).ok_or("expected patch root job id in enqueue output")?;
    wait_for_job_completion(&repo, &job_id, Duration::from_secs(20))?;

    let root_record = read_job_record(&repo, &job_id)?;
    assert_eq!(
        root_record.get("status").and_then(Value::as_str),
        Some("failed"),
        "invalid patch input should fail the queued root job: {root_record}"
    );
    assert_eq!(
        root_record
            .pointer("/metadata/workflow_template_id")
            .and_then(Value::as_str),
        Some("template.patch"),
        "patch root should persist workflow template id"
    );
    assert_eq!(
        root_record
            .pointer("/metadata/workflow_template_version")
            .and_then(Value::as_str),
        Some("v1"),
        "patch root should persist workflow template version"
    );
    assert_eq!(
        root_record
            .pointer("/metadata/workflow_node_id")
            .and_then(Value::as_str),
        Some("patch_execute"),
        "patch root should persist workflow node id"
    );
    assert_eq!(
        root_record
            .pointer("/metadata/workflow_capability_id")
            .and_then(Value::as_str),
        Some("cap.patch.execute_pipeline"),
        "patch root should persist workflow capability id"
    );
    let hash = root_record
        .pointer("/metadata/workflow_policy_snapshot_hash")
        .and_then(Value::as_str)
        .ok_or("patch root workflow policy snapshot hash missing")?;
    assert_eq!(
        hash.len(),
        64,
        "patch workflow hash should be a sha256 hex string: {hash}"
    );

    let stderr_path = repo
        .path()
        .join(".vizier/jobs")
        .join(&job_id)
        .join("stderr.log");
    let stderr_log = fs::read_to_string(stderr_path)?;
    let stdout_path = repo
        .path()
        .join(".vizier/jobs")
        .join(&job_id)
        .join("stdout.log");
    let stdout_log = fs::read_to_string(stdout_path)?;
    if !stderr_log.trim().is_empty() || !stdout_log.trim().is_empty() {
        assert!(
            stderr_log.contains("patch preflight failed")
                || stdout_log.contains("patch preflight failed"),
            "root job logs should report preflight failure when logs are emitted:\nstdout:\n{stdout_log}\n\nstderr:\n{stderr_log}"
        );
    }

    let records = load_job_records(&repo)?;
    let has_phase_jobs = records.iter().any(|record| {
        matches!(
            metadata_scope(record),
            Some("build_materialize") | Some("approve") | Some("review") | Some("merge")
        )
    });
    assert!(
        !has_phase_jobs,
        "preflight failure should not enqueue phase jobs"
    );

    Ok(())
}

#[test]
fn test_patch_follow_streams_preflight_and_preserves_cli_order() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write("specs/first.md", "First intent: alpha\n")?;
    repo.write("specs/second.md", "Second intent: bravo\n")?;

    let output = run_patch_follow(
        &repo,
        &["patch", "specs/second.md", "specs/first.md", "--yes"],
    )?;
    assert!(
        output.status.success(),
        "patch --follow failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let patch_session =
        find_save_field(&stdout, "Patch session").ok_or("patch output missing Patch session")?;
    let pipeline = find_save_field(&stdout, "Pipeline").ok_or("patch output missing Pipeline")?;
    assert_eq!(
        pipeline, "approve-review-merge",
        "patch default pipeline should auto-merge when --pipeline is omitted"
    );
    assert!(
        stdout.contains("Patch queue:"),
        "patch output should include queue block: {stdout}"
    );
    assert!(
        stdout.contains("1. specs/second.md") && stdout.contains("2. specs/first.md"),
        "patch queue should preserve CLI order: {stdout}"
    );

    let root_job_id =
        extract_job_id(&stdout).ok_or("expected root patch job id in --follow output")?;
    let root_record = read_job_record(&repo, &root_job_id)?;
    assert_eq!(
        root_record
            .pointer("/metadata/scope")
            .and_then(Value::as_str),
        Some("patch"),
        "root patch job should record scope=patch"
    );

    let branch = format!("build/{patch_session}");
    let manifest_rel = format!(".vizier/implementation-plans/builds/{patch_session}/manifest.json");
    let manifest_text = read_branch_file(&repo.repo(), &branch, &manifest_rel)?;
    let manifest: Value = serde_json::from_str(&manifest_text)?;
    let steps = manifest
        .get("steps")
        .and_then(Value::as_array)
        .ok_or("manifest steps missing")?;
    assert_eq!(steps.len(), 2, "expected two manifest steps");

    let intent0 = steps[0]
        .get("intent_source")
        .and_then(Value::as_str)
        .ok_or("step 0 intent_source missing")?;
    let intent1 = steps[1]
        .get("intent_source")
        .and_then(Value::as_str)
        .ok_or("step 1 intent_source missing")?;
    assert!(
        intent0.ends_with("/specs/second.md") && intent1.ends_with("/specs/first.md"),
        "manifest intent order should match CLI order: step0={intent0}, step1={intent1}"
    );

    Ok(())
}

#[test]
fn test_patch_default_pipeline_queues_merge_phase_jobs() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write("specs/a.md", "Patch default pipeline a\n")?;
    repo.write("specs/b.md", "Patch default pipeline b\n")?;

    let output = run_patch_follow(&repo, &["patch", "specs/a.md", "specs/b.md", "--yes"])?;
    assert!(
        output.status.success(),
        "default patch run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let patch_session =
        find_save_field(&stdout, "Patch session").ok_or("patch output missing Patch session")?;
    let state = execution_state(&repo.repo(), &patch_session)?;
    let steps = state
        .get("steps")
        .and_then(Value::as_array)
        .ok_or("execution steps missing")?;
    assert_eq!(steps.len(), 2, "expected two execution steps");

    for step in steps {
        assert_eq!(
            step.pointer("/policy/pipeline").and_then(Value::as_str),
            Some("approve-review-merge"),
            "default patch policy should include merge phase: {step}"
        );
        assert!(
            step.pointer("/node_job_ids/review")
                .and_then(Value::as_str)
                .is_some(),
            "default patch pipeline should queue review jobs: {step}"
        );
        assert!(
            step.pointer("/node_job_ids/merge")
                .and_then(Value::as_str)
                .is_some(),
            "default patch pipeline should queue merge jobs: {step}"
        );
    }

    Ok(())
}

#[test]
fn test_patch_explicit_approve_pipeline_skips_review_and_merge() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write("specs/a.md", "Patch approve pipeline a\n")?;
    repo.write("specs/b.md", "Patch approve pipeline b\n")?;

    let output = run_patch_follow(
        &repo,
        &[
            "patch",
            "specs/a.md",
            "specs/b.md",
            "--pipeline",
            "approve",
            "--yes",
        ],
    )?;
    assert!(
        output.status.success(),
        "approve-only patch run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let patch_session =
        find_save_field(&stdout, "Patch session").ok_or("patch output missing Patch session")?;
    wait_for_patch_execution_manifest(&repo, &patch_session, Duration::from_secs(30))?;
    let state = execution_state(&repo.repo(), &patch_session)?;
    let steps = state
        .get("steps")
        .and_then(Value::as_array)
        .ok_or("execution steps missing")?;
    assert_eq!(steps.len(), 2, "expected two execution steps");

    for step in steps {
        assert_eq!(
            step.pointer("/policy/pipeline").and_then(Value::as_str),
            Some("approve"),
            "explicit patch pipeline should stay approve-only: {step}"
        );
        assert!(
            step.pointer("/node_job_ids/review")
                .and_then(Value::as_str)
                .is_none(),
            "approve pipeline should not queue review jobs: {step}"
        );
        assert!(
            step.pointer("/node_job_ids/merge")
                .and_then(Value::as_str)
                .is_none(),
            "approve pipeline should not queue merge jobs: {step}"
        );
    }

    Ok(())
}

#[test]
fn test_patch_resume_reuses_phase_jobs() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write("specs/a.md", "Patch resume a\n")?;
    repo.write("specs/b.md", "Patch resume b\n")?;

    let first = run_patch_follow(
        &repo,
        &[
            "patch",
            "specs/a.md",
            "specs/b.md",
            "--pipeline",
            "approve",
            "--yes",
        ],
    )?;
    assert!(
        first.status.success(),
        "initial patch run failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    let first_records = load_job_records(&repo)?;
    let phase_before = non_patch_phase_job_ids(&first_records);
    assert!(
        !phase_before.is_empty(),
        "expected initial patch run to enqueue non-root phase jobs"
    );

    let resumed = run_patch_follow(
        &repo,
        &[
            "patch",
            "specs/a.md",
            "specs/b.md",
            "--pipeline",
            "approve",
            "--yes",
            "--resume",
        ],
    )?;
    assert!(
        resumed.status.success(),
        "patch resume failed: {}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let resume_stdout = String::from_utf8_lossy(&resumed.stdout);
    assert!(
        resume_stdout.contains("Build execution resumed"),
        "expected resumed execution output: {resume_stdout}"
    );

    let second_records = load_job_records(&repo)?;
    let phase_after = non_patch_phase_job_ids(&second_records);
    assert_eq!(
        phase_before, phase_after,
        "resume should reuse existing phase jobs without enqueuing new ones"
    );

    Ok(())
}

#[test]
fn test_patch_after_applies_to_root_job_schedule() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;
    repo.write("specs/after.md", "Patch after dependency\n")?;

    let predecessor = repo.vizier_cmd_background().args(["save"]).output()?;
    assert!(
        predecessor.status.success(),
        "failed to enqueue predecessor job: {}",
        String::from_utf8_lossy(&predecessor.stderr)
    );
    let predecessor_id = extract_job_id(&String::from_utf8_lossy(&predecessor.stdout))
        .ok_or("missing predecessor job id")?;
    wait_for_job_completion(&repo, &predecessor_id, Duration::from_secs(20))?;

    let patch = run_patch(
        &repo,
        &[
            "patch",
            "specs/after.md",
            "--after",
            &predecessor_id,
            "--yes",
        ],
    )?;
    assert!(
        patch.status.success(),
        "patch enqueue with --after failed: {}",
        String::from_utf8_lossy(&patch.stderr)
    );
    let patch_job_id =
        extract_job_id(&String::from_utf8_lossy(&patch.stdout)).ok_or("missing patch job id")?;

    let record = read_job_record(&repo, &patch_job_id)?;
    let after = record
        .pointer("/schedule/after")
        .and_then(Value::as_array)
        .ok_or("patch root schedule.after missing")?;
    let after_ids = after
        .iter()
        .filter_map(|entry| entry.get("job_id").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert_eq!(
        after_ids,
        vec![predecessor_id.as_str()],
        "patch root should keep explicit --after dependencies: {after:?}"
    );

    Ok(())
}
