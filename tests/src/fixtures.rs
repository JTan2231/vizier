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
pub(crate) use std::sync::{Mutex, MutexGuard, OnceLock};
pub(crate) use std::time::{Duration, Instant, SystemTime};
pub(crate) use tempfile::TempDir;

pub(crate) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const BUILD_ROOT_PREFIX: &str = "vizier-tests-build-";
const REPO_ROOT_PREFIX: &str = "vizier-tests-repo-";
const LEGACY_TMP_PREFIX: &str = ".tmp";
const BUILD_ROOT_MARKER: &str = ".vizier-test-build-root";
const REPO_ROOT_MARKER: &str = ".vizier-test-integration-repo";
const ACTIVE_PID_MARKER: &str = ".vizier-test-active-pid";
const KEEP_TEMP_ENV: &str = "VIZIER_TEST_KEEP_TEMP";
const STALE_SECS_ENV: &str = "VIZIER_TEST_TEMP_STALE_SECS";
const DEFAULT_STALE_SECS: u64 = 60 * 30;

static BUILD_ROOT: OnceLock<BuildRoot> = OnceLock::new();
static BUILD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static INTEGRATION_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static TEMP_CLEANUP_ONCE: OnceLock<()> = OnceLock::new();

#[derive(Debug)]
struct BuildRoot {
    path: PathBuf,
    _temp_dir: Option<TempDir>,
}

#[derive(Debug, Default, Clone, Copy)]
struct TempCleanupStats {
    removed_build_roots: usize,
    removed_repo_roots: usize,
}

fn build_root() -> &'static PathBuf {
    &BUILD_ROOT.get_or_init(create_build_root).path
}

fn create_build_root() -> BuildRoot {
    ensure_fixture_temp_hygiene();
    create_build_root_in(&env::temp_dir(), keep_temp_artifacts())
        .expect("create temp dir for vizier test builds")
}

fn create_build_root_in(root: &Path, preserve: bool) -> io::Result<BuildRoot> {
    let temp_dir = tempfile::Builder::new()
        .prefix(BUILD_ROOT_PREFIX)
        .tempdir_in(root)?;
    mark_fixture_owner(temp_dir.path(), BUILD_ROOT_MARKER)?;
    write_active_pid_marker(temp_dir.path())?;

    if preserve {
        let path = temp_dir.keep();
        Ok(BuildRoot {
            path,
            _temp_dir: None,
        })
    } else {
        let path = temp_dir.path().to_path_buf();
        Ok(BuildRoot {
            path,
            _temp_dir: Some(temp_dir),
        })
    }
}

fn ensure_fixture_temp_hygiene() {
    let _ = TEMP_CLEANUP_ONCE.get_or_init(|| {
        let _ = cleanup_stale_fixture_temp_dirs();
    });
}

fn cleanup_stale_fixture_temp_dirs() -> io::Result<TempCleanupStats> {
    cleanup_stale_fixture_temp_dirs_in(&env::temp_dir(), SystemTime::now(), stale_temp_window())
}

fn cleanup_stale_fixture_temp_dirs_in(
    root: &Path,
    now: SystemTime,
    stale_after: Duration,
) -> io::Result<TempCleanupStats> {
    let mut stats = TempCleanupStats::default();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(stats),
        Err(err) => return Err(err),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };

        if name.starts_with(BUILD_ROOT_PREFIX) {
            if remove_if_stale(&path, now, stale_after) {
                stats.removed_build_roots += 1;
            }
            continue;
        }

        if name.starts_with(REPO_ROOT_PREFIX) {
            if path.join(REPO_ROOT_MARKER).exists() && remove_if_stale(&path, now, stale_after) {
                stats.removed_repo_roots += 1;
            }
            continue;
        }

        if name.starts_with(LEGACY_TMP_PREFIX) {
            if is_legacy_vizier_repo(path.as_path()) && remove_if_stale(&path, now, stale_after) {
                stats.removed_repo_roots += 1;
            }
            continue;
        }
    }

    Ok(stats)
}

fn remove_if_stale(path: &Path, now: SystemTime, stale_after: Duration) -> bool {
    if dir_has_live_owner(path) {
        return false;
    }

    let has_pid_marker = path.join(ACTIVE_PID_MARKER).exists();
    if !has_pid_marker && !is_stale(path, now, stale_after) {
        return false;
    }

    fs::remove_dir_all(path).is_ok()
}

