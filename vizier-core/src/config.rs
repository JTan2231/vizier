use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use lazy_static::lazy_static;

use crate::{
    COMMIT_PROMPT, DOCUMENTATION_PROMPT, IMPLEMENTATION_PLAN_PROMPT, MERGE_CONFLICT_PROMPT,
    PLAN_REFINE_PROMPT, REVIEW_PROMPT,
    agent::{AgentRunner, ScriptRunner},
    tools, tree,
};

lazy_static! {
    static ref CONFIG: RwLock<Config> = RwLock::new(Config::default());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Deserialize)]
pub enum PromptKind {
    Documentation,
    Commit,
    ImplementationPlan,
    PlanRefine,
    Review,
    MergeConflict,
}

/// Alias for prompt variants that feed the system prompt builder.
pub type SystemPrompt = PromptKind;

impl PromptKind {
    pub fn all() -> &'static [PromptKind] {
        const ALL: &[PromptKind] = &[
            PromptKind::Documentation,
            PromptKind::Commit,
            PromptKind::ImplementationPlan,
            PromptKind::PlanRefine,
            PromptKind::Review,
            PromptKind::MergeConflict,
        ];
        ALL
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            PromptKind::Documentation => "documentation",
            PromptKind::Commit => "commit",
            PromptKind::ImplementationPlan => "implementation_plan",
            PromptKind::PlanRefine => "plan_refine",
            PromptKind::Review => "review",
            PromptKind::MergeConflict => "merge_conflict",
        }
    }

    fn filename_candidates(&self) -> &'static [&'static str] {
        match self {
            PromptKind::Documentation => &["DOCUMENTATION_PROMPT.md"],
            PromptKind::Commit => &["COMMIT_PROMPT.md"],
            PromptKind::ImplementationPlan => &["IMPLEMENTATION_PLAN_PROMPT.md"],
            PromptKind::PlanRefine => &["PLAN_REFINE_PROMPT.md"],
            PromptKind::Review => &["REVIEW_PROMPT.md"],
            PromptKind::MergeConflict => &["MERGE_CONFLICT_PROMPT.md"],
        }
    }

    fn default_template(&self) -> &'static str {
        match self {
            PromptKind::Documentation => DOCUMENTATION_PROMPT,
            PromptKind::Commit => COMMIT_PROMPT,
            PromptKind::ImplementationPlan => IMPLEMENTATION_PLAN_PROMPT,
            PromptKind::PlanRefine => PLAN_REFINE_PROMPT,
            PromptKind::Review => REVIEW_PROMPT,
            PromptKind::MergeConflict => MERGE_CONFLICT_PROMPT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    Agent,
    Gemini,
}

impl BackendKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "agent" | "codex" => Some(Self::Agent),
            "gemini" => Some(Self::Gemini),
            _ => None,
        }
    }

    pub fn requires_agent_runner(&self) -> bool {
        matches!(self, BackendKind::Agent | BackendKind::Gemini)
    }
}

impl std::fmt::Display for BackendKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendKind::Agent => write!(f, "agent"),
            BackendKind::Gemini => write!(f, "gemini"),
        }
    }
}

impl std::str::FromStr for BackendKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or_else(|| format!("unknown backend `{value}`"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandScope {
    Ask,
    Save,
    Draft,
    Refine,
    Approve,
    Review,
    Merge,
}

impl CommandScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            CommandScope::Ask => "ask",
            CommandScope::Save => "save",
            CommandScope::Draft => "draft",
            CommandScope::Refine => "refine",
            CommandScope::Approve => "approve",
            CommandScope::Review => "review",
            CommandScope::Merge => "merge",
        }
    }

    pub fn all() -> &'static [CommandScope] {
        &[
            CommandScope::Ask,
            CommandScope::Save,
            CommandScope::Draft,
            CommandScope::Refine,
            CommandScope::Approve,
            CommandScope::Review,
            CommandScope::Merge,
        ]
    }
}

impl std::str::FromStr for CommandScope {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "ask" => Ok(CommandScope::Ask),
            "save" => Ok(CommandScope::Save),
            "draft" => Ok(CommandScope::Draft),
            "refine" => Ok(CommandScope::Refine),
            "approve" => Ok(CommandScope::Approve),
            "review" => Ok(CommandScope::Review),
            "merge" => Ok(CommandScope::Merge),
            other => Err(format!("unknown command scope `{other}`")),
        }
    }
}

impl std::fmt::Display for CommandScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentationSettings {
    pub use_documentation_prompt: bool,
    pub include_snapshot: bool,
    pub include_narrative_docs: bool,
}

