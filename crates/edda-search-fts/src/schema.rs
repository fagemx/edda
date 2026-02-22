use rusqlite::Connection;
use std::path::Path;
use tantivy::schema::*;
use tantivy::{Index, IndexWriter};

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

    // Full-text searchable fields
    builder.add_text_field("title", TEXT | STORED);
    builder.add_text_field("body", TEXT | STORED);
    builder.add_text_field("tags", TEXT | STORED);
    builder.add_text_field("tokens", TEXT);

    builder.build()
}

/// Open or create a Tantivy index at the given directory.
pub fn ensure_index(index_dir: &Path) -> anyhow::Result<Index> {
    let schema = build_schema();
    if index_dir.exists() {
        // Try to open existing index
        match Index::open_in_dir(index_dir) {
            Ok(index) => return Ok(index),
            Err(_) => {
                // Corrupted or incompatible — rebuild
                std::fs::remove_dir_all(index_dir)?;
            }
        }
    }
    std::fs::create_dir_all(index_dir)?;
    let index = Index::create_in_dir(index_dir, schema)?;
    Ok(index)
}

/// Create an in-memory Tantivy index (for testing).
pub fn ensure_index_ram() -> anyhow::Result<Index> {
    let schema = build_schema();
    let index = Index::create_in_ram(schema);
    Ok(index)
}

/// Create an IndexWriter with a reasonable heap size.
pub fn index_writer(index: &Index) -> anyhow::Result<IndexWriter> {
    // 15 MB heap — small index, single writer
    let writer = index.writer(15_000_000)?;
    Ok(writer)
}

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

    conn.execute_batch(
        "
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
        ",
    )?;

    Ok(conn)
}

/// Open an in-memory SQLite database with turns_meta schema (for testing).
pub fn ensure_meta_db_memory() -> anyhow::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(
        "
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
        ",
    )?;
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
}
