//! Deterministic session digest extraction.
//!
//! Reads a session ledger (EventEnvelope JSONL) and produces a
//! `edda_core::Event` milestone summarizing the session — without LLM,
//! without touching the workspace ledger.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A failed Bash command extracted from the session ledger.
#[derive(Debug, Clone)]
pub struct FailedCommand {
    pub command: String,
    pub cwd: String,
    pub exit_code: i32,
}

/// A task snapshot for digest payload (cross-session continuity).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigestTaskSnapshot {
    pub subject: String,
    pub status: String,
}

/// Session outcome classification.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionOutcome {
    /// Normal session end.
    #[default]
    Completed,
    /// User left mid-conversation (last event is a user prompt with no response).
    Interrupted,
    /// Session ended stuck on repeated failures.
    ErrorStuck,
}

impl std::fmt::Display for SessionOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionOutcome::Completed => write!(f, "completed"),
            SessionOutcome::Interrupted => write!(f, "interrupted"),
            SessionOutcome::ErrorStuck => write!(f, "error_stuck"),
        }
    }
}

/// Activity classification for a session.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityType {
    #[default]
    Unknown,
    Feature,
    Fix,
    Debug,
    Docs,
    Research,
    Chat,
    Ops,
}

impl std::fmt::Display for ActivityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActivityType::Unknown => write!(f, "unknown"),
            ActivityType::Feature => write!(f, "feature"),
            ActivityType::Fix => write!(f, "fix"),
            ActivityType::Debug => write!(f, "debug"),
            ActivityType::Docs => write!(f, "docs"),
            ActivityType::Research => write!(f, "research"),
            ActivityType::Chat => write!(f, "chat"),
            ActivityType::Ops => write!(f, "ops"),
        }
    }
}

/// Statistics extracted from a session ledger.
#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    pub tool_calls: u64,
    pub tool_failures: u64,
    pub user_prompts: u64,
    pub files_modified: Vec<String>,
    pub failed_commands: Vec<String>,
    /// Rich detail for each failed command (for cmd milestone events).
    pub failed_cmds_detail: Vec<FailedCommand>,
    /// Git commits made during this session (commit messages).
    pub commits_made: Vec<String>,
    /// Task subjects + statuses at session end.
    pub tasks_snapshot: Vec<DigestTaskSnapshot>,
    /// How the session ended.
    pub outcome: SessionOutcome,
    pub duration_minutes: u64,
    pub first_ts: Option<String>,
    pub last_ts: Option<String>,
    /// Number of nudges emitted during this session.
    pub nudge_count: u64,
    /// Number of times agent called `edda decide`.
    pub decide_count: u64,
    /// Total number of decision-worthy signals detected (including suppressed ones).
    pub signal_count: u64,
    /// Dependency packages added during this session (for passive harvest).
    pub deps_added: Vec<String>,
    /// Per-file edit counts: (file_path, count).
    pub file_edit_counts: Vec<(String, u64)>,
    /// Per-tool call counts (e.g. "Read" -> 15, "Edit" -> 8).
    pub tool_call_breakdown: BTreeMap<String, u64>,
    /// Model name used in this session.
    pub model: String,
    /// Total input tokens consumed.
    pub input_tokens: u64,
    /// Total output tokens consumed.
    pub output_tokens: u64,
    /// Total cache-read input tokens.
    pub cache_read_tokens: u64,
    /// Total cache-creation input tokens.
    pub cache_creation_tokens: u64,
    /// Estimated cost in USD.
    pub estimated_cost_usd: f64,
    /// Activity classification for this session.
    pub activity: ActivityType,
}

/// Extract statistics from a session ledger file.
mod extract;
mod helpers;
mod orchestrate;
mod prev;
mod render;

// Re-export all public items to preserve API
pub use extract::{extract_stats, load_tasks_for_digest, render_digest_text};
pub use orchestrate::{
    digest_previous_sessions, digest_previous_sessions_with_opts, digest_session_manual,
    find_all_pending_sessions, load_digest_state, pending_failure_warning, save_digest_state,
    DigestResult, DigestState,
};
pub use prev::{
    collect_session_ledger_extras, read_prev_digest, write_prev_digest,
    write_prev_digest_from_store, PrevDigest,
};
pub use render::{build_cmd_milestone_event, build_digest_event, extract_session_digest};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
