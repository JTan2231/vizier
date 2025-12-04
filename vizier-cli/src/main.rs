use std::{
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use clap::{
    ArgAction, ArgGroup, Args as ClapArgs, ColorChoice, CommandFactory, FromArgMatches, Parser,
    Subcommand, ValueEnum, error::ErrorKind,
};
use clap_complete::Shell;
use serde_json::json;
use vizier_core::{
    auditor, config,
    display::{self, LogLevel},
    tools, vcs,
};

mod actions;
use crate::actions::*;
mod completions;
mod jobs;
mod plan;
mod workspace;
use crate::jobs::JobStatus;

/// A CLI for LLM project management.
#[derive(Parser, Debug)]
#[command(
    name = "vizier",
    version,
    about,
    // Show help when you forget a subcommand
    arg_required_else_help = true,
    // Make version available to subcommands automatically
    propagate_version = true
)]
struct Cli {
    #[command(flatten)]
    global: GlobalOpts,

    #[command(subcommand)]
    command: Commands,
}

#[derive(ClapArgs, Debug)]
struct GlobalOpts {
    /// Increase stderr verbosity (`-v` = info, `-vv` = debug); quiet wins over verbose, and output still honors TTY/--no-ansi gating
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count, global = true)]
    verbose: u8,

    /// Silence progress/history; only errors and explicit output (help/outcome) remain
    #[arg(short = 'q', long, global = true)]
    quiet: bool,

    /// Enable debug logging (alias for -vv; kept for parity with older workflows)
    #[arg(short = 'd', long, global = true)]
    debug: bool,

    /// Disable ANSI control sequences even on TTYs (non-TTY is always plain); useful for CI/log scrapers
    #[arg(long = "no-ansi", global = true)]
    no_ansi: bool,

    /// Page help output when available; defaults to TTY-only paging and honors $VIZIER_PAGER
    #[arg(long = "pager", action = ArgAction::SetTrue, global = true, conflicts_with = "no_pager")]
    pager: bool,

    /// Disable paging for help output even on a TTY
    #[arg(long = "no-pager", action = ArgAction::SetTrue, global = true, conflicts_with = "pager")]
    no_pager: bool,

    /// Load session context from `.vizier/sessions/<id>/session.json` (or legacy config dir) before running
    #[arg(short = 'l', long = "load-session", global = true)]
    load_session: Option<String>,

    /// Skip writing session logs (for compliance-sensitive runs)
    #[arg(short = 'n', long = "no-session", global = true)]
    no_session: bool,

    /// Agent selector to run for assistant-backed commands (e.g., `codex`, `gemini`, or a custom shim name). Overrides config for this run.
    #[arg(long = "agent", value_name = "SELECTOR", global = true)]
    agent: Option<String>,

    /// Deprecated backend selector (`agent` or `gemini`). Use `--agent` instead.
    #[arg(long = "backend", value_enum, global = true, hide = true)]
    backend: Option<BackendArg>,

    /// Bundled agent shim label to run (for example, `codex` or `gemini`); overrides config until the end of this invocation
    #[arg(long = "agent-label", value_name = "LABEL", global = true)]
    agent_label: Option<String>,

    /// Path to a custom agent script (stdout = assistant text; stderr = progress/errors); wins over labels/config for this run
    #[arg(long = "agent-command", value_name = "PATH", global = true)]
    agent_command: Option<String>,

    /// Emit the audit/outcome as JSON to stdout (human epilogues may be suppressed depending on the command)
    #[arg(short = 'j', long, global = true)]
    json: bool,

    /// Config file to load (supports JSON or TOML); bypasses the normal global+repo layering
    #[arg(short = 'C', long = "config-file", global = true)]
    config_file: Option<String>,

    /// Push the current branch to origin after mutating git history (approve/merge/save flows)
    #[arg(short = 'P', long, global = true)]
    push: bool,

    /// Leave changes staged/dirty instead of committing automatically (`[workflow] no_commit_default` sets the default posture)
    #[arg(long = "no-commit", action = ArgAction::SetTrue, global = true)]
    no_commit: bool,

    /// Run supported commands in the background and return immediately with a job handle
    #[arg(long = "background", action = ArgAction::SetTrue, global = true)]
    background: bool,

    /// Internal hook for background child processes; do not set manually
    #[arg(long = "background-job-id", hide = true, global = true)]
    background_job_id: Option<String>,
}

