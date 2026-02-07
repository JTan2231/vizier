use std::collections::HashMap;
use std::path::PathBuf;

use super::{PromptKind, PromptOrigin, PromptSelection, SystemPrompt};

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
    Save,
    Draft,
    Approve,
    Review,
    Merge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProfileScope {
    Default,
    Command(CommandScope),
}

impl ProfileScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProfileScope::Default => "default",
            ProfileScope::Command(scope) => scope.as_str(),
        }
    }

    pub fn command_scope(&self) -> Option<CommandScope> {
        match self {
            ProfileScope::Default => None,
            ProfileScope::Command(scope) => Some(*scope),
        }
    }
}

impl From<CommandScope> for ProfileScope {
    fn from(value: CommandScope) -> Self {
        Self::Command(value)
    }
}

impl std::fmt::Display for ProfileScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl CommandScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            CommandScope::Save => "save",
            CommandScope::Draft => "draft",
            CommandScope::Approve => "approve",
            CommandScope::Review => "review",
            CommandScope::Merge => "merge",
        }
    }

    pub fn all() -> &'static [CommandScope] {
        &[
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
    pub fn is_empty(&self) -> bool {
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

    pub fn apply_to(&self, settings: &mut DocumentationSettings) {
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
    pub(crate) fn apply_override(&mut self, overrides: &AgentRuntimeOverride) {
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
pub struct PromptTemplate {
    pub path: PathBuf,
    pub contents: String,
}

#[derive(Clone)]
pub struct Config {
    pub no_session: bool,
    pub agent_selector: String,
    pub backend: BackendKind,
    pub agent_runtime: AgentRuntimeOptions,
    pub build: BuildConfig,
    pub approve: ApproveConfig,
    pub review: ReviewConfig,
    pub merge: MergeConfig,
    pub commits: CommitConfig,
    pub display: DisplaySettings,
    pub jobs: JobsConfig,
    pub workflow: WorkflowConfig,
    pub agent_defaults: AgentOverrides,
    pub agent_scopes: HashMap<CommandScope, AgentOverrides>,
    pub(crate) repo_prompts: HashMap<SystemPrompt, PromptTemplate>,
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

#[derive(Clone)]
pub struct MergeConfig {
    pub cicd_gate: MergeCicdGateConfig,
    pub conflicts: MergeConflictsConfig,
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
        let normalized = value.trim().to_ascii_lowercase().replace(['-', ' '], "_");
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
        let normalized = value.trim().to_ascii_lowercase().replace(['-', '_'], " ");
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuildPipeline {
    Approve,
    ApproveReview,
    ApproveReviewMerge,
}

impl BuildPipeline {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "approve" => Some(Self::Approve),
            "approve-review" | "approve_review" => Some(Self::ApproveReview),
            "approve-review-merge" | "approve_review_merge" => Some(Self::ApproveReviewMerge),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::ApproveReview => "approve-review",
            Self::ApproveReviewMerge => "approve-review-merge",
        }
    }

    pub fn includes_review(self) -> bool {
        matches!(self, Self::ApproveReview | Self::ApproveReviewMerge)
    }

    pub fn includes_merge(self) -> bool {
        matches!(self, Self::ApproveReviewMerge)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuildReviewMode {
    ApplyFixes,
    ReviewOnly,
    ReviewFile,
}

impl BuildReviewMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "apply_fixes" | "apply-fixes" => Some(Self::ApplyFixes),
            "review_only" | "review-only" => Some(Self::ReviewOnly),
            "review_file" | "review-file" => Some(Self::ReviewFile),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApplyFixes => "apply_fixes",
            Self::ReviewOnly => "review_only",
            Self::ReviewFile => "review_file",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuildStageBarrier {
    Strict,
    Explicit,
}

impl BuildStageBarrier {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "strict" => Some(Self::Strict),
            "explicit" => Some(Self::Explicit),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Explicit => "explicit",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuildFailureMode {
    BlockDownstream,
    ContinueIndependent,
}

impl BuildFailureMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "block_downstream" | "block-downstream" => Some(Self::BlockDownstream),
            "continue_independent" | "continue-independent" => Some(Self::ContinueIndependent),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::BlockDownstream => "block_downstream",
            Self::ContinueIndependent => "continue_independent",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BuildMergeTarget {
    Primary,
    Build,
    Branch(String),
}

impl BuildMergeTarget {
    pub fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }

        match trimmed.to_ascii_lowercase().as_str() {
            "primary" => Some(Self::Primary),
            "build" => Some(Self::Build),
            _ => Some(Self::Branch(trimmed.to_string())),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Primary => "primary",
            Self::Build => "build",
            Self::Branch(name) => name.as_str(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildProfileConfig {
    pub pipeline: Option<BuildPipeline>,
    pub merge_target: Option<BuildMergeTarget>,
    pub review_mode: Option<BuildReviewMode>,
    pub skip_checks: Option<bool>,
    pub keep_branch: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildConfig {
    pub default_pipeline: BuildPipeline,
    pub default_merge_target: BuildMergeTarget,
    pub stage_barrier: BuildStageBarrier,
    pub failure_mode: BuildFailureMode,
    pub default_review_mode: BuildReviewMode,
    pub default_skip_checks: bool,
    pub default_keep_draft_branch: bool,
    pub default_profile: Option<String>,
    pub profiles: HashMap<String, BuildProfileConfig>,
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
pub struct BuildProfileLayer {
    pub pipeline: Option<BuildPipeline>,
    pub merge_target: Option<BuildMergeTarget>,
    pub review_mode: Option<BuildReviewMode>,
    pub skip_checks: Option<bool>,
    pub keep_branch: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildLayer {
    pub default_pipeline: Option<BuildPipeline>,
    pub default_merge_target: Option<BuildMergeTarget>,
    pub stage_barrier: Option<BuildStageBarrier>,
    pub failure_mode: Option<BuildFailureMode>,
    pub default_review_mode: Option<BuildReviewMode>,
    pub default_skip_checks: Option<bool>,
    pub default_keep_draft_branch: Option<bool>,
    pub default_profile: Option<String>,
    pub profiles: HashMap<String, BuildProfileLayer>,
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
    pub build: BuildLayer,
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
    pub fn prompt_for_command(&self, scope: CommandScope, kind: PromptKind) -> PromptSelection {
        self.prompt_for_profile(ProfileScope::Command(scope), kind)
    }

    pub fn prompt_for_default(&self, kind: PromptKind) -> PromptSelection {
        self.prompt_for_profile(ProfileScope::Default, kind)
    }

    pub fn prompt_for(&self, scope: CommandScope, kind: PromptKind) -> PromptSelection {
        self.prompt_for_command(scope, kind)
    }

    fn prompt_for_profile(
        &self,
        requested_scope: ProfileScope,
        kind: PromptKind,
    ) -> PromptSelection {
        if let Some(selection) = self.prompt_from_agent_override(requested_scope, kind) {
            return selection;
        }

        if let Some(repo) = self.repo_prompts.get(&kind) {
            return PromptSelection {
                text: repo.contents.clone(),
                kind,
                requested_scope,
                origin: PromptOrigin::RepoFile {
                    path: repo.path.clone(),
                },
                source_path: Some(repo.path.clone()),
            };
        }

        PromptSelection {
            text: kind.default_template().to_string(),
            kind,
            requested_scope,
            origin: PromptOrigin::Default,
            source_path: None,
        }
    }

    fn prompt_from_agent_override(
        &self,
        requested_scope: ProfileScope,
        kind: PromptKind,
    ) -> Option<PromptSelection> {
        if let Some(scope) = requested_scope.command_scope()
            && let Some(scoped) = self
                .agent_scopes
                .get(&scope)
                .and_then(|value| value.prompt_overrides.get(&kind))
            && let Some(selection) = Self::selection_from_override(
                requested_scope,
                kind,
                scoped,
                ProfileScope::Command(scope),
            )
        {
            return Some(selection);
        }

        if let Some(defaults) = self.agent_defaults.prompt_overrides.get(&kind) {
            return Self::selection_from_override(
                requested_scope,
                kind,
                defaults,
                ProfileScope::Default,
            );
        }

        None
    }

    fn selection_from_override(
        requested_scope: ProfileScope,
        kind: PromptKind,
        overrides: &PromptOverrides,
        origin_scope: ProfileScope,
    ) -> Option<PromptSelection> {
        overrides.text.as_ref().map(|text| PromptSelection {
            text: text.clone(),
            kind,
            requested_scope,
            origin: PromptOrigin::ScopedConfig {
                scope: origin_scope,
            },
            source_path: overrides.source_path.clone(),
        })
    }

    pub fn set_repo_prompts(&mut self, prompts: HashMap<SystemPrompt, PromptTemplate>) {
        self.repo_prompts = prompts;
    }

    pub fn with_repo_prompts(mut self, prompts: HashMap<SystemPrompt, PromptTemplate>) -> Self {
        self.repo_prompts = prompts;
        self
    }

    pub fn repo_prompts(&self) -> &HashMap<SystemPrompt, PromptTemplate> {
        &self.repo_prompts
    }
}

pub fn backend_kind_for_selector(selector: &str) -> BackendKind {
    match selector.trim().to_ascii_lowercase().as_str() {
        "gemini" => BackendKind::Gemini,
        _ => BackendKind::Agent,
    }
}

pub fn default_selector_for_backend(backend: BackendKind) -> &'static str {
    match backend {
        BackendKind::Gemini => "gemini",
        BackendKind::Agent => "codex",
    }
}

pub fn normalize_selector_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_selector_trims_and_lowercases() {
        assert_eq!(
            normalize_selector_value("  CODEX  "),
            Some("codex".to_string())
        );
        assert_eq!(normalize_selector_value(""), None);
        assert_eq!(normalize_selector_value("   "), None);
    }

    #[test]
    fn runtime_normalization_sets_missing_label() {
        let runtime = AgentRuntimeOptions {
            label: None,
            command: vec!["/bin/echo".to_string()],
            progress_filter: None,
            output: AgentOutputMode::Auto,
            enable_script_wrapper: true,
        };

        let normalized = runtime.normalized_for_selector("codex");
        assert_eq!(normalized.label.as_deref(), Some("codex"));
        assert_eq!(normalized.command, runtime.command);
    }

    #[test]
    fn runtime_normalization_keeps_existing_label() {
        let runtime = AgentRuntimeOptions {
            label: Some("custom".to_string()),
            command: vec!["/bin/echo".to_string()],
            progress_filter: None,
            output: AgentOutputMode::Auto,
            enable_script_wrapper: true,
        };

        let normalized = runtime.normalized_for_selector("codex");
        assert_eq!(normalized.label.as_deref(), Some("custom"));
    }

    #[test]
    fn selector_to_backend_is_case_insensitive() {
        assert_eq!(backend_kind_for_selector("GEMINI"), BackendKind::Gemini);
        assert_eq!(backend_kind_for_selector("codex"), BackendKind::Agent);
    }

    #[test]
    fn prompt_for_default_ignores_command_prompt_overrides() {
        let mut cfg = Config::default();
        cfg.agent_defaults.prompt_overrides.insert(
            PromptKind::Documentation,
            PromptOverrides {
                text: Some("default documentation prompt".to_string()),
                source_path: None,
                agent: None,
            },
        );
        cfg.agent_scopes.insert(
            CommandScope::Save,
            AgentOverrides {
                prompt_overrides: {
                    let mut map = HashMap::new();
                    map.insert(
                        PromptKind::Documentation,
                        PromptOverrides {
                            text: Some("save documentation prompt".to_string()),
                            source_path: None,
                            agent: None,
                        },
                    );
                    map
                },
                ..Default::default()
            },
        );

        let selection = cfg.prompt_for_default(PromptKind::Documentation);
        assert_eq!(selection.text, "default documentation prompt");
        assert_eq!(selection.requested_scope, ProfileScope::Default);
        assert_eq!(
            selection.origin,
            PromptOrigin::ScopedConfig {
                scope: ProfileScope::Default
            }
        );
    }

    #[test]
    fn prompt_for_command_preserves_command_overrides() {
        let mut cfg = Config::default();
        cfg.agent_defaults.prompt_overrides.insert(
            PromptKind::Documentation,
            PromptOverrides {
                text: Some("default documentation prompt".to_string()),
                source_path: None,
                agent: None,
            },
        );
        cfg.agent_scopes.insert(
            CommandScope::Save,
            AgentOverrides {
                prompt_overrides: {
                    let mut map = HashMap::new();
                    map.insert(
                        PromptKind::Documentation,
                        PromptOverrides {
                            text: Some("save documentation prompt".to_string()),
                            source_path: None,
                            agent: None,
                        },
                    );
                    map
                },
                ..Default::default()
            },
        );

        let selection = cfg.prompt_for_command(CommandScope::Save, PromptKind::Documentation);
        assert_eq!(selection.text, "save documentation prompt");
        assert_eq!(
            selection.requested_scope,
            ProfileScope::Command(CommandScope::Save)
        );
        assert_eq!(
            selection.origin,
            PromptOrigin::ScopedConfig {
                scope: ProfileScope::Command(CommandScope::Save)
            }
        );
    }
}
