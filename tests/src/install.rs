use crate::fixtures::*;

fn write_cargo_stub(dir: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join("cargo");
    fs::write(
        &path,
        "#!/bin/sh
set -eu

if [ -n \"${CARGO_INVOCATIONS_LOG:-}\" ]; then
  printf \"%s|%s\\n\" \"${CARGO_TARGET_DIR:-}\" \"$*\" >>\"${CARGO_INVOCATIONS_LOG}\"
fi

subcommand=\"$1\"
shift

case \"${subcommand}\" in
  generate-lockfile)
    : >Cargo.lock
    ;;
  build)
    target_dir=\"${CARGO_TARGET_DIR:-target}\"
    mkdir -p \"${target_dir}/release\"
    cat >\"${target_dir}/release/vizier\" <<'EOF'
#!/bin/sh
printf '%s\\n' 'vizier stub'
EOF
    chmod +x \"${target_dir}/release/vizier\"
    ;;
  *)
    printf '%s\\n' \"unexpected cargo subcommand: ${subcommand}\" 1>&2
    exit 1
    ;;
esac
",
    )?;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms)?;
    }
    Ok(path)
}
fn run_install_sh(
    root: &Path,
    args: &[&str],
    envs: &[(&str, &std::ffi::OsStr)],
) -> io::Result<Output> {
    let mut cmd = Command::new("sh");
    cmd.current_dir(root);
    cmd.env_remove("CARGO_TARGET_DIR");
    cmd.arg("install.sh");
    cmd.args(args);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.output()
}

fn seed_install_fixture_root(root: &Path) -> io::Result<()> {
    fs::copy(repo_root().join("install.sh"), root.join("install.sh"))?;
    copy_dir_recursive(
        &repo_root().join("examples/agents"),
        &root.join("examples/agents"),
    )?;
    copy_dir_recursive(&repo_root().join("docs/man"), &root.join("docs/man"))?;
    copy_dir_recursive(
        &repo_root().join(".vizier/workflows"),
        &root.join(".vizier/workflows"),
    )?;
    fs::create_dir_all(root.join(".vizier"))?;
    fs::copy(
        repo_root().join(".vizier/develop.hcl"),
        root.join(".vizier/develop.hcl"),
    )?;
    Ok(())
}

