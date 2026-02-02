mod actions;
mod cli;
mod completions;
mod errors;
mod jobs;
mod plan;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    cli::dispatch::run().await
}
