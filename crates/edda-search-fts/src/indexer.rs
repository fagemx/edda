use anyhow::Context;
use edda_index::{fetch_store_line, IndexRecordV1};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::Path;
use tantivy::schema::*;
use tantivy::{doc, IndexWriter, Term};

/// Index all ledger events into Tantivy.
///
/// Deletes existing event documents first (dedup by rebuild), then re-adds all.
/// Returns the number of documents added.
pub fn index_events<F>(
    writer: &IndexWriter,
    schema: &Schema,
    project_id: &str,
    iter_fn: F,
) -> anyhow::Result<usize>
where
    F: FnOnce() -> anyhow::Result<Vec<edda_core::Event>>,
{
    // Delete all existing event docs to avoid duplicates on re-index
    let f_doc_type = schema.get_field("doc_type")?;
    writer.delete_term(Term::from_field_text(f_doc_type, "event"));

    let events = iter_fn()?;
    let mut count = 0;
    for event in &events {
        add_event_doc(writer, schema, project_id, event)?;
        count += 1;
    }
    Ok(count)
}

/// Add a single ledger event as a Tantivy document.
///
/// Used both by bulk `index_events` and by append-time indexing.
pub fn add_event_doc(
    writer: &IndexWriter,
    schema: &Schema,
    project_id: &str,
    event: &edda_core::Event,
) -> anyhow::Result<()> {
    let f_doc_type = schema.get_field("doc_type")?;
    let f_event_type = schema.get_field("event_type")?;
    let f_branch = schema.get_field("branch")?;
    let f_ts = schema.get_field("ts")?;
    let f_doc_id = schema.get_field("doc_id")?;
    let f_session_id = schema.get_field("session_id")?;
    let f_project_id = schema.get_field("project_id")?;
    let f_title = schema.get_field("title")?;
    let f_body = schema.get_field("body")?;
    let f_tags = schema.get_field("tags")?;
    let f_tokens = schema.get_field("tokens")?;

    let (title, body) = extract_event_title_body(event);
    let tags = extract_event_tags(event);

    writer.add_document(doc!(
        f_doc_type => "event",
        f_event_type => event.event_type.as_str(),
        f_branch => event.branch.as_str(),
        f_ts => event.ts.as_str(),
        f_doc_id => event.event_id.as_str(),
        f_session_id => "",
        f_project_id => project_id,
        f_title => title.as_str(),
        f_body => body.as_str(),
        f_tags => tags.as_str(),
        f_tokens => "",
    ))?;

    Ok(())
}

/// Extract title and body from an event for search indexing.
fn extract_event_title_body(event: &edda_core::Event) -> (String, String) {
    let payload = &event.payload;

    // Decision events: title = key, body = "value — reason"
    if let Some(d) = payload.get("decision") {
        let key = d.get("key").and_then(|v| v.as_str()).unwrap_or("");
        let value = d.get("value").and_then(|v| v.as_str()).unwrap_or("");
        let reason = d.get("reason").and_then(|v| v.as_str()).unwrap_or("");
        let body = if reason.is_empty() {
            value.to_string()
        } else {
            format!("{value} — {reason}")
        };
        return (key.to_string(), body);
    }

    // Commit events: title = first line of text, body = rest
    if event.event_type == "commit" {
        let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let (title, body) = text.split_once('\n').unwrap_or((text, ""));
        return (title.to_string(), body.to_string());
    }

    // Generic: title empty, body = text field
    let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
    (String::new(), text.to_string())
}

