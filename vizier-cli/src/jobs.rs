use chrono::{DateTime, Duration, Utc};
use git2::{Oid, Repository, WorktreePruneOptions};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    thread,
    time::Duration as StdDuration,
};
use vizier_core::display;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    WaitingOnDeps,
    WaitingOnLocks,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    BlockedByDependency,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum JobArtifact {
    PlanBranch { slug: String, branch: String },
    PlanDoc { slug: String, branch: String },
    PlanCommits { slug: String, branch: String },
    TargetBranch { name: String },
    MergeSentinel { slug: String },
    AskSavePatch { job_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobDependency {
    pub artifact: JobArtifact,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LockMode {
    Shared,
    Exclusive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobLock {
    pub key: String,
    pub mode: LockMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PinnedHead {
    pub branch: String,
    pub oid: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobWaitKind {
    Dependencies,
    Locks,
    PinnedHead,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobWaitReason {
    pub kind: JobWaitKind,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobSchedule {
    #[serde(default)]
    pub dependencies: Vec<JobDependency>,
    #[serde(default)]
    pub locks: Vec<JobLock>,
    #[serde(default)]
    pub artifacts: Vec<JobArtifact>,
    #[serde(default)]
    pub pinned_head: Option<PinnedHead>,
    #[serde(default)]
    pub wait_reason: Option<JobWaitReason>,
    #[serde(default)]
    pub waited_on: Vec<JobWaitKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub status: JobStatus,
    pub command: Vec<String>,
    #[serde(default)]
    pub child_args: Vec<String>,
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
    #[serde(default)]
    pub schedule: Option<JobSchedule>,
}

#[derive(Debug, Clone)]
pub struct JobPaths {
    pub job_dir: PathBuf,
    pub record_path: PathBuf,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobMetadata {
    pub scope: Option<String>,
    pub target: Option<String>,
    pub plan: Option<String>,
    pub branch: Option<String>,
    pub revision: Option<String>,
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
    #[serde(default)]
    pub child_args: Vec<String>,
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
    #[serde(default)]
    pub schedule: Option<JobSchedule>,
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

fn scheduler_lock_path(jobs_root: &Path) -> PathBuf {
    jobs_root.join("scheduler.lock")
}

pub struct SchedulerLock {
    path: PathBuf,
}

impl SchedulerLock {
    pub fn acquire(jobs_root: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        fs::create_dir_all(jobs_root)?;
        let path = scheduler_lock_path(jobs_root);
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
                        return Err("scheduler is busy; retry the command".into());
                    }
                    thread::sleep(StdDuration::from_millis(50));
                }
                Err(err) => return Err(Box::new(err)),
            }
        }
    }
}

impl Drop for SchedulerLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
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

pub(crate) fn ask_save_patch_path(jobs_root: &Path, job_id: &str) -> PathBuf {
    jobs_root.join(job_id).join("ask-save.patch")
}

pub(crate) fn save_input_patch_path(jobs_root: &Path, job_id: &str) -> PathBuf {
    jobs_root.join(job_id).join("save-input.patch")
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

#[derive(Debug, Default)]
pub struct SchedulerOutcome {
    pub started: Vec<String>,
    pub updated: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
pub fn enqueue_job(
    project_root: &Path,
    jobs_root: &Path,
    job_id: &str,
    child_args: &[String],
    recorded_args: &[String],
    metadata: Option<JobMetadata>,
    config_snapshot: Option<serde_json::Value>,
    schedule: Option<JobSchedule>,
) -> Result<JobRecord, Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    fs::create_dir_all(&paths.job_dir)?;

    let _ = File::create(&paths.stdout_path)?;
    let _ = File::create(&paths.stderr_path)?;

    let now = Utc::now();
    let record = JobRecord {
        id: job_id.to_string(),
        status: JobStatus::Queued,
        command: recorded_args.to_vec(),
        child_args: child_args.to_vec(),
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
        schedule,
    };

    persist_record(&paths, &record)?;
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
    schedule: Option<JobSchedule>,
) -> Result<JobRecord, Box<dyn std::error::Error>> {
    enqueue_job(
        project_root,
        jobs_root,
        job_id,
        child_args,
        recorded_args,
        metadata,
        config_snapshot,
        schedule,
    )?;
    start_job(project_root, jobs_root, binary, job_id)
}

pub fn start_job(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    job_id: &str,
) -> Result<JobRecord, Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    let mut record = load_record(&paths)?;

    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.stdout_path)?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.stderr_path)?;

    let mut child = Command::new(binary);
    child
        .args(&record.child_args)
        .current_dir(project_root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));

    let child = child.spawn()?;
    record.status = JobStatus::Running;
    record.started_at = Some(Utc::now());
    record.pid = Some(child.id());
    persist_record(&paths, &record)?;

    Ok(record)
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
        child_args: record.child_args.clone(),
        exit_code: record.exit_code,
        created_at: record.created_at,
        started_at: record.started_at,
        finished_at: record.finished_at,
        stdout_path: record.stdout_path.clone(),
        stderr_path: record.stderr_path.clone(),
        session_path: record.session_path.clone(),
        metadata: record.metadata.clone(),
        config_snapshot: record.config_snapshot.clone(),
        schedule: record.schedule.clone(),
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

fn job_is_terminal(status: JobStatus) -> bool {
    matches!(
        status,
        JobStatus::Succeeded
            | JobStatus::Failed
            | JobStatus::Cancelled
            | JobStatus::BlockedByDependency
    )
}

fn job_is_active(status: JobStatus) -> bool {
    matches!(
        status,
        JobStatus::Queued
            | JobStatus::WaitingOnDeps
            | JobStatus::WaitingOnLocks
            | JobStatus::Running
    )
}

fn format_artifact(artifact: &JobArtifact) -> String {
    match artifact {
        JobArtifact::PlanBranch { slug, branch } => format!("plan_branch:{slug} ({branch})"),
        JobArtifact::PlanDoc { slug, branch } => format!("plan_doc:{slug} ({branch})"),
        JobArtifact::PlanCommits { slug, branch } => format!("plan_commits:{slug} ({branch})"),
        JobArtifact::TargetBranch { name } => format!("target_branch:{name}"),
        JobArtifact::MergeSentinel { slug } => format!("merge_sentinel:{slug}"),
        JobArtifact::AskSavePatch { job_id } => format!("ask_save_patch:{job_id}"),
    }
}

fn artifact_exists(repo: &Repository, artifact: &JobArtifact) -> bool {
    match artifact {
        JobArtifact::PlanBranch { branch, .. } | JobArtifact::PlanCommits { branch, .. } => {
            repo.find_branch(branch, git2::BranchType::Local).is_ok()
        }
        JobArtifact::PlanDoc { slug, branch } => {
            let plan_path = crate::plan::plan_rel_path(slug);
            let Ok(branch_ref) = repo.find_branch(branch, git2::BranchType::Local) else {
                return false;
            };
            let Ok(commit) = branch_ref.into_reference().peel_to_commit() else {
                return false;
            };
            let Ok(tree) = commit.tree() else {
                return false;
            };
            tree.get_path(&plan_path).is_ok()
        }
        JobArtifact::TargetBranch { name } => {
            repo.find_branch(name, git2::BranchType::Local).is_ok()
        }
        JobArtifact::MergeSentinel { slug } => {
            let path = repo
                .path()
                .join(".vizier/tmp/merge-conflicts")
                .join(format!("{slug}.json"));
            path.exists()
        }
        JobArtifact::AskSavePatch { job_id } => {
            let repo_root = repo.path().parent().unwrap_or_else(|| Path::new("."));
            let jobs_root = repo_root.join(".vizier/jobs");
            ask_save_patch_path(&jobs_root, job_id).exists()
        }
    }
}

fn pinned_head_matches(repo: &Repository, pinned: &PinnedHead) -> Result<bool, git2::Error> {
    let branch_ref = repo.find_branch(&pinned.branch, git2::BranchType::Local)?;
    let commit = branch_ref.into_reference().peel_to_commit()?;
    let expected = Oid::from_str(&pinned.oid).ok();
    Ok(Some(commit.id()) == expected)
}

#[derive(Default)]
struct LockState {
    exclusive: HashMap<String, usize>,
    shared: HashMap<String, usize>,
}

impl LockState {
    fn can_acquire(&self, lock: &JobLock) -> bool {
        match lock.mode {
            LockMode::Exclusive => {
                !self.exclusive.contains_key(&lock.key) && !self.shared.contains_key(&lock.key)
            }
            LockMode::Shared => !self.exclusive.contains_key(&lock.key),
        }
    }

    fn can_acquire_all(&self, locks: &[JobLock]) -> bool {
        locks.iter().all(|lock| self.can_acquire(lock))
    }

    fn acquire(&mut self, locks: &[JobLock]) {
        for lock in locks {
            match lock.mode {
                LockMode::Exclusive => {
                    *self.exclusive.entry(lock.key.clone()).or_insert(0) += 1;
                }
                LockMode::Shared => {
                    *self.shared.entry(lock.key.clone()).or_insert(0) += 1;
                }
            }
        }
    }
}

enum DependencyState {
    Ready,
    Waiting { detail: String },
    Blocked { detail: String },
}

fn dependency_state(
    repo: &Repository,
    deps: &[JobDependency],
    producers: &HashMap<JobArtifact, Vec<JobStatus>>,
) -> DependencyState {
    for dep in deps {
        let artifact = &dep.artifact;
        if artifact_exists(repo, artifact) {
            continue;
        }
        match producers.get(artifact) {
            Some(statuses) if statuses.iter().any(|status| job_is_active(*status)) => {
                return DependencyState::Waiting {
                    detail: format!("waiting on {}", format_artifact(artifact)),
                };
            }
            Some(statuses)
                if statuses
                    .iter()
                    .any(|status| matches!(status, JobStatus::Succeeded)) =>
            {
                // Artifact still missing even after producer success.
                return DependencyState::Blocked {
                    detail: format!("missing {}", format_artifact(artifact)),
                };
            }
            Some(_) => {
                return DependencyState::Blocked {
                    detail: format!("dependency failed for {}", format_artifact(artifact)),
                };
            }
            None => {
                return DependencyState::Blocked {
                    detail: format!("missing {}", format_artifact(artifact)),
                };
            }
        }
    }

    DependencyState::Ready
}

fn note_waited(schedule: &mut JobSchedule, kind: JobWaitKind) {
    if !schedule.waited_on.contains(&kind) {
        schedule.waited_on.push(kind);
    }
}

pub fn scheduler_tick(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
) -> Result<SchedulerOutcome, Box<dyn std::error::Error>> {
    let _lock = SchedulerLock::acquire(jobs_root)?;
    let mut records = list_records(jobs_root)?;
    let mut outcome = SchedulerOutcome::default();

    if records.is_empty() {
        return Ok(outcome);
    }

    let repo = Repository::discover(project_root)?;

    let mut producers: HashMap<JobArtifact, Vec<JobStatus>> = HashMap::new();
    for record in &records {
        if let Some(schedule) = record.schedule.as_ref() {
            for artifact in &schedule.artifacts {
                if job_is_active(record.status) || record.status == JobStatus::Succeeded {
                    producers
                        .entry(artifact.clone())
                        .or_default()
                        .push(record.status);
                }
            }
        }
    }

    let mut lock_state = LockState::default();
    for record in &records {
        if record.status == JobStatus::Running
            && let Some(schedule) = record.schedule.as_ref()
        {
            lock_state.acquire(&schedule.locks);
        }
    }

    records.sort_by(|a, b| a.created_at.cmp(&b.created_at));

    for mut record in records {
        if job_is_terminal(record.status) || record.status == JobStatus::Running {
            continue;
        }

        let mut schedule = record.schedule.clone().unwrap_or_default();
        let dep_state = dependency_state(&repo, &schedule.dependencies, &producers);
        match dep_state {
            DependencyState::Blocked { detail } => {
                schedule.wait_reason = Some(JobWaitReason {
                    kind: JobWaitKind::Dependencies,
                    detail: Some(detail),
                });
                note_waited(&mut schedule, JobWaitKind::Dependencies);
                if record.status != JobStatus::BlockedByDependency
                    || record
                        .schedule
                        .as_ref()
                        .and_then(|s| s.wait_reason.as_ref())
                        != schedule.wait_reason.as_ref()
                {
                    record.status = JobStatus::BlockedByDependency;
                    record.schedule = Some(schedule);
                    persist_record(&paths_for(jobs_root, &record.id), &record)?;
                    outcome.updated.push(record.id.clone());
                }
                continue;
            }
            DependencyState::Waiting { detail } => {
                schedule.wait_reason = Some(JobWaitReason {
                    kind: JobWaitKind::Dependencies,
                    detail: Some(detail),
                });
                note_waited(&mut schedule, JobWaitKind::Dependencies);
                if record.status != JobStatus::WaitingOnDeps
                    || record
                        .schedule
                        .as_ref()
                        .and_then(|s| s.wait_reason.as_ref())
                        != schedule.wait_reason.as_ref()
                {
                    record.status = JobStatus::WaitingOnDeps;
                    record.schedule = Some(schedule);
                    persist_record(&paths_for(jobs_root, &record.id), &record)?;
                    outcome.updated.push(record.id.clone());
                }
                continue;
            }
            DependencyState::Ready => {}
        }

        if let Some(pinned) = schedule.pinned_head.as_ref()
            && !pinned_head_matches(&repo, pinned)?
        {
            schedule.wait_reason = Some(JobWaitReason {
                kind: JobWaitKind::PinnedHead,
                detail: Some(format!("pinned head mismatch on {}", pinned.branch)),
            });
            note_waited(&mut schedule, JobWaitKind::PinnedHead);
            if record.status != JobStatus::WaitingOnDeps
                || record
                    .schedule
                    .as_ref()
                    .and_then(|s| s.wait_reason.as_ref())
                    != schedule.wait_reason.as_ref()
            {
                record.status = JobStatus::WaitingOnDeps;
                record.schedule = Some(schedule);
                persist_record(&paths_for(jobs_root, &record.id), &record)?;
                outcome.updated.push(record.id.clone());
            }
            continue;
        }

        if !lock_state.can_acquire_all(&schedule.locks) {
            schedule.wait_reason = Some(JobWaitReason {
                kind: JobWaitKind::Locks,
                detail: Some("waiting on locks".to_string()),
            });
            note_waited(&mut schedule, JobWaitKind::Locks);
            if record.status != JobStatus::WaitingOnLocks
                || record
                    .schedule
                    .as_ref()
                    .and_then(|s| s.wait_reason.as_ref())
                    != schedule.wait_reason.as_ref()
            {
                record.status = JobStatus::WaitingOnLocks;
                record.schedule = Some(schedule);
                persist_record(&paths_for(jobs_root, &record.id), &record)?;
                outcome.updated.push(record.id.clone());
            }
            continue;
        }

        if record.child_args.is_empty() {
            record.status = JobStatus::Failed;
            schedule.wait_reason = Some(JobWaitReason {
                kind: JobWaitKind::Dependencies,
                detail: Some("missing child args".to_string()),
            });
            record.schedule = Some(schedule);
            persist_record(&paths_for(jobs_root, &record.id), &record)?;
            outcome.updated.push(record.id.clone());
            continue;
        }

        schedule.wait_reason = None;
        record.schedule = Some(schedule);
        persist_record(&paths_for(jobs_root, &record.id), &record)?;
        start_job(project_root, jobs_root, binary, &record.id)?;
        lock_state.acquire(
            record
                .schedule
                .as_ref()
                .map(|s| s.locks.as_slice())
                .unwrap_or_default(),
        );
        outcome.started.push(record.id.clone());
    }

    Ok(outcome)
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
        JobStatus::Queued => "queued",
        JobStatus::WaitingOnDeps => "waiting_on_deps",
        JobStatus::WaitingOnLocks => "waiting_on_locks",
        JobStatus::Running => "running",
        JobStatus::Succeeded => "succeeded",
        JobStatus::Failed => "failed",
        JobStatus::Cancelled => "cancelled",
        JobStatus::BlockedByDependency => "blocked_by_dependency",
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
        let running = job_is_active(record.status);
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
        let running = job_is_active(record.status);
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
    if !job_is_active(record.status) {
        return Err(format!("job {job_id} is not active").into());
    }

    let cleanup = if let Some(pid) = record.pid {
        let status = Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()?;
        if !status.success() {
            return Err(format!("failed to signal job {job_id} (pid {pid})").into());
        }

        if cleanup_worktree {
            attempt_cancel_cleanup(project_root, &record, pid)
        } else {
            CancelCleanupResult::skipped()
        }
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
        if job_is_active(record.status) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn init_repo(temp: &TempDir) -> Result<Repository, git2::Error> {
        let repo = Repository::init(temp.path())?;
        Ok(repo)
    }

    #[test]
    fn scheduler_lock_busy_returns_error() {
        let temp = TempDir::new().expect("temp dir");
        let jobs_root = temp.path().join(".vizier/jobs");
        fs::create_dir_all(&jobs_root).expect("create jobs root");
        fs::write(jobs_root.join("scheduler.lock"), "locked").expect("write lock");

        let err = SchedulerLock::acquire(&jobs_root)
            .err()
            .expect("expected scheduler lock error");
        assert!(
            err.to_string().contains("scheduler is busy"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn scheduler_tick_marks_blocked_by_dependency() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        let schedule = JobSchedule {
            dependencies: vec![JobDependency {
                artifact: JobArtifact::PlanDoc {
                    slug: "alpha".to_string(),
                    branch: "draft/alpha".to_string(),
                },
            }],
            ..JobSchedule::default()
        };

        enqueue_job(
            project_root,
            &jobs_root,
            "blocked-job",
            &["ask".to_string()],
            &["vizier".to_string(), "ask".to_string()],
            None,
            None,
            Some(schedule),
        )
        .expect("enqueue job");

        let binary = std::env::current_exe().expect("current exe");
        scheduler_tick(project_root, &jobs_root, &binary).expect("scheduler tick");

        let record = read_record(&jobs_root, "blocked-job").expect("read record");
        assert_eq!(record.status, JobStatus::BlockedByDependency);
        let wait_reason = record
            .schedule
            .as_ref()
            .and_then(|sched| sched.wait_reason.as_ref())
            .expect("wait reason");
        assert_eq!(wait_reason.kind, JobWaitKind::Dependencies);
        let detail = wait_reason.detail.as_deref().unwrap_or("");
        assert!(
            detail.contains("missing plan_doc:alpha (draft/alpha)"),
            "unexpected wait detail: {detail}"
        );
        let waited_on = record
            .schedule
            .as_ref()
            .map(|sched| sched.waited_on.clone())
            .unwrap_or_default();
        assert!(
            waited_on.contains(&JobWaitKind::Dependencies),
            "expected waited_on to include dependencies"
        );
    }

    #[test]
    fn scheduler_tick_errors_on_missing_binary() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "spawn-failure",
            &["ask".to_string()],
            &["vizier".to_string(), "ask".to_string()],
            None,
            None,
            None,
        )
        .expect("enqueue job");

        let missing_binary = project_root.join("does-not-exist");
        let result = scheduler_tick(project_root, &jobs_root, &missing_binary);
        assert!(result.is_err(), "expected scheduler tick to fail");
    }

    #[cfg(unix)]
    #[test]
    fn scheduler_tick_errors_on_persist_failure() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "persist-failure",
            &[],
            &["vizier".to_string()],
            None,
            None,
            None,
        )
        .expect("enqueue job");

        let paths = paths_for(&jobs_root, "persist-failure");
        let original = fs::metadata(&paths.job_dir)
            .expect("metadata")
            .permissions();
        let mut read_only = original.clone();
        read_only.set_mode(0o555);
        fs::set_permissions(&paths.job_dir, read_only).expect("set perms");

        let binary = project_root.join("missing-binary");
        let result = scheduler_tick(project_root, &jobs_root, &binary);

        fs::set_permissions(&paths.job_dir, original).expect("restore perms");
        assert!(result.is_err(), "expected scheduler tick to fail");
    }
}
