mod commands;
mod output;

#[cfg(not(unix))]
compile_error!("brain-mcp supports macOS and Linux only");

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
        /// Run as stdio bridge (for MCP command transport)
        #[arg(long)]
        stdio: bool,
    },
    /// Print a compact memory index for context injection (e.g. SessionStart hooks)
    Recall {
        /// Only include memories for this project (cross-project memories are always included)
        #[arg(long)]
        project: Option<String>,
        /// Maximum number of memories to include
        #[arg(long, default_value_t = 30)]
        limit: usize,
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
    let cli = Cli::parse();
    let default_filter = match &cli.command {
        Commands::Serve { stdio: false } => "info",
        _ => "warn",
    };
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter)),
        )
        .init();

    match cli.command {
        Commands::Init => commands::init::run(cli.json).await,
        Commands::Serve { stdio } => commands::serve::run(cli.config, stdio).await,
        Commands::Recall { project, limit } => {
            commands::recall::run(cli.config, project, limit).await
        }
        Commands::Status => commands::status::run(cli.json).await,
        Commands::Stop => commands::stop::run().await,
        Commands::Reindex => commands::reindex::run(cli.config, cli.json).await,
    }
}
