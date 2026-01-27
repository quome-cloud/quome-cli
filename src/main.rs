mod errors;

use clap::Parser;

#[derive(Parser)]
#[command(name = "quome")]
#[command(about = "CLI for the Quome platform")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Show version information
    Version,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Version => {
            println!("quome {}", env!("CARGO_PKG_VERSION"));
        }
    }

    Ok(())
}
