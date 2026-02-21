use edda_ledger::Ledger;
use std::collections::HashSet;
use std::path::Path;

/// `edda query <text>` — search workspace decisions and transcripts by keyword.
pub fn execute(
    repo_root: &Path,
    query_str: &str,
    limit: usize,
    json: bool,
    all: bool,
) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let events = ledger.iter_events()?;

    let query_lower = query_str.to_lowercase();

    // --- Source 1: Ledger decisions ---
    let all_decisions: Vec<_> = events
        .iter()
        .rev() // newest first
        .filter(|e| {
            if e.event_type != "note" {
                return false;
            }
            let has_decision_tag = e
                .payload
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().any(|t| t.as_str() == Some("decision")))
                .unwrap_or(false);
            if !has_decision_tag {
                return false;
            }
            let text = e.payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
            text.to_lowercase().contains(&query_lower)
        })
        .collect();

    // Build superseded set from provenance links.
    // NOTE: scoped to query-matched decisions only — if a non-matching decision
    // supersedes a matching one, the matching one still appears as active.
    let superseded: HashSet<&str> = all_decisions
        .iter()
        .flat_map(|e| {
            e.refs
                .provenance
                .iter()
                .filter(|p| p.rel == "supersedes")
                .map(|p| p.target.as_str())
        })
        .collect();

    // Filter: apply supersession unless --all
    let decisions: Vec<_> = all_decisions
        .iter()
        .filter(|e| all || !superseded.contains(e.event_id.as_str()))
        .take(limit)
        .copied()
        .collect();

    // --- Source 2: Transcript turns (FTS5) ---
    let conversations = search_transcripts(repo_root, query_str, limit);

    if decisions.is_empty() && conversations.is_empty() {
        println!("No results for \"{query_str}\".");
        return Ok(());
    }

    if json {
        let mut results: Vec<serde_json::Value> = Vec::new();

        for e in &decisions {
            let (key, value, reason) = extract_decision_fields(e);
            let is_superseded = superseded.contains(e.event_id.as_str());
            results.push(serde_json::json!({
                "source": "decision",
                "event_id": e.event_id,
                "key": key,
                "value": value,
                "reason": reason,
                "ts": e.ts,
                "branch": e.branch,
                "superseded": is_superseded,
            }));
        }

        for r in &conversations {
            results.push(serde_json::json!({
                "source": "conversation",
                "turn_id": r.turn_id,
                "session_id": r.session_id,
                "ts": r.ts,
                "snippet": r.snippet,
                "rank": r.rank,
            }));
        }

        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        if !decisions.is_empty() {
            println!("Decisions ({}):\n", decisions.len(),);
            for e in &decisions {
                let (key, value, reason) = extract_decision_fields(e);
                let date = if e.ts.len() >= 10 { &e.ts[..10] } else { &e.ts };
                let superseded_marker = if superseded.contains(e.event_id.as_str()) {
                    " [superseded]"
                } else {
                    ""
                };

                if key.is_empty() {
                    println!("  [{date}] {value}{superseded_marker}");
                } else {
                    println!("  [{date}] {key} = {value}{superseded_marker}");
                }
                if !reason.is_empty() {
                    println!("    {reason}");
                }
                println!();
            }
        }

        if !conversations.is_empty() {
            println!("Conversations ({}):\n", conversations.len(),);
            for r in &conversations {
                let date = if r.ts.len() >= 10 { &r.ts[..10] } else { &r.ts };
                let sid_short = &r.session_id[..r.session_id.len().min(8)];
                println!("  [{date}] session {sid_short}");
                println!("    {}\n", r.snippet.replace('\n', " "));
            }
        }
    }

    Ok(())
}

/// Extract (key, value, reason) from a decision event.
/// Prefers structured `payload.decision` fields, falls back to text parse.
fn extract_decision_fields(event: &edda_core::Event) -> (String, String, String) {
    if let Some(d) = event.payload.get("decision") {
        let key = d
            .get("key")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let value = d
            .get("value")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let reason = d
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        return (key, value, reason);
    }
    // Fallback: parse flat text
    let text = event
        .payload
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let (k, v, r) = parse_decision_text(text);
    (k.to_string(), v.to_string(), r.to_string())
}

