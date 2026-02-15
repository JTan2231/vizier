#![allow(dead_code, unused_imports)]

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
const SERIAL_TEST_ENV: &str = "VIZIER_TEST_SERIAL";
const JOB_POLL_MS_ENV: &str = "VIZIER_TEST_JOB_POLL_MS";
const DEFAULT_STALE_SECS: u64 = 60 * 30;
const DEFAULT_JOB_POLL_MS: u64 = 50;

static BUILD_ROOT: OnceLock<BuildRoot> = OnceLock::new();
static TEMPLATE_REPO: OnceLock<PathBuf> = OnceLock::new();
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

fn template_repo() -> &'static PathBuf {
    TEMPLATE_REPO.get_or_init(create_template_repo)
}

fn create_build_root() -> BuildRoot {
    ensure_fixture_temp_hygiene();
    create_build_root_in(&env::temp_dir(), keep_temp_artifacts())
        .expect("create temp dir for vizier test builds")
}

fn create_template_repo() -> PathBuf {
    let template_path = build_root().join("repo-template");
    if template_path.exists() {
        fs::remove_dir_all(&template_path)
            .unwrap_or_else(|err| panic!("remove stale fixture template repo: {err}"));
    }

    let source_root = repo_root();
    copy_dir_recursive(&source_root.join("test-repo"), &template_path)
        .expect("copy test-repo into fixture template");
    seed_vizier_dir(&source_root, &template_path)
        .expect("seed .vizier runtime surface into fixture template");
    clear_jobs_dir(&template_path).expect("clear jobs dir in fixture template");
    ensure_gitignore(&template_path).expect("ensure .gitignore for fixture template");
    write_default_cicd_script(&template_path).expect("write default cicd.sh for fixture template");
    create_agent_shims(&template_path).expect("create agent shims in fixture template");
    init_repo_at(&template_path).expect("initialize fixture template repo");
    template_path
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

fn job_poll_interval() -> Duration {
    let poll_ms = env::var(JOB_POLL_MS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_JOB_POLL_MS);
    Duration::from_millis(poll_ms)
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
    let target_dir = fixture_target_dir(&root, env::var_os("CARGO_TARGET_DIR").as_deref());
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
    let compiled_path = target_dir
        .join("release")
        .join(format!("vizier{}", env::consts::EXE_SUFFIX));
    if !compiled_path.exists() {
        panic!("expected vizier binary at {}", compiled_path.display());
    }

    // Keep a process-local fixture copy under the build root so integration repos can
    // link to a stable source path even when Cargo's target directory is shared.
    let fixture_bin_dir = build_root().join("bin-cache").join(&label);
    fs::create_dir_all(&fixture_bin_dir).expect("create fixture binary cache directory");
    let fixture_bin = fixture_bin_dir.join(
        compiled_path
            .file_name()
            .unwrap_or(std::ffi::OsStr::new("vizier")),
    );
    link_or_copy_file(&compiled_path, &fixture_bin).expect("stage fixture vizier binary");
    fixture_bin
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

fn copy_file_if_exists(src: &Path, dst: &Path) -> io::Result<()> {
    if !src.is_file() {
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(src, dst)?;
    Ok(())
}

fn fixture_target_dir(repo_root: &Path, cargo_target_dir: Option<&std::ffi::OsStr>) -> PathBuf {
    cargo_target_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root.join(".vizier/tmp/cargo-target"))
}

fn clone_template_repo(template: &Path, dst: &Path) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let status = Command::new("git")
        .arg("clone")
        .arg("--local")
        .arg("--quiet")
        .arg(template)
        .arg(dst)
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "git clone from fixture template failed with status {status:?}"
        )));
    }

    let cleanup_status = Command::new("git")
        .arg("-C")
        .arg(dst)
        .args(["remote", "remove", "origin"])
        .status()?;
    if !cleanup_status.success() {
        return Err(io::Error::other(format!(
            "git remote cleanup failed with status {cleanup_status:?}"
        )));
    }
    Ok(())
}

fn link_or_copy_file(src: &Path, dst: &Path) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    if dst.exists() {
        fs::remove_file(dst)?;
    }
    if fs::hard_link(src, dst).is_ok() {
        return Ok(());
    }
    #[cfg(unix)]
    if std::os::unix::fs::symlink(src, dst).is_ok() {
        return Ok(());
    }
    fs::copy(src, dst)?;
    Ok(())
}

