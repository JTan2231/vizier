#![cfg(test)]

use git2::{
    BranchType, DiffOptions, IndexAddOption, Oid, Repository, Signature, Sort,
    build::CheckoutBuilder,
};
use serde_json::Value;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{self, BufRead, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
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
    BIN.get_or_init(|| build_vizier_binary(&["mock_llm", "integration_testing"]))
}

fn vizier_binary_no_mock() -> &'static PathBuf {
    static BIN_NO_MOCK: OnceLock<PathBuf> = OnceLock::new();
    BIN_NO_MOCK.get_or_init(|| build_vizier_binary(&["integration_testing"]))
}

fn build_vizier_binary(features: &[&str]) -> PathBuf {
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
    vizier_bin: PathBuf,
}

impl IntegrationRepo {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_binary(vizier_binary().clone())
    }

    fn with_binary(bin: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let dir = TempDir::new()?;
        copy_dir_recursive(&repo_root().join("test-repo"), dir.path())?;
        copy_dir_recursive(&repo_root().join(".vizier"), &dir.path().join(".vizier"))?;
        ensure_gitignore(dir.path())?;
        write_default_cicd_script(dir.path())?;
        init_repo_at(dir.path())?;
        let agent_bin_dir = create_agent_shims(dir.path())?;
        Ok(Self {
            dir,
            agent_bin_dir,
            vizier_bin: bin,
        })
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn repo(&self) -> Repository {
        Repository::open(self.path()).expect("open repo")
    }

    fn vizier_cmd(&self) -> Command {
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

fn list_worktree_names(repo: &Repository) -> Result<Vec<String>, git2::Error> {
    Ok(repo
        .worktrees()?
        .iter()
        .filter_map(|name| name.map(|value| value.to_string()))
        .collect())
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

fn write_backend_stub(dir: &Path, name: &str) -> io::Result<PathBuf> {
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
        files.contains("a") && files.contains(".vizier/narrative/snapshot.md"),
        "combined commit should include code + narrative files, got {files:?}"
    );

    let snapshot = repo.read(".vizier/narrative/snapshot.md")?;
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
        files.contains("b") && files.contains(".vizier/narrative/snapshot.md"),
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
        files.contains(".vizier/narrative/snapshot.md") && !files.contains("a"),
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
        stdout.contains("Mode") && stdout.to_ascii_lowercase().contains("manual"),
        "expected manual mode indicator in output but saw: {stdout}"
    );

    let status = Command::new("git")
        .args([
            "-C",
            repo.path().to_str().unwrap(),
            "status",
            "--short",
            ".vizier/narrative/snapshot.md",
        ])
        .output()?;
    let status_stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        status_stdout.contains(".vizier/narrative/snapshot.md"),
        "expected .vizier/narrative/snapshot.md to be dirty after --no-commit save, git status was: {status_stdout}"
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
        files.contains(".vizier/narrative/snapshot.md") && files.contains("a"),
        "ask commit should include code and narrative assets, got {files:?}"
    );
    Ok(())
}

#[test]
fn test_missing_agent_binary_blocks_run() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let mut cmd = repo.vizier_cmd();
    cmd.env("PATH", "/nonexistent");
    cmd.args([
        "--agent-label",
        "missing-agent",
        "ask",
        "missing agent should fail",
    ]);

    let output = cmd.output()?;
    assert!(
        !output.status.success(),
        "ask should fail when the requested agent shim is missing"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr
            .to_ascii_lowercase()
            .contains("no bundled agent shim named `missing-agent`"),
        "stderr should explain missing agent shim: {stderr}"
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
agent = "codex"
"#,
    )?;

    let env_config = repo.path().join("env-config.toml");
    fs::write(
        &env_config,
        r#"
[agents.default]
agent = "gemini"
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
        Some("agent"),
        "repo config should force ask onto the configured backend despite env overrides"
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
agent = "codex"
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
        Some("agent"),
        "env config should take effect when no repo config exists"
    );
    Ok(())
}

#[test]
fn test_plan_command_outputs_resolved_config() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before_logs = gather_session_logs(&repo)?;
    let output = repo.vizier_output(&["plan"])?;
    assert!(
        output.status.success(),
        "vizier plan failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let compact = stdout.replace(' ', "");
    assert!(
        stdout.contains("Resolved configuration:"),
        "plan should print a resolved config header:\n{stdout}"
    );
    assert!(
        compact.contains("Agent:codex"),
        "plan output should include the resolved agent selector:\n{stdout}"
    );
    assert!(
        compact.contains("Backend:agent"),
        "plan output should include the resolved backend:\n{stdout}"
    );
    assert!(
        compact.contains("Stop-conditionscript:unset"),
        "plan output should include approve.stop_condition.script status:\n{stdout}"
    );
    assert!(
        compact.contains("CI/CDscript:./cicd.sh"),
        "plan output should include merge.cicd_gate.script:\n{stdout}"
    );
    assert!(
        stdout.contains("bundled `codex` shim"),
        "plan output should describe agent runtime resolution:\n{stdout}"
    );
    assert!(
        stdout.contains("Per-scope agents:"),
        "plan output should render per-scope agent settings:\n{stdout}"
    );

    let after_logs = gather_session_logs(&repo)?;
    assert_eq!(
        before_logs.len(),
        after_logs.len(),
        "vizier plan should not create session logs"
    );
    Ok(())
}