impl Default for DocumentationSettings {
    fn default() -> Self {
        Self {
            use_documentation_prompt: true,
            include_snapshot: true,
            include_narrative_docs: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocumentationSettingsOverride {
    pub use_documentation_prompt: Option<bool>,
    pub include_snapshot: Option<bool>,
    pub include_narrative_docs: Option<bool>,
}

impl DocumentationSettingsOverride {
    fn is_empty(&self) -> bool {
        self.use_documentation_prompt.is_none()
            && self.include_snapshot.is_none()
            && self.include_narrative_docs.is_none()
    }

    fn merge(&mut self, other: &DocumentationSettingsOverride) {
        if let Some(enabled) = other.use_documentation_prompt {
            self.use_documentation_prompt = Some(enabled);
        }

        if let Some(include_snapshot) = other.include_snapshot {
            self.include_snapshot = Some(include_snapshot);
        }

        if let Some(include_docs) = other.include_narrative_docs {
            self.include_narrative_docs = Some(include_docs);
        }
    }

    fn apply_to(&self, settings: &mut DocumentationSettings) {
        if let Some(enabled) = self.use_documentation_prompt {
            settings.use_documentation_prompt = enabled;
        }

        if let Some(include_snapshot) = self.include_snapshot {
            settings.include_snapshot = include_snapshot;
        }

        if let Some(include_docs) = self.include_narrative_docs {
            settings.include_narrative_docs = include_docs;
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentOverrides {
    pub selector: Option<String>,
    pub agent_runtime: Option<AgentRuntimeOverride>,
    pub documentation: DocumentationSettingsOverride,
    pub prompt_overrides: HashMap<PromptKind, PromptOverrides>,
}

/// Prompt-level overrides live under `[agents.<scope>.prompts.<kind>]` so the same
/// table controls the template, agent overrides, and runtime options for a specific
/// command/prompt pairing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromptOverrides {
    pub text: Option<String>,
    pub source_path: Option<PathBuf>,
    pub agent: Option<Box<AgentOverrides>>,
}

impl PromptOverrides {
    pub fn agent_overrides(&self) -> Option<&AgentOverrides> {
        self.agent.as_deref()
    }
}

impl AgentOverrides {
    pub fn is_empty(&self) -> bool {
        self.selector.is_none()
            && self.agent_runtime.is_none()
            && self.documentation.is_empty()
            && self.prompt_overrides.is_empty()
    }

    pub fn merge(&mut self, other: &AgentOverrides) {
        if let Some(selector) = other.selector.as_ref() {
            self.selector = Some(selector.clone());
        }

        if let Some(runtime) = other.agent_runtime.as_ref() {
            if let Some(existing) = self.agent_runtime.as_mut() {
                existing.merge(runtime);
            } else {
                self.agent_runtime = Some(runtime.clone());
            }
        }

        self.documentation.merge(&other.documentation);

        for (kind, overrides) in other.prompt_overrides.iter() {
            self.prompt_overrides.insert(*kind, overrides.clone());
        }
    }
}

#[derive(Clone)]
pub struct AgentSettings {
    pub scope: CommandScope,
    pub selector: String,
    pub backend: BackendKind,
    pub runner: Option<Arc<dyn AgentRunner>>,
    pub agent_runtime: ResolvedAgentRuntime,
    pub documentation: DocumentationSettings,
    pub prompt: Option<PromptSelection>,
    pub cli_override: Option<AgentOverrides>,
}

impl AgentSettings {
    pub fn for_prompt(
        &self,
        kind: PromptKind,
    ) -> Result<AgentSettings, Box<dyn std::error::Error>> {
        get_config().resolve_prompt_profile(self.scope, kind, self.cli_override.as_ref())
    }

    pub fn prompt_selection(&self) -> Option<&PromptSelection> {
        self.prompt.as_ref()
    }

    pub fn agent_runner(&self) -> Result<&Arc<dyn AgentRunner>, Box<dyn std::error::Error>> {
        self.runner.as_ref().ok_or_else(|| {
            format!(
                "agent scope `{}` requires an agent backend runner, but none was resolved",
                self.scope.as_str()
            )
            .into()
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AgentOutputMode {
    #[default]
    Auto,
}

impl AgentOutputMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentOutputMode::Auto => "auto",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentOutputHandling {
    Wrapped,
}

impl AgentOutputHandling {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentOutputHandling::Wrapped => "wrapped",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentRuntimeOverride {
    pub label: Option<String>,
    pub command: Option<Vec<String>>,
    pub progress_filter: Option<Vec<String>>,
    pub output: Option<AgentOutputMode>,
    pub enable_script_wrapper: Option<bool>,
}

impl AgentRuntimeOverride {
    fn merge(&mut self, other: &AgentRuntimeOverride) {
        if let Some(label) = other.label.as_ref() {
            self.label = Some(label.clone());
        }

        if let Some(command) = other.command.as_ref() {
            self.command = Some(command.clone());
        }

        if let Some(filter) = other.progress_filter.as_ref() {
            self.progress_filter = Some(filter.clone());
        }

        if let Some(output) = other.output.as_ref() {
            self.output = Some(*output);
        }

        if let Some(enable_script_wrapper) = other.enable_script_wrapper {
            self.enable_script_wrapper = Some(enable_script_wrapper);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRuntimeOptions {
    pub label: Option<String>,
    pub command: Vec<String>,
    pub progress_filter: Option<Vec<String>>,
    pub output: AgentOutputMode,
    pub enable_script_wrapper: bool,
}

impl AgentRuntimeOptions {
    fn apply_override(&mut self, overrides: &AgentRuntimeOverride) {
        if let Some(label) = overrides.label.as_ref() {
            self.label = Some(label.clone());
        }

        if let Some(command) = overrides.command.as_ref() {
            self.command = command.clone();
        }

        if let Some(filter) = overrides.progress_filter.as_ref() {
            self.progress_filter = Some(filter.clone());
        }

        if let Some(output) = overrides.output.as_ref() {
            self.output = *output;
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

impl AgentRuntimeOptions {
    pub fn normalized_for_selector(&self, selector: &str) -> Self {
        let mut runtime = self.clone();

        if runtime.label.is_none() {
            runtime.label = Some(selector.to_string());
        }

        runtime
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentRuntimeResolution {
    BundledShim { label: String, path: PathBuf },
    ProvidedCommand,
}

#[derive(Clone, Debug)]
pub struct ResolvedAgentRuntime {
    pub label: String,
    pub command: Vec<String>,
    pub progress_filter: Option<Vec<String>>,
    pub output: AgentOutputHandling,
    pub enable_script_wrapper: bool,
    pub resolution: AgentRuntimeResolution,
}

#[derive(Clone, Debug)]
struct RepoPrompt {
    path: PathBuf,
    contents: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptOrigin {
    ScopedConfig { scope: CommandScope },
    RepoFile { path: PathBuf },
    Default,
}

impl PromptOrigin {
    pub fn label(&self) -> &'static str {
        match self {
            PromptOrigin::ScopedConfig { .. } => "scoped-config",
            PromptOrigin::RepoFile { .. } => "repo-file",
            PromptOrigin::Default => "default",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PromptSelection {
    pub text: String,
    pub kind: PromptKind,
    pub requested_scope: CommandScope,
    pub origin: PromptOrigin,
    pub source_path: Option<PathBuf>,
}

#[derive(Clone)]
pub struct Config {
    pub no_session: bool,
    pub agent_selector: String,
    pub backend: BackendKind,
    pub agent_runtime: AgentRuntimeOptions,
    pub approve: ApproveConfig,
    pub review: ReviewConfig,
    pub merge: MergeConfig,
    pub workflow: WorkflowConfig,
    pub agent_defaults: AgentOverrides,
    pub agent_scopes: HashMap<CommandScope, AgentOverrides>,
    repo_prompts: HashMap<SystemPrompt, RepoPrompt>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApproveStopConditionConfig {
    pub script: Option<PathBuf>,
    pub retries: u32,
}

impl Default for ApproveStopConditionConfig {
    fn default() -> Self {
        Self {
            script: None,
            retries: 3,
        }
    }
}

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

#[derive(Clone, Default)]
pub struct ApproveConfig {
    pub stop_condition: ApproveStopConditionConfig,
}

impl ApproveConfig {
    fn apply_layer(&mut self, layer: &ApproveLayer) {
        self.stop_condition.apply_layer(&layer.stop_condition);
    }
}

#[derive(Clone, Default)]
pub struct ReviewConfig {
    pub checks: ReviewChecksConfig,
}

#[derive(Clone, Default)]
pub struct ReviewChecksConfig {
    pub commands: Vec<String>,
}

#[derive(Clone, Default)]
pub struct MergeConflictsConfig {
    pub auto_resolve: bool,
}

impl MergeConflictsConfig {
    fn apply_layer(&mut self, layer: &MergeConflictsLayer) {
        if let Some(auto_resolve) = layer.auto_resolve {
            self.auto_resolve = auto_resolve;
        }
    }
}

#[derive(Clone)]
pub struct MergeConfig {
    pub cicd_gate: MergeCicdGateConfig,
    pub conflicts: MergeConflictsConfig,
    pub squash_default: bool,
    pub squash_mainline: Option<u32>,
}

impl Default for MergeConfig {
    fn default() -> Self {
        Self {
            cicd_gate: MergeCicdGateConfig::default(),
            conflicts: MergeConflictsConfig::default(),
            squash_default: true,
            squash_mainline: None,
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

#[derive(Clone)]
pub struct BackgroundConfig {
    pub enabled: bool,
    pub quiet: bool,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            quiet: false,
        }
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

#[derive(Clone, Default)]
pub struct WorkflowConfig {
    pub no_commit_default: bool,
    pub background: BackgroundConfig,
}

impl WorkflowConfig {
    fn apply_layer(&mut self, layer: &WorkflowLayer) {
        if let Some(no_commit) = layer.no_commit_default {
            self.no_commit_default = no_commit;
        }

        self.background.apply_layer(&layer.background);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeCicdGateConfig {
    pub script: Option<PathBuf>,
    pub auto_resolve: bool,
    pub retries: u32,
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeCicdGateLayer {
    pub script: Option<PathBuf>,
    pub auto_resolve: Option<bool>,
    pub retries: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeConflictsLayer {
    pub auto_resolve: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeLayer {
    pub cicd_gate: MergeCicdGateLayer,
    pub conflicts: MergeConflictsLayer,
    pub squash_default: Option<bool>,
    pub squash_mainline: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewLayer {
    pub checks: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackgroundLayer {
    pub enabled: Option<bool>,
    pub quiet: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkflowLayer {
    pub no_commit_default: Option<bool>,
    pub background: BackgroundLayer,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApproveStopConditionLayer {
    pub script: Option<PathBuf>,
    pub retries: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApproveLayer {
    pub stop_condition: ApproveStopConditionLayer,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfigLayer {
    pub agent_selector: Option<String>,
    pub agent_runtime: Option<AgentRuntimeOverride>,
    pub approve: ApproveLayer,
    pub review: ReviewLayer,
    pub merge: MergeLayer,
    pub workflow: WorkflowLayer,
    pub agent_defaults: Option<AgentOverrides>,
    pub agent_scopes: HashMap<CommandScope, AgentOverrides>,
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
            workflow: WorkflowConfig::default(),
            agent_defaults: AgentOverrides::default(),
            agent_scopes: HashMap::new(),
            repo_prompts,
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

    pub fn from_json(filepath: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        Self::from_reader(filepath.as_path(), FileFormat::Json)
    }

    pub fn from_toml(filepath: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        Self::from_reader(filepath.as_path(), FileFormat::Toml)
    }

    pub fn from_path<P: AsRef<Path>>(filepath: P) -> Result<Self, Box<dyn std::error::Error>> {
        let path = filepath.as_ref();

        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase());

        match ext.as_deref() {
            Some("json") => Self::from_reader(path, FileFormat::Json),
            Some("toml") => Self::from_reader(path, FileFormat::Toml),
            _ => Self::from_reader(path, FileFormat::Toml)
                .or_else(|_| Self::from_reader(path, FileFormat::Json)),
        }
    }

    fn from_reader(path: &Path, format: FileFormat) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)?;
        let base_dir = path.parent();
        Self::from_str(&contents, format, base_dir)
    }

    fn from_str(
        contents: &str,
        format: FileFormat,
        base_dir: Option<&Path>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let layer = ConfigLayer::from_str(contents, format, base_dir)?;
        Ok(Config::from_layers(&[layer]))
    }

    pub fn prompt_for(&self, scope: CommandScope, kind: PromptKind) -> PromptSelection {
        if let Some(selection) = self.prompt_from_agent_override(scope, kind) {
            return selection;
        }

        if let Some(repo) = self.repo_prompts.get(&kind) {
            return PromptSelection {
                text: repo.contents.clone(),
                kind,
                requested_scope: scope,
                origin: PromptOrigin::RepoFile {
                    path: repo.path.clone(),
                },
                source_path: Some(repo.path.clone()),
            };
        }

        PromptSelection {
            text: kind.default_template().to_string(),
            kind,
            requested_scope: scope,
            origin: PromptOrigin::Default,
            source_path: None,
        }
    }

    fn prompt_from_agent_override(
        &self,
        scope: CommandScope,
        kind: PromptKind,
    ) -> Option<PromptSelection> {
        if let Some(scoped) = self
            .agent_scopes
            .get(&scope)
            .and_then(|value| value.prompt_overrides.get(&kind))
            && let Some(selection) = Self::selection_from_override(scope, kind, scoped, scope)
        {
            return Some(selection);
        }

        if let Some(defaults) = self.agent_defaults.prompt_overrides.get(&kind) {
            return Self::selection_from_override(scope, kind, defaults, scope);
        }

        None
    }

    fn selection_from_override(
        scope: CommandScope,
        kind: PromptKind,
        overrides: &PromptOverrides,
        origin_scope: CommandScope,
    ) -> Option<PromptSelection> {
        overrides.text.as_ref().map(|text| PromptSelection {
            text: text.clone(),
            kind,
            requested_scope: scope,
            origin: PromptOrigin::ScopedConfig {
                scope: origin_scope,
            },
            source_path: overrides.source_path.clone(),
        })
    }

    pub fn get_prompt(&self, prompt: SystemPrompt) -> String {
        self.prompt_for(CommandScope::Ask, prompt).text
    }

    pub fn resolve_agent_settings(
        &self,
        scope: CommandScope,
        cli_override: Option<&AgentOverrides>,
    ) -> Result<AgentSettings, Box<dyn std::error::Error>> {
        let mut builder = AgentSettingsBuilder::new(self);

        if !self.agent_defaults.is_empty() {
            builder.apply(&self.agent_defaults);
        }

        if let Some(overrides) = self.agent_scopes.get(&scope) {
            builder.apply(overrides);
        }

        if let Some(overrides) = cli_override
            && !overrides.is_empty()
        {
            builder.apply_cli_override(overrides);
        }

        builder.build(scope, None, cli_override)
    }

    pub fn resolve_prompt_profile(
        &self,
        scope: CommandScope,
        kind: PromptKind,
        cli_override: Option<&AgentOverrides>,
    ) -> Result<AgentSettings, Box<dyn std::error::Error>> {
        let mut builder = AgentSettingsBuilder::new(self);

        if !self.agent_defaults.is_empty() {
            builder.apply(&self.agent_defaults);
        }

        if let Some(scope_overrides) = self.agent_scopes.get(&scope) {
            builder.apply(scope_overrides);
        }

        if let Some(default_prompt) = self.agent_defaults.prompt_overrides.get(&kind) {
            builder.apply_prompt_overrides(default_prompt);
        }

        if let Some(scoped_prompt) = self
            .agent_scopes
            .get(&scope)
            .and_then(|scope_overrides| scope_overrides.prompt_overrides.get(&kind))
        {
            builder.apply_prompt_overrides(scoped_prompt);
        }

        if let Some(overrides) = cli_override
            && !overrides.is_empty()
        {
            builder.apply_cli_override(overrides);
        }

        let prompt = if kind == PromptKind::Documentation
            && !builder.documentation.use_documentation_prompt
        {
            None
        } else {
            Some(self.prompt_for(scope, kind))
        };
        builder.build(scope, prompt, cli_override)
    }
}

#[derive(Copy, Clone)]
enum FileFormat {
    Json,
    Toml,
}

const MODEL_KEY_PATHS: &[&[&str]] = &[
    &["model"],
    &["provider"],
    &["provider", "model"],
    &["provider", "name"],
];
const BACKEND_KEY_PATHS: &[&[&str]] = &[&["backend"], &["provider", "backend"]];
const FALLBACK_BACKEND_KEY_PATHS: &[&[&str]] = &[&["fallback_backend"], &["fallback-backend"]];
const FALLBACK_BACKEND_DEPRECATION_MESSAGE: &str =
    "fallback_backend entries are unsupported; remove them from your config.";
const REASONING_EFFORT_KEY_PATHS: &[&[&str]] = &[
    &["reasoning_effort"],
    &["reasoning-effort"],
    &["thinking_level"],
    &["thinking-level"],
    &["provider", "reasoning_effort"],
    &["provider", "reasoning-effort"],
    &["provider", "thinking_level"],
    &["provider", "thinking-level"],
    &["flags", "reasoning_effort"],
    &["flags", "reasoning-effort"],
    &["flags", "thinking_level"],
    &["flags", "thinking-level"],
];
const MODEL_CONFIG_REMOVED_MESSAGE: &str =
    "model overrides are no longer supported now that the wire backend has been removed.";
const REASONING_CONFIG_REMOVED_MESSAGE: &str = "reasoning-effort overrides are no longer supported now that the wire backend has been removed.";
// Prompt templates resolve from scoped agent profiles, then repo prompt files, then defaults.

#[derive(Clone)]
struct AgentSettingsBuilder {
    selector: String,
    backend: BackendKind,
    agent_runtime: AgentRuntimeOptions,
    documentation: DocumentationSettings,
}

impl AgentSettingsBuilder {
    fn new(cfg: &Config) -> Self {
        let selector = cfg.agent_selector.clone();
        Self {
            selector: selector.clone(),
            backend: backend_kind_for_selector(&selector),
            agent_runtime: cfg.agent_runtime.clone(),
            documentation: DocumentationSettings::default(),
        }
    }

    fn apply(&mut self, overrides: &AgentOverrides) {
        if let Some(selector) = overrides.selector.as_ref() {
            self.set_selector(selector);
        }

        if let Some(runtime) = overrides.agent_runtime.as_ref() {
            if let Some(label) = runtime.label.as_ref() {
                self.agent_runtime.label = Some(label.clone());
            }

            if let Some(command) = runtime.command.as_ref() {
                self.agent_runtime.command = command.clone();
            }

            if let Some(filter) = runtime.progress_filter.as_ref() {
                self.agent_runtime.progress_filter = Some(filter.clone());
            }

            if let Some(output) = runtime.output.as_ref() {
                self.agent_runtime.output = *output;
            }
        }

        overrides.documentation.apply_to(&mut self.documentation);
    }

    fn apply_cli_override(&mut self, overrides: &AgentOverrides) {
        if let Some(selector) = overrides.selector.as_ref() {
            self.set_selector(selector);
        }

        if let Some(runtime) = overrides.agent_runtime.as_ref() {
            if let Some(label) = runtime.label.as_ref() {
                self.agent_runtime.label = Some(label.clone());
            }

            if let Some(command) = runtime.command.as_ref() {
                self.agent_runtime.command = command.clone();
            }

            if let Some(filter) = runtime.progress_filter.as_ref() {
                self.agent_runtime.progress_filter = Some(filter.clone());
            }

            if let Some(output) = runtime.output.as_ref() {
                self.agent_runtime.output = *output;
            }

            if let Some(enable_script_wrapper) = runtime.enable_script_wrapper {
                self.agent_runtime.enable_script_wrapper = enable_script_wrapper;
            }
        }

        overrides.documentation.apply_to(&mut self.documentation);
    }

    fn apply_prompt_overrides(&mut self, overrides: &PromptOverrides) {
        if let Some(agent) = overrides.agent_overrides() {
            self.apply(agent);
        }
    }

    fn set_selector<S: AsRef<str>>(&mut self, selector: S) {
        let normalized = selector.as_ref().trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return;
        }
        self.backend = backend_kind_for_selector(&normalized);
        self.selector = normalized;
    }

    fn build(
        &self,
        scope: CommandScope,
        prompt: Option<PromptSelection>,
        cli_override: Option<&AgentOverrides>,
    ) -> Result<AgentSettings, Box<dyn std::error::Error>> {
        let agent_runtime = self.agent_runtime.normalized_for_selector(&self.selector);

        let resolved_runtime =
            resolve_agent_runtime(agent_runtime.clone(), &self.selector, self.backend)?;

        Ok(AgentSettings {
            scope,
            selector: self.selector.clone(),
            backend: self.backend,
            runner: resolve_agent_runner(self.backend)?,
            agent_runtime: resolved_runtime,
            documentation: self.documentation.clone(),
            prompt,
            cli_override: cli_override.cloned(),
        })
    }
}

pub fn backend_kind_for_selector(selector: &str) -> BackendKind {
    match selector.trim().to_ascii_lowercase().as_str() {
        "gemini" => BackendKind::Gemini,
        _ => BackendKind::Agent,
    }
}

fn default_selector_for_backend(backend: BackendKind) -> &'static str {
    match backend {
        BackendKind::Gemini => "gemini",
        BackendKind::Agent => "codex",
    }
}

fn normalize_selector_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

fn command_label(command: &[String]) -> Option<String> {
    let candidate = PathBuf::from(command.first()?);
    let stem = candidate.file_stem()?.to_string_lossy().to_string();
    if stem.is_empty() { None } else { Some(stem) }
}

// Attach a bundled progress filter for any agent label that ships one (codex, gemini,
// or custom shims), so wrapped output stays consistent without per-backend branching.
fn default_progress_filter_for_label(label: &str) -> Option<Vec<String>> {
    bundled_progress_filter(label).map(|path| vec![path.display().to_string()])
}

fn bundled_agent_shim_dir_candidates() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(dir) = std::env::var("VIZIER_AGENT_SHIMS_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            dirs.push(PathBuf::from(trimmed));
        }
    }

    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        dirs.push(dir.join("agents"));
        if let Some(prefix) = dir.parent() {
            dirs.push(prefix.join("share").join("vizier").join("agents"));
        }
    }

    let workspace_agents = PathBuf::from("examples").join("agents");
    dirs.push(workspace_agents);

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(workspace_root) = manifest_dir.parent() {
        dirs.push(workspace_root.join("examples").join("agents"));
    }

    dirs.retain(|path| path.is_dir());
    dirs
}

fn bundled_agent_command(label: &str) -> Option<PathBuf> {
    find_first_in_shim_dirs(vec![
        format!("{label}/agent.sh"),
        format!("{label}.sh"), // backward compatibility
    ])
}

fn bundled_progress_filter(label: &str) -> Option<PathBuf> {
    find_first_in_shim_dirs(vec![
        format!("{label}/filter.sh"),
        format!("{label}-filter.sh"), // backward compatibility
    ])
}

fn find_in_shim_dirs(filename: &str) -> Option<PathBuf> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for dir in bundled_agent_shim_dir_candidates() {
        if !seen.insert(dir.clone()) {
            continue;
        }
        let candidate = dir.join(filename);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn find_first_in_shim_dirs(candidates: Vec<String>) -> Option<PathBuf> {
    for name in candidates {
        if let Some(path) = find_in_shim_dirs(&name) {
            return Some(path);
        }
    }
    None
}

fn resolve_agent_runtime(
    runtime: AgentRuntimeOptions,
    selector: &str,
    backend: BackendKind,
) -> Result<ResolvedAgentRuntime, Box<dyn std::error::Error>> {
    let mut label = runtime.label.clone().unwrap_or_else(|| {
        if selector.trim().is_empty() {
            default_selector_for_backend(backend).to_string()
        } else {
            selector.to_string()
        }
    });
    let mut progress_filter = runtime.progress_filter.clone();
    let output = AgentOutputHandling::Wrapped;

    if progress_filter.is_none() {
        progress_filter = default_progress_filter_for_label(&label);
    }

    if !runtime.command.is_empty() {
        if label.is_empty() {
            label = default_selector_for_backend(backend).to_string();
        } else if runtime.label.is_none() {
            label = command_label(&runtime.command).unwrap_or(label);
        }

        return Ok(ResolvedAgentRuntime {
            label,
            command: runtime.command,
            progress_filter,
            output,
            enable_script_wrapper: runtime.enable_script_wrapper,
            resolution: AgentRuntimeResolution::ProvidedCommand,
        });
    }

    if backend.requires_agent_runner() {
        let Some(path) = bundled_agent_command(&label) else {
            let locations: Vec<String> = bundled_agent_shim_dir_candidates()
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            let hint = if locations.is_empty() {
                "no shim directories detected".to_string()
            } else {
                format!("looked in {}", locations.join(", "))
            };
            return Err(Box::new(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "no bundled agent shim named `{label}` was found ({hint}); set agent.command to a script that prints assistant output to stdout and progress/errors to stderr"
                ),
            )));
        };

        return Ok(ResolvedAgentRuntime {
            label: label.clone(),
            command: vec![path.display().to_string()],
            progress_filter,
            output,
            enable_script_wrapper: runtime.enable_script_wrapper,
            resolution: AgentRuntimeResolution::BundledShim { label, path },
        });
    }

    Ok(ResolvedAgentRuntime {
        label,
        command: Vec::new(),
        progress_filter,
        output,
        enable_script_wrapper: runtime.enable_script_wrapper,
        resolution: AgentRuntimeResolution::ProvidedCommand,
    })
}

fn resolve_agent_runner(
    backend: BackendKind,
) -> Result<Option<Arc<dyn AgentRunner>>, Box<dyn std::error::Error>> {
    if !backend.requires_agent_runner() {
        return Ok(None);
    }

    Ok(Some(Arc::new(ScriptRunner)))
}

fn value_at_path<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    let mut current = value;

    for segment in path {
        match current {
            serde_json::Value::Object(map) => {
                current = map.get(*segment)?;
            }
            _ => return None,
        }
    }

    Some(current)
}

fn find_string(value: &serde_json::Value, paths: &[&[&str]]) -> Option<String> {
    for path in paths {
        if let Some(serde_json::Value::String(s)) = value_at_path(value, path)
            && !s.is_empty()
        {
            return Some(s.clone());
        }
    }

    None
}

fn parse_agent_overrides(
    value: &serde_json::Value,
    allow_prompt_children: bool,
    base_dir: Option<&Path>,
) -> Result<Option<AgentOverrides>, Box<dyn std::error::Error>> {
    if !value.is_object() {
        return Ok(None);
    }

    if find_string(value, FALLBACK_BACKEND_KEY_PATHS).is_some() {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidInput,
            FALLBACK_BACKEND_DEPRECATION_MESSAGE,
        )));
    }

    let mut overrides = AgentOverrides::default();

    if find_string(value, MODEL_KEY_PATHS).is_some() {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidInput,
            MODEL_CONFIG_REMOVED_MESSAGE,
        )));
    }

    if find_string(value, REASONING_EFFORT_KEY_PATHS).is_some() {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidInput,
            REASONING_CONFIG_REMOVED_MESSAGE,
        )));
    }

    if let Some(agent_value) = value.get("agent") {
        if let Some(raw) = agent_value.as_str() {
            if overrides.selector.is_none()
                && let Some(selector) = normalize_selector_value(raw)
            {
                overrides.selector = Some(selector);
            }
        } else if let Some(parsed) = parse_agent_runtime_override(agent_value)? {
            overrides.agent_runtime = Some(parsed);
        }
    }

    if find_string(value, BACKEND_KEY_PATHS).is_some() {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidInput,
            "backend entries are unsupported; use agent selectors instead",
        )));
    }

    if allow_prompt_children {
        if let Some(doc_settings) = parse_documentation_settings(value)? {
            overrides.documentation = doc_settings;
        }

        if let Some(prompts_value) = value_at_path(value, &["prompts"]) {
            overrides.prompt_overrides =
                parse_prompt_override_table(prompts_value, base_dir)?.unwrap_or_default();
        }
    }

    if overrides.is_empty() {
        Ok(None)
    } else {
        Ok(Some(overrides))
    }
}

fn parse_prompt_override_table(
    value: &serde_json::Value,
    base_dir: Option<&Path>,
) -> Result<Option<HashMap<PromptKind, PromptOverrides>>, Box<dyn std::error::Error>> {
    let table = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(None),
    };

    let mut overrides = HashMap::new();

    for (key, entry) in table {
        let Some(kind) = prompt_kind_from_key(key) else {
            continue;
        };

        let mut prompt_override = PromptOverrides::default();

        match entry {
            serde_json::Value::String(text) => {
                if !text.trim().is_empty() {
                    prompt_override.text = Some(text.clone());
                }
            }
            serde_json::Value::Object(_) => {
                if let Some(path) = parse_prompt_path(entry, base_dir)? {
                    prompt_override.source_path = Some(path.clone());
                    prompt_override.text = Some(std::fs::read_to_string(&path)?);
                } else if let Some(text) =
                    parse_inline_prompt_text(entry).map(|text| text.to_string())
                {
                    prompt_override.text = Some(text);
                }

                if let Some(agent) = parse_agent_overrides(entry, false, base_dir)? {
                    prompt_override.agent = Some(Box::new(agent));
                }
            }
            _ => continue,
        }

        if prompt_override.text.is_none() && prompt_override.agent.is_none() {
            continue;
        }

        overrides.insert(kind, prompt_override);
    }

    if overrides.is_empty() {
        Ok(None)
    } else {
        Ok(Some(overrides))
    }
}

fn parse_prompt_path(
    entry: &serde_json::Value,
    base_dir: Option<&Path>,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    let Some(object) = entry.as_object() else {
        return Ok(None);
    };

    let path_value = object
        .get("path")
        .or_else(|| object.get("file"))
        .and_then(|value| value.as_str());

    let Some(raw_path) = path_value
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    let mut resolved = PathBuf::from(raw_path);
    if resolved.is_relative()
        && let Some(base) = base_dir
    {
        resolved = base.join(resolved);
    }

    Ok(Some(resolved))
}

fn parse_inline_prompt_text(entry: &serde_json::Value) -> Option<&str> {
    let object = entry.as_object()?;
    for key in ["text", "prompt", "template", "inline"] {
        if let Some(value) = object.get(key)
            && let Some(text) = value.as_str()
        {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    None
}

fn parse_command_value(value: &serde_json::Value) -> Option<Vec<String>> {
    match value {
        serde_json::Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(vec![trimmed.to_string()])
            }
        }
        serde_json::Value::Array(entries) => {
            let mut parts = Vec::new();
            for entry in entries {
                if let Some(text) = entry.as_str().map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    parts.push(text.to_string());
                }
            }
            if parts.is_empty() { None } else { Some(parts) }
        }
        _ => None,
    }
}

fn parse_agent_sections_into_layer(
    layer: &mut ConfigLayer,
    agents_value: &serde_json::Value,
    base_dir: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let table = agents_value
        .as_object()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "[agents] must be a table"))?;

    for (key, value) in table.iter() {
        let Some(overrides) = parse_agent_overrides(value, true, base_dir)? else {
            continue;
        };

        if key.eq_ignore_ascii_case("default") {
            layer.agent_defaults = Some(overrides);
            continue;
        }

        let scope = key.parse::<CommandScope>().map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown [agents.{key}] section: {err}"),
            )
        })?;
        layer.agent_scopes.insert(scope, overrides);
    }

    Ok(())
}

impl ConfigLayer {
    pub fn from_json(filepath: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        Self::from_reader(filepath.as_path(), FileFormat::Json)
    }

    pub fn from_toml(filepath: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        Self::from_reader(filepath.as_path(), FileFormat::Toml)
    }

    pub fn from_path<P: AsRef<Path>>(filepath: P) -> Result<Self, Box<dyn std::error::Error>> {
        let path = filepath.as_ref();

        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase());

        match ext.as_deref() {
            Some("json") => Self::from_reader(path, FileFormat::Json),
            Some("toml") => Self::from_reader(path, FileFormat::Toml),
            _ => Self::from_reader(path, FileFormat::Toml)
                .or_else(|_| Self::from_reader(path, FileFormat::Json)),
        }
    }

    fn from_reader(path: &Path, format: FileFormat) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)?;
        let base_dir = path.parent();
        Self::from_str(&contents, format, base_dir)
    }

    fn from_str(
        contents: &str,
        format: FileFormat,
        base_dir: Option<&Path>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let file_config: serde_json::Value = match format {
            FileFormat::Json => serde_json::from_str(contents)?,
            FileFormat::Toml => toml::from_str(contents)?,
        };

        Self::from_value(file_config, base_dir)
    }

    fn from_value(
        file_config: serde_json::Value,
        base_dir: Option<&Path>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut layer = ConfigLayer::default();

        if find_string(&file_config, MODEL_KEY_PATHS).is_some() {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                MODEL_CONFIG_REMOVED_MESSAGE,
            )));
        }

        if find_string(&file_config, REASONING_EFFORT_KEY_PATHS).is_some() {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                REASONING_CONFIG_REMOVED_MESSAGE,
            )));
        }

        if find_string(&file_config, FALLBACK_BACKEND_KEY_PATHS).is_some() {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                FALLBACK_BACKEND_DEPRECATION_MESSAGE,
            )));
        }

        if let Some(agent_value) = value_at_path(&file_config, &["agent"]) {
            if let Some(raw) = agent_value.as_str() {
                if let Some(selector) = normalize_selector_value(raw) {
                    layer.agent_selector = Some(selector);
                }
            } else if let Some(parsed) = parse_agent_runtime_override(agent_value)? {
                layer.agent_runtime = Some(parsed);
            }
        }

        if find_string(&file_config, BACKEND_KEY_PATHS).is_some() {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                "backend entries are unsupported; use agent selectors instead",
            )));
        }

        if let Some(commands) = parse_string_array(value_at_path(
            &file_config,
            &["review", "checks", "commands"],
        )) {
            layer.review.checks = Some(commands);
        } else if let Some(commands) =
            parse_string_array(value_at_path(&file_config, &["review", "checks"]))
        {
            layer.review.checks = Some(commands);
        }

        if let Some(stop_condition) = value_at_path(&file_config, &["approve", "stop_condition"])
            && let Some(object) = stop_condition.as_object()
        {
            if let Some(script_value) = object
                .get("script")
                .and_then(|value| value.as_str().map(|s| s.trim()).filter(|s| !s.is_empty()))
            {
                layer.approve.stop_condition.script = Some(PathBuf::from(script_value));
            }

            if let Some(retries) = parse_u32(object.get("retries"))
                .or_else(|| parse_u32(object.get("max_retries")))
                .or_else(|| parse_u32(object.get("max_attempts")))
            {
                layer.approve.stop_condition.retries = Some(retries);
            }
        }

        if let Some(cicd_gate) = value_at_path(&file_config, &["merge", "cicd_gate"])
            && let Some(gate_object) = cicd_gate.as_object()
        {
            if let Some(script_value) = gate_object
                .get("script")
                .and_then(|value| value.as_str().map(|s| s.trim()).filter(|s| !s.is_empty()))
            {
                layer.merge.cicd_gate.script = Some(PathBuf::from(script_value));
            }

            if let Some(auto_value) = parse_bool(gate_object.get("auto_resolve")) {
                layer.merge.cicd_gate.auto_resolve = Some(auto_value);
            } else if let Some(auto_value) = parse_bool(gate_object.get("auto-fix")) {
                layer.merge.cicd_gate.auto_resolve = Some(auto_value);
            }

            if let Some(retries) = parse_u32(gate_object.get("retries")) {
                layer.merge.cicd_gate.retries = Some(retries);
            } else if let Some(retries) = parse_u32(gate_object.get("max_retries")) {
                layer.merge.cicd_gate.retries = Some(retries);
            } else if let Some(retries) = parse_u32(gate_object.get("max_attempts")) {
                layer.merge.cicd_gate.retries = Some(retries);
            }
        }

        if let Some(merge_table) = value_at_path(&file_config, &["merge"])
            && let Some(table) = merge_table.as_object()
        {
            if let Some(squash) = parse_bool(
                table
                    .get("squash")
                    .or_else(|| table.get("squash_default"))
                    .or_else(|| table.get("squash-default")),
            ) {
                layer.merge.squash_default = Some(squash);
            }

            if let Some(mainline) = parse_u32(
                table
                    .get("squash_mainline")
                    .or_else(|| table.get("squash-mainline")),
            ) {
                layer.merge.squash_mainline = Some(mainline);
            }

            if let Some(conflicts_value) = table.get("conflicts")
                && let Some(conflicts) = conflicts_value.as_object()
            {
                let auto_resolve = conflicts
                    .get("auto_resolve")
                    .or_else(|| conflicts.get("auto-resolve"))
                    .or_else(|| conflicts.get("auto_resolve_conflicts"))
                    .or_else(|| conflicts.get("auto-resolve-conflicts"))
                    .and_then(|value| parse_bool(Some(value)));
                if let Some(auto) = auto_resolve {
                    layer.merge.conflicts.auto_resolve = Some(auto);
                }
            }
        }

        if let Some(workflow_value) = value_at_path(&file_config, &["workflow"])
            && let Some(workflow_table) = workflow_value.as_object()
        {
            if let Some(no_commit) = parse_bool(workflow_table.get("no_commit_default"))
                .or_else(|| parse_bool(workflow_table.get("no-commit-default")))
            {
                layer.workflow.no_commit_default = Some(no_commit);
            }

            if let Some(background_value) = workflow_table.get("background")
                && let Some(background_table) = background_value.as_object()
            {
                if let Some(enabled) = parse_bool(
                    background_table
                        .get("enabled")
                        .or_else(|| background_table.get("allow")),
                ) {
                    layer.workflow.background.enabled = Some(enabled);
                }

                if let Some(quiet) = parse_bool(
                    background_table
                        .get("quiet")
                        .or_else(|| background_table.get("silent")),
                ) {
                    layer.workflow.background.quiet = Some(quiet);
                }
            }
        }

        if let Some(agent_value) = value_at_path(&file_config, &["agents"]) {
            parse_agent_sections_into_layer(&mut layer, agent_value, base_dir)?;
        }

        Ok(layer)
    }
}

