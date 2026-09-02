use std::collections::BTreeMap;
use std::path::Path;

use super::extract::*;
use super::helpers::*;
use super::orchestrate::*;
use super::*;

use std::io::Write;
use std::sync::{MutexGuard, PoisonError};

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    previous_store_root: Option<std::ffi::OsString>,
    _store_root: tempfile::TempDir,
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.previous_store_root.take() {
            Some(root) => std::env::set_var("EDDA_STORE_ROOT", root),
            None => std::env::remove_var("EDDA_STORE_ROOT"),
        }
    }
}

impl EnvGuard {
    fn new(lock: MutexGuard<'static, ()>) -> Self {
        let previous_store_root = std::env::var_os("EDDA_STORE_ROOT");
        let store_root = tempfile::tempdir().unwrap();
        std::env::set_var("EDDA_STORE_ROOT", store_root.path());
        Self {
            _lock: lock,
            previous_store_root,
            _store_root: store_root,
        }
    }
}

fn env_guard() -> EnvGuard {
    let lock = crate::ENV_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    EnvGuard::new(lock)
}

#[test]
fn env_guard_isolates_store_root_and_restores_on_drop() {
    let lock = crate::ENV_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let before = std::env::var_os("EDDA_STORE_ROOT");
    let guard = EnvGuard::new(lock);
    let inside = std::env::var_os("EDDA_STORE_ROOT");
    assert_ne!(inside, before, "fixture needs a private store root");
    drop(guard);

    let _lock = crate::ENV_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    assert_eq!(
        std::env::var_os("EDDA_STORE_ROOT"),
        before,
        "fixture must restore the caller environment"
    );
}

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

    // A 35-minute silent gap exceeds the 30-minute idle cap (GH-578):
    // only 30 minutes of the span counts as activity.
    assert_eq!(stats.duration_minutes, 30);
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
    let event = build_digest_event("sess-outcome", &stats, "main", None, &[], None).unwrap();
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

    let event = build_digest_event("sess-tasks", &stats, "main", None, &[], None).unwrap();

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
    std::fs::create_dir_all(workspace.join(".git")).unwrap();
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
    let _env = env_guard();
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
    let _env = env_guard();
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
    let _env = env_guard();
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

    // Round-1 P0-1: the session ledger file is NEVER deleted
    assert!(
        ledger_dir.join("sess-once.jsonl").exists(),
        "session ledger file must be kept after successful digest"
    );

    // Digest again — should be NoPending (watermark covers the file)
    let r2 = digest_previous_sessions(&project_id, "current", workspace.to_str().unwrap(), 2000);
    assert!(matches!(r2, DigestResult::NoPending));

    // Workspace ledger should still have exactly 1 event
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    assert_eq!(ledger.iter_events().unwrap().len(), 1);
}

#[test]
fn digest_no_reduplicate_across_sessions() {
    let _env = env_guard();
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

    // Round-1 P0-1: all 3 session ledger files are kept
    assert!(ledger_dir.join("sess-001.jsonl").exists());
    assert!(ledger_dir.join("sess-002.jsonl").exists());
    assert!(ledger_dir.join("sess-003.jsonl").exists());

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
    let _env = env_guard();
    // Hermetic isolation: `find_root` climbs parents, so a bare tempdir
    // would silently resolve to whatever workspace exists above %TEMP%
    // (a probe scratch or the fleet coordination workspace) and the
    // digest would write THERE. Instead, anchor the climb at this test's
    // own directory and make the ledger deterministically unopenable:
    // `.edda/` exists (so find_root stops here) but `ledger.db` is a
    // directory (so SqliteStore cannot open it). The digest then reports
    // an unreachable-ledger failure and writes nowhere.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".edda")).unwrap();
    std::fs::create_dir(tmp.path().join(".edda").join("ledger.db")).unwrap();
    // Just a store, with the anchored-but-unopenable workspace as cwd
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
    let _env = env_guard();
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
    let _env = env_guard();
    let project_id = "test_state_rt";
    // Writes to the real per-user store and, unlike its neighbours, never
    // cleaned up — so it left `projects/test_state_rt` on every developer
    // machine that ever ran the suite (GH-415). Clearing first also makes it
    // independent of whatever a previous run left behind.
    let _ = std::fs::remove_dir_all(edda_store::project_dir(project_id));
    let _ = edda_store::ensure_dirs(project_id);

    let state = DigestState {
        session_id: "sess-123".to_string(),
        digested_at: "2026-02-14T10:00:00Z".to_string(),
        event_id: "evt_abc".to_string(),
        retry_count: 0,
        pending_session_id: String::new(),
        last_error: String::new(),
        sessions: BTreeMap::from([(
            "sess-123".to_string(),
            DigestedSession {
                offset: 42,
                prefix_hash: String::new(),
                event_id: "evt_abc".to_string(),
                digested_at: "2026-02-14T10:00:00Z".to_string(),
            },
        )]),
        digested: Vec::new(),
    };
    save_digest_state(project_id, &state).unwrap();

    let loaded = load_digest_state(project_id);
    let entry = loaded.sessions.get("sess-123").expect("sess-123 watermark");
    assert_eq!(entry.offset, 42);
    assert_eq!(entry.event_id, "evt_abc");
    assert!(
        loaded.digested.is_empty(),
        "deprecated ids are never written"
    );

    let _ = std::fs::remove_dir_all(edda_store::project_dir(project_id));

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
    let _env = env_guard();
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
    let _env = env_guard();
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
    let _env = env_guard();
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

// ── GH-578 regression tests ──

#[test]
fn manual_digest_zero_call_session_writes_no_event() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    // Session with a user prompt but no tool calls: nothing to summarize.
    write_store_session_ledger(
        &project_id,
        "sess-zero-call",
        &[make_envelope("UserPromptSubmit", "", serde_json::json!({}))],
    );

    let event_id = digest_session_manual(
        &project_id,
        "sess-zero-call",
        workspace.to_str().unwrap(),
        true,
    )
    .unwrap();
    assert!(
        event_id.is_empty(),
        "zero-call session must not write a digest"
    );

    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let events = ledger.iter_events().unwrap();
    assert_eq!(
        events.len(),
        0,
        "zero-call session must not write a ledger event"
    );
}

