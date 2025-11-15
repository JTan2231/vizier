use git2::{
    BranchType, Diff, DiffOptions, IndexAddOption, Oid, Repository, Signature, Sort, StatusOptions,
    build::CheckoutBuilder,
};
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
    test!(test_save_without_code_changes);
    test!(test_draft_creates_branch_and_plan);
    test!(test_approve_merges_plan);
    test!(test_approve_keeps_primary_checkout_clean);
    test!(test_merge_removes_plan_document);
    test!(test_merge_conflict_manual_resume);
    test!(test_merge_conflict_auto_resolve);

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

    if dst.exists() {
        fs::remove_dir_all(dst)?;
    }

    copy_dir_recursive(src, dst)?;
    Ok(())
}

fn open_repo() -> Result<Repository, git2::Error> {
    Repository::open("test-repo-active")
}

fn run_git(args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg("test-repo-active")
        .args(args)
        .status()?;
    assert!(
        status.success(),
        "git {:?} failed with status {:?}",
        args,
        status.code()
    );
    Ok(())
}

fn prepare_conflicting_plan(
    slug: &str,
    on_master: &str,
    on_plan: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let draft = std::process::Command::new("../target/release/vizier")
        .args(["draft", "--name", slug, "conflict smoke"])
        .current_dir("test-repo-active")
        .output()?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    run_git(&["checkout", &format!("draft/{slug}")])?;
    std::fs::write("test-repo-active/a", on_plan)?;
    run_git(&["add", "a"])?;
    run_git(&["commit", "-m", "plan branch change"])?;

    run_git(&["checkout", "master"])?;
    std::fs::write("test-repo-active/a", on_master)?;
    run_git(&["commit", "-am", "master change"])?;

    Ok(())
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

fn assert_clean_primary_checkout(repo: &Repository) -> Result<(), Box<dyn std::error::Error>> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .exclude_submodules(true);
    let statuses = repo.statuses(Some(&mut opts))?;
    assert!(
        statuses.is_empty(),
        "expected primary checkout to remain clean but saw {} entries",
        statuses.len()
    );
    Ok(())
}

fn session_id_from_commit(
    repo: &Repository,
    spec: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let commit = repo.find_commit(oid_for_spec(repo, spec)?)?;
    let message = commit.message().unwrap_or("");
    for line in message.lines() {
        if let Some(rest) = line.strip_prefix("Session ID: ") {
            let trimmed = rest.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }

    Err("Session ID not found in commit".into())
}

fn session_log_path(session_id: &str) -> PathBuf {
    Path::new("test-repo-active")
        .join(".vizier")
        .join("sessions")
        .join(session_id)
        .join("session.json")
}

fn assert_session_log_contains(
    session_id: &str,
    needle: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = session_log_path(session_id);
    assert!(
        path.exists(),
        "session log does not exist: {}",
        path.display()
    );
    let contents = std::fs::read_to_string(&path)?;
    assert!(
        contents.contains(needle),
        "session log missing expected text: {}",
        needle
    );
    Ok(())
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
//   expect: +2 commits (narrative + code)
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
    assert_eq!(after_count - before_count, 2);

    let snapshot = std::fs::read_to_string("test-repo-active/.vizier/.snapshot")?;
    assert!(
        snapshot.contains("some snapshot change"),
        "expected Codex mock snapshot update"
    );

    let session_id = session_id_from_commit(&repo, "HEAD~1")?;
    assert_session_log_contains(&session_id, "mock codex response")?;
    Ok(())
}

// TEST: save with staged files
//   setup: write a file, stage it
//   action: run vizier save
//   expect: last two commits correspond to code/narrative with file counts {2, 2}
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

    Ok(())
}

// TEST: save with clean tree (no tracked code changes)
//   action: run vizier save with code-change stub disabled
//   expect: only the narrative commit; CLI reports no code commit and session path
fn test_save_without_code_changes() -> Result<(), Box<dyn std::error::Error>> {
    let repo = open_repo()?;
    let before_count = count_commits_from_head(&repo)?;

    let output = std::process::Command::new("../target/release/vizier")
        .arg("save")
        .current_dir("test-repo-active")
        .env("VIZIER_IT_SKIP_CODE_CHANGE", "1")
        .output()?;

    assert!(
        output.status.success(),
        "vizier save failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("code_commit=none"),
        "expected save output to skip code commit but saw: {}",
        stdout
    );
    assert!(
        stdout.contains("session="),
        "expected save output to include session path"
    );

    let repo = open_repo()?;
    let after_count = count_commits_from_head(&repo)?;
    assert_eq!(
        after_count - before_count,
        1,
        "expected only the narrative commit"
    );

    // narrative change (HEAD)
    assert_eq!(count_files_in_commit(&repo, "HEAD")?, 2);

    let session_id = session_id_from_commit(&repo, "HEAD")?;
    assert_session_log_contains(&session_id, "mock codex response")?;

    Ok(())
}

