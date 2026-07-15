use crate::tokenizer::{CjkBigramTokenizer, CJK_TOKENIZER};
use fs2::FileExt;
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;
use tantivy::schema::*;
use tantivy::{Index, IndexWriter};

/// Exclusive lock for a project's search index (GH-402), keyed by the index
/// location itself — not the ledger — so two `edda search index` runs targeting
/// the same `--project` serialize even from different working directories.
/// Released on drop.
pub struct IndexLock {
    _file: std::fs::File,
}

impl IndexLock {
    /// Acquire an exclusive, non-blocking lock on `<search_dir>/index.lock`.
    pub fn acquire(search_dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(search_dir)?;
        let lock_path = search_dir.join("index.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        file.try_lock_exclusive().map_err(|_| {
            anyhow::anyhow!(
                "search index is being rebuilt by another process ({})",
                lock_path.display()
            )
        })?;
        Ok(Self { _file: file })
    }
}

/// On-disk index schema version (GH-402). Bump whenever the tokenizer or field
/// layout changes so a stale index is rebuilt rather than mixing tokenizations.
/// v2: CJK bigram tokenizer on all full-text fields.
pub const INDEX_VERSION: u32 = 2;

fn version_file(index_dir: &Path) -> std::path::PathBuf {
    index_dir.join("edda_schema_version")
}

/// Read the schema version marker beside an index dir, or `None` if absent
/// (pre-v2 indexes have no marker and are treated as outdated).
pub fn read_index_version(index_dir: &Path) -> Option<u32> {
    std::fs::read_to_string(version_file(index_dir))
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Write the current [`INDEX_VERSION`] marker beside an index dir.
pub fn write_index_version(index_dir: &Path) -> anyhow::Result<()> {
    std::fs::write(version_file(index_dir), INDEX_VERSION.to_string())?;
    Ok(())
}

/// Whether the on-disk index needs a full rebuild for a schema upgrade.
pub fn index_is_outdated(index_dir: &Path) -> bool {
    index_dir.exists() && read_index_version(index_dir) != Some(INDEX_VERSION)
}

/// Register edda's custom tokenizers on an index. Must be called on every
/// opened or created index so both indexing and `QueryParser` tokenize
/// symmetrically (GH-402).
pub fn register_tokenizers(index: &Index) {
    index
        .tokenizers()
        .register(CJK_TOKENIZER, CjkBigramTokenizer);
}

/// Build the Tantivy schema used for all search documents.
///
/// Fields:
/// - `doc_type`: "event" or "turn" (filterable)
/// - `event_type`: "note", "commit", "merge", etc (filterable)
/// - `branch`: git branch name (filterable)
/// - `ts`: RFC 3339 timestamp (stored only)
/// - `doc_id`: event_id or turn_id (stored)
/// - `session_id`: session UUID (filterable)
/// - `project_id`: project hash (filterable)
/// - `title`: decision key, commit title (TEXT, boosted at query time)
/// - `body`: full text content (TEXT)
/// - `tags`: space-separated event tags (TEXT)
/// - `tokens`: tool names, commands, file paths (TEXT)
pub fn build_schema() -> Schema {
    let mut builder = Schema::builder();

    // Filterable string fields (indexed as single token, stored for retrieval)
    let string_opts = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("raw")
                .set_index_option(IndexRecordOption::Basic),
        )
        .set_stored();

    builder.add_text_field("doc_type", string_opts.clone());
    builder.add_text_field("event_type", string_opts.clone());
    builder.add_text_field("branch", string_opts.clone());
    builder.add_text_field("doc_id", string_opts.clone());
    builder.add_text_field("session_id", string_opts.clone());
    builder.add_text_field("project_id", string_opts.clone());

    // Stored-only field (not indexed)
    builder.add_text_field("ts", STORED);

    // Full-text searchable fields — CJK bigram tokenizer (GH-402) with
    // positions (needed for snippets/phrases).
    let cjk_indexing = TextFieldIndexing::default()
        .set_tokenizer(CJK_TOKENIZER)
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);
    let cjk_stored = TextOptions::default()
        .set_indexing_options(cjk_indexing.clone())
        .set_stored();
    let cjk_unstored = TextOptions::default().set_indexing_options(cjk_indexing);

    builder.add_text_field("title", cjk_stored.clone());
    builder.add_text_field("body", cjk_stored.clone());
    builder.add_text_field("tags", cjk_stored);
    builder.add_text_field("tokens", cjk_unstored);

    builder.build()
}

