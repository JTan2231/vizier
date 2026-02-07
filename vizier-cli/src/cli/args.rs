use std::collections::HashMap;
use std::path::PathBuf;

use clap::{ArgAction, ArgGroup, Args as ClapArgs, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use vizier_core::{config, display};

use crate::actions::SpecSource;
use crate::jobs;

/// A CLI for LLM project management.
#[derive(Parser, Debug)]
#[command(
    name = "vizier",
    version,
    about,
    disable_help_subcommand = true,
    // Show help when you forget a subcommand
    arg_required_else_help = true,
    // Make version available to subcommands automatically
    propagate_version = true
)]
pub(crate) struct Cli {
    #[command(flatten)]
    pub(crate) global: GlobalOpts,

    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(ClapArgs, Debug, Default)]
pub(crate) struct GlobalOpts {
    /// Increase stderr verbosity (`-v` = info, `-vv` = debug); quiet wins over verbose, and output still honors TTY/--no-ansi gating
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count, global = true)]
    pub(crate) verbose: u8,

    /// Silence progress/history; only errors and explicit output (help/outcome) remain
    #[arg(short = 'q', long, global = true)]
    pub(crate) quiet: bool,

    /// Enable debug logging (alias for -vv; kept for parity with older workflows)
    #[arg(short = 'd', long, global = true)]
    pub(crate) debug: bool,

    /// Disable ANSI control sequences even on TTYs (non-TTY is always plain); useful for CI/log scrapers
    #[arg(long = "no-ansi", global = true)]
    pub(crate) no_ansi: bool,

    /// Page help output when available; defaults to TTY-only paging and honors $VIZIER_PAGER
    #[arg(long = "pager", action = ArgAction::SetTrue, global = true, conflicts_with = "no_pager")]
    pub(crate) pager: bool,

    /// Disable paging for help output even on a TTY
    #[arg(long = "no-pager", action = ArgAction::SetTrue, global = true, conflicts_with = "pager")]
    pub(crate) no_pager: bool,

    /// Load session context from `.vizier/sessions/<id>/session.json` before running
    #[arg(short = 'l', long = "load-session", global = true)]
    pub(crate) load_session: Option<String>,

    /// Skip writing session logs (for compliance-sensitive runs)
    #[arg(short = 'n', long = "no-session", global = true)]
    pub(crate) no_session: bool,

    /// Agent selector to run for assistant-backed commands (e.g., `codex`, `gemini`, or a custom shim name). Overrides config for this run.
    #[arg(long = "agent", value_name = "SELECTOR", global = true)]
    pub(crate) agent: Option<String>,

    /// Bundled agent shim label to run (for example, `codex` or `gemini`); overrides config until the end of this invocation
    #[arg(long = "agent-label", value_name = "LABEL", global = true)]
    pub(crate) agent_label: Option<String>,

    /// Path to a custom agent script (stdout = assistant text; stderr = progress/errors); wins over labels/config for this run
    #[arg(long = "agent-command", value_name = "PATH", global = true)]
    pub(crate) agent_command: Option<String>,

    /// Emit the audit/outcome as JSON to stdout (human epilogues may be suppressed depending on the command)
    #[arg(short = 'j', long, global = true)]
    pub(crate) json: bool,

    /// Config file to load (supports JSON or TOML); bypasses the normal global+repo layering
    #[arg(short = 'C', long = "config-file", global = true)]
    pub(crate) config_file: Option<String>,

    /// Push the current branch to origin after mutating git history (approve/merge/save flows)
    #[arg(short = 'P', long, global = true)]
    pub(crate) push: bool,

    /// Leave changes staged/dirty instead of committing automatically (`[workflow] no_commit_default` sets the default posture)
    #[arg(long = "no-commit", action = ArgAction::SetTrue, global = true)]
    pub(crate) no_commit: bool,

    /// Deprecated: assistant-backed commands always enqueue jobs; use --follow to stream logs
    #[arg(long = "background", action = ArgAction::SetTrue, global = true)]
    pub(crate) background: bool,

    /// Attach to a scheduled job and stream logs until completion (requires MESSAGE/--file when stdin would otherwise be read)
    #[arg(
        long = "follow",
        action = ArgAction::SetTrue,
        global = true,
        conflicts_with_all = ["background", "no_background"]
    )]
    pub(crate) follow: bool,

    /// Deprecated: foreground execution is no longer supported for assistant-backed commands
    #[arg(
        long = "no-background",
        action = ArgAction::SetTrue,
        global = true,
        conflicts_with_all = ["background", "follow"]
    )]
    pub(crate) no_background: bool,

    /// Internal hook for background child processes; do not set manually
    #[arg(long = "background-job-id", hide = true, global = true)]
    pub(crate) background_job_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum ScopeArg {
    Save,
    Draft,
    Approve,
    Review,
    Merge,
}

