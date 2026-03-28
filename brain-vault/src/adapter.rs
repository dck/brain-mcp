use std::path::PathBuf;

use brain_core::error::{BrainError, Result};
use brain_core::model::Memory;
use brain_core::ports::{BoxFuture, VaultPort};
use tracing::warn;

use crate::frontmatter::{parse_markdown, to_markdown};
use crate::template::{apply_template, load_template};

pub struct VaultAdapter {
    vault_path: PathBuf,
    templates_dir: String,
}

impl VaultAdapter {
    pub fn new(vault_path: PathBuf, templates_dir: String) -> Self {
        Self {
            vault_path,
            templates_dir,
        }
    }

    fn memory_path(&self, category: &str, id: &str) -> PathBuf {
        self.vault_path.join(category).join(format!("{id}.md"))
    }

    /// Scan category directories for a file named `{id}.md`.
    fn find_file(&self, id: &str) -> Option<PathBuf> {
        let filename = format!("{id}.md");
        let entries = std::fs::read_dir(&self.vault_path).ok()?;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            // Skip templates dir and dotdirs
            let dir_name = entry.file_name();
            let dir_name = dir_name.to_string_lossy();
            if dir_name.starts_with('.') || dir_name.starts_with('_') {
                continue;
            }
            let candidate = path.join(&filename);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }
}

impl VaultPort for VaultAdapter {
    fn write(&self, memory: &Memory) -> BoxFuture<'_, Result<()>> {
        let memory = memory.clone();
        Box::pin(async move {
            let content =
                match load_template(&self.vault_path, &self.templates_dir, &memory.category) {
                    Some(template) => apply_template(&template, &memory),
                    None => to_markdown(&memory),
                };

            let path = self.memory_path(&memory.category, &memory.id);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| BrainError::Vault(format!("failed to create directory: {e}")))?;
            }

            std::fs::write(&path, content)
                .map_err(|e| BrainError::Vault(format!("failed to write file: {e}")))?;

