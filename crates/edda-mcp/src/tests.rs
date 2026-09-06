//! MCP server tests (extracted from lib.rs for the GH-779 length ratchet).

use super::*;
use tempfile::TempDir;

fn setup_workspace() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let paths = edda_ledger::paths::EddaPaths::discover(&root);
    paths.ensure_layout().unwrap();
    edda_ledger::ledger::init_workspace(&paths).unwrap();
    edda_ledger::ledger::init_head(&paths, "main").unwrap();
    edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
    (tmp, root)
}

#[test]
fn server_info_has_tools_and_resources() {
    let (_tmp, root) = setup_workspace();
    let server = EddaServer::new(root);
    let info = server.get_info();
    assert!(info.capabilities.tools.is_some());
    assert!(info.capabilities.resources.is_some());
}

#[test]
fn open_ledger_works_for_valid_workspace() {
    let (_tmp, root) = setup_workspace();
    let server = EddaServer::new(root);
    assert!(server.open_ledger().is_ok());
}

#[test]
fn open_ledger_fails_for_invalid_path() {
    let server = EddaServer::new(PathBuf::from("/nonexistent/path"));
    assert!(server.open_ledger().is_err());
}

// ── GH-651 compat golden fixtures ──

/// GH-651 golden fixture for the `edda_ask` MCP tool response (ledger
/// decision `compat.stable-json-surfaces`; policy page: COMPATIBILITY.md
/// § "Stable `--json` contracts"). Within 0.x, keys may be added, never
/// deleted, renamed, or retyped. The tool returns an `AskResult` rendered
/// as JSON text, so the pinned shape matches `edda ask --json`.
#[tokio::test]
async fn compat_golden_fixture_ask_tool_response_keys_and_types() {
    let (_tmp, root) = setup_workspace();
    let server = EddaServer::new(root);

    server
        .edda_decide(Parameters(DecideParams {
            decision: "db.engine=postgres".to_string(),
            reason: Some("golden fixture".to_string()),
        }))
        .await
        .unwrap();

    let result = server
        .edda_ask(Parameters(AskParams {
            query: Some("db".to_string()),
            context_summary: None,
            limit: None,
            include_superseded: None,
            branch: None,
        }))
        .await
        .unwrap();

    let text = result.content[0].raw.as_text().unwrap().text.as_str();
    let v: serde_json::Value = serde_json::from_str(text).expect("tool returns valid JSON");

    let mut keys: Vec<&str> = v
        .as_object()
        .expect("one JSON object")
        .keys()
        .map(|k| k.as_str())
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "conversations",
            "decisions",
            "input_type",
            "query",
            "related_commits",
            "related_notes",
            "timeline",
            "workspace_decision_count",
            "workspace_event_count",
        ],
        "edda_ask tool response key set changed — this is a stable contract; \
             see COMPATIBILITY.md (tasks/dependents/override_risk and the \
             workspace counts are absent when empty/unknown by the \
             skip_serializing_if contract)"
    );
    // The two workspace counts are integers when the ledger is readable,
    // and absent from the key set above when unknown (#728).
    assert!(
        v["workspace_event_count"].is_u64(),
        "workspace_event_count must be an integer: {v}"
    );
    assert!(
        v["workspace_decision_count"].is_u64(),
        "workspace_decision_count must be an integer: {v}"
    );
    assert!(v["query"].is_string());
    assert!(v["input_type"].is_string());
    for section in [
        "decisions",
        "timeline",
        "related_commits",
        "related_notes",
        "conversations",
    ] {
        assert!(v[section].is_array(), "{section} must be an array: {v}");
    }

    let d = &v["decisions"][0];
    let mut dkeys: Vec<&str> = d
        .as_object()
        .expect("decision object")
        .keys()
        .map(|k| k.as_str())
        .collect();
    dkeys.sort_unstable();
    assert_eq!(
        dkeys,
        vec![
            "branch",
            "domain",
            "event_id",
            "governance",
            "is_active",
            "key",
            "reason",
            "ts",
            "value",
        ],
        "DecisionHit key set changed — this is a stable contract; \
             see COMPATIBILITY.md"
    );
    assert!(d["event_id"].is_string());
    assert_eq!(d["key"], "db.engine");
    assert_eq!(d["value"], "postgres");
    assert_eq!(d["is_active"], serde_json::Value::Bool(true));
    assert!(d["governance"].is_object());
    assert_eq!(d["governance"]["status"], "unratified");
}

