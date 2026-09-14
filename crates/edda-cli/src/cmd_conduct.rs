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
use std::path::{Path, PathBuf};
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
    let plan = if dry_run {
        eprintln!(
            "Schema preview only: draft carriers/checks are validated, not executed or accepted."
        );
        edda_conductor::plan::preview::load_preview(plan_file)?
    } else {
        load_plan(plan_file)?
    };
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

    // GH-557: record the store this run actually uses, so recovery verbs can
    // find a plan launched from a directory no worktree scan can reach (the
    // plan YAML's own folder). A dry run writes nothing.
    if !dry_run {
        let shell_cwd = std::env::current_dir().unwrap_or_else(|_| cwd.clone());
        for root in store::registry_roots_for(&cwd, &shell_cwd) {
            store::record_registry(&root, &plan.name, &cwd);
        }
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
            require_codex_persistence: false,
            require_codex_thread: false,
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
    print!("{}", status_impl(repo_root, plan_name, json)?);
    Ok(())
}

/// GH-557: plan state lives in the store that launched it (the invocation
/// root or any git worktree) — scan all of them. Split from [`status`] so
/// the rendered text is testable without stdout capture.
fn status_impl(repo_root: &Path, plan_name: Option<&str>, json: bool) -> Result<String> {
    let mut out = String::new();
    let plans: Vec<(String, PathBuf)> = if let Some(name) = plan_name {
        match store::resolve_plan_store(repo_root, name)? {
            Some(store) => vec![(name.to_string(), store)],
            None => vec![(name.to_string(), repo_root.to_path_buf())],
        }
    } else {
        // One discovery pass shared with the recovery verbs, so the listing
        // and the verbs can never disagree. A corrupt state file degrades to
        // a stderr warning + omission on this read-only overview; mutating
        // verbs propagate it instead.
        let (mut found, corrupt) = store::discover_plans(repo_root);
        for (name, store, e) in &corrupt {
            eprintln!(
                "⚠ plan \"{name}\" state in {} unreadable, omitted from the listing: {e:#}",
                store.display()
            );
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    };

    if plans.is_empty() {
        out.push_str(if json { "[]\n" } else { "No plans found.\n" });
        return Ok(out);
    }

    if json {
        // Every machine-readable object carries `store` so same-named plans
        // across lanes are distinguishable; flattened so existing field
        // paths (`plan_name`, `phases`, …) stay stable.
        #[derive(serde::Serialize)]
        struct StatusJson {
            store: String,
            #[serde(flatten)]
            state: PlanState,
        }
        let row = |store: &Path, state: PlanState| StatusJson {
            store: store::normalize_store_path(store),
            state,
        };
        if plan_name.is_some() {
            let (name, store) = &plans[0];
            match load_state(store, name)? {
                Some(s) => {
                    out.push_str(&serde_json::to_string_pretty(&row(store, s))?);
                    out.push('\n');
                }
                None => out.push_str("null\n"),
            }
        } else {
            let mut states = Vec::new();
            for (name, store) in &plans {
                match load_state(store, name) {
                    Ok(Some(s)) => states.push(row(store, s)),
                    Ok(None) => eprintln!(
                        "⚠ plan \"{name}\" state in {} disappeared before load, omitted",
                        store.display()
                    ),
                    Err(e) => eprintln!(
                        "⚠ plan \"{name}\" state in {} unreadable at load, omitted: {e:#}",
                        store.display()
                    ),
                }
            }
            out.push_str(&serde_json::to_string_pretty(&states)?);
            out.push('\n');
        }
    } else {
        for (name, store) in &plans {
            let state = load_state(store, name)?;
            match state {
                Some(s) => {
                    let label = if store::normalize_store_path(store)
                        == store::normalize_store_path(repo_root)
                    {
                        "(invocation root)".to_string()
                    } else {
                        store::normalize_store_path(store)
                    };
                    out.push_str(&format!("  Store: {label}\n"));
                    out.push_str(&print_status_to_string(&s));
                }
                None => out.push_str(&format!("Plan \"{name}\": no state file found\n")),
            }
        }
    }

    Ok(out)
}

/// Execute `edda conduct retry <phase-id>`
pub fn retry(repo_root: &Path, phase_id: &str, plan_name: Option<&str>) -> Result<()> {
    let name = store::resolve_plan_name(repo_root, plan_name)?;
    let store = store::resolve_plan_store(repo_root, &name)?
        .ok_or_else(|| store::no_state_error(repo_root, &name))?;
    let plan_file = update_state(&store, &name, |state| {
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

        Ok(state.plan_file.clone())
    })?;

    println!("Phase \"{phase_id}\" reset to Pending.");
    println!("  store: {}", store.display());
    if plan_file.is_empty() {
        println!(
            "  resume: `edda conduct run <plan.yaml> --cwd {}`",
            store.display()
        );
    } else {
        println!(
            "  resume: `edda conduct run {plan_file} --cwd {}`",
            store.display()
        );
    }
    Ok(())
}

/// Execute `edda conduct skip <phase-id>`
pub fn skip(
    repo_root: &Path,
    phase_id: &str,
    reason: Option<&str>,
    plan_name: Option<&str>,
) -> Result<()> {
    let name = store::resolve_plan_name(repo_root, plan_name)?;
    let store = store::resolve_plan_store(repo_root, &name)?
        .ok_or_else(|| store::no_state_error(repo_root, &name))?;
    let is_waived = update_state(&store, &name, |state| {
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
    println!("  store: {}", store.display());
    Ok(())
}

/// Execute `edda conduct abort [plan-name]`
pub fn abort(repo_root: &Path, plan_name: Option<&str>) -> Result<()> {
    let name = store::resolve_plan_name(repo_root, plan_name)?;
    let store = store::resolve_plan_store(repo_root, &name)?
        .ok_or_else(|| store::no_state_error(repo_root, &name))?;
    update_state(&store, &name, |state| {
        if state.plan_status == PlanStatus::Completed || state.plan_status == PlanStatus::Aborted {
            bail!("Plan \"{}\" is already {:?}.", name, state.plan_status);
        }

        state.plan_status = PlanStatus::Aborted;
        state.aborted_at = Some(now_rfc3339());
        Ok(())
    })?;

    println!("Plan \"{name}\" aborted. (store: {})", store.display());
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

fn print_status_to_string(state: &PlanState) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "\nPlan: {} ({:?})\n",
        state.plan_name, state.plan_status
    ));
    if !state.plan_file.is_empty() {
        out.push_str(&format!("  File: {}\n", state.plan_file));
    }
    out.push_str(&format!(
        "  Cost: {}\n",
        cost_line(state.total_cost_usd, state.cost_measured)
    ));

    out.push('\n');
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
        let elapsed = ps
            .duration_ms
            .map(|ms| format!("{ms} ms"))
            .unwrap_or_else(|| "—".into());
        out.push_str(&format!(
            "  {icon} {:<24} {:?} {detail} elapsed={elapsed}\n",
            ps.id, ps.status
        ));
    }
    out.push('\n');
    out
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

mod store;

#[cfg(test)]
mod tests;
