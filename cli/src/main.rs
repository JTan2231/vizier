use clap::Parser;
use colored::*;

use prompts::tools::TODO_DIR;

mod config;

#[derive(Parser)]
#[command(version, about = "A CLI for LLM project management.")]
struct Args {
    user_message: Option<String>,

    /// List and browse existing TODOs
    #[arg(short = 'l', long)]
    list: bool,

    /// Summarize outstanding TODOs
    #[arg(short = 's', long)]
    summarize: bool,

    /// Set LLM provider to use for main prompting + tool usage
    #[arg(short = 'p', long)]
    provider: Option<String>,

    /// Chat interface with LLM
    #[arg(short = 'c', long)]
    chat: bool,
}

fn print_usage() {
    println!(
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

// TODO: this will need to account for statuses and whatnot in the future--it doesn't right now
pub async fn summarize_todos() -> Result<wire::types::Message, Box<dyn std::error::Error>> {
    let contents = std::fs::read_dir(prompts::tools::TODO_DIR)
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<Vec<String>>()
        .join("\n\n###\n\n");

    let prompt =
        "You will be given a list of TODO items. Return a summary of all the outstanding work. Focus on broad themes and directions."
            .to_string();

    let response = crate::config::llm_request(vec![], prompt, contents).await?;

    Ok(response)
}

// Make sure the gitignore contains our .vizier folder, don't want to cause a mess
fn gitignore_check(project_root: &std::path::PathBuf) {
    let gitignore =
        std::fs::read_to_string(project_root.join(".gitignore")).unwrap_or(String::new());

    for line in gitignore.lines() {
        if line.starts_with(".vizier") {
            return;
        }
    }

    println!(
        "{} You should add .vizier to your .gitignore",
        "Warning:".yellow()
    );
}

fn print_token_usage(response: &wire::types::Message) {
    println!("{}", "Token Usage:".yellow());
    println!("- {} {}", "Prompt Tokens:".green(), response.input_tokens);
    println!(
        "- {} {}",
        "Completion Tokens:".green(),
        response.output_tokens
    );
}

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

    gitignore_check(&project_root);

    // TODO: Bro this condition has got to go
    if args.user_message.is_none() && !args.list && !args.summarize && !args.chat {
        print_usage();
        std::process::exit(1);
    }

    if args.summarize {
        let response = summarize_todos().await?;
        println!("\r{}", response.content);
        print_token_usage(&response);

        std::process::exit(0);
    }

    if !std::fs::metadata(TODO_DIR).is_ok() {
        std::fs::create_dir_all(TODO_DIR)?;
    }

    let mut config = config::get_config();

    if let Some(p) = args.provider {
        config.provider = provider_arg_to_enum(p);
    }

    config::set_config(config);

    if args.list {
        tui::list_tui(project_root.join(TODO_DIR))?;
        return Ok(());
    }

    if args.chat {
        tui::chat_tui().await?;
        return Ok(());
    }

    let response = crate::config::llm_request_with_tools(
        vec![],
        crate::config::get_system_prompt()?,
        args.user_message.unwrap(),
        prompts::tools::get_tools(),
    )
    .await?;

    println!("{} {}", "Assistant:".blue(), response.content);
    print_token_usage(&response);

    Ok(())
}
