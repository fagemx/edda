//! Pre-computed rollup statistics cached at `~/.edda/collabs/<tool>/rollup.json`.
//!
//! Supports daily, weekly, and monthly granularity with incremental updates.

use crate::aggregate::{self, DateRange};
use edda_store::registry::ProjectEntry;
use edda_store::{store_root, write_atomic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Daily statistics.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DayStat {
    pub date: String,
    pub events: usize,
    pub commits: usize,
    pub sessions: usize,
    pub file_edits: BTreeMap<String, FileEditStat>,
}

/// Weekly statistics.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WeekStat {
    pub week_start: String,
    pub events: usize,
    pub commits: usize,
    pub file_edits: BTreeMap<String, FileEditStat>,
}

/// Monthly statistics.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MonthStat {
    pub month: String,
    pub events: usize,
    pub commits: usize,
    pub file_edits: BTreeMap<String, FileEditStat>,
}

/// Per-file edit statistics.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileEditStat {
    pub edits: u64,
    pub reverts: u64,
    pub agents: usize,
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

    let daily = build_daily_stats(&events_map, &commits_map, &file_edits_map);
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
    file_edits_map: &BTreeMap<String, BTreeMap<String, FileEditStat>>,
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

    all_dates
        .into_iter()
        .map(|date| DayStat {
            date: date.to_string(),
            events: events_map.get(date).copied().unwrap_or(0),
            commits: commits_map.get(date).copied().unwrap_or(0),
            sessions: 0,
            file_edits: file_edits_map.get(date).cloned().unwrap_or_default(),
        })
        .collect()
}

/// Build weekly stats from daily stats.
fn build_weekly_stats(daily: &[DayStat]) -> Vec<WeekStat> {
    let mut weekly_map: BTreeMap<String, (usize, usize, BTreeMap<String, FileEditStat>)> =
        BTreeMap::new();

    for d in daily {
        if let Some(week_start) = iso_week_start(&d.date) {
            let entry = weekly_map.entry(week_start).or_default();
            entry.0 += d.events;
            entry.1 += d.commits;
            merge_file_edits(&mut entry.2, &d.file_edits);
        }
    }

    weekly_map
        .into_iter()
        .map(|(week_start, (events, commits, file_edits))| WeekStat {
            week_start,
            events,
            commits,
            file_edits,
        })
        .collect()
}

/// Build monthly stats from daily stats.
fn build_monthly_stats(daily: &[DayStat]) -> Vec<MonthStat> {
    let mut monthly_map: BTreeMap<String, (usize, usize, BTreeMap<String, FileEditStat>)> =
        BTreeMap::new();

    for d in daily {
        let month = &d.date[..7.min(d.date.len())]; // "YYYY-MM"
        let entry = monthly_map.entry(month.to_string()).or_default();
        entry.0 += d.events;
        entry.1 += d.commits;
        merge_file_edits(&mut entry.2, &d.file_edits);
    }

    monthly_map
        .into_iter()
        .map(|(month, (events, commits, file_edits))| MonthStat {
            month,
            events,
            commits,
            file_edits,
        })
        .collect()
}

/// Merge file edits from source into destination.
fn merge_file_edits(
    dest: &mut BTreeMap<String, FileEditStat>,
    source: &BTreeMap<String, FileEditStat>,
) {
    for (file, source_stat) in source {
        let entry = dest.entry(file.clone()).or_default();
        entry.edits += source_stat.edits;
        entry.reverts += source_stat.reverts;
        entry.agents = entry.agents.max(source_stat.agents);
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
                    sessions: 0,
                    file_edits: BTreeMap::new(),
                },
                DayStat {
                    date: "2026-03-02".into(),
                    events: 3,
                    commits: 0,
                    sessions: 0,
                    file_edits: BTreeMap::new(),
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
                    sessions: 0,
                    file_edits: BTreeMap::new(),
                },
                DayStat {
                    date: "2026-03-03".into(),
                    events: 7,
                    commits: 3,
                    sessions: 0,
                    file_edits: BTreeMap::new(),
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

        let daily = build_daily_stats(&events, &commits, &file_edits);
        assert_eq!(daily.len(), 3);
        assert_eq!(daily[0].date, "2026-03-01");
        assert_eq!(daily[0].events, 5);
        assert_eq!(daily[0].commits, 2);
        assert_eq!(daily[2].date, "2026-03-03");
        assert_eq!(daily[2].commits, 1);
    }
}
