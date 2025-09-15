use std::env;
use std::fs;
use std::process::Command;

use colored::*;
use tempfile::{Builder, TempPath};

use vizier_core::{
    auditor,
    auditor::{Auditor, CommitMessageBuilder, CommitMessageType},
    file_tracking, tools, vcs,
};

pub fn print_usage() {
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

pub fn provider_arg_to_enum(provider: String) -> wire::api::API {
    match provider.as_str() {
        "anthropic" => wire::api::API::Anthropic(wire::api::AnthropicModel::Claude35SonnetNew),
        "openai" => wire::api::API::OpenAI(wire::api::OpenAIModel::GPT4o),
        _ => panic!("Unrecognized LLM provider: {}", provider),
    }
}

pub fn print_token_usage() {
    let usage = Auditor::get_total_usage();
    eprintln!("{}", "Token Usage:".yellow());
    eprintln!("- {} {}", "Prompt Tokens:".green(), usage.input_tokens);
    eprintln!("- {} {}", "Completion Tokens:".green(), usage.output_tokens);
}

pub async fn run_save(
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

async fn save(
    diff: String,
    // NOTE: These two should never be Some(...) && true
    user_message: Option<String>,
    use_message_editor: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let provided_note = if let Some(message) = user_message {
        Some(message)
    } else if use_message_editor {
        if let Ok(edited) = get_editor_message() {
            Some(edited)
        } else {
            None
        }
    } else {
        None
    };

    let mut save_instruction =
        "<instruction>Update the snapshot and existing TODOs as needed</instruction>".to_string();

    if let Some(note) = &provided_note {
        save_instruction = format!(
            "{}<change_author_note>{}</change_author_note>",
            save_instruction, note
        );
    }

    let response = Auditor::llm_request_with_tools(
        crate::config::get_system_prompt()?,
        save_instruction,
        tools::get_tools(),
    )
    .await?;

    let conversation_hash = auditor::Auditor::commit_audit().await?;

    eprintln!("{} {}", "Assistant:".blue(), response.content);
    print_token_usage();

    let mut message_builder = CommitMessageBuilder::new(
        Auditor::llm_request(vizier_core::COMMIT_PROMPT.to_string(), diff)
            .await?
            .content,
    );

    message_builder
        .set_header(CommitMessageType::CodeChange)
        .with_conversation_hash(conversation_hash.clone());

    if let Some(note) = provided_note {
        message_builder.with_author_note(note);
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

/// NOTE: Filters items in the .vizier directory by whether they're markdown files
pub async fn clean(todo_list: String) -> Result<(), Box<dyn std::error::Error>> {
    let todo_dir = tools::get_todo_dir();
    let targets = match todo_list.as_str() {
        "*" => std::fs::read_dir(&todo_dir)?
            .filter_map(|entry| {
                entry
                    .ok()
                    .and_then(|e| e.file_name().to_str().map(|s| s.to_string()))
                    .filter(|name| name.ends_with(".md"))
                    .map(|p| format!("{}{}", todo_dir, p))
            })
            .collect::<Vec<_>>(),
        _ => {
            let filenames: std::collections::HashSet<_> =
                todo_list.split(',').map(|s| s.trim().to_string()).collect();

            let path_filenames: std::collections::HashSet<_> = std::fs::read_dir(&todo_dir)?
                .filter_map(|entry| entry.ok())
                .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
                .collect();

            filenames
                .intersection(&path_filenames)
                .filter(|name| name.ends_with(".md"))
                .map(|p| format!("{}{}", todo_dir, p))
                .collect()
        }
    };

    let mut revised = 0;
    let mut removed = 0;

    for target in targets.iter() {
        eprintln!("Cleaning {}...", target.blue());
        let content = std::fs::read_to_string(target)?;
        let response = Auditor::llm_request(
            format!(
                "{}{}<snapshot>{}</snapshot>",
                vizier_core::REVISE_TODO_PROMPT,
                vizier_core::SYSTEM_PROMPT_BASE.replace("mainInstruction", "SYSTEM_PROMPT_BASE"),
                tools::read_snapshot()
            ),
            content.clone(),
        )
        .await?
        .content;

        let revised_content = match response.as_str() {
            "null" => Some(content.clone()),
            "delete" => None,
            _ => Some(response.clone()),
        };

        match revised_content {
            Some(rc) => {
                if response != "null" {
                    eprintln!("{} {}...", "Revising".yellow(), target.blue());

                    file_tracking::FileTracker::write(target, &rc)?;
                    revised += 1;
                }
            }
            None => {
                eprintln!("{} {}...", "Removing".red(), target.blue());
                file_tracking::FileTracker::delete(target)?;
                removed += 1
            }
        };
    }

    eprintln!("{} {} TODO items", "Revised".yellow(), revised);
    eprintln!("{} {} TODO items", "Removed".red(), removed);

    let _ = Auditor::commit_audit().await?;

    Ok(())
}

pub async fn inline_command(user_message: String) -> Result<(), Box<dyn std::error::Error>> {
    let system_prompt = match crate::config::get_system_prompt() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error loading system prompt: {e}");
            return Err(Box::<dyn std::error::Error>::from(e));
        }
    };

    let response = match Auditor::llm_request_with_tools(
        system_prompt,
        user_message,
        tools::get_tools(),
    )
    .await
    {
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
