use clap::Parser;
use colored::*;

use prompts::tools::get_todo_dir;

use crate::auditor::Auditor;

mod auditor;
mod config;

#[derive(Parser)]
#[command(version, about = "A CLI for LLM project management.")]
struct Args {
    user_message: Option<String>,

    /// List and browse existing TODOs
    #[arg(short = 'l', long)]
    list: bool,

    /// Set LLM provider to use for main prompting + tool usage
    #[arg(short = 'p', long)]
    provider: Option<String>,

    /// Chat interface with LLM
    #[arg(short = 'c', long)]
    chat: bool,

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
}

fn print_usage() {
    eprintln!(
        r#"{} - AI-powered project management assistant

{}
    {} [OPTIONS] [MESSAGE]

{}
    {}    Send a message to the AI assistant

{}
    {}, {}               Start interactive chat session
    {}, {}               Browse and manage TODOs interactively
    {}, {}          Get AI summary of current TODOs
    {}, {} <NAME>    Set LLM provider (openai, anthropic, etc.)
    {}, {}               Print help
    {}, {}            Print version

{}
    {} "add a TODO to implement auth"
    {} --chat
    {} --list
    {} --summarize --provider anthropic"#,
        "vizier".bright_cyan().bold(),
        "USAGE:".bright_yellow().bold(),
        "vizier".bright_green(),
        "ARGS:".bright_yellow().bold(),
        "[MESSAGE]".bright_blue(),
        "OPTIONS:".bright_yellow().bold(),
        "-c".bright_green(),
        "--chat".bright_green(),
        "-l".bright_green(),
        "--list".bright_green(),
        "-s".bright_green(),
        "--summarize".bright_green(),
        "-p".bright_green(),
        "--provider".bright_green(),
        "-h".bright_green(),
        "--help".bright_green(),
        "-V".bright_green(),
        "--version".bright_green(),
        "EXAMPLES:".bright_yellow().bold(),
        "vizier".bright_green(),
        "vizier".bright_green(),
        "vizier".bright_green(),
        "vizier".bright_green()
    );
}

fn find_project_root() -> std::io::Result<Option<std::path::PathBuf>> {
    let mut current_dir = std::env::current_dir()?;

    loop {
        if current_dir.join(".git").is_dir() {
            return Ok(Some(current_dir));
        }

        if let Some(parent) = current_dir.parent() {
            current_dir = parent.to_path_buf();
        } else {
            return Ok(None);
        }
    }
}

fn provider_arg_to_enum(provider: String) -> wire::types::API {
    match provider.as_str() {
        "anthropic" => wire::types::API::Anthropic(wire::types::AnthropicModel::Claude35SonnetNew),
        "openai" => wire::types::API::OpenAI(wire::types::OpenAIModel::GPT4o),
        _ => panic!("Unrecognized LLM provider: {}", provider),
    }
}

fn print_token_usage(response: &wire::types::Message) {
    eprintln!("{}", "Token Usage:".yellow());
    eprintln!("- {} {}", "Prompt Tokens:".green(), response.input_tokens);
    eprintln!(
        "- {} {}",
        "Completion Tokens:".green(),
        response.output_tokens
    );
}

// TODO: This shouldn't be here
const COMMIT_PROMPT: &str = r#"
You are a git commit message writer. Given a git diff, write a clear, concise commit message that follows conventional commit standards.

Structure your commit message as:
- First line: <type>: <brief summary> (50 chars or less)
- Blank line
- Body: Explain what changed and why (wrap at 72 chars)

Common types: feat, fix, docs, style, refactor, test, chore

Focus on the intent and impact of changes, not just listing what files were modified. Be specific but concise.
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let project_root = match find_project_root() {
        Ok(p) => match p {
            Some(pp) => pp,
            None => panic!("vizier cannot be used outside a git repository"),
        },
        Err(e) => panic!("error finding project root: {}", e),
    };

    std::fs::create_dir_all(project_root.join(".vizier"))?;

    // TODO: Bro this condition has got to go
    if args.user_message.is_none() && !args.list && !args.chat && args.save.is_none() {
        print_usage();
        std::process::exit(1);
    }

    if let Some(commit_reference) = args.save {
        if let Ok(output) = std::process::Command::new("git")
            .args(&["diff", &commit_reference, "--", ":!.vizier/"])
            .output()
        {
            if let Ok(diff) = String::from_utf8(output.stdout) {
                let response = Auditor::llm_request_with_tools(
                    crate::config::get_system_prompt()?,
                    format!("Update the snapshot and existing TODOs as needed",),
                    prompts::tools::get_tools(),
                )
                .await?;

                eprintln!("{} {}", "Assistant:".blue(), response.content);
                print_token_usage(&response);

                let commit_message = Auditor::llm_request(COMMIT_PROMPT.to_string(), diff)
                    .await?
                    .content;

                std::process::Command::new("git")
                    .args(&["add", "-u"])
                    .status()?;

                std::process::Command::new("git")
                    .args(&["commit", "-m", &commit_message])
                    .status()?;

                eprintln!("Changes committed with message: {}", commit_message);

                std::process::exit(0);
            }
        }
    }

    if !std::fs::metadata(get_todo_dir()).is_ok() {
        std::fs::create_dir_all(get_todo_dir())?;
    }

    let mut config = config::get_config();

    if let Some(p) = args.provider {
        config.provider = provider_arg_to_enum(p);
    }

    config.force_action = args.force_action;

    config::set_config(config);

    if args.list {
        tui::list_tui(project_root.join(get_todo_dir()))?;
        return Ok(());
    }

    if args.chat {
        tui::chat_tui().await?;
        return Ok(());
    }

    // Default case, `vizier "some message"`

    let response = Auditor::llm_request_with_tools(
        crate::config::get_system_prompt()?,
        args.user_message.unwrap(),
        prompts::tools::get_tools(),
    )
    .await?;

    eprintln!("{} {}", "Assistant:".blue(), response.content);
    print_token_usage(&response);

    Ok(())
}
