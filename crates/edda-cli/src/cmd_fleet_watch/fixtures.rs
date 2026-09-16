//! Fixtures shared by the `cmd_fleet_watch` test modules.

use super::*;
use edda_conductor::plan::parser::parse_plan;
use edda_conductor::state::machine::PlanState;

pub(super) fn fabricated_state(plan: &str, phase: &str, status: PhaseStatus) -> PlanState {
    let yaml = format!("name: {plan}\nphases:\n  - id: {phase}\n    prompt: x\n");
    let parsed = parse_plan(&yaml).unwrap();
    let mut state = PlanState::from_plan(&parsed, "plan.yaml");
    state.phases[0].status = status;
    state.plan_status = PlanStatus::Running;
    state
}

/// Write a conductor-lane heartbeat (the shape the runner stamps), aged by
/// `age_secs`. Mirrors `cmd_conduct::tests::write_lane_heartbeat`.
pub(super) fn lane_heartbeat(
    repo: &Path,
    session: &str,
    plan: &str,
    phase: &str,
    age_secs: u64,
    pid: u32,
) {
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

pub(super) fn fleet_notes(repo: &Path, action: &str) -> usize {
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

pub(super) fn fleet_note_total(repo: &Path) -> usize {
    let ledger = Ledger::open_or_init(repo).unwrap();
    ledger
        .iter_events()
        .unwrap()
        .iter()
        .filter(|e| e.payload.get(PAYLOAD_KEY).is_some())
        .count()
}

pub(super) fn fleet_note_payload(repo: &Path, action: &str) -> Option<serde_json::Value> {
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
pub(super) fn conductor_note(repo: &Path, plan: &str, phase: &str, status: &str) {
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
pub(super) fn write_damaged_board(project_id: &str) {
    let _ = edda_store::ensure_dirs(project_id);
    let dir = edda_store::project_dir(project_id).join("state");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("coordination.jsonl"),
        "{\"ts\":\"2026-09-15T00:00:00Z\",\"session_id\":\"x\",\"event_type\":\"bogus\",\"payload\":{}}\n",
    )
    .unwrap();
}

pub(super) fn repo_dir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    tmp
}

pub(super) fn git(cwd: &Path, args: &[&str]) {
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
pub(super) fn repo_with_worktree(root: &Path) -> (PathBuf, PathBuf) {
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

pub(super) fn phase_status(repo: &Path, plan: &str, phase: &str) -> PhaseStatus {
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

#[derive(clap::Parser)]
pub(super) struct FleetTestCli {
    #[command(subcommand)]
    pub(super) cmd: crate::cmd_fleet::FleetCmd,
}
