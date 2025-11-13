use chrono::Utc;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tokio::sync::mpsc::channel;

use crate::{
    codex,
    config::{self, SystemPrompt},
    display, file_tracking, tools, vcs,
};

lazy_static! {
    static ref AUDITOR: Mutex<Auditor> = Mutex::new(Auditor::new());
}

#[derive(Clone, Debug)]
pub struct TokenUsage {
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub known: bool,
}

pub struct AuditorCleanup {
    pub debug: bool,
    pub print_json: bool,
}

impl Drop for AuditorCleanup {
    fn drop(&mut self) {
        if let Ok(auditor) = AUDITOR.lock() {
            // double negative, I know
            if auditor.messages.len() > 0 && !config::get_config().no_session {
                let output = serde_json::to_string_pretty(&auditor.messages).unwrap();
                if let Some(config_dir) = config::base_config_dir() {
                    let sessions_dir = config_dir.join("vizier").join("sessions");
                    if let Err(e) = std::fs::create_dir_all(&sessions_dir) {
                        display::warn(format!("Failed to create sessions directory: {}", e));
                        display::warn("Session left unsaved");

                        return;
                    }

                    match std::fs::write(
                        sessions_dir.join(format!("./{}.json", auditor.session_id)),
                        output.clone(),
                    ) {
                        Ok(_) => {
                            display::info(format!("Session saved to {}", auditor.session_start))
                        }
                        Err(e) => display::emit(
                            display::LogLevel::Error,
                            format!("Error writing session file {}: {}", "./debug.json", e),
                        ),
                    };
                }

                if self.print_json {
                    println!("{}", output);
                }
            }
        }
    }
}

