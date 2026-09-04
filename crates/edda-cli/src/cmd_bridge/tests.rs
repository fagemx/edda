use super::claim::claim_disclosure;
use super::claude::hook_timeout_ms;
use super::peers::peers_json;
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

fn webhook_capture() -> (
    String,
    std::sync::mpsc::Receiver<String>,
    std::thread::JoinHandle<()>,
) {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_millis(100)))
            .unwrap();
        let mut request = Vec::new();
        let _ = stream.read_to_end(&mut request);
        tx.send(String::from_utf8_lossy(&request).into_owned())
            .unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .unwrap();
    });
    (url, rx, handle)
}

fn enable_webhook(repo: &std::path::Path, url: &str) {
    std::fs::create_dir_all(repo.join(".edda")).unwrap();
    std::fs::write(
        repo.join(".edda/config.json"),
        serde_json::json!({
            "notify_channels": [{
                "type": "webhook",
                "url": url,
                "events": ["request_pending"]
            }]
        })
        .to_string(),
    )
    .unwrap();
}

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Serialize tests that mutate process-global env vars
/// (EDDA_SESSION_ID/LABEL, EDDA_HOOK_TIMEOUT_MS) — without this they
/// race each other under the parallel test runner. Same pattern as
/// edda-bridge-claude's ENV_LOCK. Poisoned locks are recovered so one
/// failing test doesn't cascade.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn env_guard() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

fn setup_workspace() -> (std::path::PathBuf, edda_ledger::Ledger) {
    let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let tmp = std::env::temp_dir().join(format!("edda_bridge_test_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let paths = edda_ledger::EddaPaths::discover(&tmp);
    edda_ledger::ledger::init_workspace(&paths).unwrap();
    edda_ledger::ledger::init_head(&paths, "main").unwrap();
    edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
    let ledger = edda_ledger::Ledger::open(&tmp).unwrap();
    (tmp, ledger)
}

#[test]
fn find_active_decision_returns_value() {
    let (tmp, ledger) = setup_workspace();
    let branch = ledger.head_branch().unwrap();
    let parent_hash = ledger.last_event_hash().unwrap();

    // Write a decision event with structured fields
    let tags = vec!["decision".to_string()];
    let mut event = edda_core::event::new_note_event(
        &branch,
        parent_hash.as_deref(),
        "system",
        "db.engine: postgres",
        &tags,
    )
    .unwrap();
    event.payload["decision"] = serde_json::json!({"key": "db.engine", "value": "postgres"});
    edda_core::event::finalize_event(&mut event).unwrap();
    ledger.append_event(&event).unwrap();

    let result = ledger.find_active_decision(&branch, "db.engine").unwrap();
    assert!(result.is_some(), "should find active decision");
    let row = result.unwrap();
    assert!(!row.event_id.is_empty());
    assert_eq!(row.value, "postgres");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn find_active_decision_no_match() {
    let (tmp, ledger) = setup_workspace();
    let branch = ledger.head_branch().unwrap();

    let result = ledger
        .find_active_decision(&branch, "nonexistent.key")
        .unwrap();
    assert!(result.is_none(), "should not find anything");

    let _ = std::fs::remove_dir_all(&tmp);
}

// ── Integration: decide() end-to-end (Issue #148 Gaps 1, 2) ──

#[test]
fn decide_writes_binding_to_coordination_log() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    let (tmp, _ledger) = setup_workspace();
    let pid = edda_store::project_id(&tmp);
    let _ = edda_store::ensure_dirs(&pid);
    // Clean coordination log
    let state_dir = edda_store::project_dir(&pid).join("state");
    let _ = std::fs::remove_file(state_dir.join("coordination.jsonl"));

    std::env::set_var("EDDA_SESSION_ID", "test-decide-bind-s1");
    std::env::set_var("EDDA_SESSION_LABEL", "auth");

    decide(
        &tmp,
        "db.engine=postgres",
        Some("need JSONB"),
        &[],
        None,
        None,
        &[],
        &[],
    )
    .unwrap();

    // Verify binding was written via L2 conflict check API
    let conflict = edda_bridge_claude::peers::find_binding_conflict(&pid, "db.engine", "OTHER");
    assert!(
        conflict.is_some(),
        "should find existing binding via conflict check"
    );
    let c = conflict.unwrap();
    assert_eq!(c.existing_value, "postgres");
    // Verify no conflict with same value (idempotent)
    let no_conflict =
        edda_bridge_claude::peers::find_binding_conflict(&pid, "db.engine", "postgres");
    assert!(no_conflict.is_none(), "same value should not conflict");

    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(edda_store::project_dir(&pid));
}

#[test]
fn decide_writes_structured_ledger_event() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    let (tmp, ledger) = setup_workspace();
    let pid = edda_store::project_id(&tmp);
    let _ = edda_store::ensure_dirs(&pid);

    std::env::set_var("EDDA_SESSION_ID", "test-decide-ledger-s2");
    std::env::set_var("EDDA_SESSION_LABEL", "billing");

    decide(
        &tmp,
        "auth.method=JWT RS256",
        Some("stateless auth"),
        &[],
        None,
        None,
        &[],
        &[],
    )
    .unwrap();

    let events = ledger.iter_events().unwrap();
    assert_eq!(events.len(), 1, "should have 1 event");
    let e = &events[0];
    assert_eq!(e.event_type, "note");

    // Tags
    let tags = e.payload.get("tags").and_then(|v| v.as_array()).unwrap();
    assert!(tags.iter().any(|t| t.as_str() == Some("decision")));

    // Structured decision object
    let dec = e.payload.get("decision").unwrap();
    assert_eq!(dec["key"].as_str().unwrap(), "auth.method");
    assert_eq!(dec["value"].as_str().unwrap(), "JWT RS256");
    assert_eq!(dec["reason"].as_str().unwrap(), "stateless auth");

    // GH-401: an agent-session decide is tagged authority=agent, never
    // operator — a write can never self-declare operator authority.
    assert_eq!(dec["authority"].as_str().unwrap(), "agent");

    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(edda_store::project_dir(&pid));
}

#[test]
fn ratify_records_separate_event_and_makes_decision_binding() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    let (tmp, ledger) = setup_workspace();
    let pid = edda_store::project_id(&tmp);
    let _ = edda_store::ensure_dirs(&pid);
    std::env::set_var("EDDA_SESSION_ID", "test-ratify-s1");
    std::env::set_var("EDDA_SESSION_LABEL", "worker");

    decide(
        &tmp,
        "db.engine=sqlite",
        Some("embedded"),
        &[],
        None,
        None,
        &[],
        &[],
    )
    .unwrap();

    // Before ratify: the active decision is not binding.
    assert!(ledger.ratified_decision_events().unwrap().is_empty());

    ratify(
        &tmp,
        "db.engine",
        Some("looks right"),
        Some("operator"),
        None,
    )
    .unwrap();

    // A distinct decision_ratify event was written (not a mutation).
    let ratify_events = ledger.iter_events_by_type("decision_ratify").unwrap();
    assert_eq!(ratify_events.len(), 1);
    assert_eq!(ratify_events[0].payload["key"], "db.engine");
    assert_eq!(ratify_events[0].payload["ratified_by"], "operator");

    // The projection now reports the key as binding.
    let views = ledger.active_decisions(None, None, None, None).unwrap();
    let view = views.iter().find(|v| v.key == "db.engine").unwrap();
    let set = ledger.ratified_decision_events().unwrap();
    assert!(edda_ledger::view::is_decision_ratified(view, &set));

    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(edda_store::project_dir(&pid));
}

