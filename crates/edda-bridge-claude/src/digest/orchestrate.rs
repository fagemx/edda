use std::collections::BTreeMap;
use std::path::Path;

use edda_core::event::finalize_event;
use edda_core::types::Provenance;
use serde::{Deserialize, Serialize};

use super::extract::{extract_stats_from, load_tasks_for_digest};
use super::helpers::now_rfc3339;
use super::prev::collect_session_ledger_extras;
use super::render::{build_cmd_milestone_event, build_digest_event};
use super::SessionStats;

// ── Auto-Digest Orchestration ──

/// Per-session digest watermark (GH-578 round-1).
///
/// Idempotency ruling `digest.idempotency=per-session-watermark-never-delete-the-source`:
/// the digest paths never delete the session ledger, so the watermark — not
/// the file's disappearance — is what bounds re-digesting.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct DigestedSession {
    /// Byte offset into the session ledger: everything strictly before this
    /// offset has been digested. Only complete, newline-terminated lines are
    /// ever consumed, so a truncated or concurrently-written final line is
    /// picked up once its write completes (round-1 P0-2).
    #[serde(default)]
    pub offset: u64,
    /// Event id of the latest digest event written for THIS session
    /// (round-1 P1-4: per-session id, not one global latest id).
    #[serde(default)]
    pub event_id: String,
    /// When this watermark was last advanced.
    #[serde(default)]
    pub digested_at: String,
}

/// State file tracking which sessions have been digested.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DigestState {
    /// The most recent session_id that was successfully digested (mirror of
    /// the latest entry in `sessions`; kept for legacy readers).
    #[serde(default)]
    pub session_id: String,
    /// When the latest digest was written.
    #[serde(default)]
    pub digested_at: String,
    /// Event id of the latest digest event written (any session; mirror of
    /// the latest entry in `sessions`). Per-session ids live in `sessions`.
    #[serde(default)]
    pub event_id: String,
    /// Number of consecutive failed digest attempts for the pending session.
    #[serde(default)]
    pub retry_count: u32,
    /// Session ID that failed to digest (if any).
    #[serde(default)]
    pub pending_session_id: String,
    /// Last failure message.
    #[serde(default)]
    pub last_error: String,
    /// Per-session digested watermarks (GH-578 round-1).
    ///
    /// Replaces the previous 64-entry FIFO of session ids: a bounded FIFO
    /// evicts the oldest id and re-opens the digest loop for a session whose
    /// ledger still exists (round-1 P1-3). Unbounded per-session entries are
    /// intentional — the file is small (~100 bytes per session) and written
    /// atomically, and each entry is what makes retries of a remembered
    /// session return that session's own event id.
    #[serde(default)]
    pub sessions: BTreeMap<String, DigestedSession>,
    /// Deprecated pre-watermark session ids, kept only so old state files
    /// deserialize; migrated into `sessions` on load and never written back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub digested: Vec<String>,
}

/// Load the digest state from the per-user store.
pub fn load_digest_state(project_id: &str) -> DigestState {
    let path = digest_state_path(project_id);
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let mut state: DigestState = serde_json::from_str(&content).unwrap_or_default();
            migrate_legacy_state(project_id, &mut state);
            state
        }
        Err(_) => DigestState::default(),
    }
}

