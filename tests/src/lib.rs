#![cfg(test)]

use git2::{
    BranchType, DiffOptions, IndexAddOption, Oid, Repository, Signature, Sort,
    build::CheckoutBuilder,
};
use serde_json::Value;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
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
    agent_bin_dir: PathBuf,
}

impl IntegrationRepo {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let dir = TempDir::new()?;
        copy_dir_recursive(&repo_root().join("test-repo"), dir.path())?;
        copy_dir_recursive(&repo_root().join(".vizier"), &dir.path().join(".vizier"))?;
        ensure_gitignore(dir.path())?;
        write_default_cicd_script(dir.path())?;
        init_repo_at(dir.path())?;
        let agent_bin_dir = create_agent_shims(dir.path())?;
        Ok(Self { dir, agent_bin_dir })
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
        let mut paths = vec![self.agent_bin_dir.clone()];
        if let Some(existing) = env::var_os("PATH") {
            paths.extend(env::split_paths(&existing));
        }
        if let Ok(joined) = env::join_paths(paths) {
            cmd.env("PATH", joined);
        }
        cmd
    }

    fn vizier_cmd_with_config(&self, config: &Path) -> Command {
        let mut cmd = self.vizier_cmd();
        cmd.env("VIZIER_CONFIG_FILE", config);
        cmd.arg("--config-file");
        cmd.arg(config);
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

fn files_changed_in_commit(repo: &Repository, spec: &str) -> Result<HashSet<String>, git2::Error> {
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

fn count_commits_from_head(repo: &Repository) -> Result<usize, git2::Error> {
    let mut walk = repo.revwalk()?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
    walk.push_head()?;
    Ok(walk.count())
}

fn find_save_field(output: &str, key: &str) -> Option<String> {
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

fn session_log_contents_from_output(
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UsageSnapshot {
    prompt_total: usize,
    completion_total: usize,
    total: usize,
    prompt_delta: usize,
    completion_delta: usize,
    total_delta: usize,
    cached_input_total: usize,
    cached_input_delta: usize,
    reasoning_output_total: usize,
    reasoning_output_delta: usize,
    known: bool,
}

fn parse_session_usage(contents: &str) -> Result<UsageSnapshot, Box<dyn std::error::Error>> {
    let value: Value = serde_json::from_str(contents)?;
    let usage = value
        .get("outcome")
        .and_then(|outcome| outcome.get("token_usage"))
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::Other, "session outcome missing token usage")
        })?;

    let mut snapshot = UsageSnapshot::default();
    snapshot.prompt_total = usage_value(usage, "prompt_total")?;
    snapshot.completion_total = usage_value(usage, "completion_total")?;
    snapshot.total = usage_value(usage, "total")?;
    snapshot.prompt_delta = usage_value(usage, "prompt_delta")?;
    snapshot.completion_delta = usage_value(usage, "completion_delta")?;
    snapshot.total_delta = usage_value(usage, "delta_total")?;
    snapshot.cached_input_total = usage_value_optional(usage, "cached_input_total")?;
    snapshot.cached_input_delta = usage_value_optional(usage, "cached_input_delta")?;
    snapshot.reasoning_output_total = usage_value_optional(usage, "reasoning_output_total")?;
    snapshot.reasoning_output_delta = usage_value_optional(usage, "reasoning_output_delta")?;
    snapshot.known = usage
        .get("known")
        .and_then(Value::as_bool)
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "token_usage.known missing"))?;
    Ok(snapshot)
}

fn usage_value(usage: &Value, key: &str) -> Result<usize, Box<dyn std::error::Error>> {
    usage
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, format!("token_usage.{key} missing")))
        .map(|value| value as usize)
        .map_err(|err| err.into())
}

fn usage_value_optional(usage: &Value, key: &str) -> Result<usize, Box<dyn std::error::Error>> {
    Ok(usage.get(key).and_then(Value::as_u64).unwrap_or(0) as usize)
}

fn format_number(value: usize) -> String {
    let digits: Vec<char> = value.to_string().chars().collect();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);

    for (idx, ch) in digits.iter().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            formatted.push(',');
        }
        formatted.push(*ch);
    }

    formatted.chars().rev().collect()
}

fn gather_session_logs(repo: &IntegrationRepo) -> io::Result<Vec<PathBuf>> {
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

fn new_session_log<'a>(before: &'a [PathBuf], after: &'a [PathBuf]) -> Option<&'a PathBuf> {
    let before_set: HashSet<_> = before.iter().collect();
    after.iter().find(|path| !before_set.contains(path))
}

fn usage_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|line| line.contains("[usage]"))
        .collect()
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

fn write_cicd_script(repo: &IntegrationRepo, name: &str, contents: &str) -> io::Result<PathBuf> {
    let scripts_dir = repo.path().join(".vizier/tmp/cicd-scripts");
    fs::create_dir_all(&scripts_dir)?;
    let path = scripts_dir.join(name);
    fs::write(&path, contents)?;
    Ok(path)
}

fn create_agent_shims(root: &Path) -> io::Result<PathBuf> {
    // Keep shims under .vizier/tmp so they stay ignored when commands require a clean tree.
    let bin_dir = root.join(".vizier/tmp/bin");
    fs::create_dir_all(&bin_dir)?;
    for name in ["codex", "gemini"] {
        let path = bin_dir.join(name);
        fs::write(&path, "#!/bin/sh\nexit 0\n")?;
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(&path)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms)?;
        }
    }
    Ok(bin_dir)
}

