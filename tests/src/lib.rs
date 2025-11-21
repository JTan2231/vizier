#![cfg(test)]

use git2::{
    BranchType, DiffOptions, IndexAddOption, Oid, Repository, Signature, Sort,
    build::CheckoutBuilder,
};
use serde_json::Value;
use std::collections::HashSet;
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
}

impl IntegrationRepo {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let dir = TempDir::new()?;
        copy_dir_recursive(&repo_root().join("test-repo"), dir.path())?;
        copy_dir_recursive(&repo_root().join(".vizier"), &dir.path().join(".vizier"))?;
        ensure_gitignore(dir.path())?;
        write_default_cicd_script(dir.path())?;
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
    for line in output.lines() {
        for part in line.split(';') {
            let trimmed = part.trim();
            if let Some(value) = trimmed.strip_prefix(&prefix) {
                return Some(value.trim().to_string());
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

fn parse_usage_line(line: &str) -> Option<UsageSnapshot> {
    if !line.contains("[usage] token-usage") {
        return None;
    }

    if line.contains("unknown usage") {
        return Some(UsageSnapshot {
            known: false,
            ..UsageSnapshot::default()
        });
    }

    let mut snapshot = UsageSnapshot {
        known: true,
        ..UsageSnapshot::default()
    };
    let mut last_key: Option<&str> = None;

    for token in line.split_whitespace() {
        if let Some(value) = token.strip_prefix("prompt=") {
            snapshot.prompt_total = value.parse().ok()?;
            last_key = Some("prompt");
            continue;
        }
        if let Some(value) = token.strip_prefix("completion=") {
            snapshot.completion_total = value.parse().ok()?;
            last_key = Some("completion");
            continue;
        }
        if let Some(value) = token.strip_prefix("total=") {
            snapshot.total = value.parse().ok()?;
            last_key = Some("total");
            continue;
        }
        if let Some(value) = token.strip_prefix("cached_input=") {
            snapshot.cached_input_total = value.parse().ok()?;
            last_key = Some("cached_input");
            continue;
        }
        if let Some(value) = token.strip_prefix("reasoning_output=") {
            snapshot.reasoning_output_total = value.parse().ok()?;
            last_key = Some("reasoning_output");
            continue;
        }
        if let Some(value) = token.strip_prefix("(+") {
            let number = value.trim_end_matches(')').parse().ok()?;
            match last_key {
                Some("prompt") => snapshot.prompt_delta = number,
                Some("completion") => snapshot.completion_delta = number,
                Some("total") => snapshot.total_delta = number,
                Some("cached_input") => snapshot.cached_input_delta = number,
                Some("reasoning_output") => snapshot.reasoning_output_delta = number,
                _ => return None,
            }
        }
    }

    Some(snapshot)
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

fn last_usage_line(stderr: &str) -> Option<&str> {
    stderr
        .lines()
        .rev()
        .find(|line| line.contains("[usage] token-usage"))
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
        stdout.contains("mode=manual"),
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
        Some("process"),
        "save should use the default process backend"
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
        stderr.contains("cached_input="),
        "usage line should include cached input counts:\n{}",
        stderr
    );
    assert!(
        stderr.contains("reasoning_output="),
        "usage line should include reasoning output counts:\n{}",
        stderr
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
    let usage_line = last_usage_line(&stderr)
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "stderr missing usage line"))?;
    let cli_usage = parse_usage_line(usage_line)
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "failed to parse CLI usage line"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let contents = session_log_contents_from_output(&repo, &stdout)?;
    let session_usage = parse_session_usage(&contents)?;

    assert_eq!(
        session_usage, cli_usage,
        "session log usage should match CLI usage"
    );
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
    let usage_line = last_usage_line(&stderr)
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "usage line missing in stderr"))?;
    assert!(
        usage_line.contains("unknown usage"),
        "usage line should note unknown counts but was:\n{}",
        usage_line
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
fn test_draft_creates_branch_and_plan() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = count_commits_from_head(&repo.repo())?;
    let sessions_dir = repo.path().join(".vizier/sessions");
    if sessions_dir.exists() {
        fs::remove_dir_all(&sessions_dir)?;
    }

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
    let sessions_clean = if sessions_dir.exists() {
        let mut entries = fs::read_dir(&sessions_dir)?;
        entries.next().is_none()
    } else {
        true
    };
    assert!(
        sessions_clean,
        "session logs should not be created in the operator's working tree"
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

    let list_before = repo.vizier_output(&["list"])?;
    assert!(
        list_before.status.success(),
        "vizier list failed: {}",
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
    let approve_stderr = String::from_utf8_lossy(&approve.stderr);
    assert!(
        approve_stderr.contains("[agent:approve] apply plan"),
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
        stderr.contains("requires the process backend"),
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
        2,
        "merge commit should keep both parents"
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
        stdout.contains("fixes=[") && stdout.contains("amend:"),
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
        stdout.contains(" (tokens: total="),
        "review summary should include token suffix but was:\n{}",
        stdout
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

    let resume =
        repo.vizier_output(&["merge", "conflict-complete", "--yes", "--complete-conflict"])?;
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
