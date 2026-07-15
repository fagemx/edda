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
use anyhow::Context;
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
        // GH-418: this wipe is destruction too, and the guard further down cannot
        // see what it destroyed — afterwards the index is empty, so "would this
        // empty a populated index?" trivially passes. The upgrade path is exactly
        // where it matters: the index is thrown away *on purpose* to be rebuilt
        // from the ledger, so if the ledger is gone the wipe destroys the only
        // copy. The ledger is checked first because it is the definitive half and
        // this read only happens on the rare upgrade path.
        if events_after(0)?.is_empty() {
            // An index that will not open has nothing to lose, so a missing one
            // is fine to wipe. But one that opens and cannot be counted is the
            // opposite: we would be deleting an unknown quantity. Assuming zero
            // there fails open on the only guard against silent, unrecoverable
            // loss — it would reproduce GH-418 through the guard instead of
            // around it. So the count propagates rather than defaulting.
            if let Some(old) = schema::open_index(&index_dir) {
                let old_schema = old.schema();
                let existing = event_doc_count(&old, &old_schema).with_context(|| {
                    format!(
                        "refusing to wipe project {project_id}'s outdated index: its \
                         ledger reports no events and the index could not be counted, \
                         so wiping it might destroy the only copy"
                    )
                })?;
                if existing > 0 {
                    return Err(empty_ledger_refusal(
                        project_id,
                        existing,
                        proj_dir,
                        &search_dir,
                    ));
                }
            }
        }
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
    let (batch, rebuilt) = resolve_batch(&events_after, &cursor)?;

    // GH-418: refuse to let an empty ledger empty a populated index.
    //
    // `Ledger::open` creates an empty `ledger.db` when one is missing, so a
    // deleted ledger is indistinguishable from a legitimately empty one. Events
    // are append-only: a ledger that was populated and now reports nothing means
    // the ledger is gone, not that its events were removed. Purging on that
    // reading destroys the only surviving copy of the index, and reports it as a
    // successful rebuild — silently, since at SessionEnd nobody reads the log.
    //
    // Checked BEFORE the delete: a refusal that fires after the documents are
    // gone is not a refusal.
    if rebuilt && batch.is_empty() {
        let existing = event_doc_count(&index, &tantivy_schema)?;
        if existing > 0 {
            return Err(empty_ledger_refusal(
                project_id,
                existing,
                proj_dir,
                &search_dir,
            ));
        }
    }

    // A rebuild must purge events the ledger no longer has. Re-adding the
    // current ones is not enough: a replaced ledger would leave the old
    // documents searchable forever, answering queries with events that no longer
    // exist. (No-op when the index was just created fresh.)
    if rebuilt {
        indexer::delete_all_event_docs(&writer, &tantivy_schema)?;
    }

    let events = indexer::index_events_since(&writer, &tantivy_schema, project_id, &batch)?;

    // A rebuild must cover every session, otherwise sessions other than the
    // requested one vanish behind the fresh index.
    //
    // Nothing here touches SQLite: the turns_meta rows and session offsets come
    // back pending and are flushed only after the commit below (GH-413).
    let pending = match session_id {
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

    // Commit BEFORE recording anything. A crash in between re-runs this batch:
    // index_events_since makes the event re-add idempotent, and the turns are
    // simply not marked done, so the next run picks them up. The reverse order
    // would mark work done that Tantivy never received, hiding it forever.
    writer.commit()?;
    schema::write_index_version(&index_dir)?;

    // Now that the documents are durable, it is safe to say so.
    let turns = pending.flush(&meta_conn)?;

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

/// The refusal both destructive paths share (GH-418): the schema-upgrade wipe,
/// and the rebuild's event purge.
fn empty_ledger_refusal(
    project_id: &str,
    existing: usize,
    proj_dir: &Path,
    search_dir: &Path,
) -> anyhow::Error {
    anyhow::anyhow!(
        "Refusing to rebuild project {project_id}: its ledger reports no events, but \
         the index holds {existing}. Events are append-only, so this almost certainly \
         means the ledger is missing rather than empty — check {}/.edda/ledger.db. \
         Rebuilding would empty the index. To discard it deliberately, delete {}.",
        proj_dir.display(),
        search_dir.display()
    )
}

/// How many event documents the index currently holds.
///
/// Only `doc_type: "event"` — turns live in the same index and are not what a
/// rebuild would purge.
fn event_doc_count(
    index: &tantivy::Index,
    schema: &tantivy::schema::Schema,
) -> anyhow::Result<usize> {
    use tantivy::collector::Count;
    use tantivy::query::TermQuery;
    use tantivy::schema::IndexRecordOption;

    let f_doc_type = schema.get_field("doc_type")?;
    let reader = index.reader()?;
    let searcher = reader.searcher();
    let query = TermQuery::new(
        tantivy::Term::from_field_text(f_doc_type, "event"),
        IndexRecordOption::Basic,
    );
    Ok(searcher.search(&query, &Count)?)
}

/// Choose the events to index and report whether this is a full rebuild.
///
/// Probes at `rowid - 1` rather than `rowid` so the boundary event itself comes
/// back, then checks it is still the *same* event we indexed — matching both its
/// rowid and its timestamp.
///
/// The rowid alone is not identity. A ledger restored from a different backup,
/// or another repo mapped onto this project id, can occupy the same rowid range
/// with entirely different events; a rowid-only check would see the boundary,
/// trust the cursor, and silently skip every event in the replacement forever.
/// Matching the timestamp too means a ledger restored from its *own* backup
/// still compares equal and avoids a needless rebuild.
///
/// Costs one extra event on the hot path, and a second read only in the rare
/// broken case.
fn resolve_batch<F>(
    events_after: &F,
    cursor: &schema::EventsCursor,
) -> anyhow::Result<(Vec<(i64, edda_core::Event)>, bool)>
where
    F: Fn(i64) -> anyhow::Result<Vec<(i64, edda_core::Event)>>,
{
    if cursor.rowid <= 0 {
        return Ok((events_after(0)?, true));
    }
    let probe = events_after(cursor.rowid - 1)?;
    match probe.first() {
        // Same rowid AND same event: already indexed, so skip just it.
        Some((rowid, ev))
            if *rowid == cursor.rowid && cursor.ts.as_deref() == Some(ev.ts.as_str()) =>
        {
            Ok((probe[1..].to_vec(), false))
        }
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
        assert_eq!(
            first.indexed_through.as_deref(),
            Some("2026-07-15T12:01:00Z")
        );

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
        assert_eq!(
            sync(tmp.path(), "p1", None, led.source()).unwrap().events,
            1
        );

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
        assert_eq!(
            stats.indexed_through.as_deref(),
            Some("2026-07-15T13:00:00Z")
        );
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
    fn ledger_replaced_at_the_same_rowids_is_not_trusted() {
        let tmp = tempfile::tempdir().unwrap();
        let led = FakeLedger::new(vec![
            (1, mk_event("evt_a", "2026-07-15T12:00:00Z")),
            (2, mk_event("evt_b", "2026-07-15T12:01:00Z")),
        ]);
        sync(tmp.path(), "p1", None, led.source()).unwrap();

        // A DIFFERENT ledger now occupies the same rowid range — restored from
        // another backup, or another repo mapped onto this project id. The
        // boundary rowid still exists, so validating on rowid alone would trust
        // the cursor and silently skip every event in the replacement.
        let other = FakeLedger::new(vec![
            (1, mk_event("evt_x", "2026-07-15T20:00:00Z")),
            (2, mk_event("evt_y", "2026-07-15T20:01:00Z")),
        ]);
        let stats = sync(tmp.path(), "p1", None, other.source()).unwrap();

        assert!(
            stats.rebuilt,
            "a replaced ledger must not be trusted on rowid alone"
        );
        assert_eq!(stats.events, 2, "the replacement's events must be indexed");
    }

    /// GH-418: an empty ledger must not be allowed to empty a populated index.
    ///
    /// `Ledger::open` creates an empty `ledger.db` when one is missing, so a
    /// deleted ledger looks exactly like a legitimately empty one. The rebuild
    /// path would then delete every event document and report a successful
    /// "rebuild" of nothing. Events are append-only, so a ledger that was
    /// populated and is now empty means the ledger is gone, not that the events
    /// were removed — trusting it destroys the only remaining copy of that index.
    #[test]
    fn an_empty_ledger_must_not_empty_a_populated_index() {
        let tmp = tempfile::tempdir().unwrap();
        let led = FakeLedger::new(vec![
            (1, mk_event("evt_a", "2026-07-15T12:00:00Z")),
            (2, mk_event("evt_b", "2026-07-15T12:01:00Z")),
        ]);
        sync(tmp.path(), "p1", None, led.source()).unwrap();

        // The ledger.db vanished; what remains opens as empty.
        let vanished = FakeLedger::new(vec![]);
        let err = sync(tmp.path(), "p1", None, vanished.source()).unwrap_err();

        assert!(
            err.to_string().contains("would empty"),
            "unhelpful error: {err}"
        );

        // And the index must still be intact — refusing is worthless if the
        // documents were already deleted before the check.
        let index = crate::schema::open_index(&tmp.path().join("search").join("tantivy")).unwrap();
        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        assert_eq!(
            reader.searcher().num_docs(),
            2,
            "the refusal must leave the index untouched"
        );
    }

    /// GH-418, the other destructive step: a schema upgrade wipes the index dir
    /// outright, and that happens BEFORE the emptiness guard. Once the wipe has
    /// run, the guard counts zero event docs and waves the rebuild through — so
    /// the check has to come before the wipe, not just before the delete.
    ///
    /// The upgrade path is exactly when this matters: the index has been thrown
    /// away on purpose, to be rebuilt from the ledger. If the ledger is gone too,
    /// the wipe destroyed the only copy.
    #[test]
    fn an_outdated_index_is_not_wiped_when_the_ledger_has_nothing_to_rebuild_from() {
        let tmp = tempfile::tempdir().unwrap();
        let led = FakeLedger::new(vec![(1, mk_event("evt_a", "2026-07-15T12:00:00Z"))]);
        sync(tmp.path(), "p1", None, led.source()).unwrap();

        // Make the on-disk index look like an older schema version.
        let index_dir = tmp.path().join("search").join("tantivy");
        std::fs::write(index_dir.join("edda_schema_version"), "1").unwrap();

        // ...and the ledger.db vanished, so it opens as empty.
        let vanished = FakeLedger::new(vec![]);
        let err = sync(tmp.path(), "p1", None, vanished.source()).unwrap_err();

        assert!(
            err.to_string().contains("Refusing to rebuild"),
            "unhelpful error: {err}"
        );

        let index =
            crate::schema::open_index(&index_dir).expect("the index dir must survive the refusal");
        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        assert_eq!(
            reader.searcher().num_docs(),
            1,
            "refusing after the wipe is not refusing"
        );
    }

    #[test]
    fn rebuild_purges_events_the_ledger_no_longer_has() {
        let tmp = tempfile::tempdir().unwrap();
        let led = FakeLedger::new(vec![
            (1, mk_event("evt_a", "2026-07-15T12:00:00Z")),
            (2, mk_event("evt_b", "2026-07-15T12:01:00Z")),
        ]);
        sync(tmp.path(), "p1", None, led.source()).unwrap();

        // Ledger replaced with different events. The removed bulk path purged
        // orphans implicitly by deleting every event doc before re-adding; the
        // incremental path must do it explicitly on rebuild, or evt_a/evt_b stay
        // searchable forever.
        //
        // Uses a REPLACEMENT rather than an empty ledger on purpose: an empty one
        // is now refused (GH-418), because it cannot be told apart from a deleted
        // ledger.db. A replacement is also the stronger test — it pins that the
        // old events go AND the new ones arrive.
        let replaced = FakeLedger::new(vec![(1, mk_event("evt_z", "2026-07-15T20:00:00Z"))]);
        let stats = sync(tmp.path(), "p1", None, replaced.source()).unwrap();
        assert!(stats.rebuilt);

        assert_eq!(stats.events, 1);

        // Exactly the replacement: evt_a and evt_b are gone, evt_z is there.
        let index = crate::schema::open_index(&tmp.path().join("search").join("tantivy")).unwrap();
        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        assert_eq!(
            reader.searcher().num_docs(),
            1,
            "orphaned event docs must be purged on rebuild, and only the new one kept"
        );
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
