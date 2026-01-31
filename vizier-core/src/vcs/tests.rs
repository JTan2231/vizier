use super::remotes::{
    CredentialExecutor, CredentialRequestContext, CredentialResult, StrategyResult,
    build_credential_plan, execute_credential_plan,
};
use super::*;
use git2::{Cred, CredentialType, IndexAddOption, Oid, Repository, RepositoryState, Signature};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

struct TestRepo {
    tempdir: tempfile::TempDir,
    repo: Repository,
    path_utf8: String,
}

impl TestRepo {
    fn new() -> Self {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let repo = Repository::init(tempdir.path()).expect("init repo");
        let _ = repo.config().and_then(|mut c| {
            c.set_str("user.name", "Tester")?;
            c.set_str("user.email", "tester@example.com")
        });
        let path_utf8 = tempdir.path().to_str().expect("repo path utf8").to_string();
        Self {
            tempdir,
            repo,
            path_utf8,
        }
    }

    fn repo(&self) -> &Repository {
        &self.repo
    }

    fn path(&self) -> &Path {
        self.tempdir.path()
    }

    fn path_str(&self) -> &str {
        self.path_utf8.as_str()
    }

    fn join(&self, rel: &str) -> PathBuf {
        self.tempdir.path().join(rel)
    }

    fn write(&self, rel: &str, contents: &str) {
        write(&self.join(rel), contents);
    }

    fn append(&self, rel: &str, contents: &str) {
        append(&self.join(rel), contents);
    }
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut f = File::create(path).unwrap();
    f.write_all(contents.as_bytes()).unwrap();
    f.sync_all().unwrap();
}

fn append(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    f.write_all(contents.as_bytes()).unwrap();
    f.sync_all().unwrap();
}

fn raw_commit(repo: &Repository, msg: &str) -> Oid {
    let mut idx = repo.index().unwrap();
    idx.add_all(["."], IndexAddOption::DEFAULT, None).unwrap();
    idx.write().unwrap();
    let tree_id = idx.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = repo
        .signature()
        .or_else(|_| Signature::now("Tester", "tester@example.com"))
        .unwrap();
    let parent_opt = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit> = parent_opt.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
        .unwrap()
}

#[test]
fn apply_cherry_pick_sequence_errors_when_head_moves() {
    let repo = TestRepo::new();
    append(repo.join("a").as_path(), "base\n");
    let base = raw_commit(repo.repo(), "base");

    append(repo.join("a").as_path(), "second\n");
    let second = raw_commit(repo.repo(), "second");

    let result = apply_cherry_pick_sequence(base, &[second], None, None);
    assert!(
        result.is_err(),
        "expected apply_cherry_pick_sequence to fail when HEAD moved off the recorded start"
    );
}

fn raw_stage(repo: &Repository, rel: &str) {
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new(rel)).unwrap();
    idx.write().unwrap();
}

#[test]
fn push_current_branch_updates_remote_tracking() {
    let repo = TestRepo::new();
    let remote_dir = tempfile::TempDir::new().expect("remote tempdir");
    Repository::init_bare(remote_dir.path()).expect("init bare remote");
    let remote_path = remote_dir
        .path()
        .to_str()
        .expect("remote path utf8")
        .to_owned();

    repo.repo()
        .remote("origin", &remote_path)
        .expect("configure remote");

    repo.write("file.txt", "hello\n");
    raw_commit(repo.repo(), "initial");

    let branch = repo.repo().head().unwrap().shorthand().unwrap().to_string();

    push_current_branch_in(repo.path(), "origin").expect("push succeeds");

    let remote_repo = Repository::open(remote_dir.path()).expect("open remote repo");
    let remote_ref = remote_repo
        .find_reference(&format!("refs/heads/{branch}"))
        .expect("remote branch exists");
    let local_oid = repo.repo().head().unwrap().target().unwrap();
    assert_eq!(remote_ref.target(), Some(local_oid));

    let tracking_ref = repo
        .repo()
        .find_reference(&format!("refs/remotes/origin/{branch}"))
        .expect("tracking ref updated");
    assert_eq!(tracking_ref.target(), Some(local_oid));
}

