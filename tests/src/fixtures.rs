pub(crate) use git2::{
    BranchType, DiffOptions, IndexAddOption, Oid, Repository, Signature, Sort,
    build::CheckoutBuilder,
};
pub(crate) use serde_json::{Value, json};
pub(crate) use std::collections::HashSet;
pub(crate) use std::env;
pub(crate) use std::fs;
pub(crate) use std::io::{self, BufRead, Read, Write};
#[cfg(unix)]
pub(crate) use std::os::unix::fs::PermissionsExt;
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::process::{Command, Output, Stdio};
pub(crate) use std::sync::{Condvar, Mutex, OnceLock};
pub(crate) use std::thread::ThreadId;
pub(crate) use std::time::{Duration, Instant};
pub(crate) use tempfile::TempDir;

pub(crate) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

// Integration tests spawn external processes, temporary repos, and background jobs. Serialize
// them to avoid cross-test races, but allow re-entrant locking within a single test.
pub(crate) struct IntegrationTestLock {
    state: Mutex<IntegrationTestState>,
    cvar: Condvar,
}

#[derive(Default)]
pub(crate) struct IntegrationTestState {
    owner: Option<ThreadId>,
    depth: usize,
}

impl IntegrationTestLock {
    fn new() -> Self {
        Self {
            state: Mutex::new(IntegrationTestState::default()),
            cvar: Condvar::new(),
        }
    }

    fn lock(&'static self) -> IntegrationTestGuard {
        let current = std::thread::current().id();
        let mut state = self.state.lock().expect("lock integration test mutex");
        loop {
            match state.owner {
                None => {
                    state.owner = Some(current);
                    state.depth = 1;
                    return IntegrationTestGuard {
                        lock: self,
                        owner: current,
                    };
                }
                Some(owner) if owner == current => {
                    state.depth += 1;
                    return IntegrationTestGuard {
                        lock: self,
                        owner: current,
                    };
                }
                _ => {
                    state = self
                        .cvar
                        .wait(state)
                        .expect("wait on integration test mutex");
                }
            }
        }
    }
}

pub(crate) struct IntegrationTestGuard {
    lock: &'static IntegrationTestLock,
    owner: ThreadId,
}

impl Drop for IntegrationTestGuard {
    fn drop(&mut self) {
        let mut state = self.lock.state.lock().expect("lock integration test mutex");
        if state.owner == Some(self.owner) {
            state.depth = state.depth.saturating_sub(1);
            if state.depth == 0 {
                state.owner = None;
                self.lock.cvar.notify_all();
            }
        }
    }
}

static INTEGRATION_TEST_LOCK: OnceLock<IntegrationTestLock> = OnceLock::new();

pub(crate) fn integration_test_lock() -> &'static IntegrationTestLock {
    INTEGRATION_TEST_LOCK.get_or_init(IntegrationTestLock::new)
}

pub(crate) fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tests crate lives under repo root")
        .to_path_buf()
}

pub(crate) fn vizier_binary() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| build_vizier_binary(&["mock_llm", "integration_testing"]))
}

pub(crate) fn vizier_binary_no_mock() -> &'static PathBuf {
    static BIN_NO_MOCK: OnceLock<PathBuf> = OnceLock::new();
    BIN_NO_MOCK.get_or_init(|| build_vizier_binary(&["integration_testing"]))
}

pub(crate) fn build_vizier_binary(features: &[&str]) -> PathBuf {
    let root = repo_root();
    let label = if features.is_empty() {
        "base".to_string()
    } else {
        features.join("_")
    };
    let target_dir = root.join("target").join(format!("tests-{label}"));
    let mut args = vec!["build".to_string(), "--release".to_string()];
    if !features.is_empty() {
        args.push("--features".to_string());
        args.push(features.join(","));
    }
    let status = Command::new("cargo")
        .current_dir(&root)
        .args(&args)
        .env("CARGO_TARGET_DIR", &target_dir)
        .status()
        .expect("failed to invoke cargo build for vizier");
    if !status.success() {
        panic!("cargo build for vizier failed with status {status:?}");
    }
    let path = target_dir.join("release/vizier");
    if !path.exists() {
        panic!("expected vizier binary at {}", path.display());
    }
    path
}

pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
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

pub(crate) struct IntegrationRepo {
    dir: TempDir,
    agent_bin_dir: PathBuf,
    vizier_bin: PathBuf,
    _guard: IntegrationTestGuard,
}