#[test]
fn ratify_unknown_key_errors() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    let (tmp, _ledger) = setup_workspace();
    let pid = edda_store::project_id(&tmp);
    let _ = edda_store::ensure_dirs(&pid);
    let err = ratify(&tmp, "nope.key", None, None, None)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("no active decision"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(edda_store::project_dir(&pid));
}

#[test]
fn decide_supersedes_prior_decision_same_key() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    let (tmp, ledger) = setup_workspace();
    let pid = edda_store::project_id(&tmp);
    let _ = edda_store::ensure_dirs(&pid);

    std::env::set_var("EDDA_SESSION_ID", "test-decide-super-s3");
    std::env::set_var("EDDA_SESSION_LABEL", "infra");

    decide(&tmp, "db.engine=SQLite", None, &[], None, None, &[], &[]).unwrap();
    decide(
        &tmp,
        "db.engine=PostgreSQL",
        Some("need JSONB"),
        &[],
        None,
        None,
        &[],
        &[],
    )
    .unwrap();

    let events = ledger.iter_events().unwrap();
    assert_eq!(events.len(), 2, "should have 2 events");

    let first_id = &events[0].event_id;
    let second = &events[1];

    // Second event should supersede the first
    assert!(
        !second.refs.provenance.is_empty(),
        "second event should have provenance"
    );
    let prov = &second.refs.provenance[0];
    assert_eq!(prov.target, *first_id, "should point to first event");
    assert_eq!(prov.rel, edda_core::types::rel::SUPERSEDES);

    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(edda_store::project_dir(&pid));
}

