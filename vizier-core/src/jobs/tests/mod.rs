use super::*;
use crate::workflow_template::{
    WorkflowArtifactContract, WorkflowNode, WorkflowNodeKind, WorkflowOutcomeArtifacts,
    WorkflowOutcomeEdges, WorkflowRetryMode, WorkflowTemplate, WorkflowTemplatePolicy,
};
use chrono::TimeZone;
use git2::{BranchType, Signature};
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::Path;
use std::sync::{Arc, Barrier, Mutex, OnceLock};
use tempfile::TempDir;

static AGENT_SHIM_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

struct EnvVarGuard {
    key: &'static str,
    original: Option<OsString>,
}

impl EnvVarGuard {
    fn set_path(key: &'static str, value: &Path) -> Self {
        let original = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.original {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn agent_shim_env_lock() -> &'static Mutex<()> {
    AGENT_SHIM_ENV_LOCK.get_or_init(|| Mutex::new(()))
}

fn create_mock_agent_shims(root: &Path) -> io::Result<PathBuf> {
    let bin_dir = root.join(".vizier/tmp/bin");
    let shim_dir = bin_dir.join("codex");
    fs::create_dir_all(&shim_dir)?;
    let script = b"#!/bin/sh
set -eu
cat >/dev/null
printf '%s\n' '{\"type\":\"item.started\",\"item\":{\"type\":\"reasoning\",\"text\":\"prep\"}}'
printf '%s\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"mock agent response\"}}'
printf 'mock agent running\n' 1>&2
";
    let path = shim_dir.join("agent.sh");
    fs::write(&path, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms)?;
    }
    Ok(bin_dir)
}

fn init_repo(temp: &TempDir) -> Result<Repository, git2::Error> {
    let repo = Repository::init(temp.path())?;
    Ok(repo)
}

fn seed_repo(repo: &Repository) -> Result<Oid, Box<dyn std::error::Error>> {
    let workdir = repo.workdir().ok_or("missing workdir")?;
    let readme = workdir.join("README.md");
    fs::write(&readme, "seed")?;
    let mut index = repo.index()?;
    index.add_path(Path::new("README.md"))?;
    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;
    let sig = Signature::now("vizier", "vizier@example.com")?;
    let oid = repo.commit(Some("HEAD"), &sig, &sig, "seed", &tree, &[])?;
    Ok(oid)
}

fn ensure_branch(repo: &Repository, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    if repo.find_branch(name, BranchType::Local).is_ok() {
        return Ok(());
    }
    let head = repo.head()?.peel_to_commit()?;
    repo.branch(name, &head, false)?;
    Ok(())
}

fn commit_plan_doc(
    repo: &Repository,
    slug: &str,
    branch: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let workdir = repo.workdir().ok_or("missing workdir")?;
    let plan_path = crate::plan::plan_rel_path(slug);
    let full_path = workdir.join(&plan_path);
    if let Some(parent) = full_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&full_path, format!("# plan {}\n", slug))?;

    let mut index = repo.index()?;
    index.add_path(&plan_path)?;
    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;
    let sig = Signature::now("vizier", "vizier@example.com")?;
    let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
    let parents = parent.iter().collect::<Vec<_>>();
    let refname = format!("refs/heads/{branch}");
    repo.commit(
        Some(refname.as_str()),
        &sig,
        &sig,
        "plan doc",
        &tree,
        &parents,
    )?;
    Ok(())
}

fn ensure_artifact_exists(
    repo: &Repository,
    jobs_root: &Path,
    artifact: &JobArtifact,
) -> Result<(), Box<dyn std::error::Error>> {
    match artifact {
        JobArtifact::PlanBranch { branch, .. } | JobArtifact::PlanCommits { branch, .. } => {
            ensure_branch(repo, branch)?;
        }
        JobArtifact::PlanDoc { slug, branch } => {
            commit_plan_doc(repo, slug, branch)?;
        }
        JobArtifact::TargetBranch { name } => {
            ensure_branch(repo, name)?;
        }
        JobArtifact::MergeSentinel { slug } => {
            let path = repo
                .path()
                .join(".vizier/tmp/merge-conflicts")
                .join(format!("{slug}.json"));
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, "{}")?;
        }
        JobArtifact::CommandPatch { job_id } => {
            let path = command_patch_path(jobs_root, job_id);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, "patch")?;
        }
        JobArtifact::Custom { .. } => {
            let project_root = repo.path().parent().ok_or("missing repo root")?;
            write_custom_artifact_markers(
                project_root,
                "fixture-artifact-producer",
                std::slice::from_ref(artifact),
            )?;
        }
    }
    Ok(())
}

fn prompt_invoke_template() -> WorkflowTemplate {
    WorkflowTemplate {
        id: "template.runtime.prompt_invoke".to_string(),
        version: "v1".to_string(),
        params: BTreeMap::new(),
        node_lock_scope_contexts: BTreeMap::new(),
        policy: WorkflowTemplatePolicy::default(),
        artifact_contracts: vec![WorkflowArtifactContract {
            id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
            version: "v1".to_string(),
            schema: None,
        }],
        nodes: vec![
            WorkflowNode {
                id: "resolve_prompt".to_string(),
                name: None,
                kind: WorkflowNodeKind::Builtin,
                uses: "cap.env.builtin.prompt.resolve".to_string(),
                args: BTreeMap::from([("prompt_text".to_string(), "hello world".to_string())]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts {
                    succeeded: vec![JobArtifact::Custom {
                        type_id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
                        key: "approve_prompt".to_string(),
                    }],
                    ..WorkflowOutcomeArtifacts::default()
                },
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges {
                    succeeded: vec!["invoke_agent".to_string()],
                    ..WorkflowOutcomeEdges::default()
                },
            },
            WorkflowNode {
                id: "invoke_agent".to_string(),
                name: None,
                kind: WorkflowNodeKind::Agent,
                uses: "cap.agent.invoke".to_string(),
                args: BTreeMap::new(),
                after: Vec::new(),
                needs: vec![JobArtifact::Custom {
                    type_id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
                    key: "approve_prompt".to_string(),
                }],
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges::default(),
            },
        ],
    }
}

fn worktree_command_template(script: &str) -> WorkflowTemplate {
    WorkflowTemplate {
        id: "template.runtime.worktree_command".to_string(),
        version: "v1".to_string(),
        params: BTreeMap::new(),
        node_lock_scope_contexts: BTreeMap::new(),
        policy: WorkflowTemplatePolicy::default(),
        artifact_contracts: Vec::new(),
        nodes: vec![
            WorkflowNode {
                id: "prepare_worktree".to_string(),
                name: None,
                kind: WorkflowNodeKind::Builtin,
                uses: "cap.env.builtin.worktree.prepare".to_string(),
                args: BTreeMap::from([(
                    "branch".to_string(),
                    "draft/runtime-worktree-command".to_string(),
                )]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges {
                    succeeded: vec!["run_target".to_string()],
                    ..WorkflowOutcomeEdges::default()
                },
            },
            WorkflowNode {
                id: "run_target".to_string(),
                name: None,
                kind: WorkflowNodeKind::Shell,
                uses: "cap.env.shell.command.run".to_string(),
                args: BTreeMap::from([("script".to_string(), script.to_string())]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges::default(),
            },
        ],
    }
}

fn worktree_prompt_invoke_template() -> WorkflowTemplate {
    WorkflowTemplate {
        id: "template.runtime.worktree_prompt_invoke".to_string(),
        version: "v1".to_string(),
        params: BTreeMap::new(),
        node_lock_scope_contexts: BTreeMap::new(),
        policy: WorkflowTemplatePolicy::default(),
        artifact_contracts: vec![WorkflowArtifactContract {
            id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
            version: "v1".to_string(),
            schema: None,
        }],
        nodes: vec![
            WorkflowNode {
                id: "prepare_worktree".to_string(),
                name: None,
                kind: WorkflowNodeKind::Builtin,
                uses: "cap.env.builtin.worktree.prepare".to_string(),
                args: BTreeMap::from([(
                    "branch".to_string(),
                    "draft/runtime-worktree-prompt".to_string(),
                )]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges {
                    succeeded: vec!["resolve_prompt".to_string()],
                    ..WorkflowOutcomeEdges::default()
                },
            },
            WorkflowNode {
                id: "resolve_prompt".to_string(),
                name: None,
                kind: WorkflowNodeKind::Builtin,
                uses: "cap.env.builtin.prompt.resolve".to_string(),
                args: BTreeMap::from([(
                    "prompt_text".to_string(),
                    "worktree chain prompt".to_string(),
                )]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts {
                    succeeded: vec![JobArtifact::Custom {
                        type_id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
                        key: "worktree_chain_prompt".to_string(),
                    }],
                    ..WorkflowOutcomeArtifacts::default()
                },
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges {
                    succeeded: vec!["invoke_agent".to_string()],
                    ..WorkflowOutcomeEdges::default()
                },
            },
            WorkflowNode {
                id: "invoke_agent".to_string(),
                name: None,
                kind: WorkflowNodeKind::Agent,
                uses: "cap.agent.invoke".to_string(),
                args: BTreeMap::new(),
                after: Vec::new(),
                needs: vec![JobArtifact::Custom {
                    type_id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
                    key: "worktree_chain_prompt".to_string(),
                }],
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges::default(),
            },
        ],
    }
}

fn runtime_executor_node(
    node_id: &str,
    job_id: &str,
    uses: &str,
    operation: &str,
    args: BTreeMap<String, String>,
) -> WorkflowRuntimeNodeManifest {
    WorkflowRuntimeNodeManifest {
        node_id: node_id.to_string(),
        name: None,
        job_id: job_id.to_string(),
        uses: uses.to_string(),
        kind: WorkflowNodeKind::Builtin,
        args,
        executor_operation: Some(operation.to_string()),
        control_policy: None,
        gates: Vec::new(),
        retry: crate::workflow_template::WorkflowRetryPolicy::default(),
        routes: WorkflowRouteTargets::default(),
        artifacts_by_outcome: WorkflowOutcomeArtifactsByOutcome::default(),
    }
}

fn runtime_control_node(
    node_id: &str,
    job_id: &str,
    uses: &str,
    policy: &str,
    args: BTreeMap<String, String>,
) -> WorkflowRuntimeNodeManifest {
    WorkflowRuntimeNodeManifest {
        node_id: node_id.to_string(),
        name: None,
        job_id: job_id.to_string(),
        uses: uses.to_string(),
        kind: WorkflowNodeKind::Gate,
        args,
        executor_operation: None,
        control_policy: Some(policy.to_string()),
        gates: Vec::new(),
        retry: crate::workflow_template::WorkflowRetryPolicy::default(),
        routes: WorkflowRouteTargets::default(),
        artifacts_by_outcome: WorkflowOutcomeArtifactsByOutcome::default(),
    }
}

struct SucceededCompletionPauseGuard;

impl Drop for SucceededCompletionPauseGuard {
    fn drop(&mut self) {
        set_succeeded_completion_pause_barrier(None);
    }
}

fn install_succeeded_completion_pause(barrier: Arc<Barrier>) -> SucceededCompletionPauseGuard {
    set_succeeded_completion_pause_barrier(Some(barrier));
    SucceededCompletionPauseGuard
}

fn git_status(project_root: &Path, args: &[&str]) -> Result<(), String> {
    match args {
        ["add", "-A"] => crate::vcs::stage_all_in(project_root).map_err(|err| err.to_string()),
        ["checkout", "-b", branch] => {
            if !crate::vcs::branch_exists_in(project_root, branch).map_err(|err| err.to_string())? {
                crate::vcs::create_branch_from_head_in(project_root, branch)
                    .map_err(|err| err.to_string())?;
            }
            crate::vcs::checkout_branch_in(project_root, branch).map_err(|err| err.to_string())
        }
        ["checkout", "--", path] => {
            let repo = Repository::open(project_root).map_err(|err| err.to_string())?;
            let mut checkout = git2::build::CheckoutBuilder::new();
            checkout.path(path).force();
            repo.checkout_head(Some(&mut checkout))
                .map_err(|err| err.to_string())
        }
        ["checkout", branch] => {
            crate::vcs::checkout_branch_in(project_root, branch).map_err(|err| err.to_string())
        }
        ["merge", "--no-ff", source_branch] => {
            let merge = crate::vcs::prepare_merge_in(project_root, source_branch)
                .map_err(|err| err.to_string())?;
            match merge {
                crate::vcs::MergePreparation::Ready(ready) => crate::vcs::commit_ready_merge_in(
                    project_root,
                    &format!("Merge branch `{source_branch}`"),
                    ready,
                )
                .map(|_| ())
                .map_err(|err| err.to_string()),
                crate::vcs::MergePreparation::Conflicted(_conflict) => {
                    Err("merge conflict".to_string())
                }
            }
        }
        ["-c", _user_name, "-c", _email, "commit", "-m", message] => {
            crate::vcs::commit_staged_in(project_root, message, false)
                .map(|_| ())
                .map_err(|err| err.to_string())
        }
        _ => Err(format!("unsupported git_status args: {:?}", args)),
    }
}

fn git_output(project_root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    match args {
        ["log", "-1", "--pretty=%B"] => {
            let repo = Repository::open(project_root).map_err(|err| err.to_string())?;
            let commit = repo
                .head()
                .and_then(|head| head.peel_to_commit())
                .map_err(|err| err.to_string())?;
            Ok(commit.message().unwrap_or_default().as_bytes().to_vec())
        }
        ["diff", "--binary", "HEAD"] => {
            crate::vcs::diff_binary_against_head_in(project_root).map_err(|err| err.to_string())
        }
        ["diff", "--cached", "--name-only"] => {
            let staged = crate::vcs::snapshot_staged(
                project_root
                    .to_str()
                    .ok_or_else(|| "project root path is not valid utf-8".to_string())?,
            )
            .map_err(|err| err.to_string())?;
            let mut names = staged.into_iter().map(|item| item.path).collect::<Vec<_>>();
            names.sort();
            names.dedup();
            Ok(names.join("\n").into_bytes())
        }
        _ => Err(format!("unsupported git_output args: {:?}", args)),
    }
}

fn git_commit_all(project_root: &Path, message: &str) {
    let add = git_status(project_root, &["add", "-A"]);
    assert!(add.is_ok(), "git add failed: {add:?}");
    let commit = git_status(
        project_root,
        &[
            "-c",
            "user.name=vizier",
            "-c",
            "user.email=vizier@example.com",
            "commit",
            "-m",
            message,
        ],
    );
    assert!(commit.is_ok(), "git commit failed: {commit:?}");
}

struct ConflictGateFixture {
    _temp: TempDir,
    project_root: PathBuf,
    record: JobRecord,
    sentinel: PathBuf,
}

fn prepare_conflict_gate_fixture(
    slug: &str,
    source_branch: &str,
    conflict_path: &str,
) -> ConflictGateFixture {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path().to_path_buf();
    let jobs_root = project_root.join(".vizier/jobs");
    let target = current_branch_name(&project_root).expect("target branch");

    fs::write(project_root.join(conflict_path), "base\n").expect("write base");
    git_commit_all(&project_root, "base conflict");

    let checkout = git_status(&project_root, &["checkout", "-b", source_branch]);
    assert!(checkout.is_ok(), "create draft branch: {checkout:?}");
    fs::write(project_root.join(conflict_path), "draft\n").expect("write draft");
    git_commit_all(&project_root, "draft conflict");

    let checkout_target = git_status(&project_root, &["checkout", &target]);
    assert!(
        checkout_target.is_ok(),
        "checkout target: {checkout_target:?}"
    );
    fs::write(project_root.join(conflict_path), "target\n").expect("write target");
    git_commit_all(&project_root, "target conflict");

    let merge = git_status(&project_root, &["merge", "--no-ff", source_branch]);
    assert!(
        merge.is_err(),
        "expected deliberate merge conflict for gate coverage"
    );

    let sentinel = project_root
        .join(".vizier/tmp/merge-conflicts")
        .join(format!("{slug}.json"));
    if let Some(parent) = sentinel.parent() {
        fs::create_dir_all(parent).expect("create sentinel dir");
    }
    fs::write(&sentinel, "{}").expect("write sentinel");

    enqueue_job(
        &project_root,
        &jobs_root,
        "job-conflict-gate",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        Some(JobMetadata {
            plan: Some(slug.to_string()),
            branch: Some(source_branch.to_string()),
            target: Some(target),
            ..JobMetadata::default()
        }),
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");
    let record = read_record(&jobs_root, "job-conflict-gate").expect("record");

    ConflictGateFixture {
        _temp: temp,
        project_root,
        record,
        sentinel,
    }
}

#[test]
fn follow_poll_delay_uses_short_backoff_and_resets_on_activity() {
    let mut idle_polls = 0u32;

    assert_eq!(
        follow_poll_delay(false, &mut idle_polls),
        StdDuration::from_millis(40)
    );
    assert_eq!(
        follow_poll_delay(false, &mut idle_polls),
        StdDuration::from_millis(80)
    );
    assert_eq!(
        follow_poll_delay(false, &mut idle_polls),
        StdDuration::from_millis(160)
    );
    assert_eq!(
        follow_poll_delay(false, &mut idle_polls),
        StdDuration::from_millis(240)
    );
    assert_eq!(
        follow_poll_delay(false, &mut idle_polls),
        StdDuration::from_millis(240)
    );

    assert_eq!(
        follow_poll_delay(true, &mut idle_polls),
        StdDuration::from_millis(15)
    );
    assert_eq!(
        follow_poll_delay(false, &mut idle_polls),
        StdDuration::from_millis(40)
    );
}

#[test]
fn latest_job_log_line_returns_stdout_when_only_stdout_has_content() {
    let temp = TempDir::new().expect("temp dir");
    let jobs_root = temp.path().join(".vizier/jobs");
    let paths = paths_for(&jobs_root, "job-stdout-only");
    fs::create_dir_all(&paths.job_dir).expect("create job dir");
    fs::write(&paths.stdout_path, "first line\nlatest stdout\n").expect("write stdout log");

    let latest = latest_job_log_line(&jobs_root, "job-stdout-only", 8 * 1024)
        .expect("resolve latest log line")
        .expect("expected stdout line");
    assert_eq!(latest.stream, LatestLogStream::Stdout);
    assert_eq!(latest.line, "latest stdout");
}

#[test]
fn latest_job_log_line_returns_stderr_when_only_stderr_has_content() {
    let temp = TempDir::new().expect("temp dir");
    let jobs_root = temp.path().join(".vizier/jobs");
    let paths = paths_for(&jobs_root, "job-stderr-only");
    fs::create_dir_all(&paths.job_dir).expect("create job dir");
    fs::write(&paths.stderr_path, "latest stderr\n").expect("write stderr log");

    let latest = latest_job_log_line(&jobs_root, "job-stderr-only", 8 * 1024)
        .expect("resolve latest log line")
        .expect("expected stderr line");
    assert_eq!(latest.stream, LatestLogStream::Stderr);
    assert_eq!(latest.line, "latest stderr");
}

#[test]
fn latest_job_log_line_prefers_newer_stream_when_both_have_content() {
    let temp = TempDir::new().expect("temp dir");
    let jobs_root = temp.path().join(".vizier/jobs");
    let paths = paths_for(&jobs_root, "job-both-streams");
    fs::create_dir_all(&paths.job_dir).expect("create job dir");

    fs::write(&paths.stdout_path, "old stdout\n").expect("write stdout log");
    thread::sleep(StdDuration::from_millis(20));
    fs::write(&paths.stderr_path, "new stderr\n").expect("write stderr log");

    let latest = latest_job_log_line(&jobs_root, "job-both-streams", 8 * 1024)
        .expect("resolve latest log line")
        .expect("expected latest line");
    assert_eq!(latest.stream, LatestLogStream::Stderr);
    assert_eq!(latest.line, "new stderr");
}

#[test]
fn latest_job_log_line_returns_none_for_missing_or_empty_logs() {
    let temp = TempDir::new().expect("temp dir");
    let jobs_root = temp.path().join(".vizier/jobs");
    let paths = paths_for(&jobs_root, "job-empty");
    fs::create_dir_all(&paths.job_dir).expect("create job dir");
    fs::write(&paths.stdout_path, "\n\n").expect("write stdout log");
    fs::write(&paths.stderr_path, "   \n").expect("write stderr log");

    let latest =
        latest_job_log_line(&jobs_root, "job-empty", 8 * 1024).expect("resolve latest line");
    assert!(latest.is_none(), "expected no latest line for empty logs");

    let missing =
        latest_job_log_line(&jobs_root, "job-missing", 8 * 1024).expect("resolve missing");
    assert!(
        missing.is_none(),
        "expected no latest line for missing logs"
    );
}

#[test]
fn persist_record_handles_concurrent_writers_without_tmp_collisions() {
    let temp = TempDir::new().expect("temp dir");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");
    fs::create_dir_all(&jobs_root).expect("create jobs root");

    enqueue_job(
        project_root,
        &jobs_root,
        "race-job",
        &["save".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        None,
    )
    .expect("enqueue race job");

    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();

    for worker in 0..2u32 {
        let jobs_root = jobs_root.clone();
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || -> Result<(), String> {
            barrier.wait();
            for attempt in 0..200u32 {
                let paths = paths_for(&jobs_root, "race-job");
                let mut record = load_record(&paths).map_err(|err| err.to_string())?;
                let metadata = record.metadata.get_or_insert_with(JobMetadata::default);
                metadata.patch_index = Some((worker * 1000 + attempt) as usize);
                persist_record(&paths, &record).map_err(|err| err.to_string())?;
            }
            Ok(())
        }));
    }

    barrier.wait();
    for handle in handles {
        handle
            .join()
            .expect("concurrent writer should not panic")
            .expect("concurrent writer should not fail");
    }

    let record = read_record(&jobs_root, "race-job").expect("read final race record");
    assert_eq!(record.id, "race-job");
}

fn write_job_with_status(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
    status: JobStatus,
    schedule: JobSchedule,
    child_args: &[String],
) -> Result<JobRecord, Box<dyn std::error::Error>> {
    enqueue_job(
        project_root,
        jobs_root,
        job_id,
        child_args,
        &["vizier".to_string()],
        None,
        None,
        Some(schedule.clone()),
    )?;
    let paths = paths_for(jobs_root, job_id);
    let mut record = load_record(&paths)?;
    record.status = status;
    record.schedule = Some(schedule);
    persist_record(&paths, &record)?;
    Ok(record)
}

fn update_job_record<F: FnOnce(&mut JobRecord)>(
    jobs_root: &Path,
    job_id: &str,
    updater: F,
) -> Result<JobRecord, Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    let mut record = load_record(&paths)?;
    updater(&mut record);
    persist_record(&paths, &record)?;
    Ok(record)
}

#[derive(Clone, Copy)]
enum ArtifactKind {
    PlanBranch,
    PlanDoc,
    PlanCommits,
    TargetBranch,
    MergeSentinel,
    CommandPatch,
    Custom,
}

fn artifact_for(kind: ArtifactKind, suffix: &str) -> JobArtifact {
    match kind {
        ArtifactKind::PlanBranch => JobArtifact::PlanBranch {
            slug: format!("plan-{suffix}"),
            branch: format!("draft/plan-{suffix}"),
        },
        ArtifactKind::PlanDoc => JobArtifact::PlanDoc {
            slug: format!("doc-{suffix}"),
            branch: format!("draft/doc-{suffix}"),
        },
        ArtifactKind::PlanCommits => JobArtifact::PlanCommits {
            slug: format!("commits-{suffix}"),
            branch: format!("draft/commits-{suffix}"),
        },
        ArtifactKind::TargetBranch => JobArtifact::TargetBranch {
            name: format!("target-{suffix}"),
        },
        ArtifactKind::MergeSentinel => JobArtifact::MergeSentinel {
            slug: format!("merge-{suffix}"),
        },
        ArtifactKind::CommandPatch => JobArtifact::CommandPatch {
            job_id: format!("job-{suffix}"),
        },
        ArtifactKind::Custom => JobArtifact::Custom {
            type_id: "acme.execution".to_string(),
            key: format!("key-{suffix}"),
        },
    }
}

fn make_record(
    job_id: &str,
    status: JobStatus,
    created_at: DateTime<Utc>,
    schedule: Option<JobSchedule>,
) -> JobRecord {
    JobRecord {
        id: job_id.to_string(),
        status,
        command: Vec::new(),
        child_args: Vec::new(),
        created_at,
        started_at: None,
        finished_at: None,
        pid: None,
        exit_code: None,
        stdout_path: String::new(),
        stderr_path: String::new(),
        session_path: None,
        outcome_path: None,
        metadata: None,
        config_snapshot: None,
        schedule,
    }
}

fn after_dependency(job_id: &str) -> JobAfterDependency {
    JobAfterDependency {
        job_id: job_id.to_string(),
        policy: AfterPolicy::Success,
    }
}

#[test]
fn resolve_after_dependencies_rejects_unknown_job_id() {
    let temp = TempDir::new().expect("temp dir");
    let jobs_root = temp.path().join(".vizier/jobs");
    fs::create_dir_all(&jobs_root).expect("jobs root");

    let err =
        resolve_after_dependencies_for_enqueue(&jobs_root, "job-new", &["missing-job".to_string()])
            .expect_err("expected unknown dependency to fail");
    assert!(
        err.to_string()
            .contains("unknown --after job id: missing-job"),
        "unexpected error: {err}"
    );
}

#[test]
fn resolve_after_dependencies_rejects_self_dependency() {
    let temp = TempDir::new().expect("temp dir");
    let jobs_root = temp.path().join(".vizier/jobs");
    fs::create_dir_all(&jobs_root).expect("jobs root");

    let err =
        resolve_after_dependencies_for_enqueue(&jobs_root, "job-self", &["job-self".to_string()])
            .expect_err("expected self dependency to fail");
    assert!(
        err.to_string()
            .contains("invalid --after self dependency: job-self"),
        "unexpected error: {err}"
    );
}

#[test]
fn resolve_after_dependencies_dedupes_repeated_ids() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-a",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        None,
    )
    .expect("enqueue job-a");

    let after = resolve_after_dependencies_for_enqueue(
        &jobs_root,
        "job-new",
        &["job-a".to_string(), "job-a".to_string()],
    )
    .expect("resolve after");
    assert_eq!(
        after,
        vec![JobAfterDependency {
            job_id: "job-a".to_string(),
            policy: AfterPolicy::Success,
        }]
    );
}

#[test]
fn resolve_after_dependencies_rejects_cycles() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-a",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        Some(JobSchedule {
            after: vec![after_dependency("job-b")],
            ..JobSchedule::default()
        }),
    )
    .expect("enqueue job-a");
    enqueue_job(
        project_root,
        &jobs_root,
        "job-b",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        Some(JobSchedule {
            after: vec![after_dependency("job-c")],
            ..JobSchedule::default()
        }),
    )
    .expect("enqueue job-b");

    let err = resolve_after_dependencies_for_enqueue(&jobs_root, "job-c", &["job-a".to_string()])
        .expect_err("expected cycle to fail");
    assert!(
        err.to_string().contains("invalid --after cycle:"),
        "unexpected error: {err}"
    );
    assert!(
        err.to_string().contains("job-a")
            && err.to_string().contains("job-b")
            && err.to_string().contains("job-c"),
        "expected cycle path in error: {err}"
    );
}