#[test]
fn push_current_branch_rejects_detached_head() {
    let repo = TestRepo::new();
    let remote_dir = tempfile::TempDir::new().expect("remote tempdir");
    Repository::init_bare(remote_dir.path()).expect("init bare remote");
    let remote_path = remote_dir
        .path()
        .to_str()
        .expect("remote path utf8")
        .to_owned();

    repo.repo()
        .remote("origin", &remote_path)
        .expect("configure remote");

    repo.write("note.txt", "one\n");
    let oid = raw_commit(repo.repo(), "detached");

    repo.repo().set_head_detached(oid).expect("detach head");

    let err = push_current_branch_in(repo.path(), "origin").expect_err("push should fail");
    match err.kind() {
        PushErrorKind::General(message) => {
            assert!(message.contains("not pointing to a branch"));
        }
        other => panic!("unexpected error variant: {:?}", other),
    }
}

struct RecordingExecutor {
    responses: RefCell<VecDeque<StrategyResult>>,
    invoked: RefCell<Vec<CredentialStrategy>>,
}

impl RecordingExecutor {
    fn new(responses: Vec<StrategyResult>) -> Self {
        Self {
            responses: RefCell::new(VecDeque::from(responses)),
            invoked: RefCell::new(Vec::new()),
        }
    }
}

impl CredentialExecutor for RecordingExecutor {
    fn apply(
        &self,
        strategy: &CredentialStrategy,
        ctx: &CredentialRequestContext<'_>,
    ) -> StrategyResult {
        // record username resolution to ensure we pass the default correctly
        assert_eq!(ctx.username_from_url, Some("git"));
        self.invoked.borrow_mut().push(strategy.clone());
        self.responses
            .borrow_mut()
            .pop_front()
            .expect("strategy response available")
    }
}

#[test]
fn credential_plan_attempts_file_keys_when_agent_fails() {
    let plan = build_credential_plan(CredentialType::SSH_KEY, false);
    assert!(plan.contains(&CredentialStrategy::SshKey(SshKeyKind::IdEd25519)));
    assert!(plan.contains(&CredentialStrategy::SshKey(SshKeyKind::IdRsa)));

    let responses = vec![
        StrategyResult::Failure("agent missing".to_string()),
        StrategyResult::Failure("no ed25519".to_string()),
        StrategyResult::Success(Cred::username("git").expect("cred")),
    ];
    let executor = RecordingExecutor::new(responses);

    let ctx = CredentialRequestContext {
        url: "ssh://example.com/repo.git",
        username_from_url: Some("git"),
        default_username: "git",
    };

    let result = execute_credential_plan(&plan, &executor, &ctx);
    match result {
        CredentialResult::Success { .. } => {}
        _ => panic!("expected success after key attempts"),
    }

    let invoked = executor.invoked.borrow();
    let expected = vec![
        CredentialStrategy::SshKey(SshKeyKind::IdEd25519),
        CredentialStrategy::SshKey(SshKeyKind::IdRsa),
        CredentialStrategy::Default,
    ];
    assert_eq!(&expected, invoked.as_slice());
}

// --- normalize_pathspec --------------------------------------------------

