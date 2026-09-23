use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use brain_mcp_proto::jsonrpc::{INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR, Request, Response};
use brain_mcp_proto::mcp::{SERVER_VERSION, initialize_result, tool_error};
use brain_mcp_proto::schema::tool_definitions;
use brain_server::auth::generate_token;
use brain_server::identity::{ServerIdentity, current_exe_path, parse_version};
use brain_server::lifecycle::{HEARTBEAT_INTERVAL, SESSION_HEADER};
use brain_server::singleton::{ServerState, Singleton};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use super::server_client::{self, IdentityError};

const SPAWN_TIMEOUT: Duration = Duration::from_secs(10);
const IDENTIFY_TIMEOUT: Duration = Duration::from_secs(2);
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
    Hung {
        timeout: Duration,
    },
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
            UpstreamError::Hung { timeout } => write!(
                f,
                "request timed out after {}s; the server was unresponsive and has been stopped; retry the call to start a fresh one",
                timeout.as_secs()
            ),
        }
    }
}

struct Upstream {
    state_dir: PathBuf,
    config_path: Option<PathBuf>,
    client: reqwest::Client,
    conn: tokio::sync::Mutex<Option<ServerState>>,
    spawn_enabled: bool,
    consecutive_timeouts: AtomicU32,
    session_id: String,
    heartbeat_started: AtomicBool,
}

impl Upstream {
    fn new(state_dir: PathBuf, spawn_enabled: bool, config_path: Option<PathBuf>) -> Self {
        Self {
            state_dir,
            config_path,
            client: server_client::loopback_client(),
            conn: tokio::sync::Mutex::new(None),
            spawn_enabled,
            consecutive_timeouts: AtomicU32::new(0),
            session_id: format!("{}-{}", std::process::id(), &generate_token()[..8]),
            heartbeat_started: AtomicBool::new(false),
        }
    }

