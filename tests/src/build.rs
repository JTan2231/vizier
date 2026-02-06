use crate::fixtures::*;

fn read_branch_file(repo: &Repository, branch: &str, rel_path: &str) -> TestResult<String> {
    let branch_ref = repo.find_branch(branch, BranchType::Local)?;
    let commit = branch_ref.get().peel_to_commit()?;
    let tree = commit.tree()?;
    let entry = tree.get_path(Path::new(rel_path))?;
    let blob = repo.find_blob(entry.id())?;
    Ok(String::from_utf8(blob.content().to_vec())?)
}

fn step_reads(step: &Value) -> Vec<String> {
    step.get("reads")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| {
            entry
                .get("step_key")
                .and_then(Value::as_str)
                .map(|value| value.to_string())
        })
        .collect()
}

fn execution_state(repo: &Repository, build_id: &str) -> TestResult<Value> {
    let branch = format!("build/{build_id}");
    let path = format!(".vizier/implementation-plans/builds/{build_id}/execution.json");
    let text = read_branch_file(repo, &branch, &path)?;
    Ok(serde_json::from_str(&text)?)
}

fn run_build_execute(
    repo: &IntegrationRepo,
    args: &[&str],
) -> Result<Output, Box<dyn std::error::Error>> {
    let mut cmd = repo.vizier_cmd_base();
    cmd.args(args);
    Ok(cmd.output()?)
}

fn dependency_has_completion_job(job_record: &Value, job_id: &str) -> bool {
    job_record
        .get("schedule")
        .and_then(|value| value.get("dependencies"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .any(|entry| {
            entry
                .get("artifact")
                .and_then(|artifact| artifact.get("ask_save_patch"))
                .and_then(|artifact| artifact.get("job_id"))
                .and_then(Value::as_str)
                == Some(job_id)
        })
}

#[test]
fn test_build_creates_session_artifacts_on_build_branch() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write("intents/alpha.md", "Alpha spec for build\n")?;

    let toml = r#"
steps = [
  { text = "Inline spec for build" },
  { file = "../intents/alpha.md" },
]
"#;
    repo.write("configs/build.toml", toml)?;

    let output = repo.vizier_output(&["build", "--file", "configs/build.toml"])?;
    assert!(
        output.status.success(),
        "vizier build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let build_id = find_save_field(&stdout, "Build").ok_or("build output missing Build")?;
    let branch = find_save_field(&stdout, "Branch").ok_or("build output missing Branch")?;
    let manifest_rel =
        find_save_field(&stdout, "Manifest").ok_or("build output missing Manifest")?;

    assert_eq!(branch, format!("build/{build_id}"));

    let repo_handle = repo.repo();
    let _ = repo_handle.find_branch(&branch, BranchType::Local)?;

    let manifest_text = read_branch_file(&repo_handle, &branch, &manifest_rel)?;
    let manifest: Value = serde_json::from_str(&manifest_text)?;

    assert_eq!(
        manifest
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "succeeded"
    );

    let copied_input = manifest
        .get("input_file")
        .and_then(|value| value.get("copied_path"))
        .and_then(Value::as_str)
        .ok_or("manifest input_file.copied_path missing")?;
    let copied_input_text = read_branch_file(&repo_handle, &branch, copied_input)?;
    assert!(copied_input_text.contains("Inline spec for build"));
    assert!(copied_input_text.contains("intents/alpha.md"));

    let steps = manifest
        .get("steps")
        .and_then(Value::as_array)
        .ok_or("manifest steps missing")?;
    assert_eq!(steps.len(), 2);

    for step in steps {
        let plan_path = step
            .get("output_plan_path")
            .and_then(Value::as_str)
            .ok_or("step output_plan_path missing")?;
        let plan_text = read_branch_file(&repo_handle, &branch, plan_path)?;
        assert!(plan_text.contains("## Operator Spec"));
        assert!(plan_text.contains("## Implementation Plan"));
    }

    let summary_rel = manifest
        .get("artifacts")
        .and_then(|value| value.get("summary"))
        .and_then(Value::as_str)
        .ok_or("manifest artifacts.summary missing")?;
    let summary_text = read_branch_file(&repo_handle, &branch, summary_rel)?;
    assert!(summary_text.contains("# Build Session Summary"));

    let temp_root = repo.path().join(".vizier/tmp-worktrees");
    if temp_root.exists() {
        let leftover_build_worktrees = fs::read_dir(&temp_root)?
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("build-"))
            .count();
        assert_eq!(
            leftover_build_worktrees, 0,
            "build worktree should be cleaned"
        );
    }

    Ok(())
}

#[test]
fn test_build_parses_json() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let json = r#"
{
  "steps": [
    { "text": "Build JSON spec" }
  ]
}
"#;
    repo.write("build.json", json)?;

    let output = repo.vizier_output(&["build", "--file", "build.json"])?;
    assert!(
        output.status.success(),
        "vizier build JSON failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let branch = find_save_field(&stdout, "Branch").ok_or("build output missing Branch")?;
    let manifest_rel =
        find_save_field(&stdout, "Manifest").ok_or("build output missing Manifest")?;

    let repo_handle = repo.repo();
    let manifest_text = read_branch_file(&repo_handle, &branch, &manifest_rel)?;
    let manifest: Value = serde_json::from_str(&manifest_text)?;

    let steps = manifest
        .get("steps")
        .and_then(Value::as_array)
        .ok_or("manifest steps missing")?;
    assert_eq!(steps.len(), 1);
    assert_eq!(
        steps[0]
            .get("step_key")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "01"
    );

    Ok(())
}

#[test]
fn test_build_rejects_invalid_entries() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let json = r#"
{
  "steps": [
    { "text": "ok", "extra": "nope" }
  ]
}
"#;
    repo.write("bad.json", json)?;
    let output = repo.vizier_output(&["build", "--file", "bad.json"])?;
    assert!(
        !output.status.success(),
        "expected build with unknown keys to fail"
    );

    let json_empty = r#"
{
  "steps": [
    { "text": "   " }
  ]
}
"#;
    repo.write("empty.json", json_empty)?;
    let output = repo.vizier_output(&["build", "--file", "empty.json"])?;
    assert!(
        !output.status.success(),
        "expected build with empty intent content to fail"
    );

    Ok(())
}

