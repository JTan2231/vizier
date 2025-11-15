use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use git2::{Repository, StatusOptions, StatusShow};
use lazy_static::lazy_static;

use crate::vcs;

lazy_static! {
    static ref FILE_TRACKER: Mutex<FileTracker> = Mutex::new(FileTracker::new());
}

// TODO: This feels confused with the auditor. Why is that?

pub struct FileTracker {
    updated_files: HashSet<String>,
    all_files: Vec<String>,
}

impl FileTracker {
    fn new() -> Self {
        let all_files = crate::walker::get_non_ignored_files();

        FileTracker {
            updated_files: HashSet::new(),
            all_files: all_files
                .iter()
                .map(|p| p.to_str().unwrap().to_string())
                .collect::<Vec<_>>(),
        }
    }

    pub fn write(path: &str, content: &str) -> std::io::Result<()> {
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;

        file.write_all(content.as_bytes())?;

        FILE_TRACKER
            .lock()
            .unwrap()
            .updated_files
            .insert(path.to_string());

        Ok(())
    }

    pub fn has_pending_changes() -> bool {
        FILE_TRACKER.lock().unwrap().updated_files.len() > 0
    }

    /// Commits changes specific to the `.vizier` directory--primarily intended as a log for Vizier
    /// changes to existing TODOs and narrative threads
    /// Doesn't do anything if there are no changes to commit
    pub async fn commit_changes(
        _conversation_hash: &str,
        message: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if FILE_TRACKER.lock().unwrap().updated_files.len() == 0 {
            return Ok(());
        }

        // TODO: Commit message builder
        vcs::add_and_commit(Some(vec![&crate::tools::get_todo_dir()]), message, false)?;

        Self::clear();

        Ok(())
    }

    pub fn delete(path: &str) -> std::io::Result<()> {
        let mut tracker = FILE_TRACKER.lock().unwrap();

        tracker.updated_files.insert(path.to_string());
        tracker.all_files.retain(|f| f != path);

        std::fs::remove_file(path)?;

        Ok(())
    }

    pub fn read(path: &str) -> std::io::Result<String> {
        let matched = FileTracker::fuzzy_match_path(path);
        std::fs::read_to_string(matched)
    }

    pub fn sync_vizier_changes(repo_root: &Path) -> Result<(), git2::Error> {
        if !repo_root.join(".git").exists() {
            return Ok(());
        }

        let changed = Self::collect_vizier_changes(repo_root)?;
        if changed.is_empty() {
            return Ok(());
        }

        let mut tracker = FILE_TRACKER.lock().unwrap();
        for path in changed {
            if tracker.updated_files.insert(path.clone()) {
                if !tracker.all_files.iter().any(|p| p == &path) {
                    tracker.all_files.push(path);
                }
            }
        }

        Ok(())
    }

    fn clear() {
        FILE_TRACKER.lock().unwrap().updated_files.clear();
    }

    fn collect_vizier_changes(repo_root: &Path) -> Result<Vec<String>, git2::Error> {
        let repo = Repository::open(repo_root)?;
        let mut opts = StatusOptions::new();
        opts.include_untracked(true)
            .include_unmodified(false)
            .include_ignored(false)
            .recurse_untracked_dirs(true)
            .renames_index_to_workdir(true)
            .show(StatusShow::Workdir)
            .pathspec(".vizier/");

        let statuses = repo.statuses(Some(&mut opts))?;
        let mut files = Vec::new();

        for entry in statuses.iter() {
            if let Some(path) = entry.path() {
                files.push(Self::normalize_repo_path(Path::new(path)));
                continue;
            }

            if let Some(delta) = entry.index_to_workdir() {
                if let Some(path) = delta.new_file().path().or_else(|| delta.old_file().path()) {
                    files.push(Self::normalize_repo_path(path));
                }
            }
        }

        Ok(files)
    }

    fn normalize_repo_path(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    fn fuzzy_match_path(input: &str) -> String {
        let paths = &FILE_TRACKER.lock().unwrap().all_files;

        let matcher = SkimMatcherV2::default();
        let best_match = paths
            .iter()
            .filter_map(|path| matcher.fuzzy_match(path, input).map(|score| (score, path)))
            .max_by_key(|(score, _)| *score);

        match best_match {
            Some((score, path)) if (score as f64 / 100.0) > 0.8 => path.clone(),
            _ => input.to_string(),
        }
    }
}
