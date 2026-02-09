use std::collections::HashSet;
use std::path::Path;

use vizier_core::tools;

const SNAPSHOT_STARTER: &str = "\
# Running Snapshot

Narrative theme
- TODO

Code state (behaviors that matter)
- TODO
";

const GLOSSARY_STARTER: &str = "\
# Glossary

- Add high-signal terms here.
";

#[derive(Debug, Clone)]
struct DurableMarker {
    relative_path: String,
    starter_template: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InitEvaluation {
    missing_markers: Vec<String>,
    missing_ignore_rules: Vec<String>,
}

impl InitEvaluation {
    fn durable_initialized(&self) -> bool {
        self.missing_markers.is_empty()
    }

    fn contract_satisfied(&self) -> bool {
        self.durable_initialized() && self.missing_ignore_rules.is_empty()
    }

    fn missing_items(&self) -> Vec<String> {
        let mut items = Vec::new();
        items.extend(self.missing_markers.iter().cloned());
        items.extend(
            self.missing_ignore_rules
                .iter()
                .map(|rule| format!(".gitignore: {rule}")),
        );
        items
    }
}

pub(crate) fn run_init(repo_root: &Path, check: bool) -> Result<(), Box<dyn std::error::Error>> {
    let before = evaluate_init_state(repo_root)?;

    if check {
        if before.contract_satisfied() {
            println!("Outcome: vizier init check: satisfied");
            return Ok(());
        }

        println!("Outcome: vizier init check: missing required items");
        for missing in before.missing_items() {
            println!("missing: {missing}");
        }
        return Err("vizier init --check failed".into());
    }

    apply_initialization(repo_root)?;
    let after = evaluate_init_state(repo_root)?;

    if !after.contract_satisfied() {
        return Err("vizier init failed to satisfy initialization contract".into());
    }

    if before.contract_satisfied() {
        println!("Outcome: vizier init already satisfied");
    } else {
        println!("Outcome: vizier init newly initialized");
    }

    Ok(())
}

fn apply_initialization(repo_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let vizier_dir = repo_root.join(tools::VIZIER_DIR.trim_end_matches('/'));
    std::fs::create_dir_all(&vizier_dir)
        .map_err(|err| io_error("create directory", &vizier_dir, err))?;

    let narrative_dir = repo_root.join(tools::VIZIER_DIR).join(tools::NARRATIVE_DIR);
    std::fs::create_dir_all(&narrative_dir)
        .map_err(|err| io_error("create directory", &narrative_dir, err))?;

    for marker in durable_markers() {
        let marker_path = repo_root.join(&marker.relative_path);
        if marker_path.is_file() {
            continue;
        }
        if marker_path.exists() {
            return Err(format!(
                "failed to create marker {}: path exists and is not a file",
                marker_path.display()
            )
            .into());
        }
        if let Some(parent) = marker_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| io_error("create directory", parent, err))?;
        }
        std::fs::write(&marker_path, marker.starter_template)
            .map_err(|err| io_error("write file", &marker_path, err))?;
    }

    ensure_gitignore_rules(repo_root)?;
    Ok(())
}

fn evaluate_init_state(repo_root: &Path) -> Result<InitEvaluation, Box<dyn std::error::Error>> {
    let mut missing_markers = Vec::new();
    for marker in durable_markers() {
        let marker_path = repo_root.join(&marker.relative_path);
        if !marker_path.is_file() {
            missing_markers.push(marker.relative_path);
        }
    }

    let required_rules = required_ignore_rules();
    let gitignore_path = repo_root.join(".gitignore");
    let gitignore_contents = read_text_lossy(&gitignore_path)?;
    let missing_ignore_rules = missing_ignore_rules(&gitignore_contents, &required_rules);

    Ok(InitEvaluation {
        missing_markers,
        missing_ignore_rules,
    })
}

fn ensure_gitignore_rules(repo_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let required_rules = required_ignore_rules();
    let gitignore_path = repo_root.join(".gitignore");
    let existing = read_text_lossy(&gitignore_path)?;
    let missing = missing_ignore_rules(&existing, &required_rules);
    if missing.is_empty() {
        return Ok(());
    }

    let updated = append_missing_rules(&existing, &missing);
    std::fs::write(&gitignore_path, updated)
        .map_err(|err| io_error("write file", &gitignore_path, err))?;
    Ok(())
}

fn read_text_lossy(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(io_error("read file", path, err)),
    }
}

fn durable_markers() -> Vec<DurableMarker> {
    vec![
        DurableMarker {
            relative_path: format!(
                "{}{}{}",
                tools::VIZIER_DIR,
                tools::NARRATIVE_DIR,
                tools::SNAPSHOT_FILE
            ),
            starter_template: SNAPSHOT_STARTER,
        },
        DurableMarker {
            relative_path: format!(
                "{}{}{}",
                tools::VIZIER_DIR,
                tools::NARRATIVE_DIR,
                tools::GLOSSARY_FILE
            ),
            starter_template: GLOSSARY_STARTER,
        },
    ]
}

fn required_ignore_rules() -> Vec<String> {
    vec![
        format!("{}tmp/", tools::VIZIER_DIR),
        format!("{}tmp-worktrees/", tools::VIZIER_DIR),
        format!("{}jobs/", tools::VIZIER_DIR),
        format!("{}sessions/", tools::VIZIER_DIR),
    ]
}

