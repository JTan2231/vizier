use crossterm::event::{self, Event, KeyCode};
use ratatui::{
    Terminal,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use std::time::Duration;

fn get_spinner_char(index: usize) -> String {
    const SPINNER_CHARS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    SPINNER_CHARS[index % SPINNER_CHARS.len()].to_string()
}

pub struct Chat {
    api: wire::types::API,
    messages: Vec<wire::types::Message>,
    input: String,
    scroll: u16,
    spinner_increment: usize,

    // These two are both cumulative
    input_tokens: usize,
    output_tokens: usize,

    // UI updates while the model is responding
    tx: tokio::sync::mpsc::Sender<String>,
    rx: tokio::sync::mpsc::Receiver<String>,

    // NOTE: This should _always_ be None when we are not waiting on the model, and should have a
    //       value otherwise
    receiving_handle: Option<tokio::task::JoinHandle<Vec<wire::types::Message>>>,
}

impl Chat {
    pub fn new() -> Chat {
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(32);

        Chat {
            api: wire::types::API::OpenAI(wire::types::OpenAIModel::GPT4o),
            messages: vec![],
            input: String::new(),
            scroll: 0,
            spinner_increment: 0,
            input_tokens: 0,
            output_tokens: 0,
            tx,
            rx,
            receiving_handle: None,
        }
    }

    async fn send_message(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if !self.input.is_empty() {
            self.messages.extend(vec![wire::types::Message {
                system_prompt: String::new(),
                api: self.api.clone(),
                message_type: wire::types::MessageType::User,
                content: self.input.clone(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                input_tokens: 0,
                output_tokens: 0,
            }]);

            let tx_clone = self.tx.clone();
            let api_clone = self.api.clone();
            let message_history = self.messages.clone();
            self.receiving_handle = Some(tokio::spawn(async move {
                wire::prompt_with_tools_and_status(
                    tx_clone,
                    api_clone,
                    prompts::SYSTEM_PROMPT_BASE,
                    message_history,
                    prompts::tools::get_tools(),
                )
                .await
                .unwrap()
            }));

            self.input.clear();
            self.scroll = 0;
        }

        Ok(())
    }
}

pub async fn run_chat<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    mut app: Chat,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(3)].as_ref())
                .split(f.area());

            let mut messages: Vec<ListItem> = app
                .messages
                .iter()
                .filter(|m| {
                    m.message_type != wire::types::MessageType::FunctionCallOutput
                        && m.message_type != wire::types::MessageType::System
                })
                .map(|m| {
                    let prefix = format!(
                        "{}: ",
                        match m.message_type {
                            wire::types::MessageType::User => "You",
                            wire::types::MessageType::Assistant => "Assistant",
                            wire::types::MessageType::FunctionCall => "Assistant Tool Call",
                            _ => "",
                        }
                    );

                    let text = match m.message_type {
                        wire::types::MessageType::User | wire::types::MessageType::Assistant => {
                            &m.content
                        }
                        wire::types::MessageType::FunctionCall => m.name.as_ref().unwrap(),
                        _ => "???",
                    };

                    let lines: Vec<Line> = text
                        .lines()
                        .enumerate()
                        .map(|(i, line)| {
                            if i == 0 {
                                Line::from(vec![
                                    Span::styled(prefix.clone(), Style::default().fg(Color::Cyan)),
                                    Span::raw(line),
                                ])
                            } else {
                                Line::from(Span::raw(line))
                            }
                        })
                        .collect();

                    ListItem::new(lines)
                })
                .collect();

            // display a little spinner if we're waiting on the model to complete
            if let Some(_) = app.receiving_handle {
                messages.extend(vec![ListItem::new(vec![Line::from(get_spinner_char(
                    app.spinner_increment,
                ))])]);

                app.spinner_increment += 1;
            }

            let messages_list = List::new(messages)
                .block(Block::default().borders(Borders::ALL).title(format!(
                    "Chat (Total tokens: {}, prompt tokens: {}, completion tokens: {})",
                    app.input_tokens + app.output_tokens,
                    app.input_tokens,
                    app.output_tokens,
                )))
                .style(Style::default().fg(Color::White));

            f.render_widget(messages_list, chunks[0]);

            let input = Paragraph::new(app.input.as_str())
                .style(Style::default().fg(Color::Yellow))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Input (ctrl + q to quit)"),
                );
            f.render_widget(input, chunks[1]);
        })?;

        // TODO: This should probably be used for streaming later on, but for now it's just for
        //       tool updates
        if let Ok(status) = app.rx.try_recv() {
            app.messages.extend(vec![wire::types::Message {
                system_prompt: String::new(),
                api: app.messages.iter().last().unwrap().api.clone(),
                message_type: wire::types::MessageType::Assistant,
                content: status,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                input_tokens: 0,
                output_tokens: 0,
            }]);
        }

        if let Some(handle) = &mut app.receiving_handle {
            if handle.is_finished() {
                let new_messages = handle.await?;
                let last_message = new_messages.last().unwrap().clone();

                app.input_tokens += last_message.input_tokens;
                app.output_tokens += last_message.output_tokens;

                app.messages.extend(vec![last_message]);
                app.receiving_handle = None;
            }
        }

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                        return Ok(());
                    }
                    KeyCode::Char(c) => {
                        app.input.push(c);
                    }
                    KeyCode::Backspace => {
                        app.input.pop();
                    }
                    KeyCode::Enter => {
                        if key.modifiers.contains(event::KeyModifiers::SHIFT) {
                            app.input.push('\n');
                        } else {
                            app.send_message().await?;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