#[test]
fn manual_digest_same_session_twice_writes_one_event() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    write_store_session_ledger(
        &project_id,
        "sess-twice",
        &[
            make_envelope("PostToolUse", "Edit", serde_json::json!({})),
            make_envelope("PostToolUse", "Bash", serde_json::json!({})),
        ],
    );

    let id1 = digest_session_manual(&project_id, "sess-twice", workspace.to_str().unwrap(), true)
        .unwrap();
    // Second call (e.g. the bridge firing again on agent_end) must be a no-op
    // returning the same session's own event id (round-1 P1-4).
    let id2 = digest_session_manual(&project_id, "sess-twice", workspace.to_str().unwrap(), true)
        .unwrap();
    assert_eq!(
        id2, id1,
        "retry must return the same session's own event id"
    );
    assert!(!id1.is_empty());

    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let events = ledger.iter_events().unwrap();
    let digests = events
        .iter()
        .filter(|e| e.payload["source"] == "bridge:session_digest")
        .count();
    assert_eq!(digests, 1, "same session must not be digested twice");
}

#[test]
fn auto_digest_zero_call_session_writes_no_event() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    // Chat-only session: 1 user prompt, 0 tool calls.
    write_store_session_ledger(
        &project_id,
        "prev-chat-only",
        &[make_envelope("UserPromptSubmit", "", serde_json::json!({}))],
    );

    let result = digest_previous_sessions_with_opts(
        &project_id,
        "current",
        workspace.to_str().unwrap(),
        2000,
        false,
    );
    assert!(matches!(result, DigestResult::NoPending));

    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let events = ledger.iter_events().unwrap();
    assert_eq!(
        events.len(),
        0,
        "zero-call session must not write a ledger event"
    );
}

#[test]
fn digest_duration_excludes_idle_gap() {
    let tmp = tempfile::tempdir().unwrap();
    let lines = vec![
        make_envelope_at(
            "PostToolUse",
            "Bash",
            "2026-02-14T10:00:00Z",
            serde_json::json!({}),
        ),
        make_envelope_at(
            "PostToolUse",
            "Bash",
            "2026-02-14T10:05:00Z",
            serde_json::json!({}),
        ),
        // 10 idle days later, two more events one minute apart
        make_envelope_at(
            "PostToolUse",
            "Bash",
            "2026-02-24T10:05:00Z",
            serde_json::json!({}),
        ),
        make_envelope_at(
            "PostToolUse",
            "Bash",
            "2026-02-24T10:06:00Z",
            serde_json::json!({}),
        ),
    ];
    let path = write_session_ledger(tmp.path(), &lines);
    let stats = extract_stats(&path).unwrap();

    // 5m active + 30m idle-gap cap + 1m active = 36, not 1440*10+6
    assert_eq!(stats.duration_minutes, 36);
}

// ── PrevDigest tests ──

#[test]
fn prev_digest_roundtrip() {
    let _env = env_guard();
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
    let _env = env_guard();
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
    let _env = env_guard();
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
    let _env = env_guard();
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
    let _env = env_guard();
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

    // Round-1 P0-2: the empty session's ledger file is kept, not removed
    let session_path = edda_store::project_dir(&project_id)
        .join("ledger")
        .join("sess-empty-skip.jsonl");
    assert!(
        session_path.exists(),
        "empty session ledger must not be deleted"
    );

    // State should mark as processed to avoid re-processing
    let state = load_digest_state(&project_id);
    assert_eq!(state.session_id, "sess-empty-skip");
    assert!(state.sessions.contains_key("sess-empty-skip"));
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
    let event = build_digest_event("sess-recall", &stats, "main", None, &[], None).unwrap();
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
    let event = build_digest_event("sess-notes", &stats, "main", None, &notes, None).unwrap();

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
    let event = build_digest_event("sess-no-notes", &stats, "main", None, &[], None).unwrap();

    let payload_notes = event.payload["session_stats"]["notes"]
        .as_array()
        .expect("notes should be an array even when empty");
    assert!(payload_notes.is_empty());
}

#[test]
fn prev_digest_has_recall_fields() {
    let _env = env_guard();
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
    let event = build_digest_event("sess-signal", &stats, "main", None, &[], None).unwrap();
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
    let _env = env_guard();
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
    let _env = env_guard();
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

// ── PR #607 round-1 review: failing-first regression tests ──

fn write_store_session_ledger_bytes(
    project_id: &str,
    session_id: &str,
    chunks: &[&[u8]],
) -> std::path::PathBuf {
    let dir = edda_store::project_dir(project_id).join("ledger");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{session_id}.jsonl"));
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap();
    for chunk in chunks {
        f.write_all(chunk).unwrap();
    }
    path
}

// P0-1/P0-2 (dissolved by the no-deletion ruling): the digest paths must
// never delete the session ledger. A ledger may belong to a live producer
// (the motivating case, oc-test-3, was appended for ten days) and may end
// in a truncated, concurrently-written final line.

#[test]
fn manual_digest_never_deletes_session_ledger() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    write_store_session_ledger(
        &project_id,
        "sess-keep",
        &[make_envelope("PostToolUse", "Edit", serde_json::json!({}))],
    );
    let path = edda_store::project_dir(&project_id)
        .join("ledger")
        .join("sess-keep.jsonl");

    digest_session_manual(&project_id, "sess-keep", workspace.to_str().unwrap(), true).unwrap();

    assert!(
        path.exists(),
        "manual digest must not delete the session ledger (round-1 P0-1)"
    );
}

#[test]
fn auto_digest_never_deletes_session_ledger() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    write_store_session_ledger(
        &project_id,
        "sess-auto-keep",
        &[make_envelope("PostToolUse", "Bash", serde_json::json!({}))],
    );
    let path = edda_store::project_dir(&project_id)
        .join("ledger")
        .join("sess-auto-keep.jsonl");

    let result =
        digest_previous_sessions(&project_id, "current", workspace.to_str().unwrap(), 2000);
    assert!(matches!(result, DigestResult::Written { .. }));

    assert!(
        path.exists(),
        "auto digest must not delete the session ledger (round-1 P0-1)"
    );
}

