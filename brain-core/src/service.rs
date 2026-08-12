use std::sync::Arc;

use chrono::Utc;
use tracing::warn;

use crate::error::{BrainError, Result};
use crate::id::generate_id;
use crate::model::{Filter, Memory, Metadata, SearchResult};
use crate::ports::{EmbeddingPort, IndexPort, VaultPort};

const DUPLICATE_THRESHOLD: f32 = 0.9;

pub struct MemoryService {
    vault: Arc<dyn VaultPort>,
    embedder: Arc<dyn EmbeddingPort>,
    index: Arc<dyn IndexPort>,
    min_score: f32,
}

impl MemoryService {
    pub fn new(
        vault: Arc<dyn VaultPort>,
        embedder: Arc<dyn EmbeddingPort>,
        index: Arc<dyn IndexPort>,
    ) -> Self {
        Self {
            vault,
            embedder,
            index,
            min_score: 0.0,
        }
    }

    pub fn with_min_score(mut self, min_score: f32) -> Self {
        self.min_score = min_score;
        self
    }

    pub async fn store(
        &self,
        title: String,
        content: String,
        tags: Vec<String>,
        category: String,
        project: Option<String>,
        force: bool,
    ) -> Result<Memory> {
        let now = Utc::now();
        let id = generate_id(&title, now);

        let embedding = self.embedder.embed(&content).await?;

        if !force {
            if self.vault.read(&id).await?.is_some() {
                return Err(BrainError::AlreadyExists(format!(
                    "{id} — extend it with memory_update, or retry with force=true to overwrite"
                )));
            }
            let similar = self.index.search(&embedding, 1, &Filter::default()).await?;
            if let Some(top) = similar.first()
                && top.score >= DUPLICATE_THRESHOLD
            {
                return Err(BrainError::Duplicate {
                    id: top.memory.id.clone(),
                    title: top.memory.title.clone(),
                    score: top.score,
                });
            }
        }

        let memory = Memory {
            id,
            title,
            content,
            tags,
            category,
            project,
            created_at: now,
        };

        self.vault.write(&memory).await?;

        let metadata = Metadata::from(&memory);
        self.index.upsert(&memory.id, &embedding, &metadata).await?;

        Ok(memory)
    }

    pub async fn search(
        &self,
        query: &str,
        limit: usize,
        filter: &Filter,
    ) -> Result<Vec<SearchResult>> {
        let embedding = self.embedder.embed(query).await?;
        let mut results = self.index.search(&embedding, limit, filter).await?;
        results.retain(|r| r.score >= self.min_score);

        if !results.is_empty() {
            let ids: Vec<String> = results.iter().map(|r| r.memory.id.clone()).collect();
            self.index.record_access(&ids).await?;
        }

        let mut hydrated = Vec::with_capacity(results.len());
        for result in results {
            let memory = match self.vault.read(&result.memory.id).await {
                Ok(Some(memory)) => memory,
                Ok(None) => result.memory,
                Err(e) => {
                    warn!("hydrating {} from vault failed: {e}", result.memory.id);
                    result.memory
                }
            };
            hydrated.push(SearchResult {
                memory,
                score: result.score,
            });
        }

        Ok(hydrated)
    }

    pub async fn list(&self, filter: &Filter) -> Result<Vec<Metadata>> {
        self.index.list(filter).await
    }

    pub async fn update(
        &self,
        id: &str,
        title: Option<String>,
        content: Option<String>,
        tags: Option<Vec<String>>,
    ) -> Result<Memory> {
        let existing = self
            .vault
            .read(id)
            .await?
            .ok_or_else(|| BrainError::NotFound(id.to_string()))?;

        let updated = Memory {
            id: existing.id,
            title: title.unwrap_or(existing.title),
            content: content.unwrap_or(existing.content),
            tags: tags.unwrap_or(existing.tags),
            category: existing.category,
            project: existing.project,
            created_at: existing.created_at,
        };

        self.vault.write(&updated).await?;

        let embedding = self.embedder.embed(&updated.content).await?;
        let metadata = Metadata::from(&updated);
        self.index
            .upsert(&updated.id, &embedding, &metadata)
            .await?;

        Ok(updated)
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        self.vault.delete(id).await?;
        self.index.delete(id).await?;
        Ok(())
    }

    pub async fn reindex(&self) -> Result<usize> {
        self.index.clear().await?;
        self.index.set_model_id(self.embedder.model_id()).await?;

        let memories = self.vault.list_all().await?;
        let count = memories.len();

        for memory in &memories {
            let embedding = self.embedder.embed(&memory.content).await?;
            let metadata = Metadata::from(memory);
            self.index.upsert(&memory.id, &embedding, &metadata).await?;
        }

        Ok(count)
    }

