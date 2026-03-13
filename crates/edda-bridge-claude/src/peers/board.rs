use std::fs;

use super::{
    coordination_path, BindingEntry, BoardState, ClaimEntry, CoordEvent, CoordEventType,
    RequestAckEntry, RequestEntry, SubagentCompletedEntry,
};

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
                claims.insert(
                    event.session_id.clone(),
                    ClaimEntry {
                        session_id: event.session_id,
                        label,
                        paths,
                        ts: event.ts,
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
                requests.push(RequestEntry {
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
                request_acks.push(RequestAckEntry {
                    acker_session: event.session_id,
                    from_label,
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

    for request in &board.requests {
        let event = CoordEvent {
            ts: request.ts.clone(),
            session_id: request.from_session.clone(),
            event_type: CoordEventType::Request,
            payload: serde_json::json!({
                "from_label": request.from_label,
                "to_label": request.to_label,
                "message": request.message,
            }),
        };
        if let Ok(line) = serde_json::to_string(&event) {
            lines.push(line);
        }
    }

    for ack in &board.request_acks {
        let event = CoordEvent {
            ts: ack.ts.clone(),
            session_id: ack.acker_session.clone(),
            event_type: CoordEventType::RequestAck,
            payload: serde_json::json!({
                "from_label": ack.from_label,
            }),
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
