//! Incremental search reindex at SessionEnd (GH-403).
//!
//! Design: non-blocking, idempotent, cheap.
//!
//! Two deliberate departures from the sibling background tasks:
//!
//! - **No cooldown or interval gate.** `bg_scan`/`bg_detect` are gated because
//!   they spend LLM calls; an incremental sync with nothing new is a cursor read
//!   and a no-op commit. Gating it would only reintroduce the staleness this
//!   exists to remove.
//! - **No cold build.** A missing index is left alone and picked up by
//!   `edda search query`'s build-if-missing. A first build costs ~25s on a real
//!   corpus, and a session's exit must never pay that.

use anyhow::Result;
use edda_store::project_dir;
use std::path::Path;

/// Run only when there is already an index to top up.
pub fn should_run(project_id: &str) -> bool {
    if std::env::var("EDDA_BG_ENABLED").unwrap_or_else(|_| "1".into()) == "0" {
        return false;
    }
    index_exists(&project_dir(project_id))
}

/// Whether a project has an index worth topping up.
///
/// Split out from `should_run` so it can be tested against a temp dir: resolving
/// a project dir means reading the process-wide store root, and a test that
/// redirected it would corrupt every other test sharing this process.
fn index_exists(proj_dir: &Path) -> bool {
    proj_dir.join("search").join("tantivy").exists()
}

/// Bring the index up to date with events written during this session.
pub fn run_index(project_id: &str, cwd: &str) -> Result<()> {
    let ledger = edda_ledger::Ledger::open(std::path::Path::new(cwd))?;
    let proj = project_dir(project_id);
    let stats = edda_search_fts::sync::sync(&proj, project_id, None, |after| {
        ledger.events_after_rowid(after)
    })?;
    tracing::debug!(
        events = stats.events,
        turns = stats.turns,
        "search index synced"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Deliberately tests `index_exists` rather than `should_run`: the latter
    // resolves the store root from the environment, and setting that here would
    // redirect it for every other test in this process (cargo runs them in
    // parallel threads), breaking bg_detect/bg_scan/bg_digest.
    #[test]
    fn index_exists_only_when_the_tantivy_dir_is_present() {
        let tmp = tempfile::tempdir().unwrap();

        // Cold builds belong to `edda search query`, never to a session's exit.
        assert!(!index_exists(tmp.path()));

        std::fs::create_dir_all(tmp.path().join("search").join("tantivy")).unwrap();
        assert!(index_exists(tmp.path()));
    }
}