impl From<ScopeArg> for config::CommandScope {
    fn from(value: ScopeArg) -> Self {
        match value {
            ScopeArg::Save => config::CommandScope::Save,
            ScopeArg::Draft => config::CommandScope::Draft,
            ScopeArg::Approve => config::CommandScope::Approve,
            ScopeArg::Review => config::CommandScope::Review,
            ScopeArg::Merge => config::CommandScope::Merge,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum JobLogStreamArg {
    Stdout,
    Stderr,
    Both,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum ListFormatArg {
    Block,
    Table,
    Json,
}

impl From<ListFormatArg> for config::ListFormat {
    fn from(value: ListFormatArg) -> Self {
        match value {
            ListFormatArg::Block => config::ListFormat::Block,
            ListFormatArg::Table => config::ListFormat::Table,
            ListFormatArg::Json => config::ListFormat::Json,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum BuildPipelineArg {
    Approve,
    #[value(name = "approve-review")]
    ApproveReview,
    #[value(name = "approve-review-merge")]
    ApproveReviewMerge,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum JobsScheduleFormatArg {
    Dag,
    Json,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum JobsListField {
    Job,
    Status,
    Created,
    After,
    Dependencies,
    Locks,
    Wait,
    WaitedOn,
    PinnedHead,
    Artifacts,
    Failed,
    Command,
}

impl JobsListField {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match normalize_field_key(value).as_str() {
            "job" => Some(Self::Job),
            "status" => Some(Self::Status),
            "created" => Some(Self::Created),
            "after" => Some(Self::After),
            "dependencies" => Some(Self::Dependencies),
            "locks" => Some(Self::Locks),
            "wait" => Some(Self::Wait),
            "waited on" => Some(Self::WaitedOn),
            "pinned head" => Some(Self::PinnedHead),
            "artifacts" => Some(Self::Artifacts),
            "failed" => Some(Self::Failed),
            "command" => Some(Self::Command),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Job => "Job",
            Self::Status => "Status",
            Self::Created => "Created",
            Self::After => "After",
            Self::Dependencies => "Dependencies",
            Self::Locks => "Locks",
            Self::Wait => "Wait",
            Self::WaitedOn => "Waited on",
            Self::PinnedHead => "Pinned head",
            Self::Artifacts => "Artifacts",
            Self::Failed => "Failed",
            Self::Command => "Command",
        }
    }

    pub(crate) fn json_key(self) -> &'static str {
        match self {
            Self::Job => "job",
            Self::Status => "status",
            Self::Created => "created",
            Self::After => "after",
            Self::Dependencies => "dependencies",
            Self::Locks => "locks",
            Self::Wait => "wait",
            Self::WaitedOn => "waited_on",
            Self::PinnedHead => "pinned_head",
            Self::Artifacts => "artifacts",
            Self::Failed => "failed",
            Self::Command => "command",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum JobsShowField {
    Job,
    Status,
    Pid,
    Started,
    Finished,
    ExitCode,
    Stdout,
    Stderr,
    Session,
    Outcome,
    Scope,
    Plan,
    Target,
    Branch,
    BuildPipeline,
    BuildTarget,
    BuildReviewMode,
    BuildSkipChecks,
    BuildKeepBranch,
    BuildDependencies,
    PatchFile,
    PatchIndex,
    PatchTotal,
    Revision,
    After,
    Dependencies,
    Locks,
    Wait,
    WaitedOn,
    PinnedHead,
    Artifacts,
    Worktree,
    WorktreeName,
    AgentBackend,
    AgentLabel,
    AgentCommand,
    AgentExit,
    CancelCleanup,
    CancelCleanupError,
    ConfigSnapshot,
    Command,
}

impl JobsShowField {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match normalize_field_key(value).as_str() {
            "job" => Some(Self::Job),
            "status" => Some(Self::Status),
            "pid" => Some(Self::Pid),
            "started" => Some(Self::Started),
            "finished" => Some(Self::Finished),
            "exit code" => Some(Self::ExitCode),
            "stdout" => Some(Self::Stdout),
            "stderr" => Some(Self::Stderr),
            "session" => Some(Self::Session),
            "outcome" => Some(Self::Outcome),
            "scope" => Some(Self::Scope),
            "plan" => Some(Self::Plan),
            "target" => Some(Self::Target),
            "branch" => Some(Self::Branch),
            "build pipeline" => Some(Self::BuildPipeline),
            "build target" => Some(Self::BuildTarget),
            "build review mode" => Some(Self::BuildReviewMode),
            "build skip checks" => Some(Self::BuildSkipChecks),
            "build keep branch" => Some(Self::BuildKeepBranch),
            "build dependencies" => Some(Self::BuildDependencies),
            "patch file" => Some(Self::PatchFile),
            "patch index" => Some(Self::PatchIndex),
            "patch total" => Some(Self::PatchTotal),
            "revision" => Some(Self::Revision),
            "after" => Some(Self::After),
            "dependencies" => Some(Self::Dependencies),
            "locks" => Some(Self::Locks),
            "wait" => Some(Self::Wait),
            "waited on" => Some(Self::WaitedOn),
            "pinned head" => Some(Self::PinnedHead),
            "artifacts" => Some(Self::Artifacts),
            "worktree" => Some(Self::Worktree),
            "worktree name" => Some(Self::WorktreeName),
            "agent backend" => Some(Self::AgentBackend),
            "agent label" => Some(Self::AgentLabel),
            "agent command" => Some(Self::AgentCommand),
            "agent exit" => Some(Self::AgentExit),
            "cancel cleanup" => Some(Self::CancelCleanup),
            "cancel cleanup error" => Some(Self::CancelCleanupError),
            "config snapshot" => Some(Self::ConfigSnapshot),
            "command" => Some(Self::Command),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Job => "Job",
            Self::Status => "Status",
            Self::Pid => "PID",
            Self::Started => "Started",
            Self::Finished => "Finished",
            Self::ExitCode => "Exit code",
            Self::Stdout => "Stdout",
            Self::Stderr => "Stderr",
            Self::Session => "Session",
            Self::Outcome => "Outcome",
            Self::Scope => "Scope",
            Self::Plan => "Plan",
            Self::Target => "Target",
            Self::Branch => "Branch",
            Self::BuildPipeline => "Build pipeline",
            Self::BuildTarget => "Build target",
            Self::BuildReviewMode => "Build review mode",
            Self::BuildSkipChecks => "Build skip checks",
            Self::BuildKeepBranch => "Build keep branch",
            Self::BuildDependencies => "Build dependencies",
            Self::PatchFile => "Patch file",
            Self::PatchIndex => "Patch index",
            Self::PatchTotal => "Patch total",
            Self::Revision => "Revision",
            Self::After => "After",
            Self::Dependencies => "Dependencies",
            Self::Locks => "Locks",
            Self::Wait => "Wait",
            Self::WaitedOn => "Waited on",
            Self::PinnedHead => "Pinned head",
            Self::Artifacts => "Artifacts",
            Self::Worktree => "Worktree",
            Self::WorktreeName => "Worktree name",
            Self::AgentBackend => "Agent backend",
            Self::AgentLabel => "Agent label",
            Self::AgentCommand => "Agent command",
            Self::AgentExit => "Agent exit",
            Self::CancelCleanup => "Cancel cleanup",
            Self::CancelCleanupError => "Cancel cleanup error",
            Self::ConfigSnapshot => "Config snapshot",
            Self::Command => "Command",
        }
    }

    pub(crate) fn json_key(self) -> &'static str {
        match self {
            Self::Job => "job",
            Self::Status => "status",
            Self::Pid => "pid",
            Self::Started => "started",
            Self::Finished => "finished",
            Self::ExitCode => "exit_code",
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Session => "session",
            Self::Outcome => "outcome",
            Self::Scope => "scope",
            Self::Plan => "plan",
            Self::Target => "target",
            Self::Branch => "branch",
            Self::BuildPipeline => "build_pipeline",
            Self::BuildTarget => "build_target",
            Self::BuildReviewMode => "build_review_mode",
            Self::BuildSkipChecks => "build_skip_checks",
            Self::BuildKeepBranch => "build_keep_branch",
            Self::BuildDependencies => "build_dependencies",
            Self::PatchFile => "patch_file",
            Self::PatchIndex => "patch_index",
            Self::PatchTotal => "patch_total",
            Self::Revision => "revision",
            Self::After => "after",
            Self::Dependencies => "dependencies",
            Self::Locks => "locks",
            Self::Wait => "wait",
            Self::WaitedOn => "waited_on",
            Self::PinnedHead => "pinned_head",
            Self::Artifacts => "artifacts",
            Self::Worktree => "worktree",
            Self::WorktreeName => "worktree_name",
            Self::AgentBackend => "agent_backend",
            Self::AgentLabel => "agent_label",
            Self::AgentCommand => "agent_command",
            Self::AgentExit => "agent_exit",
            Self::CancelCleanup => "cancel_cleanup",
            Self::CancelCleanupError => "cancel_cleanup_error",
            Self::ConfigSnapshot => "config_snapshot",
            Self::Command => "command",
        }
    }
}

impl From<JobLogStreamArg> for jobs::LogStream {
    fn from(value: JobLogStreamArg) -> Self {
        match value {
            JobLogStreamArg::Stdout => jobs::LogStream::Stdout,
            JobLogStreamArg::Stderr => jobs::LogStream::Stderr,
            JobLogStreamArg::Both => jobs::LogStream::Both,
        }
    }
}

#[derive(Subcommand, Debug)]
pub(crate) enum Commands {
    /// Show a short, workflow-oriented help page (or the full reference with --all)
    Help(HelpCmd),

    /// Generate an implementation-plan draft branch from an operator spec in a disposable worktree
    Draft(DraftCmd),

    /// Create build sessions and execute them through queued materialize/approve/review/merge jobs
    Build(BuildCmd),

    /// Execute one or more intent/spec files in deterministic order using build execution pipelines
    Patch(PatchCmd),

    /// List pending implementation-plan branches that are ahead of the target branch
    List(ListCmd),

    /// Create or reuse a plan workspace and print its path
    Cd(CdCmd),

    /// Remove Vizier-managed plan workspaces
    Clean(CleanCmd),

    /// Print the resolved configuration (global + repo + CLI overrides) and exit
    Plan(PlanCmd),

    /// Inspect detached Vizier background jobs
    Jobs(JobsCmd),

    /// Generate shell completion scripts
    Completions(CompletionsCmd),

    /// Internal completion entry point (invoked by shell integration)
    #[command(name = "__complete", hide = true)]
    Complete(HiddenCompleteCmd),

    /// Implement and commit a stored plan on its draft branch using a disposable worktree
    Approve(ApproveCmd),

    /// Review a plan branch (runs gate/checks first), stream critique, and optionally apply fixes
    Review(ReviewCmd),

    /// Merge approved plan branches back into the target branch (squash-by-default, CI/CD gate-aware)
    Merge(MergeCmd),

    /// Smoke-test the configured agent/display wiring without touching `.vizier`
    #[command(name = "test-display")]
    TestDisplay(TestDisplayCmd),

    /// Commit tracked changes with an LLM-generated message and update snapshot/narrative docs
    ///
    /// Examples:
    ///   vizier save                # defaults to HEAD
    ///   vizier save HEAD~3..HEAD   # explicit range
    ///   vizier save main           # single rev compared to workdir/index
    Save(SaveCmd),
}

#[derive(ClapArgs, Debug)]
#[command(group(
    ArgGroup::new("help_mode")
        .args(["all", "command"])
        .multiple(false)
        .required(false)
))]
pub(crate) struct HelpCmd {
    /// Print the full command reference (Clap help dump)
    #[arg(short = 'a', long = "all", action = ArgAction::SetTrue)]
    pub(crate) all: bool,

    /// Show help for a specific subcommand (equivalent to `vizier <command> --help`)
    #[arg(value_name = "COMMAND")]
    pub(crate) command: Option<String>,
}

#[derive(ClapArgs, Debug)]
pub(crate) struct DraftCmd {
    /// Operator spec used to seed the implementation plan
    #[arg(value_name = "SPEC")]
    pub(crate) spec: Option<String>,

    /// Read the operator spec from a file instead of inline text
    #[arg(short = 'f', long = "file", value_name = "PATH")]
    pub(crate) file: Option<PathBuf>,

    /// Override the derived plan/branch slug (letters, numbers, dashes only)
    #[arg(long = "name", value_name = "NAME")]
    pub(crate) name: Option<String>,

    /// Wait for one or more predecessor jobs to succeed before this job can run
    #[arg(long = "after", value_name = "JOB_ID", action = ArgAction::Append)]
    pub(crate) after: Vec<String>,
}

#[derive(ClapArgs, Debug)]
pub(crate) struct BuildCmd {
    /// Path to the build file (TOML or JSON)
    #[arg(
        short = 'f',
        long = "file",
        value_name = "PATH",
        conflicts_with = "command"
    )]
    pub(crate) file: Option<PathBuf>,

    /// Optional stable build id override (letters/numbers/dashes, no leading '.', no '/')
    #[arg(
        long = "name",
        value_name = "NAME",
        requires = "file",
        conflicts_with = "command"
    )]
    pub(crate) name: Option<String>,

    #[command(subcommand)]
    pub(crate) command: Option<BuildActionCmd>,
}

#[derive(ClapArgs, Debug)]
pub(crate) struct PatchCmd {
    /// One or more intent/spec files to process in the exact CLI order
    #[arg(value_name = "FILE", required = true)]
    pub(crate) files: Vec<PathBuf>,