/// Open an existing index, or create a fresh one. Returns `(index,
/// created_fresh)` where `created_fresh` is `true` whenever a NEW empty index
/// was created — because the dir never existed, was corrupt (wiped here), or
/// was removed by a caller for a schema rebuild (or a crash mid-wipe). The
/// caller MUST then clear the turns watermark so turns re-index against the
/// empty tantivy index; otherwise `turns_meta` would skip them.
///
/// The version marker is NOT written here: the indexer writes it only after a
/// full commit succeeds (see `cmd_search::index`), so an interrupted rebuild
/// leaves no marker and self-heals on the next run.
pub fn open_or_create_index(index_dir: &Path) -> anyhow::Result<(Index, bool)> {
    let schema = build_schema();
    if index_dir.exists() {
        match Index::open_in_dir(index_dir) {
            Ok(index) => {
                register_tokenizers(&index);
                return Ok((index, false));
            }
            Err(_) => {
                // Corrupted or incompatible — wipe and recreate.
                std::fs::remove_dir_all(index_dir)?;
            }
        }
    }
    std::fs::create_dir_all(index_dir)?;
    let index = Index::create_in_dir(index_dir, schema)?;
    register_tokenizers(&index);
    Ok((index, true))
}

/// Open or create a Tantivy index at the given directory (ignoring whether it
/// was freshly created). Use [`open_or_create_index`] on the write path.
pub fn ensure_index(index_dir: &Path) -> anyhow::Result<Index> {
    Ok(open_or_create_index(index_dir)?.0)
}

/// Open an EXISTING index read-only, without creating or wiping it. Returns
/// `None` if the directory is missing or the index cannot be opened (corrupt).
/// Read paths (query, ask) use this so answering a query never deletes the
/// index (GH-402).
pub fn open_index(index_dir: &Path) -> Option<Index> {
    if !index_dir.exists() {
        return None;
    }
    match Index::open_in_dir(index_dir) {
        Ok(index) => {
            register_tokenizers(&index);
            Some(index)
        }
        Err(_) => None,
    }
}

/// Create an in-memory Tantivy index (for testing).
pub fn ensure_index_ram() -> anyhow::Result<Index> {
    let schema = build_schema();
    let index = Index::create_in_ram(schema);
    register_tokenizers(&index);
    Ok(index)
}

/// Create an IndexWriter with a reasonable heap size.
pub fn index_writer(index: &Index) -> anyhow::Result<IndexWriter> {
    // 15 MB heap — small index, single writer
    let writer = index.writer(15_000_000)?;
    Ok(writer)
}

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

/// Open (or create) the SQLite database for turns_meta (byte-offset pointers).
///
/// This is kept alongside Tantivy because `show` needs byte offsets
/// into transcript JSONL files, and Tantivy is not ideal for this.
pub fn ensure_meta_db(db_path: &Path) -> anyhow::Result<Connection> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;

    conn.execute_batch(META_DDL)?;

    Ok(conn)
}

/// Clear the turns watermark and per-turn index metadata so a full rebuild
/// re-indexes every turn instead of skipping ones marked "already indexed".
/// Lives here (beside `ensure_meta_db`) so the meta-DB table names stay in one
/// crate rather than being duplicated by callers (GH-402).
pub fn clear_index_watermark(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "DELETE FROM turns_meta; DELETE FROM index_watermark; DELETE FROM events_watermark;",
    )?;
    Ok(())
}

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

