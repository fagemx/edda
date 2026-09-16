use super::fixtures::*;
use super::*;
use crate::test_support::{isolated_store, write_aged_claim};
use edda_conductor::state::persist::save_state;

#[test]
fn watch_does_not_recover_a_superseded_attempt() {
    let _store = isolated_store();
    let tmp = repo_dir();
    let repo = tmp.path().join("repo");
    let _ = Ledger::open_or_init(&repo).unwrap();
    // Attempt 2 of the phase is live (the state says so); the stale heartbeat
    // we are looking at is attempt 1's corpse. Re-arming the phase off it would
    // stomp the live attempt and burn its redispatch budget.
    let mut state = fabricated_state("wave-super", "p1", PhaseStatus::Running);
    state.phases[0].attempts = 2;
    save_state(&repo, &state).unwrap();
    lane_heartbeat(
        &repo,
        "lane-super",
        "wave-super",
        "p1",
        peers::stale_secs() * 10,
        111,
    );

    let report = build_report(
        &WatchArgs {
            apply: true,
            json: false,
            max_redispatch: Some(1),
        },
        &repo,
    )
    .unwrap();
    assert_eq!(report.orphan_count, 0);
    assert_eq!(fleet_note_total(&repo), 0);
    let lane = report
        .lanes
        .iter()
        .find(|l| l.session_id == "lane-super")
        .expect("lane must be reported");
    assert_eq!(lane.verdict, LaneVerdict::Superseded);
    assert_eq!(
        phase_status(&repo, "wave-super", "p1"),
        PhaseStatus::Running
    );
}

#[test]
fn watch_recovers_an_orphan_lane_and_is_idempotent() {
    let _store = isolated_store();
    let tmp = repo_dir();
    let repo = tmp.path().join("repo");
    let project_id = edda_store::project_id(&repo);
    let _ = Ledger::open_or_init(&repo).unwrap();
    save_state(
        &repo,
        &fabricated_state("wave-x", "p1", PhaseStatus::Running),
    )
    .unwrap();
    let stale = peers::stale_secs();
    lane_heartbeat(&repo, "lane-dead", "wave-x", "p1", stale * 10, 444);
    write_aged_claim(&project_id, "lane-dead", 10, &["crates/x".to_string()]);

    // Dry run: nothing is written, the claim stays on the board.
    run(
        WatchArgs {
            apply: false,
            json: false,
            max_redispatch: Some(1),
        },
        &repo,
    )
    .unwrap();
    assert_eq!(fleet_notes(&repo, "terminal"), 0);
    assert!(crate::cmd_claim::read_active_claims(&project_id)
        .unwrap()
        .iter()
        .any(|c| c.session_id == "lane-dead"));

    // Apply: terminal record, claim release, bounded redispatch.
    run(
        WatchArgs {
            apply: true,
            json: false,
            max_redispatch: Some(1),
        },
        &repo,
    )
    .unwrap();
    assert_eq!(fleet_notes(&repo, "terminal"), 1);
    assert_eq!(fleet_notes(&repo, "redispatch"), 1);
    // The takeover hand-off carries the worktree state it read (doneWhen 3),
    // captured before the redispatch was recorded.
    let redispatch = fleet_note_payload(&repo, "redispatch").expect("redispatch note");
    assert!(
        redispatch.get("worktree").is_some(),
        "redispatch payload must carry the worktree state, got: {redispatch}"
    );
    assert!(
        redispatch["worktree"].get("dirty_files").is_some(),
        "the worktree read must be recorded (null when unreadable)"
    );
    // This fixture repo is not a git checkout, so the read did not happen and
    // must be recorded as unknown rather than as a clean zero.
    assert!(redispatch["worktree"]["dirty_files"].is_null());
    assert!(redispatch["worktree"]["unpushed_commits"].is_null());
    assert!(
        redispatch["takeover_instruction"]
            .as_str()
            .is_some_and(|text| text.contains("could not be fully read")),
        "an unreadable worktree must not be called clean: {redispatch}"
    );
    assert!(!crate::cmd_claim::read_active_claims(&project_id)
        .unwrap()
        .iter()
        .any(|c| c.session_id == "lane-dead"));
    assert_eq!(phase_status(&repo, "wave-x", "p1"), PhaseStatus::Pending);

    // Second apply with no state change is a no-op: the terminal record makes
    // the lane Finished, so nothing is written again.
    let report = build_report(
        &WatchArgs {
            apply: true,
            json: false,
            max_redispatch: Some(1),
        },
        &repo,
    )
    .unwrap();
    assert_eq!(fleet_notes(&repo, "terminal"), 1);
    assert_eq!(fleet_notes(&repo, "stop_loss"), 0);
    let lane = report
        .lanes
        .iter()
        .find(|l| l.session_id == "lane-dead")
        .expect("lane must be reported");
    assert_eq!(lane.verdict, LaneVerdict::Finished);
    assert_eq!(report.orphan_count, 0);
}

