use crate::fixtures::*;

fn run_json(repo: &IntegrationRepo, args: &[&str]) -> TestResult<Value> {
    let output = repo.vizier_output(args)?;
    assert!(
        output.status.success(),
        "command {:?} failed: stderr={}\nstdout={}",
        args,
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    Ok(serde_json::from_slice::<Value>(&output.stdout)?)
}

fn branch_blob_text(repo: &IntegrationRepo, branch: &str, rel_path: &str) -> TestResult<String> {
    let repo_handle = repo.repo();
    let revision = format!("{branch}:{rel_path}");
    let object = repo_handle.revparse_single(&revision)?;
    let blob = object.peel_to_blob()?;
    Ok(String::from_utf8_lossy(blob.content()).to_string())
}

fn head_subject(repo: &IntegrationRepo) -> TestResult<String> {
    let repo_handle = repo.repo();
    let commit = repo_handle.head()?.peel_to_commit()?;
    Ok(commit.summary().unwrap_or_default().to_string())
}

fn head_message(repo: &IntegrationRepo) -> TestResult<String> {
    let repo_handle = repo.repo();
    let commit = repo_handle.head()?.peel_to_commit()?;
    Ok(commit.message().unwrap_or_default().to_string())
}

fn write_single_run_template(repo: &IntegrationRepo, rel: &str, script: &str) -> TestResult {
    repo.write(
        rel,
        &format!(
            "id = \"template.single\"\nversion = \"v1\"\n\
[[nodes]]\n\
id = \"single\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
script = \"{}\"\n",
            script.replace('"', "\\\"")
        ),
    )?;
    Ok(())
}

fn write_stage_token_dependency_templates(repo: &IntegrationRepo) -> TestResult {
    repo.write(
        ".vizier/workflows/custom-stage-token-producer.toml",
        "id = \"template.custom.stage_token.producer\"\n\
version = \"v1\"\n\
[params]\n\
slug = \"alpha\"\n\
[[artifact_contracts]]\n\
id = \"stage_token\"\n\
version = \"v1\"\n\
[[nodes]]\n\
id = \"produce\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
script = \"true\"\n\
[nodes.produces]\n\
succeeded = [{ custom = { type_id = \"stage_token\", key = \"approve:${slug}\" } }]\n",
    )?;
    repo.write(
        ".vizier/workflows/custom-stage-token-consumer-wait.toml",
        "id = \"template.custom.stage_token.consumer_wait\"\n\
version = \"v1\"\n\
[params]\n\
slug = \"alpha\"\n\
[policy.dependencies]\n\
missing_producer = \"wait\"\n\
[[artifact_contracts]]\n\
id = \"stage_token\"\n\
version = \"v1\"\n\
[[nodes]]\n\
id = \"consume\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
script = \"true\"\n\
[[nodes.needs]]\n\
custom = { type_id = \"stage_token\", key = \"approve:${slug}\" }\n",
    )?;
    repo.write(
        ".vizier/workflows/custom-stage-token-consumer-block.toml",
        "id = \"template.custom.stage_token.consumer_block\"\n\
version = \"v1\"\n\
[params]\n\
slug = \"alpha\"\n\
[policy.dependencies]\n\
missing_producer = \"block\"\n\
[[artifact_contracts]]\n\
id = \"stage_token\"\n\
version = \"v1\"\n\
[[nodes]]\n\
id = \"consume\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
script = \"true\"\n\
[[nodes.needs]]\n\
custom = { type_id = \"stage_token\", key = \"approve:${slug}\" }\n",
    )?;
    Ok(())
}

fn write_stage_alias_test_config(repo: &IntegrationRepo) -> TestResult {
    write_stage_alias_test_config_with_agent_command(
        repo,
        "cat >/dev/null; printf '%s\\n' 'mock agent response'",
    )
}

fn write_stage_alias_test_config_with_agent_command(
    repo: &IntegrationRepo,
    command: &str,
) -> TestResult {
    repo.write(
        ".vizier/config.toml",
        &format!(
            r#"[commands]
draft = "file:.vizier/workflows/draft.hcl"
approve = "file:.vizier/workflows/approve.hcl"
merge = "file:.vizier/workflows/merge.hcl"
develop = "file:.vizier/develop.hcl"

[agents.default]
selector = "mock"

[agents.default.agent]
command = ["sh", "-lc", "{}"]
"#,
            command.replace('\\', "\\\\").replace('"', "\\\"")
        ),
    )?;
    Ok(())
}

fn seed_plan_branch(repo: &IntegrationRepo, slug: &str, branch: &str) -> TestResult {
    repo.git(&["checkout", "-b", branch])?;
    let plan_rel = format!(".vizier/implementation-plans/{slug}.md");
    let plan_doc = format!(
        "---\nplan_id: pln_{slug}\nplan: {slug}\nbranch: {branch}\n---\n\n## Operator Spec\nSeeded plan for integration tests.\n\n## Implementation Plan\n- Seeded step\n"
    );
    repo.write(&plan_rel, &plan_doc)?;
    repo.git(&["add", &plan_rel])?;
    repo.git(&["commit", "-m", &format!("docs: seed plan {slug}")])?;
    repo.git(&["checkout", "master"])?;
    Ok(())
}

fn run_stage_approve_follow(repo: &IntegrationRepo, slug: &str, branch: &str) -> TestResult<Value> {
    run_json(
        repo,
        &[
            "run",
            "approve",
            "--set",
            &format!("slug={slug}"),
            "--set",
            &format!("branch={branch}"),
            "--follow",
            "--format",
            "json",
        ],
    )
}

fn load_run_manifest(repo: &IntegrationRepo, run_id: &str) -> TestResult<Value> {
    let manifest_path = repo.path().join(format!(".vizier/jobs/runs/{run_id}.json"));
    Ok(serde_json::from_str(&fs::read_to_string(manifest_path)?)?)
}

fn first_root_job_id(payload: &Value) -> TestResult<String> {
    Ok(payload
        .get("root_job_ids")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .ok_or("missing root job id")?
        .to_string())
}

fn repeated_runs(payload: &Value) -> TestResult<&Vec<Value>> {
    payload
        .get("runs")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing runs array in repeat payload".into())
}

fn repeated_run_id(payload: &Value, index: usize) -> TestResult<String> {
    Ok(repeated_runs(payload)?
        .get(index)
        .and_then(|entry| entry.get("run_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing run_id for repeat index {}", index + 1))?
        .to_string())
}

fn repeated_root_job_id(payload: &Value, index: usize) -> TestResult<String> {
    Ok(repeated_runs(payload)?
        .get(index)
        .and_then(|entry| entry.get("root_job_ids"))
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing root_job_ids[0] for repeat index {}", index + 1))?
        .to_string())
}

fn schedule_after_job_ids(record: &Value) -> Vec<String> {
    record
        .pointer("/schedule/after")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| {
            entry
                .get("job_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<Vec<_>>()
}

fn count_run_manifests(repo: &IntegrationRepo) -> TestResult<usize> {
    let runs_dir = repo.path().join(".vizier/jobs/runs");
    if !runs_dir.is_dir() {
        return Ok(0);
    }
    Ok(fs::read_dir(runs_dir)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .count())
}

fn count_job_records(repo: &IntegrationRepo) -> TestResult<usize> {
    let jobs_dir = repo.path().join(".vizier/jobs");
    if !jobs_dir.is_dir() {
        return Ok(0);
    }

    let mut count = 0usize;
    for entry in fs::read_dir(jobs_dir)? {
        let path = entry?.path();
        if path.is_dir() && path.join("job.json").is_file() {
            count += 1;
        }
    }
    Ok(count)
}

fn manifest_node_job_id(manifest: &Value, node_id: &str) -> TestResult<String> {
    Ok(manifest
        .pointer(&format!("/nodes/{node_id}/job_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing node job id for {node_id}"))?
        .to_string())
}

fn wait_for_manifest_jobs(
    repo: &IntegrationRepo,
    manifest: &Value,
    timeout: Duration,
) -> TestResult {
    let Some(nodes) = manifest.get("nodes").and_then(Value::as_object) else {
        return Err("manifest is missing nodes map".into());
    };
    let mut job_ids = Vec::with_capacity(nodes.len());
    for node in nodes.values() {
        let job_id = node
            .get("job_id")
            .and_then(Value::as_str)
            .ok_or("manifest node missing job_id")?;
        job_ids.push(job_id.to_string());
    }
    job_ids.sort();

    let max_ticks = ((timeout.as_millis() / 100) as usize).max(1);
    for _ in 0..max_ticks {
        let tick = repo.vizier_output(&["jobs", "schedule"])?;
        assert!(
            tick.status.success(),
            "jobs schedule failed while waiting for manifest jobs: stderr={}\nstdout={}",
            String::from_utf8_lossy(&tick.stderr),
            String::from_utf8_lossy(&tick.stdout)
        );

        let mut all_terminal = true;
        for job_id in &job_ids {
            let record = read_job_record(repo, job_id)?;
            let status = record
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            if matches!(
                status,
                "queued"
                    | "waiting_on_deps"
                    | "waiting_on_approval"
                    | "waiting_on_locks"
                    | "running"
            ) {
                all_terminal = false;
            }
        }

        if all_terminal {
            return Ok(());
        }

        std::thread::sleep(Duration::from_millis(100));
    }

    let mut statuses = Vec::with_capacity(job_ids.len());
    for job_id in &job_ids {
        let record = read_job_record(repo, job_id)?;
        let status = record
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let detail = record
            .get("summary")
            .and_then(Value::as_str)
            .or_else(|| {
                record
                    .pointer("/schedule/wait_reason/detail")
                    .and_then(Value::as_str)
            })
            .unwrap_or("");
        if status == "failed" {
            let mut failure_details = Vec::new();
            if !detail.is_empty() {
                failure_details.push(detail.to_string());
            }
            if let Some(stderr_path) = record.get("stderr_path").and_then(Value::as_str)
                && let Ok(stderr) = fs::read_to_string(repo.path().join(stderr_path))
            {
                let tail = stderr
                    .lines()
                    .rev()
                    .take(6)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join(" | ");
                if !tail.trim().is_empty() {
                    failure_details.push(format!("stderr_tail={tail}"));
                }
            }
            if let Some(stdout_path) = record.get("stdout_path").and_then(Value::as_str)
                && let Ok(stdout) = fs::read_to_string(repo.path().join(stdout_path))
            {
                let tail = stdout
                    .lines()
                    .rev()
                    .take(6)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join(" | ");
                if !tail.trim().is_empty() {
                    failure_details.push(format!("stdout_tail={tail}"));
                }
            }
            if failure_details.is_empty() {
                statuses.push(format!("{job_id}:{status}"));
            } else {
                statuses.push(format!(
                    "{job_id}:{status} ({})",
                    failure_details.join("; ")
                ));
            }
            continue;
        }
        if detail.is_empty() {
            statuses.push(format!("{job_id}:{status}"));
        } else {
            statuses.push(format!("{job_id}:{status} ({detail})"));
        }
    }
    Err(format!(
        "timed out waiting for manifest jobs to reach terminal state: {}",
        statuses.join(", ")
    )
    .into())
}

#[test]
fn test_run_alias_composes_and_applies_set_overrides() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/develop.hcl",
        "id = \"template.develop\"\n\
version = \"v1\"\n\
imports = [\n\
  { name = \"stage_one\", path = \"workflows/stage_one.toml\" },\n\
  { name = \"stage_two\", path = \"workflows/stage_two.toml\" }\n\
]\n\
links = [\n\
  { from = \"stage_one\", to = \"stage_two\" }\n\
]\n",
    )?;
    repo.write(
        ".vizier/workflows/stage_one.toml",
        "id = \"template.stage_one\"\n\
version = \"v1\"\n\
[params]\n\
message = \"default\"\n\
[[nodes]]\n\
id = \"one\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
script = \"echo ${message} > compose.txt\"\n",
    )?;
    repo.write(
        ".vizier/workflows/stage_two.toml",
        "id = \"template.stage_two\"\n\
version = \"v1\"\n\
[[nodes]]\n\
id = \"two\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
script = \"test -f compose.txt\"\n",
    )?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "develop",
            "--set",
            "message=override",
            "--format",
            "json",
        ],
    )?;

    let run_id = payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing run_id")?;
    let roots = payload
        .get("root_job_ids")
        .and_then(Value::as_array)
        .ok_or("missing root_job_ids")?;
    assert_eq!(roots.len(), 1, "expected a single root job: {payload}");

    let manifest_path = repo.path().join(format!(".vizier/jobs/runs/{run_id}.json"));
    assert!(
        manifest_path.is_file(),
        "missing run manifest: {manifest_path:?}"
    );
    let manifest: Value = serde_json::from_str(&fs::read_to_string(&manifest_path)?)?;
    assert_eq!(
        manifest
            .get("template_selector")
            .and_then(Value::as_str)
            .unwrap_or(""),
        "file:.vizier/develop.hcl"
    );

    let stage_one_script = manifest
        .pointer("/nodes/stage_one__one/args/script")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        stage_one_script.contains("override"),
        "expected --set override in manifest args, got: {stage_one_script}"
    );

    let root_job = roots[0].as_str().ok_or("invalid root job id")?;
    let root_record = read_job_record(&repo, root_job)?;
    assert_eq!(
        root_record
            .pointer("/metadata/command_alias")
            .and_then(Value::as_str),
        Some("develop")
    );
    assert_eq!(
        root_record
            .pointer("/metadata/workflow_template_selector")
            .and_then(Value::as_str),
        Some("file:.vizier/develop.hcl")
    );

    Ok(())
}

