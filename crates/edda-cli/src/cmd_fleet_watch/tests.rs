use super::fixtures::*;
use super::*;
use crate::test_support::isolated_store;
use edda_conductor::state::persist::save_state;

#[test]
fn verdict_never_infers_death_from_the_heartbeat_alone() {
    use TerminalRecord::*;
    assert_eq!(verdict(false, Unreadable, false, None), LaneVerdict::Live);
    assert_eq!(
        verdict(false, Absent, false, Some(false)),
        LaneVerdict::Live
    );
    assert_eq!(
        verdict(true, Recorded, false, Some(false)),
        LaneVerdict::Finished
    );
    assert_eq!(
        verdict(true, Absent, false, Some(true)),
        LaneVerdict::Claimed
    );
    assert_eq!(
        verdict(true, Absent, false, Some(false)),
        LaneVerdict::Orphan
    );
    assert_eq!(
        verdict(true, Unreadable, false, Some(false)),
        LaneVerdict::Unjudged
    );
    // The product keeps no terminal state for a stateless lane that recorded
    // no claim lifecycle: report it, never recover it (R17).
    assert_eq!(
        verdict(true, NotKept, false, Some(false)),
        LaneVerdict::Unrecorded
    );
    // A newer attempt owning the phase outranks every other reading.
    assert_eq!(
        verdict(true, Absent, true, Some(false)),
        LaneVerdict::Superseded
    );
    assert_eq!(
        verdict(true, Unreadable, true, None),
        LaneVerdict::Superseded
    );
    assert_eq!(verdict(false, Absent, true, None), LaneVerdict::Live);
    // An unreadable board never becomes a death verdict, and a standing claim
    // is respected even when the terminal record is unreadable.
    assert_eq!(verdict(true, Absent, false, None), LaneVerdict::Unjudged);
    assert_eq!(
        verdict(true, Unreadable, false, None),
        LaneVerdict::Unjudged
    );
    assert_eq!(verdict(true, NotKept, false, None), LaneVerdict::Unjudged);
    assert_eq!(
        verdict(true, Unreadable, false, Some(true)),
        LaneVerdict::Claimed
    );
}

#[test]
fn recovery_order_is_terminal_then_claim_then_redispatch() {
    assert_eq!(
        plan_recovery(0, 1),
        vec![
            RecoveryStep::WriteTerminal,
            RecoveryStep::ReleaseClaim,
            RecoveryStep::Redispatch
        ]
    );
}

#[test]
fn recovery_stops_loss_at_the_cap() {
    assert_eq!(
        plan_recovery(1, 1),
        vec![
            RecoveryStep::WriteTerminal,
            RecoveryStep::ReleaseClaim,
            RecoveryStep::StopLoss
        ]
    );
    assert_eq!(
        plan_recovery(5, 2),
        vec![
            RecoveryStep::WriteTerminal,
            RecoveryStep::ReleaseClaim,
            RecoveryStep::StopLoss
        ]
    );
}

// ── End-to-end recovery ──────────────────────────────────────────────────────

#[test]
fn watch_json_reports_verdicts_and_orphans() {
    let _store = isolated_store();
    let tmp = repo_dir();
    let repo = tmp.path().join("repo");
    let _ = Ledger::open_or_init(&repo).unwrap();
    save_state(
        &repo,
        &fabricated_state("live-plan", "p1", PhaseStatus::Running),
    )
    .unwrap();
    save_state(
        &repo,
        &fabricated_state("done-plan", "p1", PhaseStatus::Passed),
    )
    .unwrap();
    save_state(
        &repo,
        &fabricated_state("orphan-plan", "p1", PhaseStatus::Running),
    )
    .unwrap();
    lane_heartbeat(&repo, "lane-live", "live-plan", "p1", 3, 111);
    lane_heartbeat(
        &repo,
        "lane-finished",
        "done-plan",
        "p1",
        peers::stale_secs() * 10,
        222,
    );
    lane_heartbeat(
        &repo,
        "lane-orphan",
        "orphan-plan",
        "p1",
        peers::stale_secs() * 10,
        333,
    );

    let report = build_report(
        &WatchArgs {
            apply: false,
            json: true,
            max_redispatch: Some(1),
        },
        &repo,
    )
    .unwrap();
    assert_eq!(report.lane_count, 3);
    assert_eq!(report.orphan_count, 1);
    assert!(report.dry_run);
    let by_session = |id: &str| {
        report
            .lanes
            .iter()
            .find(|l| l.session_id == id)
            .unwrap_or_else(|| panic!("{id} missing"))
            .verdict
    };
    assert_eq!(by_session("lane-live"), LaneVerdict::Live);
    assert_eq!(by_session("lane-finished"), LaneVerdict::Finished);
    assert_eq!(by_session("lane-orphan"), LaneVerdict::Orphan);

    // The same report renders as text and JSON without a second code path.
    let text = render_text(&report);
    assert!(text.contains("lane-orphan"), "got: {text}");
    assert!(text.contains("would recover"), "got: {text}");
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["orphan_count"], 1);
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["recovered"][0]["steps"][0], "write_terminal");
}

#[test]
fn watch_cli_flags_parse_into_the_args() {
    use clap::Parser;
    let cli = FleetTestCli::try_parse_from([
        "edda",
        "watch",
        "--apply",
        "--json",
        "--max-redispatch",
        "2",
    ])
    .expect("the watch flags must parse");
    match cli.cmd {
        crate::cmd_fleet::FleetCmd::Watch { args } => {
            assert!(args.apply);
            assert!(args.json);
            assert_eq!(args.max_redispatch, Some(2));
        }
        _ => panic!("expected the Watch subcommand"),
    }

    let cli = FleetTestCli::try_parse_from(["edda", "watch"]).unwrap();
    match cli.cmd {
        crate::cmd_fleet::FleetCmd::Watch { args } => {
            assert!(!args.apply, "dry run is the default");
            assert!(!args.json);
            assert_eq!(args.max_redispatch, None);
        }
        _ => panic!("expected the Watch subcommand"),
    }
}