#[test]
fn watch_does_not_recover_when_the_board_is_unreadable() {
    let _store = isolated_store();
    let tmp = repo_dir();
    let repo = tmp.path().join("repo");
    let project_id = edda_store::project_id(&repo);
    let _ = Ledger::open_or_init(&repo).unwrap();
    save_state(
        &repo,
        &fabricated_state("wave-board", "p1", PhaseStatus::Running),
    )
    .unwrap();
    lane_heartbeat(
        &repo,
        "lane-board",
        "wave-board",
        "p1",
        peers::stale_secs() * 10,
        888,
    );
    write_damaged_board(&project_id);

    let report = build_report(
        &WatchArgs {
            apply: true,
            json: false,
            max_redispatch: Some(1),
        },
        &repo,
    )
    .unwrap();
    assert_eq!(fleet_note_total(&repo), 0);
    assert_eq!(
        phase_status(&repo, "wave-board", "p1"),
        PhaseStatus::Running
    );
    let lane = report
        .lanes
        .iter()
        .find(|l| l.session_id == "lane-board")
        .expect("lane must be reported");
    assert_eq!(lane.verdict, LaneVerdict::Unjudged);
    assert_eq!(report.orphan_count, 0);
}

#[test]
fn watch_recovers_a_rerun_attempt_after_an_earlier_failed_note() {
    let _store = isolated_store();
    let tmp = repo_dir();
    let repo = tmp.path().join("repo");
    let _ = Ledger::open_or_init(&repo).unwrap();
    // Attempt 1 failed (the conductor wrote its receipt), the default
    // `on_fail: auto_retry` reset the phase to Pending, attempt 2 ran, and
    // attempt 2 died abnormally. The ledger note from attempt 1 carries no
    // session or attempt identity, so it must NOT mark attempt 2 finished.
    conductor_note(&repo, "wave-rerun", "p1", "failed");
    save_state(
        &repo,
        &fabricated_state("wave-rerun", "p1", PhaseStatus::Running),
    )
    .unwrap();
    lane_heartbeat(
        &repo,
        "lane-rerun",
        "wave-rerun",
        "p1",
        peers::stale_secs() * 10,
        321,
    );

    run(
        WatchArgs {
            apply: true,
            json: false,
            max_redispatch: Some(1),
        },
        &repo,
    )
    .unwrap();
    assert_eq!(fleet_notes(&repo, "terminal"), 1);
    assert_eq!(
        phase_status(&repo, "wave-rerun", "p1"),
        PhaseStatus::Pending
    );
}

#[test]
fn watch_recovers_into_the_store_that_holds_the_worktree_plan_state() {
    let _store = isolated_store();
    let tmp = tempfile::tempdir().unwrap();
    let (main, wt) = repo_with_worktree(tmp.path());
    // The plan's store is an initialized workspace before the plan state is
    // written into it, exactly as the conductor's runner leaves it.
    let _ = Ledger::open_or_init(&main).unwrap();
    let _ = Ledger::open_or_init(&wt).unwrap();
    save_state(
        &wt,
        &fabricated_state("wave-wt", "p1", PhaseStatus::Running),
    )
    .unwrap();
    // Every worktree of a repository shares one project id, so the lane's
    // heartbeat lands in the shared project dir and BOTH stores are recovery
    // candidates — the invocation root comes first in `candidate_stores`.
    assert_eq!(edda_store::project_id(&main), edda_store::project_id(&wt));
    lane_heartbeat(
        &main,
        "lane-wt",
        "wave-wt",
        "p1",
        peers::stale_secs() * 10,
        4321,
    );

    run(
        WatchArgs {
            apply: true,
            json: false,
            max_redispatch: Some(1),
        },
        &main,
    )
    .unwrap();

    // The phase transition and the terminal record must land in the store that
    // owns the plan state, not in whichever candidate store came first: a
    // record in the invocation root would read the lane `finished` forever
    // while the worktree phase stayed Running.
    assert_eq!(fleet_notes(&wt, "terminal"), 1);
    assert_eq!(fleet_notes(&wt, "redispatch"), 1);
    assert_eq!(fleet_note_total(&main), 0);
    assert_eq!(phase_status(&wt, "wave-wt", "p1"), PhaseStatus::Pending);
}