#[test]
fn test_run_set_expands_non_args_runtime_fields() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/set-surface.json",
        "{\n\
  \"id\": \"template.set.surface\",\n\
  \"version\": \"v1\",\n\
  \"params\": {\n\
    \"slug\": \"alpha\",\n\
    \"branch\": \"draft/alpha\",\n\
    \"lock_key\": \"alpha\",\n\
    \"gate_script\": \"test -f README.md\",\n\
    \"retry_mode\": \"on_failure\",\n\
    \"retry_budget\": \"2\"\n\
  },\n\
  \"artifact_contracts\": [\n\
    {\"id\": \"plan_doc\", \"version\": \"v1\"},\n\
    {\"id\": \"prompt_text\", \"version\": \"v1\"}\n\
  ],\n\
  \"nodes\": [\n\
    {\n\
      \"id\": \"single\",\n\
      \"kind\": \"shell\",\n\
      \"uses\": \"cap.env.shell.command.run\",\n\
      \"args\": {\"script\": \"true\"},\n\
      \"needs\": [\n\
        {\"plan_doc\": {\"slug\": \"${slug}\", \"branch\": \"${branch}\"}}\n\
      ],\n\
      \"produces\": {\n\
        \"succeeded\": [\n\
          {\"custom\": {\"type_id\": \"prompt_text\", \"key\": \"${slug}\"}}\n\
        ]\n\
      },\n\
      \"locks\": [\n\
        {\"key\": \"plan:${lock_key}\", \"mode\": \"exclusive\"}\n\
      ],\n\
      \"preconditions\": [\n\
        {\"kind\": \"custom\", \"id\": \"branch_ready\", \"args\": {\"branch\": \"${branch}\"}}\n\
      ],\n\
      \"gates\": [\n\
        {\"kind\": \"script\", \"script\": \"${gate_script}\"}\n\
      ],\n\
      \"retry\": {\"mode\": \"${retry_mode}\", \"budget\": \"${retry_budget}\"}\n\
    }\n\
  ]\n\
}\n",
    )?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/set-surface.json",
            "--set",
            "slug=beta",
            "--set",
            "branch=draft/beta",
            "--set",
            "lock_key=beta",
            "--set",
            "gate_script=echo expanded-gate",
            "--set",
            "retry_budget=5",
            "--format",
            "json",
        ],
    )?;

    let run_id = payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing run_id")?;
    let root_job = payload
        .get("root_job_ids")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .ok_or("missing root job id")?;

    let manifest_path = repo.path().join(format!(".vizier/jobs/runs/{run_id}.json"));
    let manifest: Value = serde_json::from_str(&fs::read_to_string(&manifest_path)?)?;
    assert_eq!(
        manifest
            .pointer("/nodes/single/gates/0/script")
            .and_then(Value::as_str),
        Some("echo expanded-gate")
    );
    assert_eq!(
        manifest
            .pointer("/nodes/single/retry/mode")
            .and_then(Value::as_str),
        Some("on_failure")
    );
    assert_eq!(
        manifest
            .pointer("/nodes/single/retry/budget")
            .and_then(Value::as_u64),
        Some(5)
    );
    assert_eq!(
        manifest
            .pointer("/nodes/single/artifacts_by_outcome/succeeded/0/custom/key")
            .and_then(Value::as_str),
        Some("beta")
    );

    let root_record = read_job_record(&repo, root_job)?;
    assert_eq!(
        root_record
            .pointer("/schedule/dependencies/0/artifact/plan_doc/slug")
            .and_then(Value::as_str),
        Some("beta")
    );
    assert_eq!(
        root_record
            .pointer("/schedule/dependencies/0/artifact/plan_doc/branch")
            .and_then(Value::as_str),
        Some("draft/beta")
    );
    assert_eq!(
        root_record
            .pointer("/schedule/locks/0/key")
            .and_then(Value::as_str),
        Some("plan:beta")
    );
    assert_eq!(
        root_record
            .pointer("/schedule/preconditions/0/args/branch")
            .and_then(Value::as_str),
        Some("draft/beta")
    );

    Ok(())
}

#[test]
fn test_run_file_selector_enqueues_workflow() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    write_single_run_template(&repo, ".vizier/workflows/single.toml", "true")?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/single.toml",
            "--format",
            "json",
        ],
    )?;

    let run_id = payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing run_id")?;
    let manifest_path = repo.path().join(format!(".vizier/jobs/runs/{run_id}.json"));
    assert!(
        manifest_path.exists(),
        "missing run manifest: {manifest_path:?}"
    );

    Ok(())
}

#[test]
fn test_run_workflow_help_writes_no_manifests_or_jobs() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    write_single_run_template(&repo, ".vizier/workflows/single.toml", "true")?;
    let before_run_manifests = count_run_manifests(&repo)?;
    let before_jobs = count_job_records(&repo)?;

    let output = repo.vizier_output(&[
        "run",
        "file:.vizier/workflows/single.toml",
        "--help",
        "--no-ansi",
    ])?;
    assert!(
        output.status.success(),
        "workflow help should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Workflow: file:.vizier/workflows/single.toml")
            && stdout.contains("Run options:"),
        "expected workflow-specific help output, got: {stdout}"
    );

    assert_eq!(
        count_run_manifests(&repo)?,
        before_run_manifests,
        "workflow help must not write run manifests"
    );
    assert_eq!(
        count_job_records(&repo)?,
        before_jobs,
        "workflow help must not enqueue jobs"
    );

    Ok(())
}

#[test]
fn test_run_check_validates_and_writes_no_manifests_or_jobs() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    write_single_run_template(&repo, ".vizier/workflows/single.toml", "true")?;
    let before_run_manifests = count_run_manifests(&repo)?;
    let before_jobs = count_job_records(&repo)?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/single.toml",
            "--check",
            "--format",
            "json",
        ],
    )?;

    assert_eq!(
        payload.get("outcome").and_then(Value::as_str),
        Some("workflow_validation_passed")
    );
    assert_eq!(
        payload.get("workflow_template_id").and_then(Value::as_str),
        Some("template.single")
    );
    assert_eq!(
        payload
            .get("workflow_template_version")
            .and_then(Value::as_str),
        Some("v1")
    );
    assert_eq!(payload.get("node_count").and_then(Value::as_u64), Some(1));
    assert!(
        payload
            .get("workflow_template_selector")
            .and_then(Value::as_str)
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false),
        "expected non-empty workflow selector: {payload}"
    );

    assert_eq!(
        count_run_manifests(&repo)?,
        before_run_manifests,
        "check mode must not write run manifests"
    );
    assert_eq!(
        count_job_records(&repo)?,
        before_jobs,
        "check mode must not enqueue jobs"
    );

    Ok(())
}

#[test]
fn test_run_check_text_output_contract() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    write_single_run_template(&repo, ".vizier/workflows/single.toml", "true")?;
    let output = repo.vizier_output(&["run", "file:.vizier/workflows/single.toml", "--check"])?;
    assert!(
        output.status.success(),
        "check mode should succeed for valid flow: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Outcome") && stdout.contains("Workflow validation passed"),
        "missing check-mode outcome line: {stdout}"
    );
    assert!(
        stdout.contains("Selector") && stdout.contains("file:.vizier/workflows/single.toml"),
        "missing selector line in check-mode output: {stdout}"
    );
    assert!(
        stdout.contains("Template") && stdout.contains("template.single@v1"),
        "missing template line in check-mode output: {stdout}"
    );
    assert!(
        stdout.contains("Nodes") && stdout.contains("1"),
        "missing node count line in check-mode output: {stdout}"
    );

    Ok(())
}

#[test]
fn test_run_check_rejects_runtime_only_flags() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    write_single_run_template(&repo, ".vizier/workflows/single.toml", "true")?;
    let before_run_manifests = count_run_manifests(&repo)?;
    let before_jobs = count_job_records(&repo)?;

    for args in [
        vec![
            "run",
            "file:.vizier/workflows/single.toml",
            "--check",
            "--follow",
        ],
        vec![
            "run",
            "file:.vizier/workflows/single.toml",
            "--check",
            "--after",
            "job-123",
        ],
        vec![
            "run",
            "file:.vizier/workflows/single.toml",
            "--check",
            "--require-approval",
        ],
        vec![
            "run",
            "file:.vizier/workflows/single.toml",
            "--check",
            "--no-require-approval",
        ],
        vec![
            "run",
            "file:.vizier/workflows/single.toml",
            "--check",
            "--repeat",
            "2",
        ],
    ] {
        let output = repo.vizier_output(&args)?;
        assert!(
            !output.status.success(),
            "expected args {:?} to fail in check mode",
            args
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("--check"),
            "expected clap conflict mentioning --check, got: {stderr}"
        );
    }

    assert_eq!(
        count_run_manifests(&repo)?,
        before_run_manifests,
        "invalid check invocations must not write run manifests"
    );
    assert_eq!(
        count_job_records(&repo)?,
        before_jobs,
        "invalid check invocations must not enqueue jobs"
    );

    Ok(())
}

