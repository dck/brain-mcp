use std::collections::BTreeMap;

use brain_core::error::{BrainError, Result};
use brain_core::model::Memory;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Frontmatter {
    id: String,
    title: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    updated_at: Option<DateTime<Utc>>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_yaml::Value>,
}

impl From<&Memory> for Frontmatter {
    fn from(m: &Memory) -> Self {
        Frontmatter {
            id: m.id.clone(),
            title: m.title.clone(),
            category: m.category.clone(),
            tags: m.tags.clone(),
            project: m.project.clone(),
            created_at: m.created_at,
            updated_at: m.updated_at,
            extra: m.extra.clone(),
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
            updated_at: self.updated_at,
            extra: self.extra,
        }
    }
}

pub(crate) fn split_frontmatter(text: &str) -> Result<(&str, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text).trim_start();
    let mut lines = text.split_inclusive('\n');
    let first = lines
        .next()
        .ok_or_else(|| BrainError::Vault("missing frontmatter delimiter".to_string()))?;
    if first.trim_end() != "---" {
        return Err(BrainError::Vault(
            "missing frontmatter delimiter".to_string(),
        ));
    }
    let yaml_start = first.len();
    let mut offset = yaml_start;
    for line in lines {
        if line.trim_end() == "---" {
            return Ok((&text[yaml_start..offset], &text[offset + line.len()..]));
        }
        offset += line.len();
    }
    Err(BrainError::Vault(
        "missing closing frontmatter delimiter".to_string(),
    ))
}

