use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Memory {
    pub id: String,
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    pub category: String,
    pub project: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(skip)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Metadata {
    pub id: String,
    pub title: String,
    pub tags: Vec<String>,
    pub category: String,
    pub project: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<&Memory> for Metadata {
    fn from(m: &Memory) -> Self {
        Metadata {
            id: m.id.clone(),
            title: m.title.clone(),
            tags: m.tags.clone(),
            category: m.category.clone(),
            project: m.project.clone(),
            created_at: m.created_at,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub memory: Memory,
    pub score: f32,
}

#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub embedding: Vec<f32>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    pub tags: Option<Vec<String>>,
    pub category: Option<String>,
    pub project: Option<String>,
    pub since: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CallContext {
    pub client: Option<String>,
    pub session_id: Option<String>,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoggedHit {
    pub id: String,
    pub rank: usize,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchLogEntry {
    pub ts: DateTime<Utc>,
    pub ctx: CallContext,
    pub project: Option<String>,
    pub query: String,
    pub filter: Filter,
    pub limit: usize,
    pub results: Vec<LoggedHit>,
    pub latency_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreOutcome {
    Stored,
    DuplicateRejected,
    IdConflict,
    Error,
}

impl StoreOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            StoreOutcome::Stored => "stored",
            StoreOutcome::DuplicateRejected => "duplicate_rejected",
            StoreOutcome::IdConflict => "id_conflict",
            StoreOutcome::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "stored" => Some(StoreOutcome::Stored),
            "duplicate_rejected" => Some(StoreOutcome::DuplicateRejected),
            "id_conflict" => Some(StoreOutcome::IdConflict),
            "error" => Some(StoreOutcome::Error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoreLogEntry {
    pub ts: DateTime<Utc>,
    pub ctx: CallContext,
    pub memory_id: Option<String>,
    pub outcome: StoreOutcome,
    pub forced: bool,
    pub neighbors: Vec<LoggedHit>,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn metadata_from_memory() {
        let memory = Memory {
            id: "20260328-test".into(),
            title: "Test".into(),
            content: "body".into(),
            tags: vec!["a".into()],
            category: "learnings".into(),
            project: Some("brain-mcp".into()),
            created_at: Utc::now(),
            updated_at: None,
            extra: BTreeMap::new(),
        };
        let meta = Metadata::from(&memory);
        assert_eq!(meta.id, memory.id);
        assert_eq!(meta.title, memory.title);
        assert_eq!(meta.tags, memory.tags);
    }

    #[test]
    fn filter_default_is_empty() {
        let f = Filter::default();
        assert!(f.tags.is_none());
        assert!(f.category.is_none());
        assert!(f.project.is_none());
        assert!(f.since.is_none());
    }
}