/// Extract space-separated tags from event payload.
fn extract_event_tags(event: &edda_core::Event) -> String {
    event
        .payload
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

/// Index a single session's transcript records into Tantivy + turns_meta.
///
/// Reads index records from `<project_dir>/index/<session_id>.jsonl`,
/// fetches raw transcript data from `<project_dir>/transcripts/<session_id>.jsonl`,
/// and creates Tantivy documents + turns_meta entries.
///
/// Returns the number of newly indexed turns.
pub fn index_session(
    writer: &IndexWriter,
    schema: &Schema,
    meta_conn: &Connection,
    project_dir: &Path,
    project_id: &str,
    session_id: &str,
) -> anyhow::Result<usize> {
    let index_path = project_dir
        .join("index")
        .join(format!("{session_id}.jsonl"));
    if !index_path.exists() {
        return Ok(0);
    }

    let store_path = project_dir
        .join("transcripts")
        .join(format!("{session_id}.jsonl"));

    // Read all index records
    let records = edda_index::read_index_tail(&index_path, 100_000, 256 * 1024 * 1024)?;
    if records.is_empty() {
        return Ok(0);
    }

    // Build lookup by uuid
    let by_uuid: HashMap<String, &IndexRecordV1> =
        records.iter().map(|r| (r.uuid.clone(), r)).collect();

    // Collect assistant records
    let assistants: Vec<&IndexRecordV1> = records
        .iter()
        .filter(|r| r.record_type == "assistant")
        .collect();

    let f_doc_type = schema.get_field("doc_type")?;
    let f_event_type = schema.get_field("event_type")?;
    let f_branch = schema.get_field("branch")?;
    let f_ts = schema.get_field("ts")?;
    let f_doc_id = schema.get_field("doc_id")?;
    let f_session_id = schema.get_field("session_id")?;
    let f_project_id = schema.get_field("project_id")?;
    let f_title = schema.get_field("title")?;
    let f_body = schema.get_field("body")?;
    let f_tags = schema.get_field("tags")?;
    let f_tokens = schema.get_field("tokens")?;

    let tx = meta_conn.unchecked_transaction()?;
    let mut count = 0;

    for asst_rec in &assistants {
        // Walk parent chain to find root user prompt
        let mut current_parent = asst_rec.parent_uuid.as_deref();
        let mut user_rec: Option<&IndexRecordV1> = None;
        let mut depth = 0;

        while let Some(parent_id) = current_parent {
            if depth >= 50 {
                break;
            }
            depth += 1;
            let parent = match by_uuid.get(parent_id) {
                Some(r) => r,
                None => break,
            };
            if parent.record_type == "user" {
                if let Ok(raw) =
                    fetch_store_line(&store_path, parent.store_offset, parent.store_len)
                {
                    if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&raw) {
                        let text = extract_user_text(&json);
                        if !text.is_empty() {
                            user_rec = Some(parent);
                            break;
                        }
                    }
                }
                current_parent = parent.parent_uuid.as_deref();
            } else {
                current_parent = parent.parent_uuid.as_deref();
            }
        }

        let user_rec = match user_rec {
            Some(r) => r,
            None => continue,
        };

        let turn_id = format!("{}:{}", user_rec.uuid, asst_rec.uuid);

        // Check if already indexed (via turns_meta)
        let exists: bool = tx
            .query_row(
                "SELECT 1 FROM turns_meta WHERE turn_id = ?1",
                params![turn_id],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if exists {
            continue;
        }

        // Fetch user text
        let user_text = if let Ok(raw) =
            fetch_store_line(&store_path, user_rec.store_offset, user_rec.store_len)
        {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&raw) {
                extract_user_text(&json)
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        // Fetch assistant content
        let (assistant_text, tool_names, tool_commands, file_paths) = if let Ok(raw) =
            fetch_store_line(&store_path, asst_rec.store_offset, asst_rec.store_len)
        {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&raw) {
                extract_assistant_fields(&json)
            } else {
                (String::new(), String::new(), String::new(), String::new())
            }
        } else {
            (String::new(), String::new(), String::new(), String::new())
        };

        let ts = &asst_rec.ts;
        let git_branch = asst_rec.git_branch.as_deref().unwrap_or("");

        let body = if user_text.is_empty() {
            assistant_text.clone()
        } else if assistant_text.is_empty() {
            user_text.clone()
        } else {
            format!("{user_text}\n\n{assistant_text}")
        };

        let tokens = format!("{tool_names} {tool_commands} {file_paths}");

        // Add Tantivy document
        writer.add_document(doc!(
            f_doc_type => "turn",
            f_event_type => "",
            f_branch => git_branch,
            f_ts => ts.as_str(),
            f_doc_id => turn_id.as_str(),
            f_session_id => session_id,
            f_project_id => project_id,
            f_title => "",
            f_body => body.as_str(),
            f_tags => "",
            f_tokens => tokens.as_str(),
        ))?;

        // Insert into turns_meta (for show command byte offsets)
        tx.execute(
            "INSERT INTO turns_meta (turn_id, project_id, session_id, ts, \
             user_uuid, assistant_uuid, \
             user_store_offset, user_store_len, \
             assistant_store_offset, assistant_store_len) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                turn_id,
                project_id,
                session_id,
                ts,
                user_rec.uuid,
                asst_rec.uuid,
                user_rec.store_offset as i64,
                user_rec.store_len as i64,
                asst_rec.store_offset as i64,
                asst_rec.store_len as i64,
            ],
        )
        .context("insert turns_meta")?;

        count += 1;
    }

    tx.commit()?;
    Ok(count)
}

