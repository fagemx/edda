use super::*;
use clap::Parser;
use edda_conductor::plan::parser::parse_plan;

/// Minimal parser harness: `ConductCmd` is a `Subcommand`, so it needs a
/// root command to be parsed standalone.
#[derive(Parser)]
struct TestCli {
    #[command(subcommand)]
    cmd: ConductCmd,
}

fn parse(args: &[&str]) -> ConductCmd {
    TestCli::try_parse_from(args)
        .expect("args should parse")
        .cmd
}

fn agent_of(cmd: ConductCmd) -> AgentKind {
    match cmd {
        ConductCmd::Run { agent, .. } => agent,
        _ => panic!("expected the Run subcommand"),
    }
}

#[test]
fn run_defaults_to_claude_agent() {
    // Guards the single line keeping every existing `conduct run`
    // invocation on the claude backend.
    assert_eq!(
        agent_of(parse(&["edda", "run", "plan.yaml"])),
        AgentKind::Claude
    );
}

#[test]
fn run_accepts_explicit_agents() {
    assert_eq!(
        agent_of(parse(&["edda", "run", "plan.yaml", "--agent", "pi"])),
        AgentKind::Pi
    );
    assert_eq!(
        agent_of(parse(&["edda", "run", "plan.yaml", "--agent", "claude"])),
        AgentKind::Claude
    );
    assert_eq!(
        agent_of(parse(&["edda", "run", "plan.yaml", "--agent", "codex"])),
        AgentKind::Codex
    );
}

#[test]
fn run_rejects_unknown_agent() {
    let error = match TestCli::try_parse_from(["edda", "run", "plan.yaml", "--agent", "gpt"]) {
        Err(error) => error,
        Ok(_) => panic!("unknown agent must be rejected"),
    };
    let text = error.to_string();
    for expected in ["claude", "pi", "codex"] {
        assert!(
            text.contains(expected),
            "error should list the valid agents, missing {expected:?}: {text}"
        );
    }
}

#[test]
fn budget_warning_fires_for_codex_with_plan_budget() {
    let plan = parse_plan("name: t\nphases:\n  - id: a\n    prompt: x\nbudget_usd: 5.0\n")
        .expect("test plan parses");
    let warning = budget_warning(&plan, AgentKind::Codex).expect("warning expected");
    assert!(warning.contains("codex"), "{warning}");
    assert!(warning.contains("budget_usd"), "{warning}");
    assert!(warning.contains("not be enforced"), "{warning}");
    assert!(warning.contains("cost is unavailable"), "{warning}");
}

#[test]
fn budget_warning_fires_for_codex_with_phase_budget() {
    let plan = parse_plan("name: t\nphases:\n  - id: a\n    prompt: x\n    budget_usd: 1.0\n")
        .expect("test plan parses");
    assert!(budget_warning(&plan, AgentKind::Codex).is_some());
}

#[test]
fn budget_warning_stays_silent_without_a_budget() {
    let plan =
        parse_plan("name: t\nphases:\n  - id: a\n    prompt: x\n").expect("test plan parses");
    assert!(budget_warning(&plan, AgentKind::Codex).is_none());
}

#[test]
fn budget_warning_stays_silent_for_other_agents() {
    let plan = parse_plan("name: t\nphases:\n  - id: a\n    prompt: x\nbudget_usd: 5.0\n")
        .expect("test plan parses");
    assert!(budget_warning(&plan, AgentKind::Claude).is_none());
    assert!(budget_warning(&plan, AgentKind::Pi).is_none());
}

#[test]
fn budget_warning_for_agent_fires_on_codex_with_a_budget() {
    // The flag form shared with `edda dispatch`.
    assert!(budget_warning_for_agent(AgentKind::Codex, true).is_some());
    assert!(budget_warning_for_agent(AgentKind::Codex, false).is_none());
    assert!(budget_warning_for_agent(AgentKind::Claude, true).is_none());
    assert!(budget_warning_for_agent(AgentKind::Pi, true).is_none());
}

#[test]
fn gate_preview_renders_gate_timeout_and_policy() {
    let plan = parse_plan(
            "name: t\nphases:\n  - id: a\n    prompt: x\n    gate: verdict\n    gate_timeout_sec: 3600\n    on_reject: halt\n",
        )
        .expect("test plan parses");
    assert_eq!(
        gate_preview(&plan.phases[0]),
        "  [gate: verdict, timeout: 3600s, on_reject: halt]"
    );
}

