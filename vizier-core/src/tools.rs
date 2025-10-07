use std::sync::RwLock;

use tokio::sync::mpsc;
use wire::prelude::{Tool, ToolWrapper, get_tool, tool};

use crate::{file_tracking, observer::CaptureGuard, vcs};

const TODO_DIR: &str = ".vizier/";

// TODO: We should only default to the current directory if there isn't a configured target
//       directory (for more automated/transient uses)
pub fn get_todo_dir() -> String {
    let start_dir = std::env::current_dir()
        .ok()
        .expect("Couldn't grab the current working directory");
    let mut current = start_dir.clone();
    let mut levels_up = 0;

    loop {
        if current.join(TODO_DIR).exists() {
            let mut path = "../".repeat(levels_up);
            path.push_str(TODO_DIR);
            return path;
        }

        if current.join(".git").exists() {
            panic!("Couldn't find `.vizier/`! How'd this happen?");
        }

        match current.parent() {
            Some(parent) => {
                current = parent.to_path_buf();
                levels_up += 1;
            }
            None => panic!("Couldn't find `.vizier/`! How'd this happen?"),
        }
    }
}

pub fn get_tools() -> Vec<Tool> {
    vec![
        get_tool!(create_git_issue),
        get_tool!(diff),
        get_tool!(git_log),
        get_tool!(add_todo),
        get_tool!(delete_todo),
        get_tool!(update_todo),
        get_tool!(list_todos),
        get_tool!(read_file),
        get_tool!(read_todo),
        get_tool!(read_snapshot),
        get_tool!(update_snapshot),
    ]
}

pub fn get_snapshot_tools() -> Vec<Tool> {
    vec![
        get_tool!(git_log),
        get_tool!(list_todos),
        get_tool!(read_file),
        get_tool!(update_snapshot),
    ]
}

pub fn is_action(name: &str) -> bool {
    name == "add_todo"
        || name == "update_todo"
        || name == "update_snapshot"
        || name == "update_todo_status"
}

// TODO: We need a better way of handling errors as they happen here.
//       Right now the current approach is to just unwrap them and that really isn't working at
//       all in terms of maintaining flow with the language models.

// TODO: The wire needs updated to accept more kinds of types for the function arguments

pub fn llm_error(message: &str) -> String {
    format!("<error>{}</error>", message)
}

pub fn build_llm_response(tool_output: String, guard: &CaptureGuard) -> String {
    let mut response = format!("<tool_output>{}</tool_output>", tool_output);

    let (out, err) = guard.take_both();
    if out.len() > 0 {
        response = format!("<stdout>{}</stdout>", out);
    }

    if err.len() > 0 {
        response = format!("<stderr>{}</stderr>", err);
    }

    response
}

// This is our hook into TUI app state
// Initialized in the editor::App constructor
// TODO: This is probably bad form

pub static SENDER: RwLock<Option<mpsc::UnboundedSender<String>>> = RwLock::new(None);

pub fn get_editor_tools() -> Vec<Tool> {
    vec![get_tool!(edit_content)]
}

#[tool(description = "Replace the content of the file shown to the user")]
pub fn edit_content(content: String) -> String {
    let guard = CaptureGuard::start();

    // bro lmfao
    match SENDER.read().unwrap().clone().unwrap().send(content) {
        Ok(_) => build_llm_response(String::new(), &guard),
        Err(e) => return llm_error(&format!("Error getting diff: {}", e)),
    }
}

#[tool(description = "Get the `git diff` of the project")]
pub fn diff() -> String {
    let guard = CaptureGuard::start();
    match vcs::get_diff(".", None, None) {
        Ok(d) => build_llm_response(d, &guard),
        Err(e) => return llm_error(&format!("Error getting diff: {}", e)),
    }
}

#[tool(description = "
Display recent commits from the current Git repository.

Parameters:
    depth: Maximum number of commits to display (default: 10 if parsing fails--this is also the maximum).
    commit_message_type: Optional filter; only commits whose messages contain this string
                         (case-insensitive) will be shown. If empty, no filtering is applied.
                         Null is inapplicable here. Use an empty string to omit.

Output:
- Each commit is listed on a new line in the format:
    <short_sha>  <summary> — <author>

Notes:
- Commits are ordered chronologically descending (most recent first).
- `depth` applies *after* filtering, so fewer than `depth` commits may appear if
  not enough commits match the filter.
- There is a filter applied after retrieval that removes conversation logs
- If no commit messages match, output will be empty.
")]
pub fn git_log(depth: String, commit_message_type: String) -> String {
    let guard = CaptureGuard::start();

    match vcs::get_log(
        depth.parse::<usize>().unwrap_or(10).max(10),
        if commit_message_type.len() > 0 {
            Some(vec![commit_message_type])
        } else {
            None
        },
    ) {
        Ok(d) => build_llm_response(
            d.iter()
                .filter(|m| !m.contains("VIZIER CONVERSATION"))
                .cloned()
                .collect::<Vec<String>>()
                .join("\n"),
            &guard,
        ),
        Err(e) => return llm_error(&format!("Error getting git log: {}", e)),
    }
}

#[tool(description = "
Add a TODO item.

Parameters:
    todo_name: Name of the TODO item to add
    description: Content of the TODO item

Notes:
- `name` will be a name for a markdown file--_do not_ assign its directory, just give it a name
- `description` should be in markdown
- The provided `todo_name` argument will be formatted as `todo_{todo_name}.md` for the final filename
")]
fn add_todo(name: String, description: String) -> String {
    let filename = format!("{}todo_{}.md", get_todo_dir(), name);
    if let Err(e) = file_tracking::FileTracker::write(&filename, &description) {
        llm_error(&format!("Failed to create todo file {}: {}", filename, e))
    } else {
        "Todo added successfully".to_string()
    }
}