#[test]
fn resolve_after_dependencies_rejects_malformed_records() {
    let temp = TempDir::new().expect("temp dir");
    let jobs_root = temp.path().join(".vizier/jobs");
    let job_dir = jobs_root.join("job-bad");
    fs::create_dir_all(&job_dir).expect("create bad job dir");
    fs::write(job_dir.join("job.json"), "{ not json }").expect("write malformed json");

    let err =
        resolve_after_dependencies_for_enqueue(&jobs_root, "job-new", &["job-bad".to_string()])
            .expect_err("expected malformed record to fail");
    assert!(
        err.to_string()
            .contains("cannot read job record for --after job-bad"),
        "unexpected error: {err}"
    );
}

#[test]
fn scheduler_lock_busy_returns_error() {
    let temp = TempDir::new().expect("temp dir");
    let jobs_root = temp.path().join(".vizier/jobs");
    fs::create_dir_all(&jobs_root).expect("create jobs root");
    fs::write(jobs_root.join("scheduler.lock"), "locked").expect("write lock");

    let err = SchedulerLock::acquire(&jobs_root)
        .err()
        .expect("expected scheduler lock error");
    assert!(
        err.to_string().contains("scheduler is busy"),
        "unexpected error: {err}"
    );
}

#[test]
fn scheduler_tick_marks_blocked_by_dependency() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    let schedule = JobSchedule {
        dependencies: vec![JobDependency {
            artifact: JobArtifact::PlanDoc {
                slug: "alpha".to_string(),
                branch: "draft/alpha".to_string(),
            },
        }],
        ..JobSchedule::default()
    };

    enqueue_job(
        project_root,
        &jobs_root,
        "blocked-job",
        &["save".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        Some(schedule),
    )
    .expect("enqueue job");

    let binary = std::env::current_exe().expect("current exe");
    scheduler_tick(project_root, &jobs_root, &binary).expect("scheduler tick");

    let record = read_record(&jobs_root, "blocked-job").expect("read record");
    assert_eq!(record.status, JobStatus::BlockedByDependency);
    let wait_reason = record
        .schedule
        .as_ref()
        .and_then(|sched| sched.wait_reason.as_ref())
        .expect("wait reason");
    assert_eq!(wait_reason.kind, JobWaitKind::Dependencies);
    let detail = wait_reason.detail.as_deref().unwrap_or("");
    assert!(
        detail.contains("missing plan_doc:alpha (draft/alpha)"),
        "unexpected wait detail: {detail}"
    );
    let waited_on = record
        .schedule
        .as_ref()
        .map(|sched| sched.waited_on.clone())
        .unwrap_or_default();
    assert!(
        waited_on.contains(&JobWaitKind::Dependencies),
        "expected waited_on to include dependencies"
    );
}

#[test]
fn scheduler_tick_waits_for_missing_producer_when_policy_is_wait() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    let schedule = JobSchedule {
        dependencies: vec![JobDependency {
            artifact: JobArtifact::PlanDoc {
                slug: "alpha".to_string(),
                branch: "draft/alpha".to_string(),
            },
        }],
        dependency_policy: JobDependenciesPolicy {
            missing_producer: MissingProducerPolicy::Wait,
        },
        ..JobSchedule::default()
    };

    enqueue_job(
        project_root,
        &jobs_root,
        "waiting-job",
        &["save".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        Some(schedule),
    )
    .expect("enqueue job");

    let binary = std::env::current_exe().expect("current exe");
    scheduler_tick(project_root, &jobs_root, &binary).expect("scheduler tick");

    let record = read_record(&jobs_root, "waiting-job").expect("read record");
    assert_eq!(record.status, JobStatus::WaitingOnDeps);
    let wait_reason = record
        .schedule
        .as_ref()
        .and_then(|sched| sched.wait_reason.as_ref())
        .expect("wait reason");
    assert_eq!(wait_reason.kind, JobWaitKind::Dependencies);
    let detail = wait_reason.detail.as_deref().unwrap_or("");
    assert!(
        detail.contains("awaiting producer for plan_doc:alpha (draft/alpha)"),
        "unexpected wait detail: {detail}"
    );
}

#[test]
fn scheduler_tick_waits_on_after_dependency() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "dep-running",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        None,
    )
    .expect("enqueue dep");
    update_job_record(&jobs_root, "dep-running", |record| {
        record.status = JobStatus::WaitingOnDeps;
    })
    .expect("set dep status");

    enqueue_job(
        project_root,
        &jobs_root,
        "dependent",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        Some(JobSchedule {
            after: vec![after_dependency("dep-running")],
            ..JobSchedule::default()
        }),
    )
    .expect("enqueue dependent");

    let binary = std::env::current_exe().expect("current exe");
    scheduler_tick(project_root, &jobs_root, &binary).expect("tick");

    let record = read_record(&jobs_root, "dependent").expect("read dependent");
    assert_eq!(record.status, JobStatus::WaitingOnDeps);
    let wait = record
        .schedule
        .as_ref()
        .and_then(|schedule| schedule.wait_reason.as_ref())
        .and_then(|reason| reason.detail.as_deref());
    assert_eq!(wait, Some("waiting on job dep-running"));
}

#[test]
fn scheduler_tick_reconciles_running_job_with_missing_pid() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "stale-missing-pid",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        None,
    )
    .expect("enqueue stale job");
    update_job_record(&jobs_root, "stale-missing-pid", |record| {
        record.status = JobStatus::Running;
        record.started_at = Some(Utc::now());
        record.pid = None;
    })
    .expect("mark stale running");

    let binary = std::env::current_exe().expect("current exe");
    scheduler_tick(project_root, &jobs_root, &binary).expect("tick");

    let record = read_record(&jobs_root, "stale-missing-pid").expect("read stale record");
    assert_eq!(record.status, JobStatus::Failed);
    assert_eq!(record.exit_code, Some(1));
    assert!(record.finished_at.is_some());
    let metadata = record.metadata.as_ref().expect("metadata");
    assert_eq!(
        metadata.process_liveness_state,
        Some(ProcessLivenessState::StaleMissingPid)
    );
    assert!(metadata.process_liveness_checked_at.is_some());
    let reason = metadata
        .process_liveness_failure_reason
        .as_deref()
        .unwrap_or("");
    assert!(
        reason.contains("missing pid"),
        "expected missing pid reason, got: {reason}"
    );
}

#[test]
fn scheduler_tick_reconciles_dead_running_producer_before_dependency_facts() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");
    let artifact = JobArtifact::Custom {
        type_id: "acme.execution".to_string(),
        key: "stale-producer".to_string(),
    };

    enqueue_job(
        project_root,
        &jobs_root,
        "stale-producer",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        Some(JobSchedule {
            artifacts: vec![artifact.clone()],
            ..JobSchedule::default()
        }),
    )
    .expect("enqueue producer");
    update_job_record(&jobs_root, "stale-producer", |record| {
        record.status = JobStatus::Running;
        record.started_at = Some(Utc::now());
        record.pid = Some(999_999);
    })
    .expect("mark producer stale running");

    enqueue_job(
        project_root,
        &jobs_root,
        "artifact-consumer",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        Some(JobSchedule {
            dependencies: vec![JobDependency {
                artifact: artifact.clone(),
            }],
            ..JobSchedule::default()
        }),
    )
    .expect("enqueue consumer");

    let binary = std::env::current_exe().expect("current exe");
    scheduler_tick(project_root, &jobs_root, &binary).expect("tick");

    let producer = read_record(&jobs_root, "stale-producer").expect("producer record");
    assert_eq!(producer.status, JobStatus::Failed);
    let producer_meta = producer.metadata.as_ref().expect("producer metadata");
    assert_eq!(
        producer_meta.process_liveness_state,
        Some(ProcessLivenessState::StaleNotRunning)
    );

    let consumer = read_record(&jobs_root, "artifact-consumer").expect("consumer record");
    assert_eq!(consumer.status, JobStatus::BlockedByDependency);
    let wait = consumer
        .schedule
        .as_ref()
        .and_then(|schedule| schedule.wait_reason.as_ref())
        .and_then(|reason| reason.detail.clone())
        .unwrap_or_default();
    assert!(
        wait.contains("dependency failed for custom:acme.execution:stale-producer"),
        "unexpected dependency wait detail: {wait}"
    );
}

#[test]
fn scheduler_tick_reconciles_stale_workflow_node_and_applies_failed_retry_route() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    let template = WorkflowTemplate {
        id: "template.reconcile.stale.retry".to_string(),
        version: "v1".to_string(),
        params: BTreeMap::new(),
        node_lock_scope_contexts: BTreeMap::new(),
        policy: WorkflowTemplatePolicy::default(),
        artifact_contracts: Vec::new(),
        nodes: vec![
            WorkflowNode {
                id: "root".to_string(),
                name: None,
                kind: WorkflowNodeKind::Shell,
                uses: "cap.env.shell.command.run".to_string(),
                args: BTreeMap::from([("script".to_string(), "echo root".to_string())]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges {
                    failed: vec!["retry_target".to_string()],
                    ..WorkflowOutcomeEdges::default()
                },
            },
            WorkflowNode {
                id: "retry_target".to_string(),
                name: None,
                kind: WorkflowNodeKind::Shell,
                uses: "cap.env.shell.command.run".to_string(),
                args: BTreeMap::from([("script".to_string(), "echo retry".to_string())]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges::default(),
            },
        ],
    };

    let enqueue = enqueue_workflow_run(
        project_root,
        &jobs_root,
        "run-stale-retry",
        "template.reconcile.stale.retry@v1",
        &template,
        &["vizier".to_string(), "__workflow-node".to_string()],
        None,
    )
    .expect("enqueue workflow");
    let root_job = enqueue.job_ids.get("root").expect("root job id").clone();
    let target_job = enqueue
        .job_ids
        .get("retry_target")
        .expect("target job id")
        .clone();

    update_job_record(&jobs_root, &root_job, |record| {
        record.status = JobStatus::Running;
        record.started_at = Some(Utc::now());
        record.pid = Some(999_999);
    })
    .expect("mark root stale running");

    update_job_record(&jobs_root, &target_job, |record| {
        record.status = JobStatus::Failed;
        record.child_args.clear();
        record.started_at = Some(Utc::now());
        record.finished_at = Some(Utc::now());
        record.exit_code = Some(1);
    })
    .expect("mark target failed");

    let before_attempt = read_record(&jobs_root, &target_job)
        .expect("target before")
        .metadata
        .as_ref()
        .and_then(|meta| meta.workflow_node_attempt)
        .unwrap_or(1);

    let binary = std::env::current_exe().expect("current exe");
    scheduler_tick(project_root, &jobs_root, &binary).expect("tick");

    let root = read_record(&jobs_root, &root_job).expect("root record");
    assert_eq!(root.status, JobStatus::Failed);
    let root_meta = root.metadata.as_ref().expect("root metadata");
    assert_eq!(root_meta.workflow_node_outcome.as_deref(), Some("failed"));
    assert_eq!(
        root_meta.process_liveness_state,
        Some(ProcessLivenessState::StaleNotRunning)
    );

    let target = read_record(&jobs_root, &target_job).expect("target record");
    let after_attempt = target
        .metadata
        .as_ref()
        .and_then(|meta| meta.workflow_node_attempt)
        .unwrap_or(before_attempt);
    assert!(
        after_attempt > before_attempt,
        "expected failed-route retry to increment attempt: before={} after={} target={}",
        before_attempt,
        after_attempt,
        target.id
    );
}

#[test]
fn scheduler_tick_reconciles_legacy_running_record_without_liveness_metadata_fields() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");
    let paths = paths_for(&jobs_root, "legacy-running");
    fs::create_dir_all(&paths.job_dir).expect("create legacy job dir");
    fs::write(&paths.stdout_path, "").expect("write stdout");
    fs::write(&paths.stderr_path, "").expect("write stderr");

    let legacy = serde_json::json!({
        "id": "legacy-running",
        "status": "running",
        "command": ["vizier", "save", "legacy"],
        "created_at": "2026-02-20T00:00:00Z",
        "started_at": "2026-02-20T00:00:01Z",
        "finished_at": null,
        "pid": null,
        "exit_code": null,
        "stdout_path": ".vizier/jobs/legacy-running/stdout.log",
        "stderr_path": ".vizier/jobs/legacy-running/stderr.log",
        "session_path": null,
        "outcome_path": null,
        "metadata": {
            "command_alias": "legacy-save"
        },
        "config_snapshot": null
    });
    fs::write(
        &paths.record_path,
        serde_json::to_string_pretty(&legacy).expect("serialize legacy record"),
    )
    .expect("write legacy record");

    let binary = std::env::current_exe().expect("current exe");
    scheduler_tick(project_root, &jobs_root, &binary).expect("tick");

    let record = read_record(&jobs_root, "legacy-running").expect("legacy record");
    assert_eq!(record.status, JobStatus::Failed);
    let metadata = record.metadata.as_ref().expect("legacy metadata");
    assert_eq!(metadata.command_alias.as_deref(), Some("legacy-save"));
    assert_eq!(
        metadata.process_liveness_state,
        Some(ProcessLivenessState::StaleMissingPid)
    );
}

#[test]
fn scheduler_tick_blocks_on_after_data_errors() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    let bad_dir = jobs_root.join("bad-predecessor");
    fs::create_dir_all(&bad_dir).expect("bad dir");
    fs::write(bad_dir.join("job.json"), "{ invalid }").expect("malformed job json");

    enqueue_job(
        project_root,
        &jobs_root,
        "dependent",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        Some(JobSchedule {
            after: vec![after_dependency("bad-predecessor")],
            ..JobSchedule::default()
        }),
    )
    .expect("enqueue dependent");

    let binary = std::env::current_exe().expect("current exe");
    scheduler_tick(project_root, &jobs_root, &binary).expect("tick");

    let record = read_record(&jobs_root, "dependent").expect("read dependent");
    assert_eq!(record.status, JobStatus::BlockedByDependency);
    let wait = record
        .schedule
        .as_ref()
        .and_then(|schedule| schedule.wait_reason.as_ref())
        .and_then(|reason| reason.detail.clone())
        .unwrap_or_default();
    assert!(
        wait.contains("scheduler data error for job dependency bad-predecessor"),
        "unexpected wait detail: {wait}"
    );
}

#[test]
fn scheduler_tick_errors_on_missing_binary() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "spawn-failure",
        &["save".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        None,
    )
    .expect("enqueue job");

    let missing_binary = project_root.join("does-not-exist");
    let result = scheduler_tick(project_root, &jobs_root, &missing_binary);
    assert!(result.is_err(), "expected scheduler tick to fail");
}

#[cfg(unix)]
#[test]
fn scheduler_tick_errors_on_persist_failure() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "persist-failure",
        &[],
        &["vizier".to_string()],
        None,
        None,
        None,
    )
    .expect("enqueue job");

    let paths = paths_for(&jobs_root, "persist-failure");
    let original = fs::metadata(&paths.job_dir)
        .expect("metadata")
        .permissions();
    let mut read_only = original.clone();
    read_only.set_mode(0o555);
    fs::set_permissions(&paths.job_dir, read_only).expect("set perms");

    let binary = project_root.join("missing-binary");
    let result = scheduler_tick(project_root, &jobs_root, &binary);

    fs::set_permissions(&paths.job_dir, original).expect("restore perms");
    assert!(result.is_err(), "expected scheduler tick to fail");
}