fn write_default_cicd_script(repo_root: &Path) -> io::Result<()> {
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

fn ensure_gitignore(path: &Path) -> io::Result<()> {
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

#[test]
fn test_save() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;

    let output = repo.vizier_cmd().arg("save").output()?;
    assert!(
        output.status.success(),
        "vizier save exited with {:?}",
        output.status
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    let after = count_commits_from_head(&repo.repo())?;
    assert_eq!(after - before, 1, "save should create a single commit");

    let files = files_changed_in_commit(&repo.repo(), "HEAD")?;
    assert!(
        files.contains("a") && files.contains(".vizier/.snapshot"),
        "combined commit should include code + narrative files, got {files:?}"
    );

    let snapshot = repo.read(".vizier/.snapshot")?;
    assert!(
        snapshot.contains("some snapshot change"),
        "expected mock backend snapshot update"
    );

    let session_log = session_log_contents_from_output(&repo, &stdout)?;
    assert!(
        session_log
            .to_ascii_lowercase()
            .contains("mock agent response"),
        "session log missing backend response"
    );
    Ok(())
}

#[test]
fn test_save_with_staged_files() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;
    repo.write("b", "this is an integration test")?;
    add_all(&repo.repo(), &["."])?;

    let status = repo.vizier_cmd().arg("save").status()?;
    assert!(status.success(), "vizier save exited with {status:?}");

    let repo_handle = repo.repo();
    let after = count_commits_from_head(&repo_handle)?;
    assert_eq!(
        after - before,
        1,
        "save should still create a single combined commit when files are pre-staged"
    );
    let files = files_changed_in_commit(&repo_handle, "HEAD")?;
    assert!(
        files.contains("b") && files.contains(".vizier/.snapshot"),
        "combined commit should include staged code and narrative files, got {files:?}"
    );
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
    let session_log = session_log_contents_from_output(&repo, &stdout)?;
    assert!(
        session_log
            .to_ascii_lowercase()
            .contains("mock agent response"),
        "session log missing backend response"
    );

    let after = count_commits_from_head(&repo.repo())?;
    assert_eq!(after - before, 1, "should create a single commit");
    let files = files_changed_in_commit(&repo.repo(), "HEAD")?;
    assert!(
        files.contains(".vizier/.snapshot") && !files.contains("a"),
        "expected commit to contain only narrative assets when code changes are skipped, got {files:?}"
    );
    Ok(())
}

#[test]
fn test_save_no_commit_leaves_pending_changes() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;

    let output = repo.vizier_cmd().args(["--no-commit", "save"]).output()?;
    assert!(
        output.status.success(),
        "vizier save --no-commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after = count_commits_from_head(&repo.repo())?;
    assert_eq!(
        after, before,
        "no-commit save should not create new commits"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Mode       : manual"),
        "expected manual mode indicator in output but saw: {stdout}"
    );

    let status = Command::new("git")
        .args([
            "-C",
            repo.path().to_str().unwrap(),
            "status",
            "--short",
            ".vizier/.snapshot",
        ])
        .output()?;
    let status_stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        status_stdout.contains(".vizier/.snapshot"),
        "expected .vizier/.snapshot to be dirty after --no-commit save, git status was: {status_stdout}"
    );

    let code_status = Command::new("git")
        .args([
            "-C",
            repo.path().to_str().unwrap(),
            "status",
            "--short",
            "a",
        ])
        .output()?;
    let code_stdout = String::from_utf8_lossy(&code_status.stdout);
    assert!(
        code_stdout.contains("a"),
        "expected code changes to remain unstaged after --no-commit save, git status was: {code_stdout}"
    );
    Ok(())
}

#[test]
fn test_ask_creates_single_combined_commit() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;

    let output = repo.vizier_output(&["ask", "single commit check"])?;
    assert!(
        output.status.success(),
        "vizier ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after = count_commits_from_head(&repo.repo())?;
    assert_eq!(after - before, 1, "ask should create one combined commit");
    let files = files_changed_in_commit(&repo.repo(), "HEAD")?;
    assert!(
        files.contains(".vizier/.snapshot") && files.contains("a"),
        "ask commit should include code and narrative assets, got {files:?}"
    );
    Ok(())
}

#[test]
fn test_agent_scope_resolution() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let config_path = repo.path().join("agents.toml");
    fs::write(
        &config_path,
        r#"
[agents.default]
backend = "codex"

[agents.ask]
backend = "wire"
model = "gpt-5"
reasoning_effort = "low"
"#,
    )?;

    let before_logs = gather_session_logs(&repo)?;
    let ask_output = repo
        .vizier_cmd_with_config(&config_path)
        .args(["ask", "refresh snapshot context"])
        .output()?;
    assert!(
        ask_output.status.success(),
        "vizier ask failed: {}",
        String::from_utf8_lossy(&ask_output.stderr)
    );
    let after_logs = gather_session_logs(&repo)?;
    let new_log = new_session_log(&before_logs, &after_logs)
        .ok_or_else(|| "expected vizier ask to write a session log")?;
    let ask_contents = fs::read_to_string(new_log)?;
    let ask_json: Value = serde_json::from_str(&ask_contents)?;
    assert_eq!(
        ask_json
            .get("model")
            .and_then(|model| model.get("provider"))
            .and_then(Value::as_str),
        Some("wire"),
        "ask session log should report the wire backend"
    );
    assert_eq!(
        ask_json
            .get("model")
            .and_then(|model| model.get("scope"))
            .and_then(Value::as_str),
        Some("ask"),
        "ask session log should report scope=ask"
    );

    let save_output = repo
        .vizier_cmd_with_config(&config_path)
        .arg("save")
        .output()?;
    assert!(
        save_output.status.success(),
        "vizier save failed: {}",
        String::from_utf8_lossy(&save_output.stderr)
    );
    let stdout = String::from_utf8_lossy(&save_output.stdout);
    let save_contents = session_log_contents_from_output(&repo, &stdout)?;
    let save_json: Value = serde_json::from_str(&save_contents)?;
    assert_eq!(
        save_json
            .get("model")
            .and_then(|model| model.get("provider"))
            .and_then(Value::as_str),
        Some("codex"),
        "save should use the configured agent backend runner"
    );
    assert_eq!(
        save_json
            .get("model")
            .and_then(|model| model.get("scope"))
            .and_then(Value::as_str),
        Some("save"),
        "save session log should include scope"
    );

    Ok(())
}

#[test]
fn test_repo_config_overrides_env_config() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let repo_config = repo.path().join(".vizier").join("config.toml");
    fs::write(
        &repo_config,
        r#"
[agents.default]
backend = "wire"
model = "gpt-5"
"#,
    )?;

    let env_config = repo.path().join("env-config.toml");
    fs::write(
        &env_config,
        r#"
[agents.default]
backend = "codex"
"#,
    )?;

    let before_logs = gather_session_logs(&repo)?;
    let isolated_config = TempDir::new()?;
    let mut cmd = repo.vizier_cmd();
    cmd.env("VIZIER_CONFIG_FILE", env_config.as_os_str());
    cmd.env("VIZIER_CONFIG_DIR", isolated_config.path());
    cmd.env("XDG_CONFIG_HOME", isolated_config.path());
    cmd.args(["ask", "repo config should win over env"]);
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "vizier ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after_logs = gather_session_logs(&repo)?;
    let new_log = new_session_log(&before_logs, &after_logs)
        .ok_or_else(|| "expected vizier ask to produce a new session log")?;
    let contents = fs::read_to_string(new_log)?;
    let json: Value = serde_json::from_str(&contents)?;
    assert_eq!(
        json.get("model")
            .and_then(|model| model.get("provider"))
            .and_then(Value::as_str),
        Some("wire"),
        "repo config should force ask onto the wire backend despite env overrides"
    );
    Ok(())
}

