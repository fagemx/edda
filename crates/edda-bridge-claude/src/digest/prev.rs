use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::extract::{compute_tool_ratios, extract_stats, load_tasks_for_digest};
use super::helpers::now_rfc3339;
use super::SessionStats;

// ── Previous Session Digest Snapshot ──

/// Snapshot of a completed session, persisted for next session's context injection.
/// Written at SessionEnd, read at next SessionStart, deleted at that session's end.
#[derive(Debug, Serialize, Deserialize)]
pub struct PrevDigest {
    pub session_id: String,
    pub completed_at: String,
    pub outcome: String,
    pub duration_minutes: u64,
    pub completed_tasks: Vec<String>,
    pub pending_tasks: Vec<String>,
    pub commits: Vec<String>,
    pub files_modified_count: usize,
    pub total_edits: usize,
    /// Decisions recorded via `edda decide` during the session.
    #[serde(default)]
    pub decisions: Vec<String>,
    /// Notes recorded via `edda note` during the session.
    #[serde(default)]
    pub notes: Vec<String>,
    /// Failed commands from the session (data-only, not rendered).
    #[serde(default)]
    pub failed_commands: Vec<String>,
    /// Number of nudges emitted during this session.
    #[serde(default)]
    pub nudge_count: u64,
    /// Number of times agent called `edda decide`.
    #[serde(default)]
    pub decide_count: u64,
    /// Total decision-worthy signals detected (including suppressed ones).
    #[serde(default)]
    pub signal_count: u64,
    /// Per-tool call counts (e.g. "Read" -> 15, "Edit" -> 8).
    #[serde(default)]
    pub tool_call_breakdown: BTreeMap<String, u64>,
    /// Ratio of edit tools (Edit, Write, NotebookEdit) to total tool calls.
    #[serde(default)]
    pub edit_ratio: f64,
    /// Ratio of search tools (Read, Grep, Glob, Agent) to total tool calls.
    #[serde(default)]
    pub search_ratio: f64,
    /// Model name used in this session.
    #[serde(default)]
    pub model: String,
    /// Total input tokens consumed.
    #[serde(default)]
    pub input_tokens: u64,
    /// Total output tokens consumed.
    #[serde(default)]
    pub output_tokens: u64,
    /// Total cache-read input tokens.
    #[serde(default)]
    pub cache_read_tokens: u64,
    /// Total cache-creation input tokens.
    #[serde(default)]
    pub cache_creation_tokens: u64,
    /// Estimated cost in USD.
    #[serde(default)]
    pub estimated_cost_usd: f64,
    /// Activity classification for this session.
    #[serde(default)]
    pub activity: String,
}

/// Write prev_digest.json from SessionStats + optional ledger extras.
pub fn write_prev_digest(
    project_id: &str,
    session_id: &str,
    stats: &SessionStats,
    decisions: Vec<String>,
    notes: Vec<String>,
) {
    let completed: Vec<String> = stats
        .tasks_snapshot
        .iter()
        .filter(|t| t.status == "completed")
        .map(|t| t.subject.clone())
        .collect();
    let pending: Vec<String> = stats
        .tasks_snapshot
        .iter()
        .filter(|t| t.status != "completed")
        .map(|t| t.subject.clone())
        .collect();

    // Read total_edits from state/files_modified.json (still alive at SessionEnd)
    let total_edits = read_total_edits(project_id);

    let (edit_ratio, search_ratio) =
        compute_tool_ratios(&stats.tool_call_breakdown, stats.tool_calls);

    let digest = PrevDigest {
        session_id: session_id.to_string(),
        completed_at: now_rfc3339(),
        outcome: stats.outcome.to_string(),
        duration_minutes: stats.duration_minutes,
        completed_tasks: completed,
        pending_tasks: pending,
        commits: stats.commits_made.clone(),
        files_modified_count: stats.files_modified.len(),
        total_edits,
        decisions,
        notes,
        failed_commands: stats.failed_commands.clone(),
        nudge_count: stats.nudge_count,
        decide_count: stats.decide_count,
        signal_count: stats.signal_count,
        tool_call_breakdown: stats.tool_call_breakdown.clone(),
        edit_ratio,
        search_ratio,
        model: stats.model.clone(),
        input_tokens: stats.input_tokens,
        output_tokens: stats.output_tokens,
        cache_read_tokens: stats.cache_read_tokens,
        cache_creation_tokens: stats.cache_creation_tokens,
        estimated_cost_usd: stats.estimated_cost_usd,
        activity: stats.activity.to_string(),
    };

    let path = edda_store::project_dir(project_id)
        .join("state")
        .join("prev_digest.json");
    if let Ok(data) = serde_json::to_string_pretty(&digest) {
        let _ = edda_store::write_atomic(&path, data.as_bytes());
    }
}

