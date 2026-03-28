use brain_server::singleton::Singleton;

use super::state_dir;
use crate::output;

pub async fn run() -> anyhow::Result<()> {
    let state = Singleton::read_state(&state_dir());

    match state {
        Some(s) => {
            // Send SIGTERM to the server process.
            let ret = unsafe { libc::kill(s.pid as libc::pid_t, libc::SIGTERM) };
            if ret == 0 {
                println!("{}", output::success("Server stopped"));
            } else {
                let err = std::io::Error::last_os_error();
                eprintln!(
                    "{}",
                    output::error(&format!("Failed to stop server (PID {}): {err}", s.pid))
                );
            }
        }
        None => {
            eprintln!("{}", output::error("No running server found"));
        }
    }

    Ok(())
}
