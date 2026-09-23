use std::time::Duration;

use brain_server::singleton::Singleton;

use super::server_client::{loopback_client, request_shutdown, wait_released};
use super::state_dir;
use crate::output;

pub async fn run() -> anyhow::Result<()> {
    let state = Singleton::read_live_state(&state_dir());

    match state {
        Some(s) => {
            let requested = !s.token.is_empty() && request_shutdown(&loopback_client(), &s).await;

            if !requested {
                // Send SIGTERM to the server process.
                let ret = unsafe { libc::kill(s.pid as libc::pid_t, libc::SIGTERM) };
                if ret != 0 {
                    let err = std::io::Error::last_os_error();
                    eprintln!(
                        "{}",
                        output::error(&format!("Failed to stop server (PID {}): {err}", s.pid))
                    );
                    return Ok(());
                }
            }

            if wait_released(&state_dir(), s.pid, Duration::from_secs(10)).await {
                println!("{}", output::success("Server stopped"));
            } else {
                eprintln!(
                    "{}",
                    output::error(&format!(
                        "Server (PID {}) did not exit within 10s. Force it with: kill -9 {}",
                        s.pid, s.pid
                    ))
                );
                std::process::exit(1);
            }
        }
        None => {
            eprintln!("{}", output::error("No running server found"));
        }
    }

    Ok(())
}