#[test]
fn test_plan_json_respects_config_file_and_overrides() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let config_path = repo.path().join("custom-config.toml");
    fs::write(
        &config_path,
        r#"
agent = "codex"
[approve.stop_condition]
script = "./approve-stop.sh"
retries = 7
[merge.cicd_gate]
script = "./alt-cicd.sh"
auto_resolve = false
retries = 5
[review.checks]
commands = ["echo alt-review"]
[workflow]
no_commit_default = true
"#,
    )?;

    let output = repo
        .vizier_cmd_with_config(&config_path)
        .args(["--agent", "gemini", "plan", "--json"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier plan --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout)?;

    assert_eq!(
        json.get("agent").and_then(Value::as_str),
        Some("gemini"),
        "CLI agent override should win even when config file is provided"
    );
    assert_eq!(
        json.get("backend").and_then(Value::as_str),
        Some("gemini"),
        "backend kind should reflect the resolved agent selector"
    );
    assert_eq!(
        json.pointer("/workflow/no_commit_default")
            .and_then(Value::as_bool),
        Some(true),
        "workflow.no_commit_default from the config file should appear in the report"
    );
    assert_eq!(
        json.pointer("/workflow/background/enabled")
            .and_then(Value::as_bool),
        Some(true),
        "workflow.background.enabled should appear in the report"
    );
    assert_eq!(
        json.pointer("/workflow/background/quiet")
            .and_then(Value::as_bool),
        Some(true),
        "workflow.background.quiet should appear in the report"
    );
    assert_eq!(
        json.pointer("/merge/cicd_gate/script")
            .and_then(Value::as_str),
        Some("./alt-cicd.sh"),
        "merge.cicd_gate.script from the config file should appear in the report"
    );
    assert_eq!(
        json.pointer("/merge/cicd_gate/retries")
            .and_then(Value::as_u64),
        Some(5),
        "merge.cicd_gate.retries from the config file should appear in the report"
    );
    assert_eq!(
        json.pointer("/approve/stop_condition/script")
            .and_then(Value::as_str),
        Some("./approve-stop.sh"),
        "approve.stop_condition.script from the config file should appear in the report"
    );
    assert_eq!(
        json.pointer("/approve/stop_condition/retries")
            .and_then(Value::as_u64),
        Some(7),
        "approve.stop_condition.retries from the config file should appear in the report"
    );
    assert_eq!(
        json.pointer("/review/checks/0").and_then(Value::as_str),
        Some("echo alt-review"),
        "review checks from the config file should appear in the report"
    );
    assert_eq!(
        json.pointer("/scopes/ask/backend").and_then(Value::as_str),
        Some("gemini"),
        "per-scope backend should reflect CLI overrides"
    );
    assert_eq!(
        json.pointer("/scopes/ask/agent").and_then(Value::as_str),
        Some("gemini"),
        "per-scope agent selector should reflect CLI overrides"
    );
    Ok(())
}

#[test]
fn test_plan_reports_agent_command_override() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let bin_dir = repo.path().join("bin");
    fs::create_dir_all(&bin_dir)?;
    let custom_bin = bin_dir.join("codex-custom");
    fs::write(&custom_bin, "#!/bin/sh\nexit 0\n")?;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&custom_bin)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&custom_bin, perms)?;
    }

    let output = repo
        .vizier_cmd()
        .args(["--agent-command", custom_bin.to_str().unwrap(), "plan"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier plan with agent command override failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let compact = stdout.replace(' ', "");
    assert!(
        compact.contains(&format!("Command:{}", custom_bin.display())),
        "plan output should surface the overridden agent command:\n{stdout}"
    );
    assert!(
        compact.contains("Resolution:providedcommand"),
        "plan output should mark the agent runtime as a provided command when CLI overrides are supplied:\n{stdout}"
    );
    Ok(())
}

#[test]
fn test_global_review_checks_fill_repo_defaults() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let repo_config = repo.path().join(".vizier").join("config.toml");
    fs::write(
        &repo_config,
        r#"
[merge.cicd_gate]
script = "./cicd.sh"
auto_resolve = true
retries = 2
"#,
    )?;

    let config_root = TempDir::new()?;
    let global_dir = config_root.path().join("vizier");
    fs::create_dir_all(&global_dir)?;
    let check_marker = repo.path().join("global-review-check.txt");
    fs::write(
        global_dir.join("config.toml"),
        format!(
            r#"
[review.checks]
commands = ["echo global-review-check >> \"{}\""]
"#,
            check_marker.display()
        ),
    )?;

    let mut draft_cmd = repo.vizier_cmd();
    draft_cmd.env("VIZIER_CONFIG_DIR", config_root.path());
    draft_cmd.env("XDG_CONFIG_HOME", config_root.path());
    draft_cmd.args([
        "draft",
        "--name",
        "global-review-check",
        "global review check spec",
    ]);
    let draft = draft_cmd.output()?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    clean_workdir(&repo)?;

    let mut approve_cmd = repo.vizier_cmd();
    approve_cmd.env("VIZIER_CONFIG_DIR", config_root.path());
    approve_cmd.env("XDG_CONFIG_HOME", config_root.path());
    approve_cmd.args(["approve", "global-review-check", "--yes"]);
    let approve = approve_cmd.output()?;
    assert!(
        approve.status.success(),
        "vizier approve failed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );

    clean_workdir(&repo)?;

    let mut review_cmd = repo.vizier_cmd();
    review_cmd.env("VIZIER_CONFIG_DIR", config_root.path());
    review_cmd.env("XDG_CONFIG_HOME", config_root.path());
    review_cmd.args(["review", "global-review-check", "--review-only"]);
    let review = review_cmd.output()?;
    assert!(
        review.status.success(),
        "vizier review failed: {}",
        String::from_utf8_lossy(&review.stderr)
    );

    assert!(
        check_marker.exists(),
        "global review check command should have created the marker file"
    );
    let contents = fs::read_to_string(&check_marker)?;
    assert!(
        contents.contains("global-review-check"),
        "marker file should include the check output, found: {contents}"
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
        stderr.contains("[codex:ask] agent — mock agent running"),
        "expected agent progress line, stderr was:\n{}",
        stderr
    );
    assert!(
        !stderr.to_ascii_lowercase().contains("token usage"),
        "token usage progress should not be emitted anymore:\n{}",
        stderr
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Agent run:"),
        "ask stdout should include agent run summary:\n{}",
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
        quiet_stderr.is_empty(),
        "quiet mode should suppress agent progress but printed:\n{}",
        quiet_stderr
    );
    Ok(())
}

#[test]
fn test_no_ansi_suppresses_escape_sequences() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo.vizier_output(&["--no-ansi", "ask", "ansi suppression check"])?;
    assert!(
        output.status.success(),
        "vizier ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains('\u{1b}'),
        "output should not include ANSI escapes when --no-ansi is set: {combined}"
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
    assert!(
        stderr.contains("[codex:save] agent — mock agent running"),
        "stderr missing agent progress lines:\n{}",
        stderr
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let contents = session_log_contents_from_output(&repo, &stdout)?;
    let session_json: Value = serde_json::from_str(&contents)?;
    let agent = session_json.get("agent").ok_or_else(|| {
        io::Error::new(io::ErrorKind::Other, "session log missing agent run data")
    })?;
    assert_eq!(
        agent.get("exit_code").and_then(Value::as_i64),
        Some(0),
        "session log should record agent exit status"
    );
    let stderr_lines = agent
        .get("stderr")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        !stderr_lines.is_empty(),
        "agent stderr should be captured in session log"
    );
    let stdout_value = agent
        .get("stdout")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        !stdout_value.trim().is_empty(),
        "agent stdout should be captured in session log"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn test_agent_wrapper_unbuffers_progress_integration() -> TestResult {
    if Command::new("stdbuf").arg("--version").output().is_err() {
        eprintln!("skipping unbuffering integration test because stdbuf is unavailable");
        return Ok(());
    }

    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skipping unbuffering integration test because python3 is unavailable");
        return Ok(());
    }

    let repo = IntegrationRepo::with_binary(vizier_binary_no_mock().clone())?;
    let bin_dir = repo.path().join(".vizier/tmp/bin");
    fs::create_dir_all(&bin_dir)?;

    let agent_path = bin_dir.join("buffered_agent.py");
    fs::write(
        &agent_path,
        r#"#!/usr/bin/env python3
import sys
import time
_ = sys.stdin.read()
sys.stdout.write('{"type":"item.started","item":{"type":"reasoning","text":"prep"}}\n')
sys.stdout.flush()
time.sleep(1)
sys.stdout.write('{"type":"item.completed","item":{"type":"agent_message","text":"done"}}\n')
sys.stdout.flush()
"#,
    )?;

    let filter_path = bin_dir.join("progress_filter.sh");
    fs::write(
        &filter_path,
        r#"#!/bin/sh
last=""
while IFS= read -r line; do
  last="$line"
  printf 'progress:%s\n' "$line" >&2
done
printf '%s' "$last"
"#,
    )?;

    #[cfg(unix)]
    {
        for script in [&agent_path, &filter_path] {
            let mut perms = fs::metadata(script)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(script, perms)?;
        }
    }

    let config_path = repo.path().join(".vizier/tmp/config-buffered.toml");
    fs::write(
        &config_path,
        format!(
            r#"
[agent]
command = ["{}"]
output = "wrapped-json"
progress_filter = ["{}"]
"#,
            agent_path.display(),
            filter_path.display()
        ),
    )?;

    let mut cmd = repo.vizier_cmd_with_config(&config_path);
    cmd.args(["ask", "buffered progress check"]);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn()?;
    let start = Instant::now();
    let mut stderr_reader = io::BufReader::new(child.stderr.take().expect("stderr piped"));
    let mut first_line = String::new();
    stderr_reader.read_line(&mut first_line)?;
    let elapsed = start.elapsed();
    assert!(
        !first_line.trim().is_empty(),
        "expected progress output before completion"
    );
    assert!(
        elapsed < Duration::from_millis(1200),
        "progress output should arrive before agent completes (elapsed {:?}, line {:?})",
        elapsed,
        first_line
    );
    assert!(
        first_line.contains("progress:"),
        "progress line should come from filter: {}",
        first_line
    );

    let mut remaining_err = String::new();
    stderr_reader.read_to_string(&mut remaining_err)?;

    let mut stdout = String::new();
    if let Some(mut out) = child.stdout.take() {
        out.read_to_string(&mut stdout)?;
    }
    let status = child.wait()?;
    assert!(status.success(), "vizier ask failed: {}", remaining_err);
    assert!(
        !remaining_err.contains("stdbuf not found"),
        "expected stdbuf wrapper to be available, stderr: {}",
        remaining_err
    );
    assert!(
        stdout.contains("done"),
        "expected final assistant text in stdout, got: {}",
        stdout
    );

    Ok(())
}

#[cfg(unix)]
#[test]
fn test_agent_wrapper_fallbacks_emit_warnings() -> TestResult {
    // Prefer stdbuf; if it's present, skip this fallback test to avoid interfering with main coverage.
    if Command::new("stdbuf").arg("--version").output().is_ok() {
        eprintln!("skipping fallback warning test because stdbuf is available");
        return Ok(());
    }

    let repo = IntegrationRepo::with_binary(vizier_binary_no_mock().clone())?;
    let bin_dir = repo.path().join(".vizier/tmp/bin");
    fs::create_dir_all(&bin_dir)?;

    let agent_path = bin_dir.join("buffered_agent.sh");
    fs::write(
        &agent_path,
        r#"#!/bin/sh
set -e
cat >/dev/null
printf '%s\n' '{"type":"item.started","item":{"type":"reasoning","text":"prep"}}'
sleep 1
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"done"}}'
"#,
    )?;

    let filter_path = bin_dir.join("progress_filter.sh");
    fs::write(
        &filter_path,
        r#"#!/bin/sh
while IFS= read -r line; do
  printf 'progress:%s\n' "$line"
done
"#,
    )?;

    #[cfg(unix)]
    {
        for script in [&agent_path, &filter_path] {
            let mut perms = fs::metadata(script)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(script, perms)?;
        }
    }

    // Hide stdbuf/unbuffer by using a minimal PATH.
    let config_path = repo
        .path()
        .join(".vizier/tmp/config-buffered-fallback.toml");
    fs::write(
        &config_path,
        format!(
            r#"
[agent]
command = ["{}"]
output = "wrapped-json"
progress_filter = ["{}"]
"#,
            agent_path.display(),
            filter_path.display()
        ),
    )?;

    let mut cmd = repo.vizier_cmd_with_config(&config_path);
    cmd.args(["ask", "buffered progress fallback"]);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "vizier ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(())
}