fn is_legacy_vizier_repo(path: &Path) -> bool {
    path.join(".git").is_dir()
        && path.join(".vizier").is_dir()
        && path.join("a").is_file()
        && path.join("b").is_file()
        && path.join("c").is_file()
}

fn is_stale(path: &Path, now: SystemTime, stale_after: Duration) -> bool {
    if stale_after.is_zero() {
        return true;
    }
    match fs::metadata(path).and_then(|metadata| metadata.modified()) {
        Ok(modified) => match now.duration_since(modified) {
            Ok(age) => age >= stale_after,
            Err(_) => true,
        },
        Err(_) => true,
    }
}

fn dir_has_live_owner(path: &Path) -> bool {
    let marker = path.join(ACTIVE_PID_MARKER);
    let Ok(contents) = fs::read_to_string(marker) else {
        return false;
    };
    let Ok(pid) = contents.trim().parse::<u32>() else {
        return false;
    };
    process_is_running(pid)
}

#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn process_is_running(_pid: u32) -> bool {
    false
}

fn keep_temp_artifacts() -> bool {
    env_flag_enabled(env::var(KEEP_TEMP_ENV).ok().as_deref())
}

fn stale_temp_window() -> Duration {
    env::var(STALE_SECS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(DEFAULT_STALE_SECS))
}

fn env_flag_enabled(value: Option<&str>) -> bool {
    matches!(
        value.map(str::trim),
        Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES")
    )
}

fn mark_fixture_owner(path: &Path, marker: &str) -> io::Result<()> {
    fs::write(path.join(marker), "vizier test fixture\n")
}

fn write_active_pid_marker(path: &Path) -> io::Result<()> {
    fs::write(
        path.join(ACTIVE_PID_MARKER),
        format!("{}\n", std::process::id()),
    )
}

fn build_lock() -> &'static Mutex<()> {
    BUILD_LOCK.get_or_init(|| Mutex::new(()))
}

fn integration_test_lock() -> &'static Mutex<()> {
    INTEGRATION_TEST_LOCK.get_or_init(|| Mutex::new(()))
}

pub(crate) fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tests crate lives under repo root")
        .to_path_buf()
}

pub(crate) fn vizier_binary() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| build_vizier_binary(&["integration_testing"]))
}

pub(crate) fn build_vizier_binary(features: &[&str]) -> PathBuf {
    let _guard = build_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = repo_root();
    let label = if features.is_empty() {
        "base".to_string()
    } else {
        features.join("_")
    };
    let target_dir = build_root().join(format!("target-{label}"));
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
    mock_agent: bool,
    // Hold the global integration lock for the lifetime of the fixture.
    _guard: MutexGuard<'static, ()>,
}

