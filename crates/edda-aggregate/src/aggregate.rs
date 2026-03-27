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
            .filter(|e| e.event_type == "note" && edda_core::decision::is_decision(&e.payload))
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

        let decisions = match ledger.active_decisions(None, None, None, None) {
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

/// Combined rollup metrics collected in a single pass over the event ledger.
///
/// Instead of calling 5 separate functions (each opening ledgers and scanning
/// all events independently), this struct holds all metrics produced by a
/// single traversal via [`rollup_metrics_by_date`].
pub struct RollupMetrics {
    pub events: BTreeMap<String, usize>,
    pub commits: BTreeMap<String, usize>,
    pub file_edits: BTreeMap<String, Vec<FileEditStat>>,
    pub cost: BTreeMap<String, f64>,
    pub quality: BTreeMap<String, (u64, u64)>,
}

/// Collect all rollup metrics in a single pass over each project's ledger.
///
/// This replaces 5 separate calls to `events_by_date`, `commits_by_date`,
/// `file_edits_by_date`, `cost_by_date`, and `quality_by_date`, reducing
/// ledger opens and event scans from 5N to 1N (where N = number of projects).
pub fn rollup_metrics_by_date(projects: &[ProjectEntry], range: &DateRange) -> RollupMetrics {
    use std::collections::{HashMap, HashSet};

    // Per-file accumulator for file edits: (edit_count, last_ts, unique sessions)
    type FileAcc = (u64, String, HashSet<String>);

    let mut events: BTreeMap<String, usize> = BTreeMap::new();
    let mut commits: BTreeMap<String, usize> = BTreeMap::new();
    let mut cost: BTreeMap<String, f64> = BTreeMap::new();
    let mut quality: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut file_edits_acc: HashMap<String, HashMap<String, FileAcc>> = HashMap::new();

    for entry in projects {
        let root = Path::new(&entry.path);
        let ledger = match Ledger::open(root) {
            Ok(l) => l,
            Err(_) => continue,
        };

        let all_events = match ledger.iter_events() {
            Ok(e) => e,
            Err(_) => continue,
        };

        for event in &all_events {
            if !range.matches(&event.ts) {
                continue;
            }
            let date = &event.ts[..10.min(event.ts.len())];
            let date_str = date.to_string();

            // Always count events
            *events.entry(date_str.clone()).or_insert(0) += 1;

            // Count commits
            if event.event_type == "commit" {
                *commits.entry(date_str.clone()).or_insert(0) += 1;
            }

            // Accumulate cost and quality for execution events
            if event.event_type == "execution_event" {
                // Cost
                let cost_val = event
                    .payload
                    .get("usage")
                    .and_then(|u| u.get("cost_usd"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                if cost_val > 0.0 {
                    *cost.entry(date_str.clone()).or_insert(0.0) += cost_val;
                }

                // Quality
                let status = event
                    .payload
                    .get("result")
                    .and_then(|r| r.get("status"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let q_entry = quality.entry(date_str.clone()).or_insert((0, 0));
                if status == "success" {
                    q_entry.0 += 1;
                }
                q_entry.1 += 1;
            }

            // Accumulate file edits from session_stats
            let stats = match event.payload.get("session_stats") {
                Some(s) => s,
                None => continue,
            };

            let file_edit_counts = match stats.get("file_edit_counts").and_then(|v| v.as_array()) {
                Some(arr) => arr,
                None => continue,
            };

            let session_id = event
                .payload
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            for edit in file_edit_counts {
                let arr = match edit.as_array() {
                    Some(a) if a.len() == 2 => a,
                    _ => continue,
                };
                let path = match arr[0].as_str() {
                    Some(p) => p.to_string(),
                    None => continue,
                };
                let count = arr[1].as_u64().unwrap_or(0);

                let day_map = file_edits_acc.entry(date_str.clone()).or_default();
                let file_entry = day_map
                    .entry(path)
                    .or_insert_with(|| (0, String::new(), HashSet::new()));
                file_entry.0 += count;
                if event.ts > file_entry.1 {
                    file_entry.1.clone_from(&event.ts);
                }
                if !session_id.is_empty() {
                    file_entry.2.insert(session_id.clone());
                }
            }
        }
    }

    // Convert file edits accumulator to final structure
    let mut file_edits: BTreeMap<String, Vec<FileEditStat>> = BTreeMap::new();
    for (date, files) in file_edits_acc {
        let mut edits: Vec<FileEditStat> = files
            .into_iter()
            .map(|(path, (edit_count, last_edited, sessions))| FileEditStat {
                path,
                edit_count,
                agent_count: sessions.len(),
                last_edited,
                revert_count: 0,
            })
            .collect();
        edits.sort_by(|a, b| b.edit_count.cmp(&a.edit_count));
        file_edits.insert(date, edits);
    }

    RollupMetrics {
        events,
        commits,
        file_edits,
        cost,
        quality,
    }
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

/// Aggregate per-file edit counts by date across all projects.
///
/// Reads `session_stats.file_edit_counts` from session digest events.
/// Uses `HashSet` to deduplicate agent sessions per (date, file).
pub fn file_edits_by_date(
    projects: &[ProjectEntry],
    range: &DateRange,
) -> BTreeMap<String, Vec<FileEditStat>> {
    use std::collections::{HashMap, HashSet};

    // Per-file accumulator: (edit_count, last_ts, unique sessions)
    type FileAcc = (u64, String, HashSet<String>);

    // Intermediate accumulator: date -> file -> FileAcc
    let mut acc: HashMap<String, HashMap<String, FileAcc>> = HashMap::new();

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
            let date = event.ts[..10.min(event.ts.len())].to_string();

            let session_id = event
                .payload
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let stats = match event.payload.get("session_stats") {
                Some(s) => s,
                None => continue,
            };

            let file_edits = match stats.get("file_edit_counts").and_then(|v| v.as_array()) {
                Some(arr) => arr,
                None => continue,
            };

            for edit in file_edits {
                // file_edit_counts is Vec<(String, u64)>, serialized as [["path", count], ...]
                let arr = match edit.as_array() {
                    Some(a) if a.len() == 2 => a,
                    _ => continue,
                };
                let path = match arr[0].as_str() {
                    Some(p) => p.to_string(),
                    None => continue,
                };
                let count = arr[1].as_u64().unwrap_or(0);

                let day_map = acc.entry(date.clone()).or_default();
                let file_entry = day_map
                    .entry(path)
                    .or_insert_with(|| (0, String::new(), HashSet::new()));
                file_entry.0 += count;
                if event.ts > file_entry.1 {
                    file_entry.1.clone_from(&event.ts);
                }
                if !session_id.is_empty() {
                    file_entry.2.insert(session_id.clone());
                }
            }
        }
    }

    // Convert accumulator to final structure
    let mut result: BTreeMap<String, Vec<FileEditStat>> = BTreeMap::new();
    for (date, files) in acc {
        let mut edits: Vec<FileEditStat> = files
            .into_iter()
            .map(|(path, (edit_count, last_edited, sessions))| FileEditStat {
                path,
                edit_count,
                agent_count: sessions.len(),
                last_edited,
                revert_count: 0,
            })
            .collect();
        // Sort by edit_count descending for consistent output
        edits.sort_by(|a, b| b.edit_count.cmp(&a.edit_count));
        result.insert(date, edits);
    }

    result
}

/// Aggregate execution event cost by date (YYYY-MM-DD) across all projects.
pub fn cost_by_date(projects: &[ProjectEntry], range: &DateRange) -> BTreeMap<String, f64> {
    let mut costs: BTreeMap<String, f64> = BTreeMap::new();

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
            if event.event_type != "execution_event" {
                continue;
            }
            if !range.matches(&event.ts) {
                continue;
            }
            let cost = event
                .payload
                .get("usage")
                .and_then(|u| u.get("cost_usd"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            if cost > 0.0 {
                let date = &event.ts[..10.min(event.ts.len())];
                *costs.entry(date.to_string()).or_insert(0.0) += cost;
            }
        }
    }

    costs
}

/// Aggregate execution event quality by date (YYYY-MM-DD) across all projects.
///
/// Returns `BTreeMap<date, (success_count, total_count)>`.
pub fn quality_by_date(
    projects: &[ProjectEntry],
    range: &DateRange,
) -> BTreeMap<String, (u64, u64)> {
    let mut quality: BTreeMap<String, (u64, u64)> = BTreeMap::new();

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
            if event.event_type != "execution_event" {
                continue;
            }
            if !range.matches(&event.ts) {
                continue;
            }
            let status = event
                .payload
                .get("result")
                .and_then(|r| r.get("status"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            let date = &event.ts[..10.min(event.ts.len())];
            let entry = quality.entry(date.to_string()).or_insert((0, 0));
            if status == "success" {
                entry.0 += 1;
            }
            entry.1 += 1;
        }
    }

    quality
}

/// Per-project metrics with cost, quality, and activity breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMetrics {
    pub project_id: String,
    pub name: String,
    pub group: Option<String>,
    pub activity: ActivityMetrics,
    pub cost: CostMetrics,
    pub quality: QualityMetrics,
}

/// Activity counts for a project within a time range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityMetrics {
    pub events: usize,
    pub commits: usize,
    pub decisions: usize,
    pub sessions: usize,
}

/// Per-model cost breakdown entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCost {
    pub model: String,
    pub cost_usd: f64,
    pub steps: u64,
}

/// Cost metrics for a project within a time range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostMetrics {
    pub total_usd: f64,
    pub daily_avg_usd: f64,
    /// Cost of the most recent day in the period (for anomaly detection).
    #[serde(default)]
    pub last_day_usd: f64,
    pub by_model: Vec<ModelCost>,
}

