use std::fs;

use super::autoclaim::*;
use super::board::*;
use super::discovery::*;
use super::heartbeat::*;
use super::helpers::*;
use super::render_coord::*;
use super::*;
use crate::parse::now_rfc3339;
use crate::signals::{CommitInfo, FileEditCount, SessionSignals, TaskSnapshot};

#[test]
fn heartbeat_write_read_roundtrip() {
    let pid = "test_peers_hb_roundtrip";
    let sid = "test-session-001";
    let _ = edda_store::ensure_dirs(pid);

    let signals = SessionSignals {
        tasks: vec![TaskSnapshot {
            id: "1".into(),
            subject: "Implement auth".into(),
            status: "in_progress".into(),
        }],
        files_modified: vec![
            FileEditCount {
                path: "src/auth/mod.rs".into(),
                count: 5,
            },
            FileEditCount {
                path: "src/auth/jwt.rs".into(),
                count: 3,
            },
        ],
        commits: vec![CommitInfo {
            hash: "abc1234".into(),
            message: "feat: add JWT auth".into(),
        }],
        failed_commands: vec![],
        ..Default::default()
    };

    write_heartbeat(pid, sid, &signals, Some("auth"));
    let hb = read_heartbeat(pid, sid).expect("should read heartbeat");

    assert_eq!(hb.session_id, sid);
    assert_eq!(hb.label, "auth");
    assert_eq!(hb.files_modified_count, 2);
    assert_eq!(hb.total_edits, 8);
    assert_eq!(hb.active_tasks.len(), 1);
    assert_eq!(hb.recent_commits.len(), 1);
    assert!(hb.recent_commits[0].contains("JWT auth"));

    // Cleanup
    remove_heartbeat(pid, sid);
    assert!(read_heartbeat(pid, sid).is_none());

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn coord_event_append_and_board_state() {
    let pid = "test_peers_board_state";
    let _ = edda_store::ensure_dirs(pid);

    // Clean up any existing decisions file
    let _ = fs::remove_file(coordination_path(pid));

    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
    write_claim(pid, "s2", "billing", &["src/billing/*".into()]);
    write_binding(pid, "s1", "auth", "auth.method", "JWT RS256");
    write_request(pid, "s2", "billing", "auth", "Export AuthToken type");

    let board = compute_board_state(pid);
    assert_eq!(board.claims.len(), 2);
    assert_eq!(board.bindings.len(), 1);
    assert_eq!(board.bindings[0].key, "auth.method");
    assert_eq!(board.bindings[0].value, "JWT RS256");
    assert_eq!(board.requests.len(), 1);
    assert_eq!(board.requests[0].to_label, "auth");

    // Unclaim should remove
    write_unclaim(pid, "s1");
    let board2 = compute_board_state(pid);
    assert_eq!(board2.claims.len(), 1);
    assert_eq!(board2.claims[0].label, "billing");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn discover_peers_excludes_self() {
    let pid = "test_peers_discover";
    let _ = edda_store::ensure_dirs(pid);

    let signals = SessionSignals::default();
    write_heartbeat(pid, "self-session", &signals, Some("self"));
    write_heartbeat(pid, "peer-session", &signals, Some("peer"));

    let peers = discover_active_peers(pid, "self-session");
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].label, "peer");

    remove_heartbeat(pid, "self-session");
    remove_heartbeat(pid, "peer-session");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_protocol_solo_no_bindings_returns_none() {
    let pid = "test_peers_solo";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let result = render_coordination_protocol(pid, "only-session", ".");
    assert!(result.is_none(), "solo with no bindings should return None");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_protocol_multi_session() {
    let pid = "test_peers_multi";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let signals = SessionSignals::default();
    write_heartbeat(pid, "s1", &signals, Some("auth"));
    write_heartbeat(pid, "s2", &signals, Some("billing"));
    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
    write_claim(pid, "s2", "billing", &["src/billing/*".into()]);
    write_binding(pid, "s1", "auth", "auth.method", "JWT RS256");

    let result = render_coordination_protocol(pid, "s2", ".").unwrap();
    assert!(result.contains("Coordination Protocol"));
    assert!(result.contains("Off-limits"));
    assert!(result.contains("auth"));
    assert!(result.contains("Binding Decisions"));
    assert!(result.contains("JWT RS256"));

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn auto_label_from_crate_path() {
    let signals = SessionSignals {
        files_modified: vec![FileEditCount {
            path: "crates/edda-bridge-claude/src/peers.rs".into(),
            count: 10,
        }],
        ..Default::default()
    };
    assert_eq!(auto_label(&signals), "edda-bridge-claude");
}

#[test]
fn auto_label_from_src_module() {
    let signals = SessionSignals {
        files_modified: vec![FileEditCount {
            path: "src/auth/jwt.rs".into(),
            count: 5,
        }],
        ..Default::default()
    };
    assert_eq!(auto_label(&signals), "auth");
}

#[test]
fn format_age_display() {
    assert_eq!(format_age(30), "30s ago");
    assert_eq!(format_age(90), "1m ago");
    assert_eq!(format_age(3700), "1h ago");
}

#[test]
fn parse_rfc3339_basic() {
    let epoch = parse_rfc3339_to_epoch("2026-02-16T10:05:23Z").unwrap();
    assert!(epoch > 0);

    // Two timestamps 60 seconds apart should differ by ~60
    let a = parse_rfc3339_to_epoch("2026-02-16T10:05:00Z").unwrap();
    let b = parse_rfc3339_to_epoch("2026-02-16T10:06:00Z").unwrap();
    assert_eq!(b - a, 60);
}

#[test]
fn compaction_preserves_current_state() {
    let pid = "test_peers_compaction";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Write a bunch of events including overrides
    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
    write_claim(pid, "s2", "billing", &["src/billing/*".into()]);
    write_binding(pid, "s1", "auth", "db.engine", "SQLite");
    write_binding(pid, "s1", "auth", "db.engine", "PostgreSQL"); // override
    write_request(pid, "s2", "billing", "auth", "Export AuthToken");
    write_unclaim(pid, "s1"); // removes s1 claim

    // Compact
    let lines = compute_board_state_for_compaction(pid);
    // Should have: 1 claim (s2), 1 decision (PostgreSQL), 1 request
    assert_eq!(lines.len(), 3);

    // Verify by parsing
    let board_before = compute_board_state(pid);
    assert_eq!(board_before.claims.len(), 1);
    assert_eq!(board_before.claims[0].label, "billing");
    assert_eq!(board_before.bindings.len(), 1);
    assert_eq!(board_before.bindings[0].value, "PostgreSQL");

    // Write compacted back
    let path = coordination_path(pid);
    let content = lines.join("\n");
    fs::write(&path, format!("{content}\n")).unwrap();

    // Verify same state after compaction
    let board_after = compute_board_state(pid);
    assert_eq!(board_after.claims.len(), 1);
    assert_eq!(board_after.claims[0].label, "billing");
    assert_eq!(board_after.bindings.len(), 1);
    assert_eq!(board_after.bindings[0].value, "PostgreSQL");
    assert_eq!(board_after.requests.len(), 1);

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn full_lifecycle_multi_session() {
    let pid = "test_peers_lifecycle";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Simulate 4 sessions starting
    let signals = SessionSignals::default();
    write_heartbeat(pid, "s1", &signals, Some("auth"));
    write_heartbeat(pid, "s2", &signals, Some("billing"));
    write_heartbeat(pid, "s3", &signals, Some("api"));
    write_heartbeat(pid, "s4", &signals, Some("frontend"));

    // Claims
    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
    write_claim(pid, "s2", "billing", &["src/billing/*".into()]);
    write_claim(pid, "s3", "api", &["src/api/*".into()]);
    write_claim(pid, "s4", "frontend", &["src/ui/*".into()]);

    // s1 makes a decision
    write_binding(pid, "s1", "auth", "auth.method", "JWT RS256");

    // s3 sends request to s2
    write_request(pid, "s3", "api", "billing", "Export BillingPlan type");

    // Verify s3 sees coordination protocol
    let proto = render_coordination_protocol(pid, "s3", ".").unwrap();
    assert!(proto.contains("Coordination Protocol"));
    assert!(proto.contains("4")); // 3 peers + self = 4 agents
    assert!(proto.contains("JWT RS256"));

    // Verify s2 sees the request
    let proto_s2 = render_coordination_protocol(pid, "s2", ".").unwrap();
    assert!(proto_s2.contains("Export BillingPlan type"));

    // s2 sees peer updates (lightweight)
    let updates = render_peer_updates(pid, "s2").unwrap();
    assert!(updates.contains("Peers"));
    assert!(updates.contains("Export BillingPlan"));

    // Solo session should still see bindings (but not peer sections)
    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    remove_heartbeat(pid, "s3");
    remove_heartbeat(pid, "s4");
    let solo = render_coordination_protocol(pid, "s5", ".").unwrap();
    assert!(
        solo.contains("Binding Decisions"),
        "solo should show bindings"
    );
    assert!(solo.contains("JWT RS256"), "solo should show binding value");
    assert!(
        !solo.contains("Coordination Protocol"),
        "solo should NOT show coordination header"
    );
    assert!(
        !solo.contains("Peers Working On"),
        "solo should NOT show peer sections"
    );

    // discover_all_sessions returns nothing after cleanup
    let all = discover_all_sessions(pid);
    assert!(all.is_empty());

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn binding_dedup_in_board() {
    let pid = "test_peers_decision_dedup";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_binding(pid, "s1", "auth", "db.engine", "SQLite");
    write_binding(pid, "s1", "auth", "db.engine", "PostgreSQL");

    let board = compute_board_state(pid);
    assert_eq!(board.bindings.len(), 1);
    assert_eq!(board.bindings[0].value, "PostgreSQL");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn migration_renames_decisions_to_coordination() {
    let pid = "test_peers_migration";
    let _ = edda_store::ensure_dirs(pid);
    let state_dir = edda_store::project_dir(pid).join("state");
    let _ = fs::create_dir_all(&state_dir);

    // Create legacy decisions.jsonl with content
    let old_path = state_dir.join("decisions.jsonl");
    let new_path = state_dir.join("coordination.jsonl");
    let _ = fs::remove_file(&old_path);
    let _ = fs::remove_file(&new_path);
    fs::write(&old_path, "{\"test\":true}\n").unwrap();

    // Calling coordination_path triggers migration
    let result = coordination_path(pid);
    assert_eq!(result, new_path);
    assert!(
        new_path.exists(),
        "coordination.jsonl should exist after migration"
    );
    assert!(
        !old_path.exists(),
        "decisions.jsonl should be removed after migration"
    );
    let content = fs::read_to_string(&new_path).unwrap();
    assert!(content.contains("test"), "content should be preserved");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn migration_skips_if_coordination_exists() {
    let pid = "test_peers_migration_skip";
    let _ = edda_store::ensure_dirs(pid);
    let state_dir = edda_store::project_dir(pid).join("state");
    let _ = fs::create_dir_all(&state_dir);

    // Both files exist — should NOT migrate (coordination.jsonl takes priority)
    let old_path = state_dir.join("decisions.jsonl");
    let new_path = state_dir.join("coordination.jsonl");
    fs::write(&old_path, "old\n").unwrap();
    fs::write(&new_path, "new\n").unwrap();

    let _ = coordination_path(pid);
    // coordination.jsonl should keep its original content
    let content = fs::read_to_string(&new_path).unwrap();
    assert_eq!(content, "new\n");
    // decisions.jsonl should still exist (not deleted when coordination.jsonl exists)
    assert!(old_path.exists());

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn serde_backward_compat_decision_deserializes_as_binding() {
    // Old coordination logs have event_type: "decision". Verify they deserialize as Binding.
    let json = r#"{"ts":"2026-02-18T00:00:00Z","session_id":"s1","event_type":"decision","payload":{"key":"db","value":"pg","by_label":"auth"}}"#;
    let event: CoordEvent = serde_json::from_str(json).unwrap();
    assert_eq!(event.event_type, CoordEventType::Binding);
}

#[test]
fn serde_new_binding_serializes_as_binding() {
    let event = CoordEvent {
        ts: "2026-02-18T00:00:00Z".to_string(),
        session_id: "s1".to_string(),
        event_type: CoordEventType::Binding,
        payload: serde_json::json!({"key": "db"}),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(
        json.contains("\"binding\""),
        "new events should serialize as 'binding', got: {json}"
    );
}

#[test]
fn render_protocol_shows_peer_tasks() {
    let pid = "test_peers_tasks_render";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let signals_with_task = SessionSignals {
        tasks: vec![TaskSnapshot {
            id: "1".into(),
            subject: "Implement auth flow".into(),
            status: "in_progress".into(),
        }],
        files_modified: vec![FileEditCount {
            path: "crates/edda-auth/src/lib.rs".into(),
            count: 3,
        }],
        ..Default::default()
    };
    write_heartbeat(pid, "s1", &signals_with_task, Some("auth"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));

    let result = render_coordination_protocol(pid, "s2", ".").unwrap();
    assert!(
        result.contains("Peers Working On"),
        "should have working-on section, got:\n{result}"
    );
    assert!(
        result.contains("Implement auth flow"),
        "should show task subject, got:\n{result}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_protocol_shows_focus_files_when_no_tasks() {
    let pid = "test_peers_focus_render";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Session with files but no in_progress tasks
    let signals = SessionSignals {
        files_modified: vec![FileEditCount {
            path: "crates/edda-auth/src/lib.rs".into(),
            count: 5,
        }],
        ..Default::default()
    };
    write_heartbeat(pid, "s1", &signals, Some("auth"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));

    let result = render_coordination_protocol(pid, "s2", ".").unwrap();
    assert!(
        result.contains("Peers Working On"),
        "should have working-on section, got:\n{result}"
    );
    assert!(
        result.contains("editing"),
        "should show focus files, got:\n{result}"
    );
    assert!(
        result.contains("lib.rs"),
        "should show file basename, got:\n{result}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_peer_updates_shows_tasks() {
    let pid = "test_peers_updates_tasks";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let signals = SessionSignals {
        tasks: vec![TaskSnapshot {
            id: "1".into(),
            subject: "Fix billing bug".into(),
            status: "in_progress".into(),
        }],
        ..Default::default()
    };
    write_heartbeat(pid, "s1", &signals, Some("billing"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"));

    let result = render_peer_updates(pid, "s2").unwrap();
    assert!(
        result.contains("Fix billing bug"),
        "should show peer task, got:\n{result}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_peer_updates_shows_focus_files() {
    let pid = "test_peers_updates_focus";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Peer with focus files but no tasks
    let signals = SessionSignals {
        files_modified: vec![crate::signals::FileEditCount {
            path: "src/billing/invoice.rs".into(),
            count: 3,
        }],
        ..Default::default()
    };
    write_heartbeat(pid, "s1", &signals, Some("billing"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"));

    let result = render_peer_updates(pid, "s2").unwrap();
    assert!(
        result.contains("invoice.rs"),
        "should show focus file, got:\n{result}"
    );
    assert!(
        result.contains("billing"),
        "should show peer label, got:\n{result}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_peer_updates_shows_bare_label() {
    let pid = "test_peers_updates_bare";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Peer with no tasks and no focus files
    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("billing"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"));

    let result = render_peer_updates(pid, "s2").unwrap();
    assert!(
        result.contains("billing"),
        "should show peer label even without tasks/files, got:\n{result}"
    );
    // Should not be just the header
    let lines: Vec<&str> = result.lines().collect();
    assert!(
        lines.len() > 2,
        "should have more than just header + L2 instructions, got:\n{result}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_peer_updates_includes_l2_instructions() {
    let pid = "test_peers_updates_l2";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("billing"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"));

    let result = render_peer_updates(pid, "s2").unwrap();
    assert!(
        result.contains("edda claim"),
        "should include claim instruction, got:\n{result}"
    );
    assert!(
        result.contains("edda request"),
        "should include request instruction, got:\n{result}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Solo binding visibility tests (issue #147) ──

#[test]
fn render_protocol_solo_with_bindings() {
    let pid = "test_peers_solo_bindings";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // No heartbeats (solo), but write bindings
    write_binding(pid, "s1", "auth", "auth.method", "JWT RS256");
    write_binding(pid, "s1", "auth", "db.engine", "PostgreSQL");

    let result = render_coordination_protocol(pid, "solo-session", ".").unwrap();
    assert!(
        result.contains("Binding Decisions"),
        "should have binding header, got:\n{result}"
    );
    assert!(
        result.contains("JWT RS256"),
        "should show binding value, got:\n{result}"
    );
    assert!(
        result.contains("PostgreSQL"),
        "should show second binding, got:\n{result}"
    );
    assert!(
        !result.contains("Coordination Protocol"),
        "should NOT have coordination header, got:\n{result}"
    );
    assert!(
        !result.contains("Peers Working On"),
        "should NOT have peer sections, got:\n{result}"
    );
    assert!(
        !result.contains("Off-limits"),
        "should NOT have off-limits, got:\n{result}"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_peer_updates_solo_with_bindings() {
    let pid = "test_peers_updates_solo_bindings";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // No heartbeats (solo), but write bindings
    write_binding(pid, "s1", "auth", "auth.method", "JWT RS256");

    let result = render_peer_updates(pid, "solo-session").unwrap();
    assert!(
        result.contains("JWT RS256"),
        "should show binding, got:\n{result}"
    );
    assert!(
        !result.contains("Peers"),
        "should NOT have peers header, got:\n{result}"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_peer_updates_solo_no_bindings() {
    let pid = "test_peers_updates_solo_none";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // No heartbeats, no bindings
    let result = render_peer_updates(pid, "solo-session");
    assert!(result.is_none(), "solo with no bindings should return None");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── find_binding_conflict tests (issue #121) ──

#[test]
fn binding_conflict_detects_different_value() {
    let pid = "test_conflict_different";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_binding(pid, "s1", "auth", "db.engine", "postgres");

    let conflict = find_binding_conflict(pid, "db.engine", "mysql");
    assert!(conflict.is_some(), "should detect conflict");
    let c = conflict.unwrap();
    assert_eq!(c.existing_value, "postgres");
    assert_eq!(c.by_label, "auth");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn binding_conflict_same_value_no_conflict() {
    let pid = "test_conflict_same";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_binding(pid, "s1", "auth", "db.engine", "postgres");

    let conflict = find_binding_conflict(pid, "db.engine", "postgres");
    assert!(conflict.is_none(), "same value should not conflict");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn binding_conflict_no_existing_binding() {
    let pid = "test_conflict_none";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let conflict = find_binding_conflict(pid, "db.engine", "postgres");
    assert!(
        conflict.is_none(),
        "no existing binding should not conflict"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── infer_session_id tests ──

#[test]
fn infer_session_no_heartbeats() {
    let pid = "test_infer_none";
    let _ = edda_store::ensure_dirs(pid);

    let result = infer_session_id(pid);
    assert!(result.is_none(), "no heartbeats → None");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn infer_session_one_active() {
    let pid = "test_infer_one";
    let _ = edda_store::ensure_dirs(pid);

    write_heartbeat(pid, "sess-abc", &SessionSignals::default(), Some("auth"));

    let result = infer_session_id(pid);
    assert_eq!(result, Some(("sess-abc".into(), "auth".into())));

    remove_heartbeat(pid, "sess-abc");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn infer_session_two_active_is_ambiguous() {
    let pid = "test_infer_two";
    let _ = edda_store::ensure_dirs(pid);

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));

    let result = infer_session_id(pid);
    assert!(result.is_none(), "two active → ambiguous → None");

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn infer_session_one_active_one_stale() {
    let pid = "test_infer_stale";
    let _ = edda_store::ensure_dirs(pid);

    // Write one fresh heartbeat
    write_heartbeat(pid, "fresh", &SessionSignals::default(), Some("frontend"));

    // Write a stale heartbeat by manually setting old timestamp
    let stale_path = heartbeat_path(pid, "stale");
    let stale_hb = serde_json::json!({
        "session_id": "stale",
        "started_at": "2020-01-01T00:00:00Z",
        "last_heartbeat": "2020-01-01T00:00:00Z",
        "label": "old",
        "focus_files": [],
        "active_tasks": [],
        "files_modified_count": 0,
        "total_edits": 0,
        "recent_commits": []
    });
    let _ = fs::create_dir_all(stale_path.parent().unwrap());
    let _ = fs::write(
        &stale_path,
        serde_json::to_string_pretty(&stale_hb).unwrap(),
    );

    let result = infer_session_id(pid);
    assert_eq!(result, Some(("fresh".into(), "frontend".into())));

    remove_heartbeat(pid, "fresh");
    remove_heartbeat(pid, "stale");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn infer_session_only_stale() {
    let pid = "test_infer_all_stale";
    let _ = edda_store::ensure_dirs(pid);

    let stale_path = heartbeat_path(pid, "old-session");
    let stale_hb = serde_json::json!({
        "session_id": "old-session",
        "started_at": "2020-01-01T00:00:00Z",
        "last_heartbeat": "2020-01-01T00:00:00Z",
        "label": "old",
        "focus_files": [],
        "active_tasks": [],
        "files_modified_count": 0,
        "total_edits": 0,
        "recent_commits": []
    });
    let _ = fs::create_dir_all(stale_path.parent().unwrap());
    let _ = fs::write(
        &stale_path,
        serde_json::to_string_pretty(&stale_hb).unwrap(),
    );

    let result = infer_session_id(pid);
    assert!(result.is_none(), "only stale heartbeats → None");

    remove_heartbeat(pid, "old-session");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Issue #148 Gap 6: Cross-session decision conflict ──

#[test]
fn cross_session_binding_conflict_last_write_wins() {
    let pid = "test_cross_sess_conflict";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Session A decides db.engine=postgres
    write_binding(pid, "s1", "auth", "db.engine", "postgres");
    // Session B decides db.engine=mysql (conflict — last write wins)
    write_binding(pid, "s2", "billing", "db.engine", "mysql");

    let board = compute_board_state(pid);
    assert_eq!(
        board.bindings.len(),
        1,
        "should have 1 binding (deduped by key)"
    );
    assert_eq!(board.bindings[0].value, "mysql", "last write should win");
    assert_eq!(board.bindings[0].by_session, "s2");

    // Both sessions see the latest value via render_peer_updates
    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));

    let updates_s1 = render_peer_updates(pid, "s1").unwrap();
    assert!(
        updates_s1.contains("mysql"),
        "Session A should see latest binding, got:\n{updates_s1}"
    );

    let updates_s2 = render_peer_updates(pid, "s2").unwrap();
    assert!(
        updates_s2.contains("mysql"),
        "Session B should see latest binding, got:\n{updates_s2}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn cross_session_different_keys_both_visible() {
    let pid = "test_cross_sess_diff_keys";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Session A decides db.engine=postgres
    write_binding(pid, "s1", "auth", "db.engine", "postgres");
    // Session B decides auth.method=JWT (different key — no conflict)
    write_binding(pid, "s2", "billing", "auth.method", "JWT");

    let board = compute_board_state(pid);
    assert_eq!(
        board.bindings.len(),
        2,
        "should have 2 bindings (different keys)"
    );

    // Both sessions see both bindings
    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));

    let updates_s1 = render_peer_updates(pid, "s1").unwrap();
    assert!(
        updates_s1.contains("postgres"),
        "s1 should see db.engine binding"
    );
    assert!(
        updates_s1.contains("JWT"),
        "s1 should see auth.method binding"
    );

    let updates_s2 = render_peer_updates(pid, "s2").unwrap();
    assert!(
        updates_s2.contains("postgres"),
        "s2 should see db.engine binding"
    );
    assert!(
        updates_s2.contains("JWT"),
        "s2 should see auth.method binding"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Heartbeat label fallback tests (#146) ──

#[test]
fn request_delivered_via_heartbeat_label_no_claim() {
    let pid = "test_hb_fallback_request";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Two sessions: s1 (peer) and s2 (me) — both have heartbeats, no claims
    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));

    // s1 sends request to "billing" (s2's heartbeat label)
    write_request(pid, "s1", "auth", "billing", "please expose /api/users");

    let result = render_coordination_protocol(pid, "s2", ".").unwrap();
    assert!(
        result.contains("Requests to you"),
        "request to heartbeat label should appear, got:\n{result}"
    );
    assert!(
        result.contains("please expose /api/users"),
        "request message should appear, got:\n{result}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn explicit_claim_wins_over_heartbeat_for_requests() {
    let pid = "test_claim_wins_request";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // s2 has heartbeat "auth" but claim "backend"
    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("peer"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"));
    write_claim(pid, "s2", "backend", &[]);

    // Request to "backend" (claim label) should arrive
    write_request(pid, "s1", "peer", "backend", "need backend help");
    // Request to "auth" (heartbeat label) should NOT arrive (claim overrides)
    write_request(pid, "s1", "peer", "auth", "wrong target");

    let result = render_coordination_protocol(pid, "s2", ".").unwrap();
    assert!(
        result.contains("need backend help"),
        "request to claim label should appear, got:\n{result}"
    );
    assert!(
        !result.contains("wrong target"),
        "request to heartbeat label should NOT appear when claim exists, got:\n{result}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn no_heartbeat_no_claim_no_requests() {
    let pid = "test_no_identity_request";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // s1 is peer, s2 has no heartbeat and no claim
    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"));
    write_request(pid, "s1", "auth", "ghost", "hello ghost");

    // s2 renders — should not see the request (no identity)
    let result = render_coordination_protocol(pid, "s2", ".").unwrap();
    assert!(
        !result.contains("Requests to you"),
        "agent with no identity should see no requests, got:\n{result}"
    );

    remove_heartbeat(pid, "s1");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn heartbeat_scope_display_without_claim() {
    let pid = "test_hb_scope_display";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("peer"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"));

    let result = render_coordination_protocol(pid, "s2", ".").unwrap();
    // Without a claim, should show actionable nudge with label-based suggestion
    assert!(
        result.contains("**Claim your scope**"),
        "should show claim nudge when no claim exists, got:\n{result}"
    );
    assert!(
        result.contains("edda claim \"auth\""),
        "should suggest claim with heartbeat label, got:\n{result}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn claim_scope_display_with_paths() {
    let pid = "test_claim_scope_display";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("peer"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"));
    write_claim(pid, "s2", "backend", &["src/api/*".into()]);

    let result = render_coordination_protocol(pid, "s2", ".").unwrap();
    assert!(
        result.contains("Your scope: **backend** (src/api/*)"),
        "claim scope should show label + paths, got:\n{result}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn multi_session_shows_l2_instructions() {
    let pid = "test_l2_instructions";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));

    let result = render_coordination_protocol(pid, "s2", ".").unwrap();
    assert!(
        result.contains("edda claim"),
        "multi-session should contain claim instruction, got:\n{result}"
    );
    assert!(
        result.contains("edda request"),
        "multi-session should contain request instruction, got:\n{result}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn solo_mode_no_l2_instructions() {
    let pid = "test_solo_no_l2_instr";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Solo with binding (renders "## Binding Decisions" only)
    write_binding(pid, "s1", "auth", "db.engine", "postgres");
    let result = render_coordination_protocol(pid, "solo", ".").unwrap();
    assert!(
        !result.contains("edda claim"),
        "solo mode should NOT contain claim instruction, got:\n{result}"
    );
    assert!(
        !result.contains("edda request"),
        "solo mode should NOT contain request instruction, got:\n{result}"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn peer_updates_request_via_heartbeat_fallback() {
    let pid = "test_peer_updates_hb_req";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));
    write_request(pid, "s1", "auth", "billing", "need billing API");

    let result = render_peer_updates(pid, "s2").unwrap();
    assert!(
        result.contains("need billing API"),
        "peer_updates should route request via heartbeat label, got:\n{result}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Auto-claim tests (issue #24) ──

#[test]
fn derive_scope_from_crate_files() {
    let files = vec![
        FileEditCount {
            path: "crates/edda-store/src/lib.rs".into(),
            count: 5,
        },
        FileEditCount {
            path: "crates/edda-store/src/resolve.rs".into(),
            count: 3,
        },
    ];
    let (label, paths) = derive_scope_from_files(&files).unwrap();
    assert_eq!(label, "edda-store");
    assert_eq!(paths, vec!["crates/edda-store/*"]);
}

#[test]
fn derive_scope_from_src_module() {
    let files = vec![
        FileEditCount {
            path: "/repo/src/auth/jwt.rs".into(),
            count: 5,
        },
        FileEditCount {
            path: "/repo/src/auth/middleware.rs".into(),
            count: 2,
        },
    ];
    let (label, paths) = derive_scope_from_files(&files).unwrap();
    assert_eq!(label, "auth");
    assert_eq!(paths, vec!["src/auth/*"]);
}

#[test]
fn derive_scope_empty_files() {
    assert!(derive_scope_from_files(&[]).is_none());
}

#[test]
fn auto_claim_writes_claim_from_signals() {
    let pid = "test_autoclaim_writes";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let signals = SessionSignals {
        files_modified: vec![FileEditCount {
            path: "crates/edda-store/src/lib.rs".into(),
            count: 5,
        }],
        ..Default::default()
    };

    maybe_auto_claim(pid, "s1", &signals);

    let board = compute_board_state(pid);
    assert_eq!(board.claims.len(), 1, "should have 1 claim");
    assert_eq!(board.claims[0].label, "edda-store");
    assert_eq!(board.claims[0].paths, vec!["crates/edda-store/*"]);

    remove_autoclaim_state(pid, "s1");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn auto_claim_skips_when_manual_claim_exists() {
    let pid = "test_autoclaim_skip_manual";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Manual claim first
    write_claim(pid, "s1", "backend", &["src/api/*".into()]);

    let signals = SessionSignals {
        files_modified: vec![FileEditCount {
            path: "crates/edda-store/src/lib.rs".into(),
            count: 5,
        }],
        ..Default::default()
    };

    maybe_auto_claim(pid, "s1", &signals);

    let board = compute_board_state(pid);
    let claim = board.claims.iter().find(|c| c.session_id == "s1").unwrap();
    assert_eq!(
        claim.label, "backend",
        "manual claim should be preserved, not overwritten by auto-claim"
    );

    remove_autoclaim_state(pid, "s1");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn auto_claim_dedup_no_repeated_writes() {
    let pid = "test_autoclaim_dedup";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let signals = SessionSignals {
        files_modified: vec![FileEditCount {
            path: "crates/edda-store/src/lib.rs".into(),
            count: 5,
        }],
        ..Default::default()
    };

    maybe_auto_claim(pid, "s1", &signals);
    maybe_auto_claim(pid, "s1", &signals);

    let content = fs::read_to_string(coordination_path(pid)).unwrap_or_default();
    let claim_count = content.lines().filter(|l| l.contains("\"claim\"")).count();
    assert_eq!(claim_count, 1, "dedup should prevent repeated claim writes");

    remove_autoclaim_state(pid, "s1");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn auto_claim_updates_on_scope_change() {
    let pid = "test_autoclaim_scope_change";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let signals1 = SessionSignals {
        files_modified: vec![FileEditCount {
            path: "crates/edda-store/src/lib.rs".into(),
            count: 5,
        }],
        ..Default::default()
    };
    maybe_auto_claim(pid, "s1", &signals1);

    let signals2 = SessionSignals {
        files_modified: vec![FileEditCount {
            path: "crates/edda-bridge-claude/src/peers.rs".into(),
            count: 10,
        }],
        ..Default::default()
    };
    maybe_auto_claim(pid, "s1", &signals2);

    let board = compute_board_state(pid);
    let claim = board.claims.iter().find(|c| c.session_id == "s1").unwrap();
    assert_eq!(
        claim.label, "edda-bridge-claude",
        "claim should update to new scope"
    );

    remove_autoclaim_state(pid, "s1");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn auto_claim_cleanup_removes_state_file() {
    let pid = "test_autoclaim_cleanup";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let signals = SessionSignals {
        files_modified: vec![FileEditCount {
            path: "crates/edda-store/src/lib.rs".into(),
            count: 5,
        }],
        ..Default::default()
    };
    maybe_auto_claim(pid, "s1", &signals);

    let state_path = autoclaim_state_path(pid, "s1");
    assert!(
        state_path.exists(),
        "state file should exist after auto-claim"
    );

    remove_autoclaim_state(pid, "s1");
    assert!(
        !state_path.exists(),
        "state file should be removed after cleanup"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_shows_branch_when_present() {
    let pid = "test_peers_branch_render";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Write heartbeat with branch via JSON (bypassing auto-detect)
    let hb_json = serde_json::json!({
        "session_id": "s1",
        "started_at": now_rfc3339(),
        "last_heartbeat": now_rfc3339(),
        "label": "auth",
        "focus_files": ["src/auth/lib.rs"],
        "active_tasks": [],
        "files_modified_count": 1,
        "total_edits": 3,
        "recent_commits": [],
        "branch": "feat/issue-81-peer-branch"
    });
    let path = edda_store::project_dir(pid)
        .join("state")
        .join("session.s1.json");
    let _ = fs::create_dir_all(path.parent().unwrap());
    fs::write(&path, serde_json::to_string_pretty(&hb_json).unwrap()).unwrap();

    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));

    let result = render_coordination_protocol(pid, "s2", ".").unwrap();
    assert!(
        result.contains("[branch: feat/issue-81-peer-branch]"),
        "should show branch in protocol, got:\n{result}"
    );

    let updates = render_peer_updates(pid, "s2").unwrap();
    assert!(
        updates.contains("[branch: feat/issue-81-peer-branch]"),
        "should show branch in peer updates, got:\n{updates}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_omits_branch_when_absent() {
    let pid = "test_peers_branch_absent";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Write heartbeat WITHOUT branch field (simulating old heartbeat format)
    let hb_json = serde_json::json!({
        "session_id": "s1",
        "started_at": now_rfc3339(),
        "last_heartbeat": now_rfc3339(),
        "label": "auth",
        "focus_files": ["src/auth/lib.rs"],
        "active_tasks": [],
        "files_modified_count": 1,
        "total_edits": 3,
        "recent_commits": []
    });
    let path = edda_store::project_dir(pid)
        .join("state")
        .join("session.s1.json");
    let _ = fs::create_dir_all(path.parent().unwrap());
    fs::write(&path, serde_json::to_string_pretty(&hb_json).unwrap()).unwrap();

    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));

    let result = render_coordination_protocol(pid, "s2", ".").unwrap();
    assert!(
        !result.contains("[branch:"),
        "should NOT show branch marker when absent, got:\n{result}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Precomputed _with variants match original output (#83) ──

#[test]
fn render_peer_updates_with_matches_original() {
    let pid = "test_updates_with_match";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let signals = SessionSignals {
        tasks: vec![TaskSnapshot {
            id: "1".into(),
            subject: "Fix auth bug".into(),
            status: "in_progress".into(),
        }],
        ..Default::default()
    };
    write_heartbeat(pid, "s1", &signals, Some("auth"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"));
    write_binding(pid, "s1", "auth", "db.engine", "postgres");

    // Call original wrapper
    let original = render_peer_updates(pid, "s2");

    // Call _with variant with same data
    let peers = discover_active_peers(pid, "s2");
    let board = compute_board_state(pid);
    let precomputed = render_peer_updates_with(&peers, &board, pid, "s2");

    assert_eq!(
        original, precomputed,
        "precomputed variant should match original"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_coordination_protocol_with_matches_original() {
    let pid = "test_protocol_with_match";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let signals = SessionSignals {
        tasks: vec![TaskSnapshot {
            id: "1".into(),
            subject: "Implement billing".into(),
            status: "in_progress".into(),
        }],
        ..Default::default()
    };
    write_heartbeat(pid, "s1", &signals, Some("billing"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"));
    write_binding(pid, "s1", "billing", "payment.provider", "stripe");

    // Call original wrapper
    let original = render_coordination_protocol(pid, "s2", ".");

    // Call _with variant with same data
    let peers = discover_active_peers(pid, "s2");
    let board = compute_board_state(pid);
    let precomputed = render_coordination_protocol_with(&peers, &board, pid, "s2");

    assert_eq!(
        original, precomputed,
        "precomputed variant should match original"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn suggest_claim_command_from_focus_files() {
    let hb = SessionHeartbeat {
        session_id: "s1".into(),
        started_at: String::new(),
        last_heartbeat: String::new(),
        label: "worker".into(),
        focus_files: vec!["crates/edda-cli/src/main.rs".into()],
        active_tasks: Vec::new(),
        files_modified_count: 0,
        total_edits: 0,
        recent_commits: Vec::new(),
        branch: Some("feat/issue-131".into()),
        current_phase: None,
        parent_session_id: None,
    };
    let result = suggest_claim_command("worker", &Some(hb));
    assert!(result.contains("edda claim"), "should contain edda claim");
    assert!(
        result.contains("edda-cli"),
        "should derive crate name: {result}"
    );
}

#[test]
fn suggest_claim_command_from_branch() {
    let hb = SessionHeartbeat {
        session_id: "s1".into(),
        started_at: String::new(),
        last_heartbeat: String::new(),
        label: String::new(),
        focus_files: Vec::new(),
        active_tasks: Vec::new(),
        files_modified_count: 0,
        total_edits: 0,
        recent_commits: Vec::new(),
        branch: Some("feat/auth-refactor".into()),
        current_phase: None,
        parent_session_id: None,
    };
    let result = suggest_claim_command("", &Some(hb));
    assert!(
        result.contains("auth-refactor"),
        "should use branch suffix: {result}"
    );
}

#[test]
fn suggest_claim_command_fallback_label() {
    let result = suggest_claim_command("my-task", &None);
    assert!(
        result.contains("my-task"),
        "should use provided label: {result}"
    );
}

#[test]
fn suggest_claim_command_generic_fallback() {
    let result = suggest_claim_command("", &None);
    assert!(
        result.contains("<your-task>"),
        "should use generic placeholder: {result}"
    );
}

#[test]
fn protocol_no_claim_shows_nudge() {
    let pid = "test_protocol_no_claim_nudge";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // s1 is a peer, s2 is our session — neither has a claim
    let signals = SessionSignals {
        files_modified: vec![FileEditCount {
            path: "crates/edda-cli/src/main.rs".into(),
            count: 1,
        }],
        ..Default::default()
    };
    write_heartbeat(pid, "s1", &signals, Some("peer-agent"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("my-agent"));

    let peers = discover_active_peers(pid, "s2");
    let board = compute_board_state(pid);
    let result = render_coordination_protocol_with(&peers, &board, pid, "s2");

    assert!(result.is_some());
    let text = result.unwrap();
    assert!(
        text.contains("**Claim your scope**"),
        "should contain claim nudge: {text}"
    );
    assert!(
        text.contains("edda claim"),
        "should contain edda claim command: {text}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn protocol_with_claim_shows_scope() {
    let pid = "test_protocol_with_claim_scope";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("peer-agent"));
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("my-agent"));
    write_claim(pid, "s2", "my-agent", &["crates/edda-cli/*".to_string()]);

    let peers = discover_active_peers(pid, "s2");
    let board = compute_board_state(pid);
    let result = render_coordination_protocol_with(&peers, &board, pid, "s2");

    assert!(result.is_some());
    let text = result.unwrap();
    assert!(
        text.contains("Your scope: **my-agent**"),
        "should show claimed scope: {text}"
    );
    assert!(
        !text.contains("**Claim your scope**"),
        "should NOT show nudge when claimed: {text}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn protocol_nudge_uses_branch_context() {
    let pid = "test_protocol_nudge_branch";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Create heartbeat with branch info but no label — branch should be used
    let hb = SessionHeartbeat {
        session_id: "s2".into(),
        started_at: now_rfc3339(),
        last_heartbeat: now_rfc3339(),
        label: String::new(),
        focus_files: Vec::new(),
        active_tasks: Vec::new(),
        files_modified_count: 0,
        total_edits: 0,
        recent_commits: Vec::new(),
        branch: Some("feat/billing-v2".into()),
        current_phase: None,
        parent_session_id: None,
    };
    let hb_path = heartbeat_path(pid, "s2");
    let _ = fs::create_dir_all(hb_path.parent().unwrap());
    let _ = fs::write(&hb_path, serde_json::to_string_pretty(&hb).unwrap());

    // Create peer
    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("peer-agent"));

    let peers = discover_active_peers(pid, "s2");
    let board = compute_board_state(pid);
    let result = render_coordination_protocol_with(&peers, &board, pid, "s2");

    assert!(result.is_some());
    let text = result.unwrap();
    assert!(
        text.contains("billing-v2"),
        "should derive claim label from branch: {text}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_peer_updates_with_solo_bindings() {
    let pid = "test_updates_with_solo";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // No heartbeats (solo), but write bindings
    write_binding(pid, "s1", "auth", "auth.method", "JWT RS256");

    let peers = discover_active_peers(pid, "solo-session");
    let board = compute_board_state(pid);
    let result = render_peer_updates_with(&peers, &board, pid, "solo-session");

    assert!(result.is_some(), "solo with bindings should render");
    assert!(result.unwrap().contains("JWT RS256"), "should show binding");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_peer_updates_with_solo_no_bindings() {
    let pid = "test_updates_with_solo_empty";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let peers = discover_active_peers(pid, "solo-session");
    let board = compute_board_state(pid);
    let result = render_peer_updates_with(&peers, &board, pid, "solo-session");

    assert!(result.is_none(), "solo with no bindings should return None");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Auto-claim file incremental tests (#56) ──

#[test]
fn auto_claim_file_incremental_same_crate() {
    let pid = "test_autoclaim_file_incr";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Edit 3 files in same crate → single claim written
    maybe_auto_claim_file(pid, "s1", "crates/edda-store/src/lib.rs");
    maybe_auto_claim_file(pid, "s1", "crates/edda-store/src/paths.rs");
    maybe_auto_claim_file(pid, "s1", "crates/edda-store/src/event.rs");

    let board = compute_board_state(pid);
    let claims: Vec<_> = board
        .claims
        .iter()
        .filter(|c| c.session_id == "s1")
        .collect();
    assert_eq!(claims.len(), 1, "should have exactly one claim");
    assert_eq!(claims[0].label, "edda-store");

    // Verify state file has all 3 files tracked
    let state_path = autoclaim_state_path(pid, "s1");
    let state: AutoClaimState =
        serde_json::from_str(&fs::read_to_string(&state_path).unwrap()).unwrap();
    assert_eq!(state.files.len(), 3);

    remove_autoclaim_state(pid, "s1");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn auto_claim_file_scope_change() {
    let pid = "test_autoclaim_file_scope_change";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // First file in edda-store
    maybe_auto_claim_file(pid, "s1", "crates/edda-store/src/lib.rs");
    let board = compute_board_state(pid);
    let claim = board.claims.iter().find(|c| c.session_id == "s1").unwrap();
    assert_eq!(claim.label, "edda-store");

    // Second file in different crate → scope should change
    maybe_auto_claim_file(pid, "s1", "crates/edda-bridge-claude/src/dispatch.rs");
    let board2 = compute_board_state(pid);
    let claim2 = board2.claims.iter().find(|c| c.session_id == "s1").unwrap();
    // With 2 crates, label should be updated (might become multi-crate or dominant one)
    assert!(
        !claim2.label.is_empty(),
        "label should be non-empty after cross-crate edit"
    );

    remove_autoclaim_state(pid, "s1");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn auto_claim_file_skips_manual_claim() {
    let pid = "test_autoclaim_file_manual";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Manual claim exists
    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);

    // Auto-claim file should be skipped (no state file, manual claim exists)
    maybe_auto_claim_file(pid, "s1", "crates/edda-store/src/lib.rs");

    // Claim should still be "auth" (manual), not "edda-store" (auto)
    let board = compute_board_state(pid);
    let claim = board.claims.iter().find(|c| c.session_id == "s1").unwrap();
    assert_eq!(
        claim.label, "auth",
        "manual claim should not be overwritten"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn auto_claim_file_dedup_no_extra_writes() {
    let pid = "test_autoclaim_file_dedup";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Same file twice → only one claim event
    maybe_auto_claim_file(pid, "s1", "crates/edda-store/src/lib.rs");
    maybe_auto_claim_file(pid, "s1", "crates/edda-store/src/lib.rs");

    let board = compute_board_state(pid);
    let claims: Vec<_> = board
        .claims
        .iter()
        .filter(|c| c.session_id == "s1")
        .collect();
    assert_eq!(claims.len(), 1, "dedup: same file should produce one claim");

    remove_autoclaim_state(pid, "s1");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Request ack tests (#56) ──

#[test]
fn request_ack_filters_pending() {
    let pid = "test_req_ack_filters";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Setup: s1 claims "auth", s2 sends request to "auth"
    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
    write_request(pid, "s2", "billing", "auth", "Export AuthToken type");

    // s1 should see the pending request
    let pending = pending_requests_for_session(pid, "s1");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].message, "Export AuthToken type");

    // s1 acks the request
    write_request_ack(pid, "s1", "billing");

    // Now pending should be empty for s1
    let pending_after = pending_requests_for_session(pid, "s1");
    assert!(
        pending_after.is_empty(),
        "acked request should not appear as pending"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn request_ack_only_for_acker_session() {
    let pid = "test_req_ack_session_scope";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // s1 and s3 both claim "auth"
    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
    write_claim(pid, "s3", "auth", &["src/auth/*".into()]);
    write_request(pid, "s2", "billing", "auth", "Export AuthToken");

    // s1 acks
    write_request_ack(pid, "s1", "billing");

    // s1 should no longer see it
    let pending_s1 = pending_requests_for_session(pid, "s1");
    assert!(pending_s1.is_empty(), "s1 acked, should not see request");

    // s3 should still see it (different session, same label)
    let pending_s3 = pending_requests_for_session(pid, "s3");
    assert_eq!(
        pending_s3.len(),
        1,
        "s3 has not acked, should still see request"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn request_ack_in_board_state() {
    let pid = "test_req_ack_board";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_request_ack(pid, "s1", "billing");
    let board = compute_board_state(pid);
    assert_eq!(board.request_acks.len(), 1);
    assert_eq!(board.request_acks[0].acker_session, "s1");
    assert_eq!(board.request_acks[0].from_label, "billing");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn compaction_preserves_request_acks() {
    let pid = "test_compaction_acks";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
    write_request(pid, "s2", "billing", "auth", "Export AuthToken");
    write_request_ack(pid, "s1", "billing");

    // Before compaction: ack should exist
    let board_before = compute_board_state(pid);
    assert_eq!(board_before.request_acks.len(), 1);
    let pending_before = pending_requests_for_session(pid, "s1");
    assert!(
        pending_before.is_empty(),
        "acked request should not be pending"
    );

    // Compact
    let lines = compute_board_state_for_compaction(pid);
    assert_eq!(lines.len(), 3, "claim + request + ack = 3 lines");

    // Write compacted back
    let path = coordination_path(pid);
    let content = lines.join("\n");
    fs::write(&path, format!("{content}\n")).unwrap();

    // After compaction: ack should still exist
    let board_after = compute_board_state(pid);
    assert_eq!(board_after.request_acks.len(), 1);
    let pending_after = pending_requests_for_session(pid, "s1");
    assert!(
        pending_after.is_empty(),
        "acked request should still not be pending after compaction"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn serde_subagent_completed_serializes_and_parses() {
    let event = CoordEvent {
        ts: "2026-02-18T00:00:00Z".to_string(),
        session_id: "parent-session".to_string(),
        event_type: CoordEventType::SubagentCompleted,
        payload: serde_json::json!({
            "kind": "subagent_completed",
            "parent_session_id": "parent-session",
            "agent_id": "agent-1",
            "agent_type": "Explore",
            "summary": "done",
            "files_touched": ["a.rs"],
            "decisions": ["Decision: keep parser"],
            "commits": ["abc1234 feat: x"]
        }),
    };

    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"subagent_completed\""));

    let parsed: CoordEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.event_type, CoordEventType::SubagentCompleted);
}

#[test]
fn board_state_includes_subagent_completed_entries() {
    let pid = "test_subagent_board_state";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_subagent_completed(
        pid,
        "parent-session",
        &SubagentReport {
            agent_id: "agent-7",
            agent_type: "Plan",
            summary: "planning done",
            files_touched: &["a.rs".into(), "b.rs".into()],
            decisions: &["Decision: use compact mode".into()],
            commits: &["abc1234 feat: plan".into()],
        },
    );

    let board = compute_board_state(pid);
    assert_eq!(board.subagent_completions.len(), 1);
    let entry = &board.subagent_completions[0];
    assert_eq!(entry.parent_session_id, "parent-session");
    assert_eq!(entry.agent_id, "agent-7");
    assert_eq!(entry.agent_type, "Plan");
    assert!(entry.summary.contains("planning"));
    assert_eq!(entry.files_touched.len(), 2);
    assert_eq!(entry.decisions.len(), 1);
    assert_eq!(entry.commits.len(), 1);

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn compaction_preserves_subagent_completed() {
    let pid = "test_subagent_compaction";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_subagent_completed(
        pid,
        "parent-session",
        &SubagentReport {
            agent_id: "agent-8",
            agent_type: "Bash",
            summary: "completed",
            files_touched: &["x.rs".into()],
            decisions: &["Decision: run targeted tests".into()],
            commits: &["def5678 fix: adjust".into()],
        },
    );

    let lines = compute_board_state_for_compaction(pid);
    assert_eq!(lines.len(), 1, "only subagent event should remain");

    let path = coordination_path(pid);
    let content = lines.join("\n");
    fs::write(&path, format!("{content}\n")).unwrap();

    let board = compute_board_state(pid);
    assert_eq!(board.subagent_completions.len(), 1);
    assert_eq!(board.subagent_completions[0].agent_id, "agent-8");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn pending_requests_no_label_returns_empty() {
    let pid = "test_pending_no_label";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // s1 has no claim and no heartbeat → no label → no pending requests
    write_request(pid, "s2", "billing", "auth", "Need auth API");
    let pending = pending_requests_for_session(pid, "s1");
    assert!(
        pending.is_empty(),
        "session with no label should have no pending requests"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn write_subagent_heartbeat_sets_parent() {
    let pid = "test_subagent_heartbeat";
    let _ = edda_store::ensure_dirs(pid);

    write_subagent_heartbeat(pid, "agent-123", "parent-session", "sub:Explore", ".");

    let hb = read_heartbeat(pid, "agent-123").expect("heartbeat should exist");
    assert_eq!(hb.session_id, "agent-123");
    assert_eq!(hb.label, "sub:Explore");
    assert_eq!(
        hb.parent_session_id.as_deref(),
        Some("parent-session"),
        "parent_session_id should be set"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn cleanup_subagent_heartbeats_selective() {
    let pid = "test_cleanup_subagent";
    let _ = edda_store::ensure_dirs(pid);

    // Create parent heartbeat
    write_heartbeat_minimal(pid, "parent-1", "main-session");
    // Create two sub-agent heartbeats for parent-1
    write_subagent_heartbeat(pid, "sub-a", "parent-1", "sub:Explore", ".");
    write_subagent_heartbeat(pid, "sub-b", "parent-1", "sub:Plan", ".");
    // Create a sub-agent heartbeat for a different parent
    write_subagent_heartbeat(pid, "sub-c", "parent-2", "sub:Bash", ".");

    // Cleanup for parent-1 only
    cleanup_subagent_heartbeats(pid, "parent-1");

    assert!(
        read_heartbeat(pid, "sub-a").is_none(),
        "sub-a should be cleaned up"
    );
    assert!(
        read_heartbeat(pid, "sub-b").is_none(),
        "sub-b should be cleaned up"
    );
    assert!(
        read_heartbeat(pid, "sub-c").is_some(),
        "sub-c belongs to parent-2 and should survive"
    );
    assert!(
        read_heartbeat(pid, "parent-1").is_some(),
        "parent heartbeat should survive"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn heartbeat_backwards_compatible_no_parent() {
    // Heartbeat JSON without parent_session_id should deserialize correctly
    let json = serde_json::json!({
        "session_id": "old-session",
        "started_at": "2026-01-01T00:00:00Z",
        "last_heartbeat": "2026-01-01T00:00:00Z",
        "label": "worker",
        "focus_files": [],
        "active_tasks": [],
        "files_modified_count": 0,
        "total_edits": 0,
        "recent_commits": []
    });
    let hb: SessionHeartbeat =
        serde_json::from_value(json).expect("should deserialize without parent_session_id");
    assert!(
        hb.parent_session_id.is_none(),
        "missing parent_session_id should default to None"
    );
}

#[test]
fn subagent_stale_threshold_extended() {
    let pid = "test_subagent_stale";
    let _ = edda_store::ensure_dirs(pid);

    // Write a sub-agent heartbeat with a last_heartbeat 5 minutes ago
    // (stale for normal sessions at 120s, but within 15x = 30min threshold)
    let five_min_ago = {
        let now = time::OffsetDateTime::now_utc() - time::Duration::seconds(300);
        now.format(&time::format_description::well_known::Rfc3339)
            .unwrap()
    };
    let hb = SessionHeartbeat {
        session_id: "sub-stale".to_string(),
        started_at: five_min_ago.clone(),
        last_heartbeat: five_min_ago,
        label: "sub:Explore".to_string(),
        focus_files: Vec::new(),
        active_tasks: Vec::new(),
        files_modified_count: 0,
        total_edits: 0,
        recent_commits: Vec::new(),
        branch: None,
        current_phase: None,
        parent_session_id: Some("parent-session".to_string()),
    };
    let path = heartbeat_path(pid, "sub-stale");
    let _ = fs::create_dir_all(path.parent().unwrap());
    let _ = fs::write(&path, serde_json::to_string_pretty(&hb).unwrap());

    // Discover peers — sub-agent at 5min old should NOT be stale (threshold is 30min)
    let peers = discover_active_peers(pid, "other-session");
    assert!(
        peers.iter().any(|p| p.session_id == "sub-stale"),
        "sub-agent at 5min should still be active with extended threshold"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// Tests for derive_scope_from_files
mod derive_scope_tests {
    use super::*;

    #[test]
    fn test_top_level_directory_basic() {
        let files = vec![
            FileEditCount {
                path: "server/vault.js".to_string(),
                count: 2,
            },
            FileEditCount {
                path: "server/api.js".to_string(),
                count: 1,
            },
        ];

        let result = derive_scope_from_files(&files);
        assert_eq!(
            result,
            Some(("server".to_string(), vec!["server/*".to_string()]))
        );
    }

    #[test]
    fn test_top_level_directory_mixed() {
        let files = vec![
            FileEditCount {
                path: "app/hooks/useSSE.ts".to_string(),
                count: 3,
            },
            FileEditCount {
                path: "lib/utils.py".to_string(),
                count: 1,
            },
        ];

        let result = derive_scope_from_files(&files);
        // "app" has higher count
        assert_eq!(result, Some(("app".to_string(), vec!["app/*".to_string()])));
    }

    #[test]
    fn test_skip_hidden_directories() {
        let files = vec![
            FileEditCount {
                path: ".github/workflows/ci.yml".to_string(),
                count: 1,
            },
            FileEditCount {
                path: "server/api.js".to_string(),
                count: 2,
            },
        ];

        let result = derive_scope_from_files(&files);
        // Should skip .github and use server
        assert_eq!(
            result,
            Some(("server".to_string(), vec!["server/*".to_string()]))
        );
    }

    #[test]
    fn test_skip_root_level_files() {
        let files = vec![
            FileEditCount {
                path: "README.md".to_string(),
                count: 1,
            },
            FileEditCount {
                path: "Cargo.toml".to_string(),
                count: 1,
            },
        ];

        let result = derive_scope_from_files(&files);
        // Root-level files should not produce a scope
        assert_eq!(result, None);
    }

    #[test]
    fn test_backward_compat_crates() {
        let files = vec![
            FileEditCount {
                path: "crates/edda-store/src/lib.rs".to_string(),
                count: 1,
            },
            FileEditCount {
                path: "crates/edda-bridge/src/main.rs".to_string(),
                count: 2,
            },
        ];

        let result = derive_scope_from_files(&files);
        // Should still use crate-level grouping
        assert_eq!(
            result,
            Some((
                "edda-bridge".to_string(),
                vec!["crates/edda-bridge/*".to_string()]
            ))
        );
    }

    #[test]
    fn test_backward_compat_src() {
        let files = vec![
            FileEditCount {
                path: "src/auth/login.rs".to_string(),
                count: 2,
            },
            FileEditCount {
                path: "src/db/connection.rs".to_string(),
                count: 1,
            },
        ];

        let result = derive_scope_from_files(&files);
        // Should still use src/module grouping
        assert_eq!(
            result,
            Some(("auth".to_string(), vec!["src/auth/*".to_string()]))
        );
    }

    #[test]
    fn test_windows_paths() {
        let files = vec![
            FileEditCount {
                path: "server\\vault.js".to_string(),
                count: 1,
            },
            FileEditCount {
                path: "server\\api.js".to_string(),
                count: 2,
            },
        ];

        let result = derive_scope_from_files(&files);
        // Should normalize backslashes
        assert_eq!(
            result,
            Some(("server".to_string(), vec!["server/*".to_string()]))
        );
    }
}
