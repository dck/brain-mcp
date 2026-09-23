use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerIdentity {
    pub name: String,
    pub version: String,
    pub pid: u32,
    pub started_at: DateTime<Utc>,
    pub exe: String,
}

impl ServerIdentity {
    pub fn current(started_at: DateTime<Utc>) -> Self {
        Self {
            name: brain_mcp_proto::mcp::SERVER_NAME.to_string(),
            version: brain_mcp_proto::mcp::SERVER_VERSION.to_string(),
            pid: std::process::id(),
            started_at,
            exe: current_exe_path()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    }
}

pub fn current_exe_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = PathBuf::from(strip_deleted(&exe.to_string_lossy()));
    Some(exe.canonicalize().unwrap_or(exe))
}

pub fn strip_deleted(path: &str) -> &str {
    path.strip_suffix(" (deleted)").unwrap_or(path)
}

pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next()??, it.next()??))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_cases() {
        assert_eq!(parse_version("0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_version("1.2.3-rc.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("x"), None);
        assert_eq!(parse_version("1.2"), None);
    }

    #[test]
    fn strip_deleted_suffix() {
        assert_eq!(strip_deleted("/a/b (deleted)"), "/a/b");
        assert_eq!(strip_deleted("/a/b"), "/a/b");
    }

    #[test]
    fn current_identity_fields() {
        let identity = ServerIdentity::current(Utc::now());
        assert_eq!(identity.name, "brain-mcp");
        assert_eq!(identity.pid, std::process::id());
        assert!(!identity.exe.is_empty());
    }
}
