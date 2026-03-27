use std::sync::Arc;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use edda_aggregate::aggregate::{
    aggregate_decisions, aggregate_overview, per_project_metrics, DateRange, ProjectMetrics,
};
use edda_aggregate::graph::build_dependency_graph;
use edda_aggregate::risk::{compute_decision_risks, DecisionInput, DecisionRisk};
use edda_ledger::Ledger;
use edda_store::registry::list_projects;

use crate::error::AppError;
use crate::state::AppState;

use super::analytics::{OverviewGreenItem, OverviewRedItem, OverviewResponse, OverviewYellowItem};

// ── GET /api/dashboard ──

#[derive(Deserialize)]
struct DashboardQuery {
    #[serde(default = "default_days")]
    days: usize,
}

fn default_days() -> usize {
    7
}

#[derive(Serialize)]
struct DashboardResponse {
    period: DashboardPeriod,
    summary: DashboardSummary,
    attention: OverviewResponse,
    timeline: Vec<TimelineEntry>,
    graph: edda_aggregate::graph::DependencyGraph,
    risks: Vec<DecisionRisk>,
    project_metrics: Vec<ProjectMetrics>,
}

#[derive(Serialize)]
pub(crate) struct DashboardPeriod {
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) days: usize,
}

#[derive(Serialize)]
struct DashboardSummary {
    total_projects: usize,
    total_decisions: usize,
    total_events: usize,
    total_commits: usize,
    total_cost_usd: f64,
    overall_success_rate: f64,
}

#[derive(Serialize)]
struct TimelineEntry {
    ts: String,
    event_type: String,
    key: String,
    value: String,
    reason: String,
    project: String,
    risk_level: String,
    supersedes: Option<String>,
}