#[test]
fn test_run_check_reuses_queue_time_validation_failures_without_side_effects() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/check-unresolved-needs.json",
        "{\n\
  \"id\": \"template.check.unresolved.needs\",\n\
  \"version\": \"v1\",\n\
  \"nodes\": [\n\
    {\n\
      \"id\": \"single\",\n\
      \"kind\": \"shell\",\n\
      \"uses\": \"cap.env.shell.command.run\",\n\
      \"args\": {\"script\": \"true\"},\n\
      \"needs\": [\n\
        {\"plan_doc\": {\"slug\": \"${missing}\", \"branch\": \"main\"}}\n\
      ]\n\
    }\n\
  ]\n\
}\n",
    )?;
    repo.write(
        ".vizier/check-invalid-coercion.json",
        "{\n\
  \"id\": \"template.check.invalid.coercion\",\n\
  \"version\": \"v1\",\n\
  \"params\": {\n\
    \"require_gate\": \"true\"\n\
  },\n\
  \"nodes\": [\n\
    {\n\
      \"id\": \"single\",\n\
      \"kind\": \"shell\",\n\
      \"uses\": \"cap.env.shell.command.run\",\n\
      \"args\": {\"script\": \"true\"},\n\
      \"gates\": [\n\
        {\"kind\": \"approval\", \"required\": \"${require_gate}\"}\n\
      ]\n\
    }\n\
  ]\n\
}\n",
    )?;
    repo.write(
        ".vizier/check-legacy.toml",
        "id = \"template.check.legacy\"\n\
version = \"v1\"\n\
[[nodes]]\n\
id = \"legacy\"\n\
kind = \"builtin\"\n\
uses = \"vizier.merge.integrate\"\n",
    )?;

    let before_run_manifests = count_run_manifests(&repo)?;
    let before_jobs = count_job_records(&repo)?;

    let unresolved =
        repo.vizier_output(&["run", "file:.vizier/check-unresolved-needs.json", "--check"])?;
    assert!(
        !unresolved.status.success(),
        "unresolved placeholders should fail in check mode"
    );
    let unresolved_stderr = String::from_utf8_lossy(&unresolved.stderr);
    assert!(
        unresolved_stderr.contains("unresolved parameter `missing`")
            && unresolved_stderr.contains("nodes[single].needs[0].plan_doc.slug"),
        "expected unresolved placeholder field-path error in check mode, got: {unresolved_stderr}"
    );

    let coercion = repo.vizier_output(&[
        "run",
        "file:.vizier/check-invalid-coercion.json",
        "--set",
        "require_gate=maybe",
        "--check",
    ])?;
    assert!(
        !coercion.status.success(),
        "invalid bool coercion should fail in check mode"
    );
    let coercion_stderr = String::from_utf8_lossy(&coercion.stderr);
    assert!(
        coercion_stderr.contains("invalid bool value `maybe`")
            && coercion_stderr.contains("nodes[single].gates[0].approval.required"),
        "expected typed coercion error in check mode, got: {coercion_stderr}"
    );

    let legacy = repo.vizier_output(&["run", "file:.vizier/check-legacy.toml", "--check"])?;
    assert!(
        !legacy.status.success(),
        "legacy uses label should fail in check mode"
    );
    let legacy_stderr = String::from_utf8_lossy(&legacy.stderr);
    assert!(
        legacy_stderr.contains("uses unknown label") || legacy_stderr.contains("validation"),
        "expected legacy uses validation failure, got: {legacy_stderr}"
    );

    assert_eq!(
        count_run_manifests(&repo)?,
        before_run_manifests,
        "failed check-mode validation must not write run manifests"
    );
    assert_eq!(
        count_job_records(&repo)?,
        before_jobs,
        "failed check-mode validation must not enqueue jobs"
    );

    Ok(())
}

#[test]
fn test_run_dynamic_named_flags_expand_to_set_overrides() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflows/named-flags.toml",
        "id = \"template.named.flags\"\n\
version = \"v1\"\n\
[params]\n\
message = \"default\"\n\
[[nodes]]\n\
id = \"single\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
script = \"echo ${message} > named-flags.txt\"\n",
    )?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/named-flags.toml",
            "--message",
            "overridden",
            "--format",
            "json",
        ],
    )?;

    let run_id = payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing run_id")?;
    let manifest = load_run_manifest(&repo, run_id)?;
    let script = manifest
        .pointer("/nodes/single/args/script")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        script.contains("overridden"),
        "expected named flag override in manifest args, got: {script}"
    );

    Ok(())
}

#[test]
fn test_run_positional_inputs_follow_cli_positional_mapping() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflows/positional.toml",
        "id = \"template.positional\"\n\
version = \"v1\"\n\
[cli]\n\
positional = [\"first\", \"second\"]\n\
[params]\n\
first = \"\"\n\
second = \"\"\n\
[[nodes]]\n\
id = \"single\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
script = \"echo ${first}-${second} > positional.txt\"\n",
    )?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/positional.toml",
            "alpha",
            "beta",
            "--format",
            "json",
        ],
    )?;

    let run_id = payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing run_id")?;
    let manifest = load_run_manifest(&repo, run_id)?;
    let script = manifest
        .pointer("/nodes/single/args/script")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        script.contains("alpha-beta"),
        "expected positional overrides in manifest args, got: {script}"
    );

    Ok(())
}

#[test]
fn test_run_stage_named_alias_flags_map_to_params() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;
    write_stage_alias_test_config(&repo)?;
    repo.write("specs/DEFAULT.md", "Stage alias mapping smoke spec.\n")?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "draft",
            "--name",
            "alias-flag-smoke",
            "--file",
            "specs/DEFAULT.md",
            "--follow",
            "--format",
            "json",
        ],
    )?;

    let run_id = payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing run_id")?;
    let manifest = load_run_manifest(&repo, run_id)?;

    assert_eq!(
        manifest
            .pointer("/nodes/worktree_prepare/args/slug")
            .and_then(Value::as_str),
        Some("alias-flag-smoke"),
        "expected --name to map to worktree slug: {manifest}"
    );
    assert_eq!(
        manifest
            .pointer("/nodes/persist_plan/args/spec_file")
            .and_then(Value::as_str),
        Some("specs/DEFAULT.md"),
        "expected --file to map to persist_plan spec_file: {manifest}"
    );

    Ok(())
}

#[test]
fn test_run_stage_file_input_is_inlined_at_enqueue() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;
    write_stage_alias_test_config(&repo)?;

    let spec_rel = "specs/LOCAL_UNTRACKED.md";
    let spec_text = "Untracked draft spec line 1.\nUntracked draft spec line 2.\n";
    repo.write(spec_rel, spec_text)?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "draft",
            "--name",
            "inline-untracked",
            "--file",
            spec_rel,
            "--follow",
            "--format",
            "json",
        ],
    )?;

    let run_id = payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing run_id")?;
    let manifest = load_run_manifest(&repo, run_id)?;

    assert_eq!(
        manifest
            .pointer("/nodes/persist_plan/args/spec_file")
            .and_then(Value::as_str),
        Some(spec_rel),
        "expected persist_plan spec_file to keep the requested path: {manifest}"
    );
    assert_eq!(
        manifest
            .pointer("/nodes/persist_plan/args/spec_text")
            .and_then(Value::as_str),
        Some(spec_text),
        "expected persist_plan spec_text to snapshot file contents at enqueue: {manifest}"
    );

    Ok(())
}

#[test]
fn test_run_entrypoint_preflight_reports_missing_root_inputs() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;
    write_stage_alias_test_config(&repo)?;

    let output = repo.vizier_output(&["run", "draft", "specs/DEFAULT.md"])?;
    assert!(
        !output.status.success(),
        "draft run should fail when required root-node inputs are missing"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error: missing required input for workflow `draft`")
            && stderr.contains(
                "usage: vizier run draft [--file <file>] [--name <name>] [--branch <branch>]"
            )
            && stderr.contains("example: vizier run draft --file LIBRARY.md --name my-change")
            && stderr.contains("example (positional): vizier run draft LIBRARY.md my-change")
            && stderr.contains("hint: vizier run draft --help"),
        "expected concise CLI-style input guidance, got: {stderr}"
    );
    assert!(
        !stderr.contains("entry node `worktree_prepare`") && !stderr.contains("`worktree.prepare`"),
        "internal node/capability IDs should not be exposed in the primary error text: {stderr}"
    );

    Ok(())
}

#[test]
fn test_seeded_stage_templates_use_canonical_labels() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    let checks: [(&str, &[&str]); 3] = [
        (
            ".vizier/workflows/draft.hcl",
            &[
                "version = \"v2\"",
                "positional = [\"spec_file\", \"slug\", \"branch\"]",
                "named = {",
                "file = \"spec_file\"",
                "name = \"slug\"",
                "missing_producer = \"wait\"",
                "id = \"plan_text\"",
                "id = \"plan_branch\"",
                "id = \"plan_doc\"",
                "cap.env.builtin.worktree.prepare",
                "slug = \"$${slug}\"",
                "cap.env.builtin.prompt.resolve",
                "prompt_file = \".vizier/prompts/DRAFT_PROMPTS.md\"",
                "cap.agent.invoke",
                "type_id = \"plan_text\"",
                "key = \"draft_plan:$${slug}\"",
                "cap.env.builtin.plan.persist",
                "plan_branch = { slug = \"$${slug}\", branch = \"$${branch}\" }",
                "plan_doc = { slug = \"$${slug}\", branch = \"$${branch}\" }",
                "cap.env.builtin.git.stage",
                "cap.env.builtin.git.commit",
            ],
        ),
        (
            ".vizier/workflows/approve.hcl",
            &[
                "version = \"v2\"",
                "positional = [\"slug\", \"branch\"]",
                "named = {",
                "name = \"slug\"",
                "missing_producer = \"wait\"",
                "id = \"plan_branch\"",
                "id = \"plan_doc\"",
                "id = \"stage_token\"",
                "cap.env.builtin.worktree.prepare",
                "slug = \"$${slug}\"",
                "plan_branch = { slug = \"$${slug}\", branch = \"$${branch}\" }",
                "plan_doc = { slug = \"$${slug}\", branch = \"$${branch}\" }",
                "cap.env.builtin.prompt.resolve",
                "prompt_file = \".vizier/prompts/APPROVE_PROMPTS.md\"",
                "cap.agent.invoke",
                "cap.env.builtin.git.stage",
                "cap.env.builtin.git.commit",
                "control.gate.stop_condition",
                "type_id = \"stage_token\"",
                "key = \"approve:$${slug}\"",
            ],
        ),
        (
            ".vizier/workflows/merge.hcl",
            &[
                "version = \"v2\"",
                "positional = [\"slug\", \"branch\", \"target_branch\"]",
                "named = {",
                "name = \"slug\"",
                "source = \"branch\"",
                "target = \"target_branch\"",
                "missing_producer = \"wait\"",
                "conflict_auto_resolve = \"true\"",
                "id = \"stage_token\"",
                "cap.env.builtin.git.integrate_plan_branch",
                "type_id = \"stage_token\"",
                "key = \"approve:$${slug}\"",
                "control.gate.conflict_resolution",
                "prompt_file = \".vizier/prompts/MERGE_PROMPTS.md\"",
                "control.gate.cicd",
                "control.terminal",
            ],
        ),
    ];

    for (relative_path, expected_tokens) in checks {
        let path = repo.path().join(relative_path);
        let contents = fs::read_to_string(&path)?;
        assert!(
            !contents.contains("uses = \"vizier."),
            "legacy uses label should not appear in {}:\n{}",
            path.display(),
            contents
        );
        for token in expected_tokens {
            assert!(
                contents.contains(token),
                "expected token `{token}` in {}:\n{}",
                path.display(),
                contents
            );
        }
    }

    Ok(())
}

