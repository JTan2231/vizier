use crate::fixtures::*;
use std::collections::HashMap;

fn load_job_records(repo: &IntegrationRepo) -> TestResult<Vec<Value>> {
    let jobs_root = repo.path().join(".vizier/jobs");
    let mut records = Vec::new();
    if !jobs_root.exists() {
        return Ok(records);
    }

    for entry in fs::read_dir(&jobs_root)? {
        let path = entry?.path().join("job.json");
        if !path.exists() {
            continue;
        }
        records.push(serde_json::from_str(&fs::read_to_string(path)?)?);
    }
    Ok(records)
}

fn workflow_node_jobs_for_plan(
    records: &[Value],
    template_id: &str,
    plan_slug: &str,
) -> HashMap<String, String> {
    let mut jobs = HashMap::new();
    for record in records {
        let metadata = record.get("metadata").and_then(Value::as_object);
        let Some(metadata) = metadata else {
            continue;
        };
        let plan = metadata.get("plan").and_then(Value::as_str);
        let workflow_template = metadata.get("workflow_template_id").and_then(Value::as_str);
        let workflow_node = metadata.get("workflow_node_id").and_then(Value::as_str);
        let job_id = record.get("id").and_then(Value::as_str);
        if plan == Some(plan_slug)
            && workflow_template == Some(template_id)
            && let (Some(node), Some(job_id)) = (workflow_node, job_id)
        {
            jobs.insert(node.to_string(), job_id.to_string());
        }
    }
    jobs
}

fn run_with_config_no_follow(
    repo: &IntegrationRepo,
    config: &Path,
    args: &[&str],
) -> TestResult<Output> {
    let output = repo
        .vizier_cmd_background_with_config(config)
        .args(args)
        .output()?;
    Ok(output)
}

#[test]
fn test_run_develop_composed_workflow_succeeds_with_stage_chain() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let (_output, root_record) = schedule_job_and_wait(
        &repo,
        &[
            "run",
            "develop",
            "--name",
            "run-develop",
            "run composed develop flow",
        ],
        Duration::from_secs(120),
    )?;
    assert_eq!(
        root_record.get("status").and_then(Value::as_str),
        Some("succeeded"),
        "run develop root job should succeed: {root_record}"
    );

    let records = load_job_records(&repo)?;
    let node_jobs = workflow_node_jobs_for_plan(&records, "template.develop", "run-develop");
    let draft_job = node_jobs
        .get("develop_draft__draft_generate_plan")
        .ok_or("missing draft stage job in composed develop run")?
        .to_string();
    let approve_job = node_jobs
        .get("develop_approve__approve_apply_once")
        .ok_or("missing approve stage job in composed develop run")?
        .to_string();
    let merge_job = node_jobs
        .get("develop_merge__merge_integrate")
        .ok_or("missing merge stage job in composed develop run")?
        .to_string();

    let approve_record = read_job_record(&repo, &approve_job)?;
    let approve_after = approve_record
        .pointer("/schedule/after")
        .and_then(Value::as_array)
        .ok_or("approve stage after deps missing")?;
    assert!(
        approve_after
            .iter()
            .any(|dep| dep.get("job_id").and_then(Value::as_str) == Some(draft_job.as_str())),
        "approve stage should depend on draft stage: {approve_record}"
    );

    let merge_record = read_job_record(&repo, &merge_job)?;
    let merge_after = merge_record
        .pointer("/schedule/after")
        .and_then(Value::as_array)
        .ok_or("merge stage after deps missing")?;
    assert!(
        merge_after
            .iter()
            .any(|dep| dep.get("job_id").and_then(Value::as_str) == Some(approve_job.as_str())),
        "merge stage should depend on approve stage: {merge_record}"
    );

    // `run` roots bind to the first node; wait for the merge-stage terminal node
    // before asserting branch cleanup semantics.
    wait_for_job_completion(&repo, &merge_job, Duration::from_secs(120))?;
    let merge_record = read_job_record(&repo, &merge_job)?;
    assert_eq!(
        merge_record.get("status").and_then(Value::as_str),
        Some("succeeded"),
        "merge stage should succeed before branch cleanup assertions: {merge_record}"
    );

    assert!(
        repo.repo()
            .find_branch("draft/run-develop", BranchType::Local)
            .is_err(),
        "merge stage should delete the draft branch by default"
    );

    Ok(())
}

