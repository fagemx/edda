use crate::agent_kind::{build_launcher, AgentKind, LauncherOptions};
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use edda_bridge_claude::peers::{
    format_age, liveness_from_heartbeat, SessionHeartbeat, SessionLiveness,
};
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
use std::collections::HashMap;
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
            PlanState::from_plan(&plan, &absolute_plan_path(plan_file))
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
///
/// GH-567 adds a read-only lane half: the same call also joins the shared
/// session heartbeat surface (the one `edda peers` reads) so a
/// `edda dispatch` single-turn lane — which has no plan state of its own —
/// appears alongside the plan phases with its heartbeat age and pid.
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

    // Load every plan state once: the plan listing and the GH-567 lane join
    // must read one snapshot. A corrupt record degrades to a warning +
    // omission on the JSON overview (matching the pre-lane behaviour); a
    // named plan and the text overview propagate instead.
    let mut loaded: Vec<(String, PathBuf, PlanState)> = Vec::new();
    for (name, store) in &plans {
        match load_state(store, name) {
            Ok(Some(state)) => loaded.push((name.clone(), store.clone(), state)),
            Ok(None) => {
                if plan_name.is_none() {
                    eprintln!(
                        "⚠ plan \"{name}\" state in {} disappeared before load, omitted",
                        store.display()
                    );
                }
            }
            Err(e) => {
                if plan_name.is_some() || !json {
                    return Err(e);
                }
                eprintln!(
                    "⚠ plan \"{name}\" state in {} unreadable at load, omitted: {e:#}",
                    store.display()
                );
            }
        }
    }

    // Plan-recorded status per (plan, phase), so a stale lane can be told from
    // a finished one: the issue's "expired and no terminal state = suspected"
    // line, marked rather than guessed. Absent = the lane has no plan state.
    let mut phase_states: HashMap<(String, String), Option<PhaseStatus>> = HashMap::new();
    for (_, _, state) in &loaded {
        for ps in &state.phases {
            let key = (state.plan_name.clone(), ps.id.clone());
            match phase_states.get(&key) {
                None => {
                    phase_states.insert(key, Some(ps.status));
                }
                // Same-named plans in different stores are allowed to
                // disagree, and a lane heartbeat carries no store identity to
                // disambiguate: claim neither status rather than one plan's.
                Some(Some(existing)) if *existing == ps.status => {}
                Some(_) => {
                    phase_states.insert(key, None);
                }
            }
        }
    }
    let lanes = in_flight_lanes(repo_root, &plans, &phase_states, plan_name);

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
            match loaded.into_iter().next() {
                Some((_, store, state)) => {
                    // Additive field so a named-plan consumer keeps reading
                    // every pre-existing path (`plan_name`, `phases`, `store`).
                    let mut value = serde_json::to_value(row(&store, state))?;
                    if let Some(object) = value.as_object_mut() {
                        object.insert(
                            "lane_heartbeats".to_string(),
                            serde_json::to_value(
                                lanes.iter().map(LaneJson::from).collect::<Vec<_>>(),
                            )?,
                        );
                    }
                    out.push_str(&serde_json::to_string_pretty(&value)?);
                    out.push('\n');
                }
                None => out.push_str("null\n"),
            }
        } else {
            // Keep the top-level array stable; lane rows are appended and
            // tagged `kind: "lane"`, so a consumer keyed on `plan_name` sees
            // exactly the rows it saw before.
            let mut rows: Vec<serde_json::Value> = Vec::new();
            for (_, store, state) in loaded {
                rows.push(serde_json::to_value(row(&store, state))?);
            }
            for lane in &lanes {
                rows.push(serde_json::to_value(LaneJson::from(lane))?);
            }
            out.push_str(&serde_json::to_string_pretty(&rows)?);
            out.push('\n');
        }
    } else if loaded.is_empty() && lanes.is_empty() {
        out.push_str("No plans found.\n");
    } else if plan_name.is_some() {
        match loaded.first() {
            Some((_, store, state)) => {
                out.push_str(&format!("  Store: {}\n", store_label(store, repo_root)));
                out.push_str(&print_status_to_string(state));
            }
            None => out.push_str(&format!(
                "Plan \"{}\": no state file found\n",
                plan_name.unwrap_or_default()
            )),
        }
        out.push_str(&render_lanes_text(&lanes));
    } else {
        for (_, store, state) in &loaded {
            out.push_str(&format!("  Store: {}\n", store_label(store, repo_root)));
            out.push_str(&print_status_to_string(state));
        }
        out.push_str(&render_lanes_text(&lanes));
    }

    Ok(out)
}

