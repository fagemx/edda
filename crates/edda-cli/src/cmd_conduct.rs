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

    // GH-557 round-3 P0-2 / round-4 P0-3: record the store this run actually
    // uses, so the recovery verbs can find a plan launched from a plain
    // directory (the plan YAML's own folder — the reported incident's shape)
    // that no worktree scan can enumerate. find_root(run_cwd) returns the
    // run cwd ITSELF once its .edda exists, so a single-root choice files
    // every plan after the first where nothing reads it. Record into EVERY
    // candidate root — the invoking shell's root first (the lane the
    // operator/agent stands in), then the run cwd's — reads scan all of
    // them, so at least one lands inside plan_stores scope.
    // Round-4 P1-4: the chain is a pure function so tests can drive it.
    for root in registry_roots_for(
        &cwd,
        &std::env::current_dir().unwrap_or_else(|_| cwd.clone()),
    ) {
        registry_record(&root, &plan.name, &cwd);
    }

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
    let out = status_impl(repo_root, plan_name, json)?;
    print!("{out}");
    Ok(())
}

/// GH-557: plan state lives in the store that launched it (repo root or a
/// git worktree) — scan all of them. Split from [`status`] so the text is
/// testable without stdout capture.
fn status_impl(repo_root: &Path, plan_name: Option<&str>, json: bool) -> Result<String> {
    let mut out = String::new();
    let plans: Vec<(String, PathBuf)> = if let Some(name) = plan_name {
        match resolve_plan_store(repo_root, name)? {
            Some(store) => vec![(name.to_string(), store)],
            None => vec![(name.to_string(), repo_root.to_path_buf())],
        }
    } else {
        // Round-2 P2, adjusted by round-3 P1-1, adjusted by round-4 P1-1/P1-3:
        // the unnamed listing is a READ-ONLY overview — it scans the direct
        // worktree union AND every store registry (a scratchpad-launched plan
        // is invisible otherwise, which is the literally-reported symptom of
        // #557), and a corrupt state file degrades to a stderr warning + a
        // marked entry instead of killing the overview of every healthy lane.
        // Mutating verbs still propagate corrupt files (retarget risk).
        let mut found: Vec<(String, PathBuf)> = Vec::new();
        let mark = |name: &str, store: &Path, found: &mut Vec<(String, PathBuf)>| {
            if !found.iter().any(|(n, _)| n == name) {
                found.push((name.to_string(), store.to_path_buf()));
            }
        };
        for store in plan_stores(repo_root) {
            let conductor_dir = store.join(".edda").join("conductor");
            if !conductor_dir.exists() {
                continue;
            }
            // Registry-referenced stores first (round-4 P1-1).
            match registry_read(&store) {
                Ok(registry) => {
                    for (name, referenced) in &registry {
                        match load_state(referenced, name) {
                            Ok(Some(_)) => mark(name, referenced, &mut found),
                            Ok(None) => continue,
                            Err(e) => eprintln!(
                                "⚠ plan \"{name}\" (registry → {}) unreadable, listed as broken: {e}",
                                referenced.display()
                            ),
                        }
                    }
                }
                Err(e) => eprintln!("⚠ store registry unreadable ({}): {e}", store.display()),
            }
            for entry in std::fs::read_dir(&conductor_dir)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        match load_state(&store, name) {
                            Ok(Some(_)) => mark(name, &store, &mut found),
                            Ok(None) => continue,
                            Err(e) => eprintln!(
                                "⚠ plan \"{name}\" state in {} unreadable, omitted from listing: {e}",
                                store.display()
                            ),
                        }
                    }
                }
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    };

    if plans.is_empty() {
        if json {
            out.push_str("[]\n");
        } else {
            out.push_str("No plans found.\n");
        }
        return Ok(out);
    }

    if json {
        // Named-missing keeps the pre-GH-557 "null" contract (round-3 P1-1);
        // a CORRUPT state file still propagates as an error. The unnamed
        // listing builds only from parseable states, so its array is clean.
        if plan_name.is_some() {
            let (name, store) = &plans[0];
            let state = load_state(store, name)?;
            match state {
                Some(s) => {
                    out.push_str(&serde_json::to_string_pretty(&s)?);
                    out.push('\n');
                }
                None => out.push_str("null\n"),
            }
        } else {
            let mut states = Vec::new();
            for (name, store) in &plans {
                states.push(load_state(store, name)?.ok_or_else(|| {
                    anyhow::anyhow!("plan \"{name}\": no state file in {}", store.display())
                })?);
            }
            out.push_str(&serde_json::to_string_pretty(&states)?);
            out.push('\n');
        }
    } else {
        for (name, store) in &plans {
            let state = load_state(store, name)?;
            match state {
                Some(s) => {
                    out.push_str(&format!(
                        "  Store: {}\n",
                        if *store == repo_root {
                            "(repo root)".to_string()
                        } else {
                            store.display().to_string()
                        }
                    ));
                    out.push_str(&print_status_to_string(&s));
                }
                None => out.push_str(&format!("Plan \"{name}\": no state file found\n")),
            }
        }
    }

    Ok(out)
}

