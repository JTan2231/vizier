use crate::context::CliContext;
use crate::{jobs, plan, workspace};

use vizier_core::display::{self, Verbosity, format_number};

use super::shared::{append_agent_rows, current_verbosity, format_block, format_block_with_indent};
use super::types::{CdOptions, CleanOptions, ListOptions};

fn is_active_job(status: jobs::JobStatus) -> bool {
    matches!(status, jobs::JobStatus::Running | jobs::JobStatus::Pending)
}

fn job_sort_key(
    record: &jobs::JobRecord,
) -> (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) {
    let started = record.started_at.unwrap_or(record.created_at);
    (started, record.created_at)
}

fn select_inline_job<'a>(
    records: &'a [jobs::JobRecord],
    entry: &plan::PlanSlugEntry,
) -> Option<&'a jobs::JobRecord> {
    let by_plan: Vec<&jobs::JobRecord> = records
        .iter()
        .filter(|record| {
            record
                .metadata
                .as_ref()
                .and_then(|meta| meta.plan.as_deref())
                == Some(entry.slug.as_str())
        })
        .collect();

    let candidates: Vec<&jobs::JobRecord> = if by_plan.is_empty() {
        records
            .iter()
            .filter(|record| {
                record
                    .metadata
                    .as_ref()
                    .and_then(|meta| meta.branch.as_deref())
                    == Some(entry.branch.as_str())
            })
            .collect()
    } else {
        by_plan
    };

    if candidates.is_empty() {
        return None;
    }

    let mut active = Vec::new();
    for record in &candidates {
        if is_active_job(record.status) {
            active.push(*record);
        }
    }

    let pool = if active.is_empty() {
        &candidates
    } else {
        &active
    };
    pool.iter()
        .copied()
        .max_by_key(|record| job_sort_key(record))
}

pub(crate) fn run_list(opts: ListOptions) -> Result<(), Box<dyn std::error::Error>> {
    list_pending_plans(opts.target)
}

pub(crate) fn run_cd(opts: CdOptions) -> Result<(), Box<dyn std::error::Error>> {
    let ctx = CliContext::load()?;
    let repo_root = ctx.repo_root;
    let mut store = workspace::WorkspaceStore::load(&repo_root)?;
    let status = store.ensure_workspace(&opts.slug, &opts.branch)?;

    if matches!(status.clean, Some(false)) {
        display::warn(format!(
            "workspace {} has uncommitted or untracked changes",
            status.path.display()
        ));
    }

    println!("{}", status.path.display());
    if opts.path_only || matches!(ctx.verbosity, Verbosity::Quiet) {
        return Ok(());
    }

    let mut rows = vec![
        (
            "Outcome".to_string(),
            if status.created {
                "Workspace created".to_string()
            } else {
                "Workspace ready".to_string()
            },
        ),
        ("Plan".to_string(), opts.slug),
        ("Branch".to_string(), status.branch),
        ("Worktree".to_string(), status.worktree_name),
        ("Path".to_string(), status.path.display().to_string()),
    ];

    if matches!(status.clean, Some(false)) {
        rows.push((
            "Note".to_string(),
            "workspace has uncommitted changes".to_string(),
        ));
    }

    println!("{}", format_block(rows));
    Ok(())
}

pub(crate) fn run_clean(opts: CleanOptions) -> Result<(), Box<dyn std::error::Error>> {
    let repo_root = vizier_core::vcs::repo_root()
        .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let mut store = workspace::WorkspaceStore::load(&repo_root)?;
    let candidates = store.discover(opts.slug.as_deref())?;

    if candidates.is_empty() {
        println!("Outcome: no Vizier workspaces found");
        return Ok(());
    }

    if !opts.assume_yes {
        let mut header = vec![(
            "Outcome".to_string(),
            format!(
                "Remove {} workspace{}",
                format_number(candidates.len()),
                if candidates.len() == 1 { "" } else { "s" }
            ),
        )];
        if let Some(slug) = opts.slug.as_ref() {
            header.push(("Plan".to_string(), slug.clone()));
        }
        println!("{}", format_block(header));
        println!();

        for (idx, candidate) in candidates.iter().enumerate() {
            let mut rows = vec![
                ("Plan".to_string(), candidate.slug.clone()),
                ("Path".to_string(), candidate.path.display().to_string()),
            ];
            if let Some(branch) = candidate.branch.as_ref() {
                rows.push(("Branch".to_string(), branch.clone()));
            }
            if !candidate.registered {
                rows.push((
                    "Note".to_string(),
                    "not registered with git worktree".to_string(),
                ));
            }
            if candidate.path.exists() {
                if let Ok(clean) = workspace::worktree_cleanliness(&candidate.path)
                    && !clean
                {
                    rows.push((
                        "Note".to_string(),
                        "workspace has uncommitted changes".to_string(),
                    ));
                }
            } else {
                rows.push(("Note".to_string(), "workspace path missing".to_string()));
            }
            println!("{}", format_block_with_indent(rows, 2));
            if idx + 1 < candidates.len() {
                println!();
            }
        }

        if !super::shared::prompt_for_confirmation("Remove the workspaces above? [y/N]: ")? {
            println!("Outcome: clean cancelled");
            return Ok(());
        }
    }

    let mut removed = 0usize;
    let mut failed: Vec<(String, String)> = Vec::new();
    for candidate in &candidates {
        let mut removed_this = false;
        if candidate.registered || candidate.path.exists() {
            match workspace::remove_workspace(&repo_root, candidate) {
                Ok(_) => removed_this = true,
                Err(err) => failed.push((candidate.slug.clone(), err.to_string())),
            }
        } else {
            removed_this = true;
        }

        if removed_this {
            removed += 1;
            store.forget(&candidate.slug);
        }
    }

    store.save()?;

    let mut rows = vec![(
        "Outcome".to_string(),
        if failed.is_empty() {
            format!(
                "Removed {} workspace{}",
                format_number(removed),
                if removed == 1 { "" } else { "s" }
            )
        } else {
            format!(
                "Removed {} workspace{} ({} failed)",
                format_number(removed),
                if removed == 1 { "" } else { "s" },
                format_number(failed.len())
            )
        },
    )];
    if let Some(slug) = opts.slug.as_ref() {
        rows.push(("Plan".to_string(), slug.clone()));
    }
    append_agent_rows(&mut rows, current_verbosity());
    println!("{}", format_block(rows));

    if !failed.is_empty() {
        display::warn("Some workspaces could not be removed:");
        for (slug, err) in failed {
            display::warn(format!("{slug}: {err}"));
        }
    }

    Ok(())
}

