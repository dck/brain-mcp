use thiserror::Error;

#[derive(Debug, Error)]
pub enum BrainError {
    #[error("Memory not found: {0}")]
    NotFound(String),

    #[error("Memory already exists: {0}")]
    AlreadyExists(String),

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