    fn start_heartbeat(self: &Arc<Self>) {
        if self.heartbeat_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let up = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(HEARTBEAT_INTERVAL).await;
                let Some(state) = up.conn.lock().await.clone() else {
                    continue;
                };
                let ok = up
                    .client
                    .post(state.url("/session/heartbeat"))
                    .bearer_auth(&state.token)
                    .header(SESSION_HEADER, &up.session_id)
                    .timeout(Duration::from_secs(2))
                    .send()
                    .await
                    .is_ok_and(|r| r.status().is_success());
                if !ok {
                    up.invalidate().await;
                }
            }
        });
    }

    async fn close_session(&self) {
        let state = self.conn.lock().await.clone();
        if let Some(state) = state {
            let _ = self
                .client
                .post(state.url("/session/close"))
                .bearer_auth(&state.token)
                .header(SESSION_HEADER, &self.session_id)
                .timeout(Duration::from_millis(500))
                .send()
                .await;
        }
    }

    async fn ensure(&self) -> Result<ServerState, UpstreamError> {
        let mut guard = self.conn.lock().await;
        if let Some(state) = guard.as_ref() {
            return Ok(state.clone());
        }

        if let Some(state) = Singleton::read_live_state(&self.state_dir) {
            if state.token.is_empty() {
                self.kill_server(state.pid).await;
            } else {
                match self.identify(&state).await {
                    Ok(id) => {
                        let own = OwnBuild::current();
                        if needs_restart(&own, &state, &id) {
                            self.retire(&state).await;
                        } else {
                            *guard = Some(state.clone());
                            return Ok(state);
                        }
                    }
                    Err(_) => self.kill_server(state.pid).await,
                }
            }
        }

        if !self.spawn_enabled {
            return Err(UpstreamError::StartTimeout);
        }

        let mut child = spawn_server(&self.state_dir, self.config_path.as_deref())
            .map_err(UpstreamError::Spawn)?;
        let state = self.wait_for_server(&mut child).await?;
        *guard = Some(state.clone());
        Ok(state)
    }

    async fn identify(&self, state: &ServerState) -> Result<ServerIdentity, IdentityError> {
        match server_client::fetch_identity(&self.client, state, IDENTIFY_TIMEOUT).await {
            Err(IdentityError::Unreachable) => {
                tokio::time::sleep(Duration::from_millis(500)).await;
                server_client::fetch_identity(&self.client, state, IDENTIFY_TIMEOUT).await
            }
            other => other,
        }
    }

    async fn retire(&self, state: &ServerState) {
        if !self.spawn_enabled {
            eprintln!(
                "brain-mcp proxy: not restarting server (PID {}); spawning disabled",
                state.pid
            );
            return;
        }
        server_client::request_shutdown(&self.client, state).await;
        if server_client::wait_released(&self.state_dir, state.pid, Duration::from_secs(5)).await {
            return;
        }
        self.kill_server(state.pid).await;
    }

    async fn kill_server(&self, pid: u32) {
        if !self.spawn_enabled {
            eprintln!("brain-mcp proxy: not stopping server (PID {pid}); spawning disabled");
            return;
        }
        if pid <= 1 || pid == std::process::id() {
            return;
        }
        eprintln!("brain-mcp proxy: stopping unresponsive server (PID {pid})");
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if server_client::wait_released(&self.state_dir, pid, Duration::from_secs(3)).await {
            return;
        }
        eprintln!("brain-mcp proxy: stopping unresponsive server (PID {pid}) ... sending SIGKILL");
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        server_client::wait_released(&self.state_dir, pid, Duration::from_secs(2)).await;
    }

    async fn invalidate(&self) {
        let mut guard = self.conn.lock().await;
        *guard = None;
    }

    async fn post(
        &self,
        state: &ServerState,
        body: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, UpstreamError> {
        let resp = self
            .client
            .post(&state.http)
            .header("Content-Type", "application/json")
            .header(SESSION_HEADER, &self.session_id)
            .bearer_auth(&state.token)
            .body(body.to_vec())
            .timeout(timeout)
            .send()
            .await;

        let resp = match resp {
            Ok(resp) => {
                self.consecutive_timeouts.store(0, Ordering::SeqCst);
                resp
            }
            Err(error) if error.is_timeout() => {
                let n = self.consecutive_timeouts.fetch_add(1, Ordering::SeqCst) + 1;
                if n >= 2 {
                    self.kill_server(state.pid).await;
                    self.invalidate().await;
                    self.consecutive_timeouts.store(0, Ordering::SeqCst);
                    return Err(UpstreamError::Hung { timeout });
                }
                return Err(UpstreamError::Request { error, timeout });
            }
            Err(error) => return Err(UpstreamError::Request { error, timeout }),
        };

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
        let state = self.ensure().await?;
        match self.post(&state, body.as_bytes(), timeout).await {
            Err(UpstreamError::Request { error, .. }) if error.is_connect() => {
                self.invalidate().await;
                let state = self.ensure().await?;
                self.post(&state, body.as_bytes(), timeout).await
            }
            Err(UpstreamError::Status(401, _)) => {
                self.invalidate().await;
                let state = self.ensure().await?;
                self.post(&state, body.as_bytes(), timeout).await
            }
            other => other,
        }
    }

    async fn wait_for_server(&self, child: &mut Child) -> Result<ServerState, UpstreamError> {
        let start = Instant::now();
        loop {
            if let Some(state) = Singleton::read_live_state(&self.state_dir)
                && self.identify(&state).await.is_ok()
            {
                return Ok(state);
            }
            if let Some(status) = child.try_wait().map_err(UpstreamError::Spawn)? {
                if let Some(state) = Singleton::read_live_state(&self.state_dir)
                    && self.identify(&state).await.is_ok()
                {
                    return Ok(state);
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

struct OwnBuild {
    exe: Option<PathBuf>,
    mtime: Option<DateTime<Utc>>,
    version: &'static str,
}

impl OwnBuild {
    fn current() -> Self {
        let exe = current_exe_path();
        let mtime = exe
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok())
            .map(DateTime::<Utc>::from);
        Self {
            exe,
            mtime,
            version: SERVER_VERSION,
        }
    }
}

fn needs_restart(own: &OwnBuild, state: &ServerState, id: &ServerIdentity) -> bool {
    if state.token.is_empty() {
        return true;
    }
    let same_exe = own
        .exe
        .as_ref()
        .is_some_and(|e| e.to_string_lossy() == id.exe);
    if same_exe && own.mtime.is_some_and(|m| m > id.started_at) {
        return true;
    }
    matches!(
        (parse_version(&id.version), parse_version(own.version)),
        (Some(theirs), Some(ours)) if theirs < ours
    )
}

fn server_args(config_path: Option<&Path>) -> Vec<std::ffi::OsString> {
    let mut args = vec![std::ffi::OsString::from("serve")];
    if let Some(path) = config_path {
        args.push(std::ffi::OsString::from("--config"));
        args.push(path.into());
    }
    args
}

fn spawn_server(
    state_dir: &Path,
    config_path: Option<&Path>,
) -> std::io::Result<std::process::Child> {
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
    let exe = current_exe_path()
        .ok_or_else(|| std::io::Error::other("cannot resolve current executable"))?;
    unsafe {
        std::process::Command::new(exe)
            .args(server_args(config_path))
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
                if up.ensure().await.is_ok() {
                    up.start_heartbeat();
                }
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
                Ok(body) => {
                    upstream.start_heartbeat();
                    Outcome::Reply(body)
                }
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

pub async fn run(state_dir: PathBuf, config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let upstream = Arc::new(Upstream::new(state_dir, true, config_path));
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
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        tokio::select! {
            line = line_rx.recv() => match line {
                Some(line) => {
                    let upstream = upstream.clone();
                    let out_tx = out_tx.clone();
                    tasks.spawn(async move {
                        if let Outcome::Reply(frame) = handle_line(line, upstream).await {
                            let _ = out_tx.send(frame);
                        }
                    });
                    while tasks.try_join_next().is_some() {}
                }
                None => break,
            },
            _ = tokio::signal::ctrl_c() => {
                upstream.close_session().await;
                return Ok(());
            }
            _ = term.recv() => {
                upstream.close_session().await;
                return Ok(());
            }
        }
    }
    while tasks.join_next().await.is_some() {}
    upstream.close_session().await;
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
        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false, None));
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
        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false, None));
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
        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false, None));
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
        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false, None));
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
        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false, None));
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
        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false, None));
        let outcome = handle_line("not json".to_string(), upstream).await;
        let resp = value_of(outcome);
        assert_eq!(resp["error"]["code"], -32700);
    }

    #[tokio::test]
    async fn unknown_method_is_method_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false, None));
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
        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false, None));
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
    fn spawn_command_includes_config() {
        assert_eq!(server_args(None), vec![std::ffi::OsString::from("serve")]);
        assert_eq!(
            server_args(Some(Path::new("/a/b.toml"))),
            vec![
                std::ffi::OsString::from("serve"),
                std::ffi::OsString::from("--config"),
                std::ffi::OsString::from("/a/b.toml"),
            ]
        );
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

    async fn start_test_server(token: String) -> (u16, tokio::sync::watch::Sender<bool>) {
        let vault = Arc::new(brain_core::mocks::MockVault::new());
        let embedder = Arc::new(brain_core::mocks::MockEmbedder::new(8));
        let index = Arc::new(brain_core::mocks::MockIndex::new());
        let service = Arc::new(brain_core::service::MemoryService::new(
            vault, embedder, index,
        ));
        let handler = Arc::new(brain_mcp_proto::handler::McpHandler::new(service));
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
        let port = brain_server::http::run_on_random_port(handler, token, shutdown_tx.clone())
            .await
            .unwrap();
        (port, shutdown_tx)
    }

    async fn start_test_server_with_sessions(
        token: String,
    ) -> (u16, Arc<brain_server::lifecycle::SessionTracker>) {
        let vault = Arc::new(brain_core::mocks::MockVault::new());
        let embedder = Arc::new(brain_core::mocks::MockEmbedder::new(8));
        let index = Arc::new(brain_core::mocks::MockIndex::new());
        let service = Arc::new(brain_core::service::MemoryService::new(
            vault, embedder, index,
        ));
        let handler = Arc::new(brain_mcp_proto::handler::McpHandler::new(service));
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
        let sessions = brain_server::lifecycle::SessionTracker::new(
            brain_server::lifecycle::LEASE,
            Duration::ZERO,
            shutdown_tx.clone(),
        );
        let server = brain_server::http::HttpServer::new(
            handler,
            ServerIdentity::current(chrono::Utc::now()),
            token,
            shutdown_tx,
            sessions.clone(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = server.serve(listener).await;
        });
        (port, sessions)
    }

    #[tokio::test]
    async fn forward_touches_session() {
        let dir = tempfile::tempdir().unwrap();
        let token = brain_server::auth::generate_token();
        let (port, sessions) = start_test_server_with_sessions(token.clone()).await;

        let singleton = Singleton::acquire(dir.path()).unwrap();
        singleton
            .write_state(&ServerState {
                pid: std::process::id(),
                http: format!("http://127.0.0.1:{port}/mcp"),
                started_at: chrono::Utc::now(),
                version: SERVER_VERSION.to_string(),
                token: token.clone(),
            })
            .unwrap();

        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false, None));
        upstream
            .forward(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memory_list","arguments":{}}}"#
                    .to_string(),
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        assert_eq!(sessions.active(), 1);

        upstream.close_session().await;
        assert_eq!(sessions.active(), 0);
    }

    #[tokio::test]
    async fn forward_to_live_server() {
        let dir = tempfile::tempdir().unwrap();
        let token = brain_server::auth::generate_token();
        let (port, _shutdown_tx) = start_test_server(token.clone()).await;

        let singleton = Singleton::acquire(dir.path()).unwrap();
        singleton
            .write_state(&ServerState {
                pid: std::process::id(),
                http: format!("http://127.0.0.1:{port}/mcp"),
                started_at: chrono::Utc::now(),
                version: SERVER_VERSION.to_string(),
                token: token.clone(),
            })
            .unwrap();

        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false, None));
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

    #[tokio::test]
    async fn stale_token_is_refreshed() {
        let dir = tempfile::tempdir().unwrap();
        let token_a = brain_server::auth::generate_token();
        let (port, _shutdown_tx) = start_test_server(token_a.clone()).await;

        let singleton = Singleton::acquire(dir.path()).unwrap();
        let token_b = brain_server::auth::generate_token();
        singleton
            .write_state(&ServerState {
                pid: std::process::id(),
                http: format!("http://127.0.0.1:{port}/mcp"),
                started_at: chrono::Utc::now(),
                version: SERVER_VERSION.to_string(),
                token: token_b,
            })
            .unwrap();

        let upstream = Arc::new(Upstream::new(dir.path().to_path_buf(), false, None));
        upstream
            .ensure()
            .await
            .expect("ensure succeeds via /health");

        singleton
            .write_state(&ServerState {
                pid: std::process::id(),
                http: format!("http://127.0.0.1:{port}/mcp"),
                started_at: chrono::Utc::now(),
                version: SERVER_VERSION.to_string(),
                token: token_a,
            })
            .unwrap();

        let outcome = handle_line(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memory_list","arguments":{}}}"#
                .to_string(),
            upstream,
        )
        .await;
        let resp = value_of(outcome);
        assert!(resp["error"].is_null());
        assert!(resp["result"]["isError"].is_null());
    }

    #[test]
    fn needs_restart_matrix() {
        let mut state = ServerState {
            pid: 100,
            http: "http://127.0.0.1:1/mcp".into(),
            started_at: Utc::now(),
            version: "0.1.0".into(),
            token: "t".into(),
        };
        let mut identity = ServerIdentity {
            name: "brain-mcp".into(),
            version: "0.1.0".into(),
            pid: 100,
            started_at: Utc::now(),
            exe: "/other/brain-mcp".into(),
        };
        let own = OwnBuild {
            exe: Some(PathBuf::from("/own/brain-mcp")),
            mtime: Some(Utc::now()),
            version: "0.1.0",
        };

        state.token = String::new();
        assert!(needs_restart(&own, &state, &identity));
        state.token = "t".into();

        identity.exe = "/own/brain-mcp".into();
        identity.started_at = Utc::now() - chrono::Duration::seconds(10);
        assert!(needs_restart(&own, &state, &identity));

        identity.started_at = Utc::now() + chrono::Duration::seconds(10);
        assert!(!needs_restart(&own, &state, &identity));

        identity.exe = "/other/brain-mcp".into();
        identity.version = "0.0.9".into();
        assert!(needs_restart(&own, &state, &identity));

        identity.version = "0.2.0".into();
        assert!(!needs_restart(&own, &state, &identity));

        identity.version = "0.1.0".into();
        assert!(!needs_restart(&own, &state, &identity));
    }
}
