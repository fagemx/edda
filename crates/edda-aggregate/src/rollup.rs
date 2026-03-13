//! Pre-computed rollup statistics cached at `~/.edda/collabs/<tool>/rollup.json`.
//!
//! Supports daily, weekly, and monthly granularity with incremental updates.

use crate::aggregate::{self, DateRange};
use edda_store::registry::ProjectEntry;
use edda_store::{store_root, write_atomic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Per-file edit statistics for code heatmap.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileEditStat {
    /// File path relative to the repository root.
    pub path: String,
    /// Number of Edit/Write tool calls on this file.
    pub edit_count: u64,
    /// Number of unique agent sessions that edited this file.
    pub agent_count: usize,
    /// ISO 8601 timestamp of the last edit.
    #[serde(default)]
    pub last_edited: String,
    /// Number of reverts affecting this file (0 for now).
    #[serde(default)]
    pub revert_count: u64,
}

/// Daily statistics.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DayStat {
    pub date: String,
    pub events: usize,
    pub commits: usize,
    pub sessions: usize,
    /// Per-file edit statistics for this day.
    #[serde(default)]
    pub file_edits: Vec<FileEditStat>,
    /// Total cost in USD from execution events.
    #[serde(default)]
    pub cost_usd: f64,
    /// Number of execution events.
    #[serde(default)]
    pub execution_count: u64,
    /// Number of successful execution events.
    #[serde(default)]
    pub success_count: u64,
}

/// Weekly statistics.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WeekStat {
    pub week_start: String,
    pub events: usize,
    pub commits: usize,
    /// Per-file edit statistics for this week.
    #[serde(default)]
    pub file_edits: Vec<FileEditStat>,
    /// Total cost in USD from execution events.
    #[serde(default)]
    pub cost_usd: f64,
    /// Number of execution events.
    #[serde(default)]
    pub execution_count: u64,
    /// Number of successful execution events.
    #[serde(default)]
    pub success_count: u64,
}

/// Monthly statistics.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MonthStat {
    pub month: String,
    pub events: usize,
    pub commits: usize,
    /// Per-file edit statistics for this month.
    #[serde(default)]
    pub file_edits: Vec<FileEditStat>,
    /// Total cost in USD from execution events.
    #[serde(default)]
    pub cost_usd: f64,
    /// Number of execution events.
    #[serde(default)]
    pub execution_count: u64,
    /// Number of successful execution events.
    #[serde(default)]
    pub success_count: u64,
}

/// The full rollup cache.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Rollup {
    pub tool: String,
    pub last_updated: String,
    pub daily: Vec<DayStat>,
    pub weekly: Vec<WeekStat>,
    pub monthly: Vec<MonthStat>,
}

/// Path to the collabs directory for a tool.
pub fn collabs_dir(tool: &str) -> PathBuf {
    store_root().join("collabs").join(tool)
}

/// Path to the rollup cache file for a tool.
pub fn rollup_path(tool: &str) -> PathBuf {
    collabs_dir(tool).join("rollup.json")
}

/// Ensure the collabs directory structure exists.
pub fn ensure_collabs_dir(tool: &str) -> anyhow::Result<()> {
    let dir = collabs_dir(tool);
    std::fs::create_dir_all(&dir)?;
    Ok(())
}

/// Load an existing rollup from disk.
pub fn load_rollup(tool: &str) -> Option<Rollup> {
    let path = rollup_path(tool);
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Save a rollup to disk.
pub fn save_rollup(rollup: &Rollup) -> anyhow::Result<()> {
    ensure_collabs_dir(&rollup.tool)?;
    let json = serde_json::to_string_pretty(rollup)?;
    write_atomic(&rollup_path(&rollup.tool), json.as_bytes())
}

/// Compute a full rollup from scratch.
pub fn compute_rollup(projects: &[ProjectEntry], range: &DateRange, tool: &str) -> Rollup {
    let events_map = aggregate::events_by_date(projects, range);
    let commits_map = aggregate::commits_by_date(projects, range);
    let file_edits_map = aggregate::file_edits_by_date(projects, range);
    let cost_map = aggregate::cost_by_date(projects, range);
    let quality_map = aggregate::quality_by_date(projects, range);

    let daily = build_daily_stats(
        &events_map,
        &commits_map,
        &file_edits_map,
        &cost_map,
        &quality_map,
    );
    let weekly = build_weekly_stats(&daily);
    let monthly = build_monthly_stats(&daily);

    let now = time::OffsetDateTime::now_utc();
    let last_updated = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string());

    Rollup {
        tool: tool.to_string(),
        last_updated,
        daily,
        weekly,
        monthly,
    }
}

