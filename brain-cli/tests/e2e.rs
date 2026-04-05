use std::sync::Arc;

use serde_json::json;

async fn mcp_call(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params
    });
    let resp = client.post(url).json(&body).send().await.unwrap();
    resp.json().await.unwrap()
}

async fn tool_call(
    client: &reqwest::Client,
    url: &str,
    tool: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    mcp_call(
        client,
        url,
        "tools/call",
        json!({
            "name": tool,
            "arguments": args
        }),
    )
    .await
}

#[tokio::test]
async fn full_roundtrip() {
    // Setup: temp vault, mock embedder, in-memory index, start server
    let tmp = tempfile::tempdir().unwrap();
    let vault = Arc::new(brain_vault::adapter::VaultAdapter::new(
        tmp.path().to_path_buf(),
        "_templates".into(),
    ));
    let embedder = Arc::new(brain_core::mocks::MockEmbedder::new(8));
    let index = Arc::new(brain_index::adapter::SqliteVecIndex::open_in_memory(8).unwrap());
    let service = Arc::new(brain_core::service::MemoryService::new(
        vault, embedder, index,
    ));
    let handler = Arc::new(brain_mcp_proto::handler::McpHandler::new(service));

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let port = brain_server::http::run_on_random_port(handler, shutdown_rx)
        .await
        .unwrap();
    let url = format!("http://127.0.0.1:{port}/mcp");
    let client = reqwest::Client::new();

    // 1. Initialize
    let resp = mcp_call(&client, &url, "initialize", json!({})).await;
    assert_eq!(resp["result"]["serverInfo"]["name"], "brain-mcp");

    // 2. List tools
    let resp = mcp_call(&client, &url, "tools/list", json!({})).await;
    let tools = resp["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 6);

    // 3. Store a memory
    let resp = tool_call(
        &client,
        &url,
        "memory_store",
        json!({
            "title": "Deploy process",
            "content": "Run terraform apply then helm upgrade",
            "tags": ["deploy", "terraform"],
            "category": "procedures"
        }),
    )
    .await;
    assert!(resp.get("error").is_none(), "store failed: {resp}");
    let content_text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let stored: serde_json::Value = serde_json::from_str(content_text).unwrap();
    let memory_id = stored["id"].as_str().unwrap().to_string();

    // 4. Search for it
    let resp = tool_call(
        &client,
        &url,
        "memory_search",
        json!({ "query": "deploy terraform" }),
    )
    .await;
    assert!(resp.get("error").is_none(), "search failed: {resp}");
    let results_text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(results_text.contains("Deploy process"));

    // 5. List memories
    let resp = tool_call(
        &client,
        &url,
        "memory_list",
        json!({ "category": "procedures" }),
    )
    .await;
    assert!(resp.get("error").is_none(), "list failed: {resp}");
    let list_text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(list_text.contains(&memory_id));

    // 6. Update it
    let resp = tool_call(
        &client,
        &url,
        "memory_update",
        json!({
            "id": memory_id,
            "title": "Updated deploy process"
        }),
    )
    .await;
    assert!(resp.get("error").is_none(), "update failed: {resp}");
    let updated_text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(updated_text.contains("Updated deploy process"));

    // 7. Verify the file on disk has correct content
    let file_path = tmp
        .path()
        .join("procedures")
        .join(format!("{memory_id}.md"));
    let on_disk = std::fs::read_to_string(&file_path).unwrap();
    assert!(on_disk.contains("Updated deploy process"));

    // 8. Delete it
    let resp = tool_call(&client, &url, "memory_delete", json!({ "id": memory_id })).await;
    assert!(resp.get("error").is_none(), "delete failed: {resp}");

    // 9. Verify search returns empty results
    let resp = tool_call(
        &client,
        &url,
        "memory_search",
        json!({ "query": "deploy terraform" }),
    )
    .await;
    assert!(
        resp.get("error").is_none(),
        "search-after-delete failed: {resp}"
    );
    let empty_text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let empty_results: Vec<serde_json::Value> = serde_json::from_str(empty_text).unwrap();
    assert!(empty_results.is_empty(), "expected no results after delete");

    // 10. Reindex (should return 0 since the memory was deleted)
    let resp = tool_call(&client, &url, "memory_reindex", json!({})).await;
    assert!(resp.get("error").is_none(), "reindex failed: {resp}");
    let reindex_text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(reindex_text.contains("\"reindexed\":0"));

    // Shutdown server
    let _ = shutdown_tx.send(true);
}