/// Index all sessions for a project.
///
/// Scans `<project_dir>/index/` for `*.jsonl` files and indexes each session.
/// Returns total number of newly indexed turns.
pub fn index_project(
    writer: &IndexWriter,
    schema: &Schema,
    meta_conn: &Connection,
    project_dir: &Path,
    project_id: &str,
) -> anyhow::Result<usize> {
    let index_dir = project_dir.join("index");
    if !index_dir.exists() {
        return Ok(0);
    }

    let mut total = 0;
    for entry in std::fs::read_dir(&index_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            let session_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if session_id.is_empty() {
                continue;
            }
            match index_session(writer, schema, meta_conn, project_dir, project_id, &session_id) {
                Ok(n) => total += n,
                Err(e) => {
                    eprintln!("warn: indexing session {session_id}: {e}");
                }
            }
        }
    }

    Ok(total)
}

/// Extract user text from a transcript user record.
/// Returns non-empty string only for real user prompts (STRING content).
fn extract_user_text(user_json: &serde_json::Value) -> String {
    let content = match user_json.get("message").and_then(|m| m.get("content")) {
        Some(c) => c,
        None => return String::new(),
    };

    if let Some(s) = content.as_str() {
        return s.to_string();
    }

    if let Some(arr) = content.as_array() {
        let has_tool_result = arr
            .iter()
            .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"));
        if has_tool_result {
            return String::new();
        }
        let texts: Vec<&str> = arr
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                    b.get("text").and_then(|t| t.as_str())
                } else {
                    None
                }
            })
            .collect();
        if !texts.is_empty() {
            return texts.join(" ");
        }
    }

    String::new()
}

