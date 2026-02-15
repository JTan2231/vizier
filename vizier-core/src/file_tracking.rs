use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::tools;
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
            if tracker.updated_files.insert(path.clone())
                && !tracker.all_files.iter().any(|p| p == &path)
            {
                tracker.all_files.push(path);
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
            .renames_head_to_index(true)
            .renames_index_to_workdir(true)
            .show(StatusShow::IndexAndWorkdir)
            .pathspec(".vizier/");

        let statuses = repo.statuses(Some(&mut opts))?;
        let mut files = Vec::new();

        for entry in statuses.iter() {
            if let Some(path) = entry.path() {
                files.push(Self::normalize_repo_path(Path::new(path)));
                continue;
            }

            if let Some(delta) = entry.head_to_index().or_else(|| entry.index_to_workdir())
                && let Some(path) = delta.new_file().path().or_else(|| delta.old_file().path())
            {
                files.push(Self::normalize_repo_path(path));
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
    let normalized = path.replace('\\', "/");
    normalized.starts_with(".vizier/") || normalized.starts_with("./.vizier/")
}

pub fn is_canonical_story_path(path: &str) -> bool {
    if !is_vizier_path(path) {
        return false;
    }

    let normalized = path.replace('\\', "/");
    let trimmed = normalized
        .trim_start_matches("./")
        .trim_start_matches(".vizier/");

    if tools::is_snapshot_file(trimmed) {
        return true;
    }

    if !trimmed.starts_with("narrative/") {
        return false;
    }

    let relative = trimmed.trim_start_matches("narrative/");
    if relative.is_empty() {
        return false;
    }

    PathBuf::from(relative)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::Signature;
    use std::fs;
    use tempfile::tempdir;

    fn run_git(repo_root: &Path, args: &[&str]) {
        let repo = match Repository::open(repo_root) {
            Ok(repo) => repo,
            Err(_) => Repository::init(repo_root).expect("init repo"),
        };
        match args {
            ["init"] => {
                let _ = Repository::open(repo_root).expect("open initialized repo");
            }
            ["config", "user.name", value] => {
                repo.config()
                    .and_then(|mut cfg| cfg.set_str("user.name", value))
                    .expect("set user.name");
            }
            ["config", "user.email", value] => {
                repo.config()
                    .and_then(|mut cfg| cfg.set_str("user.email", value))
                    .expect("set user.email");
            }
            ["add", path] => {
                let mut index = repo.index().expect("open index");
                index.add_path(Path::new(path)).expect("add path");
                index.write().expect("write index");
            }
            ["commit", "-m", message] => {
                let mut index = repo.index().expect("open index");
                index.write().expect("write index");
                let tree_id = index.write_tree().expect("write tree");
                let tree = repo.find_tree(tree_id).expect("find tree");
                let sig = repo
                    .signature()
                    .or_else(|_| Signature::now("Test User", "test@example.com"))
                    .expect("signature");
                let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
                let parents = parent.iter().collect::<Vec<_>>();
                repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
                    .expect("commit");
            }
            _ => panic!("unsupported test git operation: {:?}", args),
        }
    }

    #[test]
    fn snapshot_paths_are_treated_as_canonical_story_paths() {
        assert!(is_canonical_story_path(".vizier/narrative/snapshot.md"));
        assert!(is_canonical_story_path("./.vizier/custom/snapshot.md"));
    }

    #[test]
    fn non_story_paths_are_ignored() {
        assert!(!is_canonical_story_path(".vizier/config.toml"));
        assert!(!is_canonical_story_path(".vizier/narrative/notes.txt"));
        assert!(!is_canonical_story_path(".vizier/narrative/"));
    }

    #[test]
    fn collect_vizier_changes_includes_staged_only_canonical_paths() {
        let tmp = tempdir().expect("tempdir");
        run_git(tmp.path(), &["init"]);
        run_git(tmp.path(), &["config", "user.name", "Test User"]);
        run_git(tmp.path(), &["config", "user.email", "test@example.com"]);

        fs::write(tmp.path().join("README.md"), "seed\n").expect("write seed");
        run_git(tmp.path(), &["add", "README.md"]);
        run_git(tmp.path(), &["commit", "-m", "init"]);

        let staged_path = tmp.path().join(".vizier/narrative/staged-only.md");
        fs::create_dir_all(
            staged_path
                .parent()
                .expect("staged narrative file should have a parent"),
        )
        .expect("create narrative dir");
        fs::write(&staged_path, "staged narrative update\n").expect("write staged narrative file");
        run_git(tmp.path(), &["add", ".vizier/narrative/staged-only.md"]);

        let changes =
            FileTracker::collect_vizier_changes(tmp.path()).expect("collect .vizier changes");
        assert!(
            changes.contains(&".vizier/narrative/staged-only.md".to_string()),
            "expected staged-only canonical narrative path in collected changes: {changes:?}"
        );
    }
}