/// Render one plan's status block (the body `print_status` writes).
fn print_status_to_string(state: &PlanState) -> String {
    let mut out = String::new();
    out.push('\n');
    out.push_str(&format!(
        "Plan: {} ({:?})\n",
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
            PhaseStatus::Passed => "\u{2713}",
            PhaseStatus::Failed => "\u{2717}",
            PhaseStatus::Running | PhaseStatus::Checking => "\u{25B6}",
            PhaseStatus::Skipped => "\u{2298}",
            PhaseStatus::Stale => "\u{23F0}",
            PhaseStatus::GateTimedOut => "\u{29D7}",
            PhaseStatus::AwaitingVerdict => "\u{23F8}",
            PhaseStatus::Pending => "\u{25CB}",
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
            // GH-552: a waived gate keeps the honest status — "skip" would
            // understate what was verified (the phase ran and passed).
            PhaseStatus::GateTimedOut => match ps.skip_reason.as_deref() {
                Some(reason) => format!("(waived: {})", reason),
                None => "(awaiting operator: retry or waive)".to_string(),
            },
            _ => String::new(),
        };
        out.push_str(&format!(
            "  {icon} {:<24} {:?} {detail}\n",
            ps.id, ps.status
        ));
    }
    out.push('\n');
    out
}

/// Execute `edda conduct retry <phase-id>`
pub fn retry(repo_root: &Path, phase_id: &str, plan_name: Option<&str>) -> Result<()> {
    let name = resolve_plan_name(repo_root, plan_name)?;
    let store =
        resolve_plan_store(repo_root, &name)?.ok_or_else(|| no_state_error(repo_root, &name))?;
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

    // GH-557 review round 2, P1-1: a destructive verb must name the store
    // it wrote, and the resume hint must resolve the SAME store.
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
    let name = resolve_plan_name(repo_root, plan_name)?;
    let store =
        resolve_plan_store(repo_root, &name)?.ok_or_else(|| no_state_error(repo_root, &name))?;
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

    println!("  store: {}", store.display());
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
    let store =
        resolve_plan_store(repo_root, &name)?.ok_or_else(|| no_state_error(repo_root, &name))?;
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

/// Directories that can hold conductor state for this repo (GH-557):
/// the repo root itself plus every git worktree reported by
/// `git worktree list --porcelain`. A plan launched with `--cwd
/// <worktree>` stores its state in that worktree's `.edda/conductor/`,
/// and the recovery verbs must be able to find it no matter which cwd
/// the operator invokes from.
/// Normalized identity of a store path: canonicalized where possible
/// (resolving 8.3 short names, symlinks, and case) with the `\\?\` UNC
/// prefix stripped, lowercased — Windows path forms diverge wildly
/// (`RUNNER~1` vs the long name, forward vs back slashes) and the same
/// physical store MUST NOT enter the list twice (GH-557 round-4 P0-2).
fn normalize_store_path(p: &Path) -> String {
    let canonical = std::fs::canonicalize(p)
        .map(|c| c.to_string_lossy().trim_start_matches(r"\\?\").to_string())
        .unwrap_or_else(|_| p.to_string_lossy().to_string());
    canonical.replace('/', "\\").to_lowercase()
}

fn plan_stores(repo_root: &Path) -> Vec<PathBuf> {
    let mut stores: Vec<PathBuf> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let push = |p: PathBuf, stores: &mut Vec<PathBuf>, seen: &mut Vec<String>| {
        let key = normalize_store_path(&p);
        if !seen.contains(&key) {
            seen.push(key);
            stores.push(p);
        }
    };
    push(repo_root.to_path_buf(), &mut stores, &mut seen);
    let out = std::process::Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_root)
        .output();
    if let Ok(out) = out {
        if out.status.success() {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if let Some(path) = line.strip_prefix("worktree ") {
                    push(PathBuf::from(path), &mut stores, &mut seen);
                }
            }
        } else {
            // GH-557 round-3 P1-2: silent degradation turns the no-state
            // error into a false diagnosis ("searched: <repo root>").
            eprintln!(
                "⚠ could not enumerate git worktrees ({}); searching the repo root only",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    }
    stores
}

/// Store registry: `<root>/.edda/conductor/.stores.json` maps plan name →
/// the store directory `conduct run` actually used (GH-557 round 3, P0-2).
/// The reported incident's store was the plan YAML's own directory — a
/// plain path that is neither the repo root nor a registered worktree —
/// so search-order heuristics can never cover it; `run` records the store
/// it chose and the recovery verbs look it up.
fn registry_path(root: &Path) -> PathBuf {
    root.join(".edda").join("conductor").join(".stores.json")
}

/// Read a store registry. Best-effort: missing or corrupt file reads as an
/// empty map — the direct worktree scan remains the fallback.
/// Candidate registry roots for a run launched with `run_cwd`, invoked
/// from `shell_cwd` (GH-557 round-4 P0-3): the invoking shell's root
/// first — the lane the operator/agent stands in — then the run cwd's.
/// `find_root(run_cwd)` returns the run cwd ITSELF once its `.edda`
/// exists (created by the first run there), so a single-root choice
/// files every plan after the first where nothing reads it; recording
/// into every candidate and reading from every scanned store makes the
/// write side and the read side meet.
fn registry_roots_for(run_cwd: &Path, shell_cwd: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for r in [
        edda_ledger::EddaPaths::find_root(shell_cwd),
        edda_ledger::EddaPaths::find_root(run_cwd),
    ]
    .into_iter()
    .flatten()
    {
        if !roots.contains(&r) {
            roots.push(r);
        }
    }
    if roots.is_empty() {
        roots.push(run_cwd.to_path_buf());
    }
    roots
}

/// Read a store registry. Missing file reads as an empty map; a CORRUPT
/// file is an error — round-4 P1-2: silently reading it as empty makes the
/// next record persist an empty map plus one key, destroying every other
/// plan's entry.
fn registry_read(root: &Path) -> Result<std::collections::BTreeMap<String, PathBuf>> {
    let path = registry_path(root);
    if !path.exists() {
        return Ok(std::collections::BTreeMap::new());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("reading store registry {}", path.display()))?;
    let parsed: std::collections::BTreeMap<String, String> = serde_json::from_str(&content)
        .with_context(|| format!("parsing store registry {}", path.display()))?;
    Ok(parsed
        .into_iter()
        .map(|(plan, store)| (plan, PathBuf::from(store)))
        .collect())
}

/// Record plan → store in the registry under `root`, under the registry's
/// exclusive lock (round-4 P1-2: two concurrent `conduct run` processes
/// are the parallel-wave norm; unlocked read-modify-write loses entries).
/// Best-effort: lock contention or a corrupt existing registry prints a
/// warning and skips the write — discovery still has the direct scan.
fn registry_record(root: &Path, plan: &str, store: &Path) {
    let path = registry_path(root);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let lock = match edda_store::lock_file(&PathBuf::from(format!("{}.lock", path.display()))) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("⚠ store registry lock failed ({}): {e}", path.display());
            return;
        }
    };
    let mut map = match registry_read(root) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("⚠ store registry unreadable, skipping record: {e}");
            return;
        }
    };
    map.insert(plan.to_string(), store.to_path_buf());
    match serde_json::to_string_pretty(&map) {
        Ok(data) => {
            if let Err(e) = edda_store::write_atomic(&path, data.as_bytes()) {
                eprintln!("⚠ store registry write failed ({}): {e}", path.display());
            }
        }
        Err(e) => eprintln!("⚠ store registry serialize failed: {e}"),
    }
    drop(lock);
}