fn test_draft_creates_branch_and_plan() -> Result<(), Box<dyn std::error::Error>> {
    let before_commits = {
        let repo = open_repo()?;
        count_commits_from_head(&repo)?
    };
    let output = std::process::Command::new("../target/release/vizier")
        .args(["draft", "--name", "smoke", "ship the draft flow"])
        .current_dir("test-repo-active")
        .output()?;

    assert!(
        output.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let repo = open_repo()?;
    let head = repo.head()?;
    assert_eq!(
        head.shorthand(),
        Some("master"),
        "draft should not move the current branch"
    );

    assert!(
        !Path::new("test-repo-active/.vizier/implementation-plans/smoke.md").exists(),
        "plan should not appear in the operator’s working tree"
    );

    let branch = repo.find_branch("draft/smoke", BranchType::Local)?;
    let commit = branch.get().peel_to_commit()?;
    let tree = commit.tree()?;
    let entry = tree.get_path(Path::new(".vizier/implementation-plans/smoke.md"))?;
    let blob = repo.find_blob(entry.id())?;
    let contents = std::str::from_utf8(blob.content())?;

    assert!(
        contents.contains("status: draft"),
        "plan metadata missing draft status"
    );
    assert!(
        contents.contains("spec_source: inline"),
        "plan metadata missing spec source"
    );
    assert!(
        contents.contains("ship the draft flow"),
        "operator spec was not embedded"
    );
    assert!(
        contents.contains("## Implementation Plan"),
        "plan body heading missing"
    );

    let after_commits = count_commits_from_head(&repo)?;
    assert_eq!(
        after_commits, before_commits,
        "vizier draft should not add commits to the primary branch"
    );

    Ok(())
}

fn test_approve_merges_plan() -> Result<(), Box<dyn std::error::Error>> {
    let draft = std::process::Command::new("../target/release/vizier")
        .args([
            "draft",
            "--name",
            "approve-smoke",
            "approval smoke test spec",
        ])
        .current_dir("test-repo-active")
        .output()?;

    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    let list_before = std::process::Command::new("../target/release/vizier")
        .args(["approve", "--list"])
        .current_dir("test-repo-active")
        .output()?;
    assert!(
        list_before.status.success(),
        "vizier approve --list failed: {}",
        String::from_utf8_lossy(&list_before.stderr)
    );
    let stdout_before = String::from_utf8_lossy(&list_before.stdout);
    assert!(
        stdout_before.contains("plan=approve-smoke"),
        "pending plans missing approve-smoke: {}",
        stdout_before
    );

    {
        let repo = open_repo()?;
        let mut checkout = CheckoutBuilder::new();
        checkout.force();
        repo.checkout_head(Some(&mut checkout))?;
    }

    let approve = std::process::Command::new("../target/release/vizier")
        .args(["approve", "approve-smoke", "--yes", "--delete-branch"])
        .current_dir("test-repo-active")
        .output()?;
    assert!(
        approve.status.success(),
        "vizier approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );

    assert!(
        Path::new("test-repo-active/.vizier/implementation-plans/approve-smoke.md").exists(),
        "plan file should be present after approval"
    );

    let repo = open_repo()?;
    let merge_commit = repo.head()?.peel_to_commit()?;
    let message = merge_commit.message().unwrap_or_default().to_string();
    assert!(
        message.contains("Implementation Plan:\n---"),
        "merge commit missing implementation plan block: {}",
        message
    );

    assert!(
        repo.find_branch("draft/approve-smoke", BranchType::Local)
            .is_err(),
        "draft branch should be deleted when --delete-branch is used"
    );

    let list_after = std::process::Command::new("../target/release/vizier")
        .args(["approve", "--list"])
        .current_dir("test-repo-active")
        .output()?;
    assert!(
        list_after.status.success(),
        "vizier approve --list failed after merge: {}",
        String::from_utf8_lossy(&list_after.stderr)
    );
    let stdout_after = String::from_utf8_lossy(&list_after.stdout);
    assert!(
        stdout_after.contains("No pending draft branches"),
        "expected no pending plans but saw: {}",
        stdout_after
    );

    Ok(())
}

fn test_approve_keeps_primary_checkout_clean() -> Result<(), Box<dyn std::error::Error>> {
    let draft = std::process::Command::new("../target/release/vizier")
        .args([
            "draft",
            "--name",
            "approve-clean",
            "ensure approve keeps edits inside the plan worktree",
        ])
        .current_dir("test-repo-active")
        .output()?;

    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    let approve = std::process::Command::new("../target/release/vizier")
        .args(["approve", "approve-clean", "--yes"])
        .current_dir("test-repo-active")
        .output()?;

    assert!(
        approve.status.success(),
        "vizier approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );

    let repo = open_repo()?;
    assert_clean_primary_checkout(&repo)?;

    Ok(())
}

fn test_merge_removes_plan_document() -> Result<(), Box<dyn std::error::Error>> {
    let draft = std::process::Command::new("../target/release/vizier")
        .args([
            "draft",
            "--name",
            "remove-plan",
            "remove plan document during merge",
        ])
        .current_dir("test-repo-active")
        .output()?;

    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    let merge = std::process::Command::new("../target/release/vizier")
        .args(["merge", "remove-plan", "--yes", "--delete-branch"])
        .current_dir("test-repo-active")
        .output()?;
    assert!(
        merge.status.success(),
        "vizier merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );

    assert!(
        !Path::new("test-repo-active/.vizier/implementation-plans/remove-plan.md").exists(),
        "plan document should be removed after vizier merge"
    );

    let repo = open_repo()?;
    let head = repo.head()?.peel_to_commit()?;
    let message = head.message().unwrap_or_default().to_string();
    assert!(
        message.contains("Implementation Plan:\n---"),
        "merge commit should inline the plan document even after deletion: {}",
        message
    );
    assert!(
        message.contains("plan: remove-plan"),
        "merge commit should include the plan front matter: {}",
        message
    );
    assert!(
        message.contains("## Implementation Plan"),
        "merge commit should include the plan body: {}",
        message
    );
    assert!(
        !message.contains(".vizier/implementation-plans"),
        "merge commit should not reference implementation plan file paths: {}",
        message
    );

    Ok(())
}

fn test_merge_conflict_manual_resume() -> Result<(), Box<dyn std::error::Error>> {
    prepare_conflicting_plan(
        "conflict-manual",
        "master branch keeps its version\n",
        "plan branch prefers this text\n",
    )?;

    let first_merge = std::process::Command::new("../target/release/vizier")
        .args(["merge", "conflict-manual", "--yes"])
        .current_dir("test-repo-active")
        .output()?;
    assert!(
        !first_merge.status.success(),
        "expected merge to fail on conflicts"
    );

    let sentinel = Path::new("test-repo-active/.vizier/tmp/merge-conflicts/conflict-manual.json");
    assert!(
        sentinel.exists(),
        "conflict sentinel missing after failed merge"
    );

    std::fs::write("test-repo-active/a", "manual resolution wins\n")?;
    run_git(&["add", "a"])?;

    let rerun = std::process::Command::new("../target/release/vizier")
        .args(["merge", "conflict-manual", "--yes"])
        .current_dir("test-repo-active")
        .output()?;
    assert!(
        rerun.status.success(),
        "vizier merge rerun failed: {}",
        String::from_utf8_lossy(&rerun.stderr)
    );

    assert!(
        !sentinel.exists(),
        "sentinel should be removed after successful merge resume"
    );

    let repo = open_repo()?;
    let head = repo.head()?.peel_to_commit()?;
    let message = head.message().unwrap_or_default().to_string();
    assert!(
        message.contains("Implementation Plan:\n---"),
        "merge commit missing implementation plan block after resume: {}",
        message
    );

    Ok(())
}

fn test_merge_conflict_auto_resolve() -> Result<(), Box<dyn std::error::Error>> {
    prepare_conflicting_plan(
        "conflict-auto",
        "master edits collide\n",
        "auto resolution should keep this line\n",
    )?;

    let merge = std::process::Command::new("../target/release/vizier")
        .args([
            "merge",
            "conflict-auto",
            "--yes",
            "--auto-resolve-conflicts",
        ])
        .current_dir("test-repo-active")
        .output()?;
    assert!(
        merge.status.success(),
        "auto-resolve merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );

    let sentinel = Path::new("test-repo-active/.vizier/tmp/merge-conflicts/conflict-auto.json");
    assert!(
        !sentinel.exists(),
        "sentinel should not remain after auto resolution"
    );

    let contents = std::fs::read_to_string("test-repo-active/a")?;
    assert!(
        contents.contains("auto resolution should keep this line"),
        "file contents did not reflect plan branch after auto resolution: {}",
        contents
    );

    let status = std::process::Command::new("git")
        .args([
            "-C",
            "test-repo-active",
            "status",
            "--porcelain",
            "--untracked-files=no",
        ])
        .output()?;
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "working tree should be clean after auto resolution"
    );

    Ok(())
}
