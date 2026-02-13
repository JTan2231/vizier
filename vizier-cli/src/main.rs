#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    vizier::run_cli().await
}
