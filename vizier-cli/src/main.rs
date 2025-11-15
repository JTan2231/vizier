use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
};

use clap::{ArgAction, ArgGroup, Args as ClapArgs, Parser, Subcommand, ValueEnum};
use vizier_core::{
    auditor, config,
    display::{self, LogLevel},
    tools,
};

mod actions;
use crate::actions::*;
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

    /// Set LLM model to use for main prompting + tool usage
    #[arg(short = 'p', long, global = true)]
    model: Option<String>,

    /// Backend to use for edit orchestration (`codex` or `wire`)
    #[arg(long = "backend", value_enum, global = true)]
    backend: Option<BackendArg>,

    /// Path to the Codex binary (defaults to resolving `codex` on PATH)
    #[arg(long = "codex-bin", value_name = "PATH", global = true)]
    codex_bin: Option<PathBuf>,

    /// Codex profile to load (pass empty to unset)
    #[arg(long = "codex-profile", value_name = "NAME", global = true)]
    codex_profile: Option<String>,

    /// Override Codex bounds prompt with a file on disk
    #[arg(long = "codex-bounds-prompt", value_name = "PATH", global = true)]
    codex_bounds_prompt: Option<PathBuf>,

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
            codex_bin: None,
            codex_profile: None,
            codex_bounds_prompt: None,
            json: false,
            config_file: None,
            reasoning_effort: None,
            push: false,
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
    Codex,
    Wire,
}

