use std::io::Read;

use grep_searcher::{Searcher, Sink, SinkContext, SinkFinish, SinkMatch};
use ignore::WalkBuilder;

#[derive(Debug, Clone)]
pub struct Match {
    pub before: Vec<String>,
    pub line: String,
    pub after: Vec<String>,
}

pub struct MatchCollector {
    pub current_match: Option<Match>,
    pub matches: Vec<Match>,
    pub context_before: Vec<String>,
}

impl Sink for MatchCollector {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch) -> Result<bool, Self::Error> {
        // TODO: this isn't catching context_before for some reason
        if let Some(cm) = &self.current_match {
            self.matches.push(cm.clone());
        }

        let line = String::from_utf8_lossy(mat.bytes()).into_owned();

        let match_entry = Match {
            before: self.context_before.drain(..).collect(),
            line,
            after: Vec::new(),
        };

        self.current_match = Some(match_entry);
        Ok(true)
    }

    fn context(&mut self, _searcher: &Searcher, ctx: &SinkContext) -> Result<bool, Self::Error> {
        let line = String::from_utf8_lossy(ctx.bytes()).into_owned();

        match &mut self.current_match {
            Some(m) => m.after.push(line),
            None => self.context_before.push(line),
        }

        Ok(true)
    }

    fn finish(&mut self, _searcher: &Searcher, _: &SinkFinish) -> Result<(), Self::Error> {
        if let Some(match_entry) = self.current_match.take() {
            self.matches.push(match_entry);
        }

        Ok(())
    }
}

pub fn is_binary(path: &std::path::Path) -> std::io::Result<bool> {
    let mut byte = [0; 1];
    let mut file = std::fs::File::open(path)?;
    file.read_exact(&mut byte)?;
    Ok(byte[0] == 0)
}

pub fn default_walker() -> ignore::Walk {
    WalkBuilder::new(std::env::current_dir().unwrap())
        .add_custom_ignore_filename("vizier.db")
        .build()
}

pub fn get_non_ignored_files() -> Vec<std::path::PathBuf> {
    let mut files = default_walker()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map_or(false, |ft| ft.is_file()))
        .map(|entry| entry.path().to_owned())
        .collect::<Vec<_>>();

    if let Ok(extra_entries) = std::fs::read_dir(crate::tools::TODO_DIR) {
        files.extend(
            extra_entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().map_or(false, |ft| ft.is_file()))
                .map(|entry| entry.path()),
        );
    }

    files
}
