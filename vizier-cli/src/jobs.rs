use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration as StdDuration,
};
use vizier_core::display;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub status: JobStatus,
    pub command: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub stdout_path: String,
    pub stderr_path: String,
    pub session_path: Option<String>,
    #[serde(default)]
    pub outcome_path: Option<String>,
    #[serde(default)]
    pub metadata: Option<JobMetadata>,
    #[serde(default)]
    pub config_snapshot: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct JobPaths {
    pub job_dir: PathBuf,
    pub record_path: PathBuf,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct LaunchResult {
    pub record: JobRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobMetadata {
    pub scope: Option<String>,
    pub target: Option<String>,
    pub plan: Option<String>,
    pub branch: Option<String>,
    pub revision: Option<String>,
    pub agent_selector: Option<String>,
    pub agent_backend: Option<String>,
    pub agent_label: Option<String>,
    pub agent_command: Option<Vec<String>>,
    pub config_backend: Option<String>,
    pub config_agent_selector: Option<String>,
    pub config_agent_label: Option<String>,
    pub config_agent_command: Option<Vec<String>>,
    pub background_quiet: Option<bool>,
    pub agent_exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobOutcome {
    pub id: String,
    pub status: JobStatus,
    pub command: Vec<String>,
    pub exit_code: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub stdout_path: String,
    pub stderr_path: String,
    pub session_path: Option<String>,
    pub metadata: Option<JobMetadata>,
    #[serde(default)]
    pub config_snapshot: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug)]
pub enum LogStream {
    Stdout,
    Stderr,
    Both,
}

pub fn ensure_jobs_root(project_root: &Path) -> io::Result<PathBuf> {
    let root = project_root.join(".vizier").join("jobs");
    fs::create_dir_all(&root)?;
    Ok(root)
}

pub fn paths_for(jobs_root: &Path, job_id: &str) -> JobPaths {
    let job_dir = jobs_root.join(job_id);
    JobPaths {
        record_path: job_dir.join("job.json"),
        stdout_path: job_dir.join("stdout.log"),
        stderr_path: job_dir.join("stderr.log"),
        job_dir,
    }
}

fn relative_path(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn is_empty_vec(value: &Option<Vec<String>>) -> bool {
    value
        .as_ref()
        .map(|entries| entries.is_empty())
        .unwrap_or(true)
}

fn merge_metadata(
    existing: Option<JobMetadata>,
    update: Option<JobMetadata>,
) -> Option<JobMetadata> {
    match (existing, update) {
        (None, None) => None,
        (Some(meta), None) => Some(meta),
        (None, Some(meta)) => Some(meta),
        (Some(mut base), Some(update)) => {
            if base.scope.is_none() {
                base.scope = update.scope;
            }
            if base.target.is_none() {
                base.target = update.target;
            }
            if base.plan.is_none() {
                base.plan = update.plan;
            }
            if base.branch.is_none() {
                base.branch = update.branch;
            }
            if base.revision.is_none() {
                base.revision = update.revision;
            }
            if base.agent_selector.is_none() {
                base.agent_selector = update.agent_selector;
            }
            if base.agent_backend.is_none() {
                base.agent_backend = update.agent_backend;
            }
            if base.agent_label.is_none() {
                base.agent_label = update.agent_label;
            }
            if is_empty_vec(&base.agent_command) {
                base.agent_command = update.agent_command;
            }
            if base.config_agent_selector.is_none() {
                base.config_agent_selector = update.config_agent_selector;
            }
            if base.config_backend.is_none() {
                base.config_backend = update.config_backend;
            }
            if base.config_agent_label.is_none() {
                base.config_agent_label = update.config_agent_label;
            }
            if is_empty_vec(&base.config_agent_command) {
                base.config_agent_command = update.config_agent_command;
            }
            if base.background_quiet.is_none() {
                base.background_quiet = update.background_quiet;
            }
            if update.agent_exit_code.is_some() {
                base.agent_exit_code = update.agent_exit_code;
            }
            Some(base)
        }
    }
}

fn persist_record(paths: &JobPaths, record: &JobRecord) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = paths.record_path.parent() {
        fs::create_dir_all(parent)?;
    }

    serde_json::to_writer_pretty(File::create(&paths.record_path)?, record)?;
    Ok(())
}

fn load_record(paths: &JobPaths) -> Result<JobRecord, Box<dyn std::error::Error>> {
    let mut buf = String::new();
    File::open(&paths.record_path)?.read_to_string(&mut buf)?;
    let record: JobRecord = serde_json::from_str(&buf)?;
    Ok(record)
}

pub fn launch_background_job(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    job_id: &str,
    child_args: &[String],
    recorded_args: &[String],
    metadata: Option<JobMetadata>,
    config_snapshot: Option<serde_json::Value>,
) -> Result<LaunchResult, Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    fs::create_dir_all(&paths.job_dir)?;

    let stdout = File::create(&paths.stdout_path)?;
    let stderr = File::create(&paths.stderr_path)?;

    let now = Utc::now();
    let mut record = JobRecord {
        id: job_id.to_string(),
        status: JobStatus::Pending,
        command: recorded_args.to_vec(),
        created_at: now,
        started_at: None,
        finished_at: None,
        pid: None,
        exit_code: None,
        stdout_path: relative_path(project_root, &paths.stdout_path),
        stderr_path: relative_path(project_root, &paths.stderr_path),
        session_path: None,
        outcome_path: None,
        metadata,
        config_snapshot,
    };

    persist_record(&paths, &record)?;

    let mut child = Command::new(binary);
    child
        .args(child_args)
        .current_dir(project_root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));

    let child = child.spawn()?;
    record.status = JobStatus::Running;
    record.started_at = Some(Utc::now());
    record.pid = Some(child.id());
    persist_record(&paths, &record)?;

    Ok(LaunchResult { record })
}