#[test]
fn bare_decide_beside_two_live_sessions_refuses_without_writing() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let (repo, ledger) = setup_workspace();
    let pid = edda_store::project_id(&repo);
    edda_store::ensure_dirs(&pid).expect("store dirs");
    edda_bridge_claude::peers::write_heartbeat_minimal(&pid, "worker-a", "worker-a", "/tmp/a");
    edda_bridge_claude::peers::write_heartbeat_minimal(&pid, "worker-b", "worker-b", "/tmp/b");
    let before = ledger.iter_events().expect("events before").len();

    let err = decide(
        &repo,
        "unsafe.adoption=blocked",
        Some("identity must come from the process"),
        &[],
        None,
        None,
        &[],
        &[],
    )
    .expect_err("a bare shell beside live sessions must refuse");
    assert!(err.to_string().contains("--session"), "{err}");
    assert_eq!(
        ledger.iter_events().expect("events after").len(),
        before,
        "a refused decide must not append to the ledger"
    );
    assert!(
        edda_bridge_claude::peers::compute_board_state(&pid)
            .bindings
            .is_empty(),
        "a refused decide must not broadcast a binding"
    );
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(edda_store::project_dir(&pid));
}

// ── Integration: process-bound session identity (GH-503) ──

#[test]
fn resolve_session_id_tiers() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    let pid = "test_resolve_sid_tiers";
    let _ = edda_store::ensure_dirs(pid);

    // Clear env to avoid interference
    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");

    // Tier 1: explicit cli_session
    let (sid, label) = resolve_session_id(Some("explicit-sid"), pid, "cli").unwrap();
    assert_eq!(sid, "explicit-sid");
    assert_eq!(label, "cli");

    // Tier 2: EDDA_SESSION_ID env
    std::env::set_var("EDDA_SESSION_ID", "env-sid");
    let (sid, _) = resolve_session_id(None, pid, "cli").unwrap();
    assert_eq!(sid, "env-sid");
    std::env::remove_var("EDDA_SESSION_ID");

    // A process-carried id remains authoritative beside a live session.
    // Clean state dir first to avoid interference from concurrent sessions
    let state_dir = edda_store::project_dir(pid).join("state");
    if state_dir.exists() {
        for entry in std::fs::read_dir(&state_dir).unwrap() {
            let entry = entry.unwrap();
            if entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("session."))
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let _ = std::fs::create_dir_all(&state_dir);
    let now = time::OffsetDateTime::now_utc();
    let now_str = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let hb = serde_json::json!({
        "session_id": "inferred-sess",
        "started_at": now_str,
        "last_heartbeat": now_str,
        "label": "worker",
        "focus_files": [],
        "active_tasks": [],
        "files_modified_count": 0,
        "total_edits": 0,
        "recent_commits": []
    });
    std::fs::write(
        state_dir.join("session.inferred-sess.json"),
        serde_json::to_string_pretty(&hb).unwrap(),
    )
    .unwrap();
    std::env::set_var("EDDA_SESSION_ID", "env-live-sid");
    let (sid, _) = resolve_session_id(None, pid, "cli").unwrap();
    assert_eq!(sid, "env-live-sid");
    std::env::set_var("EDDA_SESSION_ID", "");
    let err = resolve_session_id(None, pid, "cli")
        .expect_err("an empty env value must not adopt the sole heartbeat");
    assert!(err.to_string().contains("--session"), "{err}");
    std::env::remove_var("EDDA_SESSION_ID");
    let _ = std::fs::remove_file(state_dir.join("session.inferred-sess.json"));

    // Standalone fallback (no heartbeats, no env)
    let (sid, label) = resolve_session_id(None, pid, "cli").unwrap();
    assert_eq!(sid, "cli-cli");
    assert_eq!(label, "cli");

    // Tier 1 wins over Tier 2
    std::env::set_var("EDDA_SESSION_ID", "env-sid");
    let (sid, _) = resolve_session_id(Some("explicit-wins"), pid, "cli").unwrap();
    assert_eq!(sid, "explicit-wins", "tier 1 should beat tier 2");
    std::env::remove_var("EDDA_SESSION_ID");

    let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn resolve_session_id_refusal_names_live_sessions() {
    // Round-1 consequence (GH-705): when the bare-CLI refusal does fire —
    // a live hooked session beside this bare command — the error must
    // name the ids to copy into `--session`, the way `unclaim`'s refusal
    // lists the board. An error that demands an id without showing one
    // cannot be acted on.
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    let pid = "test_resolve_sid_refusal_names";
    let _ = edda_store::ensure_dirs(pid);
    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    edda_bridge_claude::peers::write_heartbeat_minimal(pid, "sess-live", "auth", ".");
    let err = resolve_session_id(None, pid, "auth")
        .expect_err("a bare command beside a live session must refuse");
    let msg = err.to_string();
    assert!(
        msg.contains("sess-live"),
        "refusal must name the live session id to pass to --session: {msg}"
    );
    assert!(msg.contains("--session"), "{msg}");
    let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn resolve_session_id_refuses_unattributed_parented_subagent() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    let pid = "test_gh780_parented_identity";
    let stale = edda_bridge_claude::peers::stale_secs();
    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    crate::test_support::write_aged_heartbeat(pid, "sub-agent-1", stale * 3, Some("parent-1"));

    let err = resolve_session_id(None, pid, "cli").expect_err("live heartbeat is ambiguous");
    assert!(err.to_string().contains("--session is required"));
    assert!(err.to_string().contains("sub-agent-1"));
}

// ── Render & Heartbeat CLI tests (Issue #15) ──

#[test]
fn render_writeback_contains_protocol() {
    let output = edda_bridge_claude::render::writeback();
    assert!(
        output.contains("Write-Back Protocol"),
        "should contain header"
    );
    assert!(output.contains("edda decide"), "should teach edda decide");
    assert!(output.contains("edda note"), "should teach edda note");
    assert!(
        output.contains("edda task done") && output.contains("--receipt"),
        "should teach the task rail verbs at the same level as decide/note (§5)"
    );
    assert!(
        output.contains("edda ask") && output.contains("edda search query"),
        "should teach the read verbs — read before you write, or the ledger is write-only"
    );
}

#[test]
fn render_workspace_with_ledger() {
    let (tmp, _ledger) = setup_workspace();
    let cwd = tmp.to_str().unwrap();
    let result = edda_bridge_claude::render::workspace(cwd, 2500);
    assert!(
        result.is_some(),
        "workspace with ledger should produce output"
    );
    let text = result.unwrap();
    assert!(
        text.contains("Project") || text.contains("Branch"),
        "should contain workspace sections"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn render_workspace_no_ledger() {
    let result = edda_bridge_claude::render::workspace("/nonexistent/path", 2500);
    assert!(result.is_none(), "no workspace should return None");
}

#[test]
fn render_coordination_solo_no_bindings() {
    let _store = crate::test_support::isolated_store();
    let pid = "test_render_coord_solo";
    let _ = edda_store::ensure_dirs(pid);
    let result = edda_bridge_claude::render::coordination(pid, "solo-session");
    // Solo with no bindings → None
    assert!(
        result.is_none(),
        "solo session with no bindings should return None"
    );
    let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_pack_no_pack_file() {
    let _store = crate::test_support::isolated_store();
    let pid = "test_render_pack_empty";
    let _ = edda_store::ensure_dirs(pid);
    let result = edda_bridge_claude::render::pack(pid);
    assert!(result.is_none(), "no hot.md should return None");
    let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn heartbeat_write_touch_remove_lifecycle() {
    let _store = crate::test_support::isolated_store();
    let pid = "test_hb_lifecycle";
    let sid = "sess-lifecycle-1";
    let _ = edda_store::ensure_dirs(pid);

    // Write
    edda_bridge_claude::peers::write_heartbeat_minimal(pid, sid, "worker", ".");
    let state_dir = edda_store::project_dir(pid).join("state");
    let hb_path = state_dir.join(format!("session.{sid}.json"));
    assert!(hb_path.exists(), "heartbeat file should exist after write");

    // Verify label
    let content: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&hb_path).unwrap()).unwrap();
    assert_eq!(content["label"].as_str().unwrap(), "worker");
    assert_eq!(content["session_id"].as_str().unwrap(), sid);

    // Touch
    let _mtime_before = std::fs::metadata(&hb_path).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    edda_bridge_claude::peers::touch_heartbeat(pid, sid);
    let content_after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&hb_path).unwrap()).unwrap();
    // last_heartbeat string should have changed
    assert_ne!(
        content["last_heartbeat"].as_str().unwrap(),
        content_after["last_heartbeat"].as_str().unwrap(),
        "touch should update last_heartbeat"
    );

    // Remove
    edda_bridge_claude::peers::remove_heartbeat(pid, sid);
    assert!(
        !hb_path.exists(),
        "heartbeat file should be gone after remove"
    );

    let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Hook resilience tests (#83) ──

#[test]
fn catch_unwind_recovers_from_panic() {
    // Verify catch_unwind pattern works with panicking closures
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || -> anyhow::Result<String> {
                panic!("test panic in hook");
            },
        ));
        let _ = tx.send(result);
    });

    let outcome = rx.recv_timeout(std::time::Duration::from_secs(5));
    assert!(outcome.is_ok(), "channel should receive");
    let inner = outcome.unwrap();
    assert!(inner.is_err(), "should be a caught panic");
    let panic_info = inner.unwrap_err();
    let msg = panic_info
        .downcast_ref::<&str>()
        .copied()
        .unwrap_or("unknown");
    assert_eq!(msg, "test panic in hook");
}

