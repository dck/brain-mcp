use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

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

// --- stdio bridge mode ---

async fn run_stdio(_config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let state_dir = state_dir();

    // Check if server is already running, spawn if not
    let state = match read_existing_state(&state_dir).await {
        Some(state) => state,
        None => {
            eprintln!("Starting brain-mcp server...");
            spawn_server()?;
            wait_for_server(&state_dir, Duration::from_secs(30)).await?
        }
    };

    eprintln!("Connected to server at {}", state.http);
    run_stdio_bridge(&state.http).await
}

/// Read state and verify the HTTP endpoint is actually responding.
async fn read_existing_state(state_dir: &Path) -> Option<ServerState> {
    let state = Singleton::read_state(state_dir)?;
    let client = reqwest::Client::new();
    let ok = client
        .post(&state.http)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialize",
            "id": 0,
            "params": {}
        }))
        .send()
        .await
        .is_ok();
    if ok { Some(state) } else { None }
}

fn spawn_server() -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;

    let exe = std::env::current_exe()?;
    unsafe {
        std::process::Command::new(exe)
            .arg("serve")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .pre_exec(|| {
                libc::setsid();
                Ok(())
            })
            .spawn()?;
    }
    Ok(())
}

async fn wait_for_server(state_dir: &Path, timeout: Duration) -> anyhow::Result<ServerState> {
    let start = Instant::now();
    loop {
        if let Some(state) = Singleton::read_state(state_dir) {
            let client = reqwest::Client::new();
            if client
                .post(&state.http)
                .json(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "initialize",
                    "id": 0,
                    "params": {}
                }))
                .send()
                .await
                .is_ok()
            {
                return Ok(state);
            }
        }
        if start.elapsed() > timeout {
            anyhow::bail!("Timeout waiting for server to start");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn run_stdio_bridge(http_url: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let url = http_url.to_string();

    // Read stdin in a blocking thread — tokio's async stdin can miss
    // pipe input from parent processes like Claude Code.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(32);
    std::thread::spawn(move || {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) if !line.trim().is_empty() => {
                    if tx.blocking_send(line).is_err() {
                        break;
                    }
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    });

    while let Some(line) = rx.recv().await {
        let response = client
            .post(&url)
            .header("Content-Type", "application/json")
            .body(line)
            .send()
            .await;

        match response {
            Ok(resp) => {
                use std::io::Write;
                let body = resp.bytes().await?;
                let mut stdout = std::io::stdout().lock();
                stdout.write_all(&body)?;
                stdout.write_all(b"\n")?;
                stdout.flush()?;
            }
            Err(e) => {
                eprintln!("HTTP request failed: {e}");
                anyhow::bail!("Server unreachable");
            }
        }
    }

    Ok(())
}