#[test]
fn gate_preview_spells_out_the_no_timeout_case() {
    // The footgun for unattended batches must not render as a bare
    // "timeout: -" or silently look bounded.
    let plan = parse_plan("name: t\nphases:\n  - id: a\n    prompt: x\n    gate: verdict\n")
        .expect("test plan parses");
    assert_eq!(
        gate_preview(&plan.phases[0]),
        "  [gate: verdict, timeout: waits until cancelled, on_reject: redispatch]"
    );
}

#[test]
fn gate_preview_is_empty_for_ungated_phases() {
    let plan =
        parse_plan("name: t\nphases:\n  - id: a\n    prompt: x\n").expect("test plan parses");
    assert_eq!(gate_preview(&plan.phases[0]), "");
}

#[test]
fn cost_line_reports_na_when_unmeasured() {
    // GH-533: measured-ness comes from the model, not the zero sentinel.
    assert_eq!(cost_line(0.0, false), "n/a (no usage data reported)");
}

#[test]
fn cost_line_formats_a_measured_total() {
    assert_eq!(cost_line(1.234, true), "$1.23");
}

#[test]
fn cost_line_asserts_a_genuinely_measured_zero() {
    // A backend that reported usage summing to zero measured a real $0.00;
    // the model now distinguishes it from "nobody measured anything".
    assert_eq!(cost_line(0.0, true), "$0.00");
}

/// GH-564 P1-1: `conduct run` builds its notifier through
/// `ChannelNotifier::for_repo`, so a `phase_terminal` channel configured
/// in `.edda/config.json` actually receives terminal events instead of
/// every event being dropped by a bare `StdoutNotifier`.
#[test]
fn run_notifier_delivers_phase_terminal_to_configured_channel() {
    use edda_conductor::runner::notify::Notifier;
    use std::io::Read;
    use std::time::Duration;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".edda")).unwrap();
    std::fs::write(
            dir.path().join(".edda").join("config.json"),
            format!(
                r#"{{"notify_channels":[{{"type":"webhook","url":"http://127.0.0.1:{port}","events":["phase_terminal"]}}]}}"#
            ),
        )
        .unwrap();

    let notifier = ChannelNotifier::for_repo(dir.path());
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        notifier
            .notify_phase_terminal(edda_notify::NotifyEvent::PhaseTerminal {
                plan: "gh564".into(),
                phase: "implement".into(),
                state: "Passed".into(),
                attempt: 1,
                final_output: Some("PR: https://github.com/x/y/pull/620".into()),
            })
            .await;
    });

    // Dispatch finished before notify_phase_terminal returned; the local
    // webhook must have received the event.
    let (mut stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut request = String::new();
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => request.push_str(&String::from_utf8_lossy(&buf[..n])),
        }
        if request.contains("phase_terminal") {
            break;
        }
    }
    assert!(
        request.contains("phase_terminal")
            && request.contains("PR: https://github.com/x/y/pull/620"),
        "configured webhook channel must receive the terminal event, got: {request}"
    );
}

/// GH-751 P1-2: `conduct run` builds its notifier through
/// `ChannelNotifier::for_repo`, so a `gate_progress` channel configured
/// in `.edda/config.json` actually receives progress events instead of
/// being dropped by ChannelNotifier.
#[test]
fn run_notifier_delivers_gate_progress_to_configured_channel() {
    use edda_conductor::runner::notify::Notifier;
    use std::io::Read;
    use std::time::Duration;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".edda")).unwrap();
    std::fs::write(
            dir.path().join(".edda").join("config.json"),
            format!(
                r#"{{"notify_channels":[{{"type":"webhook","url":"http://127.0.0.1:{port}","events":["gate_progress"]}}]}}"#
            ),
        )
        .unwrap();

    let notifier = ChannelNotifier::for_repo(dir.path());
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        notifier
            .notify_gate_progress(edda_notify::NotifyEvent::GateProgress {
                plan: "gh751".into(),
                phase: "review".into(),
                subject: "gh751/review".into(),
                gate_sha: "c".repeat(40),
                wait_label: "9m0s remaining".into(),
            })
            .await;
    });

    // Dispatch finished before notify_gate_progress returned; the local
    // webhook must have received the event.
    let (mut stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut request = String::new();
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => request.push_str(&String::from_utf8_lossy(&buf[..n])),
        }
        if request.contains("gate_progress") {
            break;
        }
    }
    assert!(
        request.contains("gate_progress")
            && request.contains("gh751/review")
            && request.contains("9m0s remaining"),
        "configured webhook channel must receive the gate_progress event, got: {request}"
    );
}

