use std::path::PathBuf;

use brain_core::model::Memory;
use brain_core::ports::VaultPort;
use brain_vault::VaultAdapter;

use super::load_config;

pub async fn run(
    config_path: Option<PathBuf>,
    project: Option<String>,
    limit: usize,
) -> anyhow::Result<()> {
    let config = load_config(config_path)?;
    let vault = VaultAdapter::new(
        PathBuf::from(&config.vault.path),
        config.vault.templates_dir.clone(),
    );

    let mut memories = vault.list_all().await?;
    if let Some(ref project) = project {
        memories.retain(|m| m.project.as_deref().is_none_or(|p| p == project));
    }
    memories.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    memories.truncate(limit);

    if memories.is_empty() {
        return Ok(());
    }

    let scope = project
        .map(|p| format!(" (project: {p})"))
        .unwrap_or_default();
    println!("# Persistent memory index{scope}");
    println!(
        "You have stored memories relevant to this work. Before acting on a task, \
         retrieve full details with the brain-mcp memory_search tool (semantic query)."
    );
    println!();
    for memory in &memories {
        println!("{}", index_line(memory));
    }

    Ok(())
}

fn index_line(memory: &Memory) -> String {
    let mut line = format!(
        "- {} [{}] {}",
        memory.created_at.format("%Y-%m-%d"),
        memory.category,
        memory.title
    );
    if let Some(summary) = first_content_line(&memory.content) {
        line.push_str(" — ");
        line.push_str(&summary);
    }
    if !memory.tags.is_empty() {
        line.push_str(&format!(" (tags: {})", memory.tags.join(", ")));
    }
    line
}

fn first_content_line(content: &str) -> Option<String> {
    let line = content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))?;
    let mut summary: String = line.chars().take(120).collect();
    if line.chars().count() > 120 {
        summary.push('…');
    }
    Some(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_memory(content: &str) -> Memory {
        Memory {
            id: "20260712-test".into(),
            title: "Test Memory".into(),
            content: content.into(),
            tags: vec!["rust".into(), "cli".into()],
            category: "learnings".into(),
            project: None,
            created_at: "2026-07-12T10:00:00Z"
                .parse()
                .unwrap_or_else(|_| Utc::now()),
        }
    }

    #[test]
    fn index_line_includes_date_category_title_summary_tags() {
        let memory = make_memory("## Context\n\nThe actual insight body.\nMore detail.");
        let line = index_line(&memory);
        assert_eq!(
            line,
            "- 2026-07-12 [learnings] Test Memory — The actual insight body. (tags: rust, cli)"
        );
    }

    #[test]
    fn first_content_line_skips_headings_and_blanks() {
        assert_eq!(
            first_content_line("## Heading\n\n   \nreal text"),
            Some("real text".into())
        );
        assert_eq!(first_content_line("## Only headings\n### Another"), None);
    }

    #[test]
    fn first_content_line_truncates_long_lines() {
        let long = "x".repeat(200);
        let summary = first_content_line(&long).unwrap();
        assert_eq!(summary.chars().count(), 121);
        assert!(summary.ends_with('…'));
    }
}
