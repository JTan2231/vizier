use lazy_static::lazy_static;
use std::sync::RwLock;
use tokio::sync::mpsc::channel;

lazy_static! {
    static ref CONFIG: RwLock<Config> = RwLock::new(Config::default());
}

#[derive(Clone)]
pub struct Config {
    pub provider: wire::types::API,
    pub force_action: bool,
}

impl Config {
    pub fn default() -> Self {
        Self {
            provider: wire::types::API::OpenAI(wire::types::OpenAIModel::GPT5),
            force_action: false,
        }
    }
}

pub fn set_config(new_config: Config) {
    *CONFIG.write().unwrap() = new_config;
}

pub fn get_config() -> Config {
    CONFIG.read().unwrap().clone()
}

pub fn get_system_prompt() -> Result<String, Box<dyn std::error::Error>> {
    let mut prompt = prompts::SYSTEM_PROMPT_BASE.to_string();

    prompt.push_str("<meta>");

    let file_tree = prompts::tree::build_tree()?;

    prompt.push_str(&format!(
        "<fileTree>{}</fileTree>",
        prompts::tree::tree_to_string(&file_tree, "")
    ));

    prompt.push_str(&format!("<todos>{}</todos>", prompts::tools::list_todos()));

    prompt.push_str(&format!(
        "<currentWorkingDirectory>{}</currentWorkingDirectory>",
        std::env::current_dir().unwrap().to_str().unwrap()
    ));

    prompt.push_str("</meta>");

    Ok(prompt)
}

// Basic LLM request without tools
pub async fn llm_request(
    history: Vec<wire::types::Message>,
    system_prompt: String,
    user_message: String,
) -> Result<wire::types::Message, Box<dyn std::error::Error>> {
    let api = get_config().provider;

    let mut conversation = history;
    conversation.extend(vec![wire::types::Message {
        message_type: wire::types::MessageType::User,
        content: user_message,
        api: api.clone(),
        system_prompt: system_prompt.clone(),
        tool_calls: None,
        tool_call_id: None,
        name: None,
        input_tokens: 0,
        output_tokens: 0,
    }]);

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
            get_config().provider,
            &system_prompt,
            conversation,
            vec![],
        )
        .await?;

        println!();

        Ok(output.iter().last().unwrap().clone())
    })
    .await
    .unwrap();

    Ok(response)
}

pub async fn llm_request_with_tools(
    history: Vec<wire::types::Message>,
    system_prompt: String,
    user_message: String,
    tools: Vec<wire::types::Tool>,
) -> Result<wire::types::Message, Box<dyn std::error::Error>> {
    let api = get_config().provider;

    let mut conversation = history;
    conversation.extend(vec![wire::types::Message {
        message_type: wire::types::MessageType::User,
        content: user_message,
        api: api.clone(),
        system_prompt: system_prompt.clone(),
        tool_calls: None,
        tool_call_id: None,
        name: None,
        input_tokens: 0,
        output_tokens: 0,
    }]);

    let response = tui::call_with_status(async move |tx| {
        tx.send(tui::Status::Working("Thinking...".into())).await?;

        let (request_tx, mut request_rx) = channel(10);

        tokio::spawn(async move {
            while let Some(msg) = request_rx.recv().await {
                tx.send(tui::Status::Working(msg)).await.unwrap();
            }
        });

        // TODO: The number of clones here is outrageous

        let mut output = wire::prompt_with_tools_and_status(
            request_tx.clone(),
            get_config().provider,
            &system_prompt,
            conversation.clone(),
            tools.clone(),
        )
        .await?;

        while get_config().force_action
            && !output
                .iter()
                .any(|m| prompts::tools::is_action(&m.clone().name.unwrap_or(String::new())))
        {
            output.push(wire::types::Message {
                message_type: wire::types::MessageType::User,
                content: "SYSTEM: Perform an action--the user has the `force_action` flag set."
                    .to_string(),
                api: api.clone(),
                system_prompt: system_prompt.clone(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                input_tokens: 0,
                output_tokens: 0,
            });

            output = wire::prompt_with_tools_and_status(
                request_tx.clone(),
                get_config().provider,
                &system_prompt,
                conversation.clone(),
                tools.clone(),
            )
            .await?;
        }

        println!();

        Ok(output.iter().last().unwrap().clone())
    })
    .await
    .unwrap();

    if prompts::file_tracking::FileTracker::has_pending_changes() {
        if let Ok(output) = std::process::Command::new("git")
            .args(&["diff", &crate::get_todo_dir()])
            .output()
        {
            if let Ok(diff) = String::from_utf8(output.stdout) {
                let commit_message = llm_request(
                    vec![],
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

    Ok(response)
}