#[test]
fn scheduler_tick_handles_graph_shapes() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");
    fs::create_dir_all(&jobs_root).expect("jobs root");

    let artifact_a = JobArtifact::CommandPatch {
        job_id: "a-artifact".to_string(),
    };
    let artifact_b = JobArtifact::CommandPatch {
        job_id: "b-artifact".to_string(),
    };
    write_job_with_status(
        project_root,
        &jobs_root,
        "job-a",
        JobStatus::WaitingOnDeps,
        JobSchedule {
            artifacts: vec![artifact_a.clone()],
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("job a");
    write_job_with_status(
        project_root,
        &jobs_root,
        "job-b",
        JobStatus::Queued,
        JobSchedule {
            dependencies: vec![JobDependency {
                artifact: artifact_a.clone(),
            }],
            artifacts: vec![artifact_b.clone()],
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("job b");
    write_job_with_status(
        project_root,
        &jobs_root,
        "job-c",
        JobStatus::Queued,
        JobSchedule {
            dependencies: vec![JobDependency {
                artifact: artifact_b.clone(),
            }],
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("job c");

    let fan_artifact = JobArtifact::CommandPatch {
        job_id: "fan-root".to_string(),
    };
    write_job_with_status(
        project_root,
        &jobs_root,
        "job-fan-root",
        JobStatus::WaitingOnDeps,
        JobSchedule {
            artifacts: vec![fan_artifact.clone()],
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("fan root");
    for job_id in ["job-fan-left", "job-fan-right"] {
        write_job_with_status(
            project_root,
            &jobs_root,
            job_id,
            JobStatus::Queued,
            JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: fan_artifact.clone(),
                }],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("fan job");
    }

    let fan_in_left = JobArtifact::CommandPatch {
        job_id: "fanin-left".to_string(),
    };
    let fan_in_right = JobArtifact::CommandPatch {
        job_id: "fanin-right".to_string(),
    };
    write_job_with_status(
        project_root,
        &jobs_root,
        "job-fanin-left",
        JobStatus::WaitingOnDeps,
        JobSchedule {
            artifacts: vec![fan_in_left.clone()],
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("fanin left");
    write_job_with_status(
        project_root,
        &jobs_root,
        "job-fanin-right",
        JobStatus::WaitingOnDeps,
        JobSchedule {
            artifacts: vec![fan_in_right.clone()],
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("fanin right");
    write_job_with_status(
        project_root,
        &jobs_root,
        "job-fanin",
        JobStatus::Queued,
        JobSchedule {
            dependencies: vec![
                JobDependency {
                    artifact: fan_in_left.clone(),
                },
                JobDependency {
                    artifact: fan_in_right.clone(),
                },
            ],
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("fanin");

    let diamond_root = JobArtifact::CommandPatch {
        job_id: "diamond-root".to_string(),
    };
    let diamond_left = JobArtifact::CommandPatch {
        job_id: "diamond-left".to_string(),
    };
    let diamond_right = JobArtifact::CommandPatch {
        job_id: "diamond-right".to_string(),
    };
    write_job_with_status(
        project_root,
        &jobs_root,
        "job-diamond-root",
        JobStatus::WaitingOnDeps,
        JobSchedule {
            artifacts: vec![diamond_root.clone()],
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("diamond root");
    write_job_with_status(
        project_root,
        &jobs_root,
        "job-diamond-left",
        JobStatus::Queued,
        JobSchedule {
            dependencies: vec![JobDependency {
                artifact: diamond_root.clone(),
            }],
            artifacts: vec![diamond_left.clone()],
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("diamond left");
    write_job_with_status(
        project_root,
        &jobs_root,
        "job-diamond-right",
        JobStatus::Queued,
        JobSchedule {
            dependencies: vec![JobDependency {
                artifact: diamond_root.clone(),
            }],
            artifacts: vec![diamond_right.clone()],
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("diamond right");
    write_job_with_status(
        project_root,
        &jobs_root,
        "job-diamond-leaf",
        JobStatus::Queued,
        JobSchedule {
            dependencies: vec![
                JobDependency {
                    artifact: diamond_left.clone(),
                },
                JobDependency {
                    artifact: diamond_right.clone(),
                },
            ],
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("diamond leaf");

    let disjoint_artifact = JobArtifact::CommandPatch {
        job_id: "disjoint-root".to_string(),
    };
    write_job_with_status(
        project_root,
        &jobs_root,
        "job-disjoint-root",
        JobStatus::WaitingOnDeps,
        JobSchedule {
            artifacts: vec![disjoint_artifact.clone()],
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("disjoint root");
    write_job_with_status(
        project_root,
        &jobs_root,
        "job-disjoint-leaf",
        JobStatus::Queued,
        JobSchedule {
            dependencies: vec![JobDependency {
                artifact: disjoint_artifact.clone(),
            }],
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("disjoint leaf");

    let binary = std::env::current_exe().expect("current exe");
    scheduler_tick(project_root, &jobs_root, &binary).expect("scheduler tick");

    let record_b = read_record(&jobs_root, "job-b").expect("read job b");
    assert_eq!(record_b.status, JobStatus::WaitingOnDeps);
    let detail_b = record_b
        .schedule
        .as_ref()
        .and_then(|sched| sched.wait_reason.as_ref())
        .and_then(|reason| reason.detail.clone())
        .unwrap_or_default();
    assert!(
        detail_b.contains("waiting on command_patch:a-artifact"),
        "unexpected wait detail for job-b: {detail_b}"
    );

    let record_c = read_record(&jobs_root, "job-c").expect("read job c");
    let detail_c = record_c
        .schedule
        .as_ref()
        .and_then(|sched| sched.wait_reason.as_ref())
        .and_then(|reason| reason.detail.clone())
        .unwrap_or_default();
    assert!(
        detail_c.contains("waiting on command_patch:b-artifact"),
        "unexpected wait detail for job-c: {detail_c}"
    );

    for job_id in ["job-fan-left", "job-fan-right"] {
        let record = read_record(&jobs_root, job_id).expect("read fan job");
        assert_eq!(record.status, JobStatus::WaitingOnDeps);
    }

    let record_fanin = read_record(&jobs_root, "job-fanin").expect("read fanin job");
    let detail_fanin = record_fanin
        .schedule
        .as_ref()
        .and_then(|sched| sched.wait_reason.as_ref())
        .and_then(|reason| reason.detail.clone())
        .unwrap_or_default();
    assert!(
        detail_fanin.contains("waiting on command_patch:fanin-left"),
        "unexpected fan-in detail: {detail_fanin}"
    );

    let record_diamond = read_record(&jobs_root, "job-diamond-leaf").expect("read diamond");
    let detail_diamond = record_diamond
        .schedule
        .as_ref()
        .and_then(|sched| sched.wait_reason.as_ref())
        .and_then(|reason| reason.detail.clone())
        .unwrap_or_default();
    assert!(
        detail_diamond.contains("waiting on command_patch:diamond-left"),
        "unexpected diamond detail: {detail_diamond}"
    );

    let record_disjoint = read_record(&jobs_root, "job-disjoint-leaf").expect("read disjoint");
    assert_eq!(record_disjoint.status, JobStatus::WaitingOnDeps);
}

#[test]
fn scheduler_tick_waited_on_accumulates_and_stabilizes() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");
    fs::create_dir_all(&jobs_root).expect("jobs root");
    let current_process_token = std::env::current_exe()
        .expect("current exe")
        .display()
        .to_string();

    let artifact = JobArtifact::CommandPatch {
        job_id: "dep-ready".to_string(),
    };
    write_job_with_status(
        project_root,
        &jobs_root,
        "dep-producer",
        JobStatus::WaitingOnDeps,
        JobSchedule {
            artifacts: vec![artifact.clone()],
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("dep producer");
    write_job_with_status(
        project_root,
        &jobs_root,
        "lock-holder",
        JobStatus::Running,
        JobSchedule {
            locks: vec![JobLock {
                key: "lock-a".to_string(),
                mode: LockMode::Exclusive,
            }],
            ..JobSchedule::default()
        },
        std::slice::from_ref(&current_process_token),
    )
    .expect("lock holder");
    update_job_record(&jobs_root, "lock-holder", |record| {
        record.started_at = Some(Utc::now());
        record.pid = Some(std::process::id());
    })
    .expect("seed lock holder pid");

    write_job_with_status(
        project_root,
        &jobs_root,
        "waiting-job",
        JobStatus::Queued,
        JobSchedule {
            dependencies: vec![JobDependency {
                artifact: artifact.clone(),
            }],
            locks: vec![JobLock {
                key: "lock-a".to_string(),
                mode: LockMode::Exclusive,
            }],
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("waiting job");

    let binary = std::env::current_exe().expect("current exe");
    let outcome = scheduler_tick(project_root, &jobs_root, &binary).expect("tick 1");
    assert!(outcome.updated.contains(&"waiting-job".to_string()));

    let record = read_record(&jobs_root, "waiting-job").expect("read waiting job");
    assert_eq!(record.status, JobStatus::WaitingOnDeps);
    let waited_on = record
        .schedule
        .as_ref()
        .map(|sched| sched.waited_on.clone())
        .unwrap_or_default();
    assert_eq!(waited_on, vec![JobWaitKind::Dependencies]);

    ensure_artifact_exists(
        &Repository::discover(project_root).expect("repo"),
        &jobs_root,
        &artifact,
    )
    .expect("create artifact");
    let outcome = scheduler_tick(project_root, &jobs_root, &binary).expect("tick 2");
    assert!(outcome.updated.contains(&"waiting-job".to_string()));

    let record = read_record(&jobs_root, "waiting-job").expect("read waiting job");
    assert_eq!(record.status, JobStatus::WaitingOnLocks);
    let waited_on = record
        .schedule
        .as_ref()
        .map(|sched| sched.waited_on.clone())
        .unwrap_or_default();
    assert_eq!(
        waited_on,
        vec![JobWaitKind::Dependencies, JobWaitKind::Locks]
    );

    let outcome = scheduler_tick(project_root, &jobs_root, &binary).expect("tick 3");
    assert!(
        !outcome.updated.contains(&"waiting-job".to_string()),
        "expected no-op tick to avoid updates"
    );

    update_job_record(&jobs_root, "lock-holder", |record| {
        record.status = JobStatus::Succeeded;
    })
    .expect("release lock");

    scheduler_tick(project_root, &jobs_root, &binary).expect("tick 4");
    let record = read_record(&jobs_root, "waiting-job").expect("read waiting job");
    assert_eq!(record.status, JobStatus::Running);
    let wait_reason = record
        .schedule
        .as_ref()
        .and_then(|sched| sched.wait_reason.as_ref());
    assert!(wait_reason.is_none(), "wait reason should clear on start");
}

#[test]
fn scheduler_tick_missing_child_args_fails() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "missing-child-args",
        &[],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        None,
    )
    .expect("enqueue");

    let binary = std::env::current_exe().expect("current exe");
    scheduler_tick(project_root, &jobs_root, &binary).expect("tick");

    let record = read_record(&jobs_root, "missing-child-args").expect("record");
    assert_eq!(record.status, JobStatus::Failed);
    assert_eq!(record.exit_code, Some(1));
    assert!(
        record.finished_at.is_some(),
        "expected finished_at to be set"
    );
    let outcome_path = record.outcome_path.as_deref().expect("outcome path");
    assert!(
        project_root.join(outcome_path).exists(),
        "expected outcome file to exist"
    );
    let wait_reason = record
        .schedule
        .as_ref()
        .and_then(|sched| sched.wait_reason.as_ref())
        .expect("wait reason");
    assert_eq!(wait_reason.kind, JobWaitKind::Dependencies);
    assert_eq!(wait_reason.detail.as_deref(), Some("missing child args"));
}

#[test]
fn scheduler_tick_starts_with_empty_schedule() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "empty-schedule",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        None,
    )
    .expect("enqueue");

    let binary = std::env::current_exe().expect("current exe");
    scheduler_tick(project_root, &jobs_root, &binary).expect("tick");

    let record = read_record(&jobs_root, "empty-schedule").expect("record");
    assert_eq!(record.status, JobStatus::Running);
    let wait_reason = record
        .schedule
        .as_ref()
        .and_then(|sched| sched.wait_reason.as_ref());
    assert!(wait_reason.is_none(), "wait reason should be cleared");
}

#[test]
fn scheduler_facts_collect_artifact_existence() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");
    fs::create_dir_all(&jobs_root).expect("jobs root");

    let kinds = [
        ArtifactKind::PlanBranch,
        ArtifactKind::PlanDoc,
        ArtifactKind::PlanCommits,
        ArtifactKind::TargetBranch,
        ArtifactKind::MergeSentinel,
        ArtifactKind::CommandPatch,
        ArtifactKind::Custom,
    ];

    for (idx, kind) in kinds.iter().enumerate() {
        let exists = artifact_for(*kind, &format!("exists-{idx}"));
        let missing = artifact_for(*kind, &format!("missing-{idx}"));
        ensure_artifact_exists(&repo, &jobs_root, &exists).expect("create artifact");

        write_job_with_status(
            project_root,
            &jobs_root,
            &format!("job-{idx}"),
            JobStatus::Queued,
            JobSchedule {
                dependencies: vec![
                    JobDependency {
                        artifact: exists.clone(),
                    },
                    JobDependency {
                        artifact: missing.clone(),
                    },
                ],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("write job");
    }

    let mut records = list_records(&jobs_root).expect("list records");
    records.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let facts = build_scheduler_facts(&repo, &jobs_root, &records).expect("facts");

    for (idx, kind) in kinds.iter().enumerate() {
        let exists = artifact_for(*kind, &format!("exists-{idx}"));
        let missing = artifact_for(*kind, &format!("missing-{idx}"));
        assert!(
            facts.artifact_exists.contains(&exists),
            "expected artifact to exist: {exists:?}"
        );
        assert!(
            !facts.artifact_exists.contains(&missing),
            "expected artifact to be missing: {missing:?}"
        );
    }
}

#[test]
fn artifact_exists_derives_default_branch_when_plan_artifact_branch_is_empty() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");

    commit_plan_doc(&repo, "alpha", "draft/alpha").expect("seed draft plan doc");

    let plan_branch = JobArtifact::PlanBranch {
        slug: "alpha".to_string(),
        branch: String::new(),
    };
    assert!(
        artifact_exists(&repo, &plan_branch),
        "expected plan_branch artifact with empty branch to resolve via draft/<slug>"
    );

    let plan_doc = JobArtifact::PlanDoc {
        slug: "alpha".to_string(),
        branch: String::new(),
    };
    assert!(
        artifact_exists(&repo, &plan_doc),
        "expected plan_doc artifact with empty branch to resolve via draft/<slug>"
    );

    let plan_commits = JobArtifact::PlanCommits {
        slug: "alpha".to_string(),
        branch: String::new(),
    };
    assert!(
        artifact_exists(&repo, &plan_commits),
        "expected plan_commits artifact with empty branch to resolve via draft/<slug>"
    );
}

#[test]
fn finalize_job_writes_custom_artifact_markers() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    let artifact = JobArtifact::Custom {
        type_id: "acme.execution".to_string(),
        key: "final".to_string(),
    };
    enqueue_job(
        project_root,
        &jobs_root,
        "custom-producer",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        None,
        None,
        Some(JobSchedule {
            artifacts: vec![artifact.clone()],
            ..JobSchedule::default()
        }),
    )
    .expect("enqueue custom producer");
    finalize_job(
        project_root,
        &jobs_root,
        "custom-producer",
        JobStatus::Succeeded,
        0,
        None,
        None,
    )
    .expect("finalize custom producer");

    let marker =
        custom_artifact_marker_path(project_root, "custom-producer", "acme.execution", "final");
    assert!(
        marker.exists(),
        "expected custom artifact marker {}",
        marker.display()
    );
    assert!(
        artifact_exists(&repo, &artifact),
        "custom artifact should be externally discoverable after finalize"
    );
}

#[test]
fn scheduler_facts_collect_producer_statuses() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");
    fs::create_dir_all(&jobs_root).expect("jobs root");

    let artifact = JobArtifact::CommandPatch {
        job_id: "artifact".to_string(),
    };
    write_job_with_status(
        project_root,
        &jobs_root,
        "producer-running",
        JobStatus::Running,
        JobSchedule {
            artifacts: vec![artifact.clone()],
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("producer running");
    write_job_with_status(
        project_root,
        &jobs_root,
        "producer-succeeded",
        JobStatus::Succeeded,
        JobSchedule {
            artifacts: vec![artifact.clone()],
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("producer succeeded");

    let mut records = list_records(&jobs_root).expect("list records");
    records.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let facts = build_scheduler_facts(
        &Repository::discover(project_root).expect("repo"),
        &jobs_root,
        &records,
    )
    .expect("facts");
    let statuses = facts
        .producer_statuses
        .get(&artifact)
        .expect("producer statuses");
    assert!(statuses.contains(&JobStatus::Running));
    assert!(statuses.contains(&JobStatus::Succeeded));
}

#[test]
fn scheduler_facts_collect_missing_producer_policy() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");
    fs::create_dir_all(&jobs_root).expect("jobs root");

    let artifact = JobArtifact::CommandPatch {
        job_id: "policy-artifact".to_string(),
    };
    write_job_with_status(
        project_root,
        &jobs_root,
        "policy-job",
        JobStatus::Queued,
        JobSchedule {
            dependencies: vec![JobDependency {
                artifact: artifact.clone(),
            }],
            dependency_policy: JobDependenciesPolicy {
                missing_producer: MissingProducerPolicy::Wait,
            },
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("policy job");

    let mut records = list_records(&jobs_root).expect("list records");
    records.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let facts = build_scheduler_facts(
        &Repository::discover(project_root).expect("repo"),
        &jobs_root,
        &records,
    )
    .expect("facts");

    assert_eq!(
        facts.job_missing_producer_policy.get("policy-job").copied(),
        Some(MissingProducerPolicy::Wait)
    );
}

#[test]
fn scheduler_facts_record_pinned_head_status() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");
    fs::create_dir_all(&jobs_root).expect("jobs root");

    let head = repo.head().expect("head");
    let branch = head.shorthand().expect("branch").to_string();
    let oid = head.target().map(|id| id.to_string()).expect("head oid");

    write_job_with_status(
        project_root,
        &jobs_root,
        "pinned-ok",
        JobStatus::Queued,
        JobSchedule {
            pinned_head: Some(PinnedHead {
                branch: branch.clone(),
                oid: oid.clone(),
            }),
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("pinned ok");
    write_job_with_status(
        project_root,
        &jobs_root,
        "pinned-bad",
        JobStatus::Queued,
        JobSchedule {
            pinned_head: Some(PinnedHead {
                branch: branch.clone(),
                oid: "deadbeef".to_string(),
            }),
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("pinned bad");

    let mut records = list_records(&jobs_root).expect("list records");
    records.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let facts = build_scheduler_facts(&repo, &jobs_root, &records).expect("facts");

    let ok = facts.pinned_heads.get("pinned-ok").expect("pinned ok fact");
    assert!(ok.matches);
    assert_eq!(ok.branch, branch);

    let bad = facts
        .pinned_heads
        .get("pinned-bad")
        .expect("pinned bad fact");
    assert!(!bad.matches);
    assert_eq!(bad.branch, branch);
}

#[test]
fn scheduler_facts_track_running_locks() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");
    fs::create_dir_all(&jobs_root).expect("jobs root");

    let lock = JobLock {
        key: "lock-a".to_string(),
        mode: LockMode::Exclusive,
    };
    write_job_with_status(
        project_root,
        &jobs_root,
        "lock-holder",
        JobStatus::Running,
        JobSchedule {
            locks: vec![lock.clone()],
            ..JobSchedule::default()
        },
        &["--help".to_string()],
    )
    .expect("lock holder");

    let mut records = list_records(&jobs_root).expect("list records");
    records.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let facts = build_scheduler_facts(
        &Repository::discover(project_root).expect("repo"),
        &jobs_root,
        &records,
    )
    .expect("facts");
    assert!(
        !facts.lock_state.can_acquire(&lock),
        "expected lock to be held"
    );
}

#[test]
fn schedule_graph_orders_dependencies_by_artifact_key() {
    let deps = vec![
        JobDependency {
            artifact: JobArtifact::TargetBranch {
                name: "main".to_string(),
            },
        },
        JobDependency {
            artifact: JobArtifact::CommandPatch {
                job_id: "job-z".to_string(),
            },
        },
        JobDependency {
            artifact: JobArtifact::PlanDoc {
                slug: "alpha".to_string(),
                branch: "draft/alpha".to_string(),
            },
        },
    ];

    let record = make_record(
        "job-1",
        JobStatus::Queued,
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        Some(JobSchedule {
            dependencies: deps,
            ..JobSchedule::default()
        }),
    );

    let graph = ScheduleGraph::new(vec![record]);
    let ordered = graph.dependencies_for("job-1");
    let expected = vec![
        JobArtifact::PlanDoc {
            slug: "alpha".to_string(),
            branch: "draft/alpha".to_string(),
        },
        JobArtifact::TargetBranch {
            name: "main".to_string(),
        },
        JobArtifact::CommandPatch {
            job_id: "job-z".to_string(),
        },
    ];
    assert_eq!(ordered, expected);
}

#[test]
fn schedule_graph_orders_producers_by_created_at_then_id() {
    let artifact = JobArtifact::CommandPatch {
        job_id: "shared".to_string(),
    };
    let record_a = make_record(
        "job-a",
        JobStatus::Queued,
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        Some(JobSchedule {
            artifacts: vec![artifact.clone()],
            ..JobSchedule::default()
        }),
    );
    let record_b = make_record(
        "job-b",
        JobStatus::Queued,
        Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
        Some(JobSchedule {
            artifacts: vec![artifact.clone()],
            ..JobSchedule::default()
        }),
    );

    let graph = ScheduleGraph::new(vec![record_b, record_a]);
    let producers = graph.producers_for(&artifact);
    assert_eq!(producers, vec!["job-a".to_string(), "job-b".to_string()]);
}

#[test]
fn schedule_graph_collect_focus_includes_after_neighbors() {
    let focused = make_record(
        "job-focused",
        JobStatus::Queued,
        Utc.with_ymd_and_hms(2026, 1, 3, 0, 0, 0).unwrap(),
        Some(JobSchedule {
            after: vec![after_dependency("job-parent")],
            ..JobSchedule::default()
        }),
    );
    let parent = make_record(
        "job-parent",
        JobStatus::Succeeded,
        Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
        Some(JobSchedule::default()),
    );
    let child = make_record(
        "job-child",
        JobStatus::Queued,
        Utc.with_ymd_and_hms(2026, 1, 4, 0, 0, 0).unwrap(),
        Some(JobSchedule {
            after: vec![after_dependency("job-focused")],
            ..JobSchedule::default()
        }),
    );

    let graph = ScheduleGraph::new(vec![focused, parent, child]);
    let focus = graph.collect_focus_jobs("job-focused", 1);
    assert!(focus.contains("job-focused"));
    assert!(focus.contains("job-parent"));
    assert!(focus.contains("job-child"));
}

#[test]
fn schedule_snapshot_includes_after_edges() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");

    let predecessor = make_record(
        "job-predecessor",
        JobStatus::Succeeded,
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        Some(JobSchedule::default()),
    );
    let dependent = make_record(
        "job-dependent",
        JobStatus::Queued,
        Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
        Some(JobSchedule {
            after: vec![after_dependency("job-predecessor")],
            ..JobSchedule::default()
        }),
    );

    let graph = ScheduleGraph::new(vec![dependent, predecessor]);
    let edges = graph.snapshot_edges(&repo, &["job-dependent".to_string()], 2);
    assert!(
        edges.iter().any(|edge| {
            edge.from == "job-dependent"
                && edge.to == "job-predecessor"
                && edge.after.as_ref().map(|after| after.policy) == Some(AfterPolicy::Success)
        }),
        "expected snapshot to include explicit after edge"
    );
}

#[test]
fn schedule_graph_reports_artifact_state() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    ensure_branch(&repo, "present").expect("ensure branch");

    let graph = ScheduleGraph::new(Vec::new());
    let present = JobArtifact::TargetBranch {
        name: "present".to_string(),
    };
    let missing = JobArtifact::TargetBranch {
        name: "missing".to_string(),
    };
    assert_eq!(
        graph.artifact_state(&repo, &present),
        ScheduleArtifactState::Present
    );
    assert_eq!(
        graph.artifact_state(&repo, &missing),
        ScheduleArtifactState::Missing
    );
}

#[test]
fn schedule_snapshot_respects_depth_limit() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");

    let artifact_b = JobArtifact::CommandPatch {
        job_id: "b".to_string(),
    };
    let artifact_c = JobArtifact::CommandPatch {
        job_id: "c".to_string(),
    };

    let job_c = make_record(
        "job-c",
        JobStatus::Succeeded,
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        Some(JobSchedule {
            artifacts: vec![artifact_c.clone()],
            ..JobSchedule::default()
        }),
    );
    let job_b = make_record(
        "job-b",
        JobStatus::Succeeded,
        Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
        Some(JobSchedule {
            dependencies: vec![JobDependency {
                artifact: artifact_c.clone(),
            }],
            artifacts: vec![artifact_b.clone()],
            ..JobSchedule::default()
        }),
    );
    let job_a = make_record(
        "job-a",
        JobStatus::Queued,
        Utc.with_ymd_and_hms(2026, 1, 3, 0, 0, 0).unwrap(),
        Some(JobSchedule {
            dependencies: vec![JobDependency {
                artifact: artifact_b.clone(),
            }],
            ..JobSchedule::default()
        }),
    );

    let graph = ScheduleGraph::new(vec![job_a, job_b, job_c]);
    let roots = vec!["job-a".to_string()];

    let edges = graph.snapshot_edges(&repo, &roots, 1);
    assert!(
        edges
            .iter()
            .any(|edge| edge.from == "job-a" && edge.to == "job-b"),
        "expected job-a -> job-b edge"
    );
    assert!(
        edges.iter().all(|edge| edge.from != "job-b"),
        "expected depth=1 to skip job-b dependencies"
    );

    let deeper = graph.snapshot_edges(&repo, &roots, 2);
    assert!(
        deeper
            .iter()
            .any(|edge| edge.from == "job-b" && edge.to == "job-c"),
        "expected depth=2 to include job-b -> job-c edge"
    );
}

#[test]
fn retry_set_includes_downstream_dependents_only() {
    let predecessor = make_record(
        "job-predecessor",
        JobStatus::Succeeded,
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        Some(JobSchedule {
            artifacts: vec![JobArtifact::CommandPatch {
                job_id: "pred-artifact".to_string(),
            }],
            ..JobSchedule::default()
        }),
    );
    let root_artifact = JobArtifact::CommandPatch {
        job_id: "root-artifact".to_string(),
    };
    let root = make_record(
        "job-root",
        JobStatus::Failed,
        Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
        Some(JobSchedule {
            after: vec![after_dependency("job-predecessor")],
            artifacts: vec![root_artifact.clone()],
            ..JobSchedule::default()
        }),
    );
    let dependent_after = make_record(
        "job-dependent-after",
        JobStatus::BlockedByDependency,
        Utc.with_ymd_and_hms(2026, 1, 3, 0, 0, 0).unwrap(),
        Some(JobSchedule {
            after: vec![after_dependency("job-root")],
            ..JobSchedule::default()
        }),
    );
    let dependent_artifact = make_record(
        "job-dependent-artifact",
        JobStatus::BlockedByDependency,
        Utc.with_ymd_and_hms(2026, 1, 4, 0, 0, 0).unwrap(),
        Some(JobSchedule {
            dependencies: vec![JobDependency {
                artifact: root_artifact,
            }],
            ..JobSchedule::default()
        }),
    );

    let graph = ScheduleGraph::new(vec![dependent_artifact, dependent_after, predecessor, root]);
    let retry_set = collect_retry_set(&graph, "job-root");
    assert_eq!(
        retry_set,
        vec![
            "job-root".to_string(),
            "job-dependent-after".to_string(),
            "job-dependent-artifact".to_string(),
        ]
    );
}

#[test]
fn rewind_job_record_for_retry_clears_runtime_and_artifacts() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-retry",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");

    let worktree_path = project_root.join(".vizier/tmp-worktrees/retry-cleanup");
    fs::create_dir_all(&worktree_path).expect("create worktree path");

    let paths = paths_for(&jobs_root, "job-retry");
    fs::write(&paths.stdout_path, "stdout").expect("write stdout");
    fs::write(&paths.stderr_path, "stderr").expect("write stderr");
    let outcome_path = paths.job_dir.join("outcome.json");
    fs::write(&outcome_path, "{}").expect("write outcome");
    let ask_patch = command_patch_path(&jobs_root, "job-retry");
    let save_patch = save_input_patch_path(&jobs_root, "job-retry");
    fs::write(&ask_patch, "ask patch").expect("write ask patch");
    fs::write(&save_patch, "save patch").expect("write save patch");
    let custom_artifact = JobArtifact::Custom {
        type_id: "acme.execution".to_string(),
        key: "retry-node".to_string(),
    };
    write_custom_artifact_markers(
        project_root,
        "job-retry",
        std::slice::from_ref(&custom_artifact),
    )
    .expect("write custom marker");
    let custom_marker =
        custom_artifact_marker_path(project_root, "job-retry", "acme.execution", "retry-node");
    let custom_payload = write_custom_artifact_payload(
        project_root,
        "job-retry",
        "acme.execution",
        "retry-node",
        &serde_json::json!({"text": "payload"}),
    )
    .expect("write custom payload");

    let mut record = update_job_record(&jobs_root, "job-retry", |record| {
        record.status = JobStatus::Failed;
        let now = Utc::now();
        record.started_at = Some(now);
        record.finished_at = Some(now);
        record.pid = Some(4242);
        record.exit_code = Some(1);
        record.session_path = Some(".vizier/sessions/s1/session.json".to_string());
        record.outcome_path = Some(".vizier/jobs/job-retry/outcome.json".to_string());
        record.schedule = Some(JobSchedule {
            wait_reason: Some(JobWaitReason {
                kind: JobWaitKind::Dependencies,
                detail: Some("waiting on old state".to_string()),
            }),
            waited_on: vec![JobWaitKind::Dependencies],
            artifacts: vec![custom_artifact.clone()],
            ..record.schedule.clone().unwrap_or_default()
        });
        record.metadata = Some(JobMetadata {
            execution_root: Some(".vizier/tmp-worktrees/retry-cleanup".to_string()),
            worktree_owned: Some(true),
            worktree_path: Some(".vizier/tmp-worktrees/retry-cleanup".to_string()),
            workflow_node_attempt: Some(4),
            workflow_node_outcome: Some("failed".to_string()),
            workflow_payload_refs: Some(vec!["payload.json".to_string()]),
            agent_exit_code: Some(12),
            cancel_cleanup_status: Some(CancelCleanupStatus::Failed),
            cancel_cleanup_error: Some("old error".to_string()),
            ..JobMetadata::default()
        });
    })
    .expect("set runtime fields");

    rewind_job_record_for_retry(project_root, &jobs_root, &mut record).expect("rewind record");
    persist_record(&paths, &record).expect("persist rewinded record");

    assert_eq!(record.status, JobStatus::Queued);
    assert!(record.started_at.is_none());
    assert!(record.finished_at.is_none());
    assert!(record.pid.is_none());
    assert!(record.exit_code.is_none());
    assert!(record.session_path.is_none());
    assert!(record.outcome_path.is_none());

    let schedule = record.schedule.as_ref().expect("schedule");
    assert!(schedule.wait_reason.is_none(), "wait reason should clear");
    assert!(schedule.waited_on.is_empty(), "waited_on should clear");

    let metadata = record.metadata.as_ref().expect("metadata");
    assert!(metadata.worktree_name.is_none());
    assert!(metadata.worktree_path.is_none());
    assert!(metadata.worktree_owned.is_none());
    assert_eq!(metadata.execution_root.as_deref(), Some("."));
    assert_eq!(metadata.workflow_node_attempt, Some(5));
    assert!(metadata.workflow_node_outcome.is_none());
    assert!(metadata.workflow_payload_refs.is_none());
    assert!(metadata.agent_exit_code.is_none());
    assert!(metadata.cancel_cleanup_status.is_none());
    assert!(metadata.cancel_cleanup_error.is_none());
    assert_eq!(
        metadata.retry_cleanup_status,
        Some(RetryCleanupStatus::Done)
    );
    assert!(metadata.retry_cleanup_error.is_none());

    assert!(
        !outcome_path.exists(),
        "expected outcome file to be removed during rewind"
    );
    assert!(
        !ask_patch.exists(),
        "expected ask-save patch to be removed during rewind"
    );
    assert!(
        !save_patch.exists(),
        "expected save-input patch to be removed during rewind"
    );
    assert!(
        !custom_marker.exists(),
        "expected custom artifact marker to be removed during rewind"
    );
    assert!(
        !custom_payload.exists(),
        "expected custom artifact payload to be removed during rewind"
    );
    assert!(
        !worktree_path.exists(),
        "expected retry-owned worktree to be removed during rewind"
    );
    let stdout = fs::read_to_string(&paths.stdout_path).expect("read stdout");
    let stderr = fs::read_to_string(&paths.stderr_path).expect("read stderr");
    assert!(stdout.is_empty(), "expected stdout log truncation");
    assert!(stderr.is_empty(), "expected stderr log truncation");
}

#[test]
fn rewind_job_record_for_retry_retains_worktree_metadata_when_cleanup_degrades() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-retry-degraded",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");

    let worktree_rel = ".vizier/tmp-worktrees/retry-degraded";
    let worktree_path = project_root.join(worktree_rel);
    fs::create_dir_all(&worktree_path).expect("create worktree path");

    let mut record = update_job_record(&jobs_root, "job-retry-degraded", |record| {
        record.status = JobStatus::Failed;
        record.metadata = Some(JobMetadata {
            execution_root: Some(worktree_rel.to_string()),
            worktree_name: Some("missing-retry-worktree".to_string()),
            worktree_owned: Some(true),
            worktree_path: Some(worktree_rel.to_string()),
            ..JobMetadata::default()
        });
    })
    .expect("set runtime fields");

    rewind_job_record_for_retry(project_root, &jobs_root, &mut record).expect("rewind record");

    let metadata = record.metadata.as_ref().expect("metadata");
    assert_eq!(
        metadata.worktree_name.as_deref(),
        Some("missing-retry-worktree")
    );
    assert_eq!(metadata.worktree_path.as_deref(), Some(worktree_rel));
    assert_eq!(metadata.worktree_owned, Some(true));
    assert_eq!(
        metadata.retry_cleanup_status,
        Some(RetryCleanupStatus::Degraded)
    );
    assert_eq!(metadata.execution_root.as_deref(), Some(worktree_rel));
    let detail = metadata.retry_cleanup_error.as_deref().unwrap_or("");
    assert!(
        detail.contains("fallback cleanup failed"),
        "expected fallback failure detail, got: {detail}"
    );
}

#[test]
fn rewind_job_record_for_retry_clears_worktree_metadata_when_fallback_succeeds() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-retry-fallback",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");

    let worktree_rel = ".vizier/tmp-worktrees/retry-fallback";
    let worktree_path = project_root.join(worktree_rel);
    if let Some(parent) = worktree_path.parent() {
        fs::create_dir_all(parent).expect("create worktree parent");
    }
    let head_branch = current_branch_name(project_root).unwrap_or_else(|| "master".to_string());
    crate::vcs::add_worktree_for_branch_in(
        project_root,
        "retry-fallback",
        &worktree_path,
        &head_branch,
    )
    .expect("add retry fallback worktree");

    let mut record = update_job_record(&jobs_root, "job-retry-fallback", |record| {
        record.status = JobStatus::Failed;
        record.metadata = Some(JobMetadata {
            execution_root: Some(worktree_rel.to_string()),
            worktree_name: Some("wrong-worktree-name".to_string()),
            worktree_owned: Some(true),
            worktree_path: Some(worktree_rel.to_string()),
            ..JobMetadata::default()
        });
    })
    .expect("set runtime fields");

    rewind_job_record_for_retry(project_root, &jobs_root, &mut record).expect("rewind record");

    let metadata = record.metadata.as_ref().expect("metadata");
    assert!(metadata.worktree_name.is_none());
    assert!(metadata.worktree_path.is_none());
    assert!(metadata.worktree_owned.is_none());
    assert_eq!(metadata.execution_root.as_deref(), Some("."));
    assert_eq!(
        metadata.retry_cleanup_status,
        Some(RetryCleanupStatus::Done)
    );
    assert!(metadata.retry_cleanup_error.is_none());
    assert!(
        !worktree_path.exists(),
        "expected fallback cleanup to remove worktree path"
    );
}

#[test]
fn prune_error_mentions_missing_shallow_detects_known_message() {
    let sample = "could not find '/tmp/repo/.git/shallow' to stat";
    assert!(
        prune_error_mentions_missing_shallow(sample),
        "expected missing shallow detection"
    );
    assert!(
        !prune_error_mentions_missing_shallow("failed to prune worktree"),
        "unexpected shallow detection for unrelated error"
    );
}

#[test]
fn retry_job_clears_merge_sentinel_when_git_state_is_clean() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-merge-retry",
        &["--help".to_string()],
        &["vizier".to_string(), "merge".to_string()],
        Some(JobMetadata {
            scope: Some("merge".to_string()),
            plan: Some("retry-merge".to_string()),
            ..JobMetadata::default()
        }),
        None,
        Some(JobSchedule {
            dependencies: vec![JobDependency {
                artifact: JobArtifact::TargetBranch {
                    name: "missing-target".to_string(),
                },
            }],
            locks: vec![JobLock {
                key: "merge_sentinel:retry-merge".to_string(),
                mode: LockMode::Exclusive,
            }],
            ..JobSchedule::default()
        }),
    )
    .expect("enqueue merge retry");
    update_job_record(&jobs_root, "job-merge-retry", |record| {
        record.status = JobStatus::Failed;
        record.exit_code = Some(1);
    })
    .expect("set failed status");

    let sentinel = project_root
        .join(".vizier/tmp/merge-conflicts")
        .join("retry-merge.json");
    if let Some(parent) = sentinel.parent() {
        fs::create_dir_all(parent).expect("create merge-conflict parent");
    }
    fs::write(&sentinel, "{}").expect("write sentinel");

    let binary = std::env::current_exe().expect("current exe");
    retry_job(project_root, &jobs_root, &binary, "job-merge-retry").expect("retry merge job");

    assert!(
        !sentinel.exists(),
        "expected merge sentinel cleanup during retry"
    );
}

#[test]
fn retry_job_rejects_running_jobs_in_retry_set() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-root",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        None,
    )
    .expect("enqueue root");
    update_job_record(&jobs_root, "job-root", |record| {
        record.status = JobStatus::Failed;
        record.exit_code = Some(1);
    })
    .expect("mark root failed");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-dependent",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        Some(JobSchedule {
            after: vec![after_dependency("job-root")],
            ..JobSchedule::default()
        }),
    )
    .expect("enqueue dependent");
    update_job_record(&jobs_root, "job-dependent", |record| {
        record.status = JobStatus::Running;
    })
    .expect("mark dependent running");

    let binary = std::env::current_exe().expect("current exe");
    let err = retry_job(project_root, &jobs_root, &binary, "job-root")
        .expect_err("expected running dependent to block retry");
    assert!(
        err.to_string().contains("job-dependent (running)"),
        "unexpected retry active-set error: {err}"
    );
}

#[test]
fn retry_job_allows_waiting_jobs_in_retry_set() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    let root_artifact = JobArtifact::CommandPatch {
        job_id: "job-root".to_string(),
    };

    enqueue_job(
        project_root,
        &jobs_root,
        "job-root",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        Some(JobSchedule {
            dependencies: vec![JobDependency {
                artifact: JobArtifact::TargetBranch {
                    name: "missing-retry-target".to_string(),
                },
            }],
            artifacts: vec![root_artifact.clone()],
            ..JobSchedule::default()
        }),
    )
    .expect("enqueue root");
    update_job_record(&jobs_root, "job-root", |record| {
        record.status = JobStatus::Failed;
        record.exit_code = Some(1);
        if let Some(schedule) = record.schedule.as_mut() {
            schedule.wait_reason = Some(JobWaitReason {
                kind: JobWaitKind::Dependencies,
                detail: Some("dependency failed for previous attempt".to_string()),
            });
            schedule.waited_on = vec![JobWaitKind::Dependencies];
        }
    })
    .expect("mark root failed");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-dependent",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        Some(JobSchedule {
            dependencies: vec![JobDependency {
                artifact: root_artifact,
            }],
            wait_reason: Some(JobWaitReason {
                kind: JobWaitKind::Dependencies,
                detail: Some("waiting on command_patch:job-root".to_string()),
            }),
            waited_on: vec![JobWaitKind::Dependencies],
            ..JobSchedule::default()
        }),
    )
    .expect("enqueue dependent");
    update_job_record(&jobs_root, "job-dependent", |record| {
        record.status = JobStatus::WaitingOnDeps;
    })
    .expect("mark dependent waiting");

    let binary = std::env::current_exe().expect("current exe");
    let outcome = retry_job(project_root, &jobs_root, &binary, "job-root")
        .expect("waiting jobs in retry set should not block retry");
    assert_eq!(
        outcome.retry_set,
        vec!["job-root".to_string(), "job-dependent".to_string()]
    );
    assert_eq!(
        outcome.reset,
        vec!["job-root".to_string(), "job-dependent".to_string()]
    );
}

#[test]
fn retry_job_internal_applies_propagated_execution_context_before_scheduler_tick() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-root",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        Some(JobMetadata {
            execution_root: Some(".".to_string()),
            ..JobMetadata::default()
        }),
        None,
        Some(JobSchedule {
            dependencies: vec![JobDependency {
                artifact: JobArtifact::TargetBranch {
                    name: "missing-retry-target".to_string(),
                },
            }],
            ..JobSchedule::default()
        }),
    )
    .expect("enqueue root");
    update_job_record(&jobs_root, "job-root", |record| {
        record.status = JobStatus::Failed;
        record.exit_code = Some(1);
    })
    .expect("mark root failed");

    let propagated = WorkflowExecutionContext {
        execution_root: Some(".vizier/tmp-worktrees/propagated".to_string()),
        worktree_path: Some(".vizier/tmp-worktrees/propagated".to_string()),
        worktree_name: Some("propagated".to_string()),
        worktree_owned: Some(true),
    };
    let binary = std::env::current_exe().expect("current exe");
    retry_job_internal(
        project_root,
        &jobs_root,
        &binary,
        "job-root",
        Some(&propagated),
    )
    .expect("retry with propagated context");

    let root = read_record(&jobs_root, "job-root").expect("root record");
    let metadata = root.metadata.as_ref().expect("root metadata");
    assert_eq!(metadata.execution_root, propagated.execution_root);
    assert_eq!(metadata.worktree_path, propagated.worktree_path);
    assert_eq!(metadata.worktree_name, propagated.worktree_name);
    assert_eq!(metadata.worktree_owned, propagated.worktree_owned);
}

#[test]
fn execute_workflow_node_job_succeeded_path_applies_context_before_concurrent_tick_can_start_target()
 {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    let template = worktree_command_template("sleep 1");
    let result = enqueue_workflow_run(
        project_root,
        &jobs_root,
        "run-lock-atomic",
        "template.runtime.worktree_command@v1",
        &template,
        &[
            "vizier".to_string(),
            "jobs".to_string(),
            "schedule".to_string(),
        ],
        None,
    )
    .expect("enqueue workflow run");
    let prepare_job = result
        .job_ids
        .get("prepare_worktree")
        .expect("prepare job id")
        .clone();
    let target_job = result
        .job_ids
        .get("run_target")
        .expect("target job id")
        .clone();

    update_job_record(&jobs_root, &prepare_job, |record| {
        record.status = JobStatus::Running;
        record.started_at = Some(Utc::now());
        record.pid = Some(std::process::id());
    })
    .expect("mark prepare running");

    let barrier = Arc::new(Barrier::new(2));
    let _pause_guard = install_succeeded_completion_pause(barrier.clone());

    let completion_root = project_root.to_path_buf();
    let completion_jobs = jobs_root.clone();
    let completion_job = prepare_job.clone();
    let completion = std::thread::spawn(move || {
        execute_workflow_node_job(&completion_root, &completion_jobs, &completion_job)
            .map_err(|err| err.to_string())
    });

    barrier.wait();

    let tick_root = project_root.to_path_buf();
    let tick_jobs = jobs_root.clone();
    let tick_binary = std::env::current_exe().expect("current exe");
    let tick_handle = std::thread::spawn(move || {
        scheduler_tick(&tick_root, &tick_jobs, &tick_binary).map_err(|err| err.to_string())
    });

    barrier.wait();

    let prepare_exit = completion
        .join()
        .expect("completion thread should not panic")
        .expect("prepare completion");
    assert_eq!(prepare_exit, 0);

    tick_handle
        .join()
        .expect("tick thread should not panic")
        .expect("concurrent scheduler tick");

    let prepare_record = read_record(&jobs_root, &prepare_job).expect("prepare record");
    let prepare_meta = prepare_record.metadata.expect("prepare metadata");
    assert!(
        prepare_meta.execution_root.is_some(),
        "prepare should produce execution_root metadata"
    );

    let target_record = read_record(&jobs_root, &target_job).expect("target record");
    let target_meta = target_record.metadata.expect("target metadata");
    assert_eq!(target_meta.execution_root, prepare_meta.execution_root);
    assert_eq!(target_meta.worktree_path, prepare_meta.worktree_path);
    assert_eq!(target_meta.worktree_name, prepare_meta.worktree_name);
    assert_eq!(target_meta.worktree_owned, prepare_meta.worktree_owned);
    assert!(
        target_record.started_at.is_some() || target_record.finished_at.is_some(),
        "target should become start-eligible after succeeded completion"
    );

    if job_is_active(target_record.status) {
        let _ = cancel_job_with_cleanup(project_root, &jobs_root, &target_job, false);
    }
}

#[test]
fn worktree_prepare_resolve_invoke_success_chain_preserves_non_null_execution_context() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    let template = worktree_prompt_invoke_template();
    let result = enqueue_workflow_run(
        project_root,
        &jobs_root,
        "run-worktree-chain",
        "template.runtime.worktree_prompt_invoke@v1",
        &template,
        &[
            "vizier".to_string(),
            "jobs".to_string(),
            "schedule".to_string(),
        ],
        None,
    )
    .expect("enqueue workflow run");

    let prepare_job = result
        .job_ids
        .get("prepare_worktree")
        .expect("prepare job id")
        .clone();
    let resolve_job = result
        .job_ids
        .get("resolve_prompt")
        .expect("resolve job id")
        .clone();
    let invoke_job = result
        .job_ids
        .get("invoke_agent")
        .expect("invoke job id")
        .clone();

    for job_id in [&resolve_job, &invoke_job] {
        update_job_record(&jobs_root, job_id, |record| {
            let schedule = record.schedule.get_or_insert_with(JobSchedule::default);
            schedule.approval = Some(JobApproval::pending(Some("test".to_string())));
        })
        .expect("gate downstream node with pending approval");
    }

    update_job_record(&jobs_root, &prepare_job, |record| {
        record.status = JobStatus::Running;
        record.started_at = Some(Utc::now());
        record.pid = Some(std::process::id());
    })
    .expect("mark prepare running");
    let prepare_exit = execute_workflow_node_job(project_root, &jobs_root, &prepare_job)
        .expect("execute prepare_worktree");
    assert_eq!(prepare_exit, 0);

    let prepare_record = read_record(&jobs_root, &prepare_job).expect("prepare record");
    let prepare_meta = prepare_record.metadata.expect("prepare metadata");
    assert!(
        prepare_meta.execution_root.is_some(),
        "prepare metadata should carry non-null execution_root"
    );

    let resolve_record = read_record(&jobs_root, &resolve_job).expect("resolve record");
    let resolve_meta = resolve_record
        .metadata
        .expect("resolve metadata after prepare");
    assert_eq!(resolve_meta.execution_root, prepare_meta.execution_root);
    assert_eq!(resolve_meta.worktree_path, prepare_meta.worktree_path);
    assert_eq!(resolve_meta.worktree_name, prepare_meta.worktree_name);
    assert_eq!(resolve_meta.worktree_owned, prepare_meta.worktree_owned);

    update_job_record(&jobs_root, &resolve_job, |record| {
        record.status = JobStatus::Running;
        record.started_at = Some(Utc::now());
        record.pid = Some(std::process::id());
        if let Some(schedule) = record.schedule.as_mut() {
            schedule.approval = None;
        }
    })
    .expect("mark resolve running");
    let resolve_exit = execute_workflow_node_job(project_root, &jobs_root, &resolve_job)
        .expect("execute resolve_prompt");
    assert_eq!(resolve_exit, 0);

    let invoke_record = read_record(&jobs_root, &invoke_job).expect("invoke record");
    let invoke_meta = invoke_record
        .metadata
        .expect("invoke metadata after resolve");
    assert_eq!(invoke_meta.execution_root, prepare_meta.execution_root);
    assert_eq!(invoke_meta.worktree_path, prepare_meta.worktree_path);
    assert_eq!(invoke_meta.worktree_name, prepare_meta.worktree_name);
    assert_eq!(invoke_meta.worktree_owned, prepare_meta.worktree_owned);
}

#[test]
fn enqueue_workflow_run_materializes_runtime_node_jobs() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    let template = prompt_invoke_template();
    let result = enqueue_workflow_run(
        project_root,
        &jobs_root,
        "run-runtime",
        "template.runtime.prompt_invoke@v1",
        &template,
        &[
            "vizier".to_string(),
            "jobs".to_string(),
            "schedule".to_string(),
        ],
        None,
    )
    .expect("enqueue workflow run");

    assert_eq!(result.job_ids.len(), 2);
    let resolve_job = result
        .job_ids
        .get("resolve_prompt")
        .expect("resolve job id")
        .clone();
    let invoke_job = result
        .job_ids
        .get("invoke_agent")
        .expect("invoke job id")
        .clone();

    let resolve_record = read_record(&jobs_root, &resolve_job).expect("resolve record");
    assert_eq!(
        resolve_record.child_args,
        vec![
            "__workflow-node".to_string(),
            "--job-id".to_string(),
            resolve_job.clone()
        ]
    );
    let resolve_meta = resolve_record.metadata.as_ref().expect("resolve metadata");
    assert_eq!(resolve_meta.workflow_run_id.as_deref(), Some("run-runtime"));
    assert_eq!(resolve_meta.workflow_node_attempt, Some(1));
    assert_eq!(
        resolve_meta.workflow_executor_operation.as_deref(),
        Some("prompt.resolve")
    );
    let resolve_locks = resolve_record
        .schedule
        .as_ref()
        .map(|schedule| schedule.locks.clone())
        .unwrap_or_default();
    assert_eq!(
        resolve_locks,
        vec![JobLock {
            key: "repo_serial".to_string(),
            mode: LockMode::Exclusive,
        }],
        "expected lockless workflow node to receive inferred repo_serial lock"
    );
    let resolve_artifacts = resolve_record
        .schedule
        .as_ref()
        .map(|schedule| schedule.artifacts.clone())
        .unwrap_or_default();
    assert!(
        resolve_artifacts.iter().any(|artifact| {
            matches!(
                artifact,
                JobArtifact::Custom { type_id, key }
                if type_id == OPERATION_OUTPUT_ARTIFACT_TYPE_ID && key == "resolve_prompt"
            )
        }),
        "expected implicit operation-output artifact on resolve node schedule"
    );

    let invoke_record = read_record(&jobs_root, &invoke_job).expect("invoke record");
    let after = invoke_record
        .schedule
        .as_ref()
        .map(|schedule| schedule.after.clone())
        .unwrap_or_default();
    assert!(
        after
            .iter()
            .any(|dependency| dependency.job_id == resolve_job),
        "expected invoke node to depend on resolve node via on.succeeded routing"
    );
    let invoke_locks = invoke_record
        .schedule
        .as_ref()
        .map(|schedule| schedule.locks.clone())
        .unwrap_or_default();
    assert_eq!(
        invoke_locks,
        vec![JobLock {
            key: "repo_serial".to_string(),
            mode: LockMode::Exclusive,
        }],
        "expected inferred locks to persist in node schedule metadata"
    );
    let invoke_artifacts = invoke_record
        .schedule
        .as_ref()
        .map(|schedule| schedule.artifacts.clone())
        .unwrap_or_default();
    assert!(
        invoke_artifacts.iter().any(|artifact| {
            matches!(
                artifact,
                JobArtifact::Custom { type_id, key }
                if type_id == OPERATION_OUTPUT_ARTIFACT_TYPE_ID && key == "invoke_agent"
            )
        }),
        "expected implicit operation-output artifact on invoke node schedule"
    );

    let manifest =
        load_workflow_run_manifest(project_root, "run-runtime").expect("workflow run manifest");
    assert_eq!(manifest.nodes.len(), 2);
    assert!(manifest.nodes.contains_key("resolve_prompt"));
    assert!(manifest.nodes.contains_key("invoke_agent"));
    let resolve_manifest = manifest
        .nodes
        .get("resolve_prompt")
        .expect("resolve manifest");
    assert!(
        resolve_manifest
            .artifacts_by_outcome
            .succeeded
            .iter()
            .any(|artifact| {
                matches!(
                    artifact,
                    JobArtifact::Custom { type_id, key }
                    if type_id == OPERATION_OUTPUT_ARTIFACT_TYPE_ID && key == "resolve_prompt"
                )
            }),
        "expected implicit operation-output artifact in manifest outcome artifacts"
    );
    assert!(
        resolve_manifest.routes.succeeded.iter().any(|target| {
            target.node_id == "invoke_agent"
                && matches!(target.mode, WorkflowRouteMode::PropagateContext)
        }),
        "expected success edge to materialize as context-propagation route"
    );
}

#[test]
fn workflow_node_execution_persists_operation_output_payload_and_refs() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    let template = prompt_invoke_template();
    let result = enqueue_workflow_run(
        project_root,
        &jobs_root,
        "run-operation-output",
        "template.runtime.prompt_invoke@v1",
        &template,
        &[
            "vizier".to_string(),
            "jobs".to_string(),
            "schedule".to_string(),
        ],
        None,
    )
    .expect("enqueue workflow run");
    let resolve_job = result
        .job_ids
        .get("resolve_prompt")
        .expect("resolve job id")
        .clone();

    update_job_record(&jobs_root, &resolve_job, |record| {
        record.status = JobStatus::Running;
        record.started_at = Some(Utc::now());
        record.pid = Some(std::process::id());
    })
    .expect("mark resolve running");

    let exit_code =
        execute_workflow_node_job(project_root, &jobs_root, &resolve_job).expect("execute");
    assert_eq!(exit_code, 0);

    let operation_payload_path = custom_artifact_payload_path(
        project_root,
        &resolve_job,
        OPERATION_OUTPUT_ARTIFACT_TYPE_ID,
        "resolve_prompt",
    );
    assert!(
        operation_payload_path.exists(),
        "expected operation output payload file"
    );
    let operation_payload_raw =
        fs::read_to_string(&operation_payload_path).expect("read operation output payload");
    let operation_payload: serde_json::Value =
        serde_json::from_str(&operation_payload_raw).expect("parse payload");
    assert_eq!(
        operation_payload
            .get("schema")
            .and_then(|value| value.as_str()),
        Some(OPERATION_OUTPUT_SCHEMA_ID)
    );
    assert_eq!(
        operation_payload
            .get("executor_operation")
            .and_then(|value| value.as_str()),
        Some("prompt.resolve")
    );
    assert_eq!(
        operation_payload
            .get("node_id")
            .and_then(|value| value.as_str()),
        Some("resolve_prompt")
    );
    assert!(
        operation_payload
            .get("stdout_text")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .contains("vizier.operation_result.v1"),
        "expected canonical operation-result stdout for prompt.resolve payload: {}",
        operation_payload
    );
    assert!(
        operation_payload
            .get("stderr_lines")
            .and_then(|value| value.as_array())
            .map(|lines| lines.iter().any(|line| {
                line.as_str()
                    .unwrap_or("")
                    .contains("[workflow-node] start node=resolve_prompt")
            }))
            .unwrap_or(false),
        "expected lifecycle stderr start line in payload: {}",
        operation_payload
    );

    let operation_marker = custom_artifact_marker_path(
        project_root,
        &resolve_job,
        OPERATION_OUTPUT_ARTIFACT_TYPE_ID,
        "resolve_prompt",
    );
    assert!(
        operation_marker.exists(),
        "expected operation-output marker to be written"
    );

    let resolve_record = read_record(&jobs_root, &resolve_job).expect("resolve record");
    let workflow_payload_refs = resolve_record
        .metadata
        .as_ref()
        .and_then(|meta| meta.workflow_payload_refs.clone())
        .unwrap_or_default();
    let expected_ref = relative_path(project_root, &operation_payload_path);
    assert!(
        workflow_payload_refs
            .iter()
            .any(|value| value == &expected_ref),
        "expected metadata.workflow_payload_refs to include operation output payload ref"
    );
}

#[test]
fn workflow_runtime_can_consume_operation_output_via_read_payload_dependency() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    let template = WorkflowTemplate {
        id: "template.runtime.operation_output_read_payload".to_string(),
        version: "v1".to_string(),
        params: BTreeMap::new(),
        node_lock_scope_contexts: BTreeMap::new(),
        policy: WorkflowTemplatePolicy::default(),
        artifact_contracts: Vec::new(),
        nodes: vec![
            WorkflowNode {
                id: "emit_message".to_string(),
                name: None,
                kind: WorkflowNodeKind::Shell,
                uses: "cap.env.shell.command.run".to_string(),
                args: BTreeMap::from([(
                    "script".to_string(),
                    "printf 'runtime operation output\\n' > op-output.txt && git add op-output.txt && printf 'commit-message-from-operation-output'".to_string(),
                )]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges {
                    succeeded: vec!["stage_commit".to_string()],
                    ..WorkflowOutcomeEdges::default()
                },
            },
            WorkflowNode {
                id: "stage_commit".to_string(),
                name: None,
                kind: WorkflowNodeKind::Builtin,
                uses: "cap.env.builtin.git.commit".to_string(),
                args: BTreeMap::from([(
                    "message".to_string(),
                    "read_payload(emit_message)".to_string(),
                )]),
                after: Vec::new(),
                needs: vec![JobArtifact::Custom {
                    type_id: OPERATION_OUTPUT_ARTIFACT_TYPE_ID.to_string(),
                    key: "emit_message".to_string(),
                }],
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges::default(),
            },
        ],
    };

    let enqueue = enqueue_workflow_run(
        project_root,
        &jobs_root,
        "run-operation-output-read",
        "template.runtime.operation_output_read_payload@v1",
        &template,
        &[
            "vizier".to_string(),
            "jobs".to_string(),
            "schedule".to_string(),
        ],
        None,
    )
    .expect("enqueue run");
    let emit_job = enqueue
        .job_ids
        .get("emit_message")
        .expect("emit job id")
        .clone();
    let commit_job = enqueue
        .job_ids
        .get("stage_commit")
        .expect("commit job id")
        .clone();

    update_job_record(&jobs_root, &emit_job, |record| {
        record.status = JobStatus::Running;
        record.started_at = Some(Utc::now());
        record.pid = Some(std::process::id());
    })
    .expect("mark emit running");
    let emit_exit = execute_workflow_node_job(project_root, &jobs_root, &emit_job)
        .expect("execute emit_message");
    assert_eq!(emit_exit, 0);

    update_job_record(&jobs_root, &commit_job, |record| {
        record.status = JobStatus::Running;
        record.started_at = Some(Utc::now());
        record.pid = Some(std::process::id());
    })
    .expect("mark commit running");
    let commit_exit = execute_workflow_node_job(project_root, &jobs_root, &commit_job)
        .expect("execute stage_commit");
    assert_eq!(commit_exit, 0);

    let head =
        git_output(project_root, &["log", "-1", "--pretty=%B"]).expect("read head commit message");
    let message = String::from_utf8_lossy(&head);
    assert!(
        message.contains("commit-message-from-operation-output"),
        "expected read_payload(commit_message) to consume operation output text: {message}"
    );
}

#[test]
fn workflow_runtime_prompt_payload_roundtrip() {
    let _guard = agent_shim_env_lock().lock().expect("lock agent shim env");
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");
    let shims = create_mock_agent_shims(project_root).expect("create mock agent shims");
    let _shim_env = EnvVarGuard::set_path("VIZIER_AGENT_SHIMS_DIR", &shims);

    let template = prompt_invoke_template();
    let result = enqueue_workflow_run(
        project_root,
        &jobs_root,
        "run-prompt",
        "template.runtime.prompt_invoke@v1",
        &template,
        &[
            "vizier".to_string(),
            "jobs".to_string(),
            "schedule".to_string(),
        ],
        None,
    )
    .expect("enqueue workflow run");
    let manifest =
        load_workflow_run_manifest(project_root, "run-prompt").expect("workflow manifest");

    let resolve_job = result
        .job_ids
        .get("resolve_prompt")
        .expect("resolve job id")
        .clone();
    let resolve_record = read_record(&jobs_root, &resolve_job).expect("resolve record");
    let resolve_node = manifest
        .nodes
        .get("resolve_prompt")
        .expect("resolve node manifest");
    let resolve_result =
        execute_workflow_executor(project_root, &jobs_root, &resolve_record, resolve_node)
            .expect("execute prompt.resolve");
    assert_eq!(resolve_result.outcome, WorkflowNodeOutcome::Succeeded);
    assert_eq!(resolve_result.payload_refs.len(), 1);
    let payload_ref = project_root.join(resolve_result.payload_refs[0].as_str());
    assert!(payload_ref.exists(), "expected payload file to exist");
    let (status, exit_code) =
        map_workflow_outcome_to_job_status(resolve_result.outcome, resolve_result.exit_code);
    let _ = finalize_job_with_artifacts(
        project_root,
        &jobs_root,
        &resolve_job,
        status,
        exit_code,
        None,
        Some(JobMetadata::default()),
        Some(&resolve_result.artifacts_written),
    )
    .expect("finalize resolve");

    let invoke_job = result
        .job_ids
        .get("invoke_agent")
        .expect("invoke job id")
        .clone();
    let invoke_record = read_record(&jobs_root, &invoke_job).expect("invoke record");
    let invoke_node = manifest
        .nodes
        .get("invoke_agent")
        .expect("invoke node manifest");
    let invoke_result =
        execute_workflow_executor(project_root, &jobs_root, &invoke_record, invoke_node)
            .expect("execute agent.invoke");
    assert_eq!(invoke_result.outcome, WorkflowNodeOutcome::Succeeded);
    assert_eq!(invoke_result.payload_refs.len(), 1);
}

#[test]
fn workflow_runtime_prompt_resolve_renders_template_placeholders() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    let narrative_dir = project_root.join(".vizier/narrative");
    fs::create_dir_all(&narrative_dir).expect("create narrative dir");
    fs::write(
        narrative_dir.join("snapshot.md"),
        "Snapshot focus: scheduler runtime.\n",
    )
    .expect("write snapshot");

    let template = WorkflowTemplate {
        id: "template.runtime.prompt_placeholders".to_string(),
        version: "v1".to_string(),
        params: BTreeMap::new(),
        node_lock_scope_contexts: BTreeMap::new(),
        policy: WorkflowTemplatePolicy::default(),
        artifact_contracts: vec![WorkflowArtifactContract {
            id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
            version: "v1".to_string(),
            schema: None,
        }],
        nodes: vec![
            WorkflowNode {
                id: "resolve_prompt".to_string(),
                name: None,
                kind: WorkflowNodeKind::Builtin,
                uses: "cap.env.builtin.prompt.resolve".to_string(),
                args: BTreeMap::from([
                    (
                        "prompt_text".to_string(),
                        "slug={{persist_plan.name_override}}\nlocal={{local_value}}\nsnapshot={{file:.vizier/narrative/snapshot.md}}\nspec={{persist_plan.spec_text}}\n".to_string(),
                    ),
                    ("local_value".to_string(), "from-resolve".to_string()),
                ]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts {
                    succeeded: vec![JobArtifact::Custom {
                        type_id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
                        key: "draft_main".to_string(),
                    }],
                    ..WorkflowOutcomeArtifacts::default()
                },
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges::default(),
            },
            WorkflowNode {
                id: "persist_plan".to_string(),
                name: None,
                kind: WorkflowNodeKind::Builtin,
                uses: "cap.env.builtin.plan.persist".to_string(),
                args: BTreeMap::from([
                    ("spec_source".to_string(), "inline".to_string()),
                    ("spec_text".to_string(), "Spec body from node".to_string()),
                    ("name_override".to_string(), "jobs".to_string()),
                ]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges::default(),
            },
        ],
    };

    let result = enqueue_workflow_run(
        project_root,
        &jobs_root,
        "run-prompt-placeholders",
        "template.runtime.prompt_placeholders@v1",
        &template,
        &[
            "vizier".to_string(),
            "jobs".to_string(),
            "schedule".to_string(),
        ],
        None,
    )
    .expect("enqueue workflow run");
    let manifest = load_workflow_run_manifest(project_root, "run-prompt-placeholders")
        .expect("workflow manifest");

    let resolve_job = result
        .job_ids
        .get("resolve_prompt")
        .expect("resolve job id")
        .clone();
    let resolve_record = read_record(&jobs_root, &resolve_job).expect("resolve record");
    let resolve_node = manifest
        .nodes
        .get("resolve_prompt")
        .expect("resolve node manifest");
    let resolve_result =
        execute_workflow_executor(project_root, &jobs_root, &resolve_record, resolve_node)
            .expect("execute prompt.resolve");
    assert_eq!(resolve_result.outcome, WorkflowNodeOutcome::Succeeded);
    assert_eq!(resolve_result.payload_refs.len(), 1);

    let payload_ref = project_root.join(resolve_result.payload_refs[0].as_str());
    let payload_raw = fs::read_to_string(payload_ref).expect("read payload");
    let payload: serde_json::Value =
        serde_json::from_str(&payload_raw).expect("parse payload json");
    assert_eq!(
        payload.get("text").and_then(|value| value.as_str()),
        Some(
            "slug=jobs\nlocal=from-resolve\nsnapshot=Snapshot focus: scheduler runtime.\n\nspec=Spec body from node\n"
        )
    );
}

#[test]
fn workflow_runtime_prompt_resolve_supports_composed_namespace_aliases() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    let template = WorkflowTemplate {
        id: "template.runtime.prompt_namespace_aliases".to_string(),
        version: "v1".to_string(),
        params: BTreeMap::new(),
        node_lock_scope_contexts: BTreeMap::new(),
        policy: WorkflowTemplatePolicy::default(),
        artifact_contracts: vec![WorkflowArtifactContract {
            id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
            version: "v1".to_string(),
            schema: None,
        }],
        nodes: vec![
            WorkflowNode {
                id: "develop_draft__resolve_prompt".to_string(),
                name: None,
                kind: WorkflowNodeKind::Builtin,
                uses: "cap.env.builtin.prompt.resolve".to_string(),
                args: BTreeMap::from([(
                    "prompt_text".to_string(),
                    "local={{persist_plan.name_override}}\nunique={{persist_meta.name_override}}\nspec={{persist_plan.spec_text}}\n"
                        .to_string(),
                )]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts {
                    succeeded: vec![JobArtifact::Custom {
                        type_id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
                        key: "draft_main".to_string(),
                    }],
                    ..WorkflowOutcomeArtifacts::default()
                },
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges::default(),
            },
            WorkflowNode {
                id: "develop_draft__persist_plan".to_string(),
                name: None,
                kind: WorkflowNodeKind::Builtin,
                uses: "cap.env.builtin.plan.persist".to_string(),
                args: BTreeMap::from([
                    ("spec_source".to_string(), "inline".to_string()),
                    ("spec_text".to_string(), "Spec body from namespace".to_string()),
                    ("name_override".to_string(), "run".to_string()),
                ]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges::default(),
            },
            WorkflowNode {
                id: "develop_merge__persist_plan".to_string(),
                name: None,
                kind: WorkflowNodeKind::Builtin,
                uses: "cap.env.builtin.plan.persist".to_string(),
                args: BTreeMap::from([
                    ("spec_source".to_string(), "inline".to_string()),
                    ("spec_text".to_string(), "Spec body from other namespace".to_string()),
                    ("name_override".to_string(), "other".to_string()),
                ]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges::default(),
            },
            WorkflowNode {
                id: "develop_shared__persist_meta".to_string(),
                name: None,
                kind: WorkflowNodeKind::Builtin,
                uses: "cap.env.builtin.plan.persist".to_string(),
                args: BTreeMap::from([
                    ("spec_source".to_string(), "inline".to_string()),
                    (
                        "spec_text".to_string(),
                        "Spec body for unique suffix alias".to_string(),
                    ),
                    ("name_override".to_string(), "shared".to_string()),
                ]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges::default(),
            },
        ],
    };

    let result = enqueue_workflow_run(
        project_root,
        &jobs_root,
        "run-prompt-namespace-aliases",
        "template.runtime.prompt_namespace_aliases@v1",
        &template,
        &[
            "vizier".to_string(),
            "jobs".to_string(),
            "schedule".to_string(),
        ],
        None,
    )
    .expect("enqueue workflow run");
    let manifest = load_workflow_run_manifest(project_root, "run-prompt-namespace-aliases")
        .expect("workflow manifest");

    let resolve_job = result
        .job_ids
        .get("develop_draft__resolve_prompt")
        .expect("resolve job id")
        .clone();
    let resolve_record = read_record(&jobs_root, &resolve_job).expect("resolve record");
    let resolve_node = manifest
        .nodes
        .get("develop_draft__resolve_prompt")
        .expect("resolve node manifest");
    let resolve_result =
        execute_workflow_executor(project_root, &jobs_root, &resolve_record, resolve_node)
            .expect("execute prompt.resolve");
    assert_eq!(resolve_result.outcome, WorkflowNodeOutcome::Succeeded);
    assert_eq!(resolve_result.payload_refs.len(), 1);

    let payload_ref = project_root.join(resolve_result.payload_refs[0].as_str());
    let payload_raw = fs::read_to_string(payload_ref).expect("read payload");
    let payload: serde_json::Value =
        serde_json::from_str(&payload_raw).expect("parse payload json");
    assert_eq!(
        payload.get("text").and_then(|value| value.as_str()),
        Some("local=run\nunique=shared\nspec=Spec body from namespace\n")
    );
}

#[test]
fn workflow_runtime_prompt_resolve_uses_na_for_missing_narrative_placeholder_in_ephemeral_run() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    let template = WorkflowTemplate {
        id: "template.runtime.prompt_missing_narrative".to_string(),
        version: "v1".to_string(),
        params: BTreeMap::new(),
        node_lock_scope_contexts: BTreeMap::new(),
        policy: WorkflowTemplatePolicy::default(),
        artifact_contracts: vec![WorkflowArtifactContract {
            id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
            version: "v1".to_string(),
            schema: None,
        }],
        nodes: vec![WorkflowNode {
            id: "resolve_prompt".to_string(),
            name: None,
            kind: WorkflowNodeKind::Builtin,
            uses: "cap.env.builtin.prompt.resolve".to_string(),
            args: BTreeMap::from([(
                "prompt_text".to_string(),
                "snapshot={{file:.vizier/narrative/snapshot.md}}\n".to_string(),
            )]),
            after: Vec::new(),
            needs: Vec::new(),
            produces: WorkflowOutcomeArtifacts {
                succeeded: vec![JobArtifact::Custom {
                    type_id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
                    key: "draft_main".to_string(),
                }],
                ..WorkflowOutcomeArtifacts::default()
            },
            locks: Vec::new(),
            preconditions: Vec::new(),
            gates: Vec::new(),
            retry: Default::default(),
            on: WorkflowOutcomeEdges::default(),
        }],
    };

    let result = enqueue_workflow_run_with_options(
        project_root,
        &jobs_root,
        "run-prompt-missing-narrative",
        "template.runtime.prompt_missing_narrative@v1",
        &template,
        &[
            "vizier".to_string(),
            "jobs".to_string(),
            "schedule".to_string(),
        ],
        None,
        WorkflowRunEnqueueOptions {
            ephemeral: true,
            vizier_root_existed_before_runtime: Some(false),
        },
    )
    .expect("enqueue workflow run");
    let manifest = load_workflow_run_manifest(project_root, "run-prompt-missing-narrative")
        .expect("workflow manifest");

    let resolve_job = result
        .job_ids
        .get("resolve_prompt")
        .expect("resolve job id")
        .clone();
    let resolve_record = read_record(&jobs_root, &resolve_job).expect("resolve record");
    let resolve_node = manifest
        .nodes
        .get("resolve_prompt")
        .expect("resolve node manifest");
    let resolve_result =
        execute_workflow_executor(project_root, &jobs_root, &resolve_record, resolve_node)
            .expect("execute prompt.resolve");
    assert_eq!(resolve_result.outcome, WorkflowNodeOutcome::Succeeded);
    assert!(
        resolve_result
            .stderr_lines
            .iter()
            .any(|line| line.contains("substituting N/A for ephemeral run")),
        "expected prompt.resolve fallback notice: {:?}",
        resolve_result.stderr_lines
    );

    let payload_ref = project_root.join(resolve_result.payload_refs[0].as_str());
    let payload_raw = fs::read_to_string(payload_ref).expect("read payload");
    let payload: serde_json::Value =
        serde_json::from_str(&payload_raw).expect("parse payload json");
    assert_eq!(
        payload.get("text").and_then(|value| value.as_str()),
        Some("snapshot=N/A\n")
    );
}

#[test]
fn workflow_runtime_prompt_resolve_fails_on_unresolved_placeholder() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    let template = WorkflowTemplate {
        id: "template.runtime.prompt_unresolved".to_string(),
        version: "v1".to_string(),
        params: BTreeMap::new(),
        node_lock_scope_contexts: BTreeMap::new(),
        policy: WorkflowTemplatePolicy::default(),
        artifact_contracts: vec![WorkflowArtifactContract {
            id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
            version: "v1".to_string(),
            schema: None,
        }],
        nodes: vec![
            WorkflowNode {
                id: "resolve_prompt".to_string(),
                name: None,
                kind: WorkflowNodeKind::Builtin,
                uses: "cap.env.builtin.prompt.resolve".to_string(),
                args: BTreeMap::from([(
                    "prompt_text".to_string(),
                    "missing={{persist_plan.missing_value}}".to_string(),
                )]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts {
                    succeeded: vec![JobArtifact::Custom {
                        type_id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
                        key: "draft_main".to_string(),
                    }],
                    ..WorkflowOutcomeArtifacts::default()
                },
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges::default(),
            },
            WorkflowNode {
                id: "persist_plan".to_string(),
                name: None,
                kind: WorkflowNodeKind::Builtin,
                uses: "cap.env.builtin.plan.persist".to_string(),
                args: BTreeMap::from([
                    ("spec_source".to_string(), "inline".to_string()),
                    ("spec_text".to_string(), "Spec body".to_string()),
                    ("name_override".to_string(), "jobs".to_string()),
                ]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: WorkflowOutcomeArtifacts::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: WorkflowOutcomeEdges::default(),
            },
        ],
    };

    let result = enqueue_workflow_run(
        project_root,
        &jobs_root,
        "run-prompt-unresolved",
        "template.runtime.prompt_unresolved@v1",
        &template,
        &[
            "vizier".to_string(),
            "jobs".to_string(),
            "schedule".to_string(),
        ],
        None,
    )
    .expect("enqueue workflow run");
    let manifest = load_workflow_run_manifest(project_root, "run-prompt-unresolved")
        .expect("workflow manifest");

    let resolve_job = result
        .job_ids
        .get("resolve_prompt")
        .expect("resolve job id")
        .clone();
    let resolve_record = read_record(&jobs_root, &resolve_job).expect("resolve record");
    let resolve_node = manifest
        .nodes
        .get("resolve_prompt")
        .expect("resolve node manifest");
    let err = execute_workflow_executor(project_root, &jobs_root, &resolve_record, resolve_node)
        .expect_err("expected unresolved placeholder failure");
    assert!(
        err.to_string().contains("unresolved placeholder"),
        "unexpected error: {err}"
    );
}

#[test]
fn run_workflow_node_command_finalizes_job_when_executor_errors() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    let template = WorkflowTemplate {
        id: "template.runtime.prompt_missing_file".to_string(),
        version: "v1".to_string(),
        params: BTreeMap::new(),
        node_lock_scope_contexts: BTreeMap::new(),
        policy: WorkflowTemplatePolicy::default(),
        artifact_contracts: vec![WorkflowArtifactContract {
            id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
            version: "v1".to_string(),
            schema: None,
        }],
        nodes: vec![WorkflowNode {
            id: "resolve_prompt".to_string(),
            name: None,
            kind: WorkflowNodeKind::Builtin,
            uses: "cap.env.builtin.prompt.resolve".to_string(),
            args: BTreeMap::from([(
                "prompt_file".to_string(),
                "__missing_prompt__.md".to_string(),
            )]),
            after: Vec::new(),
            needs: Vec::new(),
            produces: WorkflowOutcomeArtifacts {
                succeeded: vec![JobArtifact::Custom {
                    type_id: PROMPT_ARTIFACT_TYPE_ID.to_string(),
                    key: "draft_main".to_string(),
                }],
                ..WorkflowOutcomeArtifacts::default()
            },
            locks: Vec::new(),
            preconditions: Vec::new(),
            gates: Vec::new(),
            retry: Default::default(),
            on: WorkflowOutcomeEdges::default(),
        }],
    };

    let enqueue = enqueue_workflow_run(
        project_root,
        &jobs_root,
        "run-runtime-error",
        "template.runtime.prompt_missing_file@v1",
        &template,
        &["vizier".to_string(), "__workflow-node".to_string()],
        None,
    )
    .expect("enqueue workflow run");
    let resolve_job = enqueue
        .job_ids
        .get("resolve_prompt")
        .expect("resolve job id")
        .clone();

    update_job_record(&jobs_root, &resolve_job, |record| {
        record.status = JobStatus::Running;
        record.started_at = Some(Utc::now());
        record.pid = Some(std::process::id());
    })
    .expect("mark running");

    let err = run_workflow_node_command(project_root, &jobs_root, &resolve_job)
        .expect_err("missing prompt file should fail workflow node");
    let err_text = err.to_string();
    assert!(
        err_text.contains("No such file or directory")
            || err_text.contains("__missing_prompt__.md")
            || err_text.contains("prompt.resolve"),
        "expected executor error details: {err_text}"
    );

    let record = read_record(&jobs_root, &resolve_job).expect("resolve record");
    assert_eq!(record.status, JobStatus::Failed);
    assert_eq!(record.exit_code, Some(1));
    assert!(record.finished_at.is_some());
    let metadata = record.metadata.as_ref().expect("metadata");
    assert_eq!(metadata.workflow_node_outcome.as_deref(), Some("failed"));
}

#[test]
fn workflow_runtime_worktree_prepare_and_cleanup_manage_owned_paths() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-worktree-runtime",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");
    let record = read_record(&jobs_root, "job-worktree-runtime").expect("record");

    let prepare = runtime_executor_node(
        "prepare",
        "job-worktree-runtime",
        "cap.env.builtin.worktree.prepare",
        "worktree.prepare",
        BTreeMap::from([("branch".to_string(), "draft/worktree-runtime".to_string())]),
    );
    let prepare_result =
        execute_workflow_executor(project_root, &jobs_root, &record, &prepare).expect("prepare");
    assert_eq!(prepare_result.outcome, WorkflowNodeOutcome::Succeeded);
    let prepare_meta = prepare_result.metadata.clone().expect("worktree metadata");
    assert_eq!(
        prepare_meta.execution_root.as_deref(),
        prepare_meta.worktree_path.as_deref()
    );
    let worktree_rel = prepare_meta
        .worktree_path
        .as_deref()
        .expect("worktree path metadata");
    let worktree_abs = resolve_recorded_path(project_root, worktree_rel);
    assert!(worktree_abs.exists(), "expected worktree path to exist");

    let mut cleanup_record = record.clone();
    cleanup_record.metadata = Some(prepare_meta);
    let cleanup = runtime_executor_node(
        "cleanup",
        "job-worktree-runtime",
        "cap.env.builtin.worktree.cleanup",
        "worktree.cleanup",
        BTreeMap::new(),
    );
    let cleanup_result =
        execute_workflow_executor(project_root, &jobs_root, &cleanup_record, &cleanup)
            .expect("cleanup");
    assert_eq!(cleanup_result.outcome, WorkflowNodeOutcome::Succeeded);
    let cleanup_meta = cleanup_result.metadata.clone().expect("cleanup metadata");
    assert_eq!(cleanup_meta.execution_root.as_deref(), Some("."));
    assert_eq!(cleanup_meta.worktree_owned, Some(false));
    assert!(
        !worktree_abs.exists(),
        "expected owned worktree directory to be removed"
    );
}

#[test]
fn workflow_runtime_worktree_prepare_derives_branch_from_slug_when_branch_missing() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-worktree-slug-derived",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");
    let record = read_record(&jobs_root, "job-worktree-slug-derived").expect("record");

    let prepare = runtime_executor_node(
        "prepare",
        "job-worktree-slug-derived",
        "cap.env.builtin.worktree.prepare",
        "worktree.prepare",
        BTreeMap::from([("slug".to_string(), "worktree-slug-derived".to_string())]),
    );
    let prepare_result =
        execute_workflow_executor(project_root, &jobs_root, &record, &prepare).expect("prepare");
    assert_eq!(prepare_result.outcome, WorkflowNodeOutcome::Succeeded);
    let prepare_meta = prepare_result.metadata.clone().expect("worktree metadata");
    assert_eq!(
        prepare_meta.branch.as_deref(),
        Some("draft/worktree-slug-derived")
    );

    let worktree_rel = prepare_meta
        .worktree_path
        .as_deref()
        .expect("worktree path metadata");
    let worktree_abs = resolve_recorded_path(project_root, worktree_rel);
    assert!(worktree_abs.exists(), "expected worktree path to exist");

    let mut cleanup_record = record.clone();
    cleanup_record.metadata = Some(prepare_meta);
    let cleanup = runtime_executor_node(
        "cleanup",
        "job-worktree-slug-derived",
        "cap.env.builtin.worktree.cleanup",
        "worktree.cleanup",
        BTreeMap::new(),
    );
    let cleanup_result =
        execute_workflow_executor(project_root, &jobs_root, &cleanup_record, &cleanup)
            .expect("cleanup");
    assert_eq!(cleanup_result.outcome, WorkflowNodeOutcome::Succeeded);
    assert!(
        !worktree_abs.exists(),
        "expected owned worktree directory to be removed"
    );
}

