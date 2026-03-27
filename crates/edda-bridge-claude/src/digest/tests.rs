use std::collections::BTreeMap;
use std::path::Path;

use super::extract::*;
use super::helpers::*;
use super::orchestrate::*;
use super::prev::*;
use super::render::*;
use super::*;

use std::io::Write;

fn write_session_ledger(dir: &Path, lines: &[serde_json::Value]) -> std::path::PathBuf {
    let path = dir.join("test_session.jsonl");
    let mut f = std::fs::File::create(&path).unwrap();
    for line in lines {
        writeln!(f, "{}", serde_json::to_string(line).unwrap()).unwrap();
    }
    path
}

fn make_envelope(
    hook_event_name: &str,
    tool_name: &str,
    raw_extra: serde_json::Value,
) -> serde_json::Value {
    let mut raw = serde_json::json!({
        "hook_event_name": hook_event_name,
        "tool_name": tool_name,
    });
    if let Some(obj) = raw_extra.as_object() {
        for (k, v) in obj {
            raw[k.clone()] = v.clone();
        }
    }
    serde_json::json!({
        "ts": "2026-02-14T10:00:00Z",
        "project_id": "test_proj",
        "session_id": "test_session",
        "hook_event_name": hook_event_name,
        "tool_name": tool_name,
        "tool_use_id": "",
        "raw": raw,
    })
}

fn make_envelope_at(
    hook_event_name: &str,
    tool_name: &str,
    ts: &str,
    raw_extra: serde_json::Value,
) -> serde_json::Value {
    let mut e = make_envelope(hook_event_name, tool_name, raw_extra);
    e["ts"] = serde_json::Value::String(ts.to_string());
    e
}

#[test]
fn digest_empty_session() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("empty.jsonl");
    std::fs::write(&path, "").unwrap();

    let event = extract_session_digest(&path, "sess-empty", "main", None).unwrap();
    assert_eq!(event.event_type, "note");
    assert_eq!(event.payload["source"], "bridge:session_digest");
    assert_eq!(event.payload["session_stats"]["tool_calls"], 0);
    assert_eq!(event.payload["session_stats"]["user_prompts"], 0);
    assert!(event.event_id.starts_with("evt_"));
    assert!(!event.hash.is_empty());
}

#[test]
fn digest_counts_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let lines = vec![
        make_envelope("PostToolUse", "Bash", serde_json::json!({})),
        make_envelope(
            "PostToolUse",
            "Edit",
            serde_json::json!({
                "tool_input": { "file_path": "/src/main.rs" }
            }),
        ),
        make_envelope("PostToolUse", "Read", serde_json::json!({})),
        make_envelope(
            "PostToolUseFailure",
            "Bash",
            serde_json::json!({
                "tool_input": { "command": "cargo test" }
            }),
        ),
        make_envelope("UserPromptSubmit", "", serde_json::json!({})),
        make_envelope("UserPromptSubmit", "", serde_json::json!({})),
    ];
    let path = write_session_ledger(tmp.path(), &lines);
    let stats = extract_stats(&path).unwrap();

    assert_eq!(stats.tool_calls, 3);
    assert_eq!(stats.tool_failures, 1);
    assert_eq!(stats.user_prompts, 2);
}

#[test]
fn digest_extracts_files() {
    let tmp = tempfile::tempdir().unwrap();
    let lines = vec![
        make_envelope(
            "PostToolUse",
            "Edit",
            serde_json::json!({
                "tool_input": { "file_path": "/src/main.rs" }
            }),
        ),
        make_envelope(
            "PostToolUse",
            "Write",
            serde_json::json!({
                "tool_input": { "file_path": "/src/lib.rs" }
            }),
        ),
        make_envelope(
            "PostToolUse",
            "Edit",
            serde_json::json!({
                "tool_input": { "file_path": "/src/main.rs" }
            }),
        ),
        make_envelope("PostToolUse", "Read", serde_json::json!({})),
    ];
    let path = write_session_ledger(tmp.path(), &lines);
    let stats = extract_stats(&path).unwrap();

    assert_eq!(stats.files_modified.len(), 2);
    assert!(stats.files_modified.contains(&"/src/lib.rs".to_string()));
    assert!(stats.files_modified.contains(&"/src/main.rs".to_string()));
}

#[test]
fn digest_extracts_failed_cmds() {
    let tmp = tempfile::tempdir().unwrap();
    let lines = vec![
        make_envelope(
            "PostToolUseFailure",
            "Bash",
            serde_json::json!({
                "tool_input": { "command": "cargo test --all" }
            }),
        ),
        make_envelope("PostToolUseFailure", "Edit", serde_json::json!({})),
        make_envelope(
            "PostToolUseFailure",
            "Bash",
            serde_json::json!({
                "tool_input": { "command": "npm run build" }
            }),
        ),
    ];
    let path = write_session_ledger(tmp.path(), &lines);
    let stats = extract_stats(&path).unwrap();

    assert_eq!(stats.tool_failures, 3);
    assert_eq!(stats.failed_commands.len(), 2);
    assert_eq!(stats.failed_commands[0], "cargo test --all");
    assert_eq!(stats.failed_commands[1], "npm run build");
}

#[test]
fn digest_event_has_provenance() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("prov.jsonl");
    std::fs::write(&path, "").unwrap();

    let event = extract_session_digest(&path, "sess-abc123", "main", None).unwrap();
    assert_eq!(event.refs.provenance.len(), 1);
    assert_eq!(event.refs.provenance[0].target, "session:sess-abc123");
    assert_eq!(event.refs.provenance[0].rel, "based_on");
    assert!(event.refs.provenance[0].note.is_some());
}

#[test]
fn digest_payload_has_source() {
    let tmp = tempfile::tempdir().unwrap();
    let lines = vec![make_envelope("PostToolUse", "Bash", serde_json::json!({}))];
    let path = write_session_ledger(tmp.path(), &lines);

    let event = extract_session_digest(&path, "sess-src", "main", None).unwrap();
    assert_eq!(event.payload["source"], "bridge:session_digest");
    assert_eq!(event.payload["role"], "system");
    let tags = event.payload["tags"].as_array().unwrap();
    assert!(tags.iter().any(|t| t.as_str() == Some("session_digest")));
}

