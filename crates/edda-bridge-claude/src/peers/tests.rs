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

    write_heartbeat(pid, sid, &signals, Some("auth"), ".");
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
    write_heartbeat(pid, "self-session", &signals, Some("self"), ".");
    write_heartbeat(pid, "peer-session", &signals, Some("peer"), ".");

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
    write_heartbeat(pid, "s1", &signals, Some("auth"), ".");
    write_heartbeat(pid, "s2", &signals, Some("billing"), ".");
    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
    write_claim(pid, "s2", "billing", &["src/billing/*".into()]);
    write_binding(pid, "s1", "auth", "auth.method", "JWT RS256");

    let result = render_coordination_protocol(pid, "s2", ".").unwrap();
    assert!(result.contains("Coordination Protocol"));
    assert!(result.contains("Off-limits"));
    assert!(result.contains("auth"));
    assert!(result.contains("Recorded Decisions (coordination)"));
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
    assert_eq!(auto_label(&signals, None), "edda-bridge-claude");
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
    assert_eq!(auto_label(&signals, None), "auth");
}

#[test]
fn auto_label_absolute_windows_path_uses_relative_parent() {
    let signals = SessionSignals {
        files_modified: vec![FileEditCount {
            path: r"C:\repo\docs\product\mission-runtime-control.json".into(),
            count: 9,
        }],
        ..Default::default()
    };
    assert_eq!(auto_label(&signals, Some(r"C:\repo")), "product");
}