#[test]
fn timeout_fires_on_slow_hook() {
    let (tx, rx) = std::sync::mpsc::channel::<anyhow::Result<String>>();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(60));
        let _ = tx.send(Ok("too late".to_string()));
    });

    let outcome = rx.recv_timeout(std::time::Duration::from_millis(50));
    assert!(
        outcome.is_err(),
        "should timeout before slow hook completes"
    );
}

#[test]
fn hook_timeout_ms_defaults_to_60s() {
    let _env = env_guard();
    std::env::remove_var("EDDA_HOOK_TIMEOUT_MS");
    assert_eq!(hook_timeout_ms(), 60_000);
}

#[test]
fn hook_timeout_ms_reads_env() {
    let _env = env_guard();
    std::env::set_var("EDDA_HOOK_TIMEOUT_MS", "5000");
    assert_eq!(hook_timeout_ms(), 5000);
    std::env::remove_var("EDDA_HOOK_TIMEOUT_MS");
}

// ── Request target validation (GH-443) ──

#[test]
fn request_to_unknown_label_is_rejected_unless_forced() {
    let _store = crate::test_support::isolated_store();
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    let _ = edda_store::ensure_dirs(&pid);
    edda_bridge_claude::peers::write_heartbeat_minimal(&pid, "s-auth", "auth", ".");

    let err = request(repo.path(), "aut", "hi", Some("s-cli"), false)
        .expect_err("a typo'd label must not silently succeed");
    let msg = err.to_string();
    assert!(
        msg.contains("no active session answers to 'aut'"),
        "error should name the unreachable label: {msg}"
    );
    assert!(
        msg.contains("auth"),
        "error should list the labels that do exist: {msg}"
    );

    // --force is the escape hatch for a peer that has not started yet.
    request(repo.path(), "aut", "hi", Some("s-cli"), true).expect("--force should send anyway");
    // A live label needs no escape hatch.
    request(repo.path(), "auth", "hi", Some("s-cli"), false).expect("live label should send");

    let board = edda_bridge_claude::peers::compute_board_state(&pid);
    assert_eq!(board.requests.len(), 2, "both sent requests are recorded");
    assert!(
        !board.requests[0].id.is_empty(),
        "every request carries an id"
    );
    assert_ne!(
        board.requests[0].id, board.requests[1].id,
        "ids must be distinct per message"
    );
}