fn missing_ignore_rules(existing_contents: &str, required_rules: &[String]) -> Vec<String> {
    let existing = normalized_ignore_targets(existing_contents);
    required_rules
        .iter()
        .filter_map(|rule| {
            let normalized = normalize_ignore_pattern(rule)?;
            if existing.contains(&normalized) {
                None
            } else {
                Some(rule.clone())
            }
        })
        .collect()
}

fn normalized_ignore_targets(contents: &str) -> HashSet<String> {
    contents
        .lines()
        .filter_map(normalize_ignore_pattern)
        .collect()
}

fn normalize_ignore_pattern(raw_line: &str) -> Option<String> {
    let mut value = raw_line.trim();
    if value.is_empty() || value.starts_with('#') || value.starts_with('!') {
        return None;
    }
    if let Some((before_comment, _)) = value.split_once(" #") {
        value = before_comment.trim_end();
    }

    let mut normalized = value.replace('\\', "/");
    while normalized.starts_with("./") {
        normalized = normalized.trim_start_matches("./").to_string();
    }
    while normalized.starts_with('/') {
        normalized.remove(0);
    }
    while normalized.starts_with("**/") {
        normalized = normalized.trim_start_matches("**/").to_string();
    }

    for suffix in ["/**/*", "/**", "/*"] {
        if normalized.ends_with(suffix) {
            let len = normalized.len() - suffix.len();
            normalized.truncate(len);
            break;
        }
    }
    while normalized.ends_with('/') {
        normalized.pop();
    }
    while normalized.contains("//") {
        normalized = normalized.replace("//", "/");
    }

    if normalized.is_empty() {
        return None;
    }
    Some(normalized)
}

fn detect_line_ending(contents: &str) -> &'static str {
    if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn append_missing_rules(existing_contents: &str, missing_rules: &[String]) -> String {
    if missing_rules.is_empty() {
        return existing_contents.to_string();
    }

    let line_ending = detect_line_ending(existing_contents);
    let mut updated = existing_contents.to_string();
    if !updated.is_empty() && !updated.ends_with('\n') && !updated.ends_with('\r') {
        updated.push_str(line_ending);
    }
    for rule in missing_rules {
        updated.push_str(rule);
        updated.push_str(line_ending);
    }
    updated
}

fn io_error(action: &str, path: &Path, err: std::io::Error) -> Box<dyn std::error::Error> {
    format!("failed to {action} {}: {err}", path.display()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_ignore_pattern_accepts_equivalent_forms() {
        assert_eq!(
            normalize_ignore_pattern("./.vizier/tmp-worktrees/**"),
            Some(".vizier/tmp-worktrees".to_string())
        );
        assert_eq!(
            normalize_ignore_pattern("/.vizier/jobs/*"),
            Some(".vizier/jobs".to_string())
        );
        assert_eq!(
            normalize_ignore_pattern("**/.vizier/sessions/"),
            Some(".vizier/sessions".to_string())
        );
        assert_eq!(normalize_ignore_pattern("!/.vizier/tmp/"), None);
        assert_eq!(normalize_ignore_pattern("# comment"), None);
    }

    #[test]
    fn missing_ignore_rules_treats_equivalent_patterns_as_covered() {
        let existing = "\
/.vizier/tmp/
./.vizier/tmp-worktrees/**
.vizier/jobs/*
**/.vizier/sessions/
";
        let missing = missing_ignore_rules(existing, &required_ignore_rules());
        assert!(
            missing.is_empty(),
            "expected all required rules covered by equivalent patterns: {missing:?}"
        );
    }

    #[test]
    fn append_missing_rules_preserves_crlf() {
        let existing = "target/\r\n";
        let missing = vec![".vizier/tmp/".to_string(), ".vizier/jobs/".to_string()];
        let updated = append_missing_rules(existing, &missing);
        assert_eq!(
            updated,
            "target/\r\n.vizier/tmp/\r\n.vizier/jobs/\r\n".to_string()
        );
    }

    #[test]
    fn append_missing_rules_adds_separator_when_needed() {
        let existing = "target/";
        let missing = vec![".vizier/tmp/".to_string()];
        let updated = append_missing_rules(existing, &missing);
        assert_eq!(updated, "target/\n.vizier/tmp/\n".to_string());
    }

    #[test]
    fn evaluate_init_state_requires_markers_for_durable_initialized() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let repo_root = temp.path();
        let markers = durable_markers();

        std::fs::create_dir_all(repo_root.join(".vizier/narrative")).expect("create narrative dir");
        std::fs::write(repo_root.join(".gitignore"), ".vizier/tmp/\n").expect("write gitignore");

        std::fs::write(repo_root.join(&markers[0].relative_path), "snapshot\n")
            .expect("write snapshot marker");
        std::fs::write(repo_root.join(&markers[1].relative_path), "glossary\n")
            .expect("write glossary marker");

        let evaluation = evaluate_init_state(repo_root).expect("evaluate init state");
        assert!(
            evaluation.durable_initialized(),
            "durable init should require only marker files"
        );
        assert!(
            !evaluation.contract_satisfied(),
            "contract should still fail when ignore rules are missing"
        );
    }
}