pub fn finalize_job(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
    status: JobStatus,
    exit_code: i32,
    session_path: Option<String>,
    metadata: Option<JobMetadata>,
) -> Result<JobRecord, Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    let mut record = load_record(&paths)?;

    record.status = status;
    record.exit_code = Some(exit_code);
    record.finished_at = Some(Utc::now());

    if let Some(session) = session_path {
        record.session_path = Some(relative_path(project_root, Path::new(&session)));
    }

    record.metadata = merge_metadata(record.metadata.take(), metadata);

    persist_record(&paths, &record)?;

    // Best-effort for partial runs where log paths were missing from older records.
    if record.stdout_path.is_empty() {
        record.stdout_path = relative_path(project_root, &paths.stdout_path);
    }
    if record.stderr_path.is_empty() {
        record.stderr_path = relative_path(project_root, &paths.stderr_path);
    }

    if record.outcome_path.is_none() {
        record.outcome_path = write_outcome_file(project_root, &paths, &record)?;
        persist_record(&paths, &record)?;
    }

    Ok(record)
}

fn write_outcome_file(
    project_root: &Path,
    paths: &JobPaths,
    record: &JobRecord,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let outcome = JobOutcome {
        id: record.id.clone(),
        status: record.status,
        command: record.command.clone(),
        exit_code: record.exit_code,
        created_at: record.created_at,
        started_at: record.started_at,
        finished_at: record.finished_at,
        stdout_path: record.stdout_path.clone(),
        stderr_path: record.stderr_path.clone(),
        session_path: record.session_path.clone(),
        metadata: record.metadata.clone(),
        config_snapshot: record.config_snapshot.clone(),
    };

    let path = paths.job_dir.join("outcome.json");
    serde_json::to_writer_pretty(File::create(&path)?, &outcome)?;
    Ok(Some(relative_path(project_root, &path)))
}

pub fn list_records(jobs_root: &Path) -> Result<Vec<JobRecord>, Box<dyn std::error::Error>> {
    let mut records = Vec::new();
    if !jobs_root.exists() {
        return Ok(records);
    }

    for entry in fs::read_dir(jobs_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let id = entry.file_name();
        let job_id = id.to_string_lossy();
        let paths = paths_for(jobs_root, &job_id);
        if !paths.record_path.exists() {
            continue;
        }

        match load_record(&paths) {
            Ok(record) => records.push(record),
            Err(err) => display::warn(format!("unable to load background job {}: {}", job_id, err)),
        }
    }

    records.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(records)
}

