use git2::{Diff, DiffOptions, IndexAddOption, Oid, Repository, Signature, Sort};
use std::path::{Path, PathBuf};

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

fn find_vizier() -> PathBuf {
    let candidate = PathBuf::from("./target/release/vizier");
    if candidate.exists() {
        candidate
    } else {
        panic!(
            "Integration tests require to be run from the root directory with `cargo test --release --features mock_llm`"
        );
    }
}

fn clone_test_repo() -> Result<(), Box<dyn std::error::Error>> {
    use std::fs;

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

fn open_repo() -> Result<Repository, git2::Error> {
    Repository::open("test-repo-active")
}

fn init_repo_and_initial_commit() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repository::init("test-repo-active")?;

    {
        let mut cfg = repo.config()?;
        cfg.set_str("user.name", "Vizier")?;
        cfg.set_str("user.email", "vizier@test.com")?;
    }

    add_all(&repo, &["."])?;

    let mut index = repo.index()?;
    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;

    let sig = Signature::now("Vizier", "vizier@test.com")?;

    let commit_oid = repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])?;

    let obj = repo.find_object(commit_oid, None)?;
    repo.checkout_tree(&obj, None)?;
    repo.set_head("refs/heads/master")?;
    Ok(())
}

/// `git add <specs>`; here we use `Index::add_all` for globs / directories.
fn add_all(repo: &Repository, specs: &[&str]) -> Result<(), git2::Error> {
    let mut index = repo.index()?;
    index.add_all(specs, IndexAddOption::DEFAULT, None)?;
    index.write()?;
    Ok(())
}

/// Count commits reachable from HEAD (`git rev-list --count HEAD`)
fn count_commits_from_head(repo: &Repository) -> Result<usize, git2::Error> {
    let mut walk = repo.revwalk()?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
    walk.push_head()?;
    let mut count = 0usize;
    for _ in walk {
        count += 1;
    }
    Ok(count)
}

/// Parse a revspec like "HEAD", "HEAD~1", etc. → commit OID
fn oid_for_spec(repo: &Repository, spec: &str) -> Result<Oid, git2::Error> {
    // revparse_single handles most common specs (HEAD, HEAD~N, <sha>, etc.)
    let obj = repo.revparse_single(spec)?;
    Ok(obj.peel_to_commit()?.id())
}

/// Count # of file entries touched by a commit (like `git diff-tree --no-commit-id --name-only -r <commit>`).
/// We diff parent tree → commit tree (or empty → commit tree if root).
fn count_files_in_commit(repo: &Repository, spec: &str) -> Result<usize, git2::Error> {
    let commit = repo.find_commit(oid_for_spec(repo, spec)?)?;
    let tree = commit.tree()?;

    let parent_tree = if commit.parent_count() > 0 {
        Some(commit.parent(0)?.tree()?)
    } else {
        None
    };

    let diff = match parent_tree {
        Some(ref pt) => {
            repo.diff_tree_to_tree(Some(pt), Some(&tree), Some(&mut DiffOptions::new()))?
        }
        None => repo.diff_tree_to_tree(None, Some(&tree), Some(&mut DiffOptions::new()))?,
    };

    Ok(diff_deltas_len(&diff))
}

fn diff_deltas_len(diff: &Diff) -> usize {
    diff.deltas().count()
}

fn git_init() -> Result<(), Box<dyn std::error::Error>> {
    init_repo_and_initial_commit()?;
    Ok(())
}

fn test_init() -> Result<(), Box<dyn std::error::Error>> {
    clone_test_repo()?;
    git_init()?;
    Ok(())
}

// TEST: save with nothing staged
//   before: count HEAD commits
//   action: run vizier save (external binary under test)
//   expect: +3 commits
fn test_save() -> Result<(), Box<dyn std::error::Error>> {
    let repo = open_repo()?;
    let before_count = count_commits_from_head(&repo)?;

    std::process::Command::new("../target/release/vizier")
        .arg("save")
        .current_dir("test-repo-active")
        .stdout(std::process::Stdio::from(std::io::stderr()))
        .stderr(std::process::Stdio::inherit())
        .spawn()?
        .wait()?;

    let after_count = count_commits_from_head(&repo)?;
    assert_eq!(after_count - before_count, 3);
    Ok(())
}

// TEST: save with staged files
//   setup: write a file, stage it
//   action: run vizier save
//   expect: last three commits correspond to code/narrative/conversation
//           with file counts {2, 2, 0} respectively (matches your original)
fn test_save_with_staged_files() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::write("test-repo-active/b", "this is an integration test")?;
    let repo = open_repo()?;
    add_all(&repo, &["."])?;

    std::process::Command::new("../target/release/vizier")
        .arg("save")
        .current_dir("test-repo-active")
        .stdout(std::process::Stdio::from(std::io::stderr()))
        .stderr(std::process::Stdio::inherit())
        .spawn()?
        .wait()?;

    // code change (HEAD)
    assert_eq!(count_files_in_commit(&repo, "HEAD")?, 2);
    // narrative change (HEAD~1)
    assert_eq!(count_files_in_commit(&repo, "HEAD~1")?, 2);
    // conversation (HEAD~2)
    assert_eq!(count_files_in_commit(&repo, "HEAD~2")?, 0);

    Ok(())
}
