use std::net::SocketAddr;
use std::sync::Arc;

use axum::{Router, extract::State, routing::post};
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
        let router = create_router(self.handler);
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

pub fn create_router(handler: Arc<McpHandler>) -> Router {
    Router::new()
        .route("/mcp", post(handle_mcp))
        .with_state(handler)
}

async fn handle_mcp(
    State(handler): State<Arc<McpHandler>>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;

    let request: Request = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            let resp = Response::error(None, -32700, format!("Parse error: {e}"));
            let json = serde_json::to_vec(&resp).unwrap();
            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json")],
                json,
            )
                .into_response();
        }
    };

    let resp = handler.handle(request).await;
    let json = serde_json::to_vec(&resp).unwrap();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        json,
    )
        .into_response()
}

/// Start the server on a random available port.
pub async fn run_on_random_port(
    handler: Arc<McpHandler>,
    shutdown: watch::Receiver<bool>,
) -> anyhow::Result<u16> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let router = create_router(handler);

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
    use axum::body::Body;
    use brain_core::mocks::{MockEmbedder, MockIndex, MockVault};
    use brain_core::service::MemoryService;
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;

    fn make_handler() -> Arc<McpHandler> {
        let vault = Arc::new(MockVault::new());
        let embedder = Arc::new(MockEmbedder::new(8));
        let index = Arc::new(MockIndex::new());
        let service = Arc::new(MemoryService::new(vault, embedder, index));
        Arc::new(McpHandler::new(service))
    }

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

    #[tokio::test]
    async fn test_http_handle_initialize() {
        let app = create_router(make_handler());
        let req = post_json(
            "/mcp",
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
        );

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);

        let parsed = response_json(resp).await;
        assert_eq!(parsed["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(parsed["result"]["serverInfo"]["name"], "brain-mcp");
    }

    #[tokio::test]
    async fn test_http_handle_tools_list() {
        let app = create_router(make_handler());
        let req = post_json(
            "/mcp",
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        );

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);

        let parsed = response_json(resp).await;
        let tools = parsed["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 6);
    }

    #[tokio::test]
    async fn test_http_handle_memory_store() {
        let app = create_router(make_handler());
        let req = post_json(
            "/mcp",
            json!({
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
            }),
        );

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);

        let parsed = response_json(resp).await;
        assert!(parsed.get("error").is_none());
        let text = parsed["result"]["content"][0]["text"].as_str().unwrap();
        let memory: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(memory["title"], "HTTP Test");
    }
}
