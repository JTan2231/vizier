mod actions;
mod cli;
mod completions;
mod errors;
mod jobs;
mod man;
mod plan;
mod workflow_templates;

pub use man::generate_man_pages;

pub async fn run_cli() -> Result<(), Box<dyn std::error::Error>> {
    cli::dispatch::run().await
}
