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
        }
    }
}

impl Default for CommitMetaLabels {
    fn default() -> Self {
        Self {
            session_id: "Session ID".to_string(),
            session_log: "Session Log".to_string(),
            author_note: "Author note".to_string(),
            narrative_summary: "Narrative updates".to_string(),
        }
    }
}

impl Default for CommitMetaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            style: CommitMetaStyle::Header,
            include: vec![
                CommitMetaField::SessionId,
                CommitMetaField::SessionLog,
                CommitMetaField::AuthorNote,
                CommitMetaField::NarrativeSummary,
            ],
            session_log_path: CommitSessionLogPath::Relative,
            labels: CommitMetaLabels::default(),
        }
    }
}

impl Default for CommitFallbackSubjects {
    fn default() -> Self {
        Self {
            code_change: "VIZIER CODE CHANGE".to_string(),
            narrative_change: "VIZIER NARRATIVE CHANGE".to_string(),
            conversation: "VIZIER CONVERSATION".to_string(),
        }
    }
}

impl Default for CommitImplementationConfig {
    fn default() -> Self {
        Self {
            subject: "feat: apply plan {slug}".to_string(),
            fields: vec![
                CommitImplementationField::TargetBranch,
                CommitImplementationField::PlanBranch,
                CommitImplementationField::Summary,
            ],
        }
    }
}

impl Default for CommitMergeConfig {
    fn default() -> Self {
        Self {
            subject: "feat: merge plan {slug}".to_string(),
            include_operator_note: true,
            operator_note_label: "Operator Note".to_string(),
            plan_mode: CommitMergePlanMode::Full,
            plan_label: "Implementation Plan".to_string(),
        }
    }
}

impl Default for DisplayListConfig {
    fn default() -> Self {
        Self {
            format: ListFormat::Block,
            header_fields: vec!["Outcome".to_string(), "Target".to_string()],
            entry_fields: vec![
                "Plan".to_string(),
                "Branch".to_string(),
                "Summary".to_string(),
            ],
            job_fields: vec![
                "Job".to_string(),
                "Job status".to_string(),
                "Job scope".to_string(),
                "Job started".to_string(),
            ],
            command_fields: vec![
                "Status".to_string(),
                "Logs".to_string(),
                "Attach".to_string(),
            ],
            summary_max_len: 120,
            summary_single_line: true,
            labels: HashMap::new(),
        }
    }
}

impl Default for DisplayJobsListConfig {
    fn default() -> Self {
        Self {
            format: ListFormat::Block,
            show_succeeded: false,
            fields: vec![
                "Job".to_string(),
                "Status".to_string(),
                "Created".to_string(),
                "Wait".to_string(),
                "Dependencies".to_string(),
                "Locks".to_string(),
                "Pinned head".to_string(),
                "Failed".to_string(),
                "Command".to_string(),
            ],
            labels: HashMap::new(),
        }
    }
}

impl Default for DisplayJobsShowConfig {
    fn default() -> Self {
        Self {
            format: ListFormat::Block,
            fields: vec![
                "Job".to_string(),
                "Status".to_string(),
                "PID".to_string(),
                "Started".to_string(),
                "Finished".to_string(),
                "Exit code".to_string(),
                "Stdout".to_string(),
                "Stderr".to_string(),
                "Session".to_string(),
                "Outcome".to_string(),
                "Scope".to_string(),
                "Plan".to_string(),
                "Target".to_string(),
                "Branch".to_string(),
                "Revision".to_string(),
                "Dependencies".to_string(),
                "Locks".to_string(),
                "Wait".to_string(),
                "Pinned head".to_string(),
                "Artifacts".to_string(),
                "Worktree".to_string(),
                "Worktree name".to_string(),
                "Agent backend".to_string(),
                "Agent label".to_string(),
                "Agent command".to_string(),
                "Agent exit".to_string(),
                "Cancel cleanup".to_string(),
                "Cancel cleanup error".to_string(),
                "Config snapshot".to_string(),
                "Command".to_string(),
            ],
            labels: HashMap::new(),
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
            commits: CommitConfig::default(),
            display: DisplaySettings::default(),
            jobs: JobsConfig::default(),
            workflow: WorkflowConfig::default(),
            agent_defaults: AgentOverrides::default(),
            agent_scopes: HashMap::new(),
            repo_prompts,
        }
    }
}
