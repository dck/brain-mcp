use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Arc;
use std::time::{Duration, Instant};

use brain_mcp_proto::jsonrpc::{INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR, Request, Response};
use brain_mcp_proto::mcp::{initialize_result, tool_error};
use brain_mcp_proto::schema::tool_definitions;
use brain_server::singleton::Singleton;
use serde_json::{Value, json};

const SPAWN_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const TOOL_TIMEOUT: Duration = Duration::from_secs(60);
const REINDEX_TIMEOUT: Duration = Duration::from_secs(600);
const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
const LOG_TAIL_BYTES: u64 = 4096;
const LOG_TAIL_LINES: usize = 10;

pub fn server_log_path(state_dir: &Path) -> PathBuf {
    state_dir.join("server.log")
}

#[derive(Debug)]
enum UpstreamError {
    Spawn(std::io::Error),
    Exited(std::process::ExitStatus),
    StartTimeout,
    Request {
        error: reqwest::Error,
        timeout: Duration,
    },
    Status(u16, String),
}

impl std::fmt::Display for UpstreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpstreamError::Spawn(io) => write!(f, "could not start server: {io}"),
            UpstreamError::Exited(status) => {
                write!(f, "server exited during startup ({status})")
            }
            UpstreamError::StartTimeout => {
                write!(f, "server did not become ready within 10s")
            }
            UpstreamError::Request { error, timeout } => {
                if error.is_timeout() {
                    write!(
                        f,
                        "request timed out after {}s; the server may be hung. Run `brain-mcp stop` and retry",
                        timeout.as_secs()
                    )
                } else {
                    write!(f, "request failed: {error}")
                }
            }
            UpstreamError::Status(code, body) => {
                let truncated: String = body.chars().take(200).collect();
                write!(f, "server returned HTTP {code}: {truncated}")
            }
        }
    }
}

struct Upstream {
    state_dir: PathBuf,
    client: reqwest::Client,
    url: tokio::sync::Mutex<Option<String>>,
    spawn_enabled: bool,
}

impl Upstream {
    fn new(state_dir: PathBuf, spawn_enabled: bool) -> Self {
        let client = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .expect("reqwest client without proxy never fails to build");
        Self {
            state_dir,
            client,
            url: tokio::sync::Mutex::new(None),
            spawn_enabled,
        }
    }

    async fn ensure(&self) -> Result<String, UpstreamError> {
        let mut guard = self.url.lock().await;
        if let Some(url) = guard.as_ref() {
            return Ok(url.clone());
        }

        if let Some(state) = Singleton::read_live_state(&self.state_dir)
            && self.probe(&state.http).await
        {
            *guard = Some(state.http.clone());
            return Ok(state.http);
        }

        if !self.spawn_enabled {
            return Err(UpstreamError::StartTimeout);
        }

        let mut child = spawn_server(&self.state_dir).map_err(UpstreamError::Spawn)?;
        let url = self.wait_for_server(&mut child).await?;
        *guard = Some(url.clone());
        Ok(url)
    }

    async fn probe(&self, url: &str) -> bool {
        let resp = match self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .json(&json!({"jsonrpc": "2.0", "id": 0, "method": "ping"}))
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(_) => return false,
        };
        if resp.status() != reqwest::StatusCode::OK {
            return false;
        }
        let body: Value = match resp.json().await {
            Ok(body) => body,
            Err(_) => return false,
        };
        body.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
            && (body.get("result").is_some() || body.get("error").is_some())
    }

    async fn invalidate(&self) {
        let mut guard = self.url.lock().await;
        *guard = None;
    }

    async fn post(
        &self,
        url: &str,
        body: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, UpstreamError> {
        let resp = self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .body(body.to_vec())
            .timeout(timeout)
            .send()
            .await
            .map_err(|error| UpstreamError::Request { error, timeout })?;

        let status = resp.status();
        if status != reqwest::StatusCode::OK {
            let text = resp.text().await.unwrap_or_else(|_| String::new());
            return Err(UpstreamError::Status(status.as_u16(), text));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|error| UpstreamError::Request { error, timeout })?;
        Ok(bytes.to_vec())
    }

    async fn forward(&self, body: String, timeout: Duration) -> Result<Vec<u8>, UpstreamError> {
        let url = self.ensure().await?;
        match self.post(&url, body.as_bytes(), timeout).await {
            Err(UpstreamError::Request { error, .. }) if error.is_connect() => {
                self.invalidate().await;
                let url = self.ensure().await?;
                self.post(&url, body.as_bytes(), timeout).await
            }
            other => other,
        }
    }

    async fn wait_for_server(&self, child: &mut Child) -> Result<String, UpstreamError> {
        let start = Instant::now();
        loop {
            if let Some(state) = Singleton::read_live_state(&self.state_dir)
                && self.probe(&state.http).await
            {
                return Ok(state.http);
            }
            if let Some(status) = child.try_wait().map_err(UpstreamError::Spawn)? {
                if let Some(state) = Singleton::read_live_state(&self.state_dir)
                    && self.probe(&state.http).await
                {
                    return Ok(state.http);
                }
                if !status.success() {
                    return Err(UpstreamError::Exited(status));
                }
            }
            if start.elapsed() > SPAWN_TIMEOUT {
                return Err(UpstreamError::StartTimeout);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

fn spawn_server(state_dir: &Path) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    std::fs::create_dir_all(state_dir)?;
    let log_path = server_log_path(state_dir);
    if std::fs::metadata(&log_path).is_ok_and(|m| m.len() > LOG_MAX_BYTES) {
        std::fs::File::create(&log_path)?;
    }
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    writeln!(
        log,
        "--- brain-mcp proxy {} spawning server at {} ---",
        std::process::id(),
        chrono::Utc::now().to_rfc3339()
    )?;
    let exe = std::env::current_exe()?;
    unsafe {
        std::process::Command::new(exe)
            .arg("serve")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::from(log))
            .pre_exec(|| {
                libc::setsid();
                Ok(())
            })
            .spawn()
    }
}

fn log_tail(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(LOG_TAIL_BYTES)))
        .ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().collect();
    let tail = lines[lines.len().saturating_sub(LOG_TAIL_LINES)..].join("\n");
    if tail.trim().is_empty() {
        None
    } else {
        Some(tail)
    }
}