#[test]
fn test_run_draft_stage_persists_agent_output_as_plan_doc() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    let slug = "draft-plan-visible";
    let sentinel = "mock agent response";
    write_stage_alias_test_config(&repo)?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "draft",
            "--set",
            &format!("slug={slug}"),
            "--set",
            "spec_text=Ensure draft plan body is sourced from agent output.",
            "--follow",
            "--format",
            "json",
        ],
    )?;
    let run_id = payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing draft run_id")?;
    let manifest = load_run_manifest(&repo, run_id)?;

    for node in ["persist_plan", "stage_commit", "invoke_agent", "stop_gate"] {
        let job_id = manifest_node_job_id(&manifest, node)?;
        let record = read_job_record(&repo, &job_id)?;
        assert_eq!(
            record.get("status").and_then(Value::as_str),
            Some("succeeded"),
            "draft node `{node}` should succeed: {record}"
        );
    }
    let invoke_job = manifest_node_job_id(&manifest, "invoke_agent")?;
    let persist_job = manifest_node_job_id(&manifest, "persist_plan")?;
    let persist_record = read_job_record(&repo, &persist_job)?;
    let persist_after = persist_record
        .pointer("/schedule/after")
        .and_then(Value::as_array)
        .ok_or("persist_plan missing schedule.after")?;
    assert!(
        persist_after.iter().any(|entry| {
            entry.pointer("/job_id").and_then(Value::as_str) == Some(invoke_job.as_str())
        }),
        "persist_plan should run after invoke_agent: {persist_record}"
    );

    let branch = persist_record
        .pointer("/metadata/branch")
        .and_then(Value::as_str)
        .ok_or("persist_plan missing metadata.branch")?;
    let plan_slug = persist_record
        .pointer("/metadata/plan")
        .and_then(Value::as_str)
        .ok_or("persist_plan missing metadata.plan")?;
    let plan_doc_text = branch_blob_text(
        &repo,
        branch,
        &format!(".vizier/implementation-plans/{plan_slug}.md"),
    )?;
    assert!(
        plan_doc_text.contains("## Implementation Plan"),
        "expected implementation plan section in persisted doc: {plan_doc_text}"
    );
    assert!(
        plan_doc_text.contains(sentinel),
        "expected persisted plan body to include agent output sentinel, got: {plan_doc_text}"
    );

    Ok(())
}

#[test]
fn test_run_draft_stage_force_stages_plan_doc_when_ignored() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;
    write_stage_alias_test_config(&repo)?;

    let ignore_path = repo.path().join(".gitignore");
    let mut ignore = fs::read_to_string(&ignore_path).unwrap_or_default();
    if !ignore.contains(".vizier/implementation-plans") {
        if !ignore.is_empty() && !ignore.ends_with('\n') {
            ignore.push('\n');
        }
        ignore.push_str(".vizier/implementation-plans\n");
        fs::write(&ignore_path, &ignore)?;
        repo.git(&["add", ".gitignore"])?;
        repo.git(&["commit", "-m", "test: ignore implementation plans"])?;
    }

    let payload = run_json(
        &repo,
        &[
            "run",
            "draft",
            "--set",
            "slug=ignored-plan-doc",
            "--set",
            "spec_text=Ensure ignored plan docs are force-staged.",
            "--follow",
            "--format",
            "json",
        ],
    )?;
    let run_id = payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing draft run_id")?;
    let manifest = load_run_manifest(&repo, run_id)?;
    let persist_job = manifest_node_job_id(&manifest, "persist_plan")?;
    let persist_record = read_job_record(&repo, &persist_job)?;
    let branch = persist_record
        .pointer("/metadata/branch")
        .and_then(Value::as_str)
        .ok_or("persist_plan missing metadata.branch")?;
    let slug = persist_record
        .pointer("/metadata/plan")
        .and_then(Value::as_str)
        .ok_or("persist_plan missing metadata.plan")?;

    assert!(
        branch_blob_text(
            &repo,
            branch,
            &format!(".vizier/implementation-plans/{slug}.md")
        )
        .is_ok(),
        "expected ignored plan doc to be committed on {branch}"
    );

    Ok(())
}

#[test]
fn test_run_stage_aliases_execute_templates_smoke() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;
    write_stage_alias_test_config(&repo)?;

    let draft_payload = run_json(
        &repo,
        &[
            "run",
            "draft",
            "--set",
            "slug=stage-draft-smoke",
            "--set",
            "spec_text=Ship the stage smoke path.",
            "--follow",
            "--format",
            "json",
        ],
    )?;
    let draft_run_id = draft_payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing draft run_id")?;
    let draft_manifest = load_run_manifest(&repo, draft_run_id)?;

    let persist_job = manifest_node_job_id(&draft_manifest, "persist_plan")?;
    let persist_record = read_job_record(&repo, &persist_job)?;
    assert_eq!(
        persist_record.get("status").and_then(Value::as_str),
        Some("succeeded"),
        "draft persist_plan should succeed: {persist_record}"
    );
    let persist_branch = persist_record
        .pointer("/metadata/branch")
        .and_then(Value::as_str)
        .ok_or("persist_plan missing metadata.branch")?;
    let persist_slug = persist_record
        .pointer("/metadata/plan")
        .and_then(Value::as_str)
        .ok_or("persist_plan missing metadata.plan")?;
    repo.git(&[
        "show-ref",
        "--verify",
        "--quiet",
        &format!("refs/heads/{persist_branch}"),
    ])?;
    assert!(
        branch_blob_text(
            &repo,
            persist_branch,
            &format!(".vizier/implementation-plans/{persist_slug}.md")
        )
        .is_ok(),
        "expected draft plan doc on draft branch"
    );

    seed_plan_branch(&repo, "approve-smoke", "draft/approve-smoke")?;

    let approve_payload = run_json(
        &repo,
        &[
            "run",
            "approve",
            "--set",
            "slug=approve-smoke",
            "--set",
            "branch=draft/approve-smoke",
            "--follow",
            "--format",
            "json",
        ],
    )?;
    let approve_run_id = approve_payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing approve run_id")?;
    let approve_manifest = load_run_manifest(&repo, approve_run_id)?;

    for node in ["stage_commit", "stop_gate"] {
        let job_id = manifest_node_job_id(&approve_manifest, node)?;
        let record = read_job_record(&repo, &job_id)?;
        assert_eq!(
            record.get("status").and_then(Value::as_str),
            Some("succeeded"),
            "approve node `{node}` should succeed: {record}"
        );
    }

    seed_plan_branch(&repo, "merge-smoke", "draft/merge-smoke")?;
    repo.git(&["checkout", "draft/merge-smoke"])?;
    repo.write("merge-smoke.txt", "merge smoke branch change\n")?;
    repo.git(&["add", "merge-smoke.txt"])?;
    repo.git(&["commit", "-m", "feat: merge smoke branch"])?;
    repo.git(&["checkout", "master"])?;

    run_stage_approve_follow(&repo, "merge-smoke", "draft/merge-smoke")?;

    let merge_payload = run_json(
        &repo,
        &[
            "run",
            "merge",
            "--set",
            "slug=merge-smoke",
            "--set",
            "branch=draft/merge-smoke",
            "--set",
            "target_branch=master",
            "--set",
            "cicd_script=true",
            "--set",
            "merge_message=feat: merge plan merge-smoke",
            "--follow",
            "--format",
            "json",
        ],
    )?;
    let merge_run_id = merge_payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing merge run_id")?;
    let merge_manifest = load_run_manifest(&repo, merge_run_id)?;

    for node in ["merge_integrate", "merge_gate_cicd"] {
        let job_id = manifest_node_job_id(&merge_manifest, node)?;
        let record = read_job_record(&repo, &job_id)?;
        assert_eq!(
            record.get("status").and_then(Value::as_str),
            Some("succeeded"),
            "merge node `{node}` should succeed: {record}"
        );
    }

    let subject = head_subject(&repo)?;
    assert!(
        subject.contains("feat: merge plan merge-smoke"),
        "expected merge commit subject to include slug, got: {subject}"
    );

    Ok(())
}

#[test]
fn test_run_approve_stage_succeeds_after_draft_when_branch_is_implicit() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;
    write_stage_alias_test_config(&repo)?;

    run_json(
        &repo,
        &[
            "run",
            "draft",
            "--set",
            "slug=approve-implicit-branch",
            "--set",
            "spec_text=Seed approve dependency artifacts.",
            "--follow",
            "--format",
            "json",
        ],
    )?;

    let approve_payload = run_json(
        &repo,
        &[
            "run",
            "approve",
            "approve-implicit-branch",
            "--follow",
            "--format",
            "json",
        ],
    )?;
    let approve_run_id = approve_payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing approve run_id")?;
    let approve_manifest = load_run_manifest(&repo, approve_run_id)?;

    for node in ["worktree_prepare", "stage_commit", "stop_gate"] {
        let job_id = manifest_node_job_id(&approve_manifest, node)?;
        let record = read_job_record(&repo, &job_id)?;
        assert_eq!(
            record.get("status").and_then(Value::as_str),
            Some("succeeded"),
            "approve node `{node}` should succeed with implicit branch: {record}"
        );
    }

    Ok(())
}

#[test]
fn test_run_git_commit_reads_message_from_custom_payload() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;
    write_stage_alias_test_config(&repo)?;

    repo.write(
        ".vizier/workflows/commit-from-payload.toml",
        "id = \"template.commit.from_payload\"\n\
version = \"v1\"\n\
[[artifact_contracts]]\n\
id = \"prompt_text\"\n\
version = \"v1\"\n\
[[artifact_contracts]]\n\
id = \"commit_message\"\n\
version = \"v1\"\n\
[[nodes]]\n\
id = \"seed_change\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
script = \"echo payload-commit >> payload-commit.txt\"\n\
[nodes.on]\n\
succeeded = [\"resolve_prompt\", \"stage_files\"]\n\
[[nodes]]\n\
id = \"resolve_prompt\"\n\
kind = \"builtin\"\n\
uses = \"cap.env.builtin.prompt.resolve\"\n\
[nodes.args]\n\
prompt_text = \"Write a concise commit subject.\"\n\
[nodes.produces]\n\
succeeded = [{ custom = { type_id = \"prompt_text\", key = \"commit_prompt\" } }]\n\
[[nodes.after]]\n\
node_id = \"seed_change\"\n\
[nodes.on]\n\
succeeded = [\"invoke_agent\"]\n\
[[nodes]]\n\
id = \"invoke_agent\"\n\
kind = \"agent\"\n\
uses = \"cap.agent.invoke\"\n\
[[nodes.needs]]\n\
custom = { type_id = \"prompt_text\", key = \"commit_prompt\" }\n\
[nodes.produces]\n\
succeeded = [{ custom = { type_id = \"commit_message\", key = \"subject\" } }]\n\
[[nodes.after]]\n\
node_id = \"resolve_prompt\"\n\
[[nodes]]\n\
id = \"stage_files\"\n\
kind = \"builtin\"\n\
uses = \"cap.env.builtin.git.stage\"\n\
[nodes.args]\n\
files_json = \"[\\\"payload-commit.txt\\\"]\"\n\
[[nodes.after]]\n\
node_id = \"seed_change\"\n\
[[nodes]]\n\
id = \"commit_changes\"\n\
kind = \"builtin\"\n\
uses = \"cap.env.builtin.git.commit\"\n\
[nodes.args]\n\
message = \"read_payload(commit_message)\"\n\
[[nodes.needs]]\n\
custom = { type_id = \"commit_message\", key = \"subject\" }\n\
[[nodes.after]]\n\
node_id = \"stage_files\"\n\
[[nodes.after]]\n\
node_id = \"invoke_agent\"\n\
[nodes.on]\n\
succeeded = [\"terminal\"]\n\
[[nodes]]\n\
id = \"terminal\"\n\
kind = \"gate\"\n\
uses = \"control.terminal\"\n\
[[nodes.after]]\n\
node_id = \"commit_changes\"\n",
    )?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/commit-from-payload.toml",
            "--follow",
            "--format",
            "json",
        ],
    )?;
    let run_id = payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing run_id")?;
    let manifest = load_run_manifest(&repo, run_id)?;

    for node in [
        "seed_change",
        "resolve_prompt",
        "invoke_agent",
        "stage_files",
        "commit_changes",
    ] {
        let job_id = manifest_node_job_id(&manifest, node)?;
        let record = read_job_record(&repo, &job_id)?;
        assert_eq!(
            record.get("status").and_then(Value::as_str),
            Some("succeeded"),
            "workflow node `{node}` should succeed: {record}"
        );
    }

    let subject = head_subject(&repo)?;
    assert!(
        subject.contains("mock agent response"),
        "expected commit subject from agent payload, got: {subject}"
    );

    Ok(())
}

