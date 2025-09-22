use std::io::IsTerminal;

use clap::{ArgGroup, Args as ClapArgs, Parser, Subcommand};
use vizier_core::{auditor, config, tools};

mod actions;
use crate::actions::*;

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

#[derive(ClapArgs, Debug, Default)]
struct GlobalOpts {
    /// Enable debug logging
    #[arg(short = 'd', long, global = true)]
    debug: bool,

    /// Set LLM provider to use for main prompting + tool usage
    #[arg(short = 'p', long, global = true)]
    provider: Option<String>,

    /// Emit the audit as JSON to stdout
    #[arg(short = 'j', long, global = true)]
    json: bool,

    /// Require user confirmation for commit messages
    #[arg(short = 'c', long = "require-confirmation", global = true)]
    require_confirmation: bool,

    /// JSON file for setting the config
    #[arg(short = 'C', long = "config-file", global = true)]
    config_file: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Inline one-shot interaction: send a single message and exit
    Ask(AskCmd),

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

    /// Launch interactive chat TUI
    Chat(ChatCmd),
}

#[derive(ClapArgs, Debug)]
struct AskCmd {
    /// The user message to process in a single-shot run
    #[arg(value_name = "MESSAGE")]
    message: Option<String>,
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

#[derive(ClapArgs, Debug)]
struct ChatCmd {}

fn read_all_stdin() -> Result<String, std::io::Error> {
    use std::io::{self, Read};
    let mut buf = String::new();
    io::stdin().read_to_string(&mut buf)?;
    Ok(buf)
}

fn resolve_ask_message(cmd: &AskCmd) -> Result<String, Box<dyn std::error::Error>> {
    match cmd.message.as_deref() {
        Some("-") => {
            // Explicit “read stdin”
            let msg = read_all_stdin()?;
            if msg.trim().is_empty() {
                return Err("stdin is empty; provide MESSAGE or pipe content".into());
            }
            Ok(msg)
        }
        Some(positional) => Ok(positional.to_owned()),
        None => {
            // No positional; try stdin if it’s not a TTY (i.e., piped or redirected)
            if !std::io::stdin().is_terminal() {
                let msg = read_all_stdin()?;
                if msg.trim().is_empty() {
                    return Err("stdin is empty; provide MESSAGE or pipe content".into());
                }
                Ok(msg)
            } else {
                Err("no MESSAGE provided; pass a message, use '-', or pipe stdin".into())
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let project_root = match auditor::find_project_root() {
        Ok(Some(root)) => root,
        Ok(None) => {
            eprintln!("vizier cannot be used outside a git repository");

            return Err("not a git repository".into());
        }
        Err(e) => {
            eprintln!("Error finding project root: {e}");

            return Err(Box::<dyn std::error::Error>::from(e));
        }
    };

    if let Err(e) = std::fs::create_dir_all(project_root.join(".vizier")) {
        eprintln!("Error creating .vizier directory: {e}");

        return Err(Box::<dyn std::error::Error>::from(e));
    }

    let _auditor_cleanup = auditor::AuditorCleanup {
        debug: cli.global.debug,
        print_json: cli.global.json,
    };

    if let Err(e) = std::fs::create_dir_all(tools::get_todo_dir()) {
        eprintln!(
            "Error creating TODO directory {:?}: {e}",
            tools::get_todo_dir()
        );

        return Err(Box::<dyn std::error::Error>::from(e));
    }

    let mut cfg = if let Some(config_file) = cli.global.config_file {
        config::Config::from_json(std::path::PathBuf::from(config_file))?
    } else {
        config::get_config()
    };

    if let Some(p) = &cli.global.provider {
        cfg.provider = provider_arg_to_enum(p.clone());
    }

    cfg.commit_confirmation = cli.global.require_confirmation;

    config::set_config(cfg);

    match cli.command {
        Commands::Clean(CleanCmd { todo_list }) => clean(todo_list).await,

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
            )
            .await
        }

        Commands::Chat(_cmd) => vizier_core::chat::chat_tui().await,

        Commands::Ask(cmd) => {
            let message = resolve_ask_message(&cmd)?;
            inline_command(message).await
        }
    }
}
