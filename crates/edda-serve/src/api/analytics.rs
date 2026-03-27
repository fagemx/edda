use std::sync::Arc;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use edda_aggregate::aggregate::{aggregate_decisions, DateRange};
use edda_aggregate::risk::{compute_decision_risks, DecisionInput};
use edda_ledger::Ledger;
use edda_store::registry::list_projects;

use crate::error::AppError;
use crate::state::AppState;

use super::dashboard::compute_attention;

// ── GET /api/recap ──

#[derive(Deserialize)]
struct RecapQuery {
    project: Option<String>,
    query: Option<String>,
    #[serde(rename = "since")]
    _since: Option<String>,
    week: Option<bool>,
    scope: Option<String>,
}

#[derive(Serialize)]
struct RecapAnchor {
    #[serde(rename = "type")]
    anchor_type: String,
    value: String,
}

#[derive(Serialize)]
struct NeedsYouItem {
    severity: String,
    summary: String,
    action: String,
}

#[derive(Serialize)]
struct DecisionItem {
    key: String,
    value: String,
    reason: String,
}

#[derive(Serialize)]
struct RelatedItem {
    summary: String,
    relevance: String,
}

#[derive(Serialize)]
struct RecapLayers {
    net_result: String,
    needs_you: Vec<NeedsYouItem>,
    decisions: Vec<DecisionItem>,
    related: Vec<RelatedItem>,
}

#[derive(Serialize)]
struct RecapMeta {
    sessions_analyzed: usize,
    llm_used: bool,
    cached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fallback: Option<String>,
}

#[derive(Serialize)]
struct RecapResponse {
    anchor: RecapAnchor,
    layers: RecapLayers,
    meta: RecapMeta,
}

async fn get_recap(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RecapQuery>,
) -> Result<Json<RecapResponse>, AppError> {
    if state.chronicle.is_none() {
        return Err(anyhow::anyhow!("chronicle feature not enabled").into());
    }

    let anchor = if let Some(ref project) = params.project {
        RecapAnchor {
            anchor_type: "project".to_string(),
            value: project.clone(),
        }
    } else if let Some(ref query) = params.query {
        RecapAnchor {
            anchor_type: "query".to_string(),
            value: query.clone(),
        }
    } else if params.week.unwrap_or(false) {
        RecapAnchor {
            anchor_type: "time".to_string(),
            value: "week".to_string(),
        }
    } else if params.scope.as_deref() == Some("all") {
        RecapAnchor {
            anchor_type: "scope".to_string(),
            value: "all".to_string(),
        }
    } else {
        RecapAnchor {
            anchor_type: "default".to_string(),
            value: "current".to_string(),
        }
    };

    // TODO: Replace with actual edda-chronicle integration when #173 is complete
    // For now, return a stub response
    let response = RecapResponse {
        anchor,
        layers: RecapLayers {
            net_result: "Recap engine not yet integrated (depends on #173)".to_string(),
            needs_you: vec![],
            decisions: vec![],
            related: vec![],
        },
        meta: RecapMeta {
            sessions_analyzed: 0,
            llm_used: false,
            cached: false,
            cost_usd: None,
            fallback: Some("stub".to_string()),
        },
    };

    Ok(Json(response))
}

// ── GET /api/recap/cached ──

#[derive(Deserialize)]
struct RecapCachedQuery {
    project: Option<String>,
}

async fn get_recap_cached(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RecapCachedQuery>,
) -> Result<Json<RecapResponse>, AppError> {
    if state.chronicle.is_none() {
        return Err(anyhow::anyhow!("chronicle feature not enabled").into());
    }

    let anchor = if let Some(ref project) = params.project {
        RecapAnchor {
            anchor_type: "project".to_string(),
            value: project.clone(),
        }
    } else {
        RecapAnchor {
            anchor_type: "default".to_string(),
            value: "current".to_string(),
        }
    };

    // TODO: Replace with actual cache lookup when #176 is complete
    // For now, return a 404-like response
    let response = RecapResponse {
        anchor,
        layers: RecapLayers {
            net_result: "No cached recap available".to_string(),
            needs_you: vec![],
            decisions: vec![],
            related: vec![],
        },
        meta: RecapMeta {
            sessions_analyzed: 0,
            llm_used: false,
            cached: true,
            cost_usd: None,
            fallback: Some("cache_miss".to_string()),
        },
    };

    Ok(Json(response))
}

// ── GET /api/overview ──

#[derive(Serialize)]
pub(crate) struct OverviewRedItem {
    pub(crate) project: String,
    pub(crate) summary: String,
    pub(crate) action: String,
    pub(crate) blocked_count: usize,
}

#[derive(Serialize)]
pub(crate) struct OverviewYellowItem {
    pub(crate) project: String,
    pub(crate) summary: String,
    pub(crate) eta: String,
}

#[derive(Serialize)]
pub(crate) struct OverviewGreenItem {
    pub(crate) project: String,
    pub(crate) summary: String,
}

#[derive(Serialize)]
pub(crate) struct OverviewResponse {
    pub(crate) red: Vec<OverviewRedItem>,
    pub(crate) yellow: Vec<OverviewYellowItem>,
    pub(crate) green: Vec<OverviewGreenItem>,
    pub(crate) updated_at: String,
}

async fn get_overview(
    State(state): State<Arc<AppState>>,
) -> Result<Json<OverviewResponse>, AppError> {
    if state.chronicle.is_none() {
        return Err(anyhow::anyhow!("chronicle feature not enabled").into());
    }

    let projects = list_projects();
    let range = DateRange {
        after: Some({
            let now = time::OffsetDateTime::now_utc();
            let from = now - time::Duration::days(7);
            from.format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default()[..10]
                .to_string()
        }),
        before: None,
    };

    // Compute decisions + risks for attention routing
    let decisions = aggregate_decisions(&projects);
    let now_iso = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
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

    // TODO: This event-loading block is duplicated in get_dashboard; extract into a shared helper in a follow-up.
    let mut all_events = Vec::new();
    for entry in &projects {
        let root = std::path::Path::new(&entry.path);
        if let Ok(ledger) = Ledger::open(root) {
            if let Ok(events) = ledger.iter_events() {
                all_events.extend(events);
            }
        }
    }

    let risks = compute_decision_risks(
        &decision_inputs,
        &all_events,
        &now_iso,
        &std::collections::HashSet::new(),
    );

    let response = compute_attention(&risks, &projects, &range, &[], 7);
    Ok(Json(response))
}

// ── GET /api/projects ──

#[derive(Serialize)]
struct ProjectStatus {
    name: String,
    id: String,
    last_activity: String,
    status: String,
}

#[derive(Serialize)]
struct ProjectsResponse {
    projects: Vec<ProjectStatus>,
}

async fn get_projects(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ProjectsResponse>, AppError> {
    if state.chronicle.is_none() {
        return Err(anyhow::anyhow!("chronicle feature not enabled").into());
    }

    let projects = list_projects();
    let project_statuses: Vec<ProjectStatus> = projects
        .into_iter()
        .map(|p| ProjectStatus {
            name: p.name,
            id: p.project_id,
            last_activity: p.last_seen,
            status: "unknown".to_string(), // TODO: Calculate from overview
        })
        .collect();

    Ok(Json(ProjectsResponse {
        projects: project_statuses,
    }))
}

/// Analytics routes (recap, overview, projects).
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/recap", get(get_recap))
        .route("/api/recap/cached", get(get_recap_cached))
        .route("/api/overview", get(get_overview))
        .route("/api/projects", get(get_projects))
}
