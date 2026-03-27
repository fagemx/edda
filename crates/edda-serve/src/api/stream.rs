use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event as SseEvent, KeepAlive};
use axum::response::Sse;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

use crate::error::AppError;
use crate::state::AppState;

// ── SSE Event Stream ──

/// Query parameters for the SSE event stream endpoint.
#[derive(Deserialize)]
struct StreamParams {
    /// Comma-separated event types to subscribe to (e.g. "decision,phase_change").
    /// If omitted, all event types are streamed.
    types: Option<String>,
    /// Resume from this event_id (alternative to `Last-Event-ID` header).
    since: Option<String>,
}

/// Map a ledger event to the SSE event name sent to clients.
///
/// Decisions are stored as `note` events with a `decision` key in the payload,
/// so we check the payload in addition to the `event_type` field.
fn sse_event_name(event: &edda_core::Event) -> &'static str {
    match event.event_type.as_str() {
        "agent_phase_change" => "phase_change",
        "approval_request" => "approval_pending",
        "note" if event.payload.get("decision").is_some() => "decision",
        _ => "new_event",
    }
}

/// `GET /api/events/stream` — Server-Sent Events endpoint.
///
/// Streams new ledger events in real time using a poll-based approach
/// (queries SQLite rowid cursor every 2 seconds).
///
/// Supports:
/// - `?types=decision,phase_change` — filter by SSE event type
/// - `?since=evt_xxx` or `Last-Event-ID` header — resume after disconnect
/// - 30-second keep-alive heartbeat
async fn get_event_stream(
    State(state): State<Arc<AppState>>,
    Query(params): Query<StreamParams>,
    headers: HeaderMap,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<SseEvent, Infallible>>>, AppError> {
    // Determine the resume cursor: query param takes precedence over header.
    let since = params.since.or_else(|| {
        headers
            .get("Last-Event-ID")
            .and_then(|v| v.to_str().ok())
            .map(String::from)
    });

    // Parse type filter into a set for O(1) lookups.
    let type_filter: Option<Vec<String>> = params.types.map(|t| {
        t.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });

    // Resolve the initial cursor (rowid) from `since` event_id.
    let mut cursor: i64 = if let Some(ref event_id) = since {
        let ledger = state.open_ledger().context("GET /api/events/stream")?;
        ledger.rowid_for_event_id(event_id)?.unwrap_or(0)
    } else {
        0
    };

    let repo_root = state.repo_root.clone();

    let stream = async_stream::stream! {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            interval.tick().await;

            let ledger = match edda_ledger::Ledger::open(&repo_root) {
                Ok(l) => l,
                Err(_) => continue,
            };

            let new_events = match ledger.events_after_rowid(cursor) {
                Ok(evts) => evts,
                Err(_) => continue,
            };

            if new_events.is_empty() {
                continue;
            }

            // Update cursor to the latest rowid.
            if let Some((last_rowid, _)) = new_events.last() {
                cursor = *last_rowid;
            }

            for (_rowid, event) in new_events {
                let sse_name = sse_event_name(&event);

                // Apply type filter if specified.
                if let Some(ref filters) = type_filter {
                    if !filters.iter().any(|f| f == sse_name) {
                        continue;
                    }
                }

                let event_id = event.event_id.clone();
                let data = serde_json::json!({
                    "event_type": sse_name,
                    "data": serde_json::to_value(&event).unwrap_or_default(),
                    "ts": &event.ts,
                });

                let sse_event = SseEvent::default()
                    .event(sse_name)
                    .id(event_id)
                    .json_data(data)
                    .unwrap_or_else(|_| SseEvent::default().comment("serialization error"));

                yield Ok::<_, Infallible>(sse_event);
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("ping"),
    ))
}

/// SSE event stream routes.
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/events/stream", get(get_event_stream))
}
