use crate::fixtures::*;

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
        stdout.contains("Core commands:") && stdout.contains("More help:"),
        "expected curated help sections, got: {stdout}"
    );
    for required in [
        "vizier init",
        "vizier list",
        "vizier jobs",
        "vizier run",
        "vizier audit",
        "vizier release",
    ] {
        assert!(
            stdout.contains(required),
            "missing curated entry {required}: {stdout}"
        );
    }
    for removed in [
        "vizier save",
        "vizier draft",
        "vizier approve",
        "vizier review",
        "vizier merge",
        "vizier build",
        "vizier patch",
        "vizier plan",
        "test-display",
    ] {
        assert!(
            !stdout.contains(removed),
            "curated help should not mention removed command {removed}: {stdout}"
        );
    }
    Ok(())
}

#[test]
fn test_help_all_prints_reduced_reference() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let output = repo.vizier_output(&["help", "--all", "--no-ansi"])?;
    assert!(
        output.status.success(),
        "`vizier help --all` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for command in [
        "\n  help ",
        "\n  init ",
        "\n  list ",
        "\n  cd ",
        "\n  clean ",
        "\n  jobs ",
        "\n  run ",
        "\n  audit ",
        "\n  completions ",
        "\n  release ",
    ] {
        assert!(
            stdout.contains(command),
            "missing command {command}: {stdout}"
        );
    }
    for removed in [
        "\n  save ",
        "\n  draft ",
        "\n  approve ",
        "\n  review ",
        "\n  merge ",
        "\n  build ",
        "\n  patch ",
        "\n  plan ",
        "\n  test-display ",
        "\n  __workflow-node ",
    ] {
        assert!(
            !stdout.contains(removed),
            "removed command should not appear in help inventory: {removed}\n{stdout}"
        );
    }

    for removed_flag in [
        "--agent",
        "--push",
        "--no-commit",
        "--follow",
        "--background-job-id",
    ] {
        assert!(
            !stdout.contains(removed_flag),
            "removed global flag should not appear in help output: {stdout}"
        );
    }

    Ok(())
}

#[test]
fn test_removed_commands_fail_as_unknown_subcommands() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    for removed in [
        "save",
        "draft",
        "approve",
        "review",
        "merge",
        "test-display",
        "plan",
        "build",
        "patch",
    ] {
        let output = repo.vizier_output(&[removed])?;
        assert!(
            !output.status.success(),
            "removed command `{removed}` should fail"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("unrecognized subcommand"),
            "expected generic Clap unknown-subcommand error for {removed}: {stderr}"
        );
        assert!(
            !stderr.contains("was removed")
                && !stderr.contains("use supported workflow commands")
                && !stderr.contains("global `--json` was removed"),
            "should not emit custom migration guidance for {removed}: {stderr}"
        );
    }

    Ok(())
}

#[test]
fn test_removed_pager_flag_fails_as_unknown_argument_without_custom_guidance() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    for args in [["help", "--pager"], ["list", "--pager"]] {
        let output = repo.vizier_output(&args)?;
        assert!(
            !output.status.success(),
            "`vizier {}` should fail",
            args.join(" ")
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("unexpected argument '--pager'"),
            "expected generic Clap unknown-argument error for {:?}: {stderr}",
            args
        );
        assert!(
            !stderr.contains("global `--pager` was removed")
                && !stderr.contains("use supported workflow commands")
                && !stderr.contains("was removed"),
            "should not emit custom migration guidance for {:?}: {stderr}",
            args
        );
    }

    Ok(())
}

