use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::Result;
use crate::model::{Filter, Memory, Metadata, SearchResult};
use crate::ports::{BoxFuture, EmbeddingPort, IndexPort, VaultPort};

// ---------------------------------------------------------------------------
// MockVault
// ---------------------------------------------------------------------------

pub struct MockVault {
    store: Mutex<HashMap<String, Memory>>,
}

impl MockVault {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for MockVault {
    fn default() -> Self {
        Self::new()
    }
}

impl VaultPort for MockVault {
    fn write(&self, memory: &Memory) -> BoxFuture<'_, Result<()>> {
        let memory = memory.clone();
        Box::pin(async move {
            self.store.lock().unwrap().insert(memory.id.clone(), memory);
            Ok(())
        })
    }

    fn read(&self, id: &str) -> BoxFuture<'_, Result<Option<Memory>>> {
        let id = id.to_string();
        Box::pin(async move { Ok(self.store.lock().unwrap().get(&id).cloned()) })
    }

    fn delete(&self, id: &str) -> BoxFuture<'_, Result<()>> {
        let id = id.to_string();
        Box::pin(async move {
            self.store.lock().unwrap().remove(&id);
            Ok(())
        })
    }

    fn list_all(&self) -> BoxFuture<'_, Result<Vec<Memory>>> {
        Box::pin(async move { Ok(self.store.lock().unwrap().values().cloned().collect()) })
    }
}

// ---------------------------------------------------------------------------
// MockEmbedder
// ---------------------------------------------------------------------------

pub struct MockEmbedder {
    dims: usize,
    calls: Mutex<Vec<String>>,
}

impl MockEmbedder {
    pub fn new(dims: usize) -> Self {
        Self {
            dims,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Return all texts that were passed to `embed`.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl EmbeddingPort for MockEmbedder {
    fn embed(&self, text: &str) -> BoxFuture<'_, Result<Vec<f32>>> {
        let text = text.to_string();
        Box::pin(async move {
            self.calls.lock().unwrap().push(text.clone());
            Ok(deterministic_vector(&text, self.dims))
        })
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn model_id(&self) -> &str {
        "mock-embed-v0"
    }
}

/// Produce a deterministic, normalised vector from text bytes.
fn deterministic_vector(text: &str, dims: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; dims];
    for (i, b) in text.bytes().enumerate() {
        v[i % dims] += b as f32;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
    v
}

// ---------------------------------------------------------------------------
// MockIndex
// ---------------------------------------------------------------------------

pub struct MockIndex {
    store: Mutex<HashMap<String, (Vec<f32>, Metadata)>>,
    model_id: Mutex<Option<String>>,
}

impl MockIndex {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
            model_id: Mutex::new(None),
        }
    }
}

impl Default for MockIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl IndexPort for MockIndex {
    fn upsert(
        &self,
        id: &str,
        embedding: &[f32],
        metadata: &Metadata,
    ) -> BoxFuture<'_, Result<()>> {
        let id = id.to_string();
        let embedding = embedding.to_vec();
        let metadata = metadata.clone();
        Box::pin(async move {
            self.store.lock().unwrap().insert(id, (embedding, metadata));
            Ok(())
        })
    }

    fn search(
        &self,
        embedding: &[f32],
        limit: usize,
        filter: &Filter,
    ) -> BoxFuture<'_, Result<Vec<SearchResult>>> {
        let embedding = embedding.to_vec();
        let filter = filter.clone();
        Box::pin(async move {
            let store = self.store.lock().unwrap();
            let mut scored: Vec<(f32, &Metadata)> = store
                .values()
                .filter(|(_, meta)| matches_filter(meta, &filter))
                .map(|(vec, meta)| (cosine_similarity(&embedding, vec), meta))
                .collect();
            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(limit);
            Ok(scored
                .into_iter()
                .map(|(score, meta)| SearchResult {
                    memory: Memory {
                        id: meta.id.clone(),
                        title: meta.title.clone(),
                        content: String::new(), // placeholder — service hydrates from vault
                        tags: meta.tags.clone(),
                        category: meta.category.clone(),
                        project: meta.project.clone(),
                        created_at: meta.created_at,
                    },
                    score,
                })
                .collect())
        })
    }

    fn delete(&self, id: &str) -> BoxFuture<'_, Result<()>> {
        let id = id.to_string();
        Box::pin(async move {
            self.store.lock().unwrap().remove(&id);
            Ok(())
        })
    }

    fn list(&self, filter: &Filter) -> BoxFuture<'_, Result<Vec<Metadata>>> {
        let filter = filter.clone();
        Box::pin(async move {
            let store = self.store.lock().unwrap();
            Ok(store
                .values()
                .map(|(_, meta)| meta)
                .filter(|meta| matches_filter(meta, &filter))
                .cloned()
                .collect())
        })
    }

    fn clear(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.store.lock().unwrap().clear();
            Ok(())
        })
    }

    fn stored_model_id(&self) -> BoxFuture<'_, Result<Option<String>>> {
        Box::pin(async move { Ok(self.model_id.lock().unwrap().clone()) })
    }

    fn set_model_id(&self, model_id: &str) -> BoxFuture<'_, Result<()>> {
        let model_id = model_id.to_string();
        Box::pin(async move {
            *self.model_id.lock().unwrap() = Some(model_id);
            Ok(())
        })
    }
}