async fn get_dashboard(
    State(_state): State<Arc<AppState>>,
    Query(params): Query<DashboardQuery>,
) -> Result<Json<DashboardResponse>, AppError> {
    let projects = list_projects();

    let now = time::OffsetDateTime::now_utc();
    let from_date = now - time::Duration::days(params.days as i64);
    let to_str = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let from_str = from_date
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();

    let range = DateRange {
        after: Some(from_str[..10].to_string()),
        before: None,
    };

    // Summary
    let agg = aggregate_overview(&projects, &range);

    // Decisions + risk scoring
    let decisions = aggregate_decisions(&projects);
    let now_iso = &to_str;

    let decision_inputs: Vec<DecisionInput> = decisions
        .iter()
        .map(|d| DecisionInput {
            event_id: d.event_id.clone(),
            key: d.key.clone(),
            value: d.value.clone(),
            project: d.project_name.clone(),
            ts: d.ts.clone(),
        })
        .collect();

    // Collect all events for risk computation
    // TODO: This event-loading block is duplicated in get_overview; extract into a shared helper in a follow-up.
    let mut all_events = Vec::new();
    for entry in &projects {
        let root = std::path::Path::new(&entry.path);
        if let Ok(ledger) = Ledger::open(root) {
            if let Ok(events) = ledger.iter_events() {
                all_events.extend(events);
            }
        }
    }

    // Cross-project: decision IDs that appear in provenance of events from OTHER projects
    let mut cross_project_ids = std::collections::HashSet::new();
    for entry in &projects {
        let root = std::path::Path::new(&entry.path);
        if let Ok(ledger) = Ledger::open(root) {
            if let Ok(events) = ledger.iter_events() {
                for event in &events {
                    for prov in &event.refs.provenance {
                        // If this event references a decision from another project
                        for d in &decisions {
                            if d.event_id == prov.target && d.project_name != entry.name {
                                cross_project_ids.insert(d.event_id.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    let risks = compute_decision_risks(&decision_inputs, &all_events, now_iso, &cross_project_ids);

    // Build risk lookup for timeline entries
    let risk_map: std::collections::HashMap<&str, &str> = risks
        .iter()
        .map(|r| (r.event_id.as_str(), r.risk_level.as_str()))
        .collect();

    // Timeline: decisions sorted by timestamp descending
    let mut timeline: Vec<TimelineEntry> = decisions
        .iter()
        .map(|d| {
            let risk_level = risk_map
                .get(d.event_id.as_str())
                .unwrap_or(&"low")
                .to_string();
            TimelineEntry {
                ts: d.ts.clone().unwrap_or_default(),
                event_type: "decision".to_string(),
                key: d.key.clone(),
                value: d.value.clone(),
                reason: d.reason.clone(),
                project: d.project_name.clone(),
                risk_level,
                supersedes: None, // Would need provenance walk
            }
        })
        .collect();
    timeline.sort_by(|a, b| b.ts.cmp(&a.ts));

    // Dependency graph
    let graph = build_dependency_graph(&projects);

    // Per-project metrics
    let project_metrics = per_project_metrics(&projects, &range, params.days);

    // Compute cost totals for summary
    let total_cost: f64 = project_metrics.iter().map(|m| m.cost.total_usd).sum();
    let total_steps: u64 = project_metrics.iter().map(|m| m.quality.total_steps).sum();
    let total_success: u64 = project_metrics
        .iter()
        .map(|m| (m.quality.success_rate * m.quality.total_steps as f64) as u64)
        .sum();
    let overall_success_rate = if total_steps > 0 {
        total_success as f64 / total_steps as f64
    } else {
        0.0
    };

    // Attention routing (with cost anomaly detection)
    let attention = compute_attention(&risks, &projects, &range, &project_metrics, params.days);

    let period = DashboardPeriod {
        from: from_str[..10].to_string(),
        to: to_str[..10].to_string(),
        days: params.days,
    };

    let summary = DashboardSummary {
        total_projects: agg.projects.len(),
        total_decisions: agg.total_decisions,
        total_events: agg.total_events,
        total_commits: agg.total_commits,
        total_cost_usd: total_cost,
        overall_success_rate,
    };

    Ok(Json(DashboardResponse {
        period,
        summary,
        attention,
        timeline,
        graph,
        risks,
        project_metrics,
    }))
}

/// Compute attention routing: red / yellow / green classification.
///
/// Includes cost anomaly detection when `project_metrics` is non-empty:
/// - Yellow: project daily cost > 2x period average
/// - Red: project daily cost > 5x period average
pub(crate) fn compute_attention(
    risks: &[DecisionRisk],
    projects: &[edda_store::registry::ProjectEntry],
    range: &DateRange,
    project_metrics: &[ProjectMetrics],
    days: usize,
) -> OverviewResponse {
    let mut red = Vec::new();
    let mut yellow = Vec::new();
    let mut green = Vec::new();

    let now = time::OffsetDateTime::now_utc();
    let updated_at = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string());

    // Red: high-risk decisions
    for r in risks {
        if r.risk_level == "high" {
            red.push(OverviewRedItem {
                project: r.project.clone(),
                summary: format!(
                    "{} = {} (risk {:.0}%)",
                    r.key,
                    r.value,
                    r.risk_score * 100.0
                ),
                action: "Review before overriding".to_string(),
                blocked_count: 0,
            });
        }
    }

    // Yellow: medium-risk decisions
    for r in risks {
        if r.risk_level == "medium" {
            yellow.push(OverviewYellowItem {
                project: r.project.clone(),
                summary: format!(
                    "{} = {} (risk {:.0}%)",
                    r.key,
                    r.value,
                    r.risk_score * 100.0
                ),
                eta: String::new(),
            });
        }
    }

    // Cost anomaly detection
    if days > 0 {
        for pm in project_metrics {
            let daily_avg = pm.cost.daily_avg_usd;
            if daily_avg > 0.0 && pm.cost.last_day_usd > 0.0 {
                // Use the actual most-recent-day cost from rollup data
                let last_day_cost = pm.cost.last_day_usd;
                if last_day_cost > daily_avg * 5.0 {
                    red.push(OverviewRedItem {
                        project: pm.name.clone(),
                        summary: format!(
                            "Cost spike: ${:.2}/day (5x above ${:.2} avg)",
                            last_day_cost, daily_avg
                        ),
                        action: "Investigate cost increase".to_string(),
                        blocked_count: 0,
                    });
                } else if last_day_cost > daily_avg * 2.0 {
                    yellow.push(OverviewYellowItem {
                        project: pm.name.clone(),
                        summary: format!(
                            "Elevated cost: ${:.2}/day (2x above ${:.2} avg)",
                            last_day_cost, daily_avg
                        ),
                        eta: String::new(),
                    });
                }
            }
        }
    }

    // Red: stale projects (no events in range)
    for entry in projects {
        let root = std::path::Path::new(&entry.path);
        let has_events = Ledger::open(root)
            .and_then(|l| l.iter_events())
            .map(|events| events.iter().any(|e| range.matches(&e.ts)))
            .unwrap_or(false);
        if !has_events {
            red.push(OverviewRedItem {
                project: entry.name.clone(),
                summary: "No activity in period".to_string(),
                action: "Check project status".to_string(),
                blocked_count: 0,
            });
        }
    }

    // Green: projects with normal activity
    for entry in projects {
        let root = std::path::Path::new(&entry.path);
        let has_events = Ledger::open(root)
            .and_then(|l| l.iter_events())
            .map(|events| events.iter().any(|e| range.matches(&e.ts)))
            .unwrap_or(false);
        if has_events {
            let high_risk = risks
                .iter()
                .any(|r| r.project == entry.name && r.risk_level == "high");
            if !high_risk {
                green.push(OverviewGreenItem {
                    project: entry.name.clone(),
                    summary: "Normal activity".to_string(),
                });
            }
        }
    }

    OverviewResponse {
        red,
        yellow,
        green,
        updated_at,
    }
}

/// Dashboard routes.
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/dashboard", get(get_dashboard))
}
