//! Cross-repo aggregation queries.
//!
//! Lazy aggregation: reads each project's ledger on demand, no persistent DB.

use crate::rollup::FileEditStat;
use edda_core::Event;
use edda_ledger::Ledger;
use edda_store::registry::ProjectEntry;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Inclusive date range filter using ISO 8601 prefix comparison.
#[derive(Debug, Clone, Default)]
pub struct DateRange {
    pub after: Option<String>,
    pub before: Option<String>,
}

impl DateRange {
    pub fn matches(&self, ts: &str) -> bool {
        if let Some(ref after) = self.after {
            if ts < after.as_str() {
                return false;
            }
        }
        if let Some(ref before) = self.before {
            if ts > before.as_str() {
                return false;
            }
        }
        true
    }
}

/// Per-project summary within an aggregation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub project_id: String,
    pub name: String,
    pub path: String,
    pub event_count: usize,
    pub commit_count: usize,
    pub decision_count: usize,
    pub session_count: usize,
}

/// A commit record from any project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitRecord {
    pub project_id: String,
    pub project_name: String,
    pub event_id: String,
    pub ts: String,
    pub title: String,
    pub branch: String,
}

/// A decision record from any project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub project_id: String,
    pub project_name: String,
    pub event_id: String,
    pub key: String,
    pub value: String,
    pub reason: String,
    pub domain: String,
    pub branch: String,
    pub ts: Option<String>,
}

/// Full cross-repo aggregation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateResult {
    pub projects: Vec<ProjectSummary>,
    pub total_events: usize,
    pub total_commits: usize,
    pub total_decisions: usize,
}

/// Aggregate overview across all given projects.
pub fn aggregate_overview(projects: &[ProjectEntry], range: &DateRange) -> AggregateResult {
    let mut summaries = Vec::new();
    let mut total_events = 0usize;
    let mut total_commits = 0usize;
    let mut total_decisions = 0usize;

    for entry in projects {
        let root = Path::new(&entry.path);
        let ledger = match Ledger::open(root) {
            Ok(l) => l,
            Err(_) => continue,
        };

        let events = match ledger.iter_events() {
            Ok(e) => e,
            Err(_) => continue,
        };

        let filtered: Vec<&Event> = events.iter().filter(|e| range.matches(&e.ts)).collect();
        let commit_count = filtered.iter().filter(|e| e.event_type == "commit").count();
        let decision_count = filtered
            .iter()
            .filter(|e| e.event_type == "decision")
            .count();
        let session_count = count_unique_sessions(&filtered);

        summaries.push(ProjectSummary {
            project_id: entry.project_id.clone(),
            name: entry.name.clone(),
            path: entry.path.clone(),
            event_count: filtered.len(),
            commit_count,
            decision_count,
            session_count,
        });

        total_events += filtered.len();
        total_commits += commit_count;
        total_decisions += decision_count;
    }

    AggregateResult {
        projects: summaries,
        total_events,
        total_commits,
        total_decisions,
    }
}

/// Collect commits across all projects, sorted by timestamp descending.
pub fn aggregate_commits(
    projects: &[ProjectEntry],
    range: &DateRange,
    limit: usize,
) -> Vec<CommitRecord> {
    let mut records = Vec::new();

    for entry in projects {
        let root = Path::new(&entry.path);
        let ledger = match Ledger::open(root) {
            Ok(l) => l,
            Err(_) => continue,
        };

        let events = match ledger.iter_events() {
            Ok(e) => e,
            Err(_) => continue,
        };

        for event in &events {
            if event.event_type != "commit" {
                continue;
            }
            if !range.matches(&event.ts) {
                continue;
            }

            let title = event
                .payload
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            records.push(CommitRecord {
                project_id: entry.project_id.clone(),
                project_name: entry.name.clone(),
                event_id: event.event_id.clone(),
                ts: event.ts.clone(),
                title,
                branch: event.branch.clone(),
            });
        }
    }

    records.sort_by(|a, b| b.ts.cmp(&a.ts));
    records.truncate(limit);
    records
}

/// Collect active decisions across all projects.
pub fn aggregate_decisions(projects: &[ProjectEntry]) -> Vec<DecisionRecord> {
    let mut records = Vec::new();

    for entry in projects {
        let root = Path::new(&entry.path);
        let ledger = match Ledger::open(root) {
            Ok(l) => l,
            Err(_) => continue,
        };

        let decisions = match ledger.active_decisions(None, None) {
            Ok(d) => d,
            Err(_) => continue,
        };

        for d in decisions {
            records.push(DecisionRecord {
                project_id: entry.project_id.clone(),
                project_name: entry.name.clone(),
                event_id: d.event_id,
                key: d.key,
                value: d.value,
                reason: d.reason,
                domain: d.domain,
                branch: d.branch,
                ts: d.ts,
            });
        }
    }

    records
}

/// Count events by date (YYYY-MM-DD) across all projects.
pub fn events_by_date(projects: &[ProjectEntry], range: &DateRange) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();

    for entry in projects {
        let root = Path::new(&entry.path);
        let ledger = match Ledger::open(root) {
            Ok(l) => l,
            Err(_) => continue,
        };

        let events = match ledger.iter_events() {
            Ok(e) => e,
            Err(_) => continue,
        };

        for event in &events {
            if !range.matches(&event.ts) {
                continue;
            }
            let date = &event.ts[..10.min(event.ts.len())];
            *counts.entry(date.to_string()).or_insert(0) += 1;
        }
    }

    counts
}

