#[tokio::main]
async fn main() {
    if let Err(err) = vizier::run_cli().await {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