fn parse_agent_runtime_override(
    value: &serde_json::Value,
) -> Result<Option<AgentRuntimeOverride>, Box<dyn std::error::Error>> {
    let object = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(None),
    };

    let mut overrides = AgentRuntimeOverride::default();

    let allowed_keys = [
        "label",
        "command",
        "progress_filter",
        "output",
        "enable_script_wrapper",
    ];
    for key in object.keys() {
        if !allowed_keys.contains(&key.as_str()) {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                "agent runtime supports only label, command, progress_filter, output, and enable_script_wrapper",
            )));
        }
    }

    if let Some(label) = object
        .get("label")
        .and_then(|value| value.as_str().map(|s| s.trim()).filter(|s| !s.is_empty()))
    {
        overrides.label = Some(label.to_ascii_lowercase());
    }

    if let Some(command) = object.get("command") {
        if let Some(parsed) = parse_command_value(command) {
            overrides.command = Some(parsed);
        } else {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                "`agent.command` must be a non-empty string or array",
            )));
        }
    }

    if let Some(filter) = object.get("progress_filter") {
        if let Some(parsed) = parse_command_value(filter) {
            overrides.progress_filter = Some(parsed);
        } else {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                "`agent.progress_filter` must be a non-empty string or array",
            )));
        }
    }

    if let Some(output) = object.get("output") {
        if let Some(value) = output.as_str() {
            let normalized = value.trim().to_ascii_lowercase();
            overrides.output = match normalized.as_str() {
                "" | "auto" | "wrapped" | "wrapped-json" => Some(AgentOutputMode::Auto),
                other => {
                    return Err(Box::new(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "unknown agent.output value `{other}` (expected auto|wrapped-json)"
                        ),
                    )));
                }
            };
        } else {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                "`agent.output` must be a string (auto|wrapped-json)",
            )));
        }
    }

    if let Some(enable_script) = parse_bool(object.get("enable_script_wrapper")) {
        overrides.enable_script_wrapper = Some(enable_script);
    }

    if overrides == AgentRuntimeOverride::default() {
        Ok(None)
    } else {
        Ok(Some(overrides))
    }
}