/// Count commits by date (YYYY-MM-DD) across all projects.
pub fn commits_by_date(projects: &[ProjectEntry], range: &DateRange) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();

    for entry in projects {
        let root = Path::new(&entry.path);
        let ledger = match Ledger::open(root) {
            Ok(l) => l,
            Err(_) => continue,
        };

        let events = match ledger.iter_events() {
            Ok(e) => e,
            Err(_) => continue,
        };

        for event in &events {
            if event.event_type != "commit" {
                continue;
            }
            if !range.matches(&event.ts) {
                continue;
            }
            let date = &event.ts[..10.min(event.ts.len())];
            *counts.entry(date.to_string()).or_insert(0) += 1;
        }
    }

    counts
}

/// Aggregate file edit counts by date across all projects.
pub fn file_edits_by_date(
    projects: &[ProjectEntry],
    range: &DateRange,
) -> BTreeMap<String, BTreeMap<String, FileEditStat>> {
    let mut result: BTreeMap<String, BTreeMap<String, FileEditStat>> = BTreeMap::new();

    for entry in projects {
        let root = Path::new(&entry.path);
        let ledger = match Ledger::open(root) {
            Ok(l) => l,
            Err(_) => continue,
        };

        let events = match ledger.iter_events() {
            Ok(e) => e,
            Err(_) => continue,
        };

        for event in events.iter().filter(|e| range.matches(&e.ts)) {
            let date = event.ts.chars().take(10).collect::<String>();

            if let Some(stats) = event.payload.get("session_stats") {
                if let Some(file_edits) = stats.get("file_edit_counts") {
                    if let Some(edits_arr) = file_edits.as_array() {
                        for edit in edits_arr {
                            if let (Some(path), Some(count)) = (
                                edit.get("0").and_then(|v| v.as_str()),
                                edit.get("1").and_then(|v| v.as_u64()),
                            ) {
                                let entry = result
                                    .entry(date.clone())
                                    .or_default()
                                    .entry(path.to_string())
                                    .or_default();
                                entry.edits += count;
                            }
                        }
                    }
                }

                if let Some(_session_id) = event.payload.get("session_id").and_then(|v| v.as_str())
                {
                    if let Some(stats) = event.payload.get("session_stats") {
                        if let Some(file_edits) = stats.get("file_edit_counts") {
                            if let Some(edits_arr) = file_edits.as_array() {
                                for edit in edits_arr {
                                    if let Some(path) = edit.get("0").and_then(|v| v.as_str()) {
                                        if let Some(file_entry) =
                                            result.get_mut(&date).and_then(|m| m.get_mut(path))
                                        {
                                            file_entry.agents += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    result
}

/// Count unique session IDs from events.
fn count_unique_sessions(events: &[&Event]) -> usize {
    let mut sessions = std::collections::HashSet::new();
    for event in events {
        if let Some(sid) = event.payload.get("session_id").and_then(|v| v.as_str()) {
            sessions.insert(sid.to_string());
        }
    }
    sessions.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_range_matches_open() {
        let range = DateRange::default();
        assert!(range.matches("2026-03-01T00:00:00Z"));
    }

    #[test]
    fn date_range_matches_after() {
        let range = DateRange {
            after: Some("2026-03-01".to_string()),
            before: None,
        };
        assert!(range.matches("2026-03-02T00:00:00Z"));
        assert!(!range.matches("2026-02-28T23:59:59Z"));
    }

    #[test]
    fn date_range_matches_before() {
        let range = DateRange {
            after: None,
            before: Some("2026-03-01".to_string()),
        };
        assert!(range.matches("2026-02-28T23:59:59Z"));
        assert!(!range.matches("2026-03-02T00:00:00Z"));
    }

    #[test]
    fn date_range_matches_bounded() {
        let range = DateRange {
            after: Some("2026-03-01".to_string()),
            before: Some("2026-03-31".to_string()),
        };
        assert!(range.matches("2026-03-15T12:00:00Z"));
        assert!(!range.matches("2026-02-28T00:00:00Z"));
        assert!(!range.matches("2026-04-01T00:00:00Z"));
    }

    #[test]
    fn aggregate_overview_empty_projects() {
        let result = aggregate_overview(&[], &DateRange::default());
        assert_eq!(result.total_events, 0);
        assert_eq!(result.total_commits, 0);
        assert!(result.projects.is_empty());
    }

    #[test]
    fn aggregate_overview_with_real_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Initialize a workspace
        let paths = edda_ledger::EddaPaths::discover(root);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();

        let ledger = Ledger::open(root).unwrap();
        let event =
            edda_core::event::new_note_event("main", None, "system", "test note", &[]).unwrap();
        ledger.append_event(&event).unwrap();

        let entry = ProjectEntry {
            project_id: "test".to_string(),
            path: root.to_string_lossy().to_string(),
            name: "test-project".to_string(),
            registered_at: "2026-03-01T00:00:00Z".to_string(),
            last_seen: "2026-03-01T00:00:00Z".to_string(),
        };

        let result = aggregate_overview(&[entry], &DateRange::default());
        assert_eq!(result.total_events, 1);
        assert_eq!(result.projects.len(), 1);
        assert_eq!(result.projects[0].event_count, 1);
    }
}