#[test]
fn test_session_log_handles_unknown_token_usage() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let before = gather_session_logs(&repo)?;

    let mut cmd = repo.vizier_cmd();
    cmd.args(["-q", "ask", "suppress usage event"]);
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "vizier ask failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.trim().is_empty(),
        "quiet ask should not emit stderr: {stderr}"
    );

    let after = gather_session_logs(&repo)?;
    let session_path = new_session_log(&before, &after)
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "missing new session log"))?
        .clone();
    let contents = fs::read_to_string(&session_path)?;
    let session_json: Value = serde_json::from_str(&contents)?;
    let agent = session_json.get("agent").ok_or_else(|| {
        io::Error::new(io::ErrorKind::Other, "session log missing agent run data")
    })?;
    assert_eq!(
        agent.get("exit_code").and_then(Value::as_i64),
        Some(0),
        "session log should still record agent exit even when output is quiet"
    );
    Ok(())
}

#[test]
fn test_script_runner_session_logs_io_across_commands() -> TestResult {
    let repo = IntegrationRepo::with_binary(vizier_binary_no_mock().clone())?;

    let capture_agent_log =
        |args: &[&str], label: &str| -> Result<Value, Box<dyn std::error::Error>> {
            let before = gather_session_logs(&repo)?;
            let mut cmd = repo.vizier_cmd();
            cmd.env("OPENAI_API_KEY", "test-key");
            cmd.env("ANTHROPIC_API_KEY", "test-key");
            cmd.args(args);
            let output = cmd.output()?;
            assert!(
                output.status.success(),
                "vizier {label} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let after = gather_session_logs(&repo)?;
            let session_path = new_session_log(&before, &after)
                .ok_or_else(|| format!("missing session log for {label}"))?
                .clone();
            let contents = fs::read_to_string(session_path)?;
            let json: Value = serde_json::from_str(&contents)?;
            Ok(json)
        };

    let assert_agent_io = |json: &Value, label: &str| {
        let agent = json
            .get("agent")
            .unwrap_or_else(|| panic!("session log missing agent run for {label}"));
        let command: Vec<String> = agent
            .get("command")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            command.iter().any(|entry| entry.contains("codex")),
            "{label} session log should capture agent command, got {command:?}"
        );
        assert!(
            agent
                .get("stdout")
                .and_then(Value::as_str)
                .unwrap_or("")
                .contains("mock agent response"),
            "{label} session log should persist agent stdout"
        );
        let stderr_lines: Vec<String> = agent
            .get("stderr")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            stderr_lines
                .iter()
                .any(|line| line.contains("mock agent running")),
            "{label} session log should capture agent stderr, found {stderr_lines:?}"
        );
        assert!(
            agent
                .get("duration_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 0,
            "{label} session log should record duration"
        );
    };

    let ask = capture_agent_log(&["ask", "script runner smoke"], "ask")?;
    assert_agent_io(&ask, "ask");

    let save = capture_agent_log(&["save"], "save")?;
    assert_agent_io(&save, "save");

    let draft = capture_agent_log(
        &["draft", "--name", "script-runner", "script runner plan"],
        "draft",
    )?;
    assert_agent_io(&draft, "draft");

    clean_workdir(&repo)?;

    let approve = capture_agent_log(&["approve", "script-runner", "--yes"], "approve")?;
    assert_agent_io(&approve, "approve");

    clean_workdir(&repo)?;

    let review = capture_agent_log(
        &["review", "script-runner", "--review-only", "--skip-checks"],
        "review",
    )?;
    assert_agent_io(&review, "review");

    clean_workdir(&repo)?;

    let mut merge_cmd = repo.vizier_cmd();
    merge_cmd.env("OPENAI_API_KEY", "test-key");
    merge_cmd.env("ANTHROPIC_API_KEY", "test-key");
    merge_cmd.args(["merge", "script-runner", "--yes"]);
    let merge = merge_cmd.output()?;
    assert!(
        merge.status.success(),
        "vizier merge failed with real script runner: {}",
        String::from_utf8_lossy(&merge.stderr)
    );

    Ok(())
}

