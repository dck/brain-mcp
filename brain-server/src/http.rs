use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response as HttpResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use brain_mcp_proto::handler::McpHandler;
use brain_mcp_proto::jsonrpc::{INVALID_REQUEST, PARSE_ERROR, Request, Response};
use serde_json::json;
use tokio::sync::watch;

use crate::auth;
use crate::identity::ServerIdentity;

#[derive(Clone)]
struct AppState {
    handler: Arc<McpHandler>,
    identity: Arc<ServerIdentity>,
    token: Arc<str>,
    shutdown: watch::Sender<bool>,
}

pub struct HttpServer {
    state: AppState,
}

impl HttpServer {
    pub fn new(
        handler: Arc<McpHandler>,
        identity: ServerIdentity,
        token: String,
        shutdown: watch::Sender<bool>,
    ) -> Self {
        Self {
            state: AppState {
                handler,
                identity: Arc::new(identity),
                token: Arc::from(token),
                shutdown,
            },
        }
    }

    fn router(&self) -> Router {
        Router::new()
            .route("/health", get(handle_health))
            .route("/mcp", post(handle_mcp))
            .route("/shutdown", post(handle_shutdown))
            .with_state(self.state.clone())
    }

    pub async fn serve(self, listener: tokio::net::TcpListener) -> anyhow::Result<()> {
        let mut rx = self.state.shutdown.subscribe();
        let router = self.router();
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = rx.wait_for(|v| *v).await;
            })
            .await?;
        Ok(())
    }
}

pub async fn bind_loopback(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    match tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).await {
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && port != 0 => {
            tracing::warn!(port, "port in use, binding a random loopback port");
            tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await
        }
        other => other,
    }
}

async fn handle_health(State(s): State<AppState>, headers: HeaderMap) -> HttpResponse {
    if let Err(r) = auth::check(&headers, None) {
        return r.into_response();
    }
    Json((*s.identity).clone()).into_response()
}

async fn handle_mcp(State(s): State<AppState>, headers: HeaderMap, body: Bytes) -> HttpResponse {
    if let Err(r) = auth::check(&headers, Some(&s.token)) {
        return r.into_response();
    }
    let value: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return Json(Response::error(
                Some(serde_json::Value::Null),
                PARSE_ERROR,
                format!("Parse error: {e}"),
            ))
            .into_response();
        }
    };
    let id = value.get("id").cloned();
    let is_notification = value.as_object().is_some_and(|o| !o.contains_key("id"));
    let request: Request = match serde_json::from_value(value) {
        Ok(r) => r,
        Err(_) => {
            return Json(Response::error(
                Some(id.unwrap_or(serde_json::Value::Null)),
                INVALID_REQUEST,
                "Invalid request",
            ))
            .into_response();
        }
    };
    if is_notification {
        return StatusCode::ACCEPTED.into_response();
    }
    Json(s.handler.handle(request).await).into_response()
}

async fn handle_shutdown(State(s): State<AppState>, headers: HeaderMap) -> HttpResponse {
    if let Err(r) = auth::check(&headers, Some(&s.token)) {
        return r.into_response();
    }
    tracing::info!("shutdown requested over HTTP");
    let _ = s.shutdown.send(true);
    (StatusCode::ACCEPTED, Json(json!({"shutting_down": true}))).into_response()
}

