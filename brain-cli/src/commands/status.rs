use chrono::Utc;
use serde_json::json;

use brain_server::singleton::Singleton;

use super::state_dir;
use crate::output;

pub async fn run(json: bool) -> anyhow::Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let state = Singleton::read_state(&state_dir());

    if json {
        let value = match &state {
            Some(s) => json!({
                "version": version,
                "status": "running",
                "pid": s.pid,
                "url": s.http,
                "started_at": s.started_at.to_rfc3339(),
            }),
            None => json!({
                "version": version,
                "status": "stopped",
            }),
        };
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    match state {
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
        }
        None => {
            println!(
                "  brain-mcp v{}  {}",
                version,
                console::style("○ stopped").dim()
            );
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
