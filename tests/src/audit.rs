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

fn effective_lock_keys(payload: &Value, node_id: &str) -> TestResult<Vec<String>> {
    let Some(entries) = payload.get("effective_locks").and_then(Value::as_array) else {
        return Err("missing effective_locks array".into());
    };
    let node = entries
        .iter()
        .find(|entry| entry.get("node_id").and_then(Value::as_str) == Some(node_id))
        .ok_or_else(|| format!("missing effective_locks entry for node `{node_id}`"))?;
    let mut keys = node
        .get("locks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|lock| lock.get("key").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    keys.sort();
    keys.dedup();
    Ok(keys)
}

#[test]
fn test_audit_json_contract_and_no_side_effects() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflows/audit-single.toml",
        "id = \"template.audit.single\"\n\
version = \"v1\"\n\
[[artifact_contracts]]\n\
id = \"plan_doc\"\n\
version = \"v1\"\n\
[[nodes]]\n\
id = \"single\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
script = \"true\"\n\
[nodes.produces]\n\
succeeded = [{ plan_doc = { slug = \"alpha\", branch = \"draft/alpha\" } }]\n",
    )?;

    let before_run_manifests = count_run_manifests(&repo)?;
    let before_jobs = count_job_records(&repo)?;

    let payload = run_json(
        &repo,
        &[
            "audit",
            "file:.vizier/workflows/audit-single.toml",
            "--format",
            "json",
        ],
    )?;

    assert_eq!(
        payload.get("outcome").and_then(Value::as_str),
        Some("workflow_audit_completed")
    );
    assert_eq!(
        payload
            .get("workflow_template_selector")
            .and_then(Value::as_str),
        Some("file:.vizier/workflows/audit-single.toml")
    );
    assert_eq!(
        payload.get("workflow_template_id").and_then(Value::as_str),
        Some("template.audit.single")
    );
    assert_eq!(
        payload
            .get("workflow_template_version")
            .and_then(Value::as_str),
        Some("v1")
    );
    assert_eq!(payload.get("node_count").and_then(Value::as_u64), Some(1));
    assert_eq!(
        payload
            .pointer("/output_artifacts")
            .and_then(Value::as_array)
            .map(|values| values.len()),
        Some(2)
    );
    assert_eq!(
        payload
            .pointer("/output_artifacts_by_outcome/succeeded")
            .and_then(Value::as_array)
            .map(|values| values.len()),
        Some(2)
    );
    assert_eq!(
        payload
            .pointer("/summary/untethered_count")
            .and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        payload
            .pointer("/summary/has_untethered")
            .and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        payload
            .pointer("/effective_locks/0/node_id")
            .and_then(Value::as_str),
        Some("single")
    );
    assert_eq!(
        payload
            .pointer("/effective_locks/0/locks/0/key")
            .and_then(Value::as_str),
        Some("branch:draft/alpha")
    );
    assert_eq!(
        payload
            .pointer("/effective_locks/0/locks/0/mode")
            .and_then(Value::as_str),
        Some("exclusive")
    );

    assert_eq!(
        count_run_manifests(&repo)?,
        before_run_manifests,
        "audit must not write run manifests"
    );
    assert_eq!(
        count_job_records(&repo)?,
        before_jobs,
        "audit must not enqueue jobs"
    );

    Ok(())
}

