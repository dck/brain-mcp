use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::model::{Metadata, SearchLogEntry, StoreLogEntry, StoreOutcome};

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub days: u32,
    pub weak_threshold: f32,
    pub memories_total: usize,
    pub searches: SearchStats,
    pub stores: StoreStats,
    pub per_day: Vec<DayCount>,
    pub per_project: Vec<ProjectStats>,
    pub per_client: Vec<ClientStats>,
    pub top_retrieved: Vec<RetrievedMemory>,
    pub never_retrieved: Vec<MemoryRef>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchStats {
    pub total: usize,
    pub ok: usize,
    pub errors: usize,
    pub error_rate: f64,
    pub zero: usize,
    pub zero_rate: f64,
    pub weak: usize,
    pub weak_rate: f64,
    pub median_top_score: Option<f32>,
    pub latency_p50_ms: u64,
    pub latency_p90_ms: u64,
    pub sessions: usize,
    pub per_day_avg: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoreStats {
    pub total: usize,
    pub stored: usize,
    pub duplicate_rejected: usize,
    pub id_conflict: usize,
    pub error: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct DayCount {
    pub date: String,
    pub searches: usize,
    pub stores: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectStats {
    pub project: String,
    pub searches: usize,
    pub weak_rate: f64,
    pub zero_rate: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientStats {
    pub client: String,
    pub searches: usize,
    pub stores: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetrievedMemory {
    pub id: String,
    pub title: Option<String>,
    pub project: Option<String>,
    pub appearances: usize,
    pub top1: usize,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryRef {
    pub id: String,
    pub title: String,
    pub category: String,
    pub project: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct StatsInput<'a> {
    pub searches: &'a [SearchLogEntry],
    pub stores: &'a [StoreLogEntry],
    pub memories: &'a [Metadata],
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub days: u32,
    pub weak_threshold: f32,
    pub top: usize,
}

pub fn project_from_cwd(cwd: &str) -> Option<String> {
    let trimmed = cwd.trim_end_matches('/');
    let scoped = match trimmed.find("/.claude/worktrees/") {
        Some(idx) => &trimmed[..idx],
        None => trimmed,
    };
    scoped.rsplit('/').find(|s| !s.is_empty()).map(String::from)
}

fn rate(n: usize, d: usize) -> f64 {
    if d == 0 { 0.0 } else { n as f64 / d as f64 }
}

fn percentile<T: Copy + PartialOrd>(mut values: Vec<T>, p: f64) -> Option<T> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((p * values.len() as f64).ceil() as usize).saturating_sub(1);
    Some(values[idx.min(values.len() - 1)])
}

pub fn compute(input: &StatsInput<'_>) -> Stats {
    let total_searches = input.searches.len();
    let error_searches = input.searches.iter().filter(|e| e.error.is_some()).count();
    let ok_searches = total_searches - error_searches;

    let ok_entries: Vec<&SearchLogEntry> = input
        .searches
        .iter()
        .filter(|e| e.error.is_none())
        .collect();

    let zero = ok_entries.iter().filter(|e| e.results.is_empty()).count();
    let weak = ok_entries
        .iter()
        .filter(|e| {
            e.results
                .first()
                .is_some_and(|h| h.score < input.weak_threshold)
        })
        .count();

    let top_scores: Vec<f32> = ok_entries
        .iter()
        .filter_map(|e| e.results.first().map(|h| h.score))
        .collect();
    let median_top_score = percentile(top_scores, 0.5);

    let all_latencies: Vec<u64> = input.searches.iter().map(|e| e.latency_ms).collect();
    let latency_p50_ms = percentile(all_latencies.clone(), 0.5).unwrap_or(0);
    let latency_p90_ms = percentile(all_latencies, 0.9).unwrap_or(0);

    let sessions: BTreeSet<&str> = input
        .searches
        .iter()
        .filter_map(|e| e.ctx.session_id.as_deref())
        .collect();

    let per_day_avg = if input.days > 0 {
        total_searches as f64 / input.days as f64
    } else {
        0.0
    };

    let searches_stats = SearchStats {
        total: total_searches,
        ok: ok_searches,
        errors: error_searches,
        error_rate: rate(error_searches, total_searches),
        zero,
        zero_rate: rate(zero, ok_searches),
        weak,
        weak_rate: rate(weak, ok_searches),
        median_top_score,
        latency_p50_ms,
        latency_p90_ms,
        sessions: sessions.len(),
        per_day_avg,
    };

    let stored = input
        .stores
        .iter()
        .filter(|e| e.outcome == StoreOutcome::Stored)
        .count();
    let duplicate_rejected = input
        .stores
        .iter()
        .filter(|e| e.outcome == StoreOutcome::DuplicateRejected)
        .count();
    let id_conflict = input
        .stores
        .iter()
        .filter(|e| e.outcome == StoreOutcome::IdConflict)
        .count();
    let error = input
        .stores
        .iter()
        .filter(|e| e.outcome == StoreOutcome::Error)
        .count();
    let store_stats = StoreStats {
        total: input.stores.len(),
        stored,
        duplicate_rejected,
        id_conflict,
        error,
    };

    let mut per_day_map: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for e in input.searches {
        per_day_map
            .entry(e.ts.format("%Y-%m-%d").to_string())
            .or_default()
            .0 += 1;
    }
    for e in input.stores {
        per_day_map
            .entry(e.ts.format("%Y-%m-%d").to_string())
            .or_default()
            .1 += 1;
    }
    let per_day: Vec<DayCount> = per_day_map
        .into_iter()
        .map(|(date, (searches, stores))| DayCount {
            date,
            searches,
            stores,
        })
        .collect();

    let mut project_groups: BTreeMap<String, Vec<&SearchLogEntry>> = BTreeMap::new();
    for e in input.searches {
        let project = e
            .project
            .clone()
            .or_else(|| {
                e.ctx
                    .cwd
                    .as_ref()
                    .and_then(|c| project_from_cwd(&c.to_string_lossy()))
            })
            .unwrap_or_else(|| "(unknown)".to_string());
        project_groups.entry(project).or_default().push(e);
    }
    let mut per_project: Vec<ProjectStats> = project_groups
        .into_iter()
        .map(|(project, entries)| {
            let group_total = entries.len();
            let group_errors = entries.iter().filter(|e| e.error.is_some()).count();
            let group_ok = group_total - group_errors;
            let group_zero = entries
                .iter()
                .filter(|e| e.error.is_none() && e.results.is_empty())
                .count();
            let group_weak = entries
                .iter()
                .filter(|e| {
                    e.error.is_none()
                        && e.results
                            .first()
                            .is_some_and(|h| h.score < input.weak_threshold)
                })
                .count();
            ProjectStats {
                project,
                searches: group_total,
                weak_rate: rate(group_weak, group_ok),
                zero_rate: rate(group_zero, group_ok),
            }
        })
        .collect();
    per_project.sort_by(|a, b| {
        b.searches
            .cmp(&a.searches)
            .then_with(|| a.project.cmp(&b.project))
    });

    let mut client_searches: BTreeMap<String, usize> = BTreeMap::new();
    let mut client_stores: BTreeMap<String, usize> = BTreeMap::new();
    for e in input.searches {
        *client_searches
            .entry(
                e.ctx
                    .client
                    .clone()
                    .unwrap_or_else(|| "(unknown)".to_string()),
            )
            .or_default() += 1;
    }
    for e in input.stores {
        *client_stores
            .entry(
                e.ctx
                    .client
                    .clone()
                    .unwrap_or_else(|| "(unknown)".to_string()),
            )
            .or_default() += 1;
    }
    let mut clients: BTreeSet<String> = client_searches.keys().cloned().collect();
    clients.extend(client_stores.keys().cloned());
    let mut per_client: Vec<ClientStats> = clients
        .into_iter()
        .map(|client| ClientStats {
            searches: *client_searches.get(&client).unwrap_or(&0),
            stores: *client_stores.get(&client).unwrap_or(&0),
            client,
        })
        .collect();
    per_client.sort_by(|a, b| {
        b.searches
            .cmp(&a.searches)
            .then_with(|| a.client.cmp(&b.client))
    });

    let mut appearances: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for e in input.searches {
        for hit in &e.results {
            let counts = appearances.entry(hit.id.clone()).or_default();
            counts.0 += 1;
            if hit.rank == 1 {
                counts.1 += 1;
            }
        }
    }
    let memory_by_id: HashMap<&str, &Metadata> =
        input.memories.iter().map(|m| (m.id.as_str(), m)).collect();
    let mut top_retrieved: Vec<RetrievedMemory> = appearances
        .into_iter()
        .map(|(id, (appearances, top1))| {
            let meta = memory_by_id.get(id.as_str());
            RetrievedMemory {
                title: meta.map(|m| m.title.clone()),
                project: meta.and_then(|m| m.project.clone()),
                deleted: meta.is_none(),
                id,
                appearances,
                top1,
            }
        })
        .collect();
    top_retrieved.sort_by(|a, b| {
        b.appearances
            .cmp(&a.appearances)
            .then_with(|| b.top1.cmp(&a.top1))
            .then_with(|| a.id.cmp(&b.id))
    });
    top_retrieved.truncate(input.top);

    let retrieved_ids: HashSet<&str> = input
        .searches
        .iter()
        .flat_map(|e| e.results.iter().map(|h| h.id.as_str()))
        .collect();
    let mut never_retrieved: Vec<MemoryRef> = input
        .memories
        .iter()
        .filter(|m| !retrieved_ids.contains(m.id.as_str()))
        .map(|m| MemoryRef {
            id: m.id.clone(),
            title: m.title.clone(),
            category: m.category.clone(),
            project: m.project.clone(),
            created_at: m.created_at,
        })
        .collect();
    never_retrieved.sort_by(|a, b| a.created_at.cmp(&b.created_at));

    Stats {
        since: input.since,
        until: input.until,
        days: input.days,
        weak_threshold: input.weak_threshold,
        memories_total: input.memories.len(),
        searches: searches_stats,
        stores: store_stats,
        per_day,
        per_project,
        per_client,
        top_retrieved,
        never_retrieved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CallContext, Filter, LoggedHit};

    fn search_entry(
        results: Vec<(&str, f32)>,
        error: Option<&str>,
        ts: DateTime<Utc>,
    ) -> SearchLogEntry {
        SearchLogEntry {
            ts,
            ctx: CallContext::default(),
            project: None,
            query: "q".into(),
            filter: Filter::default(),
            limit: 5,
            results: results
                .into_iter()
                .enumerate()
                .map(|(i, (id, score))| LoggedHit {
                    id: id.into(),
                    rank: i + 1,
                    score,
                })
                .collect(),
            latency_ms: 10,
            error: error.map(String::from),
        }
    }

    fn metadata(id: &str, created_at: DateTime<Utc>) -> Metadata {
        Metadata {
            id: id.into(),
            title: format!("Title {id}"),
            tags: vec![],
            category: "learnings".into(),
            project: None,
            created_at,
        }
    }

    #[test]
    fn stats_rates_and_counts() {
        let now = Utc::now();
        let searches = vec![
            search_entry(vec![], Some("boom"), now),
            search_entry(vec![], None, now),
            search_entry(vec![("a", 0.30)], None, now),
            search_entry(vec![("b", 0.70)], None, now),
        ];
        let input = StatsInput {
            searches: &searches,
            stores: &[],
            memories: &[],
            since: now,
            until: now,
            days: 1,
            weak_threshold: 0.45,
            top: 10,
        };
        let stats = compute(&input);
        assert_eq!(stats.searches.total, 4);
        assert_eq!(stats.searches.ok, 3);
        assert_eq!(stats.searches.errors, 1);
        assert_eq!(stats.searches.zero, 1);
        assert_eq!(stats.searches.weak, 1);
        assert!((stats.searches.weak_rate - 1.0 / 3.0).abs() < 1e-6);
        assert!((stats.searches.error_rate - 0.25).abs() < 1e-6);
        assert_eq!(stats.searches.median_top_score, Some(0.30));
    }

    #[test]
    fn stats_top_retrieved_and_never_retrieved() {
        let now = Utc::now();
        let searches = vec![
            search_entry(vec![("a", 0.9), ("b", 0.5)], None, now),
            search_entry(vec![("a", 0.9)], None, now),
            search_entry(vec![("a", 0.9), ("d", 0.4)], None, now),
        ];
        let memories = vec![metadata("a", now), metadata("b", now), metadata("c", now)];
        let input = StatsInput {
            searches: &searches,
            stores: &[],
            memories: &memories,
            since: now,
            until: now,
            days: 1,
            weak_threshold: 0.45,
            top: 10,
        };
        let stats = compute(&input);
        assert_eq!(stats.top_retrieved[0].id, "a");
        assert_eq!(stats.top_retrieved[0].appearances, 3);
        assert_eq!(stats.top_retrieved[0].top1, 3);
        let b = stats.top_retrieved.iter().find(|r| r.id == "b").unwrap();
        assert_eq!(b.appearances, 1);
        assert_eq!(b.top1, 0);
        let d = stats.top_retrieved.iter().find(|r| r.id == "d").unwrap();
        assert!(d.deleted);
        assert_eq!(d.title, None);
        assert_eq!(
            stats
                .never_retrieved
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["c"]
        );
    }

    #[test]
    fn stats_per_project_prefers_project_column_then_cwd() {
        let now = Utc::now();
        let mut with_project = search_entry(vec![], None, now);
        with_project.project = Some("x".into());
        let mut with_cwd = search_entry(vec![], None, now);
        with_cwd.ctx.cwd = Some("/w/maestro/.claude/worktrees/abc".into());
        let neither = search_entry(vec![], None, now);
        let searches = vec![with_project, with_cwd, neither];
        let input = StatsInput {
            searches: &searches,
            stores: &[],
            memories: &[],
            since: now,
            until: now,
            days: 1,
            weak_threshold: 0.45,
            top: 10,
        };
        let stats = compute(&input);
        let mut projects: Vec<&str> = stats
            .per_project
            .iter()
            .map(|p| p.project.as_str())
            .collect();
        projects.sort();
        assert_eq!(projects, vec!["(unknown)", "maestro", "x"]);
    }

    #[test]
    fn stats_empty_input() {
        let now = Utc::now();
        let memories = vec![metadata("a", now)];
        let input = StatsInput {
            searches: &[],
            stores: &[],
            memories: &memories,
            since: now,
            until: now,
            days: 30,
            weak_threshold: 0.45,
            top: 10,
        };
        let stats = compute(&input);
        assert_eq!(stats.searches.total, 0);
        assert_eq!(stats.searches.error_rate, 0.0);
        assert_eq!(stats.searches.zero_rate, 0.0);
        assert_eq!(stats.searches.weak_rate, 0.0);
        assert_eq!(stats.searches.median_top_score, None);
        assert_eq!(stats.never_retrieved.len(), stats.memories_total);
    }

    #[test]
    fn project_from_cwd_cases() {
        assert_eq!(
            project_from_cwd(
                "/Users/deck/work/toptal/maestro/.claude/worktrees/pure-stargazing-meteor"
            ),
            Some("maestro".to_string())
        );
        assert_eq!(
            project_from_cwd("/Users/deck/work/fitlake"),
            Some("fitlake".to_string())
        );
        assert_eq!(project_from_cwd("/"), None);
        assert_eq!(
            project_from_cwd("/Users/deck/work/fitlake/"),
            Some("fitlake".to_string())
        );
    }
}
