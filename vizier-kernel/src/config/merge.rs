use super::*;

impl ApproveStopConditionConfig {
    fn apply_layer(&mut self, layer: &ApproveStopConditionLayer) {
        if let Some(script) = layer.script.as_ref() {
            self.script = Some(script.clone());
        }

        if let Some(retries) = layer.retries {
            self.retries = retries;
        }
    }
}

impl ApproveConfig {
    fn apply_layer(&mut self, layer: &ApproveLayer) {
        self.stop_condition.apply_layer(&layer.stop_condition);
    }
}

impl MergeConflictsConfig {
    fn apply_layer(&mut self, layer: &MergeConflictsLayer) {
        if let Some(auto_resolve) = layer.auto_resolve {
            self.auto_resolve = auto_resolve;
        }
    }
}

impl MergeConfig {
    fn apply_layer(&mut self, layer: &MergeLayer) {
        self.cicd_gate.apply_layer(&layer.cicd_gate);
        self.conflicts.apply_layer(&layer.conflicts);

        if let Some(default_squash) = layer.squash_default {
            self.squash_default = default_squash;
        }

        if let Some(mainline) = layer.squash_mainline {
            self.squash_mainline = Some(mainline);
        }
    }
}

impl BuildProfileConfig {
    fn apply_layer(&mut self, layer: &BuildProfileLayer) {
        if let Some(pipeline) = layer.pipeline {
            self.pipeline = Some(pipeline);
        }
        if let Some(target) = layer.merge_target.as_ref() {
            self.merge_target = Some(target.clone());
        }
        if let Some(mode) = layer.review_mode {
            self.review_mode = Some(mode);
        }
        if let Some(skip_checks) = layer.skip_checks {
            self.skip_checks = Some(skip_checks);
        }
        if let Some(keep_branch) = layer.keep_branch {
            self.keep_branch = Some(keep_branch);
        }
    }
}

impl BuildConfig {
    fn apply_layer(&mut self, layer: &BuildLayer) {
        if let Some(pipeline) = layer.default_pipeline {
            self.default_pipeline = pipeline;
        }
        if let Some(target) = layer.default_merge_target.as_ref() {
            self.default_merge_target = target.clone();
        }
        if let Some(stage_barrier) = layer.stage_barrier {
            self.stage_barrier = stage_barrier;
        }
        if let Some(failure_mode) = layer.failure_mode {
            self.failure_mode = failure_mode;
        }
        if let Some(review_mode) = layer.default_review_mode {
            self.default_review_mode = review_mode;
        }
        if let Some(skip_checks) = layer.default_skip_checks {
            self.default_skip_checks = skip_checks;
        }
        if let Some(keep_branch) = layer.default_keep_draft_branch {
            self.default_keep_draft_branch = keep_branch;
        }
        if let Some(profile) = layer.default_profile.as_ref() {
            self.default_profile = Some(profile.clone());
        }

        for (name, profile_layer) in &layer.profiles {
            self.profiles
                .entry(name.clone())
                .and_modify(|profile| profile.apply_layer(profile_layer))
                .or_insert_with(|| {
                    let mut profile = BuildProfileConfig::default();
                    profile.apply_layer(profile_layer);
                    profile
                });
        }
    }
}

impl CommitMetaLabels {
    fn apply_layer(&mut self, layer: &CommitMetaLabelsLayer) {
        if let Some(value) = layer.session_id.as_ref() {
            self.session_id = value.clone();
        }
        if let Some(value) = layer.session_log.as_ref() {
            self.session_log = value.clone();
        }
        if let Some(value) = layer.author_note.as_ref() {
            self.author_note = value.clone();
        }
        if let Some(value) = layer.narrative_summary.as_ref() {
            self.narrative_summary = value.clone();
        }
    }
}

impl CommitMetaConfig {
    fn apply_layer(&mut self, layer: &CommitMetaLayer) {
        if let Some(enabled) = layer.enabled {
            self.enabled = enabled;
        }
        if let Some(style) = layer.style {
            self.style = style;
        }
        if let Some(include) = layer.include.as_ref() {
            self.include = include.clone();
        }
        if let Some(path_mode) = layer.session_log_path {
            self.session_log_path = path_mode;
        }
        self.labels.apply_layer(&layer.labels);
    }
}

