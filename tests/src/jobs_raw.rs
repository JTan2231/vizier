use crate::fixtures::*;

fn assert_rfc3339(value: &str, context: &str) {
    assert!(
        chrono::DateTime::parse_from_rfc3339(value).is_ok(),
        "expected RFC3339 timestamp for {context}: {value}"
    );
}

#[test]
fn test_jobs_list_format_json_raw_typed_envelope() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-json-list-raw";
    write_job_record(
        &repo,
        job_id,
        json!({
            "id": job_id,
            "status": "waiting_on_locks",
            "command": ["vizier", "__workflow-node", "--job-id", job_id],
            "created_at": "2026-02-20T18:20:10Z",
            "started_at": null,
            "finished_at": null,
            "pid": null,
            "exit_code": null,
            "stdout_path": format!(".vizier/jobs/{job_id}/stdout.log"),
            "stderr_path": format!(".vizier/jobs/{job_id}/stderr.log"),
            "session_path": null,
            "outcome_path": null,
            "metadata": {
                "command_alias": "develop",
                "plan": "json",
                "target": "main",
                "branch": "draft/json",
                "ephemeral_run": true,
                "ephemeral_cleanup_state": "deferred",
                "ephemeral_cleanup_detail": "active job job-next still depends on this run",
                "workflow_run_id": "run_json",
                "workflow_node_id": "node_list",
                "workflow_executor_class": "agent",
                "workflow_executor_operation": "agent.invoke",
                "workflow_template_selector": "file:.vizier/develop.hcl",
                "workflow_template_id": "template.develop",
                "workflow_template_version": "v1",
                "execution_root": "."
            },
            "config_snapshot": null,
            "schedule": {
                "after": [
                    { "job_id": "job-upstream", "policy": "success" }
                ],
                "dependencies": [
                    { "artifact": { "custom": { "type_id": "prompt_text", "key": "approve_main" } } }
                ],
                "locks": [
                    { "key": "repo_serial", "mode": "exclusive" }
                ],
                "artifacts": [
                    { "custom": { "type_id": "operation_output", "key": "node_list" } }
                ],
                "approval": {
                    "required": true,
                    "state": "pending",
                    "requested_at": "2026-02-20T18:00:00Z",
                    "requested_by": "tester"
                },
                "pinned_head": { "branch": "main", "oid": "deadbeef" },
                "wait_reason": {
                    "kind": "locks",
                    "detail": "waiting on locks"
                },
                "waited_on": ["dependencies", "locks"]
            }
        }),
    )?;

    let output = repo.vizier_output(&["jobs", "list", "--format", "json", "--raw"])?;
    assert!(
        output.status.success(),
        "vizier jobs list --format json --raw failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let payload: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        payload.get("version").and_then(Value::as_u64),
        Some(1),
        "raw jobs list version mismatch: {payload}"
    );
    assert_rfc3339(
        payload
            .get("generated_at")
            .and_then(Value::as_str)
            .ok_or("missing generated_at in raw jobs list")?,
        "jobs list raw generated_at",
    );

    let jobs = payload
        .get("jobs")
        .and_then(Value::as_array)
        .ok_or("expected jobs array in raw jobs list output")?;
    let job = jobs
        .iter()
        .find(|entry| entry.get("job_id").and_then(Value::as_str) == Some(job_id))
        .ok_or("expected job entry in raw jobs list output")?;

    assert_eq!(
        job.get("status").and_then(Value::as_str),
        Some("waiting_on_locks"),
        "raw list status mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/command/0").and_then(Value::as_str),
        Some("vizier"),
        "raw list command array mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/workflow/ephemeral_run")
            .and_then(Value::as_bool),
        Some(true),
        "raw list ephemeral_run mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/workflow/cleanup_state")
            .and_then(Value::as_str),
        Some("deferred"),
        "raw list cleanup_state mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/workflow/cleanup_detail")
            .and_then(Value::as_str),
        Some("active job job-next still depends on this run"),
        "raw list cleanup_detail mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/wait/kind").and_then(Value::as_str),
        Some("locks"),
        "raw list wait kind mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/wait/detail").and_then(Value::as_str),
        Some("waiting on locks"),
        "raw list wait detail mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/waited_on/0").and_then(Value::as_str),
        Some("dependencies"),
        "raw list waited_on[0] mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/waited_on/1").and_then(Value::as_str),
        Some("locks"),
        "raw list waited_on[1] mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/schedule/after/0/job_id")
            .and_then(Value::as_str),
        Some("job-upstream"),
        "raw list schedule.after mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/schedule/dependencies/0/artifact/custom/type_id")
            .and_then(Value::as_str),
        Some("prompt_text"),
        "raw list schedule.dependencies mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/schedule/artifacts/0/custom/key")
            .and_then(Value::as_str),
        Some("node_list"),
        "raw list schedule.artifacts mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/workflow/run_id").and_then(Value::as_str),
        Some("run_json"),
        "raw list workflow run id mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/workflow/executor_operation")
            .and_then(Value::as_str),
        Some("agent.invoke"),
        "raw list workflow executor operation mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/context/command_alias")
            .and_then(Value::as_str),
        Some("develop"),
        "raw list context command_alias mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/context/execution_root")
            .and_then(Value::as_str),
        Some("."),
        "raw list context execution_root mismatch: {job}"
    );

    Ok(())
}

