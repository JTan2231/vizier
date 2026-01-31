use chrono::{DateTime, Duration, Utc};
use git2::{Repository, WorktreePruneOptions};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    thread,
    time::Duration as StdDuration,
};
use uuid::Uuid;
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

#[allow(dead_code)]
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
    pub queue_id: Option<String>,
    pub queue_entry_id: Option<String>,
    pub queue_position: Option<u32>,
    pub worktree_name: Option<String>,
    pub worktree_path: Option<String>,
    pub worktree_owned: Option<bool>,
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
    pub cancel_cleanup_status: Option<CancelCleanupStatus>,
    pub cancel_cleanup_error: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeQueueEntry {
    pub id: String,
    pub plan: String,
    pub target: Option<String>,
    pub branch: Option<String>,
    pub queued_at: DateTime<Utc>,
    pub recorded_args: Vec<String>,
    pub child_args: Vec<String>,
    pub position: u32,
    pub metadata: JobMetadata,
    #[serde(default)]
    pub config_snapshot: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeQueueState {
    pub queue_id: String,
    pub active_job_id: Option<String>,
    pub active_entry_id: Option<String>,
    pub active_plan: Option<String>,
    pub blocked: bool,
    pub blocked_reason: Option<String>,
    pub entries: Vec<MergeQueueEntry>,
    pub updated_at: DateTime<Utc>,
}

impl MergeQueueState {
    fn new() -> Self {
        Self {
            queue_id: Uuid::new_v4().simple().to_string(),
            active_job_id: None,
            active_entry_id: None,
            active_plan: None,
            blocked: false,
            blocked_reason: None,
            entries: Vec::new(),
            updated_at: Utc::now(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum LogStream {
    Stdout,
    Stderr,
    Both,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CancelCleanupStatus {
    Skipped,
    Done,
    Failed,
}

impl CancelCleanupStatus {
    pub fn label(self) -> &'static str {
        match self {
            CancelCleanupStatus::Skipped => "skipped",
            CancelCleanupStatus::Done => "done",
            CancelCleanupStatus::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CancelCleanupResult {
    pub status: CancelCleanupStatus,
    pub error: Option<String>,
}

impl CancelCleanupResult {
    fn skipped() -> Self {
        Self {
            status: CancelCleanupStatus::Skipped,
            error: None,
        }
    }
}

static CURRENT_JOB_ID: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn current_job_id_state() -> &'static Mutex<Option<String>> {
    CURRENT_JOB_ID.get_or_init(|| Mutex::new(None))
}

pub fn set_current_job_id(job_id: Option<String>) {
    let mut state = current_job_id_state().lock().expect("lock current job id");
    *state = job_id;
}

pub fn current_job_id() -> Option<String> {
    current_job_id_state()
        .lock()
        .expect("lock current job id")
        .clone()
}

pub fn ensure_jobs_root(project_root: &Path) -> io::Result<PathBuf> {
    let root = project_root.join(".vizier").join("jobs");
    fs::create_dir_all(&root)?;
    Ok(root)
}

fn merge_queue_path(jobs_root: &Path) -> PathBuf {
    jobs_root.join("merge-queue.json")
}

fn merge_queue_lock_path(jobs_root: &Path) -> PathBuf {
    jobs_root.join("merge-queue.lock")
}

pub struct MergeQueueLock {
    path: PathBuf,
}

impl MergeQueueLock {
    pub fn acquire(jobs_root: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        fs::create_dir_all(jobs_root)?;
        let path = merge_queue_lock_path(jobs_root);
        let mut attempts = 0u32;
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "pid={}", std::process::id());
                    return Ok(Self { path });
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    attempts += 1;
                    if attempts > 40 {
                        return Err("merge queue is busy; retry the command".into());
                    }
                    thread::sleep(StdDuration::from_millis(50));
                }
                Err(err) => return Err(Box::new(err)),
            }
        }
    }
}

impl Drop for MergeQueueLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn load_merge_queue(jobs_root: &Path) -> Result<MergeQueueState, Box<dyn std::error::Error>> {
    let path = merge_queue_path(jobs_root);
    if !path.exists() {
        return Ok(MergeQueueState::new());
    }
    let mut buf = String::new();
    File::open(&path)?.read_to_string(&mut buf)?;
    let state = serde_json::from_str(&buf)?;
    Ok(state)
}

pub fn read_merge_queue(
    jobs_root: &Path,
) -> Result<Option<MergeQueueState>, Box<dyn std::error::Error>> {
    let path = merge_queue_path(jobs_root);
    if !path.exists() {
        return Ok(None);
    }
    let mut buf = String::new();
    File::open(&path)?.read_to_string(&mut buf)?;
    let state = serde_json::from_str(&buf)?;
    Ok(Some(state))
}

#[derive(Debug, Clone)]
pub struct MergeQueueStatus {
    pub blocked: bool,
    pub blocked_reason: Option<String>,
}

pub fn merge_queue_status_for(
    jobs_root: &Path,
    queue_id: &str,
) -> Result<Option<MergeQueueStatus>, Box<dyn std::error::Error>> {
    let Some(state) = read_merge_queue(jobs_root)? else {
        return Ok(None);
    };
    if state.queue_id != queue_id {
        return Ok(None);
    }
    Ok(Some(MergeQueueStatus {
        blocked: state.blocked,
        blocked_reason: state.blocked_reason,
    }))
}

pub fn persist_merge_queue(
    jobs_root: &Path,
    state: &MergeQueueState,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(jobs_root)?;
    let path = merge_queue_path(jobs_root);
    let tmp = path.with_extension("json.tmp");
    let contents = serde_json::to_vec_pretty(state)?;
    fs::write(&tmp, contents)?;
    fs::rename(tmp, path)?;
    Ok(())
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
            if base.queue_id.is_none() {
                base.queue_id = update.queue_id;
            }
            if base.queue_entry_id.is_none() {
                base.queue_entry_id = update.queue_entry_id;
            }
            if base.queue_position.is_none() {
                base.queue_position = update.queue_position;
            }
            if base.worktree_name.is_none() {
                base.worktree_name = update.worktree_name;
            }
            if base.worktree_path.is_none() {
                base.worktree_path = update.worktree_path;
            }
            if base.worktree_owned.is_none() {
                base.worktree_owned = update.worktree_owned;
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
            if update.cancel_cleanup_status.is_some() {
                base.cancel_cleanup_status = update.cancel_cleanup_status;
            }
            if update.cancel_cleanup_error.is_some() {
                base.cancel_cleanup_error = update.cancel_cleanup_error;
            }
            Some(base)
        }
    }
}

fn persist_record(paths: &JobPaths, record: &JobRecord) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = paths.record_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp = paths.record_path.with_extension("json.tmp");
    let contents = serde_json::to_vec_pretty(record)?;
    fs::write(&tmp, contents)?;
    fs::rename(tmp, &paths.record_path)?;
    Ok(())
}

fn load_record(paths: &JobPaths) -> Result<JobRecord, Box<dyn std::error::Error>> {
    let mut buf = String::new();
    File::open(&paths.record_path)?.read_to_string(&mut buf)?;
    let record: JobRecord = serde_json::from_str(&buf)?;
    Ok(record)
}

#[allow(dead_code, clippy::too_many_arguments)]
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

pub fn find_running_merge_job(
    jobs_root: &Path,
) -> Result<Option<JobRecord>, Box<dyn std::error::Error>> {
    let records = list_records(jobs_root)?;
    for record in records {
        if !matches!(record.status, JobStatus::Pending | JobStatus::Running) {
            continue;
        }
        let scope = record
            .metadata
            .as_ref()
            .and_then(|meta| meta.scope.as_deref());
        if scope == Some("merge") {
            return Ok(Some(record));
        }
    }
    Ok(None)
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

fn read_log_chunk(path: &Path, offset: u64) -> io::Result<(u64, Vec<u8>)> {
    if !path.exists() {
        return Ok((offset, Vec::new()));
    }

    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let new_offset = file.stream_position()?;
    Ok((new_offset, buffer))
}

pub fn follow_job_logs_raw(
    jobs_root: &Path,
    job_id: &str,
) -> Result<i32, Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    let mut stdout_offset = 0u64;
    let mut stderr_offset = 0u64;

    loop {
        let mut advanced = false;

        let (next_stdout, stdout_buf) = read_log_chunk(&paths.stdout_path, stdout_offset)?;
        if !stdout_buf.is_empty() {
            io::stdout().write_all(&stdout_buf)?;
            io::stdout().flush()?;
            advanced = true;
        }
        stdout_offset = next_stdout;

        let (next_stderr, stderr_buf) = read_log_chunk(&paths.stderr_path, stderr_offset)?;
        if !stderr_buf.is_empty() {
            io::stderr().write_all(&stderr_buf)?;
            io::stderr().flush()?;
            advanced = true;
        }
        stderr_offset = next_stderr;

        let record = read_record(jobs_root, job_id)?;
        let running = matches!(record.status, JobStatus::Running | JobStatus::Pending);
        if !running && !advanced {
            return Ok(record.exit_code.unwrap_or(1));
        }

        thread::sleep(StdDuration::from_millis(300));
    }
}

pub struct CancelJobOutcome {
    pub record: JobRecord,
    pub cleanup: CancelCleanupResult,
}

pub fn cancel_job_with_cleanup(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
    cleanup_worktree: bool,
) -> Result<CancelJobOutcome, Box<dyn std::error::Error>> {
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

    let cleanup = if cleanup_worktree {
        attempt_cancel_cleanup(project_root, &record, pid)
    } else {
        CancelCleanupResult::skipped()
    };

    let cleanup_metadata = JobMetadata {
        cancel_cleanup_status: Some(cleanup.status),
        cancel_cleanup_error: cleanup.error.clone(),
        ..JobMetadata::default()
    };

    let record = finalize_job(
        project_root,
        jobs_root,
        job_id,
        JobStatus::Cancelled,
        143,
        None,
        Some(cleanup_metadata),
    )?;

    Ok(CancelJobOutcome { record, cleanup })
}

pub fn record_job_worktree(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
    worktree_name: Option<&str>,
    worktree_path: &Path,
) -> Result<JobRecord, Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    if !paths.record_path.exists() {
        return Err(format!("no background job {}", job_id).into());
    }

    let mut record = load_record(&paths)?;
    let update = JobMetadata {
        worktree_owned: Some(true),
        worktree_path: Some(relative_path(project_root, worktree_path)),
        worktree_name: worktree_name.map(|name| name.to_string()),
        ..JobMetadata::default()
    };

    record.metadata = merge_metadata(record.metadata.take(), Some(update));
    persist_record(&paths, &record)?;
    Ok(record)
}