#[test]
fn auto_digest_zero_call_keeps_session_ledger() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    // Chat-only session: zero tool calls, zero failures.
    write_store_session_ledger(
        &project_id,
        "sess-chat-only",
        &[make_envelope("UserPromptSubmit", "", serde_json::json!({}))],
    );
    let path = edda_store::project_dir(&project_id)
        .join("ledger")
        .join("sess-chat-only.jsonl");

    let result =
        digest_previous_sessions(&project_id, "current", workspace.to_str().unwrap(), 2000);
    assert!(matches!(result, DigestResult::NoPending));

    assert!(
        path.exists(),
        "zero-call auto digest must not delete the session ledger (round-1 P0-2)"
    );
}

// P1-4: a retry of a remembered session must return THAT session's own
// digest event id, not the globally-latest one (which belongs to B).

#[test]
fn manual_digest_retry_returns_that_sessions_own_event_id() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    for sid in ["sess-aaa", "sess-bbb"] {
        write_store_session_ledger(
            &project_id,
            sid,
            &[make_envelope("PostToolUse", "Edit", serde_json::json!({}))],
        );
    }

    let id_a =
        digest_session_manual(&project_id, "sess-aaa", workspace.to_str().unwrap(), true).unwrap();
    let id_b =
        digest_session_manual(&project_id, "sess-bbb", workspace.to_str().unwrap(), true).unwrap();
    assert_ne!(id_a, id_b);

    // Retry of A (the bridge fires agent_end again): must return A's id.
    let retry =
        digest_session_manual(&project_id, "sess-aaa", workspace.to_str().unwrap(), true).unwrap();
    assert_eq!(retry, id_a, "retry of A must return A's own event id");
}

// Watermark: a session that grew digests only its delta.

#[test]
fn manual_digest_grown_session_digests_only_delta() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    let lines = vec![
        make_envelope_at(
            "PostToolUse",
            "Edit",
            "2026-02-14T10:00:00Z",
            serde_json::json!({}),
        ),
        make_envelope_at(
            "PostToolUse",
            "Edit",
            "2026-02-14T10:01:00Z",
            serde_json::json!({}),
        ),
    ];
    write_store_session_ledger(&project_id, "sess-grow", &lines);
    let path = edda_store::project_dir(&project_id)
        .join("ledger")
        .join("sess-grow.jsonl");

    digest_session_manual(&project_id, "sess-grow", workspace.to_str().unwrap(), true).unwrap();

    // The producer appends more work later (long-lived session).
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    for ts in ["2026-02-15T10:00:00Z", "2026-02-15T10:01:00Z"] {
        let e = make_envelope_at("PostToolUse", "Edit", ts, serde_json::json!({}));
        writeln!(f, "{}", serde_json::to_string(&e).unwrap()).unwrap();
    }
    drop(f);

    digest_session_manual(&project_id, "sess-grow", workspace.to_str().unwrap(), true).unwrap();

    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let events = ledger.iter_events().unwrap();
    let digests: Vec<_> = events
        .iter()
        .filter(|e| e.payload["source"] == "bridge:session_digest")
        .collect();
    assert_eq!(digests.len(), 2, "grown session gets a second delta digest");
    assert_eq!(
        digests[0].payload["session_stats"]["tool_calls"], 2,
        "first digest covers the first span"
    );
    assert_eq!(
        digests[1].payload["session_stats"]["tool_calls"], 2,
        "second digest must cover only the delta, not the whole span again"
    );
}

// P0-2: a truncated / concurrently-written final line is not consumed and
// not destroyed; it is digested once its write completes.

#[test]
fn manual_digest_truncated_final_line_is_not_consumed_early() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    let e1 = make_envelope_at(
        "PostToolUse",
        "Edit",
        "2026-02-14T10:00:00Z",
        serde_json::json!({}),
    );
    let e2 = make_envelope_at(
        "PostToolUse",
        "Edit",
        "2026-02-14T10:01:00Z",
        serde_json::json!({}),
    );
    let e3 = make_envelope_at(
        "PostToolUse",
        "Edit",
        "2026-02-14T10:02:00Z",
        serde_json::json!({}),
    );
    let e3_line = serde_json::to_string(&e3).unwrap();

    let mut full = Vec::new();
    for e in [&e1, &e2] {
        full.extend_from_slice(format!("{}\n", serde_json::to_string(e).unwrap()).as_bytes());
    }
    // A third line, truncated mid-write (no trailing newline).
    let truncated_prefix: String = e3_line.chars().take(e3_line.len() / 2).collect();

    let path = write_store_session_ledger_bytes(
        &project_id,
        "sess-trunc",
        &[full.as_slice(), truncated_prefix.as_bytes()],
    );

    let _id = digest_session_manual(&project_id, "sess-trunc", workspace.to_str().unwrap(), true)
        .unwrap();
    assert!(
        path.exists(),
        "digest must not delete a ledger with a truncated final line (round-1 P0-2)"
    );
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let events = ledger.iter_events().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].payload["session_stats"]["tool_calls"], 2,
        "digest must cover only the complete prefix"
    );

    // The producer finishes writing the third line.
    let tail = &e3_line[truncated_prefix.len()..];
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    f.write_all(tail.as_bytes()).unwrap();
    writeln!(f).unwrap();
    drop(f);

    digest_session_manual(&project_id, "sess-trunc", workspace.to_str().unwrap(), true).unwrap();
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let events = ledger.iter_events().unwrap();
    let digests: Vec<_> = events
        .iter()
        .filter(|e| e.payload["source"] == "bridge:session_digest")
        .collect();
    assert_eq!(digests.len(), 2, "completed line is digested exactly once");
    assert_eq!(
        digests[1].payload["session_stats"]["tool_calls"], 1,
        "second digest covers only the completed line"
    );
}

