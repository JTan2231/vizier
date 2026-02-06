use chrono::{DateTime, Duration, Utc};
use git2::{Oid, Repository, WorktreePruneOptions};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    thread,
    time::Duration as StdDuration,
};
use vizier_core::display;
use vizier_core::scheduler::spec::{self, PinnedHeadFact, SchedulerAction, SchedulerFacts};
pub use vizier_core::scheduler::{
    JobArtifact, JobLock, JobStatus, JobWaitKind, JobWaitReason, LockMode, PinnedHead,
    format_artifact,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobDependency {
    pub artifact: JobArtifact,
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
            if ask_save_patch_path(&jobs_root, job_id).exists() {
                return true;
            }

            let paths = paths_for(&jobs_root, job_id);
            if !paths.record_path.exists() {
                return false;
            }

            match read_record(&jobs_root, job_id) {
                Ok(record) => record.status == JobStatus::Succeeded,
                Err(_) => false,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleArtifactState {
    Present,
    Missing,
}

impl ScheduleArtifactState {
    pub fn label(self) -> &'static str {
        match self {
            ScheduleArtifactState::Present => "present",
            ScheduleArtifactState::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ScheduleNode {
    pub id: String,
    pub status: JobStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScheduleEdge {
    pub from: String,
    pub to: String,
    pub artifact: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<ScheduleArtifactState>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScheduleSnapshot {
    pub nodes: Vec<ScheduleNode>,
    pub edges: Vec<ScheduleEdge>,
}

pub(crate) struct ScheduleGraph {
    records: HashMap<String, JobRecord>,
    dependencies: HashMap<String, Vec<JobArtifact>>,
    artifacts: HashMap<String, Vec<JobArtifact>>,
    producers: HashMap<JobArtifact, Vec<String>>,
    consumers: HashMap<JobArtifact, Vec<String>>,
    job_order: Vec<String>,
}

impl ScheduleGraph {
    pub(crate) fn new(records: Vec<JobRecord>) -> Self {
        let mut records_map = HashMap::new();
        for record in records {
            records_map.insert(record.id.clone(), record);
        }

        let mut dependencies = HashMap::new();
        let mut artifacts = HashMap::new();
        let mut producers: HashMap<JobArtifact, Vec<String>> = HashMap::new();
        let mut consumers: HashMap<JobArtifact, Vec<String>> = HashMap::new();

        for record in records_map.values() {
            let schedule = record.schedule.as_ref();
            let mut deps = schedule
                .map(|sched| {
                    sched
                        .dependencies
                        .iter()
                        .map(|dep| dep.artifact.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            sort_artifacts(&mut deps);
            dependencies.insert(record.id.clone(), deps);

            let mut produced = schedule
                .map(|sched| sched.artifacts.clone())
                .unwrap_or_default();
            sort_artifacts(&mut produced);
            artifacts.insert(record.id.clone(), produced.clone());

            for artifact in &produced {
                producers
                    .entry(artifact.clone())
                    .or_default()
                    .push(record.id.clone());
            }

            if let Some(schedule) = schedule {
                for dep in &schedule.dependencies {
                    consumers
                        .entry(dep.artifact.clone())
                        .or_default()
                        .push(record.id.clone());
                }
            }
        }

        for list in producers.values_mut() {
            sort_job_ids(list, &records_map);
        }
        for list in consumers.values_mut() {
            sort_job_ids(list, &records_map);
        }

        let mut job_order = records_map.keys().cloned().collect::<Vec<_>>();
        sort_job_ids(&mut job_order, &records_map);

        Self {
            records: records_map,
            dependencies,
            artifacts,
            producers,
            consumers,
            job_order,
        }
    }

    pub(crate) fn record(&self, job_id: &str) -> Option<&JobRecord> {
        self.records.get(job_id)
    }

    pub(crate) fn job_ids_sorted(&self) -> Vec<String> {
        self.job_order.clone()
    }

    pub(crate) fn dependencies_for(&self, job_id: &str) -> Vec<JobArtifact> {
        self.dependencies.get(job_id).cloned().unwrap_or_default()
    }

    pub(crate) fn artifacts_for(&self, job_id: &str) -> Vec<JobArtifact> {
        self.artifacts.get(job_id).cloned().unwrap_or_default()
    }

    pub(crate) fn producers_for(&self, artifact: &JobArtifact) -> Vec<String> {
        self.producers.get(artifact).cloned().unwrap_or_default()
    }

    pub(crate) fn consumers_for(&self, artifact: &JobArtifact) -> Vec<String> {
        self.consumers.get(artifact).cloned().unwrap_or_default()
    }

    pub(crate) fn artifact_state(
        &self,
        repo: &Repository,
        artifact: &JobArtifact,
    ) -> ScheduleArtifactState {
        if artifact_exists(repo, artifact) {
            ScheduleArtifactState::Present
        } else {
            ScheduleArtifactState::Missing
        }
    }

    pub(crate) fn collect_focus_jobs(&self, focus: &str, max_depth: usize) -> HashSet<String> {
        if !self.records.contains_key(focus) {
            return HashSet::new();
        }

        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        seen.insert(focus.to_string());
        queue.push_back((focus.to_string(), 0usize));

        while let Some((job_id, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            let mut neighbors = Vec::new();
            for dep in self.dependencies_for(&job_id) {
                neighbors.extend(self.producers_for(&dep));
            }
            for artifact in self.artifacts_for(&job_id) {
                neighbors.extend(self.consumers_for(&artifact));
            }
            neighbors.sort_by(|a, b| self.job_compare(a, b));
            neighbors.dedup();
            for neighbor in neighbors {
                if seen.insert(neighbor.clone()) {
                    queue.push_back((neighbor, depth + 1));
                }
            }
        }

        seen
    }

    pub(crate) fn snapshot(
        &self,
        repo: &Repository,
        roots: &[String],
        max_depth: usize,
    ) -> ScheduleSnapshot {
        let mut edges = Vec::new();
        let mut node_ids = HashSet::new();

        for root in roots {
            if !self.records.contains_key(root) {
                continue;
            }
            node_ids.insert(root.clone());
            let mut path = HashSet::new();
            path.insert(root.clone());
            self.collect_edges(repo, root, max_depth, &mut path, &mut edges, &mut node_ids);
        }

        let mut nodes = Vec::new();
        for job_id in &self.job_order {
            if !node_ids.contains(job_id) {
                continue;
            }
            if let Some(record) = self.records.get(job_id) {
                nodes.push(ScheduleNode {
                    id: record.id.clone(),
                    status: record.status,
                    command: if record.command.is_empty() {
                        None
                    } else {
                        Some(record.command.join(" "))
                    },
                    wait: record
                        .schedule
                        .as_ref()
                        .and_then(|sched| sched.wait_reason.as_ref())
                        .map(|reason| {
                            reason
                                .detail
                                .clone()
                                .unwrap_or_else(|| "waiting".to_string())
                        }),
                });
            }
        }

        ScheduleSnapshot { nodes, edges }
    }

    fn collect_edges(
        &self,
        repo: &Repository,
        job_id: &str,
        depth_remaining: usize,
        path: &mut HashSet<String>,
        edges: &mut Vec<ScheduleEdge>,
        node_ids: &mut HashSet<String>,
    ) {
        if depth_remaining == 0 {
            return;
        }

        for dependency in self.dependencies_for(job_id) {
            let artifact_label = format_artifact(&dependency);
            let producers = self.producers_for(&dependency);
            if producers.is_empty() {
                let state = self.artifact_state(repo, &dependency);
                edges.push(ScheduleEdge {
                    from: job_id.to_string(),
                    to: format!("artifact:{artifact_label}"),
                    artifact: artifact_label,
                    state: Some(state),
                });
                continue;
            }

            for producer_id in producers {
                edges.push(ScheduleEdge {
                    from: job_id.to_string(),
                    to: producer_id.clone(),
                    artifact: artifact_label.clone(),
                    state: None,
                });
                node_ids.insert(producer_id.clone());
                if depth_remaining > 1 && !path.contains(&producer_id) {
                    path.insert(producer_id.clone());
                    self.collect_edges(
                        repo,
                        &producer_id,
                        depth_remaining - 1,
                        path,
                        edges,
                        node_ids,
                    );
                    path.remove(&producer_id);
                }
            }
        }
    }

    fn job_compare(&self, a: &str, b: &str) -> Ordering {
        match (self.records.get(a), self.records.get(b)) {
            (Some(left), Some(right)) => {
                let order = left.created_at.cmp(&right.created_at);
                if order == Ordering::Equal {
                    left.id.cmp(&right.id)
                } else {
                    order
                }
            }
            _ => a.cmp(b),
        }
    }
}

fn artifact_sort_key(artifact: &JobArtifact) -> (u8, &str, &str) {
    match artifact {
        JobArtifact::PlanBranch { slug, branch } => (0, slug, branch),
        JobArtifact::PlanDoc { slug, branch } => (1, slug, branch),
        JobArtifact::PlanCommits { slug, branch } => (2, slug, branch),
        JobArtifact::TargetBranch { name } => (3, name, ""),
        JobArtifact::MergeSentinel { slug } => (4, slug, ""),
        JobArtifact::AskSavePatch { job_id } => (5, job_id, ""),
    }
}

fn sort_artifacts(artifacts: &mut [JobArtifact]) {
    artifacts.sort_by(|left, right| artifact_sort_key(left).cmp(&artifact_sort_key(right)));
}

fn sort_job_ids(job_ids: &mut [String], records: &HashMap<String, JobRecord>) {
    job_ids.sort_by(
        |left, right| match (records.get(left), records.get(right)) {
            (Some(left_record), Some(right_record)) => {
                let order = left_record.created_at.cmp(&right_record.created_at);
                if order == Ordering::Equal {
                    left_record.id.cmp(&right_record.id)
                } else {
                    order
                }
            }
            _ => left.cmp(right),
        },
    );
}

fn pinned_head_matches(repo: &Repository, pinned: &PinnedHead) -> Result<bool, git2::Error> {
    let branch_ref = repo.find_branch(&pinned.branch, git2::BranchType::Local)?;
    let commit = branch_ref.into_reference().peel_to_commit()?;
    let expected = Oid::from_str(&pinned.oid).ok();
    Ok(Some(commit.id()) == expected)
}

fn build_scheduler_facts(
    repo: &Repository,
    records: &[JobRecord],
) -> Result<SchedulerFacts, Box<dyn std::error::Error>> {
    let mut facts = SchedulerFacts::default();
    let mut dependency_artifacts = HashSet::new();

    for record in records {
        facts.job_order.push(record.id.clone());
        facts.job_statuses.insert(record.id.clone(), record.status);
        if !record.child_args.is_empty() {
            facts.has_child_args.insert(record.id.clone());
        }

        if let Some(schedule) = record.schedule.as_ref() {
            let deps = schedule
                .dependencies
                .iter()
                .map(|dep| dep.artifact.clone())
                .collect::<Vec<_>>();
            if !deps.is_empty() {
                dependency_artifacts.extend(deps.iter().cloned());
                facts.job_dependencies.insert(record.id.clone(), deps);
            }

            if !schedule.locks.is_empty() {
                facts
                    .job_locks
                    .insert(record.id.clone(), schedule.locks.clone());
            }

            if let Some(pinned) = schedule.pinned_head.as_ref() {
                let matches = pinned_head_matches(repo, pinned)?;
                facts.pinned_heads.insert(
                    record.id.clone(),
                    PinnedHeadFact {
                        branch: pinned.branch.clone(),
                        matches,
                    },
                );
            }

            if !schedule.waited_on.is_empty() {
                facts
                    .waited_on
                    .insert(record.id.clone(), schedule.waited_on.clone());
            }

            for artifact in &schedule.artifacts {
                facts
                    .producer_statuses
                    .entry(artifact.clone())
                    .or_default()
                    .push(record.status);
            }

            if record.status == JobStatus::Running && !schedule.locks.is_empty() {
                facts.lock_state.acquire(&schedule.locks);
            }
        }
    }

    for artifact in dependency_artifacts {
        if artifact_exists(repo, &artifact) {
            facts.artifact_exists.insert(artifact);
        }
    }

    Ok(facts)
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

    records.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let facts = build_scheduler_facts(&repo, &records)?;
    let decisions = spec::evaluate_all(&facts);

    for mut record in records {
        if job_is_terminal(record.status) || record.status == JobStatus::Running {
            continue;
        }

        let decision = match decisions.get(&record.id) {
            Some(decision) => decision,
            None => continue,
        };

        let mut schedule = record.schedule.clone().unwrap_or_default();
        schedule.wait_reason = decision.wait_reason.clone();
        schedule.waited_on = decision.waited_on.clone();

        match decision.action {
            SchedulerAction::UpdateStatus => {
                if record.status != decision.next_status
                    || record
                        .schedule
                        .as_ref()
                        .and_then(|s| s.wait_reason.as_ref())
                        != schedule.wait_reason.as_ref()
                {
                    record.status = decision.next_status;
                    record.schedule = Some(schedule);
                    persist_record(&paths_for(jobs_root, &record.id), &record)?;
                    outcome.updated.push(record.id.clone());
                }
            }
            SchedulerAction::FailMissingChildArgs => {
                record.status = JobStatus::Failed;
                record.schedule = Some(schedule);
                persist_record(&paths_for(jobs_root, &record.id), &record)?;
                finalize_job(
                    project_root,
                    jobs_root,
                    &record.id,
                    JobStatus::Failed,
                    1,
                    None,
                    None,
                )?;
                outcome.updated.push(record.id.clone());
            }
            SchedulerAction::Start => {
                schedule.wait_reason = None;
                record.schedule = Some(schedule);
                persist_record(&paths_for(jobs_root, &record.id), &record)?;
                start_job(project_root, jobs_root, binary, &record.id)?;
                outcome.started.push(record.id.clone());
            }
        }
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

pub fn update_job_record<F>(
    jobs_root: &Path,
    job_id: &str,
    updater: F,
) -> Result<JobRecord, Box<dyn std::error::Error>>
where
    F: FnOnce(&mut JobRecord),
{
    let paths = paths_for(jobs_root, job_id);
    if !paths.record_path.exists() {
        return Err(format!("no background job {}", job_id).into());
    }

    let mut record = load_record(&paths)?;
    updater(&mut record);
    persist_record(&paths, &record)?;
    Ok(record)
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
    use chrono::TimeZone;
    use git2::{BranchType, Signature};
    use std::path::Path;
    use tempfile::TempDir;

    fn init_repo(temp: &TempDir) -> Result<Repository, git2::Error> {
        let repo = Repository::init(temp.path())?;
        Ok(repo)
    }

    fn seed_repo(repo: &Repository) -> Result<Oid, Box<dyn std::error::Error>> {
        let workdir = repo.workdir().ok_or("missing workdir")?;
        let readme = workdir.join("README.md");
        fs::write(&readme, "seed")?;
        let mut index = repo.index()?;
        index.add_path(Path::new("README.md"))?;
        let tree_id = index.write_tree()?;
        let tree = repo.find_tree(tree_id)?;
        let sig = Signature::now("vizier", "vizier@example.com")?;
        let oid = repo.commit(Some("HEAD"), &sig, &sig, "seed", &tree, &[])?;
        Ok(oid)
    }

    fn ensure_branch(repo: &Repository, name: &str) -> Result<(), Box<dyn std::error::Error>> {
        if repo.find_branch(name, BranchType::Local).is_ok() {
            return Ok(());
        }
        let head = repo.head()?.peel_to_commit()?;
        repo.branch(name, &head, false)?;
        Ok(())
    }

    fn commit_plan_doc(
        repo: &Repository,
        slug: &str,
        branch: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let workdir = repo.workdir().ok_or("missing workdir")?;
        let plan_path = crate::plan::plan_rel_path(slug);
        let full_path = workdir.join(&plan_path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&full_path, format!("# plan {}\n", slug))?;

        let mut index = repo.index()?;
        index.add_path(&plan_path)?;
        let tree_id = index.write_tree()?;
        let tree = repo.find_tree(tree_id)?;
        let sig = Signature::now("vizier", "vizier@example.com")?;
        let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
        let parents = parent.iter().collect::<Vec<_>>();
        let refname = format!("refs/heads/{branch}");
        repo.commit(
            Some(refname.as_str()),
            &sig,
            &sig,
            "plan doc",
            &tree,
            &parents,
        )?;
        Ok(())
    }

    fn ensure_artifact_exists(
        repo: &Repository,
        jobs_root: &Path,
        artifact: &JobArtifact,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match artifact {
            JobArtifact::PlanBranch { branch, .. } | JobArtifact::PlanCommits { branch, .. } => {
                ensure_branch(repo, branch)?;
            }
            JobArtifact::PlanDoc { slug, branch } => {
                commit_plan_doc(repo, slug, branch)?;
            }
            JobArtifact::TargetBranch { name } => {
                ensure_branch(repo, name)?;
            }
            JobArtifact::MergeSentinel { slug } => {
                let path = repo
                    .path()
                    .join(".vizier/tmp/merge-conflicts")
                    .join(format!("{slug}.json"));
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(path, "{}")?;
            }
            JobArtifact::AskSavePatch { job_id } => {
                let path = ask_save_patch_path(jobs_root, job_id);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(path, "patch")?;
            }
        }
        Ok(())
    }

    fn write_job_with_status(
        project_root: &Path,
        jobs_root: &Path,
        job_id: &str,
        status: JobStatus,
        schedule: JobSchedule,
        child_args: &[String],
    ) -> Result<JobRecord, Box<dyn std::error::Error>> {
        enqueue_job(
            project_root,
            jobs_root,
            job_id,
            child_args,
            &["vizier".to_string()],
            None,
            None,
            Some(schedule.clone()),
        )?;
        let paths = paths_for(jobs_root, job_id);
        let mut record = load_record(&paths)?;
        record.status = status;
        record.schedule = Some(schedule);
        persist_record(&paths, &record)?;
        Ok(record)
    }

    fn update_job_record<F: FnOnce(&mut JobRecord)>(
        jobs_root: &Path,
        job_id: &str,
        updater: F,
    ) -> Result<JobRecord, Box<dyn std::error::Error>> {
        let paths = paths_for(jobs_root, job_id);
        let mut record = load_record(&paths)?;
        updater(&mut record);
        persist_record(&paths, &record)?;
        Ok(record)
    }

    #[derive(Clone, Copy)]
    enum ArtifactKind {
        PlanBranch,
        PlanDoc,
        PlanCommits,
        TargetBranch,
        MergeSentinel,
        AskSavePatch,
    }

    fn artifact_for(kind: ArtifactKind, suffix: &str) -> JobArtifact {
        match kind {
            ArtifactKind::PlanBranch => JobArtifact::PlanBranch {
                slug: format!("plan-{suffix}"),
                branch: format!("draft/plan-{suffix}"),
            },
            ArtifactKind::PlanDoc => JobArtifact::PlanDoc {
                slug: format!("doc-{suffix}"),
                branch: format!("draft/doc-{suffix}"),
            },
            ArtifactKind::PlanCommits => JobArtifact::PlanCommits {
                slug: format!("commits-{suffix}"),
                branch: format!("draft/commits-{suffix}"),
            },
            ArtifactKind::TargetBranch => JobArtifact::TargetBranch {
                name: format!("target-{suffix}"),
            },
            ArtifactKind::MergeSentinel => JobArtifact::MergeSentinel {
                slug: format!("merge-{suffix}"),
            },
            ArtifactKind::AskSavePatch => JobArtifact::AskSavePatch {
                job_id: format!("job-{suffix}"),
            },
        }
    }

    fn make_record(
        job_id: &str,
        status: JobStatus,
        created_at: DateTime<Utc>,
        schedule: Option<JobSchedule>,
    ) -> JobRecord {
        JobRecord {
            id: job_id.to_string(),
            status,
            command: Vec::new(),
            child_args: Vec::new(),
            created_at,
            started_at: None,
            finished_at: None,
            pid: None,
            exit_code: None,
            stdout_path: String::new(),
            stderr_path: String::new(),
            session_path: None,
            outcome_path: None,
            metadata: None,
            config_snapshot: None,
            schedule,
        }
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

    #[test]
    fn scheduler_tick_handles_graph_shapes() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");
        fs::create_dir_all(&jobs_root).expect("jobs root");

        let artifact_a = JobArtifact::AskSavePatch {
            job_id: "a-artifact".to_string(),
        };
        let artifact_b = JobArtifact::AskSavePatch {
            job_id: "b-artifact".to_string(),
        };
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-a",
            JobStatus::Running,
            JobSchedule {
                artifacts: vec![artifact_a.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("job a");
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-b",
            JobStatus::Queued,
            JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: artifact_a.clone(),
                }],
                artifacts: vec![artifact_b.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("job b");
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-c",
            JobStatus::Queued,
            JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: artifact_b.clone(),
                }],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("job c");

        let fan_artifact = JobArtifact::AskSavePatch {
            job_id: "fan-root".to_string(),
        };
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-fan-root",
            JobStatus::Running,
            JobSchedule {
                artifacts: vec![fan_artifact.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("fan root");
        for job_id in ["job-fan-left", "job-fan-right"] {
            write_job_with_status(
                project_root,
                &jobs_root,
                job_id,
                JobStatus::Queued,
                JobSchedule {
                    dependencies: vec![JobDependency {
                        artifact: fan_artifact.clone(),
                    }],
                    ..JobSchedule::default()
                },
                &["--help".to_string()],
            )
            .expect("fan job");
        }

        let fan_in_left = JobArtifact::AskSavePatch {
            job_id: "fanin-left".to_string(),
        };
        let fan_in_right = JobArtifact::AskSavePatch {
            job_id: "fanin-right".to_string(),
        };
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-fanin-left",
            JobStatus::Running,
            JobSchedule {
                artifacts: vec![fan_in_left.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("fanin left");
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-fanin-right",
            JobStatus::Running,
            JobSchedule {
                artifacts: vec![fan_in_right.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("fanin right");
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-fanin",
            JobStatus::Queued,
            JobSchedule {
                dependencies: vec![
                    JobDependency {
                        artifact: fan_in_left.clone(),
                    },
                    JobDependency {
                        artifact: fan_in_right.clone(),
                    },
                ],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("fanin");

        let diamond_root = JobArtifact::AskSavePatch {
            job_id: "diamond-root".to_string(),
        };
        let diamond_left = JobArtifact::AskSavePatch {
            job_id: "diamond-left".to_string(),
        };
        let diamond_right = JobArtifact::AskSavePatch {
            job_id: "diamond-right".to_string(),
        };
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-diamond-root",
            JobStatus::Running,
            JobSchedule {
                artifacts: vec![diamond_root.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("diamond root");
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-diamond-left",
            JobStatus::Queued,
            JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: diamond_root.clone(),
                }],
                artifacts: vec![diamond_left.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("diamond left");
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-diamond-right",
            JobStatus::Queued,
            JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: diamond_root.clone(),
                }],
                artifacts: vec![diamond_right.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("diamond right");
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-diamond-leaf",
            JobStatus::Queued,
            JobSchedule {
                dependencies: vec![
                    JobDependency {
                        artifact: diamond_left.clone(),
                    },
                    JobDependency {
                        artifact: diamond_right.clone(),
                    },
                ],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("diamond leaf");

        let disjoint_artifact = JobArtifact::AskSavePatch {
            job_id: "disjoint-root".to_string(),
        };
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-disjoint-root",
            JobStatus::Running,
            JobSchedule {
                artifacts: vec![disjoint_artifact.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("disjoint root");
        write_job_with_status(
            project_root,
            &jobs_root,
            "job-disjoint-leaf",
            JobStatus::Queued,
            JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: disjoint_artifact.clone(),
                }],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("disjoint leaf");

        let binary = std::env::current_exe().expect("current exe");
        scheduler_tick(project_root, &jobs_root, &binary).expect("scheduler tick");

        let record_b = read_record(&jobs_root, "job-b").expect("read job b");
        assert_eq!(record_b.status, JobStatus::WaitingOnDeps);
        let detail_b = record_b
            .schedule
            .as_ref()
            .and_then(|sched| sched.wait_reason.as_ref())
            .and_then(|reason| reason.detail.clone())
            .unwrap_or_default();
        assert!(
            detail_b.contains("waiting on ask_save_patch:a-artifact"),
            "unexpected wait detail for job-b: {detail_b}"
        );

        let record_c = read_record(&jobs_root, "job-c").expect("read job c");
        let detail_c = record_c
            .schedule
            .as_ref()
            .and_then(|sched| sched.wait_reason.as_ref())
            .and_then(|reason| reason.detail.clone())
            .unwrap_or_default();
        assert!(
            detail_c.contains("waiting on ask_save_patch:b-artifact"),
            "unexpected wait detail for job-c: {detail_c}"
        );

        for job_id in ["job-fan-left", "job-fan-right"] {
            let record = read_record(&jobs_root, job_id).expect("read fan job");
            assert_eq!(record.status, JobStatus::WaitingOnDeps);
        }

        let record_fanin = read_record(&jobs_root, "job-fanin").expect("read fanin job");
        let detail_fanin = record_fanin
            .schedule
            .as_ref()
            .and_then(|sched| sched.wait_reason.as_ref())
            .and_then(|reason| reason.detail.clone())
            .unwrap_or_default();
        assert!(
            detail_fanin.contains("waiting on ask_save_patch:fanin-left"),
            "unexpected fan-in detail: {detail_fanin}"
        );

        let record_diamond = read_record(&jobs_root, "job-diamond-leaf").expect("read diamond");
        let detail_diamond = record_diamond
            .schedule
            .as_ref()
            .and_then(|sched| sched.wait_reason.as_ref())
            .and_then(|reason| reason.detail.clone())
            .unwrap_or_default();
        assert!(
            detail_diamond.contains("waiting on ask_save_patch:diamond-left"),
            "unexpected diamond detail: {detail_diamond}"
        );

        let record_disjoint = read_record(&jobs_root, "job-disjoint-leaf").expect("read disjoint");
        assert_eq!(record_disjoint.status, JobStatus::WaitingOnDeps);
    }

    #[test]
    fn scheduler_tick_waited_on_accumulates_and_stabilizes() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");
        fs::create_dir_all(&jobs_root).expect("jobs root");

        let artifact = JobArtifact::AskSavePatch {
            job_id: "dep-ready".to_string(),
        };
        write_job_with_status(
            project_root,
            &jobs_root,
            "dep-producer",
            JobStatus::Running,
            JobSchedule {
                artifacts: vec![artifact.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("dep producer");
        write_job_with_status(
            project_root,
            &jobs_root,
            "lock-holder",
            JobStatus::Running,
            JobSchedule {
                locks: vec![JobLock {
                    key: "lock-a".to_string(),
                    mode: LockMode::Exclusive,
                }],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("lock holder");

        write_job_with_status(
            project_root,
            &jobs_root,
            "waiting-job",
            JobStatus::Queued,
            JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: artifact.clone(),
                }],
                locks: vec![JobLock {
                    key: "lock-a".to_string(),
                    mode: LockMode::Exclusive,
                }],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("waiting job");

        let binary = std::env::current_exe().expect("current exe");
        let outcome = scheduler_tick(project_root, &jobs_root, &binary).expect("tick 1");
        assert!(outcome.updated.contains(&"waiting-job".to_string()));

        let record = read_record(&jobs_root, "waiting-job").expect("read waiting job");
        assert_eq!(record.status, JobStatus::WaitingOnDeps);
        let waited_on = record
            .schedule
            .as_ref()
            .map(|sched| sched.waited_on.clone())
            .unwrap_or_default();
        assert_eq!(waited_on, vec![JobWaitKind::Dependencies]);

        ensure_artifact_exists(
            &Repository::discover(project_root).expect("repo"),
            &jobs_root,
            &artifact,
        )
        .expect("create artifact");
        let outcome = scheduler_tick(project_root, &jobs_root, &binary).expect("tick 2");
        assert!(outcome.updated.contains(&"waiting-job".to_string()));

        let record = read_record(&jobs_root, "waiting-job").expect("read waiting job");
        assert_eq!(record.status, JobStatus::WaitingOnLocks);
        let waited_on = record
            .schedule
            .as_ref()
            .map(|sched| sched.waited_on.clone())
            .unwrap_or_default();
        assert_eq!(
            waited_on,
            vec![JobWaitKind::Dependencies, JobWaitKind::Locks]
        );

        let outcome = scheduler_tick(project_root, &jobs_root, &binary).expect("tick 3");
        assert!(
            !outcome.updated.contains(&"waiting-job".to_string()),
            "expected no-op tick to avoid updates"
        );

        update_job_record(&jobs_root, "lock-holder", |record| {
            record.status = JobStatus::Succeeded;
        })
        .expect("release lock");

        scheduler_tick(project_root, &jobs_root, &binary).expect("tick 4");
        let record = read_record(&jobs_root, "waiting-job").expect("read waiting job");
        assert_eq!(record.status, JobStatus::Running);
        let wait_reason = record
            .schedule
            .as_ref()
            .and_then(|sched| sched.wait_reason.as_ref());
        assert!(wait_reason.is_none(), "wait reason should clear on start");
    }

    #[test]
    fn scheduler_tick_missing_child_args_fails() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "missing-child-args",
            &[],
            &["vizier".to_string(), "ask".to_string()],
            None,
            None,
            None,
        )
        .expect("enqueue");

        let binary = std::env::current_exe().expect("current exe");
        scheduler_tick(project_root, &jobs_root, &binary).expect("tick");

        let record = read_record(&jobs_root, "missing-child-args").expect("record");
        assert_eq!(record.status, JobStatus::Failed);
        assert_eq!(record.exit_code, Some(1));
        assert!(
            record.finished_at.is_some(),
            "expected finished_at to be set"
        );
        let outcome_path = record.outcome_path.as_deref().expect("outcome path");
        assert!(
            project_root.join(outcome_path).exists(),
            "expected outcome file to exist"
        );
        let wait_reason = record
            .schedule
            .as_ref()
            .and_then(|sched| sched.wait_reason.as_ref())
            .expect("wait reason");
        assert_eq!(wait_reason.kind, JobWaitKind::Dependencies);
        assert_eq!(wait_reason.detail.as_deref(), Some("missing child args"));
    }

    #[test]
    fn scheduler_tick_starts_with_empty_schedule() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");

        enqueue_job(
            project_root,
            &jobs_root,
            "empty-schedule",
            &["--help".to_string()],
            &["vizier".to_string(), "ask".to_string()],
            None,
            None,
            None,
        )
        .expect("enqueue");

        let binary = std::env::current_exe().expect("current exe");
        scheduler_tick(project_root, &jobs_root, &binary).expect("tick");

        let record = read_record(&jobs_root, "empty-schedule").expect("record");
        assert_eq!(record.status, JobStatus::Running);
        let wait_reason = record
            .schedule
            .as_ref()
            .and_then(|sched| sched.wait_reason.as_ref());
        assert!(wait_reason.is_none(), "wait reason should be cleared");
    }

    #[test]
    fn scheduler_facts_collect_artifact_existence() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");
        fs::create_dir_all(&jobs_root).expect("jobs root");

        let kinds = [
            ArtifactKind::PlanBranch,
            ArtifactKind::PlanDoc,
            ArtifactKind::PlanCommits,
            ArtifactKind::TargetBranch,
            ArtifactKind::MergeSentinel,
            ArtifactKind::AskSavePatch,
        ];

        for (idx, kind) in kinds.iter().enumerate() {
            let exists = artifact_for(*kind, &format!("exists-{idx}"));
            let missing = artifact_for(*kind, &format!("missing-{idx}"));
            ensure_artifact_exists(&repo, &jobs_root, &exists).expect("create artifact");

            write_job_with_status(
                project_root,
                &jobs_root,
                &format!("job-{idx}"),
                JobStatus::Queued,
                JobSchedule {
                    dependencies: vec![
                        JobDependency {
                            artifact: exists.clone(),
                        },
                        JobDependency {
                            artifact: missing.clone(),
                        },
                    ],
                    ..JobSchedule::default()
                },
                &["--help".to_string()],
            )
            .expect("write job");
        }

        let mut records = list_records(&jobs_root).expect("list records");
        records.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        let facts = build_scheduler_facts(&repo, &records).expect("facts");

        for (idx, kind) in kinds.iter().enumerate() {
            let exists = artifact_for(*kind, &format!("exists-{idx}"));
            let missing = artifact_for(*kind, &format!("missing-{idx}"));
            assert!(
                facts.artifact_exists.contains(&exists),
                "expected artifact to exist: {exists:?}"
            );
            assert!(
                !facts.artifact_exists.contains(&missing),
                "expected artifact to be missing: {missing:?}"
            );
        }
    }

    #[test]
    fn scheduler_facts_collect_producer_statuses() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");
        fs::create_dir_all(&jobs_root).expect("jobs root");

        let artifact = JobArtifact::AskSavePatch {
            job_id: "artifact".to_string(),
        };
        write_job_with_status(
            project_root,
            &jobs_root,
            "producer-running",
            JobStatus::Running,
            JobSchedule {
                artifacts: vec![artifact.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("producer running");
        write_job_with_status(
            project_root,
            &jobs_root,
            "producer-succeeded",
            JobStatus::Succeeded,
            JobSchedule {
                artifacts: vec![artifact.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("producer succeeded");

        let mut records = list_records(&jobs_root).expect("list records");
        records.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        let facts =
            build_scheduler_facts(&Repository::discover(project_root).expect("repo"), &records)
                .expect("facts");
        let statuses = facts
            .producer_statuses
            .get(&artifact)
            .expect("producer statuses");
        assert!(statuses.contains(&JobStatus::Running));
        assert!(statuses.contains(&JobStatus::Succeeded));
    }

    #[test]
    fn scheduler_facts_record_pinned_head_status() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");
        fs::create_dir_all(&jobs_root).expect("jobs root");

        let head = repo.head().expect("head");
        let branch = head.shorthand().expect("branch").to_string();
        let oid = head.target().map(|id| id.to_string()).expect("head oid");

        write_job_with_status(
            project_root,
            &jobs_root,
            "pinned-ok",
            JobStatus::Queued,
            JobSchedule {
                pinned_head: Some(PinnedHead {
                    branch: branch.clone(),
                    oid: oid.clone(),
                }),
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("pinned ok");
        write_job_with_status(
            project_root,
            &jobs_root,
            "pinned-bad",
            JobStatus::Queued,
            JobSchedule {
                pinned_head: Some(PinnedHead {
                    branch: branch.clone(),
                    oid: "deadbeef".to_string(),
                }),
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("pinned bad");

        let mut records = list_records(&jobs_root).expect("list records");
        records.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        let facts = build_scheduler_facts(&repo, &records).expect("facts");

        let ok = facts.pinned_heads.get("pinned-ok").expect("pinned ok fact");
        assert!(ok.matches);
        assert_eq!(ok.branch, branch);

        let bad = facts
            .pinned_heads
            .get("pinned-bad")
            .expect("pinned bad fact");
        assert!(!bad.matches);
        assert_eq!(bad.branch, branch);
    }

    #[test]
    fn scheduler_facts_track_running_locks() {
        let temp = TempDir::new().expect("temp dir");
        init_repo(&temp).expect("init repo");
        let project_root = temp.path();
        let jobs_root = project_root.join(".vizier/jobs");
        fs::create_dir_all(&jobs_root).expect("jobs root");

        let lock = JobLock {
            key: "lock-a".to_string(),
            mode: LockMode::Exclusive,
        };
        write_job_with_status(
            project_root,
            &jobs_root,
            "lock-holder",
            JobStatus::Running,
            JobSchedule {
                locks: vec![lock.clone()],
                ..JobSchedule::default()
            },
            &["--help".to_string()],
        )
        .expect("lock holder");

        let mut records = list_records(&jobs_root).expect("list records");
        records.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        let facts =
            build_scheduler_facts(&Repository::discover(project_root).expect("repo"), &records)
                .expect("facts");
        assert!(
            !facts.lock_state.can_acquire(&lock),
            "expected lock to be held"
        );
    }

    #[test]
    fn schedule_graph_orders_dependencies_by_artifact_key() {
        let deps = vec![
            JobDependency {
                artifact: JobArtifact::TargetBranch {
                    name: "main".to_string(),
                },
            },
            JobDependency {
                artifact: JobArtifact::AskSavePatch {
                    job_id: "job-z".to_string(),
                },
            },
            JobDependency {
                artifact: JobArtifact::PlanDoc {
                    slug: "alpha".to_string(),
                    branch: "draft/alpha".to_string(),
                },
            },
        ];

        let record = make_record(
            "job-1",
            JobStatus::Queued,
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                dependencies: deps,
                ..JobSchedule::default()
            }),
        );

        let graph = ScheduleGraph::new(vec![record]);
        let ordered = graph.dependencies_for("job-1");
        let expected = vec![
            JobArtifact::PlanDoc {
                slug: "alpha".to_string(),
                branch: "draft/alpha".to_string(),
            },
            JobArtifact::TargetBranch {
                name: "main".to_string(),
            },
            JobArtifact::AskSavePatch {
                job_id: "job-z".to_string(),
            },
        ];
        assert_eq!(ordered, expected);
    }

    #[test]
    fn schedule_graph_orders_producers_by_created_at_then_id() {
        let artifact = JobArtifact::AskSavePatch {
            job_id: "shared".to_string(),
        };
        let record_a = make_record(
            "job-a",
            JobStatus::Queued,
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                artifacts: vec![artifact.clone()],
                ..JobSchedule::default()
            }),
        );
        let record_b = make_record(
            "job-b",
            JobStatus::Queued,
            Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                artifacts: vec![artifact.clone()],
                ..JobSchedule::default()
            }),
        );

        let graph = ScheduleGraph::new(vec![record_b, record_a]);
        let producers = graph.producers_for(&artifact);
        assert_eq!(producers, vec!["job-a".to_string(), "job-b".to_string()]);
    }

    #[test]
    fn schedule_graph_reports_artifact_state() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");
        seed_repo(&repo).expect("seed repo");
        ensure_branch(&repo, "present").expect("ensure branch");

        let graph = ScheduleGraph::new(Vec::new());
        let present = JobArtifact::TargetBranch {
            name: "present".to_string(),
        };
        let missing = JobArtifact::TargetBranch {
            name: "missing".to_string(),
        };
        assert_eq!(
            graph.artifact_state(&repo, &present),
            ScheduleArtifactState::Present
        );
        assert_eq!(
            graph.artifact_state(&repo, &missing),
            ScheduleArtifactState::Missing
        );
    }

    #[test]
    fn schedule_snapshot_respects_depth_limit() {
        let temp = TempDir::new().expect("temp dir");
        let repo = init_repo(&temp).expect("init repo");

        let artifact_b = JobArtifact::AskSavePatch {
            job_id: "b".to_string(),
        };
        let artifact_c = JobArtifact::AskSavePatch {
            job_id: "c".to_string(),
        };

        let job_c = make_record(
            "job-c",
            JobStatus::Succeeded,
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                artifacts: vec![artifact_c.clone()],
                ..JobSchedule::default()
            }),
        );
        let job_b = make_record(
            "job-b",
            JobStatus::Succeeded,
            Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: artifact_c.clone(),
                }],
                artifacts: vec![artifact_b.clone()],
                ..JobSchedule::default()
            }),
        );
        let job_a = make_record(
            "job-a",
            JobStatus::Queued,
            Utc.with_ymd_and_hms(2026, 1, 3, 0, 0, 0).unwrap(),
            Some(JobSchedule {
                dependencies: vec![JobDependency {
                    artifact: artifact_b.clone(),
                }],
                ..JobSchedule::default()
            }),
        );

        let graph = ScheduleGraph::new(vec![job_a, job_b, job_c]);
        let roots = vec!["job-a".to_string()];

        let snapshot = graph.snapshot(&repo, &roots, 1);
        assert!(
            snapshot
                .edges
                .iter()
                .any(|edge| edge.from == "job-a" && edge.to == "job-b"),
            "expected job-a -> job-b edge"
        );
        assert!(
            snapshot.edges.iter().all(|edge| edge.from != "job-b"),
            "expected depth=1 to skip job-b dependencies"
        );

        let deeper = graph.snapshot(&repo, &roots, 2);
        assert!(
            deeper
                .edges
                .iter()
                .any(|edge| edge.from == "job-b" && edge.to == "job-c"),
            "expected depth=2 to include job-b -> job-c edge"
        );
    }
}