/// Search transcript turns via FTS5. Returns empty vec if index doesn't exist.
fn search_transcripts(
    repo_root: &Path,
    query_str: &str,
    limit: usize,
) -> Vec<edda_search_fts::search::SearchResult> {
    let project_id = edda_store::project_id(repo_root);
    let db_path = edda_store::project_dir(&project_id)
        .join("search")
        .join("fts.sqlite");

    if !db_path.exists() {
        return Vec::new();
    }

    let conn = match edda_search_fts::schema::ensure_db(&db_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    edda_search_fts::search::search(&conn, query_str, Some(&project_id), None, limit)
        .unwrap_or_default()
}

/// Parse decision text `"key: value — reason"` into (key, value, reason).
///
/// Lenient: missing separators degrade gracefully.
/// - No `: ` → key="", value=full text, reason=""
/// - No ` — ` → reason=""
fn parse_decision_text(text: &str) -> (&str, &str, &str) {
    let (key, rest) = match text.split_once(": ") {
        Some((k, r)) => (k, r),
        None => return ("", text, ""),
    };

    let (value, reason) = match rest.split_once(" — ") {
        Some((v, r)) => (v, r),
        None => (rest, ""),
    };

    (key, value, reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_decision_text_full() {
        let (k, v, r) = parse_decision_text("db: postgres — need JSONB for flexible user prefs");
        assert_eq!(k, "db");
        assert_eq!(v, "postgres");
        assert_eq!(r, "need JSONB for flexible user prefs");
    }

    #[test]
    fn parse_decision_text_no_reason() {
        let (k, v, r) = parse_decision_text("db: postgres");
        assert_eq!(k, "db");
        assert_eq!(v, "postgres");
        assert_eq!(r, "");
    }

    #[test]
    fn parse_decision_text_no_key() {
        let (k, v, r) = parse_decision_text("just free text");
        assert_eq!(k, "");
        assert_eq!(v, "just free text");
        assert_eq!(r, "");
    }

    #[test]
    fn parse_decision_text_colon_in_value() {
        let (k, v, r) = parse_decision_text("url: https://example.com — the endpoint");
        assert_eq!(k, "url");
        assert_eq!(v, "https://example.com");
        assert_eq!(r, "the endpoint");
    }

    #[test]
    fn extract_decision_fields_structured() {
        let event = edda_core::Event {
            event_id: "evt_test".to_string(),
            ts: "2026-02-17T12:00:00Z".to_string(),
            event_type: "note".to_string(),
            branch: "main".to_string(),
            parent_hash: None,
            hash: "abc".to_string(),
            payload: serde_json::json!({
                "role": "system",
                "text": "db: postgres — need JSONB",
                "tags": ["decision"],
                "decision": {"key": "db", "value": "postgres", "reason": "need JSONB"}
            }),
            refs: Default::default(),
            schema_version: 1,
            digests: Vec::new(),
            event_family: None,
            event_level: None,
        };
        let (k, v, r) = extract_decision_fields(&event);
        assert_eq!(k, "db");
        assert_eq!(v, "postgres");
        assert_eq!(r, "need JSONB");
    }

    #[test]
    fn extract_decision_fields_fallback() {
        let event = edda_core::Event {
            event_id: "evt_old".to_string(),
            ts: "2026-02-17T12:00:00Z".to_string(),
            event_type: "note".to_string(),
            branch: "main".to_string(),
            parent_hash: None,
            hash: "abc".to_string(),
            payload: serde_json::json!({
                "role": "system",
                "text": "db: postgres — need JSONB",
                "tags": ["decision"]
            }),
            refs: Default::default(),
            schema_version: 1,
            digests: Vec::new(),
            event_family: None,
            event_level: None,
        };
        let (k, v, r) = extract_decision_fields(&event);
        assert_eq!(k, "db");
        assert_eq!(v, "postgres");
        assert_eq!(r, "need JSONB");
    }
}
