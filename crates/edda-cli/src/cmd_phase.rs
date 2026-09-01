use clap::Subcommand;
use edda_bridge_claude::agent_phase;
use edda_conductor::agent::launcher::phase_session_id_attempt;
use edda_conductor::state::machine::PhaseStatus;
use edda_core::agent_phase::{phase_suggestion, AgentPhaseMap};
use edda_core::VerdictDecision;
use std::path::Path;

// ── GH-547: `phase approve|reject` sugar over `edda verdict` ─────────────
//
// Relaying a verdict for a gated phase by hand means typing a 40-hex SHA
// and a session UUID, and a wrong-but-valid value is accepted while the
// gate silently waits forever. The persisted conductor state already knows
// both, so this sugar resolves them and then delegates to the exact same
// `verdict.recorded` write path (validation and secret redaction included).

#[derive(Debug, Subcommand)]
pub enum PhaseCmd {
    /// Approve the live verdict gate on <plan>/<phase>
    ///
    /// gate_sha and session are resolved from the persisted conductor state;
    /// --sha and --session remain available as explicit overrides.
    Approve {
        /// Gated subject as <plan-name>/<phase-id>
        subject: String,
        /// Override: full 40-hex git SHA (default: the SHA captured at gate entry)
        #[arg(long)]
        sha: Option<String>,
        /// Optional context recorded with the approval
        #[arg(long)]
        comment: Option<String>,
        /// Override: session ID (default: the conductor session running the phase)
        #[arg(long)]
        session: Option<String>,
    },
    /// Reject the live verdict gate on <plan>/<phase>
    ///
    /// The comment is mandatory: it is fed back to the gated agent session
    /// as its next redispatch turn.
    Reject {
        /// Gated subject as <plan-name>/<phase-id>
        subject: String,
        /// Required: why the gate was rejected (becomes the redispatch prompt)
        #[arg(long)]
        comment: String,
        /// Override: full 40-hex git SHA (default: the SHA captured at gate entry)
        #[arg(long)]
        sha: Option<String>,
        /// Override: session ID (default: the conductor session running the phase)
        #[arg(long)]
        session: Option<String>,
    },
}

/// A gate resolved from the persisted conductor state (GH-547).
#[derive(Debug)]
pub(crate) struct ResolvedGate {
    pub phase_id: String,
    pub gate_sha: String,
    pub session_id: String,
}

/// Resolve subject/gate_sha/session for a gated phase from conductor state.
///
/// Refuses LOUDLY and immediately when the plan does not exist, the phase
/// does not exist, or the phase is not actually in AWAITING_VERDICT — the
/// whole point is to turn today's silent-eternal-wait failures into instant
/// errors. Resolution reads the same state.json the conductor itself uses;
/// no new notion of "where" is introduced (that is #543's problem, not ours).
pub(crate) fn resolve_gate(repo_root: &Path, subject: &str) -> anyhow::Result<ResolvedGate> {
    let (plan_name, phase_id) = subject.split_once('/').ok_or_else(|| {
        anyhow::anyhow!(
            "subject must be <plan-name>/<phase-id> (got \"{subject}\") — \
                 e.g. edda phase approve my-plan/impl"
        )
    })?;

    let state =
        edda_conductor::state::persist::load_state(repo_root, plan_name)?.ok_or_else(|| {
            anyhow::anyhow!(
                "no conductor state for plan \"{plan_name}\" — expected {}\n  \
                 run `edda conduct status` to list plans that have state",
                edda_conductor::state::persist::state_path(repo_root, plan_name).display()
            )
        })?;

    let phase = state.get_phase(phase_id).map_err(|_| {
        anyhow::anyhow!(
            "plan \"{plan_name}\" has no phase \"{phase_id}\" — phases: {}",
            state
                .phases
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;

    if phase.status != PhaseStatus::AwaitingVerdict {
        anyhow::bail!(
            "phase \"{plan_name}/{phase_id}\" is not awaiting a verdict (status: {:?});\n  \
             refusing instead of recording a verdict the gate would silently wait on forever",
            phase.status
        );
    }

    let gate_sha = phase.gate_sha.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "phase \"{plan_name}/{phase_id}\" is awaiting_verdict but has no gate_sha \
             recorded; pass --sha explicitly"
        )
    })?;

    // Same deterministic session the runner used for this attempt
    // (edda-conductor/src/runner/sequential.rs); phase.attempts >= 1 by the
    // time a phase can be in AWAITING_VERDICT.
    let session_id = phase_session_id_attempt(plan_name, phase_id, phase.attempts).to_string();

    Ok(ResolvedGate {
        phase_id: phase_id.to_string(),
        gate_sha,
        session_id,
    })
}