#[test]
fn test_jobs_show_format_json_raw_typed_envelope() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let job_id = "job-json-show-raw";
    write_job_record(
        &repo,
        job_id,
        json!({
            "id": job_id,
            "status": "waiting_on_deps",
            "command": ["vizier", "__workflow-node", "--job-id", job_id],
            "created_at": "2026-02-20T18:20:10Z",
            "started_at": null,
            "finished_at": null,
            "pid": null,
            "exit_code": null,
            "stdout_path": format!(".vizier/jobs/{job_id}/stdout.log"),
            "stderr_path": format!(".vizier/jobs/{job_id}/stderr.log"),
            "session_path": null,
            "outcome_path": null,
            "metadata": {
                "command_alias": "approve",
                "plan": "json",
                "target": "main",
                "branch": "draft/json",
                "ephemeral_run": true,
                "ephemeral_cleanup_state": "pending",
                "workflow_run_id": "run_show",
                "workflow_node_id": "node_show",
                "workflow_executor_class": "environment.builtin",
                "workflow_executor_operation": "prompt.resolve",
                "workflow_control_policy": null,
                "workflow_template_selector": "file:.vizier/workflows/approve.hcl",
                "workflow_template_id": "template.stage.approve",
                "workflow_template_version": "v2",
                "execution_root": "."
            },
            "config_snapshot": null,
            "schedule": {
                "after": [{ "job_id": "job-prev", "policy": "success" }],
                "dependencies": [
                    { "artifact": { "plan_doc": { "slug": "json", "branch": "draft/json" } } }
                ],
                "locks": [{ "key": "repo_serial", "mode": "exclusive" }],
                "artifacts": [{ "plan_branch": { "slug": "json", "branch": "draft/json" } }],
                "approval": null,
                "pinned_head": null,
                "wait_reason": {
                    "kind": "dependencies",
                    "detail": "waiting on plan_doc:json (draft/json)"
                },
                "waited_on": ["dependencies"]
            }
        }),
    )?;

    let output = repo.vizier_output(&["jobs", "show", job_id, "--format", "json", "--raw"])?;
    assert!(
        output.status.success(),
        "vizier jobs show --format json --raw failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let payload: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        payload.get("version").and_then(Value::as_u64),
        Some(1),
        "raw jobs show version mismatch: {payload}"
    );
    assert_rfc3339(
        payload
            .get("generated_at")
            .and_then(Value::as_str)
            .ok_or("missing generated_at in raw jobs show")?,
        "jobs show raw generated_at",
    );

    let job = payload
        .get("job")
        .ok_or("missing job in raw jobs show output")?;
    assert_eq!(
        job.get("job_id").and_then(Value::as_str),
        Some(job_id),
        "raw show job_id mismatch: {job}"
    );
    assert_eq!(
        job.get("status").and_then(Value::as_str),
        Some("waiting_on_deps"),
        "raw show status mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/wait/kind").and_then(Value::as_str),
        Some("dependencies"),
        "raw show wait kind mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/schedule/after/0/policy")
            .and_then(Value::as_str),
        Some("success"),
        "raw show after dependency mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/schedule/dependencies/0/artifact/plan_doc/slug")
            .and_then(Value::as_str),
        Some("json"),
        "raw show schedule dependency mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/workflow/template_selector")
            .and_then(Value::as_str),
        Some("file:.vizier/workflows/approve.hcl"),
        "raw show workflow template selector mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/workflow/ephemeral_run")
            .and_then(Value::as_bool),
        Some(true),
        "raw show ephemeral_run mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/workflow/cleanup_state")
            .and_then(Value::as_str),
        Some("pending"),
        "raw show cleanup_state mismatch: {job}"
    );
    assert_eq!(
        job.pointer("/context/plan").and_then(Value::as_str),
        Some("json"),
        "raw show context plan mismatch: {job}"
    );

    Ok(())
}