#[test]
fn workflow_runtime_worktree_prepare_allows_branch_in_multiple_worktrees() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-worktree-shared-1",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue shared 1");
    enqueue_job(
        project_root,
        &jobs_root,
        "job-worktree-shared-2",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue shared 2");

    let record_one = read_record(&jobs_root, "job-worktree-shared-1").expect("record one");
    let prepare_one = runtime_executor_node(
        "prepare_one",
        "job-worktree-shared-1",
        "cap.env.builtin.worktree.prepare",
        "worktree.prepare",
        BTreeMap::from([("branch".to_string(), "draft/worktree-shared".to_string())]),
    );
    let prepared_one =
        execute_workflow_executor(project_root, &jobs_root, &record_one, &prepare_one)
            .expect("prepare one");
    assert_eq!(prepared_one.outcome, WorkflowNodeOutcome::Succeeded);
    let prepare_one_meta = prepared_one.metadata.clone().expect("prepare one metadata");
    let worktree_one = prepare_one_meta
        .worktree_path
        .as_deref()
        .map(|path| resolve_recorded_path(project_root, path))
        .expect("worktree one path");
    assert!(worktree_one.exists(), "worktree one should exist");

    let record_two = read_record(&jobs_root, "job-worktree-shared-2").expect("record two");
    let prepare_two = runtime_executor_node(
        "prepare_two",
        "job-worktree-shared-2",
        "cap.env.builtin.worktree.prepare",
        "worktree.prepare",
        BTreeMap::from([("branch".to_string(), "draft/worktree-shared".to_string())]),
    );
    let prepared_two =
        execute_workflow_executor(project_root, &jobs_root, &record_two, &prepare_two)
            .expect("prepare two");
    assert_eq!(prepared_two.outcome, WorkflowNodeOutcome::Succeeded);
    let prepare_two_meta = prepared_two.metadata.clone().expect("prepare two metadata");
    let worktree_two = prepare_two_meta
        .worktree_path
        .as_deref()
        .map(|path| resolve_recorded_path(project_root, path))
        .expect("worktree two path");
    assert!(worktree_two.exists(), "worktree two should exist");
    assert_ne!(
        worktree_one, worktree_two,
        "shared branch prepares should still use distinct worktree paths"
    );

    let cleanup = runtime_executor_node(
        "cleanup",
        "job-worktree-shared-1",
        "cap.env.builtin.worktree.cleanup",
        "worktree.cleanup",
        BTreeMap::new(),
    );
    let mut cleanup_record_one = record_one.clone();
    cleanup_record_one.metadata = Some(prepare_one_meta);
    let cleaned_one =
        execute_workflow_executor(project_root, &jobs_root, &cleanup_record_one, &cleanup)
            .expect("cleanup one");
    assert_eq!(cleaned_one.outcome, WorkflowNodeOutcome::Succeeded);

    let mut cleanup_record_two = record_two.clone();
    cleanup_record_two.metadata = Some(prepare_two_meta);
    let cleaned_two =
        execute_workflow_executor(project_root, &jobs_root, &cleanup_record_two, &cleanup)
            .expect("cleanup two");
    assert_eq!(cleaned_two.outcome, WorkflowNodeOutcome::Succeeded);
    assert!(!worktree_one.exists(), "worktree one should be removed");
    assert!(!worktree_two.exists(), "worktree two should be removed");
}

