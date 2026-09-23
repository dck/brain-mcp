use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

fn state_dir(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/brain-mcp/run")
    } else {
        home.join(".config/brain-mcp/run")
    }
}

#[test]
fn handshake_is_fast_without_config() {
    let tmp = tempfile::tempdir().unwrap();

    let warmup = std::process::Command::new(env!("CARGO_BIN_EXE_brain-mcp"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(warmup.status.success());

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_brain-mcp"))
        .args(["serve", "--stdio"])
        .env("HOME", tmp.path())
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("RUST_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let (line_tx, line_rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if line_tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let send = |stdin: &mut std::process::ChildStdin, line: &str| {
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    };

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}"#,
    );
    let line = line_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("initialize response within 1s");
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["result"]["serverInfo"]["name"], "brain-mcp");

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    );
    let line = line_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("tools/list response within 1s");
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
    let tools = resp["result"]["tools"].as_array().unwrap();
    assert_eq!(
        tools.len(),
        brain_mcp_proto::schema::tool_definitions().len()
    );

    send(&mut stdin, r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#);
    let line = line_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("ping response within 1s");
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["result"], serde_json::json!({}));

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"memory_search","arguments":{"query":"x"}}}"#,
    );
    let line = line_rx
        .recv_timeout(Duration::from_secs(12))
        .expect("tools/call response within 12s");
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["result"]["isError"], true);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Config not found"));

    drop(stdin);
    let status = child.wait_timeout_or_kill(Duration::from_secs(5));
    assert_eq!(status, Some(0));

    assert!(state_dir(tmp.path()).join("server.log").exists());
}

#[test]
fn legacy_state_file_without_lock_is_ignored() {
    let tmp = tempfile::tempdir().unwrap();

    let warmup = std::process::Command::new(env!("CARGO_BIN_EXE_brain-mcp"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(warmup.status.success());

    let dir = state_dir(tmp.path());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("brain-mcp.state"),
        "pid = 999999\nhttp = \"http://127.0.0.1:47200/mcp\"\nstarted_at = \"2020-01-01T00:00:00Z\"\n",
    )
    .unwrap();

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_brain-mcp"))
        .args(["serve", "--stdio"])
        .env("HOME", tmp.path())
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("RUST_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let (line_tx, line_rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if line_tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let send = |stdin: &mut std::process::ChildStdin, line: &str| {
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    };

    let start = std::time::Instant::now();
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}"#,
    );
    let line = line_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("initialize response within 1s");
    assert!(start.elapsed() < Duration::from_secs(1));
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["result"]["serverInfo"]["name"], "brain-mcp");

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"memory_search","arguments":{"query":"x"}}}"#,
    );
    let line = line_rx
        .recv_timeout(Duration::from_secs(12))
        .expect("tools/call response within 12s");
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["result"]["isError"], true);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Config not found"));

    drop(stdin);
    let status = child.wait_timeout_or_kill(Duration::from_secs(5));
    assert_eq!(status, Some(0));
}

trait WaitTimeoutOrKill {
    fn wait_timeout_or_kill(&mut self, timeout: Duration) -> Option<i32>;
}

impl WaitTimeoutOrKill for std::process::Child {
    fn wait_timeout_or_kill(&mut self, timeout: Duration) -> Option<i32> {
        let start = std::time::Instant::now();
        loop {
            if let Ok(Some(status)) = self.try_wait() {
                return status.code();
            }
            if start.elapsed() > timeout {
                let _ = self.kill();
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
