#[cfg(test)]
mod tests {
    use crate::config::driver;
    use crate::config::*;
    use lazy_static::lazy_static;
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use tempfile::{NamedTempFile, tempdir};

    lazy_static! {
        static ref AGENT_SHIM_ENV_LOCK: Mutex<()> = Mutex::new(());
    }
    static CWD_LOCK: Mutex<()> = Mutex::new(());

    struct CwdGuard {
        original: PathBuf,
    }

    impl CwdGuard {
        fn enter(path: &Path) -> Self {
            let original = std::env::current_dir().expect("read current dir");
            std::env::set_current_dir(path).expect("set current dir");
            Self { original }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.original);
        }
    }

    fn write_json_file(contents: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("failed to create temp file");
        file.write_all(contents.as_bytes())
            .expect("failed to write temp file");
        file
    }

    #[test]
    fn prompt_profile_overrides_repo_prompt() {
        let _guard = CWD_LOCK.lock().unwrap();
        let temp_dir = tempdir().expect("create temp dir");
        let vizier_dir = temp_dir.path().join(".vizier");
        fs::create_dir_all(&vizier_dir).expect("create .vizier");
        fs::write(
            vizier_dir.join("DOCUMENTATION_PROMPT.md"),
            "repo documentation prompt",
        )
        .expect("write repo prompt");

        let config_path = temp_dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
[agents.default.prompts.documentation]
text = "profile documentation prompt"
"#,
        )
        .expect("write config");

        let _cwd = CwdGuard::enter(temp_dir.path());
        let cfg = load_config_from_toml(config_path).expect("parse config");
        let selection = cfg.prompt_for(CommandScope::Save, PromptKind::Documentation);
        assert_eq!(selection.text, "profile documentation prompt");
    }

    #[test]
    fn config_rejects_plan_refine_prompt_kind() {
        let toml = r#"
[agents.default.prompts.plan_refine]
text = "nope"
"#;
        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let err = match load_config_from_toml(file.path().to_path_buf()) {
            Ok(_) => panic!("plan_refine prompt kind should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string()
                .contains("unknown prompt kind `plan_refine`"),
            "error message should mention unknown prompt kind: {err}"
        );
    }

    #[test]
    fn config_rejects_refine_scope() {
        let toml = r#"
[agents.refine]
agent = "codex"
"#;
        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let err = match load_config_from_toml(file.path().to_path_buf()) {
            Ok(_) => panic!("refine scope should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("unknown [agents.refine] section"),
            "error message should mention unknown scope: {err}"
        );
    }

    #[test]
    fn config_rejects_removed_ask_scope() {
        let toml = r#"
[agents.ask]
agent = "codex"
"#;
        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let err = match load_config_from_toml(file.path().to_path_buf()) {
            Ok(_) => panic!("ask scope should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("unknown [agents.ask] section"),
            "error message should mention removed ask scope: {err}"
        );
    }

    #[test]
    fn documentation_settings_follow_scope_overrides() {
        let toml = r#"
[agents.default.documentation]
enabled = false
include_snapshot = false
include_narrative_docs = false

[agents.save.documentation]
enabled = true
include_snapshot = true
include_narrative_docs = true
"#;

        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let mut cfg =
            load_config_from_toml(file.path().to_path_buf()).expect("should parse TOML config");
        cfg.agent_runtime.command = vec!["/bin/echo".to_string()];
        cfg.agent_runtime.label = Some("doc-agent".to_string());

        let save_settings =
            resolve_prompt_profile(&cfg, CommandScope::Save, PromptKind::Documentation, None)
                .expect("resolve save settings");
        assert!(save_settings.documentation.use_documentation_prompt);
        assert!(save_settings.documentation.include_snapshot);
        assert!(save_settings.documentation.include_narrative_docs);
        assert!(save_settings.prompt_selection().is_some());

        let draft_settings =
            resolve_prompt_profile(&cfg, CommandScope::Draft, PromptKind::Documentation, None)
                .expect("resolve draft settings");
        assert!(!draft_settings.documentation.use_documentation_prompt);
        assert!(!draft_settings.documentation.include_snapshot);
        assert!(!draft_settings.documentation.include_narrative_docs);
        assert!(draft_settings.prompt_selection().is_none());
    }

    #[test]
    fn test_from_json_invalid_file() {
        let file = write_json_file("{ this is not valid json ");
        let result = load_config_from_json(file.path().to_path_buf());
        assert!(result.is_err(), "expected error for invalid JSON");
    }

    #[test]
    fn test_from_json_missing_file() {
        let path = std::path::PathBuf::from("does_not_exist.json");
        let result = load_config_from_json(path);
        assert!(result.is_err(), "expected error for missing file");
    }

    #[test]
    fn config_rejects_model_and_reasoning_keys() {
        let json = r#"{ "model": "gpt-5", "reasoning_effort": "medium" }"#;
        let file = write_json_file(json);

        let cfg = load_config_from_json(file.path().to_path_buf());
        assert!(
            cfg.is_err(),
            "model/reasoning keys should be rejected after wire removal"
        );
    }

    #[test]
    fn test_fallback_backend_rejected_in_root_config() {
        let toml = r#"
agent = "codex"
fallback_backend = "wire"
"#;
        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let err = match load_config_from_toml(file.path().to_path_buf()) {
            Ok(_) => panic!("fallback_backend should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string()
                .contains("fallback_backend entries are unsupported"),
            "error message should mention fallback_backend rejection: {err}"
        );
    }

    #[test]
    fn test_fallback_backend_rejected_in_agent_scope() {
        let toml = r#"
[agents.save]
agent = "codex"
fallback_backend = "codex"
"#;
        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let err = match load_config_from_toml(file.path().to_path_buf()) {
            Ok(_) => panic!("fallback_backend in agents.* should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string()
                .contains("fallback_backend entries are unsupported"),
            "error message should mention fallback_backend rejection: {err}"
        );
    }

    #[test]
    fn test_backend_key_rejected_in_root_config() {
        let toml = r#"
agent = "codex"
backend = "gemini"
"#;
        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let err = match load_config_from_toml(file.path().to_path_buf()) {
            Ok(_) => panic!("backend should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("backend entries are unsupported"),
            "error message should mention backend rejection: {err}"
        );
    }

    #[test]
    fn test_backend_key_rejected_in_agent_scope() {
        let toml = r#"
[agents.save]
backend = "gemini"
"#;
        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let err = match load_config_from_toml(file.path().to_path_buf()) {
            Ok(_) => panic!("backend in agents.* should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("backend entries are unsupported"),
            "error message should mention backend rejection: {err}"
        );
    }

    #[test]
    fn test_review_checks_table() {
        let toml = r#"
[review.checks]
commands = ["npm test", "cargo fmt -- --check"]
"#;
        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes()).unwrap();
        let cfg = load_config_from_toml(file.path().to_path_buf()).expect("parse review config");
        assert_eq!(
            cfg.review.checks.commands,
            vec!["npm test", "cargo fmt -- --check"]
        );
    }

    #[test]
    fn test_merge_cicd_gate_config_from_toml() {
        let toml = r#"
[merge.cicd_gate]
script = "./scripts/run-ci.sh"
auto_resolve = true
retries = 3
"#;
        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes()).unwrap();
        let cfg = load_config_from_toml(file.path().to_path_buf()).expect("parse merge config");
        assert_eq!(
            cfg.merge.cicd_gate.script,
            Some(PathBuf::from("./scripts/run-ci.sh"))
        );
        assert!(cfg.merge.cicd_gate.auto_resolve);
        assert_eq!(cfg.merge.cicd_gate.retries, 3);
    }

    #[test]
    fn test_merge_cicd_gate_config_from_json_aliases() {
        let json = r#"
        {
            "merge": {
                "cicd_gate": {
                    "script": "./ci/run.sh",
                    "auto-fix": "false",
                    "max_attempts": "5"
                }
            }
        }
        "#;
        let file = write_json_file(json);
        let cfg = load_config_from_json(file.path().to_path_buf()).expect("parse merge config");
        assert_eq!(
            cfg.merge.cicd_gate.script,
            Some(PathBuf::from("./ci/run.sh"))
        );
        assert!(!cfg.merge.cicd_gate.auto_resolve);
        assert_eq!(cfg.merge.cicd_gate.retries, 5);
    }

    #[test]
    fn test_approve_stop_condition_defaults() {
        let cfg = Config::default();
        assert_eq!(cfg.approve.stop_condition.script, None);
        assert_eq!(cfg.approve.stop_condition.retries, 3);
    }

    #[test]
    fn test_approve_stop_condition_config_from_toml() {
        let toml = r#"
[approve.stop_condition]
script = "./scripts/approve-stop.sh"
retries = 5
"#;
        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes()).unwrap();
        let cfg = load_config_from_toml(file.path().to_path_buf())
            .expect("parse approve stop-condition config");
        assert_eq!(
            cfg.approve.stop_condition.script,
            Some(PathBuf::from("./scripts/approve-stop.sh"))
        );
        assert_eq!(cfg.approve.stop_condition.retries, 5);
    }

    #[test]
    fn test_approve_stop_condition_config_from_json() {
        let json = r#"
        {
            "approve": {
                "stop_condition": {
                    "script": "./scripts/approve-check.sh",
                    "max_attempts": "4"
                }
            }
        }
        "#;
        let file = write_json_file(json);
        let cfg = load_config_from_json(file.path().to_path_buf())
            .expect("parse approve stop-condition config");
        assert_eq!(
            cfg.approve.stop_condition.script,
            Some(PathBuf::from("./scripts/approve-check.sh"))
        );
        assert_eq!(cfg.approve.stop_condition.retries, 4);
    }

    #[test]
    fn config_parses_commit_metadata_overrides() {
        let toml = r#"
[commits.meta]
enabled = false
style = "trailers"
include = ["session_id"]
session_log_path = "none"

[commits.meta.labels]
session_id = "Vizier-Session"

[commits.fallback_subjects]
code_change = "CUSTOM CODE"

[commits.implementation]
subject = "chore: apply {slug}"
fields = ["Summary"]

[commits.merge]
subject = "chore: merge {slug}"
include_operator_note = false
operator_note_label = "Note"
plan_mode = "summary"
plan_label = "Plan Summary"
"#;
        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes()).unwrap();
        let cfg = load_config_from_toml(file.path().to_path_buf()).expect("parse commit config");
        assert!(!cfg.commits.meta.enabled);
        assert_eq!(cfg.commits.meta.style, CommitMetaStyle::Trailers);
        assert_eq!(cfg.commits.meta.include, vec![CommitMetaField::SessionId]);
        assert_eq!(
            cfg.commits.meta.session_log_path,
            CommitSessionLogPath::None
        );
        assert_eq!(cfg.commits.meta.labels.session_id, "Vizier-Session");
        assert_eq!(cfg.commits.fallback_subjects.code_change, "CUSTOM CODE");
        assert_eq!(cfg.commits.implementation.subject, "chore: apply {slug}");
        assert_eq!(
            cfg.commits.implementation.fields,
            vec![CommitImplementationField::Summary]
        );
        assert_eq!(cfg.commits.merge.subject, "chore: merge {slug}");
        assert!(!cfg.commits.merge.include_operator_note);
        assert_eq!(cfg.commits.merge.operator_note_label, "Note");
        assert_eq!(cfg.commits.merge.plan_mode, CommitMergePlanMode::Summary);
        assert_eq!(cfg.commits.merge.plan_label, "Plan Summary");
    }

    #[test]
    fn config_parses_display_list_settings() {
        let toml = r#"
[display.lists.list]
format = "table"
header_fields = ["Outcome"]
entry_fields = ["Plan", "Summary"]
job_fields = []
command_fields = []
summary_max_len = 42
summary_single_line = false

[display.lists.jobs]
format = "json"
show_succeeded = true
fields = ["Job", "Status"]

[display.lists.jobs_show]
format = "table"
fields = ["Job", "Status", "Command"]
"#;
        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes()).unwrap();
        let cfg = load_config_from_toml(file.path().to_path_buf()).expect("parse display config");
        assert_eq!(cfg.display.lists.list.format, ListFormat::Table);
        assert_eq!(cfg.display.lists.list.header_fields, vec!["Outcome"]);
        assert_eq!(cfg.display.lists.list.entry_fields, vec!["Plan", "Summary"]);
        assert_eq!(cfg.display.lists.list.job_fields.len(), 0);
        assert_eq!(cfg.display.lists.list.command_fields.len(), 0);
        assert_eq!(cfg.display.lists.list.summary_max_len, 42);
        assert!(!cfg.display.lists.list.summary_single_line);
        assert_eq!(cfg.display.lists.jobs.format, ListFormat::Json);
        assert!(cfg.display.lists.jobs.show_succeeded);
        assert_eq!(cfg.display.lists.jobs.fields, vec!["Job", "Status"]);
        assert_eq!(cfg.display.lists.jobs_show.format, ListFormat::Table);
        assert_eq!(
            cfg.display.lists.jobs_show.fields,
            vec!["Job", "Status", "Command"]
        );
    }

    #[test]
    fn test_merge_conflict_auto_resolve_from_toml() {
        let toml = r#"
[merge.conflicts]
auto_resolve = true
"#;
        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes()).unwrap();
        let cfg =
            load_config_from_toml(file.path().to_path_buf()).expect("parse merge conflict config");
        assert!(
            cfg.merge.conflicts.auto_resolve,
            "conflict auto-resolve should parse from toml"
        );
    }

    #[test]
    fn test_build_config_from_toml_with_profiles() {
        let toml = r#"
[build]
default_pipeline = "approve-review"
default_merge_target = "release/main"
stage_barrier = "explicit"
failure_mode = "continue_independent"
default_review_mode = "review_only"
default_skip_checks = true
default_keep_draft_branch = true
default_profile = "integration"

[build.profiles.integration]
pipeline = "approve-review-merge"
merge_target = "build"
review_mode = "apply_fixes"
skip_checks = false
keep_branch = true
"#;
        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes()).unwrap();
        let cfg = load_config_from_toml(file.path().to_path_buf()).expect("parse build config");

        assert_eq!(cfg.build.default_pipeline, BuildPipeline::ApproveReview);
        assert_eq!(
            cfg.build.default_merge_target,
            BuildMergeTarget::Branch("release/main".to_string())
        );
        assert_eq!(cfg.build.stage_barrier, BuildStageBarrier::Explicit);
        assert_eq!(
            cfg.build.failure_mode,
            BuildFailureMode::ContinueIndependent
        );
        assert_eq!(cfg.build.default_review_mode, BuildReviewMode::ReviewOnly);
        assert!(cfg.build.default_skip_checks);
        assert!(cfg.build.default_keep_draft_branch);
        assert_eq!(cfg.build.default_profile.as_deref(), Some("integration"));

        let profile = cfg
            .build
            .profiles
            .get("integration")
            .expect("integration build profile");
        assert_eq!(profile.pipeline, Some(BuildPipeline::ApproveReviewMerge));
        assert_eq!(profile.merge_target, Some(BuildMergeTarget::Build));
        assert_eq!(profile.review_mode, Some(BuildReviewMode::ApplyFixes));
        assert_eq!(profile.skip_checks, Some(false));
        assert_eq!(profile.keep_branch, Some(true));
    }

    #[test]
    fn test_build_config_from_json_aliases() {
        let json = r#"
{
  "build": {
    "default-pipeline": "approve-review-merge",
    "default-merge-target": "primary",
    "stage-barrier": "strict",
    "failure-mode": "block_downstream",
    "default-review-mode": "review-file",
    "default-skip-checks": "false",
    "default-keep-draft-branch": "false",
    "default-profile": "qa",
    "profiles": {
      "qa": {
        "pipeline": "approve-review",
        "merge-target": "release/qa",
        "review-mode": "review_only",
        "skip-checks": "true",
        "keep-branch": "false"
      }
    }
  }
}
"#;

        let file = write_json_file(json);
        let cfg = load_config_from_json(file.path().to_path_buf()).expect("parse build config");

        assert_eq!(
            cfg.build.default_pipeline,
            BuildPipeline::ApproveReviewMerge
        );
        assert_eq!(cfg.build.default_merge_target, BuildMergeTarget::Primary);
        assert_eq!(cfg.build.stage_barrier, BuildStageBarrier::Strict);
        assert_eq!(cfg.build.failure_mode, BuildFailureMode::BlockDownstream);
        assert_eq!(cfg.build.default_review_mode, BuildReviewMode::ReviewFile);
        assert!(!cfg.build.default_skip_checks);
        assert!(!cfg.build.default_keep_draft_branch);
        assert_eq!(cfg.build.default_profile.as_deref(), Some("qa"));

        let profile = cfg.build.profiles.get("qa").expect("qa build profile");
        assert_eq!(profile.pipeline, Some(BuildPipeline::ApproveReview));
        assert_eq!(
            profile.merge_target,
            Some(BuildMergeTarget::Branch("release/qa".to_string()))
        );
        assert_eq!(profile.review_mode, Some(BuildReviewMode::ReviewOnly));
        assert_eq!(profile.skip_checks, Some(true));
        assert_eq!(profile.keep_branch, Some(false));
    }

    #[test]
    fn layered_config_merges_global_and_repo_overrides() {
        let temp_dir = tempdir().expect("create temp dir");
        let global_path = temp_dir.path().join("global.toml");
        fs::write(
            &global_path,
            r#"
agent = "codex"

[approve.stop_condition]
script = "./scripts/global-approve-stop.sh"
retries = 5

[merge.cicd_gate]
script = "./scripts/global-ci.sh"
retries = 4

[merge.conflicts]
auto_resolve = false

[review.checks]
commands = ["echo global"]

[build]
default_pipeline = "approve-review"
default_merge_target = "release/global"
default_review_mode = "review_only"
default_skip_checks = false
default_keep_draft_branch = false
stage_barrier = "strict"
failure_mode = "block_downstream"

[build.profiles.release]
pipeline = "approve-review-merge"
merge_target = "primary"
review_mode = "apply_fixes"
skip_checks = false
keep_branch = false
"#,
        )
        .expect("write global config");

        let repo_path = temp_dir.path().join("repo.toml");
        fs::write(
            &repo_path,
            r#"
[agents.default]
agent = "gemini"

[approve.stop_condition]
retries = 2

[merge.cicd_gate]
auto_resolve = true

[merge.conflicts]
auto_resolve = true

[build]
default_skip_checks = true
default_keep_draft_branch = true
default_profile = "release"

[build.profiles.release]
keep_branch = true
"#,
        )
        .expect("write repo config");

        let cfg = Config::from_layers(&[
            load_config_layer_from_toml(global_path).expect("global layer"),
            load_config_layer_from_toml(repo_path).expect("repo layer"),
        ]);

        assert_eq!(
            cfg.agent_defaults.selector.as_deref(),
            Some("gemini"),
            "repo agent selector should win over global defaults"
        );
        assert_eq!(
            cfg.merge.cicd_gate.script,
            Some(PathBuf::from("./scripts/global-ci.sh")),
            "global merge script should be preserved when repo omits it"
        );
        assert!(
            cfg.merge.cicd_gate.auto_resolve,
            "repo boolean override should apply"
        );
        assert_eq!(
            cfg.merge.cicd_gate.retries, 4,
            "numeric config should fall back to the global layer when repo omits it"
        );
        assert_eq!(
            cfg.approve.stop_condition.script,
            Some(PathBuf::from("./scripts/global-approve-stop.sh")),
            "global approve.stop_condition.script should be preserved when repo omits it"
        );
        assert_eq!(
            cfg.approve.stop_condition.retries, 2,
            "repo approve.stop_condition.retries should override global default"
        );
        assert!(
            cfg.merge.conflicts.auto_resolve,
            "repo conflict auto-resolve should override global default"
        );
        assert_eq!(
            cfg.review.checks.commands,
            vec!["echo global"],
            "global review checks should populate when repo config omits them"
        );
        assert_eq!(
            cfg.build.default_pipeline,
            BuildPipeline::ApproveReview,
            "global build pipeline should persist when repo omits it"
        );
        assert_eq!(
            cfg.build.default_merge_target,
            BuildMergeTarget::Branch("release/global".to_string()),
            "global build merge target should persist when repo omits it"
        );
        assert!(
            cfg.build.default_skip_checks,
            "repo build default_skip_checks should override global value"
        );
        assert!(
            cfg.build.default_keep_draft_branch,
            "repo build keep-branch default should override global value"
        );
        assert_eq!(
            cfg.build.default_profile.as_deref(),
            Some("release"),
            "repo build default_profile should apply"
        );
        let release_profile = cfg
            .build
            .profiles
            .get("release")
            .expect("merged release profile should exist");
        assert_eq!(
            release_profile.pipeline,
            Some(BuildPipeline::ApproveReviewMerge),
            "global profile pipeline should persist"
        );
        assert_eq!(
            release_profile.keep_branch,
            Some(true),
            "repo profile field should override global profile field"
        );
    }

    #[test]
    fn test_project_config_path_prefers_toml_over_json() {
        let temp_dir = tempdir().expect("create temp dir");
        assert!(
            project_config_path(temp_dir.path()).is_none(),
            "no config files should return None"
        );

        let vizier_dir = temp_dir.path().join(".vizier");
        fs::create_dir_all(&vizier_dir).expect("make .vizier dir");
        let json_path = vizier_dir.join("config.json");
        fs::write(&json_path, "{}").expect("write json config");
        assert_eq!(
            project_config_path(temp_dir.path()).expect("json config should be detected"),
            json_path
        );

        let toml_path = vizier_dir.join("config.toml");
        fs::write(&toml_path, "agent = \"codex\"").expect("write toml config");
        assert_eq!(
            project_config_path(temp_dir.path()).expect("toml config should override json"),
            toml_path
        );
    }

    #[test]
    fn test_env_config_path_trims_blank_values() {
        const KEY: &str = "VIZIER_CONFIG_FILE";
        let original = std::env::var(KEY).ok();

        unsafe {
            std::env::set_var(KEY, "   ");
        }
        assert!(
            env_config_path().is_none(),
            "blank env var should be ignored"
        );

        unsafe {
            std::env::set_var(KEY, "/tmp/custom-config.toml");
        }
        assert_eq!(
            env_config_path(),
            Some(PathBuf::from("/tmp/custom-config.toml")),
            "non-blank env var should be returned"
        );

        match original {
            Some(value) => unsafe {
                std::env::set_var(KEY, value);
            },
            None => unsafe {
                std::env::remove_var(KEY);
            },
        }
    }

    #[test]
    fn test_agent_prompt_override_with_path_and_backend() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let prompt_path = temp_dir.path().join("profile_documentation.md");
        fs::write(&prompt_path, "scoped prompt from file").expect("write prompt file");

        let config_path = temp_dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