#[test]
fn test_run_alias_prefers_commands_mapping_over_repo_fallback_file() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/mapped-run.toml",
        r#"
id = "custom.mapped"
version = "v1"

[[nodes]]
id = "mapped"
kind = "shell"
uses = "acme.mapped"

[nodes.args]
command = "printf 'mapped\n' > .vizier/tmp/mapped-marker.txt"
"#,
    )?;
    repo.write(
        ".vizier/selector_order.toml",
        r#"
id = "custom.fallback"
version = "v1"

[[nodes]]
id = "fallback"
kind = "shell"
uses = "acme.fallback"

[nodes.args]
command = "printf 'fallback\n' > .vizier/tmp/fallback-marker.txt"
"#,
    )?;

    let cfg = repo.path().join(".vizier/tmp/run-selector-order.toml");
    fs::create_dir_all(cfg.parent().ok_or("missing config parent")?)?;
    fs::write(
        &cfg,
        r#"
[commands]
selector_order = "file:.vizier/mapped-run.toml"
"#,
    )?;

    let output = run_with_config_no_follow(&repo, &cfg, &["run", "selector_order"])?;
    assert!(
        output.status.success(),
        "run selector_order should queue successfully: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let job_id = extract_job_id(&stdout).ok_or("run selector_order missing job id")?;
    wait_for_job_completion(&repo, &job_id, Duration::from_secs(40))?;
    let record = read_job_record(&repo, &job_id)?;
    assert_eq!(
        record.get("status").and_then(Value::as_str),
        Some("succeeded"),
        "run selector_order should succeed: {record}"
    );

    assert!(
        repo.path().join(".vizier/tmp/mapped-marker.txt").exists(),
        "mapped selector should execute when [commands] is configured"
    );
    assert!(
        !repo.path().join(".vizier/tmp/fallback-marker.txt").exists(),
        "repo fallback template should not execute when [commands] mapping exists"
    );

    Ok(())
}

#[test]
fn test_run_alias_repo_fallback_executes_when_unmapped() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;
    repo.write(
        ".vizier/fallback_only.toml",
        r#"
id = "custom.fallback_only"
version = "v1"

[[nodes]]
id = "fallback_only"
kind = "shell"
uses = "acme.fallback_only"

[nodes.args]
command = "printf 'ok\n' > .vizier/tmp/fallback-only-marker.txt"
"#,
    )?;

    let (_output, record) =
        schedule_job_and_wait(&repo, &["run", "fallback_only"], Duration::from_secs(40))?;
    assert_eq!(
        record.get("status").and_then(Value::as_str),
        Some("succeeded"),
        "run fallback_only should succeed: {record}"
    );
    assert!(
        repo.path()
            .join(".vizier/tmp/fallback-only-marker.txt")
            .exists(),
        "run fallback_only should resolve .vizier/fallback_only.toml"
    );
    Ok(())
}

#[test]
fn test_run_failure_blocks_downstream_nodes() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;
    repo.write(
        ".vizier/fail_chain.toml",
        r#"
id = "custom.fail_chain"
version = "v1"

[[nodes]]
id = "first"
kind = "shell"
uses = "acme.first"

[nodes.args]
command = "exit 1"

[[nodes]]
id = "second"
kind = "shell"
uses = "acme.second"
after = [{ node_id = "first", policy = "success" }]

[nodes.args]
command = "printf 'downstream\n' > .vizier/tmp/downstream-should-not-run.txt"
"#,
    )?;

    let (_output, record) =
        schedule_job_and_wait(&repo, &["run", "fail_chain"], Duration::from_secs(40))?;
    assert_eq!(
        record.get("status").and_then(Value::as_str),
        Some("failed"),
        "run fail_chain should fail at the first node: {record}"
    );
    assert!(
        !repo
            .path()
            .join(".vizier/tmp/downstream-should-not-run.txt")
            .exists(),
        "downstream node should not run when upstream stage fails"
    );
    Ok(())
}
