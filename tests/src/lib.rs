#![cfg(test)]

use git2::{
    BranchType, Diff, DiffOptions, IndexAddOption, Oid, Repository, Signature, Sort,
    build::CheckoutBuilder,
};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tests crate lives under repo root")
        .to_path_buf()
}

fn vizier_binary() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let root = repo_root();
        let status = Command::new("cargo")
            .current_dir(&root)
            .args([
                "build",
                "--release",
                "--features",
                "mock_llm,integration_testing",
            ])
            .status()
            .expect("failed to invoke cargo build for vizier");
        if !status.success() {
            panic!("cargo build for vizier failed with status {status:?}");
        }
        let path = root.join("target/release/vizier");
        if !path.exists() {
            panic!("expected vizier binary at {}", path.display());
        }
        path
    })
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest_path = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &dest_path)?;
        } else {
            fs::copy(&path, &dest_path)?;
        }
    }
    Ok(())
}

struct IntegrationRepo {
    dir: TempDir,
}

impl IntegrationRepo {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let dir = TempDir::new()?;
        copy_dir_recursive(&repo_root().join("test-repo"), dir.path())?;
        copy_dir_recursive(&repo_root().join(".vizier"), &dir.path().join(".vizier"))?;
        ensure_gitignore(dir.path())?;
        init_repo_at(dir.path())?;
        Ok(Self { dir })
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn repo(&self) -> Repository {
        Repository::open(self.path()).expect("open repo")
    }

    fn vizier_cmd(&self) -> Command {
        let mut cmd = Command::new(vizier_binary());
        cmd.current_dir(self.path());
        cmd
    }

    fn vizier_output(&self, args: &[&str]) -> io::Result<Output> {
        let mut cmd = self.vizier_cmd();
        cmd.args(args);
        cmd.output()
    }

    fn write(&self, rel: &str, contents: &str) -> io::Result<()> {
        let path = self.path().join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, contents)
    }

    fn read(&self, rel: &str) -> io::Result<String> {
        fs::read_to_string(self.path().join(rel))
    }

    fn git(&self, args: &[&str]) -> TestResult {
        let status = Command::new("git")
            .arg("-C")
            .arg(self.path())
            .args(args)
            .status()?;
        if !status.success() {
            return Err(format!("git {:?} failed with status {status:?}", args).into());
        }
        Ok(())
    }
}

fn init_repo_at(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repository::init(path)?;
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

fn add_all(repo: &Repository, specs: &[&str]) -> Result<(), git2::Error> {
    let mut index = repo.index()?;
    index.add_all(specs, IndexAddOption::DEFAULT, None)?;
    index.write()?;
    Ok(())
}

fn oid_for_spec(repo: &Repository, spec: &str) -> Result<Oid, git2::Error> {
    let obj = repo.revparse_single(spec)?;
    Ok(obj.peel_to_commit()?.id())
}

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

fn count_commits_from_head(repo: &Repository) -> Result<usize, git2::Error> {
    let mut walk = repo.revwalk()?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
    walk.push_head()?;
    Ok(walk.count())
}

fn find_conversation_commit(repo: &Repository) -> Result<String, Box<dyn std::error::Error>> {
    let mut revwalk = repo.revwalk()?;
    revwalk.push_head()?;

    for oid in revwalk {
        let oid = oid?;
        let commit = repo.find_commit(oid)?;
        if let Some(message) = commit.message() {
            if message.contains("VIZIER CONVERSATION") {
                return Ok(message.to_string());
            }
        }
    }

    Err("failed to find conversation commit".into())
}

fn prepare_conflicting_plan(
    repo: &IntegrationRepo,
    slug: &str,
    master_contents: &str,
    plan_contents: &str,
) -> TestResult {
    let draft = repo.vizier_output(&["draft", "--name", slug, "conflict smoke"])?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    repo.git(&["checkout", &format!("draft/{slug}")])?;
    repo.write("a", plan_contents)?;
    repo.git(&["add", "a"])?;
    repo.git(&["commit", "-m", "plan branch change"])?;

    repo.git(&["checkout", "master"])?;
    repo.write("a", master_contents)?;
    repo.git(&["commit", "-am", "master change"])?;
    Ok(())
}

fn clean_workdir(repo: &IntegrationRepo) -> TestResult {
    reset_workdir(repo)?;
    repo.git(&["clean", "-fd"])?;
    Ok(())
}

fn reset_workdir(repo: &IntegrationRepo) -> TestResult {
    repo.git(&["reset", "--hard"])?;
    Ok(())
}

fn ensure_gitignore(path: &Path) -> io::Result<()> {
    let ignore_path = path.join(".gitignore");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ignore_path)?;
    writeln!(file, "\n# Vizier test state")?;
    writeln!(file, ".vizier/tmp/")?;
    writeln!(file, ".vizier/tmp-worktrees/")?;
    Ok(())
}

