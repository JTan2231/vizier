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