#[test]
fn test_env_config_used_when_repo_config_missing() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let repo_toml = repo.path().join(".vizier").join("config.toml");
    if repo_toml.exists() {
        fs::remove_file(&repo_toml)?;
    }
    let repo_json = repo.path().join(".vizier").join("config.json");
    if repo_json.exists() {
        fs::remove_file(&repo_json)?;
    }

    let env_config = repo.path().join("env-config.toml");
    fs::write(
        &env_config,
        r#"
[agents.default]
backend = "wire"
"#,
    )?;

    let before_logs = gather_session_logs(&repo)?;
    let isolated_config = TempDir::new()?;
    let mut cmd = repo.vizier_cmd();
    cmd.env("VIZIER_CONFIG_FILE", env_config.as_os_str());
    cmd.env("VIZIER_CONFIG_DIR", isolated_config.path());
    cmd.env("XDG_CONFIG_HOME", isolated_config.path());
    cmd.args(["ask", "env config selection"]);
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "vizier ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after_logs = gather_session_logs(&repo)?;
    let new_log = new_session_log(&before_logs, &after_logs)
        .ok_or_else(|| "expected vizier ask to create a session log")?;
    let contents = fs::read_to_string(new_log)?;
    let json: Value = serde_json::from_str(&contents)?;
    assert_eq!(
        json.get("model")
            .and_then(|model| model.get("provider"))
            .and_then(Value::as_str),
        Some("wire"),
        "env config should take effect when no repo config exists"
    );
    Ok(())
}

#[test]
fn test_ask_reports_token_usage_progress() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo.vizier_output(&["ask", "token usage integration smoke"])?;
    assert!(
        output.status.success(),
        "vizier ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[usage] token-usage"),
        "expected usage progress line, stderr was:\n{}",
        stderr
    );
    assert!(
        stderr.to_ascii_lowercase().contains("input") && stderr.contains("cached"),
        "usage block should include cached input counts:\n{}",
        stderr
    );
    assert!(
        stderr.to_ascii_lowercase().contains("output")
            && stderr.to_ascii_lowercase().contains("reasoning"),
        "usage block should include reasoning output counts:\n{}",
        stderr
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Token usage:"),
        "ask stdout should include token usage block:\n{}",
        stdout
    );
    assert!(
        stdout.contains("Total"),
        "token usage block should include totals:\n{}",
        stdout
    );

    let quiet_repo = IntegrationRepo::new()?;
    let quiet = quiet_repo.vizier_output(&["-q", "ask", "quiet usage check"])?;
    assert!(
        quiet.status.success(),
        "quiet vizier ask failed: {}",
        String::from_utf8_lossy(&quiet.stderr)
    );
    let quiet_stderr = String::from_utf8_lossy(&quiet.stderr);
    assert!(
        !quiet_stderr.contains("[usage] token-usage"),
        "quiet mode should suppress usage events but printed:\n{}",
        quiet_stderr
    );
    let quiet_stdout = String::from_utf8_lossy(&quiet.stdout);
    assert!(
        !quiet_stdout.contains("Token usage:"),
        "quiet mode should suppress token usage block on stdout but printed:\n{}",
        quiet_stdout
    );
    Ok(())
}

#[test]
fn test_session_log_captures_token_usage_totals() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo.vizier_cmd().arg("save").output()?;
    assert!(
        output.status.success(),
        "vizier save failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let usage_lines = usage_lines(&stderr);
    assert!(
        !usage_lines.is_empty(),
        "stderr missing usage lines:\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let contents = session_log_contents_from_output(&repo, &stdout)?;
    let session_usage = parse_session_usage(&contents)?;

    let fmt = format_number;
    assert!(
        usage_lines.iter().any(|line| line
            .replace(' ', "")
            .contains(&format!("Total:{}", fmt(session_usage.total)))),
        "CLI usage should report total tokens:\n{stderr}"
    );
    if session_usage.total_delta > 0 {
        assert!(
            usage_lines
                .iter()
                .any(|line| line.contains(&format!("(+{})", fmt(session_usage.total_delta)))),
            "CLI usage should report total deltas:\n{stderr}"
        );
    }
    assert!(
        usage_lines.iter().any(|line| line
            .replace(' ', "")
            .contains(&format!("Input:{}", fmt(session_usage.prompt_total)))),
        "CLI usage should report prompt tokens:\n{stderr}"
    );
    if session_usage.prompt_delta > 0 {
        assert!(
            usage_lines
                .iter()
                .any(|line| line.contains(&format!("(+{})", fmt(session_usage.prompt_delta)))),
            "CLI usage should report prompt deltas:\n{stderr}"
        );
    }
    assert!(
        usage_lines.iter().any(|line| line
            .replace(' ', "")
            .contains(&format!("Output:{}", fmt(session_usage.completion_total)))),
        "CLI usage should report completion tokens:\n{stderr}"
    );
    if session_usage.completion_delta > 0 {
        assert!(
            usage_lines
                .iter()
                .any(|line| line.contains(&format!("(+{})", fmt(session_usage.completion_delta)))),
            "CLI usage should report completion deltas:\n{stderr}"
        );
    }
    if session_usage.cached_input_total > 0 || session_usage.cached_input_delta > 0 {
        assert!(
            usage_lines.iter().any(|line| line.contains("cached")),
            "CLI usage should mention cached input when present:\n{stderr}"
        );
    }
    if session_usage.reasoning_output_total > 0 || session_usage.reasoning_output_delta > 0 {
        assert!(
            usage_lines
                .iter()
                .any(|line| line.to_ascii_lowercase().contains("reasoning")),
            "CLI usage should mention reasoning output when present:\n{stderr}"
        );
    }
    Ok(())
}

#[test]
fn test_session_log_handles_unknown_token_usage() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = gather_session_logs(&repo)?;

    let mut cmd = repo.vizier_cmd();
    cmd.args(["ask", "suppress usage event"]);
    cmd.env("VIZIER_SUPPRESS_TOKEN_USAGE", "1");
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "vizier ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let usage_lines = usage_lines(&stderr);
    assert!(
        usage_lines
            .iter()
            .any(|line| line.to_ascii_lowercase().contains("unknown")),
        "usage lines should note unknown counts but were:\n{}",
        stderr
    );

    let after = gather_session_logs(&repo)?;
    let session_path = new_session_log(&before, &after)
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "missing new session log"))?
        .clone();
    let contents = fs::read_to_string(&session_path)?;
    let usage = parse_session_usage(&contents)?;
    assert!(
        !usage.known,
        "session log should mark token usage unknown when backend omits counts"
    );
    assert_eq!(usage.prompt_total, 0);
    assert_eq!(usage.completion_total, 0);
    assert_eq!(usage.total, 0);
    assert_eq!(usage.cached_input_total, 0);
    assert_eq!(usage.cached_input_delta, 0);
    assert_eq!(usage.reasoning_output_total, 0);
    assert_eq!(usage.reasoning_output_delta, 0);
    Ok(())
}

