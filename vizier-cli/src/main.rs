use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
};

use clap::{ArgAction, ArgGroup, Args as ClapArgs, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use vizier_core::{
    auditor, config,
    display::{self, LogLevel},
    tools, vcs,
};

mod actions;
use crate::actions::*;
mod completions;
mod plan;

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
    /// Increase verbosity (`-v` = info, `-vv` = debug)
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count, global = true)]
    verbose: u8,

    /// Silence all non-error output
    #[arg(short = 'q', long, global = true)]
    quiet: bool,

    /// Enable debug logging (alias for -vv)
    #[arg(short = 'd', long, global = true)]
    debug: bool,

    /// Disable ANSI control sequences even on TTYs
    #[arg(long = "no-ansi", global = true)]
    no_ansi: bool,

    /// Progress display mode for long-running operations
    #[arg(long = "progress", value_enum, default_value_t = ProgressArg::Auto, global = true)]
    progress: ProgressArg,

    /// Load session as existing context
    #[arg(short = 'l', long = "load-session", global = true)]
    load_session: Option<String>,

    /// Load session as existing context
    #[arg(short = 'n', long = "no-session", global = true)]
    no_session: bool,

    /// Set LLM model for wire-backed runs (agent backends ignore this; configure models in .vizier/config.toml instead)
    #[arg(short = 'p', long, global = true)]
    model: Option<String>,

    /// Backend to use for edit orchestration (`agent`, `gemini`, or `wire`). Commands fail fast when the selected backend rejects the run; there is no automatic fallback.
    #[arg(long = "backend", value_enum, global = true)]
    backend: Option<BackendArg>,

    /// Bundled agent shim label to run (for example, `codex` or `gemini`)
    #[arg(long = "agent-label", value_name = "LABEL", global = true)]
    agent_label: Option<String>,

    /// Path to a custom agent script (stdout = assistant text; stderr = progress/errors)
    #[arg(long = "agent-command", value_name = "PATH", global = true)]
    agent_command: Option<String>,

    /// Emit the audit as JSON to stdout
    #[arg(short = 'j', long, global = true)]
    json: bool,

    /// Config file to load (supports JSON or TOML)
    #[arg(short = 'C', long = "config-file", global = true)]
    config_file: Option<String>,

    /// Override model reasoning effort (minimal, low, medium, high)
    #[arg(short = 'r', long = "reasoning-effort", global = true)]
    reasoning_effort: Option<String>,

    /// Push the current branch to origin after mutating git history
    #[arg(short = 'P', long, global = true)]
    push: bool,

    /// Leave changes staged/dirty instead of committing automatically
    #[arg(long = "no-commit", action = ArgAction::SetTrue, global = true)]
    no_commit: bool,
}