fn seed_vizier_dir(source_repo_root: &Path, target_repo_root: &Path) -> io::Result<()> {
    let source_vizier = source_repo_root.join(".vizier");
    let target_vizier = target_repo_root.join(".vizier");
    fs::create_dir_all(&target_vizier)?;

    copy_file_if_exists(
        &source_vizier.join("config.toml"),
        &target_vizier.join("config.toml"),
    )?;
    copy_file_if_exists(
        &source_vizier.join("config.json"),
        &target_vizier.join("config.json"),
    )?;
    copy_file_if_exists(
        &source_vizier.join("develop.toml"),
        &target_vizier.join("develop.toml"),
    )?;

    let source_workflow = source_vizier.join("workflow");
    if source_workflow.is_dir() {
        copy_dir_recursive(&source_workflow, &target_vizier.join("workflow"))?;
    }

    let source_workflows = source_vizier.join("workflows");
    if source_workflows.is_dir() {
        copy_dir_recursive(&source_workflows, &target_vizier.join("workflows"))?;
    }

    let source_prompts = source_vizier.join("prompts");
    if source_prompts.is_dir() {
        copy_dir_recursive(&source_prompts, &target_vizier.join("prompts"))?;
    }

    let source_narrative = source_vizier.join("narrative");
    if source_narrative.is_dir() {
        copy_dir_recursive(&source_narrative, &target_vizier.join("narrative"))?;
    }

    // Keep these directories available so plan/state operations can create files lazily.
    fs::create_dir_all(target_vizier.join("implementation-plans"))?;
    fs::create_dir_all(target_vizier.join("state/plans"))?;
    Ok(())
}

pub(crate) struct IntegrationRepo {
    _dir: TempDir,
    repo_path: PathBuf,
    backend_bin_dir: PathBuf,
    agent_bin_dir: PathBuf,
    vizier_bin: PathBuf,
    mock_agent: bool,
    // Optional fixture-level lock for forcing serialized integration execution.
    _serial_guard: Option<MutexGuard<'static, ()>>,
}

impl IntegrationRepo {
    pub(crate) fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_binary_and_mock(vizier_binary().clone(), true, false)
    }

    pub(crate) fn new_serial() -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_binary_and_mock(vizier_binary().clone(), true, true)
    }

    pub(crate) fn new_without_mock() -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_binary_and_mock(vizier_binary().clone(), false, false)
    }

    fn with_binary_and_mock(
        bin: PathBuf,
        mock_agent: bool,
        force_serial: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let template = template_repo().clone();
        let serial_guard =
            if force_serial || env_flag_enabled(env::var(SERIAL_TEST_ENV).ok().as_deref()) {
                Some(
                    integration_test_lock()
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()),
                )
            } else {
                None
            };
        ensure_fixture_temp_hygiene();
        let dir = tempfile::Builder::new()
            .prefix(REPO_ROOT_PREFIX)
            .tempdir()?;
        let repo_path = dir.path().join("repo");
        clone_template_repo(&template, &repo_path)?;
        mark_fixture_owner(dir.path(), REPO_ROOT_MARKER)?;
        write_active_pid_marker(dir.path())?;
        let backend_bin_dir = create_backend_stubs(&repo_path)?;
        let agent_bin_dir = repo_path.join(".vizier/tmp/bin");
        let vizier_bin_dir = repo_path.join(".vizier/tmp/bin");
        fs::create_dir_all(&vizier_bin_dir)?;
        let vizier_bin_name = bin.file_name().unwrap_or(std::ffi::OsStr::new("vizier"));
        let vizier_bin = vizier_bin_dir.join(vizier_bin_name);
        link_or_copy_file(&bin, &vizier_bin)?;
        Ok(Self {
            _dir: dir,
            repo_path,
            backend_bin_dir,
            agent_bin_dir,
            vizier_bin,
            mock_agent,
            _serial_guard: serial_guard,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.repo_path
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
        let mut paths = vec![self.backend_bin_dir.clone(), self.agent_bin_dir.clone()];
        if let Some(existing) = env::var_os("PATH") {
            paths.extend(env::split_paths(&existing));
        }
        if let Ok(joined) = env::join_paths(paths) {
            cmd.env("PATH", joined);
        }
        cmd
    }

    pub(crate) fn vizier_cmd(&self) -> Command {
        self.vizier_cmd_base()
    }

    pub(crate) fn vizier_cmd_follow(&self) -> Command {
        self.vizier_cmd_base()
    }

    pub(crate) fn vizier_cmd_no_follow(&self) -> Command {
        self.vizier_cmd_base()
    }

    pub(crate) fn vizier_cmd_background(&self) -> Command {
        self.vizier_cmd_no_follow()
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
        let mut cmd = self.vizier_cmd_no_follow();
        cmd.args(args);
        cmd.output()
    }

    pub(crate) fn vizier_output_no_follow(&self, args: &[&str]) -> io::Result<Output> {
        let mut cmd = self.vizier_cmd_no_follow();
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
    let poll_interval = job_poll_interval();
    let job_path = repo
        .path()
        .join(".vizier/jobs")
        .join(job_id)
        .join("job.json");
    let start = Instant::now();
    let mut last_status = "unknown".to_string();
    let mut last_wait_reason = None::<String>;
    loop {
        if start.elapsed() > timeout {
            let detail = match last_wait_reason {
                Some(reason) => format!("status={last_status}, wait_reason={reason}"),
                None => format!("status={last_status}"),
            };
            return Err(format!("timed out waiting for job {job_id} ({detail})").into());
        }
        let Ok(contents) = fs::read_to_string(&job_path) else {
            std::thread::sleep(poll_interval);
            continue;
        };
        let record: Value = serde_json::from_str(&contents)?;
        let status = record
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        last_status = status.to_string();
        last_wait_reason = record
            .pointer("/schedule/wait_reason/detail")
            .and_then(Value::as_str)
            .map(|value| value.to_string());
        if !matches!(
            status,
            "queued" | "waiting_on_deps" | "waiting_on_approval" | "waiting_on_locks" | "running"
        ) {
            return Ok(());
        }
        std::thread::sleep(poll_interval);
    }
}

pub(crate) fn wait_for_job_status(
    repo: &IntegrationRepo,
    job_id: &str,
    expected: &str,
    timeout: Duration,
) -> TestResult {
    let poll_interval = job_poll_interval();
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
            std::thread::sleep(poll_interval);
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
        std::thread::sleep(poll_interval);
    }
}