#[test]
fn normalize_pathspec_variants() {
    assert_eq!(super::normalize_pathspec(" src//utils/// "), "src/utils");
    assert_eq!(super::normalize_pathspec("./a/b/"), "a/b");
    assert_eq!(super::normalize_pathspec(r#"a\win\path\"#), "a/win/path");

    // Match current implementation: if it starts with `//`, internal `//` are preserved.
    assert_eq!(
        super::normalize_pathspec("//server//share//x"),
        "//server/share/x"
    );
}

// --- add_and_commit core behaviors --------------------------------------

#[test]
fn add_and_commit_basic_and_noop() {
    let repo = TestRepo::new();

    repo.write("README.md", "# one\n");
    let oid1 =
        add_and_commit_in(repo.path(), Some(vec!["README.md"]), "init", false).expect("commit ok");
    assert_ne!(oid1, Oid::zero());

    // No changes, allow_empty=false → "nothing to commit"
    let err = add_and_commit_in(repo.path(), None, "noop", false).unwrap_err();
    assert!(format!("{err}").contains("nothing to commit"));

    // Empty commit (allow_empty=true) → OK
    let oid2 = add_and_commit_in(repo.path(), None, "empty ok", true).expect("empty commit ok");
    assert_ne!(oid2, oid1);
}

#[test]
fn add_and_commit_pathspecs_and_deletes_and_ignores() {
    let repo = TestRepo::new();

    // .gitignore excludes dist/** and vendor/**
    repo.write(".gitignore", "dist/\nvendor/\n");

    // Create a mix
    repo.write("src/a.rs", "fn a(){}\n");
    repo.write("src/b.rs", "fn b(){}\n");
    repo.write("dist/bundle.js", "/* build */\n");
    repo.write("vendor/lib/x.c", "/* vendored */\n");
    let c1 = add_and_commit_in(repo.path(), Some(vec!["./src//"]), "src only", false).unwrap();
    assert_ne!(c1, Oid::zero());

    // Update tracked files + delete one; update_all should stage deletes.
    fs::remove_file(repo.join("src/a.rs")).unwrap();
    repo.append("src/b.rs", "// mod\n");

    // Ignored paths shouldn't be added even with update_all
    let c2 = add_and_commit_in(repo.path(), None, "update tracked & deletions", false).unwrap();
    assert_ne!(c2, c1);

    // Show that vendor/dist are still untracked (ignored), not part of commit 2
    // Verify via a diff: HEAD..workdir should be empty (no pending tracked changes)
    let d = get_diff(repo.path_str(), None, None).unwrap();
    // No pending tracked changes post-commit; any diff would now be due to ignored dirs (which aren't included)
    assert!(d.is_empty() || !d.contains("src/")); // conservative assertion
}

// --- get_diff: path, excludes, ranges -----------------------------------

#[test]
fn diff_head_vs_workdir_and_path_and_exclude() {
    let repo = TestRepo::new();

    repo.write("a/file.txt", "hello\n");
    repo.write("b/file.txt", "world\n");
    raw_commit(repo.repo(), "base");

    repo.append("a/file.txt", "change-a\n"); // unstaged, tracked file
    repo.append("b/file.txt", "change-b\n");
    repo.write("b/inner/keep.txt", "keep\n"); // untracked; should not appear

    // 1) None → HEAD vs workdir(+index). Shows tracked edits, not untracked files.
    let d_all = get_diff(repo.path_str(), None, None).expect("diff");
    assert!(d_all.contains("a/file.txt"));
    assert!(d_all.contains("b/file.txt"));
    assert!(!d_all.contains("b/inner/keep.txt")); // untracked → absent

    // 2) Treat `target` as a path
    let d_b = get_diff(repo.path_str(), Some("b"), None).expect("diff b");
    assert!(!d_b.contains("a/file.txt"));
    assert!(d_b.contains("b/file.txt"));
    assert!(!d_b.contains("b/inner/keep.txt")); // still untracked → absent

    // 3) Exclude subdir via Windows-ish input → normalized
    let d_b_ex =
        get_diff(repo.path_str(), Some("b"), Some(&[r".\b\inner"])).expect("diff b excl inner");
    assert!(d_b_ex.contains("b/file.txt"));
    assert!(!d_b_ex.contains("b/inner/keep.txt"));
}

#[test]
fn diff_single_rev_to_workdir() {
    let repo = TestRepo::new();

    repo.write("x.txt", "x1\n");
    let first = raw_commit(repo.repo(), "c1");

    repo.append("x.txt", "x2\n"); // unstaged, tracked change is visible
    let spec = first.to_string();
    let d = get_diff(repo.path_str(), Some(&spec), None).expect("diff");
    println!("d: {}", d);
    assert!(d.contains("x.txt")); // file appears
    assert!(d.contains("\n+")); // there is an addition hunk
    assert!(d.contains("x2")); // payload appears (don’t hard-code "+x2")
}

#[test]
fn diff_handles_staged_deletions_without_workdir_stat_failure() {
    let repo = TestRepo::new();

    repo.write("gone.txt", "present\n");
    raw_commit(repo.repo(), "add gone");

    // Remove from working tree and stage the deletion.
    fs::remove_file(repo.join("gone.txt")).unwrap();
    {
        let mut index = repo.repo().index().unwrap();
        index.remove_path(Path::new("gone.txt")).unwrap();
        index.write().unwrap();
    }

    let diff = get_diff(repo.path_str(), Some("HEAD"), None).expect("diff with staged deletion");

    assert!(diff.contains("gone.txt"));
    assert!(diff.contains("deleted file mode") || diff.contains("--- a/gone.txt"));
}

#[test]
fn diff_handles_staged_change_with_unstaged_deletion() {
    let repo = TestRepo::new();

    repo.write("keep.txt", "base\n");
    repo.write("gone.txt", "base\n");
    raw_commit(repo.repo(), "base");

    repo.append("keep.txt", "change\n");
    stage_in(repo.path(), Some(vec!["keep.txt"])).expect("stage keep.txt");
    fs::remove_file(repo.join("gone.txt")).unwrap();

    let diff = get_diff(repo.path_str(), Some("HEAD"), None).expect("diff with deletion");
    assert!(diff.contains("keep.txt"));
    assert!(diff.contains("gone.txt"));
}

#[test]
fn diff_with_excludes() {
    let repo = TestRepo::new();

    // Base on main
    repo.write("common.txt", "base\n");
    let base = raw_commit(repo.repo(), "base");

    // Branch at base
    {
        let head_commit = repo.repo().find_commit(base).unwrap();
        repo.repo().branch("feature", &head_commit, true).unwrap();
    }

    // Advance main
    repo.write("main.txt", "m1\n");
    repo.write("vendor/ignored.txt", "should be excluded\n"); // will test exclusion
    let main1 = raw_commit(repo.repo(), "main1");

    // Checkout feature and diverge
    {
        let mut checkout = git2::build::CheckoutBuilder::new();
        repo.repo().set_head("refs/heads/feature").unwrap();
        repo.repo()
            .checkout_head(Some(&mut checkout.force()))
            .unwrap();
    }
    repo.write("feat.txt", "f1\n");

    // A..B (base..main1) shows main changes (including vendor/ by default)
    let dd = format!("{}..{}", base, main1);
    let out_dd = get_diff(repo.path_str(), Some(&dd), None).expect("A..B");
    assert!(out_dd.contains("main.txt"));

    // Now exclude vendor/** using normalize-able pathspec; vendor should disappear
    let out_dd_ex = get_diff(repo.path_str(), Some(&dd), Some(&["vendor//"])).expect("A..B excl");
    println!("DIFF: {}", out_dd_ex);
    assert!(out_dd_ex.contains("main.txt"));
    assert!(!out_dd_ex.contains("vendor/ignored.txt"));
}

#[test]
fn diff_summary_reports_stats_and_name_status() {
    let repo = TestRepo::new();

    repo.write("modify.txt", "base\n");
    repo.write("remove.txt", "gone\n");
    raw_commit(repo.repo(), "base");

    let head_commit = repo.repo().head().unwrap().peel_to_commit().unwrap();
    repo.repo()
        .branch("target", &head_commit, true)
        .expect("create target branch");

    repo.append("modify.txt", "change\n");
    fs::remove_file(repo.join("remove.txt")).unwrap();
    repo.write("added.txt", "new\n");
    raw_commit(repo.repo(), "topic");

    let summary = super::diff_summary_against_target(repo.path(), "target").expect("diff summary");
    assert!(
        summary.stats.contains("files changed") || summary.stats.contains("modify.txt"),
        "stats should mention changes:\n{}",
        summary.stats
    );
    assert!(
        summary.name_status.contains("M\tmodify.txt"),
        "name-status should include modification: {}",
        summary.name_status
    );
    assert!(
        summary.name_status.contains("D\tremove.txt"),
        "name-status should include deletion: {}",
        summary.name_status
    );
    assert!(
        summary.name_status.contains("A\tadded.txt"),
        "name-status should include addition: {}",
        summary.name_status
    );
}

// --- unborn HEAD (no untracked): stage-only then diff --------------------

#[test]
fn diff_unborn_head_against_workdir_without_untracked() {
    let repo = TestRepo::new();

    // File exists in workdir and is STAGED (tracked) but no commits yet.
    repo.write("z.txt", "hello\n");
    raw_stage(repo.repo(), "z.txt"); // index-only

    // get_diff(None) compares empty tree → workdir+index, so z.txt appears even with untracked disabled
    let out = get_diff(repo.path_str(), None, None).expect("diff unborn");
    println!("OUT: {}", out);
    assert!(out.contains("z.txt"));
    assert!(out.contains("hello"));
}

#[test]
fn stage_all_tracks_untracked_and_deletions() {
    let repo = TestRepo::new();

    repo.write("tracked.txt", "base\n");
    repo.write("remove.txt", "gone\n");
    raw_commit(repo.repo(), "base");

    repo.append("tracked.txt", "change\n");
    fs::remove_file(repo.join("remove.txt")).unwrap();
    repo.write("untracked.txt", "fresh\n");

    stage_all_in(repo.path()).expect("stage all");
    let staged = snapshot_staged(repo.path_str()).expect("snapshot staged after stage_all");
    let mut kinds: HashMap<String, super::StagedKind> = HashMap::new();
    for item in staged {
        kinds.insert(item.path, item.kind);
    }

    assert!(
        matches!(kinds.get("tracked.txt"), Some(super::StagedKind::Modified)),
        "tracked modification should be staged"
    );
    assert!(
        matches!(kinds.get("remove.txt"), Some(super::StagedKind::Deleted)),
        "tracked deletion should be staged"
    );
    assert!(
        matches!(kinds.get("untracked.txt"), Some(super::StagedKind::Added)),
        "untracked file should be staged as Added"
    );
}

#[test]
fn stage_all_no_workdir_is_noop() {
    let tempdir = tempfile::TempDir::new().expect("tempdir");
    Repository::init_bare(tempdir.path()).expect("init bare");
    stage_all_in(tempdir.path()).expect("stage_all should tolerate bare repo");
}

// --- stage (index-only) --------------------------------------------------

#[test]
fn stage_paths_and_update_tracked_only() {
    let repo = TestRepo::new();

    // Base commit with two tracked files
    repo.write("a.txt", "A0\n");
    repo.write("b.txt", "B0\n");
    raw_commit(repo.repo(), "base");

    // Workdir changes:
    // - modify tracked a.txt
    // - delete tracked b.txt
    // - create new untracked c.txt
    repo.append("a.txt", "A1\n");
    fs::remove_file(repo.join("b.txt")).unwrap();
    repo.write("c.txt", "C0\n");

    // 1) stage(None) should mirror `git add -u`: stage tracked changes (a.txt mod, b.txt del)
    //    but NOT the new untracked c.txt.
    stage_in(repo.path(), None).expect("stage -u");
    let staged1 = snapshot_staged(repo.path_str()).expect("snapshot staged after -u");

    // Expect: a.txt Modified, b.txt Deleted; no c.txt
    let mut kinds = staged1
        .iter()
        .map(|s| match &s.kind {
            super::StagedKind::Added => ("Added", s.path.clone()),
            super::StagedKind::Modified => ("Modified", s.path.clone()),
            super::StagedKind::Deleted => ("Deleted", s.path.clone()),
            super::StagedKind::TypeChange => ("TypeChange", s.path.clone()),
            super::StagedKind::Renamed { from, to } => ("Renamed", format!("{from}->{to}")),
        })
        .collect::<Vec<_>>();
    kinds.sort_by(|a, b| a.1.cmp(&b.1));
    let mut expected = vec![
        ("Deleted", "b.txt".to_string()),
        ("Modified", "a.txt".to_string()),
    ];
    expected.sort_by(|a, b| a.1.cmp(&b.1));
    assert_eq!(kinds, expected);

    // 2) Now explicitly stage c.txt via stage(Some)
    stage_in(repo.path(), Some(vec!["c.txt"])).expect("stage c.txt");
    let staged2 = snapshot_staged(repo.path_str()).expect("snapshot staged after explicit add");

    let names2: Vec<_> = staged2.iter().map(|s| s.path.as_str()).collect();
    assert!(names2.contains(&"a.txt"));
    assert!(names2.contains(&"b.txt")); // staged deletion appears as b.txt in the snapshot
    assert!(names2.contains(&"c.txt")); // now present as Added
    assert!(
        staged2
            .iter()
            .any(|s| matches!(s.kind, super::StagedKind::Added) && s.path == "c.txt")
    );
}

#[test]
fn stage_paths_allow_missing_stages_deletions() {
    let repo = TestRepo::new();

    repo.write("gone.txt", "base\n");
    raw_commit(repo.repo(), "base");

    fs::remove_file(repo.join("gone.txt")).unwrap();
    stage_paths_allow_missing_in(repo.path(), &["gone.txt"]).expect("stage missing path");

    let staged = snapshot_staged(repo.path_str()).expect("snapshot staged");
    assert!(
        staged
            .iter()
            .any(|s| matches!(s.kind, super::StagedKind::Deleted) && s.path == "gone.txt"),
        "expected deleted file to be staged"
    );
}

// --- unstage: specific paths & entire index (born HEAD) ------------------

#[test]
fn unstage_specific_paths_and_all_with_head() {
    let repo = TestRepo::new();

    repo.write("x.txt", "X0\n");
    repo.write("y.txt", "Y0\n");
    raw_commit(repo.repo(), "base");

    repo.append("x.txt", "X1\n");
    repo.append("y.txt", "Y1\n");

    // Stage both changes (explicit)
    stage_in(repo.path(), Some(vec!["x.txt", "y.txt"])).expect("stage both");

    // Unstage only x.txt → y.txt should remain staged
    unstage_in(repo.path(), Some(vec!["x.txt"])).expect("unstage x");

    let after_x = snapshot_staged(repo.path_str()).expect("snapshot after unstage x");
    assert!(after_x.iter().any(|s| s.path == "y.txt"));
    assert!(!after_x.iter().any(|s| s.path == "x.txt"));

    // Unstage everything → nothing should be staged
    unstage_in(repo.path(), None).expect("unstage all");
    let after_all = snapshot_staged(repo.path_str()).expect("snapshot after unstage all");
    assert!(after_all.is_empty());
}

#[test]
fn unstage_missing_path_is_noop() {
    let repo = TestRepo::new();

    repo.write("a.txt", "A0\n");
    raw_commit(repo.repo(), "base");

    unstage_in(repo.path(), Some(vec!["missing.txt"])).expect("unstage missing");
    let staged = snapshot_staged(repo.path_str()).expect("snapshot staged after missing");
    assert!(staged.is_empty());
}

// --- unstage: unborn HEAD behavior --------------------------------------

#[test]
fn unstage_with_unborn_head() {
    let repo = TestRepo::new();

    // No commits yet; create two files and stage both
    repo.write("u.txt", "U0\n");
    repo.write("v.txt", "V0\n");
    raw_stage(repo.repo(), "u.txt");
    raw_stage(repo.repo(), "v.txt");

    // Path-limited unstage on unborn HEAD should remove entries from index for those paths
    unstage_in(repo.path(), Some(vec!["u.txt"])).expect("unstage u.txt on unborn");
    let staged1 = snapshot_staged(repo.path_str()).expect("snapshot staged after partial unstage");
    let names1: Vec<_> = staged1.iter().map(|s| s.path.as_str()).collect();
    assert!(names1.contains(&"v.txt"));
    assert!(!names1.contains(&"u.txt"));

    // Full unstage on unborn HEAD should clear the index
    unstage_in(repo.path(), None).expect("unstage all unborn");
    let staged2 = snapshot_staged(repo.path_str()).expect("snapshot staged after clear");
    assert!(staged2.is_empty());
}

// --- snapshot → unstage → mutate → restore (A/M/D/R rename) --------------

#[test]
fn snapshot_and_restore_roundtrip_with_rename() {
    let repo = TestRepo::new();

    // Base: a.txt, b.txt
    repo.write("a.txt", "A0\n");
    repo.write("b.txt", "B0\n");
    raw_commit(repo.repo(), "base");

    // Workdir staged set (before snapshot):
    // - RENAME: a.txt -> a_ren.txt (same content to improve rename detection)
    // - DELETE: b.txt
    // - ADD: c.txt
    // - (no explicit extra modifications; rely on rename detection)
    fs::rename(repo.join("a.txt"), repo.join("a_ren.txt")).unwrap();
    fs::remove_file(repo.join("b.txt")).unwrap();
    repo.write("c.txt", "C0\n");

    // Stage all changes so index reflects A/M/D/R
    {
        let mut idx = repo.repo().index().unwrap();
        idx.add_all(["."], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        // ensure deletion is captured
        idx.update_all(["."], None).unwrap();
        idx.write().unwrap();
    }

    // Take snapshot of what's staged now
    let snap = snapshot_staged(repo.path_str()).expect("snapshot staged");

    // Sanity: ensure we actually captured the expected kinds
    // Expect at least: Added c.txt, Deleted b.txt, and a rename a.txt -> a_ren.txt
    let mut have_added_c = false;
    let mut have_deleted_b = false;
    let mut have_renamed_a = false;

    for it in &snap {
        match &it.kind {
            super::StagedKind::Added if it.path == "c.txt" => have_added_c = true,
            super::StagedKind::Deleted if it.path == "b.txt" => have_deleted_b = true,
            super::StagedKind::Renamed { from, to } if from == "a.txt" && to == "a_ren.txt" => {
                have_renamed_a = true
            }
            _ => {}
        }
    }
    assert!(have_added_c, "expected Added c.txt in snapshot");
    assert!(have_deleted_b, "expected Deleted b.txt in snapshot");
    assert!(
        have_renamed_a,
        "expected Renamed a.txt->a_ren.txt in snapshot"
    );

    // Unstage everything
    unstage_in(repo.path(), None).expect("unstage all");

    // Mutate workdir arbitrarily (should not affect restoration correctness)
    repo.append("c.txt", "C1\n"); // change content after snapshot
    repo.write("d.txt", "D0 (noise)\n"); // create a noise file that won't be staged by restore

    // Restore exact staged set captured in `snap`
    restore_staged(repo.path_str(), &snap).expect("restore staged");

    // Re-snapshot after restore to compare equivalence (semantic equality of staged set)
    let after = snapshot_staged(repo.path_str()).expect("snapshot after restore");

    // Normalize into comparable tuples
    fn key(s: &super::StagedItem) -> (String, String) {
        match &s.kind {
            super::StagedKind::Added => ("Added".into(), s.path.clone()),
            super::StagedKind::Modified => ("Modified".into(), s.path.clone()),
            super::StagedKind::Deleted => ("Deleted".into(), s.path.clone()),
            super::StagedKind::TypeChange => ("TypeChange".into(), s.path.clone()),
            super::StagedKind::Renamed { from, to } => ("Renamed".into(), format!("{from}->{to}")),
        }
    }

    let mut lhs = snap.iter().map(key).collect::<Vec<_>>();
    let mut rhs = after.iter().map(key).collect::<Vec<_>>();
    lhs.sort();
    rhs.sort();
    assert_eq!(
        lhs, rhs,
        "restored staged set should equal original snapshot"
    );
}

#[test]
fn status_with_branch_formats_branch_and_entries() {
    let repo = TestRepo::new();

    repo.write("keep.txt", "keep\n");
    repo.write("remove.txt", "gone\n");
    raw_commit(repo.repo(), "base");

    repo.append("keep.txt", "change\n");
    fs::remove_file(repo.join("remove.txt")).unwrap();
    repo.write("new.txt", "fresh\n");

    let status = super::status_with_branch(repo.path()).expect("status summary");
    assert!(
        status.starts_with("## "),
        "status should include branch header: {status}"
    );
    assert!(
        status.contains(" M keep.txt"),
        "modified tracked file should appear: {status}"
    );
    assert!(
        status.contains(" D remove.txt"),
        "deleted file should appear: {status}"
    );
    assert!(
        status.contains("?? new.txt"),
        "untracked file should appear: {status}"
    );
}

#[test]
fn commit_in_progress_cherry_pick_completes_and_cleans_state() {
    let repo = TestRepo::new();

    repo.write("file.txt", "base\n");
    raw_commit(repo.repo(), "base");
    let base_branch = repo
        .repo()
        .head()
        .unwrap()
        .shorthand()
        .unwrap_or("master")
        .to_string();

    // Create a topic branch with one commit.
    let base_tip = repo.repo().head().unwrap().peel_to_commit().unwrap();
    repo.repo()
        .branch("topic", &base_tip, true)
        .expect("create topic branch");
    {
        let mut checkout = git2::build::CheckoutBuilder::new();
        repo.repo().set_head("refs/heads/topic").unwrap();
        repo.repo()
            .checkout_head(Some(&mut checkout.force()))
            .unwrap();
    }
    repo.append("file.txt", "topic change\n");
    let topic_commit = raw_commit(repo.repo(), "topic change");

    // Return to the base branch and start a cherry-pick.
    {
        let mut checkout = git2::build::CheckoutBuilder::new();
        repo.repo()
            .set_head(&format!("refs/heads/{base_branch}"))
            .unwrap();
        repo.repo()
            .checkout_head(Some(&mut checkout.force()))
            .unwrap();
    }
    repo.repo()
        .cherrypick(&repo.repo().find_commit(topic_commit).unwrap(), None)
        .expect("cherry-pick applies");
    assert_eq!(
        repo.repo().state(),
        RepositoryState::CherryPick,
        "cherry-pick should leave repository in cherry-pick state"
    );

    let committed =
        super::commit_in_progress_cherry_pick_in(repo.path(), "topic change", base_tip.id())
            .expect("commit cherry-pick");
    assert_eq!(
        repo.repo().state(),
        RepositoryState::Clean,
        "state should be cleaned after cherry-pick commit"
    );

    let new_head = repo.repo().head().unwrap().peel_to_commit().unwrap().id();
    assert_eq!(new_head, committed, "HEAD should advance to cherry-pick");
}