impl Default for GlobalOpts {
    fn default() -> Self {
        Self {
            verbose: 0,
            quiet: false,
            debug: false,
            no_ansi: false,
            pager: false,
            no_pager: false,
            load_session: None,
            no_session: false,
            agent: None,
            backend: None,
            agent_label: None,
            agent_command: None,
            json: false,
            config_file: None,
            push: false,
            no_commit: false,
            background: false,
            background_job_id: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PagerMode {
    Auto,
    Always,
    Never,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum BackendArg {
    #[value(alias = "codex")]
    Agent,
    Gemini,
}

impl From<BackendArg> for config::BackendKind {
    fn from(value: BackendArg) -> Self {
        match value {
            BackendArg::Agent => config::BackendKind::Agent,
            BackendArg::Gemini => config::BackendKind::Gemini,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ScopeArg {
    Ask,
    Save,
    Draft,
    Approve,
    Review,
    Merge,
}

impl From<ScopeArg> for config::CommandScope {
    fn from(value: ScopeArg) -> Self {
        match value {
            ScopeArg::Ask => config::CommandScope::Ask,
            ScopeArg::Save => config::CommandScope::Save,
            ScopeArg::Draft => config::CommandScope::Draft,
            ScopeArg::Approve => config::CommandScope::Approve,
            ScopeArg::Review => config::CommandScope::Review,
            ScopeArg::Merge => config::CommandScope::Merge,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum JobLogStreamArg {
    Stdout,
    Stderr,
    Both,
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
enum Commands {
    /// One-shot interaction that applies the default-action posture (snapshot/narrative updates plus any backend edits) and exits
    Ask(AskCmd),

    /// Generate an implementation-plan draft branch from an operator spec in a disposable worktree
    Draft(DraftCmd),

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

    /// Bootstrap `.vizier/narrative/snapshot.md` and narrative docs from repo history
    #[command(name = "init-snapshot")]
    InitSnapshot(SnapshotInitCmd),

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
struct AskCmd {
    /// The user message to process in a single-shot run
    #[arg(value_name = "MESSAGE")]
    message: Option<String>,

    /// Read the user message from the specified file instead of an inline argument
    #[arg(short = 'f', long = "file", value_name = "PATH")]
    file: Option<PathBuf>,
}

#[derive(ClapArgs, Debug)]
struct DraftCmd {
    /// Operator spec used to seed the implementation plan
    #[arg(value_name = "SPEC")]
    spec: Option<String>,

    /// Read the operator spec from a file instead of inline text
    #[arg(short = 'f', long = "file", value_name = "PATH")]
    file: Option<PathBuf>,

    /// Override the derived plan/branch slug (letters, numbers, dashes only)
    #[arg(long = "name", value_name = "NAME")]
    name: Option<String>,
}

#[derive(ClapArgs, Debug)]
struct ListCmd {
    /// Target branch to compare against (defaults to detected primary)
    #[arg(long = "target", value_name = "BRANCH")]
    target: Option<String>,
}

#[derive(ClapArgs, Debug)]
struct CdCmd {
    /// Plan slug to open a workspace for (tab-completes from pending plans)
    #[arg(value_name = "PLAN", add = crate::completions::plan_slug_completer())]
    plan: Option<String>,

    /// Branch to use instead of draft/<plan>
    #[arg(long = "branch", value_name = "BRANCH")]
    branch: Option<String>,

    /// Print only the workspace path (no formatted outcome block)
    #[arg(long = "path-only", action = ArgAction::SetTrue)]
    path_only: bool,
}

#[derive(ClapArgs, Debug)]
struct CleanCmd {
    /// Plan slug to clean (omit to remove all Vizier-managed workspaces)
    #[arg(value_name = "PLAN", add = crate::completions::plan_slug_completer())]
    plan: Option<String>,

    /// Remove workspaces without prompting for confirmation
    #[arg(long = "yes", short = 'y')]
    assume_yes: bool,
}

#[derive(ClapArgs, Debug)]
struct PlanCmd {}

#[derive(ClapArgs, Debug)]
struct JobsCmd {
    #[command(subcommand)]
    action: JobsAction,
}

#[derive(Subcommand, Debug)]
enum JobsAction {
    /// List all tracked background jobs
    List,

    /// Show details for a background job id
    Show {
        #[arg(value_name = "JOB")]
        job: String,
    },

    /// Show a terse status line for a background job id
    Status {
        #[arg(value_name = "JOB")]
        job: String,
    },

    /// Tail logs for a background job (stdout/stderr)
    Tail {
        #[arg(value_name = "JOB")]
        job: String,

        /// Which log to display
        #[arg(long = "stream", value_enum, default_value_t = JobLogStreamArg::Both)]
        stream: JobLogStreamArg,

        /// Continue following the log until the job leaves Running/Pending
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
    },

    /// Garbage-collect completed jobs older than N days (default 7)
    Gc {
        #[arg(long = "days", value_name = "DAYS", default_value_t = 7)]
        days: u64,
    },
}

#[derive(ClapArgs, Debug)]
struct ApproveCmd {
    /// Plan slug to approve (tab-completes from pending plans)
    #[arg(value_name = "PLAN", add = crate::completions::plan_slug_completer())]
    plan: Option<String>,

    /// List pending plan branches instead of approving
    #[arg(
        long = "list",
        hide = true,
        help = "DEPRECATED: use `vizier list` instead."
    )]
    list: bool,

    /// Destination branch for preview/reference (defaults to detected primary)
    #[arg(long = "target", value_name = "BRANCH")]
    target: Option<String>,

    /// Draft branch name when it deviates from draft/<plan>
    #[arg(long = "branch", value_name = "BRANCH")]
    branch: Option<String>,

    /// Skip the confirmation prompt before applying the plan on the draft branch
    #[arg(long = "yes", short = 'y')]
    assume_yes: bool,

    /// Path to an approve stop-condition script (defaults to approve.stop_condition.script)
    #[arg(long = "stop-condition-script", value_name = "PATH")]
    stop_condition_script: Option<PathBuf>,

    /// Number of stop-condition retries before giving up (`approve.stop_condition.retries` by default)
    #[arg(long = "stop-condition-retries", value_name = "COUNT")]
    stop_condition_retries: Option<u32>,
}

#[derive(ClapArgs, Debug)]
struct ReviewCmd {
    /// Plan slug to review (tab-completes from pending plans)
    #[arg(value_name = "PLAN", add = crate::completions::plan_slug_completer())]
    plan: Option<String>,

    /// Destination branch for diff context (defaults to detected primary)
    #[arg(long = "target", value_name = "BRANCH")]
    target: Option<String>,

    /// Draft branch name when it deviates from draft/<plan>
    #[arg(long = "branch", value_name = "BRANCH")]
    branch: Option<String>,

    /// Skip the fix-up prompt and apply backend fixes automatically
    #[arg(long = "yes", short = 'y')]
    assume_yes: bool,

    /// Produce the critique without attempting fixes
    #[arg(long = "review-only")]
    review_only: bool,

    /// Skip running configured review checks (e.g., cargo test); merge CI/CD gate still runs once per review
    #[arg(long = "skip-checks")]
    skip_checks: bool,

    /// Path to a CI/CD gate script for this review (defaults to merge.cicd_gate.script)
    #[arg(long = "cicd-script", value_name = "PATH")]
    cicd_script: Option<PathBuf>,

    /// Force-enable backend remediation when the CI/CD script fails
    #[arg(long = "auto-cicd-fix", action = ArgAction::SetTrue, conflicts_with = "no_auto_cicd_fix")]
    auto_cicd_fix: bool,

    /// Disable backend remediation even if configured
    #[arg(long = "no-auto-cicd-fix", action = ArgAction::SetTrue, conflicts_with = "auto_cicd_fix")]
    no_auto_cicd_fix: bool,

    /// Number of remediation attempts before aborting (`merge.cicd_gate.retries` by default)
    #[arg(long = "cicd-retries", value_name = "COUNT")]
    cicd_retries: Option<u32>,
}

#[derive(ClapArgs, Debug)]
struct MergeCmd {
    /// Plan slug to merge (tab-completes from pending plans)
    #[arg(value_name = "PLAN", add = crate::completions::plan_slug_completer())]
    plan: Option<String>,

    /// Destination branch for merge (defaults to detected primary)
    #[arg(long = "target", value_name = "BRANCH")]
    target: Option<String>,

    /// Draft branch name when it deviates from draft/<plan>
    #[arg(long = "branch", value_name = "BRANCH")]
    branch: Option<String>,

    /// Skip the merge confirmation prompt
    #[arg(long = "yes", short = 'y')]
    assume_yes: bool,

    /// Keep the draft branch locally after merge (default is to delete)
    #[arg(long = "keep-branch", conflicts_with = "legacy_delete_branch")]
    keep_branch: bool,

    /// Deprecated alias for when deletion was opt-in; retained for compatibility
    #[arg(long = "delete-branch", hide = true)]
    legacy_delete_branch: bool,

    /// Optional note appended to the merge commit body
    #[arg(long = "note", value_name = "TEXT")]
    note: Option<String>,

    /// Attempt backend-backed auto-resolution when conflicts arise
    #[arg(long = "auto-resolve-conflicts")]
    auto_resolve_conflicts: bool,

    /// Skip backend conflict auto-resolution even when configured
    #[arg(
        long = "no-auto-resolve-conflicts",
        action = ArgAction::SetTrue,
        conflicts_with = "auto_resolve_conflicts"
    )]
    no_auto_resolve_conflicts: bool,

    /// Only finalize a previously conflicted merge; fail if no pending Vizier merge exists
    #[arg(long = "complete-conflict")]
    complete_conflict: bool,

    /// Path to a CI/CD gate script (defaults to merge.cicd_gate.script)
    #[arg(long = "cicd-script", value_name = "PATH")]
    cicd_script: Option<PathBuf>,

    /// Force-enable backend remediation when the CI/CD script fails
    #[arg(long = "auto-cicd-fix", action = ArgAction::SetTrue, conflicts_with = "no_auto_cicd_fix")]
    auto_cicd_fix: bool,

    /// Disable backend remediation even if configured
    #[arg(long = "no-auto-cicd-fix", action = ArgAction::SetTrue, conflicts_with = "auto_cicd_fix")]
    no_auto_cicd_fix: bool,

    /// Number of remediation attempts before aborting (`merge.cicd_gate.retries` by default)
    #[arg(long = "cicd-retries", value_name = "COUNT")]
    cicd_retries: Option<u32>,

    /// Squash implementation commits before creating the merge commit (default follows `[merge] squash`)
    #[arg(long = "squash", action = ArgAction::SetTrue, conflicts_with = "no_squash")]
    squash: bool,

    /// Preserve implementation commits (legacy behavior; overrides `[merge] squash = true`)
    #[arg(long = "no-squash", action = ArgAction::SetTrue, conflicts_with = "squash")]
    no_squash: bool,

    /// Parent index to use when cherry-picking merge commits in squash mode (1-based)
    #[arg(long = "squash-mainline", value_name = "PARENT_INDEX")]
    squash_mainline: Option<u32>,
}

#[derive(ClapArgs, Debug)]
struct CompletionsCmd {
    /// Shell to generate completion script for
    #[arg(value_enum)]
    shell: CompletionShell,
}

#[derive(ClapArgs, Debug)]
struct HiddenCompleteCmd {}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum CompletionShell {
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

#[derive(ClapArgs, Debug, Clone)]
struct SnapshotInitCmd {
    /// Overwrite existing snapshot/narrative docs without confirmation
    #[arg(long)]
    force: bool,

    /// Limit Git history scan depth
    #[arg(long, value_name = "N")]
    depth: Option<usize>,

    /// Restrict analysis to matching paths (comma-separated or repeated)
    #[arg(long, value_name = "GLOB", value_delimiter = ',')]
    paths: Vec<String>,

    /// Exclude matching paths (comma-separated or repeated)
    #[arg(long, value_name = "GLOB", value_delimiter = ',')]
    exclude: Vec<String>,

    /// Enrich snapshot with external issues (e.g., github)
    #[arg(long, value_name = "PROVIDER")]
    issues: Option<String>,
}

impl From<SnapshotInitCmd> for crate::actions::SnapshotInitOptions {
    fn from(cmd: SnapshotInitCmd) -> Self {
        crate::actions::SnapshotInitOptions {
            force: cmd.force,
            depth: cmd.depth,
            paths: cmd.paths,
            exclude: cmd.exclude,
            issues: cmd.issues,
        }
    }
}

#[derive(ClapArgs, Debug)]
#[command(
    group = ArgGroup::new("commit_msg_src")
        .args(&["commit_message", "commit_message_editor"])
        .multiple(false)
)]
struct SaveCmd {
    /// Commit reference or range; defaults to HEAD if omitted.
    ///
    /// Examples: `HEAD`, `HEAD~3..HEAD`, `feature-branch`
    #[arg(value_name = "REV_OR_RANGE", default_value = "HEAD")]
    rev_or_range: String,

    /// Developer note to append to the *code* commit message
    #[arg(short = 'm', long = "message")]
    commit_message: Option<String>,

    /// Open $EDITOR to compose the commit message
    #[arg(short = 'M', long = "edit-message")]
    commit_message_editor: bool,
}

#[derive(Debug, Clone)]
struct ResolvedInput {
    text: String,
    origin: InputOrigin,
}

#[derive(Debug, Clone)]
enum InputOrigin {
    Inline,
    File(PathBuf),
    Stdin,
}

#[derive(ClapArgs, Debug)]
struct TestDisplayCmd {
    /// Command scope to resolve agent settings from
    #[arg(long = "scope", value_enum, default_value_t = ScopeArg::Ask)]
    scope: ScopeArg,

    /// Override the default smoke-test prompt
    #[arg(long = "prompt", value_name = "TEXT")]
    prompt: Option<String>,

    /// Dump captured stdout/stderr verbatim instead of a summarized snippet
    #[arg(long = "raw", action = ArgAction::SetTrue)]
    raw: bool,

    /// Timeout in seconds before aborting the agent run
    #[arg(long = "timeout", value_name = "SECONDS")]
    timeout_secs: Option<u64>,

    /// Disable stdbuf/unbuffer/script wrapping for debugging agent output
    #[arg(long = "no-wrapper", action = ArgAction::SetTrue)]
    no_wrapper: bool,

    /// Write a session log for this smoke test (defaults to off)
    #[arg(long = "session", action = ArgAction::SetTrue, conflicts_with = "no_session")]
    session: bool,

    /// Explicitly disable session logging (default)
    #[arg(long = "no-session", action = ArgAction::SetTrue, conflicts_with = "session")]
    no_session: bool,
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

fn read_all_stdin() -> Result<String, std::io::Error> {
    use std::io::{self, Read};
    let mut buf = String::new();
    io::stdin().read_to_string(&mut buf)?;
    Ok(buf)
}

fn resolve_ask_message(cmd: &AskCmd) -> Result<String, Box<dyn std::error::Error>> {
    Ok(resolve_prompt_input(cmd.message.as_deref(), cmd.file.as_deref())?.text)
}

fn resolve_draft_spec(cmd: &DraftCmd) -> Result<ResolvedInput, Box<dyn std::error::Error>> {
    resolve_prompt_input(cmd.spec.as_deref(), cmd.file.as_deref())
}

fn resolve_list_options(cmd: &ListCmd) -> ListOptions {
    ListOptions {
        target: cmd.target.clone(),
    }
}

fn resolve_cd_options(cmd: &CdCmd) -> Result<CdOptions, Box<dyn std::error::Error>> {
    let plan = cmd
        .plan
        .as_deref()
        .ok_or("plan argument is required for vizier cd")?;
    let slug = crate::plan::sanitize_name_override(plan).map_err(|err| {
        Box::<dyn std::error::Error>::from(io::Error::new(io::ErrorKind::InvalidInput, err))
    })?;
    let branch = cmd
        .branch
        .clone()
        .unwrap_or_else(|| crate::plan::default_branch_for_slug(&slug));

    Ok(CdOptions {
        slug,
        branch,
        path_only: cmd.path_only,
    })
}

fn resolve_clean_options(cmd: &CleanCmd) -> Result<CleanOptions, Box<dyn std::error::Error>> {
    let slug = if let Some(plan) = cmd.plan.as_deref() {
        Some(crate::plan::sanitize_name_override(plan).map_err(|err| {
            Box::<dyn std::error::Error>::from(io::Error::new(io::ErrorKind::InvalidInput, err))
        })?)
    } else {
        None
    };

    Ok(CleanOptions {
        slug,
        assume_yes: cmd.assume_yes,
    })
}

fn resolve_approve_options(
    cmd: &ApproveCmd,
    push_after: bool,
) -> Result<ApproveOptions, Box<dyn std::error::Error>> {
    if !cmd.list && cmd.plan.is_none() {
        return Err(
            "plan argument is required (use `vizier list` to inspect pending drafts)".into(),
        );
    }

    let config = config::get_config();
    let repo_root = vcs::repo_root().ok();

    let mut stop_script = config
        .approve
        .stop_condition
        .script
        .clone()
        .map(|path| resolve_cicd_script_path(&path, repo_root.as_deref()));

    if let Some(script) = cmd.stop_condition_script.as_ref() {
        stop_script = Some(resolve_cicd_script_path(script, repo_root.as_deref()));
    }

    if let Some(script) = stop_script.as_ref() {
        let metadata = std::fs::metadata(script).map_err(|err| {
            format!(
                "unable to read approve stop-condition script {}: {err}",
                script.display()
            )
        })?;
        if !metadata.is_file() {
            return Err(format!(
                "approve stop-condition script {} must be a file",
                script.display()
            )
            .into());
        }
    }

    let mut stop_retries = config.approve.stop_condition.retries;
    if let Some(retries) = cmd.stop_condition_retries {
        stop_retries = retries;
    }

    Ok(ApproveOptions {
        plan: cmd.plan.clone(),
        list_only: cmd.list,
        target: cmd.target.clone(),
        branch_override: cmd.branch.clone(),
        assume_yes: cmd.assume_yes,
        stop_condition: ApproveStopCondition {
            script: stop_script,
            retries: stop_retries,
        },
        push_after,
    })
}

fn resolve_review_options(
    cmd: &ReviewCmd,
    push_after: bool,
) -> Result<ReviewOptions, Box<dyn std::error::Error>> {
    let plan = cmd
        .plan
        .clone()
        .ok_or("plan argument is required for vizier review")?;
    let config = config::get_config();
    let repo_root = vcs::repo_root().ok();
    let mut cicd_gate = CicdGateOptions::from_config(&config.merge.cicd_gate);
    let mut auto_resolve_requested = cicd_gate.auto_resolve;
    if let Some(script) = cicd_gate.script.clone() {
        cicd_gate.script = Some(resolve_cicd_script_path(&script, repo_root.as_deref()));
    }
    if let Some(script) = cmd.cicd_script.as_ref() {
        cicd_gate.script = Some(resolve_cicd_script_path(script, repo_root.as_deref()));
    }
    if cmd.auto_cicd_fix {
        auto_resolve_requested = true;
    }
    if cmd.no_auto_cicd_fix {
        auto_resolve_requested = false;
    }
    if let Some(retries) = cmd.cicd_retries {
        cicd_gate.retries = retries;
    }
    // Review runs the gate without auto-remediation unless explicitly allowed in the future.
    cicd_gate.auto_resolve = false;
    if let Some(script) = cicd_gate.script.as_ref() {
        let metadata = std::fs::metadata(script)
            .map_err(|err| format!("unable to read CI/CD script {}: {err}", script.display()))?;
        if !metadata.is_file() {
            return Err(format!("CI/CD script {} must be a file", script.display()).into());
        }
    }

    Ok(ReviewOptions {
        plan,
        target: cmd.target.clone(),
        branch_override: cmd.branch.clone(),
        assume_yes: cmd.assume_yes,
        review_only: cmd.review_only,
        skip_checks: cmd.skip_checks,
        cicd_gate,
        auto_resolve_requested,
        push_after,
    })
}

fn resolve_merge_options(
    cmd: &MergeCmd,
    push_after: bool,
) -> Result<MergeOptions, Box<dyn std::error::Error>> {
    let plan = cmd
        .plan
        .clone()
        .ok_or("plan argument is required for vizier merge")?;
    let config = config::get_config();
    let default_conflict_auto_resolve = config::MergeConflictsConfig::default().auto_resolve;
    let conflict_source = if config.merge.conflicts.auto_resolve == default_conflict_auto_resolve {
        ConflictAutoResolveSource::Default
    } else {
        ConflictAutoResolveSource::Config
    };
    let mut conflict_auto_resolve =
        ConflictAutoResolveSetting::new(config.merge.conflicts.auto_resolve, conflict_source);
    if cmd.auto_resolve_conflicts {
        conflict_auto_resolve =
            ConflictAutoResolveSetting::new(true, ConflictAutoResolveSource::FlagEnable);
    }
    if cmd.no_auto_resolve_conflicts {
        conflict_auto_resolve =
            ConflictAutoResolveSetting::new(false, ConflictAutoResolveSource::FlagDisable);
    }
    let conflict_strategy = if conflict_auto_resolve.enabled() {
        MergeConflictStrategy::Agent
    } else {
        MergeConflictStrategy::Manual
    };

    if cmd.legacy_delete_branch {
        display::warn(
            "--delete-branch is deprecated; vizier merge now deletes draft branches by default. \
             Pass --keep-branch to retain the branch after merging.",
        );
    }

    let repo_root = vcs::repo_root().ok();
    let mut cicd_gate = CicdGateOptions::from_config(&config.merge.cicd_gate);
    if let Some(script) = cicd_gate.script.clone() {
        cicd_gate.script = Some(resolve_cicd_script_path(&script, repo_root.as_deref()));
    }
    if let Some(script) = cmd.cicd_script.as_ref() {
        cicd_gate.script = Some(resolve_cicd_script_path(script, repo_root.as_deref()));
    }
    if cmd.auto_cicd_fix {
        cicd_gate.auto_resolve = true;
    }
    if cmd.no_auto_cicd_fix {
        cicd_gate.auto_resolve = false;
    }
    if let Some(retries) = cmd.cicd_retries {
        cicd_gate.retries = retries;
    }
    if let Some(script) = cicd_gate.script.as_ref() {
        let metadata = std::fs::metadata(script)
            .map_err(|err| format!("unable to read CI/CD script {}: {err}", script.display()))?;
        if !metadata.is_file() {
            return Err(format!("CI/CD script {} must be a file", script.display()).into());
        }
    }

    let mut squash = config.merge.squash_default;
    if cmd.squash {
        squash = true;
    }
    if cmd.no_squash {
        squash = false;
    }
    let mut squash_mainline = config.merge.squash_mainline;
    if let Some(mainline) = cmd.squash_mainline {
        squash_mainline = Some(mainline);
    }
    if let Some(mainline) = squash_mainline
        && mainline == 0 {
            return Err("squash mainline parent index must be at least 1".into());
        }

    Ok(MergeOptions {
        plan,
        target: cmd.target.clone(),
        branch_override: cmd.branch.clone(),
        assume_yes: cmd.assume_yes,
        delete_branch: !cmd.keep_branch,
        note: cmd.note.clone(),
        push_after,
        conflict_auto_resolve,
        conflict_strategy,
        complete_conflict: cmd.complete_conflict,
        cicd_gate,
        squash,
        squash_mainline,
    })
}

fn resolve_test_display_options(
    cmd: &TestDisplayCmd,
) -> Result<TestDisplayOptions, Box<dyn std::error::Error>> {
    if let Some(prompt) = cmd
        .prompt
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| value.is_empty())
    {
        return Err(format!("prompt override cannot be empty (got {prompt:?})").into());
    }

    Ok(TestDisplayOptions {
        scope: cmd.scope.into(),
        prompt_override: cmd.prompt.as_ref().map(|value| value.trim().to_string()),
        raw_output: cmd.raw,
        timeout: cmd.timeout_secs.map(std::time::Duration::from_secs),
        disable_wrapper: cmd.no_wrapper,
        record_session: cmd.session && !cmd.no_session,
    })
}

fn run_jobs_command(
    project_root: &Path,
    jobs_root: &Path,
    cmd: JobsCmd,
) -> Result<(), Box<dyn std::error::Error>> {
    match cmd.action {
        JobsAction::List => {
            let records = jobs::list_records(jobs_root)?;
            if records.is_empty() {
                println!("Outcome: No background jobs found");
                return Ok(());
            }

            println!("Background jobs ({})", records.len());
            for record in records {
                let status = jobs::status_label(record.status);
                let command = if record.command.is_empty() {
                    "<command unavailable>".to_string()
                } else {
                    record.command.join(" ")
                };
                println!("- {} [{}] {}", record.id, status, command);
            }
            Ok(())
        }
        JobsAction::Show { job } => {
            let record = jobs::read_record(jobs_root, &job)?;
            println!("Job {}", record.id);
            println!("Status: {}", jobs::status_label(record.status));
            if let Some(pid) = record.pid {
                println!("PID: {pid}");
            }
            if let Some(started) = record.started_at {
                println!("Started: {}", started.to_rfc3339());
            }
            if let Some(finished) = record.finished_at {
                println!("Finished: {}", finished.to_rfc3339());
            }
            if let Some(code) = record.exit_code {
                println!("Exit code: {code}");
            }
            println!("Stdout: {}", record.stdout_path);
            println!("Stderr: {}", record.stderr_path);
            if let Some(session) = record.session_path {
                println!("Session: {}", session);
            }
            if let Some(outcome) = record.outcome_path {
                println!("Outcome: {}", outcome);
            }
            if let Some(metadata) = record.metadata.as_ref() {
                if let Some(scope) = metadata.scope.as_ref() {
                    println!("Scope: {scope}");
                }
                if let Some(plan) = metadata.plan.as_ref() {
                    println!("Plan: {plan}");
                }
                if let Some(target) = metadata.target.as_ref() {
                    println!("Target: {target}");
                }
                if let Some(branch) = metadata.branch.as_ref() {
                    println!("Branch: {branch}");
                }
                if let Some(revision) = metadata.revision.as_ref() {
                    println!("Revision: {revision}");
                }
                if let Some(agent_backend) = metadata.agent_backend.as_ref() {
                    println!("Agent backend: {agent_backend}");
                }
                if let Some(label) = metadata.agent_label.as_ref() {
                    println!("Agent label: {label}");
                }
                if let Some(command) = metadata.agent_command.as_ref() {
                    println!("Agent command: {}", command.join(" "));
                }
                if let Some(exit) = metadata.agent_exit_code {
                    println!("Agent exit: {exit}");
                }
            }
            if let Some(config) = record.config_snapshot.as_ref() {
                println!("Config snapshot: {}", config);
            }
            if !record.command.is_empty() {
                println!("Command: {}", record.command.join(" "));
            }
            Ok(())
        }
        JobsAction::Status { job } => {
            let record = jobs::read_record(jobs_root, &job)?;
            let exit = record
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "-".to_string());
            println!(
                "{} [{}] exit={} stdout={} stderr={}",
                record.id,
                jobs::status_label(record.status),
                exit,
                record.stdout_path,
                record.stderr_path
            );
            Ok(())
        }
        JobsAction::Tail {
            job,
            stream,
            follow,
        } => jobs::tail_job_logs(jobs_root, &job, stream.into(), follow),
        JobsAction::Attach { job } => {
            jobs::tail_job_logs(jobs_root, &job, jobs::LogStream::Both, true)
        }
        JobsAction::Cancel { job } => {
            let record = jobs::cancel_job(project_root, jobs_root, &job)?;
            println!(
                "Job {} marked cancelled (stdout: {}, stderr: {})",
                record.id, record.stdout_path, record.stderr_path
            );
            Ok(())
        }
        JobsAction::Gc { days } => {
            let removed =
                jobs::gc_jobs(project_root, jobs_root, chrono::Duration::days(days as i64))?;
            println!("Outcome: removed {} job(s)", removed);
            Ok(())
        }
    }
}

fn resolve_cicd_script_path(script: &Path, repo_root: Option<&Path>) -> PathBuf {
    if script.is_absolute() {
        return script.to_path_buf();
    }
    if let Some(root) = repo_root {
        return root.join(script);
    }
    script.to_path_buf()
}

fn build_cli_agent_overrides(
    opts: &GlobalOpts,
) -> Result<Option<config::AgentOverrides>, Box<dyn std::error::Error>> {
    let mut overrides = config::AgentOverrides::default();

    if let Some(agent) = opts.agent.as_ref() {
        if !agent.trim().is_empty() {
            overrides.selector = Some(agent.trim().to_ascii_lowercase());
        }
    }

    if let Some(backend) = opts.backend {
        display::warn("`--backend` is deprecated; use `--agent <selector>` instead.");
        let selector = match backend {
            BackendArg::Agent => "codex",
            BackendArg::Gemini => "gemini",
        };
        overrides.selector = Some(selector.to_string());
    }

    if let Some(label) = opts
        .agent_label
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        overrides
            .agent_runtime
            .get_or_insert_with(Default::default)
            .label = Some(label.to_ascii_lowercase());
    }

    if let Some(command) = opts
        .agent_command
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        overrides
            .agent_runtime
            .get_or_insert_with(Default::default)
            .command = Some(vec![command.to_string()]);
    }

    if overrides.is_empty() {
        Ok(None)
    } else {
        Ok(Some(overrides))
    }
}

fn flag_present(args: &[String], short: Option<char>, long: &str) -> bool {
    let short_flag = short.map(|value| format!("-{value}"));
    args.iter().any(|arg| {
        if arg == long || (long.starts_with("--") && arg.starts_with(&format!("{long}="))) {
            return true;
        }
        if let Some(short_flag) = short_flag.as_ref() {
            if arg == short_flag {
                return true;
            }
            if arg.starts_with('-')
                && !arg.starts_with("--")
                && arg.contains(short_flag.trim_start_matches('-'))
            {
                return true;
            }
        }
        false
    })
}

fn pager_mode_from_args(args: &[String]) -> PagerMode {
    if flag_present(args, None, "--no-pager") {
        PagerMode::Never
    } else if flag_present(args, None, "--pager") {
        PagerMode::Always
    } else {
        PagerMode::Auto
    }
}

fn command_scope_for(command: &Commands) -> Option<config::CommandScope> {
    match command {
        Commands::Ask(_) => Some(config::CommandScope::Ask),
        Commands::Draft(_) => Some(config::CommandScope::Draft),
        Commands::Approve(_) => Some(config::CommandScope::Approve),
        Commands::Review(_) => Some(config::CommandScope::Review),
        Commands::Merge(_) => Some(config::CommandScope::Merge),
        Commands::Save(_) => Some(config::CommandScope::Save),
        _ => None,
    }
}

fn background_config_snapshot(cfg: &config::Config) -> serde_json::Value {
    json!({
        "backend": cfg.backend.to_string(),
        "agent_selector": cfg.agent_selector,
        "agent": {
            "label": cfg.agent_runtime.label,
            "command": cfg.agent_runtime.command,
        },
        "workflow": {
            "no_commit_default": cfg.workflow.no_commit_default,
            "background": {
                "enabled": cfg.workflow.background.enabled,
                "quiet": cfg.workflow.background.quiet,
            },
        },
    })
}

fn build_job_metadata(
    command: &Commands,
    cfg: &config::Config,
    cli_agent_override: Option<&config::AgentOverrides>,
) -> jobs::JobMetadata {
    let mut metadata = jobs::JobMetadata::default();
    metadata.background_quiet = Some(cfg.workflow.background.quiet);
    metadata.config_backend = Some(cfg.backend.to_string());
    metadata.config_agent_selector = Some(cfg.agent_selector.clone());
    metadata.config_agent_label = cfg.agent_runtime.label.clone();
    if !cfg.agent_runtime.command.is_empty() {
        metadata.config_agent_command = Some(cfg.agent_runtime.command.clone());
    }

    if let Some(scope) = command_scope_for(command) {
        metadata.scope = Some(scope.as_str().to_string());
        if let Ok(agent) = cfg.resolve_agent_settings(scope, cli_agent_override) {
            metadata.agent_selector = Some(agent.selector.clone());
            metadata.agent_backend = Some(agent.backend.to_string());
            metadata.agent_label = Some(agent.agent_runtime.label.clone());
            if !agent.agent_runtime.command.is_empty() {
                metadata.agent_command = Some(agent.agent_runtime.command.clone());
            }
        }
    }

    match command {
        Commands::Approve(cmd) => {
            metadata.plan = cmd.plan.clone();
            metadata.target = cmd.target.clone();
            metadata.branch = cmd.branch.clone();
        }
        Commands::Review(cmd) => {
            metadata.plan = cmd.plan.clone();
            metadata.target = cmd.target.clone();
            metadata.branch = cmd.branch.clone();
        }
        Commands::Merge(cmd) => {
            metadata.plan = cmd.plan.clone();
            metadata.target = cmd.target.clone();
            metadata.branch = cmd.branch.clone();
        }
        Commands::Save(cmd) => {
            metadata.revision = Some(cmd.rev_or_range.clone());
        }
        _ => {}
    }

    metadata
}

fn runtime_job_metadata() -> Option<jobs::JobMetadata> {
    let mut metadata = jobs::JobMetadata::default();
    if let Some(context) = auditor::Auditor::latest_agent_context() {
        metadata.agent_selector = Some(context.selector);
        metadata.agent_backend = Some(context.backend.to_string());
        metadata.agent_label = Some(context.backend_label);
    }

    if let Some(run) = auditor::Auditor::latest_agent_run() {
        metadata.agent_exit_code = Some(run.exit_code);
        if !run.command.is_empty() {
            metadata.agent_command = Some(run.command.clone());
        }
    }

    if metadata.agent_selector.is_none()
        && metadata.agent_backend.is_none()
        && metadata.agent_label.is_none()
        && metadata.agent_exit_code.is_none()
        && metadata
            .agent_command
            .as_ref()
            .map(|value| value.is_empty())
            .unwrap_or(true)
    {
        None
    } else {
        Some(metadata)
    }
}

fn background_supported(command: &Commands) -> bool {
    matches!(
        command,
        Commands::Ask(_)
            | Commands::Draft(_)
            | Commands::Approve(_)
            | Commands::Review(_)
            | Commands::Merge(_)
            | Commands::Save(_)
    )
}

fn ensure_background_safe(command: &Commands) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Commands::Approve(cmd) if !cmd.assume_yes => {
            Err("--background for vizier approve requires --yes to skip interactive prompts".into())
        }
        Commands::Merge(cmd) if !cmd.assume_yes => {
            Err("--background for vizier merge requires --yes to skip interactive prompts".into())
        }
        Commands::Review(cmd) if !cmd.assume_yes && !cmd.review_only => Err(
            "--background for vizier review requires --yes or --review-only to avoid prompts"
                .into(),
        ),
        _ => Ok(()),
    }
}

fn strip_background_flags(raw_args: &[String]) -> Vec<String> {
    let mut args = Vec::new();
    let mut skip_next = false;
    for arg in raw_args.iter().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }

        if arg == "--background" || arg.starts_with("--background=") {
            continue;
        }

        if arg == "--background-job-id" {
            skip_next = true;
            continue;
        }

        args.push(arg.clone());
    }

