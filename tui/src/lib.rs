use std::fs;
use std::io::{Result, stdout};
use std::path::PathBuf;

use std::process::Command;

use tempfile::{Builder, TempPath};

use crossterm::{
    ExecutableCommand,
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

/// Application state
struct App {
    path: PathBuf,
    files: Vec<PathBuf>,
    list_state: ListState,
    file_content: String,
    scroll: u16,
}

impl App {
    /// Create a new application instance
    fn new(todos_path: PathBuf) -> Result<Self> {
        let mut app = Self {
            path: todos_path,
            files: Vec::new(),
            list_state: ListState::default(),
            file_content: String::new(),
            scroll: 0,
        };

        app.list_state.select(Some(0));
        app.refresh_files();
        app.read_selected_file_content();
        Ok(app)
    }

    /// Refresh the list of files in the current directory
    fn refresh_files(&mut self) {
        self.files = fs::read_dir(&self.path)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .collect();

        self.files.sort_by(|a, b| {
            let a_is_dir = a.is_dir();
            let b_is_dir = b.is_dir();
            b_is_dir.cmp(&a_is_dir).then_with(|| a.cmp(b))
        });
    }

    fn get_selected_file_path(&self) -> Option<PathBuf> {
        if let Some(selected) = self.list_state.selected() {
            if let Some(path) = self.files.get(selected) {
                // Return the found path, cloned so the caller gets ownership.
                return Some(path.clone());
            }
        }
        // If 'selected' is None or the path doesn't exist at the index, return None.
        None
    }

    /// Read the content of the currently selected file
    fn read_selected_file_content(&mut self) {
        if let Some(selected) = self.list_state.selected() {
            if let Some(path) = self.files.get(selected) {
                if path.is_file() {
                    self.file_content = fs::read_to_string(path).unwrap_or_else(|e| e.to_string());
                } else {
                    self.file_content = "This is a directory.".to_string();
                }
            }
        } else {
            self.file_content = String::new();
        }

        self.scroll = 0; // Reset scroll on new file selection
    }

    /// Move selection to the next item
    fn select_next(&mut self) {
        let i = match self.list_state.selected() {
            Some(i) => {
                if i >= self.files.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };

        self.list_state.select(Some(i));
        self.read_selected_file_content();
    }

    /// Move selection to the previous item
    fn select_previous(&mut self) {
        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.files.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };

        self.list_state.select(Some(i));
        self.read_selected_file_content();
    }

    /// Enter a directory or open it in the system editor if it's a file
    fn enter_directory(&mut self) -> Result<()> {
        if let Some(selected) = self.list_state.selected() {
            if let Some(path) = self.files.get(selected) {
                if path.is_dir() {
                    self.path = path.clone();
                    self.refresh_files();
                    self.list_state.select(Some(0));
                    self.read_selected_file_content();
                } else {
                    // TODO: Error messaging
                    disable_raw_mode()?;
                    stdout().execute(LeaveAlternateScreen)?;

                    user_editor(&self.file_content)?;

                    // TODO: This is probably stupid
                    std::process::exit(0);
                }
            }
        }

        Ok(())
    }

    /// Go to the parent directory
    fn go_to_parent(&mut self) {
        if let Some(parent) = self.path.parent() {
            self.path = parent.to_path_buf();
            self.refresh_files();
            self.list_state.select(Some(0));
            self.read_selected_file_content();
        }
    }

    /// Scroll the file preview down
    fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_add(1);
    }

    /// Scroll the file preview up
    fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }
}

pub fn tui(todos_path: PathBuf) -> Result<()> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let mut app = App::new(todos_path)?;

    loop {
        terminal.draw(|f| ui(f, &mut app))?;

        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Down => app.select_next(),
                    KeyCode::Up => app.select_previous(),
                    KeyCode::Left => app.go_to_parent(),
                    KeyCode::Right | KeyCode::Enter => app.enter_directory()?,
                    KeyCode::PageDown => app.scroll_down(),
                    KeyCode::PageUp => app.scroll_up(),
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    Ok(())
}

/// Render the user interface
fn ui(frame: &mut Frame, app: &mut App) {
    // Create a two-column layout
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
        .split(frame.area());

    let items: Vec<ListItem> = app
        .files
        .iter()
        .map(|path| {
            let filename = path.file_name().unwrap().to_str().unwrap_or("?");
            let mut style = Style::default();
            if path.is_dir() {
                style = style.fg(Color::Cyan);
            }

            ListItem::new(filename).style(style)
        })
        .collect();

    let file_list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(app.path.to_str().unwrap_or(".")),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    frame.render_stateful_widget(file_list, chunks[0], &mut app.list_state);

    let preview_block = Block::default().title("Preview").borders(Borders::ALL);
    let preview_paragraph = Paragraph::new(app.file_content.as_str())
        .block(preview_block)
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0));

    frame.render_widget(preview_paragraph, chunks[1]);
}

enum Shell {
    Bash,
    Zsh,
    Fish,
    Other(String),
}

impl Shell {
    fn from_path(shell_path: &str) -> Self {
        let shell_name = std::path::PathBuf::from(shell_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_lowercase();

        match shell_name.as_str() {
            "bash" => Shell::Bash,
            "zsh" => Shell::Zsh,
            "fish" => Shell::Fish,
            other => Shell::Other(other.to_string()),
        }
    }

    fn get_rc_source_command(&self) -> String {
        match self {
            Shell::Bash => ". ~/.bashrc".to_string(),
            Shell::Zsh => ". ~/.zshrc".to_string(),
            Shell::Fish => "source ~/.config/fish/config.fish".to_string(),
            Shell::Other(_) => "".to_string(),
        }
    }

    fn get_interactive_args(&self) -> Vec<String> {
        match self {
            Shell::Fish => vec!["-C".to_string()],
            _ => vec!["-i".to_string(), "-c".to_string()],
        }
    }
}

fn user_editor(file_contents: &str) -> std::io::Result<()> {
    let temp_file = Builder::new()
        .prefix("vizier_todo")
        .suffix(".md")
        .tempfile()?;

    let temp_path: TempPath = temp_file.into_temp_path();

    match std::fs::write(temp_path.to_path_buf(), file_contents) {
        Ok(_) => {}
        Err(e) => {
            println!("Error writing to temp file");
            return Err(e);
        }
    };

    let shell_path = std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string());
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
    let shell = Shell::from_path(&shell_path);

    let command = format!("{} {}", editor, temp_path.to_str().unwrap());
    let rc_source = shell.get_rc_source_command();
    let full_command = if rc_source.is_empty() {
        command
    } else {
        format!("{} && {}", rc_source, command)
    };

    let status = Command::new(shell_path)
        .args(shell.get_interactive_args())
        .arg("-c")
        .arg(&full_command)
        .status()?;

    if !status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Editor command failed",
        ));
    }

    Ok(())
}
