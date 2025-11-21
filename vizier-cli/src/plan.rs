use git2::{BranchType, Oid, Repository};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use vizier_core::vcs::{
    add_worktree_for_branch, branch_exists, detect_primary_branch, remove_worktree, repo_root,
};

pub const PLAN_DIR: &str = ".vizier/implementation-plans";
const MAX_SUMMARY_LEN: usize = 160;

pub fn plan_rel_path(slug: &str) -> PathBuf {
    Path::new(PLAN_DIR).join(format!("{slug}.md"))
}

pub fn default_branch_for_slug(slug: &str) -> String {
    format!("draft/{slug}")
}

#[derive(Debug, Clone)]
pub struct PlanBranchSpec {
    pub slug: String,
    pub branch: String,
    pub target_branch: String,
}

impl PlanBranchSpec {
    pub fn resolve(
        plan: Option<&str>,
        branch_override: Option<&str>,
        target_override: Option<&str>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let plan_name = plan.ok_or_else(|| "plan name is required".to_string())?;
        let slug = sanitize_name_override(plan_name).map_err(|err| {
            Box::<dyn std::error::Error>::from(io::Error::new(io::ErrorKind::InvalidInput, err))
        })?;
        let branch = branch_override
            .map(|value| value.to_string())
            .unwrap_or_else(|| default_branch_for_slug(&slug));
        let target_branch = if let Some(target) = target_override {
            target.to_string()
        } else {
            detect_primary_branch().ok_or_else(|| {
                Box::<dyn std::error::Error>::from(io::Error::new(
                    io::ErrorKind::NotFound,
                    "unable to detect primary branch; use --target",
                ))
            })?
        };

        Ok(Self {
            slug,
            branch,
            target_branch,
        })
    }

    pub fn plan_rel_path(&self) -> PathBuf {
        plan_rel_path(&self.slug)
    }

    pub fn load_metadata(&self) -> Result<PlanMetadata, PlanError> {
        load_plan_from_branch(&self.slug, &self.branch)
    }

    pub fn show_preview(&self, meta: &PlanMetadata) {
        println!("Plan: {}", meta.slug);
        println!("Branch: {}", self.branch);
        println!("Spec summary: {}", summarize_spec(meta));
    }

    pub fn diff_command(&self) -> String {
        format!("git diff {}...{}", self.target_branch, self.branch)
    }
}

pub struct PlanWorktree {
    pub name: String,
    pub path: PathBuf,
}

impl PlanWorktree {
    pub fn create(
        slug: &str,
        branch: &str,
        purpose: &str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let repo_root = repo_root()
            .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?
            .to_path_buf();
        let tmp_root = repo_root.join(".vizier/tmp-worktrees");
        fs::create_dir_all(&tmp_root)?;

        let suffix = short_suffix();
        let dir_name = format!("{slug}-{suffix}");
        let worktree_path = tmp_root.join(&dir_name);
        let worktree_name = format!("vizier-{purpose}-{dir_name}");

        add_worktree_for_branch(&worktree_name, &worktree_path, branch)
            .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;

        Ok(Self {
            name: worktree_name,
            path: worktree_path,
        })
    }

    pub fn plan_path(&self, slug: &str) -> PathBuf {
        self.path.join(plan_rel_path(slug))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn cleanup(self) -> Result<(), Box<dyn std::error::Error>> {
        remove_worktree(&self.name, true)
            .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
        if self.path.exists() {
            fs::remove_dir_all(&self.path)?;
        }
        Ok(())
    }
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
    spec_text: &str,
    plan_body: &str,
) -> String {
    let mut doc = String::new();

    doc.push_str("---\n");
    doc.push_str(&format!("plan: {slug}\n"));
    doc.push_str(&format!("branch: {branch_name}\n"));
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

        let spec_excerpt = extract_section(body, "Operator Spec");
        let spec_summary = spec_excerpt.as_ref().and_then(|text| summarize_line(text));

        Ok(Self {
            slug,
            branch,
            spec_excerpt,
            spec_summary,
        })
    }
}

#[derive(Debug, Clone)]
pub struct PlanSlugEntry {
    pub slug: String,
    pub branch: String,
    pub summary: String,
}

pub struct PlanSlugInventory;

