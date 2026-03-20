//! PreToolUse decision file warning (Track E).
//!
//! Checks whether a file being edited is governed by any active decision.
//! If so, returns a formatted markdown warning string for injection into
//! `additionalContext`.
//!
//! # Boundary Rules
//! - BOUNDARY-01: No import of `DecisionRow` — only `DecisionView`
//! - BOUNDARY-02: Reads through `to_view()`, never parses `affected_paths` JSON
//! - PERF-01: Total time < 100ms — uses `DECISION_CACHE`

use edda_ledger::view::DecisionView;
use std::path::Path;
use std::sync::LazyLock;
use std::sync::Mutex;

// ── Session-scoped cache ────────────────────────────────────────────

struct DecisionCache {
    /// Cache key: (repo_root_str, branch) to detect invalidation
    key: Option<(String, String)>,
    /// Cached decisions with non-empty affected_paths
    decisions: Vec<DecisionView>,
    /// Timestamp of last load, for TTL-based expiration
    loaded_at: std::time::Instant,
}

static DECISION_CACHE: LazyLock<Mutex<DecisionCache>> = LazyLock::new(|| {
    Mutex::new(DecisionCache {
        key: None,
        decisions: Vec::new(),
        loaded_at: std::time::Instant::now(),
    })
});

const CACHE_TTL_SECS: u64 = 120;

// ── Public API ──────────────────────────────────────────────────────

/// Check if `file_path` is governed by any active decision.
///
/// Returns a formatted markdown warning listing matching decisions,
/// or `None` if no decisions match.
///
/// Performance: must complete in < 100ms (CONTRACT PERF-01).
/// Uses `DECISION_CACHE` to avoid re-querying within the same session.
///
/// # Arguments
/// - `repo_root`: path to the repository root (passed to `Ledger::open`)
/// - `file_path`: the file being edited (concrete path, e.g. `crates/edda-ledger/src/lib.rs`)
/// - `branch`: current git branch name
pub(crate) fn decision_file_warning(
    repo_root: &Path,
    file_path: &str,
    branch: &str,
) -> Option<String> {
    let decisions = load_decisions_cached(repo_root, branch);
    if decisions.is_empty() {
        return None;
    }

    let matched: Vec<&DecisionView> = decisions
        .iter()
        .filter(|d| !d.affected_paths.is_empty() && matches_any_path(file_path, &d.affected_paths))
        .collect();

    if matched.is_empty() {
        return None;
    }

    Some(format_warning(&matched))
}

// ── Internal helpers ────────────────────────────────────────────────

fn load_decisions_cached(repo_root: &Path, branch: &str) -> Vec<DecisionView> {
    let mut cache = DECISION_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let key = (repo_root.display().to_string(), branch.to_string());

    if let Some(ref cached_key) = cache.key {
        if *cached_key == key && cache.loaded_at.elapsed().as_secs() < CACHE_TTL_SECS {
            return cache.decisions.clone();
        }
    }

    // Query via edda-ledger view API (BOUNDARY-01, BOUNDARY-02)
    let decisions = match edda_ledger::Ledger::open(repo_root) {
        Ok(ledger) => ledger
            .query_active_with_paths(Some(branch), None)
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };

    cache.key = Some(key);
    cache.decisions = decisions.clone();
    cache.loaded_at = std::time::Instant::now();
    decisions
}

fn matches_any_path(file_path: &str, affected_paths: &[String]) -> bool {
    // Normalize path separators (Windows compat)
    let normalized = file_path.replace('\\', "/");

    for pattern in affected_paths {
        if let Ok(glob) = globset::Glob::new(pattern) {
            let matcher = glob.compile_matcher();
            if matcher.is_match(&normalized) {
                return true;
            }
        }
    }
    false
}

