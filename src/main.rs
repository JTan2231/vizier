use clap::Parser;
use rusqlite::{Connection, ffi::sqlite3_auto_extension};
use sqlite_vec::sqlite3_vec_init;

#[derive(Parser)]
#[command(version, about = "A CLI for LLM project management.")]
struct Args {
    /// Path to the SQLite database file
    db_path: Option<String>,
}

fn init_db(db_path: std::path::PathBuf) -> Result<Connection, Box<dyn std::error::Error>> {
    // Embedding extension for SQLite
    // See: https://github.com/asg017/sqlite-vec
    unsafe {
        sqlite3_auto_extension(Some(std::mem::transmute(sqlite3_vec_init as *const ())));
    }

    let conn = Connection::open(db_path)?;

    let init_sql = std::fs::read_to_string("./db.sql")?;
    conn.execute_batch(&init_sql)?;

    Ok(conn)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let db_path = if args.db_path.is_none() {
        std::path::PathBuf::from("./vizier.db")
    } else {
        std::path::PathBuf::from(args.db_path.unwrap().clone())
    };

    let conn = init_db(db_path)?;

    Ok(())
}