#[test]
fn workflow_runtime_worktree_prepare_fails_without_branch_or_slug() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-worktree-no-branch",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");
    let record = read_record(&jobs_root, "job-worktree-no-branch").expect("record");

    let prepare = runtime_executor_node(
        "prepare",
        "job-worktree-no-branch",
        "cap.env.builtin.worktree.prepare",
        "worktree.prepare",
        BTreeMap::new(),
    );
    let prepare_result =
        execute_workflow_executor(project_root, &jobs_root, &record, &prepare).expect("prepare");
    assert_eq!(prepare_result.outcome, WorkflowNodeOutcome::Failed);
    assert_eq!(
        prepare_result.summary.as_deref(),
        Some("worktree.prepare could not determine branch (set branch or slug/plan)")
    );
}

#[test]
fn resolve_execution_root_prefers_execution_root_and_validates_repo_bounds() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let canonical_root = project_root.canonicalize().expect("canonical repo root");

    let worktree_rel = ".vizier/tmp-worktrees/root-precedence";
    let worktree_abs = project_root.join(worktree_rel);
    fs::create_dir_all(&worktree_abs).expect("create worktree path");
    let canonical_worktree = worktree_abs
        .canonicalize()
        .expect("canonical worktree root");

    let mut record = JobRecord {
        id: "job-root-precedence".to_string(),
        status: JobStatus::Queued,
        command: vec!["vizier".to_string(), "__workflow-node".to_string()],
        child_args: Vec::new(),
        created_at: Utc::now(),
        started_at: None,
        finished_at: None,
        pid: None,
        exit_code: None,
        stdout_path: ".vizier/jobs/job-root-precedence/stdout.log".to_string(),
        stderr_path: ".vizier/jobs/job-root-precedence/stderr.log".to_string(),
        session_path: None,
        outcome_path: None,
        metadata: Some(JobMetadata {
            execution_root: Some(".".to_string()),
            worktree_path: Some(worktree_rel.to_string()),
            ..JobMetadata::default()
        }),
        config_snapshot: None,
        schedule: None,
    };

    let resolved =
        resolve_execution_root(project_root, &record).expect("resolve from explicit root");
    assert_eq!(resolved, canonical_root);

    if let Some(metadata) = record.metadata.as_mut() {
        metadata.execution_root = Some(worktree_rel.to_string());
        metadata.worktree_path = Some(".".to_string());
    }
    let resolved =
        resolve_execution_root(project_root, &record).expect("resolve from execution_root");
    assert_eq!(resolved, canonical_worktree);

    if let Some(metadata) = record.metadata.as_mut() {
        metadata.execution_root = None;
        metadata.worktree_path = Some(worktree_rel.to_string());
    }
    let resolved =
        resolve_execution_root(project_root, &record).expect("resolve from worktree_path");
    assert_eq!(resolved, canonical_worktree);

    if let Some(metadata) = record.metadata.as_mut() {
        metadata.execution_root = Some("..".to_string());
        metadata.worktree_path = Some(worktree_rel.to_string());
    }
    let err = resolve_execution_root(project_root, &record)
        .expect_err("expected repo-boundary rejection");
    assert!(
        err.to_string().contains("outside repository root"),
        "expected out-of-repo rejection, got: {err}"
    );

    if let Some(metadata) = record.metadata.as_mut() {
        metadata.execution_root = Some("missing-root".to_string());
        metadata.worktree_path = Some(worktree_rel.to_string());
    }
    let err =
        resolve_execution_root(project_root, &record).expect_err("expected missing-root failure");
    assert!(
        err.to_string().contains("metadata.execution_root"),
        "expected explicit field validation error, got: {err}"
    );
}