fn matches_filter(meta: &Metadata, filter: &Filter) -> bool {
    if let Some(cat) = &filter.category
        && &meta.category != cat
    {
        return false;
    }
    if let Some(proj) = &filter.project
        && meta.project.as_ref() != Some(proj)
    {
        return false;
    }
    if let Some(tags) = &filter.tags
        && !tags.iter().any(|t| meta.tags.contains(t))
    {
        return false;
    }
    if let Some(since) = &filter.since
        && meta.created_at < *since
    {
        return false;
    }
    true
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample_memory(id: &str, title: &str) -> Memory {
        Memory {
            id: id.to_string(),
            title: title.to_string(),
            content: format!("Content for {title}"),
            tags: vec!["test".to_string()],
            category: "learnings".to_string(),
            project: Some("brain-mcp".to_string()),
            created_at: Utc::now(),
        }
    }

    fn meta_from(m: &Memory) -> Metadata {
        Metadata::from(m)
    }

    // -- MockVault --

    #[tokio::test]
    async fn vault_write_read() {
        let vault = MockVault::new();
        let m = sample_memory("1", "First");
        vault.write(&m).await.unwrap();
        let got = vault.read("1").await.unwrap();
        assert_eq!(got, Some(m));
    }

    #[tokio::test]
    async fn vault_read_missing() {
        let vault = MockVault::new();
        assert_eq!(vault.read("nope").await.unwrap(), None);
    }

    #[tokio::test]
    async fn vault_delete() {
        let vault = MockVault::new();
        let m = sample_memory("1", "First");
        vault.write(&m).await.unwrap();
        vault.delete("1").await.unwrap();
        assert_eq!(vault.read("1").await.unwrap(), None);
    }

    // -- MockEmbedder --

    #[tokio::test]
    async fn embedder_deterministic() {
        let emb = MockEmbedder::new(8);
        let v1 = emb.embed("hello").await.unwrap();
        let v2 = emb.embed("hello").await.unwrap();
        assert_eq!(v1, v2);
        assert_eq!(v1.len(), 8);
    }

    #[tokio::test]
    async fn embedder_records_calls() {
        let emb = MockEmbedder::new(4);
        emb.embed("one").await.unwrap();
        emb.embed("two").await.unwrap();
        assert_eq!(emb.calls(), vec!["one", "two"]);
    }

    // -- MockIndex --

    #[tokio::test]
    async fn index_upsert_and_list() {
        let idx = MockIndex::new();
        let m = sample_memory("1", "First");
        let meta = meta_from(&m);
        idx.upsert("1", &[1.0, 0.0], &meta).await.unwrap();

        let all = idx.list(&Filter::default()).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "1");
    }

    #[tokio::test]
    async fn index_search_ranks_by_similarity() {
        let idx = MockIndex::new();

        let m1 = sample_memory("1", "close");
        let m2 = sample_memory("2", "far");
        idx.upsert("1", &[1.0, 0.0], &meta_from(&m1)).await.unwrap();
        idx.upsert("2", &[0.0, 1.0], &meta_from(&m2)).await.unwrap();

        let results = idx
            .search(&[1.0, 0.0], 10, &Filter::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].memory.id, "1");
        assert!(results[0].score > results[1].score);
    }

    #[tokio::test]
    async fn index_list_with_filter() {
        let idx = MockIndex::new();
        let mut m1 = sample_memory("1", "A");
        m1.category = "learnings".to_string();
        let mut m2 = sample_memory("2", "B");
        m2.category = "decisions".to_string();

        idx.upsert("1", &[1.0], &meta_from(&m1)).await.unwrap();
        idx.upsert("2", &[1.0], &meta_from(&m2)).await.unwrap();

        let filter = Filter {
            category: Some("decisions".to_string()),
            ..Default::default()
        };
        let filtered = idx.list(&filter).await.unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "2");
    }

    #[tokio::test]
    async fn index_model_id_roundtrip() {
        let idx = MockIndex::new();
        assert_eq!(idx.stored_model_id().await.unwrap(), None);
        idx.set_model_id("test-model").await.unwrap();
        assert_eq!(
            idx.stored_model_id().await.unwrap(),
            Some("test-model".to_string())
        );
    }
}
