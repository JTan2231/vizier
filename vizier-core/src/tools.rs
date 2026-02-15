use std::path::{Path, PathBuf};

use crate::{config, observer::CaptureGuard, vcs};

pub const VIZIER_DIR: &str = ".vizier/";
pub const NARRATIVE_DIR: &str = "narrative/";
pub const SNAPSHOT_FILE: &str = "snapshot.md";
pub const GLOSSARY_FILE: &str = "glossary.md";

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
    Some(vizier_root.join(NARRATIVE_DIR).join(SNAPSHOT_FILE))
}

pub fn glossary_path() -> PathBuf {
    try_glossary_path().unwrap_or_else(|| PathBuf::from(get_narrative_dir()).join(GLOSSARY_FILE))
}

pub fn try_glossary_path() -> Option<PathBuf> {
    let vizier_root = try_get_vizier_dir().map(PathBuf::from)?;
    Some(vizier_root.join(NARRATIVE_DIR).join(GLOSSARY_FILE))
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

    file_name.eq_ignore_ascii_case(SNAPSHOT_FILE)
}

pub fn is_glossary_file(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let file_name = Path::new(normalized.as_str())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();

    file_name.eq_ignore_ascii_case(GLOSSARY_FILE)
}

pub fn story_diff_targets() -> Vec<String> {
    vec![get_narrative_dir()]
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

    names.sort_by(|a, b| {
        let a_priority = if a == GLOSSARY_FILE { 0 } else { 1 };
        let b_priority = if b == GLOSSARY_FILE { 0 } else { 1 };
        a_priority.cmp(&b_priority).then_with(|| a.cmp(b))
    });
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
    use std::{env, fs, path::PathBuf};
    use tempfile::tempdir;

    struct CwdGuard {
        previous: PathBuf,
    }

    impl CwdGuard {
        fn enter(path: &std::path::Path) -> Self {
            let previous = env::current_dir().expect("current dir");
            env::set_current_dir(path).expect("set current dir");
            Self { previous }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = env::set_current_dir(&self.previous);
        }
    }

    #[test]
    fn try_snapshot_path_uses_canonical_location() {
        let temp = tempdir().unwrap();
        let _cwd = CwdGuard::enter(temp.path());
        fs::create_dir_all(".vizier/narrative").unwrap();
        let found = try_snapshot_path().expect("snapshot path");
        assert_eq!(found, PathBuf::from(".vizier/narrative/snapshot.md"));
    }

    #[test]
    fn snapshot_file_detection_is_canonical_only() {
        assert!(is_snapshot_file(".vizier/narrative/snapshot.md"));
        assert!(!is_snapshot_file(".vizier/.snapshot"));
    }
}