fn parse_documentation_settings(
    value: &serde_json::Value,
) -> Result<Option<DocumentationSettingsOverride>, Box<dyn std::error::Error>> {
    let Some(table) = value_at_path(value, &["documentation"]).and_then(|v| v.as_object()) else {
        return Ok(None);
    };

    let mut overrides = DocumentationSettingsOverride::default();

    if let Some(enabled) = parse_bool(
        table
            .get("enabled")
            .or_else(|| table.get("enable"))
            .or_else(|| table.get("use_prompt"))
            .or_else(|| table.get("use-prompt"))
            .or_else(|| table.get("use_documentation_prompt"))
            .or_else(|| table.get("use-documentation-prompt")),
    ) {
        overrides.use_documentation_prompt = Some(enabled);
    }

    if let Some(include_snapshot) = parse_bool(
        table
            .get("include_snapshot")
            .or_else(|| table.get("include-snapshot"))
            .or_else(|| table.get("snapshot")),
    ) {
        overrides.include_snapshot = Some(include_snapshot);
    }

    if let Some(include_docs) = parse_bool(
        table
            .get("include_narrative_docs")
            .or_else(|| table.get("include-narrative-docs"))
            .or_else(|| table.get("include_narrative")),
    ) {
        overrides.include_narrative_docs = Some(include_docs);
    }

    if overrides.is_empty() {
        Ok(None)
    } else {
        Ok(Some(overrides))
    }
}

