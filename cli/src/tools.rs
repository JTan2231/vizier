use wire::prelude::{Tool, ToolWrapper, get_tool, tool};

use serde::{Deserialize, Serialize};

pub const TODO_DIR: &str = "./.vizier/";

pub fn get_tools() -> Vec<Tool> {
    vec![
        get_tool!(diff),
        get_tool!(add_todo),
        get_tool!(update_todo),
        get_tool!(list_todos),
        get_tool!(read_file),
        get_tool!(read_todo),
        get_tool!(read_snapshot),
        get_tool!(update_snapshot),
        get_tool!(update_todo_status),
        get_tool!(read_todo_status),
    ]
}

// TODO: We need a better way of handling errors as they happen here.
//       Right now the current approach is to just unwrap them and that really isn't working at
//       all in terms of maintaining flow with the language models.

#[tool(description = "Get the `git diff` of the project")]
fn diff() -> String {
    let output = std::process::Command::new("git")
        .arg("diff")
        .output()
        .expect("failed to run");

    String::from_utf8(output.stdout).unwrap()
}

#[tool(description = "
Add a TODO item.

Notes:
- `name` will be a name for a markdown file--_do not_ assign its directory, just give it a name
- `description` should be in markdown
")]
fn add_todo(name: String, description: String) {
    let filename = format!("{}todo_{}.md", TODO_DIR, name);
    if let Err(e) = crate::file_tracking::FileTracker::write(&filename, &description) {
        panic!("Failed to create todo file {}: {}", filename, e);
    }
}

#[tool(description = "Updates an existing TODO item by appending new content.

Parameters:
    todo_name: Name of the TODO item to update
    update: Content to append to the item

Notes: Content is appended with separator lines for readability
")]
fn update_todo(todo_name: String, update: String) {
    let filename = format!("{}{}", TODO_DIR, todo_name.clone());

    if let Err(e) =
        crate::file_tracking::FileTracker::write(&filename, &format!("{}\n\n---\n\n", update))
    {
        panic!("Failed to create todo file {}: {}", filename, e);
    }
}

#[tool(description = "Reads and returns the contents of a specified file.

Parameters:
    filepath: Path to the file to read

Returns: String containing file contents or error message if read fails")]
fn read_file(filepath: String) -> String {
    let contents = crate::file_tracking::FileTracker::read(&filepath);
    if let Err(e) = contents {
        return format!("Failed to read todo file {}: {}", filepath, e);
    }

    contents.unwrap()
}

#[tool(
    description = "Finds and returns the most relevant TODO item using semantic search.

Parameters:
    query: Search query to match against TODO contents

Returns: Content of the best-matching TODO item, optionally limited to relevant subset"
)]
fn todo_lookup(query: String) -> String {
    let mut dewey = dewey_lib::Dewey::new().unwrap();

    let source = dewey.query(query, 1).unwrap()[0].clone();

    let todo_contents = std::fs::read_to_string(source.filepath).unwrap();

    if let Some(subset) = source.subset {
        todo_contents[subset.0 as usize..subset.1 as usize].to_string()
    } else {
        todo_contents
    }
}

#[tool(description = "Lists all existing TODO items.

Returns: Semicolon-separated string of TODO item names")]
pub fn list_todos() -> String {
    std::fs::read_dir(TODO_DIR)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<String>>()
        .join("; ")
}

#[tool(description = "Retrieves the contents of a specific TODO item.

Parameters:
    todo_name: Name of the TODO item to read

Returns: String containing the TODO item's contents")]
fn read_todo(todo_name: String) -> String {
    let filename = format!("{}{}", TODO_DIR, todo_name);

    let contents = crate::file_tracking::FileTracker::read(&filename.clone());
    if let Err(e) = contents {
        panic!("Failed to read todo file {}: {}", filename, e);
    }

    contents.unwrap()
}

#[tool(description = "Retrieves the current project trajectory snapshot.

Returns: String containing snapshot contents or empty string if none exists")]
fn read_snapshot() -> String {
    let filename = format!("{}{}", TODO_DIR, ".snapshot");
    std::fs::read_to_string(&filename).unwrap_or_default()
}

#[tool(
    description = "Updates the project trajectory snapshot with new content.

Parameters:
    content: New snapshot content to write

Notes: Overwrites any existing snapshot"
)]
fn update_snapshot(content: String) {
    let filename = format!("{}{}", TODO_DIR, ".snapshot");

    if let Err(e) = std::fs::write(&filename, &content) {
        panic!("Failed to update snapshot: {}", e);
    }
}

#[derive(Serialize, Deserialize, Debug)]
enum Status {
    Ready,
    InProgress,
    Done,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Status::Ready => write!(f, "ready"),
            Status::InProgress => write!(f, "in_progress"),
            Status::Done => write!(f, "done"),
        }
    }
}

impl std::str::FromStr for Status {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ready" => Ok(Status::Ready),
            "in_progress" => Ok(Status::InProgress),
            "done" => Ok(Status::Done),
            _ => Err(format!("Invalid status: {}", s)),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct TodoEntry {
    status: Status,
    notes: String,
}

type TodoMap = std::collections::HashMap<String, TodoEntry>;

// TODO: When the model gets the filename wrong?
#[tool(description = "Updates the status and notes of an existing TODO item.

Parameters:
    todo_name: The unique identifier of the TODO to update
    status: New status to set (Ready/In Progress/Done)
    notes: Additional notes or context for the status update. If empty, existing notes are preserved.

Returns:
    None. Updates are saved to storage directly.")]
fn update_todo_status(todo_name: String, status: String, notes: String) {
    let mut todos = load_todos().unwrap();

    if let Some(existing) = todos.get(&todo_name) {
        todos.insert(
            todo_name,
            TodoEntry {
                status: status.parse::<Status>().unwrap(),
                notes: if notes.is_empty() {
                    existing.notes.clone()
                } else {
                    notes
                },
            },
        );
    } else {
        todos.insert(
            todo_name,
            TodoEntry {
                status: status.parse::<Status>().unwrap(),
                notes,
            },
        );
    }
}

#[tool(description = "Retrieves the status and notes of a TODO item.

Parameters:
    todo_name: The unique identifier of the TODO to look up

Returns:
    XML string containing the TODO details if found (<todo><name>...</name><status>...</status><notes>...</notes></todo>)
    or an error message if not found (<error>...</error>)")]
fn read_todo_status(todo_name: String) -> String {
    let todos = load_todos().unwrap();

    match todos.get(&todo_name) {
        Some(todo) => format!(
            "<todo><name>{}</name><status>{}</status><notes>{}</notes></todo>",
            todo_name, todo.status, todo.notes
        ),
        None => format!("<error>TODO '{}' not found</error>", todo_name),
    }
}

fn load_todos() -> Result<TodoMap, std::io::Error> {
    let data = std::fs::read_to_string("todos.json")?;
    Ok(serde_json::from_str(&data)?)
}

// TODO: this will need to account for statuses and whatnot in the future--it doesn't right now
pub async fn summarize_todos() -> Result<String, Box<dyn std::error::Error>> {
    let contents = std::fs::read_dir(TODO_DIR)
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<Vec<String>>()
        .join("\n\n###\n\n");

    let prompt =
        "You will be given a list of TODO items. Return a summary of all the outstanding work. Focus on broad themes and directions."
            .to_string();

    let response = crate::config::llm_request(vec![], prompt, contents).await?;

    Ok(response)
}
