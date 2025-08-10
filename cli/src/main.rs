use clap::Parser;

use crate::tools::TODO_DIR;

mod config;
mod file_tracking;
mod tools;
mod tree;
mod walker;

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
}

fn print_usage() {
    println!(
        r#"
A CLI for LLM project management.

Usage: llm-cli [OPTIONS] [USER_MESSAGE]

Arguments:
  [USER_MESSAGE]  Message to process with the LLM

Options:
  -l, --list              List and browse existing TODOs
  -p, --provider <NAME>   Set LLM provider (e.g., 'openai', 'anthropic')
  -h, --help             Show this help message
  -V, --version          Show version information
"#
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
        "anthropic" => wire::types::API::Anthropic(wire::types::AnthropicModel::Claude35Sonnet),
        "openai" => wire::types::API::OpenAI(wire::types::OpenAIModel::GPT4o),
        _ => panic!("Unrecognized LLM provider: {}", provider),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // TODO: Bro
    if args.user_message.is_none() && !args.list && !args.summarize {
        print_usage();
        std::process::exit(1);
    }

    if args.summarize {
        tui::call_with_status(async move |tx| {
            tx.send(tui::Status::Working("Summarizing TODOs...".into()))
                .await?;
            println!("\r{}", tools::summarize_todos().await?);
            Ok(())
        })
        .await
        .unwrap();

        std::process::exit(0);
    }

    if !std::fs::metadata(TODO_DIR).is_ok() {
        std::fs::create_dir_all(TODO_DIR)?;
    }

    let project_root = match find_project_root() {
        Ok(p) => match p {
            Some(pp) => pp,
            None => panic!("vizier cannot be used outside a git repository"),
        },
        Err(e) => panic!("error finding project root: {}", e),
    };

    let mut config = config::get_config();

    if let Some(p) = args.provider {
        config.provider = provider_arg_to_enum(p);
    }

    config::set_config(config);

    if args.list {
        tui::tui(project_root.join(TODO_DIR))?;
        return Ok(());
    }

    dewey_lib::config::setup()?;

    let response = crate::config::llm_request_with_tools(
        vec![],
        crate::config::get_system_prompt()?,
        args.user_message.unwrap(),
        crate::tools::get_tools(),
    )
    .await?;

    println!(
        "FINAL RESPONSE: {}",
        response.iter().last().unwrap().content
    );

    Ok(())
}