// P1-2 / round-2 ruling: legacy migration cannot prove what the legacy
// build had consumed at the moment it wrote its state, so it must RE-READ
// rather than seed the offset at current EOF — content appended between
// the legacy state write and the first post-upgrade load must not be
// swallowed.
#[test]
fn legacy_migration_re_reads_appended_tail() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    // One tool line on disk when the legacy build digests and records state.
    write_store_session_ledger(
        &project_id,
        "sess-legacy",
        &[make_envelope("PostToolUse", "Edit", serde_json::json!({}))],
    );

    // Legacy state shape: single session slot, no per-session map.
    let state_dir = edda_store::project_dir(&project_id).join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    let legacy = serde_json::json!({
        "session_id": "sess-legacy",
        "digested_at": "2026-02-14T09:00:00Z",
        "event_id": "evt_legacy000000000000000000000000"
    });
    std::fs::write(
        state_dir.join("last_digested_session.json"),
        serde_json::to_string_pretty(&legacy).unwrap(),
    )
    .unwrap();

    // The producer appends more work AFTER the legacy state was written and
    // BEFORE the first post-upgrade load. Migration must not treat this
    // tail as consumed (round-2 finding 2: seeding the offset at current
    // EOF silently discarded it).
    let path = edda_store::project_dir(&project_id)
        .join("ledger")
        .join("sess-legacy.jsonl");
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    let e = make_envelope_at(
        "PostToolUse",
        "Edit",
        "2026-02-14T11:00:00Z",
        serde_json::json!({}),
    );
    writeln!(f, "{}", serde_json::to_string(&e).unwrap()).unwrap();
    drop(f);

    digest_session_manual(
        &project_id,
        "sess-legacy",
        workspace.to_str().unwrap(),
        true,
    )
    .unwrap();

    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let digests: Vec<_> = ledger
        .iter_events()
        .unwrap()
        .into_iter()
        .filter(|e| e.payload["source"] == "bridge:session_digest")
        .collect();
    assert_eq!(
        digests.len(),
        1,
        "migration must re-read: the post-legacy tail must be digested (round-2 finding 2)"
    );
    assert_eq!(
        digests[0].payload["session_stats"]["tool_calls"], 2,
        "the re-read must cover the tail appended after the legacy state write"
    );

    // At-least-once, not at-least-twice: once the re-read watermark is
    // durable, a retry is a no-op.
    digest_session_manual(
        &project_id,
        "sess-legacy",
        workspace.to_str().unwrap(),
        true,
    )
    .unwrap();
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let digests: Vec<_> = ledger
        .iter_events()
        .unwrap()
        .into_iter()
        .filter(|e| e.payload["source"] == "bridge:session_digest")
        .collect();
    assert_eq!(
        digests.len(),
        1,
        "the migration re-read happens exactly once"
    );
}

// P1-3 (round-1 fix, re-pinned under the round-2 identity ruling): the
// per-session map never evicts, and a legacy-listed session is re-read
// AT MOST ONCE — after the re-read the watermark carries an identity
// proof, so repeats cannot resurrect a digest loop.

#[test]
fn legacy_re_read_cannot_resurrect_a_digest_loop() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    let n: usize = 65;
    let ids: Vec<String> = (1..=n).map(|i| format!("seed-{i:03}")).collect();
    for id in &ids {
        write_store_session_ledger(
            &project_id,
            id,
            &[make_envelope("PostToolUse", "Edit", serde_json::json!({}))],
        );
    }

    // Seed state remembering the first 64 sessions via the deprecated list.
    let state_dir = edda_store::project_dir(&project_id).join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    let remembered: Vec<serde_json::Value> = ids[..n - 1]
        .iter()
        .map(|id| serde_json::Value::String(id.clone()))
        .collect();
    std::fs::write(
        state_dir.join("last_digested_session.json"),
        serde_json::json!({ "digested": remembered }).to_string(),
    )
    .unwrap();

    // Digest session 65.
    digest_session_manual(&project_id, &ids[64], workspace.to_str().unwrap(), true).unwrap();

    // Legacy-listed session 1: migration cannot prove what was consumed, so
    // the first post-upgrade digest re-reads it once — and only once.
    digest_session_manual(&project_id, &ids[0], workspace.to_str().unwrap(), true).unwrap();
    digest_session_manual(&project_id, &ids[0], workspace.to_str().unwrap(), true).unwrap();

    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let count = ledger
        .iter_events()
        .unwrap()
        .iter()
        .filter(|e| e.payload["session_id"] == ids[0])
        .count();
    assert_eq!(
        count, 1,
        "a legacy-listed session is re-read at most once; repeats must be no-ops"
    );
}

// P1-1: extraction must recognize the non-Claude tool event names the
// bridges actually persist, or real work is classified zero-call.

#[test]
fn extract_recognizes_openclaw_after_tool_call() {
    let tmp = tempfile::tempdir().unwrap();
    let envelope = serde_json::json!({
        "ts": "2026-02-14T10:00:00Z",
        "project_id": "p",
        "session_id": "oc-1",
        "hook_event_name": "after_tool_call",
        "agent_id": "main",
        "event_data": {
            "tool_name": "bash",
            "tool_input": { "command": "git commit -m 'feat: bridge digest'" }
        }
    });
    let path = write_session_ledger(tmp.path(), &[envelope]);
    let stats = extract_stats(&path).unwrap();
    assert_eq!(
        stats.tool_calls, 1,
        "after_tool_call must count as a tool call"
    );
    assert_eq!(
        stats.commits_made.len(),
        1,
        "tool data nested under event_data must be parsed"
    );
    assert_eq!(stats.commits_made[0], "feat: bridge digest");
}

#[test]
fn extract_recognizes_openclaw_after_tool_call_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let envelope = serde_json::json!({
        "ts": "2026-02-14T10:00:00Z",
        "project_id": "p",
        "session_id": "oc-2",
        "hook_event_name": "after_tool_call",
        "event_data": {
            "tool_name": "bash",
            "tool_input": { "command": "cargo build" },
            "error": "Exit code 101"
        }
    });
    let path = write_session_ledger(tmp.path(), &[envelope]);
    let stats = extract_stats(&path).unwrap();
    assert_eq!(
        stats.tool_failures, 1,
        "a failed after_tool_call is a failure, not a call"
    );
    assert_eq!(stats.failed_commands, vec!["cargo build"]);
}