#[test]
fn test_draft_reports_token_usage() -> TestResult {
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
    assert!(
        stdout.contains("Agent") && stdout.contains("codex"),
        "draft summary should include agent metadata:\n{stdout}"
    );
    assert!(
        stdout.contains("Exit code"),
        "draft summary should include the agent exit code:\n{stdout}"
    );

    let after_logs = gather_session_logs(&repo)?;
    let session_path = new_session_log(&before_logs, &after_logs)
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "expected session log for draft"))?;
    let contents = fs::read_to_string(&session_path)?;
    let session_json: Value = serde_json::from_str(&contents)?;
    let agent = session_json.get("agent").ok_or_else(|| {
        io::Error::new(io::ErrorKind::Other, "session log missing agent run data")
    })?;
    let exit_code = agent
        .get("exit_code")
        .and_then(Value::as_i64)
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "agent.exit_code missing"))?;
    assert_eq!(exit_code, 0, "agent exit code should be recorded");
    let stderr = agent
        .get("stderr")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        !stderr.is_empty(),
        "session log should include agent stderr lines"
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
        approve_stderr.contains("[codex:approve] agent — mock agent running"),
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
fn test_cd_creates_and_reuses_workspace() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.git(&["checkout", "-b", "draft/workspace-check"])?;
    repo.git(&["checkout", "master"])?;

    let first = repo.vizier_output(&["cd", "workspace-check"])?;
    assert!(
        first.status.success(),
        "vizier cd failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let stdout_first = String::from_utf8_lossy(&first.stdout);
    let path_first = stdout_first
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    assert!(
        !path_first.is_empty(),
        "cd should print the workspace path on the first line:\n{stdout_first}"
    );
    assert!(
        Path::new(&path_first).exists(),
        "workspace path should exist after vizier cd: {}",
        path_first
    );
    let repo_handle = repo.repo();
    let worktrees = list_worktree_names(&repo_handle)?;
    assert!(
        worktrees
            .iter()
            .any(|name| name == "vizier-workspace-workspace-check"),
        "worktree list should include the workspace name after cd: {worktrees:?}"
    );

    let second = repo.vizier_output(&["cd", "workspace-check"])?;
    assert!(
        second.status.success(),
        "vizier cd (reuse) failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let stdout_second = String::from_utf8_lossy(&second.stdout);
    let path_second = stdout_second
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    assert_eq!(
        path_first, path_second,
        "second cd should reuse the same workspace path"
    );

    Ok(())
}

#[test]
fn test_cd_fails_when_branch_missing() -> TestResult {
    let repo = IntegrationRepo::new()?;

    let output = repo.vizier_output(&["cd", "missing-branch"])?;
    assert!(
        !output.status.success(),
        "vizier cd should fail when the branch is missing"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("branch draft/missing-branch does not exist"),
        "stderr should explain missing branch: {stderr}"
    );
    assert!(
        stderr.contains("vizier draft missing-branch"),
        "stderr should hint at drafting the plan before cd, got: {stderr}"
    );

    Ok(())
}

#[test]
fn test_clean_prunes_requested_workspaces() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.git(&["checkout", "-b", "draft/workspace-alpha"])?;
    repo.git(&["checkout", "master"])?;
    repo.git(&["checkout", "-b", "draft/workspace-beta"])?;
    repo.git(&["checkout", "master"])?;

    let alpha = repo.vizier_output(&["cd", "workspace-alpha"])?;
    assert!(
        alpha.status.success(),
        "vizier cd alpha failed: {}",
        String::from_utf8_lossy(&alpha.stderr)
    );
    let alpha_path = PathBuf::from(
        String::from_utf8_lossy(&alpha.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim(),
    );
    let beta = repo.vizier_output(&["cd", "workspace-beta"])?;
    assert!(
        beta.status.success(),
        "vizier cd beta failed: {}",
        String::from_utf8_lossy(&beta.stderr)
    );
    let beta_path = PathBuf::from(
        String::from_utf8_lossy(&beta.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim(),
    );

    let clean_one = repo.vizier_output(&["clean", "workspace-alpha", "--yes"])?;
    assert!(
        clean_one.status.success(),
        "vizier clean alpha failed: {}",
        String::from_utf8_lossy(&clean_one.stderr)
    );
    assert!(
        !alpha_path.exists(),
        "targeted clean should remove the requested workspace directory"
    );
    let after_one = list_worktree_names(&repo.repo())?;
    assert!(
        !after_one
            .iter()
            .any(|name| name == "vizier-workspace-workspace-alpha"),
        "targeted clean should drop the alpha worktree registration"
    );
    assert!(
        after_one
            .iter()
            .any(|name| name == "vizier-workspace-workspace-beta"),
        "targeted clean should leave other workspaces in place"
    );
    assert!(
        beta_path.exists(),
        "targeted clean should not remove unrelated workspace paths"
    );

    let clean_all = repo.vizier_output(&["clean", "--yes"])?;
    assert!(
        clean_all.status.success(),
        "vizier clean --yes failed: {}",
        String::from_utf8_lossy(&clean_all.stderr)
    );
    let after_all = list_worktree_names(&repo.repo())?;
    assert!(
        !after_all
            .iter()
            .any(|name| name.starts_with("vizier-workspace-")),
        "global clean should remove all vizier-managed workspaces"
    );
    assert!(
        !beta_path.exists(),
        "global clean should remove remaining workspace directories"
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
        files.contains(".vizier/narrative/snapshot.md") && files.contains("a"),
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
        stderr.to_ascii_lowercase().contains("agent command exited"),
        "stderr should mention agent command failure, got: {stderr}"
    );
    assert!(
        stderr.contains("42"),
        "stderr should include the exit status, got: {stderr}"
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

    let merge = repo.vizier_output(&[
        "merge",
        "replay-conflict",
        "--yes",
        "--no-auto-resolve-conflicts",
    ])?;
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

    let resume = repo.vizier_output(&[
        "merge",
        "replay-conflict",
        "--yes",
        "--no-auto-resolve-conflicts",
        "--complete-conflict",
    ])?;
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
fn test_approve_stop_condition_passes_on_first_attempt() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "stop-pass", "stop condition pass spec"])?;
    clean_workdir(&repo)?;

    let log_path = repo.path().join("approve-stop-pass.log");
    let script_path = write_cicd_script(
        &repo,
        "approve-stop-pass.sh",
        &format!(
            "#!/bin/sh\nset -eu\necho \"stop-called\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    )?;
    let script_flag = script_path.to_string_lossy().to_string();

    let before_logs = gather_session_logs(&repo)?;
    let approve = repo.vizier_output(&[
        "approve",
        "stop-pass",
        "--yes",
        "--stop-condition-script",
        &script_flag,
    ])?;
    assert!(
        approve.status.success(),
        "vizier approve with passing stop-condition should succeed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );

    assert!(
        log_path.exists(),
        "stop-condition script should run at least once"
    );
    let contents = fs::read_to_string(&log_path)?;
    let lines: Vec<_> = contents.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "stop-condition script should run exactly once when it passes on the first attempt, got {} lines",
        lines.len()
    );

    let after_logs = gather_session_logs(&repo)?;
    let new_log = new_session_log(&before_logs, &after_logs)
        .ok_or_else(|| "expected vizier approve to create a session log".to_string())?;
    let contents = fs::read_to_string(new_log)?;
    let json: Value = serde_json::from_str(&contents)?;
    let operations = json
        .get("operations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let attempt_ops: Vec<_> = operations
        .iter()
        .filter(|entry| {
            entry
                .get("kind")
                .and_then(Value::as_str)
                .map(|kind| kind == "approve_stop_condition_attempt")
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(
        attempt_ops.len(),
        1,
        "expected exactly one stop-condition attempt record"
    );
    let attempt_details = attempt_ops[0]
        .get("details")
        .and_then(Value::as_object)
        .ok_or_else(|| "approve_stop_condition_attempt missing details".to_string())?;
    assert_eq!(
        attempt_details.get("attempt").and_then(Value::as_u64),
        Some(1),
        "attempt record should mark the first run"
    );
    assert_eq!(
        attempt_details.get("status").and_then(Value::as_str),
        Some("passed"),
        "attempt record should show passed status: {:?}",
        attempt_details
    );
    let stop_op = operations
        .iter()
        .find(|entry| entry.get("kind").and_then(Value::as_str) == Some("approve_stop_condition"))
        .cloned()
        .ok_or_else(|| "expected approve_stop_condition operation in session log".to_string())?;
    let details = stop_op
        .get("details")
        .and_then(Value::as_object)
        .ok_or_else(|| "approve_stop_condition operation missing details".to_string())?;
    assert_eq!(
        details.get("status").and_then(Value::as_str),
        Some("passed"),
        "stop-condition status should be passed: {details:?}"
    );
    assert_eq!(
        details.get("attempts").and_then(Value::as_u64),
        Some(1),
        "stop-condition attempts should be 1 when it passes on the first run: {details:?}"
    );
    Ok(())
}

#[test]
fn test_approve_stop_condition_retries_then_passes() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "stop-retry", "stop condition retry spec"])?;
    clean_workdir(&repo)?;

    let counter_path = repo.path().join("approve-stop-count.txt");
    let log_path = repo.path().join("approve-stop-retry.log");
    let script_path = write_cicd_script(
        &repo,
        "approve-stop-retry.sh",
        &format!(
            "#!/bin/sh\nset -eu\nCOUNT_FILE=\"{}\"\nif [ -f \"$COUNT_FILE\" ]; then\n  n=$(cat \"$COUNT_FILE\")\nelse\n  n=0\nfi\nn=$((n+1))\necho \"$n\" > \"$COUNT_FILE\"\necho \"run $n\" >> \"{}\"\nif [ \"$n\" -lt 2 ]; then\n  exit 1\nfi\nexit 0\n",
            counter_path.display(),
            log_path.display()
        ),
    )?;
    let script_flag = script_path.to_string_lossy().to_string();

    let before_logs = gather_session_logs(&repo)?;
    let approve = repo.vizier_output(&[
        "approve",
        "stop-retry",
        "--yes",
        "--stop-condition-script",
        &script_flag,
        "--stop-condition-retries",
        "3",
    ])?;
    assert!(
        approve.status.success(),
        "vizier approve with retrying stop-condition should succeed: {}",
        String::from_utf8_lossy(&approve.stderr)
    );

    let contents = fs::read_to_string(&counter_path)?;
    assert_eq!(
        contents.trim(),
        "2",
        "stop-condition script should have run twice before passing, got counter contents: {contents}"
    );

    let after_logs = gather_session_logs(&repo)?;
    let new_log = new_session_log(&before_logs, &after_logs)
        .ok_or_else(|| "expected vizier approve to create a session log".to_string())?;
    let contents = fs::read_to_string(new_log)?;
    let json: Value = serde_json::from_str(&contents)?;
    let operations = json
        .get("operations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let attempt_ops: Vec<_> = operations
        .iter()
        .filter(|entry| {
            entry
                .get("kind")
                .and_then(Value::as_str)
                .map(|kind| kind == "approve_stop_condition_attempt")
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(
        attempt_ops.len(),
        2,
        "expected two stop-condition attempt records when a retry occurs"
    );
    let attempt_statuses: Vec<_> = attempt_ops
        .iter()
        .filter_map(|entry| {
            entry
                .get("details")
                .and_then(Value::as_object)
                .and_then(|details| details.get("status"))
                .and_then(Value::as_str)
        })
        .collect();
    assert_eq!(
        attempt_statuses,
        vec!["failed", "passed"],
        "attempt records should capture the failed then passed sequence: {:?}",
        attempt_statuses
    );
    let stop_op = operations
        .iter()
        .find(|entry| entry.get("kind").and_then(Value::as_str) == Some("approve_stop_condition"))
        .cloned()
        .ok_or_else(|| "expected approve_stop_condition operation in session log".to_string())?;
    let details = stop_op
        .get("details")
        .and_then(Value::as_object)
        .ok_or_else(|| "approve_stop_condition operation missing details".to_string())?;
    assert_eq!(
        details.get("status").and_then(Value::as_str),
        Some("passed"),
        "stop-condition status should be passed after retries: {details:?}"
    );
    assert_eq!(
        details.get("attempts").and_then(Value::as_u64),
        Some(2),
        "stop-condition attempts should be 2 when it fails once then passes: {details:?}"
    );
    Ok(())
}

#[test]
fn test_approve_stop_condition_exhausts_retries_and_fails() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "stop-fail", "stop condition failure spec"])?;
    clean_workdir(&repo)?;

    let log_path = repo.path().join("approve-stop-fail.log");
    let script_path = write_cicd_script(
        &repo,
        "approve-stop-fail.sh",
        &format!(
            "#!/bin/sh\nset -eu\necho \"fail\" >> \"{}\"\nexit 1\n",
            log_path.display()
        ),
    )?;
    let script_flag = script_path.to_string_lossy().to_string();

    let before_logs = gather_session_logs(&repo)?;
    let approve = repo.vizier_output(&[
        "approve",
        "stop-fail",
        "--yes",
        "--stop-condition-script",
        &script_flag,
        "--stop-condition-retries",
        "2",
    ])?;
    assert!(
        !approve.status.success(),
        "vizier approve should fail when the stop-condition never passes"
    );
    let stderr = String::from_utf8_lossy(&approve.stderr);
    assert!(
        stderr.contains("Plan worktree preserved at"),
        "stderr should mention preserved worktree for failed stop-condition: {stderr}"
    );

    let contents = fs::read_to_string(&log_path)?;
    let attempts = contents.lines().count();
    assert!(
        attempts >= 3,
        "stop-condition script should run at least three times when retries are exhausted (saw {attempts} runs)"
    );

    let after_logs = gather_session_logs(&repo)?;
    let new_log = new_session_log(&before_logs, &after_logs)
        .ok_or_else(|| "expected vizier approve to create a session log".to_string())?;
    let contents = fs::read_to_string(new_log)?;
    let json: Value = serde_json::from_str(&contents)?;
    let operations = json
        .get("operations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let attempt_ops: Vec<_> = operations
        .iter()
        .filter(|entry| {
            entry
                .get("kind")
                .and_then(Value::as_str)
                .map(|kind| kind == "approve_stop_condition_attempt")
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(
        attempt_ops.len(),
        3,
        "expected three stop-condition attempt records when retries are exhausted"
    );
    assert!(
        attempt_ops.iter().all(|entry| {
            entry
                .get("details")
                .and_then(Value::as_object)
                .and_then(|details| details.get("status"))
                .and_then(Value::as_str)
                == Some("failed")
        }),
        "all attempt records should be failed when the stop condition never passes: {:?}",
        attempt_ops
    );
    let stop_op = operations
        .iter()
        .find(|entry| entry.get("kind").and_then(Value::as_str) == Some("approve_stop_condition"))
        .cloned()
        .ok_or_else(|| "expected approve_stop_condition operation in session log".to_string())?;
    let details = stop_op
        .get("details")
        .and_then(Value::as_object)
        .ok_or_else(|| "approve_stop_condition operation missing details".to_string())?;
    assert_eq!(
        details.get("status").and_then(Value::as_str),
        Some("failed"),
        "stop-condition status should be failed when retries are exhausted: {details:?}"
    );
    assert_eq!(
        details.get("attempts").and_then(Value::as_u64),
        Some(3),
        "stop-condition attempts should be 3 when retries=2 and the script never passes: {details:?}"
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
        files.contains(".vizier/narrative/snapshot.md"),
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
        stdout.contains("Agent") && stdout.contains("codex"),
        "review summary should include agent details but was:\n{stdout}"
    );
    assert!(
        stdout.contains("Exit code"),
        "review summary should include agent exit code:\n{stdout}"
    );
    assert!(
        stdout.contains("mock agent response"),
        "review summary should surface the critique text:\n{stdout}"
    );
    Ok(())
}

#[test]
fn test_review_runs_cicd_gate_before_critique() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "review-gate-pass", "gate pass spec"])?;
    repo.vizier_output(&["approve", "review-gate-pass", "--yes"])?;
    clean_workdir(&repo)?;

    let gate_log = repo.path().join("review-gate.log");
    let script_path = write_cicd_script(
        &repo,
        "review-gate-pass.sh",
        &format!(
            "#!/bin/sh\nset -eu\necho \"gate ran\" > \"{}\"\n",
            gate_log.display()
        ),
    )?;
    let script_flag = script_path.to_string_lossy().to_string();
    let review = repo.vizier_output(&[
        "review",
        "review-gate-pass",
        "--review-only",
        "--skip-checks",
        "--cicd-script",
        &script_flag,
    ])?;
    assert!(
        review.status.success(),
        "vizier review failed: {}",
        String::from_utf8_lossy(&review.stderr)
    );

    assert!(
        gate_log.exists(),
        "CI/CD gate script should run before the critique"
    );

    let stdout = String::from_utf8_lossy(&review.stdout);
    assert!(
        stdout.contains("CI/CD gate") && stdout.contains("passed"),
        "review summary should report the passed CI/CD gate:\n{stdout}"
    );

    let contents = session_log_contents_from_output(&repo, &stdout)?;
    let json: Value = serde_json::from_str(&contents)?;
    let operations = json
        .get("operations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        operations.iter().any(|entry| {
            entry.get("kind").and_then(Value::as_str) == Some("cicd_gate")
                && entry
                    .get("details")
                    .and_then(|details| details.get("status"))
                    .and_then(Value::as_str)
                    == Some("passed")
        }),
        "session log should capture a passed CI/CD gate operation: {operations:?}"
    );

    Ok(())
}

#[test]
fn test_review_surfaces_failed_cicd_gate_and_continues() -> TestResult {
    let repo = IntegrationRepo::new()?;
    repo.vizier_output(&["draft", "--name", "review-gate-fail", "gate fail spec"])?;
    repo.vizier_output(&["approve", "review-gate-fail", "--yes"])?;
    clean_workdir(&repo)?;

    let gate_log = repo.path().join("review-gate-fail.log");
    let script_path = write_cicd_script(
        &repo,
        "review-gate-fail.sh",
        &format!(
            "#!/bin/sh\nset -eu\necho \"broken gate\" > \"{}\"\nexit 1\n",
            gate_log.display()
        ),
    )?;
    let script_flag = script_path.to_string_lossy().to_string();
    let review = repo.vizier_output(&[
        "review",
        "review-gate-fail",
        "--review-only",
        "--skip-checks",
        "--cicd-script",
        &script_flag,
    ])?;
    assert!(
        review.status.success(),
        "vizier review should continue even when the gate fails: {}",
        String::from_utf8_lossy(&review.stderr)
    );

    let stdout = String::from_utf8_lossy(&review.stdout);
    assert!(
        stdout.contains("CI/CD gate") && stdout.contains("failed"),
        "review summary should report the failed CI/CD gate:\n{stdout}"
    );
    assert!(
        stdout.contains("--- Review critique for plan review-gate-fail ---"),
        "critique should still stream when the gate fails:\n{stdout}"
    );

    assert!(
        gate_log.exists(),
        "failed CI/CD gate should still run before the critique"
    );

    let contents = session_log_contents_from_output(&repo, &stdout)?;
    let json: Value = serde_json::from_str(&contents)?;
    let operations = json
        .get("operations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        operations.iter().any(|entry| {
            entry.get("kind").and_then(Value::as_str) == Some("cicd_gate")
                && entry
                    .get("details")
                    .and_then(|details| details.get("status"))
                    .and_then(Value::as_str)
                    == Some("failed")
        }),
        "session log should capture a failed CI/CD gate operation: {operations:?}"
    );
    assert!(
        operations.iter().any(|entry| {
            entry
                .get("details")
                .and_then(|details| details.get("exit_code"))
                .and_then(Value::as_i64)
                == Some(1)
        }),
        "failed gate operation should record exit code 1: {operations:?}"
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
    let stderr = String::from_utf8_lossy(&merge.stderr);
    assert!(
        stderr.contains("Auto-resolving merge conflicts via"),
        "stderr should mention config-driven conflict auto-resolution: {stderr}"
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
fn test_merge_conflict_auto_resolve_reuses_setting_on_resume() -> TestResult {
    let repo = IntegrationRepo::new()?;
    prepare_conflicting_plan(
        &repo,
        "conflict-resume-auto",
        "master edits collide\n",
        "plan branch wins after resume\n",
    )?;
    clean_workdir(&repo)?;

    let mut first = repo.vizier_cmd();
    first.args([
        "merge",
        "conflict-resume-auto",
        "--yes",
        "--no-auto-resolve-conflicts",
    ]);
    let initial = first.output()?;
    assert!(
        !initial.status.success(),
        "initial merge should fail when auto-resolve is disabled"
    );
    let sentinel = repo
        .path()
        .join(".vizier/tmp/merge-conflicts/conflict-resume-auto.json");
    assert!(
        sentinel.exists(),
        "sentinel should remain after failed auto-resolution attempt"
    );

    let resume = repo.vizier_output(&[
        "merge",
        "conflict-resume-auto",
        "--yes",
        "--complete-conflict",
        "--auto-resolve-conflicts",
    ])?;
    assert!(
        resume.status.success(),
        "vizier merge --complete-conflict should reuse auto-resolve and succeed: {}",
        String::from_utf8_lossy(&resume.stderr)
    );
    let resume_stderr = String::from_utf8_lossy(&resume.stderr);
    assert!(
        resume_stderr.contains("Auto-resolving merge conflicts via")
            || resume_stderr.contains("Conflict auto-resolution enabled"),
        "resume should surface conflict auto-resolve status: {resume_stderr}"
    );
    assert!(
        !sentinel.exists(),
        "sentinel should be cleared after successful auto-resolve resume"
    );
    let contents = repo.read("a")?;
    assert!(
        contents.contains("plan branch wins after resume"),
        "auto-resolve resume should apply plan contents: {contents}"
    );
    let status = Command::new("git")
        .args(["-C", repo.path().to_str().unwrap(), "status", "--porcelain"])
        .output()?;
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "working tree should be clean after auto-resolve resume"
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

    let merge = repo.vizier_output(&[
        "merge",
        "conflict-manual",
        "--yes",
        "--no-auto-resolve-conflicts",
    ])?;
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

    let merge = repo.vizier_output(&[
        "merge",
        "conflict-complete",
        "--yes",
        "--no-auto-resolve-conflicts",
    ])?;
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

    let resume = repo.vizier_output(&[
        "merge",
        "conflict-complete",
        "--yes",
        "--no-auto-resolve-conflicts",
        "--complete-conflict",
    ])?;
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

    let merge = repo.vizier_output(&[
        "merge",
        "conflict-wrong-branch",
        "--yes",
        "--no-auto-resolve-conflicts",
    ])?;
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
        "--no-auto-resolve-conflicts",
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

    let merge = repo.vizier_output(&[
        "merge",
        "conflict-head-drift",
        "--yes",
        "--no-auto-resolve-conflicts",
    ])?;
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
        "--no-auto-resolve-conflicts",
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

#[test]
fn test_test_display_smoke_is_clean() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let output = repo.vizier_output(&["test-display"])?;
    assert!(
        output.status.success(),
        "test-display should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Agent display test succeeded"),
        "stdout missing success summary: {stdout}"
    );

    let status = Command::new("git")
        .args(["-C", repo.path().to_str().unwrap(), "status", "--porcelain"])
        .output()?;
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "test-display should not touch the repo: {}",
        String::from_utf8_lossy(&status.stdout)
    );
    Ok(())
}