impl Default for GlobalOpts {
    fn default() -> Self {
        Self {
            verbose: 0,
            quiet: false,
            debug: false,
            no_ansi: false,
            progress: ProgressArg::Auto,
            load_session: None,
            no_session: false,
            model: None,
            backend: None,
            agent_label: None,
            agent_command: None,
            json: false,
            config_file: None,
            reasoning_effort: None,
            push: false,
            no_commit: false,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ProgressArg {
    Auto,
    Never,
    Always,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum BackendArg {
    #[value(alias = "codex")]
    Agent,
    Gemini,
    Wire,
}

impl From<BackendArg> for config::BackendKind {
    fn from(value: BackendArg) -> Self {
        match value {
            BackendArg::Agent => config::BackendKind::Agent,
            BackendArg::Gemini => config::BackendKind::Gemini,
            BackendArg::Wire => config::BackendKind::Wire,
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

impl From<ProgressArg> for display::ProgressMode {
    fn from(value: ProgressArg) -> Self {
        match value {
            ProgressArg::Auto => display::ProgressMode::Auto,
            ProgressArg::Never => display::ProgressMode::Never,
            ProgressArg::Always => display::ProgressMode::Always,
        }
    }
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Inline one-shot interaction: send a single message and exit
    Ask(AskCmd),

    /// Generate an implementation-plan draft branch from an operator spec
    Draft(DraftCmd),

    /// List pending implementation-plan branches that are ahead of the target branch
    List(ListCmd),

    /// Print the resolved configuration and exit
    Plan(PlanCmd),

    /// Generate shell completion scripts
    Completions(CompletionsCmd),

    /// Internal completion entry point (invoked by shell integration)
    #[command(name = "__complete", hide = true)]
    Complete(HiddenCompleteCmd),

    /// Approve plan branches created by `vizier draft`
    Approve(ApproveCmd),

    /// Review a plan branch, run checks, and optionally apply fixes
    Review(ReviewCmd),

    /// Merge approved plan branches back into the primary branch
    Merge(MergeCmd),

    /// Bootstrap `.vizier/.snapshot` and TODO threads from repo history
    #[command(name = "init-snapshot")]
    InitSnapshot(SnapshotInitCmd),

    /// Smoke-test the configured agent/display wiring without touching `.vizier`
    #[command(name = "test-display")]
    TestDisplay(TestDisplayCmd),

    /// Commit tracked changes with an LLM-generated message and update TODOs/snapshot
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
struct PlanCmd {}

#[derive(ClapArgs, Debug)]
struct ApproveCmd {
    /// Plan slug to approve
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

    /// Skip confirmation prompt
    #[arg(long = "yes", short = 'y')]
    assume_yes: bool,
}

#[derive(ClapArgs, Debug)]
struct ReviewCmd {
    /// Plan slug to review
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

    /// Skip running configured review checks (e.g., cargo test)
    #[arg(long = "skip-checks")]
    skip_checks: bool,
}

#[derive(ClapArgs, Debug)]
struct MergeCmd {
    /// Plan slug to merge
    #[arg(value_name = "PLAN", add = crate::completions::plan_slug_completer())]
    plan: Option<String>,

    /// Destination branch for merge (defaults to detected primary)
    #[arg(long = "target", value_name = "BRANCH")]
    target: Option<String>,

    /// Draft branch name when it deviates from draft/<plan>
    #[arg(long = "branch", value_name = "BRANCH")]
    branch: Option<String>,

    /// Skip confirmation prompt
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

    /// Squash implementation commits before creating the merge commit
    #[arg(long = "squash", action = ArgAction::SetTrue, conflicts_with = "no_squash")]
    squash: bool,

    /// Preserve implementation commits (legacy behavior)
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
    /// Overwrite existing snapshot/TODOs without confirmation
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

fn resolve_approve_options(
    cmd: &ApproveCmd,
    push_after: bool,
) -> Result<ApproveOptions, Box<dyn std::error::Error>> {
    if !cmd.list && cmd.plan.is_none() {
        return Err(
            "plan argument is required (use `vizier list` to inspect pending drafts)".into(),
        );
    }

    Ok(ApproveOptions {
        plan: cmd.plan.clone(),
        list_only: cmd.list,
        target: cmd.target.clone(),
        branch_override: cmd.branch.clone(),
        assume_yes: cmd.assume_yes,
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
        .ok_or_else(|| "plan argument is required for vizier review")?;

    Ok(ReviewOptions {
        plan,
        target: cmd.target.clone(),
        branch_override: cmd.branch.clone(),
        assume_yes: cmd.assume_yes,
        review_only: cmd.review_only,
        skip_checks: cmd.skip_checks,
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
        .ok_or_else(|| "plan argument is required for vizier merge")?;
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
    if let Some(mainline) = squash_mainline {
        if mainline == 0 {
            return Err("squash mainline parent index must be at least 1".into());
        }
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

    if let Some(backend) = opts.backend {
        overrides.backend = Some(backend.into());
    }

    if let Some(model) = opts.model.as_ref() {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            return Err("model name cannot be empty".into());
        }
        overrides.model = Some(trimmed.to_string());
    }

    if let Some(reasoning) = opts.reasoning_effort.as_ref() {
        let trimmed = reasoning.trim();
        if trimmed.is_empty() {
            return Err("reasoning effort cannot be empty".into());
        }
        overrides.reasoning_effort = Some(wire::config::ThinkingLevel::from_string(trimmed)?);
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

fn warn_if_model_override_ignored(
    model_override_requested: bool,
    scope: config::CommandScope,
    agent: &config::AgentSettings,
) {
    if model_override_requested && agent.backend != config::BackendKind::Wire {
        display::warn(format!(
            "--model override ignored for `{}` because the {} backend is active; update [agents.{}] or rerun with --backend wire.",
            scope.as_str(),
            agent.backend,
            scope.as_str()
        ));
    }
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
    if crate::completions::try_handle_completion(|| Cli::command())
        .map_err(|err| Box::<dyn std::error::Error>::from(err))?
    {
        return Ok(());
    }

    let cli = Cli::parse();

    let stdout_is_tty = std::io::stdout().is_terminal();
    let stderr_is_tty = std::io::stderr().is_terminal();

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

    let ansi_enabled = !cli.global.no_ansi && stdout_is_tty && stderr_is_tty;

    display::set_display_config(display::DisplayConfig {
        verbosity,
        progress: cli.global.progress.into(),
        ansi_enabled,
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

    let _auditor_cleanup = auditor::AuditorCleanup {
        debug: cli.global.debug,
        print_json: cli.global.json,
    };

    if let Err(e) = std::fs::create_dir_all(tools::get_todo_dir()) {
        display::emit(
            LogLevel::Error,
            format!(
                "Error creating TODO directory {:?}: {e}",
                tools::get_todo_dir()
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

        let _ = auditor::Auditor::replace_messages(&messages);
    }

    cfg.no_session = cli.global.no_session;

    if let Some(backend_arg) = cli.global.backend {
        cfg.backend = backend_arg.into();
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

    let mut provider_needs_rebuild =
        cfg.provider_model != config::DEFAULT_MODEL || cfg.reasoning_effort.is_some();

    if let Some(model) = &cli.global.model {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            return Err("model name cannot be empty".into());
        }
        if cfg.backend == config::BackendKind::Wire {
            cfg.provider_model = trimmed.to_owned();
            provider_needs_rebuild = true;
        }
    }

    if let Some(reasoning_effort) = &cli.global.reasoning_effort {
        let trimmed = reasoning_effort.trim();
        if trimmed.is_empty() {
            return Err("reasoning effort cannot be empty".into());
        }

        cfg.reasoning_effort = Some(wire::config::ThinkingLevel::from_string(trimmed)?);
        provider_needs_rebuild = true;
    }

    if provider_needs_rebuild {
        cfg.provider =
            config::Config::provider_from_settings(&cfg.provider_model, cfg.reasoning_effort)?;
    }

    let workflow_defaults = cfg.workflow.clone();

    config::set_config(cfg);

    let cli_agent_override = build_cli_agent_overrides(&cli.global)?;
    let model_override_requested = cli
        .global
        .model
        .as_ref()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);

    let push_after = cli.global.push;
    let commit_mode = if cli.global.no_commit {
        CommitMode::HoldForReview
    } else if workflow_defaults.no_commit_default {
        CommitMode::HoldForReview
    } else {
        CommitMode::AutoCommit
    };

    match cli.command {
        Commands::Completions(cmd) => {
            crate::completions::write_registration(cmd.shell.into(), || Cli::command())?;
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
            warn_if_model_override_ignored(
                model_override_requested,
                config::CommandScope::Save,
                &agent,
            );
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
            warn_if_model_override_ignored(
                model_override_requested,
                config::CommandScope::Ask,
                &agent,
            );
            inline_command(message, push_after, &agent, commit_mode).await
        }

        Commands::TestDisplay(cmd) => {
            let opts = resolve_test_display_options(&cmd)?;
            let agent = config::get_config()
                .resolve_agent_settings(opts.scope, cli_agent_override.as_ref())?;
            log_agent_runtime_resolution(&agent);
            warn_if_model_override_ignored(model_override_requested, opts.scope, &agent);
            run_test_display(opts, &agent).await
        }

        Commands::Draft(cmd) => {
            let resolved = resolve_draft_spec(&cmd)?;
            let agent = config::get_config()
                .resolve_agent_settings(config::CommandScope::Draft, cli_agent_override.as_ref())?;
            log_agent_runtime_resolution(&agent);
            warn_if_model_override_ignored(
                model_override_requested,
                config::CommandScope::Draft,
                &agent,
            );
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
        Commands::Plan(_) => run_plan_summary(cli_agent_override.as_ref(), cli.global.json),

        Commands::Approve(cmd) => {
            let opts = resolve_approve_options(&cmd, push_after)?;
            let agent = config::get_config().resolve_agent_settings(
                config::CommandScope::Approve,
                cli_agent_override.as_ref(),
            )?;
            log_agent_runtime_resolution(&agent);
            warn_if_model_override_ignored(
                model_override_requested,
                config::CommandScope::Approve,
                &agent,
            );
            run_approve(opts, &agent, commit_mode).await
        }
        Commands::Review(cmd) => {
            let opts = resolve_review_options(&cmd, push_after)?;
            let agent = config::get_config().resolve_agent_settings(
                config::CommandScope::Review,
                cli_agent_override.as_ref(),
            )?;
            log_agent_runtime_resolution(&agent);
            warn_if_model_override_ignored(
                model_override_requested,
                config::CommandScope::Review,
                &agent,
            );
            run_review(opts, &agent, commit_mode).await
        }
        Commands::Merge(cmd) => {
            let opts = resolve_merge_options(&cmd, push_after)?;
            let agent = config::get_config()
                .resolve_agent_settings(config::CommandScope::Merge, cli_agent_override.as_ref())?;
            log_agent_runtime_resolution(&agent);
            warn_if_model_override_ignored(
                model_override_requested,
                config::CommandScope::Merge,
                &agent,
            );
            run_merge(opts, &agent, commit_mode).await
        }
    }
}
