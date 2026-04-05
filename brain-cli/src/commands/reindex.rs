use std::path::PathBuf;
use std::sync::Arc;

use indicatif::ProgressBar;
use serde_json::json;

use brain_core::service::MemoryService;
use brain_embed::create_embedder;
use brain_index::adapter::SqliteVecIndex;
use brain_server::singleton::Singleton;
use brain_vault::VaultAdapter;

use super::{load_config, state_dir};
use crate::output;

pub async fn run(config_path: Option<PathBuf>, json_output: bool) -> anyhow::Result<()> {
    // Check if a server is already running — if so, POST to it.
    let state = Singleton::read_state(&state_dir());

    if let Some(state) = state {
        return reindex_via_server(&state.http, json_output).await;
    }

    // No running server — do it locally.
    reindex_local(config_path, json_output).await
}

async fn reindex_via_server(base_url: &str, json_output: bool) -> anyhow::Result<()> {
    let spinner = if !json_output {
        let sp = ProgressBar::new_spinner();
        sp.set_message("Reindexing via running server...");
        sp.enable_steady_tick(std::time::Duration::from_millis(100));
        Some(sp)
    } else {
        None
    };

    let client = reqwest::Client::builder().no_proxy().build()?;
    let resp: serde_json::Value = client
        .post(base_url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "memory_reindex",
                "arguments": {}
            }
        }))
        .send()
        .await?
        .json()
        .await?;

    if let Some(sp) = spinner {
        sp.finish_and_clear();
    }

    if let Some(err) = resp.get("error") {
        let msg = err["message"].as_str().unwrap_or("unknown error");
        eprintln!("{}", output::error(&format!("Reindex failed: {msg}")));
        std::process::exit(1);
    }

    if json_output {
        println!("{}", serde_json::to_string_pretty(&resp)?);
    } else {
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("done");
        println!("{}", output::success(&format!("Reindex complete: {text}")));
    }

    Ok(())
}

async fn reindex_local(config_path: Option<PathBuf>, json_output: bool) -> anyhow::Result<()> {
    let config = load_config(config_path)?;

    let spinner = if !json_output {
        let sp = ProgressBar::new_spinner();
        sp.set_message("Reindexing...");
        sp.enable_steady_tick(std::time::Duration::from_millis(100));
        Some(sp)
    } else {
        None
    };

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

    let service = MemoryService::new(vault, embedder, index);
    let count = service.reindex().await?;

    if let Some(sp) = spinner {
        sp.finish_and_clear();
    }

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "reindexed": count }))?
        );
    } else {
        println!(
            "{}",
            output::success(&format!("Reindexed {count} memories"))
        );
    }

    Ok(())
}
