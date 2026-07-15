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

    Ok(SyncStats {
        events,
        turns,
        indexed_through,
        rebuilt,
    })
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
            Self {
                events,
                calls: RefCell::new(Vec::new()),
            }
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
        assert_eq!(
            second.indexed_through.as_deref(),
            Some("2026-07-15T12:01:00Z")
        );
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
        let meta =
            crate::schema::ensure_meta_db(&tmp.path().join("search").join("meta.sqlite")).unwrap();
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