    args
}

fn user_friendly_args(raw_args: &[String]) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(binary) = raw_args.first() {
        args.push(binary.clone());
    }
    args.extend(strip_background_flags(raw_args));
    args
}

fn build_background_child_args(
    raw_args: &[String],
    job_id: &str,
    cfg: &config::BackgroundConfig,
) -> Vec<String> {
    let mut args = strip_background_flags(raw_args);
    args.push("--background-job-id".to_string());
    args.push(job_id.to_string());

    if !flag_present(&args, None, "--no-ansi") {
        args.push("--no-ansi".to_string());
    }

    let quiet_flagged =
        flag_present(&args, Some('q'), "--quiet") || flag_present(&args, Some('v'), "--verbose");
    if cfg.quiet && !quiet_flagged && !flag_present(&args, Some('d'), "--debug") {
        args.push("--quiet".to_string());
    }

    if !flag_present(&args, None, "--no-pager") && !flag_present(&args, None, "--pager") {
        args.push("--no-pager".to_string());
    }

    args
}

fn render_help_with_pager(
    err: clap::Error,
    pager_mode: PagerMode,
    stdout_is_tty: bool,
    suppress_pager: bool,
    ansi_enabled: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let rendered = err.render().to_string();
    let help_text = if ansi_enabled {
        rendered
    } else {
        strip_ansi_codes(&rendered)
    };

    if suppress_pager || !stdout_is_tty || matches!(pager_mode, PagerMode::Never) {
        print!("{help_text}");
        return Ok(());
    }

    if let Some(pager) = std::env::var("VIZIER_PAGER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        && try_page_output(&pager, &help_text).is_ok() {
            return Ok(());
        }

    if matches!(pager_mode, PagerMode::Always | PagerMode::Auto)
        && try_page_output("less -FRSX", &help_text).is_ok()
    {
        return Ok(());
    }

    print!("{help_text}");
    Ok(())
}