// ── GH-557: one resolution authority for conductor plan state ──

use edda_conductor::state::persist::save_state;

/// Run `git` in `dir`, panicking with git's stderr on failure.
fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git should be runnable");
    assert!(
        out.status.success(),
        "git {:?} in {} failed: {}",
        args,
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A git repo plus a sibling worktree. State fabricated under the worktree
/// mirrors a plan launched from a worktree cwd.
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

fn fabricated_state(plan: &str, phase: &str, status: PhaseStatus) -> PlanState {
    let plan_yaml = format!("name: {plan}\nphases:\n  - id: {phase}\n    prompt: x\n");
    let parsed = parse_plan(&plan_yaml).unwrap();
    let mut state = PlanState::from_plan(&parsed, "plan.yaml");
    state.phases[0].status = status;
    state.plan_status = PlanStatus::Blocked;
    state
}

/// GH-557 regression: a plan whose state lives in a git worktree must be
/// visible to `status`/`retry`/`skip`/`abort` invoked from the main repo.
#[test]
fn recovery_verbs_resolve_state_in_a_git_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let (main, wt) = repo_with_worktree(tmp.path());
    let plan = "wave-b";
    save_state(&wt, &fabricated_state(plan, "p1", PhaseStatus::Stale)).unwrap();

    // status (no --plan) lists the worktree-launched plan and names its store.
    let text = status_impl(&main, None, false).unwrap();
    assert!(
        text.contains(plan),
        "status must list the worktree plan, got: {text}"
    );
    assert!(
        text.contains(&store::normalize_store_path(&wt)),
        "status must name the worktree store, got: {text}"
    );

    // retry acts on the same state `run` would resume.
    retry(&main, "p1", Some(plan)).unwrap();
    assert_eq!(
        load_state(&wt, plan).unwrap().unwrap().phases[0].status,
        PhaseStatus::Pending
    );

    // skip and abort resolve the same store too.
    skip(&main, "p1", Some("test"), Some(plan)).unwrap();
    assert_eq!(
        load_state(&wt, plan).unwrap().unwrap().phases[0].status,
        PhaseStatus::Skipped
    );
    abort(&main, Some(plan)).unwrap();
    assert_eq!(
        load_state(&wt, plan).unwrap().unwrap().plan_status,
        PlanStatus::Aborted
    );
}

/// GH-557: a store reachable by no worktree scan (the plan YAML's own plain
/// directory) is found through the registry `run` records.
#[test]
fn registry_points_recovery_verbs_at_a_plain_launch_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join("main");
    let plain = main.join("plans");
    save_state(
        &plain,
        &fabricated_state("plain-plan", "p1", PhaseStatus::Stale),
    )
    .unwrap();
    store::record_registry(&main, "plain-plan", &plain);

    let text = status_impl(&main, None, false).unwrap();
    assert!(text.contains("plain-plan"), "got: {text}");
    assert_eq!(
        store::resolve_plan_store(&main, "plain-plan").unwrap(),
        Some(plain.clone())
    );
    retry(&main, "p1", Some("plain-plan")).unwrap();
    assert_eq!(
        load_state(&plain, "plain-plan").unwrap().unwrap().phases[0].status,
        PhaseStatus::Pending
    );
}

/// GH-557: the machine-readable status carries the store identity.
#[test]
fn status_json_reports_the_resolved_store() {
    let tmp = tempfile::tempdir().unwrap();
    let (main, wt) = repo_with_worktree(tmp.path());
    save_state(
        &wt,
        &fabricated_state("json-plan", "p1", PhaseStatus::Stale),
    )
    .unwrap();

    let text = status_impl(&main, None, true).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    let row = &parsed.as_array().unwrap()[0];
    assert_eq!(row["plan_name"], "json-plan");
    assert_eq!(row["store"], store::normalize_store_path(&wt));
}