fn prompt_kind_from_key(key: &str) -> Option<PromptKind> {
    let normalized = key.trim().to_ascii_lowercase().replace('-', "_");

    match normalized.as_str() {
        "documentation" => Some(PromptKind::Documentation),
        "commit" => Some(PromptKind::Commit),
        "implementation_plan" => Some(PromptKind::ImplementationPlan),
        "plan_refine" => Some(PromptKind::PlanRefine),
        "review" => Some(PromptKind::Review),
        "merge_conflict" => Some(PromptKind::MergeConflict),
        _ => None,
    }
}

fn parse_string_array(value: Option<&serde_json::Value>) -> Option<Vec<String>> {
    let array = value?.as_array()?;
    let mut entries = Vec::new();
    for entry in array {
        if let Some(text) = entry.as_str().map(|s| s.trim()).filter(|s| !s.is_empty()) {
            entries.push(text.to_string());
        }
    }
    if entries.is_empty() {
        None
    } else {
        Some(entries)
    }
}

fn parse_bool(value: Option<&serde_json::Value>) -> Option<bool> {
    let raw = value?;
    match raw {
        serde_json::Value::Bool(inner) => Some(*inner),
        serde_json::Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.eq_ignore_ascii_case("true") {
                Some(true)
            } else if trimmed.eq_ignore_ascii_case("false") {
                Some(false)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn parse_u32(value: Option<&serde_json::Value>) -> Option<u32> {
    let raw = value?;
    if let Some(num) = raw.as_u64() {
        return u32::try_from(num).ok();
    }
    if let Some(text) = raw.as_str() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Ok(parsed) = trimmed.parse::<u32>() {
            return Some(parsed);
        }
    }
    None
}

/// Returns the repo-local config path if `.vizier/config.toml` or `.vizier/config.json` exists.
///
/// Canonical search order (highest precedence first):
/// 1. CLI `--config-file` flag (handled in the CLI entrypoint)
/// 2. Repo-local `.vizier/config.toml` (falling back to `.vizier/config.json`)
/// 3. Global config under `$XDG_CONFIG_HOME`/platform default (`~/.config/vizier/config.toml`)
/// 4. `VIZIER_CONFIG_FILE` environment variable (lowest precedence)
pub fn project_config_path(project_root: &Path) -> Option<PathBuf> {
    let vizier_dir = project_root.join(".vizier");
    let toml_path = vizier_dir.join("config.toml");
    if toml_path.is_file() {
        return Some(toml_path);
    }
    let json_path = vizier_dir.join("config.json");
    if json_path.is_file() {
        Some(json_path)
    } else {
        None
    }
}

/// Returns the user-global config path (`~/.config/vizier/config.toml` on Unix).
pub fn global_config_path() -> Option<PathBuf> {
    let base_dir = base_config_dir()?;
    Some(base_dir.join("vizier").join("config.toml"))
}

/// Returns the config path provided via `VIZIER_CONFIG_FILE`, ignoring blank values.
pub fn env_config_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("VIZIER_CONFIG_FILE") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    None
}

pub fn base_config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("VIZIER_CONFIG_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    if let Ok(dir) = std::env::var("APPDATA") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed).join(".config"));
        }
    }

    if let Ok(profile) = std::env::var("USERPROFILE") {
        let trimmed = profile.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed).join("AppData").join("Roaming"));
        }
    }

    None
}

