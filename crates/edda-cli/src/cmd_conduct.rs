use crate::agent_kind::{build_launcher, AgentKind, LauncherOptions};
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use edda_conductor::agent::budget::BudgetTracker;
use edda_conductor::agent::launcher::phase_session_id;
use edda_conductor::check::engine::CheckEngine;
use edda_conductor::plan::parser::load_plan;
use edda_conductor::plan::schema::{GateKind, OnReject, Phase, Plan};
use edda_conductor::runner::notify::ChannelNotifier;
use edda_conductor::runner::sequential::{run_plan, RunContext};
use edda_conductor::state::machine::{PhaseStatus, PlanState, PlanStatus};
use edda_conductor::state::persist::{load_state, update_state};
use edda_conductor::tmux::TmuxSession;
use std::path::Path;
use tokio_util::sync::CancellationToken;

// ── CLI Schema ──

#[derive(Subcommand)]
pub enum ConductCmd {
    /// Run a plan from a YAML file
    Run {
        /// Path to plan.yaml
        plan_file: String,
        /// Override working directory
        #[arg(long)]
        cwd: Option<String>,
        /// Preview plan without executing
        #[arg(long)]
        dry_run: bool,
        /// Suppress live agent activity output
        #[arg(short, long)]
        quiet: bool,
        /// Output events as JSONL to stdout (for machine consumption)
        #[arg(long)]
        json: bool,
        /// Create a tmux session with per-phase transcript panes + dashboard
        #[arg(long)]
        tmux: bool,
        /// Agent backend that runs the phases (default: claude)
        #[arg(long, value_enum, default_value_t = AgentKind::Claude)]
        agent: AgentKind,
    },
    /// Show status of running/completed plans
    Status {
        /// Plan name (auto-detects if only one)
        plan_name: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Reset a failed/stale phase to Pending
    Retry {
        /// Phase ID to retry
        phase_id: String,
        /// Plan name (auto-detects if only one)
        #[arg(long)]
        plan: Option<String>,
    },
    /// Skip a failed/stale/pending phase
    Skip {
        /// Phase ID to skip
        phase_id: String,
        /// Reason for skipping
        #[arg(long)]
        reason: Option<String>,
        /// Plan name (auto-detects if only one)
        #[arg(long)]
        plan: Option<String>,
    },
    /// Abort a running plan
    Abort {
        /// Plan name (auto-detects if only one)
        plan_name: Option<String>,
    },
}

// ── Dispatch ──

pub fn run_cmd(cmd: ConductCmd, repo_root: &Path) -> Result<()> {
    match cmd {
        ConductCmd::Run {
            plan_file,
            cwd,
            dry_run,
            quiet,
            json,
            tmux,
            agent,
        } => run(
            Path::new(&plan_file),
            cwd.as_deref().map(Path::new),
            dry_run,
            !quiet,
            json,
            tmux,
            agent,
        ),
        ConductCmd::Status { plan_name, json } => status(repo_root, plan_name.as_deref(), json),
        ConductCmd::Retry { phase_id, plan } => retry(repo_root, &phase_id, plan.as_deref()),
        ConductCmd::Skip {
            phase_id,
            reason,
            plan,
        } => skip(repo_root, &phase_id, reason.as_deref(), plan.as_deref()),
        ConductCmd::Abort { plan_name } => abort(repo_root, plan_name.as_deref()),
    }
}

// ── Command Implementations ──

/// Execute `edda conduct run <plan.yaml>`
#[allow(clippy::too_many_lines)] // 201 lines at #779; split tracked in none
pub fn run(
    plan_file: &Path,
    cwd_override: Option<&Path>,
    dry_run: bool,
    verbose: bool,
    json_events: bool,
    tmux: bool,
    agent: AgentKind,
) -> Result<()> {
    let plan = load_plan(plan_file)?;
    let cwd = cwd_override
        .map(|p| p.to_path_buf())
        .or_else(|| {
            plan.cwd
                .as_ref()
                .map(|p| plan_file.parent().unwrap_or(Path::new(".")).join(p))
        })
        .unwrap_or_else(|| plan_file.parent().unwrap_or(Path::new(".")).to_path_buf());
    let cwd = if cwd.is_relative() {
        std::env::current_dir()?.join(&cwd)
    } else {
        cwd
    };

    // When --json, suppress human-readable output (verbose/TUI)
    let verbose = if json_events { false } else { verbose };

    // Resolve tmux availability
    let use_tmux = if tmux {
        if !TmuxSession::is_available() {
            eprintln!(
                "Warning: --tmux requested but tmux is not installed. \
                 Falling back to normal mode."
            );
            false
        } else if !agent.writes_transcripts() {
            // Phase panes tail transcript files; an agent that writes none
            // would leave every pane permanently blank.
            eprintln!(
                "Warning: --tmux requested but agent \"{}\" does not write phase \
                 transcripts, so the panes would stay empty. \
                 Falling back to normal mode.",
                agent.as_str()
            );
            false
        } else {
            true
        }
    } else {
        false
    };

    if let Some(warning) = budget_warning(&plan, agent) {
        eprintln!("{warning}");
    }

    // Load or create state
    let mut state = match load_state(&cwd, &plan.name)? {
        Some(s) => {
            if !json_events {
                println!("Resuming plan \"{}\"", plan.name);
            }
            s
        }
        None => {
            if !json_events {
                println!(
                    "Starting plan \"{}\" ({} phases)",
                    plan.name,
                    plan.phases.len()
                );
            }
            PlanState::from_plan(&plan, &plan_file.display().to_string())
        }
    };

    let order = edda_conductor::plan::topo::topo_sort(&plan)?;

    if dry_run {
        println!("\n[dry-run] Plan: {}", plan.name);
        println!("  Phases: {}", plan.phases.len());
        println!(
            "  Budget: {}",
            plan.budget_usd
                .map_or("unlimited".into(), |b| format!("${b:.2}"))
        );
        println!("  Max attempts: {}", plan.max_attempts);
        println!("  On fail: {:?}", plan.on_fail);
        println!("\n  Phase order:");
        for (i, id) in order.iter().enumerate() {
            let phase = plan
                .phases
                .iter()
                .find(|p| p.id == *id)
                .context("phase referenced in topo order not found in plan")?;
            let checks = if phase.check.is_empty() {
                String::new()
            } else {
                format!(" ({} checks)", phase.check.len())
            };
            println!("  {}. {}{}{}", i + 1, id, checks, gate_preview(phase));
        }
        println!("\n  Session IDs:");
        for id in &order {
            println!("    {} \u{2192} {}", id, phase_session_id(&plan.name, id));
        }
        if use_tmux {
            TmuxSession::print_layout_preview(&plan.name, &order);
        }
        return Ok(());
    }

    let transcript_dir = cwd
        .join(".edda")
        .join("conductor")
        .join(&plan.name)
        .join("transcripts");

    let launcher = build_launcher(
        agent,
        LauncherOptions {
            verbose,
            transcript_dir: Some(transcript_dir.clone()),
            // Conduct never persists codex threads (GH-535 round 1): its
            // session ids are deterministic per plan/phase/attempt, so a
            // persisted binding could leak a stale thread/resume into a
            // later invocation and every turn would gain store I/O.
            persistent_codex_threads: false,
            // Conduct has no session-dir surface (GH-574); pi uses its own
            // default session storage under conduct.
            session_dir: None,
            // Conduct's session ids are deterministic per plan/phase/attempt
            // and each attempt is a fresh conversation, so it never resumes
            // (GH-708). Retries change the attempt, and therefore the id.
            resume: false,
        },
    )?;
    let engine = CheckEngine::new(cwd.clone());
    // GH-564 P1-1: the run notifier must deliver configured channel events —
    // a bare StdoutNotifier silently drops every phase terminal event. With
    // no channels configured this is behaviorally identical to stdout-only.
    let notifier = ChannelNotifier::for_repo(&cwd);
    let mut budget = BudgetTracker::new(plan.budget_usd);
    let cancel = CancellationToken::new();

    // Handle Ctrl+C gracefully
    let cancel_clone = cancel.clone();
    ctrlc_cancel(cancel_clone);

    let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());