#[test]
fn test_help_command_matches_subcommand_help() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let help_jobs = repo.vizier_output(&["help", "jobs", "--no-ansi"])?;
    assert!(
        help_jobs.status.success(),
        "`vizier help jobs` failed: {}",
        String::from_utf8_lossy(&help_jobs.stderr)
    );

    let jobs_help = repo.vizier_output(&["jobs", "--help", "--no-ansi"])?;
    assert!(
        jobs_help.status.success(),
        "`vizier jobs --help` failed: {}",
        String::from_utf8_lossy(&jobs_help.stderr)
    );

    assert_eq!(
        help_jobs.stdout, jobs_help.stdout,
        "`vizier help <command>` should match `<command> --help` output"
    );

    let help_run = repo.vizier_output(&["help", "run", "--no-ansi"])?;
    assert!(
        help_run.status.success(),
        "`vizier help run` failed: {}",
        String::from_utf8_lossy(&help_run.stderr)
    );

    let run_help = repo.vizier_output(&["run", "--help", "--no-ansi"])?;
    assert!(
        run_help.status.success(),
        "`vizier run --help` failed: {}",
        String::from_utf8_lossy(&run_help.stderr)
    );

    assert_eq!(
        help_run.stdout, run_help.stdout,
        "`vizier help run` should match `vizier run --help` output"
    );

    let help_audit = repo.vizier_output(&["help", "audit", "--no-ansi"])?;
    assert!(
        help_audit.status.success(),
        "`vizier help audit` failed: {}",
        String::from_utf8_lossy(&help_audit.stderr)
    );

    let audit_help = repo.vizier_output(&["audit", "--help", "--no-ansi"])?;
    assert!(
        audit_help.status.success(),
        "`vizier audit --help` failed: {}",
        String::from_utf8_lossy(&audit_help.stderr)
    );

    assert_eq!(
        help_audit.stdout, audit_help.stdout,
        "`vizier help audit` should match `vizier audit --help` output"
    );
    Ok(())
}

#[test]
fn test_run_workflow_help_uses_resolved_alias_context() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;

    let output = repo.vizier_output(&["run", "draft", "--help", "--no-ansi"])?;
    assert!(
        output.status.success(),
        "`vizier run draft --help` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Workflow: draft"),
        "expected workflow alias label in flow help: {stdout}"
    );
    assert!(
        stdout.contains("Source: file:.vizier/workflows/draft.hcl"),
        "expected resolved workflow source in flow help: {stdout}"
    );
    assert!(
        stdout.contains("Usage:")
            && stdout.contains("Inputs:")
            && stdout.contains("Examples:")
            && stdout.contains("Run options:"),
        "missing workflow help sections: {stdout}"
    );
    assert!(
        stdout.contains("--file <file> -> spec_file") && stdout.contains("--name <name> -> slug"),
        "expected alias mappings in flow help: {stdout}"
    );
    assert!(
        !stdout.contains("Usage: vizier run [OPTIONS] <FLOW> [INPUT]..."),
        "flow help should not fall back to generic run help: {stdout}"
    );

    Ok(())
}

#[test]
fn test_run_workflow_help_file_selector_without_cli_shows_set_guidance() -> TestResult {
    let repo = IntegrationRepo::new_serial()?;
    clean_workdir(&repo)?;
    write_single_run_template(&repo, ".vizier/workflows/single.toml", "true")?;

    let output = repo.vizier_output(&[
        "run",
        "file:.vizier/workflows/single.toml",
        "--help",
        "--no-ansi",
    ])?;
    assert!(
        output.status.success(),
        "`vizier run file:.vizier/workflows/single.toml --help` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Workflow: file:.vizier/workflows/single.toml")
            && stdout.contains("Source: file:.vizier/workflows/single.toml"),
        "expected file-based workflow identity in help: {stdout}"
    );
    assert!(
        stdout.contains("vizier run file:.vizier/workflows/single.toml [--set <KEY=VALUE>]...")
            && stdout
                .contains("No [cli] aliases are defined; pass parameters with --set key=value."),
        "expected no-cli fallback guidance in workflow help: {stdout}"
    );

    Ok(())
}

#[test]
fn test_manpage_layout_uses_sectioned_real_files() -> TestResult {
    let repo_root = repo_root();
    let required = [
        "docs/man/man1/vizier.1",
        "docs/man/man1/vizier-jobs.1",
        "docs/man/man5/vizier-config.5",
        "docs/man/man7/vizier-workflow.7",
        "docs/man/man7/vizier-workflow-template.7",
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

    let removed = repo_root.join("docs/man/man1/vizier-build.1");
    assert!(
        !removed.exists(),
        "removed build man page should not exist: {}",
        removed.display()
    );

    let root_page = fs::read_to_string(repo_root.join("docs/man/man1/vizier.1"))?;
    for removed in [
        "\n  save ",
        "\n  draft ",
        "\n  approve ",
        "\n  review ",
        "\n  merge ",
        "\n  build ",
        "\n  patch ",
        "\n  test-display ",
        "\n  plan ",
    ] {
        assert!(
            !root_page.contains(removed),
            "removed command marker should not appear in generated root man page: {removed}\n{root_page}"
        );
    }
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
