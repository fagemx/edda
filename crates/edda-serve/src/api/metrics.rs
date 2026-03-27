use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as AxumPath, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use edda_aggregate::aggregate::{per_project_metrics, DateRange, ProjectMetrics};
use edda_aggregate::controls::evaluate_controls_rules;
use edda_aggregate::quality::{model_quality_from_events, QualityReport};
use edda_aggregate::rollup;
use edda_store::registry::list_projects;

use crate::error::AppError;
use crate::state::AppState;

use super::dashboard::DashboardPeriod;

// ── GET /api/metrics/quality ──

#[derive(Deserialize)]
struct QualityQuery {
    after: Option<String>,
    before: Option<String>,
}

async fn get_quality_metrics(
    State(state): State<Arc<AppState>>,
    Query(params): Query<QualityQuery>,
) -> Result<Json<QualityReport>, AppError> {
    let range = DateRange {
        after: params.after,
        before: params.before,
    };
    let ledger = state.open_ledger()?;
    let events = ledger.iter_events_by_type("execution_event")?;
    let report = model_quality_from_events(&events, &range);
    Ok(Json(report))
}

// ── GET /api/controls/suggestions ──

#[derive(Deserialize)]
struct ControlsSuggestionsQuery {
    after: Option<String>,
    before: Option<String>,
    min_samples: Option<u64>,
}

#[derive(Serialize)]
struct ControlsSuggestionsResponse {
    suggestions: Vec<edda_aggregate::controls::ControlsSuggestion>,
    quality: QualityReport,
}

async fn get_controls_suggestions(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ControlsSuggestionsQuery>,
) -> Result<Json<ControlsSuggestionsResponse>, AppError> {
    let range = DateRange {
        after: params.after,
        before: params.before,
    };
    let ledger = state.open_ledger()?;
    let events = ledger.iter_events_by_type("execution_event")?;
    let report = model_quality_from_events(&events, &range);

    let rules = edda_bridge_claude::controls_suggest::load_rules();
    let suggestions = evaluate_controls_rules(&rules, &report, params.min_samples);

    Ok(Json(ControlsSuggestionsResponse {
        suggestions,
        quality: report,
    }))
}

// ── GET /api/controls/patches ──

#[derive(Deserialize)]
struct ControlsPatchesQuery {
    status: Option<String>,
}

async fn get_controls_patches(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ControlsPatchesQuery>,
) -> Result<Json<Vec<edda_bridge_claude::controls_suggest::ControlsPatch>>, AppError> {
    let project_id = edda_store::project_id(&state.repo_root);

    let status_filter = match params.status.as_deref() {
        Some("pending") => Some(edda_bridge_claude::controls_suggest::PatchStatus::Pending),
        Some("approved") => Some(edda_bridge_claude::controls_suggest::PatchStatus::Approved),
        Some("dismissed") => Some(edda_bridge_claude::controls_suggest::PatchStatus::Dismissed),
        Some("applied") => Some(edda_bridge_claude::controls_suggest::PatchStatus::Applied),
        Some(s) => {
            return Err(AppError::Validation(format!(
                "Unknown status: {s} (expected: pending, approved, dismissed, applied)"
            )));
        }
        None => None,
    };

    let patches =
        edda_bridge_claude::controls_suggest::list_patches(&project_id, status_filter.as_ref())?;
    Ok(Json(patches))
}

// ── POST /api/controls/patches/{patch_id}/approve ──

#[derive(Deserialize)]
struct ApprovePatchBody {
    #[serde(default = "default_approve_actor")]
    by: String,
}

fn default_approve_actor() -> String {
    "api".to_string()
}

async fn post_approve_controls_patch(
    State(state): State<Arc<AppState>>,
    AxumPath(patch_id): AxumPath<String>,
    body: Result<Json<ApprovePatchBody>, JsonRejection>,
) -> Result<Json<edda_bridge_claude::controls_suggest::ControlsPatch>, AppError> {
    let project_id = edda_store::project_id(&state.repo_root);
    let by = match body {
        Ok(Json(b)) => b.by,
        Err(_) => "api".to_string(),
    };

    let patch = edda_bridge_claude::controls_suggest::approve_patch(&project_id, &patch_id, &by)?;
    Ok(Json(patch))
}

// ── GET /api/metrics/overview ──

fn default_overview_days() -> usize {
    30
}

#[derive(Deserialize)]
struct MetricsOverviewQuery {
    #[serde(default = "default_overview_days")]
    days: usize,
    group: Option<String>,
}

#[derive(Serialize)]
struct MetricsOverviewResponse {
    period: DashboardPeriod,
    projects: Vec<ProjectMetrics>,
    totals: MetricsTotals,
}

#[derive(Serialize)]
struct MetricsTotals {
    total_cost_usd: f64,
    total_events: usize,
    total_commits: usize,
    total_steps: u64,
    overall_success_rate: f64,
}

