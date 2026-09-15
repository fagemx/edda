use super::*;
use crate::test_support::{isolated_store, write_aged_claim};
use edda_conductor::plan::parser::parse_plan;
use edda_conductor::state::machine::PlanState;
use edda_conductor::state::persist::save_state;

fn fabricated_state(plan: &str, phase: &str, status: PhaseStatus) -> PlanState {
    let yaml = format!("name: {plan}\nphases:\n  - id: {phase}\n    prompt: x\n");
    let parsed = parse_plan(&yaml).unwrap();
    let mut state = PlanState::from_plan(&parsed, "plan.yaml");
    state.phases[0].status = status;
    state.plan_status = PlanStatus::Running;
    state
}

/// Write a conductor-lane heartbeat (the shape the runner stamps), aged by
/// `age_secs`. Mirrors `cmd_conduct::tests::write_lane_heartbeat`.
fn lane_heartbeat(repo: &Path, session: &str, plan: &str, phase: &str, age_secs: u64, pid: u32) {
    let project_id = edda_store::project_id(repo);
    let ts = (OffsetDateTime::now_utc() - time::Duration::seconds(age_secs as i64))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let mut hb = peers::SessionHeartbeat::blank(session);
    hb.started_at = ts.clone();
    hb.last_heartbeat = ts;
    hb.label = plan.to_string();
    hb.plan = Some(plan.to_string());
    hb.phase = Some(phase.to_string());
    hb.attempt = Some(1);
    hb.stage = Some("running".to_string());
    hb.pid = Some(pid);
    edda_store::write_heartbeat(&project_id, &hb).unwrap();
}

fn fleet_notes(repo: &Path, action: &str) -> usize {
    let ledger = Ledger::open_or_init(repo).unwrap();
    ledger
        .iter_events()
        .unwrap()
        .iter()
        .filter(|e| {
            e.payload
                .get(PAYLOAD_KEY)
                .and_then(|fw| fw.get("action"))
                .and_then(|a| a.as_str())
                == Some(action)
        })
        .count()
}

fn fleet_note_total(repo: &Path) -> usize {
    let ledger = Ledger::open_or_init(repo).unwrap();
    ledger
        .iter_events()
        .unwrap()
        .iter()
        .filter(|e| e.payload.get(PAYLOAD_KEY).is_some())
        .count()
}

fn fleet_note_payload(repo: &Path, action: &str) -> Option<serde_json::Value> {
    let ledger = Ledger::open_or_init(repo).unwrap();
    ledger.iter_events().unwrap().into_iter().find_map(|e| {
        e.payload
            .get(PAYLOAD_KEY)
            .filter(|fw| fw.get("action").and_then(|a| a.as_str()) == Some(action))
            .cloned()
    })
}

/// Append a conductor `conductor_phase` note, the shape the runner writes on a
/// terminal phase transition.
fn conductor_note(repo: &Path, plan: &str, phase: &str, status: &str) {
    let ledger = Ledger::open_or_init(repo).unwrap();
    let branch = ledger.head_branch().unwrap();
    let parent = ledger.last_event_hash().unwrap();
    let tags = vec!["conductor".to_string(), format!("phase:{phase}")];
    let mut event = edda_core::event::new_note_event(
        &branch,
        parent.as_deref(),
        "conductor",
        "phase note",
        &tags,
    )
    .unwrap();
    event.payload["conductor_phase"] = serde_json::json!({
        "plan_id": plan,
        "phase_id": phase,
        "status": status,
    });
    edda_core::event::finalize_event(&mut event).unwrap();
    ledger.append_event(&event).unwrap();
}

/// A coordination board that `read_active_claims` refuses to fold (an unknown
/// event type), i.e. "unreadable", which is a different fact from "empty".
fn write_damaged_board(project_id: &str) {
    let _ = edda_store::ensure_dirs(project_id);
    let dir = edda_store::project_dir(project_id).join("state");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("coordination.jsonl"),
        "{\"ts\":\"2026-09-15T00:00:00Z\",\"session_id\":\"x\",\"event_type\":\"bogus\",\"payload\":{}}\n",
    )
    .unwrap();
}

fn repo_dir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    tmp
}

fn git(cwd: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A main checkout plus one git worktree of it, the shape a conductor plan is
/// launched from.
fn repo_with_worktree(root: &Path) -> (PathBuf, PathBuf) {
    let main = root.join("main");
    let wt = root.join("wt");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q"]);
    git(&main, &["config", "user.email", "t@example.invalid"]);
    git(&main, &["config", "user.name", "Test"]);
    std::fs::write(main.join("README"), "seed\n").unwrap();
    git(&main, &["add", "README"]);
    git(&main, &["commit", "-qm", "seed"]);
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            "wtbranch",
        ],
    );
    (main, wt)
}

fn phase_status(repo: &Path, plan: &str, phase: &str) -> PhaseStatus {
    load_state(repo, plan)
        .unwrap()
        .unwrap()
        .phases
        .iter()
        .find(|p| p.id == phase)
        .unwrap()
        .status
}

// ── Pure classification ──────────────────────────────────────────────────────

#[test]
fn verdict_never_infers_death_from_the_heartbeat_alone() {
    use TerminalRecord::*;
    assert_eq!(verdict(false, Unreadable, None), LaneVerdict::Live);
    assert_eq!(verdict(false, Absent, Some(false)), LaneVerdict::Live);
    assert_eq!(verdict(true, Recorded, Some(false)), LaneVerdict::Finished);
    assert_eq!(verdict(true, Absent, Some(true)), LaneVerdict::Claimed);
    assert_eq!(verdict(true, Absent, Some(false)), LaneVerdict::Orphan);
    assert_eq!(
        verdict(true, Unreadable, Some(false)),
        LaneVerdict::Unjudged
    );
    // The product keeps no terminal state for a stateless lane that recorded
    // no claim lifecycle: report it, never recover it (R17).
    assert_eq!(verdict(true, NotKept, Some(false)), LaneVerdict::Unrecorded);
    // An unreadable board never becomes a death verdict, and a standing claim
    // is respected even when the terminal record is unreadable.
    assert_eq!(verdict(true, Absent, None), LaneVerdict::Unjudged);
    assert_eq!(verdict(true, Unreadable, None), LaneVerdict::Unjudged);
    assert_eq!(verdict(true, NotKept, None), LaneVerdict::Unjudged);
    assert_eq!(verdict(true, Unreadable, Some(true)), LaneVerdict::Claimed);
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
    assert_eq!(redispatch["worktree"]["dirty_files"], 0);
    assert_eq!(redispatch["worktree"]["unpushed_commits"], 0);
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
fn watch_treats_a_conductor_note_as_a_terminal_record_whatever_the_outcome() {
    let _store = isolated_store();
    let tmp = repo_dir();
    let repo = tmp.path().join("repo");
    let _ = Ledger::open_or_init(&repo).unwrap();
    lane_heartbeat(
        &repo,
        "lane-note",
        "wave-note",
        "p1",
        peers::stale_secs() * 10,
        999,
    );
    // The conductor writes a `conductor_phase` note only on a terminal
    // transition; `stale` is one of those records, so the work is recorded and
    // this observer must not take it over.
    conductor_note(&repo, "wave-note", "p1", "stale");

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
        .find(|l| l.session_id == "lane-note")
        .expect("lane must be reported");
    assert_eq!(lane.verdict, LaneVerdict::Finished);
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

#[derive(clap::Parser)]
struct FleetTestCli {
    #[command(subcommand)]
    cmd: crate::cmd_fleet::FleetCmd,
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
