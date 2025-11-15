use chrono::{DateTime, SecondsFormat, Utc};
use git2::{BranchType, Repository};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use vizier_core::vcs::branch_exists;

pub const PLAN_DIR: &str = ".vizier/implementation-plans";
const MAX_SUMMARY_LEN: usize = 160;

pub fn plan_rel_path(slug: &str) -> PathBuf {
    Path::new(PLAN_DIR).join(format!("{slug}.md"))
}

pub fn default_branch_for_slug(slug: &str) -> String {
    format!("draft/{slug}")
}

pub fn short_suffix() -> String {
    let raw = Uuid::new_v4().simple().to_string();
    raw[..8].to_string()
}

pub fn normalize_slug(input: &str) -> String {
    let mut normalized = String::new();
    let mut last_dash = false;

    for ch in input.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            normalized.push(lower);
            last_dash = false;
        } else if !last_dash {
            normalized.push('-');
            last_dash = true;
        }
    }

    while normalized.starts_with('-') {
        normalized.remove(0);
    }
    while normalized.ends_with('-') {
        normalized.pop();
    }

    if normalized.len() > 32 {
        normalized.truncate(32);
        while normalized.ends_with('-') {
            normalized.pop();
        }
    }

    normalized
}

pub fn slug_from_spec(spec: &str) -> String {
    let words: Vec<&str> = spec.split_whitespace().take(6).collect();
    let candidate = if words.is_empty() {
        "draft-plan".to_string()
    } else {
        words.join("-")
    };

    let normalized = normalize_slug(&candidate);
    if normalized.is_empty() {
        "draft-plan".to_string()
    } else {
        normalized
    }
}

pub fn sanitize_name_override(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("plan name cannot be empty".to_string());
    }
    if trimmed.starts_with('.') {
        return Err("plan name cannot start with '.'".to_string());
    }
    if trimmed.contains('/') {
        return Err("plan name cannot contain '/'".to_string());
    }
    let normalized = normalize_slug(trimmed);
    if normalized.is_empty() {
        Err("plan name must include letters or numbers".to_string())
    } else {
        Ok(normalized)
    }
}

pub fn ensure_unique_slug(
    base: &str,
    plan_dir: &Path,
    branch_prefix: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut attempts = 0usize;
    let mut slug = base.to_string();

    loop {
        let branch_name = format!("{branch_prefix}{slug}");
        let plan_path = plan_dir.join(format!("{slug}.md"));
        let branch_taken = branch_exists(&branch_name)
            .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
        if !branch_taken && !plan_path.exists() {
            return Ok(slug);
        }

        attempts += 1;
        if attempts <= 5 {
            slug = normalize_slug(&format!("{base}-{attempts}"));
            if slug.is_empty() {
                slug = format!("{base}-{attempts}");
                slug = normalize_slug(&slug);
            }
            continue;
        }

        slug = normalize_slug(&format!("{base}-{}", short_suffix()));
        if slug.is_empty() {
            slug = normalize_slug("draft-plan");
        }

        if attempts > 20 {
            return Err("unable to allocate a unique draft slug after multiple attempts".into());
        }
    }
}

pub fn trim_trailing_newlines(text: &str) -> &str {
    let trimmed = text.trim_end_matches(|c| c == '\n' || c == '\r');
    if trimmed.is_empty() { "" } else { trimmed }
}

pub fn render_plan_document(
    slug: &str,
    branch_name: &str,
    spec_source: &str,
    spec_text: &str,
    plan_body: &str,
) -> String {
    let mut doc = String::new();
    let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);

    doc.push_str("---\n");
    doc.push_str(&format!("plan: {slug}\n"));
    doc.push_str(&format!("branch: {branch_name}\n"));
    doc.push_str("status: draft\n");
    doc.push_str(&format!("created_at: {timestamp}\n"));
    doc.push_str(&format!("spec_source: {spec_source}\n"));
    doc.push_str("---\n\n");

    doc.push_str("## Operator Spec\n");
    let spec_section = trim_trailing_newlines(spec_text);
    if !spec_section.is_empty() {
        doc.push_str(spec_section);
    }
    doc.push('\n');
    doc.push('\n');

    doc.push_str("## Implementation Plan\n");
    let plan_section = plan_body.trim();
    if plan_section.is_empty() {
        doc.push_str("(plan generation returned empty content)");
    } else {
        doc.push_str(plan_section);
    }
    doc.push('\n');

    doc
}