fn try_page_output(command: &str, contents: &str) -> std::io::Result<()> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .spawn()?;

    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(contents.as_bytes())?;
    }

    let _ = child.wait()?;
    Ok(())
}

fn strip_ansi_codes(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b'
            && chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
                continue;
            }
        output.push(ch);
    }
    output
}

fn log_agent_runtime_resolution(agent: &config::AgentSettings) {
    if !agent.backend.requires_agent_runner() {
        return;
    }

    match &agent.agent_runtime.resolution {
        config::AgentRuntimeResolution::BundledShim { label, path } => {
            display::info(format!(
                "Using bundled `{label}` agent shim at {} (command: {})",
                path.display(),
                agent.agent_runtime.command.join(" ")
            ));
        }
        config::AgentRuntimeResolution::ProvidedCommand => {
            display::debug(format!(
                "Using configured agent command `{}` (label `{}`)",
                agent.agent_runtime.command.join(" "),
                agent.agent_runtime.label
            ));
        }
    }
}

fn resolve_prompt_input(
    positional: Option<&str>,
    file: Option<&Path>,
) -> Result<ResolvedInput, Box<dyn std::error::Error>> {
    use std::io::{Error, ErrorKind};

    if positional.is_some() && file.is_some() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "cannot provide both MESSAGE and --file; choose one input source",
        )
        .into());
    }

    if let Some(path) = file {
        let msg = std::fs::read_to_string(path).map_err(|err| {
            Error::new(
                err.kind(),
                format!("failed to read {}: {err}", path.display()),
            )
        })?;

        if msg.trim().is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "file {} is empty; provide non-empty content",
                    path.display()
                ),
            )
            .into());
        }

        return Ok(ResolvedInput {
            text: msg,
            origin: InputOrigin::File(path.to_path_buf()),
        });
    }

    match positional {
        Some("-") => {
            // Explicit “read stdin”
            let msg = read_all_stdin()?;
            if msg.trim().is_empty() {
                return Err("stdin is empty; provide MESSAGE or pipe content".into());
            }
            Ok(ResolvedInput {
                text: msg,
                origin: InputOrigin::Stdin,
            })
        }
        Some(positional) => Ok(ResolvedInput {
            text: positional.to_owned(),
            origin: InputOrigin::Inline,
        }),
        None => {
            // No positional; try stdin if it’s not a TTY (i.e., piped or redirected)
            if !std::io::stdin().is_terminal() {
                let msg = read_all_stdin()?;
                if msg.trim().is_empty() {
                    return Err("stdin is empty; provide MESSAGE or pipe content".into());
                }
                Ok(ResolvedInput {
                    text: msg,
                    origin: InputOrigin::Stdin,
                })
            } else {
                Err("no MESSAGE provided; pass a message, use '-', or pipe stdin".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn resolve_ask_message_reads_file_contents() -> Result<(), Box<dyn std::error::Error>> {
        let mut tmp = tempfile::NamedTempFile::new()?;
        write!(tmp, "File-backed prompt")?;

        let cmd = AskCmd {
            message: None,
            file: Some(tmp.path().to_path_buf()),
        };

        let resolved = resolve_ask_message(&cmd)?;
        assert_eq!(resolved, "File-backed prompt");
        Ok(())
    }

    #[test]
    fn resolve_ask_message_rejects_both_sources() {
        let cmd = AskCmd {
            message: Some("inline".to_string()),
            file: Some(PathBuf::from("ignored")),
        };

        let err = resolve_ask_message(&cmd).unwrap_err();
        assert!(
            err.to_string()
                .contains("cannot provide both MESSAGE and --file"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn resolve_ask_message_rejects_empty_file() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::NamedTempFile::new()?;

        let cmd = AskCmd {
            message: None,
            file: Some(tmp.path().to_path_buf()),
        };

        let err = resolve_ask_message(&cmd)
            .expect_err("empty file should produce an error for ask input");
        assert!(err.to_string().contains("empty"), "unexpected error: {err}");
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if crate::completions::try_handle_completion(Cli::command)
        .map_err(Box::<dyn std::error::Error>::from)?
    {
        return Ok(());
    }

    let stdout_is_tty = std::io::stdout().is_terminal();
    let stderr_is_tty = std::io::stderr().is_terminal();
    let raw_args: Vec<String> = std::env::args().collect();
    let quiet_requested = flag_present(&raw_args, Some('q'), "--quiet");
    let json_requested = flag_present(&raw_args, Some('j'), "--json");
    let no_ansi_requested = flag_present(&raw_args, None, "--no-ansi");
    let pager_mode = pager_mode_from_args(&raw_args);

    let color_choice = if !no_ansi_requested && stdout_is_tty && stderr_is_tty {
        ColorChoice::Auto
    } else {
        ColorChoice::Never
    };

    let command = Cli::command().color(color_choice);
    let matches = match command.try_get_matches_from(&raw_args) {
        Ok(matches) => matches,
        Err(err) => match err.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                render_help_with_pager(
                    err,
                    pager_mode,
                    stdout_is_tty,
                    quiet_requested || json_requested,
                    color_choice != ColorChoice::Never,
                )?;
                return Ok(());
            }
            ErrorKind::DisplayVersion => {
                let rendered = err.render().to_string();
                println!("{rendered}");
                return Ok(());
            }
            _ => err.exit(),
        },
    };

    let cli = Cli::from_arg_matches(&matches)?;

    let mut verbosity = if cli.global.quiet {
        display::Verbosity::Quiet
    } else {
        match cli.global.verbose {
            0 => display::Verbosity::Normal,
            1 => display::Verbosity::Info,
            _ => display::Verbosity::Debug,
        }
    };

    if !cli.global.quiet && cli.global.debug {
        verbosity = display::Verbosity::Debug;
    }

    display::set_display_config(display::DisplayConfig {
        verbosity,
        stdout_is_tty,
        stderr_is_tty,
    });

    let project_root = match auditor::find_project_root() {
        Ok(Some(root)) => root,
        Ok(None) => {
            display::emit(
                LogLevel::Error,
                "vizier cannot be used outside a git repository",
            );
            return Err("not a git repository".into());
        }
        Err(e) => {
            display::emit(LogLevel::Error, format!("Error finding project root: {e}"));
            return Err(Box::<dyn std::error::Error>::from(e));
        }
    };

    if let Err(e) = std::fs::create_dir_all(project_root.join(".vizier")) {
        display::emit(
            LogLevel::Error,
            format!("Error creating .vizier directory: {e}"),
        );
        return Err(Box::<dyn std::error::Error>::from(e));
    }

    if let Err(e) = std::fs::create_dir_all(project_root.join(".vizier").join("sessions")) {
        display::emit(
            LogLevel::Error,
            format!("Error creating .vizier/sessions directory: {e}"),
        );
        return Err(Box::<dyn std::error::Error>::from(e));
    }

    let jobs_root = match jobs::ensure_jobs_root(&project_root) {
        Ok(path) => path,
        Err(e) => {
            display::emit(
                LogLevel::Error,
                format!("Error creating .vizier/jobs directory: {e}"),
            );
            return Err(Box::<dyn std::error::Error>::from(e));
        }
    };

    let mut auditor_cleanup = auditor::AuditorCleanup {
        debug: cli.global.debug,
        print_json: cli.global.json,
        persisted: false,
    };

    if let Err(e) = std::fs::create_dir_all(tools::get_vizier_dir()) {
        display::emit(
            LogLevel::Error,
            format!(
                "Error creating .vizier directory {:?}: {e}",
                tools::get_vizier_dir()
            ),
        );

        return Err(Box::<dyn std::error::Error>::from(e));
    }

    if let Err(e) = std::fs::create_dir_all(tools::get_narrative_dir()) {
        display::emit(
            LogLevel::Error,
            format!(
                "Error creating .vizier/narrative directory {:?}: {e}",
                tools::get_narrative_dir()
            ),
        );

        return Err(Box::<dyn std::error::Error>::from(e));
    }

    let mut cfg = if let Some(ref config_file) = cli.global.config_file {
        config::Config::from_path(std::path::PathBuf::from(config_file))?
    } else {
        let mut layers = Vec::new();

        if let Some(path) = config::global_config_path().filter(|path| path.exists()) {
            display::emit(
                LogLevel::Info,
                format!("Loading global config from {}", path.display()),
            );
            layers.push(config::ConfigLayer::from_path(path)?);
        }

        if let Some(path) = config::project_config_path(&project_root) {
            display::emit(
                LogLevel::Info,
                format!("Loading repo config from {}", path.display()),
            );
            layers.push(config::ConfigLayer::from_path(path)?);
        }

        if !layers.is_empty() {
            config::Config::from_layers(&layers)
        } else if let Some(path) = config::env_config_path().filter(|path| path.exists()) {
            display::emit(
                LogLevel::Info,
                format!("Loading env config from {}", path.display()),
            );
            config::Config::from_path(path)?
        } else {
            config::get_config()
        }
    };

    if let Some(session_id) = &cli.global.load_session {
        let repo_session = project_root
            .join(".vizier")
            .join("sessions")
            .join(session_id)
            .join("session.json");

        let messages = if repo_session.exists() {
            auditor::Auditor::load_session_messages_from_path(&repo_session)?
        } else if let Some(config_dir) = config::base_config_dir() {
            let legacy = config_dir
                .join("vizier")
                .join(format!("{}.json", session_id));
            if legacy.exists() {
                auditor::Auditor::load_session_messages_from_path(&legacy)?
            } else {
                return Err("could not find session file".into());
            }
        } else {
            return Err("could not find session file".into());
        };

        auditor::Auditor::replace_messages(&messages);
    }

    cfg.no_session = cli.global.no_session;

    if let Some(selector) = cli
        .global
        .agent
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        cfg.agent_selector = selector.to_ascii_lowercase();
        cfg.backend = config::backend_kind_for_selector(&cfg.agent_selector);
    } else if let Some(backend_arg) = cli.global.backend {
        display::warn("`--backend` is deprecated; use `--agent` instead.");
        cfg.agent_selector = match backend_arg {
            BackendArg::Agent => "codex".to_string(),
            BackendArg::Gemini => "gemini".to_string(),
        };
        cfg.backend = config::backend_kind_for_selector(&cfg.agent_selector);
    }

    if let Some(label) = cli
        .global
        .agent_label
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        cfg.agent_runtime.label = Some(label.to_ascii_lowercase());
    }

    if let Some(command) = cli
        .global
        .agent_command
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        cfg.agent_runtime.command = vec![command.to_string()];
    }

    let workflow_defaults = cfg.workflow.clone();

    config::set_config(cfg);

    let cli_agent_override = build_cli_agent_overrides(&cli.global)?;

    let push_after = cli.global.push;
    let commit_mode = if cli.global.no_commit {
        CommitMode::HoldForReview
    } else if workflow_defaults.no_commit_default {
        CommitMode::HoldForReview
    } else {
        CommitMode::AutoCommit
    };

    if cli.global.background && cli.global.background_job_id.is_none() {
        display::emit(
            LogLevel::Error,
            "--background is experimental and currently disabled; run commands in the foreground for this release",
        );
        return Err("background execution disabled for this release".into());
    }

    let result = match cli.command {
        Commands::Completions(cmd) => {
            crate::completions::write_registration(cmd.shell.into(), Cli::command)?;
            Ok(())
        }
        Commands::Complete(_) => Ok(()),
        Commands::InitSnapshot(cmd) => run_snapshot_init(cmd.into()).await,

        Commands::Save(SaveCmd {
            rev_or_range,
            commit_message,
            commit_message_editor,
        }) => {
            let agent = config::get_config()
                .resolve_agent_settings(config::CommandScope::Save, cli_agent_override.as_ref())?;
            log_agent_runtime_resolution(&agent);
            run_save(
                &rev_or_range,
                &[".vizier/"],
                commit_message,
                commit_message_editor,
                commit_mode,
                push_after,
                &agent,
            )
            .await
        }

        Commands::Ask(cmd) => {
            let message = resolve_ask_message(&cmd)?;
            let agent = config::get_config()
                .resolve_agent_settings(config::CommandScope::Ask, cli_agent_override.as_ref())?;
            log_agent_runtime_resolution(&agent);
            inline_command(message, push_after, &agent, commit_mode).await
        }

        Commands::TestDisplay(cmd) => {
            let opts = resolve_test_display_options(&cmd)?;
            let agent = config::get_config()
                .resolve_agent_settings(opts.scope, cli_agent_override.as_ref())?;
            log_agent_runtime_resolution(&agent);
            run_test_display(opts, &agent).await
        }

        Commands::Draft(cmd) => {
            let resolved = resolve_draft_spec(&cmd)?;
            let agent = config::get_config()
                .resolve_agent_settings(config::CommandScope::Draft, cli_agent_override.as_ref())?;
            log_agent_runtime_resolution(&agent);
            run_draft(
                DraftArgs {
                    spec_text: resolved.text,
                    spec_source: resolved.origin.into(),
                    name_override: cmd.name.clone(),
                },
                &agent,
                commit_mode,
            )
            .await
        }

        Commands::List(cmd) => run_list(resolve_list_options(&cmd)),
        Commands::Cd(cmd) => run_cd(resolve_cd_options(&cmd)?),
        Commands::Clean(cmd) => run_clean(resolve_clean_options(&cmd)?),
        Commands::Plan(_) => run_plan_summary(cli_agent_override.as_ref(), cli.global.json),
        Commands::Jobs(cmd) => run_jobs_command(&project_root, &jobs_root, cmd),

        Commands::Approve(cmd) => {
            let opts = resolve_approve_options(&cmd, push_after)?;
            let agent = config::get_config().resolve_agent_settings(
                config::CommandScope::Approve,
                cli_agent_override.as_ref(),
            )?;
            log_agent_runtime_resolution(&agent);
            run_approve(opts, &agent, commit_mode).await
        }
        Commands::Review(cmd) => {
            let opts = resolve_review_options(&cmd, push_after)?;
            let agent = config::get_config().resolve_agent_settings(
                config::CommandScope::Review,
                cli_agent_override.as_ref(),
            )?;
            log_agent_runtime_resolution(&agent);
            run_review(opts, &agent, commit_mode).await
        }
        Commands::Merge(cmd) => {
            let opts = resolve_merge_options(&cmd, push_after)?;
            let agent = config::get_config()
                .resolve_agent_settings(config::CommandScope::Merge, cli_agent_override.as_ref())?;
            log_agent_runtime_resolution(&agent);
            run_merge(opts, &agent, commit_mode).await
        }
    };

    if let Some(job_id) = cli.global.background_job_id.as_ref() {
        let status = if result.is_ok() {
            JobStatus::Succeeded
        } else {
            JobStatus::Failed
        };

        let mut exit_code = if result.is_ok() { 0 } else { 1 };
        if let Some(run) = auditor::Auditor::latest_agent_run() {
            exit_code = run.exit_code;
        }

        let session_path = auditor::Auditor::persist_session_log().map(|artifact| {
            auditor_cleanup.persisted = true;
            display::info(format!("Session saved to {}", artifact.display_path()));
            if auditor_cleanup.print_json
                && let Ok(contents) = std::fs::read_to_string(&artifact.path) {
                    println!("{contents}");
                }
            artifact.display_path()
        });

        if let Err(err) = jobs::finalize_job(
            &project_root,
            &jobs_root,
            job_id,
            status,
            exit_code,
            session_path,
            runtime_job_metadata(),
        ) {
            display::warn(format!(
                "unable to update background job {} status: {}",
                job_id, err
            ));
        }
    }

    result
}
