use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, Transaction};
use tokio::sync::Mutex;
use tracing::warn;

use brain_core::error::{BrainError, Result};
use brain_core::model::{Filter, IndexEntry, Memory, Metadata, SearchResult};
use brain_core::ports::{BoxFuture, IndexPort};

pub struct SqliteVecIndex {
    conn: Mutex<Connection>,
}

impl SqliteVecIndex {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = prepare_connection(Connection::open(path)?, &path.display().to_string())?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = prepare_connection(Connection::open_in_memory()?, ":memory:")?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

const RECENCY_WEIGHT: f32 = 0.05;
const RECENCY_DECAY_DAYS: f32 = 90.0;

const MIGRATION_1_SQL: &str = "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
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
     );";

type Migration = fn(&Transaction<'_>) -> rusqlite::Result<()>;

const MIGRATIONS: &[Migration] = &[migration_1];

fn migration_1(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(MIGRATION_1_SQL)?;
    for (column, ddl) in [
        (
            "access_count",
            "ALTER TABLE memories ADD COLUMN access_count INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "last_accessed_at",
            "ALTER TABLE memories ADD COLUMN last_accessed_at TEXT",
        ),
    ] {
        let exists = tx
            .prepare("SELECT 1 FROM pragma_table_info('memories') WHERE name = ?1")?
            .exists([column])?;
        if !exists {
            tx.execute_batch(ddl)?;
        }
    }
    Ok(())
}

fn migrate(conn: &mut Connection, label: &str) -> anyhow::Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let latest = MIGRATIONS.len() as i64;
    if current > latest {
        anyhow::bail!(
            "index schema version {current} is newer than this brain-mcp supports ({latest}); upgrade brain-mcp or delete {label} to rebuild it"
        );
    }
    for (i, migration) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        migration(&tx)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

fn prepare_connection(mut conn: Connection, label: &str) -> anyhow::Result<Connection> {
    conn.busy_timeout(Duration::from_secs(5))?;
    let _mode: String = conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))?;
    migrate(&mut conn, label)?;
    Ok(conn)
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
    let created_at: DateTime<Utc> = created_at_str.parse().map_err(|e: chrono::ParseError| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(Metadata {
        id,
        title,
        tags,
        category,
        project,
        created_at,
    })
}

