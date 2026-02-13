use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "gen-man",
    about = "Generate man1 pages from Vizier Clap command metadata"
)]
struct GenManArgs {
    /// Fail if checked-in man1 pages differ from generated output.
    #[arg(long = "check")]
    check: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = GenManArgs::parse();
    vizier::generate_man_pages(args.check)
}