    /// Override the default phase pipeline for each file
    #[arg(long = "pipeline", value_enum)]
    pub(crate) pipeline: Option<BuildPipelineArg>,

    /// Override the merge target branch used by downstream phases
    #[arg(long = "target", value_name = "BRANCH")]
    pub(crate) target: Option<String>,

    /// Resume from prior execution state by enqueueing only missing/non-terminal phases
    #[arg(long = "resume", action = ArgAction::SetTrue)]
    pub(crate) resume: bool,

    /// Skip interactive confirmation prompts
    #[arg(long = "yes", short = 'y')]
    pub(crate) assume_yes: bool,

    /// Wait for one or more predecessor jobs before the first queued patch root starts
    #[arg(long = "after", value_name = "JOB_ID", action = ArgAction::Append)]
    pub(crate) after: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub(crate) enum BuildActionCmd {
    /// Execute a succeeded build session by queueing materialize/approve/review/merge jobs
    Execute(BuildExecuteCmd),

    /// Internal scheduler hook to materialize a build step into a draft branch
    #[command(name = "__materialize", hide = true)]
    Materialize(BuildMaterializeCmd),
}

#[derive(ClapArgs, Debug)]
pub(crate) struct BuildExecuteCmd {
    /// Build session id (matches build/<id> and .vizier/implementation-plans/builds/<id>)
    #[arg(value_name = "BUILD")]
    pub(crate) build_id: String,

