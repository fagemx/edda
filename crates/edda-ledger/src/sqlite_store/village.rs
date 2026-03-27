//! Village statistics and recurring pattern detection.

use rusqlite::params;

use super::types::*;
use super::SqliteStore;

impl SqliteStore {
    /// Compute aggregate statistics for a village's decisions.
    pub fn village_stats(
        &self,
        village_id: &str,
        after: Option<&str>,
        before: Option<&str>,
    ) -> anyhow::Result<VillageStats> {
        use std::collections::HashMap;

        // Build temporal WHERE clause fragments and string params
        let mut temporal_sql = String::new();
        let mut string_params: Vec<String> = vec![village_id.to_string()];
        let mut idx = 2;

        if let Some(a) = after {
            temporal_sql.push_str(&format!(" AND e.ts >= ?{idx}"));
            string_params.push(a.to_string());
            idx += 1;
        }
        if let Some(b) = before {
            temporal_sql.push_str(&format!(" AND e.ts <= ?{idx}"));
            string_params.push(b.to_string());
            let _ = idx;
        }

        // Helper: convert string params to rusqlite param refs
        let refs = || -> Vec<&dyn rusqlite::types::ToSql> {
            string_params
                .iter()
                .map(|s| s as &dyn rusqlite::types::ToSql)
                .collect()
        };

        // Total count
        let total_sql = format!(
            "SELECT COUNT(*) FROM decisions d JOIN events e ON d.event_id = e.event_id
             WHERE d.village_id = ?1{temporal_sql}"
        );
        let total: usize = self
            .conn
            .query_row(&total_sql, refs().as_slice(), |row| row.get::<_, usize>(0))?;

        // By status
        let status_sql = format!(
            "SELECT d.status, COUNT(*) FROM decisions d JOIN events e ON d.event_id = e.event_id
             WHERE d.village_id = ?1{temporal_sql} GROUP BY d.status"
        );
        let mut status_stmt = self.conn.prepare(&status_sql)?;
        let status_rows = status_stmt.query_map(refs().as_slice(), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
        })?;
        let by_status: HashMap<String, usize> = status_rows.filter_map(|r| r.ok()).collect();