/// Quality metrics for a project within a time range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityMetrics {
    pub success_rate: f64,
    pub avg_latency_ms: f64,
    pub total_steps: u64,
}

/// Compute per-project metrics for all given projects within the specified date range.
pub fn per_project_metrics(
    projects: &[ProjectEntry],
    range: &DateRange,
    days: usize,
) -> Vec<ProjectMetrics> {
    use crate::quality::model_quality_from_events;

    let mut results = Vec::new();

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
            .filter(|e| e.event_type == "note" && edda_core::decision::is_decision(&e.payload))
            .count();
        let session_count = count_unique_sessions(&filtered);

        // Quality: filter execution events and compute
        let exec_events: Vec<Event> = events
            .iter()
            .filter(|e| e.event_type == "execution_event" && range.matches(&e.ts))
            .cloned()
            .collect();
        let quality_report = model_quality_from_events(&exec_events, range);

        let by_model: Vec<ModelCost> = quality_report
            .models
            .iter()
            .map(|m| ModelCost {
                model: m.model.clone(),
                cost_usd: m.total_cost_usd,
                steps: m.total_steps,
            })
            .collect();

        let daily_avg = if days > 0 {
            quality_report.total_cost_usd / days as f64
        } else {
            0.0
        };

        // Compute per-day costs to find the most recent day's cost
        let last_day_usd = {
            let mut day_costs: BTreeMap<&str, f64> = BTreeMap::new();
            for event in &exec_events {
                let cost = event
                    .payload
                    .get("usage")
                    .and_then(|u| u.get("cost_usd"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                if cost > 0.0 {
                    let date = &event.ts[..10.min(event.ts.len())];
                    *day_costs.entry(date).or_insert(0.0) += cost;
                }
            }
            // Last entry in BTreeMap is the most recent date
            day_costs.values().last().copied().unwrap_or(0.0)
        };

        results.push(ProjectMetrics {
            project_id: entry.project_id.clone(),
            name: entry.name.clone(),
            group: entry.group.clone(),
            activity: ActivityMetrics {
                events: filtered.len(),
                commits: commit_count,
                decisions: decision_count,
                sessions: session_count,
            },
            cost: CostMetrics {
                total_usd: quality_report.total_cost_usd,
                daily_avg_usd: daily_avg,
                last_day_usd,
                by_model,
            },
            quality: QualityMetrics {
                success_rate: quality_report.overall_success_rate,
                avg_latency_ms: if quality_report.total_steps > 0 {
                    quality_report
                        .models
                        .iter()
                        .map(|m| m.avg_latency_ms * m.total_steps as f64)
                        .sum::<f64>()
                        / quality_report.total_steps as f64
                } else {
                    0.0
                },
                total_steps: quality_report.total_steps,
            },
        });
    }

    results
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
            group: None,
        };

        let result = aggregate_overview(&[entry], &DateRange::default());
        assert_eq!(result.total_events, 1);
        assert_eq!(result.projects.len(), 1);
        assert_eq!(result.projects[0].event_count, 1);
    }

    #[test]
    fn aggregate_overview_counts_decisions() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let paths = edda_ledger::EddaPaths::discover(root);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();

        let ledger = Ledger::open(root).unwrap();

        // Create a decision event (note with "decision" tag)
        let decision = edda_core::types::DecisionPayload {
            key: "db.engine".to_string(),
            value: "sqlite".to_string(),
            reason: Some("embedded".to_string()),
            scope: None,
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: None,
        };
        let event =
            edda_core::event::new_decision_event("main", None, "system", &decision).unwrap();
        ledger.append_event(&event).unwrap();

        let entry = ProjectEntry {
            project_id: "test".to_string(),
            path: root.to_string_lossy().to_string(),
            name: "test-project".to_string(),
            registered_at: "2026-03-01T00:00:00Z".to_string(),
            last_seen: "2026-03-01T00:00:00Z".to_string(),
            group: None,
        };

        let result = aggregate_overview(&[entry], &DateRange::default());
        assert_eq!(result.total_decisions, 1);
        assert_eq!(result.projects[0].decision_count, 1);
    }

    /// Helper: create a ledger event with session_stats containing file_edit_counts.
    fn make_session_event(session_id: &str, file_edits: &[(&str, u64)]) -> edda_core::types::Event {
        let edits_json: Vec<serde_json::Value> = file_edits
            .iter()
            .map(|(path, count)| serde_json::json!([path, count]))
            .collect();

        let mut event =
            edda_core::event::new_note_event("main", None, "system", "test", &[]).unwrap();
        event.payload = serde_json::json!({
            "session_id": session_id,
            "session_stats": {
                "file_edit_counts": edits_json,
            }
        });
        event
    }

    #[test]
    fn file_edits_by_date_empty_projects() {
        let result = file_edits_by_date(&[], &DateRange::default());
        assert!(result.is_empty());
    }

    #[test]
    fn file_edits_by_date_parses_tuples() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let paths = edda_ledger::EddaPaths::discover(root);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();

        let ledger = Ledger::open(root).unwrap();
        let event = make_session_event("sess-1", &[("src/main.rs", 5), ("src/lib.rs", 3)]);
        ledger.append_event(&event).unwrap();

        let entry = ProjectEntry {
            project_id: "test".to_string(),
            path: root.to_string_lossy().to_string(),
            name: "test".to_string(),
            registered_at: "2026-01-01".to_string(),
            last_seen: "2026-01-01".to_string(),
            group: None,
        };

        let result = file_edits_by_date(&[entry], &DateRange::default());
        assert_eq!(result.len(), 1);

        let date_key = result.keys().next().unwrap();
        let edits = &result[date_key];
        assert_eq!(edits.len(), 2);

        let main_rs = edits.iter().find(|e| e.path == "src/main.rs").unwrap();
        assert_eq!(main_rs.edit_count, 5);
        assert_eq!(main_rs.agent_count, 1);

        let lib_rs = edits.iter().find(|e| e.path == "src/lib.rs").unwrap();
        assert_eq!(lib_rs.edit_count, 3);
        assert_eq!(lib_rs.agent_count, 1);
    }

    #[test]
    fn file_edits_by_date_deduplicates_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let paths = edda_ledger::EddaPaths::discover(root);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();

        let ledger = Ledger::open(root).unwrap();

        // Two different sessions editing the same file
        let event1 = make_session_event("sess-1", &[("src/main.rs", 5)]);
        ledger.append_event(&event1).unwrap();

        let event2 = make_session_event("sess-2", &[("src/main.rs", 3)]);
        ledger.append_event(&event2).unwrap();

        let entry = ProjectEntry {
            project_id: "test".to_string(),
            path: root.to_string_lossy().to_string(),
            name: "test".to_string(),
            registered_at: "2026-01-01".to_string(),
            last_seen: "2026-01-01".to_string(),
            group: None,
        };

        let result = file_edits_by_date(&[entry], &DateRange::default());
        assert_eq!(result.len(), 1);

        let date_key = result.keys().next().unwrap();
        let edits = &result[date_key];
        let main_rs = edits.iter().find(|e| e.path == "src/main.rs").unwrap();

        // Total edits: 5 + 3 = 8
        assert_eq!(main_rs.edit_count, 8);
        // Unique sessions: 2 (sess-1 and sess-2)
        assert_eq!(main_rs.agent_count, 2);
    }

    /// Helper: create an execution event with cost and result status.
    fn make_execution_event(cost_usd: f64, status: &str) -> edda_core::types::Event {
        let mut event =
            edda_core::event::new_note_event("main", None, "system", "exec", &[]).unwrap();
        event.event_type = "execution_event".to_string();
        event.payload = serde_json::json!({
            "usage": { "cost_usd": cost_usd },
            "result": { "status": status },
        });
        event
    }

    /// Helper: create a commit event.
    fn make_commit_event() -> edda_core::types::Event {
        let mut event =
            edda_core::event::new_note_event("main", None, "system", "commit", &[]).unwrap();
        event.event_type = "commit".to_string();
        event.payload = serde_json::json!({ "title": "test commit" });
        event
    }

    #[test]
    fn rollup_metrics_single_pass_collects_all_metrics() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let paths = edda_ledger::EddaPaths::discover(root);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();

        let ledger = Ledger::open(root).unwrap();

        // Add a commit event
        ledger.append_event(&make_commit_event()).unwrap();

        // Add an execution event with cost
        ledger
            .append_event(&make_execution_event(0.05, "success"))
            .unwrap();

        // Add a failed execution event
        ledger
            .append_event(&make_execution_event(0.02, "failure"))
            .unwrap();

        // Add a session event with file edits
        let session_event = make_session_event("sess-1", &[("src/main.rs", 3), ("src/lib.rs", 1)]);
        ledger.append_event(&session_event).unwrap();

        let entry = ProjectEntry {
            project_id: "test".to_string(),
            path: root.to_string_lossy().to_string(),
            name: "test".to_string(),
            registered_at: "2026-01-01".to_string(),
            last_seen: "2026-01-01".to_string(),
            group: None,
        };

        let range = DateRange::default();
        let metrics = rollup_metrics_by_date(&[entry], &range);

        // All 4 events counted
        let total_events: usize = metrics.events.values().sum();
        assert_eq!(total_events, 4);

        // 1 commit
        let total_commits: usize = metrics.commits.values().sum();
        assert_eq!(total_commits, 1);

        // Cost: 0.05 + 0.02 = 0.07
        let total_cost: f64 = metrics.cost.values().sum();
        assert!((total_cost - 0.07).abs() < 1e-9);

        // Quality: 1 success out of 2 execution events
        let (total_success, total_exec): (u64, u64) = metrics
            .quality
            .values()
            .fold((0, 0), |acc, &(s, t)| (acc.0 + s, acc.1 + t));
        assert_eq!(total_success, 1);
        assert_eq!(total_exec, 2);

        // File edits: 2 files
        let total_files: usize = metrics.file_edits.values().map(|v| v.len()).sum();
        assert_eq!(total_files, 2);
    }

    #[test]
    fn rollup_metrics_matches_individual_functions() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let paths = edda_ledger::EddaPaths::discover(root);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();

        let ledger = Ledger::open(root).unwrap();
        ledger.append_event(&make_commit_event()).unwrap();
        ledger
            .append_event(&make_execution_event(0.10, "success"))
            .unwrap();
        ledger
            .append_event(&make_execution_event(0.03, "failure"))
            .unwrap();
        let session_event = make_session_event("sess-1", &[("src/main.rs", 7)]);
        ledger.append_event(&session_event).unwrap();

        let entry = ProjectEntry {
            project_id: "test".to_string(),
            path: root.to_string_lossy().to_string(),
            name: "test".to_string(),
            registered_at: "2026-01-01".to_string(),
            last_seen: "2026-01-01".to_string(),
            group: None,
        };

        let range = DateRange::default();
        let projects = &[entry];

        // Single-pass
        let metrics = rollup_metrics_by_date(projects, &range);

        // Individual functions
        let ind_events = events_by_date(projects, &range);
        let ind_commits = commits_by_date(projects, &range);
        let ind_cost = cost_by_date(projects, &range);
        let ind_quality = quality_by_date(projects, &range);
        let ind_file_edits = file_edits_by_date(projects, &range);

        // Compare results
        assert_eq!(metrics.events, ind_events);
        assert_eq!(metrics.commits, ind_commits);
        assert_eq!(metrics.quality, ind_quality);

        // Compare cost with floating-point tolerance
        assert_eq!(metrics.cost.len(), ind_cost.len());
        for (date, val) in &metrics.cost {
            let ind_val = ind_cost.get(date).copied().unwrap_or(0.0);
            assert!(
                (val - ind_val).abs() < 1e-9,
                "cost mismatch on {date}: {val} vs {ind_val}"
            );
        }

        // Compare file edits structure
        assert_eq!(metrics.file_edits.len(), ind_file_edits.len());
        for (date, edits) in &metrics.file_edits {
            let ind_edits = &ind_file_edits[date];
            assert_eq!(edits.len(), ind_edits.len());
            for edit in edits {
                let ind_edit = ind_edits.iter().find(|e| e.path == edit.path).unwrap();
                assert_eq!(edit.edit_count, ind_edit.edit_count);
                assert_eq!(edit.agent_count, ind_edit.agent_count);
            }
        }
    }
}