/// Incremental rollup: compute only new data since last update, then merge.
pub fn compute_rollup_incremental(projects: &[ProjectEntry], tool: &str) -> anyhow::Result<Rollup> {
    let existing = load_rollup(tool);

    let range = if let Some(ref existing) = existing {
        // Only fetch events since last_updated (not last daily entry date).
        let cutoff = &existing.last_updated;
        DateRange {
            after: if cutoff.is_empty() {
                None
            } else {
                // Use the date portion (YYYY-MM-DD) of last_updated as the
                // "after" boundary so we re-scan the partial day.
                Some(cutoff.chars().take(10).collect())
            },
            before: None,
        }
    } else {
        DateRange::default()
    };

    let new_rollup = compute_rollup(projects, &range, tool);

    let merged = if let Some(existing) = existing {
        merge_rollups(&existing, &new_rollup)
    } else {
        new_rollup
    };

    save_rollup(&merged)?;
    Ok(merged)
}

/// Merge two rollups: new data replaces overlapping days in base.
pub fn merge_rollups(base: &Rollup, new: &Rollup) -> Rollup {
    let mut daily_map: BTreeMap<String, DayStat> = BTreeMap::new();

    for d in &base.daily {
        daily_map.insert(d.date.clone(), d.clone());
    }
    for d in &new.daily {
        daily_map.insert(d.date.clone(), d.clone());
    }

    let daily: Vec<DayStat> = daily_map.into_values().collect();
    let weekly = build_weekly_stats(&daily);
    let monthly = build_monthly_stats(&daily);

    Rollup {
        tool: new.tool.clone(),
        last_updated: new.last_updated.clone(),
        daily,
        weekly,
        monthly,
    }
}

/// Build daily stats from event and commit counts by date.
fn build_daily_stats(
    events_map: &BTreeMap<String, usize>,
    commits_map: &BTreeMap<String, usize>,
    file_edits_map: &BTreeMap<String, Vec<FileEditStat>>,
    cost_map: &BTreeMap<String, f64>,
    quality_map: &BTreeMap<String, (u64, u64)>,
) -> Vec<DayStat> {
    let mut all_dates: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for key in events_map.keys() {
        all_dates.insert(key.as_str());
    }
    for key in commits_map.keys() {
        all_dates.insert(key.as_str());
    }
    for key in file_edits_map.keys() {
        all_dates.insert(key.as_str());
    }
    for key in cost_map.keys() {
        all_dates.insert(key.as_str());
    }
    for key in quality_map.keys() {
        all_dates.insert(key.as_str());
    }

    all_dates
        .into_iter()
        .map(|date| {
            let (success_count, execution_count) = quality_map.get(date).copied().unwrap_or((0, 0));
            DayStat {
                date: date.to_string(),
                events: events_map.get(date).copied().unwrap_or(0),
                commits: commits_map.get(date).copied().unwrap_or(0),
                sessions: 0,
                file_edits: file_edits_map.get(date).cloned().unwrap_or_default(),
                cost_usd: cost_map.get(date).copied().unwrap_or(0.0),
                execution_count,
                success_count,
            }
        })
        .collect()
}

/// Accumulator for aggregating stats: (events, commits, file_edits, cost_usd, exec_count, success_count).
type StatAccum = (usize, usize, Vec<FileEditStat>, f64, u64, u64);

/// Build weekly stats from daily stats.
fn build_weekly_stats(daily: &[DayStat]) -> Vec<WeekStat> {
    let mut weekly_map: BTreeMap<String, StatAccum> = BTreeMap::new();

    for d in daily {
        if let Some(week_start) = iso_week_start(&d.date) {
            let entry = weekly_map
                .entry(week_start)
                .or_insert((0, 0, Vec::new(), 0.0, 0, 0));
            entry.0 += d.events;
            entry.1 += d.commits;
            merge_file_edits(&mut entry.2, &d.file_edits);
            entry.3 += d.cost_usd;
            entry.4 += d.execution_count;
            entry.5 += d.success_count;
        }
    }

    weekly_map
        .into_iter()
        .map(
            |(
                week_start,
                (events, commits, file_edits, cost_usd, execution_count, success_count),
            )| {
                WeekStat {
                    week_start,
                    events,
                    commits,
                    file_edits,
                    cost_usd,
                    execution_count,
                    success_count,
                }
            },
        )
        .collect()
}