#[test]
fn watch_reports_a_stateless_dispatch_lane_as_unrecorded() {
    let _store = isolated_store();
    let tmp = repo_dir();
    let repo = tmp.path().join("repo");
    let _ = Ledger::open_or_init(&repo).unwrap();
    // A normally finished `edda dispatch` lane without --owns: no plan state,
    // no digest note, no claim lifecycle. The product keeps no terminal record
    // for it, and one proof (heartbeat absence) is not a verdict (R17).
    lane_heartbeat(
        &repo,
        "lane-dispatch",
        "dispatch",
        "setup",
        peers::stale_secs() * 10,
        1234,
    );

    let report = build_report(
        &WatchArgs {
            apply: true,
            json: false,
            max_redispatch: Some(1),
        },
        &repo,
    )
    .unwrap();
    assert_eq!(fleet_note_total(&repo), 0);
    assert_eq!(report.orphan_count, 0);
    let lane = report
        .lanes
        .iter()
        .find(|l| l.session_id == "lane-dispatch")
        .expect("lane must be reported");
    assert_eq!(lane.verdict, LaneVerdict::Unrecorded);
    assert!(lane.detail.is_some(), "the report must say why");
}

#[test]
fn watch_recovers_a_dispatch_lane_whose_claim_was_never_released() {
    let _store = isolated_store();
    let tmp = repo_dir();
    let repo = tmp.path().join("repo");
    let project_id = edda_store::project_id(&repo);
    let _ = Ledger::open_or_init(&repo).unwrap();
    // The 2026-09-01 shape: a dispatch lane that owned paths claimed them and
    // died without releasing, so the claim lifecycle is the record that says
    // the unit was real and never finished.
    lane_heartbeat(
        &repo,
        "lane-owns",
        "dispatch",
        "setup",
        peers::stale_secs() * 10,
        4321,
    );
    write_aged_claim(
        &project_id,
        "lane-owns",
        peers::stale_secs() * 10,
        &["crates/x".to_string()],
    );

    let report = build_report(
        &WatchArgs {
            apply: true,
            json: false,
            max_redispatch: Some(1),
        },
        &repo,
    )
    .unwrap();
    assert_eq!(report.orphan_count, 1);
    assert_eq!(fleet_notes(&repo, "terminal"), 1);
    assert!(!crate::cmd_claim::read_active_claims(&project_id)
        .unwrap()
        .iter()
        .any(|c| c.session_id == "lane-owns"));
}

#[test]
fn watch_treats_a_session_digest_note_as_a_completion_record() {
    let _store = isolated_store();
    let tmp = repo_dir();
    let repo = tmp.path().join("repo");
    let ledger = Ledger::open_or_init(&repo).unwrap();
    let branch = ledger.head_branch().unwrap();
    let parent = ledger.last_event_hash().unwrap();
    let mut event = edda_core::event::new_note_event(
        &branch,
        parent.as_deref(),
        "system",
        "session digest",
        &["session_digest".to_string()],
    )
    .unwrap();
    event.payload["source"] = serde_json::json!("bridge:session_digest");
    event.payload["session_id"] = serde_json::json!("lane-digest");
    edda_core::event::finalize_event(&mut event).unwrap();
    ledger.append_event(&event).unwrap();

    lane_heartbeat(
        &repo,
        "lane-digest",
        "dispatch",
        "setup",
        peers::stale_secs() * 10,
        555,
    );
    let report = build_report(
        &WatchArgs {
            apply: true,
            json: false,
            max_redispatch: Some(1),
        },
        &repo,
    )
    .unwrap();
    assert_eq!(fleet_note_total(&repo), 0);
    let lane = report
        .lanes
        .iter()
        .find(|l| l.session_id == "lane-digest")
        .expect("lane must be reported");
    assert_eq!(lane.verdict, LaneVerdict::Finished);
}

#[test]
fn watch_does_not_re_arm_a_lane_a_live_peer_claims() {
    let _store = isolated_store();
    let tmp = repo_dir();
    let repo = tmp.path().join("repo");
    let project_id = edda_store::project_id(&repo);
    let _ = Ledger::open_or_init(&repo).unwrap();
    save_state(
        &repo,
        &fabricated_state("wave-peer", "p1", PhaseStatus::Running),
    )
    .unwrap();
    lane_heartbeat(
        &repo,
        "lane-peer",
        "wave-peer",
        "p1",
        peers::stale_secs() * 10,
        111,
    );
    // The dead lane owned `crates/x` and never released it.
    write_aged_claim(
        &project_id,
        "lane-peer",
        peers::stale_secs() * 10,
        &["crates/x".to_string()],
    );
    // A live peer holds a LIVE claim on the same surface: fresh heartbeat, so
    // its claim still stands, and `cmd_claim::check` intersects.
    lane_heartbeat(&repo, "peer-live", "dispatch", "setup", 3, 222);
    write_aged_claim(&project_id, "peer-live", 5, &["crates/x".to_string()]);

    let report = build_report(
        &WatchArgs {
            apply: true,
            json: false,
            max_redispatch: Some(1),
        },
        &repo,
    )
    .unwrap();
    let lane = report
        .lanes
        .iter()
        .find(|l| l.session_id == "lane-peer")
        .expect("lane must be reported");
    assert_eq!(lane.verdict, LaneVerdict::Claimed);
    assert_eq!(fleet_note_total(&repo), 0);
    assert_eq!(phase_status(&repo, "wave-peer", "p1"), PhaseStatus::Running);
}

