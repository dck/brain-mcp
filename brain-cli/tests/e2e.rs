use std::sync::Arc;

use axum::body::Body;
use brain_server::http::create_router;
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;

fn post_json(uri: &str, body: serde_json::Value) -> axum::http::Request<Body> {
    axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

async fn response_json(resp: axum::http::Response<Body>) -> serde_json::Value {
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

async fn mcp_call(
    app: &axum::Router,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let req = post_json(
        "/mcp",
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params
        }),
    );
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    response_json(resp).await
}

async fn tool_call(app: &axum::Router, tool: &str, args: serde_json::Value) -> serde_json::Value {
    mcp_call(app, "tools/call", json!({"name": tool, "arguments": args})).await
}

#[tokio::test]
async fn full_roundtrip() {
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
    let app = create_router(handler);

    // 1. Initialize
    let resp = mcp_call(&app, "initialize", json!({})).await;
    assert_eq!(resp["result"]["serverInfo"]["name"], "brain-mcp");

    // 2. List tools
    let resp = mcp_call(&app, "tools/list", json!({})).await;
    let tools = resp["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 6);

    // 3. Store a memory
    let resp = tool_call(
        &app,
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

    // 4. Search
    let resp = tool_call(&app, "memory_search", json!({"query": "deploy terraform"})).await;
    assert!(resp.get("error").is_none(), "search failed: {resp}");
    let results_text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(results_text.contains("Deploy process"));

    // 5. List
    let resp = tool_call(&app, "memory_list", json!({"category": "procedures"})).await;
    assert!(resp.get("error").is_none(), "list failed: {resp}");
    let list_text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(list_text.contains(&memory_id));

    // 6. Update
    let resp = tool_call(
        &app,
        "memory_update",
        json!({"id": memory_id, "title": "Updated deploy process"}),
    )
    .await;
    assert!(resp.get("error").is_none(), "update failed: {resp}");
    let updated_text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(updated_text.contains("Updated deploy process"));

    // 7. Verify on disk
    let file_path = tmp
        .path()
        .join("procedures")
        .join(format!("{memory_id}.md"));
    let on_disk = std::fs::read_to_string(&file_path).unwrap();
    assert!(on_disk.contains("Updated deploy process"));

    // 8. Delete
    let resp = tool_call(&app, "memory_delete", json!({"id": memory_id})).await;
    assert!(resp.get("error").is_none(), "delete failed: {resp}");

    // 9. Search returns empty
    let resp = tool_call(&app, "memory_search", json!({"query": "deploy terraform"})).await;
    let empty_text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let empty_results: Vec<serde_json::Value> = serde_json::from_str(empty_text).unwrap();
    assert!(empty_results.is_empty());

    // 10. Reindex returns 0
    let resp = tool_call(&app, "memory_reindex", json!({})).await;
    let reindex_text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(reindex_text.contains("\"reindexed\":0"));
}
