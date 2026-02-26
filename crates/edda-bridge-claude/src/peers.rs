use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::parse::now_rfc3339;
use crate::signals::{FileEditCount, SessionSignals, TaskSnapshot};

// ── Configuration ──

/// Staleness threshold: peers not heard from in this many seconds are considered dead.
pub(crate) fn stale_secs() -> u64 {
    std::env::var("EDDA_PEER_STALE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120)
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

/// Detect the current git branch via `git rev-parse --abbrev-ref HEAD`.
/// Returns `None` if not in a git repo or git is unavailable.
fn detect_git_branch() -> Option<String> {
    std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

// ── Data Structures ──

/// Per-session heartbeat file.
/// Location: ~/.edda/projects/{pid}/state/session.{sid}.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHeartbeat {
    pub session_id: String,
    pub started_at: String,
    pub last_heartbeat: String,
    pub label: String,
    pub focus_files: Vec<String>,
    pub active_tasks: Vec<TaskSnapshot>,
    pub files_modified_count: usize,
    pub total_edits: usize,
    pub recent_commits: Vec<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_phase: Option<String>,
}

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
}

/// A scope claim by a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimEntry {
    pub session_id: String,
    pub label: String,
    pub paths: Vec<String>,
    pub ts: String,
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
    pub ts: String,
}

/// Computed board state from coordination.jsonl.
#[derive(Debug, Default)]
pub struct BoardState {
    pub claims: Vec<ClaimEntry>,
    pub bindings: Vec<BindingEntry>,
    pub requests: Vec<RequestEntry>,
    pub request_acks: Vec<RequestAckEntry>,
}

/// Summary of a peer session for rendering.
#[derive(Debug, Clone)]
pub struct PeerSummary {
    pub session_id: String,
    pub label: String,
    pub age_secs: u64,
    pub focus_files: Vec<String>,
    pub task_subjects: Vec<String>,
    pub files_modified_count: usize,
    pub recent_commits: Vec<String>,
    pub claimed_paths: Vec<String>,
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

fn heartbeat_path(project_id: &str, session_id: &str) -> PathBuf {
    edda_store::project_dir(project_id)
        .join("state")
        .join(format!("session.{session_id}.json"))
}

fn coordination_path(project_id: &str) -> PathBuf {
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

// ── Heartbeat Write/Read ──

/// Write a full heartbeat (called from ingest_and_build_pack after signal extraction).
pub(crate) fn write_heartbeat(
    project_id: &str,
    session_id: &str,
    signals: &SessionSignals,
    label: Option<&str>,
) {
    let now = now_rfc3339();
    let path = heartbeat_path(project_id, session_id);

    // Preserve started_at from existing heartbeat, or use now
    let started_at = read_heartbeat(project_id, session_id)
        .map(|h| h.started_at)
        .unwrap_or_else(|| now.clone());

    let derived_label = label
        .map(|s| s.to_string())
        .or_else(env_label)
        .unwrap_or_else(|| auto_label(signals));

    let heartbeat = SessionHeartbeat {
        session_id: session_id.to_string(),
        started_at,
        last_heartbeat: now,
        label: derived_label,
        focus_files: signals
            .files_modified
            .iter()
            .take(5)
            .map(|f| f.path.clone())
            .collect(),
        active_tasks: signals.tasks.clone(),
        files_modified_count: signals.files_modified.len(),
        total_edits: signals.files_modified.iter().map(|f| f.count).sum(),
        recent_commits: signals
            .commits
            .iter()
            .rev()
            .take(3)
            .map(|c| format!("{} {}", &c.hash[..7.min(c.hash.len())], c.message))
            .collect(),
        branch: detect_git_branch(),
        current_phase: crate::agent_phase::read_phase_state(project_id, session_id)
            .map(|ps| ps.phase.to_string()),
    };

    let data = match serde_json::to_string_pretty(&heartbeat) {
        Ok(d) => d,
        Err(_) => return,
    };
    let _ = edda_store::write_atomic(&path, data.as_bytes());
}

/// Lightweight heartbeat touch: only update last_heartbeat timestamp.
pub fn touch_heartbeat(project_id: &str, session_id: &str) {
    let path = heartbeat_path(project_id, session_id);
    if let Some(mut hb) = read_heartbeat(project_id, session_id) {
        hb.last_heartbeat = now_rfc3339();
        if let Ok(data) = serde_json::to_string_pretty(&hb) {
            let _ = edda_store::write_atomic(&path, data.as_bytes());
        }
    }
    // If no existing heartbeat, skip touch (write_heartbeat will create it)
}

/// Ensure a heartbeat file exists for this session.
/// If one already exists (e.g. written by `ingest_and_build_pack`), it is preserved.
/// If none exists, writes a minimal heartbeat with empty signals so that other
/// sessions can discover this peer via `discover_active_peers` immediately.
///
/// This is needed because `ingest_and_build_pack` skips when the transcript file
/// doesn't exist yet — which is the normal case for brand-new SessionStart events
/// (Claude Code creates the transcript *after* the hook fires).
pub(crate) fn ensure_heartbeat_exists(project_id: &str, session_id: &str) {
    if read_heartbeat(project_id, session_id).is_some() {
        return;
    }
    write_heartbeat(project_id, session_id, &SessionSignals::default(), None);
}

/// Remove heartbeat on SessionEnd.
pub fn remove_heartbeat(project_id: &str, session_id: &str) {
    let _ = fs::remove_file(heartbeat_path(project_id, session_id));
}

/// Write a minimal heartbeat for CLI/external bridge use (no signal data).
///
/// Creates a heartbeat with the given label and empty signals, sufficient
/// for peer discovery. Use `write_heartbeat` for full signal-enriched heartbeats.
pub fn write_heartbeat_minimal(project_id: &str, session_id: &str, label: &str) {
    let now = now_rfc3339();
    let path = heartbeat_path(project_id, session_id);

    let started_at = read_heartbeat(project_id, session_id)
        .map(|h| h.started_at)
        .unwrap_or_else(|| now.clone());

    let heartbeat = SessionHeartbeat {
        session_id: session_id.to_string(),
        started_at,
        last_heartbeat: now,
        label: label.to_string(),
        focus_files: Vec::new(),
        active_tasks: Vec::new(),
        files_modified_count: 0,
        total_edits: 0,
        recent_commits: Vec::new(),
        branch: detect_git_branch(),
        current_phase: None,
    };

    let data = match serde_json::to_string_pretty(&heartbeat) {
        Ok(d) => d,
        Err(_) => return,
    };
    let _ = edda_store::write_atomic(&path, data.as_bytes());
}

/// Read a single session's heartbeat file.
fn read_heartbeat(project_id: &str, session_id: &str) -> Option<SessionHeartbeat> {
    let path = heartbeat_path(project_id, session_id);
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

// ── Coordination Events (append-only log) ──

/// Append a coordination event to coordination.jsonl.
pub(crate) fn append_coord_event(project_id: &str, event: &CoordEvent) {
    let path = coordination_path(project_id);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let line = match serde_json::to_string(event) {
        Ok(l) => l,
        Err(_) => return,
    };
    let mut file = match fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(_) => return,
    };
    let _ = writeln!(file, "{line}");
}

/// Write a claim event.
pub fn write_claim(project_id: &str, session_id: &str, label: &str, paths: &[String]) {
    let event = CoordEvent {
        ts: now_rfc3339(),
        session_id: session_id.to_string(),
        event_type: CoordEventType::Claim,
        payload: serde_json::json!({
            "label": label,
            "paths": paths,
        }),
    };
    append_coord_event(project_id, &event);
}

/// Write an unclaim event (on session end).
pub fn write_unclaim(project_id: &str, session_id: &str) {
    let event = CoordEvent {
        ts: now_rfc3339(),
        session_id: session_id.to_string(),
        event_type: CoordEventType::Unclaim,
        payload: serde_json::json!({}),
    };
    append_coord_event(project_id, &event);
}

/// Write a binding event to the coordination log.
pub fn write_binding(project_id: &str, session_id: &str, label: &str, key: &str, value: &str) {
    let event = CoordEvent {
        ts: now_rfc3339(),
        session_id: session_id.to_string(),
        event_type: CoordEventType::Binding,
        payload: serde_json::json!({
            "key": key,
            "value": value,
            "by_label": label,
        }),
    };
    append_coord_event(project_id, &event);
}

/// Write a cross-agent request event.
pub fn write_request(
    project_id: &str,
    session_id: &str,
    from_label: &str,
    to_label: &str,
    message: &str,
) {
    let event = CoordEvent {
        ts: now_rfc3339(),
        session_id: session_id.to_string(),
        event_type: CoordEventType::Request,
        payload: serde_json::json!({
            "from_label": from_label,
            "to_label": to_label,
            "message": message,
        }),
    };
    append_coord_event(project_id, &event);
}

/// Write a request acknowledgement event.
pub fn write_request_ack(project_id: &str, session_id: &str, from_label: &str) {
    let event = CoordEvent {
        ts: now_rfc3339(),
        session_id: session_id.to_string(),
        event_type: CoordEventType::RequestAck,
        payload: serde_json::json!({ "from_label": from_label }),
    };
    append_coord_event(project_id, &event);
}

/// Check if a binding conflict exists for the given key in coordination.jsonl.
///
/// Returns `Some(BindingConflict)` if a binding with the same key but a
/// different value already exists. Returns `None` if no existing binding
/// or the value is identical (idempotent re-decide).
pub fn find_binding_conflict(
    project_id: &str,
    key: &str,
    new_value: &str,
) -> Option<BindingConflict> {
    let board = compute_board_state(project_id);
    let existing = board.bindings.iter().find(|b| b.key == key)?;
    if existing.value == new_value {
        return None; // idempotent — same value, no conflict
    }
    Some(BindingConflict {
        existing_value: existing.value.clone(),
        by_session: existing.by_session.clone(),
        by_label: existing.by_label.clone(),
        ts: existing.ts.clone(),
    })
}

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

    lines
}

// ── Peer Discovery ──

/// Discover active peer sessions (excluding current session and stale ones).
pub fn discover_active_peers(project_id: &str, current_session_id: &str) -> Vec<PeerSummary> {
    let state_dir = edda_store::project_dir(project_id).join("state");
    let stale_threshold = stale_secs();
    let now = parse_rfc3339_to_epoch(&now_rfc3339()).unwrap_or(0);

    let board = compute_board_state(project_id);

    let entries = match fs::read_dir(&state_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut peers = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("session.") || !name.ends_with(".json") {
            continue;
        }
        let sid = name
            .strip_prefix("session.")
            .and_then(|s| s.strip_suffix(".json"))
            .unwrap_or("");
        if sid.is_empty() || sid == current_session_id {
            continue;
        }

        let content = match fs::read_to_string(entry.path()) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let hb: SessionHeartbeat = match serde_json::from_str(&content) {
            Ok(h) => h,
            Err(_) => continue,
        };

        let hb_epoch = parse_rfc3339_to_epoch(&hb.last_heartbeat).unwrap_or(0);
        let age = now.saturating_sub(hb_epoch);

        if age > stale_threshold {
            continue;
        }

        let claimed_paths = board
            .claims
            .iter()
            .find(|c| c.session_id == hb.session_id)
            .map(|c| c.paths.clone())
            .unwrap_or_default();

        let task_subjects: Vec<String> = hb
            .active_tasks
            .iter()
            .filter(|t| t.status == "in_progress")
            .take(2)
            .map(|t| t.subject.clone())
            .collect();

        peers.push(PeerSummary {
            session_id: hb.session_id,
            label: hb.label,
            age_secs: age,
            focus_files: hb.focus_files,
            task_subjects,
            files_modified_count: hb.files_modified_count,
            recent_commits: hb.recent_commits,
            claimed_paths,
            branch: hb.branch,
            current_phase: hb.current_phase,
        });
    }

    // Sort by most recently active
    peers.sort_by_key(|p| p.age_secs);
    peers
}

/// Discover ALL sessions (including current one), for CLI display.
pub fn discover_all_sessions(project_id: &str) -> Vec<PeerSummary> {
    let state_dir = edda_store::project_dir(project_id).join("state");
    let now = parse_rfc3339_to_epoch(&now_rfc3339()).unwrap_or(0);
    let board = compute_board_state(project_id);

    let entries = match fs::read_dir(&state_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut peers = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("session.") || !name.ends_with(".json") {
            continue;
        }

        let content = match fs::read_to_string(entry.path()) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let hb: SessionHeartbeat = match serde_json::from_str(&content) {
            Ok(h) => h,
            Err(_) => continue,
        };

        let hb_epoch = parse_rfc3339_to_epoch(&hb.last_heartbeat).unwrap_or(0);
        let age = now.saturating_sub(hb_epoch);

        let claimed_paths = board
            .claims
            .iter()
            .find(|c| c.session_id == hb.session_id)
            .map(|c| c.paths.clone())
            .unwrap_or_default();

        let task_subjects: Vec<String> = hb
            .active_tasks
            .iter()
            .filter(|t| t.status == "in_progress")
            .take(2)
            .map(|t| t.subject.clone())
            .collect();

        peers.push(PeerSummary {
            session_id: hb.session_id,
            label: hb.label,
            age_secs: age,
            focus_files: hb.focus_files,
            task_subjects,
            files_modified_count: hb.files_modified_count,
            recent_commits: hb.recent_commits,
            claimed_paths,
            branch: hb.branch,
            current_phase: hb.current_phase,
        });
    }

    peers.sort_by_key(|p| p.age_secs);
    peers
}

/// Infer the current session from heartbeat files.
///
/// If exactly one non-stale session exists for the project, returns
/// `Some((session_id, label))`. Otherwise returns `None` (ambiguous or no context).
///
/// Used by CLI commands (`edda decide`, etc.) to resolve session identity
/// when `EDDA_SESSION_ID` env var is not set.
pub fn infer_session_id(project_id: &str) -> Option<(String, String)> {
    let state_dir = edda_store::project_dir(project_id).join("state");
    let stale_threshold = stale_secs();
    let now = parse_rfc3339_to_epoch(&now_rfc3339()).unwrap_or(0);

    let entries = match fs::read_dir(&state_dir) {
        Ok(e) => e,
        Err(_) => return None,
    };

    let mut active: Vec<(String, String)> = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("session.") || !name.ends_with(".json") {
            continue;
        }

        let content = match fs::read_to_string(entry.path()) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let hb: SessionHeartbeat = match serde_json::from_str(&content) {
            Ok(h) => h,
            Err(_) => continue,
        };

        let hb_epoch = parse_rfc3339_to_epoch(&hb.last_heartbeat).unwrap_or(0);
        let age = now.saturating_sub(hb_epoch);

        if age <= stale_threshold {
            active.push((hb.session_id, hb.label));
        }
    }

