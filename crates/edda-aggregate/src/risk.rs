//! Override risk scoring for active decisions.
//!
//! Each decision receives a `risk_score` (0.0..1.0) computed from five weighted
//! factors: downstream dependents, age, execution references, cross-project
//! usage, and approval status.

use edda_core::Event;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Risk assessment for a single decision.
#[derive(Debug, Clone, Serialize)]
pub struct DecisionRisk {
    pub event_id: String,
    pub key: String,
    pub value: String,
    pub project: String,
    pub risk_score: f64,
    pub risk_level: String,
    pub factors: RiskFactors,
}

/// Individual risk factor values used to compute the overall score.
#[derive(Debug, Clone, Serialize)]
pub struct RiskFactors {
    pub downstream_count: usize,
    pub age_days: u32,
    pub execution_refs: usize,
    pub cross_project: bool,
    pub has_approval: bool,
}

/// Input describing one active decision for risk computation.
pub struct DecisionInput {
    pub event_id: String,
    pub key: String,
    pub value: String,
    pub project: String,
    pub ts: Option<String>,
}

/// Compute risk scores for a set of decisions given all events across projects.
///
/// `now_iso` should be an ISO 8601 timestamp used as the reference point for
/// age calculations (e.g. `"2026-03-12T00:00:00Z"`).
pub fn compute_decision_risks(
    decisions: &[DecisionInput],
    all_events: &[Event],
    now_iso: &str,
    cross_project_ids: &HashSet<String>,
) -> Vec<DecisionRisk> {
    // Pre-compute: how many provenance refs target each event_id
    let mut downstream_counts: HashMap<&str, usize> = HashMap::new();
    let mut execution_counts: HashMap<&str, usize> = HashMap::new();
    let mut approval_set: HashSet<&str> = HashSet::new();

    for event in all_events {
        // Count provenance links targeting decision events
        for prov in &event.refs.provenance {
            *downstream_counts.entry(prov.target.as_str()).or_insert(0) += 1;
        }

        // Count execution events referencing decisions
        if event.event_type == "execution_event" {
            for prov in &event.refs.provenance {
                *execution_counts.entry(prov.target.as_str()).or_insert(0) += 1;
            }
        }

        // Track approved decisions (approval events referencing a decision)
        if event.event_type == "approval" {
            for prov in &event.refs.provenance {
                approval_set.insert(prov.target.as_str());
            }
            for eid in &event.refs.events {
                approval_set.insert(eid.as_str());
            }
        }
    }

    decisions
        .iter()
        .map(|d| {
            let downstream_count =
                downstream_counts.get(d.event_id.as_str()).copied().unwrap_or(0);
            let execution_refs =
                execution_counts.get(d.event_id.as_str()).copied().unwrap_or(0);
            let cross_project = cross_project_ids.contains(&d.event_id);
            let has_approval = approval_set.contains(d.event_id.as_str());
            let age_days = compute_age_days(d.ts.as_deref(), now_iso);

            let factors = RiskFactors {
                downstream_count,
                age_days,
                execution_refs,
                cross_project,
                has_approval,
            };

            let risk_score = score_from_factors(&factors);
            let risk_level = level_from_score(risk_score).to_string();

            DecisionRisk {
                event_id: d.event_id.clone(),
                key: d.key.clone(),
                value: d.value.clone(),
                project: d.project.clone(),
                risk_score,
                risk_level,
                factors,
            }
        })
        .collect()
}

/// Weighted risk formula.
fn score_from_factors(f: &RiskFactors) -> f64 {
    let downstream = (f.downstream_count as f64 / 5.0).min(1.0) * 0.3;
    let age = (f.age_days as f64 / 30.0).min(1.0) * 0.15;
    let exec = (f.execution_refs as f64 / 10.0).min(1.0) * 0.25;
    let cross = if f.cross_project { 0.2 } else { 0.0 };
    // Approved decisions carry higher override risk because they have
    // organizational backing — overriding them has greater organizational impact.
    let approval = if f.has_approval { 0.1 } else { 0.0 };
    downstream + age + exec + cross + approval
}

/// Map score to human-readable level.
fn level_from_score(score: f64) -> &'static str {
    if score > 0.6 {
        "high"
    } else if score >= 0.3 {
        "medium"
    } else {
        "low"
    }
}