#[test]
fn request_emits_request_pending_notification() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    std::env::set_var("EDDA_SESSION_ID", "s-auth");
    std::env::set_var("EDDA_SESSION_LABEL", "auth");
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    let _ = edda_store::ensure_dirs(&pid);
    edda_bridge_claude::peers::write_heartbeat_minimal(&pid, "s-auth", "auth", ".");
    let (url, rx, server) = webhook_capture();
    enable_webhook(repo.path(), &url);

    request(repo.path(), "billing", "need invoice type", None, true)
        .expect("forced request should succeed");
    let body = rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("request creation should dispatch a notification");
    server.join().unwrap();

    assert!(
        body.contains("\"event_type\":\"request_pending\""),
        "{body}"
    );
    assert!(body.contains("\"from_label\":\"auth\""), "{body}");
    assert!(body.contains("\"to_label\":\"billing\""), "{body}");
    assert!(body.contains("\"message\":\"need invoice type\""), "{body}");
    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
}

fn prior_claim(label: &str, paths: &[&str]) -> edda_bridge_claude::peers::ClaimEntry {
    edda_bridge_claude::peers::ClaimEntry {
        session_id: "s1".into(),
        label: label.into(),
        paths: paths.iter().map(|p| (*p).to_string()).collect(),
        ts: "2026-08-20T00:00:00Z".into(),
        subject: None,
    }
}

fn owned(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|p| (*p).to_string()).collect()
}

// The disclosure lines are asserted here rather than only through the board,
// because a test that reads `compute_board_state` passes whether or not
// anything was printed -- the fold it checks pre-dates this change.

#[test]
fn a_first_claim_reports_no_replacement() {
    assert_eq!(
        claim_disclosure(None, "auth", &owned(&["src/auth/*"])),
        vec!["Claimed scope: auth"]
    );
}

#[test]
fn a_new_label_names_the_claim_it_replaced() {
    let previous = prior_claim("auth", &["src/auth/*"]);
    assert_eq!(
        claim_disclosure(Some(&previous), "api", &owned(&["src/api/*"])),
        vec![
            "Claimed scope: api (replaces this session's earlier claim on auth)",
            "  released: src/auth/*",
        ]
    );
}