[agents.default.prompts.documentation]
path = "profile_documentation.md"
agent = "gemini"
"#,
        )
        .expect("write config");

        let cfg =
            load_config_from_toml(config_path).expect("should parse config with prompt overrides");
        let selection = cfg.prompt_for(CommandScope::Save, PromptKind::Documentation);
        assert_eq!(selection.text.trim(), "scoped prompt from file");
        assert_eq!(selection.source_path, Some(prompt_path.clone()));

        let agent =
            resolve_prompt_profile(&cfg, CommandScope::Save, PromptKind::Documentation, None)
                .expect("resolve prompt profile");
        assert_eq!(
            agent
                .prompt
                .as_ref()
                .expect("prompt should be attached")
                .text
                .trim(),
            "scoped prompt from file"
        );
        assert_eq!(agent.backend, BackendKind::Gemini);
    }

    #[test]
    fn agent_command_accepts_command_tokens() {
        let toml = r#"
[agent]
command = ["./bin/codex", "exec", "--local"]
"#;

        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let cfg =
            load_config_from_toml(file.path().to_path_buf()).expect("should parse agent command");
        assert_eq!(
            cfg.agent_runtime.command,
            vec![
                "./bin/codex".to_string(),
                "exec".to_string(),
                "--local".to_string()
            ]
        );
    }

    #[test]
    fn agent_label_parses_from_config() {
        let toml = r#"
[agent]
label = "gemini"
"#;

        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let cfg =
            load_config_from_toml(file.path().to_path_buf()).expect("should parse agent label");
        assert_eq!(cfg.agent_runtime.label.as_deref(), Some("gemini"));
    }

    #[test]
    fn legacy_agent_runtime_keys_error() {
        let toml = r#"
[agent]
profile = "deprecated"
"#;

        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        match load_config_from_toml(file.path().to_path_buf()) {
            Ok(_) => panic!("legacy agent keys should be rejected"),
            Err(err) => assert!(
                err.to_string()
                    .contains("agent runtime supports only label, command, progress_filter, output, and enable_script_wrapper"),
                "unexpected error: {err}"
            ),
        }
    }

    #[test]
    fn resolve_runtime_prefers_bundled_shim_dir_env() {
        let _guard = AGENT_SHIM_ENV_LOCK.lock().unwrap();
        let temp_dir = tempdir().expect("create temp dir");
        let shim_dir = temp_dir.path().join("codex");
        fs::create_dir_all(&shim_dir).expect("create shim dir");
        let shim_path = shim_dir.join("agent.sh");
        fs::write(&shim_path, "#!/bin/sh\n").expect("write shim");

        let original = std::env::var("VIZIER_AGENT_SHIMS_DIR").ok();
        unsafe {
            std::env::set_var("VIZIER_AGENT_SHIMS_DIR", temp_dir.path());
        }

        let runtime = AgentRuntimeOptions::default();
        let resolved = driver::resolve_agent_runtime(runtime, "codex", BackendKind::Agent)
            .expect("bundled shim should resolve from env");

        match original {
            Some(value) => unsafe {
                std::env::set_var("VIZIER_AGENT_SHIMS_DIR", value);
            },
            None => unsafe {
                std::env::remove_var("VIZIER_AGENT_SHIMS_DIR");
            },
        }

        assert_eq!(resolved.label, "codex");
        assert_eq!(resolved.command, vec![shim_path.display().to_string()]);
        assert!(matches!(
            resolved.resolution,
            AgentRuntimeResolution::BundledShim { .. }
        ));
    }

    #[test]
    fn resolve_runtime_uses_provided_command() {
        let runtime = AgentRuntimeOptions {
            label: Some("custom".to_string()),
            command: vec!["/opt/custom-agent".to_string(), "--flag".to_string()],
            ..Default::default()
        };

        let resolved = driver::resolve_agent_runtime(runtime, "codex", BackendKind::Agent)
            .expect("explicit command should resolve");
        assert_eq!(resolved.label, "custom");
        assert_eq!(
            resolved.command,
            vec!["/opt/custom-agent".to_string(), "--flag".to_string()]
        );
        assert!(matches!(
            resolved.resolution,
            AgentRuntimeResolution::ProvidedCommand
        ));
    }

    #[test]
    fn default_codex_runtime_wraps_and_sets_progress_filter() {
        let cfg = Config::default();
        let agent = resolve_agent_settings(&cfg, CommandScope::Save, None)
            .expect("default agent settings should resolve");
        assert_eq!(agent.agent_runtime.output, AgentOutputHandling::Wrapped);
        assert!(
            agent.agent_runtime.progress_filter.is_some(),
            "default codex runtime should pick a progress filter"
        );
    }

    #[test]
    fn default_gemini_runtime_sets_progress_filter() {
        let mut cfg = Config::default();
        cfg.agent_selector = "gemini".to_string();
        cfg.backend = backend_kind_for_selector(&cfg.agent_selector);

        let agent = resolve_agent_settings(&cfg, CommandScope::Save, None)
            .expect("default gemini settings should resolve");
        assert_eq!(agent.agent_runtime.output, AgentOutputHandling::Wrapped);
        assert!(
            agent.agent_runtime.progress_filter.is_some(),
            "default gemini runtime should pick a progress filter"
        );
    }

    #[test]
    fn bundled_progress_filter_applies_to_custom_label() {
        let _guard = AGENT_SHIM_ENV_LOCK.lock().unwrap();
        let temp_dir = tempdir().expect("create temp dir");
        let shim_dir = temp_dir.path().join("custom");
        fs::create_dir_all(&shim_dir).expect("create shim dir");
        let agent_path = shim_dir.join("agent.sh");
        fs::write(&agent_path, "#!/bin/sh\n").expect("write agent shim");
        let filter_path = shim_dir.join("filter.sh");
        fs::write(&filter_path, "#!/bin/sh\n").expect("write filter shim");

        let original = std::env::var("VIZIER_AGENT_SHIMS_DIR").ok();
        unsafe {
            std::env::set_var("VIZIER_AGENT_SHIMS_DIR", temp_dir.path());
        }

        let mut cfg = Config::default();
        cfg.agent_runtime.label = Some("custom".to_string());

        let agent = resolve_agent_settings(&cfg, CommandScope::Save, None)
            .expect("custom agent settings should resolve");

        match original {
            Some(value) => unsafe {
                std::env::set_var("VIZIER_AGENT_SHIMS_DIR", value);
            },
            None => unsafe {
                std::env::remove_var("VIZIER_AGENT_SHIMS_DIR");
            },
        }

        assert_eq!(
            agent.agent_runtime.command,
            vec![agent_path.display().to_string()],
            "custom label should reuse the bundled shim"
        );
        assert_eq!(
            agent.agent_runtime.progress_filter,
            Some(vec![filter_path.display().to_string()]),
            "custom label should pick up a bundled progress filter when unset"
        );
    }

    #[test]
    fn progress_filter_override_enables_wrapped_output() {
        let mut cfg = Config::default();
        cfg.agent_runtime.command = vec!["/opt/custom-agent".to_string()];
        cfg.agent_runtime.progress_filter = Some(vec!["/usr/bin/cat".to_string()]);

        let agent = resolve_agent_settings(&cfg, CommandScope::Save, None)
            .expect("agent with filter should resolve");
        assert_eq!(agent.agent_runtime.output, AgentOutputHandling::Wrapped);
        assert_eq!(
            agent.agent_runtime.progress_filter,
            Some(vec!["/usr/bin/cat".to_string()])
        );
    }

    #[test]
    fn agent_command_precedence_prefers_cli_then_scope_then_default() {
        let mut cfg = Config::default();
        cfg.agent_runtime.command = vec!["base-cmd".to_string()];

        let defaults = AgentOverrides {
            agent_runtime: Some(AgentRuntimeOverride {
                label: Some("default".to_string()),
                command: Some(vec!["default-cmd".to_string()]),
                progress_filter: None,
                output: None,
                enable_script_wrapper: None,
            }),
            ..Default::default()
        };
        cfg.agent_defaults = defaults;

        let scoped = AgentOverrides {
            agent_runtime: Some(AgentRuntimeOverride {
                label: Some("scoped".to_string()),
                command: Some(vec!["scoped-cmd".to_string()]),
                progress_filter: None,
                output: None,
                enable_script_wrapper: None,
            }),
            ..Default::default()
        };
        cfg.agent_scopes.insert(CommandScope::Save, scoped);

        let save = resolve_agent_settings(&cfg, CommandScope::Save, None)
            .expect("save scope should resolve");
        assert_eq!(
            save.agent_runtime.command,
            vec!["scoped-cmd".to_string()],
            "scoped command should override defaults and base config"
        );
        assert_eq!(save.agent_runtime.label, "scoped");

        let draft = resolve_agent_settings(&cfg, CommandScope::Draft, None)
            .expect("draft scope should resolve");
        assert_eq!(
            draft.agent_runtime.command,
            vec!["default-cmd".to_string()],
            "default agent override should replace base command for other scopes"
        );
        assert_eq!(draft.agent_runtime.label, "default");

        let cli_override = AgentOverrides {
            agent_runtime: Some(AgentRuntimeOverride {
                label: Some("cli".to_string()),
                command: Some(vec!["cli-cmd".to_string(), "--flag".to_string()]),
                progress_filter: None,
                output: None,
                enable_script_wrapper: None,
            }),
            ..Default::default()
        };

        let save_with_cli = resolve_agent_settings(&cfg, CommandScope::Save, Some(&cli_override))
            .expect("cli override should resolve");
        assert_eq!(
            save_with_cli.agent_runtime.command,
            vec!["cli-cmd".to_string(), "--flag".to_string()],
            "CLI command should take precedence over scoped/default commands"
        );
        assert_eq!(save_with_cli.agent_runtime.label, "cli");
    }

    #[test]
    fn resolve_default_agent_settings_ignores_command_tables() {
        let mut cfg = Config::default();
        cfg.agent_defaults.agent_runtime = Some(AgentRuntimeOverride {
            label: Some("default".to_string()),
            command: Some(vec!["default-cmd".to_string()]),
            progress_filter: None,
            output: None,
            enable_script_wrapper: None,
        });
        cfg.agent_scopes.insert(
            CommandScope::Save,
            AgentOverrides {
                agent_runtime: Some(AgentRuntimeOverride {
                    label: Some("ask".to_string()),
                    command: Some(vec!["ask-cmd".to_string()]),
                    progress_filter: None,
                    output: None,
                    enable_script_wrapper: None,
                }),
                ..Default::default()
            },
        );

        let default = resolve_default_agent_settings(&cfg, None).expect("resolve default scope");
        assert_eq!(default.profile_scope, ProfileScope::Default);
        assert_eq!(default.scope, None);
        assert_eq!(default.agent_runtime.label, "default");
        assert_eq!(
            default.agent_runtime.command,
            vec!["default-cmd".to_string()]
        );

        let ask = resolve_agent_settings(&cfg, CommandScope::Save, None).expect("resolve ask");
        assert_eq!(ask.profile_scope, ProfileScope::Command(CommandScope::Save));
        assert_eq!(ask.scope, Some(CommandScope::Save));
        assert_eq!(ask.agent_runtime.label, "ask");
        assert_eq!(ask.agent_runtime.command, vec!["ask-cmd".to_string()]);
    }

    #[test]
    fn resolve_default_prompt_profile_ignores_command_prompt_overrides() {
        let mut cfg = Config::default();
        cfg.agent_defaults.prompt_overrides.insert(
            PromptKind::Documentation,
            PromptOverrides {
                text: Some("default-doc".to_string()),
                source_path: None,
                agent: None,
            },
        );
        cfg.agent_scopes.insert(
            CommandScope::Save,
            AgentOverrides {
                prompt_overrides: {
                    let mut overrides = std::collections::HashMap::new();
                    overrides.insert(
                        PromptKind::Documentation,
                        PromptOverrides {
                            text: Some("ask-doc".to_string()),
                            source_path: None,
                            agent: None,
                        },
                    );
                    overrides
                },
                ..Default::default()
            },
        );

        let default = resolve_default_prompt_profile(&cfg, PromptKind::Documentation, None)
            .expect("resolve default documentation profile");
        assert_eq!(default.profile_scope, ProfileScope::Default);
        assert_eq!(default.scope, None);
        assert_eq!(
            default.prompt.as_ref().expect("default prompt").text.trim(),
            "default-doc"
        );

        let ask = resolve_prompt_profile(&cfg, CommandScope::Save, PromptKind::Documentation, None)
            .expect("resolve ask documentation profile");
        assert_eq!(ask.profile_scope, ProfileScope::Command(CommandScope::Save));
        assert_eq!(ask.scope, Some(CommandScope::Save));
        assert_eq!(
            ask.prompt.as_ref().expect("ask prompt").text.trim(),
            "ask-doc"
        );
    }

    #[test]
    fn config_parses_command_alias_and_template_tables() {
        let toml = r#"
[commands]
patch = "template.patch.custom@v2"

[agents.commands.patch]
agent = "gemini"

[agents.templates."template.patch.custom@v2".prompts.implementation_plan]
text = "template scoped prompt"
"#;

        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let cfg = load_config_from_toml(file.path().to_path_buf()).expect("parse command tables");
        let patch_alias = "patch".parse::<CommandAlias>().expect("parse patch alias");
        let selector = cfg
            .template_selector_for_alias(&patch_alias)
            .expect("resolve patch selector");
        assert_eq!(selector.as_str(), "template.patch.custom@v2");
        assert!(
            cfg.agent_commands.contains_key(&patch_alias),
            "expected [agents.commands.patch] to parse"
        );
        assert!(
            cfg.agent_templates.contains_key(
                &"template.patch.custom@v2"
                    .parse::<TemplateSelector>()
                    .unwrap()
            ),
            "expected [agents.templates.\"template.patch.custom@v2\"] to parse"
        );
    }

    #[test]
    fn config_rejects_empty_command_selector() {
        let toml = r#"
[commands]
save = "   "
"#;

        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let err = match load_config_from_toml(file.path().to_path_buf()) {
            Ok(_) => panic!("empty command selector should fail"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("invalid [commands.save] selector"),
            "error should explain invalid selector: {err}"
        );
    }

    #[test]
    fn config_rejects_non_table_agents_commands_section() {
        let toml = r#"
[agents]
commands = "save"
"#;

        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let err = match load_config_from_toml(file.path().to_path_buf()) {
            Ok(_) => panic!("[agents.commands] must be a table"),
            Err(err) => err,
        };
        assert!(
            err.to_string()
                .contains("[agents.commands] must be a table"),
            "error should explain malformed [agents.commands] section: {err}"
        );
    }

    #[test]
    fn config_rejects_invalid_agents_template_selector_key() {
        let toml = r#"
[agents.templates."   "]
agent = "codex"
"#;

        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let err = match load_config_from_toml(file.path().to_path_buf()) {
            Ok(_) => panic!("invalid template key should fail"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("invalid [agents.templates."),
            "error should explain malformed [agents.templates] key: {err}"
        );
    }

    #[test]
    fn alias_resolution_prefers_template_then_alias_then_legacy_scope() {
        let mut cfg = Config::default();
        cfg.agent_runtime.command = vec!["/bin/echo".to_string()];
        cfg.agent_runtime.label = Some("base".to_string());

        cfg.agent_scopes.insert(
            CommandScope::Save,
            AgentOverrides {
                agent_runtime: Some(AgentRuntimeOverride {
                    label: Some("legacy".to_string()),
                    command: Some(vec!["legacy-cmd".to_string()]),
                    progress_filter: None,
                    output: None,
                    enable_script_wrapper: None,
                }),
                ..Default::default()
            },
        );

        let alias = "save".parse::<CommandAlias>().expect("parse alias");
        cfg.agent_commands.insert(
            alias.clone(),
            AgentOverrides {
                agent_runtime: Some(AgentRuntimeOverride {
                    label: Some("alias".to_string()),
                    command: Some(vec!["alias-cmd".to_string()]),
                    progress_filter: None,
                    output: None,
                    enable_script_wrapper: None,
                }),
                ..Default::default()
            },
        );

        let selector = "template.save.custom@v2"
            .parse::<TemplateSelector>()
            .expect("parse selector");
        cfg.commands.insert(alias.clone(), selector.clone());
        cfg.agent_templates.insert(
            selector.clone(),
            AgentOverrides {
                agent_runtime: Some(AgentRuntimeOverride {
                    label: Some("template".to_string()),
                    command: Some(vec!["template-cmd".to_string()]),
                    progress_filter: None,
                    output: None,
                    enable_script_wrapper: None,
                }),
                ..Default::default()
            },
        );

        let resolved =
            resolve_agent_settings_for_alias_template(&cfg, &alias, Some(&selector), None)
                .expect("resolve alias/template settings");
        assert_eq!(resolved.agent_runtime.label, "template");
        assert_eq!(
            resolved.agent_runtime.command,
            vec!["template-cmd".to_string()]
        );
        assert_eq!(
            resolved.profile_scope,
            ProfileScope::Template(selector.clone())
        );

        let without_template = resolve_agent_settings_for_alias_template(&cfg, &alias, None, None)
            .expect("resolve alias settings");
        assert_eq!(without_template.agent_runtime.label, "alias");

        let compatibility = resolve_agent_settings(&cfg, CommandScope::Save, None)
            .expect("resolve legacy scope settings");
        assert_eq!(compatibility.agent_runtime.label, "template");
    }

    #[test]
    fn alias_prompt_resolution_prefers_template_prompt_over_alias_and_scope() {
        let mut cfg = Config::default();
        cfg.agent_runtime.command = vec!["/bin/echo".to_string()];
        cfg.agent_runtime.label = Some("prompt".to_string());
        cfg.agent_defaults.prompt_overrides.insert(
            PromptKind::Documentation,
            PromptOverrides {
                text: Some("default".to_string()),
                source_path: None,
                agent: None,
            },
        );
        cfg.agent_scopes.insert(
            CommandScope::Save,
            AgentOverrides {
                prompt_overrides: {
                    let mut map = std::collections::HashMap::new();
                    map.insert(
                        PromptKind::Documentation,
                        PromptOverrides {
                            text: Some("legacy".to_string()),
                            source_path: None,
                            agent: None,
                        },
                    );
                    map
                },
                ..Default::default()
            },
        );

        let alias = "save".parse::<CommandAlias>().expect("parse alias");
        cfg.agent_commands.insert(
            alias.clone(),
            AgentOverrides {
                prompt_overrides: {
                    let mut map = std::collections::HashMap::new();
                    map.insert(
                        PromptKind::Documentation,
                        PromptOverrides {
                            text: Some("alias".to_string()),
                            source_path: None,
                            agent: None,
                        },
                    );
                    map
                },
                ..Default::default()
            },
        );

        let selector = "template.save.custom@v2"
            .parse::<TemplateSelector>()
            .expect("parse selector");
        cfg.agent_templates.insert(
            selector.clone(),
            AgentOverrides {
                prompt_overrides: {
                    let mut map = std::collections::HashMap::new();
                    map.insert(
                        PromptKind::Documentation,
                        PromptOverrides {
                            text: Some("template".to_string()),
                            source_path: None,
                            agent: None,
                        },
                    );
                    map
                },
                ..Default::default()
            },
        );

        let prompt =
            cfg.prompt_for_alias_template(&alias, Some(&selector), PromptKind::Documentation);
        assert_eq!(prompt.text, "template");
        assert_eq!(
            prompt.origin,
            PromptOrigin::ScopedConfig {
                scope: ProfileScope::Template(selector.clone()),
            }
        );
    }
}
