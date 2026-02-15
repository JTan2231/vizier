#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const PLAN_DIR: &str = ".vizier/implementation-plans";
pub const PLAN_STATE_DIR: &str = ".vizier/state/plans";

pub fn plan_rel_path(slug: &str) -> PathBuf {
    Path::new(PLAN_DIR).join(format!("{slug}.md"))
}

pub fn plan_state_rel_path(plan_id: &str) -> PathBuf {
    Path::new(PLAN_STATE_DIR).join(format!("{plan_id}.json"))
}

pub fn new_plan_id() -> String {
    format!("pln_{}", Uuid::new_v4().simple())
}

pub fn default_branch_for_slug(slug: &str) -> String {
    format!("draft/{slug}")
}

fn normalize_slug(input: &str) -> String {
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
