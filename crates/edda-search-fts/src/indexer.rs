use anyhow::Context;
use edda_index::{fetch_store_line, IndexRecordV1};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::Path;

/// Index a single session's transcript records into the FTS5 database.
///
/// Reads index records from `<project_dir>/index/<session_id>.jsonl`,
/// fetches raw transcript data from `<project_dir>/transcripts/<session_id>.jsonl`,
/// and inserts turn-level entries into `turns_fts` + `turns_meta`.
///
/// Returns the number of newly indexed turns.
pub fn index_session(
    conn: &Connection,
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

    let tx = conn.unchecked_transaction()?;
    let mut count = 0;

    for asst_rec in &assistants {
        // Build a turn_id from user_uuid + assistant_uuid
        // First, find the root user prompt by walking up the parent chain
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
                // Check if this is a real user prompt (not tool_result)
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

        // Check if already indexed
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
        let cwd = asst_rec.cwd.as_deref().unwrap_or("");

        // Build tokens: concat of key identifiers for exact-match search
        let tokens = format!(
            "{} {} {} {}",
            &tool_names, &tool_commands, &file_paths, git_branch
        );

        // Insert into FTS5
        tx.execute(
            "INSERT INTO turns_fts (turn_id, project_id, session_id, ts, git_branch, cwd, \
             user_text, assistant_text, tool_names, tool_commands, file_paths, tokens) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
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
            ],
        )
        .context("insert turns_fts")?;

        // Insert into turns_meta
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
    conn: &Connection,
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
            match index_session(conn, project_dir, project_id, &session_id) {
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
    use crate::schema::ensure_db_memory;

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
    fn index_session_empty_project_dir() {
        let conn = ensure_db_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let result = index_session(&conn, tmp.path(), "p1", "nonexistent");
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
        let user_len = user_line.len() as u64 + 1; // +newline
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
        let conn = ensure_db_memory().unwrap();
        let count = index_session(&conn, project_dir, "p1", "s1").unwrap();
        assert_eq!(count, 1);

        // Verify FTS5 data
        let fts_count: i64 = conn
            .query_row("SELECT count(*) FROM turns_fts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(fts_count, 1);

        // Verify turns_meta
        let turn_id: String = conn
            .query_row(
                "SELECT turn_id FROM turns_meta WHERE project_id = ?1",
                params!["p1"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(turn_id, "u1:a1");

        // Re-index is idempotent (dedup)
        let count2 = index_session(&conn, project_dir, "p1", "s1").unwrap();
        assert_eq!(count2, 0);
    }
}
