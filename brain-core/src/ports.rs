use async_trait::async_trait;

use crate::error::Result;
use crate::model::{Filter, Memory, Metadata, SearchResult};

#[async_trait]
pub trait VaultPort: Send + Sync {
    async fn write(&self, memory: &Memory) -> Result<()>;
    async fn read(&self, id: &str) -> Result<Option<Memory>>;
    async fn delete(&self, id: &str) -> Result<()>;
    async fn list_all(&self) -> Result<Vec<Memory>>;
}

#[async_trait]
pub trait EmbeddingPort: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
    fn dimensions(&self) -> usize;
    fn model_id(&self) -> &str;
}

#[async_trait]
pub trait IndexPort: Send + Sync {
    async fn upsert(&self, id: &str, embedding: &[f32], metadata: &Metadata) -> Result<()>;
    async fn search(
        &self,
        embedding: &[f32],
        limit: usize,
        filter: &Filter,
    ) -> Result<Vec<SearchResult>>;
    async fn delete(&self, id: &str) -> Result<()>;
    async fn list(&self, filter: &Filter) -> Result<Vec<Metadata>>;
    async fn clear(&self) -> Result<()>;
    async fn stored_model_id(&self) -> Result<Option<String>>;
    async fn set_model_id(&self, model_id: &str) -> Result<()>;
}
