use rusqlite::{params, Connection};
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, QueryParser, RegexQuery, TermQuery};
use tantivy::schema::*;
use tantivy::snippet::SnippetGenerator;
use tantivy::{Index, Term};

/// A single search result from the Tantivy index.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub doc_id: String,
    pub doc_type: String,
    pub event_type: String,
    pub session_id: String,
    pub ts: String,
    pub snippet: String,
    pub rank: f64,
}

/// Search options for filtering results.
#[derive(Debug, Default)]
pub struct SearchOptions<'a> {
    pub project_id: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub doc_type: Option<&'a str>,
    pub event_type: Option<&'a str>,
    pub exact: bool,
}

/// Search the Tantivy index for documents matching the query.
///
/// Supports:
/// - Fuzzy text search (default): tolerates typos with Levenshtein distance 1
/// - Exact match: `options.exact = true` disables fuzzy
/// - Regex: query wrapped in `/pattern/` uses RegexQuery on body field
/// - Field boosting: title matches ranked 5x higher than body
/// - Filtering by doc_type, event_type, project_id, session_id
pub fn search(
    index: &Index,
    query_str: &str,
    options: &SearchOptions,
    limit: usize,
) -> anyhow::Result<Vec<SearchResult>> {
    let schema = index.schema();
    let reader = index.reader()?;
    let searcher = reader.searcher();

    let f_doc_type = schema.get_field("doc_type")?;
    let f_event_type = schema.get_field("event_type")?;
    let f_doc_id = schema.get_field("doc_id")?;
    let f_session_id = schema.get_field("session_id")?;
    let f_project_id = schema.get_field("project_id")?;
    let f_ts = schema.get_field("ts")?;
    let f_title = schema.get_field("title")?;
    let f_body = schema.get_field("body")?;

    // Build the text query
    let text_query: Box<dyn tantivy::query::Query> =
        if query_str.starts_with('/') && query_str.ends_with('/') && query_str.len() > 2 {
            // Regex mode: /pattern/
            let pattern = &query_str[1..query_str.len() - 1];
            Box::new(RegexQuery::from_pattern(pattern, f_body)?)
        } else {
            // Standard text search with field boost
            let mut parser = QueryParser::for_index(index, vec![f_title, f_body]);
            parser.set_field_boost(f_title, 5.0);
            parser.set_field_boost(f_body, 1.0);
            if !options.exact {
                parser.set_field_fuzzy(f_title, true, 1, true);
                parser.set_field_fuzzy(f_body, true, 1, true);
            }
            let parsed = parser.parse_query(query_str)?;
            parsed
        };

    // Build filter queries
    let mut must_clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> = Vec::new();
    must_clauses.push((Occur::Must, text_query));

    if let Some(dt) = options.doc_type {
        must_clauses.push((
            Occur::Must,
            Box::new(TermQuery::new(
                Term::from_field_text(f_doc_type, dt),
                IndexRecordOption::Basic,
            )),
        ));
    }
    if let Some(et) = options.event_type {
        must_clauses.push((
            Occur::Must,
            Box::new(TermQuery::new(
                Term::from_field_text(f_event_type, et),
                IndexRecordOption::Basic,
            )),
        ));
    }
    if let Some(pid) = options.project_id {
        must_clauses.push((
            Occur::Must,
            Box::new(TermQuery::new(
                Term::from_field_text(f_project_id, pid),
                IndexRecordOption::Basic,
            )),
        ));
    }
    if let Some(sid) = options.session_id {
        must_clauses.push((
            Occur::Must,
            Box::new(TermQuery::new(
                Term::from_field_text(f_session_id, sid),
                IndexRecordOption::Basic,
            )),
        ));
    }

    let final_query = if must_clauses.len() == 1 {
        must_clauses.pop().unwrap().1
    } else {
        Box::new(BooleanQuery::from(must_clauses))
    };

    // Execute search
    let top_docs = searcher.search(&final_query, &TopDocs::with_limit(limit))?;

    // Generate snippets from body field
    let snippet_gen = SnippetGenerator::create(&searcher, &final_query, f_body)?;

    let mut results = Vec::new();
    for (score, doc_address) in top_docs {
        let doc = searcher.doc::<tantivy::TantivyDocument>(doc_address)?;
        let get_text = |field: Field| -> String {
            doc.get_first(field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        let snippet = snippet_gen.snippet_from_doc(&doc);
        let snippet_html = snippet.to_html();
        // Convert <b>match</b> to «match» for consistency with old FTS5 output
        let snippet_text = snippet_html
            .replace("<b>", "«")
            .replace("</b>", "»");

        results.push(SearchResult {
            doc_id: get_text(f_doc_id),
            doc_type: get_text(f_doc_type),
            event_type: get_text(f_event_type),
            session_id: get_text(f_session_id),
            ts: get_text(f_ts),
            snippet: snippet_text,
            rank: score as f64,
        });
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

/// Look up turn metadata by turn_id (from SQLite turns_meta table).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ensure_index_ram, ensure_meta_db_memory, index_writer};
    use tantivy::doc;

    fn insert_test_docs(index: &Index) {
        let schema = index.schema();
        let mut writer = index_writer(index).unwrap();

        let f_doc_type = schema.get_field("doc_type").unwrap();
        let f_event_type = schema.get_field("event_type").unwrap();
        let f_branch = schema.get_field("branch").unwrap();
        let f_ts = schema.get_field("ts").unwrap();
        let f_doc_id = schema.get_field("doc_id").unwrap();
        let f_session_id = schema.get_field("session_id").unwrap();
        let f_project_id = schema.get_field("project_id").unwrap();
        let f_title = schema.get_field("title").unwrap();
        let f_body = schema.get_field("body").unwrap();
        let f_tags = schema.get_field("tags").unwrap();
        let f_tokens = schema.get_field("tokens").unwrap();

        // Decision event
        writer
            .add_document(doc!(
                f_doc_type => "event",
                f_event_type => "note",
                f_branch => "main",
                f_ts => "2026-02-14T10:00:00Z",
                f_doc_id => "evt_001",
                f_session_id => "",
                f_project_id => "p1",
                f_title => "db engine",
                f_body => "chose postgres for JSONB support",
                f_tags => "decision",
                f_tokens => "",
            ))
            .unwrap();

        // Transcript turn
        writer
            .add_document(doc!(
                f_doc_type => "turn",
                f_event_type => "",
                f_branch => "main",
                f_ts => "2026-02-14T11:00:00Z",
                f_doc_id => "u1:a1",
                f_session_id => "s1",
                f_project_id => "p1",
                f_title => "",
                f_body => "How to dispatch bridge messages across L1 and L2?",
                f_tags => "",
                f_tokens => "Bash Read cargo test",
            ))
            .unwrap();

        // Another event (commit)
        writer
            .add_document(doc!(
                f_doc_type => "event",
                f_event_type => "commit",
                f_branch => "feat/auth",
                f_ts => "2026-02-15T09:00:00Z",
                f_doc_id => "evt_002",
                f_session_id => "",
                f_project_id => "p1",
                f_title => "feat: add authentication",
                f_body => "JWT-based auth with refresh tokens",
                f_tags => "",
                f_tokens => "",
            ))
            .unwrap();

        writer.commit().unwrap();
    }

    #[test]
    fn search_finds_matching_docs() {
        let index = ensure_index_ram().unwrap();
        insert_test_docs(&index);

        let results = search(&index, "postgres", &SearchOptions::default(), 10).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].doc_id, "evt_001");
    }

    #[test]
    fn search_title_boost_ranks_higher() {
        let index = ensure_index_ram().unwrap();
        insert_test_docs(&index);

        // "authentication" appears in title of evt_002 and nowhere else
        let results = search(&index, "authentication", &SearchOptions::default(), 10).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].doc_id, "evt_002");
    }

    #[test]
    fn search_filter_by_doc_type() {
        let index = ensure_index_ram().unwrap();
        insert_test_docs(&index);

        let opts = SearchOptions {
            doc_type: Some("event"),
            ..Default::default()
        };
        let results = search(&index, "postgres OR dispatch OR authentication", &opts, 10).unwrap();
        // Only events, no turns
        for r in &results {
            assert_eq!(r.doc_type, "event");
        }
    }

    #[test]
    fn search_filter_by_event_type() {
        let index = ensure_index_ram().unwrap();
        insert_test_docs(&index);

        let opts = SearchOptions {
            event_type: Some("commit"),
            ..Default::default()
        };
        let results = search(&index, "authentication", &opts, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].event_type, "commit");
    }

    #[test]
    fn search_no_results() {
        let index = ensure_index_ram().unwrap();
        insert_test_docs(&index);

        let results =
            search(&index, "nonexistent_query_xyz", &SearchOptions::default(), 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_limit_works() {
        let index = ensure_index_ram().unwrap();
        insert_test_docs(&index);

        let results = search(
            &index,
            "postgres OR dispatch OR authentication",
            &SearchOptions::default(),
            1,
        )
        .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_fuzzy_finds_typo() {
        let index = ensure_index_ram().unwrap();
        insert_test_docs(&index);

        // "dispatc" (missing 'h') should find "dispatch" via fuzzy search
        let results = search(&index, "dispatc", &SearchOptions::default(), 10).unwrap();
        assert!(!results.is_empty(), "fuzzy search should find 'dispatch' with typo 'dispatc'");
        assert_eq!(results[0].doc_id, "u1:a1");
    }

    #[test]
    fn search_regex_pattern() {
        let index = ensure_index_ram().unwrap();
        insert_test_docs(&index);

        // Regex: /bridge.*/ should match "bridge messages"
        let results = search(&index, "/bridge.*/", &SearchOptions::default(), 10).unwrap();
        assert!(!results.is_empty(), "regex search should find 'bridge messages'");
    }

    #[test]
    fn search_exact_mode() {
        let index = ensure_index_ram().unwrap();
        insert_test_docs(&index);

        let opts = SearchOptions {
            exact: true,
            ..Default::default()
        };
        // Exact search for "postgres" should find it
        let results = search(&index, "postgres", &opts, 10).unwrap();
        assert!(!results.is_empty());

        // Exact search for "postgre" (typo) should NOT find anything (no fuzzy)
        let results = search(&index, "postgre", &opts, 10).unwrap();
        assert!(results.is_empty(), "exact mode should not use fuzzy matching");
    }

    #[test]
    fn get_turn_meta_found() {
        let conn = ensure_meta_db_memory().unwrap();
        conn.execute(
            "INSERT INTO turns_meta (turn_id, project_id, session_id, ts, \
             user_uuid, assistant_uuid, \
             user_store_offset, user_store_len, \
             assistant_store_offset, assistant_store_len) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params!["t1", "p1", "s1", "2026-02-14T10:00:00Z", "u1", "a1", 0i64, 100i64, 101i64, 200i64],
        )
        .unwrap();

        let meta = get_turn_meta(&conn, "t1").unwrap().unwrap();
        assert_eq!(meta.turn_id, "t1");
        assert_eq!(meta.project_id, "p1");
        assert_eq!(meta.session_id, "s1");
        assert_eq!(meta.user_store_offset, 0);
        assert_eq!(meta.assistant_store_offset, 101);
    }

    #[test]
    fn get_turn_meta_not_found() {
        let conn = ensure_meta_db_memory().unwrap();
        let meta = get_turn_meta(&conn, "nonexistent").unwrap();
        assert!(meta.is_none());
    }
}