/// Execute `edda phase approve|reject` — resolve the gate from conductor
/// state, then record through the unchanged `edda verdict` path.
pub fn run_gate_sugar(cmd: PhaseCmd, repo_root: &Path) -> anyhow::Result<()> {
    let (subject, decision, comment, sha_override, session_override) = match cmd {
        PhaseCmd::Approve {
            subject,
            sha,
            comment,
            session,
        } => (subject, VerdictDecision::Approved, comment, sha, session),
        PhaseCmd::Reject {
            subject,
            comment,
            sha,
            session,
        } => (
            subject,
            VerdictDecision::Rejected,
            Some(comment),
            sha,
            session,
        ),
    };

    let gate = resolve_gate(repo_root, &subject)?;
    let sha = sha_override.unwrap_or_else(|| gate.gate_sha.clone());
    let session = session_override.unwrap_or_else(|| gate.session_id.clone());
    println!(
        "Resolved gate for \"{subject}\" from conductor state (phase: {}, status: awaiting_verdict):",
        gate.phase_id
    );
    if sha == gate.gate_sha {
        println!("  gate_sha: {}", gate.gate_sha);
    } else {
        println!(
            "  gate_sha: {sha} (--sha override; state had {})",
            gate.gate_sha
        );
    }
    if session == gate.session_id {
        println!("  session:  {session}");
    } else {
        println!(
            "  session:  {session} (--session override; state had {})",
            gate.session_id
        );
    }

    let outcome = crate::cmd_verdict::do_record(
        repo_root,
        &crate::cmd_verdict::RecordVerdictArgs {
            subject: &subject,
            decision,
            sha: &sha,
            comment: comment.as_deref(),
            cli_session: Some(&session),
        },
    )?;
    println!("Verdict recorded: {} {subject} @ {sha}", outcome.decision);
    println!("  event: {}", outcome.event_id);
    if let Some(c) = comment.as_deref().filter(|c| !c.trim().is_empty()) {
        println!("  comment: {c}");
    }
    Ok(())
}

/// Execute `edda phase` — show current agent phase map.
pub fn execute(repo_root: &Path, json: bool) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let session_id = infer_session_id(&project_id);

    let map = agent_phase::build_phase_map(&project_id, session_id.as_deref().unwrap_or(""));

    if json {
        println!("{}", serde_json::to_string_pretty(&map)?);
        return Ok(());
    }

    print_phase_map(&map, session_id.as_deref());
    Ok(())
}

/// Print human-readable phase map.
fn print_phase_map(map: &AgentPhaseMap, current_session: Option<&str>) {
    if map.agents.is_empty() && map.stale.is_empty() {
        println!("No agent phase data found.");
        println!("Phase detection runs automatically during Claude Code hook dispatch.");
        return;
    }

    println!("Agent Phase Map");
    println!("===============");
    println!();

    if !map.agents.is_empty() {
        println!("Active ({}):", map.agents.len());
        for state in &map.agents {
            let is_me = current_session
                .map(|s| s == state.session_id)
                .unwrap_or(false);
            let marker = if is_me { " (you)" } else { "" };
            let id = state.label.as_deref().unwrap_or(&state.session_id);
            let context = match (state.issue, state.pr) {
                (_, Some(pr)) => format!(" PR #{pr}"),
                (Some(issue), _) => format!(" #{issue}"),
                _ => String::new(),
            };
            let suggestion = phase_suggestion(&state.phase, state.issue, state.pr);
            println!(
                "  {} {}{context}{marker}  (confidence: {:.0}%)  suggested: {suggestion}",
                id,
                state.phase,
                state.confidence * 100.0
            );
            if !state.signals.is_empty() {
                for signal in &state.signals {
                    println!("    - {signal}");
                }
            }
        }
    }

    if !map.stale.is_empty() {
        println!();
        println!("Stale ({}):", map.stale.len());
        for state in &map.stale {
            let id = state.label.as_deref().unwrap_or(&state.session_id);
            println!(
                "  {} {} (stale since {})",
                id, state.phase, state.detected_at
            );
        }
    }

    println!();
    println!("Summary: {}", map.summary);
}

