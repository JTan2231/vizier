use crossterm::event::{self, Event, KeyCode};
use ratatui::{
    Terminal,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use std::time::Duration;

// TODO: System prompt?
pub async fn llm_request(
    api: wire::types::API,
    history: Vec<wire::types::Message>,
) -> Result<wire::types::Message, Box<dyn std::error::Error>> {
    let response = wire::prompt_with_tools(api, "", history, vec![]).await?;

    Ok(response.iter().last().unwrap().clone())
}

pub struct Chat {
    api: wire::types::API,
    messages: Vec<wire::types::Message>,
    input: String,
    scroll: u16,
    tx: tokio::sync::mpsc::Sender<String>,
    rx: tokio::sync::mpsc::Receiver<String>,
    receiving_handle: Option<tokio::task::JoinHandle<wire::types::Message>>,
}

impl Chat {
    pub fn new() -> Chat {
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(32);

        Chat {
            api: wire::types::API::OpenAI(wire::types::OpenAIModel::GPT4o),
            messages: vec![],
            input: String::new(),
            scroll: 0,
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
            }]);

            let tx_clone = self.tx.clone();
            let api_clone = self.api.clone();
            let message_history = self.messages.clone();
            self.receiving_handle = Some(tokio::spawn(async move {
                wire::prompt_stream(api_clone, "", &message_history, tx_clone)
                    .await
                    .unwrap()
            }));

            self.messages.extend(vec![wire::types::Message {
                system_prompt: String::new(),
                api: self.api.clone(),
                message_type: wire::types::MessageType::Assistant,
                content: String::new(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            }]);

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

            let messages: Vec<ListItem> = app
                .messages
                .iter()
                .map(|m| {
                    let content = vec![Line::from(vec![
                        Span::styled(
                            format!(
                                "{}: ",
                                match m.message_type {
                                    wire::types::MessageType::User => "You",
                                    wire::types::MessageType::Assistant => "Assistant",
                                    _ => "???",
                                }
                            ),
                            Style::default().fg(Color::Cyan),
                        ),
                        Span::raw(&m.content),
                    ])];
                    ListItem::new(content)
                })
                .collect();

            let messages_list = List::new(messages)
                .block(Block::default().borders(Borders::ALL).title("Messages"))
                .style(Style::default().fg(Color::White));

            f.render_widget(messages_list, chunks[0]);

            let input = Paragraph::new(app.input.as_str())
                .style(Style::default().fg(Color::Yellow))
                .block(Block::default().borders(Borders::ALL).title("Input"));
            f.render_widget(input, chunks[1]);
        })?;

        if let Ok(delta) = app.rx.try_recv() {
            app.messages
                .iter_mut()
                .last()
                .unwrap()
                .content
                .push_str(&delta);
        }

        if let Some(handle) = &app.receiving_handle {
            if handle.is_finished() {
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
