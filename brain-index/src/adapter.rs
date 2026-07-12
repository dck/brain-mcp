use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use tokio::sync::Mutex;

use brain_core::error::{BrainError, Result};
use brain_core::model::{Filter, Memory, Metadata, SearchResult};
use brain_core::ports::{BoxFuture, IndexPort};

pub struct SqliteVecIndex {
    conn: Mutex<Connection>,
    #[allow(dead_code)]
    dims: usize,
}

impl SqliteVecIndex {
    pub fn open(path: &Path, dims: usize) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        create_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            dims,
        })
    }

    pub fn open_in_memory(dims: usize) -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        create_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            dims,
        })
    }
}

const RECENCY_WEIGHT: f32 = 0.05;
const RECENCY_DECAY_DAYS: f32 = 90.0;

fn create_schema(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
         CREATE TABLE IF NOT EXISTS memories (
             id TEXT PRIMARY KEY,
             title TEXT NOT NULL,
             tags TEXT NOT NULL,
             category TEXT NOT NULL,
             project TEXT,
             created_at TEXT NOT NULL,
             access_count INTEGER NOT NULL DEFAULT 0,
             last_accessed_at TEXT
         );
         CREATE TABLE IF NOT EXISTS memory_vectors (
             id TEXT PRIMARY KEY,
             embedding BLOB NOT NULL
         );",
    )?;
    for stmt in [
        "ALTER TABLE memories ADD COLUMN access_count INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE memories ADD COLUMN last_accessed_at TEXT",
    ] {
        if let Err(e) = conn.execute(stmt, [])
            && !e.to_string().contains("duplicate column name")
        {
            return Err(e.into());
        }
    }
    Ok(())
}

fn f32_slice_to_bytes(slice: &[f32]) -> Vec<u8> {
    slice.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn bytes_to_f32_vec(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }
    dot / (mag_a * mag_b)
}

fn matches_filter(meta: &Metadata, filter: &Filter) -> bool {
    if let Some(ref cat) = filter.category
        && meta.category != *cat
    {
        return false;
    }
    if let Some(ref proj) = filter.project
        && meta.project.as_deref() != Some(proj.as_str())
    {
        return false;
    }
    if let Some(ref since) = filter.since
        && meta.created_at < *since
    {
        return false;
    }
    if let Some(ref tags) = filter.tags {
        for tag in tags {
            if !meta.tags.contains(tag) {
                return false;
            }
        }
    }
    true
}

fn row_to_metadata(row: &rusqlite::Row<'_>) -> rusqlite::Result<Metadata> {
    let id: String = row.get(0)?;
    let title: String = row.get(1)?;
    let tags_json: String = row.get(2)?;
    let category: String = row.get(3)?;
    let project: Option<String> = row.get(4)?;
    let created_at_str: String = row.get(5)?;

    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
    let created_at: DateTime<Utc> = created_at_str.parse().unwrap();

    Ok(Metadata {
        id,
        title,
        tags,
        category,
        project,
        created_at,
    })
}

impl IndexPort for SqliteVecIndex {
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
            let conn = self.conn.lock().await;
            let tags_json = serde_json::to_string(&metadata.tags)
                .map_err(|e| BrainError::Index(e.to_string()))?;
            let created_at_str = metadata.created_at.to_rfc3339();
            let blob = f32_slice_to_bytes(&embedding);

            conn.execute(
                "INSERT INTO memories (id, title, tags, category, project, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET title = excluded.title, tags = excluded.tags,
                     category = excluded.category, project = excluded.project, created_at = excluded.created_at",
                rusqlite::params![id, metadata.title, tags_json, metadata.category, metadata.project, created_at_str],
            ).map_err(|e| BrainError::Index(e.to_string()))?;

            conn.execute(
                "INSERT OR REPLACE INTO memory_vectors (id, embedding) VALUES (?1, ?2)",
                rusqlite::params![id, blob],
            )
            .map_err(|e| BrainError::Index(e.to_string()))?;

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
            let conn = self.conn.lock().await;