fn write_row(conn: &Connection, id: &str, embedding: &[f32], metadata: &Metadata) -> Result<()> {
    let tags_json =
        serde_json::to_string(&metadata.tags).map_err(|e| BrainError::Index(e.to_string()))?;
    let created_at_str = metadata.created_at.to_rfc3339();
    let blob = f32_slice_to_bytes(embedding);

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
            let mut conn = self.conn.lock().await;
            let tx = conn
                .transaction()
                .map_err(|e| BrainError::Index(e.to_string()))?;
            write_row(&tx, &id, &embedding, &metadata)?;
            tx.commit().map_err(|e| BrainError::Index(e.to_string()))?;
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
                .filter_map(|r| match r {
                    Ok(v) => Some(v),
                    Err(e) => {
                        warn!("skipping index row: {e}");
                        None
                    }
                })
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
                        updated_at: None,
                        extra: BTreeMap::new(),
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
                .filter_map(|r| match r {
                    Ok(v) => Some(v),
                    Err(e) => {
                        warn!("skipping index row: {e}");
                        None
                    }
                })
                .filter(|meta| matches_filter(meta, &filter))
                .collect();

            Ok(all)
        })
    }

    fn record_access(&self, ids: &[String]) -> BoxFuture<'_, Result<()>> {
        let ids = ids.to_vec();
        Box::pin(async move {
            let mut conn = self.conn.lock().await;
            let now = Utc::now().to_rfc3339();
            let tx = conn
                .transaction()
                .map_err(|e| BrainError::Index(e.to_string()))?;
            {
                let mut stmt = tx
                    .prepare(
                        "UPDATE memories SET access_count = access_count + 1, last_accessed_at = ?1 WHERE id = ?2",
                    )
                    .map_err(|e| BrainError::Index(e.to_string()))?;
                for id in &ids {
                    stmt.execute(rusqlite::params![now, id])
                        .map_err(|e| BrainError::Index(e.to_string()))?;
                }
            }
            tx.commit().map_err(|e| BrainError::Index(e.to_string()))?;
            Ok(())
        })
    }

    fn rebuild(&self, entries: Vec<IndexEntry>, model_id: &str) -> BoxFuture<'_, Result<()>> {
        let model_id = model_id.to_string();
        Box::pin(async move {
            let mut conn = self.conn.lock().await;
            let tx = conn
                .transaction()
                .map_err(|e| BrainError::Index(e.to_string()))?;
            let keep: std::collections::HashSet<&str> =
                entries.iter().map(|e| e.metadata.id.as_str()).collect();
            for entry in &entries {
                write_row(&tx, &entry.metadata.id, &entry.embedding, &entry.metadata)?;
            }
            let existing: Vec<String> = {
                let mut stmt = tx
                    .prepare("SELECT id FROM memories")
                    .map_err(|e| BrainError::Index(e.to_string()))?;
                stmt.query_map([], |r| r.get(0))
                    .map_err(|e| BrainError::Index(e.to_string()))?
                    .collect::<rusqlite::Result<_>>()
                    .map_err(|e| BrainError::Index(e.to_string()))?
            };
            for id in existing.iter().filter(|id| !keep.contains(id.as_str())) {
                tx.execute("DELETE FROM memories WHERE id = ?1", [id])
                    .map_err(|e| BrainError::Index(e.to_string()))?;
            }
            tx.execute(
                "DELETE FROM memory_vectors WHERE id NOT IN (SELECT id FROM memories)",
                [],
            )
            .map_err(|e| BrainError::Index(e.to_string()))?;
            tx.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('model_id', ?1)",
                [&model_id],
            )
            .map_err(|e| BrainError::Index(e.to_string()))?;
            tx.commit().map_err(|e| BrainError::Index(e.to_string()))?;
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
        let index = SqliteVecIndex::open_in_memory().unwrap();
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
        let index = SqliteVecIndex::open_in_memory().unwrap();
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
        let index = SqliteVecIndex::open_in_memory().unwrap();

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
        let index = SqliteVecIndex::open_in_memory().unwrap();

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
        let index = SqliteVecIndex::open_in_memory().unwrap();

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
        let index = SqliteVecIndex::open_in_memory().unwrap();

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
        let index = SqliteVecIndex::open_in_memory().unwrap();

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
    async fn test_search_recency_breaks_ties() {
        let index = SqliteVecIndex::open_in_memory().unwrap();

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
        let index = SqliteVecIndex::open_in_memory().unwrap();
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
        let index = SqliteVecIndex::open_in_memory().unwrap();
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
        let index = SqliteVecIndex::open_in_memory().unwrap();
        index.set_model_id("text-embedding-3-small").await.unwrap();
        let stored = index.stored_model_id().await.unwrap();
        assert_eq!(stored, Some("text-embedding-3-small".to_string()));
    }

    #[tokio::test]
    async fn test_model_id_initially_none() {
        let index = SqliteVecIndex::open_in_memory().unwrap();
        let stored = index.stored_model_id().await.unwrap();
        assert_eq!(stored, None);
    }

    #[tokio::test]
    async fn test_fresh_db_is_at_latest_version() {
        let index = SqliteVecIndex::open_in_memory().unwrap();
        let conn = index.conn.lock().await;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
    }

    #[tokio::test]
    async fn test_file_db_uses_wal() {
        let dir = tempfile::tempdir().unwrap();
        let index = SqliteVecIndex::open(&dir.path().join("index.db")).unwrap();
        let conn = index.conn.lock().await;
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[tokio::test]
    async fn test_migrates_legacy_db_without_access_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
                 CREATE TABLE memories (
                     id TEXT PRIMARY KEY,
                     title TEXT NOT NULL,
                     tags TEXT NOT NULL,
                     category TEXT NOT NULL,
                     project TEXT,
                     created_at TEXT NOT NULL
                 );
                 CREATE TABLE memory_vectors (
                     id TEXT PRIMARY KEY,
                     embedding BLOB NOT NULL
                 );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO memories (id, title, tags, category, project, created_at) VALUES ('m1', 'Title', '[]', 'learnings', NULL, ?1)",
                rusqlite::params![Utc::now().to_rfc3339()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO memory_vectors (id, embedding) VALUES ('m1', ?1)",
                rusqlite::params![f32_slice_to_bytes(&[1.0, 0.0, 0.0])],
            )
            .unwrap();
        }

        let index = SqliteVecIndex::open(&path).unwrap();
        {
            let conn = index.conn.lock().await;
            let version: i64 = conn
                .query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap();
            assert_eq!(version, 1);
            for column in ["access_count", "last_accessed_at"] {
                let exists = conn
                    .prepare("SELECT 1 FROM pragma_table_info('memories') WHERE name = ?1")
                    .unwrap()
                    .exists([column])
                    .unwrap();
                assert!(exists, "missing column {column}");
            }
        }

        let all = index.list(&Filter::default()).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "m1");
        index.record_access(&["m1".to_string()]).await.unwrap();
    }

    #[tokio::test]
    async fn test_migration_1_keeps_unversioned_current_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATION_1_SQL).unwrap();
            conn.execute(
                "INSERT INTO memories (id, title, tags, category, project, created_at, access_count) VALUES ('m1', 'Title', '[]', 'learnings', NULL, ?1, 3)",
                rusqlite::params![Utc::now().to_rfc3339()],
            )
            .unwrap();
        }

        let index = SqliteVecIndex::open(&path).unwrap();
        let conn = index.conn.lock().await;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1);
        let access_count: i64 = conn
            .query_row(
                "SELECT access_count FROM memories WHERE id = 'm1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(access_count, 3);
    }

    #[tokio::test]
    async fn test_reopen_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");

        {
            let index = SqliteVecIndex::open(&path).unwrap();
            let meta = make_metadata("m1", "learnings", vec![], None);
            index.upsert("m1", &[1.0, 0.0, 0.0], &meta).await.unwrap();
        }

        let index = SqliteVecIndex::open(&path).unwrap();
        let conn = index.conn.lock().await;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1);
        drop(conn);
        let all = index.list(&Filter::default()).await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn test_rejects_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA user_version = 99;").unwrap();
        }

        match SqliteVecIndex::open(&path) {
            Ok(_) => panic!("expected an error"),
            Err(e) => assert!(e.to_string().contains("newer than this brain-mcp supports")),
        }
    }

    #[tokio::test]
    async fn test_rebuild_preserves_access_stats() {
        let index = SqliteVecIndex::open_in_memory().unwrap();
        let m1 = make_metadata("m1", "learnings", vec![], None);
        let m2 = make_metadata("m2", "learnings", vec![], None);
        index.upsert("m1", &[1.0, 0.0, 0.0], &m1).await.unwrap();
        index.upsert("m2", &[0.0, 1.0, 0.0], &m2).await.unwrap();
        index.record_access(&["m1".to_string()]).await.unwrap();
        index.record_access(&["m1".to_string()]).await.unwrap();

        let mut new_m1 = make_metadata("m1", "learnings", vec![], None);
        new_m1.title = "New Title".to_string();
        let m3 = make_metadata("m3", "learnings", vec![], None);
        index
            .rebuild(
                vec![
                    IndexEntry {
                        embedding: vec![0.0, 1.0, 0.0],
                        metadata: new_m1,
                    },
                    IndexEntry {
                        embedding: vec![0.0, 0.0, 1.0],
                        metadata: m3,
                    },
                ],
                "new-model",
            )
            .await
            .unwrap();

        let all = index.list(&Filter::default()).await.unwrap();
        let mut ids: Vec<&str> = all.iter().map(|m| m.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["m1", "m3"]);

        let conn = index.conn.lock().await;
        let (count, last, title): (i64, Option<String>, String) = conn
            .query_row(
                "SELECT access_count, last_accessed_at, title FROM memories WHERE id = 'm1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(count, 2);
        assert!(last.is_some());
        assert_eq!(title, "New Title");

        let vector_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_vectors WHERE id = 'm2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(vector_count, 0);
        let memory_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories WHERE id = 'm2'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(memory_count, 0);
        drop(conn);

        let results = index
            .search(&[0.0, 1.0, 0.0], 10, &Filter::default())
            .await
            .unwrap();
        assert_eq!(results[0].memory.id, "m1");
        assert!((results[0].score - 1.0).abs() < 1e-6);

        assert_eq!(
            index.stored_model_id().await.unwrap(),
            Some("new-model".to_string())
        );
    }

    #[tokio::test]
    async fn test_rebuild_empty_entries_empties_index() {
        let index = SqliteVecIndex::open_in_memory().unwrap();
        for i in 1..=3 {
            let id = format!("m{i}");
            let meta = make_metadata(&id, "learnings", vec![], None);
            index.upsert(&id, &[1.0, 0.0, 0.0], &meta).await.unwrap();
        }

        index.rebuild(vec![], "model").await.unwrap();

        let conn = index.conn.lock().await;
        let memory_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
            .unwrap();
        let vector_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memory_vectors", [], |r| r.get(0))
            .unwrap();
        assert_eq!(memory_count, 0);
        assert_eq!(vector_count, 0);
    }

    #[tokio::test]
    async fn test_rebuild_removes_orphan_vectors() {
        let index = SqliteVecIndex::open_in_memory().unwrap();
        {
            let conn = index.conn.lock().await;
            conn.execute(
                "INSERT INTO memory_vectors (id, embedding) VALUES ('orphan', ?1)",
                rusqlite::params![f32_slice_to_bytes(&[1.0, 0.0, 0.0])],
            )
            .unwrap();
        }

        index.rebuild(vec![], "model").await.unwrap();

        let conn = index.conn.lock().await;
        let vector_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memory_vectors", [], |r| r.get(0))
            .unwrap();
        assert_eq!(vector_count, 0);
    }

    #[tokio::test]
    async fn test_bad_created_at_row_is_skipped() {
        let index = SqliteVecIndex::open_in_memory().unwrap();
        let m1 = make_metadata("m1", "learnings", vec![], None);
        index.upsert("m1", &[1.0, 0.0, 0.0], &m1).await.unwrap();
        {
            let conn = index.conn.lock().await;
            conn.execute(
                "UPDATE memories SET created_at = 'garbage' WHERE id = 'm1'",
                [],
            )
            .unwrap();
        }
        let m2 = make_metadata("m2", "learnings", vec![], None);
        index.upsert("m2", &[1.0, 0.0, 0.0], &m2).await.unwrap();

        let all = index.list(&Filter::default()).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "m2");

        let results = index
            .search(&[1.0, 0.0, 0.0], 10, &Filter::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory.id, "m2");
    }

    #[tokio::test]
    async fn test_record_access_unknown_id_is_noop() {
        let index = SqliteVecIndex::open_in_memory().unwrap();
        index.record_access(&["nope".to_string()]).await.unwrap();
    }
}