impl IntegrationRepo {
    pub(crate) fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_binary_and_mock(vizier_binary().clone(), true)
    }

    pub(crate) fn new_without_mock() -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_binary_and_mock(vizier_binary().clone(), false)
    }

    fn with_binary_and_mock(
        bin: PathBuf,
        mock_agent: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let guard = integration_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        ensure_fixture_temp_hygiene();
        let dir = tempfile::Builder::new()
            .prefix(REPO_ROOT_PREFIX)
            .tempdir()?;
        mark_fixture_owner(dir.path(), REPO_ROOT_MARKER)?;
        write_active_pid_marker(dir.path())?;
        copy_dir_recursive(&repo_root().join("test-repo"), dir.path())?;
        copy_dir_recursive(&repo_root().join(".vizier"), &dir.path().join(".vizier"))?;
        clear_jobs_dir(dir.path())?;
        ensure_gitignore(dir.path())?;
        write_default_cicd_script(dir.path())?;
        init_repo_at(dir.path())?;
        let agent_bin_dir = create_agent_shims(dir.path())?;
        let vizier_bin_dir = dir.path().join(".vizier/tmp/bin");
        fs::create_dir_all(&vizier_bin_dir)?;
        let vizier_bin_name = bin.file_name().unwrap_or(std::ffi::OsStr::new("vizier"));
        let vizier_bin = vizier_bin_dir.join(vizier_bin_name);
        fs::copy(&bin, &vizier_bin)?;
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(&vizier_bin)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&vizier_bin, perms)?;
        }
        Ok(Self {
            dir,
            agent_bin_dir,
            vizier_bin,
            mock_agent,
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
        cmd.env("VIZIER_MOCK_AGENT", if self.mock_agent { "1" } else { "0" });
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
        cmd.arg("--follow");
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

    pub(crate) fn vizier_cmd_background_with_config(&self, config: &Path) -> Command {
        let mut cmd = self.vizier_cmd_base();
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
        if !matches!(
            status,
            "queued" | "waiting_on_deps" | "waiting_on_locks" | "running"
        ) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

pub(crate) fn wait_for_job_status(
    repo: &IntegrationRepo,
    job_id: &str,
    expected: &str,
    timeout: Duration,
) -> TestResult {
    let job_path = repo
        .path()
        .join(".vizier/jobs")
        .join(job_id)
        .join("job.json");
    let start = Instant::now();
    let mut last_status = "missing".to_string();
    loop {
        if start.elapsed() > timeout {
            return Err(format!(
                "timed out waiting for job {job_id} to reach {expected} (last {last_status})"
            )
            .into());
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
        last_status = status.to_string();
        if status == expected {
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

pub(crate) fn update_job_record<F>(repo: &IntegrationRepo, job_id: &str, updater: F) -> TestResult
where
    F: FnOnce(&mut Value),
{
    let job_path = repo
        .path()
        .join(".vizier/jobs")
        .join(job_id)
        .join("job.json");
    let contents = fs::read_to_string(&job_path)?;
    let mut record: Value = serde_json::from_str(&contents)?;
    updater(&mut record);
    fs::write(job_path, serde_json::to_string_pretty(&record)?)?;
    Ok(())
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

pub(crate) fn write_sleeping_agent(
    repo: &IntegrationRepo,
    name: &str,
    sleep_secs: u64,
) -> io::Result<PathBuf> {
    let bin_dir = repo.path().join(".vizier/tmp/bin");
    fs::create_dir_all(&bin_dir)?;
    let path = bin_dir.join(format!("{name}.sh"));
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nset -eu\ncat >/dev/null\nsleep {sleep_secs}\nprintf '%s\\n' 'mock agent response'\n"
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

pub(crate) fn write_agent_config(
    repo: &IntegrationRepo,
    filename: &str,
    scope: &str,
    agent_path: &Path,
) -> io::Result<PathBuf> {
    let config_path = repo.path().join(".vizier/tmp").join(filename);
    fs::create_dir_all(config_path.parent().unwrap())?;
    let agent = agent_path.to_string_lossy().replace('\\', "\\\\");
    let label = agent_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("custom-agent");
    fs::write(
        &config_path,
        format!("[agents.{scope}.agent]\nlabel = \"{label}\"\ncommand = [\"{agent}\"]\n"),
    )?;
    Ok(config_path)
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
    writeln!(file, ".vizier/jobs/")?;
    writeln!(file, ".vizier/sessions/")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_cleanup_removes_orphaned_fixture_temp_dirs() -> TestResult {
        let temp_root = tempfile::tempdir()?;

        let stale_build = temp_root.path().join("vizier-tests-build-stale");
        fs::create_dir_all(&stale_build)?;
        mark_fixture_owner(&stale_build, BUILD_ROOT_MARKER)?;
        fs::write(stale_build.join(ACTIVE_PID_MARKER), "999999\n")?;

        let stale_repo = temp_root.path().join("vizier-tests-repo-stale");
        fs::create_dir_all(stale_repo.join(".vizier"))?;
        mark_fixture_owner(&stale_repo, REPO_ROOT_MARKER)?;
        fs::write(stale_repo.join(ACTIVE_PID_MARKER), "999999\n")?;

        let stale_legacy = temp_root.path().join(".tmplegacy");
        fs::create_dir_all(stale_legacy.join(".git"))?;
        fs::create_dir_all(stale_legacy.join(".vizier"))?;
        fs::write(stale_legacy.join("a"), "a")?;
        fs::write(stale_legacy.join("b"), "b")?;
        fs::write(stale_legacy.join("c"), "c")?;

        let stats = cleanup_stale_fixture_temp_dirs_in(
            temp_root.path(),
            SystemTime::now(),
            Duration::from_secs(0),
        )?;
        assert!(
            !stale_build.exists(),
            "stale build root should be cleaned up"
        );
        assert!(!stale_repo.exists(), "stale repo root should be cleaned up");
        assert!(
            !stale_legacy.exists(),
            "legacy stale repo root should be cleaned up"
        );
        assert_eq!(
            stats.removed_build_roots, 1,
            "expected one stale build root to be removed"
        );
        assert_eq!(
            stats.removed_repo_roots, 2,
            "expected one stale fixture repo and one legacy repo to be removed"
        );
        Ok(())
    }

    #[test]
    fn stale_cleanup_preserves_active_and_non_owned_dirs() -> TestResult {
        let temp_root = tempfile::tempdir()?;

        let active_repo = temp_root.path().join("vizier-tests-repo-active");
        fs::create_dir_all(active_repo.join(".vizier"))?;
        mark_fixture_owner(&active_repo, REPO_ROOT_MARKER)?;
        write_active_pid_marker(&active_repo)?;

        let non_owned_tmp = temp_root.path().join(".tmp-other-project");
        fs::create_dir_all(&non_owned_tmp)?;
        fs::write(non_owned_tmp.join("README.md"), "not a vizier fixture")?;

        let stats = cleanup_stale_fixture_temp_dirs_in(
            temp_root.path(),
            SystemTime::now(),
            Duration::from_secs(0),
        )?;
        assert!(active_repo.exists(), "active repo should not be removed");
        assert!(
            non_owned_tmp.exists(),
            "non-owned .tmp directory should not be removed"
        );
        assert_eq!(
            stats.removed_repo_roots, 0,
            "expected no repos to be removed"
        );
        assert_eq!(
            stats.removed_build_roots, 0,
            "expected no build roots to be removed"
        );
        Ok(())
    }

    #[test]
    fn stale_cleanup_keeps_recent_markerless_dirs_until_window_expires() -> TestResult {
        let temp_root = tempfile::tempdir()?;

        let markerless_build = temp_root.path().join("vizier-tests-build-recent");
        fs::create_dir_all(&markerless_build)?;

        let markerless_legacy = temp_root.path().join(".tmprecent");
        fs::create_dir_all(markerless_legacy.join(".git"))?;
        fs::create_dir_all(markerless_legacy.join(".vizier"))?;
        fs::write(markerless_legacy.join("a"), "a")?;
        fs::write(markerless_legacy.join("b"), "b")?;
        fs::write(markerless_legacy.join("c"), "c")?;

        let stats = cleanup_stale_fixture_temp_dirs_in(
            temp_root.path(),
            SystemTime::now(),
            Duration::from_secs(60 * 60),
        )?;
        assert!(
            markerless_build.exists(),
            "recent markerless build root should not be removed"
        );
        assert!(
            markerless_legacy.exists(),
            "recent markerless legacy root should not be removed"
        );
        assert_eq!(
            stats.removed_build_roots, 0,
            "expected no build roots to be removed"
        );
        assert_eq!(
            stats.removed_repo_roots, 0,
            "expected no repo roots to be removed"
        );
        Ok(())
    }

    #[test]
    fn build_root_preserve_mode_is_opt_in() -> TestResult {
        let temp_root = tempfile::tempdir()?;

        let ephemeral = create_build_root_in(temp_root.path(), false)?;
        let ephemeral_path = ephemeral.path.clone();
        assert!(ephemeral_path.exists(), "ephemeral build root should exist");
        drop(ephemeral);
        assert!(
            !ephemeral_path.exists(),
            "default build root should be removed on drop"
        );

        let preserved = create_build_root_in(temp_root.path(), true)?;
        let preserved_path = preserved.path.clone();
        assert!(preserved_path.exists(), "preserved build root should exist");
        drop(preserved);
        assert!(
            preserved_path.exists(),
            "opt-in preserve mode should keep the build root"
        );
        fs::remove_dir_all(&preserved_path)?;
        Ok(())
    }

    #[test]
    fn env_flag_enabled_accepts_expected_truthy_values() {
        assert!(env_flag_enabled(Some("1")));
        assert!(env_flag_enabled(Some("true")));
        assert!(env_flag_enabled(Some("TRUE")));
        assert!(env_flag_enabled(Some("yes")));
        assert!(env_flag_enabled(Some("YES")));
        assert!(!env_flag_enabled(Some("0")));
        assert!(!env_flag_enabled(Some("false")));
        assert!(!env_flag_enabled(None));
    }
}