/// Every store currently holding `<plan>`'s state.json (GH-557), in
/// deterministic order (repo root first, then `git worktree list` order).
/// A corrupt state file is an error, NOT "absent" — swallowing it would
/// retarget a mutating verb onto a different store that happens to hold a
/// stale same-name state.
fn stores_holding(repo_root: &Path, plan: &str) -> Result<Vec<PathBuf>> {
    edda_conductor::state::persist::validate_plan_name(plan)?;
    let mut holding = Vec::new();
    for store in plan_stores(repo_root) {
        match load_state(&store, plan) {
            Ok(Some(_)) => holding.push(store),
            Ok(None) => continue,
            Err(e) => {
                return Err(e.context(format!(
                    "plan \"{plan}\" state in {} is unreadable",
                    store.display()
                )))
            }
        }
    }
    // Registry-referenced stores (recorded by conduct run) — covers plan
    // YAMLs living outside the repo, which no worktree scan can reach
    // (round-3 P0-2: the reported incident's store was the plan YAML's own
    // directory).
    for store in plan_stores(repo_root) {
        // Round-4 P1-2: a corrupt REGISTRY is read-path noise (warn, keep
        // scanning) — mutating verbs still propagate corrupt state files.
        let registry = match registry_read(&store) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("⚠ store registry unreadable ({}): {e}", store.display());
                continue;
            }
        };
        if let Some(referenced) = registry.get(plan) {
            if !holding.contains(referenced)
                && load_state(referenced, plan)
                    .with_context(|| {
                        format!(
                            "plan \"{plan}\" registry points to {}",
                            referenced.display()
                        )
                    })?
                    .is_some()
            {
                holding.push(referenced.clone());
            }
        }
    }
    Ok(holding)
}

