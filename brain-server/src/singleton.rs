use std::fs;
use std::io::Read;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerState {
    pub pid: u32,
    pub http: String,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub token: String,
}

impl ServerState {
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.http.trim_end_matches("/mcp"), path)
    }
}

#[derive(Debug)]
pub struct Singleton {
    state_path: PathBuf,
    _lock_file: fs::File,
}

#[derive(Debug, thiserror::Error)]
pub enum SingletonError {
    #[error("Server already running")]
    AlreadyRunning(ServerState),
    #[error("Another server is starting")]
    Starting,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
}

impl Singleton {
    /// Try to acquire the singleton lock in `state_dir`.
    ///
    /// Returns `AlreadyRunning` if another process already holds the lock.
    pub fn acquire(state_dir: &Path) -> Result<Self, SingletonError> {
        fs::create_dir_all(state_dir)?;
        fs::set_permissions(state_dir, fs::Permissions::from_mode(0o700))?;

        let state_path = state_dir.join("brain-mcp.state");
        let lock_file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&state_path)?;
        fs::set_permissions(&state_path, fs::Permissions::from_mode(0o600))?;

        match lock_file.try_lock_exclusive() {
            Ok(()) => Ok(Self {
                state_path,
                _lock_file: lock_file,
            }),
            Err(_) => {
                // Another process holds the lock — try to read state for the error.
                match Self::read_state(state_dir) {
                    Some(state) => Err(SingletonError::AlreadyRunning(state)),
                    None => Err(SingletonError::Starting),
                }
            }
        }
    }

    /// Write server state to the lock file.
    pub fn write_state(&self, state: &ServerState) -> Result<(), SingletonError> {
        let toml =
            toml::to_string(state).map_err(|e| SingletonError::Parse(format!("serialize: {e}")))?;
        fs::write(&self.state_path, toml)?;
        Ok(())
    }

    /// Read server state without acquiring the lock.
    pub fn read_state(state_dir: &Path) -> Option<ServerState> {
        let path = state_dir.join("brain-mcp.state");
        let mut file = fs::File::open(&path).ok()?;
        let mut buf = String::new();
        file.read_to_string(&mut buf).ok()?;
        toml::from_str(&buf).ok()
    }

    /// Read server state only if a live process holds the lock.
    ///
    /// Returns `None` for a state file left behind by a server that died
    /// without cleanup (crash, SIGKILL, reboot).
    pub fn read_live_state(state_dir: &Path) -> Option<ServerState> {
        let file = fs::File::open(state_dir.join("brain-mcp.state")).ok()?;
        if fs2::FileExt::try_lock_shared(&file).is_ok() {
            return None;
        }
        Self::read_state(state_dir)
    }
}

impl Drop for Singleton {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.state_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn test_singleton_acquire() {
        let dir = temp_dir();
        let singleton = Singleton::acquire(dir.path());
        assert!(singleton.is_ok());
    }

    #[test]
    fn test_singleton_already_running() {
        let dir = temp_dir();
        let _s1 = Singleton::acquire(dir.path()).unwrap();

        // Write state so the error can parse it.
        _s1.write_state(&ServerState {
            pid: std::process::id(),
            http: "http://127.0.0.1:4321".into(),
            started_at: Utc::now(),
            version: String::new(),
            token: String::new(),
        })
        .unwrap();

        let result = Singleton::acquire(dir.path());
        assert!(
            matches!(result, Err(SingletonError::AlreadyRunning(_))),
            "expected AlreadyRunning, got {result:?}"
        );
    }

    #[test]
    fn test_singleton_releases_on_drop() {
        let dir = temp_dir();
        {
            let _s1 = Singleton::acquire(dir.path()).unwrap();
            // _s1 dropped here
        }
        let s2 = Singleton::acquire(dir.path());
        assert!(s2.is_ok(), "expected Ok after drop, got {s2:?}");
    }

    #[test]
    fn test_read_live_state_while_held() {
        let dir = temp_dir();
        let s1 = Singleton::acquire(dir.path()).unwrap();
        s1.write_state(&ServerState {
            pid: std::process::id(),
            http: "http://127.0.0.1:4321".into(),
            started_at: Utc::now(),
            version: String::new(),
            token: String::new(),
        })
        .unwrap();

        let state = Singleton::read_live_state(dir.path()).expect("live state");
        assert_eq!(state.http, "http://127.0.0.1:4321");
    }

    #[test]
    fn test_read_live_state_ignores_stale_file() {
        let dir = temp_dir();
        let path = dir.path().join("brain-mcp.state");
        fs::write(
            &path,
            "pid = 21153\nhttp = \"http://127.0.0.1:47200/mcp\"\nstarted_at = \"2026-08-09T05:00:21Z\"\n",
        )
        .unwrap();

        assert!(Singleton::read_state(dir.path()).is_some());
        assert!(Singleton::read_live_state(dir.path()).is_none());
        assert!(Singleton::acquire(dir.path()).is_ok());
    }

    #[test]
    fn test_state_file_mode_0600() {
        let dir = temp_dir();
        let s1 = Singleton::acquire(dir.path()).unwrap();
        s1.write_state(&ServerState {
            pid: std::process::id(),
            http: "http://127.0.0.1:4321".into(),
            started_at: Utc::now(),
            version: String::new(),
            token: String::new(),
        })
        .unwrap();

        let file_mode = fs::metadata(dir.path().join("brain-mcp.state"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(file_mode & 0o777, 0o600);

        let dir_mode = fs::metadata(dir.path()).unwrap().permissions().mode();
        assert_eq!(dir_mode & 0o777, 0o700);
    }

    #[test]
    fn test_legacy_state_parses_without_token() {
        let dir = temp_dir();
        let path = dir.path().join("brain-mcp.state");
        fs::write(
            &path,
            "pid = 21153\nhttp = \"http://127.0.0.1:47200/mcp\"\nstarted_at = \"2026-08-09T05:00:21Z\"\n",
        )
        .unwrap();

        let state = Singleton::read_state(dir.path()).expect("legacy state parses");
        assert_eq!(state.token, "");
        assert_eq!(state.version, "");
    }

    #[test]
    fn test_locked_but_empty_is_starting() {
        let dir = temp_dir();
        let _s1 = Singleton::acquire(dir.path()).unwrap();

        let result = Singleton::acquire(dir.path());
        assert!(
            matches!(result, Err(SingletonError::Starting)),
            "expected Starting, got {result:?}"
        );
    }

    #[test]
    fn test_state_url() {
        let state = ServerState {
            pid: std::process::id(),
            http: "http://127.0.0.1:5/mcp".into(),
            started_at: Utc::now(),
            version: String::new(),
            token: String::new(),
        };
        assert_eq!(state.url("/health"), "http://127.0.0.1:5/health");
    }
}
