use edda_index::fetch_store_line;
use edda_search_fts::{indexer, schema, search};
use edda_store::project_dir;
use std::path::Path;

/// Execute `edda search <query>` — keyword search over FTS5 index.
pub fn query(
    project_id: &str,
    query_str: &str,
    session_id: Option<&str>,
    limit: usize,
) -> anyhow::Result<()> {
    let proj_dir = project_dir(project_id);
    let db_path = proj_dir.join("search").join("fts.sqlite");
    if !db_path.exists() {
        eprintln!("No search index found. Run `edda search index --project {project_id}` first.");
        return Ok(());
    }

    let conn = schema::ensure_db(&db_path)?;
    let results = search::search(&conn, query_str, Some(project_id), session_id, limit)?;

    if results.is_empty() {
        println!("No results found for: {query_str}");
        return Ok(());
    }

    println!("Found {} result(s) for: {query_str}\n", results.len());
    for (i, r) in results.iter().enumerate() {
        println!(
            "  {}. [{}] session={} ts={}",
            i + 1,
            r.turn_id,
            &r.session_id[..r.session_id.len().min(8)],
            r.ts,
        );
        println!("     {}\n", r.snippet.replace('\n', " "));
    }

    Ok(())
}

/// Execute `edda search index` — build/update FTS5 index for a project.
pub fn index(
    project_id: &str,
    session_id: Option<&str>,
) -> anyhow::Result<()> {
    let proj_dir = project_dir(project_id);
    if !proj_dir.exists() {
        anyhow::bail!("Project directory not found: {}", proj_dir.display());
    }

    let db_path = proj_dir.join("search").join("fts.sqlite");
    let conn = schema::ensure_db(&db_path)?;

    let count = if let Some(sid) = session_id {
        indexer::index_session(&conn, &proj_dir, project_id, sid)?
    } else {
        indexer::index_project(&conn, &proj_dir, project_id)?
    };

    println!("Indexed {count} new turn(s) for project {project_id}");
    Ok(())
}

/// Execute `edda search show` — retrieve full turn content by turn_id.
pub fn show(project_id: &str, turn_id: &str) -> anyhow::Result<()> {
    let proj_dir = project_dir(project_id);
    let db_path = proj_dir.join("search").join("fts.sqlite");
    if !db_path.exists() {
        anyhow::bail!("No search index found. Run `edda search index` first.");
    }

    let conn = schema::ensure_db(&db_path)?;
    let meta = match search::get_turn_meta(&conn, turn_id)? {
        Some(m) => m,
        None => {
            println!("Turn not found: {turn_id}");
            return Ok(());
        }
    };

    let store_path = proj_dir
        .join("transcripts")
        .join(format!("{}.jsonl", meta.session_id));

    println!("Turn: {}", meta.turn_id);
    println!("Session: {}", meta.session_id);
    println!("Timestamp: {}", meta.ts.as_deref().unwrap_or("?"));
    println!("---");

    // Fetch and display user message
    if meta.user_store_len > 0 {
        if let Ok(raw) = fetch_store_line(
            &store_path,
            meta.user_store_offset as u64,
            meta.user_store_len as u64,
        ) {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&raw) {
                let text = extract_message_text(&json);
                println!("USER:\n{text}\n---");
            }
        }
    }

    // Fetch and display assistant message
    if meta.assistant_store_len > 0 {
        if let Ok(raw) = fetch_store_line(
            &store_path,
            meta.assistant_store_offset as u64,
            meta.assistant_store_len as u64,
        ) {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&raw) {
                let text = extract_message_text(&json);
                println!("ASSISTANT:\n{text}");
            }
        }
    }

    Ok(())
}

/// Resolve project ID from repo root (convenience for CLI).
pub fn resolve_project_id(repo_root: &Path) -> String {
    edda_store::project_id(repo_root)
}

/// Extract readable text from a transcript message JSON.
fn extract_message_text(json: &serde_json::Value) -> String {
    let content = match json.get("message").and_then(|m| m.get("content")) {
        Some(c) => c,
        None => return String::new(),
    };

    if let Some(s) = content.as_str() {
        return s.to_string();
    }

    if let Some(arr) = content.as_array() {
        let mut parts = Vec::new();
        for block in arr {
            let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                        parts.push(text.to_string());
                    }
                }
                "tool_use" => {
                    let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    let cmd = block
                        .get("input")
                        .and_then(|i| i.get("command"))
                        .and_then(|c| c.as_str());
                    if let Some(c) = cmd {
                        parts.push(format!("[ToolUse: {name} `{c}`]"));
                    } else {
                        parts.push(format!("[ToolUse: {name}]"));
                    }
                }
                "tool_result" => {
                    parts.push("[tool_result]".to_string());
                }
                _ => {}
            }
        }
        return parts.join("\n");
    }

    String::new()
}