#[test]
fn merge_metadata_clears_worktree_fields_when_cleanup_resets_root() {
    let existing = Some(JobMetadata {
        execution_root: Some(".vizier/tmp-worktrees/workflow".to_string()),
        worktree_name: Some("workflow-node".to_string()),
        worktree_path: Some(".vizier/tmp-worktrees/workflow".to_string()),
        worktree_owned: Some(true),
        ..JobMetadata::default()
    });
    let update = Some(JobMetadata {
        execution_root: Some(".".to_string()),
        worktree_owned: Some(false),
        retry_cleanup_status: Some(RetryCleanupStatus::Done),
        ..JobMetadata::default()
    });
    let merged = merge_metadata(existing, update).expect("merged metadata");
    assert_eq!(merged.execution_root.as_deref(), Some("."));
    assert!(merged.worktree_name.is_none());
    assert!(merged.worktree_path.is_none());
    assert!(merged.worktree_owned.is_none());
    assert_eq!(merged.retry_cleanup_status, Some(RetryCleanupStatus::Done));
}

#[test]
fn apply_workflow_execution_context_is_idempotent_and_skips_active_targets() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-target",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue target");
    let context = WorkflowExecutionContext {
        execution_root: Some(".vizier/tmp-worktrees/ctx-a".to_string()),
        worktree_path: Some(".vizier/tmp-worktrees/ctx-a".to_string()),
        worktree_name: Some("ctx-a".to_string()),
        worktree_owned: Some(true),
    };

    let first = apply_workflow_execution_context(&jobs_root, "job-target", &context, true)
        .expect("first propagation");
    assert!(first, "expected first propagation to update metadata");
    let second = apply_workflow_execution_context(&jobs_root, "job-target", &context, true)
        .expect("second propagation");
    assert!(!second, "expected unchanged propagation to be idempotent");

    update_job_record(&jobs_root, "job-target", |record| {
        record.status = JobStatus::Running;
    })
    .expect("mark target running");
    let changed_context = WorkflowExecutionContext {
        execution_root: Some(".vizier/tmp-worktrees/ctx-b".to_string()),
        worktree_path: Some(".vizier/tmp-worktrees/ctx-b".to_string()),
        worktree_name: Some("ctx-b".to_string()),
        worktree_owned: Some(true),
    };
    let active = apply_workflow_execution_context(&jobs_root, "job-target", &changed_context, true)
        .expect("active target propagation");
    assert!(
        !active,
        "expected propagation to skip active target metadata"
    );

    let target = read_record(&jobs_root, "job-target").expect("target record");
    let metadata = target.metadata.expect("target metadata");
    assert_eq!(metadata.execution_root, context.execution_root);
    assert_eq!(metadata.worktree_path, context.worktree_path);
    assert_eq!(metadata.worktree_name, context.worktree_name);
    assert_eq!(metadata.worktree_owned, context.worktree_owned);
}

#[test]
fn workflow_runtime_command_run_uses_worktree_then_repo_after_cleanup_reset() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-exec-root",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");
    let record = read_record(&jobs_root, "job-exec-root").expect("record");

    let prepare = runtime_executor_node(
        "prepare",
        "job-exec-root",
        "cap.env.builtin.worktree.prepare",
        "worktree.prepare",
        BTreeMap::from([(
            "branch".to_string(),
            "draft/execution-root-runtime".to_string(),
        )]),
    );
    let prepare_result =
        execute_workflow_executor(project_root, &jobs_root, &record, &prepare).expect("prepare");
    assert_eq!(prepare_result.outcome, WorkflowNodeOutcome::Succeeded);
    let prepare_meta = prepare_result.metadata.clone().expect("prepare metadata");
    let worktree_rel = prepare_meta
        .worktree_path
        .as_deref()
        .expect("worktree path metadata");
    let worktree_abs = resolve_recorded_path(project_root, worktree_rel);

    let mut in_worktree_record = record.clone();
    in_worktree_record.metadata = Some(prepare_meta.clone());
    let in_worktree = runtime_executor_node(
        "in-worktree",
        "job-exec-root",
        "cap.env.shell.command.run",
        "command.run",
        BTreeMap::from([(
            "script".to_string(),
            "echo from-worktree > marker-in-worktree.txt".to_string(),
        )]),
    );
    let in_worktree_result =
        execute_workflow_executor(project_root, &jobs_root, &in_worktree_record, &in_worktree)
            .expect("in-worktree command");
    assert_eq!(in_worktree_result.outcome, WorkflowNodeOutcome::Succeeded);
    assert!(
        worktree_abs.join("marker-in-worktree.txt").exists(),
        "expected marker in propagated worktree root"
    );
    assert!(
        !project_root.join("marker-in-worktree.txt").exists(),
        "worktree command should not write marker in repository root"
    );

    let cleanup = runtime_executor_node(
        "cleanup",
        "job-exec-root",
        "cap.env.builtin.worktree.cleanup",
        "worktree.cleanup",
        BTreeMap::new(),
    );
    let cleanup_result =
        execute_workflow_executor(project_root, &jobs_root, &in_worktree_record, &cleanup)
            .expect("cleanup");
    assert_eq!(cleanup_result.outcome, WorkflowNodeOutcome::Succeeded);
    let merged_meta = merge_metadata(Some(prepare_meta), cleanup_result.metadata.clone())
        .expect("merged cleanup metadata");
    assert_eq!(merged_meta.execution_root.as_deref(), Some("."));

    let mut repo_root_record = record.clone();
    repo_root_record.metadata = Some(merged_meta);
    let in_repo = runtime_executor_node(
        "in-repo",
        "job-exec-root",
        "cap.env.shell.command.run",
        "command.run",
        BTreeMap::from([(
            "script".to_string(),
            "echo from-repo > marker-in-repo.txt".to_string(),
        )]),
    );
    let in_repo_result =
        execute_workflow_executor(project_root, &jobs_root, &repo_root_record, &in_repo)
            .expect("repo command");
    assert_eq!(in_repo_result.outcome, WorkflowNodeOutcome::Succeeded);
    assert!(
        project_root.join("marker-in-repo.txt").exists(),
        "expected marker in repository root after cleanup reset"
    );
}

#[test]
fn workflow_runtime_plan_persist_writes_plan_doc_and_state() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-plan-persist",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");
    let record = read_record(&jobs_root, "job-plan-persist").expect("record");
    let node = runtime_executor_node(
        "persist",
        "job-plan-persist",
        "cap.env.builtin.plan.persist",
        "plan.persist",
        BTreeMap::from([
            ("name_override".to_string(), "runtime-plan".to_string()),
            ("spec_source".to_string(), "inline".to_string()),
            (
                "spec_text".to_string(),
                "Runtime operation completion spec".to_string(),
            ),
        ]),
    );
    let result =
        execute_workflow_executor(project_root, &jobs_root, &record, &node).expect("persist");
    assert_eq!(result.outcome, WorkflowNodeOutcome::Succeeded);
    assert!(
        result
            .artifacts_written
            .iter()
            .any(|artifact| matches!(artifact, JobArtifact::PlanBranch { .. })),
        "expected plan branch artifact"
    );
    assert!(
        result
            .artifacts_written
            .iter()
            .any(|artifact| matches!(artifact, JobArtifact::PlanDoc { .. })),
        "expected plan doc artifact"
    );
    let plan_doc = project_root.join(".vizier/implementation-plans/runtime-plan.md");
    assert!(plan_doc.exists(), "expected persisted plan doc");
    let state_ref = result
        .payload_refs
        .iter()
        .find(|entry| entry.contains(".vizier/state/plans/"))
        .cloned()
        .expect("plan state payload ref");
    assert!(
        project_root.join(state_ref).exists(),
        "expected persisted plan state"
    );
}