#[test]
fn digest_duration_computed() {
    let tmp = tempfile::tempdir().unwrap();
    let lines = vec![
        make_envelope_at(
            "UserPromptSubmit",
            "",
            "2026-02-14T10:00:00Z",
            serde_json::json!({}),
        ),
        make_envelope_at(
            "PostToolUse",
            "Bash",
            "2026-02-14T10:35:00Z",
            serde_json::json!({}),
        ),
    ];
    let path = write_session_ledger(tmp.path(), &lines);
    let stats = extract_stats(&path).unwrap();

    assert_eq!(stats.duration_minutes, 35);
}

#[test]
fn digest_extracts_commits_from_bash() {
    let tmp = tempfile::tempdir().unwrap();
    let lines = vec![
        make_envelope(
            "PostToolUse",
            "Bash",
            serde_json::json!({
                "tool_input": { "command": "git commit -m \"fix: resolve UTF-8 truncation\"" }
            }),
        ),
        make_envelope(
            "PostToolUse",
            "Bash",
            serde_json::json!({
                "tool_input": { "command": "cargo test --all" }
            }),
        ),
        make_envelope(
            "PostToolUse",
            "Bash",
            serde_json::json!({
                "tool_input": { "command": "git add . && git commit -m 'feat: add digest'" }
            }),
        ),
    ];
    let path = write_session_ledger(tmp.path(), &lines);
    let stats = extract_stats(&path).unwrap();

    assert_eq!(stats.commits_made.len(), 2);
    assert_eq!(stats.commits_made[0], "fix: resolve UTF-8 truncation");
    assert_eq!(stats.commits_made[1], "feat: add digest");
}

#[test]
fn digest_commits_in_payload() {
    let tmp = tempfile::tempdir().unwrap();
    let lines = vec![make_envelope(
        "PostToolUse",
        "Bash",
        serde_json::json!({
            "tool_input": { "command": "git commit -m \"fix: something\"" }
        }),
    )];
    let path = write_session_ledger(tmp.path(), &lines);
    let event = extract_session_digest(&path, "sess-commits", "main", None).unwrap();

    let commits = event.payload["session_stats"]["commits_made"]
        .as_array()
        .unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0], "fix: something");

    // Also in text
    let text = event.payload["text"].as_str().unwrap();
    assert!(text.contains("Commits:"));
    assert!(text.contains("fix: something"));
}

#[test]
fn outcome_completed_normal_session() {
    let tmp = tempfile::tempdir().unwrap();
    let lines = vec![
        make_envelope("UserPromptSubmit", "", serde_json::json!({})),
        make_envelope("PostToolUse", "Read", serde_json::json!({})),
        make_envelope("PostToolUse", "Edit", serde_json::json!({})),
    ];
    let path = write_session_ledger(tmp.path(), &lines);
    let stats = extract_stats(&path).unwrap();
    assert_eq!(stats.outcome, SessionOutcome::Completed);
}

#[test]
fn outcome_interrupted_last_is_user_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let lines = vec![
        make_envelope("PostToolUse", "Read", serde_json::json!({})),
        make_envelope("UserPromptSubmit", "", serde_json::json!({})),
    ];
    let path = write_session_ledger(tmp.path(), &lines);
    let stats = extract_stats(&path).unwrap();
    assert_eq!(stats.outcome, SessionOutcome::Interrupted);
}

#[test]
fn outcome_error_stuck_three_consecutive_failures() {
    let tmp = tempfile::tempdir().unwrap();
    let lines = vec![
        make_envelope("PostToolUse", "Edit", serde_json::json!({})),
        make_envelope("PostToolUseFailure", "Bash", serde_json::json!({})),
        make_envelope("PostToolUseFailure", "Bash", serde_json::json!({})),
        make_envelope("PostToolUseFailure", "Bash", serde_json::json!({})),
    ];
    let path = write_session_ledger(tmp.path(), &lines);
    let stats = extract_stats(&path).unwrap();
    assert_eq!(stats.outcome, SessionOutcome::ErrorStuck);
}

#[test]
fn outcome_not_stuck_if_success_resets_count() {
    let tmp = tempfile::tempdir().unwrap();
    let lines = vec![
        make_envelope("PostToolUseFailure", "Bash", serde_json::json!({})),
        make_envelope("PostToolUseFailure", "Bash", serde_json::json!({})),
        make_envelope("PostToolUse", "Edit", serde_json::json!({})), // resets
        make_envelope("PostToolUseFailure", "Bash", serde_json::json!({})),
    ];
    let path = write_session_ledger(tmp.path(), &lines);
    let stats = extract_stats(&path).unwrap();
    assert_eq!(stats.outcome, SessionOutcome::Completed);
}

#[test]
fn outcome_in_digest_payload() {
    let stats = SessionStats {
        outcome: SessionOutcome::ErrorStuck,
        ..Default::default()
    };
    let event = build_digest_event("sess-outcome", &stats, "main", None, &[]).unwrap();
    assert_eq!(
        event.payload["session_stats"]["outcome"].as_str().unwrap(),
        "error_stuck"
    );
}

#[test]
fn digest_tasks_snapshot_in_payload() {
    let stats = SessionStats {
        tool_calls: 5,
        tasks_snapshot: vec![
            DigestTaskSnapshot {
                subject: "Fix auth bug".to_string(),
                status: "completed".to_string(),
            },
            DigestTaskSnapshot {
                subject: "Add tests".to_string(),
                status: "in_progress".to_string(),
            },
        ],
        ..Default::default()
    };

    let event = build_digest_event("sess-tasks", &stats, "main", None, &[]).unwrap();

    // Check payload
    let tasks = event.payload["session_stats"]["tasks_snapshot"]
        .as_array()
        .unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0]["subject"], "Fix auth bug");
    assert_eq!(tasks[0]["status"], "completed");
    assert_eq!(tasks[1]["subject"], "Add tests");
    assert_eq!(tasks[1]["status"], "in_progress");

    // Check text rendering
    let text = event.payload["text"].as_str().unwrap();
    assert!(text.contains("Done: Fix auth bug"), "text: {text}");
    assert!(text.contains("WIP: Add tests"), "text: {text}");
}