/// Read total edit count from state/files_modified.json.
fn read_total_edits(project_id: &str) -> usize {
    let path = edda_store::project_dir(project_id)
        .join("state")
        .join("files_modified.json");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let val: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    val.get("files")
        .and_then(|f| f.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("count").and_then(|c| c.as_u64()))
                .sum::<u64>() as usize
        })
        .unwrap_or(0)
}

/// Read prev_digest.json for rendering.
pub fn read_prev_digest(project_id: &str) -> Option<PrevDigest> {
    let path = edda_store::project_dir(project_id)
        .join("state")
        .join("prev_digest.json");
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Collect decisions and notes from the workspace ledger that were written
/// during this session (events with `ts >= session_first_ts`).
///
/// Returns `(decisions, notes)`. Gracefully returns empty vecs on any error.
pub fn collect_session_ledger_extras(
    cwd: &str,
    session_first_ts: Option<&str>,
) -> (Vec<String>, Vec<String>) {
    let first_ts = match session_first_ts {
        Some(ts) if !ts.is_empty() => ts,
        _ => return (Vec::new(), Vec::new()),
    };

    let cwd_path = Path::new(cwd);
    let root = match edda_ledger::EddaPaths::find_root(cwd_path) {
        Some(r) => r,
        None => return (Vec::new(), Vec::new()),
    };
    let ledger = match edda_ledger::Ledger::open(&root) {
        Ok(l) => l,
        Err(_) => return (Vec::new(), Vec::new()),
    };
    let events = match ledger.iter_events() {
        Ok(e) => e,
        Err(_) => return (Vec::new(), Vec::new()),
    };

    let mut decisions = Vec::new();
    let mut notes = Vec::new();

    for event in events.iter().rev() {
        // Only events from this session (by timestamp)
        if event.ts.as_str() < first_ts {
            break;
        }
        if event.event_type != "note" {
            continue;
        }

        // Skip auto-generated digest notes
        let source = event
            .payload
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if source.starts_with("bridge:") {
            continue;
        }

        let tags: Vec<&str> = event
            .payload
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|t| t.as_str()).collect())
            .unwrap_or_default();

        if tags.contains(&"decision") {
            if let Some(dp) = edda_core::decision::extract_decision(&event.payload) {
                let formatted = match &dp.reason {
                    Some(r) => format!("{}={} ({})", dp.key, dp.value, r),
                    None => format!("{}={}", dp.key, dp.value),
                };
                decisions.push(formatted);
            } else if let Some(text) = event.payload.get("text").and_then(|v| v.as_str()) {
                decisions.push(text.to_string());
            }
        } else if tags.contains(&"session") {
            // Session note written by agent via `edda note --tag session`
            if let Some(text) = event.payload.get("text").and_then(|v| v.as_str()) {
                notes.push(text.to_string());
            }
        }
    }

    // Reverse to chronological order (we iterated in reverse)
    decisions.reverse();
    notes.reverse();

    (decisions, notes)
}

/// Convenience: extract stats from stored transcript, enrich with ledger data, and write prev_digest.
pub fn write_prev_digest_from_store(
    project_id: &str,
    session_id: &str,
    cwd: &str,
    nudge_count: u64,
    decide_count: u64,
    signal_count: u64,
) {
    let store_path = edda_store::project_dir(project_id)
        .join("ledger")
        .join(format!("{session_id}.jsonl"));
    if !store_path.exists() {
        return;
    }
    let mut stats = match extract_stats(&store_path) {
        Ok(s) => s,
        Err(_) => return,
    };
    // Supplement tasks from state (extract_stats reads from session ledger which
    // may not have task data; state/active_tasks.json is authoritative)
    if stats.tasks_snapshot.is_empty() {
        stats.tasks_snapshot = load_tasks_for_digest(project_id);
    }
    // Supplement recall rate counters from dispatch state files
    stats.nudge_count = nudge_count;
    stats.decide_count = decide_count;
    stats.signal_count = signal_count;
    // Supplement usage data from transcript signals
    {
        let usage = crate::signals::read_usage_state(project_id);
        if !usage.model.is_empty() {
            stats.model = usage.model.clone();
        }
        stats.input_tokens = usage.input_tokens;
        stats.output_tokens = usage.output_tokens;
        stats.cache_read_tokens = usage.cache_read_tokens;
        stats.cache_creation_tokens = usage.cache_creation_tokens;
        stats.estimated_cost_usd = crate::signals::estimate_cost(&usage);
    }
    // Collect decisions + notes from workspace ledger before writing
    let (decisions, notes) = collect_session_ledger_extras(cwd, stats.first_ts.as_deref());
    write_prev_digest(project_id, session_id, &stats, decisions, notes);
}