impl CommitFallbackSubjects {
    fn apply_layer(&mut self, layer: &CommitFallbackSubjectsLayer) {
        if let Some(value) = layer.code_change.as_ref() {
            self.code_change = value.clone();
        }
        if let Some(value) = layer.narrative_change.as_ref() {
            self.narrative_change = value.clone();
        }
        if let Some(value) = layer.conversation.as_ref() {
            self.conversation = value.clone();
        }
    }
}

impl CommitImplementationConfig {
    fn apply_layer(&mut self, layer: &CommitImplementationLayer) {
        if let Some(subject) = layer.subject.as_ref() {
            self.subject = subject.clone();
        }
        if let Some(fields) = layer.fields.as_ref() {
            self.fields = fields.clone();
        }
    }
}

impl CommitMergeConfig {
    fn apply_layer(&mut self, layer: &CommitMergeLayer) {
        if let Some(subject) = layer.subject.as_ref() {
            self.subject = subject.clone();
        }
        if let Some(include_note) = layer.include_operator_note {
            self.include_operator_note = include_note;
        }
        if let Some(label) = layer.operator_note_label.as_ref() {
            self.operator_note_label = label.clone();
        }
        if let Some(plan_mode) = layer.plan_mode {
            self.plan_mode = plan_mode;
        }
        if let Some(plan_label) = layer.plan_label.as_ref() {
            self.plan_label = plan_label.clone();
        }
    }
}

impl CommitConfig {
    fn apply_layer(&mut self, layer: &CommitLayer) {
        self.meta.apply_layer(&layer.meta);
        self.fallback_subjects.apply_layer(&layer.fallback_subjects);
        self.implementation.apply_layer(&layer.implementation);
        self.merge.apply_layer(&layer.merge);
    }
}

impl DisplayListConfig {
    fn apply_layer(&mut self, layer: &DisplayListLayer) {
        if let Some(format) = layer.format {
            self.format = format;
        }
        if let Some(fields) = layer.header_fields.as_ref() {
            self.header_fields = fields.clone();
        }
        if let Some(fields) = layer.entry_fields.as_ref() {
            self.entry_fields = fields.clone();
        }
        if let Some(fields) = layer.job_fields.as_ref() {
            self.job_fields = fields.clone();
        }
        if let Some(fields) = layer.command_fields.as_ref() {
            self.command_fields = fields.clone();
        }
        if let Some(max_len) = layer.summary_max_len {
            self.summary_max_len = max_len;
        }
        if let Some(single_line) = layer.summary_single_line {
            self.summary_single_line = single_line;
        }
        if let Some(labels) = layer.labels.as_ref() {
            for (key, value) in labels {
                self.labels.insert(key.clone(), value.clone());
            }
        }
    }
}

impl DisplayJobsListConfig {
    fn apply_layer(&mut self, layer: &DisplayJobsListLayer) {
        if let Some(format) = layer.format {
            self.format = format;
        }
        if let Some(show) = layer.show_succeeded {
            self.show_succeeded = show;
        }
        if let Some(fields) = layer.fields.as_ref() {
            self.fields = fields.clone();
        }
        if let Some(labels) = layer.labels.as_ref() {
            for (key, value) in labels {
                self.labels.insert(key.clone(), value.clone());
            }
        }
    }
}

impl DisplayJobsShowConfig {
    fn apply_layer(&mut self, layer: &DisplayJobsShowLayer) {
        if let Some(format) = layer.format {
            self.format = format;
        }
        if let Some(fields) = layer.fields.as_ref() {
            self.fields = fields.clone();
        }
        if let Some(labels) = layer.labels.as_ref() {
            for (key, value) in labels {
                self.labels.insert(key.clone(), value.clone());
            }
        }
    }
}

impl DisplayListsConfig {
    fn apply_layer(&mut self, layer: &DisplayListsLayer) {
        self.list.apply_layer(&layer.list);
        self.jobs.apply_layer(&layer.jobs);
        self.jobs_show.apply_layer(&layer.jobs_show);
    }
}

impl DisplaySettings {
    fn apply_layer(&mut self, layer: &DisplayLayer) {
        self.lists.apply_layer(&layer.lists);
    }
}

