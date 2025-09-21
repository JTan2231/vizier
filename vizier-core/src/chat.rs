use std::error::Error;
use std::time::Duration;

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    layout::{Constraint, Direction, Layout},
    prelude::CrosstermBackend,
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph},
};

const CHAT_PROMPT: &str = r#"
<mainInstruction>
Your Role: Be a conversational editor who keeps the project’s story straight. As the user talks, you turn their words into:
1. **Live TODOs** — actionable steps that move the project forward.
2. **A Running Snapshot** — a clear picture of where the project stands right now.

### SNAPSHOT IN CHAT
- Think of it as a shared whiteboard, updated as we talk.
- Keep it minimal: just enough CODE STATE (what the software *does*) and NARRATIVE STATE (why it matters, where it’s going) so we never lose the thread.
- Update incrementally — small corrections, not wholesale rewrites.

### HOW TO INTERACT
- Stay responsive: listen, reflect, and edit as the conversation unfolds.
- Don’t wait until the end to deliver; weave TODOs and snapshot deltas into the dialogue.
- Use natural language — explain changes as if you’re narrating aloud, not writing a report.

### TODO STYLE
- Default to **Product Level**: user-visible behavior, UX affordances, acceptance criteria.
- You may anchor with pointers (files, commands) for orientation.
- Drop to implementation detail *only if* (a) user requests, (b) correctness/safety demands it, or (c) the snapshot already fixes the constraint.
- Keep TODOs tied to real tensions in behavior — no vague “investigate X.”

### CONVERSATIONAL PRINCIPLES
- Don’t just echo — interpret. Surface the underlying theme or problem.
- Every TODO should feel like a natural next beat in the story.
- Duplicate threads = noise; merge rather than fork.
- When the user sounds lost (“what’s the state again?”), pull context from the snapshot and remind them.

### VOICE
- Match the user’s tone. Be crisp, direct, and collaborative.
- Think like a pair-programmer: suggest, clarify, and refine without ceremony.
- The response itself should *be* the work (snapshot note + TODOs), not a plan to do it later.

### GOLDEN RULES
- A good TODO in chat feels like a prompt card everyone agrees on: clear enough to act, light enough to adapt.
- A good snapshot is a quick “state of play” that lets anyone rejoin the conversation without rereading the log.
</mainInstruction>
"#;

fn get_spinner_char(index: usize) -> String {
    const SPINNER_CHARS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    SPINNER_CHARS[index % SPINNER_CHARS.len()].to_string()
}

// TODO: Not sure I like having this duplicate enum with `editor.rs`
pub enum ExitReason {
    Quit(Vec<wire::types::Message>),
    Restart(Vec<wire::types::Message>),
}

// TODO: This interacts directly over the wire and doesn't go through the auditor
//       Though coming to this later, the TUI parts probably need their own auditor
//       Initially separate, but merged later
pub struct Chat {
    api: wire::api::API,
    messages: Vec<wire::types::Message>,
    input: String,
    spinner_increment: usize,

