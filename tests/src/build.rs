use crate::fixtures::*;

use serde_json::Value;

fn collect_job_records(repo: &IntegrationRepo) -> TestResult<Vec<Value>> {
    let jobs_dir = repo.path().join(".vizier/jobs");
    let mut records = Vec::new();
    if !jobs_dir.exists() {
        return Ok(records);
    }
    for entry in fs::read_dir(jobs_dir)? {
        let entry = entry?;
        let path = entry.path().join("job.json");
        if !path.exists() {
            continue;
        }
        let contents = fs::read_to_string(path)?;
        let record: Value = serde_json::from_str(&contents)?;
        records.push(record);
    }
    Ok(records)
}

fn plan_slug(record: &Value) -> Option<String> {
    record
        .get("metadata")
        .and_then(|meta| meta.get("plan"))
        .and_then(Value::as_str)
        .map(|value| value.to_string())
}

fn dependency_plan_docs(record: &Value) -> Vec<(String, String)> {
    let mut deps = Vec::new();
    let Some(entries) = record
        .get("schedule")
        .and_then(|schedule| schedule.get("dependencies"))
        .and_then(Value::as_array)
    else {
        return deps;
    };

    for entry in entries {
        let Some(artifact) = entry.get("artifact") else {
            continue;
        };
        let Some(plan_doc) = artifact.get("plan_doc") else {
            continue;
        };
        let slug = plan_doc
            .get("slug")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let branch = plan_doc
            .get("branch")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if !slug.is_empty() {
            deps.push((slug, branch));
        }
    }
    deps
}

fn extract_child_file(record: &Value) -> Option<String> {
    let args = record.get("child_args").and_then(Value::as_array)?;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let Some(arg_str) = arg.as_str() else {
            continue;
        };
        if arg_str == "--file" {
            return iter.next().and_then(Value::as_str).map(|s| s.to_string());
        }
        if let Some(value) = arg_str.strip_prefix("--file=") {
            return Some(value.to_string());
        }
    }
    None
}

fn normalize_slug(input: &str) -> String {
    let mut normalized = String::new();
    let mut last_dash = false;

    for ch in input.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            normalized.push(lower);
            last_dash = false;
        } else if !last_dash {
            normalized.push('-');
            last_dash = true;
        }
    }

    while normalized.starts_with('-') {
        normalized.remove(0);
    }
    while normalized.ends_with('-') {
        normalized.pop();
    }

    if normalized.len() > 32 {
        normalized.truncate(32);
        while normalized.ends_with('-') {
            normalized.pop();
        }
    }

    normalized
}

fn slug_from_spec(spec: &str) -> String {
    let words: Vec<&str> = spec.split_whitespace().take(6).collect();
    let candidate = if words.is_empty() {
        "draft-plan".to_string()
    } else {
        words.join("-")
    };

    let normalized = normalize_slug(&candidate);
    if normalized.is_empty() {
        "draft-plan".to_string()
    } else {
        normalized
    }
}

#[test]
fn test_build_parses_toml_and_resolves_relative_paths() -> TestResult {
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

    let output = repo
        .vizier_cmd_background()
        .args(["build", "--file", "configs/build.toml"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let records = collect_job_records(&repo)?;
    assert_eq!(
        records.len(),
        2,
        "expected 2 build jobs, got {}",
        records.len()
    );

    let mut found_file_spec = false;
    for record in records {
        if let Some(path) = extract_child_file(&record) {
            let path = PathBuf::from(path);
            let resolved = if path.is_absolute() {
                path
            } else {
                repo.path().join(path)
            };
            let contents = fs::read_to_string(resolved)?;
            if contents.contains("Alpha spec for build") {
                found_file_spec = true;
            }
        }
    }

    assert!(
        found_file_spec,
        "expected at least one job input file to include intent file contents"
    );

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

    let output = repo
        .vizier_cmd_background()
        .args(["build", "--file", "build.json"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier build JSON failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let records = collect_job_records(&repo)?;
    assert_eq!(
        records.len(),
        1,
        "expected 1 build job, got {}",
        records.len()
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
    let output = repo
        .vizier_cmd_background()
        .args(["build", "--file", "bad.json"])
        .output()?;
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
    let output = repo
        .vizier_cmd_background()
        .args(["build", "--file", "empty.json"])
        .output()?;
    assert!(
        !output.status.success(),
        "expected build with empty intent content to fail"
    );

    Ok(())
}

#[test]
fn test_build_wires_dependencies_between_groups() -> TestResult {
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

    let output = repo
        .vizier_cmd_background()
        .args(["build", "--file", "build.toml"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let records = collect_job_records(&repo)?;
    assert_eq!(
        records.len(),
        4,
        "expected 4 build jobs, got {}",
        records.len()
    );

    let alpha = slug_from_spec("Alpha builder");
    let bravo = slug_from_spec("Bravo builder");
    let charlie = slug_from_spec("Charlie builder");
    let delta = slug_from_spec("Delta builder");

    let mut records_by_slug = std::collections::HashMap::new();
    for record in records {
        if let Some(slug) = plan_slug(&record) {
            records_by_slug.insert(slug, record);
        }
    }

    let alpha_record = records_by_slug
        .get(&alpha)
        .ok_or("missing alpha job record")?;
    assert!(
        dependency_plan_docs(alpha_record).is_empty(),
        "alpha step should have no dependencies"
    );

    let bravo_record = records_by_slug
        .get(&bravo)
        .ok_or("missing bravo job record")?;
    let bravo_deps = dependency_plan_docs(bravo_record);
    assert!(
        bravo_deps
            .iter()
            .any(|(slug, branch)| { slug == &alpha && branch == &format!("draft/{alpha}") }),
        "bravo should depend on alpha plan_doc"
    );

    let charlie_record = records_by_slug
        .get(&charlie)
        .ok_or("missing charlie job record")?;
    let charlie_deps = dependency_plan_docs(charlie_record);
    assert!(
        charlie_deps
            .iter()
            .any(|(slug, branch)| { slug == &alpha && branch == &format!("draft/{alpha}") }),
        "charlie should depend on alpha plan_doc"
    );

    let delta_record = records_by_slug
        .get(&delta)
        .ok_or("missing delta job record")?;
    let delta_deps = dependency_plan_docs(delta_record);
    assert!(
        delta_deps
            .iter()
            .any(|(slug, branch)| { slug == &bravo && branch == &format!("draft/{bravo}") }),
        "delta should depend on bravo plan_doc"
    );
    assert!(
        delta_deps
            .iter()
            .any(|(slug, branch)| { slug == &charlie && branch == &format!("draft/{charlie}") }),
        "delta should depend on charlie plan_doc"
    );

    Ok(())
}