    // Create tmux session if requested
    let tmux_session = if use_tmux {
        match TmuxSession::create(&plan.name, &order, &transcript_dir) {
            Ok(session) => {
                println!("Tmux session created: {}", session.session_name);
                println!("  Attach: tmux attach -t {}", session.session_name);
                Some(session)
            }
            Err(e) => {
                eprintln!(
                    "Warning: failed to create tmux session: {e}. \
                     Continuing without tmux."
                );
                None
            }
        }
    } else {
        None
    };

    let rt = tokio::runtime::Runtime::new()?;
    let result = rt.block_on(run_plan(
        &plan,
        &mut state,
        RunContext {
            launcher: launcher.as_ref(),
            check_engine: &engine,
            notifier: &notifier,
            budget: &mut budget,
            cancel,
            cwd: &cwd,
            interactive,
            json_events,
            tmux_session: tmux_session.as_ref(),
        },
    ));

    // Print tmux session info after run completes
    if let Some(ref session) = tmux_session {
        println!(
            "\nTmux session still active: tmux attach -t {}",
            session.session_name
        );
        println!("  Destroy: tmux kill-session -t {}", session.session_name);
    }

    result
}

/// Execute `edda conduct status [plan-name]`
pub fn status(repo_root: &Path, plan_name: Option<&str>, json: bool) -> Result<()> {
    let conductor_dir = repo_root.join(".edda").join("conductor");
    if !conductor_dir.exists() {
        if json {
            println!("[]");
        } else {
            println!("No conductor state found.");
        }
        return Ok(());
    }

    let plans: Vec<String> = if let Some(name) = plan_name {
        vec![name.to_string()]
    } else {
        // List all plan directories
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&conductor_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    names.push(name.to_string());
                }
            }
        }
        names.sort();
        names
    };

    if plans.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No plans found.");
        }
        return Ok(());
    }

    if json {
        let states: Vec<PlanState> = plans
            .iter()
            .filter_map(|name| load_state(repo_root, name).ok().flatten())
            .collect();
        // Single plan name specified: output object directly; otherwise array
        if plan_name.is_some() {
            if let Some(s) = states.into_iter().next() {
                println!("{}", serde_json::to_string_pretty(&s)?);
            } else {
                println!("null");
            }
        } else {
            println!("{}", serde_json::to_string_pretty(&states)?);
        }
    } else {
        for name in &plans {
            let state = load_state(repo_root, name)?;
            match state {
                Some(s) => print_status(&s),
                None => println!("Plan \"{name}\": no state file found"),
            }
        }
    }

    Ok(())
}