#[test]
fn test_run_stage_chain_queues_without_after_using_artifact_dependencies() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;
    write_stage_alias_test_config(&repo)?;

    let slug = "opt-chain";
    let branch = format!("draft/{slug}");

    let draft_payload = run_json(
        &repo,
        &[
            "run",
            "draft",
            "--set",
            &format!("slug={slug}"),
            "--set",
            &format!("branch={branch}"),
            "--set",
            "spec_text=Queue stage chain without --after.",
            "--format",
            "json",
        ],
    )?;
    let draft_root = draft_payload
        .get("root_job_ids")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .ok_or("missing draft root job id")?;
    let draft_run_id = draft_payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing draft run id")?;
    let draft_manifest = load_run_manifest(&repo, draft_run_id)?;

    let approve_payload = run_json(
        &repo,
        &[
            "run",
            "approve",
            "--set",
            &format!("slug={slug}"),
            "--set",
            &format!("branch={branch}"),
            "--format",
            "json",
        ],
    )?;
    let approve_root = approve_payload
        .get("root_job_ids")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .ok_or("missing approve root job id")?;
    let approve_run_id = approve_payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing approve run id")?;
    let approve_manifest = load_run_manifest(&repo, approve_run_id)?;

    let merge_payload = run_json(
        &repo,
        &[
            "run",
            "merge",
            "--set",
            &format!("slug={slug}"),
            "--set",
            &format!("branch={branch}"),
            "--set",
            "target_branch=master",
            "--set",
            "cicd_script=true",
            "--set",
            "merge_message=feat: optimistic chain merge",
            "--format",
            "json",
        ],
    )?;
    let merge_root = merge_payload
        .get("root_job_ids")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .ok_or("missing merge root job id")?;
    let merge_run_id = merge_payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing merge run id")?;
    let merge_manifest = load_run_manifest(&repo, merge_run_id)?;

    let approve_prepare_job = manifest_node_job_id(&approve_manifest, "worktree_prepare")?;
    let merge_integrate_job = manifest_node_job_id(&merge_manifest, "merge_integrate")?;

    wait_for_job_status(
        &repo,
        &approve_prepare_job,
        "waiting_on_deps",
        Duration::from_secs(10),
    )?;
    wait_for_job_status(
        &repo,
        &merge_integrate_job,
        "waiting_on_deps",
        Duration::from_secs(10),
    )?;

    let approve_wait_detail = read_job_record(&repo, &approve_prepare_job)?
        .pointer("/schedule/wait_reason/detail")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    assert!(
        approve_wait_detail.contains("plan_branch:") || approve_wait_detail.contains("plan_doc:"),
        "approve should wait on draft plan artifacts: {approve_wait_detail}"
    );

    let merge_record = read_job_record(&repo, &merge_integrate_job)?;
    let merge_dependencies = merge_record
        .pointer("/schedule/dependencies")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        merge_dependencies.iter().any(|value| {
            value
                .pointer("/artifact/custom/type_id")
                .and_then(Value::as_str)
                == Some("stage_token")
                && value
                    .pointer("/artifact/custom/key")
                    .and_then(Value::as_str)
                    == Some("approve:opt-chain")
        }),
        "merge integrate should declare stage token dependency: {merge_record}"
    );

    wait_for_manifest_jobs(&repo, &draft_manifest, Duration::from_secs(40))?;
    wait_for_manifest_jobs(&repo, &approve_manifest, Duration::from_secs(40))?;
    wait_for_manifest_jobs(&repo, &merge_manifest, Duration::from_secs(40))?;

    let draft_root_record = read_job_record(&repo, draft_root)?;
    assert_eq!(
        draft_root_record.get("status").and_then(Value::as_str),
        Some("succeeded"),
        "draft root should complete successfully: {draft_root_record}"
    );
    let approve_root_record = read_job_record(&repo, approve_root)?;
    assert_eq!(
        approve_root_record.get("status").and_then(Value::as_str),
        Some("succeeded"),
        "approve root should complete successfully: {approve_root_record}"
    );
    let merge_root_record = read_job_record(&repo, merge_root)?;
    assert_eq!(
        merge_root_record.get("status").and_then(Value::as_str),
        Some("succeeded"),
        "merge root should complete successfully: {merge_root_record}"
    );

    Ok(())
}

#[test]
fn test_run_custom_wait_policy_unblocks_consumer_when_producer_arrives() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;
    write_stage_token_dependency_templates(&repo)?;

    let slug = "late-producer";
    let consumer_payload = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/custom-stage-token-consumer-wait.toml",
            "--set",
            &format!("slug={slug}"),
            "--format",
            "json",
        ],
    )?;
    let consumer_run_id = consumer_payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing consumer run id")?;
    let consumer_manifest = load_run_manifest(&repo, consumer_run_id)?;
    let consumer_root = consumer_payload
        .get("root_job_ids")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .ok_or("missing consumer root job id")?;

    wait_for_job_status(
        &repo,
        consumer_root,
        "waiting_on_deps",
        Duration::from_secs(10),
    )?;
    let consumer_wait = read_job_record(&repo, consumer_root)?
        .pointer("/schedule/wait_reason/detail")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    assert!(
        consumer_wait.contains("awaiting producer for custom:stage_token:approve:late-producer"),
        "consumer should wait on unknown producer in optimistic mode: {consumer_wait}"
    );

    let producer_payload = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/custom-stage-token-producer.toml",
            "--set",
            &format!("slug={slug}"),
            "--format",
            "json",
        ],
    )?;
    let producer_run_id = producer_payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing producer run id")?;
    let producer_manifest = load_run_manifest(&repo, producer_run_id)?;

    wait_for_manifest_jobs(&repo, &producer_manifest, Duration::from_secs(20))?;
    wait_for_manifest_jobs(&repo, &consumer_manifest, Duration::from_secs(20))?;

    let consumer_record = read_job_record(&repo, consumer_root)?;
    assert_eq!(
        consumer_record.get("status").and_then(Value::as_str),
        Some("succeeded"),
        "consumer should unblock after producer succeeds: {consumer_record}"
    );

    Ok(())
}

#[test]
fn test_run_custom_block_policy_still_blocks_missing_producer() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;
    write_stage_token_dependency_templates(&repo)?;

    let output = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/custom-stage-token-consumer-block.toml",
            "--set",
            "slug=strict-block",
            "--format",
            "json",
        ],
    )?;
    let root = output
        .get("root_job_ids")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .ok_or("missing strict root job id")?;

    wait_for_job_status(
        &repo,
        root,
        "blocked_by_dependency",
        Duration::from_secs(10),
    )?;
    let detail = read_job_record(&repo, root)?
        .pointer("/schedule/wait_reason/detail")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    assert!(
        detail.contains("missing custom:stage_token:approve:strict-block"),
        "strict policy should block missing producers: {detail}"
    );

    Ok(())
}

#[test]
fn test_run_merge_stage_default_slug_derives_source_branch() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;
    write_stage_alias_test_config(&repo)?;

    seed_plan_branch(&repo, "default", "draft/default")?;
    repo.git(&["checkout", "draft/default"])?;
    repo.write("merge-default.txt", "merge default branch change\n")?;
    repo.git(&["add", "merge-default.txt"])?;
    repo.git(&["commit", "-m", "feat: merge default branch"])?;
    repo.git(&["checkout", "master"])?;

    run_stage_approve_follow(&repo, "default", "draft/default")?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "merge",
            "default",
            "--set",
            "cicd_script=true",
            "--set",
            "merge_message=feat: merge plan default",
            "--follow",
            "--format",
            "json",
        ],
    )?;
    let run_id = payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing run_id")?;
    let manifest = load_run_manifest(&repo, run_id)?;

    for node in ["merge_integrate", "merge_gate_cicd"] {
        let job_id = manifest_node_job_id(&manifest, node)?;
        let record = read_job_record(&repo, &job_id)?;
        assert_eq!(
            record.get("status").and_then(Value::as_str),
            Some("succeeded"),
            "merge node `{node}` should succeed: {record}"
        );
    }

    assert_eq!(
        fs::read_to_string(repo.path().join("merge-default.txt"))?.as_str(),
        "merge default branch change\n"
    );

    Ok(())
}

#[test]
fn test_run_merge_stage_embeds_plan_content_and_removes_source_plan_doc() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;
    write_stage_alias_test_config(&repo)?;

    seed_plan_branch(&repo, "merge-plan-doc", "draft/merge-plan-doc")?;
    repo.git(&["checkout", "draft/merge-plan-doc"])?;
    repo.write("merge-plan-doc.txt", "merge branch payload\n")?;
    repo.git(&["add", "merge-plan-doc.txt"])?;
    repo.git(&["commit", "-m", "feat: merge branch payload"])?;
    repo.git(&["checkout", "master"])?;

    run_stage_approve_follow(&repo, "merge-plan-doc", "draft/merge-plan-doc")?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "merge",
            "--set",
            "slug=merge-plan-doc",
            "--set",
            "branch=draft/merge-plan-doc",
            "--set",
            "target_branch=master",
            "--set",
            "delete_branch=false",
            "--set",
            "cicd_script=true",
            "--set",
            "merge_message=feat: merge plan merge-plan-doc",
            "--follow",
            "--format",
            "json",
        ],
    )?;
    let run_id = payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing run_id")?;
    let manifest = load_run_manifest(&repo, run_id)?;

    for node in ["merge_integrate", "merge_gate_cicd"] {
        let job_id = manifest_node_job_id(&manifest, node)?;
        let record = read_job_record(&repo, &job_id)?;
        assert_eq!(
            record.get("status").and_then(Value::as_str),
            Some("succeeded"),
            "merge node `{node}` should succeed: {record}"
        );
    }

    let message = head_message(&repo)?;
    assert!(
        message.contains("feat: merge plan merge-plan-doc"),
        "expected merge subject in message, got: {message}"
    );
    assert!(
        message.contains("## Implementation Plan"),
        "expected plan markdown in merge message, got: {message}"
    );
    assert!(
        message.contains("- Seeded step"),
        "expected seeded plan step in merge message, got: {message}"
    );

    let repo_handle = repo.repo();
    let draft_tip = repo_handle
        .find_branch("draft/merge-plan-doc", BranchType::Local)?
        .get()
        .peel_to_commit()?;
    assert!(
        draft_tip
            .tree()?
            .get_path(Path::new(".vizier/implementation-plans/merge-plan-doc.md"))
            .is_err(),
        "merge should remove the plan doc from the source branch tip before finalization"
    );

    Ok(())
}

