use std::collections::HashSet;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
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

const CONFIG_STARTER: &str = include_str!("../../templates/init/config.toml");
const WORKFLOW_DRAFT_STARTER: &str = include_str!("../../templates/init/workflows/draft.toml");
const WORKFLOW_APPROVE_STARTER: &str = include_str!("../../templates/init/workflows/approve.toml");
const WORKFLOW_MERGE_STARTER: &str = include_str!("../../templates/init/workflows/merge.toml");
const CI_SCRIPT_STARTER: &str = include_str!("../../templates/init/ci.sh");

#[derive(Debug, Clone)]
struct RequiredFile {
    relative_path: String,
    starter_template: &'static str,
    executable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InitEvaluation {
    missing_files: Vec<String>,
    missing_ignore_rules: Vec<String>,
}

impl InitEvaluation {
    fn contract_satisfied(&self) -> bool {
        self.missing_files.is_empty() && self.missing_ignore_rules.is_empty()
    }

    fn missing_items(&self) -> Vec<String> {
        let mut items = Vec::new();
        items.extend(self.missing_files.iter().cloned());
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

    for required_file in required_files() {
        let path = repo_root.join(&required_file.relative_path);
        if path.is_file() {
            continue;
        }
        if path.exists() {
            return Err(format!(
                "failed to create required file {}: path exists and is not a file",
                path.display()
            )
            .into());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| io_error("create directory", parent, err))?;
        }
        std::fs::write(&path, required_file.starter_template)
            .map_err(|err| io_error("write file", &path, err))?;
        set_executable_if_requested(&path, required_file.executable)?;
    }

    ensure_gitignore_rules(repo_root)?;
    Ok(())
}

fn evaluate_init_state(repo_root: &Path) -> Result<InitEvaluation, Box<dyn std::error::Error>> {
    let mut missing_files = Vec::new();
    for required_file in required_files() {
        let path = repo_root.join(&required_file.relative_path);
        if !path.is_file() {
            missing_files.push(required_file.relative_path);
        }
    }

    let required_rules = required_ignore_rules();
    let gitignore_path = repo_root.join(".gitignore");
    let gitignore_contents = read_text_lossy(&gitignore_path)?;
    let missing_ignore_rules = missing_ignore_rules(&gitignore_contents, &required_rules);

    Ok(InitEvaluation {
        missing_files,
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

fn required_files() -> Vec<RequiredFile> {
    vec![
        RequiredFile {
            relative_path: format!(
                "{}{}{}",
                tools::VIZIER_DIR,
                tools::NARRATIVE_DIR,
                tools::SNAPSHOT_FILE
            ),
            starter_template: SNAPSHOT_STARTER,
            executable: false,
        },
        RequiredFile {
            relative_path: format!(
                "{}{}{}",
                tools::VIZIER_DIR,
                tools::NARRATIVE_DIR,
                tools::GLOSSARY_FILE
            ),
            starter_template: GLOSSARY_STARTER,
            executable: false,
        },
        RequiredFile {
            relative_path: format!("{}config.toml", tools::VIZIER_DIR),
            starter_template: CONFIG_STARTER,
            executable: false,
        },
        RequiredFile {
            relative_path: format!("{}workflows/draft.toml", tools::VIZIER_DIR),
            starter_template: WORKFLOW_DRAFT_STARTER,
            executable: false,
        },
        RequiredFile {
            relative_path: format!("{}workflows/approve.toml", tools::VIZIER_DIR),
            starter_template: WORKFLOW_APPROVE_STARTER,
            executable: false,
        },
        RequiredFile {
            relative_path: format!("{}workflows/merge.toml", tools::VIZIER_DIR),
            starter_template: WORKFLOW_MERGE_STARTER,
            executable: false,
        },
        RequiredFile {
            relative_path: "ci.sh".to_string(),
            starter_template: CI_SCRIPT_STARTER,
            executable: true,
        },
    ]
}

fn required_ignore_rules() -> Vec<String> {
    vec![
        format!("{}tmp-worktrees/", tools::VIZIER_DIR),
        format!("{}tmp/", tools::VIZIER_DIR),
        format!("{}sessions/", tools::VIZIER_DIR),
        format!("{}jobs/", tools::VIZIER_DIR),
        format!("{}implementation-plans", tools::VIZIER_DIR),
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

fn set_executable_if_requested(
    path: &Path,
    executable: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if !executable {
        return Ok(());
    }

    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(path)
            .map_err(|err| io_error("read metadata", path, err))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)
            .map_err(|err| io_error("set file permissions", path, err))?;
    }

    #[cfg(not(unix))]
    {
        let _ = path;
    }

    Ok(())
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
.vizier/implementation-plans/**
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
    fn evaluate_init_state_detects_missing_required_files_and_ignore_rules() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let repo_root = temp.path();

        std::fs::create_dir_all(repo_root.join(".vizier/narrative")).expect("create narrative dir");
        std::fs::write(repo_root.join(".gitignore"), ".vizier/tmp/\n").expect("write gitignore");

        let evaluation = evaluate_init_state(repo_root).expect("evaluate init state");
        assert!(
            !evaluation.contract_satisfied(),
            "contract should fail when required files and ignore rules are missing"
        );
        assert!(
            evaluation
                .missing_files
                .iter()
                .any(|path| path == ".vizier/config.toml"),
            "expected missing .vizier/config.toml in evaluation: {:?}",
            evaluation.missing_files
        );
        assert!(
            evaluation
                .missing_ignore_rules
                .iter()
                .any(|rule| rule == ".vizier/implementation-plans"),
            "expected missing implementation-plans ignore rule in evaluation: {:?}",
            evaluation.missing_ignore_rules
        );
    }
}