/// Extract assistant text, tool names, tool commands, and file paths from a transcript assistant record.
fn extract_assistant_fields(asst_json: &serde_json::Value) -> (String, String, String, String) {
    let mut texts = Vec::new();
    let mut tool_names = Vec::new();
    let mut tool_commands = Vec::new();
    let mut file_paths = Vec::new();

    let content = asst_json.get("message").and_then(|m| m.get("content"));

    if let Some(arr) = content.and_then(|c| c.as_array()) {
        for block in arr {
            let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                        texts.push(text.to_string());
                    }
                }
                "tool_use" => {
                    if let Some(name) = block.get("name").and_then(|v| v.as_str()) {
                        tool_names.push(name.to_string());
                    }
                    let input = block.get("input");
                    if let Some(cmd) = input
                        .and_then(|i| i.get("command"))
                        .and_then(|c| c.as_str())
                    {
                        tool_commands.push(cmd.to_string());
                    }
                    if let Some(fp) = input
                        .and_then(|i| i.get("file_path"))
                        .and_then(|f| f.as_str())
                    {
                        file_paths.push(fp.to_string());
                    }
                }
                _ => {}
            }
        }
    } else if let Some(text) = content.and_then(|c| c.as_str()) {
        texts.push(text.to_string());
    }

    (
        texts.join(" "),
        tool_names.join(" "),
        tool_commands.join(" "),
        file_paths.join(" "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ensure_index_ram, ensure_meta_db_memory, index_writer};

    #[test]
    fn extract_user_text_string_content() {
        let json = serde_json::json!({
            "message": {
                "content": "Hello, what is the status?"
            }
        });
        assert_eq!(extract_user_text(&json), "Hello, what is the status?");
    }

    #[test]
    fn extract_user_text_tool_result_returns_empty() {
        let json = serde_json::json!({
            "message": {
                "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": "ok"}
                ]
            }
        });
        assert_eq!(extract_user_text(&json), "");
    }

    #[test]
    fn extract_assistant_fields_basic() {
        let json = serde_json::json!({
            "message": {
                "content": [
                    {"type": "text", "text": "Let me check."},
                    {
                        "type": "tool_use",
                        "id": "tu1",
                        "name": "Bash",
                        "input": {"command": "cargo test"}
                    },
                    {
                        "type": "tool_use",
                        "id": "tu2",
                        "name": "Read",
                        "input": {"file_path": "/tmp/foo.rs"}
                    }
                ]
            }
        });
        let (text, names, cmds, files) = extract_assistant_fields(&json);
        assert_eq!(text, "Let me check.");
        assert_eq!(names, "Bash Read");
        assert_eq!(cmds, "cargo test");
        assert_eq!(files, "/tmp/foo.rs");
    }

    #[test]
    fn add_event_doc_decision() {
        let index = ensure_index_ram().unwrap();
        let schema = index.schema();
        let mut writer = index_writer(&index).unwrap();

        let event = edda_core::Event {
            event_id: "evt_001".to_string(),
            ts: "2026-02-17T12:00:00Z".to_string(),
            event_type: "note".to_string(),
            branch: "main".to_string(),
            parent_hash: None,
            hash: "abc".to_string(),
            payload: serde_json::json!({
                "text": "db: postgres — need JSONB",
                "tags": ["decision"],
                "decision": {"key": "db.engine", "value": "postgres", "reason": "need JSONB"}
            }),
            refs: Default::default(),
            schema_version: 1,
            digests: Vec::new(),
            event_family: None,
            event_level: None,
        };

        add_event_doc(&writer, &schema, "p1", &event).unwrap();
        writer.commit().unwrap();

        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        assert_eq!(searcher.num_docs(), 1);
    }

    #[test]
    fn index_events_multiple() {
        let index = ensure_index_ram().unwrap();
        let schema = index.schema();
        let mut writer = index_writer(&index).unwrap();

        let events = vec![
            edda_core::Event {
                event_id: "evt_001".to_string(),
                ts: "2026-02-17T12:00:00Z".to_string(),
                event_type: "note".to_string(),
                branch: "main".to_string(),
                parent_hash: None,
                hash: "abc".to_string(),
                payload: serde_json::json!({"text": "hello world", "tags": ["note"]}),
                refs: Default::default(),
                schema_version: 1,
                digests: Vec::new(),
                event_family: None,
                event_level: None,
            },
            edda_core::Event {
                event_id: "evt_002".to_string(),
                ts: "2026-02-17T12:01:00Z".to_string(),
                event_type: "commit".to_string(),
                branch: "main".to_string(),
                parent_hash: None,
                hash: "def".to_string(),
                payload: serde_json::json!({"text": "feat: add search\ndetails here"}),
                refs: Default::default(),
                schema_version: 1,
                digests: Vec::new(),
                event_family: None,
                event_level: None,
            },
        ];

        let count = index_events(&writer, &schema, "p1", || Ok(events)).unwrap();
        writer.commit().unwrap();
        assert_eq!(count, 2);

        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        assert_eq!(searcher.num_docs(), 2);
    }

    #[test]
    fn index_events_dedup_on_reindex() {
        let index = ensure_index_ram().unwrap();
        let schema = index.schema();
        let mut writer = index_writer(&index).unwrap();

        let make_events = || {
            Ok(vec![edda_core::Event {
                event_id: "evt_dedup".to_string(),
                ts: "2026-02-17T12:00:00Z".to_string(),
                event_type: "note".to_string(),
                branch: "main".to_string(),
                parent_hash: None,
                hash: "abc".to_string(),
                payload: serde_json::json!({"text": "hello"}),
                refs: Default::default(),
                schema_version: 1,
                digests: Vec::new(),
                event_family: None,
                event_level: None,
            }])
        };

        // First index
        let count = index_events(&writer, &schema, "p1", make_events).unwrap();
        writer.commit().unwrap();
        assert_eq!(count, 1);

        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        assert_eq!(searcher.num_docs(), 1);

        // Second index — should delete old events then re-add, NOT duplicate
        let count2 = index_events(&writer, &schema, "p1", make_events).unwrap();
        writer.commit().unwrap();
        assert_eq!(count2, 1);

        // Reload reader to see merged segments
        let reader2 = index.reader().unwrap();
        let searcher2 = reader2.searcher();
        assert_eq!(searcher2.num_docs(), 1, "events should not be duplicated on re-index");
    }

    #[test]
    fn index_session_empty_project_dir() {
        let index = ensure_index_ram().unwrap();
        let schema = index.schema();
        let writer = index_writer(&index).unwrap();
        let meta_conn = ensure_meta_db_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let result = index_session(&writer, &schema, &meta_conn, tmp.path(), "p1", "nonexistent");
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn index_session_with_fixture_data() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path();

        // Create index directory
        let index_dir = project_dir.join("index");
        std::fs::create_dir_all(&index_dir).unwrap();

        // Create transcript store
        let transcripts_dir = project_dir.join("transcripts");
        std::fs::create_dir_all(&transcripts_dir).unwrap();

        // Write transcript records
        let user_record = serde_json::json!({
            "type": "user",
            "uuid": "u1",
            "timestamp": "2026-02-14T10:00:00Z",
            "message": {
                "content": "How do I implement FTS5 search?"
            }
        });
        let assistant_record = serde_json::json!({
            "type": "assistant",
            "uuid": "a1",
            "parentUuid": "u1",
            "timestamp": "2026-02-14T10:00:05Z",
            "message": {
                "content": [
                    {"type": "text", "text": "You can use rusqlite with the fts5 feature."},
                    {
                        "type": "tool_use",
                        "id": "tu1",
                        "name": "Bash",
                        "input": {"command": "cargo add rusqlite --features bundled,fts5"}
                    }
                ]
            }
        });

        let store_path = transcripts_dir.join("s1.jsonl");
        let user_line = serde_json::to_string(&user_record).unwrap();
        let asst_line = serde_json::to_string(&assistant_record).unwrap();

        let user_offset = 0u64;
        let user_len = user_line.len() as u64 + 1;
        let asst_offset = user_len;
        let asst_len = asst_line.len() as u64 + 1;

        std::fs::write(&store_path, format!("{user_line}\n{asst_line}\n")).unwrap();

        // Write index records
        let user_index = edda_index::IndexRecordV1 {
            v: 1,
            session_id: "s1".into(),
            uuid: "u1".into(),
            parent_uuid: None,
            record_type: "user".into(),
            ts: "2026-02-14T10:00:00Z".into(),
            git_branch: Some("main".into()),
            cwd: Some("/tmp/project".into()),
            store_offset: user_offset,
            store_len: user_len,
            assistant: None,
            usage: None,
        };
        let asst_index = edda_index::IndexRecordV1 {
            v: 1,
            session_id: "s1".into(),
            uuid: "a1".into(),
            parent_uuid: Some("u1".into()),
            record_type: "assistant".into(),
            ts: "2026-02-14T10:00:05Z".into(),
            git_branch: Some("main".into()),
            cwd: Some("/tmp/project".into()),
            store_offset: asst_offset,
            store_len: asst_len,
            assistant: Some(edda_index::AssistantMeta {
                tool_use_ids: vec!["tu1".into()],
                tool_use_names: vec!["Bash".into()],
                bash_commands: vec!["cargo add rusqlite --features bundled,fts5".into()],
            }),
            usage: None,
        };

        let index_path = index_dir.join("s1.jsonl");
        edda_index::append_index(&index_path, &user_index).unwrap();
        edda_index::append_index(&index_path, &asst_index).unwrap();

        // Index the session
        let index = ensure_index_ram().unwrap();
        let schema = index.schema();
        let mut writer = index_writer(&index).unwrap();
        let meta_conn = ensure_meta_db_memory().unwrap();

        let count =
            index_session(&writer, &schema, &meta_conn, project_dir, "p1", "s1").unwrap();
        assert_eq!(count, 1);
        writer.commit().unwrap();

        // Verify Tantivy has the document
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        assert_eq!(searcher.num_docs(), 1);

        // Verify turns_meta
        let turn_id: String = meta_conn
            .query_row(
                "SELECT turn_id FROM turns_meta WHERE project_id = ?1",
                params!["p1"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(turn_id, "u1:a1");

        // Re-index is idempotent (dedup via turns_meta check)
        let count2 =
            index_session(&writer, &schema, &meta_conn, project_dir, "p1", "s1").unwrap();
        assert_eq!(count2, 0);
    }
}