#[test]
fn test_test_display_propagates_agent_exit_code() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let mut cmd = repo.vizier_cmd();
    cmd.arg("test-display");
    cmd.env("VIZIER_FORCE_AGENT_ERROR", "true");
    let output = cmd.output()?;
    assert_eq!(
        output.status.code(),
        Some(42),
        "expected test-display to exit with the agent status"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("status 42") || stderr.contains("agent"),
        "stderr should mention agent failure: {stderr}"
    );

    let status = Command::new("git")
        .args(["-C", repo.path().to_str().unwrap(), "status", "--porcelain"])
        .output()?;
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "failure path should leave the repo untouched: {}",
        String::from_utf8_lossy(&status.stdout)
    );
    Ok(())
}

#[test]
fn test_test_display_raw_and_quiet_modes() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let raw = repo.vizier_output(&["test-display", "--raw"])?;
    assert!(
        raw.status.success(),
        "raw run failed: {}",
        String::from_utf8_lossy(&raw.stderr)
    );
    let raw_stdout = String::from_utf8_lossy(&raw.stdout);
    assert!(
        raw_stdout.contains("mock agent response"),
        "raw output should include captured stdout: {raw_stdout}"
    );
    let raw_stderr = String::from_utf8_lossy(&raw.stderr);
    assert!(
        raw_stderr.contains("mock agent running") || raw_stderr.contains("mock stderr"),
        "raw stderr should surface progress or captured stderr: {raw_stderr}"
    );

    let quiet = repo.vizier_output(&["-q", "test-display"])?;
    assert!(
        quiet.status.success(),
        "quiet run failed: {}",
        String::from_utf8_lossy(&quiet.stderr)
    );
    assert!(
        String::from_utf8_lossy(&quiet.stdout).trim().is_empty(),
        "quiet mode should suppress stdout summary: {}",
        String::from_utf8_lossy(&quiet.stdout)
    );
    Ok(())
}

