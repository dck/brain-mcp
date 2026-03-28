use brain_core::error::{BrainError, Result};
use brain_core::model::Memory;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Frontmatter {
    title: String,
    tags: Vec<String>,
    created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    category: String,
    id: String,
}

impl From<&Memory> for Frontmatter {
    fn from(m: &Memory) -> Self {
        Frontmatter {
            title: m.title.clone(),
            tags: m.tags.clone(),
            created_at: m.created_at,
            project: m.project.clone(),
            category: m.category.clone(),
            id: m.id.clone(),
        }
    }
}

impl Frontmatter {
    fn into_memory(self, content: String) -> Memory {
        Memory {
            id: self.id,
            title: self.title,
            content,
            tags: self.tags,
            category: self.category,
            project: self.project,
            created_at: self.created_at,
        }
    }
}

/// Parse a markdown file with YAML frontmatter delimited by `---`.
pub fn parse_markdown(text: &str) -> Result<Memory> {
    let text = text.trim_start();

    if !text.starts_with("---") {
        return Err(BrainError::Vault(
            "missing frontmatter delimiter".to_string(),
        ));
    }

    let after_first = &text[3..];
    let closing = after_first
        .find("---")
        .ok_or_else(|| BrainError::Vault("missing closing frontmatter delimiter".to_string()))?;

    let yaml_str = &after_first[..closing];
    let body = &after_first[closing + 3..];
    let content = body.trim_start_matches('\n').trim_start_matches('\r');

    let fm: Frontmatter = serde_yaml::from_str(yaml_str)
        .map_err(|e| BrainError::Vault(format!("failed to parse frontmatter: {e}")))?;

    Ok(fm.into_memory(content.to_string()))
}

/// Serialize a Memory to markdown with YAML frontmatter.
pub fn to_markdown(memory: &Memory) -> String {
    let fm = Frontmatter::from(memory);
    let yaml = serde_yaml::to_string(&fm).expect("frontmatter serialization should not fail");
    format!("---\n{yaml}---\n\n{}", memory.content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_memory() -> Memory {
        Memory {
            id: "20260328-deploy-new-app".into(),
            title: "Deploy new app".into(),
            content: "Steps to deploy a new app.".into(),
            tags: vec!["deploy".into(), "terraform".into()],
            category: "procedures".into(),
            project: Some("maestro".into()),
            created_at: Utc.with_ymd_and_hms(2026, 3, 28, 14, 30, 0).unwrap(),
        }
    }

    #[test]
    fn test_frontmatter_roundtrip() {
        let memory = sample_memory();
        let md = to_markdown(&memory);
        let parsed = parse_markdown(&md).unwrap();

        assert_eq!(parsed.id, memory.id);
        assert_eq!(parsed.title, memory.title);
        assert_eq!(parsed.content, memory.content);
        assert_eq!(parsed.tags, memory.tags);
        assert_eq!(parsed.category, memory.category);
        assert_eq!(parsed.project, memory.project);
        assert_eq!(parsed.created_at, memory.created_at);
    }

    #[test]
    fn test_parse_markdown_basic() {
        let md = r#"---
title: "Deploy new app"
tags:
  - deploy
  - terraform
created_at: "2026-03-28T14:30:00Z"
project: maestro
category: procedures
id: "20260328-deploy-new-app"
---

Steps to deploy a new app."#;

        let memory = parse_markdown(md).unwrap();
        assert_eq!(memory.title, "Deploy new app");
        assert_eq!(memory.id, "20260328-deploy-new-app");
        assert_eq!(memory.tags, vec!["deploy", "terraform"]);
        assert_eq!(memory.project, Some("maestro".into()));
        assert_eq!(memory.content, "Steps to deploy a new app.");
    }

    #[test]
    fn test_parse_missing_frontmatter() {
        let md = "No frontmatter here.";
        assert!(parse_markdown(md).is_err());
    }

    #[test]
    fn test_parse_missing_closing_delimiter() {
        let md = "---\ntitle: foo\n";
        assert!(parse_markdown(md).is_err());
    }

    #[test]
    fn test_project_none_omitted() {
        let mut memory = sample_memory();
        memory.project = None;
        let md = to_markdown(&memory);
        assert!(!md.contains("project"));
    }
}
