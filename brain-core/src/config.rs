use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub vault: VaultConfig,
    pub embedding: EmbeddingConfig,
    pub index: IndexConfig,
    pub server: ServerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultConfig {
    pub path: String,
    #[serde(default = "default_templates_dir")]
    pub templates_dir: String,
    #[serde(default = "default_categories")]
    pub categories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub model_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexConfig {
    #[serde(default = "default_backend")]
    pub backend: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_http_port")]
    pub http_port: u16,
    #[serde(default = "default_grace_period")]
    pub grace_period_seconds: u64,
}

impl Config {
    /// Expand `~` prefixes to the user's home directory in all path fields.
    pub fn resolve_paths(mut self) -> Self {
        self.vault.path = expand_tilde(&self.vault.path);
        self.index.path = expand_tilde(&self.index.path);
        if let Some(ref p) = self.embedding.model_path {
            self.embedding.model_path = Some(expand_tilde(p));
        }
        self
    }
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest).to_string_lossy().into_owned();
    }
    path.to_string()
}

fn default_templates_dir() -> String {
    "_templates".into()
}

fn default_categories() -> Vec<String> {
    vec![
        "procedures",
        "decisions",
        "learnings",
        "concepts",
        "projects",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

fn default_backend() -> String {
    "sqlite-vec".into()
}

fn default_http_port() -> u16 {
    47200
}

fn default_grace_period() -> u64 {
    60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_default_toml() {
        let raw = include_str!("../../config/default.toml");
        let config: Config = toml::from_str(raw).expect("default.toml should deserialize");

        assert_eq!(config.vault.path, "~/brain");
        assert_eq!(config.vault.templates_dir, "_templates");
        assert_eq!(config.vault.categories.len(), 5);
        assert_eq!(config.embedding.provider, "openai");
        assert_eq!(config.embedding.model, "text-embedding-3-small");
        assert_eq!(
            config.embedding.api_key_env.as_deref(),
            Some("OPENAI_API_KEY")
        );
        assert_eq!(config.index.backend, "sqlite-vec");
        assert_eq!(config.index.path, "~/.config/brain-mcp/index.db");
        assert_eq!(config.server.http_port, 47200);
        assert_eq!(config.server.grace_period_seconds, 60);
    }

    #[test]
    fn resolve_paths_expands_tilde() {
        let raw = include_str!("../../config/default.toml");
        let config: Config = toml::from_str(raw).unwrap();
        let resolved = config.resolve_paths();

        assert!(!resolved.vault.path.starts_with('~'));
        assert!(!resolved.index.path.starts_with('~'));
    }
}
