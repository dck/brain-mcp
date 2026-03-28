mod openai;

pub use openai::OpenAiEmbedder;

use std::sync::Arc;

use brain_core::config::EmbeddingConfig;
use brain_core::ports::EmbeddingPort;

pub fn create_embedder(config: &EmbeddingConfig) -> Result<Arc<dyn EmbeddingPort>, anyhow::Error> {
    match config.provider.as_str() {
        "openai" => {
            let env_var = config.api_key_env.as_deref().unwrap_or("OPENAI_API_KEY");
            let api_key = std::env::var(env_var)
                .map_err(|_| anyhow::anyhow!("missing env var: {env_var}"))?;

            let dims = match config.model.as_str() {
                "text-embedding-3-small" => 1536,
                "text-embedding-3-large" => 3072,
                "text-embedding-ada-002" => 1536,
                other => anyhow::bail!("unknown OpenAI embedding model: {other}"),
            };

            Ok(Arc::new(OpenAiEmbedder::new(
                "https://api.openai.com".into(),
                api_key,
                config.model.clone(),
                dims,
            )))
        }
        other => anyhow::bail!("unknown embedding provider: {other}"),
    }
}
