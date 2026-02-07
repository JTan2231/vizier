use crate::fixtures::*;

fn read_branch_file(repo: &Repository, branch: &str, rel_path: &str) -> TestResult<String> {
    let branch_ref = repo.find_branch(branch, BranchType::Local)?;
    let commit = branch_ref.get().peel_to_commit()?;
    let tree = commit.tree()?;
    let entry = tree.get_path(Path::new(rel_path))?;
    let blob = repo.find_blob(entry.id())?;
    Ok(String::from_utf8(blob.content().to_vec())?)
}

fn run_patch(repo: &IntegrationRepo, args: &[&str]) -> io::Result<Output> {
    let mut cmd = repo.vizier_cmd_base();
    cmd.args(args);
    cmd.output()
}

#[test]
fn test_patch_preflight_rejects_invalid_inputs_before_enqueue() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;
    repo.write("specs/ok.md", "Valid spec\n")?;
    repo.write("specs/empty.md", "   \n")?;

    let output = run_patch(
        &repo,
        &[
            "patch",
            "specs/ok.md",
            "specs/missing.md",
            "specs/empty.md",
            "--yes",
        ],
    )?;
    assert!(
        !output.status.success(),
        "patch preflight with invalid files should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("patch preflight failed"),
        "missing preflight error context: {stderr}"
    );

    let jobs_root = repo.path().join(".vizier/jobs");
    let queued_jobs = fs::read_dir(&jobs_root)?
        .flatten()
        .filter(|entry| entry.path().join("job.json").exists())
        .count();
    assert_eq!(queued_jobs, 0, "preflight failure should enqueue no jobs");
    Ok(())
}

#[test]
fn test_patch_preserves_cli_order_in_build_manifest() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write("specs/first.md", "First intent: alpha\n")?;
    repo.write("specs/second.md", "Second intent: bravo\n")?;

    let output = run_patch(
        &repo,
        &[
            "patch",
            "specs/second.md",
            "specs/first.md",
            "--pipeline",
            "approve-review",
            "--yes",
        ],
    )?;
    assert!(
        output.status.success(),
        "patch command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let patch_session =
        find_save_field(&stdout, "Patch session").ok_or("patch output missing Patch session")?;
    assert!(
        stdout.contains("Patch queue:"),
        "patch output should include queue table: {stdout}"
    );
    assert!(
        stdout.contains("1. specs/second.md") && stdout.contains("2. specs/first.md"),
        "patch queue should preserve CLI order: {stdout}"
    );
    assert!(
        stdout.contains("Pipeline override") && stdout.contains("approve-review"),
        "expected pipeline override in output: {stdout}"
    );

    let branch = format!("build/{patch_session}");
    let manifest_rel = format!(".vizier/implementation-plans/builds/{patch_session}/manifest.json");
    let manifest_text = read_branch_file(&repo.repo(), &branch, &manifest_rel)?;
    let manifest: Value = serde_json::from_str(&manifest_text)?;
    let steps = manifest
        .get("steps")
        .and_then(Value::as_array)
        .ok_or("manifest steps missing")?;
    assert_eq!(steps.len(), 2, "expected two manifest steps");

    let intent0 = steps[0]
        .get("intent_source")
        .and_then(Value::as_str)
        .ok_or("step 0 intent_source missing")?;
    let intent1 = steps[1]
        .get("intent_source")
        .and_then(Value::as_str)
        .ok_or("step 1 intent_source missing")?;
    assert!(
        intent0.ends_with("/specs/second.md") && intent1.ends_with("/specs/first.md"),
        "manifest intent order should match CLI order: step0={intent0}, step1={intent1}"
    );

    Ok(())
}

#[test]
fn test_patch_sets_patch_metadata_on_phase_jobs_and_resume_reuses_jobs() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write("specs/a.md", "Patch metadata a\n")?;
    repo.write("specs/b.md", "Patch metadata b\n")?;

    let first = run_patch(
        &repo,
        &[
            "patch",
            "specs/a.md",
            "specs/b.md",
            "--pipeline",
            "approve",
            "--yes",
        ],
    )?;
    assert!(
        first.status.success(),
        "initial patch run failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_stdout = String::from_utf8_lossy(&first.stdout);
    let patch_session = find_save_field(&first_stdout, "Patch session")
        .ok_or("patch output missing Patch session")?;

    let jobs_root = repo.path().join(".vizier/jobs");
    let first_job_ids: HashSet<String> = fs::read_dir(&jobs_root)?
        .flatten()
        .filter_map(|entry| {
            let id = entry.file_name().to_string_lossy().to_string();
            if entry.path().join("job.json").exists() {
                Some(id)
            } else {
                None
            }
        })
        .collect();
    assert!(
        !first_job_ids.is_empty(),
        "expected patch execution to enqueue jobs"
    );

    let job_records = fs::read_dir(&jobs_root)?
        .flatten()
        .filter_map(|entry| fs::read_to_string(entry.path().join("job.json")).ok())
        .filter_map(|raw| serde_json::from_str::<Value>(&raw).ok())
        .collect::<Vec<_>>();
    let approve_jobs = job_records
        .iter()
        .filter(|record| {
            record
                .get("metadata")
                .and_then(|meta| meta.get("scope"))
                .and_then(Value::as_str)
                == Some("approve")
        })
        .collect::<Vec<_>>();
    assert_eq!(approve_jobs.len(), 2, "expected two approve phase jobs");
    for job in approve_jobs {
        let meta = job
            .get("metadata")
            .and_then(Value::as_object)
            .ok_or("approve job metadata missing")?;
        let patch_file = meta
            .get("patch_file")
            .and_then(Value::as_str)
            .ok_or("patch_file metadata missing")?;
        let patch_index = meta
            .get("patch_index")
            .and_then(Value::as_u64)
            .ok_or("patch_index metadata missing")?;
        let patch_total = meta
            .get("patch_total")
            .and_then(Value::as_u64)
            .ok_or("patch_total metadata missing")?;
        assert!(
            patch_file == "specs/a.md" || patch_file == "specs/b.md",
            "unexpected patch_file metadata: {patch_file}"
        );
        assert!(
            (1..=2).contains(&patch_index),
            "patch_index should be 1-based for two files"
        );
        assert_eq!(patch_total, 2, "patch_total should match file count");
    }

    let resumed = run_patch(
        &repo,
        &[
            "patch",
            "specs/a.md",
            "specs/b.md",
            "--pipeline",
            "approve",
            "--yes",
            "--resume",
        ],
    )?;
    assert!(
        resumed.status.success(),
        "patch resume failed: {}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let resume_stdout = String::from_utf8_lossy(&resumed.stdout);
    assert!(
        resume_stdout.contains("Build execution resumed"),
        "expected resumed execution output: {resume_stdout}"
    );

    let second_job_ids: HashSet<String> = fs::read_dir(&jobs_root)?
        .flatten()
        .filter_map(|entry| {
            let id = entry.file_name().to_string_lossy().to_string();
            if entry.path().join("job.json").exists() {
                Some(id)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        first_job_ids, second_job_ids,
        "resume should reuse existing jobs without adding new ones for patch session {patch_session}"
    );

    Ok(())
}
