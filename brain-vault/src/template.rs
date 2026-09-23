use brain_core::model::Memory;
use std::path::Path;
use tracing::warn;

use crate::frontmatter::split_frontmatter;

/// Load a template file for the given category from the templates directory,
/// returning only its body (frontmatter, if any, is stripped).
/// Returns None if the template file doesn't exist.
pub fn load_template(vault_path: &Path, templates_dir: &str, category: &str) -> Option<String> {
    let path = vault_path
        .join(templates_dir)
        .join(format!("{category}.md"));
    let text = std::fs::read_to_string(path).ok()?;
    match split_frontmatter(&text) {
        Ok((_, body)) => Some(body.trim_start_matches(['\r', '\n']).to_string()),
        Err(_) => Some(text),
    }
}

/// Apply `{{placeholder}}` substitutions to a template body, returning the memory content.
pub fn apply_template(body: &str, memory: &Memory) -> String {
    if !body.contains("{{content}}") {
        warn!(
            "template for category '{}' has no {{{{content}}}} placeholder; writing content without template",
            memory.category
        );
        return memory.content.clone();
    }
    body.replace("{{title}}", &memory.title)
        .replace("{{id}}", &memory.id)
        .replace("{{created_at}}", &memory.created_at.to_rfc3339())
        .replace("{{project}}", memory.project.as_deref().unwrap_or(""))
        .replace("{{category}}", &memory.category)
        .replace("{{tags}}", &memory.tags.join(", "))
        .replace("{{content}}", &memory.content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::collections::BTreeMap;

    fn sample_memory() -> Memory {
        Memory {
            id: "20260328-test".into(),
            title: "Test Memory".into(),
            content: "Some content.".into(),
            tags: vec!["tag1".into(), "tag2".into()],
            category: "procedures".into(),
            project: Some("myproject".into()),
            created_at: Utc.with_ymd_and_hms(2026, 3, 28, 14, 30, 0).unwrap(),
            updated_at: None,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn test_apply_template_substitutions() {
        let body = "# {{title}}\nID: {{id}}\nProject: {{project}}\ntags: {{tags}}\n\n{{content}}";
        let memory = sample_memory();
        let result = apply_template(body, &memory);

        assert!(result.contains("# Test Memory"));
        assert!(result.contains("ID: 20260328-test"));
        assert!(result.contains("Project: myproject"));
        assert!(result.contains("tags: tag1, tag2"));
        assert!(result.contains("Some content."));
        assert!(!result.contains("---"));
    }

    #[test]
    fn test_apply_template_content_not_expanded() {
        let body = "# {{title}}\n\n{{content}}";
        let mut memory = sample_memory();
        memory.content = "See {{title}} for details.".into();

        let result = apply_template(body, &memory);
        assert!(result.contains("See {{title}} for details."));
    }

    #[test]
    fn test_apply_template_without_content_placeholder_returns_content() {
        let body = "# {{title}}\nNo content placeholder here.";
        let memory = sample_memory();

        let result = apply_template(body, &memory);
        assert_eq!(result, memory.content);
    }

    #[test]
    fn test_load_template_strips_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let tpl_dir = dir.path().join("_templates");
        std::fs::create_dir_all(&tpl_dir).unwrap();
        std::fs::write(
            tpl_dir.join("learnings.md"),
            "---\ntitle: \"{{title}}\"\nid: \"{{id}}\"\ntags:\n{{tags}}\ncreated_at: \"{{created_at}}\"\ncategory: {{category}}\n---\n\n## What I Learned\n\n{{content}}\n\n## Why It Matters\n\n## References\n\n",
        )
        .unwrap();

        let result = load_template(dir.path(), "_templates", "learnings").unwrap();
        assert!(result.starts_with("## What I Learned"));
        assert!(!result.contains("title:"));
    }

    #[test]
    fn test_load_template_missing() {
        let result = load_template(Path::new("/nonexistent"), "_templates", "procedures");
        assert!(result.is_none());
    }

    #[test]
    fn test_load_template_exists() {
        let dir = tempfile::tempdir().unwrap();
        let tpl_dir = dir.path().join("_templates");
        std::fs::create_dir_all(&tpl_dir).unwrap();
        std::fs::write(tpl_dir.join("procedures.md"), "# {{title}}").unwrap();

        let result = load_template(dir.path(), "_templates", "procedures");
        assert_eq!(result, Some("# {{title}}".to_string()));
    }
}
