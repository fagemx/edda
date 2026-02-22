use crate::paths::EddaPaths;
use crate::sqlite_store::{DecisionRow, SqliteStore};
use edda_core::Event;
use std::path::Path;

/// The append-only event ledger (SQLite backend).
pub struct Ledger {
    pub paths: EddaPaths,
    sqlite: SqliteStore,
}

impl Ledger {
    /// Open an existing workspace. Fails if `.edda/` does not exist.
    pub fn open(repo_root: impl Into<std::path::PathBuf>) -> anyhow::Result<Self> {
        let paths = EddaPaths::discover(repo_root);
        if !paths.is_initialized() {
            anyhow::bail!(
                "not a edda workspace ({}/.edda not found). Run `edda init` first.",
                paths.root.display()
            );
        }
        let sqlite = SqliteStore::open(&paths.ledger_db)?;
        Ok(Self { paths, sqlite })
    }

    /// Convenience: open from a Path ref (avoids Into<PathBuf> ambiguity).
    pub fn open_path(repo_root: &Path) -> anyhow::Result<Self> {
        Self::open(repo_root.to_path_buf())
    }

    // ── HEAD branch ─────────────────────────────────────────────────

    /// Read the current HEAD branch name.
    pub fn head_branch(&self) -> anyhow::Result<String> {
        self.sqlite.head_branch()
    }

    /// Write the HEAD branch name.
    pub fn set_head_branch(&self, name: &str) -> anyhow::Result<()> {
        self.sqlite.set_head_branch(name)
    }

    // ── Events ──────────────────────────────────────────────────────

    /// Append an event to the ledger. Append-only (CONTRACT LEDGER-02).
    pub fn append_event(&self, event: &Event) -> anyhow::Result<()> {
        self.sqlite.append_event(event)
    }

    /// Get the hash of the last event, or `None` if the ledger is empty.
    pub fn last_event_hash(&self) -> anyhow::Result<Option<String>> {
        self.sqlite.last_event_hash()
    }

    /// Read all events in the ledger.
    pub fn iter_events(&self) -> anyhow::Result<Vec<Event>> {
        self.sqlite.iter_events()
    }

    // ── Branches JSON ───────────────────────────────────────────────

    /// Read branches.json content.
    pub fn branches_json(&self) -> anyhow::Result<serde_json::Value> {
        self.sqlite.branches_json()
    }

    /// Write branches.json content.
    pub fn set_branches_json(&self, value: &serde_json::Value) -> anyhow::Result<()> {
        self.sqlite.set_branches_json(value)
    }

    // ── Decisions ───────────────────────────────────────────────────

    /// Query active decisions, optionally filtered by domain or key pattern.
    pub fn active_decisions(
        &self,
        domain: Option<&str>,
        key_pattern: Option<&str>,
    ) -> anyhow::Result<Vec<DecisionRow>> {
        self.sqlite.active_decisions(domain, key_pattern)
    }

    /// All decisions for a key (active + superseded), ordered by time.
    pub fn decision_timeline(&self, key: &str) -> anyhow::Result<Vec<DecisionRow>> {
        self.sqlite.decision_timeline(key)
    }

    /// Find the active decision for a specific key on a branch.
    pub fn find_active_decision(
        &self,
        branch: &str,
        key: &str,
    ) -> anyhow::Result<Option<DecisionRow>> {
        self.sqlite.find_active_decision(branch, key)
    }
}

// ── Init functions ──────────────────────────────────────────────────

/// Initialize a new workspace from `EddaPaths`. Used by `cmd_init`.
///
/// Creates the directory layout AND a fresh `ledger.db` with schema.
pub fn init_workspace(paths: &EddaPaths) -> anyhow::Result<()> {
    paths.ensure_layout()?;
    std::fs::create_dir_all(paths.branch_dir("main"))?;
    SqliteStore::open_or_create(&paths.ledger_db)?;
    Ok(())
}

/// Write the initial HEAD into SQLite.
pub fn init_head(paths: &EddaPaths, branch: &str) -> anyhow::Result<()> {
    let store = SqliteStore::open(&paths.ledger_db)?;
    if store.head_branch().is_err() {
        store.set_head_branch(branch)?;
    }
    Ok(())
}

/// Write initial branches.json into SQLite.
pub fn init_branches_json(paths: &EddaPaths, branch: &str) -> anyhow::Result<()> {
    let now = time_now_rfc3339();
    let json = serde_json::json!({
        "branches": {
            branch: {
                "created_at": now
            }
        }
    });
    let store = SqliteStore::open(&paths.ledger_db)?;
    if store.branches_json().is_err() {
        store.set_branches_json(&json)?;
    }
    Ok(())
}

fn time_now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use edda_core::event::new_note_event;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn setup_workspace() -> (std::path::PathBuf, Ledger) {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("edda_ledger_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let paths = EddaPaths::discover(&tmp);
        init_workspace(&paths).unwrap();
        init_head(&paths, "main").unwrap();
        init_branches_json(&paths, "main").unwrap();
        let ledger = Ledger::open(&tmp).unwrap();
        (tmp, ledger)
    }

    #[test]
    fn empty_ledger_has_no_hash() {
        let (tmp, ledger) = setup_workspace();
        assert_eq!(ledger.last_event_hash().unwrap(), None);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn append_and_read_back() {
        let (tmp, ledger) = setup_workspace();
        let e1 = new_note_event("main", None, "system", "init", &[]).unwrap();
        ledger.append_event(&e1).unwrap();
        assert_eq!(ledger.last_event_hash().unwrap(), Some(e1.hash.clone()));

        let e2 = new_note_event("main", Some(&e1.hash), "user", "hello", &[]).unwrap();
        ledger.append_event(&e2).unwrap();
        assert_eq!(ledger.last_event_hash().unwrap(), Some(e2.hash.clone()));

        let events = ledger.iter_events().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_id, e1.event_id);
        assert_eq!(events[1].event_id, e2.event_id);
        assert_eq!(events[1].parent_hash.as_deref(), Some(e1.hash.as_str()));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn head_branch_read_write() {
        let (tmp, ledger) = setup_workspace();
        assert_eq!(ledger.head_branch().unwrap(), "main");
        ledger.set_head_branch("feat/x").unwrap();
        assert_eq!(ledger.head_branch().unwrap(), "feat/x");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn branches_json_read_write() {
        let (tmp, ledger) = setup_workspace();
        let bj = ledger.branches_json().unwrap();
        assert!(bj["branches"]["main"].is_object());

        let new_json = serde_json::json!({
            "branches": {
                "main": { "created_at": "2026-01-01T00:00:00Z" },
                "dev": { "created_at": "2026-02-01T00:00:00Z" }
            }
        });
        ledger.set_branches_json(&new_json).unwrap();
        let loaded = ledger.branches_json().unwrap();
        assert_eq!(loaded, new_json);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn open_without_init_fails() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("edda_no_init_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(Ledger::open(&tmp).is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