    /// Override the default phase pipeline for each step
    #[arg(long = "pipeline", value_enum)]
    pub(crate) pipeline: Option<BuildPipelineArg>,

    /// Resume from execution.json by enqueueing only missing/non-terminal phases
    #[arg(long = "resume", action = ArgAction::SetTrue)]
    pub(crate) resume: bool,

    /// Skip interactive confirmation prompt
    #[arg(long = "yes", short = 'y')]
    pub(crate) assume_yes: bool,
}

#[derive(ClapArgs, Debug)]
pub(crate) struct BuildMaterializeCmd {
    /// Build session id
    #[arg(value_name = "BUILD")]
    pub(crate) build_id: String,

    /// Build manifest step key (for example 01, 02a)
    #[arg(long = "step", value_name = "STEP")]
    pub(crate) step_key: String,

    /// Derived plan slug to materialize
    #[arg(long = "slug", value_name = "SLUG")]
    pub(crate) slug: String,

    /// Draft branch to write
    #[arg(long = "branch", value_name = "BRANCH")]
    pub(crate) branch: String,

    /// Branch to use when creating the draft branch base
    #[arg(long = "target", value_name = "BRANCH")]
    pub(crate) target: String,
}

#[derive(ClapArgs, Debug)]
pub(crate) struct ListCmd {
    /// Target branch to compare against (defaults to detected primary)
    #[arg(long = "target", value_name = "BRANCH")]
    pub(crate) target: Option<String>,

