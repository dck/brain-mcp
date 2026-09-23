use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use brain_server::lifecycle::SESSION_HEADER;
use brain_server::singleton::{ServerState, Singleton};

fn state_dir(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/brain-mcp/run")
    } else {
        home.join(".config/brain-mcp/run")
    }
}

fn config_dir(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/brain-mcp")
    } else {
        home.join(".config/brain-mcp")
    }
}

fn write_config(home: &Path, grace_period_seconds: u64) {
    let dir = config_dir(home);
    std::fs::create_dir_all(&dir).unwrap();
    let toml = format!(
        r#"[vault]
path = "{}"

[embedding]
provider = "openai"
model = "text-embedding-3-small"
api_key_env = "BRAIN_MCP_TEST_KEY"

[index]
path = "{}"

[server]
http_port = 0
grace_period_seconds = {}
"#,
        home.join("vault").display(),
        home.join("index.db").display(),
        grace_period_seconds,
    );
    std::fs::write(dir.join("config.toml"), toml).unwrap();
}

fn ensure_binary_scanned() {
    let warmup = std::process::Command::new(env!("CARGO_BIN_EXE_brain-mcp"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(warmup.status.success());
}

fn spawn_server(home: &Path) -> std::process::Child {
    std::process::Command::new(env!("CARGO_BIN_EXE_brain-mcp"))
        .arg("serve")
        .env("HOME", home)
        .env("BRAIN_MCP_TEST_KEY", "dummy")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("RUST_LOG")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn wait_for_live_state(dir: &Path, timeout: Duration) -> Option<ServerState> {
    let start = Instant::now();
    loop {
        if let Some(state) = Singleton::read_live_state(dir) {
            return Some(state);
        }
        if start.elapsed() > timeout {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_child(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Option<std::process::ExitStatus> {
    let start = Instant::now();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn sigterm_removes_state_file() {
    ensure_binary_scanned();
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), 60);
    let mut child = spawn_server(tmp.path());
    let dir = state_dir(tmp.path());

    let state = wait_for_live_state(&dir, Duration::from_secs(10)).expect("server started");
    assert_eq!(state.pid, child.id());

    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };

    let status = wait_child(&mut child, Duration::from_secs(5)).expect("server exited");
    assert!(status.success());
    assert!(Singleton::read_live_state(&dir).is_none());
    assert!(!dir.join("brain-mcp.state").exists());
}

#[test]
fn stop_waits_for_exit() {
    ensure_binary_scanned();
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), 60);
    let mut child = spawn_server(tmp.path());
    let dir = state_dir(tmp.path());
    wait_for_live_state(&dir, Duration::from_secs(10)).expect("server started");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_brain-mcp"))
        .arg("stop")
        .env("HOME", tmp.path())
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Server stopped"), "{stdout}");
    assert!(Singleton::read_live_state(&dir).is_none());

    wait_child(&mut child, Duration::from_secs(5));
}

#[tokio::test]
async fn idle_shutdown_after_last_session_closes() {
    ensure_binary_scanned();
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), 1);
    let mut child = spawn_server(tmp.path());
    let dir = state_dir(tmp.path());
    let state = wait_for_live_state(&dir, Duration::from_secs(10)).expect("server started");

    let client = reqwest::Client::new();
    let resp = client
        .post(state.url("/session/heartbeat"))
        .bearer_auth(&state.token)
        .header(SESSION_HEADER, "t1")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    let resp = client
        .post(state.url("/session/close"))
        .bearer_auth(&state.token)
        .header(SESSION_HEADER, "t1")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    let status = wait_child(&mut child, Duration::from_secs(5)).expect("server exited on its own");
    assert!(status.success());
    assert!(Singleton::read_live_state(&dir).is_none());
}
