use std::io::Read;

use clap::Parser;
use grep_regex::RegexMatcher;
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkContext, SinkFinish, SinkMatch};
use ignore::WalkBuilder;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use vizier_macros::tool;

#[derive(Debug)]
pub struct ToolInfo {
    pub name: &'static str,
    pub schema: fn() -> serde_json::Value,
}
inventory::collect!(ToolInfo);

#[derive(Parser)]
#[command(version, about = "A CLI for LLM project management.")]
struct Args {
    /// Path to the SQLite database file
    db_path: Option<String>,
}

const SQL_INIT: &str = r#"
CREATE TABLE IF NOT EXISTS needs (
    id INTEGER PRIMARY KEY,
    content TEXT NOT NULL
);
"#;

const TODO_FILE: &str = "VIZIER.md";

fn init_db(db_path: std::path::PathBuf) -> Result<Connection, Box<dyn std::error::Error>> {
    let conn = Connection::open(db_path)?;
    conn.execute_batch(&SQL_INIT)?;

    Ok(conn)
}

#[derive(Debug, Clone)]
struct Match {
    before: Vec<String>,
    line: String,
    after: Vec<String>,
}

struct MatchCollector {
    current_match: Option<Match>,
    matches: Vec<Match>,
    context_before: Vec<String>,
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

fn is_binary(path: &std::path::Path) -> std::io::Result<bool> {
    let mut byte = [0; 1];
    let mut file = std::fs::File::open(path)?;
    file.read_exact(&mut byte)?;
    Ok(byte[0] == 0)
}

fn get_todos() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let matcher = RegexMatcher::new("TODO:")?;
    let mut searcher = SearcherBuilder::new()
        .before_context(10)
        .after_context(10)
        .build();

    let mut collector = MatchCollector {
        current_match: None,
        matches: Vec::new(),
        context_before: Vec::new(),
    };

    let walker = WalkBuilder::new(std::env::current_dir()?)
        .add_custom_ignore_filename("vizier.db")
        .build();

    for result in walker {
        if let Ok(entry) = result {
            let path = entry.path();
            // not happy about having to check whether a given file is a binary
            if path.is_file() && !is_binary(&path)? {
                searcher.search_path(&matcher, path, &mut collector)?;
            }
        }
    }

    Ok(collector
        .matches
        .iter()
        .map(|m| format!("{}{}{}", m.before.join(""), m.line, m.after.join("")))
        .collect::<Vec<String>>())
}

/// NOTE: Quick and dirty macro for identifying tools and creating arg structs for them
///       See `vizier-macros`
#[tool]
fn diff() -> String {
    let output = std::process::Command::new("git diff")
        .output()
        .expect("failed to run");

    String::from_utf8(output.stdout).unwrap()
}

// TODO: This should probably be generalized at some point
fn get_tools_context() -> String {
    let mut prompt =
        "You are a provided a variety of tools for gathering more information or context.\n\n"
            .to_string();

    prompt.push_str("");
    prompt
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let db_path = if args.db_path.is_none() {
        std::path::PathBuf::from("./vizier.db")
    } else {
        std::path::PathBuf::from(args.db_path.unwrap().clone())
    };

    // let conn = init_db(db_path)?;
    //
    // let todos = get_todos()?;

    for tool in inventory::iter::<ToolInfo> {
        println!("{}: {}", tool.name, (tool.schema)());
    }

    Ok(())
}