    if active.len() == 1 {
        Some(active.remove(0))
    } else {
        None
    }
}

// ── Directive Renderer ──

/// Render the full coordination protocol section for SessionStart injection.
///
/// - Multi-session: full protocol (peers, claims, bindings, commits, requests).
/// - Solo with bindings: "## Binding Decisions" section only.
/// - Solo without bindings: returns None.
pub fn render_coordination_protocol(
    project_id: &str,
    session_id: &str,
    _cwd: &str,
) -> Option<String> {
    let peers = discover_active_peers(project_id, session_id);
    let board = compute_board_state(project_id);
    render_coordination_protocol_with(&peers, &board, project_id, session_id)
}

/// Render full coordination protocol using pre-computed peers and board state.
///
/// "Pre-computed" refers to `peers` and `board` only — heartbeat writes and
/// other per-session I/O still happen at the call site in `dispatch.rs`.
pub fn render_coordination_protocol_with(
    peers: &[PeerSummary],
    board: &BoardState,
    project_id: &str,
    session_id: &str,
) -> Option<String> {
    let budget = protocol_budget();

    if peers.is_empty() {
        // Solo mode: only render bindings (if any exist)
        if board.bindings.is_empty() {
            return None;
        }
        let mut lines = vec!["## Binding Decisions".to_string()];
        for d in board.bindings.iter().rev().take(5) {
            lines.push(format!("- {}: {} ({})", d.key, d.value, d.by_label));
        }
        let result = lines.join("\n");
        return Some(if result.len() > budget {
            truncate_to_budget(&result, budget)
        } else {
            result
        });
    }

    let my_claim = board.claims.iter().find(|c| c.session_id == session_id);
    let my_heartbeat = read_heartbeat(project_id, session_id);

    // Resolve identity: explicit claim wins, heartbeat label is fallback
    let my_label: &str = if let Some(claim) = my_claim {
        claim.label.as_str()
    } else if let Some(ref hb) = my_heartbeat {
        hb.label.as_str()
    } else {
        ""
    };

    let mut lines = Vec::new();

    lines.push(format!(
        "## Coordination Protocol\nYou are one of {} agents working simultaneously.",
        peers.len() + 1
    ));

    // L2 command instructions (compact)
    lines.push(
        "Claim your scope: `edda claim \"label\" --paths \"src/scope/*\"`\n\
         Message a peer: `edda request \"peer-label\" \"your message\"`"
            .to_string(),
    );

    // My scope
    if let Some(claim) = my_claim {
        lines.push(format!(
            "Your scope: **{}** ({})",
            claim.label,
            claim.paths.join(", ")
        ));
    } else if !my_label.is_empty() {
        lines.push(format!("Your scope: **{my_label}**"));
    }

    // Peer activity (tasks + focus files)
    let active_peers: Vec<&PeerSummary> = peers
        .iter()
        .filter(|p| !p.task_subjects.is_empty() || !p.focus_files.is_empty())
        .collect();
    if !active_peers.is_empty() {
        lines.push("### Peers Working On".to_string());
        for p in active_peers.iter().take(5) {
            let age = format_age(p.age_secs);
            let branch_suffix = format_peer_suffix(p.branch.as_deref(), p.current_phase.as_deref());
            if !p.task_subjects.is_empty() {
                for t in p.task_subjects.iter().take(2) {
                    lines.push(format!("- {} ({age}){branch_suffix}: {t}", p.label));
                }
            } else if !p.focus_files.is_empty() {
                let files: Vec<&str> = p
                    .focus_files
                    .iter()
                    .take(2)
                    .map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f.as_str()))
                    .collect();
                lines.push(format!(
                    "- {} ({age}){branch_suffix}: editing {}",
                    p.label,
                    files.join(", ")
                ));
            }
        }
    }

    // Off-limits
    let peer_claims: Vec<&PeerSummary> = peers
        .iter()
        .filter(|p| !p.claimed_paths.is_empty())
        .collect();
    if !peer_claims.is_empty() {
        lines.push("### Off-limits (other agents active)".to_string());
        for p in peer_claims.iter().take(5) {
            let age = format_age(p.age_secs);
            lines.push(format!(
                "- {} → Agent {} ({age})",
                p.claimed_paths.join(", "),
                p.label
            ));
        }
    }

    // Binding decisions
    if !board.bindings.is_empty() {
        lines.push("### Binding Decisions".to_string());
        for d in board.bindings.iter().rev().take(5) {
            lines.push(format!("- {}: {} ({})", d.key, d.value, d.by_label));
        }
    }

    // Recent commits from peers (sourced from heartbeat, not coordination log)
    let peer_commits: Vec<(&str, &str)> = peers
        .iter()
        .flat_map(|p| {
            p.recent_commits
                .iter()
                .map(move |c| (p.label.as_str(), c.as_str()))
        })
        .take(5)
        .collect();
    if !peer_commits.is_empty() {
        lines.push("### Recent Peer Commits".to_string());
        for (label, commit) in &peer_commits {
            lines.push(format!("- {commit} ({label})"));
        }
    }

    // Requests to me (using resolved my_label from claim or heartbeat fallback)
    let my_requests: Vec<&RequestEntry> = board
        .requests
        .iter()
        .filter(|r| r.to_label == my_label && !my_label.is_empty())
        .collect();
    if !my_requests.is_empty() {
        lines.push("### Requests to you".to_string());
        for r in my_requests.iter().take(3) {
            lines.push(format!("- Agent {}: \"{}\"", r.from_label, r.message));
        }
    }

    let result = lines.join("\n");

    // Apply budget
    if result.len() > budget {
        Some(truncate_to_budget(&result, budget))
    } else {
        Some(result)
    }
}

