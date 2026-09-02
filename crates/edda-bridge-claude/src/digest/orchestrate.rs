use std::collections::BTreeMap;
use std::path::Path;

use edda_core::event::finalize_event;
use edda_core::types::Provenance;
use serde::{Deserialize, Serialize};

use super::extract::{extract_stats_delta, load_tasks_for_digest, watermark_matches};
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
    /// Hash of the first `offset` bytes of the session ledger — the file
    /// identity proof (round-2 finding 1: a bare offset silently assumes
    /// the file is append-only and never replaced; on proof mismatch the
    /// session is re-read from zero, never skipped).
    #[serde(default)]
    pub prefix_hash: String,
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

/// Migrate pre-watermark state into the per-session model.
///
/// The legacy model records only session ids and event ids — it cannot
/// prove what bytes were actually consumed at the moment the legacy state
/// was written (round-2 finding 2: seeding the offset at the ledger's
/// current EOF silently swallowed everything appended between the legacy
/// write and the first post-upgrade load). Migration therefore seeds a
/// zero offset with no identity proof: the first post-upgrade digest
/// RE-READS the whole ledger — at-least-once, it may duplicate the legacy
/// digest once — instead of skipping anything. The recorded event id is
/// kept only so a no-op (empty or unchanged-complete ledger) still returns
/// that session's own id.
pub fn migrate_legacy_state(_project_id: &str, state: &mut DigestState) {
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
    for id in legacy_ids {
        // Only the single legacy `session_id` slot has a known event id.
        let (event_id, digested_at) = if id == state.session_id {
            (state.event_id.clone(), state.digested_at.clone())
        } else {
            (String::new(), String::new())
        };
        state.sessions.insert(
            id,
            DigestedSession {
                offset: 0,
                prefix_hash: String::new(),
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

/// Record how far a session has been digested, which event id it got, and
/// the identity proof of the consumed prefix. The entry is an unverified
/// hint only: it can never suppress a digest on its own — the workspace
/// ledger is the sole authority (round-4 ruling
/// `digest.zero-call-sessions=re-read-every-time-no-cache-authority`).
fn remember_digested(
    state: &mut DigestState,
    session_id: &str,
    event_id: &str,
    offset: u64,
    prefix_hash: &str,
) {
    state.sessions.insert(
        session_id.to_string(),
        DigestedSession {
            offset,
            prefix_hash: prefix_hash.to_string(),
            event_id: event_id.to_string(),
            digested_at: now_rfc3339(),
        },
    );
}

/// A watermark candidate proven to describe the CURRENT ledger file.
#[derive(Debug, Clone)]
struct WatermarkCandidate {
    offset: u64,
    prefix_hash: String,
    event_id: String,
}

/// Recover the effective digest watermark for a session from the workspace
/// ledger — the SOLE authority (round-3 P1-2, ruling
/// `digest.proof-and-authority=derive-from-one-read-ledger-is-sole-authority`).
///
/// A session counts as digested exactly as far as the highest digest note
/// for it in the ledger whose stamped `digest_watermark` still validates
/// against the current file. The side state file is an unverified hint and
/// is deliberately NOT a source here: a cache entry cannot prove its note
/// still exists in the ledger (the ledger can be rolled back under a
/// surviving cache), so letting it set the start offset — at any offset,
/// EOF included — can silently skip content the ledger never recorded as
/// digested. Because cache state alone can never rule out a relevant note,
/// there is NO fast path that skips this scan: the scan IS the authority
/// (round-4 ruling
/// `digest.zero-call-sessions=re-read-every-time-no-cache-authority`).
/// The returned start offset is always ledger-derived, so a delta whose
/// note was rolled back is re-read.
fn effective_watermark(
    ledger: &edda_ledger::Ledger,
    session_id: &str,
    session_ledger_path: &Path,
) -> Option<WatermarkCandidate> {
    stamped_watermark_index(ledger)
        .remove(session_id)
        .unwrap_or_default()
        .into_iter()
        .find(|(offset, hash, _)| watermark_matches(session_ledger_path, *offset, hash))
        .map(|(offset, prefix_hash, event_id)| WatermarkCandidate {
            offset,
            prefix_hash,
            event_id,
        })
}

/// Stamped digest watermarks per session across the whole ledger, highest
/// offset first — the authoritative idempotency record (round-3 P1-2).
/// Validation against the current file happens at the caller.
fn stamped_watermark_index(
    ledger: &edda_ledger::Ledger,
) -> BTreeMap<String, Vec<(u64, String, String)>> {
    let mut index: BTreeMap<String, Vec<(u64, String, String)>> = BTreeMap::new();
    if let Ok(notes) = ledger.iter_events_by_type("note") {
        for event in notes {
            let payload = &event.payload;
            if payload.get("source").and_then(|v| v.as_str()) != Some("bridge:session_digest") {
                continue;
            }
            let Some(session_id) = payload.get("session_id").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(wm) = payload.get("digest_watermark") else {
                continue;
            };
            let (Some(offset), Some(hash)) = (
                wm.get("offset").and_then(|v| v.as_u64()),
                wm.get("prefix_hash").and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            index.entry(session_id.to_string()).or_default().push((
                offset,
                hash.to_string(),
                event.event_id.clone(),
            ));
        }
    }
    for candidates in index.values_mut() {
        candidates.sort_by_key(|(offset, _, _)| std::cmp::Reverse(*offset));
    }
    index
}

/// True if the session is PROVABLY fully consumed: the workspace ledger
/// records a stamped digest note whose offset covers the whole current
/// file and whose identity proof re-validates (round-2: position alone is
/// not identity).
///
/// The ledger is the ONLY authority (round-4 ruling
/// `digest.zero-call-sessions=re-read-every-time-no-cache-authority`):
/// the cache is never consulted here, so it can shorten nothing the
/// ledger has not already confirmed. A zero-call session has no note by
/// design (GH-578), so it is re-read on every call; a note-backed cache
/// entry can NEVER claim its note — the round-3 reproduction (note-backed
/// cache at EOF + rolled-back ledger) suppressed a live Edit prefix
/// precisely because the cache was verified against the session file but
/// never against the ledger.
fn is_fully_consumed(path: &Path, stamped: &[(u64, String, String)]) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let len = meta.len();
    stamped
        .iter()
        .any(|(offset, hash, _)| *offset == len && watermark_matches(path, *offset, hash))
}

/// Find session ledger files in the store, excluding the current session.
fn find_pending_sessions(
    project_id: &str,
    current_session_id: &str,
    ledger: Option<&edda_ledger::Ledger>,
) -> Vec<String> {
    let ledger_dir = edda_store::project_dir(project_id).join("ledger");
    let entries = match std::fs::read_dir(&ledger_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut sessions = Vec::new();
    // The authoritative stamped watermarks (round-3 P1-2). When no ledger
    // is available nothing is confirmable, so every session is reported
    // pending — over-listing costs a scan, never a skipped session.
    let stamped_index = ledger.map(stamped_watermark_index);
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
        // Skip only sessions whose consumption is PROVEN by a stamped note
        // in the workspace ledger — the sole authority (round-2: position
        // alone is not identity — a replaced or rewritten ledger re-opens
        // the digest from zero, and a grown one is picked up as a delta;
        // round-4: the cache, zero-call or not, can never suppress).
        let path = ledger_dir.join(&name);
        let stamped: &[(u64, String, String)] = stamped_index
            .as_ref()
            .and_then(|m| m.get(&session_id))
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        if is_fully_consumed(&path, stamped) {
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

    // The pending scan is ledger-authoritative when the workspace ledger
    // can be opened (round-3 P1-2): a session may only be skipped on a
    // stamped ledger note or a provably content-free zero-call watermark.
    // When no ledger is reachable nothing is note-backed-confirmable, so
    // nothing note-backed may suppress — pending sessions fall through to
    // digest_one_session, which reports the unreachable workspace honestly.
    let ledger = edda_ledger::EddaPaths::find_root(Path::new(cwd))
        .and_then(|root| edda_ledger::Ledger::open(&root).ok());

    // Find sessions to digest
    let pending = find_pending_sessions(project_id, current_session_id, ledger.as_ref());
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
    // Effective watermark: the session's own stamped digest notes in the
    // LEDGER, highest validated offset wins (round-3 P1-2: the ledger is
    // the sole authority — the cache is an unverified hint and cannot set
    // the start offset).
    let eff = effective_watermark(&ledger, session_id, &session_ledger_path);
    let start = eff.as_ref().map_or(0, |w| w.offset);

    // Extract the delta: everything after the last digested offset, up to
    // the last complete line, plus the identity proof of the consumed
    // prefix derived from the SAME single read (round-3 P1-1: the note's
    // content and its proof cannot come from different reads). A truncated
    // or concurrently-written final line is not consumed (round-1 P0-2) —
    // it is picked up once its write completes, and the source is never
    // destroyed either way.
    let delta = match extract_stats_delta(&session_ledger_path, start) {
        Ok(d) => d,
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
    let mut stats = delta.stats;
    let consumed = delta.consumed;
    let proof = delta.prefix_hash;

    if consumed <= start {
        // No new complete lines since the last digest. Repair the cache
        // from the durable sources if it lags (round-2 finding 3: after a
        // crash between note-append and cache-save, the ledger note is the
        // watermark and the retry must be a no-op, not a duplicate).
        if let Some(w) = &eff {
            // The cache is current only if it exactly mirrors the ledger
            // note; anything else — including a stale zero-call-advanced
            // entry from an older state file — is repaired to the note
            // (round-4: the cache is never an authority on its own).
            let cache_current = prev.as_ref().is_some_and(|c| {
                c.offset == w.offset && c.prefix_hash == w.prefix_hash && c.event_id == w.event_id
            });
            if !cache_current {
                state.session_id = session_id.to_string();
                state.digested_at = now_rfc3339();
                remember_digested(state, session_id, &w.event_id, w.offset, &w.prefix_hash);
                if let Err(e) = save_digest_state(project_id, state) {
                    tracing::warn!(error = %e, session = %session_id,
                        "watermark cache repair failed; the ledger note remains authoritative");
                }
            }
        }
        return DigestResult::NoPending;
    }

    // Skip deltas with no tool calls and no failures: nothing to summarize
    // (GH-578). A chat-only or idle delta produces a counts-only digest with
    // no information value; it must not cost a ledger event. Failure-only
    // deltas are kept: their failed commands carry information. The
    // consumed watermark is recorded as a hint, but it can never suppress a
    // rescan: with no note in the ledger the session stays pending and is
    // re-read on every call (round-4 ruling
    // `digest.zero-call-sessions=re-read-every-time-no-cache-authority`).
    if stats.tool_calls == 0 && stats.tool_failures == 0 {
        let prev_event_id = eff.as_ref().map_or(String::new(), |w| w.event_id.clone());
        // The proof comes from the same single read as the stats (round-3
        // P1-1) — no separate hash of the path.
        state.session_id = session_id.to_string();
        remember_digested(state, session_id, &prev_event_id, consumed, &proof);
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
        stats.estimated_cost_usd = super::helpers::measured_cost(&usage);
    }

    // Collect session notes and decisions from workspace ledger
    let (decisions, notes) = collect_session_ledger_extras(cwd, stats.first_ts.as_deref());

    // Identity proof of the consumed prefix: stamped into the note so the
    // ledger itself is the durable idempotency record (round-2), derived
    // from the same single read as the stats (round-3 P1-1).
    let watermark = super::DigestWatermark {
        offset: consumed,
        prefix_hash: proof.clone(),
    };

    // Build and append session digest note
    let event = match build_digest_event(
        session_id,
        &stats,
        &branch,
        parent_hash.as_deref(),
        &notes,
        Some(&watermark),
    ) {
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
    remember_digested(state, session_id, &last_event_id, consumed, &proof);
    state.retry_count = 0;
    state.pending_session_id = String::new();
    state.last_error = String::new();
    if let Err(e) = save_digest_state(project_id, state) {
        // Honest, but no longer duplicate-prone: the note above carries its
        // watermark, so the next run recovers from the ledger instead of
        // re-appending (round-2 finding 3).
        return DigestResult::Error(format!(
            "digest note {} was appended, but saving the digest state failed: {e}. \
             The note remains authoritative in the ledger; re-running will recover from it.",
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
/// Idempotent by per-session watermark with file-identity proof (GH-578;
/// round-2 ruling `digest.watermark-identity=offset-needs-content-proof
/// -and-ledger-is-the-truth`):
///
/// * the effective watermark is recovered from every durable source — the
///   cache entry and the session's own stamped digest notes in the ledger —
///   and each candidate must re-prove file identity (hash of the consumed
///   prefix); on any mismatch the session is re-read from zero, never
///   skipped;
/// * a session with no new complete lines is a no-op and returns THAT
///   session's own digest event id (empty if it never produced one);
/// * a session that grew digests only its delta;
/// * a zero-call delta writes no event but still advances the watermark;
/// * the digest note itself carries its watermark, so losing the cache
///   costs a re-scan and never a duplicate;
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
    // Effective watermark: the session's own stamped digest notes in the
    // LEDGER, highest validated offset wins (round-3 P1-2: the ledger is
    // the sole authority — the cache is an unverified hint and cannot set
    // the start offset).
    let eff = effective_watermark(&ledger, session_id, &session_ledger_path);
    let start = eff.as_ref().map_or(0, |w| w.offset);

    // Delta extraction plus the identity proof of the consumed prefix,
    // derived from the SAME single read (round-3 P1-1): only complete
    // lines after the last digested offset are consumed (round-1 P0-2);
    // the source is never modified.
    let delta = extract_stats_delta(&session_ledger_path, start)?;
    let mut stats = delta.stats;
    let consumed = delta.consumed;
    let proof = delta.prefix_hash;

    if consumed <= start {
        // Nothing new since the last digest: no-op. Repair the cache if it
        // lags the durable sources (round-2 finding 3: after a crash
        // between note-append and cache-save the ledger note IS the
        // watermark and the retry must be a no-op, not a duplicate), then
        // return this session's own event id (round-1 P1-4).
        if let Some(w) = &eff {
            // The cache is current only if it exactly mirrors the ledger
            // note; anything else — including a stale zero-call-advanced
            // entry from an older state file — is repaired to the note
            // (round-4: the cache is never an authority on its own).
            let cache_current = prev.as_ref().is_some_and(|c| {
                c.offset == w.offset && c.prefix_hash == w.prefix_hash && c.event_id == w.event_id
            });
            if !cache_current {
                state.session_id = session_id.to_string();
                state.digested_at = now_rfc3339();
                remember_digested(
                    &mut state,
                    session_id,
                    &w.event_id,
                    w.offset,
                    &w.prefix_hash,
                );
                state.retry_count = 0;
                state.pending_session_id = String::new();
                state.last_error = String::new();
                if let Err(e) = save_digest_state(project_id, &state) {
                    tracing::warn!(error = %e, session = %session_id,
                        "watermark cache repair failed; the ledger note remains authoritative");
                }
            }
        }
        return Ok(eff.map_or(String::new(), |w| w.event_id));
    }

    // Zero-call delta (GH-578): no event — there is nothing to summarize.
    // The consumed watermark (with identity proof) is recorded as a hint,
    // but it can never suppress a rescan: with no note in the ledger the
    // session stays pending and these lines are re-read on every call
    // (round-4 ruling
    // `digest.zero-call-sessions=re-read-every-time-no-cache-authority`).
    if stats.tool_calls == 0 && stats.tool_failures == 0 {
        let prev_event_id = eff.as_ref().map_or(String::new(), |w| w.event_id.clone());
        // The proof comes from the same single read as the stats (round-3
        // P1-1) — no separate hash of the path.
        state.session_id = session_id.to_string();
        remember_digested(&mut state, session_id, &prev_event_id, consumed, &proof);
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

    // Identity proof of the consumed prefix, stamped into the note so the
    // ledger itself is the durable idempotency record (round-2), derived
    // from the same single read as the stats (round-3 P1-1).
    let watermark = super::DigestWatermark {
        offset: consumed,
        prefix_hash: proof.clone(),
    };
    let event = build_digest_event(
        session_id,
        &stats,
        &branch,
        parent_hash.as_deref(),
        &notes,
        Some(&watermark),
    )?;
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
    remember_digested(&mut state, session_id, &event.event_id, consumed, &proof);
    state.retry_count = 0;
    state.pending_session_id = String::new();
    state.last_error = String::new();
    save_digest_state(project_id, &state).map_err(|e| {
        // Honest, but no longer duplicate-prone: the note above carries its
        // watermark, so the next run recovers from the ledger instead of
        // re-appending (round-2 finding 3).
        anyhow::anyhow!(
            "digest note {} was appended, but saving the digest state failed: {e}. \
             The note remains authoritative in the ledger; re-running will recover from it.",
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
///
/// The CLI listing has no workspace-ledger handle, so nothing is
/// note-backed-confirmable here (round-3 P1-2) and NOTHING may suppress
/// (round-4: the cache is never an authority): every session is listed.
/// Note-backed sessions are then no-oped by the ledger-authoritative
/// `digest_session_manual`; a zero-call session is re-read on every call —
/// over-listing costs a scan, never a skipped session.
pub fn find_all_pending_sessions(project_id: &str) -> Vec<String> {
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
        // Nothing may suppress here: no ledger handle, and the cache is
        // never an authority (round-4 ruling) — every session is listed
        // and resolved by the ledger-authoritative manual path.
        let path = ledger_dir.join(&name);
        if is_fully_consumed(&path, &[]) {
            continue;
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