    /// Output format (block, table, json); overrides display.lists.list.format
    #[arg(long = "format", value_enum)]
    pub(crate) format: Option<ListFormatArg>,

    /// Comma-separated list of entry fields (e.g., Plan,Summary)
    #[arg(long = "fields", value_name = "FIELDS")]
    pub(crate) fields: Option<String>,
}

#[derive(ClapArgs, Debug)]
pub(crate) struct CdCmd {
    /// Plan slug to open a workspace for (tab-completes from pending plans)
    #[arg(value_name = "PLAN", add = crate::completions::plan_slug_completer())]
    pub(crate) plan: Option<String>,

    /// Branch to use instead of draft/<plan>
    #[arg(long = "branch", value_name = "BRANCH")]
    pub(crate) branch: Option<String>,

    /// Print only the workspace path (no formatted outcome block)
    #[arg(long = "path-only", action = ArgAction::SetTrue)]
    pub(crate) path_only: bool,
}

#[derive(ClapArgs, Debug)]
pub(crate) struct CleanCmd {
    /// Plan slug to clean (omit to remove all Vizier-managed workspaces)
    #[arg(value_name = "PLAN", add = crate::completions::plan_slug_completer())]
    pub(crate) plan: Option<String>,

    /// Remove workspaces without prompting for confirmation
    #[arg(long = "yes", short = 'y')]
    pub(crate) assume_yes: bool,
}

#[derive(ClapArgs, Debug)]
pub(crate) struct PlanCmd {}

#[derive(ClapArgs, Debug)]
pub(crate) struct JobsCmd {
    #[command(subcommand)]
    pub(crate) action: JobsAction,
}

#[derive(Subcommand, Debug)]
pub(crate) enum JobsAction {
    /// List tracked background jobs (succeeded hidden by default; failures optional)
    List {
        /// Include succeeded jobs (default hides them)
        #[arg(long = "all", short = 'a')]
        all: bool,

        /// Hide failed jobs from the visible list (use --all to include them)
        #[arg(long = "dismiss-failures")]
        dismiss_failures: bool,

        /// Output format (block, table, json); overrides display.lists.jobs.format
        #[arg(long = "format", value_enum)]
        format: Option<ListFormatArg>,
    },

