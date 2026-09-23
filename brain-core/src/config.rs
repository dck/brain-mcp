use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub vault: VaultConfig,
    pub embedding: EmbeddingConfig,
    pub index: IndexConfig,
    pub server: ServerConfig,
    #[serde(default)]
    pub search: SearchConfig,
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

pub const SUPPORTED_INDEX_BACKENDS: &[&str] = &["sqlite", "sqlite-vec"];

impl IndexConfig {
    pub fn validate_backend(&self) -> Result<(), String> {
        if SUPPORTED_INDEX_BACKENDS.contains(&self.backend.as_str()) {
            Ok(())
        } else {
            Err(format!(
                "unsupported index backend \"{}\" in config; supported: \"sqlite\"",
                self.backend
            ))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    #[serde(default = "default_min_score")]
    pub min_score: f32,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            min_score: default_min_score(),
        }
    }
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

pub fn default_categories() -> Vec<String> {
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
    "sqlite".into()
}

fn default_min_score() -> f32 {
    0.25
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
        assert_eq!(config.index.backend, "sqlite");
        assert_eq!(config.index.path, "~/.config/brain-mcp/index.db");
        assert_eq!(config.server.http_port, 47200);
        assert_eq!(config.server.grace_period_seconds, 60);
        assert_eq!(config.search.min_score, 0.25);
    }

    #[test]
    fn resolve_paths_expands_tilde() {
        let raw = include_str!("../../config/default.toml");
        let config: Config = toml::from_str(raw).unwrap();
        let resolved = config.resolve_paths();

        assert!(!resolved.vault.path.starts_with('~'));
        assert!(!resolved.index.path.starts_with('~'));
    }

    #[test]
    fn legacy_backend_label_is_accepted() {
        let index = IndexConfig {
            backend: "sqlite-vec".into(),
            path: "x".into(),
        };
        assert!(index.validate_backend().is_ok());
    }

    #[test]
    fn unknown_backend_is_rejected() {
        let index = IndexConfig {
            backend: "libsql".into(),
            path: "x".into(),
        };
        let err = index.validate_backend().unwrap_err();
        assert!(err.contains("unsupported index backend \"libsql\""));
    }

    #[test]
    fn missing_backend_defaults_to_sqlite() {
        let raw = r#"
[vault]
path = "~/brain"
templates_dir = "_templates"
categories = ["procedures", "decisions", "learnings", "concepts", "projects"]

[embedding]
provider = "openai"
model = "text-embedding-3-small"
api_key_env = "OPENAI_API_KEY"

[index]
path = "~/.config/brain-mcp/index.db"

[server]
http_port = 47200
grace_period_seconds = 60

[search]
min_score = 0.25
"#;
        let config: Config = toml::from_str(raw).expect("should deserialize without backend");
        assert_eq!(config.index.backend, "sqlite");
    }
}
