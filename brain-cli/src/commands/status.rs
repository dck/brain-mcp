use std::time::Duration;

use chrono::Utc;
use serde_json::json;

use brain_server::singleton::Singleton;

use super::server_client::{fetch_health, loopback_client};
use super::state_dir;
use crate::output;

pub async fn run(json: bool) -> anyhow::Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let state = Singleton::read_live_state(&state_dir());
    let log_path = super::proxy::server_log_path(&state_dir());

    let health = match &state {
        Some(s) => fetch_health(&loopback_client(), s, Duration::from_secs(1)).await,
        None => None,
    };
    let server_version = health
        .as_ref()
        .and_then(|h| h.get("version"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let sessions = health
        .as_ref()
        .and_then(|h| h.get("sessions"))
        .and_then(|v| v.as_u64());

    if json {
        let value = match &state {
            Some(s) => json!({
                "version": version,
                "status": "running",
                "pid": s.pid,
                "url": s.http,
                "started_at": s.started_at.to_rfc3339(),
                "server_version": server_version,
                "reachable": health.is_some(),
                "sessions": sessions,
                "log": log_path.to_string_lossy(),
            }),
            None => json!({
                "version": version,
                "status": "stopped",
                "sessions": sessions,
                "log": log_path.to_string_lossy(),
            }),
        };
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    match &state {
        Some(s) => {
            let uptime = Utc::now().signed_duration_since(s.started_at);
            let uptime_str = format_duration(uptime);

            println!(
                "  brain-mcp v{}  {}",
                version,
                console::style("● running").green()
            );
            println!();
            println!("{}", output::info_line("PID", &s.pid.to_string()));
            println!("{}", output::info_line("Uptime", &uptime_str));
            println!("{}", output::info_line("URL", &s.http));
            println!(
                "{}",
                output::info_line("Started", &s.started_at.to_rfc3339())
            );
            let server_line = match &server_version {
                Some(v) if v != version => {
                    format!("v{v} (CLI is v{version}; the next session restarts it)")
                }
                Some(v) => format!("v{v}"),
                None => "unreachable".to_string(),
            };
            println!("{}", output::info_line("Server", &server_line));
            let sessions_line = sessions
                .map(|n| n.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            println!("{}", output::info_line("Sessions", &sessions_line));
            println!("{}", output::info_line("Log", &log_path.to_string_lossy()));
        }
        None => {
            println!(
                "  brain-mcp v{}  {}",
                version,
                console::style("○ stopped").dim()
            );
            println!("{}", output::info_line("Log", &log_path.to_string_lossy()));
        }
    }

    Ok(())
}

fn format_duration(d: chrono::Duration) -> String {
    let total_secs = d.num_seconds();
    if total_secs < 0 {
        return "0s".into();
    }
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        format!("{mins}m")
    } else {
        format!("{total_secs}s")
    }
}