/// GH-651 golden fixture for the `edda_tool_tier` MCP tool response —
/// the other JSON-returning MCP tool. Pins the exact key set and types
/// of `ToolTierResult`.
#[tokio::test]
async fn compat_golden_fixture_tool_tier_response_keys_and_types() {
    let (_tmp, root) = setup_workspace();
    let server = EddaServer::new(root);

    let result = server
        .edda_tool_tier(Parameters(ToolTierParams {
            tool_name: "bash".to_string(),
        }))
        .await
        .unwrap();

    let text = result.content[0].raw.as_text().unwrap().text.as_str();
    let v: serde_json::Value = serde_json::from_str(text).expect("tool returns valid JSON");

    let mut keys: Vec<&str> = v
        .as_object()
        .expect("one JSON object")
        .keys()
        .map(|k| k.as_str())
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["approval", "description", "tier", "tool"],
        "edda_tool_tier response key set changed — this is a stable contract; \
             see COMPATIBILITY.md"
    );
    assert_eq!(v["tool"], "bash");
    assert!(v["tier"].is_string(), "tier must be a string: {v}");
    assert!(v["approval"].is_string(), "approval must be a string: {v}");
    assert!(v["description"].is_string());
}

// --- edda_decide tests ---

#[tokio::test]
async fn test_decide_basic() {
    let (_tmp, root) = setup_workspace();
    let server = EddaServer::new(root.clone());

    let result = server
        .edda_decide(Parameters(DecideParams {
            decision: "db.engine=postgres".to_string(),
            reason: Some("JSONB support".to_string()),
        }))
        .await
        .unwrap();

    let text = result.content[0].raw.as_text().unwrap().text.as_str();
    assert!(text.contains("Decision recorded: db.engine = postgres"));
    assert!(text.contains("evt_"));

    // Verify event in ledger
    let ledger = Ledger::open(&root).unwrap();
    let events = ledger.iter_events().unwrap();
    let dec = events.iter().find(|e| {
        e.payload
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().any(|t| t.as_str() == Some("decision")))
            .unwrap_or(false)
    });
    assert!(dec.is_some());
    let dec = dec.unwrap();
    assert_eq!(
        dec.payload["decision"]["key"].as_str().unwrap(),
        "db.engine"
    );
    assert_eq!(
        dec.payload["decision"]["value"].as_str().unwrap(),
        "postgres"
    );
    assert_eq!(
        dec.payload["decision"]["reason"].as_str().unwrap(),
        "JSONB support"
    );
}

#[tokio::test]
async fn test_decide_auto_supersede() {
    let (_tmp, root) = setup_workspace();
    let server = EddaServer::new(root.clone());

    // First decision
    server
        .edda_decide(Parameters(DecideParams {
            decision: "db.engine=sqlite".to_string(),
            reason: None,
        }))
        .await
        .unwrap();

    // Second decision with same key, different value
    let result = server
        .edda_decide(Parameters(DecideParams {
            decision: "db.engine=postgres".to_string(),
            reason: Some("need JSONB".to_string()),
        }))
        .await
        .unwrap();

    let text = result.content[0].raw.as_text().unwrap().text.as_str();
    assert!(text.contains("supersedes"));

    // Verify provenance link in ledger
    let ledger = Ledger::open(&root).unwrap();
    let events = ledger.iter_events().unwrap();
    let last_dec = events
        .iter()
        .rev()
        .find(|e| {
            e.payload
                .get("decision")
                .and_then(|d| d.get("value"))
                .and_then(|v| v.as_str())
                == Some("postgres")
        })
        .unwrap();
    assert_eq!(last_dec.refs.provenance.len(), 1);
    assert_eq!(last_dec.refs.provenance[0].rel, "supersedes");
}

#[tokio::test]
async fn test_decide_idempotent_no_supersede() {
    let (_tmp, root) = setup_workspace();
    let server = EddaServer::new(root.clone());

    // Same key, same value twice — should NOT create supersede link
    server
        .edda_decide(Parameters(DecideParams {
            decision: "db.engine=postgres".to_string(),
            reason: None,
        }))
        .await
        .unwrap();

    let result = server
        .edda_decide(Parameters(DecideParams {
            decision: "db.engine=postgres".to_string(),
            reason: None,
        }))
        .await
        .unwrap();

    let text = result.content[0].raw.as_text().unwrap().text.as_str();
    assert!(!text.contains("supersedes"));

    // Verify no provenance link on second event
    let ledger = Ledger::open(&root).unwrap();
    let events = ledger.iter_events().unwrap();
    let last = events.last().unwrap();
    assert!(last.refs.provenance.is_empty());
}

