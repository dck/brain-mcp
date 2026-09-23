use std::path::Path;
use std::time::Duration;

use brain_mcp_proto::mcp::SERVER_NAME;
use brain_server::identity::ServerIdentity;
use brain_server::singleton::{ServerState, Singleton};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum IdentityError {
    Unreachable,
    Foreign,
}

pub fn loopback_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .expect("reqwest client without proxy never fails to build")
}

pub async fn fetch_identity(
    client: &reqwest::Client,
    state: &ServerState,
    timeout: Duration,
) -> Result<ServerIdentity, IdentityError> {
    let resp = client
        .get(state.url("/health"))
        .timeout(timeout)
        .send()
        .await
        .map_err(|_| IdentityError::Unreachable)?;
    if resp.status() != reqwest::StatusCode::OK {
        return Err(IdentityError::Foreign);
    }
    let identity: ServerIdentity = resp.json().await.map_err(|_| IdentityError::Foreign)?;
    if identity.name != SERVER_NAME || identity.pid != state.pid {
        return Err(IdentityError::Foreign);
    }
    Ok(identity)
}

pub async fn request_shutdown(client: &reqwest::Client, state: &ServerState) -> bool {
    client
        .post(state.url("/shutdown"))
        .bearer_auth(&state.token)
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .is_ok_and(|resp| resp.status() == reqwest::StatusCode::ACCEPTED)
}

pub async fn wait_released(state_dir: &Path, pid: u32, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        match Singleton::read_live_state(state_dir) {
            Some(state) if state.pid == pid => {}
            _ => return true,
        }
        if start.elapsed() > timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
