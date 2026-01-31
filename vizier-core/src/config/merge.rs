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

impl MergeQueueConfig {
    fn apply_layer(&mut self, layer: &MergeQueueLayer) {
        if let Some(enabled) = layer.enabled {
            self.enabled = enabled;
        }
    }
}

impl MergeConfig {
    fn apply_layer(&mut self, layer: &MergeLayer) {
        self.cicd_gate.apply_layer(&layer.cicd_gate);
        self.conflicts.apply_layer(&layer.conflicts);
        self.queue.apply_layer(&layer.queue);

        if let Some(default_squash) = layer.squash_default {
            self.squash_default = default_squash;
        }

        if let Some(mainline) = layer.squash_mainline {
            self.squash_mainline = Some(mainline);
        }
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

impl WorkflowConfig {
    fn apply_layer(&mut self, layer: &WorkflowLayer) {
        if let Some(default_no_commit) = layer.no_commit_default {
            self.no_commit_default = default_no_commit;
        }

        self.background.apply_layer(&layer.background);
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

        self.approve.apply_layer(&layer.approve);

        if let Some(commands) = layer.review.checks.as_ref() {
            self.review.checks.commands = commands.clone();
        }

        self.merge.apply_layer(&layer.merge);
        self.jobs.apply_layer(&layer.jobs);
        self.workflow.apply_layer(&layer.workflow);

        if let Some(defaults) = layer.agent_defaults.as_ref() {
            if self.agent_defaults.is_empty() {
                self.agent_defaults = defaults.clone();
            } else {
                self.agent_defaults.merge(defaults);
            }
        }

        for (scope, overrides) in layer.agent_scopes.iter() {
            self.agent_scopes
                .entry(*scope)
                .and_modify(|existing| existing.merge(overrides))
                .or_insert_with(|| overrides.clone());
        }
    }
}