    /// Show the dependency DAG for scheduled jobs
    Schedule {
        /// Include succeeded/failed/cancelled jobs (default shows active + blocked_by_dependency)
        #[arg(long = "all", short = 'a')]
        all: bool,

        /// Focus on a single job id and its ancestors/descendants
        #[arg(long = "job", value_name = "JOB")]
        job: Option<String>,

        /// Output format (dag, json)
        #[arg(long = "format", value_enum)]
        format: Option<JobsScheduleFormatArg>,

        /// Limit dependency expansion depth
        #[arg(long = "max-depth", value_name = "N", default_value_t = 3)]
        max_depth: usize,
    },

    /// Show details for a background job id
    Show {
        #[arg(value_name = "JOB")]
        job: String,

        /// Output format (block, table, json); overrides display.lists.jobs_show.format
        #[arg(long = "format", value_enum)]
        format: Option<ListFormatArg>,
    },

    /// Show a terse status line for a background job id
    Status {
        #[arg(value_name = "JOB")]
        job: String,
    },

    /// Rewind a failed/blocked job chain to its predecessor state and re-queue it
    Retry {
        #[arg(value_name = "JOB")]
        job: String,
    },

    /// Tail logs for a background job (stdout/stderr); add --follow to stream until completion
    Tail {
        #[arg(value_name = "JOB")]
        job: String,

        /// Which log to display
        #[arg(long = "stream", value_enum, default_value_t = JobLogStreamArg::Both)]
        stream: JobLogStreamArg,
    },

    /// Attach to both stdout and stderr for a running job
    Attach {
        #[arg(value_name = "JOB")]
        job: String,
    },

    /// Attempt to cancel a running background job
    Cancel {
        #[arg(value_name = "JOB")]
        job: String,

        /// Remove job-owned worktree(s) after the cancellation completes
        #[arg(long = "cleanup-worktree", action = ArgAction::SetTrue, conflicts_with = "no_cleanup_worktree")]
        cleanup_worktree: bool,

        /// Skip cleanup even if jobs.cancel.cleanup_worktree is enabled
        #[arg(long = "no-cleanup-worktree", action = ArgAction::SetTrue)]
        no_cleanup_worktree: bool,
    },

    /// Garbage-collect completed jobs older than N days (default 7)
    Gc {
        #[arg(long = "days", value_name = "DAYS", default_value_t = 7)]
        days: u64,
    },
}

#[derive(ClapArgs, Debug)]
pub(crate) struct ApproveCmd {
    /// Plan slug to approve (tab-completes from pending plans)
    #[arg(value_name = "PLAN", add = crate::completions::plan_slug_completer())]
    pub(crate) plan: String,

    /// Destination branch for preview/reference (defaults to detected primary)
    #[arg(long = "target", value_name = "BRANCH")]
    pub(crate) target: Option<String>,

    /// Draft branch name when it deviates from draft/<plan>
    #[arg(long = "branch", value_name = "BRANCH")]
    pub(crate) branch: Option<String>,

    /// Skip the confirmation prompt before applying the plan on the draft branch
    #[arg(long = "yes", short = 'y')]
    pub(crate) assume_yes: bool,

    /// Path to an approve stop-condition script (defaults to approve.stop_condition.script)
    #[arg(long = "stop-condition-script", value_name = "PATH")]
    pub(crate) stop_condition_script: Option<PathBuf>,

    /// Number of stop-condition retries before giving up (`approve.stop_condition.retries` by default)
    #[arg(long = "stop-condition-retries", value_name = "COUNT")]
    pub(crate) stop_condition_retries: Option<u32>,

    /// Wait for one or more predecessor jobs to succeed before this job can run
    #[arg(long = "after", value_name = "JOB_ID", action = ArgAction::Append)]
    pub(crate) after: Vec<String>,
}

#[derive(ClapArgs, Debug)]
pub(crate) struct ReviewCmd {
    /// Plan slug to review (tab-completes from pending plans)
    #[arg(value_name = "PLAN", add = crate::completions::plan_slug_completer())]
    pub(crate) plan: Option<String>,

    /// Destination branch for diff context (defaults to detected primary)
    #[arg(long = "target", value_name = "BRANCH")]
    pub(crate) target: Option<String>,

    /// Draft branch name when it deviates from draft/<plan>
    #[arg(long = "branch", value_name = "BRANCH")]
    pub(crate) branch: Option<String>,

    /// Skip the fix-up prompt and apply backend fixes automatically
    #[arg(long = "yes", short = 'y')]
    pub(crate) assume_yes: bool,

    /// Produce the critique without attempting fixes
    #[arg(long = "review-only")]
    pub(crate) review_only: bool,

