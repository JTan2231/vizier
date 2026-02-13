use crate::fixtures::*;
use git2::BranchType;
use std::path::Path;

fn config_fixture(name: &str) -> PathBuf {
    repo_root()
        .join("tests")
        .join("fixtures")
        .join("config")
        .join(name)
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
        stdout.contains("Per-command agents:"),
        "plan output should render per-command agent settings:\n{stdout}"
    );
    assert!(
        stdout.contains("Build:"),
        "plan output should render build policy defaults:\n{stdout}"
    );
    assert!(
        compact.contains("Defaultpipeline:approve"),
        "plan output should include build.default_pipeline with default value:\n{stdout}"
    );
    assert!(
        compact.contains("Templatesave:template.save.v1"),
        "plan output should include workflow template defaults:\n{stdout}"
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
fn test_plan_json_surfaces_develop_alias_selector() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo.vizier_output(&["plan", "--json"])?;
    assert!(
        output.status.success(),
        "vizier plan --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        json.pointer("/commands/develop/template_selector")
            .and_then(Value::as_str),
        Some("file:.vizier/develop.toml"),
        "plan JSON should surface repo-local develop alias selector"
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

[workflow.templates]
review = "custom.review.v2"
build_execute = "custom.build@v7"

[build]
default_pipeline = "approve-review-merge"
default_merge_target = "release/main"
stage_barrier = "explicit"
failure_mode = "continue_independent"
default_review_mode = "review_only"
default_skip_checks = true
default_keep_draft_branch = true
default_profile = "integration"

[build.profiles.integration]
pipeline = "approve-review"
merge_target = "build"
review_mode = "apply_fixes"
skip_checks = false
keep_branch = true
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
        Some(false),
        "workflow.background.quiet should appear in the report"
    );
    assert_eq!(
        json.pointer("/workflow/templates/save")
            .and_then(Value::as_str),
        Some("template.save.v1"),
        "workflow.templates.save should appear in the report"
    );
    assert_eq!(
        json.pointer("/workflow/templates/review")
            .and_then(Value::as_str),
        Some("custom.review.v2"),
        "workflow.templates.review should reflect config overrides"
    );
    assert_eq!(
        json.pointer("/workflow/templates/build_execute")
            .and_then(Value::as_str),
        Some("custom.build@v7"),
        "workflow.templates.build_execute should reflect config overrides"
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
        json.pointer("/build/default_pipeline")
            .and_then(Value::as_str),
        Some("approve-review-merge"),
        "build.default_pipeline should appear in the report"
    );
    assert_eq!(
        json.pointer("/build/default_merge_target")
            .and_then(Value::as_str),
        Some("release/main"),
        "build.default_merge_target should appear in the report"
    );
    assert_eq!(
        json.pointer("/build/stage_barrier").and_then(Value::as_str),
        Some("explicit"),
        "build.stage_barrier should appear in the report"
    );
    assert_eq!(
        json.pointer("/build/failure_mode").and_then(Value::as_str),
        Some("continue_independent"),
        "build.failure_mode should appear in the report"
    );
    assert_eq!(
        json.pointer("/build/default_profile")
            .and_then(Value::as_str),
        Some("integration"),
        "build.default_profile should appear in the report"
    );
    assert_eq!(
        json.pointer("/build/profiles/integration/merge_target")
            .and_then(Value::as_str),
        Some("build"),
        "build profile merge target should appear in the report"
    );
    assert_eq!(
        json.pointer("/build/profiles/integration/keep_branch")
            .and_then(Value::as_bool),
        Some(true),
        "build profile keep_branch should appear in the report"
    );
    assert_eq!(
        json.pointer("/commands/save/agent").and_then(Value::as_str),
        Some("gemini"),
        "per-command agent selector should reflect CLI overrides"
    );
    assert_eq!(
        json.pointer("/commands/review/template_selector")
            .and_then(Value::as_str),
        Some("custom.review.v2"),
        "per-command template selector should reflect workflow template overrides"
    );
    Ok(())
}
#[test]
fn test_plan_rejects_removed_agent_command_override() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo
        .vizier_cmd()
        .args(["--agent-command", "mock-agent.sh", "plan"])
        .output()?;
    assert!(
        !output.status.success(),
        "removed --agent-command should fail with migration guidance"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`--agent-command` was removed"),
        "missing removed-flag guidance:\n{stderr}"
    );
    Ok(())
}

#[test]
fn test_plan_default_runtime_uses_agents_default_not_save_scope_override() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let config_path = repo.path().join("custom-runtime-config.toml");
    fs::write(
        &config_path,
        r#"
[agents.default.agent]
label = "default-runtime"
command = ["./default-runtime.sh"]

[agents.save.agent]
label = "save-runtime"
command = ["./save-runtime.sh"]
"#,
    )?;

    let output = repo
        .vizier_cmd_with_config(&config_path)
        .args(["plan", "--json"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier plan --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        json.pointer("/agent_runtime_default/label")
            .and_then(Value::as_str),
        Some("default-runtime"),
        "default runtime should come from [agents.default], not save overrides"
    );
    assert_eq!(
        json.pointer("/commands/save/agent_runtime/label")
            .and_then(Value::as_str),
        Some("save-runtime"),
        "save command should still report save-specific runtime"
    );
    Ok(())
}

