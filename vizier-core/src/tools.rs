use std::path::{Path, PathBuf};

use crate::{config, observer::CaptureGuard, vcs};

pub const VIZIER_DIR: &str = ".vizier/";
pub const NARRATIVE_DIR: &str = "narrative/";
pub const SNAPSHOT_FILE: &str = "snapshot.md";
pub const LEGACY_SNAPSHOT_FILE: &str = ".snapshot";

#[derive(Clone, Debug, Default)]
pub struct Tool;

// TODO: We should only default to the current directory if there isn't a configured target
//       directory (for more automated/transient uses)
fn resolve_vizier_dir() -> Option<String> {
    let start_dir = std::env::current_dir().expect("Couldn't grab the current working directory");
    let mut current = start_dir.clone();
    let mut levels_up = 0;

    loop {
        if current.join(VIZIER_DIR).exists() {
            let mut path = "../".repeat(levels_up);
            path.push_str(VIZIER_DIR);
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

pub fn get_vizier_dir() -> String {
    resolve_vizier_dir().unwrap_or_else(|| panic!("Couldn't find `.vizier/`! How'd this happen?"))
}

pub fn try_get_vizier_dir() -> Option<String> {
    resolve_vizier_dir()
}

pub fn get_narrative_dir() -> String {
    format!("{}{}", get_vizier_dir(), NARRATIVE_DIR)
}

pub fn try_get_narrative_dir() -> Option<String> {
    try_get_vizier_dir().map(|dir| format!("{}{}", dir, NARRATIVE_DIR))
}

pub fn snapshot_path() -> PathBuf {
    try_snapshot_path().unwrap_or_else(|| PathBuf::from(get_narrative_dir()).join(SNAPSHOT_FILE))
}

pub fn try_snapshot_path() -> Option<PathBuf> {
    let vizier_root = try_get_vizier_dir().map(PathBuf::from)?;

    if let Some(existing) = discover_snapshot_path(&vizier_root) {
        return Some(existing);
    }

    Some(vizier_root.join(NARRATIVE_DIR).join(SNAPSHOT_FILE))
}

pub fn read_snapshot() -> String {
    try_snapshot_path()
        .and_then(|path| std::fs::read_to_string(&path).ok())
        .unwrap_or_default()
}

pub fn is_snapshot_file(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let file_name = Path::new(normalized.as_str())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();

    matches!(file_name, SNAPSHOT_FILE | LEGACY_SNAPSHOT_FILE)
}

pub fn story_diff_targets() -> Vec<String> {
    let mut targets = vec![get_narrative_dir()];

    if let Some(snapshot) = try_get_vizier_dir()
        .map(PathBuf::from)
        .and_then(|root| discover_snapshot_path(&root))
    {
        let normalized = snapshot.to_string_lossy().replace('\\', "/");
        let narrative_dir = get_narrative_dir();

        if !normalized.starts_with(&narrative_dir) {
            targets.push(normalized);
        }
    }

    targets
}

fn discover_snapshot_path(vizier_root: &Path) -> Option<PathBuf> {
    for candidate in [
        vizier_root.join(NARRATIVE_DIR).join(SNAPSHOT_FILE),
        vizier_root.join(LEGACY_SNAPSHOT_FILE),
    ] {
        if candidate.exists() {
            return Some(candidate);
        }
    }

    find_snapshot_anywhere(vizier_root)
}

fn find_snapshot_anywhere(vizier_root: &Path) -> Option<PathBuf> {
    let mut stack = vec![vizier_root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };

            if file_type.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str())
                    && matches!(name, "tmp" | "tmp-worktrees" | "tmp_worktrees" | "sessions")
                {
                    continue;
                }

                stack.push(path);
                continue;
            }

            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_snapshot_file)
            {
                return Some(path);
            }
        }
    }

    None
}

pub fn is_action(name: &str) -> bool {
    let _ = name;
    false
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
    if !out.is_empty() {
        response = format!("<stdout>{}</stdout>", out);
    }

    if !err.is_empty() {
        response = format!("<stderr>{}</stderr>", err);
    }

    response
}

pub fn diff() -> String {
    let guard = CaptureGuard::start();
    match vcs::get_diff(".", None, None) {
        Ok(d) => build_llm_response(d, &guard),
        Err(e) => llm_error(&format!("Error getting diff: {}", e)),
    }
}

pub fn git_log(depth: String, commit_message_type: String) -> String {
    let guard = CaptureGuard::start();

    match vcs::get_log(
        depth.parse::<usize>().unwrap_or(10).max(10),
        if !commit_message_type.is_empty() {
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
        Err(e) => llm_error(&format!("Error getting git log: {}", e)),
    }
}

pub fn list_narrative_docs() -> String {
    let root = match try_get_narrative_dir() {
        Some(dir) => PathBuf::from(dir),
        None => return String::new(),
    };

    let mut names = match collect_markdown_entries(&root) {
        Ok(entries) => entries,
        Err(e) => {
            return llm_error(&format!(
                "Error reading narrative directory {}: {}",
                root.display(),
                e
            ));
        }
    };

    names.sort();
    names.dedup();
    names.join("; ")
}

fn collect_markdown_entries(root: &Path) -> Result<Vec<String>, std::io::Error> {
    let mut stack = vec![root.to_path_buf()];
    let mut entries = Vec::new();

    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;

            if file_type.is_dir() {
                stack.push(path);
                continue;
            }

            if path.file_name().and_then(|n| n.to_str()) == Some(SNAPSHOT_FILE) {
                continue;
            }

            if !path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                continue;
            }

            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            entries.push(relative);
        }
    }

    Ok(entries)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn discover_snapshot_prefers_canonical_location() {
        let temp = tempdir().unwrap();
        let vizier_root = temp.path().join(".vizier");
        fs::create_dir_all(vizier_root.join("narrative")).unwrap();
        fs::write(
            vizier_root.join("narrative").join(SNAPSHOT_FILE),
            "canonical",
        )
        .unwrap();
        fs::write(vizier_root.join(LEGACY_SNAPSHOT_FILE), "legacy").unwrap();

        let found = discover_snapshot_path(&vizier_root).unwrap();
        assert_eq!(found, vizier_root.join("narrative").join(SNAPSHOT_FILE));
    }

    #[test]
    fn discover_snapshot_falls_back_to_legacy_file() {
        let temp = tempdir().unwrap();
        let vizier_root = temp.path().join(".vizier");
        fs::create_dir_all(&vizier_root).unwrap();
        fs::write(vizier_root.join(LEGACY_SNAPSHOT_FILE), "legacy").unwrap();

        let found = discover_snapshot_path(&vizier_root).unwrap();
        assert_eq!(found, vizier_root.join(LEGACY_SNAPSHOT_FILE));
    }

    #[test]
    fn discover_snapshot_scans_nested_paths() {
        let temp = tempdir().unwrap();
        let vizier_root = temp.path().join(".vizier");
        let nested = vizier_root.join("nested/deeper");
        fs::create_dir_all(&nested).unwrap();
        let nested_snapshot = nested.join(SNAPSHOT_FILE);
        fs::write(&nested_snapshot, "content").unwrap();

        let found = discover_snapshot_path(&vizier_root).unwrap();
        assert_eq!(found, nested_snapshot);
    }
}