#[test]
fn extract_git_commit_msg_works() {
    assert_eq!(
        extract_git_commit_msg(r#"git commit -m "fix: something""#),
        "fix: something"
    );
    assert_eq!(
        extract_git_commit_msg("git commit -m 'feat: new'"),
        "feat: new"
    );
    assert_eq!(extract_git_commit_msg("git add . && git commit"), "");
}

#[test]
fn digest_nonexistent_file_returns_empty_stats() {
    let path = Path::new("/nonexistent/session.jsonl");
    let stats = extract_stats(path).unwrap();
    assert_eq!(stats.tool_calls, 0);
    assert_eq!(stats.user_prompts, 0);
}

#[test]
fn digest_hash_chain_ready() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("chain.jsonl");
    std::fs::write(&path, "").unwrap();

    let e1 = extract_session_digest(&path, "sess-1", "main", None).unwrap();
    let e2 = extract_session_digest(&path, "sess-2", "main", Some(&e1.hash)).unwrap();

    assert!(e1.parent_hash.is_none());
    assert_eq!(e2.parent_hash.as_deref(), Some(e1.hash.as_str()));
    assert_ne!(e1.hash, e2.hash);
    assert_eq!(e1.digests.len(), 1);
    assert_eq!(e2.digests.len(), 1);
}

// ── Auto-Digest Integration Tests ──

/// Create a workspace (.edda/) and a fake store with a session ledger.
/// Returns (workspace_root, fake_project_id, session_id).
fn setup_digest_workspace(tmp: &Path) -> (std::path::PathBuf, String) {
    // Create workspace
    let workspace = tmp.join("repo");
    let paths = edda_ledger::EddaPaths::discover(&workspace);
    edda_ledger::ledger::init_workspace(&paths).unwrap();
    edda_ledger::ledger::init_head(&paths, "main").unwrap();
    edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();

    // Use the workspace path as project_id source
    let project_id = edda_store::project_id(&workspace);
    let _ = edda_store::ensure_dirs(&project_id);

    (workspace, project_id)
}

fn write_store_session_ledger(project_id: &str, session_id: &str, lines: &[serde_json::Value]) {
    let dir = edda_store::project_dir(project_id).join("ledger");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{session_id}.jsonl"));
    let mut f = std::fs::File::create(&path).unwrap();
    for line in lines {
        writeln!(f, "{}", serde_json::to_string(line).unwrap()).unwrap();
    }
}

#[test]
fn digest_writes_to_workspace_ledger() {
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    // Create a previous session's ledger in the store
    let prev_session = "prev-session-001";
    write_store_session_ledger(
        &project_id,
        prev_session,
        &[
            make_envelope("PostToolUse", "Bash", serde_json::json!({})),
            make_envelope("UserPromptSubmit", "", serde_json::json!({})),
        ],
    );

    let result = digest_previous_sessions(
        &project_id,
        "current-session-002",
        workspace.to_str().unwrap(),
        2000,
    );

    assert!(matches!(result, DigestResult::Written { .. }));

    // Verify event in workspace ledger
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let events = ledger.iter_events().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "note");
    assert_eq!(events[0].payload["source"], "bridge:session_digest");
    assert_eq!(events[0].payload["session_id"], prev_session);
}

#[test]
fn digest_maintains_hash_chain() {
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    // Write two previous sessions
    write_store_session_ledger(
        &project_id,
        "sess-aaa",
        &[make_envelope("PostToolUse", "Bash", serde_json::json!({}))],
    );
    write_store_session_ledger(
        &project_id,
        "sess-bbb",
        &[make_envelope("PostToolUse", "Edit", serde_json::json!({}))],
    );

    // Digest first
    let r1 = digest_previous_sessions(&project_id, "current", workspace.to_str().unwrap(), 2000);
    assert!(matches!(r1, DigestResult::Written { .. }));

    // Digest second
    let r2 = digest_previous_sessions(&project_id, "current", workspace.to_str().unwrap(), 2000);
    assert!(matches!(r2, DigestResult::Written { .. }));

    // Verify hash chain
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let events = ledger.iter_events().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events[0].parent_hash.is_none());
    assert_eq!(
        events[1].parent_hash.as_deref(),
        Some(events[0].hash.as_str())
    );
}

#[test]
fn digest_skips_already_digested() {
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    write_store_session_ledger(
        &project_id,
        "sess-once",
        &[make_envelope("PostToolUse", "Bash", serde_json::json!({}))],
    );

    let ledger_dir = edda_store::project_dir(&project_id).join("ledger");

    // Digest once
    let r1 = digest_previous_sessions(&project_id, "current", workspace.to_str().unwrap(), 2000);
    assert!(matches!(r1, DigestResult::Written { .. }));

    // Session ledger file should be deleted after successful digest
    assert!(
        !ledger_dir.join("sess-once.jsonl").exists(),
        "session ledger file should be removed after successful digest"
    );

    // Digest again — should be NoPending (file is gone, not re-discovered)
    let r2 = digest_previous_sessions(&project_id, "current", workspace.to_str().unwrap(), 2000);
    assert!(matches!(r2, DigestResult::NoPending));

    // Workspace ledger should still have exactly 1 event
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    assert_eq!(ledger.iter_events().unwrap().len(), 1);
}

#[test]
fn digest_no_reduplicate_across_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    // Create 3 session ledger files
    for sid in &["sess-001", "sess-002", "sess-003"] {
        write_store_session_ledger(
            &project_id,
            sid,
            &[make_envelope("PostToolUse", "Bash", serde_json::json!({}))],
        );
    }

    let ws = workspace.to_str().unwrap();
    let ledger_dir = edda_store::project_dir(&project_id).join("ledger");

    // digest_previous_sessions processes one session per call.
    // Call it 3 times to digest all 3, then once more to confirm NoPending.
    for _ in 0..3 {
        let r = digest_previous_sessions(&project_id, "sess-A", ws, 2000);
        assert!(matches!(r, DigestResult::Written { .. }));
    }

    // All 3 session ledger files should be removed
    assert!(!ledger_dir.join("sess-001.jsonl").exists());
    assert!(!ledger_dir.join("sess-002.jsonl").exists());
    assert!(!ledger_dir.join("sess-003.jsonl").exists());

    // Next call: no pending sessions
    let r = digest_previous_sessions(&project_id, "sess-B", ws, 2000);
    assert!(matches!(r, DigestResult::NoPending));

    // Workspace ledger should have exactly 3 digest events (not more)
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    assert_eq!(
        ledger.iter_events().unwrap().len(),
        3,
        "should have exactly 3 digest events, no duplicates"
    );
}

