use crate::fixtures::*;
use git2::ObjectType;

fn tag_names(repo: &IntegrationRepo) -> TestResult<Vec<String>> {
    let repo = repo.repo();
    let names = repo
        .tag_names(None)?
        .iter()
        .flatten()
        .map(|name| name.to_string())
        .collect::<Vec<_>>();
    Ok(names)
}

#[test]
fn test_release_dry_run_prints_plan_without_mutation() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write("release.txt", "feat one\n")?;
    repo.git(&["add", "release.txt"])?;
    repo.git(&["commit", "-m", "feat: add release flow"])?;

    let commit_count_before = count_commits_from_head(&repo.repo())?;
    let tags_before = tag_names(&repo)?;

    let output = repo.vizier_output(&["release", "--dry-run"])?;
    assert!(
        output.status.success(),
        "vizier release --dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Release dry run"),
        "expected dry-run outcome block: {stdout}"
    );
    assert!(
        stdout.contains("Last tag") && stdout.contains("none"),
        "expected no prior tag on first release: {stdout}"
    );
    assert!(
        stdout.contains("Computed bump") && stdout.contains("minor"),
        "expected minor bump from feat commit: {stdout}"
    );
    assert!(
        stdout.contains("Next version") && stdout.contains("v0.1.0"),
        "expected next version v0.1.0: {stdout}"
    );
    assert!(
        stdout.contains("Release notes preview:"),
        "expected release notes preview in dry run: {stdout}"
    );
    assert!(
        stdout.contains("Changes:"),
        "expected Changes heading in preview: {stdout}"
    );
    for legacy_heading in [
        "Breaking Changes:",
        "Features:",
        "Fixes/Performance:",
        "Other:",
    ] {
        assert!(
            !stdout.contains(legacy_heading),
            "dry-run preview should not include {legacy_heading}: {stdout}"
        );
    }

    let commit_count_after = count_commits_from_head(&repo.repo())?;
    let tags_after = tag_names(&repo)?;
    assert_eq!(
        commit_count_before, commit_count_after,
        "dry-run must not create commits"
    );
    assert_eq!(tags_before, tags_after, "dry-run must not create tags");

    Ok(())
}

#[test]
fn test_release_notes_preview_filters_non_conventional_subjects() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write("changes.txt", "one\n")?;
    repo.git(&["add", "changes.txt"])?;
    repo.git(&["commit", "-m", "feat: include this change"])?;

    repo.write("changes.txt", "two\n")?;
    repo.git(&["add", "changes.txt"])?;
    repo.git(&["commit", "-m", "Merge branch 'topic' into main"])?;

    repo.write("changes.txt", "three\n")?;
    repo.git(&["add", "changes.txt"])?;
    repo.git(&["commit", "-m", "fix(core): include this fix"])?;

    let output = repo.vizier_output(&["release", "--dry-run", "--max-commits", "10"])?;
    assert!(
        output.status.success(),
        "vizier release --dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Changes:"),
        "expected Changes heading in preview: {stdout}"
    );
    assert!(
        stdout.contains("feat: include this change"),
        "expected conventional feat commit in preview: {stdout}"
    );
    assert!(
        stdout.contains("fix(core): include this fix"),
        "expected conventional fix commit in preview: {stdout}"
    );
    assert!(
        !stdout.contains("Merge branch 'topic' into main"),
        "non-conventional commit subject should be excluded: {stdout}"
    );

    Ok(())
}

#[test]
fn test_release_notes_preview_respects_max_commits_overflow() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    for (index, subject) in [
        "feat: first change",
        "fix: second change",
        "docs: third change",
    ]
    .iter()
    .enumerate()
    {
        let contents = format!("{index}\n");
        repo.write("overflow.txt", &contents)?;
        repo.git(&["add", "overflow.txt"])?;
        repo.git(&["commit", "-m", subject])?;
    }

    let output = repo.vizier_output(&["release", "--dry-run", "--max-commits", "2"])?;
    assert!(
        output.status.success(),
        "vizier release --dry-run --max-commits 2 failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Changes:"),
        "expected Changes heading in preview: {stdout}"
    );
    assert!(
        stdout.contains("  - +1 more"),
        "expected overflow summary in preview: {stdout}"
    );

    Ok(())
}

