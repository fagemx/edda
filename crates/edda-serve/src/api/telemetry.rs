use std::sync::Arc;

use anyhow::Context;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use edda_core::event::new_telemetry_event;
use edda_ledger::lock::WorkspaceLock;

use crate::error::AppError;
use crate::state::AppState;

// ── POST /api/telemetry ──

#[derive(Deserialize)]
struct TelemetryBody {
    cycle_id: String,
    source: String,
    started_at: String,
    total_duration_ms: u64,
    #[serde(default)]
    operations: Vec<TelemetryOp>,
    #[serde(default)]
    cost: Option<TelemetryCost>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

#[derive(Deserialize, Serialize)]
struct TelemetryOp {
    name: String,
    duration_ms: u64,
    #[serde(default)]
    token_usage: Option<TelemetryTokenUsage>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct TelemetryTokenUsage {
    input_tokens: u64,
    output_tokens: u64,
}

#[derive(Deserialize, Serialize)]
struct TelemetryCost {
    total_usd: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    breakdown: Option<Vec<serde_json::Value>>,
}

#[derive(Serialize)]
struct TelemetryResponse {
    event_id: String,
    status: String,
}

async fn post_telemetry(
    State(state): State<Arc<AppState>>,
    body: Result<Json<TelemetryBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(|e| AppError::Validation(e.to_string()))?;

    // Serialize full body as payload
    let payload = serde_json::json!({
        "cycle_id": body.cycle_id,
        "source": body.source,
        "started_at": body.started_at,
        "total_duration_ms": body.total_duration_ms,
        "operations": body.operations,
        "cost": body.cost,
        "tags": body.tags,
        "metadata": body.metadata,
    });

    let ledger = state.open_ledger().context("POST /api/telemetry")?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;

    let event = new_telemetry_event(
        &branch,
        parent_hash.as_deref(),
        &body.cycle_id,
        &body.started_at,
        payload,
    )?;

    let inserted = ledger.append_event_idempotent(&event)?;

    let response = TelemetryResponse {
        event_id: event.event_id,
        status: if inserted {
            "created".to_string()
        } else {
            "duplicate".to_string()
        },
    };

    let status = if inserted {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };

    Ok((status, Json(response)).into_response())
}

// ── GET /api/telemetry ──

#[derive(Deserialize)]
struct TelemetryQuery {
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn get_telemetry(
    State(state): State<Arc<AppState>>,
    Query(q): Query<TelemetryQuery>,
) -> Result<Response, AppError> {
    let ledger = state.open_ledger().context("GET /api/telemetry")?;
    let branch = ledger.head_branch()?;
    let limit = q.limit.unwrap_or(100);

    let events = ledger.iter_events_filtered(
        &branch,
        Some("cycle_telemetry"),
        None,
        q.after.as_deref(),
        q.before.as_deref(),
        limit,
    )?;

    let mut payloads: Vec<serde_json::Value> = events
        .into_iter()
        .map(|e| {
            let mut p = e.payload;
            // Inject event_id for cross-reference
            if let Some(obj) = p.as_object_mut() {
                obj.insert("event_id".to_string(), serde_json::json!(e.event_id));
            }
            p
        })
        .collect();

    // Post-filter by source if specified
    if let Some(ref source) = q.source {
        payloads.retain(|p| {
            p.get("source")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s == source)
        });
    }

    Ok(Json(payloads).into_response())
}

// ── GET /api/telemetry/stats ──

#[derive(Deserialize)]
struct TelemetryStatsQuery {
    #[serde(default)]
    days: Option<u32>,
    #[serde(default)]
    source: Option<String>,
}

async fn get_telemetry_stats(
    State(state): State<Arc<AppState>>,
    Query(q): Query<TelemetryStatsQuery>,
) -> Result<Response, AppError> {
    let ledger = state.open_ledger().context("GET /api/telemetry/stats")?;
    let branch = ledger.head_branch()?;
    let days = q.days.unwrap_or(7);

    // Compute "after" date
    let now = time::OffsetDateTime::now_utc();
    let after_date = now - time::Duration::days(i64::from(days));
    let after_str = after_date
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();

    let events = ledger.iter_events_filtered(
        &branch,
        Some("cycle_telemetry"),
        None,
        Some(&after_str),
        None,
        10_000,
    )?;

    let mut payloads: Vec<serde_json::Value> = events.into_iter().map(|e| e.payload).collect();

    // Post-filter by source if specified
    if let Some(ref source) = q.source {
        payloads.retain(|p| {
            p.get("source")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s == source)
        });
    }