#[test]
fn test_run_approve_stage_stop_condition_retry_loop() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflows/approve-retry.toml",
        "id = \"template.approve.retry\"\n\
version = \"v1\"\n\
[params]\n\
stop_condition_script = \"\"\n\
stop_condition_retries = \"3\"\n\
[[nodes]]\n\
id = \"seed_change\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
script = \"echo seed >> retry-smoke-seed.txt\"\n\
[nodes.on]\n\
succeeded = [\"stage_files\"]\n\
[[nodes]]\n\
id = \"stage_files\"\n\
kind = \"builtin\"\n\
uses = \"cap.env.builtin.git.stage\"\n\
[nodes.args]\n\
files_json = \"[\\\"retry-smoke-seed.txt\\\"]\"\n\
[nodes.on]\n\
succeeded = [\"stage_commit\"]\n\
[[nodes]]\n\
id = \"stage_commit\"\n\
kind = \"builtin\"\n\
uses = \"cap.env.builtin.git.commit\"\n\
[nodes.args]\n\
message = \"feat: retry smoke\"\n\
[[nodes.after]]\n\
node_id = \"stage_files\"\n\
[nodes.on]\n\
succeeded = [\"stop_gate\"]\n\
[[nodes]]\n\
id = \"stop_gate\"\n\
kind = \"gate\"\n\
uses = \"control.gate.stop_condition\"\n\
[[nodes.gates]]\n\
kind = \"script\"\n\
script = \"${stop_condition_script}\"\n\
policy = \"retry\"\n\
[nodes.retry]\n\
mode = \"until_gate\"\n\
budget = \"${stop_condition_retries}\"\n\
[nodes.on]\n\
failed = [\"stage_commit\"]\n\
succeeded = [\"terminal\"]\n\
[[nodes]]\n\
id = \"terminal\"\n\
kind = \"gate\"\n\
uses = \"control.terminal\"\n",
    )?;

    let stop_script = "attempt_file=.retry-gate-attempt; n=$(cat \"$attempt_file\" 2>/dev/null || echo 0); n=$((n+1)); echo \"$n\" > \"$attempt_file\"; [ \"$n\" -ge 2 ]";
    let stop_set = format!("stop_condition_script={stop_script}");
    let approve_payload = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/approve-retry.toml",
            "--set",
            stop_set.as_str(),
            "--set",
            "stop_condition_retries=3",
            "--format",
            "json",
        ],
    )?;
    let approve_run_id = approve_payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing retry run_id")?;
    let approve_manifest = load_run_manifest(&repo, approve_run_id)?;
    wait_for_manifest_jobs(&repo, &approve_manifest, Duration::from_secs(20))?;

    let stage_commit_job = manifest_node_job_id(&approve_manifest, "stage_commit")?;
    let stop_gate_job = manifest_node_job_id(&approve_manifest, "stop_gate")?;
    let stage_commit = read_job_record(&repo, &stage_commit_job)?;
    let stop_gate = read_job_record(&repo, &stop_gate_job)?;
    assert_eq!(
        stop_gate.get("status").and_then(Value::as_str),
        Some("succeeded"),
        "stop gate should eventually pass: {stop_gate}"
    );
    let stage_attempt = stage_commit
        .pointer("/metadata/workflow_node_attempt")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let gate_attempt = stop_gate
        .pointer("/metadata/workflow_node_attempt")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    assert!(
        stage_attempt >= 2,
        "stage commit should have been retried at least once: {stage_commit}"
    );
    assert!(
        gate_attempt >= 2,
        "stop gate should have been retried at least once: {stop_gate}"
    );

    Ok(())
}

#[test]
fn test_run_stage_jobs_control_paths_cover_approve_cancel_tail_attach_and_retry() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;
    write_stage_alias_test_config(&repo)?;

    let draft_payload = run_json(
        &repo,
        &[
            "run",
            "draft",
            "--set",
            "slug=control-smoke",
            "--set",
            "branch=draft/control-smoke",
            "--set",
            "spec_text=Control-path smoke plan.",
            "--require-approval",
            "--format",
            "json",
        ],
    )?;
    let draft_root = draft_payload
        .get("root_job_ids")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .ok_or("missing draft root job id")?;
    let draft_run_id = draft_payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing draft run_id")?;
    let draft_manifest = load_run_manifest(&repo, draft_run_id)?;

    let tail = repo.vizier_output(&["jobs", "tail", draft_root])?;
    assert!(
        tail.status.success(),
        "jobs tail should succeed for staged run job: {}",
        String::from_utf8_lossy(&tail.stderr)
    );

    let approve = repo.vizier_output(&["jobs", "approve", draft_root, "--format", "json"])?;
    assert!(
        approve.status.success(),
        "jobs approve should succeed for stage root: {}",
        String::from_utf8_lossy(&approve.stderr)
    );
    wait_for_manifest_jobs(&repo, &draft_manifest, Duration::from_secs(20))?;

    let attach = repo.vizier_output(&["jobs", "attach", draft_root])?;
    assert!(
        attach.status.success(),
        "jobs attach should succeed for completed stage root: {}",
        String::from_utf8_lossy(&attach.stderr)
    );

    let draft_gate = run_json(
        &repo,
        &[
            "run",
            "draft",
            "--set",
            "slug=cancel-gate-smoke",
            "--set",
            "branch=draft/cancel-gate-smoke",
            "--set",
            "spec_text=Cancel gate smoke plan.",
            "--require-approval",
            "--format",
            "json",
        ],
    )?;
    let cancel_gate_root = draft_gate
        .get("root_job_ids")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .ok_or("missing cancel gate root job id")?;
    wait_for_job_status(
        &repo,
        cancel_gate_root,
        "waiting_on_approval",
        Duration::from_secs(5),
    )?;

    let cancel_args = vec![
        "run".to_string(),
        "draft".to_string(),
        "--set".to_string(),
        "slug=cancel-smoke".to_string(),
        "--set".to_string(),
        "branch=draft/cancel-smoke".to_string(),
        "--set".to_string(),
        "spec_text=Cancel-path smoke plan.".to_string(),
        "--after".to_string(),
        cancel_gate_root.to_string(),
        "--format".to_string(),
        "json".to_string(),
    ];
    let cancel_refs = cancel_args.iter().map(String::as_str).collect::<Vec<_>>();
    let draft_blocked = run_json(&repo, &cancel_refs)?;
    let cancel_root = draft_blocked
        .get("root_job_ids")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .ok_or("missing cancel root job id")?;
    wait_for_job_status(
        &repo,
        cancel_root,
        "waiting_on_deps",
        Duration::from_secs(5),
    )?;
    let cancel = repo.vizier_output(&["jobs", "cancel", cancel_root])?;
    assert!(
        cancel.status.success(),
        "jobs cancel should succeed for waiting stage root: {}",
        String::from_utf8_lossy(&cancel.stderr)
    );
    let cancelled = read_job_record(&repo, cancel_root)?;
    assert_eq!(
        cancelled.get("status").and_then(Value::as_str),
        Some("cancelled"),
        "cancelled stage root should be terminal cancelled: {cancelled}"
    );
    let cancel_gate = repo.vizier_output(&["jobs", "cancel", cancel_gate_root])?;
    assert!(
        cancel_gate.status.success(),
        "jobs cancel should succeed for approval-gated stage root: {}",
        String::from_utf8_lossy(&cancel_gate.stderr)
    );

    drop(repo);
    let retry_repo = IntegrationRepo::new_serial()?;
    clean_workdir(&retry_repo)?;
    write_stage_alias_test_config(&retry_repo)?;
    seed_plan_branch(&retry_repo, "retry-smoke", "draft/retry-smoke")?;
    retry_repo.git(&["checkout", "draft/retry-smoke"])?;
    retry_repo.write("retry-smoke.txt", "retry smoke branch change\n")?;
    retry_repo.git(&["add", "retry-smoke.txt"])?;
    retry_repo.git(&["commit", "-m", "feat: retry smoke branch"])?;
    retry_repo.git(&["checkout", "master"])?;

    run_stage_approve_follow(&retry_repo, "retry-smoke", "draft/retry-smoke")?;

    let merge_payload = run_json(
        &retry_repo,
        &[
            "run",
            "merge",
            "--set",
            "slug=retry-smoke",
            "--set",
            "branch=draft/retry-smoke",
            "--set",
            "target_branch=master",
            "--set",
            "cicd_script=exit 7",
            "--format",
            "json",
        ],
    )?;
    let merge_run_id = merge_payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing merge run_id")?;
    let merge_manifest = load_run_manifest(&retry_repo, merge_run_id)?;
    wait_for_manifest_jobs(&retry_repo, &merge_manifest, Duration::from_secs(20))?;
    let cicd_job = manifest_node_job_id(&merge_manifest, "merge_gate_cicd")?;
    let first_cicd = read_job_record(&retry_repo, &cicd_job)?;
    assert_eq!(
        first_cicd.get("status").and_then(Value::as_str),
        Some("failed"),
        "expected merge cicd gate failure before retry: {first_cicd}"
    );

    let retry = retry_repo.vizier_output(&["jobs", "retry", &cicd_job])?;
    assert!(
        retry.status.success(),
        "jobs retry should succeed for failed stage node: {}",
        String::from_utf8_lossy(&retry.stderr)
    );
    wait_for_job_completion(&retry_repo, &cicd_job, Duration::from_secs(20))?;
    let retried = read_job_record(&retry_repo, &cicd_job)?;
    assert!(
        retried
            .pointer("/metadata/workflow_node_attempt")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            >= 2,
        "retry should increment workflow node attempt: {retried}"
    );

    Ok(())
}

#[test]
fn test_run_merge_stage_conflict_gate_blocks_and_preserves_sentinel() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    seed_plan_branch(&repo, "conflict-smoke", "draft/conflict-smoke")?;
    repo.git(&["checkout", "draft/conflict-smoke"])?;
    repo.write("a", "draft conflict content\n")?;
    repo.git(&["add", "a"])?;
    repo.git(&["commit", "-m", "feat: conflict branch change"])?;
    repo.git(&["checkout", "master"])?;
    repo.write("a", "master conflict content\n")?;
    repo.git(&["add", "a"])?;
    repo.git(&["commit", "-m", "feat: master conflict change"])?;

    run_stage_approve_follow(&repo, "conflict-smoke", "draft/conflict-smoke")?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "merge",
            "--set",
            "slug=conflict-smoke",
            "--set",
            "branch=draft/conflict-smoke",
            "--set",
            "target_branch=master",
            "--set",
            "cicd_script=true",
            "--format",
            "json",
        ],
    )?;
    let run_id = payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing run_id")?;
    let manifest = load_run_manifest(&repo, run_id)?;

    let integrate_job = manifest_node_job_id(&manifest, "merge_integrate")?;
    let conflict_job = manifest_node_job_id(&manifest, "merge_conflict_resolution")?;
    wait_for_job_completion(&repo, &integrate_job, Duration::from_secs(20))?;
    wait_for_job_completion(&repo, &conflict_job, Duration::from_secs(20))?;
    let integrate_record = read_job_record(&repo, &integrate_job)?;
    let conflict_record = read_job_record(&repo, &conflict_job)?;
    assert_eq!(
        integrate_record.get("status").and_then(Value::as_str),
        Some("blocked_by_dependency"),
        "merge integrate should block on conflict: {integrate_record}"
    );
    let conflict_status = conflict_record.get("status").and_then(Value::as_str);
    assert!(
        matches!(
            conflict_status,
            Some("blocked_by_dependency") | Some("succeeded")
        ),
        "conflict gate should either block or no-op succeed while integrate remains blocked: {conflict_record}"
    );

    let sentinel = repo
        .path()
        .join(".vizier/tmp/merge-conflicts/conflict-smoke.json");
    assert!(
        sentinel.exists(),
        "merge conflict sentinel should remain for operator recovery: {}",
        sentinel.display()
    );

    Ok(())
}

