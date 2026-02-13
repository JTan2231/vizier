use crate::{jobs, plan};

use serde_json::{Map, Value, json};
use std::collections::HashMap;

use vizier_core::{
    config,
    display::{self, format_number},
};

use super::shared::{format_block, format_block_with_indent, format_table};
use super::types::{CdOptions, CleanOptions, ListOptions};

fn is_active_job(status: jobs::JobStatus) -> bool {
    matches!(
        status,
        jobs::JobStatus::Queued
            | jobs::JobStatus::WaitingOnDeps
            | jobs::JobStatus::WaitingOnApproval
            | jobs::JobStatus::WaitingOnLocks
            | jobs::JobStatus::Running
    )
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

#[derive(Clone, Copy, Debug)]
enum ListHeaderField {
    Outcome,
    Target,
}

impl ListHeaderField {
    fn parse(value: &str) -> Option<Self> {
        match normalize_field_key(value).as_str() {
            "outcome" => Some(Self::Outcome),
            "target" => Some(Self::Target),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Outcome => "Outcome",
            Self::Target => "Target",
        }
    }

    fn json_key(self) -> &'static str {
        match self {
            Self::Outcome => "outcome",
            Self::Target => "target",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ListEntryField {
    Plan,
    Branch,
    Summary,
}

impl ListEntryField {
    fn parse(value: &str) -> Option<Self> {
        match normalize_field_key(value).as_str() {
            "plan" => Some(Self::Plan),
            "branch" => Some(Self::Branch),
            "summary" => Some(Self::Summary),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Plan => "Plan",
            Self::Branch => "Branch",
            Self::Summary => "Summary",
        }
    }

    fn json_key(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Branch => "branch",
            Self::Summary => "summary",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ListJobField {
    Job,
    JobStatus,
    JobScope,
    JobStarted,
}

impl ListJobField {
    fn parse(value: &str) -> Option<Self> {
        match normalize_field_key(value).as_str() {
            "job" => Some(Self::Job),
            "job status" => Some(Self::JobStatus),
            "job scope" => Some(Self::JobScope),
            "job started" => Some(Self::JobStarted),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Job => "Job",
            Self::JobStatus => "Job status",
            Self::JobScope => "Job scope",
            Self::JobStarted => "Job started",
        }
    }

    fn json_key(self) -> &'static str {
        match self {
            Self::Job => "job",
            Self::JobStatus => "job_status",
            Self::JobScope => "job_scope",
            Self::JobStarted => "job_started",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ListCommandField {
    Status,
    Logs,
    Attach,
}

impl ListCommandField {
    fn parse(value: &str) -> Option<Self> {
        match normalize_field_key(value).as_str() {
            "status" => Some(Self::Status),
            "logs" => Some(Self::Logs),
            "attach" => Some(Self::Attach),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Status => "Status",
            Self::Logs => "Logs",
            Self::Attach => "Attach",
        }
    }

    fn json_key(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Logs => "logs",
            Self::Attach => "attach",
        }
    }
}

fn normalize_field_key(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_labels(labels: &HashMap<String, String>) -> HashMap<String, String> {
    labels
        .iter()
        .map(|(key, value)| (normalize_field_key(key), value.clone()))
        .collect()
}

fn resolve_label(labels: &HashMap<String, String>, default_label: &str) -> String {
    let key = normalize_field_key(default_label);
    labels
        .get(&key)
        .cloned()
        .unwrap_or_else(|| default_label.to_string())
}

fn parse_fields<T, F>(context: &str, values: &[String], parser: F) -> Vec<T>
where
    F: Fn(&str) -> Option<T>,
{
    let mut fields = Vec::new();
    for value in values {
        if let Some(field) = parser(value) {
            fields.push(field);
        } else {
            display::warn(format!("{context}: unknown field `{value}`; ignoring"));
        }
    }
    fields
}

fn format_summary(value: &str, max_len: usize, single_line: bool) -> String {
    let mut out = value.trim().replace('"', "'");
    if single_line {
        out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    if max_len > 0 && out.chars().count() > max_len {
        if max_len <= 3 {
            return "...".to_string();
        }
        let truncated: String = out.chars().take(max_len - 3).collect();
        return format!("{truncated}...");
    }
    out
}

pub(crate) fn run_list(opts: ListOptions) -> Result<(), Box<dyn std::error::Error>> {
    list_pending_plans(opts)
}

pub(crate) fn run_cd(opts: CdOptions) -> Result<(), Box<dyn std::error::Error>> {
    let note = if opts.path_only {
        " (path-only flag ignored)"
    } else {
        ""
    };
    display::emit(
        display::LogLevel::Error,
        format!(
            "vizier cd is deprecated; scheduler-managed temp worktrees replace workspaces (plan {}, branch {}){}",
            opts.slug, opts.branch, note
        ),
    );
    Err("vizier cd is deprecated; use scheduler-managed jobs instead".into())
}

pub(crate) fn run_clean(opts: CleanOptions) -> Result<(), Box<dyn std::error::Error>> {
    let target = opts.slug.clone().unwrap_or_else(|| "all".to_string());
    let note = if opts.assume_yes {
        " (--yes ignored)"
    } else {
        ""
    };
    display::emit(
        display::LogLevel::Error,
        format!(
            "vizier clean is deprecated; scheduler-managed temp worktrees replace workspaces (target: {target}){note}"
        ),
    );
    Err("vizier clean is deprecated; use scheduler-managed jobs instead".into())
}

fn list_pending_plans(opts: ListOptions) -> Result<(), Box<dyn std::error::Error>> {
    let entries = plan::PlanSlugInventory::collect(opts.target.as_deref())?;
    let mut list_config = config::get_config().display.lists.list.clone();
    if let Some(format) = opts.format {
        list_config.format = format;
    }
    if let Some(fields) = opts.fields.clone() {
        list_config.entry_fields = fields;
    }

    let header_fields = parse_fields(
        "display.lists.list.header_fields",
        &list_config.header_fields,
        ListHeaderField::parse,
    );
    let entry_fields = parse_fields(
        "display.lists.list.entry_fields",
        &list_config.entry_fields,
        ListEntryField::parse,
    );
    let job_fields = parse_fields(
        "display.lists.list.job_fields",
        &list_config.job_fields,
        ListJobField::parse,
    );
    let command_fields = parse_fields(
        "display.lists.list.command_fields",
        &list_config.command_fields,
        ListCommandField::parse,
    );
    let labels = normalize_labels(&list_config.labels);

    let outcome = if entries.is_empty() {
        "No pending draft branches".to_string()
    } else {
        format!(
            "{} pending draft {}",
            format_number(entries.len()),
            if entries.len() == 1 {
                "branch"
            } else {
                "branches"
            }
        )
    };

    if matches!(list_config.format, config::ListFormat::Json) {
        let mut job_records = Vec::new();
        if !job_fields.is_empty() || !command_fields.is_empty() {
            let repo_root = vizier_core::vcs::repo_root()
                .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
            let jobs_root = repo_root.join(".vizier").join("jobs");
            match jobs::list_records(&jobs_root) {
                Ok(records) => job_records = records,
                Err(err) => {
                    display::warn(format!("unable to load background jobs: {err}"));
                }
            }
        }

        let mut header = Map::new();
        for field in &header_fields {
            match field {
                ListHeaderField::Outcome => {
                    header.insert(field.json_key().to_string(), Value::String(outcome.clone()));
                }
                ListHeaderField::Target => {
                    if let Some(target) = opts.target.as_ref() {
                        header.insert(field.json_key().to_string(), Value::String(target.clone()));
                    }
                }
            }
        }

        let mut entries_json = Vec::new();
        for entry in &entries {
            let mut obj = Map::new();
            let summary = format_summary(
                &entry.summary,
                list_config.summary_max_len,
                list_config.summary_single_line,
            );
            for field in &entry_fields {
                let value = match field {
                    ListEntryField::Plan => entry.slug.clone(),
                    ListEntryField::Branch => entry.branch.clone(),
                    ListEntryField::Summary => summary.clone(),
                };
                obj.insert(field.json_key().to_string(), Value::String(value));
            }

            let record = select_inline_job(&job_records, entry);

            if let Some(record) = record {
                for field in &job_fields {
                    if let Some(value) = match field {
                        ListJobField::Job => Some(record.id.clone()),
                        ListJobField::JobStatus => {
                            Some(jobs::status_label(record.status).to_string())
                        }
                        ListJobField::JobScope => record
                            .metadata
                            .as_ref()
                            .and_then(|meta| meta.scope.as_ref())
                            .map(|scope| scope.to_string()),
                        ListJobField::JobStarted => {
                            record.started_at.map(|value| value.to_rfc3339())
                        }
                    } {
                        obj.insert(field.json_key().to_string(), Value::String(value));
                    }
                }

                let job_id = record.id.clone();
                for field in &command_fields {
                    let value = match field {
                        ListCommandField::Status => format!("vizier jobs status {job_id}"),
                        ListCommandField::Logs => format!("vizier jobs tail --follow {job_id}"),
                        ListCommandField::Attach => format!("vizier jobs attach {job_id}"),
                    };
                    obj.insert(field.json_key().to_string(), Value::String(value));
                }
            }

            entries_json.push(Value::Object(obj));
        }

        let payload = json!({
            "header": header,
            "entries": entries_json,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let mut header_rows = Vec::new();
    for field in &header_fields {
        match field {
            ListHeaderField::Outcome => {
                header_rows.push((resolve_label(&labels, field.label()), outcome.clone()));
            }
            ListHeaderField::Target => {
                if let Some(target) = opts.target.as_ref() {
                    header_rows.push((resolve_label(&labels, field.label()), target.clone()));
                }
            }
        }
    }
    let header_block = if header_rows.is_empty() {
        String::new()
    } else {
        format_block(header_rows)
    };
    if !header_block.is_empty() {
        println!("{header_block}");
    }

    if entries.is_empty() {
        return Ok(());
    }

    if !header_block.is_empty() {
        println!();
    }

    let mut job_records = Vec::new();
    if !job_fields.is_empty() || !command_fields.is_empty() {
        let repo_root = vizier_core::vcs::repo_root()
            .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
        let jobs_root = repo_root.join(".vizier").join("jobs");
        match jobs::list_records(&jobs_root) {
            Ok(records) => job_records = records,
            Err(err) => {
                display::warn(format!("unable to load background jobs: {err}"));
            }
        }
    }

    match list_config.format {
        config::ListFormat::Table => {
            let mut rows = Vec::new();
            let mut header = Vec::new();
            for field in &entry_fields {
                header.push(resolve_label(&labels, field.label()));
            }
            for field in &job_fields {
                header.push(resolve_label(&labels, field.label()));
            }
            for field in &command_fields {
                header.push(resolve_label(&labels, field.label()));
            }
            if !header.is_empty() {
                rows.push(header);
            }

            for entry in &entries {
                let record = select_inline_job(&job_records, entry);
                let summary = format_summary(
                    &entry.summary,
                    list_config.summary_max_len,
                    list_config.summary_single_line,
                );
                let mut row = Vec::new();
                for field in &entry_fields {
                    let value = match field {
                        ListEntryField::Plan => entry.slug.clone(),
                        ListEntryField::Branch => entry.branch.clone(),
                        ListEntryField::Summary => summary.clone(),
                    };
                    row.push(value);
                }
                for field in &job_fields {
                    let value = record.and_then(|record| match field {
                        ListJobField::Job => Some(record.id.clone()),
                        ListJobField::JobStatus => {
                            Some(jobs::status_label(record.status).to_string())
                        }
                        ListJobField::JobScope => record
                            .metadata
                            .as_ref()
                            .and_then(|meta| meta.scope.as_ref())
                            .map(|scope| scope.to_string()),
                        ListJobField::JobStarted => {
                            record.started_at.map(|value| value.to_rfc3339())
                        }
                    });
                    row.push(value.unwrap_or_default());
                }
                for field in &command_fields {
                    let value = record.map(|record| match field {
                        ListCommandField::Status => format!("vizier jobs status {}", record.id),
                        ListCommandField::Logs => {
                            format!("vizier jobs tail --follow {}", record.id)
                        }
                        ListCommandField::Attach => format!("vizier jobs attach {}", record.id),
                    });
                    row.push(value.unwrap_or_default());
                }
                rows.push(row);
            }

            let table = format_table(&rows, 0);
            if !table.is_empty() {
                println!("{table}");
            }
        }
        _ => {
            let mut entry_blocks = Vec::new();
            for entry in &entries {
                let summary = format_summary(
                    &entry.summary,
                    list_config.summary_max_len,
                    list_config.summary_single_line,
                );
                let mut rows = Vec::new();
                for field in &entry_fields {
                    let value = match field {
                        ListEntryField::Plan => entry.slug.clone(),
                        ListEntryField::Branch => entry.branch.clone(),
                        ListEntryField::Summary => summary.clone(),
                    };
                    rows.push((resolve_label(&labels, field.label()), value));
                }

                if let Some(record) = select_inline_job(&job_records, entry) {
                    for field in &job_fields {
                        if let Some(value) = match field {
                            ListJobField::Job => Some(record.id.clone()),
                            ListJobField::JobStatus => {
                                Some(jobs::status_label(record.status).to_string())
                            }
                            ListJobField::JobScope => record
                                .metadata
                                .as_ref()
                                .and_then(|meta| meta.scope.as_ref())
                                .map(|scope| scope.to_string()),
                            ListJobField::JobStarted => {
                                record.started_at.map(|value| value.to_rfc3339())
                            }
                        } {
                            rows.push((resolve_label(&labels, field.label()), value));
                        }
                    }

                    for field in &command_fields {
                        let value = match field {
                            ListCommandField::Status => format!("vizier jobs status {}", record.id),
                            ListCommandField::Logs => {
                                format!("vizier jobs tail --follow {}", record.id)
                            }
                            ListCommandField::Attach => format!("vizier jobs attach {}", record.id),
                        };
                        rows.push((resolve_label(&labels, field.label()), value));
                    }
                }

                let block = format_block_with_indent(rows, 2);
                if !block.is_empty() {
                    entry_blocks.push(block);
                }
            }

            if !entry_blocks.is_empty() {
                println!("{}", entry_blocks.join("\n\n"));
            }
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
            child_args: Vec::new(),
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
            schedule: None,
        }
    }

    #[test]
    fn select_inline_job_prefers_active_and_newest() {
        let entry = crate::plan::PlanSlugEntry {
            plan_id: "pln_alpha".to_string(),
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
                JobStatus::Queued,
                chrono::Utc.with_ymd_and_hms(2024, 3, 4, 10, 0, 0).unwrap(),
                None,
            ),
        ];

        let selected = select_inline_job(&records, &entry).expect("select job");
        assert_eq!(selected.id, "third");
    }
}