/// Migrate pre-watermark state into the per-session model (round-1 P1-2).
///
/// A pre-upgrade file records at most one digested session via
/// `session_id`/`event_id` (and, in intermediate versions, a list of ids in
/// the deprecated `digested` field) with no watermark. Seeding the map keeps
/// the recorded sessions digested: the legacy model digested a session's
/// whole file, so everything currently on disk for it is treated as already
/// consumed; content appended afterwards is picked up as a normal delta.
pub fn migrate_legacy_state(project_id: &str, state: &mut DigestState) {
    if !state.sessions.is_empty() {
        return;
    }
    let mut legacy_ids = state.digested.clone();
    if !state.session_id.is_empty() && !legacy_ids.contains(&state.session_id) {
        legacy_ids.push(state.session_id.clone());
    }
    if legacy_ids.is_empty() {
        return;
    }
    let ledger_dir = edda_store::project_dir(project_id).join("ledger");
    for id in legacy_ids {
        let offset = std::fs::metadata(ledger_dir.join(format!("{id}.jsonl")))
            .map(|m| m.len())
            .unwrap_or(0);
        // Only the single legacy `session_id` slot has a known event id.
        let (event_id, digested_at) = if id == state.session_id {
            (state.event_id.clone(), state.digested_at.clone())
        } else {
            (String::new(), String::new())
        };
        state.sessions.insert(
            id,
            DigestedSession {
                offset,
                event_id,
                digested_at,
            },
        );
    }
    state.digested.clear();
}

/// Save the digest state to the per-user store.
pub fn save_digest_state(project_id: &str, state: &DigestState) -> anyhow::Result<()> {
    let path = digest_state_path(project_id);
    let json = serde_json::to_string_pretty(state)?;
    edda_store::write_atomic(&path, json.as_bytes())
}

fn digest_state_path(project_id: &str) -> std::path::PathBuf {
    edda_store::project_dir(project_id)
        .join("state")
        .join("last_digested_session.json")
}

/// Record how far a session has been digested and which event id it got.
fn remember_digested(state: &mut DigestState, session_id: &str, event_id: &str, offset: u64) {
    state.sessions.insert(
        session_id.to_string(),
        DigestedSession {
            offset,
            event_id: event_id.to_string(),
            digested_at: now_rfc3339(),
        },
    );
}

/// Find session ledger files in the store, excluding the current session.
fn find_pending_sessions(
    project_id: &str,
    current_session_id: &str,
    state: &DigestState,
) -> Vec<String> {
    let ledger_dir = edda_store::project_dir(project_id).join("ledger");
    let entries = match std::fs::read_dir(&ledger_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".jsonl") {
            continue;
        }
        let session_id = name.trim_end_matches(".jsonl").to_string();
        // Skip current session (still in progress)
        if session_id == current_session_id {
            continue;
        }
        // Skip already-digested sessions (GH-578; round-1 P1-3: the
        // per-session map is unbounded, so an entry can never be evicted
        // back into this list while its ledger still exists).
        if session_id == state.session_id || state.sessions.contains_key(&session_id) {
            continue;
        }
        sessions.push(session_id);
    }
    // Sort for deterministic order (oldest first by ULID/name)
    sessions.sort();
    sessions
}

/// Try to acquire WorkspaceLock with a timeout (retry loop).
/// Returns None if lock cannot be acquired within the timeout.
fn try_lock_with_timeout(
    paths: &edda_ledger::EddaPaths,
    timeout_ms: u64,
) -> Option<edda_ledger::WorkspaceLock> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(timeout_ms);
    let sleep_interval = std::time::Duration::from_millis(100);

    loop {
        match edda_ledger::WorkspaceLock::acquire(paths) {
            Ok(lock) => return Some(lock),
            Err(_) => {
                if start.elapsed() >= timeout {
                    return None;
                }
                std::thread::sleep(sleep_interval);
            }
        }
    }
}

/// Result of a digest attempt.
#[derive(Debug)]
pub enum DigestResult {
    /// Successfully wrote milestone to workspace ledger.
    Written { event_id: String },
    /// No pending sessions to digest.
    NoPending,
    /// Skipped because auto_digest is disabled.
    Disabled,
    /// Failed to acquire workspace lock.
    LockTimeout,
    /// Failed with an error (recorded in state).
    Error(String),
    /// Permanently failed (retry_count >= 3), includes warning message.
    PermanentFailure(String),
}