#[test]
fn digest_no_workspace_records_failure() {
    let tmp = tempfile::tempdir().unwrap();
    // No workspace created — just a store
    let project_id = "fake_project_no_workspace";
    let _ = edda_store::ensure_dirs(project_id);
    // Reset state and ledger dir from previous test runs
    save_digest_state(project_id, &DigestState::default()).unwrap();
    let ledger_dir = edda_store::project_dir(project_id).join("ledger");
    let _ = std::fs::remove_dir_all(&ledger_dir);

    write_store_session_ledger(
        project_id,
        "sess-fail",
        &[make_envelope("PostToolUse", "Bash", serde_json::json!({}))],
    );

    let result = digest_previous_sessions(
        project_id,
        "current",
        tmp.path().to_str().unwrap(), // no .edda here
        2000,
    );

    assert!(matches!(result, DigestResult::Error(_)));

    // State should record the failure
    let state = load_digest_state(project_id);
    assert_eq!(state.pending_session_id, "sess-fail");
    assert_eq!(state.retry_count, 1);
}

#[test]
fn digest_permanent_failure_after_3_retries() {
    let tmp = tempfile::tempdir().unwrap();
    let project_id = "fake_project_perm_fail";
    let _ = edda_store::ensure_dirs(project_id);
    // Reset ledger dir from previous test runs
    let ledger_dir = edda_store::project_dir(project_id).join("ledger");
    let _ = std::fs::remove_dir_all(&ledger_dir);

    // Manually set state to 3 retries
    let state = DigestState {
        pending_session_id: "sess-stuck".to_string(),
        retry_count: 3,
        last_error: "lock timeout".to_string(),
        ..Default::default()
    };
    save_digest_state(project_id, &state).unwrap();

    write_store_session_ledger(
        project_id,
        "sess-stuck",
        &[make_envelope("PostToolUse", "Bash", serde_json::json!({}))],
    );

    let result =
        digest_previous_sessions(project_id, "current", tmp.path().to_str().unwrap(), 2000);

    assert!(matches!(result, DigestResult::PermanentFailure(_)));
    if let DigestResult::PermanentFailure(msg) = result {
        assert!(msg.contains("sess-stu"));
        assert!(msg.contains("edda bridge digest"));
    }
}

#[test]
fn digest_state_round_trip() {
    let project_id = "test_state_rt";
    let _ = edda_store::ensure_dirs(project_id);

    let state = DigestState {
        session_id: "sess-123".to_string(),
        digested_at: "2026-02-14T10:00:00Z".to_string(),
        event_id: "evt_abc".to_string(),
        retry_count: 0,
        pending_session_id: String::new(),
        last_error: String::new(),
    };
    save_digest_state(project_id, &state).unwrap();

    let loaded = load_digest_state(project_id);
    assert_eq!(loaded.session_id, "sess-123");
    assert_eq!(loaded.event_id, "evt_abc");
    assert_eq!(loaded.retry_count, 0);
}

// ── #32 Tests: failed cmd milestones + CLI digest ──

#[test]
fn failed_cmd_milestone_produced() {
    let failed = FailedCommand {
        command: "cargo test --fail".to_string(),
        cwd: "/project".to_string(),
        exit_code: 1,
    };
    let event = build_cmd_milestone_event("sess-cmd-1", &failed, "main", None).unwrap();

    assert_eq!(event.event_type, "cmd");
    assert_eq!(event.payload["source"], "bridge:cmd");
    assert_eq!(event.payload["exit_code"], 1);
    assert_eq!(event.payload["argv"][0], "cargo test --fail");
    assert_eq!(event.payload["cwd"], "/project");
    assert_eq!(event.payload["session_id"], "sess-cmd-1");
}

#[test]
fn failed_cmd_milestone_has_provenance() {
    let failed = FailedCommand {
        command: "npm install".to_string(),
        cwd: ".".to_string(),
        exit_code: 127,
    };
    let event = build_cmd_milestone_event("sess-prov-1", &failed, "main", None).unwrap();

    assert!(!event.refs.provenance.is_empty());
    assert_eq!(event.refs.provenance[0].target, "session:sess-prov-1");
    assert_eq!(event.refs.provenance[0].rel, "based_on");
}

#[test]
fn failed_cmd_milestone_chains_hash() {
    let failed = FailedCommand {
        command: "make build".to_string(),
        cwd: ".".to_string(),
        exit_code: 2,
    };
    let parent = "abc123";
    let event = build_cmd_milestone_event("sess-chain", &failed, "main", Some(parent)).unwrap();

    assert_eq!(event.parent_hash.as_deref(), Some("abc123"));
    assert!(!event.hash.is_empty());
}

#[test]
fn extract_stats_captures_failed_cmd_detail() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("sess-detail.jsonl");
    let mut f = std::fs::File::create(&path).unwrap();
    // PostToolUseFailure with real Claude Code format: error field, no toolResponse
    let envelope = serde_json::json!({
        "ts": "2026-02-14T10:00:00Z",
        "hook_event_name": "PostToolUseFailure",
        "tool_name": "Bash",
        "cwd": "/my/project",
        "raw": {
            "tool_name": "Bash",
            "tool_input": { "command": "cargo build" },
            "error": "Exit code 101\nerror[E0308]: mismatched types"
        }
    });
    writeln!(f, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();

    let stats = extract_stats(&path).unwrap();
    assert_eq!(stats.failed_cmds_detail.len(), 1);
    assert_eq!(stats.failed_cmds_detail[0].command, "cargo build");
    assert_eq!(stats.failed_cmds_detail[0].cwd, "/my/project");
    assert_eq!(stats.failed_cmds_detail[0].exit_code, 101);
}