#[test]
fn test_save() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;

    let status = repo.vizier_cmd().arg("save").status()?;
    assert!(status.success(), "vizier save exited with {status:?}");

    let after = count_commits_from_head(&repo.repo())?;
    assert_eq!(after - before, 3, "save should create three commits");

    let snapshot = repo.read(".vizier/.snapshot")?;
    assert!(
        snapshot.contains("some snapshot change"),
        "expected Codex mock snapshot update"
    );

    let convo = find_conversation_commit(&repo.repo())?;
    assert!(
        convo.to_ascii_lowercase().contains("mock codex response"),
        "conversation commit missing Codex response"
    );
    Ok(())
}

#[test]
fn test_save_with_staged_files() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.write("b", "this is an integration test")?;
    add_all(&repo.repo(), &["."])?;

    let status = repo.vizier_cmd().arg("save").status()?;
    assert!(status.success(), "vizier save exited with {status:?}");

    let repo_handle = repo.repo();
    assert_eq!(count_files_in_commit(&repo_handle, "HEAD")?, 2);
    assert_eq!(count_files_in_commit(&repo_handle, "HEAD~1")?, 2);
    assert_eq!(count_files_in_commit(&repo_handle, "HEAD~2")?, 0);
    Ok(())
}

#[test]
fn test_save_without_code_changes() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;

    let mut cmd = repo.vizier_cmd();
    cmd.arg("save");
    cmd.env("VIZIER_IT_SKIP_CODE_CHANGE", "1");
    let output = cmd.output()?;

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

    let after = count_commits_from_head(&repo.repo())?;
    assert_eq!(
        after - before,
        2,
        "should only create conversation + narrative commits"
    );
    assert_eq!(count_files_in_commit(&repo.repo(), "HEAD")?, 2);
    assert_eq!(count_files_in_commit(&repo.repo(), "HEAD~1")?, 0);
    Ok(())
}

#[test]
fn test_draft_creates_branch_and_plan() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;

    let output = repo.vizier_output(&["draft", "--name", "smoke", "ship the draft flow"])?;
    assert!(
        output.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        !repo
            .path()
            .join(".vizier/implementation-plans/smoke.md")
            .exists(),
        "plan should not appear in the operator’s working tree"
    );

    let repo_handle = repo.repo();
    let branch = repo_handle.find_branch("draft/smoke", BranchType::Local)?;
    let commit = branch.get().peel_to_commit()?;
    let tree = commit.tree()?;
    let entry = tree.get_path(Path::new(".vizier/implementation-plans/smoke.md"))?;
    let blob = repo_handle.find_blob(entry.id())?;
    let contents = std::str::from_utf8(blob.content())?;
    assert!(contents.contains("status: draft"));
    assert!(contents.contains("spec_source: inline"));
    assert!(contents.contains("ship the draft flow"));
    assert!(contents.contains("## Implementation Plan"));

    let after = count_commits_from_head(&repo_handle)?;
    assert_eq!(after, before, "draft should not add commits to master");
    Ok(())
}