/// Main orchestration: digest any pending sessions into the workspace ledger.
///
/// Called from SessionStart hook dispatch. Returns a DigestResult indicating
/// what happened (for logging/warning injection).
pub fn digest_previous_sessions(
    project_id: &str,
    current_session_id: &str,
    cwd: &str,
    lock_timeout_ms: u64,
) -> DigestResult {
    // Phantom cmd events are disabled by default: the digest note already
    // records failed_commands, and separate cmd events inflate the ledger
    // without adding value (they have duration_ms=0, no stdout/stderr).
    digest_previous_sessions_with_opts(project_id, current_session_id, cwd, lock_timeout_ms, false)
}

/// Main orchestration with explicit failed-cmd option.
pub fn digest_previous_sessions_with_opts(
    project_id: &str,
    current_session_id: &str,
    cwd: &str,
    lock_timeout_ms: u64,
    digest_failed_cmds: bool,
) -> DigestResult {
    // Load state
    let mut state = load_digest_state(project_id);

    // Check for permanent failure from previous attempts
    if !state.pending_session_id.is_empty() && state.retry_count >= 3 {
        let warning = format!(
            "edda: session {} digest failed {} times ({}). Run: edda bridge digest --session {}",
            &state.pending_session_id[..state.pending_session_id.len().min(8)],
            state.retry_count,
            state.last_error,
            state.pending_session_id,
        );
        return DigestResult::PermanentFailure(warning);
    }

    // Find sessions to digest
    let pending = find_pending_sessions(project_id, current_session_id, &state);
    if pending.is_empty() {
        // Check if there's a retry pending
        if !state.pending_session_id.is_empty() && state.retry_count > 0 {
            let retry_id = state.pending_session_id.clone();
            return digest_one_session(
                project_id,
                &retry_id,
                cwd,
                lock_timeout_ms,
                digest_failed_cmds,
                &mut state,
            );
        }
        return DigestResult::NoPending;
    }

    // Digest the first pending session (one per SessionStart to keep latency low)
    let session_id = pending[0].clone();
    digest_one_session(
        project_id,
        &session_id,
        cwd,
        lock_timeout_ms,
        digest_failed_cmds,
        &mut state,
    )
}

/// Build a context hint from active tasks and latest commit for inferred decisions.
pub(super) fn build_context_hint(stats: &SessionStats) -> String {
    let task_part = stats
        .tasks_snapshot
        .iter()
        .find(|t| t.status == "in_progress")
        .or_else(|| stats.tasks_snapshot.first())
        .map(|t| t.subject.as_str());
    let commit_part = stats.commits_made.last().map(|s| s.as_str());

    match (task_part, commit_part) {
        (Some(task), Some(commit)) => {
            let hint = format!("used in: {task} ({commit})");
            if hint.len() > 80 {
                format!("{}...", &hint[..hint.floor_char_boundary(77)])
            } else {
                hint
            }
        }
        (Some(task), None) => {
            let hint = format!("used in: {task}");
            if hint.len() > 80 {
                format!("{}...", &hint[..hint.floor_char_boundary(77)])
            } else {
                hint
            }
        }
        (None, Some(commit)) => {
            let hint = format!("used in: {commit}");
            if hint.len() > 80 {
                format!("{}...", &hint[..hint.floor_char_boundary(77)])
            } else {
                hint
            }
        }
        (None, None) => "(auto-inferred)".to_string(),
    }
}

