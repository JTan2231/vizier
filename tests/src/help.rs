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
        stdout.contains("Commands:") && stdout.contains("test-display"),
        "full help should include the command inventory (including test-display): {stdout}"
    );
    assert!(
        !stdout.contains("\n  ask ") && !stdout.contains("\n  init-snapshot "),
        "removed commands should not appear in full help inventory: {stdout}"
    );
    assert!(
        stdout.contains("--no-ansi") && stdout.contains("--pager"),
        "full help should include global options: {stdout}"
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
fn test_manpage_is_in_sync_with_help_all() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let full_help = repo.vizier_output(&["help", "--all", "--no-ansi"])?;
    assert!(
        full_help.status.success(),
        "`vizier help --all` failed: {}",
        String::from_utf8_lossy(&full_help.stderr)
    );
    let help_stdout = String::from_utf8_lossy(&full_help.stdout);

    let man_path = repo_root().join("docs/man/vizier.1");
    assert!(
        man_path.exists(),
        "expected manpage at {}",
        man_path.display()
    );

    let man_contents = fs::read_to_string(&man_path)?;

    let mut expected = String::new();
    expected.push_str(".TH VIZIER 1 \"UNRELEASED\" \"Vizier\" \"User Commands\"\n");
    expected.push_str(".SH NAME\n");
    expected.push_str("vizier \\- A CLI for LLM project management\n");
    expected.push_str(".SH REFERENCE\n");
    expected.push_str(".nf\n");
    let mut normalized_help = help_stdout.to_string();
    if !normalized_help.ends_with('\n') {
        normalized_help.push('\n');
    }
    for line in normalized_help.split_inclusive('\n') {
        if line.starts_with('.') || line.starts_with('\'') {
            expected.push_str("\\&");
        }
        expected.push_str(line);
    }
    expected.push_str(".fi\n");

    assert_eq!(
        man_contents, expected,
        "docs/man/vizier.1 should be generated from `vizier help --all --no-ansi`"
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
    cmd.args(["--pager", "--help"]);
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "help with --pager should exit 0: {}",
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
