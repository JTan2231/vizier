use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use git2::{Repository, StatusOptions, StatusShow};
use lazy_static::lazy_static;

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
            .insert(Self::normalize_repo_path(Path::new(path)));

        Ok(())
    }

    pub fn has_pending_changes() -> bool {
        FILE_TRACKER
            .lock()
            .unwrap()
            .updated_files
            .iter()
            .any(|path| is_canonical_story_path(path))
    }

    pub fn pending_paths(repo_root: &Path) -> Result<Vec<String>, git2::Error> {
        Self::sync_vizier_changes(repo_root)?;

        let tracker = FILE_TRACKER.lock().unwrap();
        let mut paths: Vec<String> = tracker
            .updated_files
            .iter()
            .filter(|path| is_canonical_story_path(path))
            .cloned()
            .collect();
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    pub fn clear_tracked(paths: &[String]) {
        if paths.is_empty() {
            return;
        }

        let mut tracker = FILE_TRACKER.lock().unwrap();
        for path in paths {
            tracker.updated_files.remove(path);
        }
    }

    pub fn delete(path: &str) -> std::io::Result<()> {
        let mut tracker = FILE_TRACKER.lock().unwrap();

        tracker
            .updated_files
            .insert(Self::normalize_repo_path(Path::new(path)));
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

        let changed = Self::collect_vizier_changes(repo_root)?
            .into_iter()
            .filter(|path| is_canonical_story_path(path))
            .collect::<Vec<_>>();
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

fn is_vizier_path(path: &str) -> bool {
    path.starts_with(".vizier/") || path.starts_with(".vizier\\")
}

fn is_canonical_story_path(path: &str) -> bool {
    if !is_vizier_path(path) {
        return false;
    }

    let trimmed = path
        .trim_start_matches(".vizier/")
        .trim_start_matches(".vizier\\");
    if trimmed == ".snapshot" {
        return true;
    }

    if trimmed.contains(['/', '\\']) {
        return false;
    }

    PathBuf::from(trimmed)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}