/// At digest time, write inferred decision events for dependency adds not matched
/// by agent-recorded decisions. Returns the IDs of written events.
pub(super) fn harvest_inferred_decisions(
    session_id: &str,
    stats: &SessionStats,
    decisions_recorded: &[String],
    ledger: &edda_ledger::Ledger,
    branch: &str,
    parent_hash: Option<&str>,
) -> Vec<String> {
    if stats.deps_added.is_empty() {
        return Vec::new();
    }

    let reason = build_context_hint(stats);
    let mut written_ids = Vec::new();
    let mut chain_hash = parent_hash.map(|s| s.to_string());

    for pkg in &stats.deps_added {
        // Skip if agent already recorded a decision containing this package name
        let pkg_lower = pkg.to_lowercase();
        let already_recorded = decisions_recorded
            .iter()
            .any(|d| d.to_lowercase().contains(&pkg_lower));
        if already_recorded {
            continue;
        }

        let dp = edda_core::types::DecisionPayload {
            key: format!("dep.{pkg}"),
            value: pkg.to_string(),
            reason: Some(reason.clone()),
            scope: None,
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: None,
        };
        let mut event =
            edda_core::event::new_decision_event(branch, chain_hash.as_deref(), "system", &dp)
                .expect("decision event creation should not fail");

        // Add harvest-specific metadata
        event.payload["source"] = serde_json::json!("bridge:passive_harvest");
        event.payload["session_id"] = serde_json::json!(session_id);
        if let Some(tags) = event.payload.get_mut("tags").and_then(|v| v.as_array_mut()) {
            tags.push(serde_json::json!("inferred"));
        }

        // Add provenance link to session
        event.refs.provenance.push(Provenance {
            target: format!("session:{session_id}"),
            rel: "inferred_from".to_string(),
            note: Some(format!(
                "passive harvest from session {}",
                &session_id[..session_id.len().min(8)]
            )),
        });

        if let Err(e) = finalize_event(&mut event) {
            tracing::warn!(event_id = %event.event_id, error = %e, "finalize failed for inferred decision, stopping harvest");
            break;
        }
        let event_id = event.event_id.clone();

        if ledger.append_event(&event).is_ok() {
            chain_hash = Some(event.hash.clone());
            written_ids.push(event_id);
        } else {
            break;
        }
    }

    written_ids
}

