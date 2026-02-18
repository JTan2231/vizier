use std::collections::HashMap;
use std::num::NonZeroU32;

use clap::{ArgAction, ArgGroup, Args as ClapArgs, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use vizier_core::{config, display};

use crate::jobs;

/// A CLI for LLM project management.
#[derive(Parser, Debug)]
#[command(
    name = "vizier",
    version,
    about,
    disable_help_subcommand = true,
    arg_required_else_help = true,
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

    /// Internal hook for child runs to disable help paging in detached/non-interactive contexts
    #[arg(long = "no-pager", action = ArgAction::SetTrue, hide = true, global = true)]
    pub(crate) no_pager: bool,

    /// Load session context from `.vizier/sessions/<id>/session.json` before running
    #[arg(short = 'l', long = "load-session", global = true)]
    pub(crate) load_session: Option<String>,

    /// Skip writing session logs (for compliance-sensitive runs)
    #[arg(short = 'n', long = "no-session", global = true)]
    pub(crate) no_session: bool,

    /// Config file to load (supports JSON or TOML); bypasses the normal global+repo layering
    #[arg(short = 'C', long = "config-file", global = true)]
    pub(crate) config_file: Option<String>,
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

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum JobsScheduleFormatArg {
    Summary,
    Dag,
    Json,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum JobsActionFormatArg {
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum RunFormatArg {
    Text,
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
    ApprovalRequired,
    ApprovalState,
    ApprovalDecidedBy,
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
            "approval required" => Some(Self::ApprovalRequired),
            "approval state" => Some(Self::ApprovalState),
            "approval decided by" => Some(Self::ApprovalDecidedBy),
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
            Self::ApprovalRequired => "Approval required",
            Self::ApprovalState => "Approval state",
            Self::ApprovalDecidedBy => "Approval decided by",
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
            Self::ApprovalRequired => "approval_required",
            Self::ApprovalState => "approval_state",
            Self::ApprovalDecidedBy => "approval_decided_by",
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
    WorkflowRun,
    WorkflowTemplate,
    WorkflowTemplateVersion,
    WorkflowNode,
    WorkflowNodeAttempt,
    WorkflowNodeOutcome,
    WorkflowPayloadRefs,
    WorkflowExecutorClass,
    WorkflowExecutorOperation,
    WorkflowControlPolicy,
    WorkflowPolicySnapshot,
    WorkflowGates,
    PatchFile,
    PatchIndex,
    PatchTotal,
    Revision,
    After,
    Dependencies,
    Locks,
    Wait,
    WaitedOn,
    ApprovalRequired,
    ApprovalState,
    ApprovalRequestedAt,
    ApprovalRequestedBy,
    ApprovalDecidedAt,
    ApprovalDecidedBy,
    ApprovalReason,
    PinnedHead,
    Artifacts,
    ExecutionRoot,
    Worktree,
    WorktreeName,
    AgentBackend,
    AgentLabel,
    AgentCommand,
    AgentExit,
    CancelCleanup,
    CancelCleanupError,
    RetryCleanup,
    RetryCleanupError,
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
            "workflow run" => Some(Self::WorkflowRun),
            "workflow template" => Some(Self::WorkflowTemplate),
            "workflow template version" => Some(Self::WorkflowTemplateVersion),
            "workflow node" => Some(Self::WorkflowNode),
            "workflow node attempt" => Some(Self::WorkflowNodeAttempt),
            "workflow node outcome" => Some(Self::WorkflowNodeOutcome),
            "workflow payload refs" => Some(Self::WorkflowPayloadRefs),
            "workflow executor class" => Some(Self::WorkflowExecutorClass),
            "workflow executor operation" => Some(Self::WorkflowExecutorOperation),
            "workflow control policy" => Some(Self::WorkflowControlPolicy),
            "workflow policy snapshot" => Some(Self::WorkflowPolicySnapshot),
            "workflow gates" => Some(Self::WorkflowGates),
            "patch file" => Some(Self::PatchFile),
            "patch index" => Some(Self::PatchIndex),
            "patch total" => Some(Self::PatchTotal),
            "revision" => Some(Self::Revision),
            "after" => Some(Self::After),
            "dependencies" => Some(Self::Dependencies),
            "locks" => Some(Self::Locks),
            "wait" => Some(Self::Wait),
            "waited on" => Some(Self::WaitedOn),
            "approval required" => Some(Self::ApprovalRequired),
            "approval state" => Some(Self::ApprovalState),
            "approval requested at" => Some(Self::ApprovalRequestedAt),
            "approval requested by" => Some(Self::ApprovalRequestedBy),
            "approval decided at" => Some(Self::ApprovalDecidedAt),
            "approval decided by" => Some(Self::ApprovalDecidedBy),
            "approval reason" => Some(Self::ApprovalReason),
            "pinned head" => Some(Self::PinnedHead),
            "artifacts" => Some(Self::Artifacts),
            "execution root" => Some(Self::ExecutionRoot),
            "worktree" => Some(Self::Worktree),
            "worktree name" => Some(Self::WorktreeName),
            "agent backend" => Some(Self::AgentBackend),
            "agent label" => Some(Self::AgentLabel),
            "agent command" => Some(Self::AgentCommand),
            "agent exit" => Some(Self::AgentExit),
            "cancel cleanup" => Some(Self::CancelCleanup),
            "cancel cleanup error" => Some(Self::CancelCleanupError),
            "retry cleanup" => Some(Self::RetryCleanup),
            "retry cleanup error" => Some(Self::RetryCleanupError),
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
            Self::WorkflowRun => "Workflow run",
            Self::WorkflowTemplate => "Workflow template",
            Self::WorkflowTemplateVersion => "Workflow template version",
            Self::WorkflowNode => "Workflow node",
            Self::WorkflowNodeAttempt => "Workflow node attempt",
            Self::WorkflowNodeOutcome => "Workflow node outcome",
            Self::WorkflowPayloadRefs => "Workflow payload refs",
            Self::WorkflowExecutorClass => "Workflow executor class",
            Self::WorkflowExecutorOperation => "Workflow executor operation",
            Self::WorkflowControlPolicy => "Workflow control policy",
            Self::WorkflowPolicySnapshot => "Workflow policy snapshot",
            Self::WorkflowGates => "Workflow gates",
            Self::PatchFile => "Patch file",
            Self::PatchIndex => "Patch index",
            Self::PatchTotal => "Patch total",
            Self::Revision => "Revision",
            Self::After => "After",
            Self::Dependencies => "Dependencies",
            Self::Locks => "Locks",
            Self::Wait => "Wait",
            Self::WaitedOn => "Waited on",
            Self::ApprovalRequired => "Approval required",
            Self::ApprovalState => "Approval state",
            Self::ApprovalRequestedAt => "Approval requested at",
            Self::ApprovalRequestedBy => "Approval requested by",
            Self::ApprovalDecidedAt => "Approval decided at",
            Self::ApprovalDecidedBy => "Approval decided by",
            Self::ApprovalReason => "Approval reason",
            Self::PinnedHead => "Pinned head",
            Self::Artifacts => "Artifacts",
            Self::ExecutionRoot => "Execution root",
            Self::Worktree => "Worktree",
            Self::WorktreeName => "Worktree name",
            Self::AgentBackend => "Agent backend",
            Self::AgentLabel => "Agent label",
            Self::AgentCommand => "Agent command",
            Self::AgentExit => "Agent exit",
            Self::CancelCleanup => "Cancel cleanup",
            Self::CancelCleanupError => "Cancel cleanup error",
            Self::RetryCleanup => "Retry cleanup",
            Self::RetryCleanupError => "Retry cleanup error",
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
            Self::WorkflowRun => "workflow_run",
            Self::WorkflowTemplate => "workflow_template",
            Self::WorkflowTemplateVersion => "workflow_template_version",
            Self::WorkflowNode => "workflow_node",
            Self::WorkflowNodeAttempt => "workflow_node_attempt",
            Self::WorkflowNodeOutcome => "workflow_node_outcome",
            Self::WorkflowPayloadRefs => "workflow_payload_refs",
            Self::WorkflowExecutorClass => "workflow_executor_class",
            Self::WorkflowExecutorOperation => "workflow_executor_operation",
            Self::WorkflowControlPolicy => "workflow_control_policy",
            Self::WorkflowPolicySnapshot => "workflow_policy_snapshot",
            Self::WorkflowGates => "workflow_gates",
            Self::PatchFile => "patch_file",
            Self::PatchIndex => "patch_index",
            Self::PatchTotal => "patch_total",
            Self::Revision => "revision",
            Self::After => "after",
            Self::Dependencies => "dependencies",
            Self::Locks => "locks",
            Self::Wait => "wait",
            Self::WaitedOn => "waited_on",
            Self::ApprovalRequired => "approval_required",
            Self::ApprovalState => "approval_state",
            Self::ApprovalRequestedAt => "approval_requested_at",
            Self::ApprovalRequestedBy => "approval_requested_by",
            Self::ApprovalDecidedAt => "approval_decided_at",
            Self::ApprovalDecidedBy => "approval_decided_by",
            Self::ApprovalReason => "approval_reason",
            Self::PinnedHead => "pinned_head",
            Self::Artifacts => "artifacts",
            Self::ExecutionRoot => "execution_root",
            Self::Worktree => "worktree",
            Self::WorktreeName => "worktree_name",
            Self::AgentBackend => "agent_backend",
            Self::AgentLabel => "agent_label",
            Self::AgentCommand => "agent_command",
            Self::AgentExit => "agent_exit",
            Self::CancelCleanup => "cancel_cleanup",
            Self::CancelCleanupError => "cancel_cleanup_error",
            Self::RetryCleanup => "retry_cleanup",
            Self::RetryCleanupError => "retry_cleanup_error",
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
    /// Show a short, command-oriented help page (or the full reference with --all)
    Help(HelpCmd),

    /// Initialize the repository for Vizier usage (idempotent) or validate init state
    Init(InitCmd),

    /// List pending implementation-plan branches that are ahead of the target branch
    List(ListCmd),

    /// Create or reuse a plan workspace and print its path
    Cd(CdCmd),

    /// Remove Vizier-managed plan workspaces
    Clean(CleanCmd),

    /// Inspect detached Vizier background jobs
    Jobs(JobsCmd),

    /// Compile and enqueue a workflow run from an alias, selector, or template file
    Run(RunCmd),

    /// Generate shell completion scripts
    Completions(CompletionsCmd),

    /// Internal completion entry point (invoked by shell integration)
    #[command(name = "__complete", hide = true)]
    Complete(HiddenCompleteCmd),

    /// Internal workflow-node runtime entry point (invoked by scheduler jobs)
    #[command(name = "__workflow-node", hide = true)]
    WorkflowNode(HiddenWorkflowNodeCmd),

    /// Create a local release commit and optional annotated tag from conventional commits
    Release(ReleaseCmd),
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
pub(crate) struct InitCmd {
    /// Validate initialization state without mutating files
    #[arg(long = "check", action = ArgAction::SetTrue)]
    pub(crate) check: bool,
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
pub(crate) struct JobsCmd {
    #[command(subcommand)]
    pub(crate) action: JobsAction,
}

#[derive(ClapArgs, Debug)]
pub(crate) struct RunCmd {
    /// Workflow source: alias, selector, file:<path>, or direct .toml/.json path
    #[arg(value_name = "FLOW")]
    pub(crate) flow: String,

    /// Ordered workflow inputs; mapped by template [cli].positional before enqueue
    #[arg(value_name = "INPUT")]
    pub(crate) inputs: Vec<String>,

    /// Template parameter override (KEY=VALUE); repeatable
    #[arg(long = "set", value_name = "KEY=VALUE", action = ArgAction::Append)]
    pub(crate) set: Vec<String>,

    /// External predecessor dependency; root jobs wait on JOB_ID or run:RUN_ID
    #[arg(long = "after", value_name = "REF", action = ArgAction::Append)]
    pub(crate) after: Vec<String>,

    /// Require explicit approval before root jobs can start
    #[arg(long = "require-approval", action = ArgAction::SetTrue, conflicts_with = "no_require_approval")]
    pub(crate) require_approval: bool,

    /// Disable approval gating for root jobs (overrides template-root approval)
    #[arg(long = "no-require-approval", action = ArgAction::SetTrue)]
    pub(crate) no_require_approval: bool,

    /// Wait for terminal run state and stream progress in text mode
    #[arg(long = "follow", action = ArgAction::SetTrue)]
    pub(crate) follow: bool,

    /// Number of times to enqueue and execute the same workflow in strict sequence
    #[arg(long = "repeat", value_name = "N", default_value_t = NonZeroU32::MIN)]
    pub(crate) repeat: NonZeroU32,

    /// Output format (text, json)
    #[arg(long = "format", value_enum, default_value_t = RunFormatArg::Text)]
    pub(crate) format: RunFormatArg,
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

    /// Show scheduled jobs and dependency relationships
    Schedule {
        /// Include succeeded/failed/cancelled jobs (default shows active/blocked plus failed blockers)
        #[arg(long = "all", short = 'a')]
        all: bool,

        /// Focus on a single job id and its ancestors/descendants
        #[arg(long = "job", value_name = "JOB")]
        job: Option<String>,

        /// Output format (summary, dag, json)
        #[arg(long = "format", value_enum)]
        format: Option<JobsScheduleFormatArg>,

        /// Render an interactive, refreshing schedule dashboard (TTY + ANSI only)
        #[arg(long = "watch", action = ArgAction::SetTrue)]
        watch: bool,

        /// Limit schedule rows shown per refresh when --watch is enabled
        #[arg(long = "top", value_name = "N", default_value_t = 10)]
        top: usize,

        /// Poll interval in milliseconds when --watch is enabled
        #[arg(long = "interval-ms", value_name = "MS", default_value_t = 500)]
        interval_ms: u64,

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

        /// Output format (text, json)
        #[arg(long = "format", value_enum, default_value_t = JobsActionFormatArg::Text)]
        format: JobsActionFormatArg,
    },

    /// Rewind a failed/blocked job chain to its predecessor state and re-queue it
    Retry {
        #[arg(value_name = "JOB")]
        job: String,

        /// Output format (text, json)
        #[arg(long = "format", value_enum, default_value_t = JobsActionFormatArg::Text)]
        format: JobsActionFormatArg,
    },

    /// Approve a queued job that is waiting on explicit human approval
    Approve {
        #[arg(value_name = "JOB")]
        job: String,

        /// Output format (text, json)
        #[arg(long = "format", value_enum, default_value_t = JobsActionFormatArg::Text)]
        format: JobsActionFormatArg,
    },

    /// Reject a queued job that is waiting on explicit human approval
    Reject {
        #[arg(value_name = "JOB")]
        job: String,

        /// Optional reason recorded in job/outcome metadata
        #[arg(long = "reason", value_name = "TEXT")]
        reason: Option<String>,

        /// Output format (text, json)
        #[arg(long = "format", value_enum, default_value_t = JobsActionFormatArg::Text)]
        format: JobsActionFormatArg,
    },

    /// Tail logs for a background job (stdout/stderr)
    Tail {
        #[arg(value_name = "JOB")]
        job: String,

        /// Which log to display
        #[arg(long = "stream", value_enum, default_value_t = JobLogStreamArg::Both)]
        stream: JobLogStreamArg,

        /// Stream until completion
        #[arg(long = "follow", action = ArgAction::SetTrue)]
        follow: bool,
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
#[command(group(
    ArgGroup::new("release_bump")
        .args(["major", "minor", "patch"])
        .multiple(false)
        .required(false)
))]
pub(crate) struct ReleaseCmd {
    /// Print the computed release plan and notes without creating a commit or tag
    #[arg(long = "dry-run", action = ArgAction::SetTrue)]
    pub(crate) dry_run: bool,

    /// Skip the confirmation prompt before creating release artifacts
    #[arg(long = "yes", short = 'y')]
    pub(crate) assume_yes: bool,

    /// Force a major version bump (overrides auto detection)
    #[arg(long = "major", action = ArgAction::SetTrue)]
    pub(crate) major: bool,

    /// Force a minor version bump (overrides auto detection)
    #[arg(long = "minor", action = ArgAction::SetTrue)]
    pub(crate) minor: bool,

    /// Force a patch version bump (overrides auto detection)
    #[arg(long = "patch", action = ArgAction::SetTrue)]
    pub(crate) patch: bool,

    /// Maximum release-note entries per section before summarizing overflow
    #[arg(long = "max-commits", value_name = "N", default_value_t = 20)]
    pub(crate) max_commits: usize,

    /// Create only the release commit and skip annotated tag creation
    #[arg(long = "no-tag", action = ArgAction::SetTrue)]
    pub(crate) no_tag: bool,
}

#[derive(ClapArgs, Debug)]
pub(crate) struct CompletionsCmd {
    /// Shell to generate completion script for
    #[arg(value_enum)]
    pub(crate) shell: CompletionShell,
}

#[derive(ClapArgs, Debug)]
pub(crate) struct HiddenCompleteCmd {}

#[derive(ClapArgs, Debug)]
pub(crate) struct HiddenWorkflowNodeCmd {
    /// Scheduler job id for the node runtime invocation
    #[arg(long = "job-id", value_name = "JOB_ID")]
    pub(crate) job_id: String,
}

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

#[cfg(test)]
mod tests {
    use super::{Cli, Commands};
    use clap::Parser;

    #[test]
    fn run_repeat_defaults_to_one() {
        let cli = Cli::try_parse_from(["vizier", "run", "draft"]).expect("parse run args");
        let Commands::Run(cmd) = cli.command else {
            panic!("expected run command");
        };
        assert_eq!(cmd.repeat.get(), 1);
    }

    #[test]
    fn run_repeat_accepts_positive_values() {
        let cli =
            Cli::try_parse_from(["vizier", "run", "draft", "--repeat", "3"]).expect("parse repeat");
        let Commands::Run(cmd) = cli.command else {
            panic!("expected run command");
        };
        assert_eq!(cmd.repeat.get(), 3);
    }

    #[test]
    fn run_repeat_rejects_zero() {
        let err = Cli::try_parse_from(["vizier", "run", "draft", "--repeat", "0"])
            .expect_err("zero repeat should fail");
        let rendered = err.to_string();
        assert!(
            rendered.contains("--repeat"),
            "expected clap error to mention --repeat: {rendered}"
        );
    }

    #[test]
    fn run_repeat_rejects_negative() {
        let err = Cli::try_parse_from(["vizier", "run", "draft", "--repeat=-2"])
            .expect_err("negative repeat should fail");
        let rendered = err.to_string();
        assert!(
            rendered.contains("--repeat"),
            "expected clap error to mention --repeat: {rendered}"
        );
    }
}
