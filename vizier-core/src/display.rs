use std::error::Error;
use std::io::stderr;

use crossterm::{
    cursor::MoveToColumn,
    execute,
    terminal::{Clear, ClearType},
};

use colored::*;
use tokio::sync::mpsc::{Receiver, Sender, channel};
use tokio::time::Duration;

pub enum Status {
    Working(String),
    Done,
    Error(String),
}

// TODO: There's a really annoying setup we have where we need to put carriage returns at the
//       beginning of task outputs to keep the terminal cursor from hovering the spinner/message
//       Essentially meaning that this function _does not_ clean up its message
async fn display_status(mut rx: Receiver<Status>) {
    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut i = 0usize;
    let mut last_message = String::new();

    loop {
        tokio::select! {
            Some(status) = rx.recv() => match status {
                Status::Working(msg) => {
                    last_message = msg.clone();
                    let _ = execute!(stderr(), MoveToColumn(0), Clear(ClearType::CurrentLine));
                    eprint!("{} {}", spinner[i % spinner.len()].blue(), msg.blue());
                    i = i.wrapping_add(1);
                }
                Status::Done => {
                    let _ = execute!(stderr(), MoveToColumn(0), Clear(ClearType::CurrentLine));
                    break;
                }
                Status::Error(e) => {
                    let _ = execute!(stderr(), MoveToColumn(0), Clear(ClearType::CurrentLine));
                    eprintln!("Error: {}", e);
                    break;
                }
            },
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                let _ = execute!(stderr(), MoveToColumn(0), Clear(ClearType::CurrentLine));
                eprint!("{} {}", spinner[i % spinner.len()].blue(), last_message.blue());
                i = i.wrapping_add(1);
            }
        }
    }
}

// TODO: Proper error handling
pub async fn call_with_status<F, Fut>(
    f: F,
) -> std::result::Result<Vec<wire::types::Message>, Box<dyn Error + Send + Sync>>
where
    F: FnOnce(Sender<Status>) -> Fut + Send + 'static,
    Fut: std::future::Future<
            Output = std::result::Result<Vec<wire::types::Message>, Box<dyn std::error::Error>>,
        > + Send
        + 'static,
{
    let (tx, rx) = channel(10);
    let _ = tokio::spawn(display_status(rx));

    let output = match f(tx.clone()).await {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(Status::Error(e.to_string())).await;
            Vec::new()
        }
    };

    let _ = tx.send(Status::Done).await;

    Ok(output)
}