            let mut stmt = conn
                .prepare(
                    "SELECT m.id, m.title, m.tags, m.category, m.project, m.created_at, v.embedding
                     FROM memories m JOIN memory_vectors v ON v.id = m.id",
                )
                .map_err(|e| BrainError::Index(e.to_string()))?;

            let now = Utc::now();
            // Rank by cosine similarity plus a small recency boost so fresher
            // memories win near-ties; the reported score stays pure cosine.
            let mut scored: Vec<(Metadata, f32, f32)> = stmt
                .query_map([], |row| {
                    let meta = row_to_metadata(row)?;
                    let blob: Vec<u8> = row.get(6)?;
                    Ok((meta, blob))
                })
                .map_err(|e| BrainError::Index(e.to_string()))?
                .filter_map(|r| r.ok())
                .filter(|(meta, _)| matches_filter(meta, &filter))
                .map(|(meta, blob)| {
                    let vec = bytes_to_f32_vec(&blob);
                    let score = cosine_similarity(&embedding, &vec);
                    let age_days =
                        ((now - meta.created_at).num_seconds() as f32 / 86_400.0).max(0.0);
                    let ranking = score + RECENCY_WEIGHT * (-age_days / RECENCY_DECAY_DAYS).exp();
                    (meta, score, ranking)
                })
                .collect();

            scored.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(limit);

            Ok(scored
                .into_iter()
                .map(|(meta, score, _)| SearchResult {
                    memory: Memory {
                        id: meta.id,
                        title: meta.title,
                        content: String::new(),
                        tags: meta.tags,
                        category: meta.category,
                        project: meta.project,
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
            let conn = self.conn.lock().await;
            conn.execute("DELETE FROM memories WHERE id = ?1", rusqlite::params![id])
                .map_err(|e| BrainError::Index(e.to_string()))?;
            conn.execute(
                "DELETE FROM memory_vectors WHERE id = ?1",
                rusqlite::params![id],
            )
            .map_err(|e| BrainError::Index(e.to_string()))?;
            Ok(())
        })
    }

    fn list(&self, filter: &Filter) -> BoxFuture<'_, Result<Vec<Metadata>>> {
        let filter = filter.clone();
        Box::pin(async move {
            let conn = self.conn.lock().await;
            let mut stmt = conn
                .prepare("SELECT id, title, tags, category, project, created_at FROM memories")
                .map_err(|e| BrainError::Index(e.to_string()))?;

            let all: Vec<Metadata> = stmt
                .query_map([], row_to_metadata)
                .map_err(|e| BrainError::Index(e.to_string()))?
                .filter_map(|r| r.ok())
                .filter(|meta| matches_filter(meta, &filter))
                .collect();

            Ok(all)
        })
    }