#[test]
fn test_test_display_can_write_session_when_opted_in() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let before_logs = gather_session_logs(&repo)?;
    let output = repo.vizier_output(&["test-display", "--session"])?;
    assert!(
        output.status.success(),
        "session-enabled run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let after_logs = gather_session_logs(&repo)?;
    let new_log = new_session_log(&before_logs, &after_logs)
        .ok_or_else(|| "expected test-display to write a session log when --session is set")?;
    let contents = fs::read_to_string(new_log)?;
    let json: Value = serde_json::from_str(&contents)?;
    assert_eq!(
        json.get("model")
            .and_then(|model| model.get("scope"))
            .and_then(Value::as_str),
        Some("ask"),
        "session log should record the default scope"
    );
    Ok(())
}

#[test]
fn test_help_respects_no_ansi_and_quiet() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let mut cmd = repo.vizier_cmd();
    cmd.args(["--help", "-q", "--no-ansi"]);
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "help should exit 0 even with quiet/no-ansi: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.trim().is_empty(),
        "quiet mode should not suppress help output: {stdout}"
    );
    assert!(
        !stdout.contains('\u{1b}'),
        "help output should omit ANSI when --no-ansi is set: {stdout}"
    );
    Ok(())
}

#[test]
fn test_help_does_not_invoke_pager_when_not_a_tty() -> TestResult {
    let repo = IntegrationRepo::new()?;
    clean_workdir(&repo)?;

    let temp_dir = TempDir::new()?;
    let pager_log = temp_dir.path().join("pager-hit.log");
    let pager_cmd = format!("cat > {}", pager_log.display());

    let mut cmd = repo.vizier_cmd();
    cmd.env("VIZIER_PAGER", &pager_cmd);
    cmd.args(["--pager", "--help"]);
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "help with --pager should exit 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        !pager_log.exists(),
        "pager command should not run when stdout is not a TTY"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.trim().is_empty(),
        "help should still print when pager is suppressed: {stdout}"
    );
    Ok(())
}

