use std::net::SocketAddr;
use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use brain_mcp_proto::handler::McpHandler;
use brain_mcp_proto::jsonrpc::{Request, Response};
use tokio::sync::watch;

pub struct HttpServer {
    handler: Arc<McpHandler>,
    port: u16,
}

impl HttpServer {
    pub fn new(handler: Arc<McpHandler>, port: u16) -> Self {
        Self { handler, port }
    }

    pub async fn run(self, shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
        let router = Router::new()
            .route("/mcp", post(handle_mcp))
            .with_state(self.handler);

        let addr = SocketAddr::from(([127, 0, 0, 1], self.port));
        let listener = tokio::net::TcpListener::bind(addr).await?;

        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let mut rx = shutdown;
                let _ = rx.changed().await;
            })
            .await?;
        Ok(())
    }
}

async fn handle_mcp(
    State(handler): State<Arc<McpHandler>>,
    Json(request): Json<Request>,
) -> Json<Response> {
    Json(handler.handle(request).await)
}

/// Start the server on a random available port.
///
/// Returns the port the server bound to. The server runs in a spawned task and
/// will shut down when `shutdown` fires.
pub async fn run_on_random_port(
    handler: Arc<McpHandler>,
    shutdown: watch::Receiver<bool>,
) -> anyhow::Result<u16> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();

    let router = Router::new()
        .route("/mcp", post(handle_mcp))
        .with_state(handler);

    tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let mut rx = shutdown;
                let _ = rx.changed().await;
            })
            .await;
    });

    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use brain_core::mocks::{MockEmbedder, MockIndex, MockVault};
    use brain_core::service::MemoryService;
    use serde_json::json;

    fn make_handler() -> Arc<McpHandler> {
        let vault = Arc::new(MockVault::new());
        let embedder = Arc::new(MockEmbedder::new(8));
        let index = Arc::new(MockIndex::new());
        let service = Arc::new(MemoryService::new(vault, embedder, index));
        Arc::new(McpHandler::new(service))
    }

    async fn start_server() -> (u16, watch::Sender<bool>) {
        let handler = make_handler();
        let (tx, rx) = watch::channel(false);
        let port = run_on_random_port(handler, rx).await.unwrap();
        (port, tx)
    }

    #[tokio::test]
    async fn test_http_handle_initialize() {
        let (port, _tx) = start_server().await;
        let client = reqwest::Client::new();
        let resp: serde_json::Value = client
            .post(format!("http://127.0.0.1:{port}/mcp"))
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize"
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(resp["result"]["serverInfo"]["name"], "brain-mcp");
    }

    #[tokio::test]
    async fn test_http_handle_tools_list() {
        let (port, _tx) = start_server().await;
        let client = reqwest::Client::new();
        let resp: serde_json::Value = client
            .post(format!("http://127.0.0.1:{port}/mcp"))
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list"
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 6);
    }

    #[tokio::test]
    async fn test_http_handle_memory_store() {
        let (port, _tx) = start_server().await;
        let client = reqwest::Client::new();
        let resp: serde_json::Value = client
            .post(format!("http://127.0.0.1:{port}/mcp"))
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "memory_store",
                    "arguments": {
                        "title": "HTTP Test",
                        "content": "Stored via HTTP",
                        "tags": ["http", "test"]
                    }
                }
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        assert!(resp.get("error").is_none());
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let memory: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(memory["title"], "HTTP Test");
    }
}