#[test]
fn test_draft_reports_token_usage() -> TestResult {
    fn parse_usage_total(summary: &str) -> Option<usize> {
        summary
            .lines()
            .find(|line| line.trim_start().starts_with("Total"))
            .and_then(|line| line.split(':').nth(1))
            .map(str::trim)
            .and_then(|value| value.split_whitespace().next())
            .map(|value| value.replace(',', ""))
            .and_then(|value| value.parse::<usize>().ok())
    }

    let repo = IntegrationRepo::new()?;
    let sessions_root = repo.path().join(".vizier/sessions");
    if sessions_root.exists() {
        fs::remove_dir_all(&sessions_root)?;
    }
    let before_logs = gather_session_logs(&repo)?;

    let output = repo.vizier_output(&[
        "draft",
        "--name",
        "token-usage",
        "capture usage for draft plans",
    ])?;
    assert!(
        output.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let summary = stdout
        .split("\n\n")
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "missing draft summary block"))?;
    let stdout_total = parse_usage_total(summary).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Other,
            format!("usage total missing from summary:\n{summary}"),
        )
    })?;
    assert!(
        stdout_total > 0,
        "draft stdout should report non-zero token usage but was {stdout_total}:\n{summary}"
    );

    let after_logs = gather_session_logs(&repo)?;
    let session_path = new_session_log(&before_logs, &after_logs)
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "expected session log for draft"))?;
    let contents = fs::read_to_string(&session_path)?;
    let session_usage = parse_session_usage(&contents)?;
    assert!(
        session_usage.total > 0,
        "session log should record non-zero token usage but was {}",
        session_usage.total
    );
    assert!(
        session_usage.known,
        "session log should mark token usage as known"
    );
    assert_eq!(
        session_usage.total, stdout_total,
        "stdout total should match session log total (log: {}, stdout: {})",
        session_usage.total, stdout_total
    );

    Ok(())
}

#[test]
fn test_draft_creates_branch_and_plan() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;
    let before_logs = gather_session_logs(&repo)?;

    let output = repo.vizier_output(&["draft", "--name", "smoke", "ship the draft flow"])?;
    assert!(
        output.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after_logs = gather_session_logs(&repo)?;
    let session_log = new_session_log(&before_logs, &after_logs)
        .ok_or_else(|| "expected vizier draft to create a session log")?;
    assert!(
        session_log.exists(),
        "session log should exist at {}",
        session_log.display()
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
    assert!(contents.contains("ship the draft flow"));
    assert!(contents.contains("## Implementation Plan"));
    assert!(
        contents.contains("plan: smoke"),
        "plan front matter should include slug"
    );
    assert!(
        contents.contains("branch: draft/smoke"),
        "plan front matter should include branch"
    );
    assert!(
        !contents.contains("status:"),
        "plan metadata should omit status fields"
    );

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

    let list_before = repo.vizier_output(&["list"])?;
    assert!(
        list_before.status.success(),
        "vizier list failed: {}",
        String::from_utf8_lossy(&list_before.stderr)
    );
    let stdout_before = String::from_utf8_lossy(&list_before.stdout);
    assert!(
        stdout_before.contains("Plan   : approve-smoke"),
        "pending plans missing approve-smoke: {}",
        stdout_before
    );
    assert!(
        stdout_before.contains("Branch : draft/approve-smoke"),
        "pending plans missing branch detail: {}",
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
    let approve_stderr = String::from_utf8_lossy(&approve.stderr);
    assert!(
        approve_stderr.contains("[codex] apply plan"),
        "Agent progress log missing expected line: {}",
        approve_stderr
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
fn test_list_outputs_prettified_blocks() -> TestResult {
    let repo = IntegrationRepo::new()?;

    let empty = repo.vizier_output(&["list"])?;
    assert!(
        empty.status.success(),
        "vizier list (empty) failed: {}",
        String::from_utf8_lossy(&empty.stderr)
    );
    let empty_stdout = String::from_utf8_lossy(&empty.stdout);
    assert!(
        empty_stdout.contains("Outcome: No pending draft branches"),
        "empty list output missing outcome: {empty_stdout}"
    );

    let draft_alpha = repo.vizier_output(&["draft", "--name", "alpha", "Alpha spec line"])?;
    assert!(
        draft_alpha.status.success(),
        "vizier draft alpha failed: {}",
        String::from_utf8_lossy(&draft_alpha.stderr)
    );
    let draft_beta = repo.vizier_output(&["draft", "--name", "beta", "Beta spec line"])?;
    assert!(
        draft_beta.status.success(),
        "vizier draft beta failed: {}",
        String::from_utf8_lossy(&draft_beta.stderr)
    );

    let list = repo.vizier_output(&["list"])?;
    assert!(
        list.status.success(),
        "vizier list failed: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(
        stdout.contains("Outcome: 2 pending draft branches"),
        "list header missing pending count: {stdout}"
    );
    assert!(
        stdout.contains("\n\n  Plan   : beta"),
        "list output should separate entries with whitespace: {stdout}"
    );
    for (slug, summary) in [("alpha", "Alpha spec line"), ("beta", "Beta spec line")] {
        assert!(
            stdout.contains(&format!("  Plan   : {slug}")),
            "list output missing plan {slug}: {stdout}"
        );
        assert!(
            stdout.contains(&format!("  Branch : draft/{slug}")),
            "list output missing branch for {slug}: {stdout}"
        );
        assert!(
            stdout.contains(&format!("  Summary: {summary}")),
            "list output missing summary for {slug}: {stdout}"
        );
    }

    Ok(())
}

#[test]
fn test_approve_creates_single_combined_commit() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "single-commit-approve", "spec"])?;

    let repo_handle = repo.repo();
    let draft_branch = repo_handle.find_branch("draft/single-commit-approve", BranchType::Local)?;
    let before_commit = draft_branch.get().peel_to_commit()?.id();

    clean_workdir(&repo)?;
    let approve = repo.vizier_output(&["approve", "single-commit-approve", "--yes"])?;
    assert!(
        approve.status.success(),
        "vizier approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );

    let repo_handle = repo.repo();
    let branch = repo_handle.find_branch("draft/single-commit-approve", BranchType::Local)?;
    let commit = branch.get().peel_to_commit()?;
    assert_eq!(
        commit.parent(0)?.id(),
        before_commit,
        "approve should add exactly one commit"
    );

    let files = files_changed_in_commit(&repo_handle, &commit.id().to_string())?;
    assert!(
        files.contains(".vizier/.snapshot") && files.contains("a"),
        "approve commit should include code and narrative assets, got {files:?}"
    );
    assert!(
        !files
            .iter()
            .any(|path| path.contains("implementation-plans")),
        "plan documents should remain scratch, got {files:?}"
    );

    Ok(())
}

#[test]
fn test_cli_backend_override_rejected_for_approve() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let config_path = repo.path().join("agents.toml");
    fs::write(
        &config_path,
        r#"
[agents.default]
backend = "codex"
"#,
    )?;

    let draft = repo
        .vizier_cmd_with_config(&config_path)
        .args(["draft", "--name", "agent-scope", "scope smoke test"])
        .output()?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    let approve = repo
        .vizier_cmd_with_config(&config_path)
        .args(["--backend", "wire", "approve", "agent-scope", "--yes"])
        .output()?;
    assert!(
        !approve.status.success(),
        "approve unexpectedly succeeded with --backend wire"
    );
    let stderr = String::from_utf8_lossy(&approve.stderr);
    assert!(
        stderr.contains("approve requires an agent-style backend"),
        "stderr missing backend warning: {}",
        stderr
    );

    Ok(())
}

#[test]
fn test_draft_fails_when_codex_errors() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let mut cmd = repo.vizier_cmd();
    cmd.env("VIZIER_FORCE_AGENT_ERROR", "1");
    cmd.args(["draft", "--name", "codex-failure", "force failure"]);
    let output = cmd.output()?;
    assert!(
        !output.status.success(),
        "vizier draft should fail when the backend errors"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("agent backend"),
        "stderr should mention backend failure, got: {stderr}"
    );
    assert!(
        !stderr.to_ascii_lowercase().contains("wire backend"),
        "stderr hinted at a wire fallback: {stderr}"
    );
    let plan_path = repo
        .path()
        .join(".vizier/implementation-plans/codex-failure.md");
    assert!(
        !plan_path.exists(),
        "failed draft should not leave a partially written plan"
    );
    Ok(())
}