#[test]
fn extract_exit_code_from_error_field() {
    // Real Claude Code PostToolUseFailure format
    let envelope = serde_json::json!({
        "raw": {
            "error": "Exit code 49",
            "tool_name": "Bash",
            "tool_input": { "command": "python3 --version" }
        }
    });
    assert_eq!(extract_exit_code(&envelope), 49);

    // Error with multiline detail
    let envelope2 = serde_json::json!({
        "raw": {
            "error": "Exit code 128\nfatal: not a git repository"
        }
    });
    assert_eq!(extract_exit_code(&envelope2), 128);

    // Legacy camelCase toolResponse.exitCode still works
    let envelope3 = serde_json::json!({
        "raw": {
            "toolResponse": { "exitCode": 42 }
        }
    });
    assert_eq!(extract_exit_code(&envelope3), 42);

    // No raw → default 1
    let envelope4 = serde_json::json!({});
    assert_eq!(extract_exit_code(&envelope4), 1);
}

#[test]
fn digest_writes_cmd_milestones_to_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    // Write session with a failed Bash command (real Claude Code format)
    write_store_session_ledger(
        &project_id,
        "sess-cmd-ws",
        &[
            make_envelope("PostToolUse", "Bash", serde_json::json!({})),
            serde_json::json!({
                "ts": "2026-02-14T10:01:00Z",
                "hook_event_name": "PostToolUseFailure",
                "tool_name": "Bash",
                "cwd": "/proj",
                "raw": {
                    "tool_name": "Bash",
                    "tool_input": { "command": "failing-cmd" },
                    "error": "Exit code 1"
                }
            }),
        ],
    );

    let result = digest_previous_sessions_with_opts(
        &project_id,
        "current",
        workspace.to_str().unwrap(),
        2000,
        true,
    );
    assert!(matches!(result, DigestResult::Written { .. }));

    // Workspace should have 2 events: note digest + cmd milestone
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let events = ledger.iter_events().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_type, "note");
    assert_eq!(events[1].event_type, "cmd");
    assert_eq!(events[1].payload["source"], "bridge:cmd");
    // Hash chain: second event parents the first
    assert_eq!(
        events[1].parent_hash.as_deref(),
        Some(events[0].hash.as_str())
    );
}

#[test]
fn digest_skips_cmd_milestones_when_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    write_store_session_ledger(
        &project_id,
        "sess-no-cmd",
        &[serde_json::json!({
            "ts": "2026-02-14T10:01:00Z",
            "hook_event_name": "PostToolUseFailure",
            "tool_name": "Bash",
            "cwd": "/proj",
            "raw": {
                "tool_name": "Bash",
                "tool_input": { "command": "fail-cmd" },
                "error": "Exit code 1"
            }
        })],
    );

    // digest_failed_cmds = false
    let result = digest_previous_sessions_with_opts(
        &project_id,
        "current",
        workspace.to_str().unwrap(),
        2000,
        false,
    );
    assert!(matches!(result, DigestResult::Written { .. }));

    // Only 1 event (note digest, no cmd)
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let events = ledger.iter_events().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "note");
}

#[test]
fn manual_digest_specific_session() {
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    write_store_session_ledger(
        &project_id,
        "sess-manual",
        &[
            make_envelope("PostToolUse", "Edit", serde_json::json!({})),
            make_envelope("PostToolUse", "Bash", serde_json::json!({})),
        ],
    );

    let event_id = digest_session_manual(
        &project_id,
        "sess-manual",
        workspace.to_str().unwrap(),
        true,
    )
    .unwrap();

    assert!(event_id.starts_with("evt_"));

    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let events = ledger.iter_events().unwrap();
    assert!(!events.is_empty());
    assert_eq!(events[0].event_type, "note");
    assert_eq!(events[0].payload["source"], "bridge:session_digest");
}

// ── PrevDigest tests ──

