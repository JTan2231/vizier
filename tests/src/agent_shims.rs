use crate::fixtures::*;

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
fn codex_shim_forwards_prompt_and_args() -> TestResult {
    let _guard = integration_test_lock().lock();
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
    let _guard = integration_test_lock().lock();
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