impl PlanSlugInventory {
    pub fn collect(
        target_override: Option<&str>,
    ) -> Result<Vec<PlanSlugEntry>, Box<dyn std::error::Error>> {
        let repo_root =
            repo_root().map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
        let plan_dir = repo_root.join(PLAN_DIR);
        let repo = Repository::discover(&repo_root)?;
        let target_branch = Self::resolve_target_branch(target_override)?;
        let target_oid = Self::target_oid(&repo, &target_branch)?;

        let mut entries: Vec<PlanSlugEntry> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        if plan_dir.exists() {
            let mut plan_paths: Vec<PathBuf> = fs::read_dir(&plan_dir)?
                .filter_map(|res| res.ok().map(|entry| entry.path()))
                .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
                .collect();
            plan_paths.sort();
            for path in plan_paths {
                if let Some(entry) = Self::entry_from_plan_path(&repo, target_oid, &path)? {
                    seen.insert(entry.slug.clone());
                    entries.push(entry);
                }
            }
        }

        let mut branches = repo.branches(Some(BranchType::Local))?;
        while let Some(branch_res) = branches.next() {
            let (branch, _) = branch_res?;
            let Some(name) = branch.name()? else {
                continue;
            };
            if !name.starts_with("draft/") {
                continue;
            }
            let slug = name.trim_start_matches("draft/").to_string();
            if seen.contains(&slug) {
                continue;
            }

            let commit = branch.get().peel_to_commit()?;
            if target_oid == commit.id() || repo.graph_descendant_of(target_oid, commit.id())? {
                continue;
            }

            match load_plan_from_branch(&slug, name) {
                Ok(meta) => {
                    seen.insert(slug.clone());
                    let summary = summarize_spec(&meta);
                    entries.push(PlanSlugEntry {
                        slug: meta.slug.clone(),
                        branch: meta.branch.clone(),
                        summary,
                    });
                }
                Err(_) => continue,
            }
        }

        entries.sort_by(|a, b| a.slug.cmp(&b.slug));
        Ok(entries)
    }

    fn resolve_target_branch(
        target_override: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        if let Some(target) = target_override {
            Ok(target.to_string())
        } else {
            detect_primary_branch().ok_or_else(|| {
                Box::<dyn std::error::Error>::from(io::Error::new(
                    io::ErrorKind::NotFound,
                    "unable to detect primary branch; use --target",
                ))
            })
        }
    }

