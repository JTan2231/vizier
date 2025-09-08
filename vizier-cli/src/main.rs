use std::env;
use std::fs;
use std::process::Command;

use clap::Parser;
use colored::*;
use tempfile::{Builder, TempPath};

use vizier_core::{
    auditor,
    auditor::{Auditor, CommitMessageBuilder, CommitMessageType},
    config, tools, vcs,
};

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
}

fn print_usage() {
    eprintln!(
        r#"{} - A CLI for LLM project management

{}
    {} [OPTIONS] [MESSAGE]

{}
    {}    Optional free-form message to the assistant

{}
    {}, {} <REF|RANGE>     "Save" tracked changes since REF/RANGE with AI commit message and update TODOs/snapshot
    {}, {}          Equivalent to `-s HEAD`
    {}, {} <MSG>   Developer note to append to the commit message (mutually exclusive with `-M`)
    {}, {}  Open editor for commit message (mutually exclusive with `-m`)
    {}, {} <NAME>      Set LLM provider (openai, anthropic, etc.)
    {}, {}         Force the agent to perform an action
    {}, {}                 Print help
    {}, {}              Print version

{}
    {} "add a TODO to implement auth"
    {} --save HEAD~3..HEAD
    {} --save-latest
    {} --save-latest -m "my commit message"
    {} --provider anthropic "what's my next task?"
"#,
        "vizier".bright_cyan().bold(),
        "USAGE:".bright_yellow().bold(),
        "vizier".bright_green(),
        "ARGS:".bright_yellow().bold(),
        "[MESSAGE]".bright_blue(),
        "OPTIONS:".bright_yellow().bold(),
        "-s".bright_green(),
        "--save".bright_green(),
        "-S".bright_green(),
        "--save-latest".bright_green(),
        "-m".bright_green(),
        "--commit-message".bright_green(),
        "-M".bright_green(),
        "--commit-message-editor".bright_green(),
        "-p".bright_green(),
        "--provider".bright_green(),
        "-f".bright_green(),
        "--force-action".bright_green(),
        "-h".bright_green(),
        "--help".bright_green(),
        "-V".bright_green(),
        "--version".bright_green(),
        "EXAMPLES:".bright_yellow().bold(),
        "vizier".bright_green(),
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

fn print_token_usage() {
    let usage = Auditor::get_total_usage();
    eprintln!("{}", "Token Usage:".yellow());
    eprintln!("- {} {}", "Prompt Tokens:".green(), usage.input_tokens);
    eprintln!("- {} {}", "Completion Tokens:".green(), usage.output_tokens);
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

async fn save(
    diff: String,
    // NOTE: These two should never be Some(...) && true
    user_message: Option<String>,
    use_message_editor: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let response = Auditor::llm_request_with_tools(
        crate::config::get_system_prompt()?,
        format!("Update the snapshot and existing TODOs as needed",),
        tools::get_tools(),
    )
    .await?;

    let conversation_hash = auditor::Auditor::commit_audit().await?;

    eprintln!("{} {}", "Assistant:".blue(), response.content);
    print_token_usage();

    let mut message_builder = CommitMessageBuilder::new(
        Auditor::llm_request(COMMIT_PROMPT.to_string(), diff)
            .await?
            .content,
    );

    message_builder
        .set_header(CommitMessageType::CodeChange)
        .with_conversation_hash(conversation_hash.clone());

    if let Some(message) = user_message {
        message_builder.with_author_note(message);
    }

    if use_message_editor {
        if let Ok(edited_message) = get_editor_message() {
            message_builder.with_author_note(edited_message);
        }
    }

    let commit_message = message_builder.build();
    vcs::add_and_commit(None, &commit_message, false)?;
    eprintln!("Changes committed with message: {}", commit_message);

    Ok(())
}

enum Shell {
    Bash,
    Zsh,
    Fish,
    Other(String),
}

impl Shell {
    fn from_path(shell_path: &str) -> Self {
        let shell_name = std::path::PathBuf::from(shell_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_lowercase();

        match shell_name.as_str() {
            "bash" => Shell::Bash,
            "zsh" => Shell::Zsh,
            "fish" => Shell::Fish,
            other => Shell::Other(other.to_string()),
        }
    }

    fn get_rc_source_command(&self) -> String {
        match self {
            Shell::Bash => ". ~/.bashrc".to_string(),
            Shell::Zsh => ". ~/.zshrc".to_string(),
            Shell::Fish => "source ~/.config/fish/config.fish".to_string(),
            Shell::Other(_) => "".to_string(),
        }
    }

    fn get_interactive_args(&self) -> Vec<String> {
        match self {
            Shell::Fish => vec!["-C".to_string()],
            _ => vec!["-i".to_string(), "-c".to_string()],
        }
    }
}

fn get_editor_message() -> Result<String, Box<dyn std::error::Error>> {
    let temp_file = Builder::new()
        .prefix("tllm_input")
        .suffix(".md")
        .tempfile()?;

    let temp_path: TempPath = temp_file.into_temp_path();

    match std::fs::write(temp_path.to_path_buf(), "") {
        Ok(_) => {}
        Err(e) => {
            println!("Error writing to temp file");
            return Err(Box::new(e));
        }
    };

    let shell_path = env::var("SHELL").unwrap_or_else(|_| "bash".to_string());
    let editor = env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
    let shell = Shell::from_path(&shell_path);

    let command = format!("{} {}", editor, temp_path.to_str().unwrap());
    let rc_source = shell.get_rc_source_command();
    let full_command = if rc_source.is_empty() {
        command
    } else {
        format!("{} && {}", rc_source, command)
    };

    let status = Command::new(shell_path)
        .args(shell.get_interactive_args())
        .arg("-c")
        .arg(&full_command)
        .status()?;

    if !status.success() {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Editor command failed",
        )));
    }

    let user_message = match fs::read_to_string(&temp_path) {
        Ok(contents) => {
            if contents.is_empty() {
                return Ok(String::new());
            }

            contents
        }
        Err(e) => {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Error reading file: {}", e),
            )));
        }
    };

    Ok(user_message)
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

    let no_primary_action = args.user_message.is_none() && args.save.is_none() && !args.save_latest;

    let invalid_commit_msg_flags = args.commit_message.is_some() && args.commit_message_editor;

    if no_primary_action || invalid_commit_msg_flags {
        print_usage();
        return Ok(());
    }

    let _auditor_cleanup = auditor::AuditorCleanup {
        debug: args.debug,
        print_json: args.json,
    };

    async fn run_save(
        commit_ref: &str,
        exclude: &[&str],
        commit_message: Option<String>,
        use_editor: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match vcs::get_diff(".", Some(commit_ref), Some(exclude)) {
            Ok(diff) => match save(diff, commit_message, use_editor).await {
                Ok(_) => Ok(()),
                Err(e) => {
                    eprintln!("Error running --save: {e}");
                    Err(Box::<dyn std::error::Error>::from(e))
                }
            },
            Err(e) => {
                eprintln!("Error generating diff for {commit_ref}: {e}");
                Err(Box::<dyn std::error::Error>::from(e))
            }
        }
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

    let system_prompt = match crate::config::get_system_prompt() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error loading system prompt: {e}");
            return Err(Box::<dyn std::error::Error>::from(e));
        }
    };

    let user_msg = args.user_message.expect("guarded above");

    let response =
        match Auditor::llm_request_with_tools(system_prompt, user_msg, tools::get_tools()).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error during LLM request: {e}");
                return Err(Box::<dyn std::error::Error>::from(e));
            }
        };

    if let Err(e) = auditor::Auditor::commit_audit().await {
        eprintln!("Error committing audit: {e}");
        return Err(Box::<dyn std::error::Error>::from(e));
    }

    eprintln!("{} {}", "Assistant:".blue(), response.content);
    print_token_usage();

    Ok(())
}
