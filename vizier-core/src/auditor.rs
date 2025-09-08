use chrono::Utc;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tokio::sync::mpsc::channel;

use crate::{display, file_tracking, tools, vcs};

lazy_static! {
    static ref AUDITOR: Mutex<Auditor> = Mutex::new(Auditor::new());
}

pub struct TokenUsage {
    pub input_tokens: usize,
    pub output_tokens: usize,
}

pub struct AuditorCleanup {
    pub debug: bool,
    pub print_json: bool,
}

impl Drop for AuditorCleanup {
    fn drop(&mut self) {
        if self.debug {
            if let Ok(auditor) = AUDITOR.lock() {
                if auditor.messages.len() > 0 {
                    let output = serde_json::to_string_pretty(&auditor.messages).unwrap();
                    match std::fs::write("./debug.json", output.clone()) {
                        Ok(_) => eprintln!("Session saved to {}", auditor.session_start),
                        Err(e) => eprintln!("Error writing session file {}: {}", "./debug.json", e),
                    };

                    if self.print_json {
                        println!("{}", output);
                    }
                }
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
    session_id: String,
}

impl Auditor {
    pub fn new() -> Self {
        let now = Utc::now();

        Auditor {
            messages: Vec::new(),
            session_start: now.to_string(),
            session_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    fn add_message(message: wire::types::Message) {
        AUDITOR.lock().unwrap().messages.push(message);
    }

    fn replace_messages(messages: &Vec<wire::types::Message>) {
        AUDITOR.lock().unwrap().messages = messages.clone();
    }

    pub fn conversation_to_string() -> String {
        let messages = AUDITOR.lock().unwrap().messages.clone();
        let mut conversation = String::new();

        for message in messages
            .iter()
            .filter(|m| m.message_type != wire::types::MessageType::FunctionCall)
        {
            conversation.push_str(&format!(
                "{}: {}\n\n###\n\n",
                message.message_type.to_string(),
                message.content
            ));
        }

        conversation
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

    /// Commit the conversation (if it exists, which it should), then the diff (if it exists)
    /// Returns the commit hash for the conversation, or an empty string if there's nothing to
    /// commit
    ///
    /// Really, though, if this is called then there should _always_ be a resulting commit hash
    pub async fn commit_audit() -> Result<String, Box<dyn std::error::Error>> {
        Ok(if file_tracking::FileTracker::has_pending_changes() {
            let mut diff_message = None;
            if let Ok(diff) = vcs::get_diff(".", Some(&tools::get_todo_dir()), None) {
                eprintln!("Writing commit message for TODO changes...");
                diff_message = Some(Self::llm_request(
                        "Given a diff on a directory of TODO items, return a commit message for these changes."
                            .to_string(),
                        if diff.len() == 0 { "init".to_string() } else { diff },
                    )
                    .await?
                    .content);
            }

            // starting to think that this should always be the case
            let conversation_hash = if AUDITOR.lock().unwrap().messages.len() > 0 {
                let conversation = Self::conversation_to_string();

                eprintln!("Committing conversation...");
                vcs::add_and_commit(
                    None,
                    &CommitMessageBuilder::new(conversation)
                        .set_header(CommitMessageType::Conversation)
                        .build(),
                    true,
                )?
                .to_string()
            } else {
                String::new()
            };

            if let Some(commit_message) = diff_message {
                eprintln!("Committing TODO changes...");
                file_tracking::FileTracker::commit_changes(
                    &conversation_hash,
                    &CommitMessageBuilder::new(commit_message)
                        .set_header(CommitMessageType::NarrativeChange)
                        .with_conversation_hash(conversation_hash.clone())
                        .build(),
                )?;
            }

            conversation_hash
        } else {
            String::new()
        })
    }

    /// Basic LLM request without tool usage
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

        let response = display::call_with_status(async move |tx| {
            tx.send(display::Status::Working("Thinking...".into()))
                .await?;

            let (request_tx, mut request_rx) = channel(10);

            tokio::spawn(async move {
                while let Some(msg) = request_rx.recv().await {
                    tx.send(display::Status::Working(msg)).await.unwrap();
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

    /// Basic LLM request with tool usage
    /// NOTE: Returns the _entire_ conversation, up to date with the LLM's responses
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

        let response = display::call_with_status(async move |tx| {
            tx.send(display::Status::Working("Thinking...".into()))
                .await?;

            let (request_tx, mut request_rx) = channel(10);

            tokio::spawn(async move {
                while let Some(msg) = request_rx.recv().await {
                    tx.send(display::Status::Working(msg)).await.unwrap();
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

        Ok(response.last().unwrap().clone())
    }
}

pub enum CommitMessageType {
    CodeChange,
    Conversation,
    NarrativeChange,
}

pub struct CommitMessageBuilder {
    header: String,
    session_id: String,
    // This should only be None for the conversation commits themselves
    conversation_hash: Option<String>,
    author_note: Option<String>,
    body: String,
}

impl CommitMessageBuilder {
    pub fn new(body: String) -> Self {
        Self {
            header: "VIZIER".to_string(),
            session_id: AUDITOR.lock().unwrap().session_id.clone(),
            conversation_hash: None,
            author_note: None,
            body,
        }
    }

    pub fn set_header(&mut self, message_type: CommitMessageType) -> &mut Self {
        match message_type {
            CommitMessageType::CodeChange => self.header = "VIZIER CODE CHANGE".to_string(),
            CommitMessageType::Conversation => self.header = "VIZIER CONVERSATION".to_string(),
            CommitMessageType::NarrativeChange => {
                self.header = "VIZIER NARRATIVE CHANGE".to_string()
            }
        };

        self
    }

    pub fn with_author_note(&mut self, note: String) -> &mut Self {
        self.author_note = Some(note);

        self
    }

    pub fn with_conversation_hash(&mut self, conversation_hash: String) -> &mut Self {
        self.conversation_hash = Some(conversation_hash);

        self
    }

    pub fn build(&self) -> String {
        let mut message = format!("{}\nSession ID: {}", self.header.clone(), self.session_id);

        if let Some(ch) = &self.conversation_hash {
            message = format!("{}\nConversation: {}", message, ch);
        }

        if let Some(an) = &self.author_note {
            message = format!("{}\nAuthor note: {}", message, an);
        }

        format!("{}\n\n{}", message, self.body)
    }
}