impl From<BackendArg> for config::BackendKind {
    fn from(value: BackendArg) -> Self {
        match value {
            BackendArg::Codex => config::BackendKind::Codex,
            BackendArg::Wire => config::BackendKind::Wire,
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

    /// Approve plan branches created by `vizier draft`
    Approve(ApproveCmd),

    /// Merge approved plan branches back into the primary branch
    Merge(MergeCmd),

    /// Documentation utilities
    Docs(DocsCmd),

    /// Snapshot-related operations (e.g., bootstrap from history)
    Snapshot(SnapshotCmd),

    /// Alias for `snapshot init`
    #[command(name = "init-snapshot")]
    InitSnapshot(SnapshotInitCmd),

    /// Commit tracked changes with an LLM-generated message and update TODOs/snapshot
    ///
    /// Examples:
    ///   vizier save                # defaults to HEAD
    ///   vizier save HEAD~3..HEAD   # explicit range
    ///   vizier save main           # single rev compared to workdir/index
    Save(SaveCmd),

    /// Decide whether to revise/leave/remove selected TODOs (use "*" for all)
    ///
    /// Examples:
    ///   vizier clean "*"
    ///   vizier clean "Parser bugs,UI polish"
    Clean(CleanCmd),
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
struct ApproveCmd {
    /// Plan slug to approve (omit when using --list)
    #[arg(value_name = "PLAN")]
    plan: Option<String>,

    /// List pending plan branches instead of approving
    #[arg(long = "list")]
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
struct MergeCmd {
    /// Plan slug to merge
    #[arg(value_name = "PLAN")]
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

    /// Delete the draft branch locally after merge
    #[arg(long = "delete-branch")]
    delete_branch: bool,

    /// Optional note appended to the merge commit body
    #[arg(long = "note", value_name = "TEXT")]
    note: Option<String>,

    /// Attempt Codex-backed auto-resolution when conflicts arise
    #[arg(long = "auto-resolve-conflicts")]
    auto_resolve_conflicts: bool,
}

#[derive(ClapArgs, Debug)]
struct DocsCmd {
    #[command(subcommand)]
    command: DocsCommands,
}

#[derive(Subcommand, Debug)]
enum DocsCommands {
    /// Emit or scaffold architecture documentation prompts
    Prompt(DocsPromptCmd),
}

#[derive(ClapArgs, Debug)]
struct DocsPromptCmd {
    #[arg(value_enum)]
    scope: DocsPromptScope,

    /// Write the template to PATH (use "-" for stdout)
    #[arg(long = "write", value_name = "PATH", conflicts_with = "scaffold")]
    write: Option<PathBuf>,

    /// Scaffold the template under .vizier/docs/prompting/
    #[arg(long, conflicts_with = "write")]
    scaffold: bool,

    /// Overwrite destination when used with --write or --scaffold
    #[arg(long)]
    force: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum DocsPromptScope {
    #[value(alias = "architecture_overview")]
    ArchitectureOverview,
    #[value(alias = "subsystem_detail")]
    SubsystemDetail,
    #[value(alias = "interface_summary")]
    InterfaceSummary,
    #[value(alias = "invariant_capture")]
    InvariantCapture,
    #[value(alias = "operational_thread")]
    OperationalThread,
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

#[derive(Subcommand, Debug)]
enum SnapshotCommands {
    /// Analyze repository history and bootstrap `.vizier/.snapshot` plus TODO threads
    Init(SnapshotInitCmd),
}

#[derive(ClapArgs, Debug)]
struct SnapshotCmd {
    #[command(subcommand)]
    command: SnapshotCommands,
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

#[derive(ClapArgs, Debug)]
struct CleanCmd {
    /// Comma-delimited list of TODO names, or "*" for all.
    ///
    /// Example: "*"  or  "Parser bugs,UI polish"
    #[arg(value_name = "TODO_LIST")]
    todo_list: String,
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

fn resolve_approve_options(
    cmd: &ApproveCmd,
    push_after: bool,
) -> Result<ApproveOptions, Box<dyn std::error::Error>> {
    if !cmd.list && cmd.plan.is_none() {
        return Err("plan argument is required unless --list is set".into());
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

fn resolve_merge_options(
    cmd: &MergeCmd,
    push_after: bool,
) -> Result<MergeOptions, Box<dyn std::error::Error>> {
    let plan = cmd
        .plan
        .clone()
        .ok_or_else(|| "plan argument is required for vizier merge")?;
    let conflict_strategy = if cmd.auto_resolve_conflicts {
        MergeConflictStrategy::Codex
    } else {
        MergeConflictStrategy::Manual
    };

    Ok(MergeOptions {
        plan,
        target: cmd.target.clone(),
        branch_override: cmd.branch.clone(),
        assume_yes: cmd.assume_yes,
        delete_branch: cmd.delete_branch,
        note: cmd.note.clone(),
        push_after,
        conflict_strategy,
    })
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

    let mut cfg = if let Some(config_file) = cli.global.config_file {
        config::Config::from_path(std::path::PathBuf::from(config_file))?
    } else if let Some(default_path) = config::default_config_path() {
        if default_path.exists() {
            config::Config::from_path(default_path)?
        } else {
            config::get_config()
        }
    } else {
        config::get_config()
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

    if let Some(backend_arg) = cli.global.backend {
        cfg.backend = backend_arg.into();
    }

    if let Some(bin_override) = cli.global.codex_bin {
        cfg.codex.binary_path = bin_override;
    }

    if let Some(profile_override) = &cli.global.codex_profile {
        let trimmed = profile_override.trim();
        if trimmed.is_empty() {
            cfg.codex.profile = None;
        } else {
            cfg.codex.profile = Some(trimmed.to_owned());
        }
    }

    if let Some(bounds_override) = cli.global.codex_bounds_prompt.clone() {
        cfg.codex.bounds_prompt_path = Some(bounds_override);
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
        } else {
            display::warn(
                "--model is ignored when Codex is the active backend; rerun with --backend wire \
                 if you need to force the HTTP provider stack.",
            );
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

    config::set_config(cfg);

    let push_after = cli.global.push;

    match cli.command {
        Commands::Clean(CleanCmd { todo_list }) => clean(todo_list, push_after).await,

        Commands::Docs(DocsCmd { command }) => match command {
            DocsCommands::Prompt(cmd) => docs_prompt(cmd).await,
        },

        Commands::Snapshot(SnapshotCmd { command }) => match command {
            SnapshotCommands::Init(cmd) => run_snapshot_init(cmd.into()).await,
        },

        Commands::InitSnapshot(cmd) => run_snapshot_init(cmd.into()).await,

        Commands::Save(SaveCmd {
            rev_or_range,
            commit_message,
            commit_message_editor,
        }) => {
            run_save(
                &rev_or_range,
                &[".vizier/"],
                commit_message,
                commit_message_editor,
                push_after,
            )
            .await
        }

        Commands::Ask(cmd) => {
            let message = resolve_ask_message(&cmd)?;
            inline_command(message, push_after).await
        }

        Commands::Draft(cmd) => {
            let resolved = resolve_draft_spec(&cmd)?;
            run_draft(DraftArgs {
                spec_text: resolved.text,
                spec_source: resolved.origin.into(),
                name_override: cmd.name.clone(),
            })
            .await
        }

        Commands::Approve(cmd) => run_approve(resolve_approve_options(&cmd, push_after)?).await,

        Commands::Merge(cmd) => run_merge(resolve_merge_options(&cmd, push_after)?).await,
    }
}