/// Build monthly stats from daily stats.
fn build_monthly_stats(daily: &[DayStat]) -> Vec<MonthStat> {
    let mut monthly_map: BTreeMap<String, StatAccum> = BTreeMap::new();

    for d in daily {
        let month = &d.date[..7.min(d.date.len())];
        let entry = monthly_map
            .entry(month.to_string())
            .or_insert((0, 0, Vec::new(), 0.0, 0, 0));
        entry.0 += d.events;
        entry.1 += d.commits;
        merge_file_edits(&mut entry.2, &d.file_edits);
        entry.3 += d.cost_usd;
        entry.4 += d.execution_count;
        entry.5 += d.success_count;
    }

    monthly_map
        .into_iter()
        .map(
            |(month, (events, commits, file_edits, cost_usd, execution_count, success_count))| {
                MonthStat {
                    month,
                    events,
                    commits,
                    file_edits,
                    cost_usd,
                    execution_count,
                    success_count,
                }
            },
        )
        .collect()
}

/// Merge file edit statistics from `source` into `target`.
/// Sums `edit_count` and `revert_count`; takes the max of `agent_count` (approximate).
fn merge_file_edits(target: &mut Vec<FileEditStat>, source: &[FileEditStat]) {
    for src in source {
        if let Some(existing) = target.iter_mut().find(|t| t.path == src.path) {
            existing.edit_count += src.edit_count;
            existing.revert_count += src.revert_count;
            // Agent count is approximate when merging across days
            if src.agent_count > existing.agent_count {
                existing.agent_count = src.agent_count;
            }
            if src.last_edited > existing.last_edited {
                existing.last_edited.clone_from(&src.last_edited);
            }
        } else {
            target.push(src.clone());
        }
    }
}

