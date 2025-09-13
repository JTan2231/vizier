use clap::Parser;

use vizier_core::{auditor, config, tools};

mod actions;

use crate::actions::*;

#[derive(Parser)]
#[command(version, about = "A CLI for LLM project management.")]
struct Args {
    user_message: Option<String>,

    #[arg(short = 'd', long)]
    debug: bool,

    /// Set LLM provider to use for main prompting + tool usage
    #[arg(short = 'p', long)]
    provider: Option<String>,

    /// Force the agent to perform an action
    #[arg(short = 'f', long)]
    force_action: bool,

    /// "Save" button--git commit tracked changes w.r.t given commit reference/range with LLM-generated commit message and update TODOs/snapshot
    /// to reflect the changes
    /// e.g., `vizier -s HEAD~3..HEAD`, or `vizier -s HEAD`
    #[arg(short = 's', long)]
    save: Option<String>,

    /// Equivalent to `vizier -s HEAD`
    #[arg(short = 'S', long)]
    save_latest: bool,

    /// Developer note to append to the commit message. Note that this goes on to the _code_ commit
    /// messaging--it doesn't affect any commits surrounding conversations or the `.vizier`
    /// directory. Note that this is mutually exclusive with `-M`.
    #[arg(short = 'm', long)]
    commit_message: Option<String>,

    /// Same as `-m`, but opens the default terminal editor ($EDITOR) instead of accepting an
    /// argument
    #[arg(short = 'M', long)]
    commit_message_editor: bool,

    /// Spit out the audit in JSON to stdout
    #[arg(short = 'j', long)]
    json: bool,

    /// Comma-delimited list of TODO names for the agent to decide whether to revise/leave/remove.
    /// Use `*` to address all TODO items
    #[arg(short = 'c', long)]
    clean: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let project_root = match find_project_root() {
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

    let no_primary_action = args.user_message.is_none()
        && args.save.is_none()
        && args.clean.is_none()
        && !args.save_latest;
    let invalid_commit_msg_flags = args.commit_message.is_some() && args.commit_message_editor;
    if no_primary_action || invalid_commit_msg_flags {
        print_usage();
        return Ok(());
    }

    let _auditor_cleanup = auditor::AuditorCleanup {
        debug: args.debug,
        print_json: args.json,
    };

    if let Some(todo_list) = args.clean {
        return clean(todo_list).await;
    }

    if let Some(commit_reference) = args.save.as_deref() {
        return run_save(
            commit_reference,
            &[".vizier/"],
            args.commit_message,
            args.commit_message_editor,
        )
        .await;
    }

    if args.save_latest {
        return run_save(
            "HEAD",
            &[".vizier/"],
            args.commit_message,
            args.commit_message_editor,
        )
        .await;
    }

    if let Err(e) = std::fs::create_dir_all(tools::get_todo_dir()) {
        eprintln!(
            "Error creating TODO directory {:?}: {e}",
            tools::get_todo_dir()
        );
        return Err(Box::<dyn std::error::Error>::from(e));
    }

    let mut cfg = config::get_config();
    if let Some(p) = args.provider {
        cfg.provider = provider_arg_to_enum(p);
    }

    cfg.force_action = args.force_action;
    config::set_config(cfg);

    inline_command(
        args.user_message
            .expect("There should be a user message already guarded against--how'd we get here?"),
    )
    .await
}
