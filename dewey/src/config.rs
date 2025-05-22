use crate::error;

fn create_if_nonexistent(path: &std::path::PathBuf) {
    if !path.exists() {
        match std::fs::create_dir_all(&path) {
            Ok(_) => (),
            Err(e) => {
                error!("Failed to create directory: {:?}, {}", path, e);
                panic!("Failed to create directory: {:?}, {}", path, e);
            }
        };
    }
}

// TODO: :sob:

pub fn get_data_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(".dewey/")
}

pub fn get_local_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(".dewey/")
}

/// Setup for Dewey-specific files + directories
pub fn setup() -> Result<(), Box<dyn std::error::Error>> {
    let data_path = get_data_dir();

    create_if_nonexistent(&data_path);

    Ok(())
}
