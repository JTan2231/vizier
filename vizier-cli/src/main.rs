mod actions;
mod cli;
mod completions;
mod errors;
mod jobs;
mod plan;
mod workflow_templates;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    cli::dispatch::run().await
}