fn digest_one_session(
    project_id: &str,
    session_id: &str,
    cwd: &str,
    lock_timeout_ms: u64,
    digest_failed_cmds: bool,
    state: &mut DigestState,
) -> DigestResult {
    // Build session ledger path
    let session_ledger_path = edda_store::project_dir(project_id)
        .join("ledger")
        .join(format!("{session_id}.jsonl"));

    if !session_ledger_path.exists() {
        return DigestResult::NoPending;
    }

    // Find workspace root from cwd
    let cwd_path = Path::new(cwd);
    let root = match edda_ledger::EddaPaths::find_root(cwd_path) {
        Some(r) => r,
        None => {
            record_failure(project_id, session_id, state, "no .edda workspace found");
            return DigestResult::Error("no .edda workspace found".to_string());
        }
    };

    let ledger = match edda_ledger::Ledger::open(&root) {
        Ok(l) => l,
        Err(e) => {
            record_failure(
                project_id,
                session_id,
                state,
                &format!("cannot open ledger: {e}"),
            );
            return DigestResult::Error(format!("cannot open ledger: {e}"));
        }
    };

    // Try to acquire lock with timeout
    let _lock = match try_lock_with_timeout(&ledger.paths, lock_timeout_ms) {
        Some(lock) => lock,
        None => {
            record_failure(project_id, session_id, state, "workspace lock timeout");
            return DigestResult::LockTimeout;
        }
    };

    // Reload the state under the workspace lock: a concurrent digest (auto on
    // one session, manual on another) may have advanced watermarks after our
    // pre-lock snapshot, and stale saves would drop each other's entries
    // (round-1 P1-3). Loading here keeps the read-modify-write serialized.
    *state = load_digest_state(project_id);

    // Read branch and last hash
    let branch = ledger.head_branch().unwrap_or_else(|_| "main".to_string());
    let parent_hash = match ledger.last_event_hash() {
        Ok(h) => h,
        Err(e) => {
            record_failure(
                project_id,
                session_id,
                state,
                &format!("cannot read last hash: {e}"),
            );
            return DigestResult::Error(format!("cannot read last hash: {e}"));
        }
    };

    let prev = state.sessions.get(session_id).cloned();
    let start = prev.as_ref().map_or(0, |e| e.offset);

    // Extract the delta: everything after the last digested offset, up to
    // the last complete line. A truncated or concurrently-written final line
    // is not consumed (round-1 P0-2) — it is picked up once its write
    // completes, and the source is never destroyed either way.
    let (mut stats, consumed) = match extract_stats_from(&session_ledger_path, start) {
        Ok(r) => r,
        Err(e) => {
            record_failure(
                project_id,
                session_id,
                state,
                &format!("extraction failed: {e}"),
            );
            return DigestResult::Error(format!("extraction failed: {e}"));
        }
    };

    if consumed <= start {
        // No new complete lines since the last digest.
        return DigestResult::NoPending;
    }

    // Skip deltas with no tool calls and no failures: nothing to summarize
    // (GH-578). A chat-only or idle delta produces a counts-only digest with
    // no information value; it must not cost a ledger event. Failure-only
    // deltas are kept: their failed commands carry information. The consumed
    // watermark is still recorded so the same delta is never rescanned.
    if stats.tool_calls == 0 && stats.tool_failures == 0 {
        let prev_event_id = prev.map_or(String::new(), |e| e.event_id);
        state.session_id = session_id.to_string();
        remember_digested(state, session_id, &prev_event_id, consumed);
        state.digested_at = now_rfc3339();
        state.retry_count = 0;
        state.pending_session_id = String::new();
        state.last_error = String::new();
        if let Err(e) = save_digest_state(project_id, state) {
            tracing::warn!(error = %e, session = %session_id,
                "zero-call digest watermark save failed; the delta will be rescanned");
        }
        return DigestResult::NoPending;
    }

    // Enrich with tasks snapshot from state file
    stats.tasks_snapshot = load_tasks_for_digest(project_id);

    // Enrich with usage data (model, tokens, cost) from transcript signals
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

    // Collect session notes and decisions from workspace ledger
    let (decisions, notes) = collect_session_ledger_extras(cwd, stats.first_ts.as_deref());

    // Build and append session digest note
    let event =
        match build_digest_event(session_id, &stats, &branch, parent_hash.as_deref(), &notes) {
            Ok(e) => e,
            Err(e) => {
                record_failure(
                    project_id,
                    session_id,
                    state,
                    &format!("build event failed: {e}"),
                );
                return DigestResult::Error(format!("build event failed: {e}"));
            }
        };

    if let Err(e) = ledger.append_event(&event) {
        record_failure(
            project_id,
            session_id,
            state,
            &format!("append failed: {e}"),
        );
        return DigestResult::Error(format!("append failed: {e}"));
    }

    let mut last_event_id = event.event_id.clone();
    let mut last_hash = event.hash.clone();

    // ── Durable idempotency boundary (round-1 P1-5) ──
    // The digest note is now in the workspace ledger. Remember it BEFORE
    // any later step (cmd milestones, passive harvest) can fail; otherwise a
    // failed milestone append would return without remembering an
    // already-written digest and the next call would duplicate the note.
    state.session_id = session_id.to_string();
    state.event_id = last_event_id.clone();
    state.digested_at = now_rfc3339();
    remember_digested(state, session_id, &last_event_id, consumed);
    state.retry_count = 0;
    state.pending_session_id = String::new();
    state.last_error = String::new();
    if let Err(e) = save_digest_state(project_id, state) {
        return DigestResult::Error(format!(
            "digest note {} was appended, but saving the digest state failed: {e}. \
             Re-running may duplicate the note.",
            event.event_id
        ));
    }

    // Append cmd milestone events for failed commands (if enabled)
    if digest_failed_cmds && !stats.failed_cmds_detail.is_empty() {
        for failed_cmd in &stats.failed_cmds_detail {
            let cmd_event = match build_cmd_milestone_event(
                session_id,
                failed_cmd,
                &branch,
                Some(&last_hash),
            ) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if ledger.append_event(&cmd_event).is_err() {
                break;
            }
            last_hash = cmd_event.hash.clone();
            last_event_id = cmd_event.event_id.clone();
        }
    }

    // Passive harvest: write inferred decisions for unmatched dependency adds
    let harvest_ids = harvest_inferred_decisions(
        session_id,
        &stats,
        &decisions,
        &ledger,
        &branch,
        Some(&last_hash),
    );
    if let Some(last_harvest_id) = harvest_ids.last() {
        last_event_id = last_harvest_id.clone();
    }

    // Refresh the entry with the final event id. Best-effort: the watermark
    // is already durable above, only the reported id changes.
    if last_event_id != event.event_id {
        state.event_id = last_event_id.clone();
        if let Some(entry) = state.sessions.get_mut(session_id) {
            entry.event_id = last_event_id.clone();
        }
        if let Err(e) = save_digest_state(project_id, state) {
            tracing::warn!(error = %e, session = %session_id,
                "final digest-state refresh failed; the digest watermark is durable");
        }
    }

    DigestResult::Written {
        event_id: last_event_id,
    }
}

