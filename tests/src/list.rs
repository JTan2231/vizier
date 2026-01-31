use crate::fixtures::*;

#[test]
fn test_list_outputs_prettified_blocks() -> TestResult {
    let repo = IntegrationRepo::new()?;

    let empty = repo.vizier_output(&["list"])?;
    assert!(
        empty.status.success(),
        "vizier list (empty) failed: {}",
        String::from_utf8_lossy(&empty.stderr)
    );
    let empty_stdout = String::from_utf8_lossy(&empty.stdout);
    assert!(
        empty_stdout.contains("Outcome: No pending draft branches"),
        "empty list output missing outcome: {empty_stdout}"
    );

    let draft_alpha = repo.vizier_output(&["draft", "--name", "alpha", "Alpha spec line"])?;
    assert!(
        draft_alpha.status.success(),
        "vizier draft alpha failed: {}",
        String::from_utf8_lossy(&draft_alpha.stderr)
    );
    let draft_beta = repo.vizier_output(&["draft", "--name", "beta", "Beta spec line"])?;
    assert!(
        draft_beta.status.success(),
        "vizier draft beta failed: {}",
        String::from_utf8_lossy(&draft_beta.stderr)
    );

    let list = repo.vizier_output(&["list"])?;
    assert!(
        list.status.success(),
        "vizier list failed: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(
        stdout.contains("Outcome: 2 pending draft branches"),
        "list header missing pending count: {stdout}"
    );
    assert!(
        stdout.contains("\n\n  Plan   : beta"),
        "list output should separate entries with whitespace: {stdout}"
    );
    for (slug, summary) in [("alpha", "Alpha spec line"), ("beta", "Beta spec line")] {
        assert!(
            stdout.contains(&format!("  Plan   : {slug}")),
            "list output missing plan {slug}: {stdout}"
        );
        assert!(
            stdout.contains(&format!("  Branch : draft/{slug}")),
            "list output missing branch for {slug}: {stdout}"
        );
        assert!(
            stdout.contains(&format!("  Summary: {summary}")),
            "list output missing summary for {slug}: {stdout}"
        );
    }

    Ok(())
}
#[test]
fn test_list_includes_inline_job_commands() -> TestResult {
    let repo = IntegrationRepo::new()?;

    let draft_alpha = repo.vizier_output(&["draft", "--name", "alpha", "Alpha spec line"])?;
    assert!(
        draft_alpha.status.success(),
        "vizier draft alpha failed: {}",
        String::from_utf8_lossy(&draft_alpha.stderr)
    );

    let job_id = "inline-job-alpha";
    write_job_record(
        &repo,
        job_id,
        json!({
            "id": job_id,
            "status": "running",
            "command": ["vizier", "approve"],
            "created_at": "2026-01-27T18:42:03Z",
            "started_at": "2026-01-27T18:42:03Z",
            "stdout_path": "stdout.log",
            "stderr_path": "stderr.log",
            "metadata": {
                "plan": "alpha",
                "branch": "draft/alpha",
                "scope": "approve"
            }
        }),
    )?;

    let list = repo.vizier_output(&["list"])?;
    assert!(
        list.status.success(),
        "vizier list failed: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(
        stdout.contains(job_id),
        "list output missing job id: {stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.contains("Job status") && line.contains("running")),
        "list output missing job status line: {stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.contains("Job scope") && line.contains("approve")),
        "list output missing job scope line: {stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.contains("Job started") && line.contains("2026-01-27T18:42:03")),
        "list output missing job started line: {stdout}"
    );
    assert!(
        stdout.contains(&format!("vizier jobs status {job_id}")),
        "list output missing status command: {stdout}"
    );
    assert!(
        stdout.contains(&format!("vizier jobs tail --follow {job_id}")),
        "list output missing follow logs command: {stdout}"
    );
    assert!(
        stdout.contains(&format!("vizier jobs attach {job_id}")),
        "list output missing attach command: {stdout}"
    );

    Ok(())
}
