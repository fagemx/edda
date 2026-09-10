use super::*;

/// A fleet session (`EDDA_MACHINE` set) owes an explicit label: the chain is
/// env label → claim label → sid prefix, and the branch/auto fallbacks that
/// made three fleet sessions all answer to `main` are gone.
///
/// Identity vars are installed through the GH-757 thread-scoped test
/// configuration, never `std::env::set_var`. libtest runs every `#[test]` as
/// a thread in ONE process, and four pre-existing `write_heartbeat` tests
/// pass `label: None` (`peers/tests.rs:1650`, `:1680`, `:1715`, `:1716`), so
/// they read `env_label()` and `fleet_session()` on exactly the chain these
/// tests drive. A process-wide mutation here flipped two of them to the sid
/// fallback whenever they were scheduled inside the window; a thread-local
/// override is invisible to them, and to every other thread in the binary.
#[test]
fn fleet_session_label_is_explicit_never_the_branch() {
    let _store = crate::isolated_store();
    let pid = "test_gh671_fleet_label";
    let sid = "8-char-minimum-session-id";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let _fleet = crate::test_config_guard(&[
        ("EDDA_MACHINE", Some("gh671-test-machine")),
        ("EDDA_SESSION_LABEL", None),
    ]);

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
    {
        let _env_label =
            crate::test_config_guard(&[("EDDA_SESSION_LABEL", Some("gh671-env-label"))]);
        write_heartbeat(pid, sid, &SessionSignals::default(), None, ".");
        let hb = read_heartbeat(pid, sid).expect("heartbeat written");
        assert_eq!(hb.label, "gh671-env-label");
    }

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

/// The bare local session keeps the permissive chain: no `EDDA_MACHINE`, no
/// edits, so the git branch still carries the identity (#128) and the fleet
/// sid fallback stays out of its way.
#[test]
fn local_session_without_machine_keeps_the_branch_label() {
    let _store = crate::isolated_store();
    let pid = "test_gh671_local_label";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    let _local = crate::test_config_guard(&[("EDDA_MACHINE", None), ("EDDA_SESSION_LABEL", None)]);

    let repo = git_repo_on_branch("gh671-local-branch");
    write_heartbeat(
        pid,
        "local-session-1",
        &SessionSignals::default(),
        None,
        repo.path().to_str().unwrap(),
    );
    let hb = read_heartbeat(pid, "local-session-1").expect("heartbeat written");
    assert_eq!(
        hb.label, "gh671-local-branch",
        "a local session keeps the branch fallback, not the fleet sid prefix"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

/// `machine_identity` resolves `EDDA_MACHINE` first, the OS host name as the
/// display fallback, and refuses to guess past them — display identity, not a
/// credential.
///
/// Same thread-scoped configuration, and here it also removes a save/restore
/// dance: `COMPUTERNAME`/`HOSTNAME` are ambient, so mutating them
/// process-wide handed every concurrent thread in the binary the wrong
/// machine — or none — for the length of this test.
#[test]
fn machine_identity_prefers_edda_machine_then_host_then_nothing() {
    {
        let _cfg = crate::test_config_guard(&[
            ("EDDA_MACHINE", Some("gh671-m")),
            ("COMPUTERNAME", Some("gh671-c")),
        ]);
        assert_eq!(machine_identity().as_deref(), Some("gh671-m"));
    }
    {
        let _cfg =
            crate::test_config_guard(&[("EDDA_MACHINE", None), ("COMPUTERNAME", Some("gh671-c"))]);
        assert_eq!(
            machine_identity().as_deref(),
            Some("gh671-c"),
            "OS host name is the display fallback"
        );
    }
    {
        let _cfg = crate::test_config_guard(&[
            ("EDDA_MACHINE", None),
            ("COMPUTERNAME", None),
            ("HOSTNAME", None),
        ]);
        assert_eq!(machine_identity(), None, "nothing resolves: no guess");
    }
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