/// The store a verb should act on: the first store holding the plan. When
/// more than one store holds the same plan name, the others are shadowed —
/// warn on stderr so a destructive verb is never mute about that (GH-557
/// independent review round 2, P1-1).
fn resolve_plan_store(repo_root: &Path, plan: &str) -> Result<Option<PathBuf>> {
    let holding = stores_holding(repo_root, plan)?;
    match holding.len() {
        0 => Ok(None),
        1 => Ok(Some(holding[0].clone())),
        _ => {
            eprintln!(
                "⚠ plan \"{plan}\" exists in {} stores; acting on {} (shadowed: {})",
                holding.len(),
                holding[0].display(),
                holding[1..]
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            Ok(Some(holding[0].clone()))
        }
    }
}

/// Refuse to act when the plan's state is nowhere on the repo (GH-557):
/// the runner's own blocked-phase message names retry/skip/abort — those
/// commands must never answer "no state for plan" without pointing at the
/// actual resolution surface.
fn no_state_error(repo_root: &Path, plan: &str) -> anyhow::Error {
    let searched: Vec<String> = plan_stores(repo_root)
        .iter()
        .filter(|p| p.join(".edda").join("conductor").is_dir())
        .map(|p| p.display().to_string())
        .collect();
    let shown = if searched.is_empty() {
        "no store with a .edda/conductor directory".to_string()
    } else if searched.len() > 5 {
        format!("{} … ({} stores)", searched[..5].join(", "), searched.len())
    } else {
        searched.join(", ")
    };
    // GH-557 review round 2, P1-2: name the REAL store rule — a plan's
    // state lives in the --cwd it was launched with, the plan's own `cwd:`
    // key, or the plan file's directory. The old text blamed a --cwd the
    // operator may never have passed.
    anyhow::anyhow!(
        "no state for plan \"{plan}\" (searched: {shown}); a plan's state lives in the store it was launched from — \
         the --cwd passed to conduct run, the plan's cwd: key, or the plan file's own directory"
    )
}

fn resolve_plan_name(repo_root: &Path, explicit: Option<&str>) -> Result<String> {
    if let Some(name) = explicit {
        return Ok(name.to_string());
    }

    // GH-557 review rounds 2–3: the auto-detection scope is the INVOKING
    // store (repo_root), falling back to another store only when exactly
    // one store on the machine holds any plan. Once more than one store
    // contributes, a destructive verb requires an explicit --plan — bare
    // auto-detection across lanes is how `abort` hits the wrong plan.
    // Corrupt state files propagate (they are errors, never "absent").
    let mut contributing: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for store in plan_stores(repo_root) {
        let conductor_dir = store.join(".edda").join("conductor");
        if !conductor_dir.exists() {
            continue;
        }
        let mut store_names = Vec::new();
        for entry in std::fs::read_dir(&conductor_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                if let Some(n) = entry.file_name().to_str() {
                    match load_state(&store, n) {
                        Ok(Some(_)) => store_names.push(n.to_string()),
                        Ok(None) => continue,
                        Err(e) => {
                            return Err(e.context(format!(
                                "plan \"{n}\" state in {} is unreadable",
                                store.display()
                            )))
                        }
                    }
                }
            }
        }
        if !store_names.is_empty() {
            contributing.push((store, store_names));
        }
    }

    if contributing.is_empty() {
        bail!("No plans found. Specify --plan <name>.");
    }
    if contributing.len() > 1 {
        let shown = contributing
            .iter()
            .map(|(s, ns)| format!("{} ({})", s.display(), ns.join("/")))
            .collect::<Vec<_>>()
            .join(", ");
        bail!("Plans found in multiple stores: {shown}. Specify --plan <name>.");
    }
    let names = &contributing[0].1;

    match names.len() {
        0 => bail!("No plans found. Specify --plan <name>."),
        1 => Ok(names[0].clone()),
        _ => bail!(
            "Multiple plans found: {}. Use --plan to specify.",
            names.join(", ")
        ),
    }
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
    use edda_conductor::state::persist::save_state;

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

    /// GH-557 harness: a real git repo plus one worktree, with a conductor
    /// state for `plan-x` fabricated inside the worktree's `.edda` — the
    /// exact shape of a plan launched with `--cwd <worktree>`.
    fn repo_with_worktree_state() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let wt = dir.path().join("wt");
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str], cwd: &std::path::Path| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"], &root);
        std::fs::write(root.join("f.txt"), "x").unwrap();
        git(&["add", "."], &root);
        git(
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                "init",
            ],
            &root,
        );
        git(
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "wt-branch",
                wt.to_str().unwrap(),
            ],
            &root,
        );

        let plan_yaml = "name: plan-x\nphases:\n  - id: a\n    prompt: x\n";
        let plan = parse_plan(plan_yaml).unwrap();
        let state = edda_conductor::state::machine::PlanState::from_plan(&plan, "plan-x.yaml");
        let state_dir = wt.join(".edda").join("conductor").join("plan-x");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join("state.json"),
            serde_json::to_string_pretty(&state).unwrap(),
        )
        .unwrap();
        (dir, wt)
    }

    /// GH-557 regression: a plan state that lives in a worktree is acted on
    /// by the recovery verbs from the MAIN repo cwd — the exact dead end
    /// from the issue (`conduct run` resumes it, its own message names
    /// retry/skip/abort, and those answered "no state for plan").
    ///
    /// Verified to FAIL before the fix: with store resolution reduced to the
    /// repo root, `retry` errors with "no state for plan \"plan-x\"".
    #[test]
    fn retry_resolves_state_living_in_a_worktree() {
        let (_dir, wt) = repo_with_worktree_state();
        let root = _dir.path().join("repo");

        // The old failure mode, asserted directly.
        assert!(
            load_state(&root, "plan-x").unwrap().is_none(),
            "precondition: the state is NOT in the repo root store"
        );

        // A failed phase, as the blocked runner leaves it.
        let mut state = load_state(&wt, "plan-x").unwrap().unwrap();
        state.phases[0].status = PhaseStatus::Failed;
        state.plan_status = PlanStatus::Blocked;
        save_state(&wt, &state).unwrap();

        retry(&root, "a", Some("plan-x")).unwrap();

        let reloaded = load_state(&wt, "plan-x").unwrap().unwrap();
        assert_eq!(reloaded.phases[0].status, PhaseStatus::Pending);
        assert_eq!(reloaded.plan_status, PlanStatus::Running);
        // The fix never writes a duplicate store in the repo root.
        assert!(
            !root.join(".edda").join("conductor").join("plan-x").exists(),
            "the fix must act in place, not fork the state into the repo root"
        );
    }

    /// GH-557 (independent-review P1-4): `status` itself must list and
    /// resolve worktree-launched plans — tested through the same code path
    /// the verb runs, not a helper that did not exist before the fix.
    #[test]
    fn status_lists_and_resolves_worktree_launch_plans() {
        let (_dir, wt) = repo_with_worktree_state();
        let root = _dir.path().join("repo");

        // Unnamed listing: the worktree plan must appear, with provenance
        // naming the exact worktree path (not a substring accident).
        let text = status_impl(&root, None, false).unwrap();
        assert!(text.contains("plan-x"), "{text}");
        let wt_normalized = wt.to_string_lossy().replace('\\', "/");
        assert!(
            text.contains(&wt_normalized),
            "listing must show the worktree store path: {text}"
        );

        // Named: resolves into the worktree store and renders the state.
        let named = status_impl(&root, Some("plan-x"), false).unwrap();
        assert!(named.contains("plan-x"), "{named}");
        assert!(named.contains("Pending"), "{named}");

        // Unknown plan: honest emptiness, not another store's data.
        let missing = status_impl(&root, Some("plan-y"), false).unwrap();
        assert!(missing.contains("no state file found"), "{missing}");
    }

    /// GH-557 (independent-review P1-4): skip and abort act on the
    /// worktree store in place.
    #[test]
    fn skip_and_abort_act_on_worktree_store_from_repo_root() {
        let (_dir, wt) = repo_with_worktree_state();
        let root = _dir.path().join("repo");

        let mut state = load_state(&wt, "plan-x").unwrap().unwrap();
        state.phases[0].status = PhaseStatus::Failed;
        state.plan_status = PlanStatus::Blocked;
        save_state(&wt, &state).unwrap();

        skip(&root, "a", Some("lane handed off"), Some("plan-x")).unwrap();
        let reloaded = load_state(&wt, "plan-x").unwrap().unwrap();
        assert_eq!(reloaded.phases[0].status, PhaseStatus::Skipped);
        assert_eq!(
            reloaded.phases[0].skip_reason.as_deref(),
            Some("lane handed off")
        );

        abort(&root, Some("plan-x")).unwrap();
        let reloaded = load_state(&wt, "plan-x").unwrap().unwrap();
        assert_eq!(reloaded.plan_status, PlanStatus::Aborted);
        assert!(!root.join(".edda").join("conductor").join("plan-x").exists());
    }

    /// GH-557: a corrupt state file is an ERROR, not "no state" — swallowing
    /// it could retarget a mutating verb onto a stale same-name state in
    /// another store (independent-review P1-1).
    #[test]
    fn resolve_plan_store_propagates_corrupt_state_as_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(root.join(".edda").join("conductor").join("plan-x")).unwrap();
        std::fs::write(
            root.join(".edda")
                .join("conductor")
                .join("plan-x")
                .join("state.json"),
            b"{corrupt",
        )
        .unwrap();

        let err = resolve_plan_store(&root, "plan-x").unwrap_err();
        assert!(
            err.to_string().contains("unreadable"),
            "corrupt state must surface as unreadable: {err}"
        );
    }

    /// GH-557 (independent-review round 3, P0-2): `conduct run` records the
    /// store it used in the registry, and a plan launched from a plain
    /// directory — the plan YAML's own folder, neither the repo root nor a
    /// worktree — is then recoverable from the repo root. This is the
    /// reported incident's actual shape (parallel-wave launches with no
    /// --cwd, plan YAMLs live outside the repo).
    #[test]
    fn registry_recorded_store_is_recoverable() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let plans_dir = dir.path().join("scratchpad");
        std::fs::create_dir_all(root.join(".edda").join("conductor")).unwrap();
        std::fs::create_dir_all(&plans_dir).unwrap();

        let plan = parse_plan("name: plan-x\nphases:\n  - id: a\n    prompt: x\n").unwrap();
        let mut state = edda_conductor::state::machine::PlanState::from_plan(&plan, "plan-x.yaml");
        state.phases[0].status = PhaseStatus::Failed;
        state.plan_status = PlanStatus::Blocked;
        let store = plans_dir.clone();
        let state_dir = store.join(".edda").join("conductor").join("plan-x");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join("state.json"),
            serde_json::to_string_pretty(&state).unwrap(),
        )
        .unwrap();

        // What conduct run writes at start (best-effort registry record).
        registry_record(&root, "plan-x", &store);

        // The store is not in any scannable location — only the registry
        // knows it.
        assert!(
            load_state(&root, "plan-x").unwrap().is_none(),
            "precondition: the store is outside the repo and its worktrees"
        );

        retry(&root, "a", Some("plan-x")).unwrap();

        let reloaded = load_state(&store, "plan-x").unwrap().unwrap();
        assert_eq!(reloaded.phases[0].status, PhaseStatus::Pending);
    }

    /// GH-557 (independent-review round 3, P0-3): bare destructive verbs
    /// refuse once more than one store contributes plans — auto-detection
    /// must never silently resolve across lanes.
    #[test]
    fn auto_detect_refuses_when_multiple_stores_contribute() {
        let (_dir, _wt) = repo_with_worktree_state();
        let root = _dir.path().join("repo");

        // A second plan in the repo-root store: now two stores contribute
        // (root holds plan-y, the worktree holds plan-x) and bare
        // destructive verbs must refuse rather than pick a lane.
        let plan_y = parse_plan("name: plan-y\nphases:\n  - id: a\n    prompt: x\n").unwrap();
        let state_y = edda_conductor::state::machine::PlanState::from_plan(&plan_y, "y.yaml");
        let y_dir = root.join(".edda").join("conductor").join("plan-y");
        std::fs::create_dir_all(&y_dir).unwrap();
        std::fs::write(
            y_dir.join("state.json"),
            serde_json::to_string_pretty(&state_y).unwrap(),
        )
        .unwrap();

        let err = resolve_plan_name(&root, None).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("multiple stores"),
            "auto-detection must refuse across stores: {text}"
        );
        // An explicit --plan still works across stores.
        assert_eq!(resolve_plan_name(&root, Some("plan-x")).unwrap(), "plan-x");
    }

    /// GH-557 (independent-review round 3, P1-1): `status <plan> --json`
    /// keeps the pre-GH-557 "null" contract for a named-but-missing plan
    /// (exit 0, body "null"), while text mode stays forgiving.
    #[test]
    fn status_json_named_missing_prints_null() {
        let (_dir, _wt) = repo_with_worktree_state();
        let root = _dir.path().join("repo");
        let out = status_impl(&root, Some("plan-y"), true).unwrap();
        assert_eq!(out.trim(), "null", "{out}");
    }

    /// GH-557 (independent-review round 4, P0-3): the registry-root chain
    /// must include the invoking shell's store even after the run cwd has
    /// grown its own `.edda` — find_root(run_cwd) returns the run cwd
    /// itself at that point, and entries filed only there are unreadable.
    #[test]
    fn registry_roots_cover_the_invoking_store_after_edda_exists() {
        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("repo");
        let run_cwd = dir.path().join("scratchpad");
        std::fs::create_dir_all(shell.join(".edda")).unwrap();
        std::fs::create_dir_all(run_cwd.join(".edda")).unwrap();

        let roots = registry_roots_for(&run_cwd, &shell);
        assert!(
            roots.contains(&shell),
            "invoking shell's store must be a registry root: {roots:?}"
        );
    }

    /// GH-557 (independent-review round 4, P0-3): two plans launched from
    /// the same external plan-YAML directory — the parallel-wave shape the
    /// issue reports (wave-b-par-b4 AND wave-b-par-b5) — must BOTH be
    /// recoverable after the first run created `.edda` there.
    #[test]
    fn second_plan_in_same_external_store_is_recoverable() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let plans_dir = dir.path().join("scratchpad");
        std::fs::create_dir_all(root.join(".edda").join("conductor")).unwrap();
        std::fs::create_dir_all(&plans_dir).unwrap();

        for name in ["plan-x", "plan-y"] {
            let yaml = format!("name: {name}\nphases:\n  - id: a\n    prompt: x\n");
            let plan = parse_plan(&yaml).unwrap();
            let mut state = edda_conductor::state::machine::PlanState::from_plan(
                &plan,
                &format!("{name}.yaml"),
            );
            state.phases[0].status = PhaseStatus::Failed;
            state.plan_status = PlanStatus::Blocked;
            let state_dir = plans_dir.join(".edda").join("conductor").join(name);
            std::fs::create_dir_all(&state_dir).unwrap();
            std::fs::write(
                state_dir.join("state.json"),
                serde_json::to_string_pretty(&state).unwrap(),
            )
            .unwrap();
        }

        // Two runs from the same plans_dir: after the first, scratchpad has
        // .edda, so find_root(run_cwd) == scratchpad. The chain records into
        // every candidate root, so both entries land somewhere the verbs read.
        for name in ["plan-x", "plan-y"] {
            for root in registry_roots_for(&plans_dir, &root) {
                registry_record(&root, name, &plans_dir);
            }
        }

        // The repo-root registry (the one verbs scan) knows both plans.
        let registry = registry_read(&root).unwrap();
        assert!(registry.contains_key("plan-x"), "{registry:?}");
        assert!(registry.contains_key("plan-y"), "{registry:?}");

        retry(&root, "a", Some("plan-y")).unwrap();
        let reloaded = load_state(&plans_dir, "plan-y").unwrap().unwrap();
        assert_eq!(reloaded.phases[0].status, PhaseStatus::Pending);
    }

    /// GH-557 (independent-review round 4, P1-3): one corrupt state file
    /// must not take down the overview of every healthy lane — the
    /// read-only listing warns and skips; mutating verbs still error.
    #[test]
    fn status_listing_survives_a_corrupt_state_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let conductor = root.join(".edda").join("conductor");
        std::fs::create_dir_all(conductor.join("plan-healthy")).unwrap();
        std::fs::create_dir_all(conductor.join("plan-corrupt")).unwrap();

        let plan = parse_plan("name: plan-healthy\nphases:\n  - id: a\n    prompt: x\n").unwrap();
        let state = edda_conductor::state::machine::PlanState::from_plan(&plan, "h.yaml");
        std::fs::write(
            conductor.join("plan-healthy").join("state.json"),
            serde_json::to_string_pretty(&state).unwrap(),
        )
        .unwrap();
        std::fs::write(
            conductor.join("plan-corrupt").join("state.json"),
            b"{corrupt",
        )
        .unwrap();

        let text = status_impl(&root, None, false).unwrap();
        assert!(
            text.contains("plan-healthy"),
            "healthy lanes must still be listed: {text}"
        );
        assert!(
            !text.contains("plan-corrupt"),
            "the corrupt entry is omitted (warned on stderr), not listed as healthy: {text}"
        );
    }

    /// GH-557 (independent-review round 4, P1-1): the unnamed listing must
    /// include registry-referenced plans — the literally-reported symptom
    /// ("status never lists wave-b-par-b4/b5").
    #[test]
    fn status_listing_includes_registry_referenced_plans() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let plans_dir = dir.path().join("scratchpad");
        std::fs::create_dir_all(root.join(".edda").join("conductor")).unwrap();
        std::fs::create_dir_all(&plans_dir).unwrap();

        let plan = parse_plan("name: plan-x\nphases:\n  - id: a\n    prompt: x\n").unwrap();
        let state = edda_conductor::state::machine::PlanState::from_plan(&plan, "x.yaml");
        let state_dir = plans_dir.join(".edda").join("conductor").join("plan-x");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join("state.json"),
            serde_json::to_string_pretty(&state).unwrap(),
        )
        .unwrap();
        registry_record(&root, "plan-x", &plans_dir);

        let text = status_impl(&root, None, false).unwrap();
        assert!(
            text.contains("plan-x") && text.contains("scratchpad"),
            "registry-referenced plans must be listed with provenance: {text}"
        );
    }

    /// GH-557: with no matching state anywhere, the error names the searched
    /// stores (capped) instead of the bare "no state for plan".
    #[test]
    fn no_state_error_names_the_searched_stores() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let err = no_state_error(&root, "plan-x");
        let text = err.to_string();
        assert!(text.contains("searched:"), "{text}");
        assert!(text.contains("plan-x"), "{text}");
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
}