#[test]
fn test_audit_reports_untethered_inputs_and_strict_exit_code() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflows/audit-untethered.toml",
        "id = \"template.audit.untethered\"\n\
version = \"v1\"\n\
[[artifact_contracts]]\n\
id = \"prompt_text\"\n\
version = \"v1\"\n\
[[nodes]]\n\
id = \"consumer\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
script = \"true\"\n\
[[nodes.needs]]\n\
custom = { type_id = \"prompt_text\", key = \"review_main\" }\n",
    )?;

    let before_run_manifests = count_run_manifests(&repo)?;
    let before_jobs = count_job_records(&repo)?;

    let payload = run_json(
        &repo,
        &[
            "audit",
            "file:.vizier/workflows/audit-untethered.toml",
            "--format",
            "json",
        ],
    )?;
    assert_eq!(
        payload
            .pointer("/summary/has_untethered")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        payload
            .pointer("/summary/untethered_count")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        payload
            .pointer("/untethered_inputs/0/artifact")
            .and_then(Value::as_str),
        Some("custom:prompt_text:review_main")
    );
    assert_eq!(
        payload
            .pointer("/untethered_inputs/0/consumers/0")
            .and_then(Value::as_str),
        Some("consumer")
    );

    let strict = repo.vizier_output(&[
        "audit",
        "file:.vizier/workflows/audit-untethered.toml",
        "--strict",
        "--format",
        "json",
    ])?;
    assert_eq!(
        strict.status.code(),
        Some(10),
        "strict audit should return exit code 10 when untethered inputs are present"
    );

    assert_eq!(
        count_run_manifests(&repo)?,
        before_run_manifests,
        "audit must not write run manifests"
    );
    assert_eq!(
        count_job_records(&repo)?,
        before_jobs,
        "audit must not enqueue jobs"
    );

    Ok(())
}

#[test]
fn test_audit_text_output_contract() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflows/audit-text.toml",
        "id = \"template.audit.text\"\n\
version = \"v1\"\n\
[[nodes]]\n\
id = \"single\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
script = \"true\"\n",
    )?;

    let output = repo.vizier_output(&["audit", "file:.vizier/workflows/audit-text.toml"])?;
    assert!(
        output.status.success(),
        "audit text mode should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Outcome") && stdout.contains("Workflow audit completed"),
        "missing audit outcome block: {stdout}"
    );
    assert!(
        stdout.contains("Output artifacts:") && stdout.contains("custom:operation_output:single"),
        "missing output artifacts section: {stdout}"
    );
    assert!(
        stdout.contains("Untethered inputs:") && stdout.contains("- none"),
        "missing untethered section: {stdout}"
    );
    assert!(
        stdout.contains("Effective locks:") && stdout.contains("single: repo_serial (exclusive)"),
        "missing effective locks section: {stdout}"
    );

    Ok(())
}

#[test]
fn test_audit_effective_locks_show_inference_and_explicit_override() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflows/audit-locks.toml",
        "id = \"template.audit.locks\"\n\
version = \"v1\"\n\
[params]\n\
target_branch = \"main\"\n\
[[nodes]]\n\
id = \"inferred\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
branch = \"draft/alpha\"\n\
script = \"true\"\n\
[[nodes]]\n\
id = \"explicit\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
branch = \"draft/alpha\"\n\
script = \"true\"\n\
[[nodes.locks]]\n\
key = \"custom:override\"\n\
mode = \"exclusive\"\n",
    )?;

    let payload = run_json(
        &repo,
        &[
            "audit",
            "file:.vizier/workflows/audit-locks.toml",
            "--format",
            "json",
        ],
    )?;

    assert_eq!(
        payload
            .pointer("/effective_locks/0/node_id")
            .and_then(Value::as_str),
        Some("explicit")
    );
    assert_eq!(
        payload
            .pointer("/effective_locks/0/locks/0/key")
            .and_then(Value::as_str),
        Some("custom:override")
    );
    assert_eq!(
        payload
            .pointer("/effective_locks/1/node_id")
            .and_then(Value::as_str),
        Some("inferred")
    );
    assert_eq!(
        payload
            .pointer("/effective_locks/1/locks/0/key")
            .and_then(Value::as_str),
        Some("branch:draft/alpha")
    );
    assert_eq!(
        payload
            .pointer("/effective_locks/1/locks/1/key")
            .and_then(Value::as_str),
        Some("branch:main")
    );

    Ok(())
}

