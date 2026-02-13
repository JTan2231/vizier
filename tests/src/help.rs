use crate::fixtures::*;

#[test]
fn test_help_respects_no_ansi_and_quiet() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let mut cmd = repo.vizier_cmd();
    cmd.args(["--help", "-q", "--no-ansi"]);
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "help should exit 0 even with quiet/no-ansi: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.trim().is_empty(),
        "quiet mode should not suppress help output: {stdout}"
    );
    assert!(
        !stdout.contains('\u{1b}'),
        "help output should omit ANSI when --no-ansi is set: {stdout}"
    );
    Ok(())
}
#[test]
fn test_help_landing_page_is_curated() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let root = repo.vizier_output(&["--help", "--no-ansi"])?;
    assert!(
        root.status.success(),
        "root --help failed: {}",
        String::from_utf8_lossy(&root.stderr)
    );
    let stdout = String::from_utf8_lossy(&root.stdout);
    assert!(
        stdout.contains("Workflow:") && stdout.contains("More help:"),
        "expected curated help sections, got: {stdout}"
    );
    assert!(
        stdout.contains("vizier build")
            && stdout.contains("vizier draft")
            && stdout.contains("vizier merge"),
        "expected curated workflow commands (including build), got: {stdout}"
    );
    assert!(
        !stdout.contains("Commands:") && !stdout.contains("test-display"),
        "curated help should not include the full command inventory: {stdout}"
    );
    assert!(
        stdout.lines().count() <= 24,
        "curated help should fit in ~1 screen (line count <= 24), got {} lines:\n{stdout}",
        stdout.lines().count()
    );

    let help_cmd = repo.vizier_output(&["help", "--no-ansi"])?;
    assert!(
        help_cmd.status.success(),
        "`vizier help` failed: {}",
        String::from_utf8_lossy(&help_cmd.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&help_cmd.stdout),
        stdout,
        "`vizier help` should match `vizier --help`"
    );
    Ok(())
}
#[test]
fn test_help_all_prints_full_reference() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let output = repo.vizier_output(&["help", "--all", "--no-ansi"])?;
    assert!(
        output.status.success(),
        "`vizier help --all` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Commands:")
            && stdout.contains("test-display")
            && stdout.contains("\n  init "),
        "full help should include the command inventory (including test-display): {stdout}"
    );
    assert!(
        !stdout.contains("\n  ask ") && !stdout.contains("\n  init-snapshot "),
        "removed commands should not appear in full help inventory: {stdout}"
    );
    assert!(
        stdout.contains("--no-ansi") && stdout.contains("--follow"),
        "full help should include global options: {stdout}"
    );
    assert!(
        !stdout.contains("--pager"),
        "full help should not advertise removed --pager: {stdout}"
    );
    Ok(())
}

#[test]
fn test_removed_ask_command_shows_migration_error() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let output = repo.vizier_output(&["ask", "legacy command should fail"])?;
    assert!(
        !output.status.success(),
        "removed `ask` command should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`ask` has been removed")
            && stderr.contains("save")
            && stderr.contains("draft")
            && stderr.contains("merge"),
        "expected migration guidance for removed ask command, got: {stderr}"
    );
    Ok(())
}
#[test]
fn test_help_command_matches_subcommand_help() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let help_merge = repo.vizier_output(&["help", "merge", "--no-ansi"])?;
    assert!(
        help_merge.status.success(),
        "`vizier help merge` failed: {}",
        String::from_utf8_lossy(&help_merge.stderr)
    );

    let merge_help = repo.vizier_output(&["merge", "--help", "--no-ansi"])?;
    assert!(
        merge_help.status.success(),
        "`vizier merge --help` failed: {}",
        String::from_utf8_lossy(&merge_help.stderr)
    );

    assert_eq!(
        help_merge.stdout, merge_help.stdout,
        "`vizier help <command>` should match `<command> --help` output"
    );
    Ok(())
}

#[test]
fn test_help_build_subcommand_renders_without_panic() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let output = repo.vizier_output(&["help", "build", "--no-ansi"])?;
    assert!(
        output.status.success(),
        "`vizier help build` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Usage: vizier build"),
        "unexpected help output for build subcommand: {stdout}"
    );
    Ok(())
}
#[test]
fn test_manpage_layout_uses_sectioned_real_files() -> TestResult {
    let repo_root = repo_root();
    let required = [
        "docs/man/man1/vizier.1",
        "docs/man/man1/vizier-jobs.1",
        "docs/man/man1/vizier-build.1",
        "docs/man/man5/vizier-config.5",
        "docs/man/man7/vizier-workflow.7",
    ];

    for rel in required {
        let path = repo_root.join(rel);
        assert!(path.exists(), "missing man page {}", path.display());
        let metadata = fs::symlink_metadata(&path)?;
        assert!(
            metadata.file_type().is_file(),
            "expected regular file for {}, got symlink or non-file",
            path.display()
        );
    }

    let legacy = repo_root.join("docs/man/vizier.1");
    assert!(
        !legacy.exists(),
        "legacy single-page path should not exist: {}",
        legacy.display()
    );

    let build_page = fs::read_to_string(repo_root.join("docs/man/man1/vizier-build.1"))?;
    assert!(
        !build_page.contains("__materialize") && !build_page.contains("__template-node"),
        "hidden build internals should not appear in generated build man page: {build_page}"
    );
    let root_page = fs::read_to_string(repo_root.join("docs/man/man1/vizier.1"))?;
    assert!(
        !root_page.contains("__complete") && !root_page.contains("__workflow-node"),
        "hidden root internals should not appear in generated root man page: {root_page}"
    );
    Ok(())
}

#[test]
fn test_manpage_generation_check_passes() -> TestResult {
    let output = Command::new("cargo")
        .current_dir(repo_root())
        .args(["run", "-p", "vizier", "--bin", "gen-man", "--", "--check"])
        .output()?;
    assert!(
        output.status.success(),
        "man page check failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
#[test]
fn test_help_does_not_invoke_pager_when_not_a_tty() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let temp_dir = TempDir::new()?;
    let pager_log = temp_dir.path().join("pager-hit.log");
    let pager_cmd = format!("cat > {}", pager_log.display());

    let mut cmd = repo.vizier_cmd();
    cmd.env("VIZIER_PAGER", &pager_cmd);
    cmd.args(["--help"]);
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "help should exit 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        !pager_log.exists(),
        "pager command should not run when stdout is not a TTY"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.trim().is_empty(),
        "help should still print when pager is suppressed: {stdout}"
    );
    Ok(())
}

#[test]
fn test_removed_pager_flag_shows_migration_guidance() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let output = repo.vizier_output_no_follow(&["--pager", "help"])?;
    assert!(!output.status.success(), "removed --pager should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`--pager` was removed"),
        "missing removed --pager guidance:\n{stderr}"
    );
    Ok(())
}