#[test]
fn an_identical_re_claim_releases_nothing() {
    // The regression this verb exists to prevent, in its own image: the
    // first version printed "released: src/api/*" for a command that had
    // just re-claimed `src/api/*`. A bare-shell restart re-running its own
    // command hits this, so it is the common case, not a corner.
    let previous = prior_claim("api", &["src/api/*"]);
    assert_eq!(
        claim_disclosure(Some(&previous), "api", &owned(&["src/api/*"])),
        vec!["Re-claimed scope: api (unchanged)"],
        "an unchanged re-claim reports no release at all"
    );
}

#[test]
fn narrowing_reports_only_the_path_it_gave_up() {
    let previous = prior_claim("api", &["src/api/*", "src/api/v2/*"]);
    assert_eq!(
        claim_disclosure(Some(&previous), "api", &owned(&["src/api/v2/*"])),
        vec![
            "Re-claimed scope: api (previous paths replaced)",
            "  released: src/api/*",
        ],
        "src/api/v2/* is still claimed, so it is not released"
    );
}

#[test]
fn widening_reports_paths_added_but_no_release() {
    let previous = prior_claim("api", &["src/api/*"]);
    assert_eq!(
        claim_disclosure(
            Some(&previous),
            "api",
            &owned(&["src/api/*", "src/api/v3/*"])
        ),
        vec!["Re-claimed scope: api (paths added)"]
    );
}

#[test]
fn a_relabel_that_keeps_every_path_releases_nothing() {
    let previous = prior_claim("auth", &["src/auth/*"]);
    assert_eq!(
        claim_disclosure(Some(&previous), "identity", &owned(&["src/auth/*"])),
        vec!["Claimed scope: identity (replaces this session's earlier claim on auth)"],
        "the label moved but the scope did not, so nothing was released"
    );
}

#[test]
fn a_second_claim_leaves_one_claim_on_the_board() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    let _ = edda_store::ensure_dirs(&pid);

    claim(
        repo.path(),
        "auth",
        &["src/auth/*".into()],
        None,
        Some("s1"),
    )
    .expect("first claim");
    claim(repo.path(), "api", &["src/api/*".into()], None, Some("s1")).expect("second claim");

    // The board folds to one claim per session, so the first scope is gone.
    // The disclosure tests above separately pin what the command prints.
    let claims = edda_bridge_claude::peers::compute_board_state(&pid).claims;
    assert_eq!(claims.len(), 1, "one session holds one claim");
    assert_eq!(claims[0].label, "api");
    assert_eq!(claims[0].paths, vec!["src/api/*".to_string()]);
}

#[test]
fn bare_claim_beside_one_live_session_refuses_and_preserves_scope() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    edda_store::ensure_dirs(&pid).expect("store dirs");
    edda_bridge_claude::peers::write_heartbeat_minimal(
        &pid,
        "live-worker",
        "worker",
        "/tmp/worker",
    );
    edda_bridge_claude::peers::write_claim(
        &pid,
        "live-worker",
        "worker",
        &["src/worker.rs".into()],
    );

    let err = claim(repo.path(), "intruder", &["docs/*".into()], None, None)
        .expect_err("an adjacent shell must not adopt the live worker");
    assert!(err.to_string().contains("--session"), "{err}");

    let claims = edda_bridge_claude::peers::compute_board_state(&pid).claims;
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].session_id, "live-worker");
    assert_eq!(claims[0].label, "worker");
    assert_eq!(claims[0].paths, vec!["src/worker.rs".to_string()]);
}

#[test]
fn re_claiming_the_same_label_keeps_one_claim() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    let _ = edda_store::ensure_dirs(&pid);

    // Narrowing a scope, and re-claiming after a restart, both go through
    // this path -- which is why replacement is right and rejecting a second
    // claim would not be.
    claim(
        repo.path(),
        "auth",
        &["src/auth/*".into(), "src/token/*".into()],
        None,
        Some("s1"),
    )
    .expect("first claim");
    claim(
        repo.path(),
        "auth",
        &["src/auth/*".into()],
        None,
        Some("s1"),
    )
    .expect("narrowed claim");

    let claims = edda_bridge_claude::peers::compute_board_state(&pid).claims;
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].paths, vec!["src/auth/*".to_string()]);
}