#[test]
fn prev_digest_roundtrip() {
    let pid = "test_prev_digest_rt";
    let _ = edda_store::ensure_dirs(pid);

    let stats = SessionStats {
        tasks_snapshot: vec![
            DigestTaskSnapshot {
                subject: "Fix bug".into(),
                status: "completed".into(),
            },
            DigestTaskSnapshot {
                subject: "Add tests".into(),
                status: "completed".into(),
            },
            DigestTaskSnapshot {
                subject: "Deploy".into(),
                status: "pending".into(),
            },
        ],
        commits_made: vec!["fix: auth flow".into(), "feat: add billing".into()],
        files_modified: vec!["src/lib.rs".into(), "src/main.rs".into()],
        duration_minutes: 25,
        outcome: SessionOutcome::Completed,
        ..Default::default()
    };

    write_prev_digest(pid, "test-sess", &stats, vec![], vec![]);

    let digest = read_prev_digest(pid).expect("should read prev_digest");
    assert_eq!(digest.session_id, "test-sess");
    assert_eq!(digest.outcome, "completed");
    assert_eq!(digest.duration_minutes, 25);
    assert_eq!(digest.completed_tasks, vec!["Fix bug", "Add tests"]);
    assert_eq!(digest.pending_tasks, vec!["Deploy"]);
    assert_eq!(digest.commits.len(), 2);
    assert_eq!(digest.files_modified_count, 2);

    let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn prev_digest_empty_tasks() {
    let pid = "test_prev_digest_empty";
    let _ = edda_store::ensure_dirs(pid);

    let stats = SessionStats {
        commits_made: vec!["chore: cleanup".into()],
        files_modified: vec!["README.md".into()],
        duration_minutes: 5,
        outcome: SessionOutcome::Interrupted,
        ..Default::default()
    };

    write_prev_digest(pid, "test-empty", &stats, vec![], vec![]);

    let digest = read_prev_digest(pid).expect("should read prev_digest");
    assert!(digest.completed_tasks.is_empty());
    assert!(digest.pending_tasks.is_empty());
    assert_eq!(digest.commits.len(), 1);
    assert_eq!(digest.outcome, "interrupted");

    let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn prev_digest_with_decisions_and_notes() {
    let pid = "test_prev_digest_dn";
    let _ = edda_store::ensure_dirs(pid);

    let stats = SessionStats {
        commits_made: vec!["feat: add auth".into()],
        files_modified: vec!["src/auth.rs".into()],
        failed_commands: vec!["cargo test".into()],
        duration_minutes: 20,
        outcome: SessionOutcome::Completed,
        ..Default::default()
    };
    write_prev_digest(
        pid,
        "test-dn",
        &stats,
        vec!["auth=jwt (stateless)".into(), "db=postgres".into()],
        vec!["OAuth deferred — needs client registration".into()],
    );

    let loaded = read_prev_digest(pid).expect("should read enriched prev_digest");
    assert_eq!(loaded.decisions.len(), 2);
    assert_eq!(loaded.decisions[0], "auth=jwt (stateless)");
    assert_eq!(loaded.notes.len(), 1);
    assert!(loaded.notes[0].contains("OAuth"));
    assert_eq!(loaded.failed_commands, vec!["cargo test"]);

    let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn prev_digest_backward_compat() {
    let pid = "test_prev_digest_compat";
    let _ = edda_store::ensure_dirs(pid);

    // Write old-format JSON without new fields
    let old_json = serde_json::json!({
        "session_id": "old-sess",
        "completed_at": "2026-02-17T10:00:00Z",
        "outcome": "completed",
        "duration_minutes": 10,
        "completed_tasks": ["Fix bug"],
        "pending_tasks": [],
        "commits": ["fix: bug"],
        "files_modified_count": 1,
        "total_edits": 5
    });
    let path = edda_store::project_dir(pid)
        .join("state")
        .join("prev_digest.json");
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    std::fs::write(&path, serde_json::to_string_pretty(&old_json).unwrap()).unwrap();

    let digest = read_prev_digest(pid).expect("old format should deserialize");
    assert_eq!(digest.session_id, "old-sess");
    assert!(
        digest.decisions.is_empty(),
        "decisions should default to empty"
    );
    assert!(digest.notes.is_empty(), "notes should default to empty");
    assert!(
        digest.failed_commands.is_empty(),
        "failed_commands should default to empty"
    );

    let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn collect_session_ledger_extras_basic() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_path_buf();
    let paths = edda_ledger::EddaPaths::discover(&workspace);
    edda_ledger::ledger::init_workspace(&paths).unwrap();
    edda_ledger::ledger::init_head(&paths, "main").unwrap();
    edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let branch = ledger.head_branch().unwrap();

    // Write a decision event
    let dp = edda_core::types::DecisionPayload {
        key: "auth".to_string(),
        value: "jwt".to_string(),
        reason: Some("stateless".to_string()),
        scope: None,
        authority: None,
        affected_paths: None,
        tags: None,
        review_after: None,
        reversibility: None,
        village_id: None,
    };
    let evt = edda_core::event::new_decision_event(&branch, None, "system", &dp).unwrap();
    let decision_ts = evt.ts.clone();
    ledger.append_event(&evt).unwrap();

    // Write a session note
    let tags_s = vec!["session".to_string()];
    let evt2 = edda_core::event::new_note_event(
        &branch,
        Some(&evt.hash),
        "user",
        "completed auth, next OAuth",
        &tags_s,
    )
    .unwrap();
    ledger.append_event(&evt2).unwrap();

    let (decisions, notes) =
        collect_session_ledger_extras(workspace.to_str().unwrap(), Some(&decision_ts));
    assert_eq!(decisions.len(), 1);
    assert!(decisions[0].contains("auth=jwt"), "got: {}", decisions[0]);
    assert!(decisions[0].contains("stateless"), "got: {}", decisions[0]);
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("completed auth"), "got: {}", notes[0]);
}

#[test]
fn collect_session_ledger_extras_excludes_digest_notes() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_path_buf();
    let paths = edda_ledger::EddaPaths::discover(&workspace);
    edda_ledger::ledger::init_workspace(&paths).unwrap();
    edda_ledger::ledger::init_head(&paths, "main").unwrap();
    edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let branch = ledger.head_branch().unwrap();

    // Write an auto-generated digest note (source: "bridge:session_digest")
    let tags = vec!["session_digest".to_string()];
    let mut evt = edda_core::event::new_note_event(
        &branch,
        None,
        "system",
        "Session abc: 10 tool calls",
        &tags,
    )
    .unwrap();
    evt.payload["source"] = serde_json::json!("bridge:session_digest");
    edda_core::event::finalize_event(&mut evt).unwrap();
    let ts = evt.ts.clone();
    ledger.append_event(&evt).unwrap();

    let (decisions, notes) = collect_session_ledger_extras(workspace.to_str().unwrap(), Some(&ts));
    assert!(decisions.is_empty(), "auto-digest should be excluded");
    assert!(notes.is_empty(), "auto-digest should be excluded");
}

#[test]
fn collect_session_ledger_extras_no_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    // No .edda/ directory
    let (decisions, notes) =
        collect_session_ledger_extras(tmp.path().to_str().unwrap(), Some("2026-02-17T10:00:00Z"));
    assert!(decisions.is_empty());
    assert!(notes.is_empty());
}

#[test]
fn digest_skips_empty_session() {
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    // Write a session with only SessionStart (no tool calls, no user prompts)
    write_store_session_ledger(
        &project_id,
        "sess-empty-skip",
        &[make_envelope("SessionStart", "", serde_json::json!({}))],
    );

    let result =
        digest_previous_sessions(&project_id, "current", workspace.to_str().unwrap(), 2000);

    // Should skip (NoPending), not write to workspace ledger
    assert!(matches!(result, DigestResult::NoPending), "got: {result:?}");

    // Workspace ledger should have 0 events
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    assert_eq!(ledger.iter_events().unwrap().len(), 0);

    // Session ledger file should be cleaned up
    let session_path = edda_store::project_dir(&project_id)
        .join("ledger")
        .join("sess-empty-skip.jsonl");
    assert!(
        !session_path.exists(),
        "empty session ledger should be removed"
    );

    // State should mark as processed to avoid re-processing
    let state = load_digest_state(&project_id);
    assert_eq!(state.session_id, "sess-empty-skip");
}

// ── Recall Rate tests ──