    fn record_access(&self, ids: &[String]) -> BoxFuture<'_, Result<()>> {
        let ids = ids.to_vec();
        Box::pin(async move {
            let conn = self.conn.lock().await;
            let now = Utc::now().to_rfc3339();
            for id in &ids {
                conn.execute(
                    "UPDATE memories SET access_count = access_count + 1, last_accessed_at = ?1 WHERE id = ?2",
                    rusqlite::params![now, id],
                )
                .map_err(|e| BrainError::Index(e.to_string()))?;
            }
            Ok(())
        })
    }

    fn clear(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let conn = self.conn.lock().await;
            conn.execute("DELETE FROM memories", [])
                .map_err(|e| BrainError::Index(e.to_string()))?;
            conn.execute("DELETE FROM memory_vectors", [])
                .map_err(|e| BrainError::Index(e.to_string()))?;
            Ok(())
        })
    }

    fn stored_model_id(&self) -> BoxFuture<'_, Result<Option<String>>> {
        Box::pin(async move {
            let conn = self.conn.lock().await;
            let result =
                conn.query_row("SELECT value FROM meta WHERE key = 'model_id'", [], |row| {
                    row.get(0)
                });
            match result {
                Ok(val) => Ok(Some(val)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(BrainError::Index(e.to_string())),
            }
        })
    }

    fn set_model_id(&self, model_id: &str) -> BoxFuture<'_, Result<()>> {
        let model_id = model_id.to_string();
        Box::pin(async move {
            let conn = self.conn.lock().await;
            conn.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('model_id', ?1)",
                rusqlite::params![model_id],
            )
            .map_err(|e| BrainError::Index(e.to_string()))?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_metadata(id: &str, category: &str, tags: Vec<&str>, project: Option<&str>) -> Metadata {
        Metadata {
            id: id.to_string(),
            title: format!("Title for {id}"),
            tags: tags.into_iter().map(String::from).collect(),
            category: category.to_string(),
            project: project.map(String::from),
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_creates_schema() {
        let index = SqliteVecIndex::open_in_memory(3).unwrap();
        let conn = index.conn.lock().await;
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(tables.contains(&"meta".to_string()));
        assert!(tables.contains(&"memories".to_string()));
        assert!(tables.contains(&"memory_vectors".to_string()));
    }

    #[tokio::test]
    async fn test_upsert_and_search() {
        let index = SqliteVecIndex::open_in_memory(3).unwrap();
        let meta = make_metadata("m1", "learnings", vec!["rust"], None);
        let vec = vec![1.0, 0.0, 0.0];
        index.upsert("m1", &vec, &meta).await.unwrap();

        let results = index.search(&vec, 10, &Filter::default()).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory.id, "m1");
        assert!((results[0].score - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_search_ranking() {
        let index = SqliteVecIndex::open_in_memory(3).unwrap();

        let m1 = make_metadata("m1", "learnings", vec![], None);
        let m2 = make_metadata("m2", "learnings", vec![], None);
        let m3 = make_metadata("m3", "learnings", vec![], None);

        // m1 is exact match, m2 is somewhat similar, m3 is orthogonal
        index.upsert("m1", &[1.0, 0.0, 0.0], &m1).await.unwrap();
        index.upsert("m2", &[0.8, 0.6, 0.0], &m2).await.unwrap();
        index.upsert("m3", &[0.0, 0.0, 1.0], &m3).await.unwrap();

        let results = index
            .search(&[1.0, 0.0, 0.0], 3, &Filter::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].memory.id, "m1");
        assert_eq!(results[1].memory.id, "m2");
        assert_eq!(results[2].memory.id, "m3");
    }

    #[tokio::test]
    async fn test_search_with_category_filter() {
        let index = SqliteVecIndex::open_in_memory(3).unwrap();

        let m1 = make_metadata("m1", "learnings", vec![], None);
        let m2 = make_metadata("m2", "decisions", vec![], None);

        index.upsert("m1", &[1.0, 0.0, 0.0], &m1).await.unwrap();
        index.upsert("m2", &[1.0, 0.0, 0.0], &m2).await.unwrap();

        let filter = Filter {
            category: Some("decisions".to_string()),
            ..Filter::default()
        };
        let results = index.search(&[1.0, 0.0, 0.0], 10, &filter).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory.id, "m2");
    }

    #[tokio::test]
    async fn test_list_all() {
        let index = SqliteVecIndex::open_in_memory(3).unwrap();

        for i in 1..=3 {
            let id = format!("m{i}");
            let meta = make_metadata(&id, "learnings", vec![], None);
            index.upsert(&id, &[1.0, 0.0, 0.0], &meta).await.unwrap();
        }

        let all = index.list(&Filter::default()).await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn test_list_with_tag_filter() {
        let index = SqliteVecIndex::open_in_memory(3).unwrap();

        let m1 = make_metadata("m1", "learnings", vec!["rust", "async"], None);
        let m2 = make_metadata("m2", "learnings", vec!["python"], None);
        let m3 = make_metadata("m3", "learnings", vec!["rust"], None);

        index.upsert("m1", &[1.0, 0.0, 0.0], &m1).await.unwrap();
        index.upsert("m2", &[1.0, 0.0, 0.0], &m2).await.unwrap();
        index.upsert("m3", &[1.0, 0.0, 0.0], &m3).await.unwrap();

        let filter = Filter {
            tags: Some(vec!["rust".to_string()]),
            ..Filter::default()
        };
        let results = index.list(&filter).await.unwrap();
        assert_eq!(results.len(), 2);
        let ids: Vec<&str> = results.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"m1"));
        assert!(ids.contains(&"m3"));
    }

    #[tokio::test]
    async fn test_delete() {
        let index = SqliteVecIndex::open_in_memory(3).unwrap();

        let meta = make_metadata("m1", "learnings", vec![], None);
        index.upsert("m1", &[1.0, 0.0, 0.0], &meta).await.unwrap();
        index.delete("m1").await.unwrap();

        let all = index.list(&Filter::default()).await.unwrap();
        assert!(all.is_empty());

        let results = index
            .search(&[1.0, 0.0, 0.0], 10, &Filter::default())
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_clear() {
        let index = SqliteVecIndex::open_in_memory(3).unwrap();

        for i in 1..=3 {
            let id = format!("m{i}");
            let meta = make_metadata(&id, "learnings", vec![], None);
            index.upsert(&id, &[1.0, 0.0, 0.0], &meta).await.unwrap();
        }

        index.clear().await.unwrap();

        let all = index.list(&Filter::default()).await.unwrap();
        assert!(all.is_empty());

        let results = index
            .search(&[1.0, 0.0, 0.0], 10, &Filter::default())
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_search_recency_breaks_ties() {
        let index = SqliteVecIndex::open_in_memory(3).unwrap();

        let mut old = make_metadata("old", "learnings", vec![], None);
        old.created_at = Utc::now() - chrono::Duration::days(365);
        let fresh = make_metadata("fresh", "learnings", vec![], None);

        index.upsert("old", &[1.0, 0.0, 0.0], &old).await.unwrap();
        index
            .upsert("fresh", &[1.0, 0.0, 0.0], &fresh)
            .await
            .unwrap();

        let results = index
            .search(&[1.0, 0.0, 0.0], 2, &Filter::default())
            .await
            .unwrap();
        assert_eq!(results[0].memory.id, "fresh");
        assert_eq!(results[1].memory.id, "old");
        assert!((results[0].score - results[1].score).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_record_access_increments() {
        let index = SqliteVecIndex::open_in_memory(3).unwrap();
        let meta = make_metadata("m1", "learnings", vec![], None);
        index.upsert("m1", &[1.0, 0.0, 0.0], &meta).await.unwrap();

        index.record_access(&["m1".to_string()]).await.unwrap();
        index.record_access(&["m1".to_string()]).await.unwrap();

        let conn = index.conn.lock().await;
        let (count, last): (i64, Option<String>) = conn
            .query_row(
                "SELECT access_count, last_accessed_at FROM memories WHERE id = 'm1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 2);
        assert!(last.is_some());
    }

    #[tokio::test]
    async fn test_upsert_preserves_access_count() {
        let index = SqliteVecIndex::open_in_memory(3).unwrap();
        let meta = make_metadata("m1", "learnings", vec![], None);
        index.upsert("m1", &[1.0, 0.0, 0.0], &meta).await.unwrap();
        index.record_access(&["m1".to_string()]).await.unwrap();

        index.upsert("m1", &[0.0, 1.0, 0.0], &meta).await.unwrap();

        let conn = index.conn.lock().await;
        let count: i64 = conn
            .query_row(
                "SELECT access_count FROM memories WHERE id = 'm1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_model_id_tracking() {
        let index = SqliteVecIndex::open_in_memory(3).unwrap();
        index.set_model_id("text-embedding-3-small").await.unwrap();
        let stored = index.stored_model_id().await.unwrap();
        assert_eq!(stored, Some("text-embedding-3-small".to_string()));
    }

    #[tokio::test]
    async fn test_model_id_initially_none() {
        let index = SqliteVecIndex::open_in_memory(3).unwrap();
        let stored = index.stored_model_id().await.unwrap();
        assert_eq!(stored, None);
    }
}