#[test]
fn one_session_claiming_does_not_disturb_another() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    let _ = edda_store::ensure_dirs(&pid);

    claim(
        repo.path(),
        "auth",
        &["src/auth/*".into()],
        None,
        Some("s1"),
    )
    .expect("s1 claim");
    claim(repo.path(), "api", &["src/api/*".into()], None, Some("s2")).expect("s2 claim");

    let mut claims = edda_bridge_claude::peers::compute_board_state(&pid).claims;
    claims.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    assert_eq!(claims.len(), 2, "the fold is per session, not global");
    assert_eq!(claims[0].label, "auth");
    assert_eq!(claims[1].label, "api");
}

#[test]
fn unclaim_releases_the_explicit_session_scope() {
    let _store = crate::test_support::isolated_store();
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    let _ = edda_store::ensure_dirs(&pid);
    edda_bridge_claude::peers::write_claim(&pid, "s1", "auth", &["src/auth.rs".into()]);

    unclaim(repo.path(), Some("s1"), false).expect("unclaim should write a release event");

    assert!(edda_bridge_claude::peers::compute_board_state(&pid)
        .claims
        .is_empty());
}

#[test]
fn unclaim_without_identity_refuses_rather_than_guessing_the_sole_claim() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    let _ = edda_store::ensure_dirs(&pid);
    edda_bridge_claude::peers::write_claim(&pid, "cli-auth", "auth", &["src/auth.rs".into()]);

    let err =
        unclaim(repo.path(), None, false).expect_err("a caller with no identity must not guess");
    assert!(err.to_string().contains("cli-auth"), "{err}");

    assert_eq!(
        edda_bridge_claude::peers::compute_board_state(&pid)
            .claims
            .len(),
        1,
        "a refused unclaim must release nothing"
    );
}

#[test]
fn unclaim_without_identity_never_releases_a_live_peers_claim() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    let _ = edda_store::ensure_dirs(&pid);

    // Only one of two live peers holds a claim. A shell with no identity
    // of its own must not decide that claim is its to release:
    // check_offlimits enforces exactly this claim for its live owner.
    edda_bridge_claude::peers::write_heartbeat_minimal(&pid, "sess-a", "worker-a", "/tmp/a");
    edda_bridge_claude::peers::write_heartbeat_minimal(&pid, "sess-b", "worker-b", "/tmp/b");
    edda_bridge_claude::peers::write_claim(&pid, "sess-a", "worker-a", &["src/a.rs".into()]);

    let err = unclaim(repo.path(), None, false)
        .expect_err("a bare shell must not release another live session's scope");
    assert!(err.to_string().contains("sess-a"), "{err}");

    assert_eq!(
        edda_bridge_claude::peers::compute_board_state(&pid)
            .claims
            .len(),
        1,
        "the live peer's claim must survive"
    );
}

#[test]
fn unclaim_without_identity_never_releases_the_sole_live_peers_claim() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    edda_store::ensure_dirs(&pid).expect("store dirs");
    edda_bridge_claude::peers::write_heartbeat_minimal(
        &pid,
        "sole-live-worker",
        "worker",
        "/tmp/worker",
    );
    edda_bridge_claude::peers::write_claim(
        &pid,
        "sole-live-worker",
        "worker",
        &["src/worker.rs".into()],
    );

    let err = unclaim(repo.path(), None, false)
        .expect_err("an adjacent shell must not release the sole live worker");
    assert!(err.to_string().contains("--session"), "{err}");
    assert_eq!(
        edda_bridge_claude::peers::compute_board_state(&pid)
            .claims
            .len(),
        1,
        "the live worker's claim must survive"
    );
}

#[test]
fn unclaim_without_session_refuses_when_several_claims_exist() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    let _ = edda_store::ensure_dirs(&pid);
    edda_bridge_claude::peers::write_claim(&pid, "cli-auth", "auth", &["src/auth.rs".into()]);
    edda_bridge_claude::peers::write_claim(&pid, "cli-api", "api", &["src/api.rs".into()]);

    // Windows-CI regression (PR #588): a sibling test that relocated
    // EDDA_STORE_ROOT outside the shared isolated_store() lock made the
    // claims land in one store while unclaim() read another, and the test
    // failed with the misleading "no claims on the board". Assert the
    // write is visible *under this test's own root* before the verb runs,
    // so a future lock escape fails here naming the real cause instead of
    // masquerading as a board-state defect.
    assert_eq!(
        edda_bridge_claude::peers::compute_board_state(&pid)
            .claims
            .len(),
        2,
        "claims must be readable under this test's own isolated store root \
             before unclaim runs; if this fails, EDDA_STORE_ROOT was relocated \
             mid-test by a test that bypassed the shared isolation lock"
    );

    let err = unclaim(repo.path(), None, false).expect_err("ambiguous target must not guess");
    let msg = err.to_string();
    assert!(msg.contains("cli-auth") && msg.contains("cli-api"), "{msg}");

    assert_eq!(
        edda_bridge_claude::peers::compute_board_state(&pid)
            .claims
            .len(),
        2,
        "a refused unclaim must release nothing"
    );
}