#[tokio::test]
async fn test_decide_invalid_format() {
    let (_tmp, root) = setup_workspace();
    let server = EddaServer::new(root);

    let result = server
        .edda_decide(Parameters(DecideParams {
            decision: "no-equals-sign".to_string(),
            reason: None,
        }))
        .await;

    assert!(result.is_err());
}

// --- edda_ask tests ---

#[tokio::test]
async fn test_ask_finds_decisions() {
    let (_tmp, root) = setup_workspace();
    let server = EddaServer::new(root);

    server
        .edda_decide(Parameters(DecideParams {
            decision: "db.engine=postgres".to_string(),
            reason: Some("JSONB support".to_string()),
        }))
        .await
        .unwrap();
    server
        .edda_decide(Parameters(DecideParams {
            decision: "auth.method=JWT".to_string(),
            reason: None,
        }))
        .await
        .unwrap();

    let result = server
        .edda_ask(Parameters(AskParams {
            query: Some("postgres".to_string()),
            context_summary: None,
            limit: None,
            include_superseded: None,
            branch: None,
        }))
        .await
        .unwrap();

    let text = result.content[0].raw.as_text().unwrap().text.as_str();
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["input_type"], "keyword");
    assert_eq!(parsed["decisions"].as_array().unwrap().len(), 1);
    assert_eq!(parsed["decisions"][0]["key"], "db.engine");
}