fn list_pending_plans(target_override: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let entries = plan::PlanSlugInventory::collect(target_override.as_deref())?;
    if entries.is_empty() {
        let mut rows = vec![(
            "Outcome".to_string(),
            "No pending draft branches".to_string(),
        )];
        if let Some(target) = target_override {
            rows.push(("Target".to_string(), target));
        }
        println!("{}", format_block(rows));
        return Ok(());
    }

    let repo_root = vizier_core::vcs::repo_root()
        .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    let jobs_root = repo_root.join(".vizier").join("jobs");
    let job_records = match jobs::list_records(&jobs_root) {
        Ok(records) => records,
        Err(err) => {
            display::warn(format!("unable to load background jobs: {err}"));
            Vec::new()
        }
    };

    let mut header_rows = vec![(
        "Outcome".to_string(),
        format!(
            "{} pending draft {}",
            format_number(entries.len()),
            if entries.len() == 1 {
                "branch"
            } else {
                "branches"
            }
        ),
    )];
    if let Some(target) = target_override {
        header_rows.push(("Target".to_string(), target));
    }
    println!("{}", format_block(header_rows));
    println!();

    for (idx, entry) in entries.iter().enumerate() {
        let summary = entry.summary.replace('"', "'").replace('\n', " ");
        let mut rows = vec![
            ("Plan".to_string(), entry.slug.clone()),
            ("Branch".to_string(), entry.branch.clone()),
            ("Summary".to_string(), summary),
        ];
        if let Some(record) = select_inline_job(&job_records, entry) {
            rows.push(("Job".to_string(), record.id.clone()));
            rows.push((
                "Job status".to_string(),
                jobs::status_label(record.status).to_string(),
            ));
            if let Some(scope) = record
                .metadata
                .as_ref()
                .and_then(|meta| meta.scope.as_ref())
            {
                rows.push(("Job scope".to_string(), scope.to_string()));
            }
            if let Some(started_at) = record.started_at {
                rows.push(("Job started".to_string(), started_at.to_rfc3339()));
            }
            let job_id = record.id.clone();
            rows.push(("Status".to_string(), format!("vizier jobs status {job_id}")));
            rows.push((
                "Logs".to_string(),
                format!("vizier jobs tail --follow {job_id}"),
            ));
            rows.push(("Attach".to_string(), format!("vizier jobs attach {job_id}")));
        }

        println!("{}", format_block_with_indent(rows, 2));
        if idx + 1 < entries.len() {
            println!();
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::select_inline_job;
    use crate::jobs::{JobMetadata, JobRecord, JobStatus};
    use chrono::TimeZone;

    fn job_record(
        id: &str,
        status: JobStatus,
        created_at: chrono::DateTime<chrono::Utc>,
        started_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> JobRecord {
        JobRecord {
            id: id.to_string(),
            status,
            command: vec!["vizier".to_string(), "approve".to_string()],
            created_at,
            started_at,
            finished_at: None,
            pid: None,
            exit_code: None,
            stdout_path: "stdout.log".to_string(),
            stderr_path: "stderr.log".to_string(),
            session_path: None,
            outcome_path: None,
            metadata: Some(JobMetadata {
                plan: Some("alpha".to_string()),
                branch: Some("draft/alpha".to_string()),
                scope: Some("approve".to_string()),
                ..JobMetadata::default()
            }),
            config_snapshot: None,
        }
    }

    #[test]
    fn select_inline_job_prefers_active_and_newest() {
        let entry = crate::plan::PlanSlugEntry {
            slug: "alpha".to_string(),
            branch: "draft/alpha".to_string(),
            summary: "Alpha spec".to_string(),
        };

        let records = vec![
            job_record(
                "first",
                JobStatus::Succeeded,
                chrono::Utc.with_ymd_and_hms(2024, 3, 2, 10, 0, 0).unwrap(),
                Some(chrono::Utc.with_ymd_and_hms(2024, 3, 2, 11, 0, 0).unwrap()),
            ),
            job_record(
                "second",
                JobStatus::Running,
                chrono::Utc.with_ymd_and_hms(2024, 3, 3, 10, 0, 0).unwrap(),
                Some(chrono::Utc.with_ymd_and_hms(2024, 3, 3, 12, 0, 0).unwrap()),
            ),
            job_record(
                "third",
                JobStatus::Pending,
                chrono::Utc.with_ymd_and_hms(2024, 3, 4, 10, 0, 0).unwrap(),
                None,
            ),
        ];

        let selected = select_inline_job(&records, &entry).expect("select job");
        assert_eq!(selected.id, "third");
    }
}