fn unavailable_message(err: &UpstreamError, state_dir: &Path) -> String {
    let mut msg = format!("brain-mcp server is unavailable: {err}.");
    let log_path = server_log_path(state_dir);
    if let Some(tail) = log_tail(&log_path) {
        msg.push_str(&format!(
            "\n\nLast lines of {}:\n{tail}",
            log_path.display()
        ));
    }
    msg.push_str(
        "\n\nFix the cause above, then retry the tool call. `brain-mcp status` shows the server state; `brain-mcp serve` runs it in the foreground.",
    );
    msg
}

enum Outcome {
    Reply(Vec<u8>),
    Silent,
}

fn encode(r: &Response) -> Vec<u8> {
    serde_json::to_vec(r).expect("JSON-RPC response serializes")
}

async fn handle_line(line: String, upstream: Arc<Upstream>) -> Outcome {
    let value: Value = match serde_json::from_str(&line) {
        Ok(v) => v,
        Err(e) => {
            return Outcome::Reply(encode(&Response::error(
                Some(Value::Null),
                PARSE_ERROR,
                format!("Parse error: {e}"),
            )));
        }
    };

    let id = value.get("id").cloned();
    let is_notification = value.as_object().is_some_and(|o| !o.contains_key("id"));

    let request: Request = match serde_json::from_value(value.clone()) {
        Ok(r) => r,
        Err(_) => {
            if is_notification {
                return Outcome::Silent;
            }
            return Outcome::Reply(encode(&Response::error(
                Some(id.unwrap_or(Value::Null)),
                INVALID_REQUEST,
                "Invalid request",
            )));
        }
    };

    if is_notification {
        return Outcome::Silent;
    }

    let id = Some(id.unwrap_or(Value::Null));

    match request.method.as_str() {
        "initialize" => {
            let up = upstream.clone();
            tokio::spawn(async move {
                let _ = up.ensure().await;
            });
            Outcome::Reply(encode(&Response::success(
                id,
                initialize_result(request.params.as_ref()),
            )))
        }
        "ping" => Outcome::Reply(encode(&Response::success(id, json!({})))),
        "tools/list" => Outcome::Reply(encode(&Response::success(
            id,
            json!({"tools": tool_definitions()}),
        ))),
        "tools/call" => {
            let timeout = if request
                .params
                .as_ref()
                .and_then(|p| p.get("name"))
                .and_then(Value::as_str)
                == Some("memory_reindex")
            {
                REINDEX_TIMEOUT
            } else {
                TOOL_TIMEOUT
            };
            match upstream.forward(line, timeout).await {
                Ok(body) => Outcome::Reply(body),
                Err(e) => Outcome::Reply(encode(&Response::success(
                    id,
                    tool_error(unavailable_message(&e, &upstream.state_dir)),
                ))),
            }
        }
        _ => Outcome::Reply(encode(&Response::error(
            id,
            METHOD_NOT_FOUND,
            "Method not found",
        ))),
    }
}

