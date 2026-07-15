# Search Auto-Index Implementation Plan (GH-403)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Tantivy event index incremental and keep it current automatically, so `edda search` never ships cold or silently omits recent history.

**Architecture:** A single `sync` entry point in `edda-search-fts` owns all index orchestration (lock, open/create, cursor, incremental event indexing, turns, commit, cursor write). Events become incremental via a sqlite rowid cursor over the existing `Ledger::events_after_rowid`, with per-event `delete_term` replace for idempotence and commit-before-cursor ordering for crash safety. Three callers sit in front of it: manual `edda search index`, query-time build-if-missing, and a new SessionEnd background task.

**Tech Stack:** Rust workspace, tantivy 0.25, rusqlite (bundled), fs2 file locks, anyhow, tracing.

**Spec:** `docs/plan/search-auto-index/SEARCH_AUTO_INDEX_V1.md`

## Global Constraints

- Branch: `agent/GH-403`. Repo `main` accepts PRs only; merge is the operator's gate.
- **No new Tantivy field.** `INDEX_VERSION` stays `2` — a bump forces every user through a second full rebuild days after #410 forced one.
- **Commit before cursor write, always.** The reverse order loses events permanently.
- Callers never pass or reset the cursor; `sync` owns it.
- SessionEnd must never block exit: best-effort, `tracing::warn!` on failure, no cold build.
- **Do not touch `cmd_ask.rs`.** Its transcript callback stays read-only (`open_index`). Build-if-missing belongs to the user-facing `search query` only — a background hook must never silently stall ~25s on a cold build.
- `RUSTFLAGS: -Dwarnings` in CI — no warnings, including unused imports after removals.
- Tests run with `cargo test -p edda-search-fts` / `-p edda` (the CLI package is named **`edda`**, not `edda-cli`).
- Known flake, not caused by this work: `secret_guard::tests::hot_path_under_ten_ms_for_realistic_length` fails under parallel load, passes in isolation.

---

### Task 1: Meta-DB cursor storage

Adds the `events_watermark` table and its accessors. The DDL is currently duplicated verbatim between `ensure_meta_db` and `ensure_meta_db_memory`; adding a third table to both by hand would let the test DB drift from the real one, so extract it first.

