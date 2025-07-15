use lazy_static::lazy_static;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;

lazy_static! {
    static ref FILE_TRACKER: Mutex<FileTracker> = Mutex::new(FileTracker::new());
}

pub struct FileTracker {
    updated_files: HashSet<String>,
}

impl FileTracker {
    fn new() -> Self {
        FileTracker {
            updated_files: HashSet::new(),
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

        println!("Updating Dewey...");

        // TODO: lol
        dewey_lib::upsert_embedding(path.to_string()).unwrap();
        FileTracker::clear();

        Ok(())
    }

    fn clear() {
        FILE_TRACKER.lock().unwrap().updated_files.clear();
    }
}
