pub mod init;
pub mod reindex;
pub mod serve;
pub mod status;
pub mod stop;

use std::path::PathBuf;

use brain_core::config::Config;

/// Default config directory: `~/.config/brain-mcp/`
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("brain-mcp")
}

/// Default config file path.
pub fn default_config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Default state directory for singleton lock / state file.
pub fn state_dir() -> PathBuf {
    config_dir().join("run")
}

/// Load config from an explicit path or the default location.
pub fn load_config(path: Option<PathBuf>) -> anyhow::Result<Config> {
    let path = path.unwrap_or_else(default_config_path);
    if !path.exists() {
        anyhow::bail!(
            "Config not found at {}. Run 'brain-mcp init' first.",
            path.display()
        );
    }
    let raw = std::fs::read_to_string(&path)?;
    let config: Config = toml::from_str(&raw)?;
    Ok(config.resolve_paths())
}