**Files:**
- Modify: `crates/edda-search-fts/src/schema.rs:200-270`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct EventsCursor { pub rowid: i64, pub ts: Option<String> }`
  - `pub fn read_events_cursor(conn: &Connection, project_id: &str) -> anyhow::Result<EventsCursor>`
  - `pub fn write_events_cursor(conn: &Connection, project_id: &str, rowid: i64, ts: Option<&str>) -> anyhow::Result<()>`
  - `clear_index_watermark` additionally clears `events_watermark`.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block at the bottom of `crates/edda-search-fts/src/schema.rs`:

```rust
    #[test]
    fn events_cursor_roundtrip_and_default() {
        let conn = ensure_meta_db_memory().unwrap();

        // Absent cursor reads as zero, not an error.
        let c = read_events_cursor(&conn, "p1").unwrap();
        assert_eq!(c.rowid, 0);
        assert_eq!(c.ts, None);

        write_events_cursor(&conn, "p1", 42, Some("2026-07-15T09:40:00Z")).unwrap();
        let c = read_events_cursor(&conn, "p1").unwrap();
        assert_eq!(c.rowid, 42);
        assert_eq!(c.ts.as_deref(), Some("2026-07-15T09:40:00Z"));

        // Upsert, not a second row.
        write_events_cursor(&conn, "p1", 99, Some("2026-07-15T10:00:00Z")).unwrap();
        let c = read_events_cursor(&conn, "p1").unwrap();
        assert_eq!(c.rowid, 99);

        // Cursors are per project.
        assert_eq!(read_events_cursor(&conn, "p2").unwrap().rowid, 0);
    }

    #[test]
    fn clear_index_watermark_also_clears_events_cursor() {
        let conn = ensure_meta_db_memory().unwrap();
        write_events_cursor(&conn, "p1", 42, Some("2026-07-15T09:40:00Z")).unwrap();

        clear_index_watermark(&conn).unwrap();

        // A fresh index must re-index every event, so the cursor must reset too.
        assert_eq!(read_events_cursor(&conn, "p1").unwrap().rowid, 0);
    }

    #[test]
    fn memory_and_file_meta_dbs_have_the_same_tables() {
        // The two builders share one DDL const; this pins that they cannot drift.
        let tmp = tempfile::tempdir().unwrap();
        let file_conn = ensure_meta_db(&tmp.path().join("meta.sqlite")).unwrap();
        let mem_conn = ensure_meta_db_memory().unwrap();

        let tables = |c: &Connection| -> Vec<String> {
            let mut stmt = c
                .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                .unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
            rows.map(|r| r.unwrap())
                .filter(|n| !n.starts_with("sqlite_"))
                .collect()
        };

        assert_eq!(tables(&file_conn), tables(&mem_conn));
        assert!(tables(&mem_conn).contains(&"events_watermark".to_string()));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p edda-search-fts schema:: 2>&1 | tail -20`
Expected: FAIL — `cannot find function 'read_events_cursor'`, `cannot find function 'write_events_cursor'`, `cannot find type 'EventsCursor'`.

- [ ] **Step 3: Extract the DDL to a single const**

In `crates/edda-search-fts/src/schema.rs`, add above `ensure_meta_db`:

```rust
/// Schema for the search meta database, shared by the on-disk and in-memory
/// builders so a test DB can never drift from the real one. All statements are
/// `IF NOT EXISTS`, so adding a table needs no migration.
const META_DDL: &str = "
    CREATE TABLE IF NOT EXISTS turns_meta (
        turn_id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        session_id TEXT NOT NULL,
        ts TEXT,
        user_uuid TEXT,
        assistant_uuid TEXT,
        user_store_offset INTEGER,
        user_store_len INTEGER,
        assistant_store_offset INTEGER,
        assistant_store_len INTEGER
    );

    CREATE TABLE IF NOT EXISTS index_watermark (
        project_id TEXT NOT NULL,
        session_id TEXT NOT NULL,
        last_offset INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (project_id, session_id)
    );

    CREATE TABLE IF NOT EXISTS events_watermark (
        project_id TEXT PRIMARY KEY,
        last_rowid INTEGER NOT NULL DEFAULT 0,
        last_ts TEXT
    );
";
```

Replace the body of `ensure_meta_db` (the `conn.execute_batch(\"...\")?;` call with the inline DDL string) with:

```rust
    conn.execute_batch(META_DDL)?;
```

Replace the same inline DDL inside `ensure_meta_db_memory` with:

```rust
    conn.execute_batch(META_DDL)?;
```

- [ ] **Step 4: Add the cursor accessors**

In `crates/edda-search-fts/src/schema.rs`, change the rusqlite import at the top of the file:

```rust
use rusqlite::{Connection, OptionalExtension};
```

Add after `clear_index_watermark`:

```rust
/// Where the event index has reached: the ledger rowid of the last indexed
/// event, plus its timestamp for staleness reporting (GH-403).
#[derive(Debug, Clone, PartialEq)]
pub struct EventsCursor {
    pub rowid: i64,
    pub ts: Option<String>,
}

/// Read a project's event cursor. An absent cursor is `rowid = 0` — index from
/// the beginning — not an error.
pub fn read_events_cursor(conn: &Connection, project_id: &str) -> anyhow::Result<EventsCursor> {
    let row = conn
        .query_row(
            "SELECT last_rowid, last_ts FROM events_watermark WHERE project_id = ?1",
            [project_id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .optional()?;
    Ok(match row {
        Some((rowid, ts)) => EventsCursor { rowid, ts },
        None => EventsCursor { rowid: 0, ts: None },
    })
}

/// Advance a project's event cursor. Written only AFTER a successful commit, so
/// a crash in between re-runs the batch instead of skipping it.
pub fn write_events_cursor(
    conn: &Connection,
    project_id: &str,
    rowid: i64,
    ts: Option<&str>,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO events_watermark (project_id, last_rowid, last_ts) VALUES (?1, ?2, ?3)
         ON CONFLICT(project_id) DO UPDATE SET last_rowid = ?2, last_ts = ?3",
        rusqlite::params![project_id, rowid, ts],
    )?;
    Ok(())
}
```

Extend `clear_index_watermark` to cover the new table:

```rust
pub fn clear_index_watermark(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "DELETE FROM turns_meta; DELETE FROM index_watermark; DELETE FROM events_watermark;",
    )?;
    Ok(())
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p edda-search-fts 2>&1 | tail -8`
Expected: PASS — all schema tests green, including the three new ones.

- [ ] **Step 6: Commit**

```bash
git add crates/edda-search-fts/src/schema.rs
git commit -m "feat(search): events_watermark cursor storage (GH-403)

Adds a per-project ledger-rowid cursor for the event index, plus accessors.
clear_index_watermark now clears it too, so the existing fresh-index reset keeps
covering the whole cursor set.

The meta DDL was duplicated verbatim between the on-disk and in-memory builders;
extracted to one META_DDL const so the test DB cannot drift from the real one."
```

---

### Task 2: Idempotent incremental event indexing

**Files:**
- Modify: `crates/edda-search-fts/src/indexer.rs:13-37`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `pub fn index_events_since(writer: &IndexWriter, schema: &Schema, project_id: &str, events: &[(i64, edda_core::Event)]) -> anyhow::Result<usize>`

`index_events` is left in place for now — Task 4 removes it once its last caller is gone, so every task compiles green.

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `crates/edda-search-fts/src/indexer.rs`:

```rust
    fn mk_test_event(id: &str) -> edda_core::Event {
        edda_core::Event {
            event_id: id.to_string(),
            ts: "2026-07-15T12:00:00Z".to_string(),
            event_type: "note".to_string(),
            branch: "main".to_string(),
            parent_hash: None,
            hash: "h".to_string(),
            payload: serde_json::json!({ "text": "hello world" }),
            refs: Default::default(),
            schema_version: 1,
            digests: Vec::new(),
            event_family: None,
            event_level: None,
        }
    }

    #[test]
    fn index_events_since_is_idempotent_per_event() {
        let index = ensure_index_ram().unwrap();
        let schema = index.schema();
        let mut writer = index_writer(&index).unwrap();

        let batch = vec![(1i64, mk_test_event("evt_a")), (2i64, mk_test_event("evt_b"))];

        let n = index_events_since(&writer, &schema, "p1", &batch).unwrap();
        writer.commit().unwrap();
        assert_eq!(n, 2);

        // Re-running the same batch is what happens after a crash between commit
        // and the cursor write. It must replace, not duplicate.
        index_events_since(&writer, &schema, "p1", &batch).unwrap();
        writer.commit().unwrap();

        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        assert_eq!(reader.searcher().num_docs(), 2, "re-run must not duplicate docs");
    }

    #[test]
    fn index_events_since_appends_without_touching_existing() {
        let index = ensure_index_ram().unwrap();
        let schema = index.schema();
        let mut writer = index_writer(&index).unwrap();

        index_events_since(&writer, &schema, "p1", &[(1i64, mk_test_event("evt_a"))]).unwrap();
        writer.commit().unwrap();

        // An incremental batch must not delete docs outside it — the whole point
        // of dropping the old delete-all.
        index_events_since(&writer, &schema, "p1", &[(2i64, mk_test_event("evt_b"))]).unwrap();
        writer.commit().unwrap();

        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        assert_eq!(reader.searcher().num_docs(), 2);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p edda-search-fts index_events_since 2>&1 | tail -12`
Expected: FAIL — `cannot find function 'index_events_since' in this scope`.

- [ ] **Step 3: Implement `index_events_since`**

In `crates/edda-search-fts/src/indexer.rs`, add directly after `index_events`:

```rust
/// Index a batch of `(rowid, Event)` pairs incrementally (GH-403).
///
/// Each event is replaced rather than blindly added: `delete_term` on its
/// `doc_id` first, then re-add. That makes re-running a batch a no-op in effect,
/// which is what allows the caller to commit before advancing its cursor — a
/// crash in between simply re-runs this batch on the next pass.
///
/// Unlike the bulk path this deletes nothing outside the batch, so callers must
/// pass only events the index has not seen.
pub fn index_events_since(
    writer: &IndexWriter,
    schema: &Schema,
    project_id: &str,
    events: &[(i64, edda_core::Event)],
) -> anyhow::Result<usize> {
    let f_doc_id = schema.get_field("doc_id")?;
    let mut count = 0;
    for (_rowid, event) in events {
        writer.delete_term(Term::from_field_text(f_doc_id, event.event_id.as_str()));
        add_event_doc(writer, schema, project_id, event)?;
        count += 1;
    }
    Ok(count)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p edda-search-fts 2>&1 | tail -8`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/edda-search-fts/src/indexer.rs
git commit -m "feat(search): index_events_since — idempotent incremental event indexing (GH-403)

Replaces per event (delete_term on doc_id, then add) instead of the bulk path's
delete-everything-then-re-add. Re-running a batch is therefore harmless, which is
what lets the caller commit before advancing its cursor."
```

---

### Task 3: The `sync` module

The heart of the change. This is where the RED test from the spec lives.

**Files:**
- Create: `crates/edda-search-fts/src/sync.rs`
- Modify: `crates/edda-search-fts/src/lib.rs:1-4`

**Interfaces:**
- Consumes: `schema::{IndexLock, index_is_outdated, open_or_create_index, index_writer, ensure_meta_db, clear_index_watermark, read_events_cursor, write_events_cursor, write_index_version}` (Task 1), `indexer::{index_events_since, index_project, index_session}` (Task 2).
- Produces:
  - `pub struct SyncStats { pub events: usize, pub turns: usize, pub indexed_through: Option<String>, pub rebuilt: bool }`
  - `pub fn sync<F>(proj_dir: &Path, project_id: &str, session_id: Option<&str>, events_after: F) -> anyhow::Result<SyncStats> where F: Fn(i64) -> anyhow::Result<Vec<(i64, edda_core::Event)>>`

Note `Fn`, not `FnOnce` (the spec's sketch said `FnOnce`): detecting a cursor that has run ahead of the ledger requires a second call in the rare broken case.

- [ ] **Step 1: Write the failing tests**

Create `crates/edda-search-fts/src/sync.rs` with the tests only for now (implementation follows in Step 3):

```rust
//! Placeholder — implementation added in Step 3.

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn mk_event(id: &str, ts: &str) -> edda_core::Event {
        edda_core::Event {
            event_id: id.to_string(),
            ts: ts.to_string(),
            event_type: "note".to_string(),
            branch: "main".to_string(),
            parent_hash: None,
            hash: "h".to_string(),
            payload: serde_json::json!({ "text": "hello world" }),
            refs: Default::default(),
            schema_version: 1,
            digests: Vec::new(),
            event_family: None,
            event_level: None,
        }
    }

    /// A fake ledger: a fixed event list plus a call counter, so tests can assert
    /// both what was indexed and how the cursor probe behaved.
    struct FakeLedger {
        events: Vec<(i64, edda_core::Event)>,
        calls: RefCell<Vec<i64>>,
    }

    impl FakeLedger {
        fn new(events: Vec<(i64, edda_core::Event)>) -> Self {
            Self { events, calls: RefCell::new(Vec::new()) }
        }
        fn source(&self) -> impl Fn(i64) -> anyhow::Result<Vec<(i64, edda_core::Event)>> + '_ {
            move |after: i64| {
                self.calls.borrow_mut().push(after);
                Ok(self
                    .events
                    .iter()
                    .filter(|(r, _)| *r > after)
                    .cloned()
                    .collect())
            }
        }
    }

    /// THE RED TEST: the executable form of the 24.7s measurement in the spec.
    #[test]
    fn second_sync_over_unchanged_ledger_indexes_zero_events() {
        let tmp = tempfile::tempdir().unwrap();
        let led = FakeLedger::new(vec![
            (1, mk_event("evt_a", "2026-07-15T12:00:00Z")),
            (2, mk_event("evt_b", "2026-07-15T12:01:00Z")),
        ]);

        let first = sync(tmp.path(), "p1", None, led.source()).unwrap();
        assert_eq!(first.events, 2);
        assert_eq!(first.indexed_through.as_deref(), Some("2026-07-15T12:01:00Z"));

        let second = sync(tmp.path(), "p1", None, led.source()).unwrap();
        assert_eq!(second.events, 0, "unchanged ledger must not re-index");
        // Still reports where the index reached, even having done nothing.
        assert_eq!(second.indexed_through.as_deref(), Some("2026-07-15T12:01:00Z"));
    }

    #[test]
    fn sync_indexes_only_new_events() {
        let tmp = tempfile::tempdir().unwrap();
        let led = FakeLedger::new(vec![(1, mk_event("evt_a", "2026-07-15T12:00:00Z"))]);
        assert_eq!(sync(tmp.path(), "p1", None, led.source()).unwrap().events, 1);

        let led2 = FakeLedger::new(vec![
            (1, mk_event("evt_a", "2026-07-15T12:00:00Z")),
            (2, mk_event("evt_b", "2026-07-15T12:01:00Z")),
        ]);
        let stats = sync(tmp.path(), "p1", None, led2.source()).unwrap();
        assert_eq!(stats.events, 1, "only the new event");
        assert!(!stats.rebuilt);
    }

    #[test]
    fn cursor_ahead_of_ledger_triggers_full_rebuild() {
        let tmp = tempfile::tempdir().unwrap();
        let led = FakeLedger::new(vec![
            (1, mk_event("evt_a", "2026-07-15T12:00:00Z")),
            (2, mk_event("evt_b", "2026-07-15T12:01:00Z")),
        ]);
        sync(tmp.path(), "p1", None, led.source()).unwrap();

        // Ledger rebuilt underneath us: rowids reset, cursor now points past the
        // end. Leaving it alone would hide every event forever — the GH-403 bug.
        let rebuilt_ledger = FakeLedger::new(vec![(1, mk_event("evt_z", "2026-07-15T13:00:00Z"))]);
        let stats = sync(tmp.path(), "p1", None, rebuilt_ledger.source()).unwrap();

        assert!(stats.rebuilt, "must detect the cursor is ahead");
        assert_eq!(stats.events, 1, "must re-index from scratch");
        assert_eq!(stats.indexed_through.as_deref(), Some("2026-07-15T13:00:00Z"));
    }

    #[test]
    fn wiped_index_with_surviving_meta_db_rebuilds() {
        let tmp = tempfile::tempdir().unwrap();
        let led = FakeLedger::new(vec![
            (1, mk_event("evt_a", "2026-07-15T12:00:00Z")),
            (2, mk_event("evt_b", "2026-07-15T12:01:00Z")),
        ]);
        sync(tmp.path(), "p1", None, led.source()).unwrap();

        // A missing tantivy dir does NOT imply a missing cursor: meta.sqlite
        // outlives it. created_fresh must clear the cursor, or every event stays
        // invisible behind a watermark that describes an index that is gone.
        std::fs::remove_dir_all(tmp.path().join("search").join("tantivy")).unwrap();

        let stats = sync(tmp.path(), "p1", None, led.source()).unwrap();
        assert!(stats.rebuilt);
        assert_eq!(stats.events, 2, "fresh index must re-take every event");

        let index = crate::schema::open_index(&tmp.path().join("search").join("tantivy")).unwrap();
        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        assert_eq!(reader.searcher().num_docs(), 2);
    }

    #[test]
    fn empty_ledger_syncs_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        let led = FakeLedger::new(vec![]);
        let stats = sync(tmp.path(), "p1", None, led.source()).unwrap();
        assert_eq!(stats.events, 0);
        assert_eq!(stats.indexed_through, None);
    }

    #[test]
    fn crash_between_commit_and_cursor_write_recovers_without_duplicates() {
        let tmp = tempfile::tempdir().unwrap();
        let led = FakeLedger::new(vec![
            (1, mk_event("evt_a", "2026-07-15T12:00:00Z")),
            (2, mk_event("evt_b", "2026-07-15T12:01:00Z")),
        ]);
        sync(tmp.path(), "p1", None, led.source()).unwrap();

        // Simulate the crash window: docs are committed, but the cursor never
        // advanced. The next run must re-run the batch harmlessly.
        let meta = crate::schema::ensure_meta_db(&tmp.path().join("search").join("meta.sqlite")).unwrap();
        crate::schema::write_events_cursor(&meta, "p1", 0, None).unwrap();
        drop(meta);

        let stats = sync(tmp.path(), "p1", None, led.source()).unwrap();
        assert_eq!(stats.events, 2, "re-runs the batch");

        let index = crate::schema::open_index(&tmp.path().join("search").join("tantivy")).unwrap();
        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        assert_eq!(reader.searcher().num_docs(), 2, "must not duplicate");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Add `pub mod sync;` to `crates/edda-search-fts/src/lib.rs` so the module compiles:

```rust
pub mod indexer;
pub mod schema;
pub mod search;
pub mod sync;
pub mod tokenizer;
```

Run: `cargo test -p edda-search-fts sync:: 2>&1 | tail -12`
Expected: FAIL — `cannot find function 'sync' in this scope`.

- [ ] **Step 3: Implement `sync`**

Replace the placeholder line at the top of `crates/edda-search-fts/src/sync.rs` (keep the `mod tests` block) with:

```rust
//! One home for search index orchestration (GH-403).
//!
//! Three callers need identical behaviour — the manual `edda search index`, the
//! query-time cold build, and the SessionEnd background task — so the lock /
//! open / cursor / commit dance lives here exactly once rather than being
//! re-implemented per call site.
//!
//! Events arrive through an injected closure, keeping this crate unaware of
//! `edda-ledger` (the same inversion `index_events` already used).

use crate::{indexer, schema};
use std::path::Path;

/// What a sync did. `indexed_through` is the timestamp of the newest indexed
/// event — reported even when this run indexed nothing, so callers can always
/// tell the user how current the index is.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncStats {
    pub events: usize,
    pub turns: usize,
    pub indexed_through: Option<String>,
    pub rebuilt: bool,
}

/// Bring a project's search index up to date with its ledger and transcripts.
///
/// `events_after` is `|rowid| ledger.events_after_rowid(rowid)`. It is `Fn`
/// rather than `FnOnce` because a cursor that has run ahead of the ledger can
/// only be detected by probing, then re-reading from the start.
///
/// The cursor is owned here: callers never pass or reset it. `sync` resets to a
/// full rebuild when the index was created fresh (schema upgrade, corruption,
/// crash mid-wipe) or when the stored cursor no longer matches the ledger.
pub fn sync<F>(
    proj_dir: &Path,
    project_id: &str,
    session_id: Option<&str>,
    events_after: F,
) -> anyhow::Result<SyncStats>
where
    F: Fn(i64) -> anyhow::Result<Vec<(i64, edda_core::Event)>>,
{
    let search_dir = proj_dir.join("search");
    let index_dir = search_dir.join("tantivy");
    let meta_db_path = search_dir.join("meta.sqlite");

    // Serialize against any other indexer touching this project (GH-402).
    let _lock = schema::IndexLock::acquire(&search_dir)?;

    // Schema upgrade: wipe so it is recreated fresh below.
    if schema::index_is_outdated(&index_dir) {
        std::fs::remove_dir_all(&index_dir)?;
    }

    let (index, created_fresh) = schema::open_or_create_index(&index_dir)?;
    let tantivy_schema = index.schema();
    let mut writer = schema::index_writer(&index)?;
    let meta_conn = schema::ensure_meta_db(&meta_db_path)?;

    // A fresh tantivy index means every watermark is a lie — including the event
    // cursor. Clearing before reading is what makes a missing index dir safe
    // even when meta.sqlite outlived it.
    if created_fresh {
        schema::clear_index_watermark(&meta_conn)?;
    }

    let cursor = schema::read_events_cursor(&meta_conn, project_id)?;
    let (batch, rebuilt) = resolve_batch(&events_after, cursor.rowid)?;

    let events = indexer::index_events_since(&writer, &tantivy_schema, project_id, &batch)?;

    // A rebuild must cover every session, otherwise sessions other than the
    // requested one vanish behind the fresh index.
    let turns = match session_id {
        Some(sid) if !rebuilt => indexer::index_session(
            &writer,
            &tantivy_schema,
            &meta_conn,
            proj_dir,
            project_id,
            sid,
        )?,
        _ => indexer::index_project(&writer, &tantivy_schema, &meta_conn, proj_dir, project_id)?,
    };

    // Commit BEFORE advancing the cursor. A crash in between re-runs this batch,
    // which index_events_since makes idempotent. The reverse order would mark
    // events indexed that are not, hiding them forever.
    writer.commit()?;
    schema::write_index_version(&index_dir)?;

    let indexed_through = if let Some((rowid, ev)) = batch.last() {
        schema::write_events_cursor(&meta_conn, project_id, *rowid, Some(ev.ts.as_str()))?;
        Some(ev.ts.clone())
    } else if rebuilt {
        // Rebuilt against an empty ledger: clear a stale-ahead cursor rather
        // than letting it survive.
        schema::write_events_cursor(&meta_conn, project_id, 0, None)?;
        None
    } else {
        cursor.ts.clone()
    };

    Ok(SyncStats { events, turns, indexed_through, rebuilt })
}

/// Choose the events to index and report whether this is a full rebuild.
///
/// Probes at `cursor - 1` rather than `cursor` so the boundary event itself
/// comes back: if it is gone, the ledger was rebuilt or truncated under a
/// surviving cursor. Trusting the cursor there would make every event invisible
/// — the exact failure GH-403 exists to remove — so we rebuild instead. The
/// probe costs one extra event on the hot path and a second read only in the
/// rare broken case.
fn resolve_batch<F>(
    events_after: &F,
    cursor: i64,
) -> anyhow::Result<(Vec<(i64, edda_core::Event)>, bool)>
where
    F: Fn(i64) -> anyhow::Result<Vec<(i64, edda_core::Event)>>,
{
    if cursor <= 0 {
        return Ok((events_after(0)?, true));
    }
    let probe = events_after(cursor - 1)?;
    match probe.first() {
        // Boundary event still there: it is already indexed, so skip it.
        Some((rowid, _)) if *rowid == cursor => Ok((probe[1..].to_vec(), false)),
        _ => Ok((events_after(0)?, true)),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p edda-search-fts 2>&1 | tail -8`
Expected: PASS — all sync tests green, including the RED test.

- [ ] **Step 5: Commit**

```bash
git add crates/edda-search-fts/src/sync.rs crates/edda-search-fts/src/lib.rs
git commit -m "feat(search): sync module — incremental index orchestration (GH-403)

One home for lock/open/cursor/commit, shared by the three callers so the dance
is not re-implemented per call site. Events arrive via an injected closure,
keeping this crate unaware of edda-ledger.

Commits before advancing the cursor, so a crash in between re-runs the batch
idempotently rather than marking events indexed that are not. A cursor that has
run ahead of the ledger is detected by probing the boundary event and triggers a
full rebuild."
```

---

### Task 4: Rewire `edda search index` onto sync, retire the bulk path

**Files:**
- Modify: `crates/edda-cli/src/cmd_search.rs:169-240` (the `index` fn)
- Modify: `crates/edda-search-fts/src/indexer.rs:13-37` (remove `index_events`, fix stale comment)

**Interfaces:**
- Consumes: `sync::sync` (Task 3).
- Produces: no new API. `indexer::index_events` ceases to exist.

- [ ] **Step 1: Replace the body of `index`**

In `crates/edda-cli/src/cmd_search.rs`, replace the whole `index` function with:

```rust
/// Execute `edda search index` — build/update the Tantivy index for a project.
pub fn index(repo_root: &Path, project_id: &str, session_id: Option<&str>) -> anyhow::Result<()> {
    let proj_dir = project_dir(project_id);
    if !proj_dir.exists() {
        anyhow::bail!("Project directory not found: {}", proj_dir.display());
    }

    let ledger = Ledger::open(repo_root)?;
    let stats = sync::sync(&proj_dir, project_id, session_id, |after| {
        ledger.events_after_rowid(after)
    })?;

    if stats.rebuilt {
        println!("Rebuilt index from scratch.");
    }
    println!(
        "Indexed {} event(s) + {} turn(s) for project {project_id}",
        stats.events, stats.turns
    );
    Ok(())
}
```

Update the imports at the top of `crates/edda-cli/src/cmd_search.rs`:

```rust
use edda_search_fts::{schema, search, sync};
```

(`indexer` is no longer referenced from this file.)

- [ ] **Step 2: Remove the retired bulk path**

In `crates/edda-search-fts/src/indexer.rs`, delete the entire `index_events` function (from its doc comment through its closing brace).

Then fix the stale claim on `add_event_doc` — it advertises a caller that has never existed:

```rust
/// Add a single ledger event as a Tantivy document.
///
/// Used by `index_events_since`; kept public for direct use in tests.
pub fn add_event_doc(
```

- [ ] **Step 3: Build and run the full test suite**

Run: `cargo build -p edda 2>&1 | tail -5 && cargo test -p edda-search-fts 2>&1 | tail -5`
Expected: clean build (no unused-import warnings — CI runs `-Dwarnings`), all tests PASS.

If `cargo test -p edda-search-fts` reports `index_events` still referenced, a test in `indexer.rs` or `search.rs` still calls the bulk path; port it to `index_events_since` with a `[(1i64, event)]` batch.

- [ ] **Step 4: Commit**

```bash
git add crates/edda-cli/src/cmd_search.rs crates/edda-search-fts/src/indexer.rs
git commit -m "refactor(search): edda search index goes through sync; drop index_events (GH-403)

The bulk path deleted every event doc and re-added all of them on every run —
24.7s on a 2227-event corpus even with nothing new. Its last caller is gone, so
it goes. Full rebuilds are now expressed as a sync with a reset cursor.

Also drops the add_event_doc doc comment's claim of an append-time caller, which
never existed."
```

---

### Task 5: Build-if-missing and staleness honesty on query

**Files:**
- Modify: `crates/edda-cli/src/cmd_search.rs:58-90` (`run_cmd` dispatch), `:94-167` (the `query` fn)

**Interfaces:**
- Consumes: `sync::sync` (Task 3), `schema::read_events_cursor` (Task 1).
- Produces: `query` gains a leading `repo_root: &Path` parameter.

Note a deliberate extension beyond the spec's letter: an **outdated** index also auto-rebuilds here, rather than keeping #402's refusal. Both states are "unusable index, one command away", and a dead-end error is exactly what GH-403 objects to. The message distinguishes the two cases.

- [ ] **Step 1: Thread `repo_root` into `query`**

In `crates/edda-cli/src/cmd_search.rs`, in `run_cmd`, change the `SearchCmd::Query` arm's call:

```rust
            query(
                repo_root,
                pid,
                &q,
                session.as_deref(),
                doc_type.as_deref(),
                event_type.as_deref(),
                exact,
                limit,
            )
```

- [ ] **Step 2: Replace the head of `query` with build-if-missing**

Replace the signature and the opening index-resolution block of `query` (down to and including the `let Some(index) = schema::open_index(&index_dir) else { ... };` block) with:

```rust
/// Execute `edda search <query>` — full-text search over the Tantivy index.
#[allow(clippy::too_many_arguments)]
pub fn query(
    repo_root: &Path,
    project_id: &str,
    query_str: &str,
    session_id: Option<&str>,
    doc_type: Option<&str>,
    event_type: Option<&str>,
    exact: bool,
    limit: usize,
) -> anyhow::Result<()> {
    let proj_dir = project_dir(project_id);
    let index_dir = proj_dir.join("search").join("tantivy");

    // GH-403: an unusable index is not a dead end. Announce, then fix it — a
    // silent 25s stall reads as a hang, and telling the user to go run another
    // command is the complaint this issue exists to answer.
    let missing = !index_dir.exists();
    let outdated = schema::index_is_outdated(&index_dir);
    if missing || outdated {
        if missing {
            println!("No search index — building now (one-time)…");
        } else {
            println!("Search index schema is outdated — rebuilding now (one-time)…");
        }
        let ledger = Ledger::open(repo_root)?;
        let stats = sync::sync(&proj_dir, project_id, None, |after| {
            ledger.events_after_rowid(after)
        })?;
        println!("Indexed {} event(s) + {} turn(s).\n", stats.events, stats.turns);
    }

    let Some(index) = schema::open_index(&index_dir) else {
        eprintln!("Search index could not be opened. Run `edda search index` to rebuild.");
        return Ok(());
    };
```

- [ ] **Step 3: Print the watermark after results**

In `query`, replace the early return for the empty case and add the watermark to both paths. Replace:

```rust
    if results.is_empty() {
        println!("No results found for: {query_str}");
        return Ok(());
    }
```

with:

```rust
    if results.is_empty() {
        println!("No results found for: {query_str}");
        print_watermark(repo_root, &proj_dir, project_id);
        return Ok(());
    }
```

and add at the very end of `query`, replacing its trailing `Ok(())`:

```rust
    print_watermark(repo_root, &proj_dir, project_id);
    Ok(())
}

/// Report how current the index is (GH-403), so silence is never mistaken for
/// absence. Best-effort: a broken watermark must not fail a query that already
/// produced results.
fn print_watermark(repo_root: &Path, proj_dir: &Path, project_id: &str) {
    let meta_path = proj_dir.join("search").join("meta.sqlite");
    let Ok(conn) = schema::ensure_meta_db(&meta_path) else {
        return;
    };
    let Ok(cursor) = schema::read_events_cursor(&conn, project_id) else {
        return;
    };
    let Some(ts) = cursor.ts else {
        return;
    };
    let newer = Ledger::open(repo_root)
        .and_then(|l| l.events_after_rowid(cursor.rowid))
        .map(|v| v.len())
        .unwrap_or(0);
    if newer > 0 {
        println!("  (indexed through {ts}; {newer} newer event(s) not yet indexed)");
    } else {
        println!("  (indexed through {ts})");
    }
}
```

- [ ] **Step 4: Build and test**

Run: `cargo build -p edda 2>&1 | tail -5 && cargo test -p edda 2>&1 | tail -6`
Expected: clean build, tests PASS (ignore the known `secret_guard` timing flake if it appears; re-run it alone to confirm: `cargo test -p edda secret_guard::tests::hot_path_under_ten_ms_for_realistic_length -- --exact`).

- [ ] **Step 5: Commit**

```bash
git add crates/edda-cli/src/cmd_search.rs
git commit -m "feat(search): build-if-missing and staleness watermark on query (GH-403)

A missing or schema-outdated index now announces itself and builds, instead of
telling the user to go run another command — the dead end this issue is about.
Every query reports the timestamp it is indexed through, and the count of newer
events it has not seen, so silence is never mistaken for absence."
```

---

### Task 6: SessionEnd incremental reindex

**Files:**
- Create: `crates/edda-bridge-claude/src/bg_index.rs`
- Modify: `crates/edda-bridge-claude/Cargo.toml` (dependencies)
- Modify: `crates/edda-bridge-claude/src/lib.rs` (module list)
- Modify: `crates/edda-bridge-claude/src/dispatch/session.rs:345-361` (insert before `drop(bg_tx)`)

**Interfaces:**
- Consumes: `edda_search_fts::sync::sync` (Task 3).
- Produces: `pub fn should_run(project_id: &str) -> bool`, `pub fn run_index(project_id: &str, cwd: &str) -> anyhow::Result<()>`

- [ ] **Step 1: Add the dependency**

In `crates/edda-bridge-claude/Cargo.toml`, add to `[dependencies]` beside the other `edda-*` path deps:

```toml
edda-search-fts = { path = "../edda-search-fts", version = "0.2.0" }
```

(No cycle: `edda-search-fts` depends on neither `edda-bridge-claude` nor `edda-ledger`.)

- [ ] **Step 2: Write the failing test**

Create `crates/edda-bridge-claude/src/bg_index.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Both assertions live in ONE test on purpose: they share the process-wide
    // EDDA_STORE_ROOT, and cargo runs tests in parallel threads — as two tests
    // they would clobber each other's store root intermittently.
    #[test]
    fn should_run_only_when_an_index_already_exists() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("EDDA_STORE_ROOT", tmp.path());

        // Cold builds belong to `edda search query`, never to a session's exit.
        assert!(!should_run("p1"));

        let idx = tmp
            .path()
            .join("projects")
            .join("p1")
            .join("search")
            .join("tantivy");
        std::fs::create_dir_all(&idx).unwrap();
        assert!(should_run("p1"));
    }
}
```

Add to `crates/edda-bridge-claude/src/lib.rs`, in the module list beside the other `bg_*` modules:

```rust
pub mod bg_index;
```

Run: `cargo test -p edda-bridge-claude bg_index 2>&1 | tail -8`
Expected: FAIL — `cannot find function 'should_run' in this scope`.

- [ ] **Step 3: Implement `bg_index`**

Put this above the `mod tests` block in `crates/edda-bridge-claude/src/bg_index.rs`:

```rust
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
//!   `edda search query`'s build-if-missing. A first build costs ~25s on a
//!   real corpus, and a session's exit must never pay that.

use anyhow::Result;
use edda_store::project_dir;

/// Run only when there is already an index to top up.
pub fn should_run(project_id: &str) -> bool {
    if std::env::var("EDDA_BG_ENABLED").unwrap_or_else(|_| "1".into()) == "0" {
        return false;
    }
    project_dir(project_id)
        .join("search")
        .join("tantivy")
        .exists()
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p edda-bridge-claude bg_index 2>&1 | tail -6`
Expected: PASS.

- [ ] **Step 5: Wire it into SessionEnd**

In `crates/edda-bridge-claude/src/dispatch/session.rs`, insert immediately before the `// Drop the original sender…` / `drop(bg_tx);` lines:

```rust
    // 2j. Background incremental search reindex (GH-403). Ungated by cooldown:
    // with nothing new this is a cursor read and a no-op commit.
    if crate::bg_index::should_run(project_id) {
        let tx = bg_tx.clone();
        let pid = project_id.to_string();
        let cwd_owned = cwd.to_string();
        std::thread::spawn(move || {
            if let Err(e) = crate::bg_index::run_index(&pid, &cwd_owned) {
                tracing::warn!(error = %e, "search reindex failed");
            }
            let _ = tx.send("bg_index");
        });
        bg_count += 1;
    }
```

- [ ] **Step 6: Build and test the workspace**

Run: `cargo build --workspace 2>&1 | tail -5 && cargo test -p edda-bridge-claude 2>&1 | tail -6`
Expected: clean build, tests PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/edda-bridge-claude/Cargo.toml crates/edda-bridge-claude/src/bg_index.rs crates/edda-bridge-claude/src/lib.rs crates/edda-bridge-claude/src/dispatch/session.rs
git commit -m "feat(search): incremental reindex at SessionEnd (GH-403)

Nothing in the hook lifecycle ever built or updated the index, so the flagship
retrieval feature shipped cold and rotted from the first event onward. SessionEnd
now tops it up incrementally, following the existing bg_* pattern (gated spawn,
mpsc completion, join within the background budget).

No cooldown: with nothing new a sync is a cursor read and a no-op commit, and
gating it would just reintroduce staleness. No cold build: a missing index is
left to query-time build-if-missing, so a session's exit never pays ~25s."
```

---

### Task 7: Live verification against the real corpus

Unit tests cannot answer the question the whole design rests on: is an empty sync actually sub-second on a real corpus? The spec's budget is verified here, on real data, or the SessionEnd hook is not shippable.

**Files:**
- No source changes expected. Fixes land in the task they belong to if this fails.

- [ ] **Step 1: Install the built binary**

`cargo install` fails at the replace step while this session's hooks hold `edda.exe` (Windows cannot replace a running exe). Build to the workspace target and copy:

```bash
cargo build --release -p edda
```

Then, in PowerShell:

```powershell
$src = "C:\ai_agent\edda\target\release\edda.exe"
$dst = "C:\Users\synvoke\.cargo\bin\edda.exe"
foreach ($i in 1..8) {
  try { Copy-Item $src $dst -Force -ErrorAction Stop; "COPY OK on attempt $i"; break }
  catch { "attempt $i locked"; Start-Sleep -Milliseconds 1500 }
}
```

- [ ] **Step 2: Verify the budget — the number that decides shippability**

Against the real corpus (project `043a8e81dcb8fa43a08c8b6e00d6d38b`, ~2227 events), from `C:\ai_project\AI Delivery Foundry`:

```bash
time edda search index          # first run after the change
time edda search index          # second run: nothing new
```

Expected: the **second** run completes in **< 1s** (baseline before this work: 24.7s).
If it does not, the SessionEnd hook is over budget — stop and fix before proceeding.

- [ ] **Step 3: Verify build-if-missing on a cold index**

```bash
mv "C:/Users/synvoke/AppData/Roaming/edda/projects/043a8e81dcb8fa43a08c8b6e00d6d38b/search" "C:/Users/synvoke/AppData/Roaming/edda/projects/043a8e81dcb8fa43a08c8b6e00d6d38b/search.bak"
edda search query "權威事實"
```

Expected: prints `No search index — building now (one-time)…`, builds, then returns the hit. (Restore or discard `search.bak` afterwards; a rebuild reproduces it.)

- [ ] **Step 4: Verify staleness honesty**

```bash
edda note "auto-index watermark probe" --tag test
edda search query "權威事實"
```

Expected: results, followed by `(indexed through <ts>; 1 newer event(s) not yet indexed)`.

- [ ] **Step 5: Verify CJK search still works (GH-402 regression)**

```bash
edda search query "機器推論"
edda search query "洗成權威事實"
edda search query "provenance"
```

Expected: 2 hits, 2 hits, 20 hits — matching the #410 live-verification baseline.

- [ ] **Step 6: Verify SessionEnd actually indexes**

The hook entrypoint is `edda hook claude`, which parses Claude Code's event JSON
from stdin (`hook_event_name`, `session_id`, `transcript_path`, `cwd`); the
project is derived from `cwd`. Use `printf`, never `echo` — Git Bash's `echo`
mangles backslashes in JSON payloads.

```bash
edda note "session-end reindex probe" --tag test
printf '%s' '{"hook_event_name":"SessionEnd","session_id":"verify-403","transcript_path":"","cwd":"C:/ai_project/AI Delivery Foundry"}' | edda hook claude 2>&1 | tail -3
edda search query "session-end reindex probe"
```

Expected: the note is found without any manual `edda search index` — the issue's second acceptance criterion.

- [ ] **Step 7: Record the receipt**

```bash
edda note "GH-403 live verify: empty sync <Ns> (was 24.7s), cold build OK, watermark OK, CJK intact, SessionEnd indexes" --tag verify
```

---

## Wrap-up

- [ ] **Full workspace green:** `cargo fmt --check && cargo clippy --workspace --all-targets 2>&1 | tail -5 && cargo test --workspace 2>&1 | tail -8`
- [ ] **Adversarial review:** hand the diff to codex for a correctness pass (crash-safety of the cursor ordering, the `resolve_batch` probe, and the `Fn` closure's re-read behaviour are the places to press).
- [ ] **PR to `main`** referencing GH-403, with the before/after budget numbers in the body. Do not merge — merge is the operator's gate.