/// Render lightweight peer updates for UserPromptSubmit (only new bindings/requests).
///
/// - Multi-session: peers header + tasks + bindings + requests.
/// - Solo with bindings: binding lines only (no header).
/// - Solo without bindings: returns None.
#[cfg(test)]
pub(crate) fn render_peer_updates(project_id: &str, session_id: &str) -> Option<String> {
    let peers = discover_active_peers(project_id, session_id);
    let board = compute_board_state(project_id);
    render_peer_updates_with(&peers, &board, project_id, session_id)
}

/// Render lightweight peer updates using pre-computed peers and board state.
///
/// "Pre-computed" refers to `peers` and `board` only — heartbeat writes and
/// other per-session I/O still happen at the call site in `dispatch.rs`.
pub(crate) fn render_peer_updates_with(
    peers: &[PeerSummary],
    board: &BoardState,
    project_id: &str,
    session_id: &str,
) -> Option<String> {
    if peers.is_empty() {
        // Solo mode: only render bindings (if any)
        if board.bindings.is_empty() {
            return None;
        }
        let mut lines = Vec::new();
        for d in board.bindings.iter().rev().take(3) {
            lines.push(format!("- {}: {} ({})", d.key, d.value, d.by_label));
        }
        let result = lines.join("\n");
        return Some(if result.len() > PEER_UPDATES_BUDGET {
            truncate_to_budget(&result, PEER_UPDATES_BUDGET)
        } else {
            result
        });
    }

    let mut lines = vec![format!("## Peers ({} active)", peers.len())];

    // L2 instructions (condensed single line)
    lines.push(
        "Claim: `edda claim \"label\" --paths \"path\"` | Message: `edda request \"peer\" \"msg\"`"
            .to_string(),
    );

    // Peer activity (tasks → focus files → bare label)
    for p in peers.iter().take(3) {
        let age = format_age(p.age_secs);
        let branch_suffix = format_peer_suffix(p.branch.as_deref(), p.current_phase.as_deref());
        if !p.task_subjects.is_empty() {
            for t in p.task_subjects.iter().take(1) {
                lines.push(format!("- {} ({age}){branch_suffix}: {t}", p.label));
            }
        } else if !p.focus_files.is_empty() {
            let file = p.focus_files[0]
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&p.focus_files[0]);
            lines.push(format!(
                "- {} ({age}){branch_suffix}: editing {file}",
                p.label
            ));
        } else {
            lines.push(format!("- {} ({age}){branch_suffix}", p.label));
        }
    }

    // Latest bindings (max 3)
    if !board.bindings.is_empty() {
        for d in board.bindings.iter().rev().take(3) {
            lines.push(format!("- {}: {} ({})", d.key, d.value, d.by_label));
        }
    }

    // Requests to current session (claim label → heartbeat label fallback)
    let my_claim = board.claims.iter().find(|c| c.session_id == session_id);
    let my_heartbeat = read_heartbeat(project_id, session_id);
    let my_label: &str = if let Some(claim) = my_claim {
        claim.label.as_str()
    } else if let Some(ref hb) = my_heartbeat {
        hb.label.as_str()
    } else {
        ""
    };
    let my_requests: Vec<&RequestEntry> = board
        .requests
        .iter()
        .filter(|r| r.to_label == my_label && !my_label.is_empty())
        .collect();
    if !my_requests.is_empty() {
        for r in my_requests.iter().take(2) {
            lines.push(format!(
                "- Request from {}: \"{}\"",
                r.from_label, r.message
            ));
        }
    }

    let result = lines.join("\n");
    if result.len() > PEER_UPDATES_BUDGET {
        Some(truncate_to_budget(&result, PEER_UPDATES_BUDGET))
    } else {
        Some(result)
    }
}

// ── Auto-Claim ──

/// Derive a scope label and path globs from edited file paths.
///
/// Groups files by crate/package directory, returns the dominant group.
/// Returns `None` if no files modified or no clear grouping.
fn derive_scope_from_files(files: &[FileEditCount]) -> Option<(String, Vec<String>)> {
    if files.is_empty() {
        return None;
    }

    // Group by crate: look for "crates/{name}" or "packages/{name}" pattern
    let mut groups: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for f in files {
        let normalized = f.path.replace('\\', "/");
        let segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
        for (i, seg) in segments.iter().enumerate() {
            if (*seg == "crates" || *seg == "packages") && i + 1 < segments.len() {
                *groups.entry(segments[i + 1].to_string()).or_default() += f.count;
                break;
            }
        }
    }

    if let Some((label, _)) = groups.iter().max_by_key(|(_, c)| *c) {
        let paths = vec![format!("crates/{}/*", label)];
        return Some((label.clone(), paths));
    }

    // Fallback: use src/{module} grouping
    let mut src_groups: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for f in files {
        let normalized = f.path.replace('\\', "/");
        let segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
        if let Some(src_pos) = segments.iter().position(|s| *s == "src") {
            if let Some(module) = segments.get(src_pos + 1) {
                if !module.contains('.') {
                    *src_groups.entry(module.to_string()).or_default() += f.count;
                }
            }
        }
    }

    if let Some((label, _)) = src_groups.iter().max_by_key(|(_, c)| *c) {
        let paths = vec![format!("src/{}/*", label)];
        return Some((label.clone(), paths));
    }

    None
}

/// Auto-claim scope from session signals if no manual claim exists.
///
/// - Skips if session already has an explicit claim in `coordination.jsonl`
/// - Skips if derived scope is identical to last auto-claim (dedup)
/// - Writes claim event + saves state file for dedup
pub(crate) fn maybe_auto_claim(project_id: &str, session_id: &str, signals: &SessionSignals) {
    // 1. Check existing state
    let board = compute_board_state(project_id);
    let existing_claim = board.claims.iter().find(|c| c.session_id == session_id);
    let state_path = autoclaim_state_path(project_id, session_id);
    let prev_auto = fs::read_to_string(&state_path)
        .ok()
        .and_then(|c| serde_json::from_str::<AutoClaimState>(&c).ok());

    // If a claim exists but no auto-claim state file → it was manual → skip
    if existing_claim.is_some() && prev_auto.is_none() {
        return;
    }

    // 2. Derive scope from edited files
    let (label, paths) = match derive_scope_from_files(&signals.files_modified) {
        Some(v) => v,
        None => return,
    };

    // 3. Dedup: skip if scope unchanged from last auto-claim
    if let Some(ref prev) = prev_auto {
        if prev.label == label && prev.paths == paths {
            return;
        }
    }

    // 4. Write claim to coordination.jsonl
    write_claim(project_id, session_id, &label, &paths);

    // 5. Save state for dedup
    let state = AutoClaimState {
        label,
        paths,
        ts: now_rfc3339(),
        files: Default::default(),
    };
    if let Ok(data) = serde_json::to_string_pretty(&state) {
        let _ = edda_store::write_atomic(&state_path, data.as_bytes());
    }
}

/// Real-time auto-claim from a single file edit (PostToolUse path).
///
/// Maintains an incremental file set in the auto-claim state file.
/// On each call, adds the file, re-derives scope, and writes a claim
/// only if the scope changed.
pub(crate) fn maybe_auto_claim_file(project_id: &str, session_id: &str, file_path: &str) {
    let state_path = autoclaim_state_path(project_id, session_id);

    // Fast path: if no state file exists, check for manual claim via coordination.jsonl.
    // This only happens once per session (first Edit); subsequent calls find the state file.
    let state_file_content = fs::read_to_string(&state_path).ok();
    if state_file_content.is_none() {
        let board = compute_board_state(project_id);
        if board.claims.iter().any(|c| c.session_id == session_id) {
            // Manual claim exists, no auto-claim state → skip
            return;
        }
    }

    let mut state: AutoClaimState = state_file_content
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();

    // Add file to tracked set
    let normalized = file_path.replace('\\', "/");
    if !state.files.insert(normalized) {
        // File already tracked, scope won't change
        return;
    }

    // Derive scope from all tracked files
    let file_counts: Vec<FileEditCount> = state
        .files
        .iter()
        .map(|p| FileEditCount {
            path: p.clone(),
            count: 1,
        })
        .collect();
    let Some((label, paths)) = derive_scope_from_files(&file_counts) else {
        // Save files set even if no scope derived yet
        if let Ok(data) = serde_json::to_string_pretty(&state) {
            let _ = edda_store::write_atomic(&state_path, data.as_bytes());
        }
        return;
    };

    // Dedup: skip claim write if scope unchanged
    if state.label == label && state.paths == paths {
        // Save updated files set
        if let Ok(data) = serde_json::to_string_pretty(&state) {
            let _ = edda_store::write_atomic(&state_path, data.as_bytes());
        }
        return;
    }

    // Write claim
    write_claim(project_id, session_id, &label, &paths);
    state.label = label;
    state.paths = paths;
    state.ts = now_rfc3339();
    if let Ok(data) = serde_json::to_string_pretty(&state) {
        let _ = edda_store::write_atomic(&state_path, data.as_bytes());
    }
}

/// Remove auto-claim state file on session end.
pub(crate) fn remove_autoclaim_state(project_id: &str, session_id: &str) {
    let _ = fs::remove_file(autoclaim_state_path(project_id, session_id));
}