    /// Write the critique to vizier-review.md in the repo root and skip fixes
    #[arg(long = "review-file")]
    pub(crate) review_file: bool,

    /// Skip running configured review checks (e.g., cargo test); merge CI/CD gate still runs once per review
    #[arg(long = "skip-checks")]
    pub(crate) skip_checks: bool,

    /// Path to a CI/CD gate script for this review (defaults to merge.cicd_gate.script)
    #[arg(long = "cicd-script", value_name = "PATH")]
    pub(crate) cicd_script: Option<PathBuf>,

    /// Force-enable backend remediation when the CI/CD script fails
    #[arg(long = "auto-cicd-fix", action = ArgAction::SetTrue, conflicts_with = "no_auto_cicd_fix")]
    pub(crate) auto_cicd_fix: bool,

    /// Disable backend remediation even if configured
    #[arg(long = "no-auto-cicd-fix", action = ArgAction::SetTrue, conflicts_with = "auto_cicd_fix")]
    pub(crate) no_auto_cicd_fix: bool,

    /// Number of remediation attempts before aborting (`merge.cicd_gate.retries` by default)
    #[arg(long = "cicd-retries", value_name = "COUNT")]
    pub(crate) cicd_retries: Option<u32>,

    /// Wait for one or more predecessor jobs to succeed before this job can run
    #[arg(long = "after", value_name = "JOB_ID", action = ArgAction::Append)]
    pub(crate) after: Vec<String>,
}

#[derive(ClapArgs, Debug)]
pub(crate) struct MergeCmd {
    /// Plan slug to merge (tab-completes from pending plans)
    #[arg(value_name = "PLAN", add = crate::completions::plan_slug_completer())]
    pub(crate) plan: Option<String>,

    /// Destination branch for merge (defaults to detected primary)
    #[arg(long = "target", value_name = "BRANCH")]
    pub(crate) target: Option<String>,

    /// Draft branch name when it deviates from draft/<plan>
    #[arg(long = "branch", value_name = "BRANCH")]
    pub(crate) branch: Option<String>,

    /// Skip the merge confirmation prompt
    #[arg(long = "yes", short = 'y')]
    pub(crate) assume_yes: bool,

    /// Keep the draft branch locally after merge (default is to delete)
    #[arg(long = "keep-branch")]
    pub(crate) keep_branch: bool,

    /// Optional note appended to the merge commit body
    #[arg(long = "note", value_name = "TEXT")]
    pub(crate) note: Option<String>,

    /// Attempt backend-backed auto-resolution when conflicts arise
    #[arg(long = "auto-resolve-conflicts")]
    pub(crate) auto_resolve_conflicts: bool,

    /// Skip backend conflict auto-resolution even when configured
    #[arg(
        long = "no-auto-resolve-conflicts",
        action = ArgAction::SetTrue,
        conflicts_with = "auto_resolve_conflicts"
    )]
    pub(crate) no_auto_resolve_conflicts: bool,

    /// Only finalize a previously conflicted merge; fail if no pending Vizier merge exists
    #[arg(long = "complete-conflict")]
    pub(crate) complete_conflict: bool,

    /// Path to a CI/CD gate script (defaults to merge.cicd_gate.script)
    #[arg(long = "cicd-script", value_name = "PATH")]
    pub(crate) cicd_script: Option<PathBuf>,

    /// Force-enable backend remediation when the CI/CD script fails
    #[arg(long = "auto-cicd-fix", action = ArgAction::SetTrue, conflicts_with = "no_auto_cicd_fix")]
    pub(crate) auto_cicd_fix: bool,

    /// Disable backend remediation even if configured
    #[arg(long = "no-auto-cicd-fix", action = ArgAction::SetTrue, conflicts_with = "auto_cicd_fix")]
    pub(crate) no_auto_cicd_fix: bool,

    /// Number of remediation attempts before aborting (`merge.cicd_gate.retries` by default)
    #[arg(long = "cicd-retries", value_name = "COUNT")]
    pub(crate) cicd_retries: Option<u32>,

    /// Squash implementation commits before creating the merge commit (default follows `[merge] squash`)
    #[arg(long = "squash", action = ArgAction::SetTrue, conflicts_with = "no_squash")]
    pub(crate) squash: bool,

    /// Preserve implementation commits (legacy behavior; overrides `[merge] squash = true`)
    #[arg(long = "no-squash", action = ArgAction::SetTrue, conflicts_with = "squash")]
    pub(crate) no_squash: bool,

    /// Parent index to use when cherry-picking merge commits in squash mode (1-based)
    #[arg(long = "squash-mainline", value_name = "PARENT_INDEX")]
    pub(crate) squash_mainline: Option<u32>,