/// The label `status` shows for a plan's store: the invocation root is named
/// rather than echoing a path the operator already knows.
fn store_label(store: &Path, repo_root: &Path) -> String {
    if store::normalize_store_path(store) == store::normalize_store_path(repo_root) {
        "(invocation root)".to_string()
    } else {
        store::normalize_store_path(store)
    }
}

/// One conductor lane's live observation (GH-567).
struct LaneEntry {
    session_id: String,
    label: String,
    plan: String,
    phase: String,
    phase_status: Option<PhaseStatus>,
    stage: Option<String>,
    attempt: Option<u32>,
    pid: Option<u32>,
    age_secs: u64,
    stale: bool,
    last_heartbeat: String,
}

/// Machine-readable lane row. `kind` distinguishes appended rows from plan
/// rows in the existing top-level array.
#[derive(serde::Serialize)]
struct LaneJson<'a> {
    kind: &'static str,
    session_id: &'a str,
    label: &'a str,
    plan: &'a str,
    phase: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    stage: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attempt: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase_status: Option<PhaseStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    age_secs: u64,
    stale: bool,
    last_heartbeat: &'a str,
}

impl<'a> From<&'a LaneEntry> for LaneJson<'a> {
    fn from(lane: &'a LaneEntry) -> Self {
        Self {
            kind: "lane",
            session_id: &lane.session_id,
            label: &lane.label,
            plan: &lane.plan,
            phase: &lane.phase,
            stage: lane.stage.as_deref(),
            attempt: lane.attempt,
            phase_status: lane.phase_status,
            pid: lane.pid,
            age_secs: lane.age_secs,
            stale: lane.stale,
            last_heartbeat: &lane.last_heartbeat,
        }
    }
}