pub async fn run(state_dir: PathBuf) -> anyhow::Result<()> {
    let upstream = Arc::new(Upstream::new(state_dir, true));
    let (line_tx, mut line_rx) = tokio::sync::mpsc::channel::<String>(32);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) if !line.trim().is_empty() => {
                    if line_tx.blocking_send(line).is_err() {
                        break;
                    }
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    });
    let (out_tx, out_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let writer = std::thread::spawn(move || -> std::io::Result<()> {
        let mut stdout = std::io::stdout().lock();
        for frame in out_rx {
            stdout.write_all(&frame)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
        Ok(())
    });
    let mut tasks = tokio::task::JoinSet::new();
    while let Some(line) = line_rx.recv().await {
        let upstream = upstream.clone();
        let out_tx = out_tx.clone();
        tasks.spawn(async move {
            if let Outcome::Reply(frame) = handle_line(line, upstream).await {
                let _ = out_tx.send(frame);
            }
        });
        while tasks.try_join_next().is_some() {}
    }
    while tasks.join_next().await.is_some() {}
    drop(out_tx);
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("stdout writer panicked"))??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn value_of(outcome: Outcome) -> Value {
        match outcome {
            Outcome::Reply(bytes) => serde_json::from_slice(&bytes).unwrap(),
            Outcome::Silent => panic!("expected Reply, got Silent"),
        }
    }

    #[tokio::test]
    async fn initialize_answered_locally() {
        let dir = tempfile::tempdir().unwrap();
        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false));
        let start = Instant::now();
        let outcome = handle_line(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#
                .to_string(),
            upstream,
        )
        .await;
        assert!(start.elapsed() < Duration::from_millis(500));
        let resp = value_of(outcome);
        assert_eq!(resp["result"]["protocolVersion"], "2025-03-26");
    }

    #[tokio::test]
    async fn tools_list_answered_locally() {
        let dir = tempfile::tempdir().unwrap();
        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false));
        let outcome = handle_line(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#.to_string(),
            upstream,
        )
        .await;
        let resp = value_of(outcome);
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), tool_definitions().len());
    }

    #[tokio::test]
    async fn ping_answered_locally() {
        let dir = tempfile::tempdir().unwrap();
        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false));
        let outcome = handle_line(
            r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#.to_string(),
            upstream,
        )
        .await;
        let resp = value_of(outcome);
        assert_eq!(resp["result"], json!({}));
    }

    #[tokio::test]
    async fn notification_is_silent() {
        let dir = tempfile::tempdir().unwrap();
        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false));
        let outcome = handle_line(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_string(),
            upstream.clone(),
        )
        .await;
        assert!(matches!(outcome, Outcome::Silent));

        let outcome = handle_line(
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#
                .to_string(),
            upstream,
        )
        .await;
        assert!(matches!(outcome, Outcome::Silent));
    }

    #[tokio::test]
    async fn null_id_is_request() {
        let dir = tempfile::tempdir().unwrap();
        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false));
        let outcome = handle_line(
            r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#.to_string(),
            upstream,
        )
        .await;
        let resp = value_of(outcome);
        assert!(resp["id"].is_null());
    }

    #[tokio::test]
    async fn parse_error_reply() {
        let dir = tempfile::tempdir().unwrap();
        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false));
        let outcome = handle_line("not json".to_string(), upstream).await;
        let resp = value_of(outcome);
        assert_eq!(resp["error"]["code"], -32700);
    }

    #[tokio::test]
    async fn unknown_method_is_method_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false));
        let outcome = handle_line(
            r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#.to_string(),
            upstream,
        )
        .await;
        let resp = value_of(outcome);
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn tools_call_without_server_is_tool_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            server_log_path(dir.path()),
            "line1\nError: ONNX support not compiled in",
        )
        .unwrap();
        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false));
        let outcome = handle_line(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memory_search","arguments":{"query":"x"}}}"#
                .to_string(),
            upstream,
        )
        .await;
        let resp = value_of(outcome);
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("brain-mcp server is unavailable:"));
        assert!(text.contains("ONNX support not compiled in"));
        assert!(text.contains("server.log"));
    }

    #[test]
    fn log_tail_limits_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = server_log_path(dir.path());
        let content: String = (0..50).map(|i| format!("line{i}\n")).collect();
        std::fs::write(&path, content).unwrap();
        let tail = log_tail(&path).unwrap();
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(lines.len(), 10);
        assert_eq!(lines[0], "line40");
        assert_eq!(lines[9], "line49");
    }

    #[tokio::test]
    async fn forward_to_live_server() {
        let dir = tempfile::tempdir().unwrap();

        let vault = Arc::new(brain_core::mocks::MockVault::new());
        let embedder = Arc::new(brain_core::mocks::MockEmbedder::new(8));
        let index = Arc::new(brain_core::mocks::MockIndex::new());
        let service = Arc::new(brain_core::service::MemoryService::new(
            vault, embedder, index,
        ));
        let handler = Arc::new(brain_mcp_proto::handler::McpHandler::new(service));
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let port = brain_server::http::run_on_random_port(handler, shutdown_rx)
            .await
            .unwrap();

        let singleton = Singleton::acquire(dir.path()).unwrap();
        singleton
            .write_state(&brain_server::singleton::ServerState {
                pid: std::process::id(),
                http: format!("http://127.0.0.1:{port}/mcp"),
                started_at: chrono::Utc::now(),
            })
            .unwrap();

        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false));
        let outcome = handle_line(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memory_store","arguments":{"title":"Forwarded","content":"body","tags":[]}}}"#
                .to_string(),
            upstream,
        )
        .await;
        let resp = value_of(outcome);
        assert!(resp["error"].is_null());
        assert!(resp["result"]["isError"].is_null());
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let memory: Value = serde_json::from_str(text).unwrap();
        assert_eq!(memory["title"], "Forwarded");
    }
}