fn format_warning(matches: &[&DecisionView]) -> String {
    let mut lines = vec!["**[edda] Active decisions governing this file:**".to_string()];
    for d in matches {
        let reason_suffix = if d.reason.is_empty() {
            String::new()
        } else {
            format!(" — {}", d.reason)
        };
        lines.push(format!(
            "  - `{}={}` [{}]{}",
            d.key, d.value, d.status, reason_suffix
        ));
    }
    lines.join("\n")
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: set up a temp workspace with ledger, returning (tmp_dir, repo_root).
    fn setup_workspace() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let root = tmp.path().to_path_buf();
        let ledger = edda_ledger::Ledger::open_or_init(&root).expect("init workspace");
        // Set HEAD branch to "main"
        ledger.set_head_branch("main").expect("set head");
        (tmp, root)
    }

    /// Helper: insert a decision with affected_paths via raw SQL.
    fn insert_decision_with_paths(
        root: &Path,
        key: &str,
        value: &str,
        reason: &str,
        paths: &[&str],
    ) {
        let ledger = edda_ledger::Ledger::open(root).expect("open ledger");
        let event = edda_core::event::new_note_event(
            "main",
            None,
            "system",
            &format!("{key}: {value}"),
            &[],
        )
        .expect("create event");
        let event_id = event.event_id.clone();
        ledger.append_event(&event).expect("append event");

        // Update the materialized decision row via raw SQL
        let paths_json = serde_json::to_string(paths).unwrap();
        let conn = rusqlite::Connection::open(ledger.paths.ledger_db.clone()).expect("open db");
        conn.execute(
            "INSERT OR REPLACE INTO decisions (event_id, key, value, reason, domain, branch, is_active, status, authority, affected_paths, tags, scope, reversibility) VALUES (?1, ?2, ?3, ?4, ?5, 'main', 1, 'active', 'human', ?6, '[]', 'local', 'medium')",
            rusqlite::params![event_id, key, value, reason, key.split('.').next().unwrap_or(""), paths_json],
        ).expect("insert decision");
    }

    /// Invalidate the static cache between tests.
    fn invalidate_cache() {
        let mut cache = DECISION_CACHE.lock().unwrap();
        cache.key = None;
        cache.decisions.clear();
    }

    #[test]
    fn test_no_decisions_returns_none() {
        invalidate_cache();
        let (_tmp, root) = setup_workspace();
        let result = decision_file_warning(&root, "src/main.rs", "main");
        assert!(result.is_none());
    }

    #[test]
    fn test_no_matching_paths_returns_none() {
        invalidate_cache();
        let (_tmp, root) = setup_workspace();
        insert_decision_with_paths(
            &root,
            "db.engine",
            "sqlite",
            "embedded",
            &["crates/edda-ledger/**"],
        );
        let result = decision_file_warning(&root, "crates/edda-cli/src/main.rs", "main");
        assert!(result.is_none());
    }

    #[test]
    fn test_matching_glob_returns_warning() {
        invalidate_cache();
        let (_tmp, root) = setup_workspace();
        insert_decision_with_paths(
            &root,
            "db.engine",
            "sqlite",
            "embedded",
            &["crates/edda-ledger/**"],
        );
        let result = decision_file_warning(&root, "crates/edda-ledger/src/lib.rs", "main");
        assert!(result.is_some());
        let warning = result.unwrap();
        assert!(warning.contains("Active decisions governing this file"));
        assert!(warning.contains("db.engine=sqlite"));
        assert!(warning.contains("embedded"));
    }

    #[test]
    fn test_multiple_matches() {
        invalidate_cache();
        let (_tmp, root) = setup_workspace();
        insert_decision_with_paths(&root, "db.engine", "sqlite", "embedded", &["crates/**"]);
        insert_decision_with_paths(
            &root,
            "error.pattern",
            "thiserror",
            "typed errors",
            &["crates/**"],
        );
        let result = decision_file_warning(&root, "crates/edda-ledger/src/lib.rs", "main");
        assert!(result.is_some());
        let warning = result.unwrap();
        assert!(warning.contains("db.engine=sqlite"));
        assert!(warning.contains("error.pattern=thiserror"));
    }

    #[test]
    fn test_matches_any_path_normalization() {
        // Backslash paths should match forward-slash globs
        assert!(matches_any_path(
            r"crates\edda-ledger\src\lib.rs",
            &["crates/edda-ledger/**".to_string()]
        ));
    }

    #[test]
    fn test_format_warning_empty_reason() {
        let view = DecisionView {
            event_id: "evt_1".to_string(),
            branch: "main".to_string(),
            ts: None,
            key: "test.key".to_string(),
            value: "val".to_string(),
            reason: String::new(),
            domain: "test".to_string(),
            status: "active".to_string(),
            authority: "human".to_string(),
            reversibility: "medium".to_string(),
            affected_paths: vec!["src/**".to_string()],
            tags: vec![],
            propagation: "local".to_string(),
            supersedes_id: None,
            review_after: None,
        };
        let warning = format_warning(&[&view]);
        assert!(warning.contains("`test.key=val` [active]"));
        // No trailing " — " when reason is empty
        assert!(!warning.contains(" — "));
    }
}