#[test]
fn test_build_manifest_reads_prior_stages_only() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let steps = r#"
steps = [
  { text = "Alpha builder" },
  [
    { text = "Bravo builder" },
    { text = "Charlie builder" },
  ],
  { text = "Delta builder" },
]
"#;
    repo.write("build.toml", steps)?;

    let output = repo.vizier_output(&["build", "--file", "build.toml"])?;
    assert!(
        output.status.success(),
        "vizier build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let branch = find_save_field(&stdout, "Branch").ok_or("build output missing Branch")?;
    let manifest_rel =
        find_save_field(&stdout, "Manifest").ok_or("build output missing Manifest")?;

    let repo_handle = repo.repo();
    let manifest_text = read_branch_file(&repo_handle, &branch, &manifest_rel)?;
    let manifest: Value = serde_json::from_str(&manifest_text)?;
    let steps = manifest
        .get("steps")
        .and_then(Value::as_array)
        .ok_or("manifest steps missing")?;

    let mut by_key = std::collections::HashMap::new();
    for step in steps {
        let key = step
            .get("step_key")
            .and_then(Value::as_str)
            .ok_or("step_key missing")?
            .to_string();
        by_key.insert(key, step.clone());
    }

    let reads_01 = step_reads(by_key.get("01").ok_or("missing step 01")?);
    assert!(reads_01.is_empty(), "01 should have no reads");

    let reads_02a = step_reads(by_key.get("02a").ok_or("missing step 02a")?);
    assert_eq!(reads_02a, vec!["01".to_string()]);

    let reads_02b = step_reads(by_key.get("02b").ok_or("missing step 02b")?);
    assert_eq!(reads_02b, vec!["01".to_string()]);

    let reads_03 = step_reads(by_key.get("03").ok_or("missing step 03")?);
    assert!(reads_03.contains(&"01".to_string()));
    assert!(reads_03.contains(&"02a".to_string()));
    assert!(reads_03.contains(&"02b".to_string()));

    Ok(())
}