pub fn record_current_job_worktree(
    project_root: &Path,
    worktree_name: Option<&str>,
    worktree_path: &Path,
) {
    let Some(job_id) = current_job_id() else {
        return;
    };

    let jobs_root = match ensure_jobs_root(project_root) {
        Ok(path) => path,
        Err(err) => {
            display::warn(format!(
                "unable to ensure jobs root for worktree recording: {err}"
            ));
            return;
        }
    };

    if let Err(err) = record_job_worktree(
        project_root,
        &jobs_root,
        &job_id,
        worktree_name,
        worktree_path,
    ) {
        display::warn(format!(
            "unable to record worktree metadata for job {}: {err}",
            job_id
        ));
    }
}

fn attempt_cancel_cleanup(
    project_root: &Path,
    record: &JobRecord,
    pid: u32,
) -> CancelCleanupResult {
    if !wait_for_exit(pid, StdDuration::from_secs(2)) {
        return CancelCleanupResult::skipped();
    }

    let Some(metadata) = record.metadata.as_ref() else {
        return CancelCleanupResult::skipped();
    };

    if metadata.worktree_owned != Some(true) {
        return CancelCleanupResult::skipped();
    }

    let Some(worktree_path) = metadata.worktree_path.as_ref() else {
        return CancelCleanupResult::skipped();
    };

    let worktree_path = resolve_recorded_path(project_root, worktree_path);
    let worktree_name = metadata.worktree_name.as_deref();
    if !worktree_safe_to_remove(project_root, &worktree_path, worktree_name) {
        return CancelCleanupResult::skipped();
    }

    match cleanup_worktree(project_root, &worktree_path, worktree_name) {
        Ok(()) => CancelCleanupResult {
            status: CancelCleanupStatus::Done,
            error: None,
        },
        Err(err) => CancelCleanupResult {
            status: CancelCleanupStatus::Failed,
            error: Some(err),
        },
    }
}