#[test]
fn test_approve_fails_when_codex_errors() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let draft = repo
        .vizier_cmd()
        .args(["draft", "--name", "codex-approve", "spec"])
        .output()?;
    assert!(
        draft.status.success(),
        "vizier draft failed unexpectedly: {}",
        String::from_utf8_lossy(&draft.stderr)
    );
    let repo_handle = repo.repo();
    let before_commit = repo_handle
        .find_branch("draft/codex-approve", BranchType::Local)?
        .get()
        .peel_to_commit()?;

    let mut approve = repo.vizier_cmd();
    approve.env("VIZIER_FORCE_AGENT_ERROR", "1");
    approve.args(["approve", "codex-approve", "--yes"]);
    let output = approve.output()?;
    assert!(
        !output.status.success(),
        "vizier approve should fail when the backend errors"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("agent backend"),
        "stderr should mention backend error, got: {stderr}"
    );
    assert!(
        !stderr.to_ascii_lowercase().contains("wire backend"),
        "stderr hinted at a wire fallback: {stderr}"
    );

    let repo_handle = repo.repo();
    let after_commit = repo_handle
        .find_branch("draft/codex-approve", BranchType::Local)?
        .get()
        .peel_to_commit()?;
    assert_eq!(
        before_commit.id(),
        after_commit.id(),
        "backend failure should not add commits to the plan branch"
    );
    Ok(())
}

#[test]
fn test_merge_auto_resolve_fails_when_codex_errors() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let draft = repo
        .vizier_cmd()
        .args(["draft", "--name", "codex-merge", "merge failure testcase"])
        .output()?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );
    let approve = repo
        .vizier_cmd()
        .args(["approve", "codex-merge", "--yes"])
        .output()?;
    assert!(
        approve.status.success(),
        "vizier approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );

    repo.write("a", "master conflicting change")?;
    repo.git(&["add", "a"])?;
    repo.git(&["commit", "-m", "master conflicting change"])?;

    let mut merge = repo.vizier_cmd();
    merge.env("VIZIER_FORCE_AGENT_ERROR", "1");
    merge.args(["merge", "codex-merge", "--yes", "--auto-resolve-conflicts"]);
    let output = merge.output()?;
    assert!(
        !output.status.success(),
        "merge should fail when backend auto-resolution errors"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Backend auto-resolution failed")
            || stderr.contains("forced mock agent failure")
            || stderr.contains("agent backend exited"),
        "stderr should mention backend failure, got: {stderr}"
    );
    assert!(
        !stderr.to_ascii_lowercase().contains("wire backend"),
        "stderr hinted at a wire fallback: {stderr}"
    );

    repo.repo()
        .find_branch("draft/codex-merge", BranchType::Local)
        .expect("plan branch should remain after failure");
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
fn test_merge_default_squash_adds_implementation_commit() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "squash-default", "squash smoke"])?;
    repo.vizier_output(&["approve", "squash-default", "--yes"])?;
    clean_workdir(&repo)?;

    let repo_handle = repo.repo();
    let base_commit = repo_handle.head()?.peel_to_commit()?.id();
    let source_tip = repo_handle
        .find_branch("draft/squash-default", BranchType::Local)?
        .get()
        .peel_to_commit()?
        .id();

    let merge = repo.vizier_output(&["merge", "squash-default", "--yes"])?;
    assert!(
        merge.status.success(),
        "vizier merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );

    let repo_handle = repo.repo();
    let head = repo_handle.head()?.peel_to_commit()?;
    assert_eq!(
        head.parent_count(),
        1,
        "squashed merge should produce a single-parent merge commit"
    );
    let implementation_commit = head.parent(0)?;
    assert_eq!(
        implementation_commit.parent_count(),
        1,
        "implementation commit should have a single parent"
    );
    assert_eq!(
        implementation_commit.parent(0)?.id(),
        base_commit,
        "implementation commit should descend from the previous master head"
    );
    assert!(
        !repo_handle.graph_descendant_of(head.id(), source_tip)?,
        "squashed merge should sever ancestry to the draft branch"
    );
    assert!(
        repo_handle
            .find_branch("draft/squash-default", BranchType::Local)
            .is_err(),
        "default squashed merge should delete the draft branch"
    );
    Ok(())
}

#[test]
fn test_merge_squash_replays_plan_history() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "squash-replay", "replay squash plan"])?;

    repo.git(&["checkout", "draft/squash-replay"])?;
    repo.write("a", "first replay change\n")?;
    repo.git(&["commit", "-am", "first replay change"])?;
    repo.write("a", "second replay change\n")?;
    repo.git(&["commit", "-am", "second replay change"])?;

    let repo_handle = repo.repo();

    repo.git(&["checkout", "master"])?;
    clean_workdir(&repo)?;
    let plan_tip = repo_handle
        .find_branch("draft/squash-replay", BranchType::Local)?
        .get()
        .peel_to_commit()?
        .id();
    let base_commit = repo_handle.head()?.peel_to_commit()?.id();

    let merge = repo.vizier_output(&["merge", "squash-replay", "--yes"])?;
    assert!(
        merge.status.success(),
        "vizier merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );

    let merge_commit = repo_handle.head()?.peel_to_commit()?;
    assert_eq!(
        merge_commit.parent_count(),
        1,
        "squashed merge should keep only the implementation commit as its parent"
    );
    let implementation_commit = merge_commit.parent(0)?;
    assert_eq!(
        implementation_commit.parent(0)?.id(),
        base_commit,
        "implementation commit should descend from the previous master head"
    );
    assert!(
        !repo_handle.graph_descendant_of(merge_commit.id(), plan_tip)?,
        "squashed merge should not keep the draft branch in the ancestry graph"
    );
    let contents = repo.read("a")?;
    assert!(
        contents.starts_with("second replay change\n"),
        "squashed merge should apply the plan branch edits to the target"
    );
    Ok(())
}