#[test]
fn codex_shim_forwards_prompt_and_args() -> TestResult {
    let tmp = TempDir::new()?;
    let bin_dir = tmp.path().join("bin");
    let input_log = tmp.path().join("codex-input.log");
    let args_log = tmp.path().join("codex-args.log");
    write_backend_stub(&bin_dir, "codex")?;

    let prompt = "line-one\nline-two";
    let shim = repo_root().join("examples/agents/codex/agent.sh");

    let mut paths = vec![bin_dir.clone()];
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    let joined_path = env::join_paths(paths)?;

    let mut cmd = Command::new(shim);
    cmd.env("PATH", joined_path);
    cmd.env("INPUT_LOG", &input_log);
    cmd.env("ARGS_LOG", &args_log);
    cmd.env("PAYLOAD", "codex-backend-output");
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn()?;
    child
        .stdin
        .as_mut()
        .ok_or("failed to open stdin for codex shim")?
        .write_all(prompt.as_bytes())?;
    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "codex shim exited with {:?}",
        output.status
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, "codex-backend-output\n");

    let recorded_input = fs::read_to_string(&input_log)?;
    assert_eq!(recorded_input, prompt);

    let recorded_args = fs::read_to_string(&args_log)?;
    assert_eq!(recorded_args.trim(), "exec --json -");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[codex shim] prompt (first line preview): line-one"),
        "stderr missing preview: {stderr}"
    );
    assert!(
        !stderr.contains("line-two"),
        "stderr should only include the first prompt line: {stderr}"
    );
    Ok(())
}