#[test]
fn test_release_yes_creates_commit_and_annotated_tag() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write("bugfix.txt", "patch\n")?;
    repo.git(&["add", "bugfix.txt"])?;
    repo.git(&["commit", "-m", "fix: patch release bug"])?;

    let output = repo.vizier_output(&["release", "--yes"])?;
    assert!(
        output.status.success(),
        "vizier release --yes failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Release complete"),
        "expected release completion output: {stdout}"
    );
    assert!(
        stdout.contains("Tag") && stdout.contains("v0.0.1"),
        "expected created tag in output: {stdout}"
    );

    let repo_handle = repo.repo();
    let head_commit = repo_handle.head()?.peel_to_commit()?;
    assert_eq!(
        head_commit.summary(),
        Some("chore(release): v0.0.1"),
        "release commit subject should match"
    );
    let commit_message = head_commit.message().unwrap_or_default();
    assert!(
        commit_message.contains("### Changes"),
        "release commit body should render Changes heading: {commit_message}"
    );
    for legacy_heading in [
        "### Breaking Changes",
        "### Features",
        "### Fixes/Performance",
        "### Other",
    ] {
        assert!(
            !commit_message.contains(legacy_heading),
            "release commit body should not include {legacy_heading}: {commit_message}"
        );
    }

    let tag_ref = repo_handle.find_reference("refs/tags/v0.0.1")?;
    let tag_object = tag_ref.peel(ObjectType::Tag)?;
    assert_eq!(
        tag_object.kind(),
        Some(ObjectType::Tag),
        "release tag should be annotated"
    );

    let tagged_commit = tag_ref.peel_to_commit()?;
    assert_eq!(
        tagged_commit.id(),
        head_commit.id(),
        "release tag should point at the release commit"
    );

    Ok(())
}

#[test]
fn test_release_force_bump_overrides_auto_and_no_tag_skips_tag() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write("docs.txt", "notes\n")?;
    repo.git(&["add", "docs.txt"])?;
    repo.git(&["commit", "-m", "docs: update release notes"])?;

    let output = repo.vizier_output(&["release", "--yes", "--minor", "--no-tag"])?;
    assert!(
        output.status.success(),
        "vizier release --yes --minor --no-tag failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let repo_handle = repo.repo();
    let head_commit = repo_handle.head()?.peel_to_commit()?;
    assert_eq!(
        head_commit.summary(),
        Some("chore(release): v0.1.0"),
        "forced minor bump should produce v0.1.0"
    );
    assert!(
        repo_handle.find_reference("refs/tags/v0.1.0").is_err(),
        "--no-tag should skip tag creation"
    );

    Ok(())
}

#[test]
fn test_release_refuses_dirty_worktree() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write("dirty.txt", "pending\n")?;

    let output = repo.vizier_output(&["release", "--yes"])?;
    assert!(
        !output.status.success(),
        "release should fail on dirty worktree"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("working tree has uncommitted or untracked changes"),
        "expected dirty worktree error, got: {stderr}"
    );

    Ok(())
}

#[test]
fn test_release_refuses_detached_head() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write("fix.txt", "one\n")?;
    repo.git(&["add", "fix.txt"])?;
    repo.git(&["commit", "-m", "fix: seed release commit"])?;
    repo.git(&["checkout", "--detach", "HEAD"])?;

    let output = repo.vizier_output(&["release", "--yes"])?;
    assert!(
        !output.status.success(),
        "release should fail on detached HEAD"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("detached HEAD"),
        "expected detached HEAD guidance, got: {stderr}"
    );

    Ok(())
}

#[test]
fn test_release_noop_when_no_releasable_commits_without_forcing() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write("docs.txt", "notes\n")?;
    repo.git(&["add", "docs.txt"])?;
    repo.git(&["commit", "-m", "docs: update notes"])?;

    let commit_count_before = count_commits_from_head(&repo.repo())?;

    let output = repo.vizier_output(&["release"])?;
    assert!(
        output.status.success(),
        "release should exit successfully for no-op case: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No release created"),
        "expected no-op outcome: {stdout}"
    );
    assert!(
        stdout.contains("No releasable commits found"),
        "expected no-op reason: {stdout}"
    );

    let commit_count_after = count_commits_from_head(&repo.repo())?;
    assert_eq!(
        commit_count_before, commit_count_after,
        "no-op release should not create commits"
    );

    Ok(())
}

#[test]
fn test_release_incremental_from_existing_tag() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    repo.write("feature.txt", "one\n")?;
    repo.git(&["add", "feature.txt"])?;
    repo.git(&["commit", "-m", "feat: initial feature"])?;
    let first = repo.vizier_output(&["release", "--yes"])?;
    assert!(
        first.status.success(),
        "first release failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    repo.write("feature.txt", "two\n")?;
    repo.git(&["add", "feature.txt"])?;
    repo.git(&["commit", "-m", "fix: patch after first release"])?;
    let second = repo.vizier_output(&["release", "--yes"])?;
    assert!(
        second.status.success(),
        "second release failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    let repo_handle = repo.repo();
    let head_commit = repo_handle.head()?.peel_to_commit()?;
    assert_eq!(
        head_commit.summary(),
        Some("chore(release): v0.1.1"),
        "incremental release should bump from v0.1.0 to v0.1.1"
    );
    assert!(
        repo_handle.find_reference("refs/tags/v0.1.1").is_ok(),
        "incremental release should create the next tag"
    );

    Ok(())
}

#[test]
fn test_release_help_lists_flags() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let output = repo.vizier_output(&["help", "release", "--no-ansi"])?;
    assert!(
        output.status.success(),
        "vizier help release failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for expected in [
        "--dry-run",
        "--yes",
        "--major",
        "--minor",
        "--patch",
        "--max-commits",
        "--no-tag",
    ] {
        assert!(
            stdout.contains(expected),
            "release help should include {expected}: {stdout}"
        );
    }

    Ok(())
}