/// Open an in-memory SQLite database with turns_meta schema (for testing).
pub fn ensure_meta_db_memory() -> anyhow::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(META_DDL)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tantivy::doc;

    #[test]
    fn build_schema_has_expected_fields() {
        let schema = build_schema();
        assert!(schema.get_field("doc_type").is_ok());
        assert!(schema.get_field("event_type").is_ok());
        assert!(schema.get_field("branch").is_ok());
        assert!(schema.get_field("ts").is_ok());
        assert!(schema.get_field("doc_id").is_ok());
        assert!(schema.get_field("session_id").is_ok());
        assert!(schema.get_field("project_id").is_ok());
        assert!(schema.get_field("title").is_ok());
        assert!(schema.get_field("body").is_ok());
        assert!(schema.get_field("tags").is_ok());
        assert!(schema.get_field("tokens").is_ok());
    }

    #[test]
    fn roundtrip_add_and_read_document() {
        let index = ensure_index_ram().unwrap();
        let schema = index.schema();
        let mut writer = index_writer(&index).unwrap();

        let doc_type = schema.get_field("doc_type").unwrap();
        let doc_id = schema.get_field("doc_id").unwrap();
        let title = schema.get_field("title").unwrap();
        let body = schema.get_field("body").unwrap();

        writer
            .add_document(doc!(
                doc_type => "event",
                doc_id => "evt_001",
                title => "db engine",
                body => "chose postgres for JSONB support",
            ))
            .unwrap();
        writer.commit().unwrap();

        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        assert_eq!(searcher.num_docs(), 1);
    }

    #[test]
    fn ensure_index_creates_and_reopens() {
        let tmp = tempfile::tempdir().unwrap();
        let index_dir = tmp.path().join("tantivy");

        // Create
        {
            let index = ensure_index(&index_dir).unwrap();
            let mut writer = index_writer(&index).unwrap();
            let schema = index.schema();
            let doc_id = schema.get_field("doc_id").unwrap();
            writer.add_document(doc!(doc_id => "test")).unwrap();
            writer.commit().unwrap();
        }

        // Reopen
        {
            let index = ensure_index(&index_dir).unwrap();
            let reader = index.reader().unwrap();
            let searcher = reader.searcher();
            assert_eq!(searcher.num_docs(), 1);
        }
    }

    #[test]
    fn version_marker_and_fresh_flag_transitions() {
        let tmp = tempfile::tempdir().unwrap();
        let index_dir = tmp.path().join("tantivy");

        // No directory yet → nothing to rebuild.
        assert!(!index_is_outdated(&index_dir));

        // First create reports fresh; with no marker it reads as outdated.
        let (_i1, fresh1) = open_or_create_index(&index_dir).unwrap();
        assert!(fresh1, "a newly created index is fresh");
        assert!(read_index_version(&index_dir).is_none());
        assert!(index_is_outdated(&index_dir), "no marker → outdated");

        // Marking it current clears outdated.
        write_index_version(&index_dir).unwrap();
        assert_eq!(read_index_version(&index_dir), Some(INDEX_VERSION));
        assert!(!index_is_outdated(&index_dir));

        // Reopening an existing valid index is NOT fresh.
        let (_i2, fresh2) = open_or_create_index(&index_dir).unwrap();
        assert!(!fresh2, "reopening an existing index is not fresh");

        // Simulate a crash mid-wipe: the dir vanishes but a caller retries.
        std::fs::remove_dir_all(&index_dir).unwrap();
        let (_i3, fresh3) = open_or_create_index(&index_dir).unwrap();
        assert!(
            fresh3,
            "recreating after a wipe is fresh → caller re-indexes"
        );
    }

    #[test]
    fn ensure_meta_db_memory_creates_tables() {
        let conn = ensure_meta_db_memory().unwrap();

        let count: i64 = conn
            .query_row("SELECT count(*) FROM turns_meta", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);

        let count: i64 = conn
            .query_row("SELECT count(*) FROM index_watermark", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

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
}