#[test]
fn digest_payload_has_recall_fields() {
    let stats = SessionStats {
        tool_calls: 10,
        nudge_count: 3,
        decide_count: 1,
        ..Default::default()
    };
    let event = build_digest_event("sess-recall", &stats, "main", None, &[]).unwrap();
    assert_eq!(event.payload["session_stats"]["nudge_count"], 3);
    assert_eq!(event.payload["session_stats"]["decide_count"], 1);
}

#[test]
fn digest_event_contains_notes() {
    let stats = SessionStats {
        tool_calls: 5,
        outcome: SessionOutcome::Completed,
        ..Default::default()
    };
    let notes = vec![
        "Switched to JWT auth approach".to_string(),
        "Need to revisit caching strategy".to_string(),
    ];
    let event = build_digest_event("sess-notes", &stats, "main", None, &notes).unwrap();

    let payload_notes = event.payload["session_stats"]["notes"]
        .as_array()
        .expect("notes should be an array");
    assert_eq!(payload_notes.len(), 2);
    assert_eq!(
        payload_notes[0].as_str().unwrap(),
        "Switched to JWT auth approach"
    );
    assert_eq!(
        payload_notes[1].as_str().unwrap(),
        "Need to revisit caching strategy"
    );
}

#[test]
fn digest_event_empty_notes_backward_compat() {
    let stats = SessionStats::default();
    let event = build_digest_event("sess-no-notes", &stats, "main", None, &[]).unwrap();

    let payload_notes = event.payload["session_stats"]["notes"]
        .as_array()
        .expect("notes should be an array even when empty");
    assert!(payload_notes.is_empty());
}

