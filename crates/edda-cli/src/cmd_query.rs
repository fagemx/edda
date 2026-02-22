use edda_ledger::Ledger;
use std::collections::HashSet;
use std::path::Path;

struct DecisionResult {
    event_id: String,
    key: String,
    value: String,
    reason: String,
    ts: String,
    branch: String,
    is_superseded: bool,
}

/// `edda query <text>` — search workspace decisions and transcripts by keyword.
pub fn execute(
    repo_root: &Path,
    query_str: &str,
    limit: usize,
    json: bool,
    all: bool,
) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;

    // --- Source 1: Ledger decisions ---
    let decisions = query_decisions_sqlite(&ledger, query_str, limit, all)?;

    // --- Source 2: Transcript turns (FTS5) ---
    let conversations = search_transcripts(repo_root, query_str, limit);

    if decisions.is_empty() && conversations.is_empty() {
        println!("No results for \"{query_str}\".");
        return Ok(());
    }

    if json {
        let mut results: Vec<serde_json::Value> = Vec::new();

        for d in &decisions {
            results.push(serde_json::json!({
                "source": "decision",
                "event_id": d.event_id,
                "key": d.key,
                "value": d.value,
                "reason": d.reason,
                "ts": d.ts,
                "branch": d.branch,
                "superseded": d.is_superseded,
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
            println!("Decisions ({}):\n", decisions.len());
            for d in &decisions {
                let date = if d.ts.len() >= 10 { &d.ts[..10] } else { &d.ts };
                let superseded_marker = if d.is_superseded { " [superseded]" } else { "" };

                if d.key.is_empty() {
                    println!("  [{date}] {}{superseded_marker}", d.value);
                } else {
                    println!("  [{date}] {} = {}{superseded_marker}", d.key, d.value);
                }
                if !d.reason.is_empty() {
                    println!("    {}", d.reason);
                }
                println!();
            }
        }

        if !conversations.is_empty() {
            println!("Conversations ({}):\n", conversations.len());
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

/// SQLite fast path: query the decisions table directly.
fn query_decisions_sqlite(
    ledger: &Ledger,
    query_str: &str,
    limit: usize,
    all: bool,
) -> anyhow::Result<Vec<DecisionResult>> {
    // active_decisions returns only is_active=TRUE with LIKE search on key/value
    let active = ledger.active_decisions(None, Some(query_str))?;
    let mut results: Vec<DecisionResult> = active
        .into_iter()
        .take(limit)
        .map(|r| DecisionResult {
            event_id: r.event_id,
            key: r.key,
            value: r.value,
            reason: r.reason,
            ts: r.ts.unwrap_or_default(),
            branch: r.branch,
            is_superseded: false,
        })
        .collect();

    // If --all, also include superseded decisions (from JSONL fallback path)
    if all {
        // Fall back to event scan for superseded decisions when --all is requested
        let events = ledger.iter_events()?;
        let active_ids: HashSet<String> = results.iter().map(|r| r.event_id.clone()).collect();
        let query_lower = query_str.to_lowercase();
        for e in events.iter().rev() {
            if active_ids.contains(&e.event_id) {
                continue;
            }
            if !is_decision_event(e) {
                continue;
            }
            let text = e.payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
            if !text.to_lowercase().contains(&query_lower) {
                continue;
            }
            let (key, value, reason) = extract_decision_fields(e);
            results.push(DecisionResult {
                event_id: e.event_id.clone(),
                key,
                value,
                reason,
                ts: e.ts.clone(),
                branch: e.branch.clone(),
                is_superseded: true,
            });
            if results.len() >= limit {
                break;
            }
        }
    }

    Ok(results)
}

fn is_decision_event(event: &edda_core::Event) -> bool {
    event.event_type == "note"
        && event
            .payload
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().any(|t| t.as_str() == Some("decision")))
            .unwrap_or(false)
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