/// Manually digest a specific session (CLI escape hatch, and the path the
/// openclaw/codex/cursor/hermes bridges hit on agent_end/session_end).
///
/// Idempotent by per-session watermark (GH-578 round-1, ruling
/// `digest.idempotency=per-session-watermark-never-delete-the-source`):
///
/// * a session with no new complete lines is a no-op and returns THAT
///   session's own digest event id (empty if it never produced one);
/// * a session that grew digests only its delta;
/// * a zero-call delta writes no event but still advances the watermark;
/// * the source ledger is never deleted, so a live producer's future
///   appends are always digestable.
pub fn digest_session_manual(
    project_id: &str,
    session_id: &str,
    cwd: &str,
    digest_failed_cmds: bool,
) -> anyhow::Result<String> {
    let session_ledger_path = edda_store::project_dir(project_id)
        .join("ledger")
        .join(format!("{session_id}.jsonl"));

    if !session_ledger_path.exists() {
        anyhow::bail!(
            "session ledger not found: {}",
            session_ledger_path.display()
        );
    }

    let cwd_path = Path::new(cwd);
    let root = edda_ledger::EddaPaths::find_root(cwd_path)
        .ok_or_else(|| anyhow::anyhow!("no .edda workspace found"))?;
    let ledger = edda_ledger::Ledger::open(&root)?;
    let _lock = edda_ledger::WorkspaceLock::acquire(&ledger.paths)?;

    // Load the state after acquiring the lock so the read-modify-write of
    // the watermark map is serialized against concurrent digests
    // (round-1 P1-3); loading before the lock let concurrent sessions save
    // stale maps and drop each other's entries.
    let mut state = load_digest_state(project_id);
    let prev = state.sessions.get(session_id).cloned();
    let start = prev.as_ref().map_or(0, |e| e.offset);

    // Delta extraction: only complete lines after the last digested offset
    // are consumed (round-1 P0-2); the source is never modified.
    let (mut stats, consumed) = extract_stats_from(&session_ledger_path, start)?;

    if consumed <= start {
        // Nothing new since the last digest: no-op. Return this session's
        // own event id, not the globally-latest one (round-1 P1-4).
        return Ok(prev.map_or(String::new(), |e| e.event_id));
    }

    // Zero-call delta (GH-578): no event — there is nothing to summarize —
    // but the consumed watermark is recorded so the same lines are never
    // rescanned; real work appended later still digests as its own delta.
    if stats.tool_calls == 0 && stats.tool_failures == 0 {
        let prev_event_id = prev.map_or(String::new(), |e| e.event_id);
        state.session_id = session_id.to_string();
        remember_digested(&mut state, session_id, &prev_event_id, consumed);
        state.digested_at = now_rfc3339();
        state.retry_count = 0;
        state.pending_session_id = String::new();
        state.last_error = String::new();
        save_digest_state(project_id, &state)?;
        return Ok(prev_event_id);
    }

    let branch = ledger.head_branch().unwrap_or_else(|_| "main".to_string());
    let parent_hash = ledger.last_event_hash()?;

    stats.tasks_snapshot = load_tasks_for_digest(project_id);
    let (_decisions, notes) = collect_session_ledger_extras(cwd, stats.first_ts.as_deref());
    let event = build_digest_event(session_id, &stats, &branch, parent_hash.as_deref(), &notes)?;
    ledger.append_event(&event)?;

    // ── Durable idempotency boundary (round-1 P1-5) ──
    // The digest note is now in the workspace ledger: remember it BEFORE the
    // cmd milestones (whose appends may fail), or a later failure would
    // return without remembering an already-written digest and the next
    // call would duplicate the note. State-save errors are propagated
    // instead of discarded: reporting success without a durable watermark
    // is what let the old code claim success without the idempotency marker
    // required by GH-578.
    state.session_id = session_id.to_string();
    state.event_id = event.event_id.clone();
    state.digested_at = now_rfc3339();
    remember_digested(&mut state, session_id, &event.event_id, consumed);
    state.retry_count = 0;
    state.pending_session_id = String::new();
    state.last_error = String::new();
    save_digest_state(project_id, &state).map_err(|e| {
        anyhow::anyhow!(
            "digest note {} was appended, but saving the digest state failed: {e}. \
             Re-running may duplicate the note.",
            event.event_id
        )
    })?;

    let mut last_event_id = event.event_id.clone();

    if digest_failed_cmds && !stats.failed_cmds_detail.is_empty() {
        let mut chain_hash = Some(event.hash.clone());
        for failed_cmd in &stats.failed_cmds_detail {
            let cmd_event =
                build_cmd_milestone_event(session_id, failed_cmd, &branch, chain_hash.as_deref())?;
            ledger.append_event(&cmd_event)?;
            chain_hash = Some(cmd_event.hash.clone());
            last_event_id = cmd_event.event_id.clone();
        }
        // Refresh the entry with the final (milestone) event id. Best-effort:
        // the watermark is already durable above, only the reported id changes.
        state.event_id = last_event_id.clone();
        if let Some(entry) = state.sessions.get_mut(session_id) {
            entry.event_id = last_event_id.clone();
        }
        if let Err(e) = save_digest_state(project_id, &state) {
            tracing::warn!(error = %e, session = %session_id,
                "final digest-state refresh failed; the digest watermark is durable");
        }
    }

    Ok(last_event_id)
}