#[test]
fn extract_recognizes_cursor_post_tool_use() {
    let tmp = tempfile::tempdir().unwrap();
    let envelope = serde_json::json!({
        "ts": "2026-02-14T10:00:00Z",
        "project_id": "p",
        "session_id": "cur-1",
        "hook_event_name": "postToolUse",
        "cwd": "C:/work/proj",
        "tool_name": "edit_file",
        "tool_input": { "file_path": "src/main.rs" },
        "bridge": "cursor"
    });
    let path = write_session_ledger(tmp.path(), &[envelope]);
    let stats = extract_stats(&path).unwrap();
    assert_eq!(stats.tool_calls, 1, "postToolUse must count as a tool call");
    assert_eq!(stats.files_modified, vec!["src/main.rs"]);
}

#[test]
fn extract_recognizes_hermes_post_tool_call() {
    let tmp = tempfile::tempdir().unwrap();
    let envelope = serde_json::json!({
        "ts": "2026-02-14T10:00:00Z",
        "project_id": "p",
        "session_id": "h-1",
        "hook_event_name": "post_tool_call",
        "cwd": "/work/proj",
        "tool_name": "terminal",
        "bridge": "hermes"
    });
    let path = write_session_ledger(tmp.path(), &[envelope]);
    let stats = extract_stats(&path).unwrap();
    assert_eq!(
        stats.tool_calls, 1,
        "post_tool_call must count as a tool call"
    );
}

#[test]
fn manual_digest_openclaw_session_writes_event() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    // Real OpenClaw ledger shape (bridge:dispatch persist on every event).
    write_store_session_ledger(
        &project_id,
        "oc-digest-1",
        &[
            serde_json::json!({
                "ts": "2026-02-14T10:00:00Z",
                "project_id": project_id,
                "session_id": "oc-digest-1",
                "hook_event_name": "before_agent_start",
                "agent_id": "main",
                "event_data": { "prompt": "do work" }
            }),
            serde_json::json!({
                "ts": "2026-02-14T10:01:00Z",
                "project_id": project_id,
                "session_id": "oc-digest-1",
                "hook_event_name": "after_tool_call",
                "agent_id": "main",
                "event_data": {
                    "tool_name": "bash",
                    "tool_input": { "command": "cargo test" }
                }
            }),
        ],
    );

    let event_id = digest_session_manual(
        &project_id,
        "oc-digest-1",
        workspace.to_str().unwrap(),
        true,
    )
    .unwrap();
    assert!(
        !event_id.is_empty(),
        "a session with real OpenClaw tool activity must not be classified zero-call (round-1 P1-1)"
    );

    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let events = ledger.iter_events().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].payload["session_stats"]["tool_calls"], 1);
}

// ── Round-2: watermark identity + ledger-authoritative idempotency ──
// Ruling: a byte offset alone cannot identify a file (finding 1), and the
// ledger — not the side state file — is durable truth (finding 3).

// Finding 1a: a ledger replaced under the same session id must be RE-READ
// from zero, never skipped via the stale offset.
#[test]
fn replaced_ledger_same_session_id_is_reread_not_skipped() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    write_store_session_ledger(
        &project_id,
        "sess-repl",
        &[
            make_envelope("PostToolUse", "Edit", serde_json::json!({})),
            make_envelope("PostToolUse", "Bash", serde_json::json!({})),
        ],
    );
    let id1 =
        digest_session_manual(&project_id, "sess-repl", workspace.to_str().unwrap(), true).unwrap();
    assert!(!id1.is_empty());

    // The ledger is replaced under the same session id: different, valid,
    // SHORTER content. The stale offset (past EOF) must not suppress it.
    write_store_session_ledger(
        &project_id,
        "sess-repl",
        &[make_envelope("PostToolUse", "Edit", serde_json::json!({}))],
    );

    let id2 =
        digest_session_manual(&project_id, "sess-repl", workspace.to_str().unwrap(), true).unwrap();
    assert!(
        !id2.is_empty() && id2 != id1,
        "a replaced ledger must be re-read and re-digested, not skipped (round-2 finding 1)"
    );

    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let digests: Vec<_> = ledger
        .iter_events()
        .unwrap()
        .into_iter()
        .filter(|e| e.payload["source"] == "bridge:session_digest")
        .collect();
    assert_eq!(
        digests.len(),
        2,
        "the replacement content must get its own digest"
    );
    assert_eq!(
        digests[1].payload["session_stats"]["tool_calls"], 1,
        "the new digest must cover the replacement content from zero"
    );
}

// Finding 1b: a same-length in-place rewrite is invisible to a byte offset
// and must also be caught by the identity proof.
#[test]
fn same_length_rewrite_is_reread_not_skipped() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    write_store_session_ledger(
        &project_id,
        "sess-rewrite",
        &[
            make_envelope("PostToolUse", "Edit", serde_json::json!({})),
            make_envelope_at(
                "PostToolUse",
                "Edit",
                "2026-02-14T10:01:00Z",
                serde_json::json!({}),
            ),
        ],
    );
    let id1 = digest_session_manual(
        &project_id,
        "sess-rewrite",
        workspace.to_str().unwrap(),
        true,
    )
    .unwrap();
    assert!(!id1.is_empty());

    // Rewrite in place: same byte length, different content (the second
    // envelope's timestamp seconds digit changes 01 -> 02).
    let path = edda_store::project_dir(&project_id)
        .join("ledger")
        .join("sess-rewrite.jsonl");
    let old = std::fs::read_to_string(&path).unwrap();
    let new = old.replacen("2026-02-14T10:01:00Z", "2026-02-14T10:02:00Z", 1);
    assert_eq!(
        old.len(),
        new.len(),
        "fixture must be a same-length rewrite"
    );
    std::fs::write(&path, new).unwrap();

    let id2 = digest_session_manual(
        &project_id,
        "sess-rewrite",
        workspace.to_str().unwrap(),
        true,
    )
    .unwrap();
    assert!(
        !id2.is_empty() && id2 != id1,
        "a same-length rewrite must be re-read, not skipped via the stale offset (round-2 finding 1)"
    );

    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let digests: Vec<_> = ledger
        .iter_events()
        .unwrap()
        .into_iter()
        .filter(|e| e.payload["source"] == "bridge:session_digest")
        .collect();
    assert_eq!(digests.len(), 2);
}

