use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

// GH-569: the heartbeat type and its path/IO live in edda-store so any crate
// (conductor runner included) can write heartbeats without depending on this
// bridge. This module re-exports to preserve the public API surface.
pub use edda_store::{heartbeat_path, read_heartbeat, SessionHeartbeat};

// ── Configuration ──

/// Staleness threshold: peers not heard from in this many seconds are considered dead.
pub fn stale_secs() -> u64 {
    std::env::var("EDDA_PEER_STALE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120)
}

/// Dead-letter horizon: unacked requests older than this are expired.
///
/// Requests are addressed by label, and a label can go away — the session
/// ended, the scope was renamed, the sender typo'd it. Without a horizon those
/// requests sit in `coordination.jsonl` forever: invisible to every renderer,
/// preserved by every compaction. Default 7 days.
pub fn request_ttl_secs() -> u64 {
    std::env::var("EDDA_REQUEST_TTL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(7 * 24 * 60 * 60)
}

/// Maximum chars for the coordination protocol section.
fn protocol_budget() -> usize {
    std::env::var("EDDA_PEERS_BUDGET_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600)
}

/// Maximum chars for the lightweight peer updates section (UserPromptSubmit).
const PEER_UPDATES_BUDGET: usize = 500;

/// Session label from env var (set before launching Claude Code).
fn env_label() -> Option<String> {
    std::env::var("EDDA_SESSION_LABEL")
        .ok()
        .filter(|v| !v.is_empty())
}

/// Detect git branch in a specific directory.
pub(crate) fn detect_git_branch_in(cwd: &str) -> Option<String> {
    std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

// ── Data Structures ──

// Per-session heartbeat file.
// Location: ~/.edda/projects/{pid}/state/session.{sid}.json
// GH-569: the struct itself lives in `edda_store::SessionHeartbeat` and is
// re-exported above.

/// Append-only coordination event.
/// Location: ~/.edda/projects/{pid}/state/coordination.jsonl
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CoordEvent {
    pub ts: String,
    pub session_id: String,
    pub event_type: CoordEventType,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CoordEventType {
    Claim,
    Unclaim,
    #[serde(alias = "decision")]
    Binding,
    Request,
    RequestAck,
    SubagentCompleted,
    TaskCompleted,
    TeammateIdle,
}

/// A scope claim by a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimEntry {
    pub session_id: String,
    pub label: String,
    pub paths: Vec<String>,
    pub ts: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

/// A binding entry in the coordination log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingEntry {
    pub key: String,
    pub value: String,
    pub by_session: String,
    pub by_label: String,
    pub ts: String,
}

/// A cross-agent request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestEntry {
    /// Stable per-message identity, so an ack can name the message it answers.
    /// Requests written before GH-442 carry no id on disk and are given a
    /// synthetic `legacy:{session}:{ts}` one on read.
    pub id: String,
    pub from_session: String,
    pub from_label: String,
    pub to_label: String,
    pub message: String,
    pub ts: String,
}

/// A request acknowledgement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestAckEntry {
    pub acker_session: String,
    pub from_label: String,
    /// Ids of the requests this ack covers. `None` is the legacy shape,
    /// which falls back to `from_label` + timestamp matching; `Some([])` is
    /// a new ack that retires nothing.
    #[serde(default)]
    pub request_ids: Option<Vec<String>>,
    pub ts: String,
}

/// A sub-agent completion summary entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentCompletedEntry {
    pub parent_session_id: String,
    pub agent_id: String,
    pub agent_type: String,
    pub summary: String,
    pub files_touched: Vec<String>,
    pub decisions: Vec<String>,
    pub commits: Vec<String>,
    pub ts: String,
}

/// Computed board state from coordination.jsonl.
#[derive(Debug, Default)]
pub struct BoardState {
    pub claims: Vec<ClaimEntry>,
    pub bindings: Vec<BindingEntry>,
    pub requests: Vec<RequestEntry>,
    pub request_acks: Vec<RequestAckEntry>,
    pub subagent_completions: Vec<SubagentCompletedEntry>,
}

/// Summary of a peer session for rendering.
#[derive(Debug, Clone, Serialize)]
pub struct PeerSummary {
    pub session_id: String,
    pub label: String,
    pub age_secs: u64,
    pub last_heartbeat: String,
    pub focus_files: Vec<String>,
    pub task_subjects: Vec<String>,
    pub files_modified_count: usize,
    pub recent_commits: Vec<String>,
    pub claimed_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_subject: Option<String>,
    pub branch: Option<String>,
    pub current_phase: Option<String>,
}

/// Conflict info when a binding with the same key but different value exists.
#[derive(Debug, Clone)]
pub struct BindingConflict {
    pub existing_value: String,
    pub by_session: String,
    pub by_label: String,
    pub ts: String,
}

/// Persisted auto-claim state for dedup (avoid repeated writes to coordination.jsonl).
/// Location: ~/.edda/projects/{pid}/state/autoclaim.{sid}.json
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AutoClaimState {
    label: String,
    paths: Vec<String>,
    ts: String,
    /// Incrementally tracked files for real-time auto-claim.
    #[serde(default)]
    files: std::collections::HashSet<String>,
}

// ── Path Helpers ──

fn autoclaim_state_path(project_id: &str, session_id: &str) -> PathBuf {
    edda_store::project_dir(project_id)
        .join("state")
        .join(format!("autoclaim.{session_id}.json"))
}

// heartbeat_path now comes from edda_store (re-exported above).

pub(crate) fn coordination_path(project_id: &str) -> PathBuf {
    let dir = edda_store::project_dir(project_id).join("state");
    let new_path = dir.join("coordination.jsonl");
    // One-time migration: rename legacy decisions.jsonl → coordination.jsonl
    if !new_path.exists() {
        let old_path = dir.join("decisions.jsonl");
        if old_path.exists() {
            let _ = fs::rename(&old_path, &new_path);
        }
    }
    new_path
}

mod autoclaim;
mod board;
mod discovery;
mod heartbeat;
mod helpers;
pub mod liveness;
mod render_coord;
mod render_fleet;

// Re-export public items to preserve API
pub(crate) use autoclaim::{maybe_auto_claim, maybe_auto_claim_file, remove_autoclaim_state};
pub use board::{
    compute_board_state, compute_board_state_for_compaction, partition_requests_for_session,
    request_is_expired,
};
pub use discovery::{discover_active_peers, discover_all_sessions, infer_session_id};
pub(crate) use heartbeat::{
    cleanup_subagent_heartbeats, ensure_heartbeat_exists, resolve_teammate_session,
    update_heartbeat_branch, update_teammate_phase, write_heartbeat, write_subagent_completed,
    write_subagent_heartbeat, write_task_completed, write_teammate_idle, SubagentReport,
};
pub use heartbeat::{
    find_binding_conflict, remove_heartbeat, resolve_request_targets, touch_heartbeat,
    write_binding, write_claim, write_claim_with_subject, write_heartbeat_minimal, write_request,
    write_request_ack, write_unclaim,
};
pub use helpers::format_age;
pub(crate) use helpers::{format_peer_suffix, pending_requests_for_session};
pub use helpers::{resolve_session_label, session_label_from_board, timestamp_at_or_after};
pub use liveness::{
    classify_session_liveness, classify_session_liveness_at, liveness_from_heartbeat,
    SessionLiveness,
};
pub(crate) use render_coord::{render_coord_diff, render_peer_updates_with};
pub use render_coord::{render_coordination_protocol, render_coordination_protocol_with};
pub use render_fleet::fleet_section;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
