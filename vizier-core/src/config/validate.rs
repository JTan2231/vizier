#[cfg(test)]
mod tests {
    use super::*;
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
        let cfg = Config::from_toml(config_path).expect("parse config");
        let selection = cfg.prompt_for(CommandScope::Ask, PromptKind::Documentation);
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

        let err = match Config::from_toml(file.path().to_path_buf()) {
            Ok(_) => panic!("plan_refine prompt kind should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("unknown prompt kind `plan_refine`"),
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

        let err = match Config::from_toml(file.path().to_path_buf()) {
            Ok(_) => panic!("refine scope should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("unknown [agents.refine] section"),
            "error message should mention unknown scope: {err}"
        );
    }

    #[test]
    fn documentation_settings_follow_scope_overrides() {
        let toml = r#"
[agents.default.documentation]
enabled = false
include_snapshot = false
include_narrative_docs = false

[agents.ask.documentation]
enabled = true
include_snapshot = true
include_narrative_docs = true
"#;

        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let mut cfg =
            Config::from_toml(file.path().to_path_buf()).expect("should parse TOML config");
        cfg.agent_runtime.command = vec!["/bin/echo".to_string()];
        cfg.agent_runtime.label = Some("doc-agent".to_string());

        let ask_settings = cfg
            .resolve_prompt_profile(CommandScope::Ask, PromptKind::Documentation, None)
            .expect("resolve ask settings");
        assert!(ask_settings.documentation.use_documentation_prompt);
        assert!(ask_settings.documentation.include_snapshot);
        assert!(ask_settings.documentation.include_narrative_docs);
        assert!(ask_settings.prompt_selection().is_some());

        let save_settings = cfg
            .resolve_prompt_profile(CommandScope::Save, PromptKind::Documentation, None)
            .expect("resolve save settings");
        assert!(!save_settings.documentation.use_documentation_prompt);
        assert!(!save_settings.documentation.include_snapshot);
        assert!(!save_settings.documentation.include_narrative_docs);
        assert!(save_settings.prompt_selection().is_none());
    }

    #[test]
    fn test_from_json_invalid_file() {
        let file = write_json_file("{ this is not valid json ");
        let result = Config::from_json(file.path().to_path_buf());
        assert!(result.is_err(), "expected error for invalid JSON");
    }

    #[test]
    fn test_from_json_missing_file() {
        let path = std::path::PathBuf::from("does_not_exist.json");
        let result = Config::from_json(path);
        assert!(result.is_err(), "expected error for missing file");
    }

    #[test]
    fn config_rejects_model_and_reasoning_keys() {
        let json = r#"{ "model": "gpt-5", "reasoning_effort": "medium" }"#;
        let file = write_json_file(json);

        let cfg = Config::from_json(file.path().to_path_buf());
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

        let err = match Config::from_toml(file.path().to_path_buf()) {
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
[agents.ask]
agent = "codex"
fallback_backend = "codex"
"#;
        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let err = match Config::from_toml(file.path().to_path_buf()) {
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

        let err = match Config::from_toml(file.path().to_path_buf()) {
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
[agents.ask]
backend = "gemini"
"#;
        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes())
            .expect("failed to write toml temp file");

        let err = match Config::from_toml(file.path().to_path_buf()) {
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
        let cfg = Config::from_toml(file.path().to_path_buf()).expect("parse review config");
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
        let cfg = Config::from_toml(file.path().to_path_buf()).expect("parse merge config");
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
        let cfg = Config::from_json(file.path().to_path_buf()).expect("parse merge config");
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
        let cfg = Config::from_toml(file.path().to_path_buf())
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
        let cfg = Config::from_json(file.path().to_path_buf())
            .expect("parse approve stop-condition config");
        assert_eq!(
            cfg.approve.stop_condition.script,
            Some(PathBuf::from("./scripts/approve-check.sh"))
        );
        assert_eq!(cfg.approve.stop_condition.retries, 4);
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
            Config::from_toml(file.path().to_path_buf()).expect("parse merge conflict config");
        assert!(
            cfg.merge.conflicts.auto_resolve,
            "conflict auto-resolve should parse from toml"
        );
    }

    #[test]
    fn test_merge_queue_config_from_toml() {
        let toml = r#"
[merge.queue]
enabled = true
"#;
        let mut file = NamedTempFile::new().expect("temp toml");
        file.write_all(toml.as_bytes()).unwrap();
        let cfg = Config::from_toml(file.path().to_path_buf()).expect("parse merge queue config");
        assert!(cfg.merge.queue.enabled);
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
"#,
        )
        .expect("write repo config");

        let cfg = Config::from_layers(&[
            ConfigLayer::from_toml(global_path).expect("global layer"),
            ConfigLayer::from_toml(repo_path).expect("repo layer"),
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
            Config::from_toml(config_path).expect("should parse config with prompt overrides");
        let selection = cfg.prompt_for(CommandScope::Ask, PromptKind::Documentation);
        assert_eq!(selection.text.trim(), "scoped prompt from file");
        assert_eq!(selection.source_path, Some(prompt_path.clone()));

        let agent = cfg
            .resolve_prompt_profile(CommandScope::Ask, PromptKind::Documentation, None)
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

        let cfg = Config::from_toml(file.path().to_path_buf()).expect("should parse agent command");
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

        let cfg = Config::from_toml(file.path().to_path_buf()).expect("should parse agent label");
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

        match Config::from_toml(file.path().to_path_buf()) {
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
        let resolved = resolve_agent_runtime(runtime, "codex", BackendKind::Agent)
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

        let resolved = resolve_agent_runtime(runtime, "codex", BackendKind::Agent)
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
        let agent = cfg
            .resolve_agent_settings(CommandScope::Ask, None)
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

        let agent = cfg
            .resolve_agent_settings(CommandScope::Ask, None)
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

        let agent = cfg
            .resolve_agent_settings(CommandScope::Ask, None)
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

        let agent = cfg
            .resolve_agent_settings(CommandScope::Ask, None)
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
        cfg.agent_scopes.insert(CommandScope::Ask, scoped);

        let ask = cfg
            .resolve_agent_settings(CommandScope::Ask, None)
            .expect("ask scope should resolve");
        assert_eq!(
            ask.agent_runtime.command,
            vec!["scoped-cmd".to_string()],
            "scoped command should override defaults and base config"
        );
        assert_eq!(ask.agent_runtime.label, "scoped");

        let save = cfg
            .resolve_agent_settings(CommandScope::Save, None)
            .expect("save scope should resolve");
        assert_eq!(
            save.agent_runtime.command,
            vec!["default-cmd".to_string()],
            "default agent override should replace base command for other scopes"
        );
        assert_eq!(save.agent_runtime.label, "default");

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

        let ask_with_cli = cfg
            .resolve_agent_settings(CommandScope::Ask, Some(&cli_override))
            .expect("cli override should resolve");
        assert_eq!(
            ask_with_cli.agent_runtime.command,
            vec!["cli-cmd".to_string(), "--flag".to_string()],
            "CLI command should take precedence over scoped/default commands"
        );
        assert_eq!(ask_with_cli.agent_runtime.label, "cli");
    }
}