impl JobsCancelConfig {
    fn apply_layer(&mut self, layer: &JobsCancelLayer) {
        if let Some(cleanup) = layer.cleanup_worktree {
            self.cleanup_worktree = cleanup;
        }
    }
}

impl JobsConfig {
    fn apply_layer(&mut self, layer: &JobsLayer) {
        self.cancel.apply_layer(&layer.cancel);
    }
}

impl BackgroundConfig {
    fn apply_layer(&mut self, layer: &BackgroundLayer) {
        if let Some(enabled) = layer.enabled {
            self.enabled = enabled;
        }

        if let Some(quiet) = layer.quiet {
            self.quiet = quiet;
        }
    }
}

impl WorkflowTemplateConfig {
    fn apply_layer(&mut self, layer: &WorkflowTemplateLayer) {
        if let Some(value) = layer.save.as_ref() {
            self.save = value.clone();
        }
        if let Some(value) = layer.draft.as_ref() {
            self.draft = value.clone();
        }
        if let Some(value) = layer.approve.as_ref() {
            self.approve = value.clone();
        }
        if let Some(value) = layer.review.as_ref() {
            self.review = value.clone();
        }
        if let Some(value) = layer.merge.as_ref() {
            self.merge = value.clone();
        }
        if let Some(value) = layer.build_execute.as_ref() {
            self.build_execute = value.clone();
        }
        if let Some(value) = layer.patch.as_ref() {
            self.patch = value.clone();
        }
    }
}

impl WorkflowConfig {
    fn apply_layer(&mut self, layer: &WorkflowLayer) {
        if let Some(default_no_commit) = layer.no_commit_default {
            self.no_commit_default = default_no_commit;
        }

        self.background.apply_layer(&layer.background);
        self.templates.apply_layer(&layer.templates);
    }
}

impl MergeCicdGateConfig {
    fn apply_layer(&mut self, layer: &MergeCicdGateLayer) {
        if let Some(script) = layer.script.as_ref() {
            self.script = Some(script.clone());
        }

        if let Some(auto_resolve) = layer.auto_resolve {
            self.auto_resolve = auto_resolve;
        }

        if let Some(retries) = layer.retries {
            self.retries = retries;
        }
    }
}

impl Config {
    pub fn from_layers(layers: &[ConfigLayer]) -> Self {
        let mut config = Self::default();
        for layer in layers {
            config.apply_layer(layer);
        }
        config
    }

    pub fn apply_layer(&mut self, layer: &ConfigLayer) {
        if let Some(selector) = layer.agent_selector.as_ref() {
            self.agent_selector = selector.clone();
            self.backend = backend_kind_for_selector(selector);
        }

        if let Some(runtime) = layer.agent_runtime.as_ref() {
            self.agent_runtime.apply_override(runtime);
        }

        self.build.apply_layer(&layer.build);
        self.approve.apply_layer(&layer.approve);

        if let Some(commands) = layer.review.checks.as_ref() {
            self.review.checks.commands = commands.clone();
        }

        self.merge.apply_layer(&layer.merge);
        self.commits.apply_layer(&layer.commits);
        self.display.apply_layer(&layer.display);
        self.jobs.apply_layer(&layer.jobs);
        self.workflow.apply_layer(&layer.workflow);

        for (alias, selector) in layer.commands.iter() {
            self.commands.insert(alias.clone(), selector.clone());
        }

        if let Some(defaults) = layer.agent_defaults.as_ref() {
            if self.agent_defaults.is_empty() {
                self.agent_defaults = defaults.clone();
            } else {
                self.agent_defaults.merge(defaults);
            }
        }

        for (alias, overrides) in layer.agent_commands.iter() {
            self.agent_commands
                .entry(alias.clone())
                .and_modify(|existing| existing.merge(overrides))
                .or_insert_with(|| overrides.clone());
        }

        for (selector, overrides) in layer.agent_templates.iter() {
            self.agent_templates
                .entry(selector.clone())
                .and_modify(|existing| existing.merge(overrides))
                .or_insert_with(|| overrides.clone());
        }

        for (scope, overrides) in layer.agent_scopes.iter() {
            self.agent_scopes
                .entry(*scope)
                .and_modify(|existing| existing.merge(overrides))
                .or_insert_with(|| overrides.clone());
        }
    }
}