/// Execute `edda conduct retry <phase-id>`
pub fn retry(repo_root: &Path, phase_id: &str, plan_name: Option<&str>) -> Result<()> {
    let name = resolve_plan_name(repo_root, plan_name)?;
    update_state(repo_root, &name, |state| {
        let current_status = {
            let ps = state.get_phase_mut(phase_id)?;
            if ps.status != PhaseStatus::Failed
                && ps.status != PhaseStatus::Stale
                && ps.status != PhaseStatus::GateTimedOut
            {
                bail!(
                    "Phase \"{}\" is {:?}, not Failed or Stale. Cannot retry.",
                    phase_id,
                    ps.status
                );
            }
            ps.status
        };

        edda_conductor::state::machine::transition(
            state,
            phase_id,
            current_status,
            PhaseStatus::Pending,
            None,
        )?;

        // Reset plan status so runner picks up
        if state.plan_status == PlanStatus::Blocked {
            state.plan_status = PlanStatus::Running;
        }

        Ok(())
    })?;

    println!("Phase \"{phase_id}\" reset to Pending. Run `edda conduct run` to resume.");
    Ok(())
}

/// Execute `edda conduct skip <phase-id>`
pub fn skip(
    repo_root: &Path,
    phase_id: &str,
    reason: Option<&str>,
    plan_name: Option<&str>,
) -> Result<()> {
    let name = resolve_plan_name(repo_root, plan_name)?;
    let is_waived = update_state(repo_root, &name, |state| {
        let ps = state.get_phase_mut(phase_id)?;
        if ps.status == PhaseStatus::GateTimedOut {
            // GH-552: skipping a timed-out gate is a WAIVER — the phase ran and
            // its checks passed, so recording `Skipped` would understate what
            // was verified. Keep the honest status, record the waiver.
            ps.skip_reason = Some(
                reason
                    .unwrap_or("gate waived by operator (edda conduct skip)")
                    .to_string(),
            );
            if state.plan_status == PlanStatus::Blocked {
                state.plan_status = PlanStatus::Running;
            }
            return Ok(true);
        }
        if ps.status != PhaseStatus::Failed
            && ps.status != PhaseStatus::Stale
            && ps.status != PhaseStatus::Pending
        {
            bail!(
                "Phase \"{}\" is {:?}. Can only skip Failed, Stale, or Pending phases.",
                phase_id,
                ps.status
            );
        }

        ps.status = PhaseStatus::Skipped;
        ps.skip_reason = Some(reason.unwrap_or("manually skipped").to_string());

        // Unblock plan
        if state.plan_status == PlanStatus::Blocked {
            state.plan_status = PlanStatus::Running;
        }

        Ok(false)
    })?;

    if is_waived {
        println!("Phase \"{phase_id}\" gate waived (status kept as GateTimedOut).");
    } else {
        println!("Phase \"{phase_id}\" skipped.");
    }
    Ok(())
}