// Finding 3: the ledger is durable truth; the side state file is a cache.
// A crash (or save failure) between the note append and the state save must
// cost a re-scan on retry — never a duplicate digest note.
#[test]
fn lost_cache_recovered_from_ledger_without_duplicate() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    write_store_session_ledger(
        &project_id,
        "sess-crash",
        &[
            make_envelope("PostToolUse", "Edit", serde_json::json!({})),
            make_envelope("PostToolUse", "Bash", serde_json::json!({})),
        ],
    );
    let id1 = digest_session_manual(&project_id, "sess-crash", workspace.to_str().unwrap(), true)
        .unwrap();
    assert!(!id1.is_empty());

    // Simulate the crash window: the digest note is durable in the ledger,
    // but the watermark cache save did not happen (failed/unwritable/lost).
    let state_path = edda_store::project_dir(&project_id)
        .join("state")
        .join("last_digested_session.json");
    std::fs::remove_file(&state_path).unwrap();

    let id2 = digest_session_manual(&project_id, "sess-crash", workspace.to_str().unwrap(), true)
        .unwrap();
    assert_eq!(
        id2, id1,
        "the ledger note is authoritative: losing the cache must recover its id, not duplicate"
    );

    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let digests: Vec<_> = ledger
        .iter_events()
        .unwrap()
        .into_iter()
        .filter(|e| e.payload["source"] == "bridge:session_digest")
        .collect();
    assert_eq!(
        digests.len(),
        1,
        "retry after cache loss must not append a second digest for the same ledger"
    );

    // The cache is repaired from the ledger on recovery.
    let state = load_digest_state(&project_id);
    assert_eq!(
        state.sessions["sess-crash"].event_id, id1,
        "recovery must repair the cache from the ledger note"
    );

    // And a growth after the crash window digests only the tail.
    std::fs::remove_file(&state_path).unwrap();
    let path = edda_store::project_dir(&project_id)
        .join("ledger")
        .join("sess-crash.jsonl");
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    let e = make_envelope_at(
        "PostToolUse",
        "Edit",
        "2026-02-15T10:00:00Z",
        serde_json::json!({}),
    );
    writeln!(f, "{}", serde_json::to_string(&e).unwrap()).unwrap();
    drop(f);

    let id3 = digest_session_manual(&project_id, "sess-crash", workspace.to_str().unwrap(), true)
        .unwrap();
    assert!(!id3.is_empty() && id3 != id1);

    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let digests: Vec<_> = ledger
        .iter_events()
        .unwrap()
        .into_iter()
        .filter(|e| e.payload["source"] == "bridge:session_digest")
        .collect();
    assert_eq!(digests.len(), 2);
    assert_eq!(
        digests[1].payload["session_stats"]["tool_calls"], 1,
        "after cache loss plus growth, only the tail beyond the ledger note is digested"
    );
}

// ── Round-3: one-read proof (P1-1) + ledger-sole-authority (P1-2) ──
// Ruling `digest.proof-and-authority=derive-from-one-read-ledger-is-sole-authority`.

fn copy_dir_all(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir_all(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), to).unwrap();
        }
    }
}

fn big_envelope_line(tool: &str) -> String {
    let mut s =
        String::from("{\"ts\":\"2026-02-14T10:00:00Z\",\"project_id\":\"p\",\"session_id\":\"s\",");
    s.push_str(&format!(
        "\"hook_event_name\":\"PostToolUse\",\"tool_name\":\"{tool}\",\"tool_use_id\":\"\",",
    ));
    s.push_str(&format!(
        "\"raw\":{{\"hook_event_name\":\"PostToolUse\",\"tool_name\":\"{tool}\"}}}}\n",
    ));
    s
}