#[test]
fn test_audit_develop_composed_effective_locks_are_stage_scoped() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    let payload = run_json(
        &repo,
        &[
            "audit",
            "file:.vizier/develop.hcl",
            "--set",
            "slug=develop-audit-lock-scope",
            "--set",
            "branch=draft/develop-audit-lock-scope",
            "--set",
            "target_branch=master",
            "--set",
            "spec_text=develop-audit-lock-scope",
            "--format",
            "json",
        ],
    )?;

    for node_id in ["develop_draft__stop_gate", "develop_approve__stop_gate"] {
        assert_eq!(
            effective_lock_keys(&payload, node_id)?,
            vec!["branch:draft/develop-audit-lock-scope".to_string()],
            "expected stage-scoped source branch lock for `{node_id}`"
        );
    }

    for node_id in [
        "develop_merge__merge_conflict_resolution",
        "develop_merge__merge_integrate",
        "develop_merge__merge_gate_cicd",
    ] {
        assert_eq!(
            effective_lock_keys(&payload, node_id)?,
            vec![
                "branch:draft/develop-audit-lock-scope".to_string(),
                "branch:master".to_string(),
            ],
            "expected merge-scoped source+target branch locks for `{node_id}`"
        );
    }

    Ok(())
}

#[test]
fn test_audit_flow_resolution_parity_across_alias_file_and_selector() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflows/audit-resolve.toml",
        "id = \"template.audit.resolve\"\n\
version = \"v1\"\n\
[[nodes]]\n\
id = \"single\"\n\
kind = \"shell\"\n\
uses = \"cap.env.shell.command.run\"\n\
[nodes.args]\n\
script = \"true\"\n",
    )?;
    repo.write(
        ".vizier/config.toml",
        "[commands]\nresolve = \"file:.vizier/workflows/audit-resolve.toml\"\n",
    )?;

    let by_alias = run_json(&repo, &["audit", "resolve", "--format", "json"])?;
    let by_file = run_json(
        &repo,
        &[
            "audit",
            "file:.vizier/workflows/audit-resolve.toml",
            "--format",
            "json",
        ],
    )?;
    let by_selector = run_json(
        &repo,
        &["audit", "template.audit.resolve@v1", "--format", "json"],
    )?;

    for payload in [&by_alias, &by_file, &by_selector] {
        assert_eq!(
            payload.get("workflow_template_id").and_then(Value::as_str),
            Some("template.audit.resolve")
        );
        assert_eq!(
            payload
                .get("workflow_template_version")
                .and_then(Value::as_str),
            Some("v1")
        );
        assert_eq!(payload.get("node_count").and_then(Value::as_u64), Some(1));
    }

    assert_eq!(
        by_alias.get("output_artifacts"),
        by_file.get("output_artifacts"),
        "alias and file resolution should produce identical audit artifacts"
    );
    assert_eq!(
        by_alias.get("effective_locks"),
        by_file.get("effective_locks"),
        "alias and file resolution should produce identical lock inference"
    );
    assert_eq!(
        by_file.get("output_artifacts"),
        by_selector.get("output_artifacts"),
        "file and selector resolution should produce identical audit artifacts"
    );
    assert_eq!(
        by_file.get("effective_locks"),
        by_selector.get("effective_locks"),
        "file and selector resolution should produce identical lock inference"
    );

    Ok(())
}

#[test]
fn test_audit_missing_required_input_uses_run_style_guidance() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflows/audit-required.toml",
        "id = \"template.audit.required\"\n\
version = \"v1\"\n\
[[nodes]]\n\
id = \"prepare\"\n\
kind = \"builtin\"\n\
uses = \"cap.env.builtin.worktree.prepare\"\n",
    )?;

    let before_run_manifests = count_run_manifests(&repo)?;
    let before_jobs = count_job_records(&repo)?;

    let output = repo.vizier_output(&["audit", "file:.vizier/workflows/audit-required.toml"])?;
    assert!(
        !output.status.success(),
        "missing required input should fail in audit mode"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error: missing required input"),
        "expected run-style input error prefix: {stderr}"
    );
    assert!(
        stderr.contains("usage:") && stderr.contains("example:") && stderr.contains("hint:"),
        "expected usage/example/hint guidance in audit mode: {stderr}"
    );
    assert!(
        stderr.contains("vizier run file:.vizier/workflows/audit-required.toml --help"),
        "expected hint to flow-scoped run help: {stderr}"
    );

    assert_eq!(
        count_run_manifests(&repo)?,
        before_run_manifests,
        "failed audit preflight must not write run manifests"
    );
    assert_eq!(
        count_job_records(&repo)?,
        before_jobs,
        "failed audit preflight must not enqueue jobs"
    );

    Ok(())
}