pub fn find_project_root() -> std::io::Result<Option<std::path::PathBuf>> {
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

async fn prompt_wire_with_tools(
    client: &dyn wire::api::Prompt,
    tx: tokio::sync::mpsc::Sender<String>,
    system_prompt: &str,
    messages: Vec<wire::types::Message>,
    tools: Vec<wire::types::Tool>,
) -> Result<Vec<wire::types::Message>, Box<dyn std::error::Error>> {
    // TODO: Mock server??? why did we even implement it if not to use it
    #[cfg(feature = "mock_llm")]
    {
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
        let mut new_messages = messages.clone();
        new_messages.push(
            client
                .new_message("mock response".to_string())
                .as_assistant()
                .build(),
        );

        Ok(new_messages)
    }

    #[cfg(not(feature = "mock_llm"))]
    {
        client
            .prompt_with_tools_with_status(tx, &system_prompt, messages, tools)
            .await
    }
}

/// _All_ LLM interactions need run through the auditor
/// This should hold every LLM interaction from the current session, in chronological order
#[derive(Debug, Serialize, Deserialize)]
pub struct Auditor {
    messages: Vec<wire::types::Message>,
    session_start: String,
    session_id: String,
    #[serde(skip)]
    usage_unknown: bool,
}

impl Auditor {
    pub fn new() -> Self {
        let now = Utc::now();

        Auditor {
            messages: Vec::new(),
            session_start: now.to_string(),
            session_id: uuid::Uuid::new_v4().to_string(),
            usage_unknown: false,
        }
    }

    /// Clones the message history in the auditor
    pub fn get_messages() -> Vec<wire::types::Message> {
        AUDITOR.lock().unwrap().messages.clone()
    }

    pub fn add_message(message: wire::types::Message) {
        AUDITOR.lock().unwrap().messages.push(message);
    }

    pub fn replace_messages(messages: &Vec<wire::types::Message>) {
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
        let auditor = AUDITOR.lock().unwrap();
        let mut usage = TokenUsage {
            input_tokens: 0,
            output_tokens: 0,
            known: !auditor.usage_unknown,
        };

        for message in auditor.messages.iter() {
            usage.input_tokens += message.input_tokens;
            usage.output_tokens += message.output_tokens;
        }

        usage
    }

    fn mark_usage_unknown() {
        if let Ok(mut auditor) = AUDITOR.lock() {
            auditor.usage_unknown = true;
        }
    }

    /// Commit the conversation (if it exists, which it should), then the narrative diff (if it exists)
    /// Returns the commit hash for the conversation, or an empty string if there's nothing to
    /// commit
    ///
    /// Really, though, if this is called then there should _always_ be a resulting commit hash
    pub async fn commit_audit() -> Result<String, Box<dyn std::error::Error>> {
        Ok(if file_tracking::FileTracker::has_pending_changes() {
            let root = match find_project_root()? {
                Some(p) => p,
                None => std::path::PathBuf::from("."),
            };

            let root = root.to_str().unwrap();

            let mut diff_message = None;
            if let Ok(diff) = vcs::get_diff(root, Some(&tools::get_todo_dir()), None) {
                display::info("Writing commit message for TODO changes...");
                diff_message = Some(Self::llm_request(
                        "Given a diff on a directory of TODO items, return a commit message for these changes."
                            .to_string(),
                        if diff.len() == 0 { "init".to_string() } else { diff },
                    )
                    .await?
                    .content);
            }

            let currently_staged = vcs::snapshot_staged(root)?;
            if currently_staged.len() > 0 {
                vcs::unstage(Some(
                    currently_staged
                        .iter()
                        .filter(|s| !s.path.contains(".vizier"))
                        .map(|s| s.path.as_str())
                        .collect(),
                ))?;
            }

            // starting to think that this should always be the case
            let conversation_hash = if AUDITOR.lock().unwrap().messages.len() > 0 {
                let conversation = Self::conversation_to_string();

                // unstage staged changes -> commit conversation -> restore staged changes
                display::info("Committing conversation...");

                let mut commit_message = CommitMessageBuilder::new(conversation)
                    .set_header(CommitMessageType::Conversation)
                    .build();

                if crate::config::get_config().commit_confirmation {
                    if let Some(new_message) = crate::editor::run_editor(&commit_message).await? {
                        commit_message = new_message;
                    }
                }

                let hash = vcs::add_and_commit(None, &commit_message, true)?.to_string();
                display::info("Committed conversation");

                hash
            } else {
                String::new()
            };

            if let Some(commit_message) = diff_message {
                display::info("Committing TODO changes...");
                file_tracking::FileTracker::commit_changes(
                    &conversation_hash,
                    &CommitMessageBuilder::new(commit_message)
                        .set_header(CommitMessageType::NarrativeChange)
                        .with_conversation_hash(conversation_hash.clone())
                        .build(),
                )
                .await?;

                display::info("Committed TODO changes");
            }

            if currently_staged.len() > 0 {
                vcs::stage(Some(
                    currently_staged
                        .iter()
                        .filter(|s| !s.path.contains(".vizier"))
                        .map(|s| s.path.as_str())
                        .collect(),
                ))?;
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
        Self::add_message(
            crate::config::get_config()
                .provider
                .new_message(user_message)
                .as_user()
                .build(),
        );

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

            Ok(prompt_wire_with_tools(
                &*crate::config::get_config().provider,
                request_tx.clone(),
                &system_prompt,
                messages.clone(),
                vec![],
            )
            .await?)
        })
        .await
        .map_err(|e| e as Box<dyn std::error::Error>)?;

        Self::replace_messages(&response);

        Ok(response.last().unwrap().clone())
    }

    /// Basic LLM request with tool usage
    /// NOTE: Returns the _entire_ conversation, up to date with the LLM's responses
    pub async fn llm_request_with_tools(
        prompt_variant: Option<SystemPrompt>,
        system_prompt: String,
        user_message: String,
        tools: Vec<wire::types::Tool>,
    ) -> Result<wire::types::Message, Box<dyn std::error::Error>> {
        let cfg = crate::config::get_config();
        let backend = cfg.backend;
        let fallback_backend = cfg.fallback_backend;
        let provider = cfg.provider.clone();
        let codex_opts = cfg.codex.clone();

        Self::add_message(provider.new_message(user_message).as_user().build());

        let messages = AUDITOR.lock().unwrap().messages.clone();

        match backend {
            config::BackendKind::Wire => {
                let response =
                    run_wire_with_status(system_prompt, messages.clone(), tools.clone()).await?;
                Self::replace_messages(&response);
                Ok(response.last().unwrap().clone())
            }
            config::BackendKind::Codex => {
                simulate_integration_changes()?;
                let repo_root =
                    find_project_root()?.unwrap_or_else(|| std::path::PathBuf::from("."));
                let messages_clone = messages.clone();
                let provider_clone = provider.clone();
                let opts_clone = codex_opts.clone();
                let prompt_clone = system_prompt.clone();

                let codex_run = display::call_with_status(async move |tx| {
                    let request = codex::CodexRequest {
                        prompt: prompt_clone.clone(),
                        repo_root: repo_root.clone(),
                        profile: opts_clone.profile.clone(),
                        bin: opts_clone.binary_path.clone(),
                        extra_args: opts_clone.extra_args.clone(),
                    };

                    let response =
                        codex::run_exec(request, Some(codex::ProgressHook::Display(tx.clone())))
                            .await
                            .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;

                    let mut updated = messages_clone.clone();
                    let mut assistant_message = provider_clone
                        .new_message(response.assistant_text.clone())
                        .as_assistant()
                        .build();
                    if let Some(usage) = response.usage {
                        assistant_message.input_tokens = usage.input_tokens;
                        assistant_message.output_tokens = usage.output_tokens;
                    } else {
                        Auditor::mark_usage_unknown();
                    }
                    updated.push(assistant_message);
                    Ok(updated)
                })
                .await;

                match codex_run {
                    Ok(response) => {
                        Self::replace_messages(&response);
                        Ok(response.last().unwrap().clone())
                    }
                    Err(err) => {
                        if fallback_backend == Some(config::BackendKind::Wire) {
                            let codex_error = match err.downcast::<codex::CodexError>() {
                                Ok(e) => *e,
                                Err(other) => return Err(other as Box<dyn std::error::Error>),
                            };
                            display::warn(format!(
                                "Codex backend failed ({}); retrying with wire backend",
                                codex_error
                            ));
                            let fallback_prompt =
                                config::get_system_prompt_with_meta(prompt_variant)?;
                            let fallback_tools = if tools.is_empty() {
                                tools::get_tools()
                            } else {
                                tools.clone()
                            };
                            let response = run_wire_with_status(
                                fallback_prompt,
                                messages.clone(),
                                fallback_tools,
                            )
                            .await?;
                            Self::replace_messages(&response);
                            Ok(response.last().unwrap().clone())
                        } else {
                            Err(err as Box<dyn std::error::Error>)
                        }
                    }
                }
            }
        }
    }

    // TODO: Rectify this with the function above
    pub async fn llm_request_with_tools_no_display(
        prompt_variant: Option<SystemPrompt>,
        system_prompt: String,
        user_message: String,
        tools: Vec<wire::types::Tool>,
        request_tx: tokio::sync::mpsc::Sender<String>,
    ) -> Result<wire::types::Message, Box<dyn std::error::Error>> {
        let cfg = crate::config::get_config();
        let backend = cfg.backend;
        let fallback_backend = cfg.fallback_backend;
        let provider = cfg.provider.clone();
        let codex_opts = cfg.codex.clone();

        Self::add_message(provider.new_message(user_message).as_user().build());

        let messages = AUDITOR.lock().unwrap().messages.clone();

        match backend {
            config::BackendKind::Wire => {
                let response = prompt_wire_with_tools(
                    &*provider,
                    request_tx.clone(),
                    &system_prompt,
                    messages.clone(),
                    tools.clone(),
                )
                .await?;

                let last = response.last().unwrap().clone();
                Self::add_message(last.clone());
                Ok(last)
            }
            config::BackendKind::Codex => {
                simulate_integration_changes()?;
                let repo_root =
                    find_project_root()?.unwrap_or_else(|| std::path::PathBuf::from("."));
                let request = codex::CodexRequest {
                    prompt: system_prompt.clone(),
                    repo_root,
                    profile: codex_opts.profile.clone(),
                    bin: codex_opts.binary_path.clone(),
                    extra_args: codex_opts.extra_args.clone(),
                };

                match codex::run_exec(
                    request,
                    Some(codex::ProgressHook::Plain(request_tx.clone())),
                )
                .await
                {
                    Ok(response) => {
                        let mut assistant_message = provider
                            .new_message(response.assistant_text.clone())
                            .as_assistant()
                            .build();
                        if let Some(usage) = response.usage {
                            assistant_message.input_tokens = usage.input_tokens;
                            assistant_message.output_tokens = usage.output_tokens;
                        } else {
                            Auditor::mark_usage_unknown();
                        }
                        Self::add_message(assistant_message.clone());
                        Ok(assistant_message)
                    }
                    Err(err) => {
                        if fallback_backend == Some(config::BackendKind::Wire) {
                            display::warn(format!(
                                "Codex backend failed ({}); retrying with wire backend",
                                err
                            ));
                            let fallback_prompt =
                                config::get_system_prompt_with_meta(prompt_variant)?;
                            let fallback_tools = if tools.is_empty() {
                                tools::get_tools()
                            } else {
                                tools.clone()
                            };
                            let response = prompt_wire_with_tools(
                                &*provider,
                                request_tx.clone(),
                                &fallback_prompt,
                                messages.clone(),
                                fallback_tools,
                            )
                            .await?;
                            let last = response.last().unwrap().clone();
                            Self::add_message(last.clone());
                            Ok(last)
                        } else {
                            Err(Box::new(err))
                        }
                    }
                }
            }
        }
    }
}

async fn run_wire_with_status(
    system_prompt: String,
    messages: Vec<wire::types::Message>,
    tools: Vec<wire::types::Tool>,
) -> Result<Vec<wire::types::Message>, Box<dyn std::error::Error>> {
    display::call_with_status(async move |tx| {
        tx.send(display::Status::Working("Thinking...".into()))
            .await?;

        let (request_tx, mut request_rx) = channel(10);
        tokio::spawn(async move {
            while let Some(msg) = request_rx.recv().await {
                tx.send(display::Status::Working(msg)).await.unwrap();
            }
        });

        simulate_integration_changes()?;

        Ok(prompt_wire_with_tools(
            &*crate::config::get_config().provider,
            request_tx.clone(),
            &system_prompt,
            messages.clone(),
            tools.clone(),
        )
        .await?)
    })
    .await
    .map_err(|e| e as Box<dyn std::error::Error>)
}

#[cfg(feature = "integration_testing")]
fn simulate_integration_changes() -> Result<(), Box<dyn std::error::Error>> {
    let skip_code_change = std::env::var("VIZIER_IT_SKIP_CODE_CHANGE").is_ok();
    let skip_vizier_change = std::env::var("VIZIER_IT_SKIP_VIZIER_CHANGE").is_ok();

    if !skip_code_change {
        crate::file_tracking::FileTracker::write("a", "some change")?;
    }

    if !skip_vizier_change {
        crate::file_tracking::FileTracker::write(".vizier/.snapshot", "some snapshot change")?;
        crate::file_tracking::FileTracker::write(".vizier/todo.md", "some todo change")?;
    }

    Ok(())
}

#[cfg(not(feature = "integration_testing"))]
fn simulate_integration_changes() -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
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