/// Collect the conductor-lane heartbeats `status` shows (GH-567).
///
/// Read-only: enumerates the shared `SessionHeartbeat` surface (the one
/// `edda peers` reads — one liveness format, no new store) under every store
/// root a plan may live in — the candidate stores the plan verbs resolve plus
/// every registry-referenced store `discover_plans` followed — and keeps only
/// the entries the conductor runner stamps (`plan` set; a hook-only Claude
/// session leaves it empty).
///
/// Every such lane is shown. A live one, or one whose heartbeat aged past the
/// shared threshold, marked `stale`. An expired lane is never dropped: the
/// issue's whole point is that a lane which looks alive but is not stays
/// visible as *stale*, neither hidden nor declared dead. Reclaiming the stale
/// observations is the separate concern #573; this read model keeps them
/// honest in the meantime.
fn in_flight_lanes(
    repo_root: &Path,
    plans: &[(String, PathBuf)],
    phase_states: &HashMap<(String, String), Option<PhaseStatus>>,
    plan_filter: Option<&str>,
) -> Vec<LaneEntry> {
    let now = edda_bridge_claude::peers::liveness::now_epoch();
    let mut project_ids: Vec<String> = Vec::new();
    let mut stores = store::candidate_stores(repo_root);
    stores.extend(plans.iter().map(|(_, store)| store.clone()));
    for store_dir in stores {
        let project_id = edda_store::project_id(&store_dir);
        if !project_ids.contains(&project_id) {
            project_ids.push(project_id);
        }
    }

    let mut lanes = Vec::new();
    for project_id in &project_ids {
        let state_dir = edda_store::project_dir(project_id).join("state");
        let entries = match std::fs::read_dir(&state_dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("session.") || !name.ends_with(".json") {
                continue;
            }
            let content = match std::fs::read_to_string(entry.path()) {
                Ok(content) => content,
                Err(_) => continue,
            };
            let hb: SessionHeartbeat = match serde_json::from_str(&content) {
                Ok(hb) => hb,
                Err(_) => continue,
            };
            // Only conductor lanes carry `plan`; a plain hook session does not.
            let plan = match hb.plan.as_deref() {
                Some(plan) if !plan.is_empty() => plan.to_string(),
                _ => continue,
            };
            if plan_filter.is_some_and(|filter| filter != plan.as_str()) {
                continue;
            }
            let phase = hb.phase.clone().unwrap_or_default();
            // The shared criterion is the only judge of freshness; adding a
            // second parser or threshold here would be a second criterion for
            // the same question (liveness.rs's one-criterion rule).
            let (age_secs, stale) = match liveness_from_heartbeat(&hb, now) {
                SessionLiveness::Live { age_secs } => (age_secs, false),
                SessionLiveness::Stale { age_secs } => (age_secs, true),
                // `liveness_from_heartbeat` is total over Live/Stale; this arm
                // belongs to the file-level classifier, which we never call.
                SessionLiveness::NoHeartbeat => continue,
            };
            let phase_status = phase_states
                .get(&(plan.clone(), phase.clone()))
                .copied()
                .flatten();
            lanes.push(LaneEntry {
                session_id: hb.session_id,
                label: hb.label,
                plan,
                phase,
                phase_status,
                stage: hb.stage,
                attempt: hb.attempt,
                pid: hb.pid,
                age_secs,
                stale,
                last_heartbeat: hb.last_heartbeat,
            });
        }
    }
    lanes.sort_by(|a, b| {
        a.age_secs
            .cmp(&b.age_secs)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    lanes
}

fn render_lanes_text(lanes: &[LaneEntry]) -> String {
    if lanes.is_empty() {
        return String::new();
    }
    let mut out = format!("\nLanes ({}):\n", lanes.len());
    for lane in lanes {
        let icon = if lane.stale { "\u{23F0}" } else { "\u{25B6}" };
        let stage = lane.stage.as_deref().unwrap_or("?");
        let pid = lane
            .pid
            .map(|pid| pid.to_string())
            .unwrap_or_else(|| "?".into());
        // A stale lane whose plan already records a phase status is marked
        // with it, so a finished phase is distinguishable from the issue's
        // suspected-death case (expired and no terminal state).
        let suffix = if lane.stale {
            match lane.phase_status {
                Some(status) => format!(
                    "  stale (phase {status:?}; no heartbeat for {}s)",
                    lane.age_secs
                ),
                None => format!("  stale (no heartbeat for {}s)", lane.age_secs),
            }
        } else {
            String::new()
        };
        out.push_str(&format!(
            "  {icon} {:<28} {:<12} age={}  pid={pid}{suffix}\n",
            format!("{}/{}", lane.plan, lane.phase),
            stage,
            format_age(lane.age_secs)
        ));
    }
    out.push('\n');
    out
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
    println!("  resume: {}", resume_hint(&plan_file, &store));
    Ok(())
}

/// Absolute, cwd-independent spelling of the plan file for state memory.
///
/// The state's `plan_file` is displayed and re-used in recovery hints, so a
/// path left relative to the launch cwd would suggest a command that fails
/// from any other cwd (GH-557 review round 1 P1).
fn absolute_plan_path(plan_file: &Path) -> String {
    std::fs::canonicalize(plan_file)
        .or_else(|_| {
            if plan_file.is_absolute() {
                Ok(plan_file.to_path_buf())
            } else {
                std::env::current_dir().map(|cwd| cwd.join(plan_file))
            }
        })
        .unwrap_or_else(|_| plan_file.to_path_buf())
        .display()
        .to_string()
}

/// The resume command a recovery verb recommends. A relative plan path is
/// resolved against a launch cwd the state does not record, so it is printed
/// as a placeholder rather than as a command that would fail elsewhere.
fn resume_hint(plan_file: &str, store: &Path) -> String {
    let plan = if plan_file.is_empty() || !Path::new(plan_file).is_absolute() {
        "<plan.yaml>".to_string()
    } else {
        plan_file.to_string()
    };
    format!("`edda conduct run {plan} --cwd {}`", store.display())
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