#[test]
fn watch_puts_the_takeover_instruction_into_the_retry_context() {
    let _store = isolated_store();
    let tmp = repo_dir();
    let repo = tmp.path().join("repo");
    let _ = Ledger::open_or_init(&repo).unwrap();
    save_state(
        &repo,
        &fabricated_state("wave-ctx", "p1", PhaseStatus::Running),
    )
    .unwrap();
    lane_heartbeat(
        &repo,
        "lane-ctx",
        "wave-ctx",
        "p1",
        peers::stale_secs() * 10,
        333,
    );

    run(
        WatchArgs {
            apply: true,
            json: false,
            max_redispatch: Some(1),
        },
        &repo,
    )
    .unwrap();

    // `retry_context` is the channel the runner injects into the next phase
    // prompt; the takeover instruction must reach the re-dispatched worker
    // there, not only in a ledger note.
    let state = load_state(&repo, "wave-ctx").unwrap().unwrap();
    let ctx = state.phases[0]
        .retry_context
        .as_deref()
        .expect("retry_context must be set by the redispatch");
    assert!(ctx.contains("do not"), "got: {ctx}");
    assert!(ctx.contains("continue on top of it"), "got: {ctx}");
}

#[test]
fn watch_stop_loss_names_an_already_queued_phase() {
    let _store = isolated_store();
    let tmp = repo_dir();
    let repo = tmp.path().join("repo");
    let _ = Ledger::open_or_init(&repo).unwrap();
    // The phase is already Pending (a previous retry queued it), so there is a
    // plan and a plan file - the stop-loss reason must not claim otherwise.
    save_state(
        &repo,
        &fabricated_state("wave-queued", "p1", PhaseStatus::Pending),
    )
    .unwrap();
    lane_heartbeat(
        &repo,
        "lane-queued",
        "wave-queued",
        "p1",
        peers::stale_secs() * 10,
        321,
    );

    run(
        WatchArgs {
            apply: true,
            json: false,
            max_redispatch: Some(1),
        },
        &repo,
    )
    .unwrap();
    assert_eq!(fleet_notes(&repo, "terminal"), 1);
    assert_eq!(fleet_notes(&repo, "redispatch"), 0);
    let stop_loss = fleet_note_payload(&repo, "stop_loss").expect("stop-loss note");
    let reason = stop_loss["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("already Pending"),
        "the reason must name the real cause, got: {reason}"
    );
    assert!(
        !reason.contains("no recorded plan/brief"),
        "a plan exists; that reason would be false, got: {reason}"
    );
}

#[test]
fn watch_does_not_flag_a_normally_finished_lane() {
    let _store = isolated_store();
    let tmp = repo_dir();
    let repo = tmp.path().join("repo");
    let _ = Ledger::open_or_init(&repo).unwrap();
    save_state(
        &repo,
        &fabricated_state("done-plan", "p1", PhaseStatus::Passed),
    )
    .unwrap();
    lane_heartbeat(
        &repo,
        "finished",
        "done-plan",
        "p1",
        peers::stale_secs() * 10,
        333,
    );

    run(
        WatchArgs {
            apply: true,
            json: false,
            max_redispatch: Some(1),
        },
        &repo,
    )
    .unwrap();
    assert_eq!(fleet_note_total(&repo), 0);
    assert_eq!(phase_status(&repo, "done-plan", "p1"), PhaseStatus::Passed);
}

#[test]
fn watch_does_not_touch_a_live_lane() {
    let _store = isolated_store();
    let tmp = repo_dir();
    let repo = tmp.path().join("repo");
    let _ = Ledger::open_or_init(&repo).unwrap();
    save_state(
        &repo,
        &fabricated_state("wave-y", "p1", PhaseStatus::Running),
    )
    .unwrap();
    lane_heartbeat(&repo, "lane-live", "wave-y", "p1", 3, 4242);

    run(
        WatchArgs {
            apply: true,
            json: false,
            max_redispatch: Some(1),
        },
        &repo,
    )
    .unwrap();
    assert_eq!(fleet_note_total(&repo), 0);
    assert_eq!(phase_status(&repo, "wave-y", "p1"), PhaseStatus::Running);
}

