use crate::fixtures::*;

fn run_json(repo: &IntegrationRepo, args: &[&str]) -> TestResult<Value> {
    let output = repo.vizier_output(args)?;
    assert!(
        output.status.success(),
        "command {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice::<Value>(&output.stdout)?)
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

#[test]
fn test_run_alias_composes_and_applies_set_overrides() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/develop.toml",
        "id = \"template.develop\"\n\
version = \"v1\"\n\
[[imports]]\n\
name = \"stage_one\"\n\
path = \"workflow/stage_one.toml\"\n\
[[imports]]\n\
name = \"stage_two\"\n\
path = \"workflow/stage_two.toml\"\n\
[[links]]\n\
from = \"stage_one\"\n\
to = \"stage_two\"\n",
    )?;
    repo.write(
        ".vizier/workflow/stage_one.toml",
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
        ".vizier/workflow/stage_two.toml",
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
        "file:.vizier/develop.toml"
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
            .pointer("/metadata/scope")
            .and_then(Value::as_str),
        Some("develop")
    );
    assert_eq!(
        root_record
            .pointer("/metadata/workflow_template_selector")
            .and_then(Value::as_str),
        Some("file:.vizier/develop.toml")
    );

    Ok(())
}

#[test]
fn test_run_set_expands_non_args_runtime_fields() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflow/set-surface.json",
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
            "file:.vizier/workflow/set-surface.json",
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
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    write_single_run_template(&repo, ".vizier/workflow/single.toml", "true")?;

    let payload = run_json(
        &repo,
        &[
            "run",
            "file:.vizier/workflow/single.toml",
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
fn test_run_set_rejects_unresolved_non_args_without_partial_enqueue() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflow/unresolved-needs.json",
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

    let output = repo.vizier_output(&["run", "file:.vizier/workflow/unresolved-needs.json"])?;
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
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflow/legacy.toml",
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

    let output = repo.vizier_output(&["run", "file:.vizier/workflow/legacy.toml"])?;
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
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    write_single_run_template(&repo, ".vizier/workflow/single.toml", "true")?;

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
            "file:.vizier/workflow/single.toml",
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
            "file:.vizier/workflow/single.toml",
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
fn test_run_follow_exit_codes_cover_success_blocked_and_failed() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    write_single_run_template(&repo, ".vizier/workflow/follow-success.toml", "true")?;
    write_single_run_template(&repo, ".vizier/workflow/follow-fail.toml", "exit 2")?;
    repo.write(
        ".vizier/workflow/follow-blocked.toml",
        "id = \"template.follow.blocked\"\n\
version = \"v1\"\n\
[[nodes]]\n\
id = \"approval_gate\"\n\
kind = \"gate\"\n\
uses = \"control.gate.approval\"\n",
    )?;

    let success = repo.vizier_output(&[
        "run",
        "file:.vizier/workflow/follow-success.toml",
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
        "file:.vizier/workflow/follow-blocked.toml",
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
        repo.vizier_output(&["run", "file:.vizier/workflow/follow-fail.toml", "--follow"])?;
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
