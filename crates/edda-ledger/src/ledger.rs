use crate::paths::EddaPaths;
use crate::sqlite_store::{BundleRow, DecisionRow, SqliteStore};
use crate::view::{self, DecisionView};
use anyhow::Context;
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
                "not an edda workspace ({}/.edda not found). Run `edda init` first.",
                paths.root.display()
            );
        }
        let sqlite = SqliteStore::open_or_create(&paths.ledger_db)?;
        Ok(Self { paths, sqlite })
    }

    /// Open a workspace, auto-initializing `.edda/` if missing.
    ///
    /// Use this for read-path consumers (e.g. `edda watch`) that should
    /// work without requiring the user to run `edda init` first.
    ///
    /// This is a **lightweight init** — it only creates the ledger directory
    /// layout and SQLite DB. Config files (`policy.yaml`, `actors.yaml`) and
    /// bridge hooks are NOT created; those require `edda init`.
    pub fn open_or_init(repo_root: impl Into<std::path::PathBuf>) -> anyhow::Result<Self> {
        let root = repo_root.into();
        let paths = EddaPaths::discover(&root);
        if !paths.is_initialized() {
            init_workspace(&paths)?;
            init_head(&paths, "main")?;
            init_branches_json(&paths, "main")?;
        }
        Self::open(root)
    }

    /// Ensure `.edda/` and ledger exist, without returning a Ledger handle.
    ///
    /// Use this when you only need the side effect (workspace creation)
    /// and will open the ledger separately later.
    pub fn ensure_initialized(repo_root: impl Into<std::path::PathBuf>) -> anyhow::Result<()> {
        let root = repo_root.into();
        let paths = EddaPaths::discover(&root);
        if !paths.is_initialized() {
            init_workspace(&paths)?;
            init_head(&paths, "main")?;
            init_branches_json(&paths, "main")?;
        }
        Ok(())
    }

    /// Convenience: open from a Path ref (avoids Into<PathBuf> ambiguity).
    pub fn open_path(repo_root: &Path) -> anyhow::Result<Self> {
        Self::open(repo_root.to_path_buf())
    }

    // ── HEAD branch ─────────────────────────────────────────────────

    /// Read the current HEAD branch name.
    pub fn head_branch(&self) -> anyhow::Result<String> {
        self.sqlite.head_branch().context("Ledger::head_branch")
    }

    /// Write the HEAD branch name.
    pub fn set_head_branch(&self, name: &str) -> anyhow::Result<()> {
        self.sqlite
            .set_head_branch(name)
            .context("Ledger::set_head_branch")
    }

    // ── Events ──────────────────────────────────────────────────────

    /// Append an event to the ledger. Append-only (CONTRACT LEDGER-02).
    pub fn append_event(&self, event: &Event) -> anyhow::Result<()> {
        self.sqlite
            .append_event(event)
            .with_context(|| format!("Ledger::append_event({})", event.event_id))
    }

    /// Append an event idempotently. Returns `true` if inserted, `false` if duplicate.
    pub fn append_event_idempotent(&self, event: &Event) -> anyhow::Result<bool> {
        self.sqlite
            .append_event_idempotent(event)
            .with_context(|| format!("Ledger::append_event_idempotent({})", event.event_id))
    }

    /// Get the hash of the last event, or `None` if the ledger is empty.
    pub fn last_event_hash(&self) -> anyhow::Result<Option<String>> {
        self.sqlite
            .last_event_hash()
            .context("Ledger::last_event_hash")
    }

    /// Read all events in the ledger.
    pub fn iter_events(&self) -> anyhow::Result<Vec<Event>> {
        self.sqlite.iter_events().context("Ledger::iter_events")
    }

    /// Get a single event by event_id.
    pub fn get_event(&self, event_id: &str) -> anyhow::Result<Option<Event>> {
        self.sqlite
            .get_event(event_id)
            .with_context(|| format!("Ledger::get_event({event_id})"))
    }

    /// Get all events of a given type, filtered at the SQL level.
    pub fn iter_events_by_type(&self, event_type: &str) -> anyhow::Result<Vec<Event>> {
        self.sqlite
            .iter_events_by_type(event_type)
            .with_context(|| format!("Ledger::iter_events_by_type({event_type})"))
    }

    /// Get all events for a specific branch, filtered at the SQL level.
    pub fn iter_branch_events(&self, branch: &str) -> anyhow::Result<Vec<Event>> {
        self.sqlite
            .iter_branch_events(branch)
            .with_context(|| format!("Ledger::iter_branch_events({branch})"))
    }

    /// Get events filtered by branch with optional type/keyword/date/limit,
    /// all pushed down to SQL. Returns newest-first, capped at `limit`.
    pub fn iter_events_filtered(
        &self,
        branch: &str,
        event_type: Option<&str>,
        keyword: Option<&str>,
        after: Option<&str>,
        before: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<Event>> {
        self.sqlite
            .iter_events_filtered(branch, event_type, keyword, after, before, limit)
            .with_context(|| format!("Ledger::iter_events_filtered(branch={branch})"))
    }

    /// Find commit events related to a query by evidence chain or keyword match.
    pub fn find_related_commits(
        &self,
        branch: Option<&str>,
        keyword: &str,
        decision_event_ids: &[&str],
        limit: usize,
    ) -> anyhow::Result<Vec<Event>> {
        self.sqlite
            .find_related_commits(branch, keyword, decision_event_ids, limit)
            .context("Ledger::find_related_commits")
    }

    /// Find note events matching a keyword, excluding decisions and session digests.
    pub fn find_related_notes(
        &self,
        branch: Option<&str>,
        keyword: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<Event>> {
        self.sqlite
            .find_related_notes(branch, keyword, limit)
            .context("Ledger::find_related_notes")
    }

    /// Get all events with rowid strictly greater than `after_rowid`.
    ///
    /// Returns `(rowid, Event)` pairs ordered by rowid, useful for cursor-based
    /// polling (e.g. SSE streaming).
    pub fn events_after_rowid(&self, after_rowid: i64) -> anyhow::Result<Vec<(i64, Event)>> {
        self.sqlite
            .events_after_rowid(after_rowid)
            .context("Ledger::events_after_rowid")
    }

    /// Look up the rowid for a given `event_id`.
    pub fn rowid_for_event_id(&self, event_id: &str) -> anyhow::Result<Option<i64>> {
        self.sqlite
            .rowid_for_event_id(event_id)
            .with_context(|| format!("Ledger::rowid_for_event_id({event_id})"))
    }

    // ── Branches JSON ───────────────────────────────────────────────

    /// Read branches.json content.
    pub fn branches_json(&self) -> anyhow::Result<serde_json::Value> {
        self.sqlite.branches_json().context("Ledger::branches_json")
    }

    /// Write branches.json content.
    pub fn set_branches_json(&self, value: &serde_json::Value) -> anyhow::Result<()> {
        self.sqlite
            .set_branches_json(value)
            .context("Ledger::set_branches_json")
    }

    // ── Decisions ───────────────────────────────────────────────────

    /// Query active decisions, optionally filtered by domain or key pattern.
    /// `after`/`before` are optional ISO 8601 bounds for temporal filtering.
    pub fn active_decisions(
        &self,
        domain: Option<&str>,
        key_pattern: Option<&str>,
        after: Option<&str>,
        before: Option<&str>,
    ) -> anyhow::Result<Vec<DecisionRow>> {
        self.sqlite
            .active_decisions(domain, key_pattern, after, before, None)
            .with_context(|| format!("Ledger::active_decisions(domain={domain:?})"))
    }

    /// Query active decisions with limit for hot path optimization.
    pub fn active_decisions_limited(
        &self,
        domain: Option<&str>,
        key_pattern: Option<&str>,
        after: Option<&str>,
        before: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<DecisionRow>> {
        self.sqlite
            .active_decisions(domain, key_pattern, after, before, Some(limit))
            .with_context(|| format!("Ledger::active_decisions_limited(domain={domain:?})"))
    }

    /// All decisions for a key (active + superseded), ordered by time.
    /// `after`/`before` are optional ISO 8601 bounds for temporal filtering.
    pub fn decision_timeline(
        &self,
        key: &str,
        after: Option<&str>,
        before: Option<&str>,
    ) -> anyhow::Result<Vec<DecisionRow>> {
        self.sqlite
            .decision_timeline(key, after, before)
            .with_context(|| format!("Ledger::decision_timeline(key={key})"))
    }

    /// All decisions for a domain (active + superseded), ordered by time.
    /// `after`/`before` are optional ISO 8601 bounds for temporal filtering.
    pub fn domain_timeline(
        &self,
        domain: &str,
        after: Option<&str>,
        before: Option<&str>,
    ) -> anyhow::Result<Vec<DecisionRow>> {
        self.sqlite
            .domain_timeline(domain, after, before)
            .with_context(|| format!("Ledger::domain_timeline(domain={domain})"))
    }

    /// Distinct domain values from active decisions.
    pub fn list_domains(&self) -> anyhow::Result<Vec<String>> {
        self.sqlite.list_domains().context("Ledger::list_domains")
    }

    /// Compute aggregate statistics for a village's decisions.
    pub fn village_stats(
        &self,
        village_id: &str,
        after: Option<&str>,
        before: Option<&str>,
    ) -> anyhow::Result<crate::sqlite_store::VillageStats> {
        self.sqlite
            .village_stats(village_id, after, before)
            .with_context(|| format!("Ledger::village_stats({village_id})"))
    }

    /// Detect recurring patterns in a village's decision history.
    pub fn detect_village_patterns(
        &self,
        village_id: &str,
        after: &str,
        min_occurrences: usize,
    ) -> anyhow::Result<Vec<crate::sqlite_store::DetectedPattern>> {
        self.sqlite
            .detect_village_patterns(village_id, after, min_occurrences)
            .with_context(|| format!("Ledger::detect_village_patterns({village_id})"))
    }

    /// Find the active decision for a specific key on a branch.
    pub fn find_active_decision(
        &self,
        branch: &str,
        key: &str,
    ) -> anyhow::Result<Option<DecisionRow>> {
        self.sqlite
            .find_active_decision(branch, key)
            .with_context(|| format!("Ledger::find_active_decision(branch={branch}, key={key})"))
    }

    /// Return active decisions that have non-empty `affected_paths`.
    /// Used by Injection to get the candidate set for glob matching.
    pub fn query_active_with_paths(
        &self,
        branch: Option<&str>,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<DecisionView>> {
        let rows = self
            .sqlite
            .active_decisions_with_paths(branch, limit)
            .context("Ledger::query_active_with_paths")?;
        Ok(rows.iter().map(view::to_view).collect())
    }

    /// Given a list of file paths, return decisions whose `affected_paths` globs
    /// match any of them. Uses `globset` for pattern matching.
    ///
    /// This is the primary query for PreToolUse hook (Track E):
    /// "which active decisions govern the file I'm about to edit?"
    ///
    /// # Arguments
    /// - `paths`: concrete file paths to check (e.g., `["crates/edda-ledger/src/lib.rs"]`)
    /// - `branch`: optional branch filter
    /// - `limit`: max decisions to return
    pub fn query_by_paths(
        &self,
        paths: &[&str],
        branch: Option<&str>,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<DecisionView>> {
        // 1. Get all active decisions that have affected_paths
        let candidates = self.query_active_with_paths(branch, None)?;

        // 2. Filter by glob match
        let mut matched: Vec<DecisionView> = Vec::new();
        for decision in candidates {
            let dominated = decision.affected_paths.iter().any(|glob_pattern| {
                match globset::Glob::new(glob_pattern) {
                    Ok(glob) => {
                        let matcher = glob.compile_matcher();
                        paths.iter().any(|p| matcher.is_match(p))
                    }
                    Err(_) => false, // invalid glob -> skip silently
                }
            });
            if dominated {
                matched.push(decision);
            }
            if let Some(lim) = limit {
                if matched.len() >= lim {
                    break;
                }
            }
        }

        Ok(matched)
    }

    // ── Cross-Project Sync ────────────────────────────────────────────

    /// Query active decisions with shared or global scope.
    pub fn shared_decisions(&self) -> anyhow::Result<Vec<DecisionRow>> {
        self.sqlite
            .shared_decisions()
            .context("Ledger::shared_decisions")
    }

    /// Check if a decision has already been imported from a source project.
    pub fn is_already_imported(
        &self,
        source_project_id: &str,
        source_event_id: &str,
    ) -> anyhow::Result<bool> {
        self.sqlite
            .is_already_imported(source_project_id, source_event_id)
            .context("Ledger::is_already_imported")
    }

    /// Insert an imported decision from another project.
    pub fn insert_imported_decision(
        &self,
        params: crate::sqlite_store::ImportParams<'_>,
    ) -> anyhow::Result<()> {
        self.sqlite
            .insert_imported_decision(params)
            .context("Ledger::insert_imported_decision")
    }

    // ── Decision Dependencies ────────────────────────────────────────

    /// Insert a dependency edge between two decision keys.
    pub fn insert_dep(
        &self,
        source_key: &str,
        target_key: &str,
        dep_type: &str,
        created_event: Option<&str>,
    ) -> anyhow::Result<()> {
        self.sqlite
            .insert_dep(source_key, target_key, dep_type, created_event)
            .with_context(|| format!("Ledger::insert_dep({source_key} -> {target_key})"))
    }

    /// What does `key` depend on?
    pub fn deps_of(&self, key: &str) -> anyhow::Result<Vec<crate::sqlite_store::DepRow>> {
        self.sqlite
            .deps_of(key)
            .with_context(|| format!("Ledger::deps_of({key})"))
    }

    /// Who depends on `key`?
    pub fn dependents_of(&self, key: &str) -> anyhow::Result<Vec<crate::sqlite_store::DepRow>> {
        self.sqlite
            .dependents_of(key)
            .with_context(|| format!("Ledger::dependents_of({key})"))
    }

    /// Who depends on `key`, joined with active decisions only.
    pub fn active_dependents_of(
        &self,
        key: &str,
    ) -> anyhow::Result<Vec<(crate::sqlite_store::DepRow, DecisionRow)>> {
        self.sqlite
            .active_dependents_of(key)
            .with_context(|| format!("Ledger::active_dependents_of({key})"))
    }

    // ── Decision Outcomes ─────────────────────────────────────────────

    /// Get aggregated outcome metrics for a decision.
    pub fn decision_outcomes(
        &self,
        decision_event_id: &str,
    ) -> anyhow::Result<Option<crate::sqlite_store::OutcomeMetrics>> {
        self.sqlite
            .decision_outcomes(decision_event_id)
            .with_context(|| format!("Ledger::decision_outcomes({decision_event_id})"))
    }

    /// Get all execution events linked to a decision via `based_on` provenance.
    pub fn executions_for_decision(
        &self,
        decision_event_id: &str,
    ) -> anyhow::Result<Vec<crate::sqlite_store::ExecutionLinked>> {
        self.sqlite
            .executions_for_decision(decision_event_id)
            .with_context(|| format!("Ledger::executions_for_decision({decision_event_id})"))
    }

    /// Transitive dependents of `key` via BFS, up to `max_depth` hops.
    /// Returns `(DepRow, DecisionRow, depth)` — only active decisions, deduplicated.
    pub fn transitive_dependents_of(
        &self,
        key: &str,
        max_depth: usize,
    ) -> anyhow::Result<Vec<(crate::sqlite_store::DepRow, DecisionRow, usize)>> {
        self.sqlite
            .transitive_dependents_of(key, max_depth)
            .with_context(|| format!("Ledger::transitive_dependents_of({key})"))
    }

    // ── Causal Chain ─────────────────────────────────────────────────

    /// Look up a single decision by event_id.
    pub fn get_decision_by_event_id(&self, event_id: &str) -> anyhow::Result<Option<DecisionRow>> {
        self.sqlite
            .get_decision_by_event_id(event_id)
            .with_context(|| format!("Ledger::get_decision_by_event_id({event_id})"))
    }

    /// Traverse the causal chain from a root decision via unified BFS.
    pub fn causal_chain(
        &self,
        event_id: &str,
        max_depth: usize,
    ) -> anyhow::Result<Option<(DecisionRow, Vec<crate::sqlite_store::ChainEntry>)>> {
        self.sqlite
            .causal_chain(event_id, max_depth)
            .with_context(|| format!("Ledger::causal_chain({event_id})"))
    }

    // ── Task Briefs ──────────────────────────────────────────────────

    /// Get a task brief by task_id.
    pub fn get_task_brief(
        &self,
        task_id: &str,
    ) -> anyhow::Result<Option<crate::sqlite_store::TaskBriefRow>> {
        self.sqlite
            .get_task_brief(task_id)
            .with_context(|| format!("Ledger::get_task_brief({task_id})"))
    }

    /// List task briefs, optionally filtered by status and/or intent.
    pub fn list_task_briefs(
        &self,
        status: Option<&str>,
        intent: Option<&str>,
    ) -> anyhow::Result<Vec<crate::sqlite_store::TaskBriefRow>> {
        self.sqlite
            .list_task_briefs(status, intent)
            .context("Ledger::list_task_briefs")
    }

    // ── Review Bundles ───────────────────────────────────────────────

    /// Get a review bundle by bundle_id.
    pub fn get_bundle(&self, bundle_id: &str) -> anyhow::Result<Option<BundleRow>> {
        self.sqlite
            .get_bundle(bundle_id)
            .with_context(|| format!("Ledger::get_bundle({bundle_id})"))
    }

    /// List review bundles, optionally filtered by status.
    pub fn list_bundles(&self, status: Option<&str>) -> anyhow::Result<Vec<BundleRow>> {
        self.sqlite
            .list_bundles(status)
            .context("Ledger::list_bundles")
    }

    // ── Device Tokens ───────────────────────────────────────────────

    /// Insert a new device token row.
    pub fn insert_device_token(
        &self,
        row: &crate::sqlite_store::DeviceTokenRow,
    ) -> anyhow::Result<()> {
        self.sqlite
            .insert_device_token(row)
            .context("Ledger::insert_device_token")
    }

    /// Validate a device token by its SHA-256 hash. Returns the row if active.
    pub fn validate_device_token(
        &self,
        token_hash: &str,
    ) -> anyhow::Result<Option<crate::sqlite_store::DeviceTokenRow>> {
        self.sqlite
            .validate_device_token(token_hash)
            .context("Ledger::validate_device_token")
    }

    /// List all device tokens (active and revoked).
    pub fn list_device_tokens(&self) -> anyhow::Result<Vec<crate::sqlite_store::DeviceTokenRow>> {
        self.sqlite
            .list_device_tokens()
            .context("Ledger::list_device_tokens")
    }

    /// Revoke a device token by device name. Returns true if revoked.
    pub fn revoke_device_token(
        &self,
        device_name: &str,
        revoke_event_id: &str,
    ) -> anyhow::Result<bool> {
        self.sqlite
            .revoke_device_token(device_name, revoke_event_id)
            .with_context(|| format!("Ledger::revoke_device_token({device_name})"))
    }

    /// Revoke all active device tokens. Returns count of revoked tokens.
    pub fn revoke_all_device_tokens(&self, revoke_event_id: &str) -> anyhow::Result<u64> {
        self.sqlite
            .revoke_all_device_tokens(revoke_event_id)
            .context("Ledger::revoke_all_device_tokens")
    }

    // ── Decide Snapshots ────────────────────────────────────────────

    /// Insert a row into the `decide_snapshots` materialized view.
    pub fn insert_snapshot(
        &self,
        row: &crate::sqlite_store::DecideSnapshotRow,
    ) -> anyhow::Result<()> {
        self.sqlite
            .insert_snapshot(row)
            .context("Ledger::insert_snapshot")
    }

    /// Query snapshots with optional filtering by village_id and engine_version.
    pub fn query_snapshots(
        &self,
        village_id: Option<&str>,
        engine_version: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<crate::sqlite_store::DecideSnapshotRow>> {
        self.sqlite
            .query_snapshots(village_id, engine_version, limit)
            .context("Ledger::query_snapshots")
    }

    /// Find all snapshots for a given context_hash.
    pub fn snapshots_by_context_hash(
        &self,
        context_hash: &str,
    ) -> anyhow::Result<Vec<crate::sqlite_store::DecideSnapshotRow>> {
        self.sqlite
            .snapshots_by_context_hash(context_hash)
            .with_context(|| format!("Ledger::snapshots_by_context_hash({context_hash})"))
    }

    // ── Suggestions ──────────────────────────────────────────────────

    /// Insert a new suggestion row.
    pub fn insert_suggestion(
        &self,
        row: &crate::sqlite_store::SuggestionRow,
    ) -> anyhow::Result<()> {
        self.sqlite
            .insert_suggestion(row)
            .context("Ledger::insert_suggestion")
    }

    /// List suggestions filtered by status.
    pub fn list_suggestions_by_status(
        &self,
        status: &str,
    ) -> anyhow::Result<Vec<crate::sqlite_store::SuggestionRow>> {
        self.sqlite
            .list_suggestions_by_status(status)
            .with_context(|| format!("Ledger::list_suggestions_by_status({status})"))
    }

    /// Get a single suggestion by id.
    pub fn get_suggestion(
        &self,
        id: &str,
    ) -> anyhow::Result<Option<crate::sqlite_store::SuggestionRow>> {
        self.sqlite
            .get_suggestion(id)
            .with_context(|| format!("Ledger::get_suggestion({id})"))
    }

    /// Update a suggestion's status and reviewed_at timestamp.
    /// Returns true if a row was updated.
    pub fn update_suggestion_status(
        &self,
        id: &str,
        status: &str,
        reviewed_at: &str,
    ) -> anyhow::Result<bool> {
        self.sqlite
            .update_suggestion_status(id, status, reviewed_at)
            .with_context(|| format!("Ledger::update_suggestion_status({id})"))
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
    fn open_or_init_creates_workspace() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("edda_auto_init_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // No .edda/ exists yet
        assert!(!tmp.join(".edda").exists());

        // open_or_init should create it
        let ledger = Ledger::open_or_init(&tmp).unwrap();
        assert!(tmp.join(".edda").exists());
        assert_eq!(ledger.head_branch().unwrap(), "main");

        // Second call is idempotent
        let ledger2 = Ledger::open_or_init(&tmp).unwrap();
        assert_eq!(ledger2.head_branch().unwrap(), "main");

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

    // ── transitive_dependents_of tests ─────────────────────────────

    fn make_decision_event(branch: &str, key: &str, value: &str) -> edda_core::Event {
        use edda_core::event::finalize_event;
        let text = format!("{key}: {value}");
        let tags = vec!["decision".to_string()];
        let mut event = new_note_event(branch, None, "system", &text, &tags).unwrap();
        event.payload["decision"] = serde_json::json!({"key": key, "value": value});
        finalize_event(&mut event).unwrap();
        event
    }

    #[test]
    fn transitive_dependents_chain() {
        let (tmp, ledger) = setup_workspace();

        // A -> B -> C chain
        ledger
            .append_event(&make_decision_event("main", "a.root", "v1"))
            .unwrap();
        ledger
            .append_event(&make_decision_event("main", "b.mid", "v2"))
            .unwrap();
        ledger
            .append_event(&make_decision_event("main", "c.leaf", "v3"))
            .unwrap();

        ledger
            .insert_dep("b.mid", "a.root", "explicit", None)
            .unwrap();
        ledger
            .insert_dep("c.leaf", "b.mid", "explicit", None)
            .unwrap();

        let deps = ledger.transitive_dependents_of("a.root", 3).unwrap();
        assert_eq!(deps.len(), 2);

        let b = deps.iter().find(|(_, d, _)| d.key == "b.mid").unwrap();
        assert_eq!(b.2, 1);

        let c = deps.iter().find(|(_, d, _)| d.key == "c.leaf").unwrap();
        assert_eq!(c.2, 2);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn transitive_dependents_diamond_no_duplicates() {
        let (tmp, ledger) = setup_workspace();

        // A -> B, A -> C, B -> D, C -> D
        for (k, v) in [
            ("a.root", "v1"),
            ("b.left", "v2"),
            ("c.right", "v3"),
            ("d.leaf", "v4"),
        ] {
            ledger
                .append_event(&make_decision_event("main", k, v))
                .unwrap();
        }
        ledger
            .insert_dep("b.left", "a.root", "explicit", None)
            .unwrap();
        ledger
            .insert_dep("c.right", "a.root", "explicit", None)
            .unwrap();
        ledger
            .insert_dep("d.leaf", "b.left", "explicit", None)
            .unwrap();
        ledger
            .insert_dep("d.leaf", "c.right", "explicit", None)
            .unwrap();

        let deps = ledger.transitive_dependents_of("a.root", 3).unwrap();
        // Should have B, C (depth 1) and D (depth 2) — no duplicates
        assert_eq!(deps.len(), 3);

        let d_hits: Vec<_> = deps.iter().filter(|(_, d, _)| d.key == "d.leaf").collect();
        assert_eq!(d_hits.len(), 1, "d.leaf should appear only once");
        assert_eq!(d_hits[0].2, 2);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn transitive_dependents_depth_limit() {
        let (tmp, ledger) = setup_workspace();

        // Chain of 5: a -> b -> c -> d -> e
        for (k, v) in [
            ("a.n1", "v1"),
            ("b.n2", "v2"),
            ("c.n3", "v3"),
            ("d.n4", "v4"),
            ("e.n5", "v5"),
        ] {
            ledger
                .append_event(&make_decision_event("main", k, v))
                .unwrap();
        }
        ledger.insert_dep("b.n2", "a.n1", "explicit", None).unwrap();
        ledger.insert_dep("c.n3", "b.n2", "explicit", None).unwrap();
        ledger.insert_dep("d.n4", "c.n3", "explicit", None).unwrap();
        ledger.insert_dep("e.n5", "d.n4", "explicit", None).unwrap();

        // Limit to 2 hops
        let deps = ledger.transitive_dependents_of("a.n1", 2).unwrap();
        assert_eq!(deps.len(), 2, "depth limit 2 should return only 2 hops");

        let keys: Vec<&str> = deps.iter().map(|(_, d, _)| d.key.as_str()).collect();
        assert!(keys.contains(&"b.n2"));
        assert!(keys.contains(&"c.n3"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn transitive_dependents_empty() {
        let (tmp, ledger) = setup_workspace();
        ledger
            .append_event(&make_decision_event("main", "solo.key", "val"))
            .unwrap();

        let deps = ledger.transitive_dependents_of("solo.key", 3).unwrap();
        assert!(deps.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn append_event_idempotent_dedup() {
        let (tmp, ledger) = setup_workspace();
        let event = new_note_event("main", None, "system", "test idempotent", &[]).unwrap();

        // First insert returns true
        let inserted = ledger.append_event_idempotent(&event).unwrap();
        assert!(inserted);

        // Duplicate insert returns false (no error)
        let inserted2 = ledger.append_event_idempotent(&event).unwrap();
        assert!(!inserted2);

        // Only one event in ledger
        let events = ledger.iter_events().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_id, event.event_id);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn iter_events_by_type_filters_correctly() {
        use edda_core::event::new_execution_event;

        let (tmp, ledger) = setup_workspace();
        let note = new_note_event("main", None, "system", "a note", &[]).unwrap();
        ledger.append_event(&note).unwrap();

        let payload = serde_json::json!({
            "runtime": "claude", "model": "claude-3-opus",
            "usage": { "token_in": 100, "token_out": 50, "cost_usd": 0.01, "latency_ms": 500 },
            "result": { "status": "success" },
            "event_type": "step_completed",
        });
        let exec = new_execution_event(
            "main",
            Some(&note.hash),
            "evt_exec_1",
            "2026-03-11T00:00:00Z",
            payload,
            None,
        )
        .unwrap();
        ledger.append_event(&exec).unwrap();

        assert_eq!(ledger.iter_events().unwrap().len(), 2);
        let exec_events = ledger.iter_events_by_type("execution_event").unwrap();
        assert_eq!(exec_events.len(), 1);
        assert_eq!(exec_events[0].event_id, "evt_exec_1");
        let note_events = ledger.iter_events_by_type("note").unwrap();
        assert_eq!(note_events.len(), 1);
        assert_eq!(note_events[0].event_id, note.event_id);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn iter_events_by_type_empty_for_unknown() {
        let (tmp, ledger) = setup_workspace();
        let note = new_note_event("main", None, "system", "a note", &[]).unwrap();
        ledger.append_event(&note).unwrap();
        let result = ledger.iter_events_by_type("nonexistent_type").unwrap();
        assert!(result.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ── Device Token tests ──────────────────────────────────────────

    /// Helper: append a note event and return its event_id (satisfies FK on device_tokens).
    fn insert_pair_event(ledger: &Ledger) -> String {
        let evt = new_note_event("main", None, "system", "device_pair placeholder", &[]).unwrap();
        let eid = evt.event_id.clone();
        ledger.append_event(&evt).unwrap();
        eid
    }

    #[test]
    fn device_token_insert_and_validate() {
        use crate::device_token::{generate_device_token, hash_token};

        let (tmp, ledger) = setup_workspace();
        let pair_eid = insert_pair_event(&ledger);

        let raw = generate_device_token();
        let hashed = hash_token(&raw);

        ledger
            .insert_device_token(&crate::sqlite_store::DeviceTokenRow {
                token_hash: hashed.clone(),
                device_name: "test-phone".to_string(),
                paired_at: "2026-01-01T00:00:00Z".to_string(),
                paired_from_ip: "127.0.0.1".to_string(),
                revoked_at: None,
                pair_event_id: pair_eid,
                revoke_event_id: None,
            })
            .unwrap();

        // Validate with correct hash succeeds
        let found = ledger.validate_device_token(&hashed).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().device_name, "test-phone");

        // Validate with wrong hash returns None
        let wrong = ledger.validate_device_token("badhash").unwrap();
        assert!(wrong.is_none());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn device_token_revoke_flow() {
        use crate::device_token::{generate_device_token, hash_token};

        let (tmp, ledger) = setup_workspace();
        let pair_eid = insert_pair_event(&ledger);

        let raw = generate_device_token();
        let hashed = hash_token(&raw);

        ledger
            .insert_device_token(&crate::sqlite_store::DeviceTokenRow {
                token_hash: hashed.clone(),
                device_name: "my-tablet".to_string(),
                paired_at: "2026-01-01T00:00:00Z".to_string(),
                paired_from_ip: "127.0.0.1".to_string(),
                revoked_at: None,
                pair_event_id: pair_eid,
                revoke_event_id: None,
            })
            .unwrap();

        // Token is valid before revocation
        assert!(ledger.validate_device_token(&hashed).unwrap().is_some());

        // Revoke it
        let revoked = ledger.revoke_device_token("my-tablet", "evt_rev1").unwrap();
        assert!(revoked);

        // Token is no longer valid after revocation
        assert!(ledger.validate_device_token(&hashed).unwrap().is_none());

        // Revoking again returns false (already revoked)
        let revoked2 = ledger.revoke_device_token("my-tablet", "evt_rev2").unwrap();
        assert!(!revoked2);

        // Revoking nonexistent device returns false
        let revoked3 = ledger
            .revoke_device_token("no-such-device", "evt_rev3")
            .unwrap();
        assert!(!revoked3);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn device_token_revoke_all() {
        use crate::device_token::{generate_device_token, hash_token};

        let (tmp, ledger) = setup_workspace();

        // Insert two devices, each with a real event for the FK
        for name in ["dev-a", "dev-b"] {
            let pair_eid = insert_pair_event(&ledger);
            let raw = generate_device_token();
            let hashed = hash_token(&raw);
            ledger
                .insert_device_token(&crate::sqlite_store::DeviceTokenRow {
                    token_hash: hashed,
                    device_name: name.to_string(),
                    paired_at: "2026-01-01T00:00:00Z".to_string(),
                    paired_from_ip: "127.0.0.1".to_string(),
                    revoked_at: None,
                    pair_event_id: pair_eid,
                    revoke_event_id: None,
                })
                .unwrap();
        }

        let tokens = ledger.list_device_tokens().unwrap();
        assert_eq!(tokens.len(), 2);
        let active = tokens.iter().filter(|t| t.revoked_at.is_none()).count();
        assert_eq!(active, 2);

        // Revoke all
        let count = ledger.revoke_all_device_tokens("evt_revall").unwrap();
        assert_eq!(count, 2);

        // All revoked
        let tokens2 = ledger.list_device_tokens().unwrap();
        let active2 = tokens2.iter().filter(|t| t.revoked_at.is_none()).count();
        assert_eq!(active2, 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn device_token_list_shows_all() {
        use crate::device_token::{generate_device_token, hash_token};

        let (tmp, ledger) = setup_workspace();

        let pair_eid1 = insert_pair_event(&ledger);
        let pair_eid2 = insert_pair_event(&ledger);

        let raw1 = generate_device_token();
        let raw2 = generate_device_token();

        ledger
            .insert_device_token(&crate::sqlite_store::DeviceTokenRow {
                token_hash: hash_token(&raw1),
                device_name: "active-dev".to_string(),
                paired_at: "2026-01-01T00:00:00Z".to_string(),
                paired_from_ip: "127.0.0.1".to_string(),
                revoked_at: None,
                pair_event_id: pair_eid1,
                revoke_event_id: None,
            })
            .unwrap();

        ledger
            .insert_device_token(&crate::sqlite_store::DeviceTokenRow {
                token_hash: hash_token(&raw2),
                device_name: "revoked-dev".to_string(),
                paired_at: "2026-01-01T00:00:00Z".to_string(),
                paired_from_ip: "192.168.1.1".to_string(),
                revoked_at: None,
                pair_event_id: pair_eid2,
                revoke_event_id: None,
            })
            .unwrap();

        // Revoke the second one
        ledger
            .revoke_device_token("revoked-dev", "evt_rev")
            .unwrap();

        let all = ledger.list_device_tokens().unwrap();
        assert_eq!(all.len(), 2, "list should include both active and revoked");

        let active: Vec<_> = all.iter().filter(|t| t.revoked_at.is_none()).collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].device_name, "active-dev");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ── query_by_paths tests ─────────────────────────────────────────

    /// Helper: create a decision event and set its affected_paths via raw SQL.
    fn insert_decision_with_paths(
        ledger: &Ledger,
        key: &str,
        value: &str,
        paths: &[&str],
    ) -> String {
        let event = make_decision_event("main", key, value);
        let event_id = event.event_id.clone();
        ledger.append_event(&event).unwrap();

        // Update affected_paths via raw SQL (materialization doesn't set it from payload)
        let paths_json = serde_json::to_string(paths).unwrap();
        let conn = rusqlite::Connection::open(&ledger.paths.ledger_db).unwrap();
        conn.execute(
            "UPDATE decisions SET affected_paths = ?1 WHERE event_id = ?2",
            rusqlite::params![paths_json, event_id],
        )
        .unwrap();

        event_id
    }

    #[test]
    fn query_by_paths_glob_matches() {
        let (tmp, ledger) = setup_workspace();
        insert_decision_with_paths(&ledger, "db.engine", "sqlite", &["crates/edda-ledger/**"]);

        let results = ledger
            .query_by_paths(&["crates/edda-ledger/src/lib.rs"], Some("main"), None)
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "db.engine");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn query_by_paths_no_match() {
        let (tmp, ledger) = setup_workspace();
        insert_decision_with_paths(&ledger, "db.engine", "sqlite", &["crates/edda-ledger/**"]);

        let results = ledger
            .query_by_paths(
                &["crates/edda-cli/src/main.rs"], // doesn't match ledger glob
                Some("main"),
                None,
            )
            .unwrap();

        assert!(results.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn query_by_paths_skips_superseded() {
        let (tmp, ledger) = setup_workspace();

        // First decision: db.engine=postgres
        insert_decision_with_paths(&ledger, "db.engine", "postgres", &["crates/**"]);

        // Second decision supersedes first (same key+branch → first becomes is_active=FALSE)
        insert_decision_with_paths(&ledger, "db.engine", "sqlite", &["crates/**"]);

        let results = ledger
            .query_by_paths(&["crates/edda-ledger/src/lib.rs"], Some("main"), None)
            .unwrap();

        // Only the active decision should appear
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, "sqlite");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn query_by_paths_multiple_globs() {
        let (tmp, ledger) = setup_workspace();
        insert_decision_with_paths(
            &ledger,
            "error.pattern",
            "thiserror",
            &["crates/edda-ledger/**", "crates/edda-core/**"],
        );

        // Match on second glob
        let results = ledger
            .query_by_paths(&["crates/edda-core/src/types.rs"], Some("main"), None)
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "error.pattern");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn query_by_paths_respects_limit() {
        let (tmp, ledger) = setup_workspace();

        // Insert 3 decisions all matching "crates/**"
        insert_decision_with_paths(&ledger, "db.engine", "sqlite", &["crates/**"]);
        insert_decision_with_paths(&ledger, "error.pattern", "thiserror", &["crates/**"]);
        insert_decision_with_paths(&ledger, "auth.strategy", "jwt", &["crates/**"]);

        let results = ledger
            .query_by_paths(&["crates/edda-ledger/src/lib.rs"], Some("main"), Some(2))
            .unwrap();

        assert!(results.len() <= 2);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn query_active_with_paths_excludes_empty() {
        let (tmp, ledger) = setup_workspace();

        // Decision without paths (defaults to "[]")
        ledger
            .append_event(&make_decision_event("main", "no.paths", "val"))
            .unwrap();

        // Decision with paths
        insert_decision_with_paths(&ledger, "has.paths", "val", &["src/**"]);

        let results = ledger.query_active_with_paths(Some("main"), None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "has.paths");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