#[test]
fn workflow_runtime_plan_persist_prefers_plan_text_dependency_payload() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    let artifact = JobArtifact::Custom {
        type_id: PLAN_TEXT_ARTIFACT_TYPE_ID.to_string(),
        key: "draft_plan:runtime-plan".to_string(),
    };
    enqueue_job(
        project_root,
        &jobs_root,
        "job-plan-persist-from-artifact",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        None,
        None,
        Some(JobSchedule {
            dependencies: vec![JobDependency {
                artifact: artifact.clone(),
            }],
            ..JobSchedule::default()
        }),
    )
    .expect("enqueue");

    write_custom_artifact_payload(
        project_root,
        "job-agent-output",
        PLAN_TEXT_ARTIFACT_TYPE_ID,
        "draft_plan:runtime-plan",
        &serde_json::json!({
            "text": "- Generated from agent artifact"
        }),
    )
    .expect("write plan payload");
    write_custom_artifact_markers(
        project_root,
        "job-agent-output",
        std::slice::from_ref(&artifact),
    )
    .expect("write artifact marker");

    let record = read_record(&jobs_root, "job-plan-persist-from-artifact").expect("record");
    let node = runtime_executor_node(
        "persist",
        "job-plan-persist-from-artifact",
        "cap.env.builtin.plan.persist",
        "plan.persist",
        BTreeMap::from([
            ("name_override".to_string(), "runtime-plan".to_string()),
            ("spec_source".to_string(), "inline".to_string()),
            (
                "spec_text".to_string(),
                "Spec comes from operator".to_string(),
            ),
        ]),
    );
    let result = execute_workflow_executor(project_root, &jobs_root, &record, &node)
        .expect("execute plan.persist");
    assert_eq!(result.outcome, WorkflowNodeOutcome::Succeeded);

    let plan_doc =
        fs::read_to_string(project_root.join(".vizier/implementation-plans/runtime-plan.md"))
            .expect("read plan doc");
    assert!(
        plan_doc.contains("## Operator Spec\nSpec comes from operator"),
        "expected operator spec section to preserve input spec: {plan_doc}"
    );
    assert!(
        plan_doc.contains("## Implementation Plan\n- Generated from agent artifact"),
        "expected implementation plan body to come from custom plan_text dependency: {plan_doc}"
    );
}

#[test]
fn workflow_runtime_integrate_plan_branch_blocks_on_conflict_and_writes_sentinel() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");
    let target = current_branch_name(project_root).expect("target branch");

    fs::write(project_root.join("conflict.txt"), "base\n").expect("write base");
    git_commit_all(project_root, "base conflict");

    let checkout = git_status(project_root, &["checkout", "-b", "draft/runtime-conflict"]);
    assert!(checkout.is_ok(), "create draft branch: {checkout:?}");
    fs::write(project_root.join("conflict.txt"), "draft\n").expect("write draft");
    git_commit_all(project_root, "draft conflict");

    let checkout_target = git_status(project_root, &["checkout", &target]);
    assert!(
        checkout_target.is_ok(),
        "checkout target: {checkout_target:?}"
    );
    fs::write(project_root.join("conflict.txt"), "target\n").expect("write target");
    git_commit_all(project_root, "target conflict");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-integrate-conflict",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        Some(JobMetadata {
            plan: Some("runtime-conflict".to_string()),
            branch: Some("draft/runtime-conflict".to_string()),
            target: Some(target.clone()),
            ..JobMetadata::default()
        }),
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");
    let record = read_record(&jobs_root, "job-integrate-conflict").expect("record");
    let node = runtime_executor_node(
        "integrate",
        "job-integrate-conflict",
        "cap.env.builtin.git.integrate_plan_branch",
        "git.integrate_plan_branch",
        BTreeMap::from([
            ("branch".to_string(), "draft/runtime-conflict".to_string()),
            ("target_branch".to_string(), target),
            ("squash".to_string(), "false".to_string()),
        ]),
    );
    let result =
        execute_workflow_executor(project_root, &jobs_root, &record, &node).expect("integrate");
    assert_eq!(result.outcome, WorkflowNodeOutcome::Blocked);
    assert!(
        result
            .artifacts_written
            .iter()
            .any(|artifact| matches!(artifact, JobArtifact::MergeSentinel { .. })),
        "expected merge sentinel artifact"
    );
    let sentinel = project_root.join(".vizier/tmp/merge-conflicts/runtime-conflict.json");
    assert!(sentinel.exists(), "expected merge sentinel file");
}

#[test]
fn workflow_runtime_integrate_plan_branch_finalizes_resolved_merge_on_retry() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");
    let target = current_branch_name(project_root).expect("target branch");
    let source_branch = "draft/runtime-conflict-retry";

    fs::write(project_root.join("a"), "base\n").expect("write base");
    git_commit_all(project_root, "base conflict retry");

    let checkout = git_status(project_root, &["checkout", "-b", source_branch]);
    assert!(checkout.is_ok(), "create draft branch: {checkout:?}");
    fs::write(project_root.join("a"), "feature2\n").expect("write source change");
    git_commit_all(project_root, "draft conflict retry");

    let checkout_target = git_status(project_root, &["checkout", &target]);
    assert!(
        checkout_target.is_ok(),
        "checkout target: {checkout_target:?}"
    );
    fs::write(project_root.join("a"), "feature1\n").expect("write target change");
    git_commit_all(project_root, "target conflict retry");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-integrate-conflict-retry",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        Some(JobMetadata {
            plan: Some("runtime-conflict-retry".to_string()),
            branch: Some(source_branch.to_string()),
            target: Some(target.clone()),
            ..JobMetadata::default()
        }),
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");
    let record = read_record(&jobs_root, "job-integrate-conflict-retry").expect("record");
    let node = runtime_executor_node(
        "integrate",
        "job-integrate-conflict-retry",
        "cap.env.builtin.git.integrate_plan_branch",
        "git.integrate_plan_branch",
        BTreeMap::from([
            ("branch".to_string(), source_branch.to_string()),
            ("slug".to_string(), "runtime-conflict-retry".to_string()),
            ("target_branch".to_string(), target.clone()),
            ("squash".to_string(), "true".to_string()),
            ("delete_branch".to_string(), "false".to_string()),
        ]),
    );

    let blocked =
        execute_workflow_executor(project_root, &jobs_root, &record, &node).expect("integrate");
    assert_eq!(blocked.outcome, WorkflowNodeOutcome::Blocked);
    assert!(
        project_root
            .join(".vizier/tmp/merge-conflicts/runtime-conflict-retry.json")
            .exists(),
        "expected merge sentinel for retry coverage"
    );

    fs::write(project_root.join("a"), "feature1\nfeature2\n").expect("resolve conflict");
    crate::vcs::stage_in(project_root, Some(vec!["a"])).expect("stage resolved file");

    let retried = execute_workflow_executor(project_root, &jobs_root, &record, &node)
        .expect("retry integrate");
    assert_eq!(
        retried.outcome,
        WorkflowNodeOutcome::Succeeded,
        "retry integrate result: {:?}",
        retried
    );
    assert_eq!(
        fs::read_to_string(project_root.join("a")).ok().as_deref(),
        Some("feature1\nfeature2\n"),
        "expected resolved merge content to be finalized on retry"
    );
    assert_eq!(
        Repository::open(project_root).expect("open repo").state(),
        git2::RepositoryState::Clean,
        "expected retry to leave repository merge state clean"
    );
}

#[test]
fn workflow_runtime_integrate_plan_branch_derives_branch_from_slug_when_branch_missing() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");
    let target = current_branch_name(project_root).expect("target branch");

    let checkout = git_status(
        project_root,
        &["checkout", "-b", "draft/runtime-slug-merge"],
    );
    assert!(checkout.is_ok(), "create draft branch: {checkout:?}");
    fs::write(project_root.join("slug-merge.txt"), "from slug source\n")
        .expect("write source file");
    git_commit_all(project_root, "feat: slug merge source");
    let checkout_target = git_status(project_root, &["checkout", &target]);
    assert!(
        checkout_target.is_ok(),
        "checkout target: {checkout_target:?}"
    );

    enqueue_job(
        project_root,
        &jobs_root,
        "job-integrate-slug-derived",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");
    let record = read_record(&jobs_root, "job-integrate-slug-derived").expect("record");
    let node = runtime_executor_node(
        "integrate",
        "job-integrate-slug-derived",
        "cap.env.builtin.git.integrate_plan_branch",
        "git.integrate_plan_branch",
        BTreeMap::from([
            ("slug".to_string(), "runtime-slug-merge".to_string()),
            ("target_branch".to_string(), target),
            ("squash".to_string(), "false".to_string()),
        ]),
    );
    let result =
        execute_workflow_executor(project_root, &jobs_root, &record, &node).expect("integrate");
    assert_eq!(result.outcome, WorkflowNodeOutcome::Succeeded);
    assert_eq!(
        fs::read_to_string(project_root.join("slug-merge.txt"))
            .ok()
            .as_deref(),
        Some("from slug source\n"),
        "expected merge to include source branch changes"
    );
}

#[test]
fn workflow_runtime_integrate_plan_branch_embeds_plan_and_cleans_source_plan_doc() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");
    let target = current_branch_name(project_root).expect("target branch");
    let slug = "runtime-plan-embed";
    let source_branch = format!("draft/{slug}");
    let plan_rel = format!(".vizier/implementation-plans/{slug}.md");
    let plan_doc = format!(
        "---\nplan: {slug}\nbranch: {source_branch}\n---\n\n## Operator Spec\nRuntime merge test\n\n## Implementation Plan\n- Runtime merge step\n"
    );

    let checkout = git_status(project_root, &["checkout", "-b", &source_branch]);
    assert!(checkout.is_ok(), "create draft branch: {checkout:?}");
    fs::create_dir_all(project_root.join(".vizier/implementation-plans")).expect("create plan dir");
    fs::write(project_root.join(&plan_rel), plan_doc).expect("write plan doc");
    fs::write(
        project_root.join("runtime-plan-merge.txt"),
        "from runtime plan merge\n",
    )
    .expect("write source file");
    git_commit_all(project_root, "feat: prepare runtime plan merge");
    let checkout_target = git_status(project_root, &["checkout", &target]);
    assert!(
        checkout_target.is_ok(),
        "checkout target: {checkout_target:?}"
    );
    let occupied_worktree = project_root.join(".vizier/tmp-worktrees/plan-cleanup-occupied");
    if let Some(parent) = occupied_worktree.parent() {
        fs::create_dir_all(parent).expect("create occupied worktree parent");
    }
    crate::vcs::add_worktree_for_branch_in(
        project_root,
        "plan-cleanup-occupied",
        &occupied_worktree,
        &source_branch,
    )
    .expect("add occupied worktree");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-integrate-plan-embed",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        Some(JobMetadata {
            plan: Some(slug.to_string()),
            branch: Some(source_branch.clone()),
            target: Some(target.clone()),
            ..JobMetadata::default()
        }),
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");
    let record = read_record(&jobs_root, "job-integrate-plan-embed").expect("record");
    let node = runtime_executor_node(
        "integrate",
        "job-integrate-plan-embed",
        "cap.env.builtin.git.integrate_plan_branch",
        "git.integrate_plan_branch",
        BTreeMap::from([
            ("branch".to_string(), source_branch.clone()),
            ("slug".to_string(), slug.to_string()),
            ("target_branch".to_string(), target),
            ("squash".to_string(), "true".to_string()),
            ("delete_branch".to_string(), "false".to_string()),
        ]),
    );
    let result =
        execute_workflow_executor(project_root, &jobs_root, &record, &node).expect("integrate");
    assert_eq!(result.outcome, WorkflowNodeOutcome::Succeeded);

    let head =
        git_output(project_root, &["log", "-1", "--pretty=%B"]).expect("read head commit message");
    let message = String::from_utf8_lossy(&head);
    assert!(
        message.contains("feat: merge plan runtime-plan-embed"),
        "expected merge subject in message: {message}"
    );
    assert!(
        message.contains("## Implementation Plan"),
        "expected plan markdown embedded in merge message: {message}"
    );
    assert!(
        message.contains("- Runtime merge step"),
        "expected plan steps embedded in merge message: {message}"
    );

    let draft_tip = repo
        .find_branch(&source_branch, BranchType::Local)
        .expect("source branch exists")
        .get()
        .peel_to_commit()
        .expect("source tip");
    assert!(
        draft_tip
            .tree()
            .expect("source tree")
            .get_path(Path::new(&plan_rel))
            .is_err(),
        "expected source branch tip to remove plan doc before merge finalization"
    );
}

#[test]
fn workflow_runtime_integrate_plan_branch_commits_plan_only_history() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");
    let target = current_branch_name(project_root).expect("target branch");
    let before_head = repo
        .head()
        .and_then(|head| head.peel_to_commit())
        .expect("read initial head")
        .id();
    let slug = "runtime-plan-only-history";
    let source_branch = format!("draft/{slug}");
    let plan_rel = format!(".vizier/implementation-plans/{slug}.md");
    let plan_doc = format!(
        "---\nplan: {slug}\nbranch: {source_branch}\n---\n\n## Operator Spec\nPlan-only runtime merge test\n\n## Implementation Plan\n- Preserve history even when tree matches target\n"
    );

    let checkout = git_status(project_root, &["checkout", "-b", &source_branch]);
    assert!(checkout.is_ok(), "create draft branch: {checkout:?}");
    fs::create_dir_all(project_root.join(".vizier/implementation-plans")).expect("create plan dir");
    fs::write(project_root.join(&plan_rel), plan_doc).expect("write plan doc");
    git_commit_all(project_root, "feat: add plan-only history");
    let checkout_target = git_status(project_root, &["checkout", &target]);
    assert!(
        checkout_target.is_ok(),
        "checkout target: {checkout_target:?}"
    );

    enqueue_job(
        project_root,
        &jobs_root,
        "job-integrate-plan-only-history",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        Some(JobMetadata {
            plan: Some(slug.to_string()),
            branch: Some(source_branch.clone()),
            target: Some(target.clone()),
            ..JobMetadata::default()
        }),
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");
    let record = read_record(&jobs_root, "job-integrate-plan-only-history").expect("record");
    let node = runtime_executor_node(
        "integrate",
        "job-integrate-plan-only-history",
        "cap.env.builtin.git.integrate_plan_branch",
        "git.integrate_plan_branch",
        BTreeMap::from([
            ("branch".to_string(), source_branch.clone()),
            ("slug".to_string(), slug.to_string()),
            ("target_branch".to_string(), target.clone()),
            ("squash".to_string(), "true".to_string()),
            ("delete_branch".to_string(), "false".to_string()),
        ]),
    );
    let result =
        execute_workflow_executor(project_root, &jobs_root, &record, &node).expect("integrate");
    assert_eq!(result.outcome, WorkflowNodeOutcome::Succeeded);

    let head = repo
        .head()
        .and_then(|head| head.peel_to_commit())
        .expect("read head commit");
    assert_ne!(
        head.id(),
        before_head,
        "expected merge to materialize a target commit even when the merged tree matches HEAD"
    );
    assert_eq!(
        head.parent_count(),
        1,
        "squash merge should keep a single parent"
    );
    let message = head.message().unwrap_or_default();
    assert!(
        message.contains("feat: merge plan runtime-plan-only-history"),
        "expected merge subject in message: {message}"
    );
    assert!(
        message.contains("## Implementation Plan"),
        "expected embedded plan markdown in merge commit: {message}"
    );

    let draft_tip = repo
        .find_branch(&source_branch, BranchType::Local)
        .expect("source branch exists")
        .get()
        .peel_to_commit()
        .expect("source tip");
    assert!(
        draft_tip
            .tree()
            .expect("source tree")
            .get_path(Path::new(&plan_rel))
            .is_err(),
        "expected source branch tip to remove plan doc before merge finalization"
    );
}

#[test]
fn workflow_runtime_git_save_worktree_patch_writes_command_patch() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");
    fs::write(project_root.join("README.md"), "updated\n").expect("update readme");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-save-patch",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");
    let record = read_record(&jobs_root, "job-save-patch").expect("record");
    let node = runtime_executor_node(
        "save_patch",
        "job-save-patch",
        "cap.env.builtin.git.save_worktree_patch",
        "git.save_worktree_patch",
        BTreeMap::new(),
    );
    let result =
        execute_workflow_executor(project_root, &jobs_root, &record, &node).expect("save patch");
    assert_eq!(result.outcome, WorkflowNodeOutcome::Succeeded);
    let patch_path = command_patch_path(&jobs_root, "job-save-patch");
    assert!(patch_path.exists(), "expected command patch output");
}

#[test]
fn workflow_runtime_patch_pipeline_prepare_execute_and_finalize() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    fs::write(project_root.join("sample.txt"), "before\n").expect("seed sample");
    git_commit_all(project_root, "seed sample");
    fs::write(project_root.join("sample.txt"), "after\n").expect("edit sample");
    let patch_path = project_root.join("sample.patch");
    let diff = git_output(project_root, &["diff", "--binary", "HEAD"]).expect("build patch diff");
    fs::write(&patch_path, diff).expect("write patch file");
    let restore = git_status(project_root, &["checkout", "--", "sample.txt"]);
    assert!(restore.is_ok(), "restore sample: {restore:?}");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-patch-pipeline",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");
    let record = read_record(&jobs_root, "job-patch-pipeline").expect("record");
    let files_json =
        serde_json::to_string(&vec![patch_path.display().to_string()]).expect("serialize files");

    let prepare = runtime_executor_node(
        "patch_prepare",
        "job-patch-pipeline",
        "cap.env.builtin.patch.pipeline_prepare",
        "patch.pipeline_prepare",
        BTreeMap::from([("files_json".to_string(), files_json.clone())]),
    );
    let prepare_result =
        execute_workflow_executor(project_root, &jobs_root, &record, &prepare).expect("prepare");
    assert_eq!(prepare_result.outcome, WorkflowNodeOutcome::Succeeded);
    assert!(
        patch_pipeline_manifest_path(&jobs_root, "job-patch-pipeline").exists(),
        "expected pipeline manifest"
    );

    let execute = runtime_executor_node(
        "patch_execute",
        "job-patch-pipeline",
        "cap.env.builtin.patch.execute_pipeline",
        "patch.execute_pipeline",
        BTreeMap::from([("files_json".to_string(), files_json)]),
    );
    let execute_result =
        execute_workflow_executor(project_root, &jobs_root, &record, &execute).expect("execute");
    assert_eq!(
        execute_result.outcome,
        WorkflowNodeOutcome::Succeeded,
        "patch pipeline execute should succeed: {:?}",
        execute_result.summary
    );
    let staged =
        git_output(project_root, &["diff", "--cached", "--name-only"]).expect("read staged names");
    assert!(
        String::from_utf8_lossy(&staged).contains("sample.txt"),
        "expected patch application to stage sample.txt"
    );

    let finalize = runtime_executor_node(
        "patch_finalize",
        "job-patch-pipeline",
        "cap.env.builtin.patch.pipeline_finalize",
        "patch.pipeline_finalize",
        BTreeMap::new(),
    );
    let finalize_result =
        execute_workflow_executor(project_root, &jobs_root, &record, &finalize).expect("finalize");
    assert_eq!(finalize_result.outcome, WorkflowNodeOutcome::Succeeded);
    assert!(
        command_patch_path(&jobs_root, "job-patch-pipeline").exists(),
        "expected finalized command patch"
    );
    assert!(
        patch_pipeline_finalize_path(&jobs_root, "job-patch-pipeline").exists(),
        "expected finalize marker"
    );
}

#[test]
fn workflow_runtime_build_materialize_step_emits_artifacts() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-build-materialize",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");
    let record = read_record(&jobs_root, "job-build-materialize").expect("record");
    let node = runtime_executor_node(
        "materialize",
        "job-build-materialize",
        "cap.env.builtin.build.materialize_step",
        "build.materialize_step",
        BTreeMap::from([
            ("build_id".to_string(), "build-runtime".to_string()),
            ("step_key".to_string(), "s1".to_string()),
            ("slug".to_string(), "runtime-build".to_string()),
            ("branch".to_string(), "draft/runtime-build".to_string()),
            ("target".to_string(), "main".to_string()),
        ]),
    );
    let result =
        execute_workflow_executor(project_root, &jobs_root, &record, &node).expect("materialize");
    assert_eq!(result.outcome, WorkflowNodeOutcome::Succeeded);
    assert!(
        result
            .artifacts_written
            .iter()
            .any(|artifact| matches!(artifact, JobArtifact::PlanBranch { .. })),
        "expected plan branch artifact"
    );
    assert!(
        project_root
            .join(".vizier/implementation-plans/builds/build-runtime/steps/s1/materialized.json")
            .exists(),
        "expected build step materialized payload"
    );
}

#[test]
fn workflow_runtime_merge_sentinel_write_and_clear() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-sentinel",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        Some(JobMetadata {
            plan: Some("runtime-sentinel".to_string()),
            ..JobMetadata::default()
        }),
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");
    let record = read_record(&jobs_root, "job-sentinel").expect("record");

    let write_node = runtime_executor_node(
        "write_sentinel",
        "job-sentinel",
        "cap.env.builtin.merge.sentinel.write",
        "merge.sentinel.write",
        BTreeMap::new(),
    );
    let write_result =
        execute_workflow_executor(project_root, &jobs_root, &record, &write_node).expect("write");
    assert_eq!(write_result.outcome, WorkflowNodeOutcome::Succeeded);
    let sentinel = project_root.join(".vizier/tmp/merge-conflicts/runtime-sentinel.json");
    assert!(sentinel.exists(), "expected sentinel written");

    let clear_node = runtime_executor_node(
        "clear_sentinel",
        "job-sentinel",
        "cap.env.builtin.merge.sentinel.clear",
        "merge.sentinel.clear",
        BTreeMap::new(),
    );
    let clear_result =
        execute_workflow_executor(project_root, &jobs_root, &record, &clear_node).expect("clear");
    assert_eq!(clear_result.outcome, WorkflowNodeOutcome::Succeeded);
    assert!(!sentinel.exists(), "expected sentinel cleared");
}

#[test]
fn workflow_runtime_command_and_cicd_shell_ops_respect_exit_status() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-shell-op",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");
    let record = read_record(&jobs_root, "job-shell-op").expect("record");

    let command_ok = runtime_executor_node(
        "command_ok",
        "job-shell-op",
        "cap.env.shell.command.run",
        "command.run",
        BTreeMap::from([("script".to_string(), "printf ok".to_string())]),
    );
    let command_ok_result =
        execute_workflow_executor(project_root, &jobs_root, &record, &command_ok)
            .expect("command ok");
    assert_eq!(command_ok_result.outcome, WorkflowNodeOutcome::Succeeded);

    let command_fail = runtime_executor_node(
        "command_fail",
        "job-shell-op",
        "cap.env.shell.command.run",
        "command.run",
        BTreeMap::from([("script".to_string(), "exit 9".to_string())]),
    );
    let command_fail_result =
        execute_workflow_executor(project_root, &jobs_root, &record, &command_fail)
            .expect("command fail");
    assert_eq!(command_fail_result.outcome, WorkflowNodeOutcome::Failed);
    assert_eq!(command_fail_result.exit_code, Some(9));

    let cicd = runtime_executor_node(
        "cicd_ok",
        "job-shell-op",
        "cap.env.shell.cicd.run",
        "cicd.run",
        BTreeMap::from([("script".to_string(), "exit 0".to_string())]),
    );
    let cicd_result =
        execute_workflow_executor(project_root, &jobs_root, &record, &cicd).expect("cicd");
    assert_eq!(cicd_result.outcome, WorkflowNodeOutcome::Succeeded);
}

