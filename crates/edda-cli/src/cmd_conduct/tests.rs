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
