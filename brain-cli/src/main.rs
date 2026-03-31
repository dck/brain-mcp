mod commands;
mod output;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "brain-mcp",
    version,
    about = "Persistent memory for AI coding agents"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Path to config file
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Output as JSON
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Interactive first-time setup
    Init,
    /// Start the memory server
    Serve {
        /// Run in background
        #[arg(long)]
        daemonize: bool,
        /// Run as stdio bridge (for MCP command transport)
        #[arg(long)]
        stdio: bool,
    },
    /// Show server status
    Status,
    /// Stop the running server
    Stop,
    /// Full reindex of the vault
    Reindex,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Init => commands::init::run(cli.json).await,
        Commands::Serve { daemonize, stdio } => {
            commands::serve::run(cli.config, daemonize, stdio).await
        }
        Commands::Status => commands::status::run(cli.json).await,
        Commands::Stop => commands::stop::run().await,
        Commands::Reindex => commands::reindex::run(cli.config, cli.json).await,
    }
}