pub fn write_plan_file(
    destination: &Path,
    contents: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let parent = destination
        .parent()
        .ok_or_else(|| "invalid plan path: missing parent directory".to_string())?;
    fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(contents.as_bytes())?;
    tmp.flush()?;
    tmp.persist(destination)?;
    Ok(())
}

#[derive(Debug)]
pub enum PlanError {
    Git(git2::Error),
    MissingFrontMatter,
    MissingField(&'static str),
    InvalidEncoding(std::str::Utf8Error),
    MissingPlanFile { branch: String, path: PathBuf },
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::Git(err) => write!(f, "{err}"),
            PlanError::MissingFrontMatter => write!(f, "plan file missing front-matter header"),
            PlanError::MissingField(field) => {
                write!(f, "plan file missing `{field}` in front-matter")
            }
            PlanError::InvalidEncoding(err) => write!(f, "plan file not valid UTF-8: {err}"),
            PlanError::MissingPlanFile { branch, path } => {
                write!(
                    f,
                    "branch {branch} does not contain plan document at {}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for PlanError {}

impl From<git2::Error> for PlanError {
    fn from(value: git2::Error) -> Self {
        PlanError::Git(value)
    }
}

impl From<std::str::Utf8Error> for PlanError {
    fn from(value: std::str::Utf8Error) -> Self {
        PlanError::InvalidEncoding(value)
    }
}

#[derive(Debug, Clone)]
pub struct PlanMetadata {
    pub slug: String,
    pub branch: String,
    pub status: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub created_at_raw: Option<String>,
    pub spec_source: Option<String>,
    pub spec_excerpt: Option<String>,
    pub spec_summary: Option<String>,
}

impl PlanMetadata {
    pub fn from_document(contents: &str) -> Result<Self, PlanError> {
        let (front_matter, body) = split_front_matter(contents)?;
        let fields = parse_front_matter(front_matter);

        let slug = fields
            .get("plan")
            .ok_or(PlanError::MissingField("plan"))?
            .to_string();
        let branch = fields
            .get("branch")
            .ok_or(PlanError::MissingField("branch"))?
            .to_string();

        let status = fields.get("status").cloned();
        let created_at_raw = fields.get("created_at").cloned();
        let created_at = created_at_raw
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|dt| dt.with_timezone(&Utc));
        let spec_source = fields.get("spec_source").cloned();

        let spec_excerpt = extract_section(body, "Operator Spec");
        let spec_summary = spec_excerpt.as_ref().and_then(|text| summarize_line(text));

        Ok(Self {
            slug,
            branch,
            status,
            created_at,
            created_at_raw,
            spec_source,
            spec_excerpt,
            spec_summary,
        })
    }

    pub fn created_at_display(&self) -> String {
        if let Some(ts) = &self.created_at {
            ts.to_rfc3339_opts(SecondsFormat::Secs, true)
        } else if let Some(raw) = &self.created_at_raw {
            raw.clone()
        } else {
            "unknown".to_string()
        }
    }
}

pub fn load_plan_from_branch(slug: &str, branch: &str) -> Result<PlanMetadata, PlanError> {
    let repo = Repository::discover(".")?;
    let plan_path = plan_rel_path(slug);
    let branch_ref = repo.find_branch(branch, BranchType::Local)?;
    let commit = branch_ref.into_reference().peel_to_commit()?;
    let tree = commit.tree()?;

    let entry = tree
        .get_path(&plan_path)
        .map_err(|_| PlanError::MissingPlanFile {
            branch: branch.to_string(),
            path: plan_path.clone(),
        })?;
    let blob = repo.find_blob(entry.id())?;
    let contents = std::str::from_utf8(blob.content())?.to_string();
    PlanMetadata::from_document(&contents)
}

pub fn summarize_spec(meta: &PlanMetadata) -> String {
    meta.spec_summary
        .clone()
        .or_else(|| meta.spec_excerpt.clone())
        .map(|text| clip_summary(&text))
        .unwrap_or_else(|| format!("Plan {} lacks an operator spec summary", meta.slug))
}

fn clip_summary(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.len() <= MAX_SUMMARY_LEN {
        return trimmed.to_string();
    }

    let mut out = String::new();
    for ch in trimmed.chars() {
        if out.chars().count() >= MAX_SUMMARY_LEN - 1 {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn split_front_matter(document: &str) -> Result<(&str, &str), PlanError> {
    let start_offset = if document.starts_with("---\r\n") {
        5
    } else if document.starts_with("---\n") {
        4
    } else {
        return Err(PlanError::MissingFrontMatter);
    };

    let candidates = ["\n---\n", "\n---\r\n", "\r\n---\n", "\r\n---\r\n"];
    for pattern in candidates {
        if let Some(idx) = document.find(pattern) {
            let front = &document[start_offset..idx];
            let body = &document[idx + pattern.len()..];
            return Ok((front, body));
        }
    }

    Err(PlanError::MissingFrontMatter)
}

fn parse_front_matter(front: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    for line in front.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            fields.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    fields
}

fn extract_section(document: &str, header: &str) -> Option<String> {
    let needle = format!("## {header}");
    let start = document.find(&needle)?;
    let after_header = &document[start + needle.len()..];
    let mut lines = after_header.lines();

    let mut collected = Vec::new();
    while let Some(line) = lines.next() {
        if line.starts_with("## ") {
            break;
        }
        if collected.is_empty() && line.trim().is_empty() {
            continue;
        }
        collected.push(line);
    }

    let section = collected.join("\n").trim().to_string();
    if section.is_empty() {
        None
    } else {
        Some(section)
    }
}

fn summarize_line(text: &str) -> Option<String> {
    let first_line = text
        .lines()
        .map(|line| line.trim())
        .find(|line| !line.is_empty())?;
    Some(clip_summary(first_line))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn slug_from_spec_limits_to_six_words() {
        let slug = slug_from_spec("One two THREE four five six seven eight");
        assert_eq!(slug, "one-two-three-four-five-six");
    }

    #[test]
    fn sanitize_override_rejects_invalid_prefixes() {
        assert!(sanitize_name_override(".hidden").is_err());
        assert!(sanitize_name_override("feature/branch").is_err());
        assert_eq!(
            sanitize_name_override("My Draft Name").unwrap(),
            "my-draft-name"
        );
    }

    #[test]
    fn render_plan_document_includes_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let doc = render_plan_document(
            "alpha",
            "draft/alpha",
            "inline",
            "spec body",
            "## Execution Plan\n- step",
        );
        assert!(doc.contains("plan: alpha"));
        assert!(doc.contains("branch: draft/alpha"));
        assert!(doc.contains("## Operator Spec"));
        Ok(())
    }

    #[test]
    fn ensure_unique_slug_skips_existing() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempdir()?;
        let plan_dir = dir.path().join("plans");
        fs::create_dir_all(&plan_dir)?;
        fs::write(plan_dir.join("demo.md"), "placeholder")?;
        assert!(plan_dir.join("demo.md").exists());
        let slug = ensure_unique_slug("demo", &plan_dir, "draft/")?;
        assert_ne!(slug, "demo");
        Ok(())
    }

    #[test]
    fn parse_metadata_extracts_summary() -> Result<(), Box<dyn std::error::Error>> {
        let doc = r#"---
plan: alpha
branch: draft/alpha
created_at: 2024-07-01T12:34:56Z
spec_source: inline
---

## Operator Spec
Add streaming UI with guard rails.

## Implementation Plan
- Step 1
- Step 2
"#;
        let meta = PlanMetadata::from_document(doc)?;
        assert_eq!(meta.slug, "alpha");
        assert_eq!(meta.branch, "draft/alpha");
        assert_eq!(meta.spec_source.as_deref(), Some("inline"));
        assert_eq!(
            meta.spec_summary.as_deref(),
            Some("Add streaming UI with guard rails.")
        );
        Ok(())
    }

    #[test]
    fn summarize_spec_falls_back_to_excerpt() {
        let meta = PlanMetadata {
            slug: "alpha".into(),
            branch: "draft/alpha".into(),
            status: None,
            created_at: None,
            created_at_raw: None,
            spec_source: None,
            spec_excerpt: Some("Line one\nLine two".into()),
            spec_summary: None,
        };

        assert_eq!(summarize_spec(&meta), "Line one\nLine two".to_string());
    }
}
