use git2::{BranchType, ErrorCode, Oid, Repository, Sort};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::jobs;
use vizier_core::vcs::{
    add_worktree_for_branch, branch_exists, detect_primary_branch, remove_worktree, repo_root,
};

pub const PLAN_DIR: &str = ".vizier/implementation-plans";
pub const PLAN_STATE_DIR: &str = ".vizier/state/plans";
const MAX_SUMMARY_LEN: usize = 160;

pub fn plan_rel_path(slug: &str) -> PathBuf {
    Path::new(PLAN_DIR).join(format!("{slug}.md"))
}

pub fn plan_state_rel_path(plan_id: &str) -> PathBuf {
    Path::new(PLAN_STATE_DIR).join(format!("{plan_id}.json"))
}

pub fn new_plan_id() -> String {
    format!("pln_{}", Uuid::new_v4().simple())
}

fn legacy_plan_id_from_slug(slug: &str) -> String {
    let normalized = normalize_slug(slug);
    if normalized.is_empty() {
        "pln_legacy".to_string()
    } else {
        format!("pln_legacy_{normalized}")
    }
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
        jobs::record_current_job_worktree(&repo_root, Some(&worktree_name), &worktree_path);

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
    let trimmed = text.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() { "" } else { trimmed }
}

pub fn render_plan_document(
    plan_id: &str,
    slug: &str,
    branch_name: &str,
    spec_text: &str,
    plan_body: &str,
) -> String {
    let mut doc = String::new();

    doc.push_str("---\n");
    doc.push_str(&format!("plan_id: {plan_id}\n"));
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
    pub plan_id: String,
    pub slug: String,
    pub branch: String,
    pub spec_excerpt: Option<String>,
    pub spec_summary: Option<String>,
}

impl PlanMetadata {
    pub fn from_document(contents: &str) -> Result<Self, PlanError> {
        let (front_matter, body) = split_front_matter(contents)?;
        let fields = parse_front_matter(front_matter);

        let slug = if let Some(value) = fields.get("plan") {
            value.to_string()
        } else if let Some(branch) = fields.get("branch") {
            branch
                .strip_prefix("draft/")
                .map(|value| value.to_string())
                .ok_or(PlanError::MissingField("plan"))?
        } else {
            return Err(PlanError::MissingField("plan"));
        };
        let branch = fields
            .get("branch")
            .cloned()
            .unwrap_or_else(|| default_branch_for_slug(&slug));
        let plan_id = fields
            .get("plan_id")
            .cloned()
            .unwrap_or_else(|| legacy_plan_id_from_slug(&slug));

        let spec_excerpt = extract_section(body, "Operator Spec");
        let spec_summary = spec_excerpt.as_ref().and_then(|text| summarize_line(text));

        Ok(Self {
            plan_id,
            slug,
            branch,
            spec_excerpt,
            spec_summary,
        })
    }
}