#[test]
fn prev_digest_has_recall_fields() {
    let pid = "test_prev_digest_recall";
    let _ = edda_store::ensure_dirs(pid);

    let stats = SessionStats {
        nudge_count: 5,
        decide_count: 2,
        duration_minutes: 15,
        outcome: SessionOutcome::Completed,
        ..Default::default()
    };
    write_prev_digest(pid, "test-recall", &stats, vec![], vec![]);

    let digest = read_prev_digest(pid).expect("should read prev_digest");
    assert_eq!(digest.nudge_count, 5);
    assert_eq!(digest.decide_count, 2);

    let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── signal_count + deps_added tests ──

#[test]
fn digest_payload_has_signal_count() {
    let stats = SessionStats {
        tool_calls: 10,
        nudge_count: 3,
        decide_count: 1,
        signal_count: 5,
        ..Default::default()
    };
    let event = build_digest_event("sess-signal", &stats, "main", None, &[]).unwrap();
    assert_eq!(event.payload["session_stats"]["signal_count"], 5);
}

#[test]
fn digest_extracts_deps_added() {
    let dir = tempfile::tempdir().unwrap();
    let lines = vec![
        make_envelope(
            "PostToolUse",
            "Bash",
            serde_json::json!({
                "tool_input": { "command": "cargo add serde" }
            }),
        ),
        make_envelope(
            "PostToolUse",
            "Bash",
            serde_json::json!({
                "tool_input": { "command": "npm install express" }
            }),
        ),
        make_envelope(
            "PostToolUse",
            "Bash",
            serde_json::json!({
                "tool_input": { "command": "pnpm add zod" }
            }),
        ),
        // Bare npm install (no package) → NOT captured
        make_envelope(
            "PostToolUse",
            "Bash",
            serde_json::json!({
                "tool_input": { "command": "npm install" }
            }),
        ),
    ];
    let path = write_session_ledger(dir.path(), &lines);
    let stats = extract_stats(&path).unwrap();
    assert_eq!(stats.deps_added, vec!["serde", "express", "zod"]);
}

#[test]
fn digest_extracts_deps_added_dedup() {
    let dir = tempfile::tempdir().unwrap();
    let lines = vec![
        make_envelope(
            "PostToolUse",
            "Bash",
            serde_json::json!({
                "tool_input": { "command": "cargo add serde" }
            }),
        ),
        make_envelope(
            "PostToolUse",
            "Bash",
            serde_json::json!({
                "tool_input": { "command": "cargo add serde --features derive" }
            }),
        ),
    ];
    let path = write_session_ledger(dir.path(), &lines);
    let stats = extract_stats(&path).unwrap();
    assert_eq!(
        stats.deps_added,
        vec!["serde"],
        "duplicate deps should be deduped"
    );
}

// ── Passive harvest tests ──

#[test]
fn passive_harvest_writes_inferred_decision() {
    let dir = tempfile::tempdir().unwrap();
    let paths = edda_ledger::EddaPaths::discover(dir.path());
    edda_ledger::ledger::init_workspace(&paths).unwrap();
    edda_ledger::ledger::init_head(&paths, "main").unwrap();
    edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
    let ledger = edda_ledger::Ledger::open(dir.path()).unwrap();

    let stats = SessionStats {
        deps_added: vec!["jsonwebtoken".to_string()],
        commits_made: vec!["feat: add auth middleware".to_string()],
        tasks_snapshot: vec![DigestTaskSnapshot {
            subject: "Add JWT authentication".to_string(),
            status: "in_progress".to_string(),
        }],
        ..Default::default()
    };

    let ids = harvest_inferred_decisions(
        "sess-harvest",
        &stats,
        &[], // no decisions recorded
        &ledger,
        "main",
        None,
    );

    assert_eq!(ids.len(), 1, "should write one inferred decision");

    // Verify the event in the ledger
    let events = ledger.iter_events().unwrap();
    let last = events.iter().last().unwrap();
    assert_eq!(last.event_type, "note");
    assert_eq!(last.payload["source"], "bridge:passive_harvest");
    assert_eq!(last.payload["decision"]["key"], "dep.jsonwebtoken");
    assert_eq!(last.payload["decision"]["value"], "jsonwebtoken");

    let tags: Vec<&str> = last.payload["tags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(tags.contains(&"decision"));
    assert!(tags.contains(&"inferred"));
}

#[test]
fn passive_harvest_skips_already_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let paths = edda_ledger::EddaPaths::discover(dir.path());
    edda_ledger::ledger::init_workspace(&paths).unwrap();
    edda_ledger::ledger::init_head(&paths, "main").unwrap();
    edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
    let ledger = edda_ledger::Ledger::open(dir.path()).unwrap();

    let stats = SessionStats {
        deps_added: vec!["serde".to_string()],
        ..Default::default()
    };

    // Agent already recorded a decision mentioning "serde"
    let decisions = vec!["dep.serde=serde (serialization)".to_string()];

    let ids = harvest_inferred_decisions("sess-skip", &stats, &decisions, &ledger, "main", None);

    assert!(
        ids.is_empty(),
        "should NOT write inferred decision when already recorded"
    );
}

#[test]
fn passive_harvest_includes_context_hint() {
    let stats = SessionStats {
        tasks_snapshot: vec![DigestTaskSnapshot {
            subject: "Add JWT authentication".to_string(),
            status: "in_progress".to_string(),
        }],
        commits_made: vec!["feat: add auth middleware".to_string()],
        ..Default::default()
    };

    let hint = build_context_hint(&stats);
    assert!(
        hint.contains("Add JWT authentication"),
        "should contain task subject"
    );
    assert!(
        hint.contains("feat: add auth middleware"),
        "should contain commit message"
    );
}

#[test]
fn passive_harvest_context_hint_fallback() {
    let stats = SessionStats::default();
    let hint = build_context_hint(&stats);
    assert_eq!(hint, "(auto-inferred)");
}

#[test]
fn passive_harvest_empty_deps_no_events() {
    let dir = tempfile::tempdir().unwrap();
    let paths = edda_ledger::EddaPaths::discover(dir.path());
    edda_ledger::ledger::init_workspace(&paths).unwrap();
    edda_ledger::ledger::init_head(&paths, "main").unwrap();
    edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
    let ledger = edda_ledger::Ledger::open(dir.path()).unwrap();

    let stats = SessionStats::default(); // no deps_added

    let ids = harvest_inferred_decisions("sess-empty", &stats, &[], &ledger, "main", None);

    assert!(ids.is_empty(), "empty deps_added should produce no events");
}

#[test]
fn prev_digest_has_signal_count() {
    let pid = "test_prev_digest_signal";
    let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);

    let stats = SessionStats {
        nudge_count: 3,
        decide_count: 1,
        signal_count: 5,
        duration_minutes: 15,
        outcome: SessionOutcome::Completed,
        ..Default::default()
    };
    write_prev_digest(pid, "test-signal", &stats, vec![], vec![]);

    let digest = read_prev_digest(pid).expect("should read prev_digest");
    assert_eq!(digest.signal_count, 5);

    let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn prev_digest_has_tool_breakdown() {
    let pid = "test_prev_digest_tool_bd";
    let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);

    let mut breakdown = BTreeMap::new();
    breakdown.insert("Read".into(), 15);
    breakdown.insert("Edit".into(), 8);
    breakdown.insert("Grep".into(), 5);
    breakdown.insert("Bash".into(), 3);

    let stats = SessionStats {
        tool_calls: 31,
        tool_call_breakdown: breakdown,
        duration_minutes: 20,
        outcome: SessionOutcome::Completed,
        ..Default::default()
    };
    write_prev_digest(pid, "test-tool-bd", &stats, vec![], vec![]);

    let digest = read_prev_digest(pid).expect("should read prev_digest");
    assert_eq!(digest.tool_call_breakdown.get("Read"), Some(&15));
    assert_eq!(digest.tool_call_breakdown.get("Edit"), Some(&8));
    assert_eq!(digest.tool_call_breakdown.get("Grep"), Some(&5));
    assert_eq!(digest.tool_call_breakdown.get("Bash"), Some(&3));
    // edit_ratio = 8 / 31
    assert!((digest.edit_ratio - 8.0 / 31.0).abs() < 1e-6);
    // search_ratio = (15 + 5) / 31
    assert!((digest.search_ratio - 20.0 / 31.0).abs() < 1e-6);

    let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn classify_docs_only() {
    let mut stats = SessionStats {
        tool_calls: 10,
        files_modified: vec!["README.md".to_string(), "docs/api.md".to_string()],
        ..Default::default()
    };
    stats.tool_call_breakdown.insert("Edit".to_string(), 5);
    assert_eq!(classify_activity(&stats), ActivityType::Docs);
}

#[test]
fn classify_research_heavy() {
    let mut stats = SessionStats {
        tool_calls: 20,
        ..Default::default()
    };
    stats.tool_call_breakdown.insert("Read".to_string(), 12);
    stats.tool_call_breakdown.insert("Grep".to_string(), 5);
    assert_eq!(classify_activity(&stats), ActivityType::Research);
}

#[test]
fn classify_debug_failures() {
    let mut stats = SessionStats {
        tool_calls: 15,
        tool_failures: 5,
        ..Default::default()
    };
    stats.tool_call_breakdown.insert("Bash".to_string(), 10);
    assert_eq!(classify_activity(&stats), ActivityType::Debug);
}

#[test]
fn classify_feature_with_commits() {
    let mut stats = SessionStats {
        tool_calls: 20,
        commits_made: vec!["feat: add new feature".to_string()],
        ..Default::default()
    };
    stats.tool_call_breakdown.insert("Edit".to_string(), 8);
    assert_eq!(classify_activity(&stats), ActivityType::Feature);
}

#[test]
fn classify_fix_with_bug_keyword() {
    let mut stats = SessionStats {
        tool_calls: 20,
        commits_made: vec!["fix: resolve bug in auth".to_string()],
        ..Default::default()
    };
    stats.tool_call_breakdown.insert("Edit".to_string(), 8);
    assert_eq!(classify_activity(&stats), ActivityType::Fix);
}

#[test]
fn classify_ops_bash_heavy() {
    let mut stats = SessionStats {
        tool_calls: 10,
        ..Default::default()
    };
    stats.tool_call_breakdown.insert("Bash".to_string(), 6);
    assert_eq!(classify_activity(&stats), ActivityType::Ops);
}

#[test]
fn classify_chat_low_tools() {
    let stats = SessionStats {
        tool_calls: 3,
        user_prompts: 5,
        ..Default::default()
    };
    assert_eq!(classify_activity(&stats), ActivityType::Chat);
}

#[test]
fn classify_unknown_no_activity() {
    let stats = SessionStats::default();
    assert_eq!(classify_activity(&stats), ActivityType::Unknown);
}