#[tokio::test]
async fn test_ask_empty_returns_all_active() {
    let (_tmp, root) = setup_workspace();
    let server = EddaServer::new(root);

    server
        .edda_decide(Parameters(DecideParams {
            decision: "db.engine=postgres".to_string(),
            reason: None,
        }))
        .await
        .unwrap();
    server
        .edda_decide(Parameters(DecideParams {
            decision: "auth.method=JWT".to_string(),
            reason: None,
        }))
        .await
        .unwrap();

    let result = server
        .edda_ask(Parameters(AskParams {
            query: None,
            context_summary: None,
            limit: None,
            include_superseded: None,
            branch: None,
        }))
        .await
        .unwrap();

    let text = result.content[0].raw.as_text().unwrap().text.as_str();
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["input_type"], "overview");
    assert_eq!(parsed["decisions"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_ask_domain_browse() {
    let (_tmp, root) = setup_workspace();
    let server = EddaServer::new(root);

    server
        .edda_decide(Parameters(DecideParams {
            decision: "db.engine=postgres".to_string(),
            reason: None,
        }))
        .await
        .unwrap();
    server
        .edda_decide(Parameters(DecideParams {
            decision: "db.pool=10".to_string(),
            reason: None,
        }))
        .await
        .unwrap();
    server
        .edda_decide(Parameters(DecideParams {
            decision: "auth.method=JWT".to_string(),
            reason: None,
        }))
        .await
        .unwrap();

    let result = server
        .edda_ask(Parameters(AskParams {
            query: Some("db".to_string()),
            context_summary: None,
            limit: None,
            include_superseded: None,
            branch: None,
        }))
        .await
        .unwrap();

    let text = result.content[0].raw.as_text().unwrap().text.as_str();
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["input_type"], "domain");
    assert_eq!(parsed["decisions"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_ask_no_results() {
    let (_tmp, root) = setup_workspace();
    let server = EddaServer::new(root);

    let result = server
        .edda_ask(Parameters(AskParams {
            query: Some("nonexistent".to_string()),
            context_summary: None,
            limit: None,
            include_superseded: None,
            branch: None,
        }))
        .await
        .unwrap();

    let text = result.content[0].raw.as_text().unwrap().text.as_str();
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    assert!(parsed["decisions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_ask_context_summary_fallback() {
    let (_tmp, root) = setup_workspace();
    let server = EddaServer::new(root);

    server
        .edda_decide(Parameters(DecideParams {
            decision: "pricing.discount_policy=daytime_revenue_shield".to_string(),
            reason: Some("avoid aggressive daytime markdowns".to_string()),
        }))
        .await
        .unwrap();

    let result = server
        .edda_ask(Parameters(AskParams {
            query: None,
            context_summary: Some("daytime discount outcome".to_string()),
            limit: None,
            include_superseded: None,
            branch: None,
        }))
        .await
        .unwrap();

    let text = result.content[0].raw.as_text().unwrap().text.as_str();
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["input_type"], "keyword");
    assert!(parsed["decisions"].is_array());
    assert_eq!(parsed["decisions"][0]["key"], "pricing.discount_policy");
}

// --- edda_log tests ---

#[tokio::test]
async fn test_log_filter_by_type() {
    let (_tmp, root) = setup_workspace();
    let server = EddaServer::new(root);

    // Add a note
    server
        .edda_note(Parameters(NoteParams {
            text: "test note".to_string(),
            role: None,
            tags: None,
        }))
        .await
        .unwrap();

    // Filter by note type — should find the event
    let result = server
        .edda_log(Parameters(LogParams {
            event_type: Some("note".to_string()),
            keyword: None,
            after: None,
            before: None,
            limit: None,
        }))
        .await
        .unwrap();

    let text = result.content[0].raw.as_text().unwrap().text.as_str();
    assert!(text.contains("note"));
    assert!(text.contains("test note"));

    // Filter by non-existent type — should return nothing
    let result = server
        .edda_log(Parameters(LogParams {
            event_type: Some("commit".to_string()),
            keyword: None,
            after: None,
            before: None,
            limit: None,
        }))
        .await
        .unwrap();

    let text = result.content[0].raw.as_text().unwrap().text.as_str();
    assert!(text.contains("No events match"));
}

#[tokio::test]
async fn test_log_filter_by_keyword() {
    let (_tmp, root) = setup_workspace();
    let server = EddaServer::new(root);

    server
        .edda_note(Parameters(NoteParams {
            text: "authentication flow".to_string(),
            role: None,
            tags: None,
        }))
        .await
        .unwrap();

    server
        .edda_note(Parameters(NoteParams {
            text: "database schema".to_string(),
            role: None,
            tags: None,
        }))
        .await
        .unwrap();

    let result = server
        .edda_log(Parameters(LogParams {
            event_type: None,
            keyword: Some("auth".to_string()),
            after: None,
            before: None,
            limit: None,
        }))
        .await
        .unwrap();

    let text = result.content[0].raw.as_text().unwrap().text.as_str();
    assert!(text.contains("authentication"));
    assert!(!text.contains("database"));
}

#[tokio::test]
async fn test_log_date_filter() {
    let (_tmp, root) = setup_workspace();
    let server = EddaServer::new(root);

    server
        .edda_note(Parameters(NoteParams {
            text: "some note".to_string(),
            role: None,
            tags: None,
        }))
        .await
        .unwrap();

    // Filter with future date should show nothing
    let result = server
        .edda_log(Parameters(LogParams {
            event_type: None,
            keyword: None,
            after: Some("2099-01-01".to_string()),
            before: None,
            limit: None,
        }))
        .await
        .unwrap();

    let text = result.content[0].raw.as_text().unwrap().text.as_str();
    assert!(text.contains("No events match"));

    // Filter with past date should show the event
    let result = server
        .edda_log(Parameters(LogParams {
            event_type: None,
            keyword: None,
            after: Some("2020-01-01".to_string()),
            before: None,
            limit: None,
        }))
        .await
        .unwrap();

    let text = result.content[0].raw.as_text().unwrap().text.as_str();
    assert!(text.contains("some note"));
}

// --- edda_draft_inbox tests ---

#[tokio::test]
async fn test_draft_inbox_empty() {
    let (_tmp, root) = setup_workspace();
    let server = EddaServer::new(root);

    let result = server.edda_draft_inbox().await.unwrap();
    let text = result.content[0].raw.as_text().unwrap().text.as_str();
    assert_eq!(text, "No pending items.");
}

#[tokio::test]
async fn test_draft_inbox_with_pending() {
    let (_tmp, root) = setup_workspace();
    let server = EddaServer::new(root.clone());

    // Create a mock draft file
    let drafts_dir = root.join(".edda").join("drafts");
    let draft_json = serde_json::json!({
        "version": 1,
        "draft_id": "drf_test123",
        "title": "Add auth module",
        "status": "proposed",
        "stages": [
            {
                "stage_id": "lead",
                "role": "lead",
                "min_approvals": 1,
                "approved_by": [],
                "status": "pending"
            }
        ]
    });
    std::fs::write(
        drafts_dir.join("drf_test123.json"),
        serde_json::to_string_pretty(&draft_json).unwrap(),
    )
    .unwrap();

    let result = server.edda_draft_inbox().await.unwrap();
    let text = result.content[0].raw.as_text().unwrap().text.as_str();
    assert!(text.contains("drf_test123"));
    assert!(text.contains("Add auth module"));
    assert!(text.contains("stage: lead"));
    assert!(text.contains("approvals: 0/1"));
}
