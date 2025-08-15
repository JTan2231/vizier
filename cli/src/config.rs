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

pub const SYSTEM_PROMPT_BASE: &str = r#"
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
- _Always_ update the project snapshot when the TODOs are changed
- _Always_ assume the user is speaking with the expectation of action on your part

Example:
BAD: "Investigate performance issues in search"
GOOD: "Replace recursive DFS in hnsw.rs:156 with iterative stack-based implementation using Vec<Node>"

Using these rules, convert TODOs from the codebase into actionable tasks.
</mainInstruction>
"#;

pub fn get_system_prompt() -> Result<String, Box<dyn std::error::Error>> {
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

// TODO: Why are we still managing conversation history here? These should represent the beginning
//       and end of a conversation

// Basic LLM request without tools
pub async fn llm_request(
    history: Vec<wire::types::Message>,
    system_prompt: String,
    user_message: String,
) -> Result<String, Box<dyn std::error::Error>> {
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

        Ok(output.iter().last().unwrap().content.clone())
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
) -> Result<String, Box<dyn std::error::Error>> {
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

        Ok(output.iter().last().unwrap().content.clone())
    })
    .await
    .unwrap();

    Ok(response)
}
