use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use lazy_static::lazy_static;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;

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

        // TODO: Is this really necessary?
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
    pub fn commit_changes(conversation_hash: &str, message: &str) -> std::io::Result<()> {
        if FILE_TRACKER.lock().unwrap().updated_files.len() == 0 {
            return Ok(());
        }

        std::process::Command::new("git")
            .args(&["add", &crate::tools::get_todo_dir()])
            .output()?;

        std::process::Command::new("git")
            .args(&[
                "commit",
                "-m",
                &format!(
                    "VIZIER\n\nConversation: {}\n\nVIZIER: {}",
                    conversation_hash, message
                ),
            ])
            .output()?;

        Self::clear();

        Ok(())
    }

    pub fn delete(path: &str) -> std::io::Result<()> {
        let mut tracker = FILE_TRACKER.lock().unwrap();

        tracker.updated_files.insert(path.to_string());
        tracker.all_files.retain(|f| f != path);

        Ok(())
    }

    pub fn read(path: &str) -> std::io::Result<String> {
        let matched = FileTracker::fuzzy_match_path(path);
        std::fs::read_to_string(matched)
    }

    fn clear() {
        FILE_TRACKER.lock().unwrap().updated_files.clear();
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