#[test]
fn test_build_failure_preserves_failed_manifest() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let toml = r#"
steps = [
  { text = "Alpha build that will fail" },
  { text = "Bravo build should not run" },
]
"#;
    repo.write("build.toml", toml)?;

    let mut cmd = repo.vizier_cmd();
    cmd.env("VIZIER_FORCE_AGENT_ERROR", "1");
    cmd.args(["build", "--file", "build.toml"]);
    let output = cmd.output()?;
    assert!(
        !output.status.success(),
        "vizier build should fail when agent backend fails"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let branch = find_save_field(&stdout, "Branch").ok_or("build output missing Branch")?;
    let manifest_rel =
        find_save_field(&stdout, "Manifest").ok_or("build output missing Manifest")?;

    let repo_handle = repo.repo();
    let _ = repo_handle.find_branch(&branch, BranchType::Local)?;

    let manifest_text = read_branch_file(&repo_handle, &branch, &manifest_rel)?;
    let manifest: Value = serde_json::from_str(&manifest_text)?;
    assert_eq!(
        manifest
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "failed"
    );

    let failed_steps = manifest
        .get("steps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|step| {
            step.get("result")
                .and_then(Value::as_str)
                .map(|value| value == "failed")
                .unwrap_or(false)
        })
        .count();
    assert!(failed_steps >= 1, "expected at least one failed step");

    Ok(())
}

#[test]
fn test_build_name_override_and_collision() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write(
        "build.toml",
        r#"
steps = [
  { text = "Named build session" },
]
"#,
    )?;

    let first =
        repo.vizier_output(&["build", "--file", "build.toml", "--name", "release-batch"])?;
    assert!(
        first.status.success(),
        "initial named build failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_stdout = String::from_utf8_lossy(&first.stdout);
    assert_eq!(
        find_save_field(&first_stdout, "Build").as_deref(),
        Some("release-batch")
    );
    assert_eq!(
        find_save_field(&first_stdout, "Branch").as_deref(),
        Some("build/release-batch")
    );

    let second =
        repo.vizier_output(&["build", "--file", "build.toml", "--name", "release-batch"])?;
    assert!(
        !second.status.success(),
        "expected build name collision to fail"
    );
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("already exists"),
        "expected collision error, got: {stderr}"
    );

    Ok(())
}

#[test]
fn test_build_execute_requires_yes_in_non_tty() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write(
        "build.toml",
        r#"
steps = [
  { text = "Require yes check" },
]
"#,
    )?;
    let built = repo.vizier_output(&["build", "--file", "build.toml", "--name", "needs-yes"])?;
    assert!(
        built.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );

    let output = run_build_execute(&repo, &["build", "execute", "needs-yes"])?;
    assert!(
        !output.status.success(),
        "expected build execute without --yes to fail in non-TTY mode"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("requires --yes"),
        "missing --yes error not surfaced: {stderr}"
    );

    Ok(())
}

