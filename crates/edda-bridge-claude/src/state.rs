//! Session-scoped state file management.
//!
//! Shared utilities for managing per-session state files (counters, dedup hashes,
//! nudge cooldown, compact recovery flags). Used by both Claude and OpenClaw bridges.

use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

// ── Recall Rate Counters ──

/// Increment a per-session counter file (e.g. `nudge_count`, `decide_count`).
pub fn increment_counter(project_id: &str, session_id: &str, name: &str) {
    let path = edda_store::project_dir(project_id)
        .join("state")
        .join(format!("{name}.{session_id}"));
    let current: u64 = fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let _ = fs::write(&path, (current + 1).to_string());
}

/// Read a per-session counter file; returns 0 if missing.
pub fn read_counter(project_id: &str, session_id: &str, name: &str) -> u64 {
    let path = edda_store::project_dir(project_id)
        .join("state")
        .join(format!("{name}.{session_id}"));
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

// ── Injection Dedup ──

/// Path to the inject hash state file for a given session.
fn inject_hash_path(project_id: &str, session_id: &str) -> PathBuf {
    edda_store::project_dir(project_id)
        .join("state")
        .join(format!("inject_hash.{session_id}"))
}

/// Compute a 64-bit hash of a string (for dedup comparison).
fn content_hash(s: &str) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

/// Check if the given content matches the last injected content for this session.
pub fn is_same_as_last_inject(project_id: &str, session_id: &str, content: &str) -> bool {
    let path = inject_hash_path(project_id, session_id);
    let current = format!("{:016x}", content_hash(content));
    match fs::read_to_string(&path) {
        Ok(stored) => stored.trim() == current,
        Err(_) => false,
    }
}

/// Write the content hash for the current injection.
pub fn write_inject_hash(project_id: &str, session_id: &str, content: &str) {
    let path = inject_hash_path(project_id, session_id);
    let hash = format!("{:016x}", content_hash(content));
    let _ = fs::write(&path, hash);
}

// ── Nudge Cooldown ──

/// Default cooldown between nudges (seconds).
const NUDGE_COOLDOWN_SECS: i64 = 180; // 3 minutes

/// Read the effective cooldown, allowing `EDDA_NUDGE_COOLDOWN_SECS` env override.
fn nudge_cooldown_secs() -> i64 {
    std::env::var("EDDA_NUDGE_COOLDOWN_SECS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(NUDGE_COOLDOWN_SECS)
}

/// Check if we should send a nudge (cooldown expired).
pub fn should_nudge(project_id: &str, session_id: &str) -> bool {
    let path = edda_store::project_dir(project_id)
        .join("state")
        .join(format!("nudge_ts.{session_id}"));
    match fs::read_to_string(&path) {
        Ok(ts) => {
            let last = time::OffsetDateTime::parse(
                ts.trim(),
                &time::format_description::well_known::Rfc3339,
            )
            .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
            let elapsed = time::OffsetDateTime::now_utc() - last;
            elapsed.whole_seconds() >= nudge_cooldown_secs()
        }
        Err(_) => true, // no previous nudge → allow
    }
}

/// Record that a nudge was sent.
pub fn mark_nudge_sent(project_id: &str, session_id: &str) {
    let path = edda_store::project_dir(project_id)
        .join("state")
        .join(format!("nudge_ts.{session_id}"));
    let _ = fs::write(&path, now_rfc3339());
}

// ── Compact Recovery ──

/// Path to the compact_pending flag file.
fn compact_pending_path(project_id: &str) -> PathBuf {
    edda_store::project_dir(project_id)
        .join("state")
        .join("compact_pending")
}

/// Set the compact_pending flag (called before compaction).
pub fn set_compact_pending(project_id: &str) {
    let path = compact_pending_path(project_id);
    let _ = fs::write(&path, b"1");
}

/// Take (read + clear) the compact_pending flag. Returns true if it was set.
pub fn take_compact_pending(project_id: &str) -> bool {
    let path = compact_pending_path(project_id);
    if path.exists() {
        let _ = fs::remove_file(&path);
        true
    } else {
        false
    }
}

// ── Peer Count Tracking (Late Peer Detection) ──

/// Read the previously recorded peer count for this session (defaults to 0).
pub fn read_peer_count(project_id: &str, session_id: &str) -> usize {
    let path = edda_store::project_dir(project_id)
        .join("state")
        .join(format!("peer_count.{session_id}"));
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Write the current peer count for this session.
pub fn write_peer_count(project_id: &str, session_id: &str, count: usize) {
    let path = edda_store::project_dir(project_id)
        .join("state")
        .join(format!("peer_count.{session_id}"));
    let _ = fs::write(&path, count.to_string());
}

// ── Utility ──

fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_round_trip() {
        let pid = "test_state_counter_rt";
        let sid = "s1";
        let _ = edda_store::ensure_dirs(pid);

        assert_eq!(read_counter(pid, sid, "test_count"), 0);
        increment_counter(pid, sid, "test_count");
        assert_eq!(read_counter(pid, sid, "test_count"), 1);
        increment_counter(pid, sid, "test_count");
        assert_eq!(read_counter(pid, sid, "test_count"), 2);

        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn dedup_hash_round_trip() {
        let pid = "test_state_dedup_rt";
        let sid = "s1";
        let _ = edda_store::ensure_dirs(pid);

        let content = "test dedup content";
        assert!(!is_same_as_last_inject(pid, sid, content));
        write_inject_hash(pid, sid, content);
        assert!(is_same_as_last_inject(pid, sid, content));
        assert!(!is_same_as_last_inject(pid, sid, "different"));

        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn compact_pending_lifecycle() {
        let pid = "test_state_compact_lc";
        let _ = edda_store::ensure_dirs(pid);

        assert!(!take_compact_pending(pid));
        set_compact_pending(pid);
        assert!(take_compact_pending(pid));
        assert!(!take_compact_pending(pid)); // cleared

        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn nudge_cooldown_env_var_override() {
        assert_eq!(nudge_cooldown_secs(), 180);

        std::env::set_var("EDDA_NUDGE_COOLDOWN_SECS", "60");
        assert_eq!(nudge_cooldown_secs(), 60);

        std::env::set_var("EDDA_NUDGE_COOLDOWN_SECS", "not_a_number");
        assert_eq!(nudge_cooldown_secs(), 180);

        std::env::remove_var("EDDA_NUDGE_COOLDOWN_SECS");
    }

    #[test]
    fn peer_count_round_trip() {
        let pid = "test_state_peer_ct";
        let sid = "s1";
        let _ = edda_store::ensure_dirs(pid);

        assert_eq!(read_peer_count(pid, sid), 0);
        write_peer_count(pid, sid, 3);
        assert_eq!(read_peer_count(pid, sid), 3);

        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }
}