#[derive(Debug, Clone)]
pub struct LoadedPlanDocument {
    pub metadata: PlanMetadata,
    pub contents: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanRecord {
    pub plan_id: String,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub target_branch: Option<String>,
    #[serde(default)]
    pub work_ref: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub job_ids: HashMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct PlanRecordUpsert {
    pub plan_id: String,
    pub slug: Option<String>,
    pub branch: Option<String>,
    pub source: Option<String>,
    pub intent: Option<String>,
    pub target_branch: Option<String>,
    pub work_ref: Option<String>,
    pub status: Option<String>,
    pub summary: Option<String>,
    pub updated_at: String,
    pub created_at: Option<String>,
    pub job_ids: Option<HashMap<String, String>>,
}

pub fn plan_id_from_document(contents: &str) -> Option<String> {
    let (front_matter, _) = split_front_matter(contents).ok()?;
    let fields = parse_front_matter(front_matter);
    fields.get("plan_id").cloned()
}

pub fn upsert_plan_record(
    repo_root: &Path,
    update: PlanRecordUpsert,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if update.plan_id.trim().is_empty() {
        return Err("plan_id cannot be empty for plan records".into());
    }

    let rel_path = plan_state_rel_path(&update.plan_id);
    let abs_path = repo_root.join(&rel_path);
    if let Some(parent) = abs_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut record = if abs_path.exists() {
        let raw = fs::read_to_string(&abs_path)?;
        serde_json::from_str::<PlanRecord>(&raw)?
    } else {
        PlanRecord {
            plan_id: update.plan_id.clone(),
            slug: None,
            branch: None,
            source: None,
            intent: None,
            target_branch: None,
            work_ref: None,
            status: None,
            summary: None,
            created_at: update
                .created_at
                .clone()
                .unwrap_or_else(|| update.updated_at.clone()),
            updated_at: update.updated_at.clone(),
            job_ids: HashMap::new(),
        }
    };

    if record.created_at.trim().is_empty() {
        record.created_at = update
            .created_at
            .clone()
            .unwrap_or_else(|| update.updated_at.clone());
    }
    record.updated_at = update.updated_at.clone();

    if update.slug.is_some() {
        record.slug = update.slug;
    }
    if update.branch.is_some() {
        record.branch = update.branch;
    }
    if update.source.is_some() {
        record.source = update.source;
    }
    if update.intent.is_some() {
        record.intent = update.intent;
    }
    if update.target_branch.is_some() {
        record.target_branch = update.target_branch;
    }
    if update.work_ref.is_some() {
        record.work_ref = update.work_ref;
    }
    if update.status.is_some() {
        record.status = update.status;
    }
    if update.summary.is_some() {
        record.summary = update.summary;
    }
    if let Some(job_ids) = update.job_ids {
        for (phase, job_id) in job_ids {
            if !phase.trim().is_empty() && !job_id.trim().is_empty() {
                record.job_ids.insert(phase, job_id);
            }
        }
    }

    let contents = serde_json::to_string_pretty(&record)?;
    fs::write(&abs_path, contents)?;
    Ok(rel_path)
}

pub fn load_plan_records(repo_root: &Path) -> Result<Vec<PlanRecord>, Box<dyn std::error::Error>> {
    let state_dir = repo_root.join(PLAN_STATE_DIR);
    if !state_dir.exists() {
        return Ok(Vec::new());
    }

    let mut records = Vec::new();
    let mut paths: Vec<PathBuf> = fs::read_dir(&state_dir)?
        .filter_map(|entry| entry.ok().map(|item| item.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect();
    paths.sort();

    for path in paths {
        let raw = match fs::read_to_string(&path) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let record: PlanRecord = match serde_json::from_str(&raw) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if record.plan_id.trim().is_empty() {
            continue;
        }
        records.push(record);
    }

    Ok(records)
}

#[derive(Debug, Clone)]
pub struct PlanSlugEntry {
    pub plan_id: String,
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
        let mut seen_slugs: HashSet<String> = HashSet::new();
        let mut seen_plan_ids: HashSet<String> = HashSet::new();

        let mut records = load_plan_records(&repo_root)?;
        records.sort_by(|a, b| {
            let left = a.slug.as_deref().unwrap_or(a.plan_id.as_str());
            let right = b.slug.as_deref().unwrap_or(b.plan_id.as_str());
            left.cmp(right)
        });
        for record in records {
            let branch = record
                .branch
                .as_ref()
                .or(record.work_ref.as_ref())
                .cloned()
                .unwrap_or_default();
            if branch.is_empty() || !branch.starts_with("draft/") {
                continue;
            }

            let slug = record
                .slug
                .clone()
                .or_else(|| branch.strip_prefix("draft/").map(|value| value.to_string()))
                .unwrap_or_default();
            if slug.is_empty() {
                continue;
            }

            if seen_slugs.contains(&slug) || seen_plan_ids.contains(&record.plan_id) {
                continue;
            }

            let branch_ref = match repo.find_branch(&branch, BranchType::Local) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let commit = match branch_ref.get().peel_to_commit() {
                Ok(value) => value,
                Err(_) => continue,
            };
            if target_oid == commit.id() || repo.graph_descendant_of(target_oid, commit.id())? {
                continue;
            }

            let summary = record.summary.clone().unwrap_or_else(|| {
                load_plan_from_branch(&slug, &branch)
                    .map(|meta| summarize_spec(&meta))
                    .unwrap_or_else(|_| format!("Plan {} summary unavailable", slug))
            });

            seen_slugs.insert(slug.clone());
            seen_plan_ids.insert(record.plan_id.clone());
            entries.push(PlanSlugEntry {
                plan_id: record.plan_id,
                slug,
                branch,
                summary,
            });
        }

        if plan_dir.exists() {
            let mut plan_paths: Vec<PathBuf> = fs::read_dir(&plan_dir)?
                .filter_map(|res| res.ok().map(|entry| entry.path()))
                .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
                .collect();
            plan_paths.sort();
            for path in plan_paths {
                if let Some(entry) = Self::entry_from_plan_path(&repo, target_oid, &path)? {
                    if seen_slugs.contains(&entry.slug) || seen_plan_ids.contains(&entry.plan_id) {
                        continue;
                    }
                    seen_slugs.insert(entry.slug.clone());
                    seen_plan_ids.insert(entry.plan_id.clone());
                    entries.push(entry);
                }
            }
        }

        let branches = repo.branches(Some(BranchType::Local))?;
        for branch_res in branches {
            let (branch, _) = branch_res?;
            let Some(name) = branch.name()? else {
                continue;
            };
            if !name.starts_with("draft/") {
                continue;
            }
            let slug = name.trim_start_matches("draft/").to_string();
            if seen_slugs.contains(&slug) {
                continue;
            }

            let commit = branch.get().peel_to_commit()?;
            if target_oid == commit.id() || repo.graph_descendant_of(target_oid, commit.id())? {
                continue;
            }

            match load_plan_from_branch(&slug, name) {
                Ok(meta) => {
                    if seen_plan_ids.contains(&meta.plan_id) {
                        continue;
                    }
                    seen_slugs.insert(slug.clone());
                    seen_plan_ids.insert(meta.plan_id.clone());
                    let summary = summarize_spec(&meta);
                    entries.push(PlanSlugEntry {
                        plan_id: meta.plan_id.clone(),
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
            plan_id: meta.plan_id.clone(),
            slug: meta.slug.clone(),
            branch: meta.branch.clone(),
            summary,
        }))
    }
}

pub fn load_plan_from_branch(slug: &str, branch: &str) -> Result<PlanMetadata, PlanError> {
    let repo = Repository::discover(".")?;
    let plan_path = plan_rel_path(slug);
    let commit = load_branch_head_commit(&repo, branch)?;
    let contents =
        load_plan_document_from_commit(&repo, &commit, &plan_path)?.ok_or_else(|| {
            PlanError::MissingPlanFile {
                branch: branch.to_string(),
                path: plan_path.clone(),
            }
        })?;
    PlanMetadata::from_document(&contents)
}

pub fn load_plan_for_merge(slug: &str, branch: &str) -> Result<LoadedPlanDocument, PlanError> {
    let repo = Repository::discover(".")?;
    let plan_path = plan_rel_path(slug);
    let head = load_branch_head_commit(&repo, branch)?;

    if let Some(contents) = load_plan_document_from_commit(&repo, &head, &plan_path)? {
        let metadata = PlanMetadata::from_document(&contents)?;
        return Ok(LoadedPlanDocument { metadata, contents });
    }

    let mut revwalk = repo.revwalk()?;
    revwalk.push(head.id())?;
    revwalk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;

    for oid in revwalk {
        let oid = oid?;
        let commit = repo.find_commit(oid)?;
        let Some(contents) = load_plan_document_from_commit(&repo, &commit, &plan_path)? else {
            continue;
        };
        let Ok(metadata) = PlanMetadata::from_document(&contents) else {
            continue;
        };
        if metadata.slug == slug && metadata.branch == branch {
            return Ok(LoadedPlanDocument { metadata, contents });
        }
    }

    Err(PlanError::MissingPlanFile {
        branch: branch.to_string(),
        path: plan_path,
    })
}

fn load_branch_head_commit<'repo>(
    repo: &'repo Repository,
    branch: &str,
) -> Result<git2::Commit<'repo>, PlanError> {
    let branch_ref = repo.find_branch(branch, BranchType::Local)?;
    let commit = branch_ref.into_reference().peel_to_commit()?;
    Ok(commit)
}

fn load_plan_document_from_commit(
    repo: &Repository,
    commit: &git2::Commit<'_>,
    plan_path: &Path,
) -> Result<Option<String>, PlanError> {
    let tree = commit.tree()?;
    let entry = match tree.get_path(plan_path) {
        Ok(value) => value,
        Err(err) if err.code() == ErrorCode::NotFound => return Ok(None),
        Err(err) => return Err(PlanError::Git(err)),
    };
    let blob = repo.find_blob(entry.id())?;
    let contents = std::str::from_utf8(blob.content())?.to_string();
    Ok(Some(contents))
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
    let lines = after_header.lines();

    let mut collected = Vec::new();
    for line in lines {
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
            "pln_alpha",
            "alpha",
            "draft/alpha",
            "spec body",
            "## Execution Plan\n- step",
        );
        assert!(doc.contains("plan_id: pln_alpha"));
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
plan_id: pln_alpha
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
        assert_eq!(meta.plan_id, "pln_alpha");
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
            plan_id: "pln_alpha".into(),
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
plan_id: pln_alpha
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
        assert_eq!(meta.plan_id, "pln_alpha");
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

    #[test]
    fn parse_metadata_uses_legacy_plan_id_fallback() -> Result<(), Box<dyn std::error::Error>> {
        let doc = r#"---
plan: alpha
branch: draft/alpha
---

## Operator Spec
Legacy format

## Implementation Plan
- step
"#;
        let meta = PlanMetadata::from_document(doc)?;
        assert_eq!(meta.plan_id, "pln_legacy_alpha");
        Ok(())
    }

    #[test]
    fn load_plan_for_merge_recovers_from_history_when_tip_file_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, _guard, repo) = initialize_repo()?;
        checkout_branch(&repo, "master")?;
        create_plan_branch(&repo, "alpha", "Alpha spec body")?;
        checkout_branch(&repo, "draft/alpha")?;

        let path = Path::new(".vizier/implementation-plans/alpha.md");
        std::fs::remove_file(path)?;
        commit_all(&repo, "remove plan file from tip")?;

        let loaded = load_plan_for_merge("alpha", "draft/alpha")?;
        assert_eq!(loaded.metadata.slug, "alpha");
        assert_eq!(loaded.metadata.branch, "draft/alpha");
        assert!(
            loaded.contents.contains("plan: alpha"),
            "expected recovered plan contents from history"
        );
        Ok(())
    }

    #[test]
    fn load_plan_for_merge_fails_when_history_has_no_plan_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, _guard, repo) = initialize_repo()?;
        let head = repo.head()?.peel_to_commit()?;
        repo.branch("draft/missing", &head, false)?;
        checkout_branch(&repo, "draft/missing")?;
        std::fs::write("notes.txt", "branch without plan docs\n")?;
        commit_all(&repo, "seed draft branch")?;

