//! GH-573: `edda fleet watch` — the external observer that notices a lane died.
//!
//! A lane that dies abnormally cannot report that it died (#564 covers only
//! graceful terminals). The one observable shape of an abnormal death is the
//! absence of the lane's heartbeat, judged by something that outlives the
//! lane. This verb is that observer. An existing scheduler may call it
//! periodically; the verb installs none.
//!
//! ## The verdict is an AND, never the heartbeat alone
//!
//! A lane is **orphaned** only when all of these hold:
//!
//! 1. its heartbeat is stale under the one shared liveness criterion
//!    (`edda_bridge_claude::peers::liveness_from_heartbeat`, re-exported as
//!    `peers::liveness_from_heartbeat` — the rule `edda peers`,
//!    `edda claim check` and `edda conduct status` read), and
//! 2. its work has **no terminal record** for (plan, phase) in the workspace
//!    ledger or the conductor plan state, and
//! 3. no board claim that still stands holds it.
//!
//! `docs/fleet/rules.md` R3/R17 state the same rule for the operator:
//! heartbeat absence is a hint, never a death verdict. A normally finished
//! lane also ages out of its heartbeat, so condition 2 is what stops the false
//! positive; condition 3 is what stops us taking over a live peer's work.
//!
//! ## Recovery is bounded, idempotent, and dry-run by default
//!
//! `--apply` runs the issue's ordered recovery for each orphan: write the
//! terminal record (workspace ledger, plus the conductor phase the runner
//! would otherwise leave `Running`), release the dead session's claim, then
//! redispatch while the per-(plan, phase) redispatch count is under the cap,
//! or stop-loss with a readable reason. The count is read back from the
//! ledger's own `fleet_watch` notes — there is no second state system. The
//! terminal record makes the lane `finished` on the next run, so a second
//! invocation with no state change is a no-op.
//!
//! Nothing here starts a loop or launches an agent. Redispatch records the
//! bounded decision and re-arms the conductor phase through the existing
//! `retry` transition, so the existing `edda conduct run` resume picks the
//! phase up. A stateless `edda dispatch` lane has no recorded plan/brief to
//! re-issue (`fleet.lane-dispatch` keeps dispatch stateless), so its redispatch
//! is a recorded stop-loss instead — honest, not silently re-run.

use clap::Args;
use edda_bridge_claude::peers::{self, SessionLiveness};
use edda_conductor::state::machine::{ErrorInfo, ErrorType, PhaseStatus, PlanStatus};
use edda_conductor::state::persist::{load_state, state_path, update_state};
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::tasks::TaskStatus;
use edda_ledger::Ledger;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

use crate::cmd_conduct::store;

/// Default redispatch bound per (plan, phase) when the ledger has no
/// `fleet.watch.max-redispatch` decision. One retry: the issue asks for a
/// bounded ladder, not an unattended loop.
const DEFAULT_MAX_REDISPATCH: u32 = 1;

/// Ledger decision key overriding [`DEFAULT_MAX_REDISPATCH`].
const MAX_REDISPATCH_KEY: &str = "fleet.watch.max-redispatch";

/// Payload key this verb embeds in its notes, so the redispatch count and the
/// per-session terminal record are read back from the ledger itself.
const PAYLOAD_KEY: &str = "fleet_watch";

#[derive(Args, Debug, Clone)]
pub struct WatchArgs {
    /// Perform the bounded recovery (default: report only)
    #[arg(long)]
    pub apply: bool,
    /// Emit machine-readable JSON
    #[arg(long)]
    pub json: bool,
    /// Stop-loss after this many redispatches per (plan, phase)
    #[arg(long)]
    pub max_redispatch: Option<u32>,
}

// ── Verdict (pure) ───────────────────────────────────────────────────────────

