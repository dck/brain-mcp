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

#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub tags: Option<Vec<String>>,
    pub category: Option<String>,
    pub project: Option<String>,
    pub since: Option<DateTime<Utc>>,
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