        let err = load_plan_for_merge("missing", "draft/missing").unwrap_err();
        assert!(
            matches!(err, PlanError::MissingPlanFile { .. }),
            "expected MissingPlanFile, got {err}"
        );
        Ok(())
    }

    #[test]
    fn load_plan_for_merge_rejects_mismatched_front_matter_in_history()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, _guard, repo) = initialize_repo()?;
        let head = repo.head()?.peel_to_commit()?;
        repo.branch("draft/alpha", &head, false)?;
        checkout_branch(&repo, "draft/alpha")?;

        let path = Path::new(".vizier/implementation-plans/alpha.md");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mismatched = r#"---
plan_id: pln_beta
plan: beta
branch: draft/beta
---

## Operator Spec
Wrong plan identity.

## Implementation Plan
- Step 1
"#;
        std::fs::write(path, mismatched)?;
        commit_all(&repo, "add mismatched plan doc")?;
        std::fs::remove_file(path)?;
        commit_all(&repo, "remove mismatched plan doc")?;

        let err = load_plan_for_merge("alpha", "draft/alpha").unwrap_err();
        assert!(
            matches!(err, PlanError::MissingPlanFile { .. }),
            "expected MissingPlanFile after rejecting mismatched history, got {err}"
        );
        Ok(())
    }

    #[test]
    fn upsert_plan_record_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempdir()?;
        let repo_root = dir.path();
        let rel = upsert_plan_record(
            repo_root,
            PlanRecordUpsert {
                plan_id: "pln_test".to_string(),
                slug: Some("alpha".to_string()),
                branch: Some("draft/alpha".to_string()),
                source: Some("draft".to_string()),
                intent: Some("inline".to_string()),
                target_branch: Some("main".to_string()),
                work_ref: Some("draft/alpha".to_string()),
                status: Some("draft".to_string()),
                summary: Some("Alpha summary".to_string()),
                updated_at: "2026-02-07T00:00:00Z".to_string(),
                created_at: Some("2026-02-07T00:00:00Z".to_string()),
                job_ids: None,
            },
        )?;
        assert_eq!(rel, Path::new(PLAN_STATE_DIR).join("pln_test.json"));

        let records = load_plan_records(repo_root)?;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].plan_id, "pln_test");
        assert_eq!(records[0].slug.as_deref(), Some("alpha"));
        assert_eq!(records[0].summary.as_deref(), Some("Alpha summary"));
        Ok(())
    }

    #[test]
    fn slug_inventory_prefers_plan_records() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, _guard, repo) = initialize_repo()?;
        checkout_branch(&repo, "master")?;
        create_plan_branch(&repo, "alpha", "Alpha spec body")?;
        checkout_branch(&repo, "master")?;
        let repo_root = std::env::current_dir()?;
        upsert_plan_record(
            &repo_root,
            PlanRecordUpsert {
                plan_id: "pln_record_alpha".to_string(),
                slug: Some("alpha".to_string()),
                branch: Some("draft/alpha".to_string()),
                source: Some("draft".to_string()),
                intent: Some("file".to_string()),
                target_branch: Some("master".to_string()),
                work_ref: Some("draft/alpha".to_string()),
                status: Some("review-ready".to_string()),
                summary: Some("Summary from plan record".to_string()),
                updated_at: "2026-02-07T00:00:00Z".to_string(),
                created_at: Some("2026-02-07T00:00:00Z".to_string()),
                job_ids: None,
            },
        )?;

        let entries = PlanSlugInventory::collect(Some("master"))?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].plan_id, "pln_record_alpha");
        assert_eq!(entries[0].summary, "Summary from plan record");
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
plan_id: pln_{slug}
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