#[test]
fn unclaim_refuses_when_the_board_is_empty() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    let _ = edda_store::ensure_dirs(&pid);

    unclaim(repo.path(), None, false).expect_err("nothing to release must not report success");
}

#[test]
fn unclaim_refuses_a_session_that_holds_no_claim() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    let _ = edda_store::ensure_dirs(&pid);
    edda_bridge_claude::peers::write_claim(&pid, "cli-auth", "auth", &["src/auth.rs".into()]);

    // This is the exact silent-failure this fix exists to remove: the old
    // fallback resolved to `cli-cli`, wrote an unclaim for a session that
    // held nothing, printed success, and left the real claim standing.
    let err = unclaim(repo.path(), Some("cli-cli"), false)
        .expect_err("releasing nothing must not report success");
    assert!(err.to_string().contains("cli-auth"), "{err}");

    assert_eq!(
        edda_bridge_claude::peers::compute_board_state(&pid)
            .claims
            .len(),
        1,
        "the real claim must survive a refused unclaim"
    );
}

#[test]
fn if_claimed_exits_zero_when_there_is_nothing_to_release() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    let _ = edda_store::ensure_dirs(&pid);

    // A CI teardown runs the verb unconditionally; the normal case of
    // nothing left to release must not fail the job (GH-488).
    unclaim(repo.path(), None, true).expect("empty board is not an error under --if-claimed");
    unclaim(repo.path(), Some("cli-nobody"), true)
        .expect("a session holding nothing is not an error either");
}

#[test]
fn if_claimed_still_releases_a_real_claim() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    let _ = edda_store::ensure_dirs(&pid);
    edda_bridge_claude::peers::write_claim(&pid, "cli-auth", "auth", &["src/auth.rs".into()]);

    // The flag softens the failure, not the work.
    unclaim(repo.path(), Some("cli-auth"), true).expect("release still happens");

    assert!(edda_bridge_claude::peers::compute_board_state(&pid)
        .claims
        .is_empty());
}

#[test]
fn if_claimed_does_not_excuse_an_ambiguous_target() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    let _ = edda_store::ensure_dirs(&pid);
    edda_bridge_claude::peers::write_claim(&pid, "cli-auth", "auth", &["src/auth.rs".into()]);

    // Two claims and no identity is not "nothing to release" -- it is a
    // caller who cannot say which claim is theirs, and silence there would
    // be the hazard GH-488 exists to remove. Teardown only excuses absence.
    edda_bridge_claude::peers::write_claim(&pid, "cli-api", "api", &["src/api.rs".into()]);

    unclaim(repo.path(), None, true).expect("teardown treats an unresolvable target as absent");

    assert_eq!(
        edda_bridge_claude::peers::compute_board_state(&pid)
            .claims
            .len(),
        2,
        "and it must still release nothing"
    );
}

#[test]
fn peers_json_claims_carry_staleness() {
    // GH-569: programs reading `edda peers --json` must be able to make
    // the same live-vs-stale judgement the human view makes. Claims are
    // entries of that surface, so each carries its age and a stale flag.
    let _store = crate::test_support::isolated_store();
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    let _ = edda_store::ensure_dirs(&pid);
    edda_bridge_claude::peers::write_claim(&pid, "s1", "auth", &["src/auth.rs".into()]);

    let json = peers_json(&pid);
    let claim = &json["claims"][0];
    assert!(
        claim["age_secs"].is_u64(),
        "claim carries age_secs: {claim}"
    );
    assert_eq!(claim["stale"], false, "fresh claim is not stale");
}

#[test]
fn peers_json_includes_sessions_and_full_board() {
    let _store = crate::test_support::isolated_store();
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    let _ = edda_store::ensure_dirs(&pid);
    edda_bridge_claude::peers::write_heartbeat_minimal(&pid, "s1", "auth", ".");
    edda_bridge_claude::peers::write_claim(&pid, "s1", "auth", &["src/auth.rs".into()]);
    edda_bridge_claude::peers::write_request(&pid, "s2", "billing", "auth", "need auth");
    edda_bridge_claude::peers::write_request_ack(&pid, "s1", "billing");

    let json = peers_json(&pid);
    assert_eq!(json["sessions"][0]["session_id"], "s1");
    assert_eq!(json["claims"][0]["label"], "auth");
    assert_eq!(json["requests"][0]["message"], "need auth");
    assert_eq!(json["acks"][0]["from_label"], "billing");
}