/// Given a date string "YYYY-MM-DD", return the ISO Monday of that week as "YYYY-MM-DD".
fn iso_week_start(date_str: &str) -> Option<String> {
    let format = time::format_description::parse("[year]-[month]-[day]").ok()?;
    let date = time::Date::parse(date_str, &format).ok()?;
    let weekday = date.weekday();
    let days_since_monday = weekday.number_days_from_monday();
    let monday = date - time::Duration::days(i64::from(days_since_monday));
    monday.format(&format).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_week_start_monday() {
        // 2026-03-02 is a Monday
        assert_eq!(iso_week_start("2026-03-02"), Some("2026-03-02".to_string()));
    }

    #[test]
    fn iso_week_start_wednesday() {
        // 2026-03-04 is a Wednesday
        assert_eq!(iso_week_start("2026-03-04"), Some("2026-03-02".to_string()));
    }

    #[test]
    fn iso_week_start_sunday() {
        // 2026-03-08 is a Sunday
        assert_eq!(iso_week_start("2026-03-08"), Some("2026-03-02".to_string()));
    }

    #[test]
    fn merge_rollups_replaces_overlapping() {
        let base = Rollup {
            tool: "test".to_string(),
            last_updated: "old".to_string(),
            daily: vec![
                DayStat {
                    date: "2026-03-01".into(),
                    events: 5,
                    commits: 1,
                    ..Default::default()
                },
                DayStat {
                    date: "2026-03-02".into(),
                    events: 3,
                    commits: 0,
                    ..Default::default()
                },
            ],
            weekly: vec![],
            monthly: vec![],
        };
        let new = Rollup {
            tool: "test".to_string(),
            last_updated: "new".to_string(),
            daily: vec![
                DayStat {
                    date: "2026-03-02".into(),
                    events: 10,
                    commits: 2,
                    ..Default::default()
                },
                DayStat {
                    date: "2026-03-03".into(),
                    events: 7,
                    commits: 3,
                    ..Default::default()
                },
            ],
            weekly: vec![],
            monthly: vec![],
        };

        let merged = merge_rollups(&base, &new);
        assert_eq!(merged.daily.len(), 3);
        assert_eq!(merged.last_updated, "new");

        // March 2 should have new data (10 events, not 3)
        let mar2 = merged
            .daily
            .iter()
            .find(|d| d.date == "2026-03-02")
            .unwrap();
        assert_eq!(mar2.events, 10);
    }

    #[test]
    fn build_daily_stats_combines_maps() {
        let mut events = BTreeMap::new();
        events.insert("2026-03-01".to_string(), 5);
        events.insert("2026-03-02".to_string(), 3);

        let mut commits = BTreeMap::new();
        commits.insert("2026-03-01".to_string(), 2);
        commits.insert("2026-03-03".to_string(), 1);

        let file_edits = BTreeMap::new();
        let cost_map = BTreeMap::new();
        let quality_map = BTreeMap::new();

        let daily = build_daily_stats(&events, &commits, &file_edits, &cost_map, &quality_map);
        assert_eq!(daily.len(), 3);
        assert_eq!(daily[0].date, "2026-03-01");
        assert_eq!(daily[0].events, 5);
        assert_eq!(daily[0].commits, 2);
        assert_eq!(daily[2].date, "2026-03-03");
        assert_eq!(daily[2].commits, 1);
    }

    #[test]
    fn backward_compat_daystat_without_file_edits() {
        let json = r#"{"date":"2026-03-01","events":5,"commits":1,"sessions":0}"#;
        let stat: DayStat = serde_json::from_str(json).unwrap();
        assert_eq!(stat.date, "2026-03-01");
        assert!(stat.file_edits.is_empty());
        // New fields default to zero
        assert_eq!(stat.cost_usd, 0.0);
        assert_eq!(stat.execution_count, 0);
        assert_eq!(stat.success_count, 0);
    }

    #[test]
    fn backward_compat_weekstat_without_cost_fields() {
        let json = r#"{"week_start":"2026-03-02","events":10,"commits":3}"#;
        let stat: WeekStat = serde_json::from_str(json).unwrap();
        assert_eq!(stat.week_start, "2026-03-02");
        assert_eq!(stat.cost_usd, 0.0);
        assert_eq!(stat.execution_count, 0);
        assert_eq!(stat.success_count, 0);
    }

    #[test]
    fn build_daily_stats_includes_cost_quality() {
        let mut events = BTreeMap::new();
        events.insert("2026-03-01".to_string(), 5);

        let commits = BTreeMap::new();
        let file_edits = BTreeMap::new();

        let mut cost_map = BTreeMap::new();
        cost_map.insert("2026-03-01".to_string(), 0.05);

        let mut quality_map = BTreeMap::new();
        quality_map.insert("2026-03-01".to_string(), (8u64, 10u64));

        let daily = build_daily_stats(&events, &commits, &file_edits, &cost_map, &quality_map);
        assert_eq!(daily.len(), 1);
        assert!((daily[0].cost_usd - 0.05).abs() < 1e-9);
        assert_eq!(daily[0].execution_count, 10);
        assert_eq!(daily[0].success_count, 8);
    }

    #[test]
    fn weekly_stats_sum_cost_quality() {
        let daily = vec![
            DayStat {
                date: "2026-03-02".into(),
                events: 5,
                commits: 1,
                cost_usd: 0.10,
                execution_count: 5,
                success_count: 4,
                ..Default::default()
            },
            DayStat {
                date: "2026-03-03".into(),
                events: 3,
                commits: 0,
                cost_usd: 0.05,
                execution_count: 3,
                success_count: 3,
                ..Default::default()
            },
        ];

        let weekly = build_weekly_stats(&daily);
        assert_eq!(weekly.len(), 1);
        assert!((weekly[0].cost_usd - 0.15).abs() < 1e-9);
        assert_eq!(weekly[0].execution_count, 8);
        assert_eq!(weekly[0].success_count, 7);
    }

    #[test]
    fn merge_file_edits_sums_correctly() {
        let mut target = vec![FileEditStat {
            path: "src/main.rs".to_string(),
            edit_count: 5,
            agent_count: 1,
            last_edited: "2026-03-01T10:00:00Z".to_string(),
            revert_count: 0,
        }];

        let source = vec![FileEditStat {
            path: "src/main.rs".to_string(),
            edit_count: 3,
            agent_count: 2,
            last_edited: "2026-03-01T14:00:00Z".to_string(),
            revert_count: 0,
        }];

        merge_file_edits(&mut target, &source);

        assert_eq!(target.len(), 1);
        assert_eq!(target[0].edit_count, 8);
        assert_eq!(target[0].agent_count, 2);
        assert_eq!(target[0].last_edited, "2026-03-01T14:00:00Z");
    }
}