    fn target_oid(repo: &Repository, branch: &str) -> Result<Oid, Box<dyn std::error::Error>> {
        let target_ref = repo.find_branch(branch, BranchType::Local).map_err(
            |_| -> Box<dyn std::error::Error> {
                Box::new(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("target branch {branch} not found"),
                ))
            },
        )?;
        let commit = target_ref.into_reference().peel_to_commit()?;
        Ok(commit.id())
    }

    fn entry_from_plan_path(
        repo: &Repository,
        target_oid: Oid,
        path: &Path,
    ) -> Result<Option<PlanSlugEntry>, Box<dyn std::error::Error>> {
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(_) => return Ok(None),
        };

        let meta = match PlanMetadata::from_document(&contents) {
            Ok(meta) => meta,
            Err(_) => return Ok(None),
        };

        if meta.slug.is_empty() || meta.branch.is_empty() || !meta.branch.starts_with("draft/") {
            return Ok(None);
        }

        let branch = match repo.find_branch(&meta.branch, BranchType::Local) {
            Ok(branch) => branch,
            Err(_) => return Ok(None),
        };
        let commit = match branch.get().peel_to_commit() {
            Ok(commit) => commit,
            Err(_) => return Ok(None),
        };
        if target_oid == commit.id() || repo.graph_descendant_of(target_oid, commit.id())? {
            return Ok(None);
        }

        let summary = summarize_spec(&meta);

        Ok(Some(PlanSlugEntry {
            slug: meta.slug.clone(),
            branch: meta.branch.clone(),
            summary,
        }))
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
    use git2::{IndexAddOption, Repository, Signature, build::CheckoutBuilder};
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard};
    use tempfile::tempdir;

    static TEST_MUTEX: Mutex<()> = Mutex::new(());

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
    fn render_plan_document_includes_front_matter() -> Result<(), Box<dyn std::error::Error>> {
        let doc = render_plan_document(
            "alpha",
            "draft/alpha",
            "spec body",
            "## Execution Plan\n- step",
        );
        assert!(doc.contains("plan: alpha"));
        assert!(doc.contains("branch: draft/alpha"));
        assert!(doc.contains("## Operator Spec"));
        assert!(
            !doc.contains("status:"),
            "lean plan format should omit status metadata"
        );
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
status: review-ready
created_at: 2024-07-01T12:34:56Z
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
            spec_excerpt: Some("Line one\nLine two".into()),
            spec_summary: None,
        };

        assert_eq!(summarize_spec(&meta), "Line one\nLine two".to_string());
    }

    #[test]
    fn from_document_ignores_unknown_front_matter() -> Result<(), Box<dyn std::error::Error>> {
        let doc = r#"---
plan: alpha
branch: draft/alpha
status: draft
spec_source: inline
created_at: 2024-07-01T12:00:00Z
reviewed_at: 2024-07-02T12:00:00Z
implemented_at: 2024-07-03T12:00:00Z
---

## Operator Spec
Unknown fields should be ignored.

## Implementation Plan
- Step 1
"#;

        let meta = PlanMetadata::from_document(doc)?;
        assert_eq!(meta.slug, "alpha");
        assert_eq!(meta.branch, "draft/alpha");
        assert_eq!(
            meta.spec_summary.as_deref(),
            Some("Unknown fields should be ignored.")
        );
        Ok(())
    }

    #[test]
    fn slug_inventory_lists_pending_slugs_sorted() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, _guard, repo) = initialize_repo()?;
        checkout_branch(&repo, "master")?;
        create_plan_branch(&repo, "beta", "Beta spec body")?;
        checkout_branch(&repo, "master")?;
        create_plan_branch(&repo, "alpha", "Alpha spec body")?;
        checkout_branch(&repo, "master")?;

        let entries = PlanSlugInventory::collect(None)?;
        let slugs: Vec<_> = entries.iter().map(|entry| entry.slug.as_str()).collect();
        assert_eq!(slugs, vec!["alpha", "beta"]);
        Ok(())
    }

    #[test]
    fn slug_inventory_uses_plan_directory_when_available() -> Result<(), Box<dyn std::error::Error>>
    {
        let (_tmp, _guard, repo) = initialize_repo()?;
        checkout_branch(&repo, "master")?;
        create_plan_branch(&repo, "alpha", "Alpha summary line")?;
        checkout_branch(&repo, "draft/alpha")?;

        let entries = PlanSlugInventory::collect(None)?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].slug, "alpha");
        assert_eq!(entries[0].summary, "Alpha summary line");
        Ok(())
    }

    #[test]
    fn slug_inventory_ignores_merged_branches() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, _guard, repo) = initialize_repo()?;
        checkout_branch(&repo, "master")?;
        let commit_oid = create_plan_branch(&repo, "alpha", "Alpha spec body")?;
        repo.reference("refs/heads/master", commit_oid, true, "fast-forward master")?;
        checkout_branch(&repo, "master")?;

        let entries = PlanSlugInventory::collect(None)?;
        assert!(entries.is_empty());
        Ok(())
    }

    fn initialize_repo()
    -> Result<(tempfile::TempDir, DirGuard, Repository), Box<dyn std::error::Error>> {
        let dir = tempdir()?;
        let repo = Repository::init(dir.path())?;
        let guard = DirGuard::new(dir.path())?;
        std::fs::write(dir.path().join("README.md"), "root\n")?;
        commit_all(&repo, "init")?;
        Ok((dir, guard, repo))
    }

    fn create_plan_branch(
        repo: &Repository,
        slug: &str,
        spec: &str,
    ) -> Result<git2::Oid, Box<dyn std::error::Error>> {
        let head_commit = repo.head()?.peel_to_commit()?;
        let branch_name = format!("draft/{slug}");
        repo.branch(&branch_name, &head_commit, false)?;
        checkout_branch(repo, &branch_name)?;
        write_plan_file(slug, spec)?;
        let oid = commit_all(repo, &format!("plan {slug}"))?;
        Ok(oid)
    }

    fn write_plan_file(slug: &str, spec: &str) -> Result<(), std::io::Error> {
        let path = Path::new(".vizier/implementation-plans").join(format!("{slug}.md"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = format!(
            r#"---
plan: {slug}
branch: draft/{slug}
---

## Operator Spec
{spec}

## Implementation Plan
- Step 1
"#
        );
        std::fs::write(&path, contents)
    }

    fn commit_all(repo: &Repository, message: &str) -> Result<git2::Oid, git2::Error> {
        let mut index = repo.index()?;
        index.add_all(["*"].iter(), IndexAddOption::DEFAULT, None)?;
        index.write()?;
        let tree_id = index.write_tree()?;
        let tree = repo.find_tree(tree_id)?;
        let sig = Signature::now("Tester", "tester@example.com")?;
        let parents: Vec<git2::Commit> = match repo.head() {
            Ok(head) if head.is_branch() => vec![head.peel_to_commit()?],
            _ => Vec::new(),
        };
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
    }

    fn checkout_branch(repo: &Repository, name: &str) -> Result<(), git2::Error> {
        let mut checkout = CheckoutBuilder::new();
        checkout.force();
        repo.set_head(&format!("refs/heads/{name}"))?;
        repo.checkout_head(Some(&mut checkout))
    }

    struct DirGuard {
        previous: PathBuf,
        _lock: MutexGuard<'static, ()>,
    }

    impl DirGuard {
        fn new(path: &Path) -> Result<Self, std::io::Error> {
            let lock = TEST_MUTEX.lock().unwrap();
            let previous = std::env::current_dir()?;
            std::env::set_current_dir(path)?;
            Ok(Self {
                previous,
                _lock: lock,
            })
        }
    }

    impl Drop for DirGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.previous);
        }
    }
}
