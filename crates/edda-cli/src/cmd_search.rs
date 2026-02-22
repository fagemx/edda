use edda_index::fetch_store_line;
use edda_ledger::Ledger;
use edda_search_fts::{indexer, schema, search};
use edda_store::project_dir;
use std::path::Path;

/// Execute `edda search <query>` — full-text search over Tantivy index.
pub fn query(
    project_id: &str,
    query_str: &str,
    session_id: Option<&str>,
    doc_type: Option<&str>,
    event_type: Option<&str>,
    exact: bool,
    limit: usize,
) -> anyhow::Result<()> {
    let proj_dir = project_dir(project_id);
    let index_dir = proj_dir.join("search").join("tantivy");
    if !index_dir.exists() {
        eprintln!("No search index found. Run `edda search index` first.");
        return Ok(());
    }

    let index = schema::ensure_index(&index_dir)?;
    let opts = search::SearchOptions {
        project_id: Some(project_id),
        session_id,
        doc_type,
        event_type,
        exact,
    };
    let results = search::search(&index, query_str, &opts, limit)?;

    if results.is_empty() {
        println!("No results found for: {query_str}");
        return Ok(());
    }

    println!("Found {} result(s) for: {query_str}\n", results.len());
    for (i, r) in results.iter().enumerate() {
        let type_label = if r.doc_type == "event" {
            format!("[{}]", r.event_type)
        } else {
            "[turn]".to_string()
        };
        let sid_display = if r.session_id.is_empty() {
            String::new()
        } else {
            format!(" session={}", &r.session_id[..r.session_id.len().min(8)])
        };
        println!(
            "  {}. {} {}{} ts={}",
            i + 1,
            type_label,
            r.doc_id,
            sid_display,
            r.ts,
        );
        if !r.snippet.is_empty() {
            println!("     {}\n", r.snippet.replace('\n', " "));
        } else {
            println!();
        }
    }

    Ok(())
}

/// Execute `edda search index` — build/update Tantivy index for a project.
pub fn index(repo_root: &Path, project_id: &str, session_id: Option<&str>) -> anyhow::Result<()> {
    let proj_dir = project_dir(project_id);
    if !proj_dir.exists() {
        anyhow::bail!("Project directory not found: {}", proj_dir.display());
    }

    let index_dir = proj_dir.join("search").join("tantivy");
    let index = schema::ensure_index(&index_dir)?;
    let tantivy_schema = index.schema();
    let mut writer = schema::index_writer(&index)?;

    // Index ledger events
    let ledger = Ledger::open(repo_root)?;
    let event_count = indexer::index_events(&writer, &tantivy_schema, || ledger.iter_events())?;

    // Index transcript turns
    let meta_db_path = proj_dir.join("search").join("meta.sqlite");
    let meta_conn = schema::ensure_meta_db(&meta_db_path)?;
    let turn_count = if let Some(sid) = session_id {
        indexer::index_session(&writer, &tantivy_schema, &meta_conn, &proj_dir, project_id, sid)?
    } else {
        indexer::index_project(&writer, &tantivy_schema, &meta_conn, &proj_dir, project_id)?
    };

    writer.commit()?;

    println!("Indexed {event_count} event(s) + {turn_count} turn(s) for project {project_id}");
    Ok(())
}

/// Execute `edda search show` — retrieve full turn content by turn_id.
pub fn show(project_id: &str, turn_id: &str) -> anyhow::Result<()> {
    let proj_dir = project_dir(project_id);
    let meta_db_path = proj_dir.join("search").join("meta.sqlite");
    if !meta_db_path.exists() {
        anyhow::bail!("No search metadata found. Run `edda search index` first.");
    }

    let meta_conn = schema::ensure_meta_db(&meta_db_path)?;
    let meta = match search::get_turn_meta(&meta_conn, turn_id)? {
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