/// Return pending (un-acked) requests addressed to the given session.
///
/// Resolves the session's label from its claim or heartbeat, then filters
/// board requests to those targeting that label, excluding any that have
/// been acknowledged by this session.
pub(crate) fn pending_requests_for_session(
    project_id: &str,
    session_id: &str,
) -> Vec<RequestEntry> {
    let board = compute_board_state(project_id);

    // Resolve my label from claim or heartbeat
    let my_label: String = board
        .claims
        .iter()
        .find(|c| c.session_id == session_id)
        .map(|c| c.label.clone())
        .or_else(|| read_heartbeat(project_id, session_id).map(|hb| hb.label))
        .unwrap_or_default();

    if my_label.is_empty() {
        return Vec::new();
    }

    board
        .requests
        .into_iter()
        .filter(|r| r.to_label == my_label)
        .filter(|r| {
            !board
                .request_acks
                .iter()
                .any(|a| a.from_label == r.from_label && a.acker_session == session_id)
        })
        .collect()
}

// ── Helpers ──

/// Auto-derive a label from session signals (focus files).
fn auto_label(signals: &SessionSignals) -> String {
    if signals.files_modified.is_empty() {
        return String::new();
    }

    // Try to extract crate/module name from the most-edited file
    let top_file = signals
        .files_modified
        .iter()
        .max_by_key(|f| f.count)
        .map(|f| f.path.as_str())
        .unwrap_or("");

    let normalized = top_file.replace('\\', "/");
    let segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();

    // Look for crate name pattern: crates/{name}/src/...
    if let Some(pos) = segments.iter().position(|&s| s == "crates") {
        if let Some(name) = segments.get(pos + 1) {
            return name.to_string();
        }
    }

    // Look for src/{name}/...
    if let Some(pos) = segments.iter().position(|&s| s == "src") {
        if let Some(name) = segments.get(pos + 1) {
            if !name.contains('.') {
                return name.to_string();
            }
        }
    }

    // Fall back to parent directory of top file
    if segments.len() >= 2 {
        return segments[segments.len() - 2].to_string();
    }

    String::new()
}

/// Format age in human-readable form.
/// Format the bracket suffix for a peer line: `[branch: x, phase]` or `[branch: x]` etc.
fn format_peer_suffix(branch: Option<&str>, phase: Option<&str>) -> String {
    match (branch, phase) {
        (Some(b), Some(p)) => format!(" [branch: {b}, {p}]"),
        (Some(b), None) => format!(" [branch: {b}]"),
        (None, Some(p)) => format!(" [{p}]"),
        (None, None) => String::new(),
    }
}

pub fn format_age(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}

/// Truncate content to budget, cutting at last newline before budget.
fn truncate_to_budget(content: &str, budget: usize) -> String {
    if content.len() <= budget {
        return content.to_string();
    }
    let truncated = &content[..budget.min(content.len())];
    // Cut at last newline for clean truncation
    if let Some(pos) = truncated.rfind('\n') {
        truncated[..pos].to_string()
    } else {
        truncated.to_string()
    }
}