/// Compute age in days from an ISO 8601 timestamp to `now_iso`.
fn compute_age_days(ts: Option<&str>, now_iso: &str) -> u32 {
    let Some(ts) = ts else { return 0 };
    // Simple date-only comparison using the first 10 chars (YYYY-MM-DD)
    let ts_date = &ts[..10.min(ts.len())];
    let now_date = &now_iso[..10.min(now_iso.len())];

    let parse = |d: &str| -> Option<(i32, u32, u32)> {
        let parts: Vec<&str> = d.split('-').collect();
        if parts.len() < 3 {
            return None;
        }
        Some((
            parts[0].parse().ok()?,
            parts[1].parse().ok()?,
            parts[2].parse().ok()?,
        ))
    };

    let Some((y1, m1, d1)) = parse(ts_date) else {
        return 0;
    };
    let Some((y2, m2, d2)) = parse(now_date) else {
        return 0;
    };

    // Rough day calculation (good enough for risk scoring)
    let days1 = y1 * 365 + m1 as i32 * 30 + d1 as i32;
    let days2 = y2 * 365 + m2 as i32 * 30 + d2 as i32;
    (days2 - days1).max(0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use edda_core::types::Refs;

    fn make_decision(event_id: &str, key: &str, value: &str, ts: &str) -> DecisionInput {
        DecisionInput {
            event_id: event_id.to_string(),
            key: key.to_string(),
            value: value.to_string(),
            project: "test-project".to_string(),
            ts: Some(ts.to_string()),
        }
    }

    fn make_event(event_type: &str, refs: Refs) -> Event {
        Event {
            event_id: format!("evt_{}", uuid_stub()),
            ts: "2026-03-12T00:00:00Z".to_string(),
            event_type: event_type.to_string(),
            branch: "main".to_string(),
            parent_hash: None,
            hash: String::new(),
            payload: serde_json::json!({}),
            refs,
            schema_version: 1,
            digests: vec![],
            event_family: None,
            event_level: None,
        }
    }

    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    fn uuid_stub() -> u32 {
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    #[test]
    fn risk_score_zero_for_isolated_new_decision() {
        let decisions = vec![make_decision("evt_1", "db.engine", "sqlite", "2026-03-12")];
        let risks =
            compute_decision_risks(&decisions, &[], "2026-03-12T00:00:00Z", &HashSet::new());
        assert_eq!(risks.len(), 1);
        assert_eq!(risks[0].risk_score, 0.0);
        assert_eq!(risks[0].risk_level, "low");
    }

    #[test]
    fn risk_score_high_for_old_decision_with_many_refs() {
        let decisions = vec![make_decision(
            "evt_old",
            "auth.strategy",
            "JWT",
            "2025-01-01",
        )];

        // Create many downstream refs and execution events
        let mut events = Vec::new();
        for _ in 0..6 {
            events.push(make_event(
                "decision",
                Refs {
                    provenance: vec![edda_core::types::Provenance {
                        target: "evt_old".to_string(),
                        rel: "supersedes".to_string(),
                        note: None,
                    }],
                    ..Default::default()
                },
            ));
        }
        for _ in 0..12 {
            events.push(make_event(
                "execution_event",
                Refs {
                    provenance: vec![edda_core::types::Provenance {
                        target: "evt_old".to_string(),
                        rel: "based_on".to_string(),
                        note: None,
                    }],
                    ..Default::default()
                },
            ));
        }

        let mut cross = HashSet::new();
        cross.insert("evt_old".to_string());

        let risks = compute_decision_risks(&decisions, &events, "2026-03-12T00:00:00Z", &cross);
        assert_eq!(risks[0].risk_level, "high");
        assert!(risks[0].risk_score > 0.6);
    }

    #[test]
    fn risk_level_thresholds() {
        assert_eq!(level_from_score(0.0), "low");
        assert_eq!(level_from_score(0.29), "low");
        assert_eq!(level_from_score(0.3), "medium");
        assert_eq!(level_from_score(0.6), "medium");
        assert_eq!(level_from_score(0.61), "high");
        assert_eq!(level_from_score(1.0), "high");
    }

    #[test]
    fn cross_project_flag_increases_risk() {
        let decisions = vec![make_decision("evt_cp", "api.version", "v2", "2026-03-12")];

        let risks_no_cross =
            compute_decision_risks(&decisions, &[], "2026-03-12T00:00:00Z", &HashSet::new());

        let mut cross = HashSet::new();
        cross.insert("evt_cp".to_string());
        let risks_cross =
            compute_decision_risks(&decisions, &[], "2026-03-12T00:00:00Z", &cross);

        assert!(risks_cross[0].risk_score > risks_no_cross[0].risk_score);
        assert_eq!(
            risks_cross[0].risk_score - risks_no_cross[0].risk_score,
            0.2
        );
    }
}