    let stats = compute_telemetry_stats(&payloads);
    Ok(Json(stats).into_response())
}

/// Compute telemetry statistics from a set of cycle_telemetry payloads.
fn compute_telemetry_stats(payloads: &[serde_json::Value]) -> serde_json::Value {
    let cycle_count = payloads.len();
    if cycle_count == 0 {
        return serde_json::json!({
            "cycle_count": 0,
            "avg_duration_ms": 0.0,
            "p95_duration_ms": 0.0,
            "total_cost_usd": 0.0,
            "slowest_operations": [],
            "error_rate": 0.0,
        });
    }

    // Collect durations
    let mut durations: Vec<f64> = payloads
        .iter()
        .filter_map(|p| p.get("total_duration_ms").and_then(|v| v.as_f64()))
        .collect();
    durations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let avg_duration_ms = if durations.is_empty() {
        0.0
    } else {
        durations.iter().sum::<f64>() / durations.len() as f64
    };

    let p95_duration_ms = if durations.is_empty() {
        0.0
    } else {
        let idx = ((durations.len() as f64) * 0.95).ceil() as usize;
        durations[idx.min(durations.len() - 1)]
    };

    // Total cost
    let total_cost_usd: f64 = payloads
        .iter()
        .filter_map(|p| {
            p.get("cost")
                .and_then(|c| c.get("total_usd"))
                .and_then(|v| v.as_f64())
        })
        .sum();

    // Per-operation stats
    let mut op_stats: std::collections::HashMap<String, (f64, u64, usize, usize)> =
        std::collections::HashMap::new(); // (sum_dur, max_dur, count, error_count)

    let mut total_ops = 0usize;
    let mut total_errors = 0usize;

    for payload in payloads {
        if let Some(ops) = payload.get("operations").and_then(|v| v.as_array()) {
            for op in ops {
                let name = op.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
                let dur = op.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0);
                let status = op.get("status").and_then(|v| v.as_str()).unwrap_or("ok");

                let entry = op_stats.entry(name.to_string()).or_insert((0.0, 0, 0, 0));
                entry.0 += dur as f64;
                if dur > entry.1 {
                    entry.1 = dur;
                }
                entry.2 += 1;
                total_ops += 1;
                if status == "error" {
                    entry.3 += 1;
                    total_errors += 1;
                }
            }
        }
    }

    // Build slowest operations (top 5 by avg duration)
    let mut op_list: Vec<serde_json::Value> = op_stats
        .iter()
        .map(|(name, (sum, max, count, _))| {
            serde_json::json!({
                "name": name,
                "avg_duration_ms": sum / *count as f64,
                "max_duration_ms": max,
                "count": count,
            })
        })
        .collect();
    op_list.sort_by(|a, b| {
        let a_avg = a["avg_duration_ms"].as_f64().unwrap_or(0.0);
        let b_avg = b["avg_duration_ms"].as_f64().unwrap_or(0.0);
        b_avg
            .partial_cmp(&a_avg)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    op_list.truncate(5);

    let error_rate = if total_ops > 0 {
        total_errors as f64 / total_ops as f64
    } else {
        0.0
    };

    serde_json::json!({
        "cycle_count": cycle_count,
        "avg_duration_ms": avg_duration_ms,
        "p95_duration_ms": p95_duration_ms,
        "total_cost_usd": total_cost_usd,
        "slowest_operations": op_list,
        "error_rate": error_rate,
    })
}

/// Telemetry routes.
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/telemetry", post(post_telemetry).get(get_telemetry))
        .route("/api/telemetry/stats", get(get_telemetry_stats))
}
