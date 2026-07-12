use thiserror::Error;

#[derive(Debug, Error)]
pub enum BrainError {
    #[error("Memory not found: {0}")]
    NotFound(String),

    #[error("Memory already exists: {0}")]
    AlreadyExists(String),

    #[error(
        "A very similar memory already exists: {id} (\"{title}\", similarity {score:.2}). Extend it with memory_update, or retry with force=true to store anyway."
    )]
    Duplicate {
        id: String,
        title: String,
        score: f32,
    },

    #[error("Invalid category: {0}")]
    InvalidCategory(String),

    #[error("Embedding model mismatch: index has '{stored}', config has '{configured}'")]
    ModelMismatch { stored: String, configured: String },

    #[error("Vault error: {0}")]
    Vault(String),

    #[error("Embedding error: {0}")]
    Embedding(String),

    #[error("Index error: {0}")]
    Index(String),
}

pub type Result<T> = std::result::Result<T, BrainError>;