/// Execute `edda conduct abort [plan-name]`
pub fn abort(repo_root: &Path, plan_name: Option<&str>) -> Result<()> {
    let name = resolve_plan_name(repo_root, plan_name)?;
    update_state(repo_root, &name, |state| {
        if state.plan_status == PlanStatus::Completed || state.plan_status == PlanStatus::Aborted {
            bail!("Plan \"{}\" is already {:?}.", name, state.plan_status);
        }

        state.plan_status = PlanStatus::Aborted;
        state.aborted_at = Some(now_rfc3339());
        Ok(())
    })?;

    println!("Plan \"{name}\" aborted.");
    Ok(())
}

// --- helpers ---

/// One-line startup warning when the selected backend cannot enforce budgets.
///
/// codex exposes no cost/usage data, so every phase reports `cost_usd: None`:
/// the per-phase gate is inert and the sequential runner never feeds the
/// plan-level `BudgetTracker`, leaving both phase and plan `budget_usd`
/// unenforced and any printed cost figure a guess. Mirrors the --tmux
/// warn-and-fall-back tone above.
fn budget_warning(plan: &Plan, agent: AgentKind) -> Option<String> {
    let any_budget =
        plan.budget_usd.is_some() || plan.phases.iter().any(|p| p.budget_usd.is_some());
    budget_warning_for_agent(agent, any_budget)
}

/// Backend + has-any-budget form of [`budget_warning`], shared with
/// `edda dispatch`, which has flags instead of a plan file.
pub(crate) fn budget_warning_for_agent(agent: AgentKind, any_budget: bool) -> Option<String> {
    if agent == AgentKind::Codex && any_budget {
        Some(format!(
            "Warning: agent \"{}\" exposes no usage data, so budget_usd limits will not \
             be enforced and reported cost is unavailable.",
            agent.as_str()
        ))
    } else {
        None
    }
}

/// Dry-run suffix rendering a phase's verdict gate, so `--dry-run` shows
/// "this phase will stop and wait for a human" — the one thing a gate
/// changes about the operational shape of the run. The no-timeout case
/// spells out "waits until cancelled" because that is the footgun for
/// unattended batches.
fn gate_preview(phase: &Phase) -> String {
    let Some(kind) = phase.gate else {
        return String::new();
    };
    // Literal YAML spellings, matched exhaustively: a future variant must
    // fail to compile here rather than render a string no plan file could
    // have spelled.
    let kind = match kind {
        GateKind::Verdict => "verdict",
    };
    let on_reject = match phase.on_reject {
        OnReject::Redispatch => "redispatch",
        OnReject::Halt => "halt",
    };
    let timeout = phase
        .gate_timeout_sec
        .map_or_else(|| "waits until cancelled".into(), |t| format!("{t}s"));
    format!("  [gate: {kind}, timeout: {timeout}, on_reject: {on_reject}]")
}