pub(crate) fn schedule_job(repo: &IntegrationRepo, args: &[&str]) -> TestResult<(Output, String)> {
    let output = repo.vizier_output_no_follow(args)?;
    if !output.status.success() {
        return Err(format!(
            "failed to schedule {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let job_id = extract_job_id(&stdout)
        .ok_or_else(|| format!("scheduled command did not report a job id: {:?}", args))?;
    Ok((output, job_id))
}

pub(crate) fn schedule_job_and_wait(
    repo: &IntegrationRepo,
    args: &[&str],
    timeout: Duration,
) -> TestResult<(Output, Value)> {
    let (output, job_id) = schedule_job(repo, args)?;
    wait_for_job_completion(repo, &job_id, timeout)?;
    let record = read_job_record(repo, &job_id)?;
    Ok((output, record))
}

pub(crate) fn schedule_job_and_expect_status(
    repo: &IntegrationRepo,
    args: &[&str],
    expected_status: &str,
    timeout: Duration,
) -> TestResult<Output> {
    let (output, record) = schedule_job_and_wait(repo, args, timeout)?;
    let status = record
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if status != expected_status {
        return Err(format!(
            "scheduled {:?} finished with status {status}; expected {expected_status}",
            args
        )
        .into());
    }
    Ok(output)
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

pub(crate) fn write_gated_agent(
    repo: &IntegrationRepo,
    name: &str,
    gate_name: &str,
) -> io::Result<(PathBuf, PathBuf)> {
    let bin_dir = repo.path().join(".vizier/tmp/bin");
    fs::create_dir_all(&bin_dir)?;

    let gate_path = repo.path().join(".vizier/tmp/gates").join(gate_name);
    if let Some(parent) = gate_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if gate_path.exists() {
        fs::remove_file(&gate_path)?;
    }

    let gate = gate_path.to_string_lossy().replace('\\', "\\\\");
    let path = bin_dir.join(format!("{name}.sh"));
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nset -eu\ncat >/dev/null\ngate=\"{gate}\"\nwhile [ ! -f \"$gate\" ]; do sleep 0.02; done\nprintf '%s\\n' 'mock agent response'\n"
        ),
    )?;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms)?;
    }
    Ok((path, gate_path))
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

fn create_backend_stubs(root: &Path) -> io::Result<PathBuf> {
    // Guard against accidental paid backend usage in integration tests by
    // front-loading local codex/gemini stub binaries on PATH.
    let bin_dir = root.join(".vizier/tmp/backend-bin");
    fs::create_dir_all(&bin_dir)?;

    let codex_stub = r#"#!/bin/sh
set -eu
if [ -n "${INPUT_LOG:-}" ]; then
  cat >"${INPUT_LOG}"
else
  cat >/dev/null
fi
if [ -n "${ARGS_LOG:-}" ]; then
  printf "%s\n" "$*" >"${ARGS_LOG}"
fi
if [ -n "${PAYLOAD:-}" ]; then
  printf "%s\n" "${PAYLOAD}"
  exit 0
fi
printf '%s\n' '{"type":"item.started","item":{"type":"reasoning","text":"prep"}}'
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"mock agent response"}}'
"#;
    let gemini_stub = r#"#!/bin/sh
