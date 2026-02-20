use crate::fixtures::*;

fn make_job_record(job_id: &str, status: &str, metadata: Value, schedule: Value) -> Value {
    json!({
        "id": job_id,
        "status": status,
        "command": ["vizier", "run", "draft"],
        "created_at": "2026-02-20T00:00:00Z",
        "started_at": "2026-02-20T00:00:01Z",
        "finished_at": "2026-02-20T00:00:02Z",
        "pid": null,
        "exit_code": 0,
        "stdout_path": format!(".vizier/jobs/{job_id}/stdout.log"),
        "stderr_path": format!(".vizier/jobs/{job_id}/stderr.log"),
        "session_path": null,
        "outcome_path": null,
        "metadata": metadata,
        "config_snapshot": null,
        "schedule": schedule
    })
}

#[test]
fn test_cd_is_deprecated() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo.vizier_output(&["cd", "workspace-check"])?;
    assert!(
        !output.status.success(),
        "vizier cd should fail when deprecated"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("vizier cd is deprecated"),
        "expected deprecation message in stderr:\n{stderr}"
    );
    Ok(())
}

#[test]
fn test_clean_removes_single_job_and_artifact_files() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-clean-single";
    write_job_record(
        &repo,
        job_id,
        make_job_record(job_id, "succeeded", json!(null), json!({})),
    )?;

    let marker_path = format!(".vizier/jobs/artifacts/custom/aa/bb/{job_id}.json");
    let payload_path = format!(".vizier/jobs/artifacts/data/aa/bb/{job_id}.json");
    repo.write(&marker_path, "{}")?;
    repo.write(&payload_path, "{}")?;

    let output = repo.vizier_output(&["clean", job_id, "--yes", "--format", "json"])?;
    assert!(
        output.status.success(),
        "vizier clean failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let payload: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        payload.get("outcome").and_then(Value::as_str),
        Some("clean_completed")
    );
    assert_eq!(payload.get("scope").and_then(Value::as_str), Some("job"));
    assert_eq!(
        payload.pointer("/removed/jobs").and_then(Value::as_i64),
        Some(1)
    );
    assert_eq!(
        payload
            .pointer("/removed/artifact_markers")
            .and_then(Value::as_i64),
        Some(1)
    );
    assert_eq!(
        payload
            .pointer("/removed/artifact_payloads")
            .and_then(Value::as_i64),
        Some(1)
    );
    assert_eq!(
        payload.get("degraded").and_then(Value::as_bool),
        Some(false),
        "expected non-degraded clean outcome: {payload}"
    );

    assert!(
        !repo.path().join(".vizier/jobs").join(job_id).exists(),
        "expected scoped job dir removed"
    );
    assert!(
        !repo.path().join(marker_path).exists(),
        "expected marker removed"
    );
    assert!(
        !repo.path().join(payload_path).exists(),
        "expected payload removed"
    );

    Ok(())
}

#[test]
fn test_clean_run_scope_removes_run_jobs_and_manifest() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let run_id = "run_clean_scope";
    let node_a = "job-run-node-a";
    let node_b = "job-run-node-b";

    for job_id in [node_a, node_b] {
        write_job_record(
            &repo,
            job_id,
            make_job_record(
                job_id,
                "succeeded",
                json!({ "workflow_run_id": run_id }),
                json!({}),
            ),
        )?;
    }

    let manifest_path = format!(".vizier/jobs/runs/{run_id}.json");
    repo.write(&manifest_path, "{}")?;

    let output = repo.vizier_output(&["clean", node_a, "--yes", "--format", "json"])?;
    assert!(
        output.status.success(),
        "vizier clean run-scope failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let payload: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(payload.get("scope").and_then(Value::as_str), Some("run"));
    assert_eq!(payload.get("run_id").and_then(Value::as_str), Some(run_id));
    assert_eq!(
        payload.pointer("/removed/jobs").and_then(Value::as_i64),
        Some(2)
    );
    assert_eq!(
        payload
            .pointer("/removed/run_manifests")
            .and_then(Value::as_i64),
        Some(1)
    );
    assert!(
        !repo.path().join(".vizier/jobs").join(node_a).exists(),
        "expected node a removed"
    );
    assert!(
        !repo.path().join(".vizier/jobs").join(node_b).exists(),
        "expected node b removed"
    );
    assert!(
        !repo.path().join(manifest_path).exists(),
        "expected run manifest removed"
    );
    Ok(())
}

#[test]
fn test_clean_safety_guard_returns_exit_10_and_force_bypasses_dependency_guard() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let source = "job-clean-source";
    let dependent = "job-clean-dependent";

    write_job_record(
        &repo,
        source,
        make_job_record(source, "succeeded", json!(null), json!({})),
    )?;
    write_job_record(
        &repo,
        dependent,
        make_job_record(
            dependent,
            "queued",
            json!(null),
            json!({
                "after": [
                    { "job_id": source, "policy": "success" }
                ]
            }),
        ),
    )?;

    let blocked = repo.vizier_output(&["clean", source, "--yes"])?;
    assert!(
        !blocked.status.success(),
        "expected clean to fail on dependency guard"
    );
    assert_eq!(
        blocked.status.code(),
        Some(10),
        "expected exit code 10 for safety guard, stderr:\n{}",
        String::from_utf8_lossy(&blocked.stderr)
    );
    let blocked_stderr = String::from_utf8_lossy(&blocked.stderr);
    assert!(
        blocked_stderr.contains("cleanup blocked by safety guards"),
        "expected guard message:\n{blocked_stderr}"
    );

    let forced = repo.vizier_output(&["clean", source, "--yes", "--force", "--format", "json"])?;
    assert!(
        forced.status.success(),
        "forced clean should succeed: {}",
        String::from_utf8_lossy(&forced.stderr)
    );
    let payload: Value = serde_json::from_slice(&forced.stdout)?;
    assert_eq!(
        payload.pointer("/removed/jobs").and_then(Value::as_i64),
        Some(1)
    );
    assert!(
        !repo.path().join(".vizier/jobs").join(source).exists(),
        "expected scoped source removed by forced clean"
    );
    assert!(
        repo.path().join(".vizier/jobs").join(dependent).exists(),
        "expected non-scoped dependent to remain"
    );
    Ok(())
}

#[test]
fn test_clean_keep_branches_preserves_draft_branch() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let branch = "draft/clean-keep";
    let job_id = "job-clean-keep-branch";
    repo.git(&["branch", branch])?;

    write_job_record(
        &repo,
        job_id,
        make_job_record(job_id, "succeeded", json!({ "branch": branch }), json!({})),
    )?;

    let output = repo.vizier_output(&["clean", job_id, "--yes", "--keep-branches"])?;
    assert!(
        output.status.success(),
        "clean with --keep-branches failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let repo_handle = repo.repo();
    assert!(
        repo_handle.find_branch(branch, BranchType::Local).is_ok(),
        "expected draft branch to remain when --keep-branches is set"
    );
    Ok(())
}