    scroll: u16,
    chat_height: u16,
    line_count: u16,

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
    pub fn new(messages: Vec<wire::types::Message>) -> Chat {
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(32);

        let (input_tokens, output_tokens) =
            messages.iter().fold((0, 0), |(input_acc, output_acc), m| {
                (input_acc + m.input_tokens, output_acc + m.output_tokens)
            });

        Chat {
            api: wire::api::API::OpenAI(wire::api::OpenAIModel::GPT5),
            messages,
            input: String::new(),
            spinner_increment: 0,

            scroll: 0,
            chat_height: 0,
            line_count: 0,

            input_tokens,
            output_tokens,

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
            let system_prompt = CHAT_PROMPT.to_string();
            let tools = crate::tools::get_tools();
            self.receiving_handle = Some(tokio::spawn(async move {
                wire::prompt_with_tools_and_status(
                    tx_clone,
                    api_clone,
                    &system_prompt,
                    message_history,
                    tools,
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

pub async fn chat_tui() -> std::result::Result<(), Box<dyn Error>> {
    let mut exit_reason = ExitReason::Restart(Vec::new());

    while let ExitReason::Restart(conversation) = &exit_reason {
        enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let app = Chat::new(conversation.clone());
        exit_reason = run_chat(&mut terminal, app).await?;

        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;

        terminal.show_cursor()?;
    }

    Ok(())
}

// TODO: Duplicate function from `editor.rs`
fn wrap_text_to_width(text: &str, max_width: u16) -> Vec<String> {
    let mut lines = Vec::new();

    for raw_line in text.split('\n') {
        let mut current = String::new();

        for word in raw_line.split_whitespace() {
            if current.len() + word.len() + 1 > max_width as usize {
                lines.push(current);
                current = String::new();
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }

        if !current.is_empty() {
            lines.push(current);
        } else {
            lines.push(String::new());
        }
    }

    lines
}

pub async fn run_chat<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    mut app: Chat,
) -> Result<ExitReason, Box<dyn std::error::Error>> {
    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(10)].as_ref())
                .split(f.area());

            app.chat_height = chunks[0].height;

            let mut messages: Vec<Line> =
                app.messages
                    .iter()
                    .filter(|m| {
                        m.message_type != wire::types::MessageType::FunctionCallOutput
                            && m.message_type != wire::types::MessageType::System
                    })
                    .flat_map(|m| {
                        let prefix = format!(
                            "{}: ",
                            match m.message_type {
                                wire::types::MessageType::User => "You",
                                wire::types::MessageType::Assistant => "Assistant",
                                wire::types::MessageType::FunctionCall => "Assistant Tool Call",
                                _ => "???",
                            }
                        );

                        let text = match m.message_type {
                            wire::types::MessageType::User
                            | wire::types::MessageType::Assistant => m.content.clone(),
                            wire::types::MessageType::FunctionCall => m
                                .name
                                .clone()
                                .unwrap_or("[unnamed function call]".to_string()),
                            _ => "???".to_string(),
                        };

                        let wrapped = wrap_text_to_width(&text, chunks[0].width.saturating_sub(4));

                        let lines: Vec<Line> = wrapped
                            .into_iter()
                            .enumerate()
                            .map(|(i, line)| {
                                if i == 0 {
                                    Line::from(vec![
                                        Span::styled(
                                            prefix.clone(),
                                            Style::default().fg(if prefix.contains("Assistant") {
                                                Color::Cyan
                                            } else {
                                                Color::Green
                                            }),
                                        ),
                                        Span::raw(line.to_string())
                                            .style(Style::default().fg(Color::Gray)),
                                    ])
                                } else {
                                    Line::from(Span::raw(line.to_string()))
                                        .style(Style::default().fg(Color::Gray))
                                }
                            })
                            .collect();

                        lines
                    })
                    .collect();

            app.line_count = messages.len() as u16;

            // display a little spinner if we're waiting on the model to complete
            if let Some(_) = app.receiving_handle {
                messages.extend(vec![
                    Line::from(get_spinner_char(app.spinner_increment))
                        .style(Style::default().fg(Color::Cyan)),
                ]);

                app.spinner_increment += 1;
            }

            let messages_list = Paragraph::new(
                messages[app.scroll as usize
                    ..std::cmp::min((app.scroll + app.chat_height) as usize, messages.len())]
                    .iter()
                    .cloned()
                    .collect::<Vec<Line>>(),
            )
            .block(Block::default().title(vec![
                        Span::from(format!(
                            "Chat ",
                        ))
                        .style(Style::default().fg(Color::White)
                        ),
                        Span::from(format!(
                            "(Total tokens: {}, prompt tokens: {}, completion tokens: {})",
                            app.input_tokens + app.output_tokens,
                            app.input_tokens,
                            app.output_tokens,
                        ))
                        .style(Style::default().fg(Color::Blue)
                        ),
                        ]))
            .style(Style::default().fg(Color::White));

            f.render_widget(messages_list, chunks[0]);

            let with_caret = format!("{}▏", app.input.clone());
            let input = Paragraph::new(Text::from(
                with_caret
                    .split('\n')
                    .map(|l| Line::from(l).style(Style::default().fg(Color::Gray)))
                    .collect::<Vec<Line>>(),
            ))
            .style(Style::default().fg(Color::White).bg(Color::Rgb(28, 32, 28)))
            .block(Block::default().title(vec![
                    Span::from("Input "),
                    Span::from("(ctrl + (q: quit, j: line break), enter: submit)")
                        .style(Style::default().fg(Color::Yellow)),
                ]));
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
                        return Ok(ExitReason::Quit(app.messages.clone()));
                    }
                    KeyCode::Char('j') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                        app.input.push('\n');
                    }
                    KeyCode::Char(c) => {
                        app.input.push(c);
                    }
                    KeyCode::Up => {
                        app.scroll = app.scroll.saturating_sub(1);
                    }
                    KeyCode::PageUp => {
                        app.scroll = app.scroll.saturating_sub(app.chat_height);
                    }
                    KeyCode::Down => {
                        app.scroll = std::cmp::min(
                            app.scroll + 1,
                            app.line_count.saturating_sub(app.chat_height) + 1,
                        );
                    }
                    KeyCode::PageDown => {
                        app.scroll = std::cmp::min(
                            app.scroll + app.chat_height,
                            app.line_count.saturating_sub(app.chat_height) + 1,
                        );
                    }
                    KeyCode::Backspace => {
                        app.input.pop();
                    }
                    KeyCode::Enter => {
                        app.send_message().await?;
                    }
                    _ => {}
                }
            }
        }
    }
}
