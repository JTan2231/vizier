use std::io::{self};
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};
use wire::types::*;

use crate::config;

// TODO: Duplicate function
fn get_spinner_char(index: usize) -> String {
    const SPINNER_CHARS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

    SPINNER_CHARS[index % SPINNER_CHARS.len()].to_string()
}

#[derive(PartialEq)]
enum ExitReason {
    Cancel,
    Save,
}

pub async fn run_editor(content: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    // terminal init
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(content);
    let tick_rate = Duration::from_millis(100);
    let mut last_tick = Instant::now();

    let exit_reason;

    // main loop
    loop {
        terminal.draw(|f| ui(f, &mut app))?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if let Some(reason) = app.handle_key(key).await? {
                    exit_reason = reason;

                    break;
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            app.on_tick().await?;
            last_tick = Instant::now();
        }
    }

    // restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    *crate::tools::SENDER.write().unwrap() = None;

    Ok(if exit_reason == ExitReason::Cancel {
        None
    } else {
        Some(app.content)
    })
}

struct App {
    // textarea (top-left)
    input: String,
    cursor: usize, // byte index in input

    // right panel (scrollable text)
    content: String,
    right_scroll: u16, // vertical scroll offset

    // bottom-left
    conversation_history: Vec<Message>,
    tool_effect_receiver: tokio::sync::mpsc::UnboundedReceiver<String>,

    response_receiving_handle: Option<tokio::task::JoinHandle<Vec<wire::types::Message>>>,
    response_status_tx: tokio::sync::mpsc::Sender<String>,
    response_status_rx: tokio::sync::mpsc::Receiver<String>,

    spinner_increment: usize,
}

impl App {
    fn new(content: &str) -> Self {
        let (tool_tx, tool_rx) = tokio::sync::mpsc::unbounded_channel();

        *crate::tools::SENDER.write().unwrap() = Some(tool_tx);

        let (response_tx, response_rx) = tokio::sync::mpsc::channel::<String>(32);
        Self {
            input: String::new(),
            cursor: 0,
            content: content.to_string(),
            right_scroll: 0,
            conversation_history: Vec::new(),
            tool_effect_receiver: tool_rx,
            response_receiving_handle: None,
            response_status_tx: response_tx,
            response_status_rx: response_rx,
            spinner_increment: 0,
        }
    }

    async fn on_tick(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        match self.tool_effect_receiver.try_recv() {
            Ok(s) => self.content = s,
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
            Err(_) => { /*???*/ }
        };

        if let Ok(status) = self.response_status_rx.try_recv() {
            self.conversation_history.extend(vec![wire::types::Message {
                system_prompt: String::new(),
                api: self.conversation_history.iter().last().unwrap().api.clone(),
                message_type: wire::types::MessageType::Assistant,
                content: status,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                input_tokens: 0,
                output_tokens: 0,
            }]);
        }

        if let Some(handle) = &mut self.response_receiving_handle {
            if handle.is_finished() {
                let new_messages = handle.await?;
                let last_message = new_messages.last().unwrap().clone();

                // self.input_tokens += last_message.input_tokens;
                // self.output_tokens += last_message.output_tokens;

                self.conversation_history.extend(vec![last_message]);
                self.response_receiving_handle = None;
            }
        }

        Ok(())
    }

    fn build_system_prompt(&self) -> String {
        format!(
            "{}{}<fileContents>{}</fileContents>",
            config::get_config().get_prompt(config::SystemPrompt::Editor),
            String::new(),
            self.content,
        )
    }

