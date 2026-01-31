impl Default for DocumentationSettings {
    fn default() -> Self {
        Self {
            use_documentation_prompt: true,
            include_snapshot: true,
            include_narrative_docs: true,
        }
    }
}

impl Default for AgentRuntimeOptions {
    fn default() -> Self {
        Self {
            label: None,
            command: Vec::new(),
            progress_filter: None,
            output: AgentOutputMode::Auto,
            enable_script_wrapper: false,
        }
    }
}

impl Default for ApproveStopConditionConfig {
    fn default() -> Self {
        Self {
            script: None,
            retries: 3,
        }
    }
}

impl Default for MergeConfig {
    fn default() -> Self {
        Self {
            squash_default: true,
            squash_mainline: None,
            cicd_gate: MergeCicdGateConfig::default(),
            conflicts: MergeConflictsConfig::default(),
            queue: MergeQueueConfig::default(),
        }
    }
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            quiet: false,
        }
    }
}

impl Default for MergeCicdGateConfig {
    fn default() -> Self {
        Self {
            script: None,
            auto_resolve: false,
            retries: 1,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        let prompt_directory = tools::try_get_vizier_dir().map(std::path::PathBuf::from);
        let mut repo_prompts = HashMap::new();

        if let Some(dir) = prompt_directory.as_ref() {
            for kind in PromptKind::all().iter().copied() {
                for filename in kind.filename_candidates() {
                    let path = dir.join(filename);
                    if let Ok(contents) = std::fs::read_to_string(&path) {
                        repo_prompts.insert(kind, RepoPrompt { path, contents });
                        break;
                    }
                }
            }
        }

        let selector = default_selector_for_backend(BackendKind::Agent).to_string();
        Self {
            no_session: false,
            agent_selector: selector.clone(),
            backend: backend_kind_for_selector(&selector),
            agent_runtime: AgentRuntimeOptions::default(),
            approve: ApproveConfig::default(),
            review: ReviewConfig::default(),
            merge: MergeConfig::default(),
            jobs: JobsConfig::default(),
            workflow: WorkflowConfig::default(),
            agent_defaults: AgentOverrides::default(),
            agent_scopes: HashMap::new(),
            repo_prompts,
        }
    }
}