#[tool(description = "
Delete a TODO item.

Parameters:
    name: Name of the TODO item to delete

Notes:
- `name` should match the name used when creating the todo (without the 'todo_' prefix or '.md' extension)
- This will permanently remove the todo file
")]
fn delete_todo(name: String) -> String {
    let filename = format!("{}todo_{}.md", get_todo_dir(), name);
    if let Err(e) = file_tracking::FileTracker::delete(&filename) {
        llm_error(&format!("Failed to create delete file {}: {}", filename, e))
    } else {
        "Todo deleted successfully".to_string()
    }
}

#[tool(description = "Updates an existing TODO item by appending new content.

Parameters:
    todo_name: Name of the TODO item to update
    update: Content to append to the item

Notes: Content is appended with separator lines for readability
")]
fn update_todo(todo_name: String, update: String) -> String {
    let filename = format!("{}todo_{}.md", get_todo_dir(), todo_name.clone());

    if let Err(e) = file_tracking::FileTracker::write(&filename, &format!("{}\n\n---\n\n", update))
    {
        llm_error(&format!("Failed to create todo file {}: {}", filename, e))
    } else {
        "Todo updated successfully".to_string()
    }
}

#[tool(description = "Reads and returns the contents of a specified file.

Parameters:
    filepath: Path to the file to read

Returns: String containing file contents or error message if read fails")]
fn read_file(filepath: String) -> String {
    let contents = file_tracking::FileTracker::read(&filepath);
    if let Err(e) = contents {
        return llm_error(&format!("Failed to read todo file {}: {}", filepath, e));
    }

    contents.unwrap_or_default()
}

#[tool(description = "Lists all existing TODO items.

Returns: Semicolon-separated string of TODO item names")]
pub fn list_todos() -> String {
    match std::fs::read_dir(get_todo_dir()) {
        Ok(d) => d
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<String>>()
            .join("; "),
        Err(e) => llm_error(&format!(
            "Error reading directory {}: {}",
            get_todo_dir(),
            e
        )),
    }
}

#[tool(description = "Retrieves the contents of a specific TODO item.

Parameters:
    todo_name: Name of the TODO item to read

Returns: String containing the TODO item's contents")]
fn read_todo(todo_name: String) -> String {
    let filename = format!("{}{}", get_todo_dir(), todo_name);

    let contents = file_tracking::FileTracker::read(&filename.clone());
    if let Err(e) = contents {
        llm_error(&format!("Failed to read todo file {}: {}", filename, e))
    } else {
        contents.unwrap_or_default()
    }
}

#[tool(description = "Retrieves the current project trajectory snapshot.

Returns: String containing snapshot contents or empty string if none exists")]
pub fn read_snapshot() -> String {
    let filename = format!("{}{}", get_todo_dir(), ".snapshot");
    std::fs::read_to_string(&filename).unwrap_or_default()
}

#[tool(
    description = "Updates the project trajectory snapshot with new content.

Parameters:
    content: New snapshot content to write

Notes: Overwrites any existing snapshot"
)]
fn update_snapshot(content: String) -> String {
    let filename = format!("{}{}", get_todo_dir(), ".snapshot");

    if let Err(e) = std::fs::write(&filename, &content) {
        llm_error(&format!("Failed to update snapshot: {}", e))
    } else {
        "Snapshot updated successfully".to_string()
    }
}

#[tool(
    description = "Creates a new GitHub issue in the repository associated with the current working directory.

Parameters:
    title: Title of the new GitHub issue
    body: Body/description content of the issue

Notes:
    - Uses GitHub’s REST API (v2022-11-28).
    - Only use this tool at the user's request.
      Please note that mentions of this tool (e.g., `need a git issue`, etc.) are authorization enough to proceed for this.
    - The issue body will automatically be signed noting that the issue was agentically created--_do not_ sign it yourself.

Returns:
    String containing the raw API response if successful, or an error message if the request fails."
)]
fn create_git_issue(title: String, body: String) -> String {
    use reqwest::blocking::Client;
    use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};

    let (owner, repo) = match vcs::origin_owner_repo(".") {
        Ok((o, r)) => (o, r),
        Err(e) => return llm_error(&format!("Failed to get owner and repo name: {}", e)),
    };

    let token = match std::env::var("GITHUB_PAT") {
        Ok(t) => t,
        Err(e) => return llm_error(&format!("Failed to get GitHub PAT: {}", e)),
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );

    headers.insert(
        "X-GitHub-Api-Version",
        HeaderValue::from_static("2022-11-28"),
    );

    let auth_value = format!("Bearer {}", token);
    headers.insert(AUTHORIZATION, HeaderValue::from_str(&auth_value).unwrap());

    headers.insert(USER_AGENT, HeaderValue::from_static("vizier"));

    let client = match Client::builder().default_headers(headers).build() {
        Ok(c) => c,
        Err(e) => return llm_error(&format!("Failed to build reqwest client: {}", e)),
    };

    let url = format!("https://api.github.com/repos/{owner}/{repo}/issues");
    let payload = serde_json::json!({
        "title": title,
        "body": format!("This issue was written by the Vizier.\n\n{}", body),
    });

    let resp = match client.post(&url).json(&payload).send() {
        Ok(r) => r,
        Err(e) => return llm_error(&format!("Error sending GitHub API request: {}", e)),
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().unwrap_or_default();

        return llm_error(&format!("GitHub API error {status}: {text}"));
    }

    match resp.text() {
        Ok(r) => r,
        Err(e) => llm_error(&format!("Error reading response text: {}", e)),
    }
}
