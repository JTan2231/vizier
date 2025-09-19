macro_rules! assert_true {
    ($expr:expr) => {
        match $expr() {
            Ok(_) => {}
            Err(e) => return Err(e),
        }
    };
}

macro_rules! test {
    ($expr:expr) => {
        eprintln!("====================");
        eprintln!("TESTING {}", stringify!($expr));
        eprintln!("---");

        assert_true!(test_init);

        let res = $expr();
        eprintln!("---");

        match res {
            Ok(_) => eprintln!("{} PASSED", stringify!($expr)),
            Err(e) => eprintln!("{} FAILED: {:?}", stringify!($expr), e),
        };

        let _ = std::fs::remove_dir_all("test-repo-active");
    };
}

macro_rules! cmd {
    ($cmd:expr $(, $arg:expr)* $(,)?) => {{
        std::process::Command::new($cmd)
            $(.arg($arg))*
            .current_dir("test-repo-active")
            .output()
    }};
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::process::Command::new("cargo")
        .args(&[
            "build",
            "--release",
            "--features",
            "mock_llm,integration_testing",
        ])
        .output()
        .expect("failed to execute cargo build");

    if !output.status.success() {
        panic!("Build failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    eprintln!("Cargo build successful");

    let _ = find_vizier();

    test!(test_save);
    test!(test_save_with_staged_files);

    Ok(())
}

fn find_vizier() -> std::path::PathBuf {
    let candidate = std::path::PathBuf::from("./target/release/vizier");

    if candidate.exists() {
        return candidate;
    } else {
        panic!(
            "Integration tests require to be run from the root directory with `cargo test --release --features mock_llm`"
        );
    }
}

fn clone_test_repo() -> Result<(), Box<dyn std::error::Error>> {
    use std::fs;
    use std::path::Path;

    fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
        fs::create_dir_all(dst)?;

        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let path = entry.path();
            let file_name = entry.file_name();
            let dest_path = dst.join(&file_name);

            if path.is_dir() {
                copy_dir_recursive(&path, &dest_path)?;
            } else {
                fs::copy(&path, &dest_path)?;
            }
        }
        Ok(())
    }

    let src = Path::new("./test-repo");
    let dst = Path::new("./test-repo-active");

    copy_dir_recursive(src, dst)?;
    Ok(())
}

fn git_init() -> Result<(), Box<dyn std::error::Error>> {
    cmd!("git", "init")?;
    cmd!("git", "add", ".")?;
    cmd!("git", "commit", "-m", "init")?;

    Ok(())
}

fn test_init() -> Result<(), Box<dyn std::error::Error>> {
    clone_test_repo()?;
    git_init()?;

    Ok(())
}

// TODO: We need a way to mock tool calls through the auditor

// setting:
// -> nothing staged
//
// action:
// -> files are changed through the auditor/file tracker
// -> run `vizier save`
//
// expectation:
// -> 3 commits (conversation, narrative change, code change)
fn test_save() -> Result<(), Box<dyn std::error::Error>> {
    cmd!("ls", "-R")?;

    let before = cmd!("git", "rev-list", "--count", "HEAD")?;
    let before_count: usize = String::from_utf8(before.stdout)?.trim().parse()?;

    std::process::Command::new("../target/release/vizier")
        .arg("save")
        .current_dir("test-repo-active")
        .stdout(std::process::Stdio::from(std::io::stderr()))
        .stderr(std::process::Stdio::inherit())
        .spawn()?
        .wait()?;

    // Get new commit count
    let after = cmd!("git", "rev-list", "--count", "HEAD")?;
    let after_count: usize = String::from_utf8(after.stdout)?.trim().parse()?;

    assert_eq!(after_count - before_count, 3);

    Ok(())
}

fn get_commit_file_count(commit: &str) -> Result<usize, Box<dyn std::error::Error>> {
    let files = cmd!(
        "git",
        "diff-tree",
        "--no-commit-id",
        "--name-only",
        "-r",
        commit
    )?;

    Ok(String::from_utf8(files.stdout)?
        .lines()
        .filter(|line| !line.is_empty())
        .count())
}

// setting:
// -> file staged
//
// action:
// -> files are changed through the auditor/file tracker
// -> run `vizier save`
//
// expectation:
// -> 3 commits (conversation, narrative change, code change)
//   -> conversation has 0 files attached
//   -> narrative has .vizier files attached
//   -> code has the code files attached
fn test_save_with_staged_files() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::write("test-repo-active/b", "this is an integration test")?;
    cmd!("git", "add", ".")?;

    std::process::Command::new("../target/release/vizier")
        .arg("save")
        .current_dir("test-repo-active")
        .stdout(std::process::Stdio::from(std::io::stderr()))
        .stderr(std::process::Stdio::inherit())
        .spawn()?
        .wait()?;

    // code change
    assert_eq!(get_commit_file_count("HEAD")?, 2);
    // narrative change
    assert_eq!(get_commit_file_count("HEAD~1")?, 2);
    // conversation
    assert_eq!(get_commit_file_count("HEAD~2")?, 0);

    Ok(())
}