    /// Returns true if the app requests to quit.
    async fn handle_key(
        &mut self,
        key: KeyEvent,
    ) -> Result<Option<ExitReason>, Box<dyn std::error::Error>> {
        match (key.modifiers, key.code) {
            // Cancel edits
            (KeyModifiers::CONTROL, KeyCode::Char('q')) => return Ok(Some(ExitReason::Cancel)),

            // Save edits
            (KeyModifiers::CONTROL, KeyCode::Char('s')) => return Ok(Some(ExitReason::Save)),

            // Textarea editing
            (KeyModifiers::NONE, KeyCode::Char(c)) | (KeyModifiers::SHIFT, KeyCode::Char(c)) => {
                // insert at cursor (simple UTF-8 by byte; fine for ASCII. For full Unicode editing,
                // switch to a rope or track grapheme clusters.)
                self.input.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            (KeyModifiers::SHIFT, KeyCode::Enter) => {
                self.input.insert(self.cursor, '\n');
                self.cursor += 1;
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                let api = wire::api::API::OpenAI(wire::api::OpenAIModel::GPT5);
                self.conversation_history.push(Message {
                    message_type: MessageType::User,
                    content: self.input.clone(),
                    api: api.clone(),
                    system_prompt: self.build_system_prompt(),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    input_tokens: 0,
                    output_tokens: 0,
                });

                let tx_clone = self.response_status_tx.clone();
                let api_clone = api.clone();
                let message_history = self.conversation_history.clone();
                let system_prompt = self.build_system_prompt();
                let tools = crate::tools::get_editor_tools();
                self.response_receiving_handle = Some(tokio::spawn(async move {
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
                self.cursor = 0;
            }
            (KeyModifiers::NONE, KeyCode::Backspace) => {
                if self.cursor > 0 {
                    // remove the previous scalar value (ASCII-simple)
                    self.cursor -= 1;
                    self.input.remove(self.cursor);
                }
            }
            (KeyModifiers::NONE, KeyCode::Left) => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
            }
            (KeyModifiers::NONE, KeyCode::Right) => {
                if self.cursor < self.input.len() {
                    self.cursor += 1;
                }
            }
            (KeyModifiers::NONE, KeyCode::Home) => {
                self.cursor = 0;
            }
            (KeyModifiers::NONE, KeyCode::End) => {
                self.cursor = self.input.len();
            }

            // Right-pane scroll
            (KeyModifiers::NONE, KeyCode::PageUp) => {
                self.right_scroll = self.right_scroll.saturating_sub(4);
            }
            (KeyModifiers::NONE, KeyCode::PageDown) => {
                self.right_scroll = self.right_scroll.saturating_add(4);
            }

            _ => {}
        }

        Ok(None)
    }
}

fn ui(f: &mut Frame, app: &mut App) {
    // 1) split screen into left/right halves
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(f.size());

    let left = chunks[0];
    let right = chunks[1];

    // 2) split left vertically into textarea (top) + log (bottom)
    // tweak the Length for your desired textarea height
    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(10), Constraint::Min(0)])
        .split(left);

    draw_textarea(f, app, left_chunks[0]);
    draw_log(f, app, left_chunks[1]);
    draw_right_panel(f, app, right);
}

fn draw_textarea(f: &mut Frame, app: &App, area: Rect) {
    // Render a Paragraph as a fake textarea with a visible cursor marker.
    // We’ll show a slim block caret at the cursor by splitting text.
    let (before, after) = app.input.split_at(app.cursor);
    let caret = Span::styled(
        "▏",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    let mut lines = vec![];
    for (i, line) in before.split('\n').enumerate() {
        if i == 0 {
            lines.push(Line::from(line.to_owned()));
        } else {
            lines.push(Line::from(line.to_owned()));
        }
    }

    // Insert caret at end of "before", then append the rest.
    let last_line_len = before
        .rsplit_once('\n')
        .map(|(_, tail)| tail.len())
        .unwrap_or(before.len());
    // Build a single Text from: 'before' + caret + 'after'
    // Simpler: join as 3 spans in one Text; ratatui will wrap visually.
    let mut text = Text::default();
    text.extend(Text::from(before.to_string()));
    text.push_line(Line::from(vec![caret]));
    text.extend(Text::from(after.to_string()));

    let widget = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title(Span::styled(
            " Input (Ctrl+L to log, Esc to quit) ",
            Style::default().fg(Color::Cyan),
        )))
        .wrap(Wrap { trim: false });

    f.render_widget(widget, area);

    // Note: true terminal cursor positioning would require calculating (x,y) from
    // wrapped layout. For simplicity, we draw a styled caret glyph instead.
    // If you want a *real* cursor, compute visual (x,y) via layout rules or use a text-area crate.
    let _ = last_line_len; // kept to hint where you'd place a real cursor.
}

fn wrap_text_to_width(text: &str, max_width: u16) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
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
    }

    lines
}

fn draw_log(f: &mut Frame, app: &mut App, area: Rect) {
    let mut items: Vec<ListItem> = app
        .conversation_history
        .iter()
        .filter(|m| matches!(m.message_type, MessageType::User | MessageType::Assistant))
        .flat_map(|m| {
            let raw = format!("{}: {}", m.message_type.to_string(), m.content);
            let wrapped = wrap_text_to_width(&raw, area.width.saturating_sub(4));

            wrapped
                .into_iter()
                .map(|line| ListItem::new(line))
                .collect::<Vec<_>>()
        })
        .collect();

    if let Some(_) = app.response_receiving_handle {
        items.extend(vec![ListItem::new(vec![Line::from(get_spinner_char(
            app.spinner_increment,
        ))])]);

        app.spinner_increment += 1;
    }

    let widget = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(Span::styled(
            " Message Log ",
            Style::default().fg(Color::Yellow),
        )))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));

    f.render_widget(widget, area);
}

fn draw_right_panel(f: &mut Frame, app: &App, area: Rect) {
    let widget = Paragraph::new(app.content.as_str())
        .block(Block::default().borders(Borders::ALL).title(Span::styled(
            " Viewer (PgUp/PgDn) ",
            Style::default().fg(Color::Green),
        )))
        .wrap(Wrap { trim: false })
        .scroll((app.right_scroll, 0));

    f.render_widget(widget, area);
}