/// What one lane is, judged by the AND in the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LaneVerdict {
    /// Heartbeat inside the shared staleness window. Never touched.
    Live,
    /// Stale heartbeat, but a claim that still stands holds the lane.
    Claimed,
    /// Stale heartbeat, but the work already has a terminal record.
    Finished,
    /// Stale heartbeat, no terminal record, no standing claim: recoverable.
    Orphan,
    /// Stale heartbeat, but the terminal record could not be read: never
    /// declared dead on an unreadable surface (fail closed).
    Unjudged,
}

/// The whole classification rule, pure over three facts.
///
/// `terminal` is `Some(true)` when a terminal record exists, `Some(false)`
/// when a readable surface says there is none, and `None` when no surface
/// could be read — which must never become a death verdict.
pub(crate) fn verdict(stale: bool, terminal: Option<bool>, claimed: bool) -> LaneVerdict {
    if !stale {
        return LaneVerdict::Live;
    }
    if claimed {
        return LaneVerdict::Claimed;
    }
    match terminal {
        Some(true) => LaneVerdict::Finished,
        Some(false) => LaneVerdict::Orphan,
        None => LaneVerdict::Unjudged,
    }
}

// ── Recovery plan (pure) ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecoveryStep {
    /// Append the terminal record (and mark a Running/Checking phase Stale).
    WriteTerminal,
    /// Release the dead session's board claim, if one stands.
    ReleaseClaim,
    /// Record the redispatch and re-arm the conductor phase.
    Redispatch,
    /// Record why no further redispatch happened.
    StopLoss,
}

/// The issue's ordered recovery, bounded by `cap` prior redispatches.
/// Terminal before claim before redispatch, never reordered.
pub(crate) fn plan_recovery(prior_redispatches: u32, cap: u32) -> Vec<RecoveryStep> {
    let mut steps = vec![RecoveryStep::WriteTerminal, RecoveryStep::ReleaseClaim];
    if prior_redispatches < cap {
        steps.push(RecoveryStep::Redispatch);
    } else {
        steps.push(RecoveryStep::StopLoss);
    }
    steps
}

// ── Observation ──────────────────────────────────────────────────────────────