#[test]
fn digest_note_proof_and_stats_come_from_one_read() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    // Same shape as the reviewer's reproduction: two same-length large
    // ledgers, one of Edit calls, one of Bash calls.
    let n: usize = 300_000;
    let a_bytes = big_envelope_line("Edit").repeat(n);
    let b_bytes = big_envelope_line("Bash").repeat(n);
    assert_eq!(a_bytes.len(), b_bytes.len(), "fixture must be same-length");
    let hash_a = blake3::hash(a_bytes.as_bytes()).to_hex().to_string();
    let hash_b = blake3::hash(b_bytes.as_bytes()).to_hex().to_string();

    let dir = edda_store::project_dir(&project_id).join("ledger");
    std::fs::create_dir_all(&dir).unwrap();
    let session_path = dir.join("sess-race.jsonl");
    std::fs::write(&session_path, &a_bytes).unwrap();
    let b_staging = tmp.path().join("b_version.jsonl");
    std::fs::write(&b_staging, &b_bytes).unwrap();

    // While extraction holds the file open, atomically replace the path
    // with the same-length Bash ledger, over and over.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let swapper = {
        let stop = std::sync::Arc::clone(&stop);
        let session_path = session_path.clone();
        let b_staging = b_staging.clone();
        std::thread::spawn(move || {
            let swap_tmp = session_path.with_extension("swap");
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                std::fs::copy(&b_staging, &swap_tmp).unwrap();
                std::fs::rename(&swap_tmp, &session_path).unwrap();
            }
        })
    };

    let id1 =
        digest_session_manual(&project_id, "sess-race", workspace.to_str().unwrap(), true).unwrap();
    assert!(!id1.is_empty());
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    swapper.join().unwrap();

    // The emitted note must be SELF-CONSISTENT: its stats and its
    // prefix_hash describe the SAME bytes.
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let note1 = ledger
        .iter_events()
        .unwrap()
        .into_iter()
        .find(|e| e.event_id == id1)
        .expect("the digest note must exist");
    let wm_offset = note1.payload["digest_watermark"]["offset"]
        .as_u64()
        .unwrap();
    let wm_hash = note1.payload["digest_watermark"]["prefix_hash"]
        .as_str()
        .unwrap();
    let breakdown = &note1.payload["session_stats"]["tool_call_breakdown"];
    let edits = breakdown["Edit"].as_u64().unwrap_or(0);
    let bashes = breakdown["Bash"].as_u64().unwrap_or(0);
    assert_eq!(
        wm_offset as usize,
        a_bytes.len(),
        "note must claim the full read"
    );
    let consistent =
        (edits == n as u64 && wm_hash == hash_a) || (bashes == n as u64 && wm_hash == hash_b);
    assert!(
        consistent,
        "note stats and proof came from DIFFERENT reads: breakdown={breakdown} hash_matches_a={} hash_matches_b={}",
        wm_hash == hash_a,
        wm_hash == hash_b
    );

    // Settle on the Bash version and digest again: every current record
    // must be accounted for — no silent skip. Two cases, both correct:
    // the racy first digest read EITHER version through its single handle.
    // If it summarized A (Edit), the retry must re-read B from zero and
    // write a Bash note; if it already summarized B, the retry must be a
    // no-op returning that note's id — B is fully digested, and a second
    // note would be a duplicate, not a rescue.
    let swap_tmp = session_path.with_extension("settle");
    std::fs::copy(&b_staging, &swap_tmp).unwrap();
    std::fs::rename(&swap_tmp, &session_path).unwrap();
    let id2 =
        digest_session_manual(&project_id, "sess-race", workspace.to_str().unwrap(), true).unwrap();
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let notes: Vec<_> = ledger
        .iter_events()
        .unwrap()
        .into_iter()
        .filter(|e| e.payload["source"] == "bridge:session_digest")
        .collect();
    if edits == n as u64 {
        // First digest summarized the Edit version; the Bash version is
        // undigested and the retry must cover it from zero.
        let note2 = notes
            .iter()
            .find(|e| e.event_id == id2 && e.event_id != id1)
            .unwrap_or_else(|| {
                panic!(
                    "retry after replacement wrote nothing: the current {n} Bash records were silently skipped"
                )
            });
        assert_eq!(
            note2.payload["session_stats"]["tool_call_breakdown"]["Bash"], n as u64,
            "the current Bash records must be digested, not skipped"
        );
    } else {
        // First digest already summarized the Bash version (the swap landed
        // before its read): the retry must be a no-op returning that note's
        // own id, with no duplicate.
        assert_eq!(
            notes.len(),
            1,
            "B is already fully digested: the retry must not duplicate"
        );
        assert_eq!(id2, id1, "the retry must return the covering note's id");
    }
}

// P1-2 (round-3): the cache is an unverified hint. A preserved cache
// watermark must not suppress a digest whose note is ABSENT from the
// authoritative workspace ledger (rolled back / lost).
#[test]
fn rolled_back_ledger_cannot_be_suppressed_by_cache() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());

    write_store_session_ledger(
        &project_id,
        "sess-rollback",
        &[make_envelope("PostToolUse", "Edit", serde_json::json!({}))],
    );

    // Snapshot the workspace ledger immediately BEFORE the digest.
    let edda_dir = workspace.join(".edda");
    let snapshot = tmp.path().join(".edda.snapshot");
    copy_dir_all(&edda_dir, &snapshot);

    let id1 = digest_session_manual(
        &project_id,
        "sess-rollback",
        workspace.to_str().unwrap(),
        true,
    )
    .unwrap();
    assert!(!id1.is_empty());

    // Restore the immediately-prior valid snapshot: the stamped note is
    // GONE from the authoritative ledger; the watermark cache survives.
    std::fs::remove_dir_all(&edda_dir).unwrap();
    copy_dir_all(&snapshot, &edda_dir);

    let id2 = digest_session_manual(
        &project_id,
        "sess-rollback",
        workspace.to_str().unwrap(),
        true,
    )
    .unwrap();

    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let notes: Vec<_> = ledger
        .iter_events()
        .unwrap()
        .into_iter()
        .filter(|e| e.payload["source"] == "bridge:session_digest")
        .collect();
    assert_eq!(
        notes.len(),
        1,
        "the authoritative ledger had zero notes for this session; the surviving cache must not suppress the digest"
    );
    assert_eq!(
        notes[0].event_id, id2,
        "the returned event id must be a note that exists in the authoritative ledger"
    );
    assert_eq!(
        notes[0].payload["session_stats"]["tool_call_breakdown"]["Edit"], 1,
        "the re-digest must cover the Edit prefix the rolled-back note used to cover"
    );

    // After appending a Bash line, the whole content is covered — the
    // Edit prefix must not stay permanently absent.
    let path = edda_store::project_dir(&project_id)
        .join("ledger")
        .join("sess-rollback.jsonl");
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    let e = make_envelope_at(
        "PostToolUse",
        "Bash",
        "2026-02-14T11:00:00Z",
        serde_json::json!({}),
    );
    writeln!(f, "{}", serde_json::to_string(&e).unwrap()).unwrap();
    drop(f);

    digest_session_manual(
        &project_id,
        "sess-rollback",
        workspace.to_str().unwrap(),
        true,
    )
    .unwrap();
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let notes: Vec<_> = ledger
        .iter_events()
        .unwrap()
        .into_iter()
        .filter(|e| e.payload["source"] == "bridge:session_digest")
        .collect();
    assert_eq!(notes.len(), 2, "the tail must be digested");
    assert_eq!(
        notes[1].payload["session_stats"]["tool_call_breakdown"]["Bash"], 1,
        "the tail digest covers the appended Bash line"
    );
}