/// Parse a markdown file with YAML frontmatter delimited by `---`.
pub fn parse_markdown(text: &str) -> Result<Memory> {
    let (yaml, body) = split_frontmatter(text)?;
    let content = body.trim_start_matches(['\r', '\n']);
    let fm: Frontmatter = serde_yaml::from_str(yaml)
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
            updated_at: None,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn test_frontmatter_roundtrip() {
        let mut memory = sample_memory();
        memory.updated_at = Some(Utc.with_ymd_and_hms(2026, 4, 2, 10, 0, 0).unwrap());
        memory.extra.insert(
            "aliases".to_string(),
            serde_yaml::Value::Sequence(vec![serde_yaml::Value::String("x".into())]),
        );

        let md = to_markdown(&memory);
        let parsed = parse_markdown(&md).unwrap();

        assert_eq!(parsed.id, memory.id);
        assert_eq!(parsed.title, memory.title);
        assert_eq!(parsed.content, memory.content);
        assert_eq!(parsed.tags, memory.tags);
        assert_eq!(parsed.category, memory.category);
        assert_eq!(parsed.project, memory.project);
        assert_eq!(parsed.created_at, memory.created_at);
        assert_eq!(parsed.updated_at, memory.updated_at);
        assert_eq!(parsed.extra, memory.extra);
    }

    #[test]
    fn test_to_markdown_golden() {
        let mut memory = sample_memory();
        memory.updated_at = Some(Utc.with_ymd_and_hms(2026, 4, 2, 10, 0, 0).unwrap());
        memory.extra.insert(
            "aliases".to_string(),
            serde_yaml::Value::Sequence(vec![serde_yaml::Value::String("x".into())]),
        );

        let md = to_markdown(&memory);
        let expected = "---\n\
id: 20260328-deploy-new-app\n\
title: Deploy new app\n\
category: procedures\n\
tags:\n\
- deploy\n\
- terraform\n\
project: maestro\n\
created_at: 2026-03-28T14:30:00Z\n\
updated_at: 2026-04-02T10:00:00Z\n\
aliases:\n\
- x\n\
---\n\
\n\
Steps to deploy a new app.";

        assert_eq!(md, expected);
    }

    #[test]
    fn test_hostile_titles_roundtrip() {
        let titles = [
            "He said \"hi\"",
            "O'Brien",
            "a: b",
            "#tag",
            "- dash",
            "[x]",
            "{y}",
            "back\\slash \\b",
            "null",
            "123",
            "~",
            "line1\nline2",
            "a\n---\nb",
        ];

        for title in titles {
            let mut memory = sample_memory();
            memory.title = title.to_string();
            memory.content = "Body.".to_string();

            let md = to_markdown(&memory);
            let parsed = parse_markdown(&md).unwrap();

            assert_eq!(parsed.title, title);
            assert_eq!(parsed.content, "Body.");
        }
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
    fn test_parse_missing_category_is_tolerated() {
        let md = r#"---
title: "Rust Design Patterns book"
tags:
  - rust
created_at: "2026-04-28T11:21:40Z"
id: "20260428-rust-design-patterns-book"
---

Body text."#;

        let memory = parse_markdown(md).unwrap();
        assert_eq!(memory.category, "");
        assert_eq!(memory.id, "20260428-rust-design-patterns-book");
        assert_eq!(memory.content, "Body text.");
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

    #[test]
    fn test_parse_style_a_template_quoted() {
        let md = r#"---
title: "Deploy new app"
id: "20260405-deploy-new-app"
tags:
  - deploy
  - terraform
created_at: "2026-04-05T15:09:20.438734+00:00"
category: procedures
---

Steps."#;

        let memory = parse_markdown(md).unwrap();
        assert_eq!(memory.title, "Deploy new app");
        assert_eq!(memory.id, "20260405-deploy-new-app");
        assert_eq!(memory.tags, vec!["deploy", "terraform"]);
        assert_eq!(memory.category, "procedures");
        let expected = Utc.with_ymd_and_hms(2026, 4, 5, 15, 9, 20).unwrap()
            + chrono::Duration::microseconds(438734);
        assert_eq!(
            memory.created_at.timestamp_micros(),
            expected.timestamp_micros()
        );
    }

    #[test]
    fn test_parse_style_b_single_quoted_title() {
        let md = r#"---
title: 'Maestro Alloy; Loki "discarded" counters'
id: "20260412-maestro-alloy"
tags:
  - maestro
created_at: "2026-04-12T09:00:00+00:00"
category: learnings
---

Body."#;

        let memory = parse_markdown(md).unwrap();
        assert!(memory.title.contains("\"discarded\""));
    }

    #[test]
    fn test_parse_style_c_serde_unquoted() {
        let md = r#"---
title: Serde style memory
tags:
- x
created_at: 2026-06-03T12:27:49.441251Z
id: 20260603-serde-style-memory
category: feedback
---

Body."#;

        let memory = parse_markdown(md).unwrap();
        assert_eq!(memory.title, "Serde style memory");
        assert_eq!(memory.tags, vec!["x"]);
        assert_eq!(memory.id, "20260603-serde-style-memory");
        assert_eq!(memory.category, "feedback");
    }

    #[test]
    fn test_parse_body_with_horizontal_rules() {
        let md = r#"---
title: "Has rules"
id: "20260501-has-rules"
created_at: "2026-05-01T00:00:00Z"
category: learnings
---

Above.

---

Below."#;

        let memory = parse_markdown(md).unwrap();
        assert!(memory.content.contains("---"));
        assert!(memory.content.contains("Above."));
        assert!(memory.content.contains("Below."));
    }

    #[test]
    fn test_parse_value_containing_dashes() {
        let md = r#"---
title: "a --- b"
id: "20260501-dashes"
created_at: "2026-05-01T00:00:00Z"
category: learnings
---

Body."#;

        let memory = parse_markdown(md).unwrap();
        assert_eq!(memory.title, "a --- b");
    }

    #[test]
    fn test_parse_crlf() {
        let md = "---\r\ntitle: \"CRLF test\"\r\nid: \"20260501-crlf\"\r\ncreated_at: \"2026-05-01T00:00:00Z\"\r\ncategory: learnings\r\n---\r\n\r\nBody.";

        let memory = parse_markdown(md).unwrap();
        assert_eq!(memory.title, "CRLF test");
        assert_eq!(memory.content, "Body.");
    }

    #[test]
    fn test_parse_unknown_keys_preserved() {
        let md = r#"---
title: "Has extras"
id: "20260501-has-extras"
created_at: "2026-05-01T00:00:00Z"
category: learnings
aliases: [foo]
cssclasses: bar
---

Body."#;

        let memory = parse_markdown(md).unwrap();
        let md2 = to_markdown(&memory);
        let memory2 = parse_markdown(&md2).unwrap();

        assert_eq!(
            memory2.extra.get("aliases"),
            Some(&serde_yaml::Value::Sequence(vec![
                serde_yaml::Value::String("foo".into())
            ]))
        );
        assert_eq!(
            memory2.extra.get("cssclasses"),
            Some(&serde_yaml::Value::String("bar".into()))
        );
    }

    #[test]
    fn test_parse_missing_tags_defaults_empty() {
        let md = r#"---
title: "No tags"
id: "20260501-no-tags"
created_at: "2026-05-01T00:00:00Z"
category: learnings
---

Body."#;

        let memory = parse_markdown(md).unwrap();
        assert_eq!(memory.tags, Vec::<String>::new());
    }
}