#[test]
fn watch_does_not_take_over_a_legitimately_claimed_lane() {
    let _store = isolated_store();
    let tmp = repo_dir();
    let repo = tmp.path().join("repo");
    let project_id = edda_store::project_id(&repo);
    let _ = Ledger::open_or_init(&repo).unwrap();
    save_state(
        &repo,
        &fabricated_state("wave-z", "p1", PhaseStatus::Running),
    )
    .unwrap();
    lane_heartbeat(
        &repo,
        "cli-claimer",
        "wave-z",
        "p1",
        peers::stale_secs() * 10,
        555,
    );
    // Inside the claim guard's TTL, so the claim still stands even though the
    // session's heartbeat aged out.
    write_aged_claim(&project_id, "cli-claimer", 10, &["crates/x".to_string()]);

    run(
        WatchArgs {
            apply: true,
            json: false,
            max_redispatch: Some(1),
        },
        &repo,
    )
    .unwrap();
    assert_eq!(fleet_note_total(&repo), 0);
    assert_eq!(phase_status(&repo, "wave-z", "p1"), PhaseStatus::Running);
    assert!(crate::cmd_claim::read_active_claims(&project_id)
        .unwrap()
        .iter()
        .any(|c| c.session_id == "cli-claimer"));
}

#[test]
fn watch_stops_loss_past_the_redispatch_cap() {
    let _store = isolated_store();
    let tmp = repo_dir();
    let repo = tmp.path().join("repo");
    let _ = Ledger::open_or_init(&repo).unwrap();
    save_state(
        &repo,
        &fabricated_state("wave-cap", "p1", PhaseStatus::Running),
    )
    .unwrap();
    lane_heartbeat(
        &repo,
        "lane-cap",
        "wave-cap",
        "p1",
        peers::stale_secs() * 10,
        777,
    );

    run(
        WatchArgs {
            apply: true,
            json: false,
            max_redispatch: Some(0),
        },
        &repo,
    )
    .unwrap();
    assert_eq!(fleet_notes(&repo, "terminal"), 1);
    assert_eq!(fleet_notes(&repo, "stop_loss"), 1);
    assert_eq!(fleet_notes(&repo, "redispatch"), 0);
    // The terminal write ran (Running → Stale); no re-arm past the cap.
    assert_eq!(phase_status(&repo, "wave-cap", "p1"), PhaseStatus::Stale);
}

#[test]
fn watch_does_not_flag_a_lane_bound_to_a_finished_rail_task() {
    let _store = isolated_store();
    let tmp = repo_dir();
    let repo = tmp.path().join("repo");
    let ledger = Ledger::open_or_init(&repo).unwrap();
    let created = edda_core::event::new_task_created_event(&edda_core::event::TaskCreatedParams {
        branch: "main",
        parent_hash: None,
        task_id: 9,
        title: "rail work",
        assignee: None,
        agent_kind: None,
        after: &[],
        plan_id: None,
        work_unit_ref: None,
        brief_ref: None,
        idempotency_key: None,
        scope_paths: &[],
    })
    .unwrap();
    ledger.append_event(&created).unwrap();
    let parent = ledger.last_event_hash().unwrap();
    let session =
        edda_core::event::new_task_session_event("main", parent.as_deref(), 9, "rail-lane")
            .unwrap();
    ledger.append_event(&session).unwrap();
    let parent = ledger.last_event_hash().unwrap();
    let done =
        edda_core::event::new_task_done_event("main", parent.as_deref(), 9, "done", &[]).unwrap();
    ledger.append_event(&done).unwrap();

    lane_heartbeat(
        &repo,
        "rail-lane",
        "dispatch",
        "setup",
        peers::stale_secs() * 10,
        666,
    );

    let report = build_report(
        &WatchArgs {
            apply: true,
            json: false,
            max_redispatch: Some(1),
        },
        &repo,
    )
    .unwrap();
    assert_eq!(fleet_note_total(&repo), 0);
    assert_eq!(report.orphan_count, 0);
    let lane = report
        .lanes
        .iter()
        .find(|l| l.session_id == "rail-lane")
        .expect("lane must be reported");
    assert_eq!(lane.verdict, LaneVerdict::Finished);
}

// ── Report shape ─────────────────────────────────────────────────────────────