pub fn read_record(
    jobs_root: &Path,
    job_id: &str,
) -> Result<JobRecord, Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    if !paths.record_path.exists() {
        return Err(format!("no background job {}", job_id).into());
    }

    load_record(&paths)
}

pub fn status_label(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Pending => "pending",
        JobStatus::Running => "running",
        JobStatus::Succeeded => "succeeded",
        JobStatus::Failed => "failed",
        JobStatus::Cancelled => "cancelled",
    }
}

fn emit_log(path: &Path, offset: u64, label: &str, labeled: bool) -> io::Result<u64> {
    if !path.exists() {
        return Ok(offset);
    }

    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buffer = String::new();
    file.read_to_string(&mut buffer)?;
    let new_offset = file.stream_position()?;

    if !buffer.is_empty() {
        if labeled {
            for line in buffer.lines() {
                println!("[{label}] {line}");
            }
        } else {
            print!("{buffer}");
        }
        let _ = std::io::stdout().flush();
    }

    Ok(new_offset)
}

pub fn tail_job_logs(
    jobs_root: &Path,
    job_id: &str,
    stream: LogStream,
    follow: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    let mut stdout_offset = 0u64;
    let mut stderr_offset = 0u64;

    let label_stdout = matches!(stream, LogStream::Both);
    let label_stderr = matches!(stream, LogStream::Both);

    loop {
        let mut advanced = false;
        if matches!(stream, LogStream::Stdout | LogStream::Both) {
            let next = emit_log(&paths.stdout_path, stdout_offset, "stdout", label_stdout)?;
            if next != stdout_offset {
                advanced = true;
                stdout_offset = next;
            }
        }

        if matches!(stream, LogStream::Stderr | LogStream::Both) {
            let next = emit_log(&paths.stderr_path, stderr_offset, "stderr", label_stderr)?;
            if next != stderr_offset {
                advanced = true;
                stderr_offset = next;
            }
        }

        if !follow {
            break;
        }

        let record = read_record(jobs_root, job_id)?;
        let running = matches!(record.status, JobStatus::Running | JobStatus::Pending);
        if !running && !advanced {
            break;
        }

        thread::sleep(StdDuration::from_millis(400));
    }

    Ok(())
}

pub fn cancel_job(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
) -> Result<JobRecord, Box<dyn std::error::Error>> {
    let record = read_record(jobs_root, job_id)?;
    if !matches!(record.status, JobStatus::Running | JobStatus::Pending) {
        return Err(format!("job {job_id} is not running").into());
    }

    let pid = record
        .pid
        .ok_or_else(|| format!("job {job_id} has no pid recorded"))?;

    let status = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()?;
    if !status.success() {
        return Err(format!("failed to signal job {job_id} (pid {pid})").into());
    }

    finalize_job(
        project_root,
        jobs_root,
        job_id,
        JobStatus::Cancelled,
        143,
        None,
        None,
    )
}

pub fn gc_jobs(
    _project_root: &Path,
    jobs_root: &Path,
    older_than: Duration,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut removed = 0usize;
    let cutoff = Utc::now() - older_than;
    if !jobs_root.exists() {
        return Ok(removed);
    }
    for entry in fs::read_dir(jobs_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        let Ok(record) = read_record(jobs_root, &id) else {
            continue;
        };
        let finished_at = record.finished_at.unwrap_or(record.created_at);
        if matches!(record.status, JobStatus::Running | JobStatus::Pending) {
            continue;
        }
        if finished_at < cutoff {
            let paths = paths_for(jobs_root, &id);
            if paths.job_dir.exists() {
                if fs::remove_dir_all(&paths.job_dir).is_ok() {
                    removed += 1;
                }
            }
        }
    }

    // Refresh the top-level jobs root when empty to keep lookups predictable.
    if removed > 0 && jobs_root.exists() && fs::read_dir(jobs_root)?.next().is_none() {
        let _ = fs::remove_dir_all(jobs_root);
        let _ = fs::create_dir_all(jobs_root);
    }

    Ok(removed)
}
