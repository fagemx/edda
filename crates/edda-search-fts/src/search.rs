use rusqlite::{params, Connection};

/// A single search result from the FTS5 index.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub turn_id: String,
    pub project_id: String,
    pub session_id: String,
    pub ts: String,
    pub snippet: String,
    pub rank: f64,
}

/// Search the FTS5 index for turns matching the query.
///
/// Uses BM25 ranking and snippet extraction. Results are ordered by relevance.
/// Optionally filter by project_id and/or session_id.
pub fn search(
    conn: &Connection,
    query: &str,
    project_id: Option<&str>,
    session_id: Option<&str>,
    limit: usize,
) -> anyhow::Result<Vec<SearchResult>> {
    // Build WHERE clause based on filters
    let (sql, bind_count) = match (project_id, session_id) {
        (Some(_), Some(_)) => (
            "SELECT turn_id, project_id, session_id, ts, \
             snippet(turns_fts, 6, '«', '»', '...', 48), rank \
             FROM turns_fts \
             WHERE turns_fts MATCH ?1 AND project_id = ?2 AND session_id = ?3 \
             ORDER BY rank \
             LIMIT ?4",
            4,
        ),
        (Some(_), None) => (
            "SELECT turn_id, project_id, session_id, ts, \
             snippet(turns_fts, 6, '«', '»', '...', 48), rank \
             FROM turns_fts \
             WHERE turns_fts MATCH ?1 AND project_id = ?2 \
             ORDER BY rank \
             LIMIT ?3",
            3,
        ),
        (None, Some(_)) => (
            "SELECT turn_id, project_id, session_id, ts, \
             snippet(turns_fts, 6, '«', '»', '...', 48), rank \
             FROM turns_fts \
             WHERE turns_fts MATCH ?1 AND session_id = ?2 \
             ORDER BY rank \
             LIMIT ?3",
            3,
        ),
        (None, None) => (
            "SELECT turn_id, project_id, session_id, ts, \
             snippet(turns_fts, 6, '«', '»', '...', 48), rank \
             FROM turns_fts \
             WHERE turns_fts MATCH ?1 \
             ORDER BY rank \
             LIMIT ?2",
            2,
        ),
    };

    let mut stmt = conn.prepare(sql)?;

    let rows = match (project_id, session_id, bind_count) {
        (Some(pid), Some(sid), 4) => {
            stmt.query_map(params![query, pid, sid, limit as i64], map_row)?
        }
        (Some(pid), None, 3) => stmt.query_map(params![query, pid, limit as i64], map_row)?,
        (None, Some(sid), 3) => stmt.query_map(params![query, sid, limit as i64], map_row)?,
        _ => stmt.query_map(params![query, limit as i64], map_row)?,
    };

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Retrieve the metadata for a specific turn (for `search show`).
pub struct TurnMeta {
    pub turn_id: String,
    pub project_id: String,
    pub session_id: String,
    pub ts: Option<String>,
    pub user_uuid: Option<String>,
    pub assistant_uuid: Option<String>,
    pub user_store_offset: i64,
    pub user_store_len: i64,
    pub assistant_store_offset: i64,
    pub assistant_store_len: i64,
}

/// Look up turn metadata by turn_id.
pub fn get_turn_meta(conn: &Connection, turn_id: &str) -> anyhow::Result<Option<TurnMeta>> {
    let mut stmt = conn.prepare(
        "SELECT turn_id, project_id, session_id, ts, \
         user_uuid, assistant_uuid, \
         user_store_offset, user_store_len, \
         assistant_store_offset, assistant_store_len \
         FROM turns_meta WHERE turn_id = ?1",
    )?;

    let result = stmt.query_row(params![turn_id], |row| {
        Ok(TurnMeta {
            turn_id: row.get(0)?,
            project_id: row.get(1)?,
            session_id: row.get(2)?,
            ts: row.get(3)?,
            user_uuid: row.get(4)?,
            assistant_uuid: row.get(5)?,
            user_store_offset: row.get(6)?,
            user_store_len: row.get(7)?,
            assistant_store_offset: row.get(8)?,
            assistant_store_len: row.get(9)?,
        })
    });

    match result {
        Ok(meta) => Ok(Some(meta)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn map_row(row: &rusqlite::Row) -> rusqlite::Result<SearchResult> {
    Ok(SearchResult {
        turn_id: row.get(0)?,
        project_id: row.get(1)?,
        session_id: row.get(2)?,
        ts: row.get(3)?,
        snippet: row.get(4)?,
        rank: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::ensure_db_memory;

    fn insert_test_turn(conn: &Connection, turn_id: &str, user_text: &str, assistant_text: &str) {
        conn.execute(
            "INSERT INTO turns_fts (turn_id, project_id, session_id, ts, git_branch, cwd, \
             user_text, assistant_text, tool_names, tool_commands, file_paths, tokens) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                turn_id,
                "p1",
                "s1",
                "2026-02-14T10:00:00Z",
                "main",
                "/tmp",
                user_text,
                assistant_text,
                "Bash Read",
                "cargo test",
                "/tmp/foo.rs",
                "Bash Read cargo test /tmp/foo.rs main",
            ],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO turns_meta (turn_id, project_id, session_id, ts, \
             user_uuid, assistant_uuid, \
             user_store_offset, user_store_len, \
             assistant_store_offset, assistant_store_len) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                turn_id,
                "p1",
                "s1",
                "2026-02-14T10:00:00Z",
                "u1",
                "a1",
                0i64,
                100i64,
                101i64,
                200i64,
            ],
        )
        .unwrap();
    }

    #[test]
    fn search_finds_matching_turn() {
        let conn = ensure_db_memory().unwrap();
        insert_test_turn(
            &conn,
            "t1",
            "How to implement FTS5?",
            "Use rusqlite with fts5 feature",
        );
        insert_test_turn(
            &conn,
            "t2",
            "What is the weather today?",
            "I cannot check weather",
        );

        let results = search(&conn, "FTS5", None, None, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].turn_id, "t1");
    }

    #[test]
    fn search_with_project_filter() {
        let conn = ensure_db_memory().unwrap();
        insert_test_turn(&conn, "t1", "FTS5 question", "FTS5 answer");

        // Matching project
        let results = search(&conn, "FTS5", Some("p1"), None, 10).unwrap();
        assert_eq!(results.len(), 1);

        // Non-matching project
        let results = search(&conn, "FTS5", Some("p_other"), None, 10).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn search_with_session_filter() {
        let conn = ensure_db_memory().unwrap();
        insert_test_turn(&conn, "t1", "FTS5 question", "FTS5 answer");

        let results = search(&conn, "FTS5", None, Some("s1"), 10).unwrap();
        assert_eq!(results.len(), 1);

        let results = search(&conn, "FTS5", None, Some("s_other"), 10).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn search_limit_works() {
        let conn = ensure_db_memory().unwrap();
        insert_test_turn(&conn, "t1", "rust programming", "rust is great");
        insert_test_turn(&conn, "t2", "rust async", "rust async with tokio");

        let results = search(&conn, "rust", None, None, 1).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_no_results() {
        let conn = ensure_db_memory().unwrap();
        insert_test_turn(&conn, "t1", "hello world", "greetings");

        let results = search(&conn, "nonexistent_query_xyz", None, None, 10).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn get_turn_meta_found() {
        let conn = ensure_db_memory().unwrap();
        insert_test_turn(&conn, "t1", "hello", "world");

        let meta = get_turn_meta(&conn, "t1").unwrap().unwrap();
        assert_eq!(meta.turn_id, "t1");
        assert_eq!(meta.project_id, "p1");
        assert_eq!(meta.session_id, "s1");
        assert_eq!(meta.user_store_offset, 0);
        assert_eq!(meta.assistant_store_offset, 101);
    }

    #[test]
    fn get_turn_meta_not_found() {
        let conn = ensure_db_memory().unwrap();
        let meta = get_turn_meta(&conn, "nonexistent").unwrap();
        assert!(meta.is_none());
    }
}