#[test]
fn test_approve_merges_plan() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let draft = repo.vizier_output(&[
        "draft",
        "--name",
        "approve-smoke",
        "approval smoke test spec",
    ])?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    let list_before = repo.vizier_output(&["approve", "--list"])?;
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

    clean_workdir(&repo)?;

    {
        let repo_handle = repo.repo();
        let mut checkout = CheckoutBuilder::new();
        checkout.force();
        repo_handle.checkout_head(Some(&mut checkout))?;
    }

    let approve = repo.vizier_output(&["approve", "approve-smoke", "--yes"])?;
    assert!(
        approve.status.success(),
        "vizier approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );

    let repo_handle = repo.repo();
    let branch = repo_handle
        .find_branch("draft/approve-smoke", BranchType::Local)
        .expect("draft branch exists after approval");
    let merge_commit = branch.get().peel_to_commit()?;
    let tree = merge_commit.tree()?;
    let entry = tree.get_path(Path::new(".vizier/implementation-plans/approve-smoke.md"))?;
    let blob = repo_handle.find_blob(entry.id())?;
    let contents = std::str::from_utf8(blob.content())?;
    assert!(
        contents.contains("approve-smoke"),
        "plan document missing slug content"
    );

    Ok(())
}

#[test]
fn test_merge_removes_plan_document() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "remove-plan", "plan removal smoke"])?;
    repo.vizier_output(&["approve", "remove-plan", "--yes"])?;
    clean_workdir(&repo)?;
    let merge = repo.vizier_output(&["merge", "remove-plan", "--yes"])?;
    assert!(
        merge.status.success(),
        "vizier merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    assert!(
        !repo
            .path()
            .join(".vizier/implementation-plans/remove-plan.md")
            .exists(),
        "plan document should be removed after vizier merge"
    );
    let _repo_handle = repo.repo();
    let head = _repo_handle.head()?.peel_to_commit()?;
    let message = head.message().unwrap_or_default().to_string();
    assert!(
        message.contains("Implementation Plan:"),
        "merge commit should inline plan metadata"
    );
    Ok(())
}

#[test]
fn test_merge_conflict_auto_resolve() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_conflicting_plan(
        &repo,
        "conflict-auto",
        "master edits collide\n",
        "auto resolution should keep this line\n",
    )?;
    clean_workdir(&repo)?;

    let merge = repo.vizier_output(&[
        "merge",
        "conflict-auto",
        "--yes",
        "--auto-resolve-conflicts",
    ])?;
    assert!(
        merge.status.success(),
        "auto-resolve merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );

    let sentinel = repo
        .path()
        .join(".vizier/tmp/merge-conflicts/conflict-auto.json");
    assert!(
        !sentinel.exists(),
        "sentinel should not remain after auto resolution"
    );

    let contents = repo.read("a")?;
    assert!(
        contents.contains("auto resolution should keep this line"),
        "file contents did not reflect plan branch after auto resolution: {}",
        contents
    );

    let status = Command::new("git")
        .args(["-C", repo.path().to_str().unwrap(), "status", "--porcelain"])
        .output()?;
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "working tree should be clean after auto resolution"
    );
    Ok(())
}

#[test]
fn test_merge_conflict_creates_sentinel() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_conflicting_plan(
        &repo,
        "conflict-manual",
        "master branch keeps its version\n",
        "plan branch prefers this text\n",
    )?;
    clean_workdir(&repo)?;

    let merge = repo.vizier_output(&["merge", "conflict-manual", "--yes"])?;
    assert!(
        !merge.status.success(),
        "expected merge to fail on conflicts"
    );

    let sentinel = repo
        .path()
        .join(".vizier/tmp/merge-conflicts/conflict-manual.json");
    assert!(sentinel.exists(), "conflict sentinel missing after failure");
    Ok(())
}