#[test]
fn test_merge_no_squash_matches_legacy_parentage() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "legacy-merge", "legacy merge spec"])?;
    repo.vizier_output(&["approve", "legacy-merge", "--yes"])?;
    clean_workdir(&repo)?;

    let repo_handle = repo.repo();
    let base_commit = repo_handle.head()?.peel_to_commit()?.id();

    let merge = repo.vizier_output(&["merge", "legacy-merge", "--yes", "--no-squash"])?;
    assert!(
        merge.status.success(),
        "vizier merge --no-squash failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );

    let repo_handle = repo.repo();
    let head = repo_handle.head()?.peel_to_commit()?;
    assert_eq!(
        head.parent(0)?.id(),
        base_commit,
        "legacy merge should point directly to the previous master head"
    );
    Ok(())
}

#[test]
fn test_merge_squash_allows_zero_diff_range() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "zero-diff", "plan with no code changes"])?;
    clean_workdir(&repo)?;

    let repo_handle = repo.repo();
    let base_commit = repo_handle.head()?.peel_to_commit()?;
    let source_tip = repo_handle
        .find_branch("draft/zero-diff", BranchType::Local)?
        .get()
        .peel_to_commit()?
        .id();

    let merge = repo.vizier_output(&["merge", "zero-diff", "--yes"])?;
    assert!(
        merge.status.success(),
        "vizier merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );

    let head = repo_handle.head()?.peel_to_commit()?;
    assert_eq!(
        head.parent_count(),
        1,
        "squashed merge should keep only the implementation commit as its parent"
    );
    let implementation_commit = head.parent(0)?;
    assert_eq!(
        implementation_commit.parent(0)?.id(),
        base_commit.id(),
        "implementation commit should still descend from the previous master head"
    );
    assert!(
        !repo_handle.graph_descendant_of(head.id(), source_tip)?,
        "squashed merge should not retain the draft branch ancestry"
    );
    Ok(())
}

#[test]
fn test_merge_squash_replay_respects_manual_resolution_before_finishing_range() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "replay-conflict", "replay conflict plan"])?;

    repo.git(&["checkout", "draft/replay-conflict"])?;
    repo.write("a", "plan step one\n")?;
    repo.git(&["commit", "-am", "plan step one"])?;
    repo.write("a", "plan step two\n")?;
    repo.git(&["commit", "-am", "plan step two"])?;

    let plan_tip = repo
        .repo()
        .find_branch("draft/replay-conflict", BranchType::Local)?
        .get()
        .peel_to_commit()?
        .id();

    repo.git(&["checkout", "master"])?;
    clean_workdir(&repo)?;
    repo.write("a", "master diverges\n")?;
    repo.git(&["commit", "-am", "master divergence"])?;
    let base_commit = repo.repo().head()?.peel_to_commit()?.id();

    let merge = repo.vizier_output(&["merge", "replay-conflict", "--yes"])?;
    assert!(
        !merge.status.success(),
        "expected merge to surface cherry-pick conflict, got:\n{}",
        String::from_utf8_lossy(&merge.stderr)
    );

    let sentinel = repo
        .path()
        .join(".vizier/tmp/merge-conflicts/replay-conflict.json");
    assert!(
        sentinel.exists(),
        "merge conflict sentinel missing after initial failure"
    );

    repo.write("a", "manual resolution wins\n")?;
    repo.git(&["add", "a"])?;

    let resume =
        repo.vizier_output(&["merge", "replay-conflict", "--yes", "--complete-conflict"])?;
    assert!(
        resume.status.success(),
        "vizier merge --complete-conflict failed after manual resolution: {}",
        String::from_utf8_lossy(&resume.stderr)
    );
    assert!(
        !sentinel.exists(),
        "sentinel should be removed after --complete-conflict succeeds"
    );

    let contents = repo.read("a")?;
    assert_eq!(
        contents, "manual resolution wins\n",
        "manual resolution should survive replaying the remaining plan commits"
    );

    let repo_handle = repo.repo();
    let head = repo_handle.head()?.peel_to_commit()?;
    assert_eq!(
        head.parent_count(),
        1,
        "squashed merge should keep only the implementation commit as its parent after replay"
    );
    let implementation_commit = head.parent(0)?;
    assert_eq!(
        implementation_commit.parent(0)?.id(),
        base_commit,
        "implementation commit should descend from the pre-merge target head"
    );
    assert!(
        !repo_handle.graph_descendant_of(head.id(), plan_tip)?,
        "squashed merge should not retain draft branch ancestry after manual conflict resolution"
    );
    Ok(())
}

fn prepare_plan_branch_with_merge_history(repo: &IntegrationRepo, slug: &str) -> TestResult {
    let plan_branch = format!("draft/{slug}");
    let side_branch = format!("{slug}-side");

    repo.vizier_output(&[
        "draft",
        "--name",
        slug,
        "plan branch includes merge history",
    ])?;
    repo.git(&["checkout", &plan_branch])?;
    repo.write("a", "main path change\n")?;
    repo.git(&["commit", "-am", "main path change"])?;

    repo.git(&["checkout", "HEAD^", "-b", &side_branch])?;
    repo.write("b", "side path change\n")?;
    repo.git(&["commit", "-am", "side path change"])?;

    repo.git(&["checkout", &plan_branch])?;
    repo.git(&["merge", &side_branch])?;

    repo.git(&["checkout", "master"])?;
    clean_workdir(repo)?;
    Ok(())
}

#[test]
fn test_merge_squash_requires_mainline_for_merge_history() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_plan_branch_with_merge_history(&repo, "replay-merge-history")?;

    let merge = repo.vizier_output(&["merge", "replay-merge-history", "--yes"])?;
    assert!(
        !merge.status.success(),
        "expected merge to fail on plan branch with merge commits; got success"
    );
    let stderr = String::from_utf8_lossy(&merge.stderr);
    assert!(
        stderr.contains("--squash-mainline") && stderr.contains("merge commits"),
        "merge failure should request --squash-mainline when merge commits exist; stderr:\n{stderr}"
    );

    repo.git(&["reset", "--hard"])?;
    Ok(())
}

#[test]
fn test_merge_squash_mainline_replays_merge_history() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_plan_branch_with_merge_history(&repo, "replay-merge-history-mainline")?;

    let merge = repo.vizier_output(&[
        "merge",
        "replay-merge-history-mainline",
        "--yes",
        "--squash-mainline",
        "1",
    ])?;
    assert!(
        merge.status.success(),
        "expected merge to succeed when squash mainline is provided: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    assert!(
        repo.read("a")?.contains("main path change"),
        "target branch should include main path change after merge"
    );
    assert!(
        repo.read("b")?.contains("side path change"),
        "target branch should include side path change after merge"
    );
    Ok(())
}