            Ok(())
        })
    }

    fn read(&self, id: &str) -> BoxFuture<'_, Result<Option<Memory>>> {
        let id = id.to_string();
        Box::pin(async move {
            let path = match self.find_file(&id) {
                Some(p) => p,
                None => return Ok(None),
            };

            let text = std::fs::read_to_string(&path)
                .map_err(|e| BrainError::Vault(format!("failed to read file: {e}")))?;

            let memory = parse_markdown(&text)?;
            Ok(Some(memory))
        })
    }

    fn delete(&self, id: &str) -> BoxFuture<'_, Result<()>> {
        let id = id.to_string();
        Box::pin(async move {
            if let Some(path) = self.find_file(&id) {
                std::fs::remove_file(&path)
                    .map_err(|e| BrainError::Vault(format!("failed to delete file: {e}")))?;
            }
            Ok(())
        })
    }

    fn list_all(&self) -> BoxFuture<'_, Result<Vec<Memory>>> {
        Box::pin(async move {
            let mut memories = Vec::new();

            let entries = std::fs::read_dir(&self.vault_path)
                .map_err(|e| BrainError::Vault(format!("failed to read vault directory: {e}")))?;

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let dir_name = entry.file_name();
                let dir_name = dir_name.to_string_lossy();
                if dir_name.starts_with('.') || dir_name.starts_with('_') {
                    continue;
                }

                let sub_entries = match std::fs::read_dir(&path) {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                for sub_entry in sub_entries.flatten() {
                    let file_path = sub_entry.path();
                    if file_path.extension().is_some_and(|ext| ext == "md") {
                        match std::fs::read_to_string(&file_path) {
                            Ok(text) => match parse_markdown(&text) {
                                Ok(memory) => memories.push(memory),
                                Err(e) => {
                                    warn!("skipping {}: {e}", file_path.display());
                                }
                            },
                            Err(e) => {
                                warn!("failed to read {}: {e}", file_path.display());
                            }
                        }
                    }
                }
            }

            Ok(memories)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;

    fn sample_memory(id: &str, category: &str) -> Memory {
        Memory {
            id: id.into(),
            title: format!("Title for {id}"),
            content: "Test content.".into(),
            tags: vec!["tag1".into(), "tag2".into()],
            category: category.into(),
            project: Some("testproject".into()),
            created_at: Utc.with_ymd_and_hms(2026, 3, 28, 14, 30, 0).unwrap(),
        }
    }

    #[tokio::test]
    async fn test_write_creates_file() {
        let dir = tempdir().unwrap();
        let adapter = VaultAdapter::new(dir.path().to_path_buf(), "_templates".into());

        let memory = sample_memory("20260328-test", "procedures");
        adapter.write(&memory).await.unwrap();

        let path = dir.path().join("procedures").join("20260328-test.md");
        assert!(path.exists());
    }

    #[tokio::test]
    async fn test_write_read_roundtrip() {
        let dir = tempdir().unwrap();
        let adapter = VaultAdapter::new(dir.path().to_path_buf(), "_templates".into());

        let memory = sample_memory("20260328-roundtrip", "learnings");
        adapter.write(&memory).await.unwrap();

        let read_back = adapter.read("20260328-roundtrip").await.unwrap().unwrap();
        assert_eq!(read_back.id, memory.id);
        assert_eq!(read_back.title, memory.title);
        assert_eq!(read_back.content, memory.content);
        assert_eq!(read_back.tags, memory.tags);
        assert_eq!(read_back.category, memory.category);
        assert_eq!(read_back.project, memory.project);
        assert_eq!(read_back.created_at, memory.created_at);
    }

    #[tokio::test]
    async fn test_read_not_found() {
        let dir = tempdir().unwrap();
        let adapter = VaultAdapter::new(dir.path().to_path_buf(), "_templates".into());

        let result = adapter.read("nonexistent-id").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_delete_removes_file() {
        let dir = tempdir().unwrap();
        let adapter = VaultAdapter::new(dir.path().to_path_buf(), "_templates".into());

        let memory = sample_memory("20260328-delete-me", "procedures");
        adapter.write(&memory).await.unwrap();

        adapter.delete("20260328-delete-me").await.unwrap();

        let path = dir.path().join("procedures").join("20260328-delete-me.md");
        assert!(!path.exists());

        let result = adapter.read("20260328-delete-me").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_delete_nonexistent_is_ok() {
        let dir = tempdir().unwrap();
        let adapter = VaultAdapter::new(dir.path().to_path_buf(), "_templates".into());

        // Should not error
        adapter.delete("does-not-exist").await.unwrap();
    }

    #[tokio::test]
    async fn test_list_all_finds_memories() {
        let dir = tempdir().unwrap();
        let adapter = VaultAdapter::new(dir.path().to_path_buf(), "_templates".into());

        let m1 = sample_memory("20260328-one", "procedures");
        let m2 = sample_memory("20260328-two", "learnings");
        let m3 = sample_memory("20260328-three", "decisions");

        adapter.write(&m1).await.unwrap();
        adapter.write(&m2).await.unwrap();
        adapter.write(&m3).await.unwrap();

        let all = adapter.list_all().await.unwrap();
        assert_eq!(all.len(), 3);

        let ids: Vec<&str> = all.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"20260328-one"));
        assert!(ids.contains(&"20260328-two"));
        assert!(ids.contains(&"20260328-three"));
    }

    #[tokio::test]
    async fn test_list_all_skips_templates() {
        let dir = tempdir().unwrap();
        let adapter = VaultAdapter::new(dir.path().to_path_buf(), "_templates".into());

        // Write a real memory
        let memory = sample_memory("20260328-real", "procedures");
        adapter.write(&memory).await.unwrap();

        // Create a _templates dir with a markdown file
        let tpl_dir = dir.path().join("_templates");
        std::fs::create_dir_all(&tpl_dir).unwrap();
        std::fs::write(tpl_dir.join("procedures.md"), "# {{title}}\n{{content}}").unwrap();

        let all = adapter.list_all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "20260328-real");
    }

    #[tokio::test]
    async fn test_list_all_skips_unparseable() {
        let dir = tempdir().unwrap();
        let adapter = VaultAdapter::new(dir.path().to_path_buf(), "_templates".into());

        // Write a real memory
        let memory = sample_memory("20260328-good", "procedures");
        adapter.write(&memory).await.unwrap();

        // Write a file without proper frontmatter
        let bad_dir = dir.path().join("notes");
        std::fs::create_dir_all(&bad_dir).unwrap();
        std::fs::write(bad_dir.join("random.md"), "Just some notes.").unwrap();

        let all = adapter.list_all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "20260328-good");
    }

    #[tokio::test]
    async fn test_template_applied_when_exists() {
        let dir = tempdir().unwrap();
        let adapter = VaultAdapter::new(dir.path().to_path_buf(), "_templates".into());

        // Create a template
        let tpl_dir = dir.path().join("_templates");
        std::fs::create_dir_all(&tpl_dir).unwrap();
        std::fs::write(
            tpl_dir.join("procedures.md"),
            "---\ntitle: \"{{title}}\"\nid: \"{{id}}\"\ntags:\n{{tags}}\ncreated_at: \"{{created_at}}\"\ncategory: {{category}}\n---\n\n# {{title}}\n\n{{content}}",
        )
        .unwrap();

        let memory = sample_memory("20260328-templated", "procedures");
        adapter.write(&memory).await.unwrap();

        let path = dir.path().join("procedures").join("20260328-templated.md");
        let content = std::fs::read_to_string(path).unwrap();

        assert!(content.contains("# Title for 20260328-templated"));
        assert!(content.contains("  - tag1"));
        assert!(content.contains("  - tag2"));
    }

    #[tokio::test]
    async fn test_no_template_uses_plain_format() {
        let dir = tempdir().unwrap();
        let adapter = VaultAdapter::new(dir.path().to_path_buf(), "_templates".into());

        let memory = sample_memory("20260328-plain", "learnings");
        adapter.write(&memory).await.unwrap();

        let path = dir.path().join("learnings").join("20260328-plain.md");
        let content = std::fs::read_to_string(path).unwrap();

        // Should have standard frontmatter format
        assert!(content.starts_with("---\n"));
        assert!(content.contains("title: Title for 20260328-plain"));
        assert!(content.contains("Test content."));
    }
}