async fn get_metrics_overview(
    State(state): State<Arc<AppState>>,
    Query(params): Query<MetricsOverviewQuery>,
) -> Result<Json<MetricsOverviewResponse>, AppError> {
    if state.chronicle.is_none() {
        return Err(AppError::NotImplemented(
            "chronicle feature not enabled".into(),
        ));
    }

    let all_projects = list_projects();
    let projects: Vec<_> = if let Some(ref group) = params.group {
        all_projects
            .into_iter()
            .filter(|p| p.group.as_deref() == Some(group.as_str()))
            .collect()
    } else {
        all_projects
    };

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

    let metrics = per_project_metrics(&projects, &range, params.days);

    let total_cost: f64 = metrics.iter().map(|m| m.cost.total_usd).sum();
    let total_events: usize = metrics.iter().map(|m| m.activity.events).sum();
    let total_commits: usize = metrics.iter().map(|m| m.activity.commits).sum();
    let total_steps: u64 = metrics.iter().map(|m| m.quality.total_steps).sum();
    let total_success: u64 = metrics
        .iter()
        .map(|m| (m.quality.success_rate * m.quality.total_steps as f64) as u64)
        .sum();

    let period = DashboardPeriod {
        from: from_str[..10].to_string(),
        to: to_str[..10].to_string(),
        days: params.days,
    };

    Ok(Json(MetricsOverviewResponse {
        period,
        projects: metrics,
        totals: MetricsTotals {
            total_cost_usd: total_cost,
            total_events,
            total_commits,
            total_steps,
            overall_success_rate: if total_steps > 0 {
                total_success as f64 / total_steps as f64
            } else {
                0.0
            },
        },
    }))
}

// ── GET /api/metrics/trends ──

fn default_trend_granularity() -> String {
    "daily".to_string()
}

#[derive(Deserialize)]
struct TrendsQuery {
    #[serde(default = "default_overview_days")]
    days: usize,
    #[serde(default = "default_trend_granularity")]
    granularity: String,
    group: Option<String>,
}

#[derive(Serialize)]
struct TrendsResponse {
    granularity: String,
    data: Vec<TrendPoint>,
}

#[derive(Serialize)]
struct TrendPoint {
    date: String,
    events: usize,
    commits: usize,
    cost_usd: f64,
    execution_count: u64,
    success_count: u64,
    success_rate: f64,
}

async fn get_metrics_trends(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TrendsQuery>,
) -> Result<Json<TrendsResponse>, AppError> {
    if state.chronicle.is_none() {
        return Err(AppError::NotImplemented(
            "chronicle feature not enabled".into(),
        ));
    }

    let all_projects = list_projects();
    let projects: Vec<_> = if let Some(ref group) = params.group {
        all_projects
            .into_iter()
            .filter(|p| p.group.as_deref() == Some(group.as_str()))
            .collect()
    } else {
        all_projects
    };

    let now = time::OffsetDateTime::now_utc();
    let from_date = now - time::Duration::days(params.days as i64);
    let from_str = from_date
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();

    let range = DateRange {
        after: Some(from_str[..10].to_string()),
        before: None,
    };

    let r = rollup::compute_rollup(&projects, &range, "edda");

    let data: Vec<TrendPoint> = match params.granularity.as_str() {
        "weekly" => r
            .weekly
            .iter()
            .map(|w| TrendPoint {
                date: w.week_start.clone(),
                events: w.events,
                commits: w.commits,
                cost_usd: w.cost_usd,
                execution_count: w.execution_count,
                success_count: w.success_count,
                success_rate: if w.execution_count > 0 {
                    w.success_count as f64 / w.execution_count as f64
                } else {
                    0.0
                },
            })
            .collect(),
        "monthly" => r
            .monthly
            .iter()
            .map(|m| TrendPoint {
                date: m.month.clone(),
                events: m.events,
                commits: m.commits,
                cost_usd: m.cost_usd,
                execution_count: m.execution_count,
                success_count: m.success_count,
                success_rate: if m.execution_count > 0 {
                    m.success_count as f64 / m.execution_count as f64
                } else {
                    0.0
                },
            })
            .collect(),
        _ => r
            .daily
            .iter()
            .map(|d| TrendPoint {
                date: d.date.clone(),
                events: d.events,
                commits: d.commits,
                cost_usd: d.cost_usd,
                execution_count: d.execution_count,
                success_count: d.success_count,
                success_rate: if d.execution_count > 0 {
                    d.success_count as f64 / d.execution_count as f64
                } else {
                    0.0
                },
            })
            .collect(),
    };

    Ok(Json(TrendsResponse {
        granularity: params.granularity,
        data,
    }))
}

/// Metrics-related routes (quality, controls, overview, trends).
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/metrics/quality", get(get_quality_metrics))
        .route("/api/metrics/overview", get(get_metrics_overview))
        .route("/api/metrics/trends", get(get_metrics_trends))
        .route("/api/controls/suggestions", get(get_controls_suggestions))
        .route("/api/controls/patches", get(get_controls_patches))
        .route(
            "/api/controls/patches/{patch_id}/approve",
            post(post_approve_controls_patch),
        )
}