/// Parse RFC3339 timestamp to Unix epoch seconds (basic parser).
fn parse_rfc3339_to_epoch(ts: &str) -> Option<u64> {
    // Format: 2026-02-16T10:05:23+00:00 or 2026-02-16T10:05:23Z
    // Simple approach: parse with chrono-like logic manually
    // We only need relative comparison, so parsing the digits is enough
    let ts = ts.trim();
    if ts.len() < 19 {
        return None;
    }

    let year: u64 = ts[0..4].parse().ok()?;
    let month: u64 = ts[5..7].parse().ok()?;
    let day: u64 = ts[8..10].parse().ok()?;
    let hour: u64 = ts[11..13].parse().ok()?;
    let min: u64 = ts[14..16].parse().ok()?;
    let sec: u64 = ts[17..19].parse().ok()?;

    // Approximate epoch (good enough for relative age computation)
    // Days since epoch (1970-01-01), ignoring leap seconds
    let days_in_year = 365;
    let years_since_1970 = year.saturating_sub(1970);
    let leap_years = (year.saturating_sub(1969)) / 4 - (year.saturating_sub(1901)) / 100
        + (year.saturating_sub(1601)) / 400;

    let month_days: [u64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut total_days = years_since_1970 * days_in_year + leap_years;
    for d in month_days
        .iter()
        .take((month.saturating_sub(1) as usize).min(11))
    {
        total_days += d;
    }
    // Add leap day for current year if applicable
    if month > 2
        && (year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)))
    {
        total_days += 1;
    }
    total_days += day.saturating_sub(1);

    Some(total_days * 86400 + hour * 3600 + min * 60 + sec)
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::{CommitInfo, FileEditCount};

    #[test]
    fn heartbeat_write_read_roundtrip() {
        let pid = "test_peers_hb_roundtrip";
        let sid = "test-session-001";
        let _ = edda_store::ensure_dirs(pid);

        let signals = SessionSignals {
            tasks: vec![TaskSnapshot {
                id: "1".into(),
                subject: "Implement auth".into(),
                status: "in_progress".into(),
            }],
            files_modified: vec![
                FileEditCount {
                    path: "src/auth/mod.rs".into(),
                    count: 5,
                },
                FileEditCount {
                    path: "src/auth/jwt.rs".into(),
                    count: 3,
                },
            ],
            commits: vec![CommitInfo {
                hash: "abc1234".into(),
                message: "feat: add JWT auth".into(),
            }],
            failed_commands: vec![],
        };

        write_heartbeat(pid, sid, &signals, Some("auth"));
        let hb = read_heartbeat(pid, sid).expect("should read heartbeat");

        assert_eq!(hb.session_id, sid);
        assert_eq!(hb.label, "auth");
        assert_eq!(hb.files_modified_count, 2);
        assert_eq!(hb.total_edits, 8);
        assert_eq!(hb.active_tasks.len(), 1);
        assert_eq!(hb.recent_commits.len(), 1);
        assert!(hb.recent_commits[0].contains("JWT auth"));

        // Cleanup
        remove_heartbeat(pid, sid);
        assert!(read_heartbeat(pid, sid).is_none());

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn coord_event_append_and_board_state() {
        let pid = "test_peers_board_state";
        let _ = edda_store::ensure_dirs(pid);

        // Clean up any existing decisions file
        let _ = fs::remove_file(coordination_path(pid));

        write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
        write_claim(pid, "s2", "billing", &["src/billing/*".into()]);
        write_binding(pid, "s1", "auth", "auth.method", "JWT RS256");
        write_request(pid, "s2", "billing", "auth", "Export AuthToken type");

        let board = compute_board_state(pid);
        assert_eq!(board.claims.len(), 2);
        assert_eq!(board.bindings.len(), 1);
        assert_eq!(board.bindings[0].key, "auth.method");
        assert_eq!(board.bindings[0].value, "JWT RS256");
        assert_eq!(board.requests.len(), 1);
        assert_eq!(board.requests[0].to_label, "auth");

        // Unclaim should remove
        write_unclaim(pid, "s1");
        let board2 = compute_board_state(pid);
        assert_eq!(board2.claims.len(), 1);
        assert_eq!(board2.claims[0].label, "billing");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn discover_peers_excludes_self() {
        let pid = "test_peers_discover";
        let _ = edda_store::ensure_dirs(pid);

        let signals = SessionSignals::default();
        write_heartbeat(pid, "self-session", &signals, Some("self"));
        write_heartbeat(pid, "peer-session", &signals, Some("peer"));

        let peers = discover_active_peers(pid, "self-session");
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].label, "peer");

        remove_heartbeat(pid, "self-session");
        remove_heartbeat(pid, "peer-session");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn render_protocol_solo_no_bindings_returns_none() {
        let pid = "test_peers_solo";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        let result = render_coordination_protocol(pid, "only-session", ".");
        assert!(result.is_none(), "solo with no bindings should return None");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn render_protocol_multi_session() {
        let pid = "test_peers_multi";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        let signals = SessionSignals::default();
        write_heartbeat(pid, "s1", &signals, Some("auth"));
        write_heartbeat(pid, "s2", &signals, Some("billing"));
        write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
        write_claim(pid, "s2", "billing", &["src/billing/*".into()]);
        write_binding(pid, "s1", "auth", "auth.method", "JWT RS256");

        let result = render_coordination_protocol(pid, "s2", ".").unwrap();
        assert!(result.contains("Coordination Protocol"));
        assert!(result.contains("Off-limits"));
        assert!(result.contains("auth"));
        assert!(result.contains("Binding Decisions"));
        assert!(result.contains("JWT RS256"));

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn auto_label_from_crate_path() {
        let signals = SessionSignals {
            files_modified: vec![FileEditCount {
                path: "crates/edda-bridge-claude/src/peers.rs".into(),
                count: 10,
            }],
            ..Default::default()
        };
        assert_eq!(auto_label(&signals), "edda-bridge-claude");
    }

    #[test]
    fn auto_label_from_src_module() {
        let signals = SessionSignals {
            files_modified: vec![FileEditCount {
                path: "src/auth/jwt.rs".into(),
                count: 5,
            }],
            ..Default::default()
        };
        assert_eq!(auto_label(&signals), "auth");
    }

    #[test]
    fn format_age_display() {
        assert_eq!(format_age(30), "30s ago");
        assert_eq!(format_age(90), "1m ago");
        assert_eq!(format_age(3700), "1h ago");
    }

    #[test]
    fn parse_rfc3339_basic() {
        let epoch = parse_rfc3339_to_epoch("2026-02-16T10:05:23Z").unwrap();
        assert!(epoch > 0);

        // Two timestamps 60 seconds apart should differ by ~60
        let a = parse_rfc3339_to_epoch("2026-02-16T10:05:00Z").unwrap();
        let b = parse_rfc3339_to_epoch("2026-02-16T10:06:00Z").unwrap();
        assert_eq!(b - a, 60);
    }

    #[test]
    fn compaction_preserves_current_state() {
        let pid = "test_peers_compaction";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // Write a bunch of events including overrides
        write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
        write_claim(pid, "s2", "billing", &["src/billing/*".into()]);
        write_binding(pid, "s1", "auth", "db.engine", "SQLite");
        write_binding(pid, "s1", "auth", "db.engine", "PostgreSQL"); // override
        write_request(pid, "s2", "billing", "auth", "Export AuthToken");
        write_unclaim(pid, "s1"); // removes s1 claim

        // Compact
        let lines = compute_board_state_for_compaction(pid);
        // Should have: 1 claim (s2), 1 decision (PostgreSQL), 1 request
        assert_eq!(lines.len(), 3);

        // Verify by parsing
        let board_before = compute_board_state(pid);
        assert_eq!(board_before.claims.len(), 1);
        assert_eq!(board_before.claims[0].label, "billing");
        assert_eq!(board_before.bindings.len(), 1);
        assert_eq!(board_before.bindings[0].value, "PostgreSQL");

        // Write compacted back
        let path = coordination_path(pid);
        let content = lines.join("\n");
        fs::write(&path, format!("{content}\n")).unwrap();

        // Verify same state after compaction
        let board_after = compute_board_state(pid);
        assert_eq!(board_after.claims.len(), 1);
        assert_eq!(board_after.claims[0].label, "billing");
        assert_eq!(board_after.bindings.len(), 1);
        assert_eq!(board_after.bindings[0].value, "PostgreSQL");
        assert_eq!(board_after.requests.len(), 1);

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn full_lifecycle_multi_session() {
        let pid = "test_peers_lifecycle";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // Simulate 4 sessions starting
        let signals = SessionSignals::default();
        write_heartbeat(pid, "s1", &signals, Some("auth"));
        write_heartbeat(pid, "s2", &signals, Some("billing"));
        write_heartbeat(pid, "s3", &signals, Some("api"));
        write_heartbeat(pid, "s4", &signals, Some("frontend"));

        // Claims
        write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
        write_claim(pid, "s2", "billing", &["src/billing/*".into()]);
        write_claim(pid, "s3", "api", &["src/api/*".into()]);
        write_claim(pid, "s4", "frontend", &["src/ui/*".into()]);

        // s1 makes a decision
        write_binding(pid, "s1", "auth", "auth.method", "JWT RS256");

        // s3 sends request to s2
        write_request(pid, "s3", "api", "billing", "Export BillingPlan type");

        // Verify s3 sees coordination protocol
        let proto = render_coordination_protocol(pid, "s3", ".").unwrap();
        assert!(proto.contains("Coordination Protocol"));
        assert!(proto.contains("4")); // 3 peers + self = 4 agents
        assert!(proto.contains("JWT RS256"));

        // Verify s2 sees the request
        let proto_s2 = render_coordination_protocol(pid, "s2", ".").unwrap();
        assert!(proto_s2.contains("Export BillingPlan type"));

        // s2 sees peer updates (lightweight)
        let updates = render_peer_updates(pid, "s2").unwrap();
        assert!(updates.contains("Peers"));
        assert!(updates.contains("Export BillingPlan"));

        // Solo session should still see bindings (but not peer sections)
        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        remove_heartbeat(pid, "s3");
        remove_heartbeat(pid, "s4");
        let solo = render_coordination_protocol(pid, "s5", ".").unwrap();
        assert!(
            solo.contains("Binding Decisions"),
            "solo should show bindings"
        );
        assert!(solo.contains("JWT RS256"), "solo should show binding value");
        assert!(
            !solo.contains("Coordination Protocol"),
            "solo should NOT show coordination header"
        );
        assert!(
            !solo.contains("Peers Working On"),
            "solo should NOT show peer sections"
        );

        // discover_all_sessions returns nothing after cleanup
        let all = discover_all_sessions(pid);
        assert!(all.is_empty());

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn binding_dedup_in_board() {
        let pid = "test_peers_decision_dedup";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        write_binding(pid, "s1", "auth", "db.engine", "SQLite");
        write_binding(pid, "s1", "auth", "db.engine", "PostgreSQL");

        let board = compute_board_state(pid);
        assert_eq!(board.bindings.len(), 1);
        assert_eq!(board.bindings[0].value, "PostgreSQL");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn migration_renames_decisions_to_coordination() {
        let pid = "test_peers_migration";
        let _ = edda_store::ensure_dirs(pid);
        let state_dir = edda_store::project_dir(pid).join("state");
        let _ = fs::create_dir_all(&state_dir);

        // Create legacy decisions.jsonl with content
        let old_path = state_dir.join("decisions.jsonl");
        let new_path = state_dir.join("coordination.jsonl");
        let _ = fs::remove_file(&old_path);
        let _ = fs::remove_file(&new_path);
        fs::write(&old_path, "{\"test\":true}\n").unwrap();

        // Calling coordination_path triggers migration
        let result = coordination_path(pid);
        assert_eq!(result, new_path);
        assert!(
            new_path.exists(),
            "coordination.jsonl should exist after migration"
        );
        assert!(
            !old_path.exists(),
            "decisions.jsonl should be removed after migration"
        );
        let content = fs::read_to_string(&new_path).unwrap();
        assert!(content.contains("test"), "content should be preserved");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn migration_skips_if_coordination_exists() {
        let pid = "test_peers_migration_skip";
        let _ = edda_store::ensure_dirs(pid);
        let state_dir = edda_store::project_dir(pid).join("state");
        let _ = fs::create_dir_all(&state_dir);

        // Both files exist — should NOT migrate (coordination.jsonl takes priority)
        let old_path = state_dir.join("decisions.jsonl");
        let new_path = state_dir.join("coordination.jsonl");
        fs::write(&old_path, "old\n").unwrap();
        fs::write(&new_path, "new\n").unwrap();

        let _ = coordination_path(pid);
        // coordination.jsonl should keep its original content
        let content = fs::read_to_string(&new_path).unwrap();
        assert_eq!(content, "new\n");
        // decisions.jsonl should still exist (not deleted when coordination.jsonl exists)
        assert!(old_path.exists());

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn serde_backward_compat_decision_deserializes_as_binding() {
        // Old coordination logs have event_type: "decision". Verify they deserialize as Binding.
        let json = r#"{"ts":"2026-02-18T00:00:00Z","session_id":"s1","event_type":"decision","payload":{"key":"db","value":"pg","by_label":"auth"}}"#;
        let event: CoordEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_type, CoordEventType::Binding);
    }

    #[test]
    fn serde_new_binding_serializes_as_binding() {
        let event = CoordEvent {
            ts: "2026-02-18T00:00:00Z".to_string(),
            session_id: "s1".to_string(),
            event_type: CoordEventType::Binding,
            payload: serde_json::json!({"key": "db"}),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(
            json.contains("\"binding\""),
            "new events should serialize as 'binding', got: {json}"
        );
    }

    #[test]
    fn render_protocol_shows_peer_tasks() {
        let pid = "test_peers_tasks_render";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        let signals_with_task = SessionSignals {
            tasks: vec![TaskSnapshot {
                id: "1".into(),
                subject: "Implement auth flow".into(),
                status: "in_progress".into(),
            }],
            files_modified: vec![FileEditCount {
                path: "crates/edda-auth/src/lib.rs".into(),
                count: 3,
            }],
            ..Default::default()
        };
        write_heartbeat(pid, "s1", &signals_with_task, Some("auth"));
        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));

        let result = render_coordination_protocol(pid, "s2", ".").unwrap();
        assert!(
            result.contains("Peers Working On"),
            "should have working-on section, got:\n{result}"
        );
        assert!(
            result.contains("Implement auth flow"),
            "should show task subject, got:\n{result}"
        );

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn render_protocol_shows_focus_files_when_no_tasks() {
        let pid = "test_peers_focus_render";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // Session with files but no in_progress tasks
        let signals = SessionSignals {
            files_modified: vec![FileEditCount {
                path: "crates/edda-auth/src/lib.rs".into(),
                count: 5,
            }],
            ..Default::default()
        };
        write_heartbeat(pid, "s1", &signals, Some("auth"));
        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));

        let result = render_coordination_protocol(pid, "s2", ".").unwrap();
        assert!(
            result.contains("Peers Working On"),
            "should have working-on section, got:\n{result}"
        );
        assert!(
            result.contains("editing"),
            "should show focus files, got:\n{result}"
        );
        assert!(
            result.contains("lib.rs"),
            "should show file basename, got:\n{result}"
        );

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn render_peer_updates_shows_tasks() {
        let pid = "test_peers_updates_tasks";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        let signals = SessionSignals {
            tasks: vec![TaskSnapshot {
                id: "1".into(),
                subject: "Fix billing bug".into(),
                status: "in_progress".into(),
            }],
            ..Default::default()
        };
        write_heartbeat(pid, "s1", &signals, Some("billing"));
        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"));

        let result = render_peer_updates(pid, "s2").unwrap();
        assert!(
            result.contains("Fix billing bug"),
            "should show peer task, got:\n{result}"
        );

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn render_peer_updates_shows_focus_files() {
        let pid = "test_peers_updates_focus";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // Peer with focus files but no tasks
        let signals = SessionSignals {
            files_modified: vec![crate::signals::FileEditCount {
                path: "src/billing/invoice.rs".into(),
                count: 3,
            }],
            ..Default::default()
        };
        write_heartbeat(pid, "s1", &signals, Some("billing"));
        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"));

        let result = render_peer_updates(pid, "s2").unwrap();
        assert!(
            result.contains("invoice.rs"),
            "should show focus file, got:\n{result}"
        );
        assert!(
            result.contains("billing"),
            "should show peer label, got:\n{result}"
        );

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn render_peer_updates_shows_bare_label() {
        let pid = "test_peers_updates_bare";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // Peer with no tasks and no focus files
        write_heartbeat(pid, "s1", &SessionSignals::default(), Some("billing"));
        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"));

        let result = render_peer_updates(pid, "s2").unwrap();
        assert!(
            result.contains("billing"),
            "should show peer label even without tasks/files, got:\n{result}"
        );
        // Should not be just the header
        let lines: Vec<&str> = result.lines().collect();
        assert!(
            lines.len() > 2,
            "should have more than just header + L2 instructions, got:\n{result}"
        );

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn render_peer_updates_includes_l2_instructions() {
        let pid = "test_peers_updates_l2";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        write_heartbeat(pid, "s1", &SessionSignals::default(), Some("billing"));
        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"));

        let result = render_peer_updates(pid, "s2").unwrap();
        assert!(
            result.contains("edda claim"),
            "should include claim instruction, got:\n{result}"
        );
        assert!(
            result.contains("edda request"),
            "should include request instruction, got:\n{result}"
        );

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── Solo binding visibility tests (issue #147) ──

    #[test]
    fn render_protocol_solo_with_bindings() {
        let pid = "test_peers_solo_bindings";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // No heartbeats (solo), but write bindings
        write_binding(pid, "s1", "auth", "auth.method", "JWT RS256");
        write_binding(pid, "s1", "auth", "db.engine", "PostgreSQL");

        let result = render_coordination_protocol(pid, "solo-session", ".").unwrap();
        assert!(
            result.contains("Binding Decisions"),
            "should have binding header, got:\n{result}"
        );
        assert!(
            result.contains("JWT RS256"),
            "should show binding value, got:\n{result}"
        );
        assert!(
            result.contains("PostgreSQL"),
            "should show second binding, got:\n{result}"
        );
        assert!(
            !result.contains("Coordination Protocol"),
            "should NOT have coordination header, got:\n{result}"
        );
        assert!(
            !result.contains("Peers Working On"),
            "should NOT have peer sections, got:\n{result}"
        );
        assert!(
            !result.contains("Off-limits"),
            "should NOT have off-limits, got:\n{result}"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn render_peer_updates_solo_with_bindings() {
        let pid = "test_peers_updates_solo_bindings";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // No heartbeats (solo), but write bindings
        write_binding(pid, "s1", "auth", "auth.method", "JWT RS256");

        let result = render_peer_updates(pid, "solo-session").unwrap();
        assert!(
            result.contains("JWT RS256"),
            "should show binding, got:\n{result}"
        );
        assert!(
            !result.contains("Peers"),
            "should NOT have peers header, got:\n{result}"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn render_peer_updates_solo_no_bindings() {
        let pid = "test_peers_updates_solo_none";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // No heartbeats, no bindings
        let result = render_peer_updates(pid, "solo-session");
        assert!(result.is_none(), "solo with no bindings should return None");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── find_binding_conflict tests (issue #121) ──

    #[test]
    fn binding_conflict_detects_different_value() {
        let pid = "test_conflict_different";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        write_binding(pid, "s1", "auth", "db.engine", "postgres");

        let conflict = find_binding_conflict(pid, "db.engine", "mysql");
        assert!(conflict.is_some(), "should detect conflict");
        let c = conflict.unwrap();
        assert_eq!(c.existing_value, "postgres");
        assert_eq!(c.by_label, "auth");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn binding_conflict_same_value_no_conflict() {
        let pid = "test_conflict_same";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        write_binding(pid, "s1", "auth", "db.engine", "postgres");

        let conflict = find_binding_conflict(pid, "db.engine", "postgres");
        assert!(conflict.is_none(), "same value should not conflict");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn binding_conflict_no_existing_binding() {
        let pid = "test_conflict_none";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        let conflict = find_binding_conflict(pid, "db.engine", "postgres");
        assert!(
            conflict.is_none(),
            "no existing binding should not conflict"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── infer_session_id tests ──

    #[test]
    fn infer_session_no_heartbeats() {
        let pid = "test_infer_none";
        let _ = edda_store::ensure_dirs(pid);

        let result = infer_session_id(pid);
        assert!(result.is_none(), "no heartbeats → None");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn infer_session_one_active() {
        let pid = "test_infer_one";
        let _ = edda_store::ensure_dirs(pid);

        write_heartbeat(pid, "sess-abc", &SessionSignals::default(), Some("auth"));

        let result = infer_session_id(pid);
        assert_eq!(result, Some(("sess-abc".into(), "auth".into())));

        remove_heartbeat(pid, "sess-abc");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn infer_session_two_active_is_ambiguous() {
        let pid = "test_infer_two";
        let _ = edda_store::ensure_dirs(pid);

        write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"));
        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));

        let result = infer_session_id(pid);
        assert!(result.is_none(), "two active → ambiguous → None");

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn infer_session_one_active_one_stale() {
        let pid = "test_infer_stale";
        let _ = edda_store::ensure_dirs(pid);

        // Write one fresh heartbeat
        write_heartbeat(pid, "fresh", &SessionSignals::default(), Some("frontend"));

        // Write a stale heartbeat by manually setting old timestamp
        let stale_path = heartbeat_path(pid, "stale");
        let stale_hb = serde_json::json!({
            "session_id": "stale",
            "started_at": "2020-01-01T00:00:00Z",
            "last_heartbeat": "2020-01-01T00:00:00Z",
            "label": "old",
            "focus_files": [],
            "active_tasks": [],
            "files_modified_count": 0,
            "total_edits": 0,
            "recent_commits": []
        });
        let _ = fs::create_dir_all(stale_path.parent().unwrap());
        let _ = fs::write(
            &stale_path,
            serde_json::to_string_pretty(&stale_hb).unwrap(),
        );

        let result = infer_session_id(pid);
        assert_eq!(result, Some(("fresh".into(), "frontend".into())));

        remove_heartbeat(pid, "fresh");
        remove_heartbeat(pid, "stale");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn infer_session_only_stale() {
        let pid = "test_infer_all_stale";
        let _ = edda_store::ensure_dirs(pid);

        let stale_path = heartbeat_path(pid, "old-session");
        let stale_hb = serde_json::json!({
            "session_id": "old-session",
            "started_at": "2020-01-01T00:00:00Z",
            "last_heartbeat": "2020-01-01T00:00:00Z",
            "label": "old",
            "focus_files": [],
            "active_tasks": [],
            "files_modified_count": 0,
            "total_edits": 0,
            "recent_commits": []
        });
        let _ = fs::create_dir_all(stale_path.parent().unwrap());
        let _ = fs::write(
            &stale_path,
            serde_json::to_string_pretty(&stale_hb).unwrap(),
        );

        let result = infer_session_id(pid);
        assert!(result.is_none(), "only stale heartbeats → None");

        remove_heartbeat(pid, "old-session");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── Issue #148 Gap 6: Cross-session decision conflict ──

    #[test]
    fn cross_session_binding_conflict_last_write_wins() {
        let pid = "test_cross_sess_conflict";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // Session A decides db.engine=postgres
        write_binding(pid, "s1", "auth", "db.engine", "postgres");
        // Session B decides db.engine=mysql (conflict — last write wins)
        write_binding(pid, "s2", "billing", "db.engine", "mysql");

        let board = compute_board_state(pid);
        assert_eq!(
            board.bindings.len(),
            1,
            "should have 1 binding (deduped by key)"
        );
        assert_eq!(board.bindings[0].value, "mysql", "last write should win");
        assert_eq!(board.bindings[0].by_session, "s2");

        // Both sessions see the latest value via render_peer_updates
        write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"));
        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));

        let updates_s1 = render_peer_updates(pid, "s1").unwrap();
        assert!(
            updates_s1.contains("mysql"),
            "Session A should see latest binding, got:\n{updates_s1}"
        );

        let updates_s2 = render_peer_updates(pid, "s2").unwrap();
        assert!(
            updates_s2.contains("mysql"),
            "Session B should see latest binding, got:\n{updates_s2}"
        );

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn cross_session_different_keys_both_visible() {
        let pid = "test_cross_sess_diff_keys";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // Session A decides db.engine=postgres
        write_binding(pid, "s1", "auth", "db.engine", "postgres");
        // Session B decides auth.method=JWT (different key — no conflict)
        write_binding(pid, "s2", "billing", "auth.method", "JWT");

        let board = compute_board_state(pid);
        assert_eq!(
            board.bindings.len(),
            2,
            "should have 2 bindings (different keys)"
        );

        // Both sessions see both bindings
        write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"));
        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));

        let updates_s1 = render_peer_updates(pid, "s1").unwrap();
        assert!(
            updates_s1.contains("postgres"),
            "s1 should see db.engine binding"
        );
        assert!(
            updates_s1.contains("JWT"),
            "s1 should see auth.method binding"
        );

        let updates_s2 = render_peer_updates(pid, "s2").unwrap();
        assert!(
            updates_s2.contains("postgres"),
            "s2 should see db.engine binding"
        );
        assert!(
            updates_s2.contains("JWT"),
            "s2 should see auth.method binding"
        );

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── Heartbeat label fallback tests (#146) ──

    #[test]
    fn request_delivered_via_heartbeat_label_no_claim() {
        let pid = "test_hb_fallback_request";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // Two sessions: s1 (peer) and s2 (me) — both have heartbeats, no claims
        write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"));
        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));

        // s1 sends request to "billing" (s2's heartbeat label)
        write_request(pid, "s1", "auth", "billing", "please expose /api/users");

        let result = render_coordination_protocol(pid, "s2", ".").unwrap();
        assert!(
            result.contains("Requests to you"),
            "request to heartbeat label should appear, got:\n{result}"
        );
        assert!(
            result.contains("please expose /api/users"),
            "request message should appear, got:\n{result}"
        );

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn explicit_claim_wins_over_heartbeat_for_requests() {
        let pid = "test_claim_wins_request";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // s2 has heartbeat "auth" but claim "backend"
        write_heartbeat(pid, "s1", &SessionSignals::default(), Some("peer"));
        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"));
        write_claim(pid, "s2", "backend", &[]);

        // Request to "backend" (claim label) should arrive
        write_request(pid, "s1", "peer", "backend", "need backend help");
        // Request to "auth" (heartbeat label) should NOT arrive (claim overrides)
        write_request(pid, "s1", "peer", "auth", "wrong target");

        let result = render_coordination_protocol(pid, "s2", ".").unwrap();
        assert!(
            result.contains("need backend help"),
            "request to claim label should appear, got:\n{result}"
        );
        assert!(
            !result.contains("wrong target"),
            "request to heartbeat label should NOT appear when claim exists, got:\n{result}"
        );

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn no_heartbeat_no_claim_no_requests() {
        let pid = "test_no_identity_request";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // s1 is peer, s2 has no heartbeat and no claim
        write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"));
        write_request(pid, "s1", "auth", "ghost", "hello ghost");

        // s2 renders — should not see the request (no identity)
        let result = render_coordination_protocol(pid, "s2", ".").unwrap();
        assert!(
            !result.contains("Requests to you"),
            "agent with no identity should see no requests, got:\n{result}"
        );

        remove_heartbeat(pid, "s1");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn heartbeat_scope_display_without_claim() {
        let pid = "test_hb_scope_display";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        write_heartbeat(pid, "s1", &SessionSignals::default(), Some("peer"));
        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"));

        let result = render_coordination_protocol(pid, "s2", ".").unwrap();
        assert!(
            result.contains("Your scope: **auth**"),
            "should show heartbeat-derived scope, got:\n{result}"
        );
        // Should NOT have paths (no claim, just heartbeat)
        assert!(
            !result.contains("Your scope: **auth** ("),
            "heartbeat scope should not have paths, got:\n{result}"
        );

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn claim_scope_display_with_paths() {
        let pid = "test_claim_scope_display";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        write_heartbeat(pid, "s1", &SessionSignals::default(), Some("peer"));
        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"));
        write_claim(pid, "s2", "backend", &["src/api/*".into()]);

        let result = render_coordination_protocol(pid, "s2", ".").unwrap();
        assert!(
            result.contains("Your scope: **backend** (src/api/*)"),
            "claim scope should show label + paths, got:\n{result}"
        );

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn multi_session_shows_l2_instructions() {
        let pid = "test_l2_instructions";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"));
        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));

        let result = render_coordination_protocol(pid, "s2", ".").unwrap();
        assert!(
            result.contains("edda claim"),
            "multi-session should contain claim instruction, got:\n{result}"
        );
        assert!(
            result.contains("edda request"),
            "multi-session should contain request instruction, got:\n{result}"
        );

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn solo_mode_no_l2_instructions() {
        let pid = "test_solo_no_l2_instr";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // Solo with binding (renders "## Binding Decisions" only)
        write_binding(pid, "s1", "auth", "db.engine", "postgres");
        let result = render_coordination_protocol(pid, "solo", ".").unwrap();
        assert!(
            !result.contains("edda claim"),
            "solo mode should NOT contain claim instruction, got:\n{result}"
        );
        assert!(
            !result.contains("edda request"),
            "solo mode should NOT contain request instruction, got:\n{result}"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn peer_updates_request_via_heartbeat_fallback() {
        let pid = "test_peer_updates_hb_req";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"));
        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));
        write_request(pid, "s1", "auth", "billing", "need billing API");

        let result = render_peer_updates(pid, "s2").unwrap();
        assert!(
            result.contains("need billing API"),
            "peer_updates should route request via heartbeat label, got:\n{result}"
        );

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── Auto-claim tests (issue #24) ──

    #[test]
    fn derive_scope_from_crate_files() {
        let files = vec![
            FileEditCount {
                path: "crates/edda-store/src/lib.rs".into(),
                count: 5,
            },
            FileEditCount {
                path: "crates/edda-store/src/resolve.rs".into(),
                count: 3,
            },
        ];
        let (label, paths) = derive_scope_from_files(&files).unwrap();
        assert_eq!(label, "edda-store");
        assert_eq!(paths, vec!["crates/edda-store/*"]);
    }

    #[test]
    fn derive_scope_from_src_module() {
        let files = vec![
            FileEditCount {
                path: "/repo/src/auth/jwt.rs".into(),
                count: 5,
            },
            FileEditCount {
                path: "/repo/src/auth/middleware.rs".into(),
                count: 2,
            },
        ];
        let (label, paths) = derive_scope_from_files(&files).unwrap();
        assert_eq!(label, "auth");
        assert_eq!(paths, vec!["src/auth/*"]);
    }

    #[test]
    fn derive_scope_empty_files() {
        assert!(derive_scope_from_files(&[]).is_none());
    }

    #[test]
    fn auto_claim_writes_claim_from_signals() {
        let pid = "test_autoclaim_writes";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        let signals = SessionSignals {
            files_modified: vec![FileEditCount {
                path: "crates/edda-store/src/lib.rs".into(),
                count: 5,
            }],
            ..Default::default()
        };

        maybe_auto_claim(pid, "s1", &signals);

        let board = compute_board_state(pid);
        assert_eq!(board.claims.len(), 1, "should have 1 claim");
        assert_eq!(board.claims[0].label, "edda-store");
        assert_eq!(board.claims[0].paths, vec!["crates/edda-store/*"]);

        remove_autoclaim_state(pid, "s1");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn auto_claim_skips_when_manual_claim_exists() {
        let pid = "test_autoclaim_skip_manual";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // Manual claim first
        write_claim(pid, "s1", "backend", &["src/api/*".into()]);

        let signals = SessionSignals {
            files_modified: vec![FileEditCount {
                path: "crates/edda-store/src/lib.rs".into(),
                count: 5,
            }],
            ..Default::default()
        };

        maybe_auto_claim(pid, "s1", &signals);

        let board = compute_board_state(pid);
        let claim = board.claims.iter().find(|c| c.session_id == "s1").unwrap();
        assert_eq!(
            claim.label, "backend",
            "manual claim should be preserved, not overwritten by auto-claim"
        );

        remove_autoclaim_state(pid, "s1");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn auto_claim_dedup_no_repeated_writes() {
        let pid = "test_autoclaim_dedup";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        let signals = SessionSignals {
            files_modified: vec![FileEditCount {
                path: "crates/edda-store/src/lib.rs".into(),
                count: 5,
            }],
            ..Default::default()
        };

        maybe_auto_claim(pid, "s1", &signals);
        maybe_auto_claim(pid, "s1", &signals);

        let content = fs::read_to_string(coordination_path(pid)).unwrap_or_default();
        let claim_count = content.lines().filter(|l| l.contains("\"claim\"")).count();
        assert_eq!(claim_count, 1, "dedup should prevent repeated claim writes");

        remove_autoclaim_state(pid, "s1");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn auto_claim_updates_on_scope_change() {
        let pid = "test_autoclaim_scope_change";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        let signals1 = SessionSignals {
            files_modified: vec![FileEditCount {
                path: "crates/edda-store/src/lib.rs".into(),
                count: 5,
            }],
            ..Default::default()
        };
        maybe_auto_claim(pid, "s1", &signals1);

        let signals2 = SessionSignals {
            files_modified: vec![FileEditCount {
                path: "crates/edda-bridge-claude/src/peers.rs".into(),
                count: 10,
            }],
            ..Default::default()
        };
        maybe_auto_claim(pid, "s1", &signals2);

        let board = compute_board_state(pid);
        let claim = board.claims.iter().find(|c| c.session_id == "s1").unwrap();
        assert_eq!(
            claim.label, "edda-bridge-claude",
            "claim should update to new scope"
        );

        remove_autoclaim_state(pid, "s1");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn auto_claim_cleanup_removes_state_file() {
        let pid = "test_autoclaim_cleanup";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        let signals = SessionSignals {
            files_modified: vec![FileEditCount {
                path: "crates/edda-store/src/lib.rs".into(),
                count: 5,
            }],
            ..Default::default()
        };
        maybe_auto_claim(pid, "s1", &signals);

        let state_path = autoclaim_state_path(pid, "s1");
        assert!(
            state_path.exists(),
            "state file should exist after auto-claim"
        );

        remove_autoclaim_state(pid, "s1");
        assert!(
            !state_path.exists(),
            "state file should be removed after cleanup"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn render_shows_branch_when_present() {
        let pid = "test_peers_branch_render";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // Write heartbeat with branch via JSON (bypassing auto-detect)
        let hb_json = serde_json::json!({
            "session_id": "s1",
            "started_at": now_rfc3339(),
            "last_heartbeat": now_rfc3339(),
            "label": "auth",
            "focus_files": ["src/auth/lib.rs"],
            "active_tasks": [],
            "files_modified_count": 1,
            "total_edits": 3,
            "recent_commits": [],
            "branch": "feat/issue-81-peer-branch"
        });
        let path = edda_store::project_dir(pid)
            .join("state")
            .join("session.s1.json");
        let _ = fs::create_dir_all(path.parent().unwrap());
        fs::write(&path, serde_json::to_string_pretty(&hb_json).unwrap()).unwrap();

        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));

        let result = render_coordination_protocol(pid, "s2", ".").unwrap();
        assert!(
            result.contains("[branch: feat/issue-81-peer-branch]"),
            "should show branch in protocol, got:\n{result}"
        );

        let updates = render_peer_updates(pid, "s2").unwrap();
        assert!(
            updates.contains("[branch: feat/issue-81-peer-branch]"),
            "should show branch in peer updates, got:\n{updates}"
        );

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn render_omits_branch_when_absent() {
        let pid = "test_peers_branch_absent";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // Write heartbeat WITHOUT branch field (simulating old heartbeat format)
        let hb_json = serde_json::json!({
            "session_id": "s1",
            "started_at": now_rfc3339(),
            "last_heartbeat": now_rfc3339(),
            "label": "auth",
            "focus_files": ["src/auth/lib.rs"],
            "active_tasks": [],
            "files_modified_count": 1,
            "total_edits": 3,
            "recent_commits": []
        });
        let path = edda_store::project_dir(pid)
            .join("state")
            .join("session.s1.json");
        let _ = fs::create_dir_all(path.parent().unwrap());
        fs::write(&path, serde_json::to_string_pretty(&hb_json).unwrap()).unwrap();

        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));

        let result = render_coordination_protocol(pid, "s2", ".").unwrap();
        assert!(
            !result.contains("[branch:"),
            "should NOT show branch marker when absent, got:\n{result}"
        );

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── Precomputed _with variants match original output (#83) ──

    #[test]
    fn render_peer_updates_with_matches_original() {
        let pid = "test_updates_with_match";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        let signals = SessionSignals {
            tasks: vec![TaskSnapshot {
                id: "1".into(),
                subject: "Fix auth bug".into(),
                status: "in_progress".into(),
            }],
            ..Default::default()
        };
        write_heartbeat(pid, "s1", &signals, Some("auth"));
        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));
        write_binding(pid, "s1", "auth", "db.engine", "postgres");

        // Call original wrapper
        let original = render_peer_updates(pid, "s2");

        // Call _with variant with same data
        let peers = discover_active_peers(pid, "s2");
        let board = compute_board_state(pid);
        let precomputed = render_peer_updates_with(&peers, &board, pid, "s2");

        assert_eq!(
            original, precomputed,
            "precomputed variant should match original"
        );

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn render_coordination_protocol_with_matches_original() {
        let pid = "test_protocol_with_match";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        let signals = SessionSignals {
            tasks: vec![TaskSnapshot {
                id: "1".into(),
                subject: "Implement billing".into(),
                status: "in_progress".into(),
            }],
            ..Default::default()
        };
        write_heartbeat(pid, "s1", &signals, Some("billing"));
        write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"));
        write_binding(pid, "s1", "billing", "payment.provider", "stripe");

        // Call original wrapper
        let original = render_coordination_protocol(pid, "s2", ".");

        // Call _with variant with same data
        let peers = discover_active_peers(pid, "s2");
        let board = compute_board_state(pid);
        let precomputed = render_coordination_protocol_with(&peers, &board, pid, "s2");

        assert_eq!(
            original, precomputed,
            "precomputed variant should match original"
        );

        remove_heartbeat(pid, "s1");
        remove_heartbeat(pid, "s2");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn render_peer_updates_with_solo_bindings() {
        let pid = "test_updates_with_solo";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // No heartbeats (solo), but write bindings
        write_binding(pid, "s1", "auth", "auth.method", "JWT RS256");

        let peers = discover_active_peers(pid, "solo-session");
        let board = compute_board_state(pid);
        let result = render_peer_updates_with(&peers, &board, pid, "solo-session");

        assert!(result.is_some(), "solo with bindings should render");
        assert!(result.unwrap().contains("JWT RS256"), "should show binding");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn render_peer_updates_with_solo_no_bindings() {
        let pid = "test_updates_with_solo_empty";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        let peers = discover_active_peers(pid, "solo-session");
        let board = compute_board_state(pid);
        let result = render_peer_updates_with(&peers, &board, pid, "solo-session");

        assert!(result.is_none(), "solo with no bindings should return None");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── Auto-claim file incremental tests (#56) ──

    #[test]
    fn auto_claim_file_incremental_same_crate() {
        let pid = "test_autoclaim_file_incr";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // Edit 3 files in same crate → single claim written
        maybe_auto_claim_file(pid, "s1", "crates/edda-store/src/lib.rs");
        maybe_auto_claim_file(pid, "s1", "crates/edda-store/src/paths.rs");
        maybe_auto_claim_file(pid, "s1", "crates/edda-store/src/event.rs");

        let board = compute_board_state(pid);
        let claims: Vec<_> = board
            .claims
            .iter()
            .filter(|c| c.session_id == "s1")
            .collect();
        assert_eq!(claims.len(), 1, "should have exactly one claim");
        assert_eq!(claims[0].label, "edda-store");

        // Verify state file has all 3 files tracked
        let state_path = autoclaim_state_path(pid, "s1");
        let state: AutoClaimState =
            serde_json::from_str(&fs::read_to_string(&state_path).unwrap()).unwrap();
        assert_eq!(state.files.len(), 3);

        remove_autoclaim_state(pid, "s1");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn auto_claim_file_scope_change() {
        let pid = "test_autoclaim_file_scope_change";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // First file in edda-store
        maybe_auto_claim_file(pid, "s1", "crates/edda-store/src/lib.rs");
        let board = compute_board_state(pid);
        let claim = board.claims.iter().find(|c| c.session_id == "s1").unwrap();
        assert_eq!(claim.label, "edda-store");

        // Second file in different crate → scope should change
        maybe_auto_claim_file(pid, "s1", "crates/edda-bridge-claude/src/dispatch.rs");
        let board2 = compute_board_state(pid);
        let claim2 = board2.claims.iter().find(|c| c.session_id == "s1").unwrap();
        // With 2 crates, label should be updated (might become multi-crate or dominant one)
        assert!(
            !claim2.label.is_empty(),
            "label should be non-empty after cross-crate edit"
        );

        remove_autoclaim_state(pid, "s1");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn auto_claim_file_skips_manual_claim() {
        let pid = "test_autoclaim_file_manual";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // Manual claim exists
        write_claim(pid, "s1", "auth", &["src/auth/*".into()]);

        // Auto-claim file should be skipped (no state file, manual claim exists)
        maybe_auto_claim_file(pid, "s1", "crates/edda-store/src/lib.rs");

        // Claim should still be "auth" (manual), not "edda-store" (auto)
        let board = compute_board_state(pid);
        let claim = board.claims.iter().find(|c| c.session_id == "s1").unwrap();
        assert_eq!(
            claim.label, "auth",
            "manual claim should not be overwritten"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn auto_claim_file_dedup_no_extra_writes() {
        let pid = "test_autoclaim_file_dedup";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // Same file twice → only one claim event
        maybe_auto_claim_file(pid, "s1", "crates/edda-store/src/lib.rs");
        maybe_auto_claim_file(pid, "s1", "crates/edda-store/src/lib.rs");

        let board = compute_board_state(pid);
        let claims: Vec<_> = board
            .claims
            .iter()
            .filter(|c| c.session_id == "s1")
            .collect();
        assert_eq!(claims.len(), 1, "dedup: same file should produce one claim");

        remove_autoclaim_state(pid, "s1");
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── Request ack tests (#56) ──

    #[test]
    fn request_ack_filters_pending() {
        let pid = "test_req_ack_filters";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // Setup: s1 claims "auth", s2 sends request to "auth"
        write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
        write_request(pid, "s2", "billing", "auth", "Export AuthToken type");

        // s1 should see the pending request
        let pending = pending_requests_for_session(pid, "s1");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].message, "Export AuthToken type");

        // s1 acks the request
        write_request_ack(pid, "s1", "billing");

        // Now pending should be empty for s1
        let pending_after = pending_requests_for_session(pid, "s1");
        assert!(
            pending_after.is_empty(),
            "acked request should not appear as pending"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn request_ack_only_for_acker_session() {
        let pid = "test_req_ack_session_scope";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // s1 and s3 both claim "auth"
        write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
        write_claim(pid, "s3", "auth", &["src/auth/*".into()]);
        write_request(pid, "s2", "billing", "auth", "Export AuthToken");

        // s1 acks
        write_request_ack(pid, "s1", "billing");

        // s1 should no longer see it
        let pending_s1 = pending_requests_for_session(pid, "s1");
        assert!(pending_s1.is_empty(), "s1 acked, should not see request");

        // s3 should still see it (different session, same label)
        let pending_s3 = pending_requests_for_session(pid, "s3");
        assert_eq!(
            pending_s3.len(),
            1,
            "s3 has not acked, should still see request"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn request_ack_in_board_state() {
        let pid = "test_req_ack_board";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        write_request_ack(pid, "s1", "billing");
        let board = compute_board_state(pid);
        assert_eq!(board.request_acks.len(), 1);
        assert_eq!(board.request_acks[0].acker_session, "s1");
        assert_eq!(board.request_acks[0].from_label, "billing");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn compaction_preserves_request_acks() {
        let pid = "test_compaction_acks";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
        write_request(pid, "s2", "billing", "auth", "Export AuthToken");
        write_request_ack(pid, "s1", "billing");

        // Before compaction: ack should exist
        let board_before = compute_board_state(pid);
        assert_eq!(board_before.request_acks.len(), 1);
        let pending_before = pending_requests_for_session(pid, "s1");
        assert!(
            pending_before.is_empty(),
            "acked request should not be pending"
        );

        // Compact
        let lines = compute_board_state_for_compaction(pid);
        assert_eq!(lines.len(), 3, "claim + request + ack = 3 lines");

        // Write compacted back
        let path = coordination_path(pid);
        let content = lines.join("\n");
        fs::write(&path, format!("{content}\n")).unwrap();

        // After compaction: ack should still exist
        let board_after = compute_board_state(pid);
        assert_eq!(board_after.request_acks.len(), 1);
        let pending_after = pending_requests_for_session(pid, "s1");
        assert!(
            pending_after.is_empty(),
            "acked request should still not be pending after compaction"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn pending_requests_no_label_returns_empty() {
        let pid = "test_pending_no_label";
        let _ = edda_store::ensure_dirs(pid);
        let _ = fs::remove_file(coordination_path(pid));

        // s1 has no claim and no heartbeat → no label → no pending requests
        write_request(pid, "s2", "billing", "auth", "Need auth API");
        let pending = pending_requests_for_session(pid, "s1");
        assert!(
            pending.is_empty(),
            "session with no label should have no pending requests"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }
}
