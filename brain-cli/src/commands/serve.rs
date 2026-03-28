use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::watch;

use brain_core::error::BrainError;
use brain_core::service::MemoryService;
use brain_embed::create_embedder;
use brain_index::adapter::SqliteVecIndex;
use brain_mcp_proto::handler::McpHandler;
use brain_server::http::HttpServer;
use brain_server::singleton::{ServerState, Singleton, SingletonError};
use brain_vault::VaultAdapter;

use super::{load_config, state_dir};
use crate::output;

pub async fn run(config_path: Option<PathBuf>, _daemonize: bool) -> anyhow::Result<()> {
    // 1. Load config
    let config = load_config(config_path)?;

    // 2. Build adapters
    let vault = Arc::new(VaultAdapter::new(
        PathBuf::from(&config.vault.path),
        config.vault.templates_dir.clone(),
    ));

    let embedder = create_embedder(&config.embedding)?;

    let index_path = PathBuf::from(&config.index.path);
    if let Some(parent) = index_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let index = Arc::new(SqliteVecIndex::open(&index_path, embedder.dimensions())?);

    // 3. Build service
    let service = Arc::new(MemoryService::new(vault, embedder.clone(), index));

    // 4. Check model compatibility
    if let Err(e) = service.check_model_compatibility().await {
        if matches!(e, BrainError::ModelMismatch { .. }) {
            eprintln!("{}", output::error(&format!("{e}")));
            eprintln!(
                "  Run '{}' to rebuild the index with the new model.",
                console::style("brain-mcp reindex").bold()
            );
            std::process::exit(1);
        }
        return Err(e.into());
    }

    // 5. Acquire singleton
    let state_dir = state_dir();
    let singleton = match Singleton::acquire(&state_dir) {
        Ok(s) => s,
        Err(SingletonError::AlreadyRunning(state)) => {
            eprintln!(
                "  Server already running (PID {}) at {}",
                state.pid, state.http
            );
            std::process::exit(0);
        }
        Err(e) => return Err(e.into()),
    };

    // 6. Shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // 7. Write server state
    let url = format!("http://127.0.0.1:{}/mcp", config.server.http_port);
    let state = ServerState {
        pid: std::process::id(),
        http: url.clone(),
        started_at: Utc::now(),
    };
    singleton.write_state(&state)?;

    // 8. Signal handling
    let sig_tx = shutdown_tx.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = sig_tx.send(true);
    });

    // 9. Build handler + server
    let handler = Arc::new(McpHandler::new(service));
    let server = HttpServer::new(handler, config.server.http_port);

    println!("{}", output::success(&format!("Listening on {url}")));

    // 10. Run (blocks until shutdown)
    server.run(shutdown_rx).await?;

    // Singleton dropped here, releasing lock + removing state file
    drop(singleton);
    println!("{}", output::success("Server stopped"));
    Ok(())
}