impl IntegrationRepo {
    pub(crate) fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_binary(vizier_binary().clone())
    }

    pub(crate) fn with_binary(bin: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let guard = integration_test_lock().lock();
        let dir = TempDir::new()?;
        copy_dir_recursive(&repo_root().join("test-repo"), dir.path())?;
        copy_dir_recursive(&repo_root().join(".vizier"), &dir.path().join(".vizier"))?;
        clear_jobs_dir(dir.path())?;
        ensure_gitignore(dir.path())?;
        write_default_cicd_script(dir.path())?;
        init_repo_at(dir.path())?;
        let agent_bin_dir = create_agent_shims(dir.path())?;
        Ok(Self {
            dir,
            agent_bin_dir,
            vizier_bin: bin,
            _guard: guard,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        self.dir.path()
    }

    pub(crate) fn repo(&self) -> Repository {
        Repository::open(self.path()).expect("open repo")
    }

    pub(crate) fn vizier_cmd_base(&self) -> Command {
        let mut cmd = Command::new(&self.vizier_bin);
        cmd.current_dir(self.path());
        // Point Vizier at an isolated config root so user-global configs
        // cannot flip backends during integration tests.
        let config_root = self.path().join(".vizier/tmp/config-root");
        let _ = fs::create_dir_all(&config_root);
        cmd.env("VIZIER_CONFIG_DIR", &config_root);
        cmd.env("VIZIER_AGENT_SHIMS_DIR", &self.agent_bin_dir);
        let mut paths = vec![self.agent_bin_dir.clone()];
        if let Some(existing) = env::var_os("PATH") {
            paths.extend(env::split_paths(&existing));
        }
        if let Ok(joined) = env::join_paths(paths) {
            cmd.env("PATH", joined);
        }
        cmd
    }

    pub(crate) fn vizier_cmd(&self) -> Command {
        let mut cmd = self.vizier_cmd_base();
        cmd.arg("--no-background");
        cmd
    }

    pub(crate) fn vizier_cmd_background(&self) -> Command {
        self.vizier_cmd_base()
    }

    pub(crate) fn vizier_cmd_with_config(&self, config: &Path) -> Command {
        let mut cmd = self.vizier_cmd();
        cmd.env("VIZIER_CONFIG_FILE", config);
        cmd.arg("--config-file");
        cmd.arg(config);
        cmd
    }

    pub(crate) fn vizier_output(&self, args: &[&str]) -> io::Result<Output> {
        let mut cmd = self.vizier_cmd();
        cmd.args(args);
        cmd.output()
    }

    pub(crate) fn write(&self, rel: &str, contents: &str) -> io::Result<()> {
        let path = self.path().join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, contents)
    }

    pub(crate) fn read(&self, rel: &str) -> io::Result<String> {
        fs::read_to_string(self.path().join(rel))
    }

    pub(crate) fn git(&self, args: &[&str]) -> TestResult {
        let status = Command::new("git")
            .arg("-C")
            .arg(self.path())
            .env("GIT_MERGE_AUTOEDIT", "no")
            .env("GIT_EDITOR", "true")
            .env("VISUAL", "true")
            .args(args)
            .status()?;
        if !status.success() {
            return Err(format!("git {:?} failed with status {status:?}", args).into());
        }
        Ok(())
    }
}

pub(crate) fn init_repo_at(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
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

pub(crate) fn add_all(repo: &Repository, specs: &[&str]) -> Result<(), git2::Error> {
    let mut index = repo.index()?;
    index.add_all(specs, IndexAddOption::DEFAULT, None)?;
    index.write()?;
    Ok(())
}

pub(crate) fn oid_for_spec(repo: &Repository, spec: &str) -> Result<Oid, git2::Error> {
    let obj = repo.revparse_single(spec)?;
    Ok(obj.peel_to_commit()?.id())
}

pub(crate) fn files_changed_in_commit(
    repo: &Repository,
    spec: &str,
) -> Result<HashSet<String>, git2::Error> {
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

    let mut paths = HashSet::new();
    for delta in diff.deltas() {
        if let Some(path) = delta.new_file().path().or_else(|| delta.old_file().path()) {
            paths.insert(path.to_string_lossy().replace('\\', "/"));
        }
    }

    Ok(paths)
}

pub(crate) fn count_commits_from_head(repo: &Repository) -> Result<usize, git2::Error> {
    let mut walk = repo.revwalk()?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
    walk.push_head()?;
    Ok(walk.count())
}

pub(crate) fn find_save_field(output: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    let lower_key = key.to_ascii_lowercase();
    for line in output.lines() {
        for part in line.split(';') {
            let trimmed = part.trim();
            if let Some(value) = trimmed.strip_prefix(&prefix) {
                return Some(value.trim().to_string());
            }
            if let Some(index) = trimmed.find(':') {
                let (label, value) = trimmed.split_at(index);
                if label.trim().eq_ignore_ascii_case(&lower_key) {
                    return Some(value.trim_start_matches(':').trim().to_string());
                }
            }
        }
    }
    None
}

pub(crate) fn session_log_contents_from_output(
    repo: &IntegrationRepo,
    stdout: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let session_rel = find_save_field(stdout, "session")
        .ok_or_else(|| "save output missing session field".to_string())?;
    if session_rel == "none" {
        return Err("save output did not report a session log path".into());
    }

    let session_path = repo.path().join(session_rel);
    let contents = match fs::read_to_string(&session_path) {
        Ok(data) => data,
        Err(err) => {
            return Err(format!(
                "failed to read session log at {}: {}",
                session_path.display(),
                err
            )
            .into());
        }
    };

    Ok(contents)
}

pub(crate) fn gather_session_logs(repo: &IntegrationRepo) -> io::Result<Vec<PathBuf>> {
    let sessions_root = repo.path().join(".vizier").join("sessions");
    let mut files = Vec::new();
    if !sessions_root.exists() {
        return Ok(files);
    }

    for entry in fs::read_dir(&sessions_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let session_path = entry.path().join("session.json");
        if session_path.exists() {
            files.push(session_path);
        }
    }

    Ok(files)
}

pub(crate) fn new_session_log<'a>(
    before: &'a [PathBuf],
    after: &'a [PathBuf],
) -> Option<&'a PathBuf> {
    let before_set: HashSet<_> = before.iter().collect();
    after.iter().find(|path| !before_set.contains(path))
}

