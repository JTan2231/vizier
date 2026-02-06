use crate::fixtures::*;

fn write_cargo_recorder(dir: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join("cargo");
    fs::write(
        &path,
        "#!/bin/sh
set -eu
printf '%s|%s\\n' \"${CARGO_TARGET_DIR:-}\" \"$*\" >>\"${CARGO_LOG:?}\"
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

fn copy_cicd_script(root: &Path) -> io::Result<PathBuf> {
    let script = root.join("cicd.sh");
    fs::copy(repo_root().join("cicd.sh"), &script)?;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&script)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms)?;
    }
    Ok(script)
}

fn prepare_path(bin_dir: &Path) -> io::Result<std::ffi::OsString> {
    let mut paths = vec![bin_dir.to_path_buf()];
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    env::join_paths(paths).map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))
}

fn read_commands(log: &Path) -> io::Result<Vec<String>> {
    fs::read_to_string(log).map(|text| text.lines().map(|line| line.to_string()).collect())
}

#[test]
fn test_cicd_defaults_cargo_target_dir_when_unset() -> TestResult {
    let tmp = TempDir::new()?;
    let root = tmp.path().join("repo");
    fs::create_dir_all(&root)?;
    copy_cicd_script(&root)?;

    let bin_dir = tmp.path().join("bin");
    write_cargo_recorder(&bin_dir)?;
    let path = prepare_path(&bin_dir)?;
    let log = tmp.path().join("cargo.log");

    let output = Command::new("./cicd.sh")
        .current_dir(&root)
        .env("PATH", path)
        .env("CARGO_LOG", &log)
        .env_remove("CARGO_TARGET_DIR")
        .output()?;
    assert!(
        output.status.success(),
        "cicd.sh failed: status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let expected_target = root.join(".vizier/tmp/cargo-target");
    assert!(
        expected_target.is_dir(),
        "expected default CARGO_TARGET_DIR to exist: {}",
        expected_target.display()
    );
    let expected_target = fs::canonicalize(expected_target)?;

    let entries = read_commands(&log)?;
    assert_eq!(
        entries.len(),
        3,
        "expected three cargo invocations; got {entries:?}"
    );

    for entry in &entries {
        let Some((target, _)) = entry.split_once('|') else {
            panic!("missing target separator in log entry: {entry:?}");
        };
        let recorded_target = fs::canonicalize(target)?;
        assert_eq!(
            recorded_target, expected_target,
            "unexpected CARGO_TARGET_DIR in log entry: {entry:?}"
        );
    }

    let commands: Vec<&str> = entries
        .iter()
        .filter_map(|line| line.split_once('|').map(|(_, cmd)| cmd))
        .collect();
    assert_eq!(
        commands,
        vec![
            "fmt",
            "clippy --all --all-targets -- -D warnings",
            "test --all --all-targets",
        ]
    );
    Ok(())
}

#[test]
fn test_cicd_respects_explicit_cargo_target_dir() -> TestResult {
    let tmp = TempDir::new()?;
    let root = tmp.path().join("repo");
    fs::create_dir_all(&root)?;
    copy_cicd_script(&root)?;

    let bin_dir = tmp.path().join("bin");
    write_cargo_recorder(&bin_dir)?;
    let path = prepare_path(&bin_dir)?;
    let log = tmp.path().join("cargo.log");
    let custom_target = tmp.path().join("custom-target");

    let output = Command::new("./cicd.sh")
        .current_dir(&root)
        .env("PATH", path)
        .env("CARGO_LOG", &log)
        .env("CARGO_TARGET_DIR", &custom_target)
        .output()?;
    assert!(
        output.status.success(),
        "cicd.sh failed: status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        custom_target.is_dir(),
        "expected explicit CARGO_TARGET_DIR to exist: {}",
        custom_target.display()
    );

    let default_target = root.join(".vizier/tmp/cargo-target");
    assert!(
        !default_target.exists(),
        "default target directory should not be created when CARGO_TARGET_DIR is set"
    );

    let entries = read_commands(&log)?;
    assert_eq!(
        entries.len(),
        3,
        "expected three cargo invocations; got {entries:?}"
    );
    let expected_target = fs::canonicalize(&custom_target)?;
    for entry in &entries {
        let Some((target, _)) = entry.split_once('|') else {
            panic!("missing target separator in log entry: {entry:?}");
        };
        let recorded_target = fs::canonicalize(target)?;
        assert_eq!(
            recorded_target, expected_target,
            "unexpected CARGO_TARGET_DIR in log entry: {entry:?}"
        );
    }
    Ok(())
}