/// Find all undigested sessions in the store for the given project.
///
/// A session counts as pending if it was never digested, or if new content
/// appeared after its watermark (CLI `--all` is the top-up path for a
/// long-lived session whose delta was digested by the bridges).
pub fn find_all_pending_sessions(project_id: &str) -> Vec<String> {
    let state = load_digest_state(project_id);
    let ledger_dir = edda_store::project_dir(project_id).join("ledger");
    let entries = match std::fs::read_dir(&ledger_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".jsonl") {
            continue;
        }
        let session_id = name.trim_end_matches(".jsonl").to_string();
        if let Some(watermark) = state.sessions.get(&session_id) {
            // Already digested; pending only if the file grew past it.
            if entry
                .metadata()
                .map(|m| m.len() <= watermark.offset)
                .unwrap_or(true)
            {
                continue;
            }
        }
        sessions.push(session_id);
    }
    sessions.sort();
    sessions
}

fn record_failure(project_id: &str, session_id: &str, state: &mut DigestState, error: &str) {
    if state.pending_session_id == session_id {
        state.retry_count += 1;
    } else {
        state.pending_session_id = session_id.to_string();
        state.retry_count = 1;
    }
    state.last_error = error.to_string();
    let _ = save_digest_state(project_id, state);
}

/// Build a warning message if there are pending digest failures.
/// Returns None if everything is fine.
pub fn pending_failure_warning(project_id: &str) -> Option<String> {
    let state = load_digest_state(project_id);
    if state.pending_session_id.is_empty() || state.retry_count < 3 {
        return None;
    }
    Some(format!(
        "⚠ edda: session {} digest failed {} times ({}). Run: edda bridge digest --session {}",
        &state.pending_session_id[..state.pending_session_id.len().min(8)],
        state.retry_count,
        state.last_error,
        state.pending_session_id,
    ))
}
