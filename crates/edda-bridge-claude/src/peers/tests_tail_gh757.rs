use super::*;

#[test]
fn explicit_empty_ack_ids_retire_nothing() {
    let _store = crate::isolated_store();
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
    let _store = crate::isolated_store();
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

/// GH-569/GH-566: a lane that fires no bridge hooks (e.g. `edda dispatch
/// --agent pi`) must still become a discoverable peer. The conductor runner
/// writes the shared session heartbeat through the store-level writer
/// (`edda-store`), NOT through any bridge API — that decoupling is the fix.
/// Before it, the only production writer sat inside the Claude hook path, so
/// such a lane never appeared in `edda peers` at all.
#[test]
fn lane_heartbeat_written_without_bridge_is_discovered_then_goes_stale() {
    let _store = crate::isolated_store();
    let pid = "test_lane_hb_discovery";
    let sid = "lane-sess-001";
    let _ = edda_store::ensure_dirs(pid);

    let fmt = |t: time::OffsetDateTime| {
        t.format(&time::format_description::well_known::Rfc3339)
            .unwrap()
    };
    let now = time::OffsetDateTime::now_utc();
    let make = |last: String| edda_store::SessionHeartbeat {
        session_id: sid.into(),
        started_at: fmt(now),
        last_heartbeat: last,
        label: "a".into(),
        focus_files: vec![],
        active_tasks: vec![],
        files_modified_count: 0,
        total_edits: 0,
        recent_commits: vec![],
        branch: None,
        current_phase: Some("running".into()),
        parent_session_id: None,
        plan: Some("hbplan".into()),
        phase: Some("a".into()),
        attempt: Some(1),
        stage: Some("running".into()),
        pid: Some(4242),
    };

    edda_store::write_heartbeat(pid, &make(fmt(now))).expect("lane heartbeat write");

    let peers = discover_active_peers(pid, "observer");
    assert_eq!(
        peers.len(),
        1,
        "a no-hook lane with a fresh heartbeat must be a peer"
    );
    assert_eq!(peers[0].label, "a");
    assert_eq!(peers[0].current_phase.as_deref(), Some("running"));

    // The lane stops: backdate beyond the stale threshold; discovery must
    // drop it without any explicit removal call.
    let stale = fmt(now - time::Duration::new(3600, 0));
    edda_store::write_heartbeat(pid, &make(stale)).expect("stale heartbeat write");
    assert!(
        discover_active_peers(pid, "observer").is_empty(),
        "a stopped lane goes stale naturally"
    );

    remove_heartbeat(pid, sid);
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn claim_with_process_subject_roundtrips_to_board_and_peer_summary() {
    let _store = crate::isolated_store();
    let pid = "test_gh581_subject_roundtrip";
    let sid = "sess-pr-review";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    write_claim_with_subject(
        pid,
        sid,
        "review-pr570",
        &["docs/spec.md".into()],
        Some("pr:570"),
    );

    let board = compute_board_state(pid);
    assert_eq!(board.claims.len(), 1);
    let claim = &board.claims[0];
    assert_eq!(claim.session_id, sid);
    assert_eq!(claim.label, "review-pr570");
    assert_eq!(claim.paths, vec!["docs/spec.md".to_string()]);
    assert_eq!(claim.subject.as_deref(), Some("pr:570"));

    // Write heartbeat so discover_all_sessions finds the session
    write_heartbeat_minimal(pid, sid, "review-pr570", "/path/to/repo");
    let peers = discover_all_sessions(pid);
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].claimed_subject.as_deref(), Some("pr:570"));
    assert_eq!(peers[0].claimed_paths, vec!["docs/spec.md".to_string()]);

    // GH-581 / Round 1 P1-4: Verify Off-limits rendering contains the claimed subject
    let rendered = render_coordination_protocol(pid, "other-agent", "/path/to/repo")
        .expect("renders protocol");
    assert!(
        rendered.contains("- pr:570, docs/spec.md → Agent review-pr570"),
        "rendered: {rendered}"
    );

    // Also verify subject-only rendering without paths
    write_claim_with_subject(pid, sid, "review-pr570", &[], Some("pr:570"));
    let rendered_sub_only = render_coordination_protocol(pid, "other-agent", "/path/to/repo")
        .expect("renders protocol");
    assert!(
        rendered_sub_only.contains("- pr:570 → Agent review-pr570"),
        "rendered_sub_only: {rendered_sub_only}"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}