    pub async fn check_model_compatibility(&self) -> Result<()> {
        let stored = self.index.stored_model_id().await?;
        if let Some(stored) = stored {
            let configured = self.embedder.model_id();
            if stored != configured {
                return Err(BrainError::ModelMismatch {
                    stored,
                    configured: configured.to_string(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mocks::{MockEmbedder, MockIndex, MockVault};

    fn make_service() -> (
        Arc<MockVault>,
        Arc<MockEmbedder>,
        Arc<MockIndex>,
        MemoryService,
    ) {
        let vault = Arc::new(MockVault::new());
        let embedder = Arc::new(MockEmbedder::new(8));
        let index = Arc::new(MockIndex::new());
        let svc = MemoryService::new(vault.clone(), embedder.clone(), index.clone());
        (vault, embedder, index, svc)
    }

    #[tokio::test]
    async fn test_store_creates_memory() {
        let (vault, _embedder, index, svc) = make_service();

        let mem = svc
            .store(
                "My Title".into(),
                "Some content".into(),
                vec!["rust".into()],
                "learnings".into(),
                Some("brain-mcp".into()),
                false,
            )
            .await
            .unwrap();

        assert_eq!(mem.title, "My Title");
        assert_eq!(mem.content, "Some content");

        // Verify it's in the vault
        let from_vault = vault.read(&mem.id).await.unwrap().unwrap();
        assert_eq!(from_vault.title, "My Title");

        // Verify it's in the index
        let listed = index.list(&Filter::default()).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, mem.id);
    }

    #[tokio::test]
    async fn test_store_generates_id() {
        let (_vault, _embedder, _index, svc) = make_service();

        let mem = svc
            .store(
                "Deploy New App".into(),
                "content".into(),
                vec![],
                "learnings".into(),
                None,
                false,
            )
            .await
            .unwrap();

        // ID should start with today's date and contain slugified title
        assert!(mem.id.contains("deploy-new-app"));
        // Date prefix: YYYYMMDD
        let date_prefix = &mem.id[..8];
        assert!(date_prefix.chars().all(|c| c.is_ascii_digit()));
    }

    #[tokio::test]
    async fn test_search_returns_results() {
        let (_vault, _embedder, _index, svc) = make_service();

        svc.store(
            "Rust Lifetimes".into(),
            "Lifetimes ensure references are valid".into(),
            vec!["rust".into()],
            "learnings".into(),
            None,
            false,
        )
        .await
        .unwrap();

        let results = svc
            .search(
                "Lifetimes ensure references are valid",
                10,
                &Filter::default(),
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory.title, "Rust Lifetimes");
    }

    struct UnreadableVault;

    impl VaultPort for UnreadableVault {
        fn write(&self, _memory: &Memory) -> crate::ports::BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn read(&self, _id: &str) -> crate::ports::BoxFuture<'_, Result<Option<Memory>>> {
            Box::pin(async {
                Err(BrainError::Vault(
                    "failed to parse frontmatter: missing field `category`".into(),
                ))
            })
        }

        fn delete(&self, _id: &str) -> crate::ports::BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn list_all(&self) -> crate::ports::BoxFuture<'_, Result<Vec<Memory>>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    #[tokio::test]
    async fn test_search_survives_unreadable_vault_file() {
        let (_vault, embedder, index, svc) = make_service();

        svc.store(
            "Rust Lifetimes".into(),
            "Lifetimes ensure references are valid".into(),
            vec!["rust".into()],
            "learnings".into(),
            None,
            false,
        )
        .await
        .unwrap();

        let broken = MemoryService::new(Arc::new(UnreadableVault), embedder, index);

        let results = broken
            .search(
                "Lifetimes ensure references are valid",
                10,
                &Filter::default(),
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory.title, "Rust Lifetimes");
    }

    #[tokio::test]
    async fn test_search_records_access() {
        let (_vault, _embedder, index, svc) = make_service();

        let mem = svc
            .store(
                "Accessed Memory".into(),
                "some searchable content".into(),
                vec![],
                "learnings".into(),
                None,
                false,
            )
            .await
            .unwrap();

        assert_eq!(index.access_count(&mem.id), 0);

        svc.search("some searchable content", 10, &Filter::default())
            .await
            .unwrap();
        svc.search("some searchable content", 10, &Filter::default())
            .await
            .unwrap();

        assert_eq!(index.access_count(&mem.id), 2);
    }

    #[tokio::test]
    async fn test_search_filters_below_min_score() {
        let vault = Arc::new(MockVault::new());
        let embedder = Arc::new(MockEmbedder::new(8));
        let index = Arc::new(MockIndex::new());
        let svc = MemoryService::new(vault, embedder, index).with_min_score(0.99);

        svc.store(
            "Rust Lifetimes".into(),
            "Lifetimes ensure references are valid".into(),
            vec![],
            "learnings".into(),
            None,
            false,
        )
        .await
        .unwrap();

        let exact = svc
            .search(
                "Lifetimes ensure references are valid",
                10,
                &Filter::default(),
            )
            .await
            .unwrap();
        assert_eq!(exact.len(), 1);

        let unrelated = svc
            .search("completely different topic", 10, &Filter::default())
            .await
            .unwrap();
        assert!(unrelated.is_empty());
    }

    #[tokio::test]
    async fn test_search_hydrates_from_vault() {
        let (_vault, _embedder, _index, svc) = make_service();

        svc.store(
            "Test Memory".into(),
            "Full content body here".into(),
            vec![],
            "learnings".into(),
            None,
            false,
        )
        .await
        .unwrap();

        let results = svc
            .search("Full content body here", 10, &Filter::default())
            .await
            .unwrap();

        // MockIndex returns empty content; service should hydrate from vault
        assert_eq!(results[0].memory.content, "Full content body here");
    }

    #[tokio::test]
    async fn test_list_delegates_to_index() {
        let (_vault, _embedder, _index, svc) = make_service();

        svc.store(
            "A".into(),
            "a".into(),
            vec![],
            "learnings".into(),
            None,
            false,
        )
        .await
        .unwrap();

        svc.store(
            "B".into(),
            "b".into(),
            vec![],
            "decisions".into(),
            None,
            true,
        )
        .await
        .unwrap();

        let filter = Filter {
            category: Some("decisions".into()),
            ..Default::default()
        };
        let listed = svc.list(&filter).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].category, "decisions");
    }

    #[tokio::test]
    async fn test_store_rejects_near_duplicate_content() {
        let (_vault, _embedder, _index, svc) = make_service();

        svc.store(
            "Original".into(),
            "identical content body".into(),
            vec![],
            "learnings".into(),
            None,
            false,
        )
        .await
        .unwrap();

        let result = svc
            .store(
                "Different Title".into(),
                "identical content body".into(),
                vec![],
                "learnings".into(),
                None,
                false,
            )
            .await;

        assert!(matches!(result.unwrap_err(), BrainError::Duplicate { .. }));
    }

    #[tokio::test]
    async fn test_store_force_bypasses_duplicate_check() {
        let (_vault, _embedder, _index, svc) = make_service();

        svc.store(
            "Original".into(),
            "identical content body".into(),
            vec![],
            "learnings".into(),
            None,
            false,
        )
        .await
        .unwrap();

        svc.store(
            "Different Title".into(),
            "identical content body".into(),
            vec![],
            "learnings".into(),
            None,
            true,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_store_rejects_same_id_overwrite() {
        let (_vault, _embedder, _index, svc) = make_service();

        svc.store(
            "Same Title".into(),
            "first version".into(),
            vec![],
            "learnings".into(),
            None,
            false,
        )
        .await
        .unwrap();

        let result = svc
            .store(
                "Same Title".into(),
                "completely unrelated second version 12345".into(),
                vec![],
                "learnings".into(),
                None,
                false,
            )
            .await;

        assert!(matches!(result.unwrap_err(), BrainError::AlreadyExists(_)));
    }

    #[tokio::test]
    async fn test_update_merges_fields() {
        let (_vault, _embedder, _index, svc) = make_service();

        let mem = svc
            .store(
                "Original Title".into(),
                "Original content".into(),
                vec!["tag1".into()],
                "learnings".into(),
                None,
                false,
            )
            .await
            .unwrap();

        let updated = svc
            .update(&mem.id, Some("New Title".into()), None, None)
            .await
            .unwrap();

        assert_eq!(updated.title, "New Title");
        assert_eq!(updated.content, "Original content");
        assert_eq!(updated.tags, vec!["tag1".to_string()]);
    }

    #[tokio::test]
    async fn test_update_not_found() {
        let (_vault, _embedder, _index, svc) = make_service();

        let result = svc
            .update("nonexistent-id", Some("Title".into()), None, None)
            .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), BrainError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_delete_removes_from_both() {
        let (vault, _embedder, index, svc) = make_service();

        let mem = svc
            .store(
                "To Delete".into(),
                "content".into(),
                vec![],
                "learnings".into(),
                None,
                false,
            )
            .await
            .unwrap();

        svc.delete(&mem.id).await.unwrap();

        assert!(vault.read(&mem.id).await.unwrap().is_none());
        assert!(index.list(&Filter::default()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_reindex_reembeds_all() {
        let (_vault, _embedder, index, svc) = make_service();

        for title in &["One", "Two", "Three"] {
            svc.store(
                title.to_string(),
                format!("content for {title}"),
                vec![],
                "learnings".into(),
                None,
                true,
            )
            .await
            .unwrap();
        }

        // Clear index directly to simulate stale state
        index.clear().await.unwrap();
        assert!(index.list(&Filter::default()).await.unwrap().is_empty());

        let count = svc.reindex().await.unwrap();
        assert_eq!(count, 3);
        assert_eq!(index.list(&Filter::default()).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn test_check_model_compatibility_ok() {
        let (_vault, _embedder, _index, svc) = make_service();

        // Fresh index with no stored model ID should pass
        svc.check_model_compatibility().await.unwrap();
    }

    #[tokio::test]
    async fn test_check_model_compatibility_match() {
        let (_vault, _embedder, index, svc) = make_service();

        index.set_model_id("mock-embed-v0").await.unwrap();
        svc.check_model_compatibility().await.unwrap();
    }

    #[tokio::test]
    async fn test_check_model_compatibility_mismatch() {
        let (_vault, _embedder, index, svc) = make_service();

        index.set_model_id("other-model-v1").await.unwrap();

        let result = svc.check_model_compatibility().await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            BrainError::ModelMismatch { .. }
        ));
    }
}
