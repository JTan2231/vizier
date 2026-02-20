use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::{ColorChoice, CommandFactory, FromArgMatches, error::ErrorKind};
use vizier_core::{
    auditor, config,
    display::{self, LogLevel},
};

use crate::actions::{
    run_cd, run_clean, run_init, run_list, run_release, run_workflow, run_workflow_audit,
};
use crate::cli::args::*;
use crate::cli::help::{
    curated_help_text, pager_mode_from_args, render_clap_help_text,
    render_clap_subcommand_help_text, render_help_with_pager, render_run_workflow_help_text,
    strip_ansi_codes, subcommand_from_raw_args,
};
use crate::cli::jobs_view::run_jobs_command;
use crate::cli::resolve::{resolve_cd_options, resolve_clean_options, resolve_list_options};
use crate::cli::util::{
    flag_present, global_option_value, normalize_run_invocation_args, run_flow_help_target,
};
use crate::jobs;

pub(crate) async fn run() -> Result<(), Box<dyn std::error::Error>> {
    if crate::completions::try_handle_completion(Cli::command)
        .map_err(Box::<dyn std::error::Error>::from)?
    {
        return Ok(());
    }

    let stdout_is_tty = std::io::stdout().is_terminal();
    let stderr_is_tty = std::io::stderr().is_terminal();
    let raw_args: Vec<String> = std::env::args().collect();
    let quiet_requested = flag_present(&raw_args, Some('q'), "--quiet");
    let no_ansi_requested = flag_present(&raw_args, None, "--no-ansi");
    let pager_mode = pager_mode_from_args(&raw_args);
    let normalized_args = normalize_run_invocation_args(&raw_args);

    let color_choice = if !no_ansi_requested && stdout_is_tty && stderr_is_tty {
        ColorChoice::Auto
    } else {
        ColorChoice::Never
    };

    let command = Cli::command().color(color_choice);
    let matches = match command.try_get_matches_from(&normalized_args) {
        Ok(matches) => matches,
        Err(err) => match err.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                if let Some(flow) = run_flow_help_target(&normalized_args) {
                    let project_root = resolve_project_root()?;
                    let explicit_config_file =
                        global_option_value(&normalized_args, Some('C'), "--config-file");
                    let cfg =
                        load_effective_config(&project_root, explicit_config_file.as_deref())?;
                    let rendered = render_run_workflow_help_text(&project_root, &flow, &cfg)?;
                    let help_text = if color_choice != ColorChoice::Never {
                        rendered
                    } else {
                        strip_ansi_codes(&rendered)
                    };
                    render_help_with_pager(&help_text, pager_mode, stdout_is_tty, quiet_requested)?;
                    return Ok(());
                }

                let rendered = match err.kind() {
                    ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                        curated_help_text().to_string()
                    }
                    ErrorKind::DisplayHelp => {
                        if subcommand_from_raw_args(&raw_args).is_none() {
                            curated_help_text().to_string()
                        } else {
                            err.render().to_string()
                        }
                    }
                    _ => unreachable!("caller guards ensure help error kinds"),
                };

                let help_text = if color_choice != ColorChoice::Never {
                    rendered
                } else {
                    strip_ansi_codes(&rendered)
                };

                render_help_with_pager(&help_text, pager_mode, stdout_is_tty, quiet_requested)?;
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

    if let Commands::Help(cmd) = &cli.command {
        let rendered = if cmd.all {
            render_clap_help_text(color_choice)
        } else if let Some(command) = cmd.command.as_deref() {
            render_clap_subcommand_help_text(color_choice, command)?
        } else {
            curated_help_text().to_string()
        };

        let help_text = if color_choice != ColorChoice::Never {
            rendered
        } else {
            strip_ansi_codes(&rendered)
        };

        render_help_with_pager(&help_text, pager_mode, stdout_is_tty, quiet_requested)?;
        return Ok(());
    }

    if let Commands::Completions(cmd) = cli.command {
        crate::completions::write_registration(cmd.shell.into(), Cli::command)?;
        return Ok(());
    }

    if matches!(cli.command, Commands::Complete(_)) {
        return Ok(());
    }

    let project_root = resolve_project_root()?;
    let mut cfg = load_effective_config(&project_root, cli.global.config_file.as_deref())?;

    if let Some(session_id) = &cli.global.load_session {
        let repo_session = project_root
            .join(".vizier")
            .join("sessions")
            .join(session_id)
            .join("session.json");

        let messages = if repo_session.exists() {
            auditor::Auditor::load_session_messages_from_path(&repo_session)?
        } else {
            return Err("could not find session file".into());
        };

        auditor::Auditor::replace_messages(&messages);
    }

    cfg.no_session = cli.global.no_session;
    config::set_config(cfg);

    match cli.command {
        Commands::Help(_) => Ok(()),
        Commands::Completions(_) | Commands::Complete(_) => Ok(()),
        Commands::Init(cmd) => run_init(&project_root, cmd.check),
        Commands::List(cmd) => run_list(resolve_list_options(&cmd)?),
        Commands::Cd(cmd) => run_cd(resolve_cd_options(&cmd)?),
        Commands::Clean(cmd) => run_clean(resolve_clean_options(&cmd)?),
        Commands::Jobs(cmd) => {
            let jobs_root = jobs::ensure_jobs_root(&project_root)?;
            run_jobs_command(&project_root, &jobs_root, cmd, cli.global.no_ansi)
        }
        Commands::Run(cmd) => {
            let jobs_root = jobs::ensure_jobs_root(&project_root)?;
            run_workflow(&project_root, &jobs_root, cmd)
        }
        Commands::Audit(cmd) => run_workflow_audit(&project_root, cmd),
        Commands::WorkflowNode(cmd) => {
            let jobs_root = jobs::ensure_jobs_root(&project_root)?;
            jobs::run_workflow_node_command(&project_root, &jobs_root, &cmd.job_id)
        }
        Commands::Release(cmd) => run_release(cmd),
    }
}

fn resolve_project_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    match auditor::find_project_root() {
        Ok(Some(root)) => Ok(root),
        Ok(None) => {
            display::emit(
                LogLevel::Error,
                "vizier cannot be used outside a git repository",
            );
            Err("not a git repository".into())
        }
        Err(e) => {
            display::emit(LogLevel::Error, format!("Error finding project root: {e}"));
            Err(Box::<dyn std::error::Error>::from(e))
        }
    }
}

fn load_effective_config(
    project_root: &Path,
    explicit_config_file: Option<&str>,
) -> Result<config::Config, Box<dyn std::error::Error>> {
    if let Some(config_file) = explicit_config_file {
        return config::load_config_from_path(PathBuf::from(config_file));
    }

    let mut layers = Vec::new();

    if let Some(path) = config::global_config_path().filter(|path| path.exists()) {
        display::emit(
            LogLevel::Info,
            format!("Loading global config from {}", path.display()),
        );
        layers.push(config::load_config_layer_from_path(path)?);
    }

    if let Some(path) = config::project_config_path(project_root) {
        display::emit(
            LogLevel::Info,
            format!("Loading repo config from {}", path.display()),
        );
        layers.push(config::load_config_layer_from_path(path)?);
    }

    if !layers.is_empty() {
        return Ok(config::Config::from_layers(&layers));
    }

    if let Some(path) = config::env_config_path().filter(|path| path.exists()) {
        display::emit(
            LogLevel::Info,
            format!("Loading env config from {}", path.display()),
        );
        return config::load_config_from_path(path);
    }

    Ok(config::get_config())
}