#[test]
fn test_build_execute_pipeline_shapes() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    for (build_id, pipeline, expects_review, expects_merge) in [
        ("pipe-approve", "approve", false, false),
        ("pipe-approve-review", "approve-review", true, false),
        (
            "pipe-approve-review-merge",
            "approve-review-merge",
            true,
            true,
        ),
    ] {
        repo.write(
            &format!("{build_id}.toml"),
            r#"
steps = [
  { text = "Pipeline shape check" },
]
"#,
        )?;
        let built = repo.vizier_output(&[
            "build",
            "--file",
            &format!("{build_id}.toml"),
            "--name",
            build_id,
        ])?;
        assert!(
            built.status.success(),
            "build {} failed: {}",
            build_id,
            String::from_utf8_lossy(&built.stderr)
        );

        let execute = run_build_execute(
            &repo,
            &[
                "build",
                "execute",
                build_id,
                "--pipeline",
                pipeline,
                "--yes",
            ],
        )?;
        assert!(
            execute.status.success(),
            "build execute {} failed: {}",
            build_id,
            String::from_utf8_lossy(&execute.stderr)
        );

        let state = execution_state(&repo.repo(), build_id)?;
        let steps = state
            .get("steps")
            .and_then(Value::as_array)
            .ok_or("execution steps missing")?;
        assert_eq!(steps.len(), 1);
        let step = &steps[0];

        assert!(
            step.get("materialize_job_id")
                .and_then(Value::as_str)
                .is_some()
        );
        assert!(
            step.get("approve_job_id").and_then(Value::as_str).is_some(),
            "approve job id missing for pipeline {pipeline}"
        );
        assert_eq!(
            step.get("review_job_id").and_then(Value::as_str).is_some(),
            expects_review,
            "review job presence mismatch for pipeline {pipeline}"
        );
        assert_eq!(
            step.get("merge_job_id").and_then(Value::as_str).is_some(),
            expects_merge,
            "merge job presence mismatch for pipeline {pipeline}"
        );
    }

    Ok(())
}

#[test]
fn test_build_execute_stage_dependencies_for_parallel_stages() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write(
        "staged.toml",
        r#"
steps = [
  { text = "Stage one" },
  [
    { text = "Stage two alpha" },
    { text = "Stage two beta" },
  ],
  { text = "Stage three" },
]
"#,
    )?;
    let built = repo.vizier_output(&["build", "--file", "staged.toml", "--name", "stage-graph"])?;
    assert!(
        built.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );

    let execute = run_build_execute(
        &repo,
        &[
            "build",
            "execute",
            "stage-graph",
            "--pipeline",
            "approve",
            "--yes",
        ],
    )?;
    assert!(
        execute.status.success(),
        "build execute failed: {}",
        String::from_utf8_lossy(&execute.stderr)
    );

    let state = execution_state(&repo.repo(), "stage-graph")?;
    let steps = state
        .get("steps")
        .and_then(Value::as_array)
        .ok_or("execution steps missing")?;

    let mut by_key = std::collections::HashMap::new();
    for step in steps {
        let key = step
            .get("step_key")
            .and_then(Value::as_str)
            .ok_or("step_key missing")?
            .to_string();
        by_key.insert(key, step.clone());
    }

    let s01_approve = by_key
        .get("01")
        .and_then(|step| step.get("approve_job_id"))
        .and_then(Value::as_str)
        .ok_or("step 01 approve job missing")?;
    let s02a_approve = by_key
        .get("02a")
        .and_then(|step| step.get("approve_job_id"))
        .and_then(Value::as_str)
        .ok_or("step 02a approve job missing")?;
    let s02b_approve = by_key
        .get("02b")
        .and_then(|step| step.get("approve_job_id"))
        .and_then(Value::as_str)
        .ok_or("step 02b approve job missing")?;

    let s02a_materialize = by_key
        .get("02a")
        .and_then(|step| step.get("materialize_job_id"))
        .and_then(Value::as_str)
        .ok_or("step 02a materialize job missing")?;
    let s02b_materialize = by_key
        .get("02b")
        .and_then(|step| step.get("materialize_job_id"))
        .and_then(Value::as_str)
        .ok_or("step 02b materialize job missing")?;
    let s03_materialize = by_key
        .get("03")
        .and_then(|step| step.get("materialize_job_id"))
        .and_then(Value::as_str)
        .ok_or("step 03 materialize job missing")?;

    let s02a_record = read_job_record(&repo, s02a_materialize)?;
    assert!(
        dependency_has_completion_job(&s02a_record, s01_approve),
        "step 02a materialize should depend on step 01 approve completion"
    );
    let s02b_record = read_job_record(&repo, s02b_materialize)?;
    assert!(
        dependency_has_completion_job(&s02b_record, s01_approve),
        "step 02b materialize should depend on step 01 approve completion"
    );

    let s03_record = read_job_record(&repo, s03_materialize)?;
    assert!(
        dependency_has_completion_job(&s03_record, s02a_approve),
        "step 03 materialize should depend on step 02a approve completion"
    );
    assert!(
        dependency_has_completion_job(&s03_record, s02b_approve),
        "step 03 materialize should depend on step 02b approve completion"
    );

    Ok(())
}