fn write_id_stub(dir: &Path, uid: u32) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join("id");
    fs::write(
        &path,
        format!(
            "#!/bin/sh
set -eu

if [ \"${{1:-}}\" = \"-u\" ]; then
  printf '%s\\n' '{uid}'
  exit 0
fi

printf '%s\\n' \"unsupported id invocation: $*\" 1>&2
exit 1
"
        ),
    )?;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms)?;
    }
    Ok(path)
}
#[cfg(unix)]
fn is_root_user() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim() == "0")
        .unwrap_or(false)
}
#[cfg(unix)]
fn assert_mode(path: &Path, expected: u32) -> TestResult {
    let mode = fs::metadata(path)?.permissions().mode() & 0o777;
    assert_eq!(mode, expected, "mode mismatch for {}", path.display());
    Ok(())
}
#[test]
fn test_install_sh_stages_and_uninstalls() -> TestResult {
    let tmp = TempDir::new()?;
    let root = tmp.path().join("src");
    fs::create_dir_all(&root)?;
    seed_install_fixture_root(&root)?;

    let bin_dir = tmp.path().join("bin");
    write_cargo_stub(&bin_dir)?;

    let cargo_log = tmp.path().join("cargo.log");
    let stage = tmp.path().join("stage");
    let cargo_target = tmp.path().join("cargo-target");
    fs::create_dir_all(&stage)?;

    let mut paths = vec![bin_dir.clone()];
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    let joined_path = env::join_paths(paths)?;

    for _ in 0..2 {
        let output = run_install_sh(
            &root,
            &[],
            &[
                ("PATH", joined_path.as_os_str()),
                ("CARGO_INVOCATIONS_LOG", cargo_log.as_os_str()),
                ("CARGO_TARGET_DIR", cargo_target.as_os_str()),
                ("DESTDIR", stage.as_os_str()),
                ("PREFIX", Path::new("/usr/local").as_os_str()),
                (
                    "WORKFLOWSDIR",
                    Path::new("/usr/local/share/vizier/workflows").as_os_str(),
                ),
            ],
        )?;
        assert!(
            output.status.success(),
            "install.sh failed: status={:?}\nstdout={}\nstderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let expected_exe = stage.join("usr/local/bin/vizier");
    let expected_man_pages = [
        "usr/local/share/man/man1/vizier.1",
        "usr/local/share/man/man1/vizier-jobs.1",
        "usr/local/share/man/man5/vizier-config.5",
        "usr/local/share/man/man7/vizier-workflow.7",
        "usr/local/share/man/man7/vizier-workflow-template.7",
    ];
    let expected_manifest = stage.join("usr/local/share/vizier/install-manifest.txt");
    let expected_agents = [
        "usr/local/share/vizier/agents/codex/agent.sh",
        "usr/local/share/vizier/agents/codex/filter.sh",
        "usr/local/share/vizier/agents/gemini/agent.sh",
        "usr/local/share/vizier/agents/gemini/filter.sh",
        "usr/local/share/vizier/agents/claude/agent.sh",
        "usr/local/share/vizier/agents/claude/filter.sh",
    ];
    let expected_workflows = [
        "usr/local/share/vizier/workflows/draft.hcl",
        "usr/local/share/vizier/workflows/approve.hcl",
        "usr/local/share/vizier/workflows/merge.hcl",
        "usr/local/share/vizier/workflows/commit.hcl",
        "usr/local/share/vizier/develop.hcl",
    ];

    assert!(expected_exe.is_file(), "missing {}", expected_exe.display());
    for rel in expected_man_pages {
        let path = stage.join(rel);
        assert!(path.is_file(), "missing {}", path.display());
    }
    for rel in expected_workflows {
        let path = stage.join(rel);
        assert!(path.is_file(), "missing {}", path.display());
    }
    assert!(
        expected_manifest.is_file(),
        "missing {}",
        expected_manifest.display()
    );

    #[cfg(unix)]
    {
        assert_mode(&expected_exe, 0o755)?;
        for rel in expected_agents {
            let path = stage.join(rel);
            assert!(path.is_file(), "missing {}", path.display());
            assert_mode(&path, 0o755)?;
        }
        for rel in expected_man_pages {
            let path = stage.join(rel);
            assert_mode(&path, 0o644)?;
        }
        for rel in expected_workflows {
            let path = stage.join(rel);
            assert_mode(&path, 0o644)?;
        }
    }

    let manifest = fs::read_to_string(&expected_manifest)?;
    let manifest_lines: HashSet<&str> = manifest.lines().collect();
    assert!(
        manifest_lines.contains("/usr/local/bin/vizier"),
        "manifest missing vizier binary: {manifest}"
    );
    for rel in expected_man_pages {
        let manifest_path = format!("/{rel}");
        assert!(
            manifest_lines.contains(manifest_path.as_str()),
            "manifest missing man page entry {manifest_path}: {manifest}"
        );
    }
    assert!(
        manifest_lines.contains("/usr/local/share/vizier/agents/codex/agent.sh"),
        "manifest missing codex shim: {manifest}"
    );
    for rel in expected_workflows {
        let manifest_path = format!("/{rel}");
        assert!(
            manifest_lines.contains(manifest_path.as_str()),
            "manifest missing workflow template entry {manifest_path}: {manifest}"
        );
    }

    let uninstall = run_install_sh(
        &root,
        &["--uninstall"],
        &[
            ("PATH", joined_path.as_os_str()),
            ("DESTDIR", stage.as_os_str()),
            ("PREFIX", Path::new("/usr/local").as_os_str()),
            (
                "WORKFLOWSDIR",
                Path::new("/usr/local/share/vizier/workflows").as_os_str(),
            ),
        ],
    )?;
    assert!(
        uninstall.status.success(),
        "install.sh --uninstall failed: status={:?}\nstdout={}\nstderr={}",
        uninstall.status,
        String::from_utf8_lossy(&uninstall.stdout),
        String::from_utf8_lossy(&uninstall.stderr)
    );

    assert!(
        !expected_exe.exists(),
        "expected binary removed: {}",
        expected_exe.display()
    );
    for rel in expected_agents {
        let path = stage.join(rel);
        assert!(!path.exists(), "expected shim removed: {}", path.display());
    }
    for rel in expected_man_pages {
        let path = stage.join(rel);
        assert!(
            !path.exists(),
            "expected man page removed: {}",
            path.display()
        );
    }
    for rel in expected_workflows {
        let path = stage.join(rel);
        assert!(
            !path.exists(),
            "expected workflow template removed: {}",
            path.display()
        );
    }
    assert!(
        !expected_manifest.exists(),
        "expected manifest removed: {}",
        expected_manifest.display()
    );

    let cargo_invocations = fs::read_to_string(&cargo_log)?;
    assert!(
        cargo_invocations.contains("generate-lockfile"),
        "expected cargo generate-lockfile invocation: {cargo_invocations}"
    );
    assert!(
        cargo_invocations.contains("build --locked --release -p vizier"),
        "expected cargo build invocation: {cargo_invocations}"
    );
    Ok(())
}
#[test]
fn test_install_sh_dry_run_writes_nothing() -> TestResult {
    let tmp = TempDir::new()?;
    let root = tmp.path().join("src");
    fs::create_dir_all(&root)?;
    seed_install_fixture_root(&root)?;

    let stage = tmp.path().join("stage");
    let cargo_target = tmp.path().join("cargo-target");
    fs::create_dir_all(&stage)?;

    let output = run_install_sh(
        &root,
        &["--dry-run"],
        &[
            ("DESTDIR", stage.as_os_str()),
            ("CARGO_TARGET_DIR", cargo_target.as_os_str()),
            ("PREFIX", Path::new("/usr/local").as_os_str()),
            (
                "WORKFLOWSDIR",
                Path::new("/usr/local/share/vizier/workflows").as_os_str(),
            ),
        ],
    )?;
    assert!(
        output.status.success(),
        "install.sh --dry-run failed: status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for rel in [
        "/usr/local/share/man/man1/vizier.1",
        "/usr/local/share/man/man1/vizier-jobs.1",
        "/usr/local/share/man/man5/vizier-config.5",
        "/usr/local/share/man/man7/vizier-workflow.7",
        "/usr/local/share/man/man7/vizier-workflow-template.7",
        "/usr/local/share/vizier/workflows/draft.hcl",
        "/usr/local/share/vizier/workflows/approve.hcl",
        "/usr/local/share/vizier/workflows/merge.hcl",
        "/usr/local/share/vizier/workflows/commit.hcl",
        "/usr/local/share/vizier/develop.hcl",
    ] {
        assert!(
            stdout.contains(rel),
            "dry-run output should include planned install target {rel}: {stdout}"
        );
    }

    let stage_empty = fs::read_dir(&stage)?.next().is_none();
    assert!(
        stage_empty,
        "expected dry-run to write nothing under DESTDIR"
    );
    assert!(
        !cargo_target.exists(),
        "expected dry-run to avoid building into CARGO_TARGET_DIR"
    );
    Ok(())
}
#[test]
#[cfg(unix)]
fn test_install_sh_requires_writable_prefix() -> TestResult {
    if is_root_user() {
        return Ok(());
    }

    let tmp = TempDir::new()?;
    let root = tmp.path().join("src");
    fs::create_dir_all(&root)?;
    seed_install_fixture_root(&root)?;

    let prefix = tmp.path().join("prefix");
    fs::create_dir_all(&prefix)?;
    let mut perms = fs::metadata(&prefix)?.permissions();
    perms.set_mode(0o555);
    fs::set_permissions(&prefix, perms)?;

    let workflows_dir = prefix.join("share/vizier/workflows");
    let output = run_install_sh(
        &root,
        &[],
        &[
            ("PREFIX", prefix.as_os_str()),
            ("WORKFLOWSDIR", workflows_dir.as_os_str()),
        ],
    )?;
    assert!(
        !output.status.success(),
        "expected install.sh to fail for unwritable prefix: status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("install destination is not writable"),
        "expected permission hint in stderr: {stderr}"
    );
    assert!(
        stderr.contains("sudo ./install.sh"),
        "expected sudo suggestion in stderr: {stderr}"
    );
    Ok(())
}

#[test]
#[cfg(unix)]
fn test_install_sh_root_defaults_to_temp_cargo_target_dir() -> TestResult {
    let tmp = TempDir::new()?;
    let root = tmp.path().join("src");
    fs::create_dir_all(&root)?;
    seed_install_fixture_root(&root)?;

    let bin_dir = tmp.path().join("bin");
    write_cargo_stub(&bin_dir)?;
    write_id_stub(&bin_dir, 0)?;

    let cargo_log = tmp.path().join("cargo.log");
    let stage = tmp.path().join("stage");
    fs::create_dir_all(&stage)?;

    let mut paths = vec![bin_dir.clone()];
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    let joined_path = env::join_paths(paths)?;

    let output = run_install_sh(
        &root,
        &[],
        &[
            ("PATH", joined_path.as_os_str()),
            ("CARGO_INVOCATIONS_LOG", cargo_log.as_os_str()),
            ("DESTDIR", stage.as_os_str()),
            ("PREFIX", Path::new("/usr/local").as_os_str()),
            (
                "WORKFLOWSDIR",
                Path::new("/usr/local/share/vizier/workflows").as_os_str(),
            ),
        ],
    )?;
    assert!(
        output.status.success(),
        "install.sh failed for root-like install: status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let cargo_invocations = fs::read_to_string(&cargo_log)?;
    let mut targets = HashSet::new();
    for line in cargo_invocations.lines() {
        let Some((target, _command)) = line.split_once('|') else {
            panic!("missing target separator in cargo log entry: {line:?}");
        };
        assert!(
            !target.is_empty(),
            "expected CARGO_TARGET_DIR for root-like install: {line:?}"
        );
        assert_ne!(
            target, "target",
            "root-like install should not build into default ./target: {line:?}"
        );
        targets.insert(target.to_string());
    }

    assert_eq!(
        targets.len(),
        1,
        "expected all cargo invocations to share one temp target dir: {cargo_invocations}"
    );
    let target_dir = targets.into_iter().next().expect("temp target dir");
    assert!(
        !Path::new(&target_dir).exists(),
        "expected temp target dir to be cleaned up: {target_dir}"
    );
    assert!(
        !root.join("target").exists(),
        "root-like install should not create ./target"
    );
    Ok(())
}

#[test]
fn test_install_sh_preserves_existing_workflow_templates() -> TestResult {
    let tmp = TempDir::new()?;
    let root = tmp.path().join("src");
    fs::create_dir_all(&root)?;
    seed_install_fixture_root(&root)?;

    let bin_dir = tmp.path().join("bin");
    write_cargo_stub(&bin_dir)?;

    let stage = tmp.path().join("stage");
    let cargo_target = tmp.path().join("cargo-target");
    fs::create_dir_all(&stage)?;
    let workflows_dir = stage.join("usr/local/share/vizier/workflows");
    fs::create_dir_all(&workflows_dir)?;
    fs::write(workflows_dir.join("draft.hcl"), "custom draft workflow\n")?;

    let mut paths = vec![bin_dir.clone()];
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    let joined_path = env::join_paths(paths)?;

    let output = run_install_sh(
        &root,
        &[],
        &[
            ("PATH", joined_path.as_os_str()),
            ("CARGO_TARGET_DIR", cargo_target.as_os_str()),
            ("DESTDIR", stage.as_os_str()),
            ("PREFIX", Path::new("/usr/local").as_os_str()),
            (
                "WORKFLOWSDIR",
                Path::new("/usr/local/share/vizier/workflows").as_os_str(),
            ),
        ],
    )?;
    assert!(
        output.status.success(),
        "install.sh failed while preserving workflow template: status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        fs::read_to_string(workflows_dir.join("draft.hcl"))?,
        "custom draft workflow\n",
        "existing workflow should be preserved"
    );
    assert!(workflows_dir.join("approve.hcl").is_file());
    assert!(workflows_dir.join("merge.hcl").is_file());
    assert!(workflows_dir.join("commit.hcl").is_file());
    assert!(stage.join("usr/local/share/vizier/develop.hcl").is_file());

    let manifest = fs::read_to_string(stage.join("usr/local/share/vizier/install-manifest.txt"))?;
    let manifest_lines: HashSet<&str> = manifest.lines().collect();
    assert!(
        !manifest_lines.contains("/usr/local/share/vizier/workflows/draft.hcl"),
        "preserved existing workflow should not be tracked for uninstall: {manifest}"
    );
    assert!(
        manifest_lines.contains("/usr/local/share/vizier/workflows/approve.hcl"),
        "installed approve workflow should be tracked: {manifest}"
    );
    assert!(
        manifest_lines.contains("/usr/local/share/vizier/workflows/merge.hcl"),
        "installed merge workflow should be tracked: {manifest}"
    );
    assert!(
        manifest_lines.contains("/usr/local/share/vizier/workflows/commit.hcl"),
        "installed commit workflow should be tracked: {manifest}"
    );
    assert!(
        manifest_lines.contains("/usr/local/share/vizier/develop.hcl"),
        "installed develop workflow should be tracked: {manifest}"
    );

    let uninstall = run_install_sh(
        &root,
        &["--uninstall"],
        &[
            ("PATH", joined_path.as_os_str()),
            ("DESTDIR", stage.as_os_str()),
            ("PREFIX", Path::new("/usr/local").as_os_str()),
            (
                "WORKFLOWSDIR",
                Path::new("/usr/local/share/vizier/workflows").as_os_str(),
            ),
        ],
    )?;
    assert!(
        uninstall.status.success(),
        "install.sh --uninstall failed: status={:?}\nstdout={}\nstderr={}",
        uninstall.status,
        String::from_utf8_lossy(&uninstall.stdout),
        String::from_utf8_lossy(&uninstall.stderr)
    );

    assert_eq!(
        fs::read_to_string(workflows_dir.join("draft.hcl"))?,
        "custom draft workflow\n",
        "preserved workflow should remain after uninstall"
    );
    assert!(
        !workflows_dir.join("approve.hcl").exists(),
        "installed approve workflow should be removed by uninstall"
    );
    assert!(
        !workflows_dir.join("merge.hcl").exists(),
        "installed merge workflow should be removed by uninstall"
    );
    assert!(
        !workflows_dir.join("commit.hcl").exists(),
        "installed commit workflow should be removed by uninstall"
    );
    assert!(
        !stage.join("usr/local/share/vizier/develop.hcl").exists(),
        "installed develop workflow should be removed by uninstall"
    );

    Ok(())
}
