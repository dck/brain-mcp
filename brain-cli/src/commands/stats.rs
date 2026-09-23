use std::path::PathBuf;

use chrono::Utc;

use brain_core::model::Metadata;
use brain_core::ports::{LogPort, VaultPort};
use brain_core::stats::{Stats, StatsInput, compute};
use brain_index::adapter::SqliteVecIndex;
use brain_vault::VaultAdapter;

use super::load_config;

pub async fn run(
    config_path: Option<PathBuf>,
    days: u32,
    weak: f32,
    top: usize,
    json: bool,
) -> anyhow::Result<()> {
    let config = load_config(config_path)?;
    let index_path = PathBuf::from(&config.index.path);
    if !index_path.exists() {
        anyhow::bail!(
            "Index not found at {}. Start the server or run 'brain-mcp reindex' first.",
            index_path.display()
        );
    }
    let index = SqliteVecIndex::open(&index_path)?;
    let vault = VaultAdapter::new(
        PathBuf::from(&config.vault.path),
        config.vault.templates_dir.clone(),
    );
    let until = Utc::now();
    let since = until - chrono::Duration::days(i64::from(days));
    let searches = index.searches_since(since).await?;
    let stores = index.stores_since(since).await?;
    let memories: Vec<Metadata> = vault.list_all().await?.iter().map(Metadata::from).collect();
    let stats = compute(&StatsInput {
        searches: &searches,
        stores: &stores,
        memories: &memories,
        since,
        until,
        days,
        weak_threshold: weak,
        top,
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&stats)?);
    } else {
        print!("{}", render(&stats));
    }
    Ok(())
}

fn render(stats: &Stats) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "brain-mcp stats — last {} days ({} → {} UTC)\n\n",
        stats.days,
        stats.since.format("%Y-%m-%d"),
        stats.until.format("%Y-%m-%d"),
    ));

    if stats.searches.total == 0 && stats.stores.total == 0 {
        out.push_str("No searches or stores logged in this window.\n\n");
    } else {
        out.push_str(&format!(
            "Searches      {}  ({:.1}/day, {} sessions)\n",
            stats.searches.total, stats.searches.per_day_avg, stats.searches.sessions
        ));
        out.push_str(&format!(
            "  weak        {:.1}%  top score < {}\n",
            stats.searches.weak_rate * 100.0,
            stats.weak_threshold
        ));
        out.push_str(&format!(
            "  zero        {:.1}%\n",
            stats.searches.zero_rate * 100.0
        ));
        out.push_str(&format!(
            "  errors      {:.1}%\n",
            stats.searches.error_rate * 100.0
        ));
        let median = stats
            .searches
            .median_top_score
            .map(|s| format!("{s:.2}"))
            .unwrap_or_else(|| "n/a".to_string());
        out.push_str(&format!(
            "  median top  {median}   latency p50 {} ms, p90 {} ms\n",
            stats.searches.latency_p50_ms, stats.searches.latency_p90_ms
        ));
        out.push_str(&format!(
            "Stores         {}  (stored {}, duplicate_rejected {}, id_conflict {}, error {})\n\n",
            stats.stores.total,
            stats.stores.stored,
            stats.stores.duplicate_rejected,
            stats.stores.id_conflict,
            stats.stores.error
        ));

        out.push_str("Per day\n");
        for day in &stats.per_day {
            out.push_str(&format!(
                "  {}  {:>5} searches  {:>5} stores\n",
                day.date, day.searches, day.stores
            ));
        }
        out.push('\n');

        out.push_str("Per project\n");
        out.push_str(&format!(
            "  {:<22}{:>10}{:>8}{:>8}\n",
            "project", "searches", "weak", "zero"
        ));
        for p in &stats.per_project {
            out.push_str(&format!(
                "  {:<22}{:>10}{:>7.1}%{:>7.1}%\n",
                p.project,
                p.searches,
                p.weak_rate * 100.0,
                p.zero_rate * 100.0
            ));
        }
        out.push('\n');

        out.push_str("Per client\n");
        for c in &stats.per_client {
            out.push_str(&format!(
                "  {:<22} searches {:>3}   stores {:>3}\n",
                c.client, c.searches, c.stores
            ));
        }
        out.push('\n');

        out.push_str("Top retrieved (appearances / as #1)\n");
        for r in &stats.top_retrieved {
            let label = if r.deleted {
                "(deleted)".to_string()
            } else {
                let title = r.title.clone().unwrap_or_default();
                match &r.project {
                    Some(p) => format!("{title} ({p})"),
                    None => title,
                }
            };
            out.push_str(&format!(
                "  {:>4} / {:>3}  {} — {}\n",
                r.appearances, r.top1, r.id, label
            ));
        }
        out.push('\n');
    }

    out.push_str(&format!(
        "Never retrieved: {} of {} memories\n",
        stats.never_retrieved.len(),
        stats.memories_total
    ));
    for m in &stats.never_retrieved {
        out.push_str(&format!(
            "  {}  [{}] {}\n",
            m.created_at.format("%Y-%m-%d"),
            m.category,
            m.id
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use brain_core::model::{CallContext, Filter, LoggedHit, SearchLogEntry};

    fn search_entry(id: &str, score: f32) -> SearchLogEntry {
        SearchLogEntry {
            ts: Utc::now(),
            ctx: CallContext::default(),
            project: None,
            query: "q".into(),
            filter: Filter::default(),
            limit: 5,
            results: vec![LoggedHit {
                id: id.into(),
                rank: 1,
                score,
            }],
            latency_ms: 5,
            error: None,
        }
    }

    fn metadata(id: &str) -> Metadata {
        Metadata {
            id: id.into(),
            title: format!("Title {id}"),
            tags: vec![],
            category: "learnings".into(),
            project: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn render_contains_sections() {
        let searches = vec![search_entry("a", 0.8)];
        let memories = vec![metadata("a"), metadata("b")];
        let stats = compute(&StatsInput {
            searches: &searches,
            stores: &[],
            memories: &memories,
            since: Utc::now(),
            until: Utc::now(),
            days: 30,
            weak_threshold: 0.45,
            top: 10,
        });
        let text = render(&stats);
        assert!(text.contains("Searches"));
        assert!(text.contains("weak"));
        assert!(text.contains("Per project"));
        assert!(text.contains("Top retrieved"));
        assert!(text.contains("Never retrieved: 1 of 2 memories"));
    }

    #[test]
    fn render_empty_window() {
        let stats = compute(&StatsInput {
            searches: &[],
            stores: &[],
            memories: &[],
            since: Utc::now(),
            until: Utc::now(),
            days: 30,
            weak_threshold: 0.45,
            top: 10,
        });
        let text = render(&stats);
        assert!(text.contains("No searches or stores logged in this window."));
    }
}
