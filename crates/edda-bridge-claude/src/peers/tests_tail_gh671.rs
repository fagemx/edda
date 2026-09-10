use super::*;
/// A fleet session (EDDA_MACHINE set) owes an explicit label: the chain is
/// env label → claim label → sid prefix, and the branch/auto fallbacks that
/// made three fleet sessions all answer to `main` are gone. The same fn
/// covers `machine_identity` resolution — every tier runs inside ONE test
/// because they all mutate EDDA_MACHINE, and separate #[test] fns doing
/// that race each other in this binary (measured: a remove_var in one
/// landed between set_var and write_heartbeat in the other, flipping the
/// fleet chain to the branch fallback). Every pre-existing write_heartbeat
/// test passes Some(label), which short-circuits before fleet_session() is
/// ever read, so they cannot see these mutations.
#[test]
fn fleet_session_label_is_explicit_never_the_branch() {
    let _store = crate::isolated_store();
    let pid = "test_gh671_fleet_label";
    let sid = "8-char-minimum-session-id";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    std::env::set_var("EDDA_MACHINE", "gh671-test-machine");
    // No edits yet (auto label empty), cwd inside this git worktree — so the
    // pre-fix chain would have written the branch name. The fleet chain must
    // refuse it and fall back to the sid prefix instead.
    write_heartbeat(pid, sid, &SessionSignals::default(), None, ".");
    let hb = read_heartbeat(pid, sid).expect("heartbeat written");
    assert_eq!(
        hb.label, "sid-8-char-m",
        "a fleet session without an explicit label must not inherit the branch: {:?}",
        hb.label
    );

    // A claim label is explicit identity and wins over the sid fallback.
    write_claim(pid, sid, "gh671-claimant", &["src/x/*".into()]);
    write_heartbeat(pid, sid, &SessionSignals::default(), None, ".");
    let hb = read_heartbeat(pid, sid).expect("heartbeat written");
    assert_eq!(hb.label, "gh671-claimant");

    // EDDA_SESSION_LABEL outranks the claim, matching the non-fleet chain.
    std::env::set_var("EDDA_SESSION_LABEL", "gh671-env-label");
    write_heartbeat(pid, sid, &SessionSignals::default(), None, ".");
    let hb = read_heartbeat(pid, sid).expect("heartbeat written");
    assert_eq!(hb.label, "gh671-env-label");

    std::env::remove_var("EDDA_SESSION_LABEL");
    std::env::remove_var("EDDA_MACHINE");

    // The bare local session keeps the permissive chain: same inputs, no
    // EDDA_MACHINE, and the branch fallback returns.
    write_heartbeat(
        pid,
        "local-session-1",
        &SessionSignals::default(),
        None,
        ".",
    );
    let hb = read_heartbeat(pid, "local-session-1").expect("heartbeat written");
    assert_ne!(hb.label, "sid-local-", "local sessions keep the old chain");

    // machine_identity: EDDA_MACHINE first, OS host name as the display
    // fallback, and no guess past them — display identity, not a credential.
    // The original host vars are saved and restored: they are ambient, and
    // later readers in this binary deserve the machine they started with.
    let real_computername = std::env::var("COMPUTERNAME").ok();
    let real_hostname = std::env::var("HOSTNAME").ok();
    std::env::set_var("EDDA_MACHINE", "gh671-m");
    std::env::set_var("COMPUTERNAME", "gh671-c");
    assert_eq!(machine_identity().as_deref(), Some("gh671-m"));
    std::env::remove_var("EDDA_MACHINE");
    assert_eq!(
        machine_identity().as_deref(),
        Some("gh671-c"),
        "OS host name is the display fallback"
    );
    std::env::remove_var("COMPUTERNAME");
    std::env::remove_var("HOSTNAME");
    assert_eq!(machine_identity(), None, "nothing resolves: no guess");
    if let Some(name) = real_computername {
        std::env::set_var("COMPUTERNAME", name);
    }
    if let Some(name) = real_hostname {
        std::env::set_var("HOSTNAME", name);
    }

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

/// Only a label shared by two or more LIVE sessions collides: dead
/// heartbeats occupy nothing, empty labels are not addressable.
#[test]
fn colliding_labels_counts_live_sessions_only() {
    let peer = |id: &str, label: &str, live: bool| PeerSummary {
        session_id: id.into(),
        label: label.into(),
        age_secs: 10,
        is_live: live,
        last_heartbeat: now_rfc3339(),
        focus_files: vec![],
        task_subjects: vec![],
        files_modified_count: 0,
        recent_commits: vec![],
        claimed_paths: vec![],
        claimed_subject: None,
        branch: None,
        current_phase: None,
    };
    let peers = vec![
        peer("s1", "main", true),
        peer("s2", "main", true),
        peer("s3", "main", false), // dead: does not collide
        peer("s4", "", true),      // unaddressable: never reported
        peer("s5", "unique", true),
    ];
    assert_eq!(
        colliding_labels(&peers),
        vec![("main".to_string(), 2)],
        "one live collision, counted live-only"
    );

    let distinct = vec![peer("s1", "auth", true), peer("s2", "billing", true)];
    assert!(
        colliding_labels(&distinct).is_empty(),
        "distinct labels do not collide"
    );
}

/// The pack's peer block warns when a label stops being an address.
#[test]
fn peer_block_warns_on_shared_label() {
    let _store = crate::isolated_store();
    let pid = "test_gh671_pack_collision";
    let sid = "me";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let peer = |id: &str, label: &str| PeerSummary {
        session_id: id.into(),
        label: label.into(),
        age_secs: 10,
        is_live: true,
        last_heartbeat: now_rfc3339(),
        focus_files: vec![],
        task_subjects: vec![],
        files_modified_count: 0,
        recent_commits: vec![],
        claimed_paths: vec![],
        claimed_subject: None,
        branch: None,
        current_phase: None,
    };
    let board = BoardState::default();

    let colliding = vec![peer("p1", "main"), peer("p2", "main")];
    let out = render_peer_updates_with(&colliding, &board, pid, sid).unwrap();
    assert!(
        out.contains("shared by 2 live sessions"),
        "the ambiguous address must be named: {out}"
    );
    assert!(
        out.contains("label \"main\""),
        "the colliding label must be named: {out}"
    );

    let distinct = vec![peer("p1", "auth"), peer("p2", "billing")];
    let out = render_peer_updates_with(&distinct, &board, pid, sid).unwrap();
    assert!(
        !out.contains("shared by"),
        "no collision, no warning: {out}"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}
