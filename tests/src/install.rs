use crate::fixtures::*;

fn write_cargo_stub(dir: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join("cargo");
    fs::write(
        &path,
        "#!/bin/sh
set -eu

if [ -n \"${CARGO_INVOCATIONS_LOG:-}\" ]; then
  printf \"%s\\n\" \"$*\" >>\"${CARGO_INVOCATIONS_LOG}\"
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
    cmd.arg("install.sh");
    cmd.args(args);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.output()
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

    fs::copy(repo_root().join("install.sh"), root.join("install.sh"))?;
    copy_dir_recursive(
        &repo_root().join("examples/agents"),
        &root.join("examples/agents"),
    )?;
    copy_dir_recursive(&repo_root().join("docs/man"), &root.join("docs/man"))?;

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
    let expected_man = stage.join("usr/local/share/man/man1/vizier.1");
    let expected_manifest = stage.join("usr/local/share/vizier/install-manifest.txt");
    let expected_agents = [
        "usr/local/share/vizier/agents/codex/agent.sh",
        "usr/local/share/vizier/agents/codex/filter.sh",
        "usr/local/share/vizier/agents/gemini/agent.sh",
        "usr/local/share/vizier/agents/gemini/filter.sh",
        "usr/local/share/vizier/agents/claude/agent.sh",
        "usr/local/share/vizier/agents/claude/filter.sh",
    ];

    assert!(expected_exe.is_file(), "missing {}", expected_exe.display());
    assert!(expected_man.is_file(), "missing {}", expected_man.display());
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
        assert_mode(&expected_man, 0o644)?;
    }

    let manifest = fs::read_to_string(&expected_manifest)?;
    let manifest_lines: HashSet<&str> = manifest.lines().collect();
    assert!(
        manifest_lines.contains("/usr/local/bin/vizier"),
        "manifest missing vizier binary: {manifest}"
    );
    assert!(
        manifest_lines.contains("/usr/local/share/man/man1/vizier.1"),
        "manifest missing man page: {manifest}"
    );
    assert!(
        manifest_lines.contains("/usr/local/share/vizier/agents/codex/agent.sh"),
        "manifest missing codex shim: {manifest}"
    );

    let uninstall = run_install_sh(
        &root,
        &["--uninstall"],
        &[
            ("PATH", joined_path.as_os_str()),
            ("DESTDIR", stage.as_os_str()),
            ("PREFIX", Path::new("/usr/local").as_os_str()),
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
    assert!(
        !expected_man.exists(),
        "expected man page removed: {}",
        expected_man.display()
    );
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

    fs::copy(repo_root().join("install.sh"), root.join("install.sh"))?;
    copy_dir_recursive(
        &repo_root().join("examples/agents"),
        &root.join("examples/agents"),
    )?;
    copy_dir_recursive(&repo_root().join("docs/man"), &root.join("docs/man"))?;

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
        ],
    )?;
    assert!(
        output.status.success(),
        "install.sh --dry-run failed: status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

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

    fs::copy(repo_root().join("install.sh"), root.join("install.sh"))?;
    copy_dir_recursive(
        &repo_root().join("examples/agents"),
        &root.join("examples/agents"),
    )?;
    copy_dir_recursive(&repo_root().join("docs/man"), &root.join("docs/man"))?;

    let prefix = tmp.path().join("prefix");
    fs::create_dir_all(&prefix)?;
    let mut perms = fs::metadata(&prefix)?.permissions();
    perms.set_mode(0o555);
    fs::set_permissions(&prefix, perms)?;

    let output = run_install_sh(&root, &[], &[("PREFIX", prefix.as_os_str())])?;
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
