use wire::prelude::{Tool, ToolWrapper, get_tool, tool};

pub const TODO_DIR: &str = "./.todos/";

pub fn get_tools() -> Vec<Tool> {
    vec![
        get_tool!(diff),
        get_tool!(add_todo),
        get_tool!(update_todo),
        get_tool!(list_todos),
        get_tool!(read_file),
        get_tool!(read_todo),
    ]
}

#[tool(description = "Get the `git diff` of the project")]
fn diff() -> String {
    let output = std::process::Command::new("git")
        .arg("diff")
        .output()
        .expect("failed to run");

    String::from_utf8(output.stdout).unwrap()
}

#[tool(description = r#"
Add a TODO item.

Notes:
- `name` will be a name for a markdown file--_do not_ assign its directory, just give it a name
- `description` should be in markdown
"#)]
fn add_todo(name: String, description: String) {
    let filename = format!("{}todo_{}.md", TODO_DIR, name);
    if let Err(e) = crate::file_tracking::FileTracker::write(&filename, &description) {
        panic!("Failed to create todo file {}: {}", filename, e);
    }
}

#[tool(
    description = "Update an existing TODO item. This looks up the TODO item by name and appends the `update` to its contents."
)]
fn update_todo(todo_name: String, update: String) {
    let filename = format!("{}{}", TODO_DIR, todo_name.clone());

    if let Err(e) =
        crate::file_tracking::FileTracker::write(&filename, &format!("{}\n\n---\n\n", update))
    {
        panic!("Failed to create todo file {}: {}", filename, e);
    }
}

#[tool(description = "Read the contents of a file.")]
fn read_file(filepath: String) -> String {
    let contents = crate::file_tracking::FileTracker::read(&filepath);
    if let Err(e) = contents {
        return format!("Failed to read todo file {}: {}", filepath, e);
    }

    contents.unwrap()
}

#[tool(
    description = "Returns the contents of the most relevant TODO item, found using embedding search with the query."
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

#[tool(description = "Lists the names of all outstanding TODO items.")]
pub fn list_todos() -> String {
    std::fs::read_dir(TODO_DIR)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<String>>()
        .join("; ")
}

#[tool(description = "Read an existing TODO item.")]
fn read_todo(todo_name: String) -> String {
    let filename = format!("{}{}", TODO_DIR, todo_name);

    let contents = crate::file_tracking::FileTracker::read(&filename.clone());
    if let Err(e) = contents {
        panic!("Failed to read todo file {}: {}", filename, e);
    }

    contents.unwrap()
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

    Ok(response.iter().last().unwrap().content.clone())
}
