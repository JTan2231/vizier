use std::collections::HashMap;

use super::*;

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

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            default_pipeline: BuildPipeline::Approve,
            default_merge_target: BuildMergeTarget::Primary,
            stage_barrier: BuildStageBarrier::Strict,
            failure_mode: BuildFailureMode::BlockDownstream,
            default_review_mode: BuildReviewMode::ApplyFixes,
            default_skip_checks: false,
            default_keep_draft_branch: false,
            default_profile: None,
            profiles: HashMap::new(),
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
                "Waited on".to_string(),
                "Dependencies".to_string(),
                "Locks".to_string(),
                "Pinned head".to_string(),
                "Approval required".to_string(),
                "Approval state".to_string(),
                "Approval decided by".to_string(),
                "Artifacts".to_string(),
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
                "Build pipeline".to_string(),
                "Build target".to_string(),
                "Build review mode".to_string(),
                "Build skip checks".to_string(),
                "Build keep branch".to_string(),
                "Build dependencies".to_string(),
                "Workflow template".to_string(),
                "Workflow template version".to_string(),
                "Workflow node".to_string(),
                "Workflow policy snapshot".to_string(),
                "Workflow gates".to_string(),
                "Patch file".to_string(),
                "Patch index".to_string(),
                "Patch total".to_string(),
                "Revision".to_string(),
                "After".to_string(),
                "Dependencies".to_string(),
                "Locks".to_string(),
                "Wait".to_string(),
                "Waited on".to_string(),
                "Approval required".to_string(),
                "Approval state".to_string(),
                "Approval requested at".to_string(),
                "Approval requested by".to_string(),
                "Approval decided at".to_string(),
                "Approval decided by".to_string(),
                "Approval reason".to_string(),
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

impl Default for WorkflowTemplateConfig {
    fn default() -> Self {
        Self {
            save: "template.save.v1".to_string(),
            draft: "template.draft.v1".to_string(),
            approve: "template.approve.v1".to_string(),
            review: "template.review.v1".to_string(),
            merge: "template.merge.v1".to_string(),
            build_execute: "template.build_execute.v1".to_string(),
            patch: "template.patch.v1".to_string(),
        }
    }
}

impl Default for WorkflowGlobalWorkflowsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            dir: std::path::PathBuf::new(),
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
        let selector = default_selector_for_backend(BackendKind::Agent).to_string();
        Self {
            no_session: false,
            agent_selector: selector.clone(),
            backend: backend_kind_for_selector(&selector),
            agent_runtime: AgentRuntimeOptions::default(),
            build: BuildConfig::default(),
            approve: ApproveConfig::default(),
            review: ReviewConfig::default(),
            merge: MergeConfig::default(),
            commits: CommitConfig::default(),
            display: DisplaySettings::default(),
            jobs: JobsConfig::default(),
            workflow: WorkflowConfig::default(),
            commands: HashMap::new(),
            agent_defaults: AgentOverrides::default(),
            agent_commands: HashMap::new(),
            agent_templates: HashMap::new(),
            agent_scopes: HashMap::new(),
            repo_prompts: HashMap::new(),
        }
    }
}