#[test]
fn test_plan_json_legacy_scope_fixture_compatibility() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo
        .vizier_cmd_with_config(&config_fixture("legacy-scope-only.toml"))
        .args(["plan", "--json"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier plan --json with legacy fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        json.pointer("/commands/save/legacy_scope")
            .and_then(Value::as_str),
        Some("save"),
        "legacy fixture should expose compatibility scope for save command"
    );
    assert_eq!(
        json.pointer("/commands/save/agent_runtime/label")
            .and_then(Value::as_str),
        Some("legacy-save-runtime"),
        "legacy [agents.save] runtime should still apply via compatibility bridge"
    );
    Ok(())
}

#[test]
fn test_plan_json_alias_template_fixture_resolution() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo
        .vizier_cmd_with_config(&config_fixture("alias-template-only.toml"))
        .args(["plan", "--json"])
        .output()?;
    assert!(
        output.status.success(),
        "vizier plan --json with alias/template fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        json.pointer("/commands/patch/template_selector")
            .and_then(Value::as_str),
        Some("template.patch.ops@v3"),
        "patch alias should resolve through [commands] mapping"
    );
    assert_eq!(
        json.pointer("/commands/patch/agent_runtime/label")
            .and_then(Value::as_str),
        Some("template-patch-runtime"),
        "template runtime overrides should beat alias runtime overrides"
    );
    assert_eq!(
        json.pointer("/commands/patch/legacy_scope")
            .and_then(Value::as_str),
        Some("draft"),
        "patch alias should preserve legacy fallback scope metadata"
    );
    Ok(())
}

#[test]
fn test_plan_json_mixed_precedence_fixture_and_cli_override() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let fixture = config_fixture("mixed-precedence.toml");

    let without_cli = repo
        .vizier_cmd_with_config(&fixture)
        .args(["plan", "--json"])
        .output()?;
    assert!(
        without_cli.status.success(),
        "vizier plan --json with mixed fixture failed: {}",
        String::from_utf8_lossy(&without_cli.stderr)
    );
    let without_cli_json: Value = serde_json::from_slice(&without_cli.stdout)?;
    assert_eq!(
        without_cli_json
            .pointer("/commands/save/agent_runtime/label")
            .and_then(Value::as_str),
        Some("template-save-runtime"),
        "template override should beat alias + legacy + default runtime labels"
    );
    assert_eq!(
        without_cli_json
            .pointer("/commands/release_flow/template_selector")
            .and_then(Value::as_str),
        None,
        "unmapped alias should not require a template selector"
    );
    assert_eq!(
        without_cli_json
            .pointer("/commands/release_flow/agent_runtime/label")
            .and_then(Value::as_str),
        Some("alias-release-runtime"),
        "unmapped alias should still resolve alias-level overrides"
    );

    let with_cli = repo
        .vizier_cmd_with_config(&fixture)
        .args(["--agent", "gemini", "plan", "--json"])
        .output()?;
    assert!(
        with_cli.status.success(),
        "vizier plan --json with mixed fixture + CLI overrides failed: {}",
        String::from_utf8_lossy(&with_cli.stderr)
    );
    let with_cli_json: Value = serde_json::from_slice(&with_cli.stdout)?;
    assert_eq!(
        with_cli_json
            .pointer("/commands/save/agent")
            .and_then(Value::as_str),
        Some("gemini"),
        "CLI agent selector override should beat template/alias/legacy/default"
    );
    assert_eq!(
        with_cli_json
            .pointer("/commands/release_flow/agent")
            .and_then(Value::as_str),
        Some("gemini"),
        "CLI agent selector override should apply to custom aliases too"
    );

    Ok(())
}

#[test]
fn test_plan_docs_require_plan_and_branch_front_matter() -> TestResult {
    let repo = IntegrationRepo::new()?;

    let output = repo.vizier_output(&[
        "draft",
        "--name",
        "plan-front-matter",
        "verify plan front matter contract",
    ])?;
    assert!(
        output.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let repo_handle = repo.repo();
    let branch = repo_handle.find_branch("draft/plan-front-matter", BranchType::Local)?;
    let commit = branch.get().peel_to_commit()?;
    let tree = commit.tree()?;
    let entry = tree.get_path(Path::new(
        ".vizier/implementation-plans/plan-front-matter.md",
    ))?;
    let blob = repo_handle.find_blob(entry.id())?;
    let contents = std::str::from_utf8(blob.content())?;

    assert!(
        contents.starts_with("---\nplan_id: ")
            && contents
                .contains("\nplan: plan-front-matter\nbranch: draft/plan-front-matter\n---\n"),
        "plan front matter should include `plan_id`, `plan`, and `branch` keys:\n{contents}"
    );
    assert!(
        contents.contains("## Operator Spec"),
        "plan doc should include an Operator Spec section"
    );
    assert!(
        contents.contains("## Implementation Plan"),
        "plan doc should include an Implementation Plan section"
    );

    Ok(())
}
