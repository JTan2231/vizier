use lazy_static::lazy_static;
use std::sync::RwLock;
use tokio::sync::mpsc::channel;

lazy_static! {
    static ref CONFIG: RwLock<Config> = RwLock::new(Config::default());
}

#[derive(Clone)]
pub struct Config {
    pub provider: wire::types::API,
}

impl Config {
    pub fn default() -> Self {
        Self {
            provider: wire::types::API::OpenAI(wire::types::OpenAIModel::GPT4o),
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

// TODO: Why are we still managing conversation history here? These should represent the beginning
//       and end of a conversation

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

        let output = wire::prompt_with_tools_and_status(
            request_tx,
            get_config().provider,
            &system_prompt,
            conversation,
            tools,
        )
        .await?;

        println!();

        Ok(output.iter().last().unwrap().clone())
    })
    .await
    .unwrap();

    Ok(response)
}
