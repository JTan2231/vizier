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

fn write_stage_alias_test_config(repo: &IntegrationRepo) -> TestResult {
    repo.write(
        ".vizier/config.toml",
        r#"[commands]
draft = "file:.vizier/workflow/draft.toml"
approve = "file:.vizier/workflow/approve.toml"
merge = "file:.vizier/workflow/merge.toml"
develop = "file:.vizier/develop.toml"

[agents.default]
selector = "mock"

[agents.default.agent]
command = ["sh", "-lc", "cat >/dev/null; printf '%s\n' 'mock agent response'"]
"#,
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

fn load_run_manifest(repo: &IntegrationRepo, run_id: &str) -> TestResult<Value> {
    let manifest_path = repo.path().join(format!(".vizier/jobs/runs/{run_id}.json"));
    Ok(serde_json::from_str(&fs::read_to_string(manifest_path)?)?)
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
        statuses.push(format!("{job_id}:{status}"));
    }
    Err(format!(
        "timed out waiting for manifest jobs to reach terminal state: {}",
        statuses.join(", ")
    )
    .into())
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
fn test_run_dynamic_named_flags_expand_to_set_overrides() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflow/named-flags.toml",
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
            "file:.vizier/workflow/named-flags.toml",
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
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflow/positional.toml",
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
            "file:.vizier/workflow/positional.toml",
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
fn test_seeded_stage_templates_use_canonical_labels() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let checks: [(&str, &[&str]); 3] = [
        (
            ".vizier/workflow/draft.toml",
            &[
                "version = \"v2\"",
                "positional = [\"spec_file\", \"slug\", \"branch\"]",
                "cap.env.builtin.worktree.prepare",
                "slug = \"${slug}\"",
                "cap.env.builtin.prompt.resolve",
                "cap.agent.invoke",
                "cap.env.builtin.plan.persist",
                "cap.env.builtin.git.stage_commit",
            ],
        ),
        (
            ".vizier/workflow/approve.toml",
            &[
                "version = \"v2\"",
                "positional = [\"slug\", \"branch\"]",
                "cap.env.builtin.worktree.prepare",
                "slug = \"${slug}\"",
                "cap.env.builtin.prompt.resolve",
                "cap.agent.invoke",
                "cap.env.builtin.git.stage_commit",
                "control.gate.stop_condition",
            ],
        ),
        (
            ".vizier/workflow/merge.toml",
            &[
                "version = \"v2\"",
                "positional = [\"slug\", \"branch\", \"target_branch\"]",
                "cap.env.builtin.git.integrate_plan_branch",
                "control.gate.conflict_resolution",
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
fn test_run_stage_aliases_execute_templates_smoke() -> TestResult {
    let repo = IntegrationRepo::new()?;
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
            "--set",
            "prompt_text=Draft a smoke implementation plan.",
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
    repo.git(&[
        "show-ref",
        "--verify",
        "--quiet",
        "refs/heads/draft/stage-draft-smoke",
    ])?;
    let draft_plan = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args([
            "show",
            "draft/stage-draft-smoke:.vizier/implementation-plans/stage-draft-smoke.md",
        ])
        .output()?;
    assert!(
        draft_plan.status.success(),
        "expected draft plan doc on draft branch: {}",
        String::from_utf8_lossy(&draft_plan.stderr)
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
            "--set",
            "prompt_text=Apply the stage smoke plan.",
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

    repo.git(&["checkout", "-b", "draft/merge-smoke"])?;
    repo.write("merge-smoke.txt", "merge smoke branch change\n")?;
    repo.git(&["add", "merge-smoke.txt"])?;
    repo.git(&["commit", "-m", "feat: merge smoke branch"])?;
    repo.git(&["checkout", "master"])?;

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

    let head_subject = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["log", "-1", "--pretty=%s"])
        .output()?;
    assert!(head_subject.status.success(), "expected git log to succeed");
    let subject = String::from_utf8_lossy(&head_subject.stdout);
    assert!(
        subject.contains("feat: merge plan merge-smoke"),
        "expected merge commit subject to include slug, got: {subject}"
    );

    Ok(())
}

#[test]
fn test_run_approve_stage_stop_condition_retry_loop() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflow/approve-retry.toml",
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
succeeded = [\"stage_commit\"]\n\
[[nodes]]\n\
id = \"stage_commit\"\n\
kind = \"builtin\"\n\
uses = \"cap.env.builtin.git.stage_commit\"\n\
[nodes.args]\n\
message = \"feat: retry smoke\"\n\
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
            "file:.vizier/workflow/approve-retry.toml",
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
    let repo = IntegrationRepo::new()?;
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
            "--set",
            "prompt_text=Draft control-path plan.",
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
            "--set",
            "prompt_text=Draft cancel gate plan.",
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
        "--set".to_string(),
        "prompt_text=Draft cancel-path plan.".to_string(),
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

    let retry_repo = IntegrationRepo::new()?;
    clean_workdir(&retry_repo)?;
    write_stage_alias_test_config(&retry_repo)?;
    retry_repo.git(&["checkout", "-b", "draft/retry-smoke"])?;
    retry_repo.write("retry-smoke.txt", "retry smoke branch change\n")?;
    retry_repo.git(&["add", "retry-smoke.txt"])?;
    retry_repo.git(&["commit", "-m", "feat: retry smoke branch"])?;
    retry_repo.git(&["checkout", "master"])?;

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
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.git(&["checkout", "-b", "draft/conflict-smoke"])?;
    repo.write("a", "draft conflict content\n")?;
    repo.git(&["add", "a"])?;
    repo.git(&["commit", "-m", "feat: conflict branch change"])?;
    repo.git(&["checkout", "master"])?;
    repo.write("a", "master conflict content\n")?;
    repo.git(&["add", "a"])?;
    repo.git(&["commit", "-m", "feat: master conflict change"])?;

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

#[test]
fn test_run_execution_root_propagates_to_successor_nodes() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write(
        ".vizier/workflow/execution-root.toml",
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
            "file:.vizier/workflow/execution-root.toml",
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
    let prepare_worktree = prepare
        .pointer("/metadata/worktree_path")
        .and_then(Value::as_str)
        .ok_or("prepare missing worktree_path metadata")?;
    assert_eq!(
        prepare_execution_root, prepare_worktree,
        "prepare should set execution_root to prepared worktree"
    );
    assert_eq!(
        in_worktree
            .pointer("/metadata/execution_root")
            .and_then(Value::as_str),
        Some(prepare_execution_root),
        "success-edge propagation should carry worktree execution_root to in_worktree node"
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
