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

    /// Set LLM provider to use for main prompting + tool usage
    #[arg(short = 'p', long)]
    provider: Option<String>,
}

const SYSTEM_PROMPT_BASE: &str = r#"
<mainInstruction>
Your Job: Convert TODOs into Actionable Tasks

RULES:
- Convert any TODO comments into specific, actionable requirements
- Every task MUST include:
  - Exact file location (with line numbers when possible)
  - Concrete technical solution/approach
  - Direct references to existing code/structure
- NO investigation/research tasks - you do that work first
- NO maybes or suggestions - be decisive
- NO progress updates or explanations to the user
- Format as a simple task list
- Assume authority to make technical decisions
- Your output should _always_ be through creating or updating a TODO item with the given tools
- NEVER ask the user if they want something done--always assume
- _Aggressively_ search the project for additional context to answer any questions you may have
- _Aggressively_ update existing TODOs as much as you create new ones

Example:
BAD: "Investigate performance issues in search"
GOOD: "Replace recursive DFS in hnsw.rs:156 with iterative stack-based implementation using Vec<Node>"

Using these rules, convert TODOs from the codebase into actionable tasks.
</mainInstruction>
"#;

fn get_system_prompt() -> Result<String, Box<dyn std::error::Error>> {
    let mut prompt = SYSTEM_PROMPT_BASE.to_string();

    prompt.push_str("<meta>");

    let file_tree = crate::tree::build_tree()?;

    prompt.push_str(&format!(
        "<fileTree>{}</fileTree>",
        crate::tree::tree_to_string(&file_tree, "")
    ));

    prompt.push_str(&format!("<todos>{}</todos>", crate::tools::list_todos()));

    prompt.push_str(&format!(
        "<currentWorkingDirectory>{}</currentWorkingDirectory>",
        std::env::current_dir().unwrap().to_str().unwrap()
    ));

    prompt.push_str("</meta>");

    Ok(prompt)
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
        config.provider = p;
    }

    config::set_config(config);

    if args.list {
        tui::tui(project_root.join(TODO_DIR))?;
        return Ok(());
    }

    dewey_lib::config::setup()?;

    let api = provider_arg_to_enum(config::get_config().provider);

    let conversation = vec![wire::types::Message {
        message_type: wire::types::MessageType::User,
        content: args.user_message.unwrap(),
        api: api.clone(),
        system_prompt: get_system_prompt()?,
        tool_calls: None,
        tool_call_id: None,
        name: None,
    }];

    let response = wire::prompt_with_tools(
        api,
        &get_system_prompt()?,
        conversation,
        crate::tools::get_tools(),
    )
    .await?;

    println!(
        "FINAL RESPONSE: {}",
        response.iter().last().unwrap().content
    );

    Ok(())
}