#[test]
fn test_run_set_rejects_unresolved_non_args_without_partial_enqueue() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/unresolved-needs.json",
        "{\n\
  \"id\": \"template.unresolved.needs\",\n\
  \"version\": \"v1\",\n\
  \"nodes\": [\n\
    {\n\
      \"id\": \"single\",\n\
      \"kind\": \"shell\",\n\
      \"uses\": \"cap.env.shell.command.run\",\n\
      \"args\": {\"script\": \"true\"},\n\
      \"needs\": [\n\
        {\"plan_doc\": {\"slug\": \"${missing}\", \"branch\": \"main\"}}\n\
      ]\n\
    }\n\
  ]\n\
}\n",
    )?;

    let runs_dir = repo.path().join(".vizier/jobs/runs");
    let before_count = if runs_dir.is_dir() {
        fs::read_dir(&runs_dir)?.count()
    } else {
        0
    };

    let output = repo.vizier_output(&["run", "file:.vizier/unresolved-needs.json"])?;
    assert!(
        !output.status.success(),
        "run should fail on unresolved non-args placeholder"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unresolved parameter `missing`")
            && stderr.contains("nodes[single].needs[0].plan_doc.slug"),
        "expected unresolved placeholder error with field path, got: {stderr}"
    );

    let after_count = if runs_dir.is_dir() {
        fs::read_dir(&runs_dir)?.count()
    } else {
        0
    };
    assert_eq!(
        before_count, after_count,
        "queue-time interpolation failure should not materialize run manifests"
    );

    Ok(())
}

#[test]
fn test_run_rejects_legacy_uses_without_partial_enqueue() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflows/legacy.toml",
        "id = \"template.legacy\"\n\
version = \"v1\"\n\
[[nodes]]\n\
id = \"legacy\"\n\
kind = \"builtin\"\n\
uses = \"vizier.merge.integrate\"\n",
    )?;

    let runs_dir = repo.path().join(".vizier/jobs/runs");
    let before_count = if runs_dir.is_dir() {
        fs::read_dir(&runs_dir)?.count()
    } else {
        0
    };

    let output = repo.vizier_output(&["run", "file:.vizier/workflows/legacy.toml"])?;
    assert!(
        !output.status.success(),
        "legacy uses should fail queue-time"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("uses unknown label") || stderr.contains("validation"),
        "expected validation failure details, got: {stderr}"
    );

    let after_count = if runs_dir.is_dir() {
        fs::read_dir(&runs_dir)?.count()
    } else {
        0
    };
    assert_eq!(
        before_count, after_count,
        "queue-time failure should not materialize run manifests"
    );

    Ok(())
}

#[test]
fn test_run_after_and_approval_overrides_affect_root_jobs() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    write_single_run_template(&repo, ".vizier/workflows/single.toml", "true")?;

    write_job_record_simple(
        &repo,
        "dep-running",
        "running",
        "2026-02-14T00:00:00Z",
        None,
        &["dep"],
    )?;

    let with_after = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/single.toml",
            "--after",
            "dep-running",
            "--format",
            "json",
        ],
    )?;
    let root_after = with_after
        .get("root_job_ids")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .ok_or("missing root job id for --after run")?;
    let after_record = read_job_record(&repo, root_after)?;
    assert_eq!(
        after_record.get("status").and_then(Value::as_str),
        Some("waiting_on_deps")
    );
    let after_entries = after_record
        .pointer("/schedule/after")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        after_entries
            .iter()
            .any(|entry| entry.get("job_id").and_then(Value::as_str) == Some("dep-running")),
        "root schedule should include dep-running in after list: {after_record}"
    );

    let with_approval = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/single.toml",
            "--require-approval",
            "--format",
            "json",
        ],
    )?;
    let root_approval = with_approval
        .get("root_job_ids")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .ok_or("missing root job id for approval run")?;
    let approval_record = read_job_record(&repo, root_approval)?;
    assert_eq!(
        approval_record.get("status").and_then(Value::as_str),
        Some("waiting_on_approval")
    );
    assert_eq!(
        approval_record
            .pointer("/schedule/approval/state")
            .and_then(Value::as_str),
        Some("pending")
    );

    let approve = repo.vizier_output(&["jobs", "approve", root_approval, "--format", "json"])?;
    assert!(
        approve.status.success(),
        "jobs approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );
    wait_for_job_completion(&repo, root_approval, Duration::from_secs(5))?;
    let approved_record = read_job_record(&repo, root_approval)?;
    assert_eq!(
        approved_record.get("status").and_then(Value::as_str),
        Some("succeeded")
    );

    Ok(())
}

#[test]
fn test_run_after_run_reference_expands_to_terminal_sink_job_ids() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    write_single_run_template(&repo, ".vizier/workflows/single.toml", "true")?;

    let first = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/single.toml",
            "--format",
            "json",
        ],
    )?;
    let first_run_id = first
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing first run id")?;
    let first_root = first_root_job_id(&first)?;
    wait_for_job_completion(&repo, &first_root, Duration::from_secs(10))?;

    let second = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/single.toml",
            "--after",
            &format!("run:{first_run_id}"),
            "--format",
            "json",
        ],
    )?;
    let second_root = first_root_job_id(&second)?;
    let second_record = read_job_record(&repo, &second_root)?;
    let after_entries = second_record
        .pointer("/schedule/after")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        after_entries
            .iter()
            .any(|entry| entry.get("job_id").and_then(Value::as_str) == Some(first_root.as_str())),
        "expected expanded run sink dependency in root schedule: {second_record}"
    );
    assert!(
        after_entries.iter().all(|entry| {
            entry
                .get("job_id")
                .and_then(Value::as_str)
                .map(|job_id| !job_id.starts_with("run:"))
                .unwrap_or(true)
        }),
        "schedule.after should persist only concrete job ids: {second_record}"
    );

    Ok(())
}

#[test]
fn test_run_after_supports_mixed_run_and_job_dependencies() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    write_single_run_template(&repo, ".vizier/workflows/single.toml", "true")?;

    let seed = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/single.toml",
            "--format",
            "json",
        ],
    )?;
    let seed_run_id = seed
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing seed run id")?;
    let seed_root = first_root_job_id(&seed)?;
    wait_for_job_completion(&repo, &seed_root, Duration::from_secs(10))?;

    write_job_record_simple(
        &repo,
        "dep-running",
        "running",
        "2026-02-17T00:00:00Z",
        None,
        &["dep"],
    )?;

    let mixed = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/single.toml",
            "--after",
            &format!("run:{seed_run_id}"),
            "--after",
            "dep-running",
            "--format",
            "json",
        ],
    )?;
    let mixed_root = first_root_job_id(&mixed)?;
    let mixed_record = read_job_record(&repo, &mixed_root)?;
    let after_job_ids = mixed_record
        .pointer("/schedule/after")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| {
            entry
                .get("job_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        after_job_ids,
        vec![seed_root.clone(), "dep-running".to_string()],
        "expected mixed run/job dependencies in first-seen order: {mixed_record}"
    );

    Ok(())
}

#[test]
fn test_run_after_missing_run_manifest_fails() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    write_single_run_template(&repo, ".vizier/workflows/single.toml", "true")?;

    let output = repo.vizier_output(&[
        "run",
        "file:.vizier/workflows/single.toml",
        "--after",
        "run:run_missing",
        "--format",
        "json",
    ])?;
    assert!(!output.status.success(), "missing run manifest should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("run:run_missing") && stderr.contains("manifest not found"),
        "expected missing manifest error with run id, got: {stderr}"
    );

    Ok(())
}

#[test]
fn test_run_after_manifest_without_success_sinks_fails() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    write_single_run_template(&repo, ".vizier/workflows/single.toml", "true")?;

    let seed = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/single.toml",
            "--format",
            "json",
        ],
    )?;
    let seed_run_id = seed
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing seed run id")?;
    let manifest_path = repo
        .path()
        .join(format!(".vizier/jobs/runs/{seed_run_id}.json"));
    let mut manifest = serde_json::from_str::<Value>(&fs::read_to_string(&manifest_path)?)?;
    let nodes = manifest
        .pointer_mut("/nodes")
        .and_then(Value::as_object_mut)
        .ok_or("missing nodes map in run manifest")?;
    for node in nodes.values_mut() {
        let node_obj = node
            .as_object_mut()
            .ok_or("manifest node is not an object")?;
        let routes = node_obj
            .entry("routes".to_string())
            .or_insert_with(|| serde_json::json!({}));
        let routes_obj = routes
            .as_object_mut()
            .ok_or("manifest routes is not an object")?;
        routes_obj.insert(
            "succeeded".to_string(),
            serde_json::json!([{"node_id": "synthetic", "mode": "propagate_context"}]),
        );
    }
    fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;

    let output = repo.vizier_output(&[
        "run",
        "file:.vizier/workflows/single.toml",
        "--after",
        &format!("run:{seed_run_id}"),
        "--format",
        "json",
    ])?;
    assert!(
        !output.status.success(),
        "manifest with no sink nodes should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no success-terminal sink nodes") && stderr.contains(seed_run_id),
        "expected sink error with run id, got: {stderr}"
    );

    Ok(())
}

#[test]
fn test_run_after_bare_run_id_requires_run_prefix() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    write_single_run_template(&repo, ".vizier/workflows/single.toml", "true")?;

    let output = repo.vizier_output(&[
        "run",
        "file:.vizier/workflows/single.toml",
        "--after",
        "run_deadbeef",
        "--format",
        "json",
    ])?;
    assert!(!output.status.success(), "bare run id should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("use `run:run_deadbeef`"),
        "expected run-prefix guidance, got: {stderr}"
    );

    Ok(())
}

#[test]
fn test_run_repeat_one_preserves_single_run_json_shape() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    write_single_run_template(&repo, ".vizier/workflows/single.toml", "true")?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/single.toml",
            "--repeat",
            "1",
            "--format",
            "json",
        ],
    )?;
    assert_eq!(
        payload.get("outcome").and_then(Value::as_str),
        Some("workflow_run_enqueued")
    );
    assert!(
        payload.get("run_id").and_then(Value::as_str).is_some(),
        "repeat=1 should keep single-run payload shape: {payload}"
    );
    assert!(
        payload.get("runs").is_none(),
        "repeat=1 should not emit aggregate runs payload: {payload}"
    );

    Ok(())
}