#[test]
fn test_merge_no_squash_handles_merge_history() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_plan_branch_with_merge_history(&repo, "replay-merge-history-no-squash")?;

    let merge = repo.vizier_output(&[
        "merge",
        "replay-merge-history-no-squash",
        "--yes",
        "--no-squash",
    ])?;
    assert!(
        merge.status.success(),
        "expected --no-squash merge to succeed even when plan history contains merges: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    assert!(
        repo.read("a")?.contains("main path change"),
        "target branch should include main path change after legacy merge"
    );
    assert!(
        repo.read("b")?.contains("side path change"),
        "target branch should include side path change after legacy merge"
    );
    Ok(())
}

#[test]
fn test_merge_squash_rejects_octopus_merge_history() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "octopus", "octopus merge history"])?;
    let plan_branch = "draft/octopus".to_string();
    let side_one = "octopus-side-1".to_string();
    let side_two = "octopus-side-2".to_string();

    repo.git(&["checkout", &plan_branch])?;
    repo.write("a", "base change\n")?;
    repo.git(&["commit", "-am", "base change"])?;
    let base_oid = oid_for_spec(&repo.repo(), "HEAD")?.to_string();

    repo.git(&["checkout", "-b", &side_one])?;
    repo.write("b", "side one\n")?;
    repo.git(&["commit", "-am", "side one change"])?;

    repo.git(&["checkout", "-b", &side_two, &base_oid])?;
    repo.write("c", "side two\n")?;
    repo.git(&["commit", "-am", "side two change"])?;

    repo.git(&["checkout", &plan_branch])?;
    repo.git(&["merge", &side_one, &side_two])?;
    repo.git(&["checkout", "master"])?;
    clean_workdir(&repo)?;

    let merge = repo.vizier_output(&["merge", "octopus", "--yes"])?;
    assert!(
        !merge.status.success(),
        "expected squash merge to abort on octopus history"
    );
    let stderr = String::from_utf8_lossy(&merge.stderr);
    assert!(
        stderr.contains("octopus") && stderr.contains("--no-squash"),
        "stderr should explain octopus history and suggest --no-squash: {stderr}"
    );

    Ok(())
}

#[test]
fn test_merge_cicd_gate_executes_script() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "cicd-pass", "cicd gate spec"])?;
    repo.vizier_output(&["approve", "cicd-pass", "--yes"])?;
    clean_workdir(&repo)?;

    let script_path = write_cicd_script(
        &repo,
        "gate-pass.sh",
        "#!/bin/sh\nset -eu\nprintf \"gate ok\" > cicd-pass.log\n",
    )?;

    let script_flag = script_path.to_string_lossy().to_string();
    let merge =
        repo.vizier_output(&["merge", "cicd-pass", "--yes", "--cicd-script", &script_flag])?;
    assert!(
        merge.status.success(),
        "vizier merge failed with CI/CD script: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let log = repo.read("cicd-pass.log")?;
    assert!(
        log.contains("gate ok"),
        "CI/CD script output missing expected line: {log}"
    );
    Ok(())
}

#[test]
fn test_merge_cicd_gate_failure_blocks_merge() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "cicd-fail", "cicd fail spec"])?;
    repo.vizier_output(&["approve", "cicd-fail", "--yes"])?;
    clean_workdir(&repo)?;

    let script_path = write_cicd_script(
        &repo,
        "gate-fail.sh",
        "#!/bin/sh\necho \"gate failure\" >&2\nexit 1\n",
    )?;
    let script_flag = script_path.to_string_lossy().to_string();
    let merge =
        repo.vizier_output(&["merge", "cicd-fail", "--yes", "--cicd-script", &script_flag])?;
    assert!(
        !merge.status.success(),
        "merge should fail when CI/CD gate exits non-zero"
    );
    let stderr = String::from_utf8_lossy(&merge.stderr);
    assert!(
        stderr.contains("CI/CD gate"),
        "stderr should mention CI/CD gate failure: {stderr}"
    );
    assert!(
        stderr.contains("gate failure"),
        "stderr should include script output: {stderr}"
    );
    let repo_handle = repo.repo();
    assert!(
        repo_handle
            .find_branch("draft/cicd-fail", BranchType::Local)
            .is_ok(),
        "draft branch should remain after CI/CD failure"
    );
    Ok(())
}

#[test]
fn test_merge_cicd_gate_auto_fix_applies_changes() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "cicd-auto", "auto ci gate spec"])?;
    repo.vizier_output(&["approve", "cicd-auto", "--yes"])?;
    clean_workdir(&repo)?;

    repo.write(".vizier/tmp/mock_cicd_fix_path", "ci/fixed.txt\n")?;
    let script_path = write_cicd_script(
        &repo,
        "gate-auto.sh",
        "#!/bin/sh\nif [ -f \"ci/fixed.txt\" ]; then\n  exit 0\nfi\necho \"ci gate still failing\" >&2\nexit 1\n",
    )?;
    let script_flag = script_path.to_string_lossy().to_string();
    let merge = repo.vizier_output(&[
        "merge",
        "cicd-auto",
        "--yes",
        "--cicd-script",
        &script_flag,
        "--auto-cicd-fix",
        "--cicd-retries",
        "2",
    ])?;
    assert!(
        merge.status.success(),
        "merge with auto CI/CD remediation should succeed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    assert!(
        repo.path().join("ci/fixed.txt").exists(),
        "auto remediation should create the expected fix file"
    );
    let stdout = String::from_utf8_lossy(&merge.stdout);
    assert!(
        stdout.contains("Gate fixes") && stdout.contains("amend:"),
        "merge summary should report the amended implementation commit: {stdout}"
    );
    Ok(())
}

#[test]
fn test_review_streams_critique() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let draft = repo.vizier_output(&["draft", "--name", "review-smoke", "review smoke spec"])?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    clean_workdir(&repo)?;

    let approve = repo.vizier_output(&["approve", "review-smoke", "--yes"])?;
    assert!(
        approve.status.success(),
        "vizier approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );

    clean_workdir(&repo)?;
    let repo_handle = repo.repo();
    let branch_before = repo_handle.find_branch("draft/review-smoke", BranchType::Local)?;
    let before_commit = branch_before.get().peel_to_commit()?.id();

    let review =
        repo.vizier_output(&["review", "review-smoke", "--review-only", "--skip-checks"])?;
    assert!(
        review.status.success(),
        "vizier review failed: {}",
        String::from_utf8_lossy(&review.stderr)
    );

    let stdout = String::from_utf8_lossy(&review.stdout);
    assert!(
        stdout.contains("--- Review critique for plan review-smoke ---"),
        "review output should stream the critique header but was:\n{}",
        stdout
    );

    let branch = repo_handle.find_branch("draft/review-smoke", BranchType::Local)?;
    let commit = branch.get().peel_to_commit()?;
    assert_eq!(
        commit.parent(0)?.id(),
        before_commit,
        "review should add exactly one commit"
    );
    let tree = commit.tree()?;
    assert!(
        tree.get_path(Path::new(".vizier/reviews/review-smoke.md"))
            .is_err(),
        "review artifacts should not be committed to the plan branch"
    );

    assert!(
        !repo.path().join(".vizier/reviews/review-smoke.md").exists(),
        "review directory should not exist after streaming critiques"
    );

    assert!(
        !repo
            .path()
            .join(".vizier/implementation-plans/review-smoke.md")
            .exists(),
        "plan document should remain confined to the draft branch"
    );

    let files = files_changed_in_commit(&repo_handle, &commit.id().to_string())?;
    assert!(
        files.contains(".vizier/.snapshot"),
        "critique commit should include narrative assets, got {files:?}"
    );
    assert!(
        !files
            .iter()
            .any(|path| path.contains("implementation-plans")),
        "plan documents should remain scratch, got {files:?}"
    );

    Ok(())
}