set -eu
if [ -n "${INPUT_LOG:-}" ]; then
  cat >"${INPUT_LOG}"
else
  cat >/dev/null
fi
if [ -n "${ARGS_LOG:-}" ]; then
  printf "%s\n" "$*" >"${ARGS_LOG}"
fi
if [ -n "${PAYLOAD:-}" ]; then
  printf "%s\n" "${PAYLOAD}"
  exit 0
fi
printf '%s\n' '{"type":"message","role":"assistant","content":"mock agent response","delta":false}'
printf '%s\n' '{"type":"result","status":"success","result":"mock agent response","stats":{"total_tokens":1,"input_tokens":1,"output_tokens":1}}'
"#;

    for (name, contents) in [("codex", codex_stub), ("gemini", gemini_stub)] {
        let path = bin_dir.join(name);
        fs::write(&path, contents)?;
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(&path)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms)?;
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

    #[test]
    fn clone_template_repo_clones_initialized_repo() -> TestResult {
        let temp_root = tempfile::tempdir()?;
        let template = temp_root.path().join("template");
        fs::create_dir_all(&template)?;
        fs::write(template.join("a"), "seed\n")?;
        init_repo_at(&template)?;

        let cloned = temp_root.path().join("cloned/repo");
        clone_template_repo(&template, &cloned)?;

        assert!(
            cloned.join(".git").is_dir(),
            "expected clone destination to contain a git repo"
        );
        assert_eq!(
            fs::read_to_string(cloned.join("a"))?,
            "seed\n",
            "expected clone to preserve working tree contents"
        );
        assert!(
            Repository::open(&cloned).is_ok(),
            "expected clone destination to be openable as a repository"
        );
        Ok(())
    }

    #[test]
    fn create_backend_stubs_writes_local_codex_and_gemini_binaries() -> TestResult {
        let temp_root = tempfile::tempdir()?;
        let bin_dir = create_backend_stubs(temp_root.path())?;
        for (name, expected) in [
            ("codex", "\"type\":\"item.completed\""),
            ("gemini", "\"type\":\"result\""),
        ] {
            let stub = bin_dir.join(name);
            assert!(
                stub.is_file(),
                "expected backend stub at {}",
                stub.display()
            );

            let mut cmd = Command::new(&stub);
            cmd.arg("exec");
            cmd.stdin(Stdio::piped());
            cmd.stdout(Stdio::piped());
            let mut child = cmd.spawn()?;
            child
                .stdin
                .as_mut()
                .ok_or("stub stdin missing")?
                .write_all(b"test prompt\n")?;
            let output = child.wait_with_output()?;
            assert!(
                output.status.success(),
                "backend stub should exit success for {name}"
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                stdout.contains(expected),
                "stub output for {name} should include {expected}: {stdout}"
            );
        }
        Ok(())
    }

    #[test]
    fn seed_vizier_dir_copies_minimal_runtime_surface() -> TestResult {
        let temp_root = tempfile::tempdir()?;
        let source = temp_root.path().join("source");
        let target = temp_root.path().join("target");

        fs::create_dir_all(source.join(".vizier/narrative/threads"))?;
        fs::create_dir_all(source.join(".vizier/workflow"))?;
        fs::create_dir_all(source.join(".vizier/prompts"))?;
        fs::create_dir_all(source.join(".vizier/tmp/cargo-target/debug"))?;
        fs::create_dir_all(source.join(".vizier/jobs/job-1"))?;
        fs::create_dir_all(source.join(".vizier/sessions/s1"))?;
        fs::create_dir_all(source.join(".vizier/tmp-worktrees/w1"))?;

        fs::write(source.join(".vizier/config.toml"), "agent = \"codex\"\n")?;
        fs::write(
            source.join(".vizier/config.json"),
            "{ \"agent\": \"gemini\" }\n",
        )?;
        fs::write(
            source.join(".vizier/develop.toml"),
            "id = \"template.develop\"\n",
        )?;
        fs::write(
            source.join(".vizier/workflow/draft.toml"),
            "id = \"template.stage.draft\"\n",
        )?;
        fs::write(
            source.join(".vizier/prompts/DRAFT_PROMPTS.md"),
            "# draft prompt\n",
        )?;
        fs::write(source.join(".vizier/narrative/snapshot.md"), "snapshot\n")?;
        fs::write(source.join(".vizier/narrative/glossary.md"), "glossary\n")?;
        fs::write(source.join(".vizier/narrative/threads/demo.md"), "thread\n")?;
        fs::write(
            source.join(".vizier/tmp/cargo-target/debug/heavy-artifact"),
            "blob",
        )?;
        fs::write(source.join(".vizier/jobs/job-1/job.json"), "{}\n")?;
        fs::write(
            source.join(".vizier/sessions/s1/session.json"),
            "{ \"id\": \"s1\" }\n",
        )?;
        fs::write(source.join(".vizier/tmp-worktrees/w1/marker.txt"), "wt\n")?;

        seed_vizier_dir(&source, &target)?;

        assert!(
            target.join(".vizier/config.toml").is_file(),
            "expected config.toml to be copied"
        );
        assert!(
            target.join(".vizier/config.json").is_file(),
            "expected config.json to be copied when present"
        );
        assert!(
            target.join(".vizier/narrative/snapshot.md").is_file(),
            "expected narrative snapshot to be copied"
        );
        assert!(
            target.join(".vizier/narrative/threads/demo.md").is_file(),
            "expected narrative threads to be copied"
        );
        assert!(
            target.join(".vizier/develop.toml").is_file(),
            "expected develop workflow composition template to be copied"
        );
        assert!(
            target.join(".vizier/workflow/draft.toml").is_file(),
            "expected workflow stage templates to be copied"
        );
        assert!(
            target.join(".vizier/prompts/DRAFT_PROMPTS.md").is_file(),
            "expected stage prompt files to be copied"
        );
        assert!(
            target.join(".vizier/implementation-plans").is_dir(),
            "expected implementation-plans dir to be seeded"
        );
        assert!(
            target.join(".vizier/state/plans").is_dir(),
            "expected state/plans dir to be seeded"
        );

        assert!(
            !target.join(".vizier/tmp").exists(),
            "transient .vizier/tmp should not be copied"
        );
        assert!(
            !target.join(".vizier/jobs").exists(),
            "transient .vizier/jobs should not be copied"
        );
        assert!(
            !target.join(".vizier/sessions").exists(),
            "transient .vizier/sessions should not be copied"
        );
        assert!(
            !target.join(".vizier/tmp-worktrees").exists(),
            "transient .vizier/tmp-worktrees should not be copied"
        );
        Ok(())
    }

    #[test]
    fn fixture_target_dir_prefers_explicit_cargo_target_dir() -> TestResult {
        let temp_root = tempfile::tempdir()?;
        let override_target = temp_root.path().join("cargo-target-override");
        let resolved = fixture_target_dir(temp_root.path(), Some(override_target.as_os_str()));
        assert_eq!(
            resolved, override_target,
            "explicit cargo target dir should be used for fixture builds"
        );
        Ok(())
    }

    #[test]
    fn fixture_target_dir_defaults_to_workspace_target() -> TestResult {
        let temp_root = tempfile::tempdir()?;
        let resolved = fixture_target_dir(temp_root.path(), None);
        assert_eq!(
            resolved,
            temp_root.path().join(".vizier/tmp/cargo-target"),
            "fixture builds should default to the shared Vizier cargo target dir"
        );
        Ok(())
    }

    #[test]
    fn link_or_copy_file_replaces_existing_destination() -> TestResult {
        let temp_root = tempfile::tempdir()?;
        let source = temp_root.path().join("source-vizier");
        let destination = temp_root.path().join("nested/destination-vizier");
        fs::write(&source, "new-binary-content")?;
        fs::create_dir_all(destination.parent().ok_or("destination parent missing")?)?;
        fs::write(&destination, "old-content")?;

        link_or_copy_file(&source, &destination)?;

        assert_eq!(
            fs::read_to_string(&destination)?,
            "new-binary-content",
            "destination should match the source contents after linking/copying"
        );
        Ok(())
    }
}
