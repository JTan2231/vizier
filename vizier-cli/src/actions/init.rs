use std::collections::{HashMap, HashSet};
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
const WORKFLOW_DEVELOP_STARTER: &str = include_str!("../../templates/init/develop.hcl");
const WORKFLOW_DRAFT_STARTER: &str = include_str!("../../templates/init/workflows/draft.hcl");
const WORKFLOW_APPROVE_STARTER: &str = include_str!("../../templates/init/workflows/approve.hcl");
const WORKFLOW_MERGE_STARTER: &str = include_str!("../../templates/init/workflows/merge.hcl");
const WORKFLOW_COMMIT_STARTER: &str = include_str!("../../templates/init/workflows/commit.hcl");
const PROMPT_DRAFT_STARTER: &str = include_str!("../../templates/init/prompts/DRAFT_PROMPTS.md");
const PROMPT_APPROVE_STARTER: &str =
    include_str!("../../templates/init/prompts/APPROVE_PROMPTS.md");
const PROMPT_MERGE_STARTER: &str = include_str!("../../templates/init/prompts/MERGE_PROMPTS.md");
const PROMPT_COMMIT_STARTER: &str = include_str!("../../templates/init/prompts/COMMIT_PROMPTS.md");
const CI_SCRIPT_STARTER: &str = include_str!("../../templates/init/ci.sh");
const VIZIER_GITIGNORE_HEADING: &str = "# Vizier";
const CANONICAL_VIZIER_GITIGNORE_ITEM: &str = ".gitignore: canonical # Vizier block";

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
    gitignore_needs_canonicalization: bool,
}

impl InitEvaluation {
    fn contract_satisfied(&self) -> bool {
        self.missing_files.is_empty()
            && self.missing_ignore_rules.is_empty()
            && !self.gitignore_needs_canonicalization
    }