#[test]
fn test_review_summary_includes_token_suffix() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let draft = repo.vizier_output(&["draft", "--name", "token-suffix", "suffix spec"])?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    clean_workdir(&repo)?;
    let approve = repo.vizier_output(&["approve", "token-suffix", "--yes"])?;
    assert!(
        approve.status.success(),
        "vizier approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );

    clean_workdir(&repo)?;
    let review =
        repo.vizier_output(&["review", "token-suffix", "--review-only", "--skip-checks"])?;
    assert!(
        review.status.success(),
        "vizier review failed: {}",
        String::from_utf8_lossy(&review.stderr)
    );

    let stdout = String::from_utf8_lossy(&review.stdout);
    assert!(
        stdout.contains("Total") && stdout.contains("Input") && stdout.contains("Output"),
        "review summary should include token usage block but was:\n{stdout}"
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

#[test]
fn test_merge_conflict_complete_flag() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_conflicting_plan(
        &repo,
        "conflict-complete",
        "master branch keeps its version\n",
        "plan branch prefers this text\n",
    )?;
    clean_workdir(&repo)?;

    let merge = repo.vizier_output(&["merge", "conflict-complete", "--yes"])?;
    assert!(
        !merge.status.success(),
        "expected merge to fail on conflicts"
    );

    repo.write("a", "manual resolution wins\n")?;
    repo.git(&["add", "a"])?;
    let status = Command::new("git")
        .args(["-C", repo.path().to_str().unwrap(), "status", "--porcelain"])
        .output()?;
    let status_out = String::from_utf8_lossy(&status.stdout);
    println!("status before resume:\n{status_out}");
    let idx_conflicts = repo.repo().index()?.has_conflicts();
    println!("index.has_conflicts before resume: {idx_conflicts}");
    let conflicts = Command::new("git")
        .args(["-C", repo.path().to_str().unwrap(), "ls-files", "-u"])
        .output()?;
    println!(
        "ls-files -u before resume:\n{}",
        String::from_utf8_lossy(&conflicts.stdout)
    );
    assert!(
        !status_out.contains("U "),
        "expected conflicts to be resolved before --complete-conflict, got:\n{status_out}"
    );

    let resume =
        repo.vizier_output(&["merge", "conflict-complete", "--yes", "--complete-conflict"])?;
    println!(
        "resume stderr:\n{}",
        String::from_utf8_lossy(&resume.stderr)
    );
    assert!(
        resume.status.success(),
        "vizier merge --complete-conflict failed after manual resolution: {}",
        String::from_utf8_lossy(&resume.stderr)
    );

    let sentinel = repo
        .path()
        .join(".vizier/tmp/merge-conflicts/conflict-complete.json");
    assert!(
        !sentinel.exists(),
        "sentinel should be removed after --complete-conflict succeeds"
    );
    Ok(())
}

#[test]
fn test_merge_conflict_complete_blocks_wrong_branch() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_conflicting_plan(
        &repo,
        "conflict-wrong-branch",
        "master branch keeps its version\n",
        "plan branch prefers this text\n",
    )?;
    clean_workdir(&repo)?;

    let merge = repo.vizier_output(&["merge", "conflict-wrong-branch", "--yes"])?;
    assert!(
        !merge.status.success(),
        "expected merge to fail on conflicts"
    );

    let sentinel = repo
        .path()
        .join(".vizier/tmp/merge-conflicts/conflict-wrong-branch.json");
    assert!(
        sentinel.exists(),
        "conflict sentinel missing after initial failure"
    );

    repo.git(&["cherry-pick", "--abort"])?;
    repo.git(&["checkout", "-b", "elsewhere"])?;

    let resume = repo.vizier_output(&[
        "merge",
        "conflict-wrong-branch",
        "--yes",
        "--complete-conflict",
    ])?;
    assert!(
        !resume.status.success(),
        "expected --complete-conflict to block when not on the target branch"
    );
    assert!(
        sentinel.exists(),
        "sentinel should remain when resume is blocked on wrong branch"
    );
    Ok(())
}

#[test]
fn test_merge_conflict_complete_flag_rejects_head_drift() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_conflicting_plan(
        &repo,
        "conflict-head-drift",
        "master branch keeps its version\n",
        "plan branch prefers this text\n",
    )?;
    clean_workdir(&repo)?;

    let merge = repo.vizier_output(&["merge", "conflict-head-drift", "--yes"])?;
    assert!(
        !merge.status.success(),
        "expected merge to fail on conflicts"
    );

    let sentinel = repo
        .path()
        .join(".vizier/tmp/merge-conflicts/conflict-head-drift.json");
    assert!(
        sentinel.exists(),
        "conflict sentinel missing after initial failure"
    );

    repo.git(&["cherry-pick", "--abort"])?;
    repo.write("a", "head moved after conflicts\n")?;
    repo.git(&["commit", "-am", "head drifted"])?;

    let resume = repo.vizier_output(&[
        "merge",
        "conflict-head-drift",
        "--yes",
        "--complete-conflict",
    ])?;
    assert!(
        !resume.status.success(),
        "expected --complete-conflict to block when HEAD moved"
    );
    assert!(
        !sentinel.exists(),
        "sentinel should be cleared when HEAD drift is detected"
    );
    Ok(())
}

#[test]
fn test_merge_complete_conflict_without_pending_state() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_conflicting_plan(
        &repo,
        "conflict-missing",
        "master has no conflicts yet\n",
        "plan branch prep work\n",
    )?;
    clean_workdir(&repo)?;

    let attempt =
        repo.vizier_output(&["merge", "conflict-missing", "--yes", "--complete-conflict"])?;
    assert!(
        !attempt.status.success(),
        "expected --complete-conflict to fail when no merge is pending"
    );
    let stderr = String::from_utf8_lossy(&attempt.stderr);
    assert!(
        stderr.contains("No Vizier-managed merge is awaiting completion"),
        "stderr missing helpful message: {}",
        stderr
    );

    let sentinel = repo
        .path()
        .join(".vizier/tmp/merge-conflicts/conflict-missing.json");
    assert!(
        !sentinel.exists(),
        "sentinel should not exist when the merge was never started"
    );
    Ok(())
}
