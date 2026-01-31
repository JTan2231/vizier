use crate::fixtures::*;

#[test]
fn test_ask_creates_single_combined_commit() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;

    let output = repo.vizier_output(&["ask", "single commit check"])?;
    assert!(
        output.status.success(),
        "vizier ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after = count_commits_from_head(&repo.repo())?;
    assert_eq!(after - before, 1, "ask should create one combined commit");
    let files = files_changed_in_commit(&repo.repo(), "HEAD")?;
    assert!(
        files.contains(".vizier/narrative/snapshot.md")
            && files.contains(".vizier/narrative/glossary.md")
            && files.contains("a"),
        "ask commit should include code and narrative assets, got {files:?}"
    );
    Ok(())
}
#[test]
fn test_ask_reports_token_usage_progress() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo.vizier_output(&["ask", "token usage integration smoke"])?;
    assert!(
        output.status.success(),
        "vizier ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[codex:ask] agent — mock agent running"),
        "expected agent progress line, stderr was:\n{}",
        stderr
    );
    assert!(
        !stderr.to_ascii_lowercase().contains("token usage"),
        "token usage progress should not be emitted anymore:\n{}",
        stderr
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Agent run:"),
        "ask stdout should include agent run summary:\n{}",
        stdout
    );

    let quiet_repo = IntegrationRepo::new()?;
    let quiet = quiet_repo.vizier_output(&["-q", "ask", "quiet usage check"])?;
    assert!(
        quiet.status.success(),
        "quiet vizier ask failed: {}",
        String::from_utf8_lossy(&quiet.stderr)
    );
    let quiet_stderr = String::from_utf8_lossy(&quiet.stderr);
    assert!(
        quiet_stderr.is_empty(),
        "quiet mode should suppress agent progress but printed:\n{}",
        quiet_stderr
    );
    Ok(())
}
