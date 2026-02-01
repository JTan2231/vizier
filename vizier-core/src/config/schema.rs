use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use lazy_static::lazy_static;

use crate::{
    COMMIT_PROMPT, DOCUMENTATION_PROMPT, IMPLEMENTATION_PLAN_PROMPT, MERGE_CONFLICT_PROMPT,
    REVIEW_PROMPT,
    agent::{AgentRunner, ScriptRunner},
    tools, tree,
};

lazy_static! {
    static ref CONFIG: RwLock<Config> = RwLock::new(Config::default());
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

#[derive(Clone)]
pub struct Config {
    pub no_session: bool,
    pub agent_selector: String,
    pub backend: BackendKind,
    pub agent_runtime: AgentRuntimeOptions,
    pub approve: ApproveConfig,
    pub review: ReviewConfig,
    pub merge: MergeConfig,
    pub commits: CommitConfig,
    pub display: DisplaySettings,
    pub jobs: JobsConfig,
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

#[derive(Clone, Default)]
pub struct ApproveConfig {
    pub stop_condition: ApproveStopConditionConfig,
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

#[derive(Clone, Default)]
pub struct MergeQueueConfig {
    pub enabled: bool,
}

#[derive(Clone)]
pub struct MergeConfig {
    pub cicd_gate: MergeCicdGateConfig,
    pub conflicts: MergeConflictsConfig,
    pub queue: MergeQueueConfig,
    pub squash_default: bool,
    pub squash_mainline: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitMetaStyle {
    Header,
    Trailers,
    Both,
    None,
}

impl CommitMetaStyle {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "header" => Some(Self::Header),
            "trailers" | "trailer" => Some(Self::Trailers),
            "both" => Some(Self::Both),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommitMetaField {
    SessionId,
    SessionLog,
    AuthorNote,
    NarrativeSummary,
}

impl CommitMetaField {
    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value
            .trim()
            .to_ascii_lowercase()
            .replace(['-', ' '], "_");
        match normalized.as_str() {
            "session_id" => Some(Self::SessionId),
            "session_log" => Some(Self::SessionLog),
            "author_note" => Some(Self::AuthorNote),
            "narrative_summary" => Some(Self::NarrativeSummary),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            CommitMetaField::SessionId => "session_id",
            CommitMetaField::SessionLog => "session_log",
            CommitMetaField::AuthorNote => "author_note",
            CommitMetaField::NarrativeSummary => "narrative_summary",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitSessionLogPath {
    Relative,
    Absolute,
    None,
}

impl CommitSessionLogPath {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "relative" => Some(Self::Relative),
            "absolute" => Some(Self::Absolute),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitMetaLabels {
    pub session_id: String,
    pub session_log: String,
    pub author_note: String,
    pub narrative_summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitMetaConfig {
    pub enabled: bool,
    pub style: CommitMetaStyle,
    pub include: Vec<CommitMetaField>,
    pub session_log_path: CommitSessionLogPath,
    pub labels: CommitMetaLabels,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitFallbackSubjects {
    pub code_change: String,
    pub narrative_change: String,
    pub conversation: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitImplementationField {
    TargetBranch,
    PlanBranch,
    Summary,
}

impl CommitImplementationField {
    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value
            .trim()
            .to_ascii_lowercase()
            .replace(['-', '_'], " ");
        match normalized.as_str() {
            "target branch" => Some(Self::TargetBranch),
            "plan branch" => Some(Self::PlanBranch),
            "summary" => Some(Self::Summary),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            CommitImplementationField::TargetBranch => "Target branch",
            CommitImplementationField::PlanBranch => "Plan branch",
            CommitImplementationField::Summary => "Summary",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitImplementationConfig {
    pub subject: String,
    pub fields: Vec<CommitImplementationField>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitMergePlanMode {
    Full,
    Summary,
    None,
}

impl CommitMergePlanMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "full" => Some(Self::Full),
            "summary" => Some(Self::Summary),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitMergeConfig {
    pub subject: String,
    pub include_operator_note: bool,
    pub operator_note_label: String,
    pub plan_mode: CommitMergePlanMode,
    pub plan_label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CommitConfig {
    pub meta: CommitMetaConfig,
    pub fallback_subjects: CommitFallbackSubjects,
    pub implementation: CommitImplementationConfig,
    pub merge: CommitMergeConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListFormat {
    Block,
    Table,
    Json,
}

impl ListFormat {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "block" => Some(Self::Block),
            "table" => Some(Self::Table),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayListConfig {
    pub format: ListFormat,
    pub header_fields: Vec<String>,
    pub entry_fields: Vec<String>,
    pub job_fields: Vec<String>,
    pub command_fields: Vec<String>,
    pub summary_max_len: usize,
    pub summary_single_line: bool,
    pub labels: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayJobsListConfig {
    pub format: ListFormat,
    pub show_succeeded: bool,
    pub fields: Vec<String>,
    pub labels: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayJobsShowConfig {
    pub format: ListFormat,
    pub fields: Vec<String>,
    pub labels: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DisplayListsConfig {
    pub list: DisplayListConfig,
    pub jobs: DisplayJobsListConfig,
    pub jobs_show: DisplayJobsShowConfig,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DisplaySettings {
    pub lists: DisplayListsConfig,
}

#[derive(Clone, Default)]
pub struct JobsCancelConfig {
    pub cleanup_worktree: bool,
}

#[derive(Clone, Default)]
pub struct JobsConfig {
    pub cancel: JobsCancelConfig,
}

#[derive(Clone)]
pub struct BackgroundConfig {
    pub enabled: bool,
    pub quiet: bool,
}

#[derive(Clone, Default)]
pub struct WorkflowConfig {
    pub no_commit_default: bool,
    pub background: BackgroundConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeCicdGateConfig {
    pub script: Option<PathBuf>,
    pub auto_resolve: bool,
    pub retries: u32,
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
pub struct MergeQueueLayer {
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeLayer {
    pub cicd_gate: MergeCicdGateLayer,
    pub conflicts: MergeConflictsLayer,
    pub queue: MergeQueueLayer,
    pub squash_default: Option<bool>,
    pub squash_mainline: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitMetaLabelsLayer {
    pub session_id: Option<String>,
    pub session_log: Option<String>,
    pub author_note: Option<String>,
    pub narrative_summary: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitMetaLayer {
    pub enabled: Option<bool>,
    pub style: Option<CommitMetaStyle>,
    pub include: Option<Vec<CommitMetaField>>,
    pub session_log_path: Option<CommitSessionLogPath>,
    pub labels: CommitMetaLabelsLayer,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitFallbackSubjectsLayer {
    pub code_change: Option<String>,
    pub narrative_change: Option<String>,
    pub conversation: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitImplementationLayer {
    pub subject: Option<String>,
    pub fields: Option<Vec<CommitImplementationField>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitMergeLayer {
    pub subject: Option<String>,
    pub include_operator_note: Option<bool>,
    pub operator_note_label: Option<String>,
    pub plan_mode: Option<CommitMergePlanMode>,
    pub plan_label: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitLayer {
    pub meta: CommitMetaLayer,
    pub fallback_subjects: CommitFallbackSubjectsLayer,
    pub implementation: CommitImplementationLayer,
    pub merge: CommitMergeLayer,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayListLayer {
    pub format: Option<ListFormat>,
    pub header_fields: Option<Vec<String>>,
    pub entry_fields: Option<Vec<String>>,
    pub job_fields: Option<Vec<String>>,
    pub command_fields: Option<Vec<String>>,
    pub summary_max_len: Option<usize>,
    pub summary_single_line: Option<bool>,
    pub labels: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayJobsListLayer {
    pub format: Option<ListFormat>,
    pub show_succeeded: Option<bool>,
    pub fields: Option<Vec<String>>,
    pub labels: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayJobsShowLayer {
    pub format: Option<ListFormat>,
    pub fields: Option<Vec<String>>,
    pub labels: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayListsLayer {
    pub list: DisplayListLayer,
    pub jobs: DisplayJobsListLayer,
    pub jobs_show: DisplayJobsShowLayer,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayLayer {
    pub lists: DisplayListsLayer,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewLayer {
    pub checks: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobsCancelLayer {
    pub cleanup_worktree: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobsLayer {
    pub cancel: JobsCancelLayer,
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
    pub commits: CommitLayer,
    pub display: DisplayLayer,
    pub jobs: JobsLayer,
    pub workflow: WorkflowLayer,
    pub agent_defaults: Option<AgentOverrides>,
    pub agent_scopes: HashMap<CommandScope, AgentOverrides>,
}

impl Config {
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