#[test]
fn workflow_runtime_conflict_cicd_approval_and_terminal_gates() {
    let temp = TempDir::new().expect("temp dir");
    let repo = init_repo(&temp).expect("init repo");
    seed_repo(&repo).expect("seed repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    fs::write(project_root.join("gate-conflict.txt"), "base\n").expect("write base");
    git_commit_all(project_root, "gate conflict base");
    let target = current_branch_name(project_root).expect("target branch");
    let checkout = git_status(project_root, &["checkout", "-b", "draft/gate-conflict"]);
    assert!(checkout.is_ok(), "create draft gate branch: {checkout:?}");
    fs::write(project_root.join("gate-conflict.txt"), "draft\n").expect("write draft");
    git_commit_all(project_root, "gate draft");
    let checkout_target = git_status(project_root, &["checkout", &target]);
    assert!(
        checkout_target.is_ok(),
        "checkout target: {checkout_target:?}"
    );
    fs::write(project_root.join("gate-conflict.txt"), "target\n").expect("write target");
    git_commit_all(project_root, "gate target");
    let merge = git_status(project_root, &["merge", "--no-ff", "draft/gate-conflict"]);
    assert!(
        merge.is_err(),
        "expected deliberate merge conflict for gate coverage"
    );

    let sentinel = project_root.join(".vizier/tmp/merge-conflicts/gate-conflict.json");
    if let Some(parent) = sentinel.parent() {
        fs::create_dir_all(parent).expect("create sentinel dir");
    }
    fs::write(&sentinel, "{}").expect("write sentinel");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-gates",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        Some(JobMetadata {
            plan: Some("gate-conflict".to_string()),
            workflow_node_attempt: Some(2),
            ..JobMetadata::default()
        }),
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");
    let mut record = read_record(&jobs_root, "job-gates").expect("record");

    let conflict_gate = runtime_control_node(
        "conflict",
        "job-gates",
        "control.gate.conflict_resolution",
        "gate.conflict_resolution",
        BTreeMap::new(),
    );
    let conflict_result =
        execute_workflow_control(project_root, &record, &conflict_gate).expect("conflict gate");
    assert_eq!(conflict_result.outcome, WorkflowNodeOutcome::Blocked);
    assert_eq!(
        conflict_result.summary.as_deref(),
        Some(
            "merge conflict resolution incomplete for slug `gate-conflict`: unmerged index entries remain"
        )
    );
    assert!(
        conflict_result
            .stderr_lines
            .iter()
            .any(|line| line == "remaining unmerged paths: gate-conflict.txt"),
        "expected remaining path diagnostics: {:?}",
        conflict_result.stderr_lines
    );

    let mut cicd_gate = runtime_control_node(
        "cicd",
        "job-gates",
        "control.gate.cicd",
        "gate.cicd",
        BTreeMap::new(),
    );
    cicd_gate.gates = vec![WorkflowGate::Cicd {
        script: "exit 7".to_string(),
        auto_resolve: false,
        policy: crate::workflow_template::WorkflowGatePolicy::Retry,
    }];
    let cicd_result =
        execute_workflow_control(project_root, &record, &cicd_gate).expect("cicd gate");
    assert_eq!(cicd_result.outcome, WorkflowNodeOutcome::Failed);
    assert_eq!(cicd_result.exit_code, Some(7));

    let approval_gate = runtime_control_node(
        "approval",
        "job-gates",
        "control.gate.approval",
        "gate.approval",
        BTreeMap::new(),
    );
    record.schedule = Some(JobSchedule {
        approval: Some(pending_job_approval()),
        ..JobSchedule::default()
    });
    let approval_pending =
        execute_workflow_control(project_root, &record, &approval_gate).expect("approval pending");
    assert_eq!(approval_pending.outcome, WorkflowNodeOutcome::Blocked);

    if let Some(schedule) = record.schedule.as_mut()
        && let Some(approval) = schedule.approval.as_mut()
    {
        approval.state = JobApprovalState::Approved;
    }
    let approval_ok =
        execute_workflow_control(project_root, &record, &approval_gate).expect("approval ok");
    assert_eq!(approval_ok.outcome, WorkflowNodeOutcome::Succeeded);

    if let Some(schedule) = record.schedule.as_mut()
        && let Some(approval) = schedule.approval.as_mut()
    {
        approval.state = JobApprovalState::Rejected;
        approval.reason = Some("manual reject".to_string());
    }
    let approval_rejected =
        execute_workflow_control(project_root, &record, &approval_gate).expect("approval rejected");
    assert_eq!(approval_rejected.outcome, WorkflowNodeOutcome::Failed);
    assert_eq!(approval_rejected.exit_code, Some(10));

    let mut terminal = runtime_control_node(
        "terminal",
        "job-gates",
        "control.terminal",
        "terminal",
        BTreeMap::new(),
    );
    terminal.routes.failed.push(WorkflowRouteTarget {
        node_id: "unexpected".to_string(),
        mode: WorkflowRouteMode::RetryJob,
    });
    let invalid_terminal =
        execute_workflow_control(project_root, &record, &terminal).expect("terminal invalid");
    assert_eq!(invalid_terminal.outcome, WorkflowNodeOutcome::Failed);

    terminal.routes = WorkflowRouteTargets::default();
    let valid_terminal =
        execute_workflow_control(project_root, &record, &terminal).expect("terminal valid");
    assert_eq!(valid_terminal.outcome, WorkflowNodeOutcome::Succeeded);
}

#[test]
fn workflow_runtime_conflict_gate_restages_resolved_paths_without_staging_unrelated_dirty_files() {
    let fixture = prepare_conflict_gate_fixture(
        "gate-auto-resolve",
        "draft/gate-auto-resolve",
        "gate-conflict.txt",
    );

    fs::write(fixture.project_root.join("README.md"), "dirty readme\n").expect("dirty readme");
    fs::write(fixture.project_root.join("untracked.txt"), "untracked\n").expect("untracked");

    let node = runtime_control_node(
        "conflict",
        "job-conflict-gate",
        "control.gate.conflict_resolution",
        "gate.conflict_resolution",
        BTreeMap::from([
            ("auto_resolve".to_string(), "true".to_string()),
            (
                "script".to_string(),
                "printf 'target\\ndraft\\n' > gate-conflict.txt".to_string(),
            ),
        ]),
    );
    let result = execute_workflow_control(&fixture.project_root, &fixture.record, &node)
        .expect("conflict gate");
    assert_eq!(result.outcome, WorkflowNodeOutcome::Succeeded);
    assert_eq!(
        result.summary.as_deref(),
        Some("merge conflicts resolved, index finalized, and sentinel cleared")
    );
    assert!(
        !fixture.sentinel.exists(),
        "expected sentinel to be cleared after restaging"
    );
    assert!(
        list_unmerged_paths(&fixture.project_root).is_empty(),
        "expected conflict index entries to be cleared"
    );

    let staged = crate::vcs::snapshot_staged(
        fixture
            .project_root
            .to_str()
            .expect("project root should be valid utf-8"),
    )
    .expect("snapshot staged");
    assert!(
        staged.iter().any(|item| item.path == "gate-conflict.txt"),
        "expected resolved conflict to be staged: {staged:?}"
    );
    assert!(
        staged
            .iter()
            .all(|item| item.path != "README.md" && item.path != "untracked.txt"),
        "expected unrelated dirty files to remain unstaged: {staged:?}"
    );
    assert_eq!(
        fs::read_to_string(fixture.project_root.join("gate-conflict.txt"))
            .expect("read resolved conflict"),
        "target\ndraft\n"
    );
}

#[test]
fn workflow_runtime_conflict_gate_stages_deleted_conflicted_paths() {
    let fixture = prepare_conflict_gate_fixture(
        "gate-auto-delete",
        "draft/gate-auto-delete",
        "gate-delete.txt",
    );
    let node = runtime_control_node(
        "conflict",
        "job-conflict-gate",
        "control.gate.conflict_resolution",
        "gate.conflict_resolution",
        BTreeMap::from([
            ("auto_resolve".to_string(), "true".to_string()),
            ("script".to_string(), "rm gate-delete.txt".to_string()),
        ]),
    );

    let result = execute_workflow_control(&fixture.project_root, &fixture.record, &node)
        .expect("conflict gate");
    assert_eq!(result.outcome, WorkflowNodeOutcome::Succeeded);
    assert!(
        !fixture.sentinel.exists(),
        "expected sentinel to be cleared after delete resolution"
    );
    assert!(
        list_unmerged_paths(&fixture.project_root).is_empty(),
        "expected delete resolution to clear conflict index entries"
    );

    let staged = crate::vcs::snapshot_staged(
        fixture
            .project_root
            .to_str()
            .expect("project root should be valid utf-8"),
    )
    .expect("snapshot staged");
    assert!(
        staged
            .iter()
            .any(|item| matches!(item.kind, crate::vcs::StagedKind::Deleted)
                && item.path == "gate-delete.txt"),
        "expected deleted conflict to be staged: {staged:?}"
    );
}

#[test]
fn workflow_runtime_conflict_gate_blocks_when_unmerged_index_entries_remain_after_restage() {
    let fixture = prepare_conflict_gate_fixture(
        "gate-unmerged-remains",
        "draft/gate-unmerged-remains",
        "gate-still-conflicted.txt",
    );
    let node = runtime_control_node(
        "conflict",
        "job-conflict-gate",
        "control.gate.conflict_resolution",
        "gate.conflict_resolution",
        BTreeMap::from([
            ("auto_resolve".to_string(), "true".to_string()),
            (
                "script".to_string(),
                "printf 'target\\ndraft\\n' > gate-still-conflicted.txt; \
                 oid1=$(printf 'one\\n' | git hash-object -w --stdin); \
                 oid2=$(printf 'two\\n' | git hash-object -w --stdin); \
                 oid3=$(printf 'three\\n' | git hash-object -w --stdin); \
                 printf '100644 %s 1\\textra.txt\\n100644 %s 2\\textra.txt\\n100644 %s 3\\textra.txt\\n' \
                 \"$oid1\" \"$oid2\" \"$oid3\" | git update-index --index-info"
                    .to_string(),
            ),
        ]),
    );

    let result = execute_workflow_control(&fixture.project_root, &fixture.record, &node)
        .expect("conflict gate");
    assert_eq!(result.outcome, WorkflowNodeOutcome::Blocked);
    assert_eq!(
        result.summary.as_deref(),
        Some(
            "merge conflict resolution incomplete for slug `gate-unmerged-remains`: unmerged index entries remain"
        )
    );
    assert!(
        result
            .stderr_lines
            .iter()
            .any(|line| line == "remaining unmerged paths: extra.txt"),
        "expected remaining unmerged path diagnostics: {:?}",
        result.stderr_lines
    );
    assert!(
        fixture.sentinel.exists(),
        "expected sentinel to remain when unmerged entries still exist"
    );
    assert_eq!(
        list_unmerged_paths(&fixture.project_root),
        vec!["extra.txt"]
    );
}

#[test]
fn stop_condition_runtime_blocks_when_retry_budget_is_exhausted() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-stop-gate",
        &["--help".to_string()],
        &["vizier".to_string(), "__workflow-node".to_string()],
        Some(JobMetadata {
            workflow_node_attempt: Some(3),
            ..JobMetadata::default()
        }),
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue");
    let record = read_record(&jobs_root, "job-stop-gate").expect("record");

    let node = WorkflowRuntimeNodeManifest {
        node_id: "gate".to_string(),
        name: None,
        job_id: "job-stop-gate".to_string(),
        uses: "control.gate.stop_condition".to_string(),
        kind: WorkflowNodeKind::Gate,
        args: BTreeMap::from([("script".to_string(), "exit 1".to_string())]),
        executor_operation: None,
        control_policy: Some("gate.stop_condition".to_string()),
        gates: Vec::new(),
        retry: crate::workflow_template::WorkflowRetryPolicy {
            mode: WorkflowRetryMode::UntilGate,
            budget: 1,
        },
        routes: WorkflowRouteTargets::default(),
        artifacts_by_outcome: WorkflowOutcomeArtifactsByOutcome::default(),
    };

    let result = execute_workflow_control(project_root, &record, &node)
        .expect("execute stop-condition gate");
    assert_eq!(result.outcome, WorkflowNodeOutcome::Blocked);
    assert!(
        result
            .summary
            .as_deref()
            .unwrap_or("")
            .contains("retry budget exhausted"),
        "expected budget summary, got {:?}",
        result.summary
    );
}

#[test]
fn gc_jobs_preserves_terminal_records_referenced_by_active_after_dependencies() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-predecessor",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        None,
    )
    .expect("enqueue predecessor");
    update_job_record(&jobs_root, "job-predecessor", |record| {
        record.status = JobStatus::Succeeded;
        let old = Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap();
        record.created_at = old;
        record.started_at = Some(old);
        record.finished_at = Some(old);
        record.exit_code = Some(0);
    })
    .expect("mark predecessor terminal");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-dependent",
        &["--help".to_string()],
        &["vizier".to_string(), "save".to_string()],
        None,
        None,
        Some(JobSchedule {
            after: vec![after_dependency("job-predecessor")],
            ..JobSchedule::default()
        }),
    )
    .expect("enqueue dependent");
    update_job_record(&jobs_root, "job-dependent", |record| {
        record.status = JobStatus::Queued;
    })
    .expect("ensure active status");

    let removed = gc_jobs(project_root, &jobs_root, Duration::days(7)).expect("gc");
    assert_eq!(removed, 0, "expected predecessor to be retained");
    assert!(
        paths_for(&jobs_root, "job-predecessor").job_dir.exists(),
        "expected referenced predecessor to remain after GC"
    );
}

#[test]
fn clean_job_scope_single_job_removes_job_record_artifacts_and_plan_state() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-clean-single",
        &["--help".to_string()],
        &["vizier".to_string(), "run".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue clean job");
    update_job_record(&jobs_root, "job-clean-single", |record| {
        record.status = JobStatus::Succeeded;
    })
    .expect("mark succeeded");

    let marker =
        custom_artifact_marker_path(project_root, "job-clean-single", "acme.clean", "result");
    let payload =
        custom_artifact_payload_path(project_root, "job-clean-single", "acme.clean", "result");
    if let Some(parent) = marker.parent() {
        fs::create_dir_all(parent).expect("create marker dir");
    }
    if let Some(parent) = payload.parent() {
        fs::create_dir_all(parent).expect("create payload dir");
    }
    fs::write(&marker, "{}").expect("write marker");
    fs::write(&payload, "{}").expect("write payload");

    let plan_state_rel = crate::plan::plan_state_rel_path("pln_clean_single");
    let plan_state_path = project_root.join(&plan_state_rel);
    if let Some(parent) = plan_state_path.parent() {
        fs::create_dir_all(parent).expect("create plan state dir");
    }
    let timestamp = Utc::now().to_rfc3339();
    let plan_state = crate::plan::PlanRecord {
        plan_id: "pln_clean_single".to_string(),
        slug: Some("clean-single".to_string()),
        branch: Some("draft/clean-single".to_string()),
        source: None,
        intent: None,
        target_branch: None,
        work_ref: Some("workflow-job:job-clean-single".to_string()),
        status: None,
        summary: None,
        created_at: timestamp.clone(),
        updated_at: timestamp,
        job_ids: HashMap::from([("persist".to_string(), "job-clean-single".to_string())]),
    };
    fs::write(
        &plan_state_path,
        serde_json::to_string_pretty(&plan_state).expect("serialize plan state"),
    )
    .expect("write plan state");

    let outcome = clean_job_scope(
        project_root,
        &jobs_root,
        CleanJobOptions {
            requested_job_id: "job-clean-single".to_string(),
            keep_branches: true,
            force: false,
        },
    )
    .expect("clean succeeds");

    assert_eq!(outcome.scope, CleanScope::Job);
    assert_eq!(outcome.removed.jobs, 1);
    assert_eq!(outcome.removed.artifact_markers, 1);
    assert_eq!(outcome.removed.artifact_payloads, 1);
    assert_eq!(outcome.removed.plan_state_deleted, 1);
    assert!(
        !paths_for(&jobs_root, "job-clean-single").job_dir.exists(),
        "expected scoped job directory removed"
    );
    assert!(!marker.exists(), "expected marker removed");
    assert!(!payload.exists(), "expected payload removed");
    assert!(
        !plan_state_path.exists(),
        "expected plan state with only scoped refs removed"
    );
}

#[test]
fn clean_job_scope_expands_to_run_scope_and_removes_run_manifest() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");
    let run_id = "run_clean_scope";

    for job_id in ["job-run-node-a", "job-run-node-b"] {
        enqueue_job(
            project_root,
            &jobs_root,
            job_id,
            &["--help".to_string()],
            &["vizier".to_string(), "__workflow-node".to_string()],
            Some(JobMetadata {
                workflow_run_id: Some(run_id.to_string()),
                ..JobMetadata::default()
            }),
            None,
            Some(JobSchedule::default()),
        )
        .expect("enqueue run node");
        update_job_record(&jobs_root, job_id, |record| {
            record.status = JobStatus::Succeeded;
        })
        .expect("mark run node succeeded");

        let marker = custom_artifact_marker_path(project_root, job_id, "acme.clean", "run");
        let payload = custom_artifact_payload_path(project_root, job_id, "acme.clean", "run");
        if let Some(parent) = marker.parent() {
            fs::create_dir_all(parent).expect("create marker dir");
        }
        if let Some(parent) = payload.parent() {
            fs::create_dir_all(parent).expect("create payload dir");
        }
        fs::write(marker, "{}").expect("write marker");
        fs::write(payload, "{}").expect("write payload");
    }

    let manifest_path = workflow_run_manifest_path(project_root, run_id);
    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent).expect("create runs dir");
    }
    fs::write(&manifest_path, "{}").expect("write run manifest");

    let outcome = clean_job_scope(
        project_root,
        &jobs_root,
        CleanJobOptions {
            requested_job_id: "job-run-node-a".to_string(),
            keep_branches: true,
            force: false,
        },
    )
    .expect("run-scoped clean succeeds");

    assert_eq!(outcome.scope, CleanScope::Run);
    assert_eq!(outcome.run_id.as_deref(), Some(run_id));
    assert_eq!(outcome.removed.jobs, 2);
    assert_eq!(outcome.removed.run_manifests, 1);
    assert_eq!(outcome.removed.artifact_markers, 2);
    assert_eq!(outcome.removed.artifact_payloads, 2);
    assert!(!manifest_path.exists(), "expected run manifest removed");
    assert!(!paths_for(&jobs_root, "job-run-node-a").job_dir.exists());
    assert!(!paths_for(&jobs_root, "job-run-node-b").job_dir.exists());
}

#[test]
fn clean_job_scope_blocks_active_scoped_jobs_even_with_force() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-clean-active",
        &["--help".to_string()],
        &["vizier".to_string(), "run".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue active job");

    let err = clean_job_scope(
        project_root,
        &jobs_root,
        CleanJobOptions {
            requested_job_id: "job-clean-active".to_string(),
            keep_branches: true,
            force: true,
        },
    )
    .expect_err("active scoped job should block cleanup");
    assert_eq!(err.kind(), CleanJobErrorKind::Guard);
    assert!(
        err.reasons()
            .iter()
            .any(|reason| reason.contains("is active")),
        "expected active guard reason: {:?}",
        err.reasons()
    );
}

#[test]
fn clean_job_scope_requires_force_for_active_non_scoped_after_dependents() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-clean-source",
        &["--help".to_string()],
        &["vizier".to_string(), "run".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue source job");
    update_job_record(&jobs_root, "job-clean-source", |record| {
        record.status = JobStatus::Succeeded;
    })
    .expect("mark source succeeded");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-clean-dependent",
        &["--help".to_string()],
        &["vizier".to_string(), "run".to_string()],
        None,
        None,
        Some(JobSchedule {
            after: vec![after_dependency("job-clean-source")],
            ..JobSchedule::default()
        }),
    )
    .expect("enqueue dependent");
    update_job_record(&jobs_root, "job-clean-dependent", |record| {
        record.status = JobStatus::Queued;
    })
    .expect("mark dependent active");

    let err = clean_job_scope(
        project_root,
        &jobs_root,
        CleanJobOptions {
            requested_job_id: "job-clean-source".to_string(),
            keep_branches: true,
            force: false,
        },
    )
    .expect_err("expected dependency safety guard");
    assert_eq!(err.kind(), CleanJobErrorKind::Guard);
    assert!(
        err.reasons()
            .iter()
            .any(|reason| reason.contains("--after dependency")),
        "expected after-dependency guard reason: {:?}",
        err.reasons()
    );

    let outcome = clean_job_scope(
        project_root,
        &jobs_root,
        CleanJobOptions {
            requested_job_id: "job-clean-source".to_string(),
            keep_branches: true,
            force: true,
        },
    )
    .expect("force should bypass dependency guard");
    assert_eq!(outcome.removed.jobs, 1);
    assert!(!paths_for(&jobs_root, "job-clean-source").job_dir.exists());
    assert!(
        paths_for(&jobs_root, "job-clean-dependent")
            .job_dir
            .exists()
    );
}

#[test]
fn clean_job_scope_rewrites_plan_state_when_non_scoped_refs_remain() {
    let temp = TempDir::new().expect("temp dir");
    init_repo(&temp).expect("init repo");
    let project_root = temp.path();
    let jobs_root = project_root.join(".vizier/jobs");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-clean-plan-target",
        &["--help".to_string()],
        &["vizier".to_string(), "run".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue target");
    update_job_record(&jobs_root, "job-clean-plan-target", |record| {
        record.status = JobStatus::Succeeded;
    })
    .expect("mark target succeeded");

    enqueue_job(
        project_root,
        &jobs_root,
        "job-clean-plan-keep",
        &["--help".to_string()],
        &["vizier".to_string(), "run".to_string()],
        None,
        None,
        Some(JobSchedule::default()),
    )
    .expect("enqueue keep");
    update_job_record(&jobs_root, "job-clean-plan-keep", |record| {
        record.status = JobStatus::Succeeded;
    })
    .expect("mark keep succeeded");

    let plan_state_rel = crate::plan::plan_state_rel_path("pln_clean_rewrite");
    let plan_state_path = project_root.join(&plan_state_rel);
    if let Some(parent) = plan_state_path.parent() {
        fs::create_dir_all(parent).expect("create plan state dir");
    }
    let timestamp = Utc::now().to_rfc3339();
    let plan_state = crate::plan::PlanRecord {
        plan_id: "pln_clean_rewrite".to_string(),
        slug: Some("clean-rewrite".to_string()),
        branch: Some("draft/clean-rewrite".to_string()),
        source: None,
        intent: None,
        target_branch: None,
        work_ref: Some("workflow-job:job-clean-plan-target".to_string()),
        status: None,
        summary: None,
        created_at: timestamp.clone(),
        updated_at: timestamp,
        job_ids: HashMap::from([
            ("persist".to_string(), "job-clean-plan-target".to_string()),
            ("other".to_string(), "job-clean-plan-keep".to_string()),
        ]),
    };
    fs::write(
        &plan_state_path,
        serde_json::to_string_pretty(&plan_state).expect("serialize plan state"),
    )
    .expect("write plan state");

    let outcome = clean_job_scope(
        project_root,
        &jobs_root,
        CleanJobOptions {
            requested_job_id: "job-clean-plan-target".to_string(),
            keep_branches: true,
            force: false,
        },
    )
    .expect("clean succeeds");
    assert_eq!(outcome.removed.plan_state_rewritten, 1);
    assert!(
        plan_state_path.exists(),
        "expected rewritten plan state to remain"
    );

    let rewritten: crate::plan::PlanRecord = serde_json::from_str(
        &fs::read_to_string(&plan_state_path).expect("read rewritten plan state"),
    )
    .expect("parse rewritten plan state");
    assert_eq!(rewritten.work_ref, None);
    assert_eq!(rewritten.job_ids.len(), 1);
    assert_eq!(
        rewritten.job_ids.get("other").map(String::as_str),
        Some("job-clean-plan-keep")
    );
}
