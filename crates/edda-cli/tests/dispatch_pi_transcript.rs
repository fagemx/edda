//! Regression test for GH-577: Ingest pi session transcripts from `edda dispatch --agent pi`.
//!
//! DoneWhen requirements:
//! 1. Turns run by `edda dispatch --agent pi` have their session transcripts ingested,
//!    producing a #session_digest note (calls, duration, tool breakdown, model, usage).
//! 2. Explicit `--session-id` maps to the same session identity in the ledger.
//! 3. Incremental ingest is idempotent across multiple calls (no duplicate notes or double counting).
//! 4. Ingest failure does not fail the dispatch turn.

use std::fs;
use std::path::{Path, PathBuf};

fn write_sample_pi_transcript(dir: &Path, session_id: &str, cwd: &Path) -> PathBuf {
    let session_file = dir.join(format!("2026-09-02T12-00-00-000Z_{session_id}.jsonl"));
    let lines = vec![
        serde_json::json!({
            "type": "session",
            "version": 3,
            "id": session_id,
            "timestamp": "2026-09-02T12:00:00.000Z",
            "cwd": cwd.to_string_lossy(),
        }),
        serde_json::json!({
            "type": "message",
            "id": "msg-u1",
            "timestamp": "2026-09-02T12:00:05.000Z",
            "message": {
                "role": "user",
                "content": [{ "type": "text", "text": "Run tests" }]
            }
        }),
        serde_json::json!({
            "type": "message",
            "id": "msg-a1",
            "timestamp": "2026-09-02T12:01:30.000Z",
            "message": {
                "role": "assistant",
                "content": [
                    {
                        "type": "toolCall",
                        "id": "call-1",
                        "name": "bash",
                        "arguments": { "command": "cargo test" }
                    },
                    {
                        "type": "toolCall",
                        "id": "call-2",
                        "name": "read",
                        "arguments": { "path": "src/lib.rs" }
                    }
                ],
                "model": "gpt-5.6-sol",
                "usage": {
                    "input": 1000,
                    "output": 200,
                    "cacheRead": 500,
                    "cacheWrite": 0,
                    "totalTokens": 1700,
                    "cost": { "total": 0.05 }
                }
            }
        }),
        serde_json::json!({
            "type": "message",
            "id": "msg-r1",
            "timestamp": "2026-09-02T12:01:35.000Z",
            "message": {
                "role": "toolResult",
                "toolCallId": "call-1",
                "toolName": "bash",
                "content": [{ "type": "text", "text": "ok" }],
                "isError": false
            }
        }),
        serde_json::json!({
            "type": "message",
            "id": "msg-r2",
            "timestamp": "2026-09-02T12:01:40.000Z",
            "message": {
                "role": "toolResult",
                "toolCallId": "call-2",
                "toolName": "read",
                "content": [{ "type": "text", "text": "code" }],
                "isError": false
            }
        }),
    ];

    let mut content = String::new();
    for l in lines {
        content.push_str(&serde_json::to_string(&l).unwrap());
        content.push('\n');
    }
    fs::write(&session_file, content).unwrap();
    session_file
}

#[test]
fn test_pi_transcript_ingested_and_idempotent() {
    let tmp_ws = tempfile::tempdir().unwrap();
    let tmp_store = tempfile::tempdir().unwrap();
    let tmp_sessions = tempfile::tempdir().unwrap();

    std::env::set_var("EDDA_STORE_ROOT", tmp_store.path());

    let ws_path = tmp_ws.path();
    let ledger = edda_ledger::Ledger::open_or_init(ws_path).unwrap();

    let session_id = "test-pi-ingest-577";
    write_sample_pi_transcript(tmp_sessions.path(), session_id, ws_path);

    let project_id = edda_store::project_id(ws_path);
    let project_dir = tmp_store.path().join("projects").join(&project_id);
    fs::create_dir_all(project_dir.join("ledger")).unwrap();
    fs::create_dir_all(project_dir.join("state")).unwrap();
    fs::create_dir_all(project_dir.join("transcripts")).unwrap();

    // Verify: find_pi_session_file finds the file
    let found =
        edda_transcript::find_pi_session_file(ws_path, session_id, Some(tmp_sessions.path()));
    assert!(
        found.is_some(),
        "find_pi_session_file should locate the session file"
    );
    let transcript_path = found.unwrap();

    // Perform ingestion
    let stats = edda_transcript::ingest_pi_transcript_delta(
        &project_dir,
        session_id,
        ws_path,
        &transcript_path,
    )
    .expect("ingest_pi_transcript_delta should succeed");

    assert_eq!(stats.tool_calls, 2, "should have found 2 tool calls");
    assert_eq!(stats.model, "gpt-5.6-sol");
    assert_eq!(stats.input_tokens, 1000);
    assert_eq!(stats.output_tokens, 200);

    // Trigger digest manual
    let cwd_str = ws_path.to_string_lossy();
    let event_id =
        edda_bridge_claude::digest::digest_session_manual(&project_id, session_id, &cwd_str, true)
            .expect("digest_session_manual should succeed");
    assert!(!event_id.is_empty());

    // Query ledger for #session_digest note
    let events = ledger.iter_events().unwrap();
    let digests: Vec<_> = events
        .iter()
        .filter(|e| {
            e.payload
                .get("tags")
                .and_then(|t: &serde_json::Value| t.as_array())
                .is_some_and(|arr: &Vec<serde_json::Value>| {
                    arr.iter().any(|tag| tag == "session_digest")
                })
        })
        .collect();

    assert_eq!(
        digests.len(),
        1,
        "exactly one session_digest note should be produced"
    );
    let text = digests[0]
        .payload
        .get("text")
        .and_then(|t: &serde_json::Value| t.as_str())
        .unwrap_or("");
    assert!(
        text.contains("2 tool calls"),
        "digest text should contain '2 tool calls': {text}"
    );
    assert!(
        text.contains("Bash:1"),
        "digest text should contain 'Bash:1': {text}"
    );
    assert!(
        text.contains("read:1"),
        "digest text should contain 'read:1': {text}"
    );
    assert!(
        text.contains("gpt-5.6-sol"),
        "digest text should contain model: {text}"
    );

    // Second ingestion should be idempotent (0 new bytes read, no duplicate note)
    let stats2 = edda_transcript::ingest_pi_transcript_delta(
        &project_dir,
        session_id,
        ws_path,
        &transcript_path,
    )
    .expect("second ingest should succeed");
    assert_eq!(
        stats2.bytes_read, 0,
        "idempotent: 0 bytes read on re-ingest"
    );

    // Digest again should not produce a duplicate note
    let _ =
        edda_bridge_claude::digest::digest_session_manual(&project_id, session_id, &cwd_str, true);
    let events2 = ledger.iter_events().unwrap();
    let digests2: Vec<_> = events2
        .iter()
        .filter(|e| {
            e.payload
                .get("tags")
                .and_then(|t: &serde_json::Value| t.as_array())
                .is_some_and(|arr: &Vec<serde_json::Value>| {
                    arr.iter().any(|tag| tag == "session_digest")
                })
        })
        .collect();
    assert_eq!(
        digests2.len(),
        1,
        "no duplicate session_digest note emitted"
    );
}