/// One lane heartbeat, resolved to the workspace store(s) its `project_id`
/// belongs to. Read-only.
struct Lane {
    session_id: String,
    label: String,
    plan: String,
    phase: String,
    attempt: u32,
    pid: Option<u32>,
    age_secs: u64,
    stale: bool,
    last_heartbeat: String,
    project_id: String,
    stores: Vec<PathBuf>,
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// Enumerate lane heartbeats — the one shared `SessionHeartbeat` surface, the
/// same files `edda peers` and `edda conduct status` read. Only entries the
/// runner stamps (`plan` set) are lanes; a hook-only Claude session leaves
/// `plan` empty and is not ours to judge.
fn collect_lanes(repo_root: &Path) -> Vec<Lane> {
    let mut stores = store::candidate_stores(repo_root);
    for (_, store_path) in store::discover_plans(repo_root).0 {
        if !stores
            .iter()
            .any(|s| store::normalize_store_path(s) == store::normalize_store_path(&store_path))
        {
            stores.push(store_path);
        }
    }

    let mut by_project: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for store_path in stores {
        let project_id = edda_store::project_id(&store_path);
        by_project.entry(project_id).or_default().push(store_path);
    }

    let now = peers::liveness::now_epoch();
    let mut lanes = Vec::new();
    for (project_id, project_stores) in &by_project {
        let state_dir = edda_store::project_dir(project_id).join("state");
        let Ok(entries) = std::fs::read_dir(&state_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("session.") || !name.ends_with(".json") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(hb) = serde_json::from_str::<peers::SessionHeartbeat>(&content) else {
                continue;
            };
            let Some(plan) = hb.plan.as_deref().filter(|p| !p.is_empty()) else {
                continue;
            };
            // The shared criterion is the only judge of freshness; a second
            // parser or threshold here would be a second criterion for one
            // question (peers::liveness's one-criterion rule).
            let (age_secs, stale) = match peers::liveness_from_heartbeat(&hb, now) {
                SessionLiveness::Live { age_secs } => (age_secs, false),
                SessionLiveness::Stale { age_secs } => (age_secs, true),
                // Total over Live/Stale; this arm belongs to the file-level
                // classifier, which we never call.
                SessionLiveness::NoHeartbeat => continue,
            };
            lanes.push(Lane {
                session_id: hb.session_id,
                label: hb.label,
                plan: plan.to_string(),
                phase: hb.phase.unwrap_or_default(),
                attempt: hb.attempt.unwrap_or(1),
                pid: hb.pid,
                age_secs,
                stale,
                last_heartbeat: hb.last_heartbeat,
                project_id: project_id.clone(),
                stores: project_stores.clone(),
            });
        }
    }
    lanes.sort_by(|a, b| {
        a.session_id
            .cmp(&b.session_id)
            .then_with(|| a.plan.cmp(&b.plan))
            .then_with(|| a.phase.cmp(&b.phase))
    });
    lanes
}

/// Open a store's ledger read-only. `Ok(None)` = this store has no ledger at
/// all (not an edda workspace): absence is not an error, and the caller keeps
/// looking at the store's plan state.
fn store_ledger(store_path: &Path) -> anyhow::Result<Option<Ledger>> {
    if !store_path.join(".edda").join("ledger.db").exists() {
        return Ok(None);
    }
    Ledger::open_existing(store_path).map(Some)
}

fn is_terminal_conductor_status(status: &str) -> bool {
    matches!(status, "passed" | "failed" | "gate_timed_out")
}

/// `Ok(Some(true))` terminal, `Ok(Some(false))` readable and not terminal,
/// `Ok(None)` no ledger in this store.
fn ledger_terminal(store_path: &Path, lane: &Lane) -> anyhow::Result<Option<bool>> {
    let Some(ledger) = store_ledger(store_path)? else {
        return Ok(None);
    };
    for event in ledger.iter_events()? {
        let payload = &event.payload;
        if let Some(cp) = payload.get("conductor_phase") {
            if cp.get("plan_id").and_then(|v| v.as_str()) == Some(lane.plan.as_str())
                && cp.get("phase_id").and_then(|v| v.as_str()) == Some(lane.phase.as_str())
                && cp
                    .get("status")
                    .and_then(|v| v.as_str())
                    .is_some_and(is_terminal_conductor_status)
            {
                return Ok(Some(true));
            }
        }
        if let Some(fw) = payload.get(PAYLOAD_KEY) {
            if fw.get("action").and_then(|v| v.as_str()) == Some("terminal")
                && fw.get("plan").and_then(|v| v.as_str()) == Some(lane.plan.as_str())
                && fw.get("phase").and_then(|v| v.as_str()) == Some(lane.phase.as_str())
                && fw.get("session_id").and_then(|v| v.as_str()) == Some(lane.session_id.as_str())
            {
                return Ok(Some(true));
            }
        }
    }
    // The rail is a separate acceptance system and this verb never writes it,
    // but a done/failed rail task bound to this session is a terminal record
    // for the same work and must stop a false orphan.
    for view in ledger.task_views()? {
        if view.session_id.as_deref() == Some(lane.session_id.as_str())
            && matches!(view.status, TaskStatus::Done | TaskStatus::Failed)
        {
            return Ok(Some(true));
        }
    }
    Ok(Some(false))
}

/// `Ok(Some(true))` terminal, `Ok(Some(false))` state exists but records no
/// terminal for this work, `Ok(None)` no state for this plan here.
fn state_terminal(store_path: &Path, lane: &Lane) -> anyhow::Result<Option<bool>> {
    if edda_conductor::state::persist::validate_plan_name(&lane.plan).is_err() {
        return Ok(None);
    }
    let Some(state) = load_state(store_path, &lane.plan)? else {
        return Ok(None);
    };
    if matches!(
        state.plan_status,
        PlanStatus::Completed | PlanStatus::Aborted
    ) {
        return Ok(Some(true));
    }
    match state.phases.iter().find(|p| p.id == lane.phase) {
        // Pending/Running/Checking are the only non-terminal phase states: the
        // lane's work is not recorded finished. Passed/Failed/Skipped/Stale/
        // AwaitingVerdict/GateTimedOut all mean the phase's turn is over.
        Some(phase) => Ok(Some(!matches!(
            phase.status,
            PhaseStatus::Pending | PhaseStatus::Running | PhaseStatus::Checking
        ))),
        None => Ok(Some(false)),
    }
}

/// Is there a terminal record for this lane's work?
///
/// `Some(true)` yes; `Some(false)` every readable surface says no; `None`
/// nothing readable could be found — which is never a death verdict. A read
/// error is `None` too: an unreadable surface must not turn into a takeover.
fn terminal_record(lane: &Lane) -> Option<bool> {
    let mut judged = false;
    for store_path in &lane.stores {
        match ledger_terminal(store_path, lane) {
            Ok(Some(true)) => return Some(true),
            Ok(Some(false)) => judged = true,
            Ok(None) => {}
            Err(e) => {
                eprintln!(
                    "⚠ fleet watch: ledger at {} unreadable for lane {}: {e:#}",
                    store_path.display(),
                    lane.session_id
                );
                return None;
            }
        }
        match state_terminal(store_path, lane) {
            Ok(Some(true)) => return Some(true),
            Ok(Some(false)) => judged = true,
            Ok(None) => {}
            Err(e) => {
                eprintln!(
                    "⚠ fleet watch: plan state in {} unreadable for lane {}: {e:#}",
                    store_path.display(),
                    lane.session_id
                );
                return None;
            }
        }
    }
    if judged {
        Some(false)
    } else {
        None
    }
}

/// A claim that still stands for this lane's session. Read through the one
/// admission rule, so `fleet watch` can never open a surface `edda claim
/// check`/`edda dispatch --owns` still refuse a writer.
fn lane_claimed(lane: &Lane, claims: &[peers::ClaimEntry], now_epoch: u64) -> bool {
    claims.iter().any(|claim| {
        claim.session_id == lane.session_id
            && crate::claim_standing::claim_standing(&lane.project_id, claim, now_epoch)
                != crate::claim_standing::ClaimStanding::Expired
    })
}

// ── Recovery ─────────────────────────────────────────────────────────────────

/// What `--apply` did (or, in a dry run, what it would do) for one orphan.
#[derive(Debug, Clone, serde::Serialize)]
struct Recovered {
    session_id: String,
    plan: String,
    phase: String,
    steps: Vec<RecoveryStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resume: Option<String>,
    terminal_recorded: bool,
    claim_released: bool,
    redispatch_recorded: bool,
    stop_loss_recorded: bool,
}

/// The store the recovery writes into: the first one that is an edda
/// workspace. `terminal_record` returned `Some(false)`, so at least one store
/// had a readable surface; a store with neither a ledger nor a plan state
/// cannot hold the record.
fn writable_store(lane: &Lane) -> Option<&Path> {
    lane.stores
        .iter()
        .find(|s| s.join(".edda").exists())
        .map(PathBuf::as_path)
}

fn append_fleet_note(
    ledger: &Ledger,
    text: &str,
    tags: &[String],
    payload: serde_json::Value,
) -> anyhow::Result<()> {
    use anyhow::Context;
    let _lock = WorkspaceLock::acquire(&ledger.paths).context("acquiring workspace lock")?;
    let branch = ledger.head_branch().context("reading HEAD branch")?;
    let parent_hash = ledger
        .last_event_hash()
        .context("reading last event hash")?;
    let (safe_text, hits) = edda_core::secret_guard::redact(text);
    if !hits.is_empty() {
        eprintln!(
            "⚠ fleet watch: secret-guard redacted {} pattern(s) before writing the note",
            hits.len()
        );
    }
    let mut event = edda_core::event::new_note_event(
        &branch,
        parent_hash.as_deref(),
        "fleet watch",
        &safe_text,
        tags,
    )
    .context("building note event")?;
    event.payload[PAYLOAD_KEY] = payload;
    // Embedding the structured payload changes the body; re-hash exactly like
    // the conductor's phase notes do, or the append rejects the event.
    edda_core::event::finalize_event(&mut event).context("re-finalizing note event")?;
    ledger
        .append_event(&event)
        .context("appending note event")?;
    let _ = edda_derive::rebuild_branch(ledger, &branch);
    Ok(())
}

fn fleet_payload(action: &str, lane: &Lane, reason: Option<&str>) -> serde_json::Value {
    let mut value = serde_json::json!({
        "action": action,
        "plan": lane.plan,
        "phase": lane.phase,
        "session_id": lane.session_id,
        "attempt": lane.attempt,
    });
    if let Some(reason) = reason {
        value["reason"] = serde_json::Value::String(reason.to_string());
    }
    value
}

/// Step 1 — write the terminal record.
///
/// The lane died; its work must not stay `Running` forever. Two writes, both
/// into surfaces that already exist: a ledger note keyed by
/// (plan, phase, session) — the record that makes this lane terminal for the
/// next run — and, when the phase is still Running/Checking, the same
/// transition `detect_stale_phases` applies on resume ("phase was running
/// when conductor stopped"). That transition is assigned directly for the
/// same reason `detect_stale_phases` assigns it directly: `Checking → Stale`
/// has no edge in the state machine, so there is no `transition` call to make.
fn write_terminal(store_path: &Path, lane: &Lane) -> anyhow::Result<bool> {
    let reason = format!(
        "lane {} terminated abnormally: heartbeat stale for {}s with no terminal record",
        lane.session_id, lane.age_secs
    );
    let ledger = Ledger::open(store_path)?;
    let text = format!(
        "fleet watch: lane \"{}\" ({}/{}) terminated abnormally — {reason}",
        lane.session_id, lane.plan, lane.phase
    );
    append_fleet_note(
        &ledger,
        &text,
        &["fleet_watch".to_string(), "orphan".to_string()],
        fleet_payload("terminal", lane, Some(&reason)),
    )?;

    if edda_conductor::state::persist::validate_plan_name(&lane.plan).is_err()
        || !state_path(store_path, &lane.plan).is_file()
    {
        return Ok(false);
    }
    update_state(store_path, &lane.plan, |state| {
        let Some(phase) = state.phases.iter_mut().find(|p| p.id == lane.phase) else {
            return Ok(false);
        };
        if phase.status != PhaseStatus::Running && phase.status != PhaseStatus::Checking {
            return Ok(false);
        }
        phase.status = PhaseStatus::Stale;
        phase.error = Some(ErrorInfo {
            error_type: ErrorType::Timeout,
            message: "phase was running when conductor stopped; lane heartbeat expired with no \
                      terminal record (edda fleet watch)"
                .to_string(),
            retryable: true,
            check_index: None,
            timestamp: now_rfc3339(),
        });
        edda_conductor::state::derive::update_plan_status(state);
        Ok(true)
    })
}

/// Step 2 — release the dead session's claim, if one is on the board. A
/// session that never claimed is a no-op, so a second run writes nothing.
fn release_claim(lane: &Lane) -> bool {
    let Ok(claims) = crate::cmd_claim::read_active_claims(&lane.project_id) else {
        eprintln!(
            "⚠ fleet watch: board unreadable for project {}; claim not released",
            lane.project_id
        );
        return false;
    };
    if !claims.iter().any(|c| c.session_id == lane.session_id) {
        return false;
    }
    peers::write_unclaim(&lane.project_id, &lane.session_id);
    true
}

/// Prior redispatches this verb recorded for (plan, phase), read back from the
/// ledger notes themselves — the cap needs no side file.
fn prior_redispatches(lane: &Lane) -> u32 {
    let mut count = 0;
    for store_path in &lane.stores {
        let Ok(Some(ledger)) = store_ledger(store_path) else {
            continue;
        };
        let Ok(events) = ledger.iter_events() else {
            continue;
        };
        for event in events {
            let Some(fw) = event.payload.get(PAYLOAD_KEY) else {
                continue;
            };
            if fw.get("action").and_then(|v| v.as_str()) == Some("redispatch")
                && fw.get("plan").and_then(|v| v.as_str()) == Some(lane.plan.as_str())
                && fw.get("phase").and_then(|v| v.as_str()) == Some(lane.phase.as_str())
            {
                count += 1;
            }
        }
    }
    count
}

/// Step 3a — record the redispatch and re-arm the conductor phase through the
/// existing retry transition (Running/Checking → Stale → Pending), so the
/// existing `edda conduct run <plan_file> --cwd <store>` resume re-runs it.
/// Returns the resume hint, or `None` when there is no plan state to re-arm
/// (a stateless dispatch lane), which the caller records as a stop-loss.
fn redispatch(store_path: &Path, lane: &Lane) -> anyhow::Result<Option<String>> {
    if !state_path(store_path, &lane.plan).is_file() {
        return Ok(None);
    }
    let plan_file = update_state(store_path, &lane.plan, |state| {
        let plan_file = state.plan_file.clone();
        let Some(current) = state
            .phases
            .iter()
            .find(|p| p.id == lane.phase)
            .map(|p| p.status)
        else {
            return Ok(None);
        };
        // Through the state machine's own retry edges (`Stale → Pending`,
        // `Failed → Pending`), exactly as `edda conduct retry` does: that is
        // what clears the transient gate/verdict state and bumps the state
        // version the runner reconciles against. A phase that is not on one of
        // those edges (e.g. already Pending after a previous stop-loss) is
        // left alone rather than forced.
        if current != PhaseStatus::Stale && current != PhaseStatus::Failed {
            return Ok(None);
        }
        edda_conductor::state::machine::transition(
            state,
            &lane.phase,
            current,
            PhaseStatus::Pending,
            None,
        )?;
        edda_conductor::state::derive::update_plan_status(state);
        Ok(Some(plan_file))
    })?;
    let Some(plan_file) = plan_file else {
        return Ok(None);
    };

    let ledger = Ledger::open(store_path)?;
    let text = format!(
        "fleet watch: redispatch lane \"{}\" ({}/{}) — the phase was re-armed to Pending; \
         read the worktree's current state and continue on top of it, do not redo it",
        lane.session_id, lane.plan, lane.phase
    );
    append_fleet_note(
        &ledger,
        &text,
        &["fleet_watch".to_string(), "redispatch".to_string()],
        fleet_payload("redispatch", lane, None),
    )?;

    let plan = if plan_file.is_empty() || !Path::new(&plan_file).is_absolute() {
        "<plan.yaml>".to_string()
    } else {
        plan_file
    };
    Ok(Some(format!(
        "edda conduct run {plan} --cwd {}",
        store_path.display()
    )))
}

/// Step 3b — record why the ladder stopped.
fn record_stop_loss(store_path: &Path, lane: &Lane, reason: &str) -> anyhow::Result<()> {
    let ledger = Ledger::open(store_path)?;
    let text = format!(
        "fleet watch: stop-loss for lane \"{}\" ({}/{}): {reason}",
        lane.session_id, lane.plan, lane.phase
    );
    append_fleet_note(
        &ledger,
        &text,
        &["fleet_watch".to_string(), "stop_loss".to_string()],
        fleet_payload("stop_loss", lane, Some(reason)),
    )
}

/// Apply the planned steps to one orphan.
fn recover(lane: &Lane, steps: &[RecoveryStep], cap: u32) -> anyhow::Result<Recovered> {
    let mut out = Recovered {
        session_id: lane.session_id.clone(),
        plan: lane.plan.clone(),
        phase: lane.phase.clone(),
        steps: steps.to_vec(),
        reason: None,
        resume: None,
        terminal_recorded: false,
        claim_released: false,
        redispatch_recorded: false,
        stop_loss_recorded: false,
    };
    let Some(store_path) = writable_store(lane) else {
        out.reason = Some(format!(
            "no edda workspace among {} to record into",
            lane.stores
                .iter()
                .map(|s| s.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        return Ok(out);
    };
    if steps.contains(&RecoveryStep::WriteTerminal) {
        write_terminal(store_path, lane)?;
        out.terminal_recorded = true;
    }
    if steps.contains(&RecoveryStep::ReleaseClaim) {
        out.claim_released = release_claim(lane);
    }
    match steps.last() {
        Some(RecoveryStep::Redispatch) => match redispatch(store_path, lane)? {
            Some(resume) => {
                out.redispatch_recorded = true;
                out.resume = Some(resume);
            }
            None => {
                let reason = format!(
                    "no recorded plan/brief for {}/{}; redispatch belongs to the caller",
                    lane.plan, lane.phase
                );
                record_stop_loss(store_path, lane, &reason)?;
                out.stop_loss_recorded = true;
                out.reason = Some(reason);
            }
        },
        Some(RecoveryStep::StopLoss) => {
            let reason = format!(
                "redispatch cap reached ({cap} prior redispatch(es) for {}/{})",
                lane.plan, lane.phase
            );
            record_stop_loss(store_path, lane, &reason)?;
            out.stop_loss_recorded = true;
            out.reason = Some(reason);
        }
        _ => {}
    }
    Ok(out)
}

// ── Entry point ──────────────────────────────────────────────────────────────

/// The redispatch bound: `--max-redispatch`, else the ledger decision
/// `fleet.watch.max-redispatch`, else [`DEFAULT_MAX_REDISPATCH`]. A ledger that
/// cannot be read never fails the command — the default applies, the same
/// discipline `fleet health` uses for its thresholds.
fn resolve_cap(repo_root: &Path, flag: Option<u32>) -> u32 {
    if let Some(value) = flag {
        return value;
    }
    if let Ok(ledger) = Ledger::open_existing(repo_root) {
        if let Ok(branch) = ledger.head_branch() {
            if let Ok(Some(decision)) = ledger.find_active_decision(&branch, MAX_REDISPATCH_KEY) {
                if let Ok(value) = decision.value.trim().parse::<u32>() {
                    return value;
                }
            }
        }
    }
    DEFAULT_MAX_REDISPATCH
}

#[derive(serde::Serialize)]
struct LaneJson {
    session_id: String,
    label: String,
    plan: String,
    phase: String,
    attempt: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    age_secs: u64,
    stale: bool,
    verdict: LaneVerdict,
    last_heartbeat: String,
}

#[derive(serde::Serialize)]
struct WatchReport {
    dry_run: bool,
    max_redispatch: u32,
    lane_count: usize,
    orphan_count: usize,
    lanes: Vec<LaneJson>,
    recovered: Vec<Recovered>,
}

fn render_text(report: &WatchReport) -> String {
    let mut out = format!(
        "fleet watch — {} lane(s), {} orphan(s){}\n",
        report.lane_count,
        report.orphan_count,
        if report.dry_run { " (dry run)" } else { "" }
    );
    for lane in &report.lanes {
        let mark = match lane.verdict {
            LaneVerdict::Live => "▶ live    ",
            LaneVerdict::Claimed => "▶ claimed ",
            LaneVerdict::Finished => "⏰ finished",
            LaneVerdict::Orphan => "⏰ ORPHAN  ",
            LaneVerdict::Unjudged => "?  unjudged",
        };
        let pid = lane
            .pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "?".into());
        out.push_str(&format!(
            "  {mark} {}  {}/{}  age={}s  pid={pid}\n",
            lane.session_id, lane.plan, lane.phase, lane.age_secs
        ));
    }
    for item in &report.recovered {
        let steps = item
            .steps
            .iter()
            .map(|s| format!("{s:?}"))
            .collect::<Vec<_>>()
            .join(" → ");
        out.push_str(&format!(
            "{}{} {}/{}: {steps}\n",
            if report.dry_run {
                "  would recover "
            } else {
                "  recovered "
            },
            item.session_id,
            item.plan,
            item.phase
        ));
        if let Some(reason) = &item.reason {
            out.push_str(&format!("    {reason}\n"));
        }
        if let Some(resume) = &item.resume {
            out.push_str(&format!("    resume: {resume}\n"));
        }
    }
    out
}

/// Collect the lanes, judge them, and (with `--apply`) run the bounded
/// recovery. No output; `run` renders the returned report, which keeps the
/// whole decision path callable from tests without capturing stdout.
fn build_report(args: &WatchArgs, repo_root: &Path) -> anyhow::Result<WatchReport> {
    let now = peers::liveness::now_epoch();
    let lanes = collect_lanes(repo_root);

    let mut claims_by_project: BTreeMap<String, Vec<peers::ClaimEntry>> = BTreeMap::new();
    for lane in &lanes {
        if claims_by_project.contains_key(&lane.project_id) {
            continue;
        }
        let claims = crate::cmd_claim::read_active_claims(&lane.project_id).unwrap_or_else(|e| {
            eprintln!(
                "⚠ fleet watch: board unreadable for project {}: {e:#}",
                lane.project_id
            );
            Vec::new()
        });
        claims_by_project.insert(lane.project_id.clone(), claims);
    }

    let cap = resolve_cap(repo_root, args.max_redispatch);
    let mut rows = Vec::new();
    let mut orphans = Vec::new();
    for lane in lanes {
        let claims = claims_by_project
            .get(&lane.project_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let claimed = lane_claimed(&lane, claims, now);
        let terminal = terminal_record(&lane);
        let judged = verdict(lane.stale, terminal, claimed);
        if judged == LaneVerdict::Orphan {
            orphans.push(lane);
        } else {
            rows.push((lane, judged));
        }
    }

    let mut recovered = Vec::new();
    let mut orphan_rows = Vec::new();
    for lane in orphans {
        let steps = plan_recovery(prior_redispatches(&lane), cap);
        if args.apply {
            recovered.push(recover(&lane, &steps, cap)?);
        } else {
            recovered.push(Recovered {
                session_id: lane.session_id.clone(),
                plan: lane.plan.clone(),
                phase: lane.phase.clone(),
                steps,
                reason: None,
                resume: None,
                terminal_recorded: false,
                claim_released: false,
                redispatch_recorded: false,
                stop_loss_recorded: false,
            });
        }
        orphan_rows.push(lane);
    }

    let mut lanes_json: Vec<LaneJson> = rows
        .iter()
        .map(|(lane, judged)| lane_json(lane, *judged))
        .collect();
    lanes_json.extend(
        orphan_rows
            .iter()
            .map(|lane| lane_json(lane, LaneVerdict::Orphan)),
    );
    lanes_json.sort_by(|a, b| {
        a.session_id
            .cmp(&b.session_id)
            .then_with(|| a.plan.cmp(&b.plan))
            .then_with(|| a.phase.cmp(&b.phase))
    });

    Ok(WatchReport {
        dry_run: !args.apply,
        max_redispatch: cap,
        lane_count: lanes_json.len(),
        orphan_count: orphan_rows.len(),
        lanes: lanes_json,
        recovered,
    })
}

/// `edda fleet watch` — detect orphaned lanes and, with `--apply`, recover
/// them within the bound. Read-only without `--apply`.
pub fn run(args: WatchArgs, repo_root: &Path) -> anyhow::Result<()> {
    let report = build_report(&args, repo_root)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_text(&report));
    }
    Ok(())
}

fn lane_json(lane: &Lane, judged: LaneVerdict) -> LaneJson {
    LaneJson {
        session_id: lane.session_id.clone(),
        label: lane.label.clone(),
        plan: lane.plan.clone(),
        phase: lane.phase.clone(),
        attempt: lane.attempt,
        pid: lane.pid,
        age_secs: lane.age_secs,
        stale: lane.stale,
        verdict: judged,
        last_heartbeat: lane.last_heartbeat.clone(),
    }
}

#[cfg(test)]
mod tests;