/// Start the server on a random available port.
///
/// Returns the port the server bound to. The server runs in a spawned task and
/// will shut down when `shutdown` fires.
pub async fn run_on_random_port(
    handler: Arc<McpHandler>,
    token: String,
    shutdown: watch::Sender<bool>,
) -> anyhow::Result<u16> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let server = HttpServer::new(
        handler,
        ServerIdentity::current(chrono::Utc::now()),
        token,
        shutdown,
    );
    tokio::spawn(async move {
        let _ = server.serve(listener).await;
    });
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::generate_token;
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

    async fn start_server() -> (u16, String, watch::Sender<bool>) {
        let handler = make_handler();
        let token = generate_token();
        let (tx, _rx) = watch::channel(false);
        let port = run_on_random_port(handler, token.clone(), tx.clone())
            .await
            .unwrap();
        (port, token, tx)
    }

    #[tokio::test]
    async fn test_http_handle_initialize() {
        let (port, token, _tx) = start_server().await;
        let client = reqwest::Client::new();
        let resp: serde_json::Value = client
            .post(format!("http://127.0.0.1:{port}/mcp"))
            .bearer_auth(&token)
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

        assert_eq!(resp["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(resp["result"]["serverInfo"]["name"], "brain-mcp");
    }

    #[tokio::test]
    async fn test_http_handle_tools_list() {
        let (port, token, _tx) = start_server().await;
        let client = reqwest::Client::new();
        let resp: serde_json::Value = client
            .post(format!("http://127.0.0.1:{port}/mcp"))
            .bearer_auth(&token)
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
        let (port, token, _tx) = start_server().await;
        let client = reqwest::Client::new();
        let resp: serde_json::Value = client
            .post(format!("http://127.0.0.1:{port}/mcp"))
            .bearer_auth(&token)
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

    #[tokio::test]
    async fn test_http_parse_error_is_jsonrpc() {
        let (port, token, _tx) = start_server().await;
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{port}/mcp"))
            .bearer_auth(&token)
            .header("Content-Type", "application/json")
            .body("not json")
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"]["code"], -32700);
        assert!(body["id"].is_null());
        assert!(body.as_object().unwrap().contains_key("id"));
    }

    #[tokio::test]
    async fn test_http_invalid_request() {
        let (port, token, _tx) = start_server().await;
        let client = reqwest::Client::new();
        let resp: serde_json::Value = client
            .post(format!("http://127.0.0.1:{port}/mcp"))
            .bearer_auth(&token)
            .json(&json!({"jsonrpc": "2.0", "id": 9}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        assert_eq!(resp["error"]["code"], -32600);
        assert_eq!(resp["id"], 9);
    }

    #[tokio::test]
    async fn test_http_notification_returns_202() {
        let (port, token, _tx) = start_server().await;
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{port}/mcp"))
            .bearer_auth(&token)
            .json(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 202);
        let body = resp.bytes().await.unwrap();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn test_http_ping() {
        let (port, token, _tx) = start_server().await;
        let client = reqwest::Client::new();
        let resp: serde_json::Value = client
            .post(format!("http://127.0.0.1:{port}/mcp"))
            .bearer_auth(&token)
            .json(&json!({"jsonrpc": "2.0", "id": 4, "method": "ping"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        assert_eq!(resp["result"], json!({}));
    }

    #[tokio::test]
    async fn test_mcp_requires_token() {
        let (port, token, _tx) = start_server().await;
        let client = reqwest::Client::new();

        let resp = client
            .post(format!("http://127.0.0.1:{port}/mcp"))
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
        assert_eq!(resp.headers().get("www-authenticate").unwrap(), "Bearer");

        let resp = client
            .post(format!("http://127.0.0.1:{port}/mcp"))
            .bearer_auth("wrong")
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);

        let resp = client
            .post(format!("http://127.0.0.1:{port}/mcp"))
            .bearer_auth(&token)
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn test_origin_rejected() {
        let (port, token, _tx) = start_server().await;
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{port}/mcp"))
            .bearer_auth(&token)
            .header("Origin", "http://evil.com")
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 403);
    }

    #[tokio::test]
    async fn test_host_rejected() {
        let (port, _token, _tx) = start_server().await;
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let body = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}";
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: evil.com\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        stream.write_all(request.as_bytes()).await.unwrap();
        stream.write_all(body).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    }

    #[tokio::test]
    async fn test_health_is_public() {
        let (port, _token, _tx) = start_server().await;
        let client = reqwest::Client::new();
        let resp: serde_json::Value = client
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(resp["name"], "brain-mcp");
        assert_eq!(resp["pid"], std::process::id());
        assert_eq!(resp["version"], env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn test_shutdown_signals() {
        let (port, token, tx) = start_server().await;
        let mut rx = tx.subscribe();
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{port}/shutdown"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 202);

        tokio::time::timeout(std::time::Duration::from_secs(1), rx.wait_for(|v| *v))
            .await
            .expect("shutdown signalled")
            .unwrap();
    }

    #[tokio::test]
    async fn test_bind_loopback_falls_back() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p = taken.local_addr().unwrap().port();
        let listener = bind_loopback(p).await.unwrap();
        assert_ne!(listener.local_addr().unwrap().port(), p);
    }
}