    fn missing_items(&self) -> Vec<String> {
        let mut items = Vec::new();
        items.extend(self.missing_files.iter().cloned());
        items.extend(
            self.missing_ignore_rules
                .iter()
                .map(|rule| format!(".gitignore: {rule}")),
        );
        if self.gitignore_needs_canonicalization {
            items.push(CANONICAL_VIZIER_GITIGNORE_ITEM.to_string());
        }
        items
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitignoreEvaluation {
    missing_ignore_rules: Vec<String>,
    needs_canonicalization: bool,
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
    let gitignore_evaluation = evaluate_gitignore_contents(&gitignore_contents, &required_rules);

    Ok(InitEvaluation {
        missing_files,
        missing_ignore_rules: gitignore_evaluation.missing_ignore_rules,
        gitignore_needs_canonicalization: gitignore_evaluation.needs_canonicalization,
    })
}

fn ensure_gitignore_rules(repo_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let required_rules = required_ignore_rules();
    let gitignore_path = repo_root.join(".gitignore");
    let existing = read_text_lossy(&gitignore_path)?;
    let gitignore_evaluation = evaluate_gitignore_contents(&existing, &required_rules);
    if gitignore_evaluation.missing_ignore_rules.is_empty()
        && !gitignore_evaluation.needs_canonicalization
    {
        return Ok(());
    }

    let updated = rewrite_vizier_gitignore_block(&existing, &required_rules);
    std::fs::write(&gitignore_path, updated)
        .map_err(|err| io_error("write file", &gitignore_path, err))?;
    Ok(())
}

fn evaluate_gitignore_contents(
    existing_contents: &str,
    required_rules: &[String],
) -> GitignoreEvaluation {
    let missing_ignore_rules = missing_ignore_rules(existing_contents, required_rules);
    let needs_canonicalization = missing_ignore_rules.is_empty()
        && gitignore_needs_canonicalization(existing_contents, required_rules);

    GitignoreEvaluation {
        missing_ignore_rules,
        needs_canonicalization,
    }
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
            relative_path: format!("{}develop.hcl", tools::VIZIER_DIR),
            starter_template: WORKFLOW_DEVELOP_STARTER,
            executable: false,
        },
        RequiredFile {
            relative_path: format!("{}workflows/draft.hcl", tools::VIZIER_DIR),
            starter_template: WORKFLOW_DRAFT_STARTER,
            executable: false,
        },
        RequiredFile {
            relative_path: format!("{}workflows/approve.hcl", tools::VIZIER_DIR),
            starter_template: WORKFLOW_APPROVE_STARTER,
            executable: false,
        },
        RequiredFile {
            relative_path: format!("{}workflows/merge.hcl", tools::VIZIER_DIR),
            starter_template: WORKFLOW_MERGE_STARTER,
            executable: false,
        },
        RequiredFile {
            relative_path: format!("{}workflows/commit.hcl", tools::VIZIER_DIR),
            starter_template: WORKFLOW_COMMIT_STARTER,
            executable: false,
        },
        RequiredFile {
            relative_path: format!("{}prompts/DRAFT_PROMPTS.md", tools::VIZIER_DIR),
            starter_template: PROMPT_DRAFT_STARTER,
            executable: false,
        },
        RequiredFile {
            relative_path: format!("{}prompts/APPROVE_PROMPTS.md", tools::VIZIER_DIR),
            starter_template: PROMPT_APPROVE_STARTER,
            executable: false,
        },
        RequiredFile {
            relative_path: format!("{}prompts/MERGE_PROMPTS.md", tools::VIZIER_DIR),
            starter_template: PROMPT_MERGE_STARTER,
            executable: false,
        },
        RequiredFile {
            relative_path: format!("{}prompts/COMMIT_PROMPTS.md", tools::VIZIER_DIR),
            starter_template: PROMPT_COMMIT_STARTER,
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
        format!("{}state/", tools::VIZIER_DIR),
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
    let value = raw_line.trim();
    if value.is_empty() || value.starts_with('#') || value.starts_with('!') {
        return None;
    }
    normalize_gitignore_pattern_body(value)
}

fn normalize_unignore_pattern(raw_line: &str) -> Option<String> {
    let value = raw_line.trim();
    if value.is_empty() || value.starts_with('#') {
        return None;
    }
    let remainder = value.strip_prefix('!')?;
    normalize_gitignore_pattern_body(remainder)
}

fn normalize_gitignore_pattern_body(raw_pattern: &str) -> Option<String> {
    let mut value = raw_pattern.trim();
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

fn canonical_vizier_gitignore_block(line_ending: &str, required_rules: &[String]) -> String {
    let mut block = String::new();
    block.push_str(VIZIER_GITIGNORE_HEADING);
    block.push_str(line_ending);
    for rule in required_rules {
        block.push_str(rule);
        block.push_str(line_ending);
    }
    block
}

fn rewrite_vizier_gitignore_block(existing_contents: &str, required_rules: &[String]) -> String {
    let line_ending = detect_line_ending(existing_contents);
    let managed_targets = managed_ignore_targets(required_rules);
    let mut preserved_lines = Vec::new();
    let mut managed_unignore_lines = Vec::new();
    for line in existing_contents.lines() {
        if is_vizier_gitignore_heading(line) || is_managed_ignore_rule(line, &managed_targets) {
            continue;
        }
        if is_managed_unignore_rule(line, &managed_targets) {
            managed_unignore_lines.push(line.to_string());
            continue;
        }
        preserved_lines.push(line.to_string());
    }

    while matches!(preserved_lines.last(), Some(line) if line.trim().is_empty()) {
        preserved_lines.pop();
    }

    let mut updated = preserved_lines.join(line_ending);
    if !updated.is_empty() {
        updated.push_str(line_ending);
        updated.push_str(line_ending);
    }
    updated.push_str(&canonical_vizier_gitignore_block(
        line_ending,
        required_rules,
    ));
    for line in managed_unignore_lines {
        updated.push_str(&line);
        updated.push_str(line_ending);
    }
    updated
}

fn is_vizier_gitignore_heading(raw_line: &str) -> bool {
    raw_line.trim_start().starts_with(VIZIER_GITIGNORE_HEADING)
}

fn count_vizier_gitignore_headings(contents: &str) -> usize {
    contents
        .lines()
        .filter(|line| is_vizier_gitignore_heading(line))
        .count()
}

fn is_managed_ignore_rule(raw_line: &str, managed_targets: &HashSet<String>) -> bool {
    match normalize_ignore_pattern(raw_line) {
        Some(pattern) => managed_targets.contains(&pattern),
        None => false,
    }
}

fn is_managed_unignore_rule(raw_line: &str, managed_targets: &HashSet<String>) -> bool {
    match normalize_unignore_pattern(raw_line) {
        Some(pattern) => targets_managed_root(&pattern, managed_targets),
        None => false,
    }
}

fn targets_managed_root(pattern: &str, managed_targets: &HashSet<String>) -> bool {
    managed_targets.iter().any(|managed_target| {
        pattern == managed_target
            || pattern
                .strip_prefix(managed_target)
                .is_some_and(|suffix| suffix.starts_with('/'))
    })
}

fn managed_ignore_targets(required_rules: &[String]) -> HashSet<String> {
    required_rules
        .iter()
        .filter_map(|rule| normalize_ignore_pattern(rule))
        .collect()
}

fn managed_ignore_rule_counts(contents: &str, required_rules: &[String]) -> HashMap<String, usize> {
    let managed_targets = managed_ignore_targets(required_rules);
    let mut counts = HashMap::new();

    for pattern in contents.lines().filter_map(normalize_ignore_pattern) {
        if managed_targets.contains(&pattern) {
            *counts.entry(pattern).or_insert(0) += 1;
        }
    }

    counts
}

fn has_canonical_vizier_gitignore_block(
    existing_contents: &str,
    required_rules: &[String],
) -> bool {
    let line_ending = detect_line_ending(existing_contents);
    existing_contents.contains(&canonical_vizier_gitignore_block(
        line_ending,
        required_rules,
    ))
}

fn has_exactly_one_copy_of_each_managed_rule(
    existing_contents: &str,
    required_rules: &[String],
) -> bool {
    let counts = managed_ignore_rule_counts(existing_contents, required_rules);
    required_rules
        .iter()
        .filter_map(|rule| normalize_ignore_pattern(rule))
        .all(|rule| counts.get(&rule) == Some(&1))
}

fn managed_unignore_rules_precede_canonical_block(
    existing_contents: &str,
    required_rules: &[String],
) -> bool {
    let line_ending = detect_line_ending(existing_contents);
    let canonical_block = canonical_vizier_gitignore_block(line_ending, required_rules);
    let Some(block_start) = existing_contents.find(&canonical_block) else {
        return false;
    };
    let managed_targets = managed_ignore_targets(required_rules);
    existing_contents[..block_start]
        .lines()
        .any(|line| is_managed_unignore_rule(line, &managed_targets))
}

fn gitignore_needs_canonicalization(existing_contents: &str, required_rules: &[String]) -> bool {
    count_vizier_gitignore_headings(existing_contents) != 1
        || !has_canonical_vizier_gitignore_block(existing_contents, required_rules)
        || !has_exactly_one_copy_of_each_managed_rule(existing_contents, required_rules)
        || managed_unignore_rules_precede_canonical_block(existing_contents, required_rules)
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
    fn normalize_unignore_pattern_accepts_equivalent_forms() {
        assert_eq!(
            normalize_unignore_pattern("!./.vizier/jobs/**"),
            Some(".vizier/jobs".to_string())
        );
        assert_eq!(
            normalize_unignore_pattern("!/.vizier/jobs/keep.json"),
            Some(".vizier/jobs/keep.json".to_string())
        );
        assert_eq!(
            normalize_unignore_pattern("!**/.vizier/implementation-plans/keep.md"),
            Some(".vizier/implementation-plans/keep.md".to_string())
        );
        assert_eq!(normalize_unignore_pattern(".vizier/jobs/"), None);
        assert_eq!(normalize_unignore_pattern("# comment"), None);
    }

    #[test]
    fn is_managed_unignore_rule_matches_roots_and_descendants_only() {
        let managed_targets = managed_ignore_targets(&required_ignore_rules());
        assert!(is_managed_unignore_rule("!.vizier/jobs/", &managed_targets,));
        assert!(is_managed_unignore_rule(
            "!/.vizier/jobs/keep.json",
            &managed_targets,
        ));
        assert!(is_managed_unignore_rule(
            "!**/.vizier/implementation-plans/keep.md",
            &managed_targets,
        ));
        assert!(!is_managed_unignore_rule(
            "!/.vizier-state/jobs/keep.json",
            &managed_targets,
        ));
        assert!(!is_managed_unignore_rule("!/target", &managed_targets));
    }

    #[test]
    fn missing_ignore_rules_treats_equivalent_patterns_as_covered() {
        let existing = "\
/.vizier/tmp/
./.vizier/tmp-worktrees/**
.vizier/jobs/*
**/.vizier/sessions/
.vizier/state/**
.vizier/implementation-plans/**
";
        let missing = missing_ignore_rules(existing, &required_ignore_rules());
        assert!(
            missing.is_empty(),
            "expected all required rules covered by equivalent patterns: {missing:?}"
        );
    }

    #[test]
    fn rewrite_vizier_gitignore_block_preserves_crlf() {
        let existing = "target/\r\n# Vizier test state\r\n.vizier/tmp/\r\n";
        let updated = rewrite_vizier_gitignore_block(existing, &required_ignore_rules());
        assert_eq!(
            updated,
            "target/\r\n\r\n# Vizier\r\n.vizier/tmp-worktrees/\r\n.vizier/tmp/\r\n.vizier/sessions/\r\n.vizier/jobs/\r\n.vizier/state/\r\n.vizier/implementation-plans\r\n"
                .to_string()
        );
    }

    #[test]
    fn rewrite_vizier_gitignore_block_moves_managed_unignore_rules_after_canonical_block() {
        let existing = "\
target/
!.vizier/jobs/
!/.vizier/jobs/keep.json
# keep working tree docs visible
!docs/dev/architecture/kernel.md
.vizier/jobs/
";
        let updated = rewrite_vizier_gitignore_block(existing, &required_ignore_rules());
        assert_eq!(
            updated,
            "target/\n# keep working tree docs visible\n!docs/dev/architecture/kernel.md\n\n# Vizier\n.vizier/tmp-worktrees/\n.vizier/tmp/\n.vizier/sessions/\n.vizier/jobs/\n.vizier/state/\n.vizier/implementation-plans\n!.vizier/jobs/\n!/.vizier/jobs/keep.json\n"
                .to_string()
        );
    }

    #[test]
    fn rewrite_vizier_gitignore_block_preserves_crlf_for_managed_unignore_rules() {
        let existing = "target/\r\n!.vizier/jobs/\r\n!/.vizier/jobs/keep.json\r\n.vizier/jobs/\r\n";
        let updated = rewrite_vizier_gitignore_block(existing, &required_ignore_rules());
        assert_eq!(
            updated,
            "target/\r\n\r\n# Vizier\r\n.vizier/tmp-worktrees/\r\n.vizier/tmp/\r\n.vizier/sessions/\r\n.vizier/jobs/\r\n.vizier/state/\r\n.vizier/implementation-plans\r\n!.vizier/jobs/\r\n!/.vizier/jobs/keep.json\r\n"
                .to_string()
        );
    }

    #[test]
    fn rewrite_vizier_gitignore_block_groups_rules_under_heading() {
        let existing = "\
target/

# Vizier test state
./.vizier/tmp-worktrees/**
/.vizier/tmp/
**/.vizier/sessions/
.vizier/jobs/*
.vizier/implementation-plans/**
";
        let updated = rewrite_vizier_gitignore_block(existing, &required_ignore_rules());
        assert_eq!(
            updated,
            "target/\n\n# Vizier\n.vizier/tmp-worktrees/\n.vizier/tmp/\n.vizier/sessions/\n.vizier/jobs/\n.vizier/state/\n.vizier/implementation-plans\n"
                .to_string()
        );
    }

    #[test]
    fn evaluate_gitignore_contents_flags_rule_complete_legacy_block_for_canonicalization() {
        let existing = "\
target/

.vizier/tmp-worktrees/
.vizier/tmp/
.vizier/sessions/
.vizier/jobs/
.vizier/state/
.vizier/implementation-plans
";
        let evaluation = evaluate_gitignore_contents(existing, &required_ignore_rules());
        assert!(
            evaluation.missing_ignore_rules.is_empty(),
            "legacy block should remain rule-complete: {:?}",
            evaluation.missing_ignore_rules
        );
        assert!(
            evaluation.needs_canonicalization,
            "rule-complete legacy block should still require canonicalization"
        );
    }

    #[test]
    fn evaluate_gitignore_contents_accepts_canonical_headed_block() {
        let existing = "\
target/

# Vizier
.vizier/tmp-worktrees/
.vizier/tmp/
.vizier/sessions/
.vizier/jobs/
.vizier/state/
.vizier/implementation-plans
";
        let evaluation = evaluate_gitignore_contents(existing, &required_ignore_rules());
        assert!(
            evaluation.missing_ignore_rules.is_empty(),
            "canonical block should cover all required rules: {:?}",
            evaluation.missing_ignore_rules
        );
        assert!(
            !evaluation.needs_canonicalization,
            "canonical headed block should satisfy gitignore shape"
        );
    }

    #[test]
    fn evaluate_gitignore_contents_flags_managed_unignore_rules_before_canonical_block() {
        let existing = "\
target/
!.vizier/jobs/
!/.vizier/jobs/keep.json

# Vizier
.vizier/tmp-worktrees/
.vizier/tmp/
.vizier/sessions/
.vizier/jobs/
.vizier/state/
.vizier/implementation-plans
";
        let evaluation = evaluate_gitignore_contents(existing, &required_ignore_rules());
        assert!(
            evaluation.missing_ignore_rules.is_empty(),
            "managed unignore ordering should not count as missing rules: {:?}",
            evaluation.missing_ignore_rules
        );
        assert!(
            evaluation.needs_canonicalization,
            "managed unignore rules before the canonical block should require canonicalization"
        );
    }

    #[test]
    fn evaluate_gitignore_contents_accepts_canonical_block_with_managed_unignore_rules_after_it() {
        let existing = "\
target/

# Vizier
.vizier/tmp-worktrees/
.vizier/tmp/
.vizier/sessions/
.vizier/jobs/
.vizier/state/
.vizier/implementation-plans
!.vizier/jobs/
!/.vizier/jobs/keep.json
";
        let evaluation = evaluate_gitignore_contents(existing, &required_ignore_rules());
        assert!(
            evaluation.missing_ignore_rules.is_empty(),
            "canonical block plus preserved managed unignores should stay rule-complete: {:?}",
            evaluation.missing_ignore_rules
        );
        assert!(
            !evaluation.needs_canonicalization,
            "managed unignore rules after the canonical block should satisfy gitignore shape"
        );
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
        assert!(
            evaluation
                .missing_files
                .iter()
                .any(|path| path == ".vizier/prompts/DRAFT_PROMPTS.md"),
            "expected missing .vizier/prompts/DRAFT_PROMPTS.md in evaluation: {:?}",
            evaluation.missing_files
        );
        assert!(
            !evaluation.gitignore_needs_canonicalization,
            "missing-rule evaluation should not also claim canonicalization: {evaluation:?}"
        );
    }

    #[test]
    fn evaluate_init_state_detects_heading_only_gitignore_migration_need() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let repo_root = temp.path();

        apply_initialization(repo_root).expect("apply initialization");
        std::fs::write(
            repo_root.join(".gitignore"),
            "\
target/

.vizier/tmp-worktrees/
.vizier/tmp/
.vizier/sessions/
.vizier/jobs/
.vizier/state/
.vizier/implementation-plans
",
        )
        .expect("write legacy gitignore");

        let evaluation = evaluate_init_state(repo_root).expect("evaluate init state");
        assert!(
            evaluation.missing_files.is_empty(),
            "expected no missing required files: {:?}",
            evaluation.missing_files
        );
        assert!(
            evaluation.missing_ignore_rules.is_empty(),
            "expected no missing ignore rules: {:?}",
            evaluation.missing_ignore_rules
        );
        assert!(
            evaluation.gitignore_needs_canonicalization,
            "rule-complete legacy block should still require canonicalization"
        );
        assert!(
            !evaluation.contract_satisfied(),
            "heading-only migration need should keep init unsatisfied"
        );
    }
}