#[test]
fn test_build_execute_materializes_plan_doc_with_derived_front_matter() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write(
        "materialize.toml",
        r#"
steps = [
  { text = "Materialize output check" },
]
"#,
    )?;
    let built = repo.vizier_output(&[
        "build",
        "--file",
        "materialize.toml",
        "--name",
        "materialize-check",
    ])?;
    assert!(
        built.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );

    let execute = run_build_execute(
        &repo,
        &[
            "build",
            "execute",
            "materialize-check",
            "--pipeline",
            "approve",
            "--yes",
        ],
    )?;
    assert!(
        execute.status.success(),
        "build execute failed: {}",
        String::from_utf8_lossy(&execute.stderr)
    );

    let state = execution_state(&repo.repo(), "materialize-check")?;
    let first_step = state
        .get("steps")
        .and_then(Value::as_array)
        .and_then(|steps| steps.first())
        .ok_or("execution step missing")?;
    let slug = first_step
        .get("derived_slug")
        .and_then(Value::as_str)
        .ok_or("derived_slug missing")?;
    let branch = first_step
        .get("derived_branch")
        .and_then(Value::as_str)
        .ok_or("derived_branch missing")?;
    let materialize_job = first_step
        .get("materialize_job_id")
        .and_then(Value::as_str)
        .ok_or("materialize job missing")?;

    wait_for_job_status(&repo, materialize_job, "succeeded", Duration::from_secs(60))?;

    let plan_path = format!(".vizier/implementation-plans/{slug}.md");
    let plan_text = read_branch_file(&repo.repo(), branch, &plan_path)?;
    assert!(
        plan_text.starts_with(&format!("---\nplan: {slug}\nbranch: {branch}\n---\n")),
        "materialized front matter mismatch:\n{plan_text}"
    );
    assert!(plan_text.contains("## Operator Spec"));
    assert!(plan_text.contains("## Implementation Plan"));

    Ok(())
}

