use std::error::Error;
use std::time::Duration;

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, MouseEventKind},
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

use crate::{
    auditor, codex,
    config::{self, BackendKind, SystemPrompt},
};

fn get_spinner_char(index: usize) -> String {
    const SPINNER_CHARS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    SPINNER_CHARS[index % SPINNER_CHARS.len()].to_string()
}

// TODO: Not sure I like having this duplicate enum with `editor.rs`
//
// TODO: with the auditor refactor in the chat, these both need to be rethough
//       really they should just be removed
pub enum ExitReason {
    Quit(Vec<wire::types::Message>),
    Restart(Vec<wire::types::Message>),
}

// TODO: This interacts directly over the wire and doesn't go through the auditor
//       Though coming to this later, the TUI parts probably need their own auditor
//       Initially separate, but merged later
pub struct Chat {
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
    receiving_handle: Option<tokio::task::JoinHandle<wire::types::Message>>,
}

impl Chat {
    pub fn new(messages: Vec<wire::types::Message>) -> Chat {
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(32);

        let (input_tokens, output_tokens) =
            messages.iter().fold((0, 0), |(input_acc, output_acc), m| {
                (input_acc + m.input_tokens, output_acc + m.output_tokens)
            });

        Chat {
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
            let tx_clone = self.tx.clone();
            let system_prompt = if config::get_config().backend == BackendKind::Codex {
                codex::build_prompt_for_codex(&self.input)?
            } else {
                config::get_system_prompt_with_meta(Some(SystemPrompt::Chat))?
            };
            let tools = crate::tools::active_tooling();
            let input = self.input.clone();
            self.receiving_handle = Some(tokio::spawn(async move {
                auditor::Auditor::llm_request_with_tools_no_display(
                    Some(SystemPrompt::Chat),
                    system_prompt,
                    input,
                    tools,
                    auditor::RequestStream::Status(tx_clone),
                    None,
                    None,
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

            let mut messages: Vec<Line> = auditor::Auditor::get_messages()
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
                        wire::types::MessageType::User | wire::types::MessageType::Assistant => {
                            m.content.clone()
                        }
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
                messages[std::cmp::min(app.scroll as usize, messages.len())
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
            auditor::Auditor::add_message(
                config::get_config()
                    .provider
                    .new_message(status)
                    .as_assistant()
                    .build(),
            );
        }

        if let Some(handle) = &mut app.receiving_handle {
            if handle.is_finished() {
                let last_message = handle.await?;

                app.input_tokens += last_message.input_tokens;
                app.output_tokens += last_message.output_tokens;

                app.receiving_handle = None;
            }
        }

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => match key.code {
                    KeyCode::Char('q') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                        return Ok(ExitReason::Quit(vec![]));
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
                },
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollDown => {
                        app.scroll = std::cmp::min(
                            app.scroll + (app.chat_height / 2),
                            app.line_count.saturating_sub(app.chat_height) + 1,
                        );
                    }
                    MouseEventKind::ScrollUp => {
                        app.scroll = app.scroll.saturating_sub(app.chat_height / 2);
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
}