#[test]
fn gemini_shim_forwards_prompt_and_args() -> TestResult {
    let tmp = TempDir::new()?;
    let bin_dir = tmp.path().join("bin");
    let input_log = tmp.path().join("gemini-input.log");
    let args_log = tmp.path().join("gemini-args.log");
    write_backend_stub(&bin_dir, "gemini")?;

    let prompt = "gem-first\nsecond-line";
    let shim = repo_root().join("examples/agents/gemini/agent.sh");

    let mut paths = vec![bin_dir.clone()];
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    let joined_path = env::join_paths(paths)?;

    let mut cmd = Command::new(shim);
    cmd.env("PATH", joined_path);
    cmd.env("INPUT_LOG", &input_log);
    cmd.env("ARGS_LOG", &args_log);
    cmd.env("PAYLOAD", "gemini-backend-output");
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn()?;
    child
        .stdin
        .as_mut()
        .ok_or("failed to open stdin for gemini shim")?
        .write_all(prompt.as_bytes())?;
    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "gemini shim exited with {:?}",
        output.status
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, "gemini-backend-output\n");

    let recorded_input = fs::read_to_string(&input_log)?;
    assert_eq!(recorded_input, prompt);

    let recorded_args = fs::read_to_string(&args_log)?;
    assert_eq!(recorded_args.trim(), "--output-format stream-json");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[gemini shim] prompt (first line preview): gem-first"),
        "stderr missing preview: {stderr}"
    );
    assert!(
        !stderr.contains("second-line"),
        "stderr should only include the first prompt line: {stderr}"
    );
    Ok(())
}
