use brain_core::model::Memory;
use std::path::Path;

/// Load a template file for the given category from the templates directory.
/// Returns None if the template file doesn't exist.
pub fn load_template(vault_path: &Path, templates_dir: &str, category: &str) -> Option<String> {
    let path = vault_path
        .join(templates_dir)
        .join(format!("{category}.md"));
    std::fs::read_to_string(path).ok()
}

/// Apply simple `{{placeholder}}` substitutions to a template string.
pub fn apply_template(template: &str, memory: &Memory) -> String {
    let tags_yaml = memory
        .tags
        .iter()
        .map(|t| format!("  - {t}"))
        .collect::<Vec<_>>()
        .join("\n");

    let project_str = memory.project.as_deref().unwrap_or("");

    template
        .replace("{{title}}", &memory.title)
        .replace("{{id}}", &memory.id)
        .replace("{{created_at}}", &memory.created_at.to_rfc3339())
        .replace("{{project}}", project_str)
        .replace("{{category}}", &memory.category)
        .replace("{{content}}", &memory.content)
        .replace("{{tags}}", &tags_yaml)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn sample_memory() -> Memory {
        Memory {
            id: "20260328-test".into(),
            title: "Test Memory".into(),
            content: "Some content.".into(),
            tags: vec!["tag1".into(), "tag2".into()],
            category: "procedures".into(),
            project: Some("myproject".into()),
            created_at: Utc.with_ymd_and_hms(2026, 3, 28, 14, 30, 0).unwrap(),
        }
    }

    #[test]
    fn test_apply_template_substitutions() {
        let template =
            "# {{title}}\nID: {{id}}\nProject: {{project}}\ntags:\n{{tags}}\n\n{{content}}";
        let memory = sample_memory();
        let result = apply_template(template, &memory);

        assert!(result.contains("# Test Memory"));
        assert!(result.contains("ID: 20260328-test"));
        assert!(result.contains("Project: myproject"));
        assert!(result.contains("  - tag1"));
        assert!(result.contains("  - tag2"));
        assert!(result.contains("Some content."));
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
