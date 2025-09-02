use chrono::Utc;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tokio::sync::mpsc::channel;

lazy_static! {
    static ref AUDITOR: Mutex<Auditor> = Mutex::new(Auditor::new());
}

pub struct TokenUsage {
    pub input_tokens: usize,
    pub output_tokens: usize,
}

// TODO: We should probably include timestamps somewhere
// TODO: Should this be in this crate?

pub fn get_audit_dir() -> std::path::PathBuf {
    let todo_dir = std::path::PathBuf::from(crate::get_todo_dir());
    todo_dir.join("audit")
}

// dump the collected messages
pub struct AuditorCleanup;

impl Drop for AuditorCleanup {
    fn drop(&mut self) {
        if let Ok(auditor) = AUDITOR.lock() {
            match std::fs::create_dir_all(get_audit_dir()) {
                Ok(_) => {}
                Err(e) => {
                    eprintln!(
                        "Error creating audit directory {}: {}",
                        get_audit_dir().to_string_lossy(),
                        e
                    );

                    return;
                }
            };

            if auditor.messages.len() > 0 {
                let output_path =
                    get_audit_dir().join(format!("{}.json", auditor.session_start.clone()));
                match std::fs::write(
                    output_path.clone(),
                    serde_json::to_string_pretty(&auditor.messages).unwrap(),
                ) {
                    Ok(_) => eprintln!("Session saved to {}", auditor.session_start),
                    Err(e) => eprintln!(
                        "Error writing session file {}: {}",
                        output_path.to_string_lossy(),
                        e
                    ),
                };
            }
        }
    }
}

/// _All_ LLM interactions need run through the auditor
/// This should hold every LLM interaction from the current session, in chronological order
#[derive(Debug, Serialize, Deserialize)]
pub struct Auditor {
    messages: Vec<wire::types::Message>,
    session_start: String,
}

impl Auditor {
    pub fn new() -> Self {
        let now = Utc::now();

        Auditor {
            messages: Vec::new(),
            session_start: now.to_string(),
        }
    }

    fn add_message(message: wire::types::Message) {
        AUDITOR.lock().unwrap().messages.push(message);
    }

    fn replace_messages(messages: &Vec<wire::types::Message>) {
        AUDITOR.lock().unwrap().messages = messages.clone();
    }

    pub fn get_total_usage() -> TokenUsage {
        let mut usage = TokenUsage {
            input_tokens: 0,
            output_tokens: 0,
        };

        for message in AUDITOR.lock().unwrap().messages.iter() {
            usage.input_tokens += message.input_tokens;
            usage.output_tokens += message.output_tokens;
        }

        usage
    }

    pub async fn llm_request(
        system_prompt: String,
        user_message: String,
    ) -> Result<wire::types::Message, Box<dyn std::error::Error>> {
        let api = crate::config::get_config().provider;

        Self::add_message(wire::types::Message {
            message_type: wire::types::MessageType::User,
            content: user_message,
            api: api.clone(),
            system_prompt: system_prompt.clone(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            input_tokens: 0,
            output_tokens: 0,
        });

        let messages = AUDITOR.lock().unwrap().messages.clone();

        let response = tui::call_with_status(async move |tx| {
            tx.send(tui::Status::Working("Thinking...".into())).await?;

            let (request_tx, mut request_rx) = channel(10);

            tokio::spawn(async move {
                while let Some(msg) = request_rx.recv().await {
                    tx.send(tui::Status::Working(msg)).await.unwrap();
                }
            });

            let output = wire::prompt_with_tools_and_status(
                request_tx,
                crate::config::get_config().provider,
                &system_prompt,
                messages,
                vec![],
            )
            .await?;

            eprintln!();

            Ok(output)
        })
        .await
        .unwrap();

        Self::replace_messages(&response);

        Ok(response.last().unwrap().clone())
    }

    pub async fn llm_request_with_tools(
        system_prompt: String,
        user_message: String,
        tools: Vec<wire::types::Tool>,
    ) -> Result<wire::types::Message, Box<dyn std::error::Error>> {
        let api = crate::config::get_config().provider;

        Self::add_message(wire::types::Message {
            message_type: wire::types::MessageType::User,
            content: user_message,
            api: api.clone(),
            system_prompt: system_prompt.clone(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            input_tokens: 0,
            output_tokens: 0,
        });

        let messages = AUDITOR.lock().unwrap().messages.clone();

        let response = tui::call_with_status(async move |tx| {
            tx.send(tui::Status::Working("Thinking...".into())).await?;

            let (request_tx, mut request_rx) = channel(10);

            tokio::spawn(async move {
                while let Some(msg) = request_rx.recv().await {
                    tx.send(tui::Status::Working(msg)).await.unwrap();
                }
            });

            // TODO: The number of clones here is outrageous

            let output = wire::prompt_with_tools_and_status(
                request_tx.clone(),
                crate::config::get_config().provider,
                &system_prompt,
                messages.clone(),
                tools.clone(),
            )
            .await?;

            eprintln!();

            Ok(output)
        })
        .await
        .unwrap();

        Self::replace_messages(&response);

        if prompts::file_tracking::FileTracker::has_pending_changes() {
            if let Ok(output) = std::process::Command::new("git")
                .args(&["diff", &crate::get_todo_dir()])
                .output()
            {
                if let Ok(diff) = String::from_utf8(output.stdout) {
                    let commit_message = Self::llm_request(
                    "Given a diff on a directory of TODO items, return a commit message for these changes."
                        .to_string(),
                    if diff.len() == 0 { "init".to_string() } else { diff },
                )
                .await?
                .content;

                    prompts::file_tracking::FileTracker::commit_changes(&commit_message)?;
                }
            }
        }

        Ok(response.last().unwrap().clone())
    }
}