#[test]
fn auto_label_never_returns_drive_letter() {
    let signals = SessionSignals {
        files_modified: vec![FileEditCount {
            path: r"C:\stray.md".into(),
            count: 1,
        }],
        ..Default::default()
    };
    // Parent segment would be "C:" — must be rejected.
    assert_eq!(auto_label(&signals, None), "");
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
    write_heartbeat(pid, "s1", &signals, Some("auth"), ".");
    write_heartbeat(pid, "s2", &signals, Some("billing"), ".");
    write_heartbeat(pid, "s3", &signals, Some("api"), ".");
    write_heartbeat(pid, "s4", &signals, Some("frontend"), ".");

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

    // s2 sees peer updates (lightweight)
    let updates = render_peer_updates(pid, "s2").unwrap();
    assert!(updates.contains("Peers"));
    assert!(updates.contains("Export BillingPlan"));

    // Verify s2 sees the request at SessionStart. Rendering does not ack it
    // (GH-442) — the request survives until s2 explicitly acknowledges.
    let proto_s2 = render_coordination_protocol(pid, "s2", ".").unwrap();
    assert!(proto_s2.contains("Export BillingPlan type"));
    assert!(
        render_peer_updates(pid, "s2")
            .unwrap()
            .contains("Export BillingPlan"),
        "rendering is delivery, not acknowledgement — request must still show"
    );

    // After an explicit ack, peer updates should no longer show the request
    write_request_ack(pid, "s2", "api");
    let updates_after = render_peer_updates(pid, "s2").unwrap();
    assert!(
        !updates_after.contains("Export BillingPlan"),
        "acked request should be filtered from subsequent peer updates"
    );

    // Solo session should still see bindings (but not peer sections)
    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    remove_heartbeat(pid, "s3");
    remove_heartbeat(pid, "s4");
    let solo = render_coordination_protocol(pid, "s5", ".").unwrap();
    assert!(
        solo.contains("Recorded Decisions (coordination)"),
        "solo should show recorded decisions"
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
    write_heartbeat(pid, "s1", &signals_with_task, Some("auth"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"), ".");

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
    write_heartbeat(pid, "s1", &signals, Some("auth"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"), ".");

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
    write_heartbeat(pid, "s1", &signals, Some("billing"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"), ".");

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
    write_heartbeat(pid, "s1", &signals, Some("billing"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"), ".");

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
    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("billing"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"), ".");

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

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("billing"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"), ".");

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
        result.contains("Recorded Decisions (coordination)"),
        "should have recorded-decisions header, got:\n{result}"
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

    write_heartbeat(
        pid,
        "sess-abc",
        &SessionSignals::default(),
        Some("auth"),
        ".",
    );

    let result = infer_session_id(pid);
    assert_eq!(result, Some(("sess-abc".into(), "auth".into())));

    remove_heartbeat(pid, "sess-abc");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn infer_session_two_active_is_ambiguous() {
    let pid = "test_infer_two";
    let _ = edda_store::ensure_dirs(pid);

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"), ".");

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
    write_heartbeat(
        pid,
        "fresh",
        &SessionSignals::default(),
        Some("frontend"),
        ".",
    );

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
    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"), ".");

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
    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"), ".");

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
    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"), ".");

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
    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("peer"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"), ".");
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
    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"), ".");
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

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("peer"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"), ".");

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

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("peer"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"), ".");
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

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"), ".");

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

    // Solo with a recorded decision (renders "## Recorded Decisions" only)
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

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"), ".");
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
    let (label, paths) = derive_scope_from_files(&files, None).unwrap();
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
    let (label, paths) = derive_scope_from_files(&files, None).unwrap();
    assert_eq!(label, "auth");
    assert_eq!(paths, vec!["src/auth/*"]);
}

#[test]
fn derive_scope_empty_files() {
    assert!(derive_scope_from_files(&[], None).is_none());
}

// ── Absolute-path regression tests (Windows hook payloads) ──
// Hook payloads carry absolute paths; before the relativize fix these
// produced garbage like label "C:" with claim "C:/*".

#[test]
fn derive_scope_absolute_windows_path_relativized_by_cwd() {
    let files = vec![
        FileEditCount {
            path: r"C:\ai_project\AI Delivery Foundry\docs\product\mission-runtime-control.json"
                .into(),
            count: 4,
        },
        FileEditCount {
            path: r"C:\ai_project\AI Delivery Foundry\docs\architecture\evidence\pkg.md".into(),
            count: 1,
        },
    ];
    let cwd = r"C:\ai_project\AI Delivery Foundry";
    let (label, paths) = derive_scope_from_files(&files, Some(cwd)).unwrap();
    assert_eq!(label, "docs");
    assert_eq!(paths, vec!["docs/*"]);
}

#[test]
fn derive_scope_absolute_path_outside_cwd_never_yields_drive_letter() {
    let files = vec![FileEditCount {
        path: r"D:\elsewhere\notes\todo.md".into(),
        count: 3,
    }];
    let cwd = r"C:\ai_project\AI Delivery Foundry";
    // Not under cwd and no crates/src pattern → no claim at all, never "D:".
    assert!(derive_scope_from_files(&files, Some(cwd)).is_none());
}

#[test]
fn derive_scope_absolute_path_without_cwd_never_yields_drive_letter() {
    let files = vec![FileEditCount {
        path: r"C:\ai_project\repo\docs\a.md".into(),
        count: 2,
    }];
    assert!(derive_scope_from_files(&files, None).is_none());
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

    maybe_auto_claim(pid, "s1", &signals, ".");

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

    maybe_auto_claim(pid, "s1", &signals, ".");

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

    maybe_auto_claim(pid, "s1", &signals, ".");
    maybe_auto_claim(pid, "s1", &signals, ".");

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
    maybe_auto_claim(pid, "s1", &signals1, ".");

    let signals2 = SessionSignals {
        files_modified: vec![FileEditCount {
            path: "crates/edda-bridge-claude/src/peers.rs".into(),
            count: 10,
        }],
        ..Default::default()
    };
    maybe_auto_claim(pid, "s1", &signals2, ".");

    let board = compute_board_state(pid);
    let claim = board.claims.iter().find(|c| c.session_id == "s1").unwrap();
    assert_eq!(
        claim.label, "edda-bridge-claude",
        "claim should update to new scope"
    );

    remove_autoclaim_state(pid, "s1");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── #444: the branch fallback is a presence signal, not a scope claim ──

/// Create a git repo in a temp dir sitting on `branch` with one commit,
/// so `detect_git_branch_in` can resolve HEAD.
fn git_repo_on_branch(branch: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    for args in [
        vec!["init"],
        vec!["config", "user.email", "test@test.com"],
        vec!["config", "user.name", "Test"],
        vec!["commit", "--allow-empty", "-m", "init"],
        vec!["checkout", "-b", branch],
    ] {
        let _ = std::process::Command::new("git")
            .args(&args)
            .current_dir(tmp.path())
            .output();
    }
    tmp
}

#[test]
fn auto_claim_writes_no_claim_for_a_fresh_session() {
    let pid = "test_autoclaim_branch_fallback";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let repo = git_repo_on_branch("fallback-branch");
    // Fresh session: no file edits yet, so there is no scope to claim. This used
    // to claim `(branch, ["**/*"])`, which blocked every peer under enforcement.
    maybe_auto_claim(
        pid,
        "s1",
        &SessionSignals::default(),
        repo.path().to_str().unwrap(),
    );

    let board = compute_board_state(pid);
    assert!(
        board.claims.is_empty(),
        "a session that edited nothing must claim nothing, got {:?}",
        board.claims
    );

    remove_autoclaim_state(pid, "s1");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn auto_claim_starts_claiming_once_files_are_edited() {
    let pid = "test_autoclaim_fallback_upgrade";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let repo = git_repo_on_branch("fallback-upgrade");
    let cwd = repo.path().to_str().unwrap();

    maybe_auto_claim(pid, "s1", &SessionSignals::default(), cwd);

    let signals = SessionSignals {
        files_modified: vec![FileEditCount {
            path: "crates/edda-store/src/lib.rs".into(),
            count: 3,
        }],
        ..Default::default()
    };
    maybe_auto_claim(pid, "s1", &signals, cwd);

    let board = compute_board_state(pid);
    let claim = board.claims.iter().find(|c| c.session_id == "s1").unwrap();
    assert_eq!(claim.label, "edda-store");
    assert_eq!(claim.paths, vec!["crates/edda-store/*"]);

    remove_autoclaim_state(pid, "s1");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn heartbeat_label_falls_back_to_git_branch_for_a_fresh_session() {
    let pid = "test_hb_branch_label";
    let _ = edda_store::ensure_dirs(pid);

    let repo = git_repo_on_branch("presence-branch");
    // No edits → `auto_label` is empty, so the branch carries the identity.
    write_heartbeat(
        pid,
        "s1",
        &SessionSignals::default(),
        None,
        repo.path().to_str().unwrap(),
    );

    let hb = read_heartbeat(pid, "s1").expect("heartbeat written");
    assert_eq!(
        hb.label, "presence-branch",
        "fresh session must stay identifiable in `edda watch` without a claim"
    );

    remove_heartbeat(pid, "s1");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn fresh_session_receives_requests_addressed_to_its_branch() {
    let pid = "test_hb_branch_request";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let repo = git_repo_on_branch("request-branch");
    let cwd = repo.path().to_str().unwrap();

    // Peer, and a fresh session that has claimed nothing.
    write_heartbeat(pid, "s-peer", &SessionSignals::default(), Some("auth"), ".");
    write_heartbeat(pid, "s-fresh", &SessionSignals::default(), None, cwd);
    maybe_auto_claim(pid, "s-fresh", &SessionSignals::default(), cwd);

    write_request(
        pid,
        "s-peer",
        "auth",
        "request-branch",
        "rebase before you push",
    );

    let pending = pending_requests_for_session(pid, "s-fresh");
    assert_eq!(
        pending.len(),
        1,
        "branch-addressed request must reach a session with no claim, got {pending:?}"
    );
    assert_eq!(pending[0].message, "rebase before you push");

    remove_heartbeat(pid, "s-peer");
    remove_heartbeat(pid, "s-fresh");
    remove_autoclaim_state(pid, "s-fresh");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn two_fresh_sessions_on_one_branch_do_not_block_each_other() {
    let pid = "test_two_fresh_no_block";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let repo = git_repo_on_branch("shared-branch");
    let cwd = repo.path().to_str().unwrap();

    write_heartbeat(pid, "s1", &SessionSignals::default(), None, cwd);
    write_heartbeat(pid, "s2", &SessionSignals::default(), None, cwd);
    maybe_auto_claim(pid, "s1", &SessionSignals::default(), cwd);
    maybe_auto_claim(pid, "s2", &SessionSignals::default(), cwd);

    // Both are visible to each other, and neither has claimed any path — the
    // mutual deadlock in #444 came from both claiming `**/*`.
    assert_eq!(discover_active_peers(pid, "s1").len(), 1);
    assert!(compute_board_state(pid).claims.is_empty());

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    remove_autoclaim_state(pid, "s1");
    remove_autoclaim_state(pid, "s2");
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
    maybe_auto_claim(pid, "s1", &signals, ".");

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

    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"), ".");

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

    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"), ".");

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
    write_heartbeat(pid, "s1", &signals, Some("auth"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"), ".");
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
    write_heartbeat(pid, "s1", &signals, Some("billing"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"), ".");
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
    write_heartbeat(pid, "s1", &signals, Some("peer-agent"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("my-agent"), ".");

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

    write_heartbeat(
        pid,
        "s1",
        &SessionSignals::default(),
        Some("peer-agent"),
        ".",
    );
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("my-agent"), ".");
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
    write_heartbeat(
        pid,
        "s1",
        &SessionSignals::default(),
        Some("peer-agent"),
        ".",
    );

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

    // The same peer sends a second message after the first was acked. A single
    // request/ack pair cannot tell per-label from per-message matching, so the
    // round-trip below has to carry two.
    write_request(pid, "s2", "billing", "auth", "Export InvoiceTotal");

    // Compact
    let lines = compute_board_state_for_compaction(pid);
    assert_eq!(
        lines.len(),
        4,
        "claim + 2 requests + ack = 4 lines, got: {lines:?}"
    );

    // Write compacted back
    let path = coordination_path(pid);
    let content = lines.join("\n");
    fs::write(&path, format!("{content}\n")).unwrap();

    // After compaction: ack should still exist, and the unacked set must be
    // unchanged. Compaction re-serializes from board state, so an identity
    // field missing there is silently stripped and this bug comes back.
    let board_after = compute_board_state(pid);
    assert_eq!(board_after.request_acks.len(), 1);
    let pending_after = pending_requests_for_session(pid, "s1");
    assert_eq!(
        pending_after.len(),
        1,
        "compaction must preserve which message was acked, got: {pending_after:?}"
    );
    assert_eq!(
        pending_after[0].message, "Export InvoiceTotal",
        "the acked message stays acked; the later one stays pending"
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
    write_heartbeat_minimal(pid, "parent-1", "main-session", ".");
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

        let result = derive_scope_from_files(&files, None);
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

        let result = derive_scope_from_files(&files, None);
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

        let result = derive_scope_from_files(&files, None);
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

        let result = derive_scope_from_files(&files, None);
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

        let result = derive_scope_from_files(&files, None);
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

        let result = derive_scope_from_files(&files, None);
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

        let result = derive_scope_from_files(&files, None);
        // Should normalize backslashes
        assert_eq!(
            result,
            Some(("server".to_string(), vec!["server/*".to_string()]))
        );
    }
}

// ── Request ack render filtering tests ──

#[test]
fn render_coordination_filters_acked_requests() {
    let pid = "test_render_coord_ack_filter";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Set up two peers: s1 (auth) and s2 (billing)
    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"), ".");
    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);

    // billing sends a request to auth
    write_request(pid, "s2", "billing", "auth", "Need AuthToken export");

    // Before ack: request should appear in render for s1 (auth)
    let peers = discover_active_peers(pid, "s1");
    let board = compute_board_state(pid);
    let result = render_coordination_protocol_with(&peers, &board, pid, "s1");
    assert!(result.is_some());
    let text = result.unwrap();
    assert!(
        text.contains("Need AuthToken export"),
        "unacked request should appear: {text}"
    );

    // s1 acks the request from billing
    write_request_ack(pid, "s1", "billing");

    // After ack: request should NOT appear for s1
    let board = compute_board_state(pid);
    let result = render_coordination_protocol_with(&peers, &board, pid, "s1");
    assert!(result.is_some());
    let text = result.unwrap();
    assert!(
        !text.contains("Need AuthToken export"),
        "acked request should be filtered out: {text}"
    );

    // The same peer sends a second message. One request/ack pair renders the
    // same either way, so only this second message proves the ack was matched
    // per message rather than per sender.
    write_request(pid, "s2", "billing", "auth", "Also export RefreshToken");
    let board = compute_board_state(pid);
    let text = render_coordination_protocol_with(&peers, &board, pid, "s1").unwrap();
    assert!(
        text.contains("Also export RefreshToken"),
        "a later request from an already-acked peer must still render: {text}"
    );
    assert!(
        !text.contains("Need AuthToken export"),
        "the acked message must stay acked: {text}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn peer_updates_filters_acked_requests() {
    let pid = "test_peer_updates_ack_filter";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"), ".");
    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);

    // billing sends a request to auth
    write_request(pid, "s2", "billing", "auth", "Export BillingPlan");

    // Before ack: request should appear in peer updates for s1
    let result = render_peer_updates(pid, "s1");
    assert!(result.is_some());
    let text = result.unwrap();
    assert!(
        text.contains("Export BillingPlan"),
        "unacked request should appear in peer updates: {text}"
    );

    // s1 acks the request
    write_request_ack(pid, "s1", "billing");

    // After ack: request should NOT appear for s1
    let result = render_peer_updates(pid, "s1");
    assert!(result.is_some());
    let text = result.unwrap();
    assert!(
        !text.contains("Export BillingPlan"),
        "acked request should be filtered from peer updates: {text}"
    );

    // Second message from the same peer — the case a per-label ack swallows.
    write_request(pid, "s2", "billing", "auth", "Export BillingCycle");
    let text = render_peer_updates(pid, "s1").unwrap();
    assert!(
        text.contains("Export BillingCycle"),
        "a later request from an already-acked peer must still surface: {text}"
    );
    assert!(
        !text.contains("Export BillingPlan"),
        "the acked message must stay acked: {text}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── Coordination Diff Tests (#146) ──

#[test]
fn coord_diff_renders_new_events() {
    let pid = "test_coord_diff_new";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Seed offset at 0 (simulate SessionStart with empty file)
    crate::state::write_coord_offset(pid, "my-sess", 0);

    // Write events from a different session
    write_claim(pid, "other-sess", "auth", &["src/auth/*".into()]);
    write_binding(pid, "other-sess", "auth", "db.engine", "sqlite");

    let diff = render_coord_diff(pid, "my-sess");
    assert!(diff.is_some(), "should render new events");
    let text = diff.unwrap();
    assert!(text.contains("[coordination update]"));
    assert!(text.contains("auth"));
    assert!(text.contains("claimed"));
    assert!(text.contains("db.engine=sqlite"));

    // Second call — no new events, should return None
    let diff2 = render_coord_diff(pid, "my-sess");
    assert!(diff2.is_none(), "should return None when no new events");

    let _ = fs::remove_file(coordination_path(pid));
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn coord_diff_filters_own_events() {
    let pid = "test_coord_diff_own";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Seed offset
    crate::state::write_coord_offset(pid, "my-sess", 0);

    // Write event from own session
    write_claim(pid, "my-sess", "auth", &["src/auth/*".into()]);

    let diff = render_coord_diff(pid, "my-sess");
    assert!(diff.is_none(), "own events should be filtered out");

    let _ = fs::remove_file(coordination_path(pid));
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn coord_diff_compaction_guard() {
    let pid = "test_coord_diff_compact";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Write some events and set offset past them
    write_claim(pid, "peer-sess", "api", &["src/api/*".into()]);
    crate::state::write_coord_offset(pid, "my-sess", 99999);

    // File is smaller than offset → compaction guard triggers, reset to 0
    let diff = render_coord_diff(pid, "my-sess");
    assert!(
        diff.is_some(),
        "should render events after compaction reset"
    );
    let text = diff.unwrap();
    assert!(text.contains("api"));

    let _ = fs::remove_file(coordination_path(pid));
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn coord_diff_skips_when_no_offset_file() {
    let pid = "test_coord_diff_no_offset";
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // Write events but do NOT seed offset (simulates no SessionStart)
    write_claim(pid, "peer-sess", "api", &["src/api/*".into()]);

    // First call: no offset file exists → seeds offset and returns None
    let diff = render_coord_diff(pid, "my-sess");
    assert!(diff.is_none(), "should skip when offset file not seeded");

    // Write more events
    write_binding(pid, "peer-sess", "api", "api.style", "REST");

    // Second call: offset file now exists, new events since last seed
    let diff2 = render_coord_diff(pid, "my-sess");
    assert!(diff2.is_some(), "should render new events after seeding");
    let text = diff2.unwrap();
    assert!(text.contains("REST"));

    let _ = fs::remove_file(coordination_path(pid));
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn resolve_teammate_by_label() {
    let pid = "test_resolve_teammate";
    let sid = "teammate-session-123";
    let _ = edda_store::ensure_dirs(pid);

    let signals = SessionSignals::default();
    write_heartbeat(pid, sid, &signals, Some("worker-auth"), ".");

    // Should resolve by label
    let resolved = resolve_teammate_session(pid, "worker-auth");
    assert_eq!(resolved, Some(sid.to_string()));

    // Should resolve by session_id
    let resolved2 = resolve_teammate_session(pid, sid);
    assert_eq!(resolved2, Some(sid.to_string()));

    // Should return None for nonexistent
    let resolved3 = resolve_teammate_session(pid, "nonexistent");
    assert!(resolved3.is_none());

    // Cleanup
    remove_heartbeat(pid, sid);
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn teammate_idle_writes_coord_event_and_updates_phase() {
    let pid = "test_teammate_idle";
    let notifier_sid = "notifier-session";
    let teammate_sid = "teammate-session";
    let _ = edda_store::ensure_dirs(pid);

    // Clean up any existing coordination file
    let _ = fs::remove_file(coordination_path(pid));

    // Setup: create a heartbeat for the teammate
    let signals = SessionSignals::default();
    write_heartbeat(pid, teammate_sid, &signals, Some("worker-auth"), ".");

    // Verify teammate starts without "idle" phase
    let hb_before = read_heartbeat(pid, teammate_sid).expect("heartbeat should exist");
    assert_ne!(hb_before.current_phase.as_deref(), Some("idle"));

    // Update teammate phase to "idle"
    update_teammate_phase(pid, teammate_sid, "idle");

    // Verify heartbeat phase is now "idle"
    let hb_after = read_heartbeat(pid, teammate_sid).expect("heartbeat should exist");
    assert_eq!(hb_after.current_phase.as_deref(), Some("idle"));

    // Write teammate idle event
    write_teammate_idle(pid, notifier_sid, "worker-auth", "team-alpha");

    // Verify coordination.jsonl contains the event
    let coord_content =
        fs::read_to_string(coordination_path(pid)).expect("coord file should exist");
    assert!(
        coord_content.contains("teammate_idle"),
        "should contain teammate_idle event type"
    );
    assert!(
        coord_content.contains("worker-auth"),
        "should contain teammate_name"
    );
    assert!(
        coord_content.contains("team-alpha"),
        "should contain team_name"
    );

    // Board state should not crash on the new event type
    let board = compute_board_state(pid);
    assert!(board.claims.is_empty());

    // Cleanup
    remove_heartbeat(pid, teammate_sid);
    let _ = fs::remove_file(coordination_path(pid));
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

// ── GH-442 / GH-443: per-message ack identity, target validation, TTL ──

/// Append a raw Request event with a caller-chosen timestamp, bypassing
/// `write_request`. Used to age a request past the TTL horizon.
fn append_request_at(pid: &str, ts: &str, from_session: &str, from_label: &str, to_label: &str) {
    let event = CoordEvent {
        ts: ts.to_string(),
        session_id: from_session.to_string(),
        event_type: CoordEventType::Request,
        payload: serde_json::json!({
            "id": format!("req-{ts}"),
            "from_label": from_label,
            "to_label": to_label,
            "message": "aged request",
        }),
    };
    append_coord_event(pid, &event);
}

#[test]
fn second_request_from_same_peer_survives_first_ack() {
    let pid = "test_gh442_second_request";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
    write_request(pid, "s2", "billing", "auth", "request A");
    write_request_ack(pid, "s1", "billing");

    // Same peer sends a second, distinct request after the first was acked.
    write_request(pid, "s2", "billing", "auth", "request B");

    let pending = pending_requests_for_session(pid, "s1");
    assert_eq!(
        pending.len(),
        1,
        "only the unacked second request should be pending, got: {pending:?}"
    );
    assert_eq!(pending[0].message, "request B");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_does_not_auto_ack_requests() {
    let pid = "test_gh442_no_auto_ack";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"), ".");
    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
    write_request(pid, "s2", "billing", "auth", "Need AuthToken export");

    let rendered = render_coordination_protocol(pid, "s1", ".").unwrap_or_default();
    assert!(
        rendered.contains("Need AuthToken export"),
        "request should render: {rendered}"
    );

    // Rendering is delivery, not acknowledgement: the request stays pending
    // until an explicit `edda request-ack`.
    let pending = pending_requests_for_session(pid, "s1");
    assert_eq!(
        pending.len(),
        1,
        "rendering must not auto-ack — request should still be pending"
    );

    write_request_ack(pid, "s1", "billing");
    assert!(
        pending_requests_for_session(pid, "s1").is_empty(),
        "explicit ack should clear the request"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn resolve_request_targets_matches_only_live_labels() {
    let pid = "test_gh443_resolve_targets";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"), ".");
    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);

    assert_eq!(
        resolve_request_targets(pid, "auth"),
        vec!["s1".to_string()],
        "an active session holding the label should resolve"
    );
    assert!(
        resolve_request_targets(pid, "aut").is_empty(),
        "a typo'd label must resolve to nobody"
    );

    remove_heartbeat(pid, "s1");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn resolve_request_targets_reports_ambiguous_labels() {
    let pid = "test_gh443_ambiguous_targets";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("auth"), ".");

    let mut targets = resolve_request_targets(pid, "auth");
    targets.sort();
    assert_eq!(
        targets,
        vec!["s1".to_string(), "s2".to_string()],
        "duplicate labels must both surface so the sender can be warned"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn expired_requests_are_not_pending_and_are_compacted_away() {
    let pid = "test_gh443_request_ttl";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
    append_request_at(pid, "2020-01-01T00:00:00Z", "s2", "ghost", "auth");
    write_request(pid, "s2", "billing", "auth", "fresh request");

    let pending = pending_requests_for_session(pid, "s1");
    assert_eq!(
        pending.len(),
        1,
        "the aged dead letter must not be delivered, got: {pending:?}"
    );
    assert_eq!(pending[0].message, "fresh request");

    let lines = compute_board_state_for_compaction(pid);
    assert!(
        !lines.iter().any(|l| l.contains("aged request")),
        "compaction must drop expired requests instead of preserving them forever"
    );
    assert!(
        lines.iter().any(|l| l.contains("fresh request")),
        "compaction must keep live requests"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_surfaces_expired_requests_as_warning() {
    let pid = "test_gh443_expired_warning";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"), ".");
    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
    append_request_at(pid, "2020-01-01T00:00:00Z", "s2", "ghost", "auth");

    let peers = discover_active_peers(pid, "s1");
    let board = compute_board_state(pid);
    let rendered = render_coordination_protocol_with(&peers, &board, pid, "s1").unwrap_or_default();
    assert!(
        rendered.contains("WARN") && rendered.contains("expired request"),
        "expired unacked requests should be surfaced as a warning, not silently hidden: {rendered}"
    );
    assert!(
        !rendered.contains("aged request"),
        "expired request bodies should not be rendered as live requests: {rendered}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_pathless_claim_says_what_is_missing() {
    let pid = "test_pathless_claim_render";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("main"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"), ".");
    // A presence-only claim: label, no paths (GH-444/445 branch fallback).
    write_claim(pid, "s1", "main", &[]);

    let peers = discover_active_peers(pid, "s1");
    let board = compute_board_state(pid);
    let rendered = render_coordination_protocol_with(&peers, &board, pid, "s1").unwrap_or_default();
    assert!(
        !rendered.contains("**main** ()"),
        "an empty path list must not render as a broken-looking empty scope: {rendered}"
    );
    assert!(
        rendered.contains("no paths claimed yet"),
        "a pathless claim should say what is missing: {rendered}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_repo_wide_claim_explains_advisory_enforcement() {
    let pid = "test_repo_wide_claim_render";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("main"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"), ".");
    write_claim(pid, "s1", "main", &["**/*".into()]);

    let rendered = render_coordination_protocol(pid, "s1", ".").unwrap_or_default();
    assert!(
        rendered.contains("repo-wide claims are advisory"),
        "repo-wide claim should explain its enforcement status: {rendered}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn render_protocol_teaches_host_doorbell() {
    let pid = "test_host_doorbell_render";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_heartbeat(pid, "s1", &SessionSignals::default(), Some("auth"), ".");
    write_heartbeat(pid, "s2", &SessionSignals::default(), Some("billing"), ".");

    let rendered = render_coordination_protocol(pid, "s2", ".").unwrap_or_default();
    assert!(
        rendered.contains("host's cross-session messaging"),
        "coordination protocol should explain the host wake path: {rendered}"
    );

    remove_heartbeat(pid, "s1");
    remove_heartbeat(pid, "s2");
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

/// A timestamp `secs` in the past, in the same format `now_rfc3339` produces.
/// Lets a test place events at known distances without touching the clock or
/// any env var.
fn ts_secs_ago(secs: i64) -> String {
    (time::OffsetDateTime::now_utc() - time::Duration::seconds(secs))
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting")
}

/// Append a Request exactly as pre-GH-442 edda wrote it: no `id` field.
fn append_legacy_request(pid: &str, ts: &str, from_label: &str, to_label: &str, message: &str) {
    let event = CoordEvent {
        ts: ts.to_string(),
        session_id: "s2".to_string(),
        event_type: CoordEventType::Request,
        payload: serde_json::json!({
            "from_label": from_label,
            "to_label": to_label,
            "message": message,
        }),
    };
    append_coord_event(pid, &event);
}

/// Append a RequestAck exactly as pre-GH-442 edda wrote it: `from_label` only.
fn append_legacy_ack(pid: &str, ts: &str, acker_session: &str, from_label: &str) {
    let event = CoordEvent {
        ts: ts.to_string(),
        session_id: acker_session.to_string(),
        event_type: CoordEventType::RequestAck,
        payload: serde_json::json!({ "from_label": from_label }),
    };
    append_coord_event(pid, &event);
}

#[test]
fn legacy_ackless_id_log_survives_compaction_without_swallowing_later_requests() {
    let pid = "test_gh442_legacy_compaction";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    // A log written entirely by the pre-fix binary: no request ids, no
    // request_ids on the ack. Only the timestamps separate the messages.
    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
    append_legacy_request(pid, &ts_secs_ago(30), "billing", "auth", "legacy request A");
    append_legacy_ack(pid, &ts_secs_ago(20), "s1", "billing");
    append_legacy_request(pid, &ts_secs_ago(10), "billing", "auth", "legacy request B");

    let expect_only_b = |stage: &str| {
        let pending = pending_requests_for_session(pid, "s1");
        assert_eq!(
            pending.len(),
            1,
            "{stage}: the ack predates B, so only A is retired, got: {pending:?}"
        );
        assert_eq!(pending[0].message, "legacy request B", "{stage}");
    };
    expect_only_b("before compaction");

    // Compaction rewrites every event through the current serializer. A legacy
    // ack comes back with an empty `request_ids`, which must keep it on the
    // timestamp-bounded path rather than promoting it to "acks nothing" or
    // demoting it to "acks everything from this peer".
    let lines = compute_board_state_for_compaction(pid);
    fs::write(coordination_path(pid), format!("{}\n", lines.join("\n"))).unwrap();
    expect_only_b("after compaction");

    let board = compute_board_state(pid);
    assert_eq!(board.request_acks.len(), 1, "the ack survives compaction");
    assert!(
        board.request_acks[0].request_ids.is_none(),
        "a legacy ack must not gain fabricated ids during compaction"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn explicit_empty_ack_ids_retire_nothing() {
    let pid = "test_gh454_empty_ack_ids";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
    let request_ts = ts_secs_ago(1);
    append_legacy_request(
        pid,
        &request_ts,
        "billing",
        "auth",
        "request with explicit empty ack",
    );
    let ack_ts = ts_secs_ago(0);
    append_coord_event(
        pid,
        &CoordEvent {
            ts: ack_ts,
            session_id: "s1".into(),
            event_type: CoordEventType::RequestAck,
            payload: serde_json::json!({"from_label": "billing", "request_ids": []}),
        },
    );

    let pending = pending_requests_for_session(pid, "s1");
    assert_eq!(
        pending.len(),
        1,
        "an explicit empty id list must ack nothing"
    );
    assert_eq!(pending[0].message, "request with explicit empty ack");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn legacy_ack_comparison_keeps_subsecond_order() {
    let pid = "test_gh454_subsecond_ack";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_claim(pid, "s1", "auth", &["src/auth/*".into()]);
    let now = time::OffsetDateTime::now_utc();
    let request_ts = now
        .replace_nanosecond(900_000_000)
        .unwrap()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let ack_ts = now
        .replace_nanosecond(100_000_000)
        .unwrap()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    append_legacy_request(pid, &request_ts, "billing", "auth", "later request");
    append_legacy_ack(pid, &ack_ts, "s1", "billing");

    let pending = pending_requests_for_session(pid, "s1");
    assert_eq!(
        pending.len(),
        1,
        "an earlier same-second ack must not retire a later request"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}
