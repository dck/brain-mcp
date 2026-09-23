use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::watch;

use brain_core::error::BrainError;
use brain_core::service::MemoryService;
use brain_embed::create_embedder;
use brain_index::adapter::SqliteVecIndex;
use brain_mcp_proto::handler::McpHandler;
use brain_server::auth::generate_token;
use brain_server::http::{HttpServer, bind_loopback};
use brain_server::identity::ServerIdentity;
use brain_server::singleton::{ServerState, Singleton, SingletonError};
use brain_vault::VaultAdapter;

use super::{load_config, state_dir};
use crate::output;

pub async fn run(
    config_path: Option<PathBuf>,
    _daemonize: bool,
    stdio: bool,
) -> anyhow::Result<()> {
    if stdio {
        return run_stdio(config_path).await;
    }

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
    let service = Arc::new(
        MemoryService::new(vault, embedder.clone(), index)
            .with_min_score(config.search.min_score)
            .with_categories(config.vault.categories.clone()),
    );

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
        Err(SingletonError::Starting) => {
            eprintln!("  Another brain-mcp server is starting");
            std::process::exit(0);
        }
        Err(e) => return Err(e.into()),
    };

    // 6. Shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    drop(shutdown_rx);

    // 7. Bind the listener before writing state, so the state file never
    // outlives a port that failed to bind.
    let listener = bind_loopback(config.server.http_port).await?;
    let port = listener.local_addr()?.port();
    if port != config.server.http_port {
        eprintln!(
            "  Port {} is in use; listening on a random port instead",
            config.server.http_port
        );
    }
    let url = format!("http://127.0.0.1:{port}/mcp");
    let started_at = Utc::now();
    let token = generate_token();
    let identity = ServerIdentity::current(started_at);
    singleton.write_state(&ServerState {
        pid: std::process::id(),
        http: url.clone(),
        started_at,
        version: identity.version.clone(),
        token: token.clone(),
    })?;

    // 8. Signal handling
    let sig_tx = shutdown_tx.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = sig_tx.send(true);
    });

    // 9. Build handler + server
    let handler = Arc::new(McpHandler::new(service));
    let server = HttpServer::new(handler, identity, token, shutdown_tx);

    println!("{}", output::success(&format!("Listening on {url}")));

    // 10. Run (blocks until shutdown)
    server.serve(listener).await?;

    // Singleton dropped here, releasing lock + removing state file
    drop(singleton);
    println!("{}", output::success("Server stopped"));
    Ok(())
}

async fn run_stdio(_config_path: Option<PathBuf>) -> anyhow::Result<()> {
    super::proxy::run(state_dir()).await
}