#[test]
fn test_build_execute_resume_dedupes_and_enforces_pipeline_match() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write(
        "resume.toml",
        r#"
steps = [
  { text = "Resume semantics check" },
]
"#,
    )?;
    let built =
        repo.vizier_output(&["build", "--file", "resume.toml", "--name", "resume-check"])?;
    assert!(
        built.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );

    let first = run_build_execute(
        &repo,
        &[
            "build",
            "execute",
            "resume-check",
            "--pipeline",
            "approve",
            "--yes",
        ],
    )?;
    assert!(
        first.status.success(),
        "initial execute failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let before = execution_state(&repo.repo(), "resume-check")?;
    let before_step = before
        .get("steps")
        .and_then(Value::as_array)
        .and_then(|steps| steps.first())
        .ok_or("execution step missing")?;
    let before_materialize = before_step
        .get("materialize_job_id")
        .and_then(Value::as_str)
        .ok_or("materialize job missing before resume")?
        .to_string();
    let before_approve = before_step
        .get("approve_job_id")
        .and_then(Value::as_str)
        .ok_or("approve job missing before resume")?
        .to_string();

    wait_for_job_status(
        &repo,
        &before_materialize,
        "succeeded",
        Duration::from_secs(60),
    )?;
    wait_for_job_status(&repo, &before_approve, "succeeded", Duration::from_secs(60))?;

    let resumed = run_build_execute(
        &repo,
        &[
            "build",
            "execute",
            "resume-check",
            "--pipeline",
            "approve",
            "--resume",
            "--yes",
        ],
    )?;
    assert!(
        resumed.status.success(),
        "resume execute failed: {}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let after = execution_state(&repo.repo(), "resume-check")?;
    let after_step = after
        .get("steps")
        .and_then(Value::as_array)
        .and_then(|steps| steps.first())
        .ok_or("execution step missing after resume")?;
    assert_eq!(
        after_step
            .get("materialize_job_id")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        before_materialize
    );
    assert_eq!(
        after_step
            .get("approve_job_id")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        before_approve
    );

    let mismatch = run_build_execute(
        &repo,
        &[
            "build",
            "execute",
            "resume-check",
            "--pipeline",
            "approve-review",
            "--resume",
            "--yes",
        ],
    )?;
    assert!(
        !mismatch.status.success(),
        "expected pipeline mismatch to fail"
    );
    let mismatch_stderr = String::from_utf8_lossy(&mismatch.stderr);
    assert!(
        mismatch_stderr.contains("pipeline mismatch"),
        "pipeline mismatch reason missing: {mismatch_stderr}"
    );

    let missing_resume = run_build_execute(
        &repo,
        &[
            "build",
            "execute",
            "resume-check",
            "--pipeline",
            "approve",
            "--yes",
        ],
    )?;
    assert!(
        !missing_resume.status.success(),
        "expected missing --resume to fail when execution state exists"
    );
    let missing_resume_stderr = String::from_utf8_lossy(&missing_resume.stderr);
    assert!(
        missing_resume_stderr.contains("rerun with --resume"),
        "missing --resume reason missing: {missing_resume_stderr}"
    );

    Ok(())
}

#[test]
fn test_build_execute_failure_blocks_downstream_stage() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write(
        "blocking.toml",
        r#"
steps = [
  { text = "Failing stage one approve" },
  { text = "Stage two should block" },
]
"#,
    )?;
    let built =
        repo.vizier_output(&["build", "--file", "blocking.toml", "--name", "block-check"])?;
    assert!(
        built.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );

    let mut execute = repo.vizier_cmd_base();
    execute.env("VIZIER_FORCE_AGENT_ERROR", "1");
    execute.args([
        "build",
        "execute",
        "block-check",
        "--pipeline",
        "approve",
        "--yes",
    ]);
    let output = execute.output()?;
    assert!(
        output.status.success(),
        "build execute should still queue even when downstream jobs fail: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let state = execution_state(&repo.repo(), "block-check")?;
    let steps = state
        .get("steps")
        .and_then(Value::as_array)
        .ok_or("execution steps missing")?;
    let step_one = steps
        .iter()
        .find(|step| step.get("step_key").and_then(Value::as_str) == Some("01"))
        .ok_or("step 01 missing")?;
    let step_two = steps
        .iter()
        .find(|step| step.get("step_key").and_then(Value::as_str) == Some("02"))
        .ok_or("step 02 missing")?;

    let approve_job = step_one
        .get("approve_job_id")
        .and_then(Value::as_str)
        .ok_or("step 01 approve job missing")?;
    let step_two_materialize = step_two
        .get("materialize_job_id")
        .and_then(Value::as_str)
        .ok_or("step 02 materialize job missing")?;

    wait_for_job_status(&repo, approve_job, "failed", Duration::from_secs(60))?;
    wait_for_job_status(
        &repo,
        step_two_materialize,
        "blocked_by_dependency",
        Duration::from_secs(60),
    )?;

    Ok(())
}