pub(crate) fn prepare_conflicting_plan(
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

pub(crate) fn clean_workdir(repo: &IntegrationRepo) -> TestResult {
    reset_workdir(repo)?;
    repo.git(&["clean", "-fd"])?;
    Ok(())
}

pub(crate) fn reset_workdir(repo: &IntegrationRepo) -> TestResult {
    repo.git(&["reset", "--hard"])?;
    Ok(())
}

pub(crate) fn extract_job_id(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("Job: "))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn wait_for_job_completion(
    repo: &IntegrationRepo,
    job_id: &str,
    timeout: Duration,
) -> TestResult {
    let job_path = repo
        .path()
        .join(".vizier/jobs")
        .join(job_id)
        .join("job.json");
    let start = Instant::now();
    loop {
        if start.elapsed() > timeout {
            return Err(format!("timed out waiting for job {job_id}").into());
        }
        let Ok(contents) = fs::read_to_string(&job_path) else {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        };
        let record: Value = serde_json::from_str(&contents)?;
        let status = record
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if status != "pending" && status != "running" {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

pub(crate) fn read_job_record(repo: &IntegrationRepo, job_id: &str) -> TestResult<Value> {
    let job_path = repo
        .path()
        .join(".vizier/jobs")
        .join(job_id)
        .join("job.json");
    let contents = fs::read_to_string(&job_path)?;
    let record: Value = serde_json::from_str(&contents)?;
    Ok(record)
}

pub(crate) fn write_job_record(
    repo: &IntegrationRepo,
    job_id: &str,
    record: Value,
) -> io::Result<()> {
    let job_dir = repo.path().join(".vizier/jobs").join(job_id);
    fs::create_dir_all(&job_dir)?;
    fs::write(job_dir.join("stdout.log"), "")?;
    fs::write(job_dir.join("stderr.log"), "")?;
    let path = job_dir.join("job.json");
    fs::write(path, serde_json::to_string_pretty(&record)?)
}

pub(crate) fn write_job_record_simple(
    repo: &IntegrationRepo,
    job_id: &str,
    status: &str,
    created_at: &str,
    finished_at: Option<&str>,
    command: &[&str],
) -> TestResult {
    let record = json!({
        "id": job_id,
        "status": status,
        "command": command,
        "created_at": created_at,
        "started_at": created_at,
        "finished_at": finished_at,
        "pid": null,
        "exit_code": null,
        "stdout_path": format!(".vizier/jobs/{job_id}/stdout.log"),
        "stderr_path": format!(".vizier/jobs/{job_id}/stderr.log"),
        "session_path": null,
        "outcome_path": null,
        "metadata": null,
        "config_snapshot": null
    });
    write_job_record(repo, job_id, record)?;
    Ok(())
}

pub(crate) fn wait_for_job_active(
    repo: &IntegrationRepo,
    job_id: &str,
    timeout: Duration,
) -> TestResult {
    let job_path = repo
        .path()
        .join(".vizier/jobs")
        .join(job_id)
        .join("job.json");
    let start = Instant::now();
    loop {
        if start.elapsed() > timeout {
            return Err(format!("timed out waiting for job {job_id} to start").into());
        }
        let Ok(contents) = fs::read_to_string(&job_path) else {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        };
        let record: Value = serde_json::from_str(&contents)?;
        let status = record
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        match status {
            "pending" | "running" => return Ok(()),
            "succeeded" | "failed" | "cancelled" => {
                return Err(format!("job {job_id} finished before queueing").into());
            }
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub(crate) fn list_merge_job_ids(
    repo: &IntegrationRepo,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let jobs_root = repo.path().join(".vizier/jobs");
    if !jobs_root.exists() {
        return Ok(Vec::new());
    }
    let mut ids = Vec::new();
    for entry in fs::read_dir(&jobs_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let job_id = entry.file_name().to_string_lossy().to_string();
        let job_path = jobs_root.join(&job_id).join("job.json");
        if !job_path.exists() {
            continue;
        }
        let contents = fs::read_to_string(&job_path)?;
        let record: Value = serde_json::from_str(&contents)?;
        let scope = record
            .get("metadata")
            .and_then(|meta| meta.get("scope"))
            .and_then(Value::as_str);
        if scope == Some("merge") {
            ids.push(job_id);
        }
    }
    Ok(ids)
}

pub(crate) fn read_merge_queue_state(
    repo: &IntegrationRepo,
) -> Result<Value, Box<dyn std::error::Error>> {
    let path = repo.path().join(".vizier/jobs/merge-queue.json");
    let contents = fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&contents)?)
}

pub(crate) fn spawn_detached_sleep(seconds: u64) -> TestResult<u32> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(format!("nohup sleep {seconds} >/dev/null 2>&1 & echo $!"))
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "failed to spawn detached sleep: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let pid = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()?;
    Ok(pid)
}

pub(crate) fn terminate_pid(pid: u32) {
    let _ = Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()
        .filter(|status| status.success())
        .and_then(|_| {
            Command::new("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .ok()
        });
}

pub(crate) fn list_worktree_names(repo: &Repository) -> Result<Vec<String>, git2::Error> {
    Ok(repo
        .worktrees()?
        .iter()
        .filter_map(|name| name.map(|value| value.to_string()))
        .collect())
}

pub(crate) fn write_cicd_script(
    repo: &IntegrationRepo,
    name: &str,
    contents: &str,
) -> io::Result<PathBuf> {
    let scripts_dir = repo.path().join(".vizier/tmp/cicd-scripts");
    fs::create_dir_all(&scripts_dir)?;
    let path = scripts_dir.join(name);
    fs::write(&path, contents)?;
    Ok(path)
}

pub(crate) fn create_agent_shims(root: &Path) -> io::Result<PathBuf> {
    // Keep shims under .vizier/tmp so they stay ignored when commands require a clean tree.
    let bin_dir = root.join(".vizier/tmp/bin");
    fs::create_dir_all(&bin_dir)?;
    let script = b"#!/bin/sh
set -euo pipefail
cat >/dev/null
printf '%s\n' '{\"type\":\"item.started\",\"item\":{\"type\":\"reasoning\",\"text\":\"prep\"}}'
printf '%s\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"mock agent response\"}}'
printf 'mock agent running\n' 1>&2
";
    for name in ["codex", "gemini"] {
        let nested = bin_dir.join(name).join("agent.sh");
        if let Some(parent) = nested.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&nested, script)?;
        let flat = bin_dir.join(format!("{name}.sh"));
        fs::write(&flat, script)?;
        #[cfg(unix)]
        {
            for path in [&nested, &flat] {
                let mut perms = fs::metadata(path)?.permissions();
                perms.set_mode(0o755);
                fs::set_permissions(path, perms)?;
            }
        }
    }
    Ok(bin_dir)
}

pub(crate) fn write_backend_stub(dir: &Path, name: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(name);
    fs::write(
        &path,
        "#!/bin/sh
set -euo pipefail

if [ -n \"${INPUT_LOG:-}\" ]; then
  cat >\"${INPUT_LOG}\"
else
  cat >/dev/null
fi

if [ -n \"${ARGS_LOG:-}\" ]; then
  printf \"%s\\n\" \"$*\" >\"${ARGS_LOG}\"
fi

printf '%s\\n' \"${PAYLOAD:-stub-output}\"
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

pub(crate) fn write_default_cicd_script(repo_root: &Path) -> io::Result<()> {
    let script_path = repo_root.join("cicd.sh");
    let contents = "#!/bin/sh\nset -eu\nprintf \"default ci gate ok\"\n";
    fs::write(&script_path, contents)?;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&script_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms)?;
    }
    Ok(())
}

fn clear_jobs_dir(repo_root: &Path) -> io::Result<()> {
    let jobs_dir = repo_root.join(".vizier/jobs");
    if jobs_dir.exists() {
        fs::remove_dir_all(&jobs_dir)?;
    }
    Ok(())
}

pub(crate) fn ensure_gitignore(path: &Path) -> io::Result<()> {
    let ignore_path = path.join(".gitignore");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ignore_path)?;
    writeln!(file, "\n# Vizier test state")?;
    writeln!(file, ".vizier/tmp/")?;
    writeln!(file, ".vizier/tmp-worktrees/")?;
    writeln!(file, ".vizier/sessions/")?;
    Ok(())
}