#[test]
fn test_run_repeat_enqueues_chained_runs_and_manifests() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    write_single_run_template(&repo, ".vizier/workflows/single.toml", "true")?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/single.toml",
            "--repeat",
            "2",
            "--format",
            "json",
        ],
    )?;
    assert_eq!(
        payload.get("outcome").and_then(Value::as_str),
        Some("workflow_runs_enqueued")
    );
    assert_eq!(payload.get("repeat").and_then(Value::as_u64), Some(2));
    assert_eq!(repeated_runs(&payload)?.len(), 2);

    let first_run_id = repeated_run_id(&payload, 0)?;
    let second_run_id = repeated_run_id(&payload, 1)?;
    assert_ne!(first_run_id, second_run_id, "repeat runs must be distinct");

    let _ = load_run_manifest(&repo, &first_run_id)?;
    let _ = load_run_manifest(&repo, &second_run_id)?;

    let first_root = repeated_root_job_id(&payload, 0)?;
    let second_root = repeated_root_job_id(&payload, 1)?;
    let second_record = read_job_record(&repo, &second_root)?;
    let second_after = schedule_after_job_ids(&second_record);
    assert!(
        second_after.iter().any(|job_id| job_id == &first_root),
        "repeat iteration 2 root must depend on iteration 1 sink root: {second_record}"
    );

    wait_for_job_completion(&repo, &first_root, Duration::from_secs(10))?;
    wait_for_job_completion(&repo, &second_root, Duration::from_secs(10))?;

    Ok(())
}

#[test]
fn test_run_repeat_composes_after_dependencies_and_approval_overrides() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    write_single_run_template(&repo, ".vizier/workflows/single.toml", "true")?;
    write_job_record_simple(
        &repo,
        "dep-running",
        "running",
        "2026-02-18T00:00:00Z",
        None,
        &["dep"],
    )?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/single.toml",
            "--repeat",
            "2",
            "--after",
            "dep-running",
            "--require-approval",
            "--format",
            "json",
        ],
    )?;
    assert_eq!(repeated_runs(&payload)?.len(), 2);

    let first_root = repeated_root_job_id(&payload, 0)?;
    let second_root = repeated_root_job_id(&payload, 1)?;

    let first_record = read_job_record(&repo, &first_root)?;
    let first_after = schedule_after_job_ids(&first_record);
    assert_eq!(
        first_record
            .pointer("/schedule/approval/state")
            .and_then(Value::as_str),
        Some("pending"),
        "repeat iteration 1 root should require approval: {first_record}"
    );
    assert!(
        first_after.iter().any(|job_id| job_id == "dep-running"),
        "repeat iteration 1 root should include user --after dependency: {first_record}"
    );

    let second_record = read_job_record(&repo, &second_root)?;
    let second_after = schedule_after_job_ids(&second_record);
    assert_eq!(
        second_record
            .pointer("/schedule/approval/state")
            .and_then(Value::as_str),
        Some("pending"),
        "repeat iteration 2 root should require approval: {second_record}"
    );
    assert!(
        second_after.iter().any(|job_id| job_id == "dep-running"),
        "repeat iteration 2 root should include user --after dependency: {second_record}"
    );
    assert!(
        second_after.iter().any(|job_id| job_id == &first_root),
        "repeat iteration 2 root should include previous run sink dependency: {second_record}"
    );

    Ok(())
}

#[test]
fn test_run_follow_repeat_success_reports_aggregate_terminal_summary() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    write_single_run_template(
        &repo,
        ".vizier/workflows/follow-repeat-success.toml",
        "true",
    )?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/follow-repeat-success.toml",
            "--repeat",
            "2",
            "--follow",
            "--format",
            "json",
        ],
    )?;
    assert_eq!(
        payload.get("outcome").and_then(Value::as_str),
        Some("workflow_runs_terminal")
    );
    assert_eq!(payload.get("repeat").and_then(Value::as_u64), Some(2));
    assert_eq!(
        payload.get("terminal_state").and_then(Value::as_str),
        Some("succeeded")
    );
    assert_eq!(payload.get("exit_code").and_then(Value::as_i64), Some(0));

    let runs = repeated_runs(&payload)?;
    assert_eq!(runs.len(), 2);
    for (index, entry) in runs.iter().enumerate() {
        assert_eq!(
            entry.get("index").and_then(Value::as_u64),
            Some((index + 1) as u64)
        );
        assert_eq!(
            entry.get("terminal_state").and_then(Value::as_str),
            Some("succeeded")
        );
        assert_eq!(entry.get("exit_code").and_then(Value::as_i64), Some(0));
    }

    Ok(())
}

#[test]
fn test_run_follow_repeat_short_circuits_on_first_blocked_run() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflows/follow-repeat-blocked.toml",
        "id = \"template.follow.repeat.blocked\"\n\
version = \"v1\"\n\
[[nodes]]\n\
id = \"approval_gate\"\n\
kind = \"gate\"\n\
uses = \"control.gate.approval\"\n",
    )?;

    let output = repo.vizier_output(&[
        "run",
        "file:.vizier/workflows/follow-repeat-blocked.toml",
        "--repeat",
        "2",
        "--follow",
        "--format",
        "json",
    ])?;
    assert_eq!(
        output.status.code(),
        Some(10),
        "blocked repeat follow should exit 10: stderr={} stdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );

    let payload = serde_json::from_slice::<Value>(&output.stdout)?;
    assert_eq!(
        payload.get("terminal_state").and_then(Value::as_str),
        Some("blocked")
    );
    assert_eq!(payload.get("exit_code").and_then(Value::as_i64), Some(10));
    assert_eq!(
        repeated_runs(&payload)?.len(),
        1,
        "follow should stop after the first blocked repeat iteration"
    );

    let runs_dir = repo.path().join(".vizier/jobs/runs");
    let manifest_count = fs::read_dir(runs_dir)?.count();
    assert_eq!(
        manifest_count, 2,
        "repeat enqueue should persist both run manifests before follow short-circuit"
    );

    Ok(())
}

#[test]
fn test_run_follow_repeat_short_circuits_on_first_failed_run() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    write_single_run_template(
        &repo,
        ".vizier/workflows/follow-repeat-fail.toml",
        "echo failing >&2; exit 7",
    )?;

    let output = repo.vizier_output(&[
        "run",
        "file:.vizier/workflows/follow-repeat-fail.toml",
        "--repeat",
        "2",
        "--follow",
        "--format",
        "json",
    ])?;
    assert_eq!(
        output.status.code(),
        Some(1),
        "failed repeat follow should map to exit 1: stderr={} stdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );

    let payload = serde_json::from_slice::<Value>(&output.stdout)?;
    assert_eq!(
        payload.get("terminal_state").and_then(Value::as_str),
        Some("failed")
    );
    assert_eq!(payload.get("exit_code").and_then(Value::as_i64), Some(1));
    assert_eq!(
        repeated_runs(&payload)?.len(),
        1,
        "follow should stop after the first failed repeat iteration"
    );

    let runs_dir = repo.path().join(".vizier/jobs/runs");
    let manifest_count = fs::read_dir(runs_dir)?.count();
    assert_eq!(
        manifest_count, 2,
        "repeat enqueue should persist both run manifests before follow short-circuit"
    );

    Ok(())
}

#[test]
fn test_run_follow_exit_codes_cover_success_blocked_and_failed() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    write_single_run_template(&repo, ".vizier/workflows/follow-success.toml", "true")?;
    write_single_run_template(&repo, ".vizier/workflows/follow-fail.toml", "exit 2")?;
    repo.write(
        ".vizier/workflows/follow-blocked.toml",
        "id = \"template.follow.blocked\"\n\
version = \"v1\"\n\
[[nodes]]\n\
id = \"approval_gate\"\n\
kind = \"gate\"\n\
uses = \"control.gate.approval\"\n",
    )?;

    let success = repo.vizier_output(&[
        "run",
        "file:.vizier/workflows/follow-success.toml",
        "--follow",
    ])?;
    assert!(
        success.status.success(),
        "follow success should exit 0: stderr={} stdout={}",
        String::from_utf8_lossy(&success.stderr),
        String::from_utf8_lossy(&success.stdout)
    );

    let blocked = repo.vizier_output(&[
        "run",
        "file:.vizier/workflows/follow-blocked.toml",
        "--follow",
    ])?;
    assert_eq!(
        blocked.status.code(),
        Some(10),
        "blocked follow run should exit 10: stderr={} stdout={}",
        String::from_utf8_lossy(&blocked.stderr),
        String::from_utf8_lossy(&blocked.stdout)
    );

    let failed =
        repo.vizier_output(&["run", "file:.vizier/workflows/follow-fail.toml", "--follow"])?;
    assert!(
        !failed.status.success(),
        "failed follow run should be non-zero"
    );
    assert_ne!(
        failed.status.code(),
        Some(10),
        "failure should not map to blocked code"
    );

    Ok(())
}

#[test]
fn test_run_execution_root_propagates_to_successor_nodes() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflows/execution-root.toml",
        "id = \"template.execution.root\"\n\
version = \"v1\"\n\
[[nodes]]\n\
id = \"prepare\"\n\
kind = \"builtin\"\n\
uses = \"cap.env.builtin.worktree.prepare\"\n\
[nodes.args]\n\
branch = \"draft/execution-root-run\"\n\
[nodes.on]\n\
succeeded = [\"in_worktree\"]\n\
[[nodes]]\n\
id = \"in_worktree\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
script = \"pwd\"\n",
    )?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflows/execution-root.toml",
            "--format",
            "json",
        ],
    )?;
    let run_id = payload
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or("missing run_id")?;

    let manifest_path = repo.path().join(format!(".vizier/jobs/runs/{run_id}.json"));
    let manifest: Value = serde_json::from_str(&fs::read_to_string(&manifest_path)?)?;
    let nodes = manifest
        .get("nodes")
        .and_then(Value::as_object)
        .ok_or("missing workflow nodes in run manifest")?;
    let node_job = |node_id: &str| -> TestResult<String> {
        Ok(nodes
            .get(node_id)
            .and_then(|node| node.get("job_id"))
            .and_then(Value::as_str)
            .ok_or_else(|| format!("missing job id for node {node_id}"))?
            .to_string())
    };
    let prepare_job = node_job("prepare")?;
    let in_worktree_job = node_job("in_worktree")?;
    for job_id in [&prepare_job, &in_worktree_job] {
        wait_for_job_completion(&repo, job_id, Duration::from_secs(15))?;
    }

    let prepare = read_job_record(&repo, &prepare_job)?;
    let in_worktree = read_job_record(&repo, &in_worktree_job)?;
    for (name, record) in [("prepare", &prepare), ("in_worktree", &in_worktree)] {
        assert_eq!(
            record.get("status").and_then(Value::as_str),
            Some("succeeded"),
            "{name} node should succeed: {record}"
        );
    }

    let prepare_execution_root = prepare
        .pointer("/metadata/execution_root")
        .and_then(Value::as_str)
        .ok_or("prepare missing execution_root metadata")?;
    assert_eq!(
        in_worktree
            .pointer("/metadata/execution_root")
            .and_then(Value::as_str),
        Some(prepare_execution_root),
        "success-edge propagation should carry worktree execution_root to in_worktree node"
    );
    assert!(
        prepare_execution_root.starts_with(".vizier/tmp-worktrees/"),
        "prepare should set execution_root to a worktree path: {prepare_execution_root}"
    );

    let worktree_stdout = fs::read_to_string(
        repo.path()
            .join(".vizier/jobs")
            .join(&in_worktree_job)
            .join("stdout.log"),
    )?;
    let observed_worktree_pwd = worktree_stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .ok_or("missing in_worktree pwd output")?;
    let expected_worktree_pwd = repo.path().join(prepare_execution_root).canonicalize()?;
    assert_eq!(
        observed_worktree_pwd,
        expected_worktree_pwd.display().to_string(),
        "in_worktree node should execute from propagated worktree root"
    );

    Ok(())
}
