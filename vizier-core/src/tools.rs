use crate::{config, file_tracking, observer::CaptureGuard, vcs};

const TODO_DIR: &str = ".vizier/";

#[derive(Clone, Debug, Default)]
pub struct Tool;

// TODO: We should only default to the current directory if there isn't a configured target
//       directory (for more automated/transient uses)
fn resolve_todo_dir() -> Option<String> {
    let start_dir = std::env::current_dir()
        .ok()
        .expect("Couldn't grab the current working directory");
    let mut current = start_dir.clone();
    let mut levels_up = 0;

    loop {
        if current.join(TODO_DIR).exists() {
            let mut path = "../".repeat(levels_up);
            path.push_str(TODO_DIR);
            return Some(path);
        }

        if current.join(".git").exists() {
            return None;
        }

        match current.parent() {
            Some(parent) => {
                current = parent.to_path_buf();
                levels_up += 1;
            }
            None => return None,
        }
    }
}

pub fn get_todo_dir() -> String {
    resolve_todo_dir().unwrap_or_else(|| panic!("Couldn't find `.vizier/`! How'd this happen?"))
}

pub fn try_get_todo_dir() -> Option<String> {
    resolve_todo_dir()
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

// TODO: Expand tool argument support for richer structured inputs

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

pub fn diff() -> String {
    let guard = CaptureGuard::start();
    match vcs::get_diff(".", None, None) {
        Ok(d) => build_llm_response(d, &guard),
        Err(e) => return llm_error(&format!("Error getting diff: {}", e)),
    }
}

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

#[allow(dead_code)]
fn build_todo_path(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(llm_error("TODO name cannot be empty"));
    }

    if trimmed.starts_with('.') {
        return Err(llm_error(
            "TODO name cannot start with '.'; choose a visible slug",
        ));
    }

    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err(llm_error(
            "TODO name cannot contain path separators. Provide a single slug.",
        ));
    }

    Ok(format!("{}{}", get_todo_dir(), trimmed))
}

#[allow(dead_code)]
fn add_todo(name: String, description: String) -> String {
    let filename = match build_todo_path(&name) {
        Ok(path) => path,
        Err(err) => return err,
    };

    if let Err(e) = file_tracking::FileTracker::write(&filename, &description) {
        llm_error(&format!("Failed to create todo file {}: {}", filename, e))
    } else {
        "Todo added successfully".to_string()
    }
}

#[allow(dead_code)]
fn delete_todo(name: String) -> String {
    let filename = match build_todo_path(&name) {
        Ok(path) => path,
        Err(err) => return err,
    };

    if let Err(e) = file_tracking::FileTracker::delete(&filename) {
        llm_error(&format!("Failed to create delete file {}: {}", filename, e))
    } else {
        "Todo deleted successfully".to_string()
    }
}

#[allow(dead_code)]
fn update_todo(todo_name: String, update: String) -> String {
    let filename = match build_todo_path(&todo_name) {
        Ok(path) => path,
        Err(err) => return err,
    };

    if let Err(e) = file_tracking::FileTracker::write(&filename, &format!("{}\n\n---\n\n", update))
    {
        llm_error(&format!("Failed to create todo file {}: {}", filename, e))
    } else {
        "Todo updated successfully".to_string()
    }
}

#[allow(dead_code)]
fn read_file(filepath: String) -> String {
    let contents = file_tracking::FileTracker::read(&filepath);
    if let Err(e) = contents {
        return llm_error(&format!("Failed to read todo file {}: {}", filepath, e));
    }

    contents.unwrap_or_default()
}

pub fn list_todos() -> String {
    match std::fs::read_dir(get_todo_dir()) {
        Ok(d) => {
            let mut names = Vec::new();

            for entry in d.flatten() {
                let file_type = match entry.file_type() {
                    Ok(ft) => ft,
                    Err(_) => continue,
                };

                if !file_type.is_file() {
                    continue;
                }

                let name = match entry.file_name().into_string() {
                    Ok(name) => name,
                    Err(_) => continue,
                };

                if name.starts_with('.') {
                    continue;
                }

                names.push(name);
            }

            names.join("; ")
        }
        Err(e) => llm_error(&format!(
            "Error reading directory {}: {}",
            get_todo_dir(),
            e
        )),
    }
}

#[allow(dead_code)]
fn read_todo(todo_name: String) -> String {
    let filename = match build_todo_path(&todo_name) {
        Ok(path) => path,
        Err(err) => return err,
    };

    let contents = file_tracking::FileTracker::read(&filename.clone());
    if let Err(e) = contents {
        llm_error(&format!("Failed to read todo file {}: {}", filename, e))
    } else {
        contents.unwrap_or_default()
    }
}

pub fn read_snapshot() -> String {
    let filename = format!("{}{}", get_todo_dir(), ".snapshot");
    std::fs::read_to_string(&filename).unwrap_or_default()
}

#[allow(dead_code)]
fn update_snapshot(content: String) -> String {
    let filename = format!("{}{}", get_todo_dir(), ".snapshot");

    if let Err(e) = std::fs::write(&filename, &content) {
        llm_error(&format!("Failed to update snapshot: {}", e))
    } else {
        "Snapshot updated successfully".to_string()
    }
}

#[allow(dead_code)]
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

pub fn get_tools() -> Vec<Tool> {
    Vec::new()
}

pub fn get_snapshot_tools() -> Vec<Tool> {
    Vec::new()
}

pub fn active_tooling_for(agent: &config::AgentSettings) -> Vec<Tool> {
    let _ = agent;
    Vec::new()
}
