use std::fs;

use super::helpers::{parse_rfc3339_to_epoch, timestamp_at_or_after};
use super::{
    coordination_path, request_ttl_secs, BindingEntry, BoardState, ClaimEntry, CoordEvent,
    CoordEventType, RequestAckEntry, RequestEntry, SubagentCompletedEntry,
};
use crate::parse::now_rfc3339;

// ── Board State Computation ──

/// Read coordination.jsonl and compute current board state.
pub fn compute_board_state(project_id: &str) -> BoardState {
    let path = coordination_path(project_id);
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return BoardState::default(),
    };

    let mut claims: std::collections::HashMap<String, ClaimEntry> =
        std::collections::HashMap::new();
    let mut bindings: Vec<BindingEntry> = Vec::new();
    let mut requests: Vec<RequestEntry> = Vec::new();
    let mut request_acks: Vec<RequestAckEntry> = Vec::new();
    let mut subagent_completions: Vec<SubagentCompletedEntry> = Vec::new();

    for line in content.lines() {
        let event: CoordEvent = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue,
        };

        match event.event_type {
            CoordEventType::Claim => {
                let label = event.payload["label"].as_str().unwrap_or("").to_string();
                let paths: Vec<String> = event.payload["paths"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let subject = event
                    .payload
                    .get("subject")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                claims.insert(
                    event.session_id.clone(),
                    ClaimEntry {
                        session_id: event.session_id,
                        label,
                        paths,
                        ts: event.ts,
                        subject,
                    },
                );
            }
            CoordEventType::Unclaim => {
                claims.remove(&event.session_id);
            }
            CoordEventType::Binding => {
                let key = event.payload["key"].as_str().unwrap_or("").to_string();
                let value = event.payload["value"].as_str().unwrap_or("").to_string();
                let by_label = event.payload["by_label"].as_str().unwrap_or("").to_string();
                // Dedup: newer binding with same key replaces older
                bindings.retain(|d| d.key != key);
                bindings.push(BindingEntry {
                    key,
                    value,
                    by_session: event.session_id,
                    by_label,
                    ts: event.ts,
                });
            }
            CoordEventType::Request => {
                let from_label = event.payload["from_label"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let to_label = event.payload["to_label"].as_str().unwrap_or("").to_string();
                let message = event.payload["message"].as_str().unwrap_or("").to_string();
                // Pre-GH-442 requests have no id; synthesize a stable one from
                // the fields that already identify the event on disk.
                let id = event.payload["id"]
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("legacy:{}:{}", event.session_id, event.ts));
                requests.push(RequestEntry {
                    id,
                    from_session: event.session_id,
                    from_label,
                    to_label,
                    message,
                    ts: event.ts,
                });
            }
            CoordEventType::RequestAck => {
                let from_label = event.payload["from_label"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let request_ids = event.payload.get("request_ids").map(|value| {
                    value
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default()
                });
                request_acks.push(RequestAckEntry {
                    acker_session: event.session_id,
                    from_label,
                    request_ids,
                    ts: event.ts,
                });
            }
            CoordEventType::TaskCompleted | CoordEventType::TeammateIdle => {
                // TaskCompleted and TeammateIdle events are informational;
                // no board-level state aggregation needed.
            }
            CoordEventType::SubagentCompleted => {
                let parent_session_id = event.payload["parent_session_id"]
                    .as_str()
                    .unwrap_or(event.session_id.as_str())
                    .to_string();
                let agent_id = event.payload["agent_id"].as_str().unwrap_or("").to_string();
                let agent_type = event.payload["agent_type"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let summary = event.payload["summary"].as_str().unwrap_or("").to_string();
                let files_touched: Vec<String> = event.payload["files_touched"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let decisions: Vec<String> = event.payload["decisions"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let commits: Vec<String> = event.payload["commits"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                subagent_completions.push(SubagentCompletedEntry {
                    parent_session_id,
                    agent_id,
                    agent_type,
                    summary,
                    files_touched,
                    decisions,
                    commits,
                    ts: event.ts,
                });
            }
        }
    }

    BoardState {
        claims: {
            let mut v: Vec<_> = claims.into_values().collect();
            v.sort_by(|a, b| a.label.cmp(&b.label));
            v
        },
        bindings,
        requests,
        request_acks,
        subagent_completions,
    }
}

// ── Request Delivery Rules ──

/// Current wall clock as epoch seconds, on the same coarse scale the peers
/// module uses for every other event timestamp.
fn now_epoch() -> u64 {
    parse_rfc3339_to_epoch(&now_rfc3339()).unwrap_or(0)
}

/// True when `session_id` has acknowledged this specific request.
///
/// Acks name the request ids they cover (GH-442), so acking one message from a
/// peer no longer suppresses that peer's later messages. Legacy acks carry no
/// ids; they fall back to label matching bounded by timestamp, so they can only
/// suppress requests that already existed when the ack was written.
fn is_acked(board: &BoardState, request: &RequestEntry, session_id: &str) -> bool {
    board.request_acks.iter().any(|a| {
        if a.acker_session != session_id {
            return false;
        }
        match &a.request_ids {
            None => a.from_label == request.from_label && timestamp_at_or_after(&a.ts, &request.ts),
            Some(request_ids) => request_ids.iter().any(|id| id == &request.id),
        }
    })
}

/// True when an unacked request has aged past the dead-letter horizon.
/// An unparseable timestamp never expires — losing a message is worse than
/// keeping a stale one.
fn is_expired(request: &RequestEntry, now: u64, ttl: u64) -> bool {
    match parse_rfc3339_to_epoch(&request.ts) {
        Some(ts) => now.saturating_sub(ts) > ttl,
        None => false,
    }
}

/// Whether a request is past the dead-letter horizon right now.
pub fn request_is_expired(request: &RequestEntry) -> bool {
    is_expired(request, now_epoch(), request_ttl_secs())
}

/// Split the unacked requests addressed to `my_label` into `(live, expired)`.
///
/// The single place delivery is decided: every renderer and the pending-request
/// nudge go through here, so they cannot drift apart on what counts as acked.
pub fn partition_requests_for_session<'a>(
    board: &'a BoardState,
    session_id: &str,
    my_label: &str,
) -> (Vec<&'a RequestEntry>, Vec<&'a RequestEntry>) {
    if my_label.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let now = now_epoch();
    let ttl = request_ttl_secs();
    let mut live = Vec::new();
    let mut expired = Vec::new();
    for r in board.requests.iter().filter(|r| r.to_label == my_label) {
        if is_acked(board, r, session_id) {
            continue;
        }
        if is_expired(r, now, ttl) {
            expired.push(r);
        } else {
            live.push(r);
        }
    }
    (live, expired)
}

/// Compact coordination.jsonl: compute current state and return as JSONL lines.
/// Used by GC to shrink the append-only log.
pub fn compute_board_state_for_compaction(project_id: &str) -> Vec<String> {
    let board = compute_board_state(project_id);
    let mut lines = Vec::new();

    for claim in &board.claims {
        let event = CoordEvent {
            ts: claim.ts.clone(),
            session_id: claim.session_id.clone(),
            event_type: CoordEventType::Claim,
            payload: serde_json::json!({
                "label": claim.label,
                "paths": claim.paths,
            }),
        };
        if let Ok(line) = serde_json::to_string(&event) {
            lines.push(line);
        }
    }

    for binding in &board.bindings {
        let event = CoordEvent {
            ts: binding.ts.clone(),
            session_id: binding.by_session.clone(),
            event_type: CoordEventType::Binding,
            payload: serde_json::json!({
                "key": binding.key,
                "value": binding.value,
                "by_label": binding.by_label,
            }),
        };
        if let Ok(line) = serde_json::to_string(&event) {
            lines.push(line);
        }
    }

    // Requests and acks past the dead-letter horizon are dropped rather than
    // carried forward forever (GH-443). An ack is never older than the request
    // it answers, so TTL-ing both on the same horizon cannot resurrect a
    // request by outliving its ack.
    let now = now_epoch();
    let ttl = request_ttl_secs();

    for request in board.requests.iter().filter(|r| !is_expired(r, now, ttl)) {
        let event = CoordEvent {
            ts: request.ts.clone(),
            session_id: request.from_session.clone(),
            event_type: CoordEventType::Request,
            payload: serde_json::json!({
                "id": request.id,
                "from_label": request.from_label,
                "to_label": request.to_label,
                "message": request.message,
            }),
        };
        if let Ok(line) = serde_json::to_string(&event) {
            lines.push(line);
        }
    }

    for ack in board.request_acks.iter().filter(|a| {
        parse_rfc3339_to_epoch(&a.ts)
            .map(|ts| now.saturating_sub(ts) <= ttl)
            .unwrap_or(true)
    }) {
        let mut payload = serde_json::json!({
            "from_label": ack.from_label,
        });
        if let Some(request_ids) = &ack.request_ids {
            payload["request_ids"] = serde_json::json!(request_ids);
        }
        let event = CoordEvent {
            ts: ack.ts.clone(),
            session_id: ack.acker_session.clone(),
            event_type: CoordEventType::RequestAck,
            payload,
        };
        if let Ok(line) = serde_json::to_string(&event) {
            lines.push(line);
        }
    }

    for sub in &board.subagent_completions {
        let event = CoordEvent {
            ts: sub.ts.clone(),
            session_id: sub.parent_session_id.clone(),
            event_type: CoordEventType::SubagentCompleted,
            payload: serde_json::json!({
                "kind": "subagent_completed",
                "parent_session_id": sub.parent_session_id,
                "agent_id": sub.agent_id,
                "agent_type": sub.agent_type,
                "summary": sub.summary,
                "files_touched": sub.files_touched,
                "decisions": sub.decisions,
                "commits": sub.commits,
            }),
        };
        if let Ok(line) = serde_json::to_string(&event) {
            lines.push(line);
        }
    }

    lines
}