// Round-4 P1 (ruling
// `digest.zero-call-sessions=re-read-every-time-no-cache-authority`):
// a zero-call watermark is NOT a ledger-independent semantic authority.
// The reviewer's reproduction: digest a real Edit prefix, append a
// zero-call prompt tail, digest again so the cache legitimately advances
// to EOF — then roll the authoritative ledger back past the Edit note
// while preserving that cache. The cache fact described only the tail,
// so it must not be trusted for the whole prefix: the session MUST be
// reported pending and the Edit work MUST be re-emitted.
#[test]
fn rolled_back_ledger_with_zero_call_cache_must_re_report_and_re_emit() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());
    let session_id = "sess-zc-rollback";

    write_store_session_ledger(
        &project_id,
        session_id,
        &[make_envelope("PostToolUse", "Edit", serde_json::json!({}))],
    );

    // Snapshot the workspace ledger BEFORE any digest note exists.
    let edda_dir = workspace.join(".edda");
    let snapshot = tmp.path().join(".edda.snapshot");
    copy_dir_all(&edda_dir, &snapshot);

    // 1. Digest the real Edit prefix: exactly one note.
    digest_session_manual(&project_id, session_id, workspace.to_str().unwrap(), true).unwrap();

    // 2. Append a zero-call (chat-only) tail and digest again: the watermark
    //    cache advances to EOF, GH-578 still forbids a second event.
    let path = edda_store::project_dir(&project_id)
        .join("ledger")
        .join(format!("{session_id}.jsonl"));
    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        for ts in ["2026-02-14T11:00:00Z", "2026-02-14T11:01:00Z"] {
            let e = make_envelope_at("UserPromptSubmit", "", ts, serde_json::json!({}));
            writeln!(f, "{}", serde_json::to_string(&e).unwrap()).unwrap();
        }
    }
    digest_session_manual(&project_id, session_id, workspace.to_str().unwrap(), true).unwrap();
    {
        let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
        let count = ledger
            .iter_events()
            .unwrap()
            .iter()
            .filter(|e| e.payload["source"] == "bridge:session_digest")
            .count();
        assert_eq!(
            count, 1,
            "a zero-call delta must still write no event (GH-578)"
        );
    }

    // 3. Roll the authoritative ledger back past the Edit note. The digest
    //    state cache lives in the per-user store, outside the workspace, so
    //    it survives with its EOF watermark.
    std::fs::remove_dir_all(&edda_dir).unwrap();
    copy_dir_all(&snapshot, &edda_dir);

    // 4. The session MUST be reported pending — no cache fact may suppress
    //    it, because the ledger contains no note covering the Edit.
    let pending = find_all_pending_sessions(&project_id);
    assert!(
        pending.contains(&session_id.to_string()),
        "a zero-call cache watermark is not ledger authority: with no note in the ledger the session must be reported pending"
    );
    let result =
        digest_previous_sessions(&project_id, "current", workspace.to_str().unwrap(), 2000);
    assert!(
        matches!(result, DigestResult::Written { .. }),
        "digest_previous_sessions must re-digest the session (got {result:?}), never report NoPending"
    );

    // 5. The Edit work must be emitted: exactly one covering note.
    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let notes: Vec<_> = ledger
        .iter_events()
        .unwrap()
        .into_iter()
        .filter(|e| e.payload["source"] == "bridge:session_digest")
        .collect();
    assert_eq!(
        notes.len(),
        1,
        "the ledger had zero notes for this session; the Edit work must be re-emitted"
    );
    assert_eq!(
        notes[0].payload["session_stats"]["tool_call_breakdown"]["Edit"], 1,
        "the re-digest must cover the Edit prefix the rolled-back ledger no longer records"
    );
}

// Round-4 P1, second probe: a HAND-EDITED state file claiming a consumed
// prefix with the CORRECT content hash must not be able to suppress a
// session the ledger has no note for. An unsigned mutable cache is not a
// semantic authority.
#[test]
fn hand_edited_cache_with_correct_hash_cannot_suppress_unnoted_session() {
    let _env = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    let (workspace, project_id) = setup_digest_workspace(tmp.path());
    let session_id = "sess-hand-edited";

    write_store_session_ledger(
        &project_id,
        session_id,
        &[make_envelope("PostToolUse", "Edit", serde_json::json!({}))],
    );
    let path = edda_store::project_dir(&project_id)
        .join("ledger")
        .join(format!("{session_id}.jsonl"));
    let len = std::fs::metadata(&path).unwrap().len();
    let correct_hash = hash_prefix(&path, len).unwrap();

    // Hand-edit the state file: a fully-consumed watermark with the correct
    // identity proof (and the legacy zero_call flag), but NO ledger note.
    let state = serde_json::json!({
        "session_id": session_id,
        "event_id": "",
        "sessions": {
            session_id: {
                "offset": len,
                "prefix_hash": correct_hash,
                "event_id": "",
                "digested_at": "2026-02-14T10:00:00Z",
                "zero_call": true,
            }
        },
    });
    let state_dir = edda_store::project_dir(&project_id).join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(
        state_dir.join("last_digested_session.json"),
        serde_json::to_string_pretty(&state).unwrap(),
    )
    .unwrap();

    let pending = find_all_pending_sessions(&project_id);
    assert!(
        pending.contains(&session_id.to_string()),
        "a hand-edited cache with a correct content hash is not ledger authority: the session must be reported pending"
    );
    let result =
        digest_previous_sessions(&project_id, "current", workspace.to_str().unwrap(), 2000);
    assert!(
        matches!(result, DigestResult::Written { .. }),
        "digest_previous_sessions must re-digest the session (got {result:?}), never report NoPending"
    );

    let ledger = edda_ledger::Ledger::open(&workspace).unwrap();
    let notes: Vec<_> = ledger
        .iter_events()
        .unwrap()
        .into_iter()
        .filter(|e| e.payload["source"] == "bridge:session_digest")
        .collect();
    assert_eq!(notes.len(), 1, "the Edit work must be emitted");
    assert_eq!(
        notes[0].payload["session_stats"]["tool_call_breakdown"]["Edit"], 1,
        "the re-digest must cover the Edit the hand-edited cache claimed consumed"
    );
}