/// Infer session ID from active heartbeats (same as other bridge commands).
fn infer_session_id(project_id: &str) -> Option<String> {
    let peers = edda_bridge_claude::peers::discover_all_sessions(project_id);
    // If only one session, that's us
    if peers.len() == 1 {
        return Some(peers[0].session_id.clone());
    }
    // Otherwise, check env var
    std::env::var("EDDA_SESSION_ID")
        .ok()
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use edda_core::agent_phase::{AgentPhase, AgentPhaseMap, AgentPhaseState};

    #[test]
    fn print_phase_map_empty() {
        let map = AgentPhaseMap::from_agents(vec![], vec![]);
        // Should not panic
        print_phase_map(&map, None);
    }

    #[test]
    fn print_phase_map_with_agents() {
        let state = AgentPhaseState {
            phase: AgentPhase::Implement,
            session_id: "sess-1".to_string(),
            label: Some("auth-worker".to_string()),
            issue: Some(45),
            pr: None,
            branch: Some("feat/auth-45".to_string()),
            confidence: 0.85,
            detected_at: "2026-02-27T10:00:00Z".to_string(),
            signals: vec!["branch feat/auth-45 created".to_string()],
        };
        let map = AgentPhaseMap::from_agents(vec![state], vec![]);
        // Should not panic
        print_phase_map(&map, Some("sess-1"));
    }

    // ── GH-547: phase approve/reject sugar over `edda verdict` ──────

    use edda_conductor::plan::parser::parse_plan;
    use edda_conductor::state::machine::{transition, PhaseUpdate, PlanState};
    use edda_ledger::Ledger;

    fn sha(ch: char) -> String {
        std::iter::repeat_n(ch, 40).collect()
    }

    /// Workspace with ledger + a conductor state where phase "impl" is
    /// AWAITING_VERDICT on gate_sha("a"*40), attempt 1.
    fn awaiting_ws(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("edda_cmdphase_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Ledger::ensure_initialized(&dir).unwrap();

        let plan = parse_plan("name: gated\nphases:\n  - id: impl\n    prompt: x\n").unwrap();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");
        transition(
            &mut state,
            "impl",
            PhaseStatus::Pending,
            PhaseStatus::Running,
            Some(PhaseUpdate {
                attempts: Some(1),
                ..Default::default()
            }),
        )
        .unwrap();
        transition(
            &mut state,
            "impl",
            PhaseStatus::Running,
            PhaseStatus::Checking,
            None,
        )
        .unwrap();
        transition(
            &mut state,
            "impl",
            PhaseStatus::Checking,
            PhaseStatus::AwaitingVerdict,
            Some(PhaseUpdate {
                gate_sha: Some(sha('a')),
                gate_entered_at: Some("2026-01-01T00:00:00Z".into()),
                ..Default::default()
            }),
        )
        .unwrap();
        edda_conductor::state::persist::save_state(&dir, &state).unwrap();
        dir
    }

    #[test]
    fn sugar_requires_plan_slash_phase_subject() {
        let ws = awaiting_ws("subject");
        let err = resolve_gate(&ws, "gated-impl").unwrap_err().to_string();
        assert!(err.contains("<plan-name>/<phase-id>"), "unexpected: {err}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn sugar_missing_plan_is_loud() {
        let ws = awaiting_ws("noplan");
        let err = resolve_gate(&ws, "no-such-plan/impl")
            .unwrap_err()
            .to_string();
        assert!(err.contains("no conductor state"), "unexpected: {err}");
        assert!(err.contains("state.json"), "unexpected: {err}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn sugar_missing_phase_lists_candidates() {
        let ws = awaiting_ws("nophase");
        let err = resolve_gate(&ws, "gated/wrong-id").unwrap_err().to_string();
        assert!(err.contains("no phase \"wrong-id\""), "unexpected: {err}");
        assert!(err.contains("impl"), "unexpected: {err}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn sugar_refuses_phase_not_awaiting_verdict() {
        let ws = awaiting_ws("notawaiting");
        // Reset the phase to Pending so the gate is not live.
        let mut state = edda_conductor::state::persist::load_state(&ws, "gated")
            .unwrap()
            .unwrap();
        state.phases[0].status = PhaseStatus::Pending;
        edda_conductor::state::persist::save_state(&ws, &state).unwrap();

        let err = resolve_gate(&ws, "gated/impl").unwrap_err().to_string();
        assert!(err.contains("not awaiting a verdict"), "unexpected: {err}");
        assert!(err.to_lowercase().contains("pending"), "unexpected: {err}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn sugar_resolves_sha_and_session_from_conductor_state() {
        let ws = awaiting_ws("resolve");
        let gate = resolve_gate(&ws, "gated/impl").unwrap();
        assert_eq!(gate.gate_sha, sha('a'));
        assert_eq!(
            gate.session_id,
            phase_session_id_attempt("gated", "impl", 1).to_string()
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn sugar_approve_records_verdict_through_the_verdict_path() {
        let ws = awaiting_ws("approve");
        run_gate_sugar(
            PhaseCmd::Approve {
                subject: "gated/impl".into(),
                sha: None,
                comment: Some("looks good".into()),
                session: None,
            },
            &ws,
        )
        .unwrap();
        let ledger = Ledger::open(&ws).unwrap();
        let verdict = ledger
            .latest_verdict("gated/impl", &sha('a'))
            .unwrap()
            .expect("verdict should be recorded at the resolved gate_sha");
        assert_eq!(verdict.payload.decision, VerdictDecision::Approved);
        assert_eq!(verdict.payload.comment.as_deref(), Some("looks good"));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn sugar_reject_records_verdict_and_keeps_comment() {
        let ws = awaiting_ws("reject");
        run_gate_sugar(
            PhaseCmd::Reject {
                subject: "gated/impl".into(),
                comment: "tests fail".into(),
                sha: None,
                session: None,
            },
            &ws,
        )
        .unwrap();
        let ledger = Ledger::open(&ws).unwrap();
        let verdict = ledger
            .latest_verdict("gated/impl", &sha('a'))
            .unwrap()
            .expect("verdict should be recorded");
        assert_eq!(verdict.payload.decision, VerdictDecision::Rejected);
        assert_eq!(verdict.payload.comment.as_deref(), Some("tests fail"));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn sugar_sha_override_wins_and_short_sha_still_refused() {
        let ws = awaiting_ws("override");
        // Explicit --sha override is honored...
        let good = sha('f');
        run_gate_sugar(
            PhaseCmd::Approve {
                subject: "gated/impl".into(),
                sha: Some(good.clone()),
                comment: None,
                session: None,
            },
            &ws,
        )
        .unwrap();
        let ledger = Ledger::open(&ws).unwrap();
        assert!(ledger
            .latest_verdict("gated/impl", &good)
            .unwrap()
            .is_some());

        // ...but the verdict path's 40-hex validation still applies.
        let err = run_gate_sugar(
            PhaseCmd::Approve {
                subject: "gated/impl".into(),
                sha: Some("abc123".into()),
                comment: None,
                session: None,
            },
            &ws,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("40-hex"), "unexpected: {err}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn sugar_reject_requires_comment_at_parse_time() {
        use clap::Parser;
        #[derive(Debug, Parser)]
        struct Cli {
            #[command(subcommand)]
            cmd: PhaseCmd,
        }
        let err = Cli::try_parse_from(["edda", "reject", "gated/impl"])
            .expect_err("missing --comment must be a parse error");
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }
}