fn resolve_recorded_path(project_root: &Path, recorded: &str) -> PathBuf {
    let path = PathBuf::from(recorded);
    if path.is_absolute() {
        path
    } else {
        project_root.join(path)
    }
}

fn worktree_safe_to_remove(
    project_root: &Path,
    worktree_path: &Path,
    worktree_name: Option<&str>,
) -> bool {
    let tmp_root = project_root.join(".vizier/tmp-worktrees");
    if !worktree_path.starts_with(&tmp_root) {
        return false;
    }

    if let Some(name) = worktree_name
        && name.starts_with("vizier-workspace-")
    {
        return false;
    }

    if let Some(dir_name) = worktree_path.file_name().and_then(|name| name.to_str())
        && dir_name.starts_with("workspace-")
    {
        return false;
    }

    true
}

fn cleanup_worktree(
    project_root: &Path,
    worktree_path: &Path,
    worktree_name: Option<&str>,
) -> Result<(), String> {
    let repo = Repository::open(project_root).map_err(|err| err.to_string())?;
    let mut errors = Vec::new();

    if let Some(name) = worktree_name
        .map(|value| value.to_string())
        .or_else(|| find_worktree_name_by_path(&repo, worktree_path))
        && let Err(err) = prune_worktree(&repo, &name)
    {
        errors.push(format!("failed to prune worktree {}: {}", name, err));
    }

    if worktree_path.exists()
        && let Err(err) = fs::remove_dir_all(worktree_path)
    {
        errors.push(format!(
            "failed to remove worktree directory {}: {}",
            worktree_path.display(),
            err
        ));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn prune_worktree(repo: &Repository, worktree_name: &str) -> Result<(), git2::Error> {
    let worktree = repo.find_worktree(worktree_name)?;
    let mut opts = WorktreePruneOptions::new();
    opts.valid(true).locked(true).working_tree(true);
    worktree.prune(Some(&mut opts))
}

fn find_worktree_name_by_path(repo: &Repository, worktree_path: &Path) -> Option<String> {
    let target = worktree_path.canonicalize().ok()?;
    let worktrees = repo.worktrees().ok()?;
    for name in worktrees.iter().flatten() {
        if let Ok(worktree) = repo.find_worktree(name)
            && worktree.path().canonicalize().ok() == Some(target.clone())
        {
            return Some(name.to_string());
        }
    }
    None
}

fn wait_for_exit(pid: u32, timeout: StdDuration) -> bool {
    let start = std::time::Instant::now();
    loop {
        if !pid_is_running(pid) {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        thread::sleep(StdDuration::from_millis(100));
    }
}

fn pid_is_running(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
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
            if paths.job_dir.exists() && fs::remove_dir_all(&paths.job_dir).is_ok() {
                removed += 1;
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