/// GH-557: a genuinely absent plan names the stores that were searched
/// instead of the bare "no state for plan".
#[test]
fn missing_plan_error_names_the_searched_stores() {
    let tmp = tempfile::tempdir().unwrap();
    let (main, wt) = repo_with_worktree(tmp.path());
    save_state(&wt, &fabricated_state("present", "p1", PhaseStatus::Stale)).unwrap();

    let err = retry(&main, "p1", Some("absent")).unwrap_err().to_string();
    assert!(err.contains("no state for plan \"absent\""), "got: {err}");
    // The searched list names the worktree store, not just the invocation root.
    let needle = format!("/{}", wt.file_name().unwrap().to_string_lossy());
    assert!(
        err.contains("searched:") && err.contains(&needle),
        "error must name the searched worktree store, got: {err}"
    );
}

/// GH-557 review round 1 P1: the resume hint must never embed a plan path
/// that is relative to a launch cwd the state does not record.
#[test]
fn resume_hint_does_not_embed_a_cwd_relative_plan_path() {
    let store = Path::new("C:/work/wt");
    assert_eq!(
        resume_hint("plans/x.yaml", store),
        "`edda conduct run <plan.yaml> --cwd C:/work/wt`"
    );
    let abs = if cfg!(windows) {
        "C:/work/plans/x.yaml"
    } else {
        "/work/plans/x.yaml"
    };
    assert_eq!(
        resume_hint(abs, store),
        format!("`edda conduct run {abs} --cwd C:/work/wt`")
    );
    assert_eq!(
        resume_hint("", store),
        "`edda conduct run <plan.yaml> --cwd C:/work/wt`"
    );
}

/// GH-557 verifier report gap 3: a lone corrupt state must not be reported
/// as "No plans found" by auto-detection.
#[test]
fn corrupt_only_state_is_not_reported_as_no_plans() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join("main");
    let dir = main.join(".edda").join("conductor").join("wave-b");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("state.json"), "{ not json").unwrap();

    let err = retry(&main, "p1", None).unwrap_err().to_string();
    assert!(err.contains("unreadable"), "got: {err}");
    assert!(!err.contains("No plans found"), "got: {err}");
}

/// GH-567: write a conductor-lane heartbeat (the shape the runner stamps for
/// both `edda conduct` phases and `edda dispatch` single-turn lanes) into an
/// isolated store. `age_secs` controls the staleness of `last_heartbeat`.
fn write_lane_heartbeat(
    repo: &Path,
    session: &str,
    plan: &str,
    phase: &str,
    age_secs: u64,
    pid: u32,
) {
    let project_id = edda_store::project_id(repo);
    let ts = (time::OffsetDateTime::now_utc() - time::Duration::seconds(age_secs as i64))
        .format(&time::format_description::well_known::Rfc3339)
        .expect("rfc3339");
    let mut hb = edda_bridge_claude::peers::SessionHeartbeat::blank(session);
    hb.started_at = ts.clone();
    hb.last_heartbeat = ts;
    hb.label = plan.to_string();
    hb.plan = Some(plan.to_string());
    hb.phase = Some(phase.to_string());
    hb.attempt = Some(1);
    hb.stage = Some("running".to_string());
    hb.pid = Some(pid);
    edda_store::write_heartbeat(&project_id, &hb).expect("write lane heartbeat");
}

/// GH-567 fail-before: a `edda dispatch` single-turn lane writes the same
/// conductor heartbeat, but `conduct status` never listed it. The unified read
/// model must surface it with the pid it is actually running under.
#[test]
fn status_lists_a_live_dispatch_lane_with_age_and_pid() {
    let _store = crate::test_support::isolated_store();
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    write_lane_heartbeat(&repo, "dispatch-1", "dispatch", "setup", 5, 4242);

    let text = status_impl(&repo, None, false).unwrap();
    assert!(text.contains("dispatch"), "lane must appear, got: {text}");
    assert!(text.contains("setup"), "phase must appear, got: {text}");
    assert!(text.contains("4242"), "pid must appear, got: {text}");
    assert!(text.contains("age="), "age must appear, got: {text}");
    assert!(
        text.contains("s ago"),
        "age must be human-readable, got: {text}"
    );
    assert!(
        !text.contains("stale"),
        "a fresh lane is not stale, got: {text}"
    );
}

/// GH-567: a heartbeat that aged past the shared threshold while the plan
/// still records the phase as running must be *marked* stale — neither hidden
/// nor promoted to a death claim.
#[test]
fn status_marks_a_stale_heartbeat_stale_without_calling_it_dead() {
    let _store = crate::test_support::isolated_store();
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    save_state(
        &repo,
        &fabricated_state("wave-x", "p1", PhaseStatus::Running),
    )
    .unwrap();
    let stale = edda_bridge_claude::peers::stale_secs();
    write_lane_heartbeat(&repo, "lane-stale", "wave-x", "p1", stale * 10, 7777);

    let text = status_impl(&repo, None, false).unwrap();
    assert!(text.contains("wave-x"), "plan must appear, got: {text}");
    assert!(
        text.contains("stale"),
        "an expired lane must be marked stale, got: {text}"
    );
    assert!(
        !text.to_lowercase().contains("dead"),
        "the read model must not declare death, got: {text}"
    );
}