pub fn set_config(new_config: Config) {
    *CONFIG.write().unwrap() = new_config;
}

pub fn get_config() -> Config {
    CONFIG.read().unwrap().clone()
}

pub fn get_system_prompt_with_meta(
    scope: CommandScope,
    prompt_kind: Option<SystemPrompt>,
) -> Result<String, Box<dyn std::error::Error>> {
    let cfg = get_config();
    let mut prompt = if let Some(kind) = prompt_kind {
        cfg.prompt_for(scope, kind).text
    } else {
        cfg.prompt_for(scope, SystemPrompt::Documentation).text
    };

    prompt.push_str("<meta>");

    let file_tree = tree::build_tree()?;

    prompt.push_str(&format!(
        "<fileTree>{}</fileTree>",
        tree::tree_to_string(&file_tree, "")
    ));

    prompt.push_str(&format!(
        "<narrativeDocs>{}</narrativeDocs>",
        tools::list_narrative_docs()
    ));

    prompt.push_str(&format!(
        "<currentWorkingDirectory>{}</currentWorkingDirectory>",
        std::env::current_dir().unwrap().to_str().unwrap()
    ));

    prompt.push_str("</meta>");

    Ok(prompt)
}

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
    fn repo_prompt_fallback_includes_plan_refine() {
        let _guard = CWD_LOCK.lock().unwrap();
        let temp_dir = tempdir().expect("create temp dir");
        let vizier_dir = temp_dir.path().join(".vizier");
        fs::create_dir_all(&vizier_dir).expect("create .vizier");
        fs::write(
            vizier_dir.join("PLAN_REFINE_PROMPT.md"),
            "repo refine prompt",
        )
        .expect("write repo prompt");

        let config_path = temp_dir.path().join("config.toml");
        fs::write(&config_path, "").expect("write empty config");

        let _cwd = CwdGuard::enter(temp_dir.path());
        let cfg = Config::from_toml(config_path).expect("parse config");
        let selection = cfg.prompt_for(CommandScope::Refine, PromptKind::PlanRefine);
        assert_eq!(selection.text, "repo refine prompt");
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