        // By authority
        let auth_sql = format!(
            "SELECT d.authority, COUNT(*) FROM decisions d JOIN events e ON d.event_id = e.event_id
             WHERE d.village_id = ?1{temporal_sql} GROUP BY d.authority"
        );
        let mut auth_stmt = self.conn.prepare(&auth_sql)?;
        let auth_rows = auth_stmt.query_map(refs().as_slice(), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
        })?;
        let by_authority: HashMap<String, usize> = auth_rows.filter_map(|r| r.ok()).collect();

        // Top domains (limit 10)
        let domain_sql = format!(
            "SELECT d.domain, COUNT(*) as cnt FROM decisions d JOIN events e ON d.event_id = e.event_id
             WHERE d.village_id = ?1{temporal_sql} GROUP BY d.domain ORDER BY cnt DESC LIMIT 10"
        );
        let mut domain_stmt = self.conn.prepare(&domain_sql)?;
        let domain_rows = domain_stmt.query_map(refs().as_slice(), |row| {
            Ok(DomainCount {
                domain: row.get(0)?,
                count: row.get(1)?,
            })
        })?;
        let top_domains: Vec<DomainCount> = domain_rows.filter_map(|r| r.ok()).collect();

        // Daily trend
        let trend_sql = format!(
            "SELECT DATE(e.ts) as day, COUNT(*) FROM decisions d JOIN events e ON d.event_id = e.event_id
             WHERE d.village_id = ?1{temporal_sql} GROUP BY day ORDER BY day"
        );
        let mut trend_stmt = self.conn.prepare(&trend_sql)?;
        let trend_rows = trend_stmt.query_map(refs().as_slice(), |row| {
            Ok(DayCount {
                date: row.get(0)?,
                count: row.get(1)?,
            })
        })?;
        let trend: Vec<DayCount> = trend_rows.filter_map(|r| r.ok()).collect();

        // Rollback rate: superseded / total
        let superseded_count = by_status.get("superseded").copied().unwrap_or(0);
        let rollback_rate = if total > 0 {
            superseded_count as f64 / total as f64
        } else {
            0.0
        };

        // Decisions per day
        let days = if trend.is_empty() {
            1.0
        } else {
            trend.len() as f64
        };
        let decisions_per_day = total as f64 / days;

        let period = if after.is_some() || before.is_some() {
            Some(VillageStatsPeriod {
                after: after.map(|s| s.to_string()),
                before: before.map(|s| s.to_string()),
            })
        } else {
            None
        };

        Ok(VillageStats {
            village_id: village_id.to_string(),
            period,
            total_decisions: total,
            decisions_per_day,
            by_status,
            by_authority,
            top_domains,
            rollback_rate,
            trend,
        })
    }

    // ── Pattern Detection ─────────────────────────────────────────────

    /// Detect recurring patterns in a village's decision history.
    ///
    /// Runs three SQL queries to find:
    /// 1. Recurring decisions (same key changed >= min_occurrences times)
    /// 2. Chief repeated actions (same authority+key >= min_occurrences times)
    /// 3. Rollback trends (supersession chains >= 2 within the window)
    pub fn detect_village_patterns(
        &self,
        village_id: &str,
        after: &str,
        min_occurrences: usize,
    ) -> anyhow::Result<Vec<DetectedPattern>> {
        let mut patterns = Vec::new();

        // Query 1: Recurring decisions — same key changed N+ times
        {
            let sql = "
                SELECT d.key, d.domain, COUNT(*) as cnt,
                       MIN(e.ts) as first_seen, MAX(e.ts) as last_seen,
                       GROUP_CONCAT(DATE(e.ts), ',') as dates
                FROM decisions d
                JOIN events e ON d.event_id = e.event_id
                WHERE d.village_id = ?1 AND e.ts >= ?2
                GROUP BY d.key, d.domain
                HAVING cnt >= ?3
                ORDER BY cnt DESC
            ";
            let mut stmt = self.conn.prepare(sql)?;
            let rows =
                stmt.query_map(params![village_id, after, min_occurrences as i64], |row| {
                    let key: String = row.get(0)?;
                    let domain: String = row.get(1)?;
                    let cnt: usize = row.get(2)?;
                    let first: String = row.get(3)?;
                    let last: String = row.get(4)?;
                    let dates_str: String = row.get::<_, Option<String>>(5)?.unwrap_or_default();
                    Ok((key, domain, cnt, first, last, dates_str))
                })?;
            for row in rows {
                let (key, domain, cnt, first, last, dates_str) = row?;
                let dates: Vec<String> = dates_str
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect();
                patterns.push(DetectedPattern {
                    pattern_type: PatternType::RecurringDecision,
                    description: format!("{key} changed {cnt} times in window"),
                    key,
                    domain,
                    authority: None,
                    occurrences: cnt,
                    first_seen: first,
                    last_seen: last,
                    dates,
                    trending_up: None,
                });
            }
        }

        // Query 2: Chief repeated actions — same authority+key N+ times
        {
            let sql = "
                SELECT d.authority, d.key, d.domain, COUNT(*) as cnt,
                       MIN(e.ts) as first_seen, MAX(e.ts) as last_seen,
                       GROUP_CONCAT(DATE(e.ts), ',') as dates
                FROM decisions d
                JOIN events e ON d.event_id = e.event_id
                WHERE d.village_id = ?1 AND e.ts >= ?2
                  AND d.authority != 'system'
                GROUP BY d.authority, d.key, d.domain
                HAVING cnt >= ?3
                ORDER BY cnt DESC
            ";
            let mut stmt = self.conn.prepare(sql)?;
            let rows =
                stmt.query_map(params![village_id, after, min_occurrences as i64], |row| {
                    let authority: String = row.get(0)?;
                    let key: String = row.get(1)?;
                    let domain: String = row.get(2)?;
                    let cnt: usize = row.get(3)?;
                    let first: String = row.get(4)?;
                    let last: String = row.get(5)?;
                    let dates_str: String = row.get::<_, Option<String>>(6)?.unwrap_or_default();
                    Ok((authority, key, domain, cnt, first, last, dates_str))
                })?;
            for row in rows {
                let (authority, key, domain, cnt, first, last, dates_str) = row?;
                let dates: Vec<String> = dates_str
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect();
                patterns.push(DetectedPattern {
                    pattern_type: PatternType::ChiefRepeatedAction,
                    description: format!("{authority} changed {key} {cnt} times in window"),
                    key,
                    domain,
                    authority: Some(authority),
                    occurrences: cnt,
                    first_seen: first,
                    last_seen: last,
                    dates,
                    trending_up: None,
                });
            }
        }

        // Query 3: Rollback trends — keys with supersession chains
        {
            let sql = "
                SELECT d.key, d.domain, COUNT(*) as cnt,
                       MIN(e.ts) as first_seen, MAX(e.ts) as last_seen,
                       GROUP_CONCAT(DATE(e.ts), ',') as dates
                FROM decisions d
                JOIN events e ON d.event_id = e.event_id
                WHERE d.village_id = ?1 AND e.ts >= ?2
                  AND d.supersedes_id IS NOT NULL
                GROUP BY d.key, d.domain
                HAVING cnt >= 2
                ORDER BY cnt DESC
            ";
            let mut stmt = self.conn.prepare(sql)?;
            let rows = stmt.query_map(params![village_id, after], |row| {
                let key: String = row.get(0)?;
                let domain: String = row.get(1)?;
                let cnt: usize = row.get(2)?;
                let first: String = row.get(3)?;
                let last: String = row.get(4)?;
                let dates_str: String = row.get::<_, Option<String>>(5)?.unwrap_or_default();
                Ok((key, domain, cnt, first, last, dates_str))
            })?;
            for row in rows {
                let (key, domain, cnt, first, last, dates_str) = row?;
                let dates: Vec<String> = dates_str
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect();

                // Trend detection: compare rollback count in first half vs second half
                let trending_up = detect_trend_direction(&dates, after);

                patterns.push(DetectedPattern {
                    pattern_type: PatternType::RollbackTrend,
                    description: format!("{key} rolled back {cnt} times in window"),
                    key,
                    domain,
                    authority: None,
                    occurrences: cnt,
                    first_seen: first,
                    last_seen: last,
                    dates,
                    trending_up: Some(trending_up),
                });
            }
        }

        Ok(patterns)
    }
}

/// Detect if rollback frequency is trending upward by comparing first half vs second half.
///
/// Sorts dates and splits at the midpoint index. Returns `true` only if the
/// second half has strictly more occurrences than the first half AND dates are
/// not all identical (a burst on one day is not a trend).
pub(super) fn detect_trend_direction(dates: &[String], _after: &str) -> bool {
    if dates.len() < 2 {
        return false;
    }

    let mut sorted: Vec<&str> = dates.iter().map(|s| s.as_str()).collect();
    sorted.sort();

    // All dates identical means a burst, not a trend
    if sorted.first() == sorted.last() {
        return false;
    }

    let mid = sorted.len() / 2;
    let mid_date = sorted[mid]; // safe: len >= 2 guarantees mid is valid
    let first_half = sorted.iter().filter(|&&d| d < mid_date).count();
    let second_half = sorted.iter().filter(|&&d| d >= mid_date).count();
    second_half > first_half
}