/// GH-567: the machine-readable view carries the same lane facts (age, pid,
/// staleness) so an external panel can retire.
#[test]
fn status_json_includes_lane_entries_with_age_and_pid() {
    let _store = crate::test_support::isolated_store();
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    write_lane_heartbeat(&repo, "dispatch-1", "dispatch", "setup", 5, 4242);

    let text = status_impl(&repo, None, true).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    let rows = parsed.as_array().expect("top level stays an array");
    let lane = rows
        .iter()
        .find(|row| row["kind"] == "lane")
        .expect("a lane row must be present");
    assert_eq!(lane["plan"], "dispatch");
    assert_eq!(lane["phase"], "setup");
    assert_eq!(lane["pid"], 4242);
    assert_eq!(lane["stale"], false);
    assert!(lane["age_secs"].as_u64().unwrap() < 60);
}

/// GH-567 bound: a dispatch lane is in flight only while its heartbeat is
/// live. Once it ages out there is no plan state to keep it in flight, but the
/// issue is explicit that an expired lane is *marked* stale rather than
/// hidden — `discovery.rs`-style silent filtering is exactly the failure mode
/// #567 exists to remove. Reclamation is the separate concern #573.
#[test]
fn status_shows_a_stale_dispatch_lane_marked_stale_not_hidden() {
    let _store = crate::test_support::isolated_store();
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let stale = edda_bridge_claude::peers::stale_secs();
    write_lane_heartbeat(&repo, "dispatch-old", "dispatch", "setup", stale * 10, 111);

    let text = status_impl(&repo, None, false).unwrap();
    assert!(
        text.contains("dispatch/setup"),
        "lane must appear, got: {text}"
    );
    assert!(
        text.contains("stale (no heartbeat for"),
        "expired lane must be marked stale, got: {text}"
    );
    assert!(
        !text.to_lowercase().contains("dead"),
        "the read model must not declare death, got: {text}"
    );
}

/// GH-567: a named-plan `--json` query carries the same lane facts in its
/// additive `lane_heartbeats` field, so the machine-readable contract is
/// proven on both shapes (array and plan object).
#[test]
fn named_plan_json_carries_lane_heartbeats() {
    let _store = crate::test_support::isolated_store();
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    save_state(
        &repo,
        &fabricated_state("wave-x", "p1", PhaseStatus::Running),
    )
    .unwrap();
    write_lane_heartbeat(&repo, "lane-live", "wave-x", "p1", 3, 8888);

    let text = status_impl(&repo, Some("wave-x"), true).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["plan_name"], "wave-x");
    let lanes = parsed["lane_heartbeats"]
        .as_array()
        .expect("lane_heartbeats array");
    assert_eq!(lanes.len(), 1, "got: {text}");
    assert_eq!(lanes[0]["pid"], 8888);
    assert_eq!(lanes[0]["plan"], "wave-x");
    assert_eq!(lanes[0]["stale"], false);
}

/// GH-567 review round 2 P2: an expired lane whose plan records a terminal
/// phase is a finished observation, while one with no terminal state is the
/// issue's suspected-death case. They must not render identically.
#[test]
fn stale_lane_names_a_terminal_phase_when_the_plan_records_one() {
    let _store = crate::test_support::isolated_store();
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    save_state(
        &repo,
        &fabricated_state("done-plan", "p1", PhaseStatus::Passed),
    )
    .unwrap();
    let stale = edda_bridge_claude::peers::stale_secs();
    write_lane_heartbeat(&repo, "finished", "done-plan", "p1", stale * 10, 333);
    write_lane_heartbeat(&repo, "orphan", "dispatch", "setup", stale * 10, 444);

    let text = status_impl(&repo, None, false).unwrap();
    assert!(
        text.contains("stale (phase Passed;"),
        "a finished phase must be named, got: {text}"
    );
    assert!(
        text.contains("stale (no heartbeat for"),
        "a lane with no terminal state keeps the bare stale mark, got: {text}"
    );
}