    /// Wait for one or more predecessor jobs to succeed before this job can run
    #[arg(long = "after", value_name = "JOB_ID", action = ArgAction::Append)]
    pub(crate) after: Vec<String>,
}

#[derive(ClapArgs, Debug)]
pub(crate) struct CompletionsCmd {
    /// Shell to generate completion script for
    #[arg(value_enum)]
    pub(crate) shell: CompletionShell,
}

#[derive(ClapArgs, Debug)]
pub(crate) struct HiddenCompleteCmd {}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum CompletionShell {
    Bash,
    Zsh,
    Fish,
    Elvish,
    Powershell,
}

impl From<CompletionShell> for Shell {
    fn from(value: CompletionShell) -> Self {
        match value {
            CompletionShell::Bash => Shell::Bash,
            CompletionShell::Zsh => Shell::Zsh,
            CompletionShell::Fish => Shell::Fish,
            CompletionShell::Elvish => Shell::Elvish,
            CompletionShell::Powershell => Shell::PowerShell,
        }
    }
}

#[derive(ClapArgs, Debug)]
#[command(
    group = ArgGroup::new("commit_msg_src")
        .args(["commit_message", "commit_message_editor"])
        .multiple(false)
)]
pub(crate) struct SaveCmd {
    /// Commit reference or range; defaults to HEAD if omitted.
    ///
    /// Examples: `HEAD`, `HEAD~3..HEAD`, `feature-branch`
    #[arg(value_name = "REV_OR_RANGE", default_value = "HEAD")]
    pub(crate) rev_or_range: String,

    /// Developer note to append to the *code* commit message
    #[arg(short = 'm', long = "message")]
    pub(crate) commit_message: Option<String>,

    /// Open $EDITOR to compose the commit message
    #[arg(short = 'M', long = "edit-message")]
    pub(crate) commit_message_editor: bool,

    /// Wait for one or more predecessor jobs to succeed before this job can run
    #[arg(long = "after", value_name = "JOB_ID", action = ArgAction::Append)]
    pub(crate) after: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedInput {
    pub(crate) text: String,
    pub(crate) origin: InputOrigin,
}

#[derive(Debug, Clone)]
pub(crate) enum InputOrigin {
    Inline,
    File(PathBuf),
    Stdin,
}

impl From<InputOrigin> for SpecSource {
    fn from(origin: InputOrigin) -> Self {
        match origin {
            InputOrigin::Inline => SpecSource::Inline,
            InputOrigin::File(path) => SpecSource::File(path),
            InputOrigin::Stdin => SpecSource::Stdin,
        }
    }
}

#[derive(ClapArgs, Debug)]
pub(crate) struct TestDisplayCmd {
    /// Command scope to resolve agent settings from
    #[arg(long = "scope", value_enum, default_value_t = ScopeArg::Save)]
    pub(crate) scope: ScopeArg,

    /// Override the default smoke-test prompt
    #[arg(long = "prompt", value_name = "TEXT")]
    pub(crate) prompt: Option<String>,

    /// Dump captured stdout/stderr verbatim instead of a summarized snippet
    #[arg(long = "raw", action = ArgAction::SetTrue)]
    pub(crate) raw: bool,

    /// Timeout in seconds before aborting the agent run
    #[arg(long = "timeout", value_name = "SECONDS")]
    pub(crate) timeout_secs: Option<u64>,

    /// Disable stdbuf/unbuffer/script wrapping for debugging agent output
    #[arg(long = "no-wrapper", action = ArgAction::SetTrue)]
    pub(crate) no_wrapper: bool,

    /// Write a session log for this smoke test (defaults to off)
    #[arg(long = "session", action = ArgAction::SetTrue, conflicts_with = "no_session")]
    pub(crate) session: bool,

    /// Explicitly disable session logging (default)
    #[arg(long = "no-session", action = ArgAction::SetTrue, conflicts_with = "session")]
    pub(crate) no_session: bool,
}

pub(crate) fn normalize_field_key(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn normalize_labels(labels: &HashMap<String, String>) -> HashMap<String, String> {
    labels
        .iter()
        .map(|(key, value)| (normalize_field_key(key), value.clone()))
        .collect()
}

pub(crate) fn resolve_label(labels: &HashMap<String, String>, default_label: &str) -> String {
    let key = normalize_field_key(default_label);
    labels
        .get(&key)
        .cloned()
        .unwrap_or_else(|| default_label.to_string())
}

pub(crate) fn parse_fields<T, F>(context: &str, values: &[String], parser: F) -> Vec<T>
where
    F: Fn(&str) -> Option<T>,
{
    let mut fields = Vec::new();
    for value in values {
        if let Some(field) = parser(value) {
            fields.push(field);
        } else {
            display::warn(format!("{context}: unknown field `{value}`; ignoring"));
        }
    }
    fields
}
