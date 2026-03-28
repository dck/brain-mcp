use std::future::Future;
use std::pin::Pin;

use crate::error::Result;
use crate::model::{Filter, Memory, Metadata, SearchResult};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait VaultPort: Send + Sync {
    fn write(&self, memory: &Memory) -> BoxFuture<'_, Result<()>>;
    fn read(&self, id: &str) -> BoxFuture<'_, Result<Option<Memory>>>;
    fn delete(&self, id: &str) -> BoxFuture<'_, Result<()>>;
    fn list_all(&self) -> BoxFuture<'_, Result<Vec<Memory>>>;
}

pub trait EmbeddingPort: Send + Sync {
    fn embed(&self, text: &str) -> BoxFuture<'_, Result<Vec<f32>>>;
    fn dimensions(&self) -> usize;
    fn model_id(&self) -> &str;
}

pub trait IndexPort: Send + Sync {
    fn upsert(&self, id: &str, embedding: &[f32], metadata: &Metadata)
    -> BoxFuture<'_, Result<()>>;
    fn search(
        &self,
        embedding: &[f32],
        limit: usize,
        filter: &Filter,
    ) -> BoxFuture<'_, Result<Vec<SearchResult>>>;
    fn delete(&self, id: &str) -> BoxFuture<'_, Result<()>>;
    fn list(&self, filter: &Filter) -> BoxFuture<'_, Result<Vec<Metadata>>>;
    fn clear(&self) -> BoxFuture<'_, Result<()>>;
    fn stored_model_id(&self) -> BoxFuture<'_, Result<Option<String>>>;
    fn set_model_id(&self, model_id: &str) -> BoxFuture<'_, Result<()>>;
}
