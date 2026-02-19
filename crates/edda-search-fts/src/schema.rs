use rusqlite::Connection;
use std::path::Path;

/// Open (or create) the FTS5 SQLite database and ensure schema is up-to-date.
pub fn ensure_db(db_path: &Path) -> anyhow::Result<Connection> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(db_path)?;

    // Enable WAL mode for better concurrent read performance
    conn.pragma_update(None, "journal_mode", "WAL")?;

    conn.execute_batch(
        "
        CREATE VIRTUAL TABLE IF NOT EXISTS turns_fts USING fts5(
            turn_id,
            project_id,
            session_id,
            ts,
            git_branch,
            cwd,
            user_text,
            assistant_text,
            tool_names,
            tool_commands,
            file_paths,
            tokens,
            tokenize = 'unicode61 remove_diacritics 2'
        );

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

/// Open an in-memory database with the same schema (for testing).
pub fn ensure_db_memory() -> anyhow::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(
        "
        CREATE VIRTUAL TABLE IF NOT EXISTS turns_fts USING fts5(
            turn_id,
            project_id,
            session_id,
            ts,
            git_branch,
            cwd,
            user_text,
            assistant_text,
            tool_names,
            tool_commands,
            file_paths,
            tokens,
            tokenize = 'unicode61 remove_diacritics 2'
        );

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

    #[test]
    fn ensure_db_memory_creates_tables() {
        let conn = ensure_db_memory().unwrap();

        // Verify turns_fts exists (FTS5 tables can be queried)
        let count: i64 = conn
            .query_row("SELECT count(*) FROM turns_fts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);

        // Verify turns_meta exists
        let count: i64 = conn
            .query_row("SELECT count(*) FROM turns_meta", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);

        // Verify index_watermark exists
        let count: i64 = conn
            .query_row("SELECT count(*) FROM index_watermark", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn ensure_db_file_creates_and_reopens() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("search").join("fts.sqlite");

        // First open creates tables
        {
            let conn = ensure_db(&db_path).unwrap();
            conn.execute(
                "INSERT INTO turns_meta (turn_id, project_id, session_id) VALUES (?1, ?2, ?3)",
                rusqlite::params!["t1", "p1", "s1"],
            )
            .unwrap();
        }

        // Reopen preserves data
        {
            let conn = ensure_db(&db_path).unwrap();
            let count: i64 = conn
                .query_row("SELECT count(*) FROM turns_meta", [], |row| row.get(0))
                .unwrap();
            assert_eq!(count, 1);
        }
    }
}
