#[derive(Debug)]
pub struct CancelledError {
    message: &'static str,
}

impl std::fmt::Display for CancelledError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CancelledError {}