/// The honest stand-in for a cost figure nobody measured, shared with
/// `edda dispatch`'s no-usage rendering so the string has one source.
pub(crate) const NO_USAGE_COST_TEXT: &str = "n/a (no usage data reported)";

/// The status cost line, derived from the cost model (GH-533).
///
/// `PlanState` records `cost_measured` alongside `total_cost_usd`, so a
/// total nobody measured (usage-free backends like codex) renders as "n/a"
/// while a genuine measured figure — including a real $0.00 — is asserted
/// as-is. Under-claiming beats asserting an unmeasured figure.
pub(crate) fn cost_line(total_cost_usd: f64, cost_measured: bool) -> String {
    if !cost_measured {
        NO_USAGE_COST_TEXT.to_owned()
    } else {
        format!("${total_cost_usd:.2}")
    }
}

fn resolve_plan_name(repo_root: &Path, explicit: Option<&str>) -> Result<String> {
    if let Some(name) = explicit {
        return Ok(name.to_string());
    }

    let conductor_dir = repo_root.join(".edda").join("conductor");
    if !conductor_dir.exists() {
        bail!("No conductor state found. Specify --plan <name>.");
    }

    let mut names = Vec::new();
    for entry in std::fs::read_dir(&conductor_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if let Some(n) = entry.file_name().to_str() {
                names.push(n.to_string());
            }
        }
    }

    match names.len() {
        0 => bail!("No plans found."),
        1 => Ok(names
            .into_iter()
            .next()
            .context("expected exactly one plan")?),
        _ => bail!(
            "Multiple plans found: {}. Use --plan to specify.",
            names.join(", ")
        ),
    }
}

fn print_status(state: &PlanState) {
    println!("\nPlan: {} ({:?})", state.plan_name, state.plan_status);
    if !state.plan_file.is_empty() {
        println!("  File: {}", state.plan_file);
    }
    println!(
        "  Cost: {}",
        cost_line(state.total_cost_usd, state.cost_measured)
    );

    println!();
    for ps in &state.phases {
        let icon = match ps.status {
            PhaseStatus::Passed => "\u{2713}",                          // ✓
            PhaseStatus::Failed => "\u{2717}",                          // ✗
            PhaseStatus::Running | PhaseStatus::Checking => "\u{25B6}", // ▶
            PhaseStatus::Skipped => "\u{2298}",                         // ⊘
            PhaseStatus::Stale => "\u{23F0}",                           // ⏰
            PhaseStatus::AwaitingVerdict => "\u{23F8}",                 // ⏸
            PhaseStatus::GateTimedOut => "\u{29D7}",                    // ⧗
            PhaseStatus::Pending => "\u{25CB}",                         // ○
        };
        let detail = match ps.status {
            PhaseStatus::Passed => format!("(attempt {})", ps.attempts),
            PhaseStatus::Failed => {
                let err = ps
                    .error
                    .as_ref()
                    .map(|e| e.message.as_str())
                    .unwrap_or("unknown");
                format!("(attempt {}, {})", ps.attempts, err)
            }
            PhaseStatus::Skipped => {
                let reason = ps.skip_reason.as_deref().unwrap_or("");
                format!("({})", reason)
            }
            PhaseStatus::GateTimedOut => {
                // GH-552: honest audit line — timed-out gate, and whether
                // it was waived (the status itself is never Skipped).
                match ps.skip_reason.as_deref() {
                    Some(reason) => format!("(waived: {})", reason),
                    None => "(awaiting operator: retry or waive)".to_string(),
                }
            }
            _ => String::new(),
        };
        println!("  {icon} {:<24} {:?} {detail}", ps.id, ps.status);
    }
    println!();
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn ctrlc_cancel(cancel: CancellationToken) {
    let _ = ctrlc::set_handler(move || {
        cancel.cancel();
    });
}

#[cfg(test)]
mod tests {
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
}
