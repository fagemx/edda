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
//! 2. its work has **no terminal record**: no done/failed rail task bound to
//!    the session, no completed `#session_digest` note for it, no terminal
//!    phase/plan in the conductor plan state, and no `fleet_watch` terminal
//!    note this verb already wrote for that session, and
//! 3. no board claim that still stands holds it.
//!
//! `docs/fleet/rules.md` R3/R17 state the same rule for the operator:
//! heartbeat absence is a hint, never a death verdict. A normally finished
//! lane also ages out of its heartbeat, so condition 2 is what stops the false
//! positive; condition 3 is what stops us taking over a live peer's work.
//!
//! The terminal record is per-lane, not per-(plan, phase): a lane's identity is
//! its session, so a record that cannot name the session cannot mark *this*
//! lane terminal. That is why the conductor's own `conductor_phase` note is not
//! consulted — see `ledger_terminal`.
//!
//! The one shape the product cannot answer for is a **stateless
//! `edda dispatch` lane that recorded no claim lifecycle**: dispatch keeps no
//! plan state, and without an un-released claim or a completed session digest
//! there is no terminal record and no evidence the unit was ever recorded at
//! all. That lane is reported `unrecorded`, never recovered — R17 exactly:
//! one proof is not a verdict.
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

/// The plan name the runner stamps on an `edda dispatch` single-turn lane
/// (`crates/edda-conductor/src/runner/heartbeat.rs` via `cmd_dispatch`).
const DISPATCH_PLAN: &str = "dispatch";

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
    /// Stale heartbeat, and the product keeps no terminal state for this lane
    /// at all (a stateless `edda dispatch` lane that never recorded a claim
    /// lifecycle). Heartbeat absence is then a hint and nothing more
    /// (`docs/fleet/rules.md` R17): report it, never recover it.
    Unrecorded,
}

impl LaneVerdict {
    /// The one-line reason a lane is not actionable, for the text/JSON report.
    pub(crate) fn detail(self) -> Option<&'static str> {
        match self {
            LaneVerdict::Unjudged => Some("a surface could not be read; not judged"),
            LaneVerdict::Unrecorded => Some(
                "stateless dispatch lane with no recorded claim or completion; \
                 heartbeat absence is a hint, not a death verdict (R17)",
            ),
            _ => None,
        }
    }
}

/// What the terminal-record read concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalRecord {
    /// A terminal record exists for this lane's work.
    Recorded,
    /// Every readable surface says there is none, and the lane's work is a
    /// unit the product does keep terminal state for.
    Absent,
    /// Every readable surface says there is none, and the product keeps no
    /// terminal state for this lane at all (no plan state, no ledger note, no
    /// un-released claim): a stateless `edda dispatch` lane.
    NotKept,
    /// A surface could not be read; never a death verdict.
    Unreadable,
}

/// The whole classification rule, pure over three facts.
///
/// `claimed` is `Some(true/false)` from the board and `None` when the board
/// itself (or the path-conflict rule over it) could not be judged. `None` must
/// never become a death verdict: a surface we could not read is not evidence
/// that the work is unowned. `terminal` carries the same fail-closed
/// direction and adds the one case the product simply cannot answer for
/// ([`TerminalRecord::NotKept`]).
pub(crate) fn verdict(stale: bool, terminal: TerminalRecord, claimed: Option<bool>) -> LaneVerdict {
    if !stale {
        return LaneVerdict::Live;
    }
    match claimed {
        Some(true) => LaneVerdict::Claimed,
        // An unreadable board cannot say "nobody holds it" — the same
        // fail-closed direction `terminal` takes below.
        None => LaneVerdict::Unjudged,
        Some(false) => match terminal {
            TerminalRecord::Recorded => LaneVerdict::Finished,
            TerminalRecord::Absent => LaneVerdict::Orphan,
            TerminalRecord::NotKept => LaneVerdict::Unrecorded,
            TerminalRecord::Unreadable => LaneVerdict::Unjudged,
        },
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
    /// Files the lane touched, as its own heartbeat reports them. The only
    /// surface identity a dead lane has when it never held a claim; used for
    /// the live-peer path-conflict check.
    focus_files: Vec<String>,
}

impl Lane {
    /// The surfaces this lane was writing: its own recorded claim paths (when
    /// it held one) plus the heartbeat's focus files.
    fn work_paths(&self, claims: &[peers::ClaimEntry]) -> Vec<String> {
        let mut paths = self.focus_files.clone();
        for claim in claims {
            if claim.session_id == self.session_id {
                for path in &claim.paths {
                    if !paths.contains(path) {
                        paths.push(path.clone());
                    }
                }
            }
        }
        paths
    }
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
                focus_files: hb.focus_files,
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

/// The one terminal vocabulary for the surface that can answer the question:
/// the conductor plan state. A phase's turn is over unless it is `Pending`,
/// `Running` or `Checking`.
///
/// `Stale`, `Failed` and `GateTimedOut` are terminal **records**: the conductor
/// already recorded an outcome, so this observer never overrides one by taking
/// the work over. Re-arming a recorded outcome stays with `edda conduct retry`,
/// which walks the machine's own retry edges.
fn phase_status_is_terminal(status: PhaseStatus) -> bool {
    !matches!(
        status,
        PhaseStatus::Pending | PhaseStatus::Running | PhaseStatus::Checking
    )
}

/// The session-scoped terminal records — the ones that can name *this* lane.
///
/// The conductor's `conductor_phase` ledger note is deliberately **not** one of
/// them: its payload carries only `plan_id`/`phase_id`/`status`
/// (`crates/edda-conductor/src/runner/edda.rs`), no session or attempt, so a
/// note from an earlier attempt would mark a re-run attempt finished. The
/// default `on_fail: auto_retry` routes `Failed → Pending` and re-runs the
/// phase, so that shape is the norm, not an edge: an attempt that died after a
/// prior failure would never be recovered and its phase would stay `Running`
/// forever — exactly what GH-573 exists to fix (REVIEW round 3, finding 1).
/// The plan state is the authority for a plan lane, and it is attempt-aware.
///
/// `Ok(Some(true))` terminal, `Ok(Some(false))` readable and not terminal,
/// `Ok(None)` no ledger in this store.
fn ledger_terminal(store_path: &Path, lane: &Lane) -> anyhow::Result<Option<bool>> {
    let Some(ledger) = store_ledger(store_path)? else {
        return Ok(None);
    };
    for event in ledger.iter_events()? {
        let payload = &event.payload;
        if let Some(fw) = payload.get(PAYLOAD_KEY) {
            if fw.get("action").and_then(|v| v.as_str()) == Some("terminal")
                && fw.get("plan").and_then(|v| v.as_str()) == Some(lane.plan.as_str())
                && fw.get("phase").and_then(|v| v.as_str()) == Some(lane.phase.as_str())
                && fw.get("session_id").and_then(|v| v.as_str()) == Some(lane.session_id.as_str())
            {
                return Ok(Some(true));
            }
        }
        // A completed `edda dispatch` turn ingests its session digest. That
        // note names the session and is the product's completion record for a
        // lane that has no plan state of its own.
        if payload.get("source").and_then(|v| v.as_str()) == Some("bridge:session_digest")
            && payload.get("session_id").and_then(|v| v.as_str()) == Some(lane.session_id.as_str())
        {
            return Ok(Some(true));
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
        Some(phase) => Ok(Some(phase_status_is_terminal(phase.status))),
        // The state exists but knows nothing about this phase: nothing terminal
        // has been recorded for this work here.
        None => Ok(Some(false)),
    }
}

/// Is there a terminal record for this lane's work?
///
/// [`TerminalRecord::Unreadable`] (no readable surface at all, or a read error)
/// is never a death verdict: an unreadable surface must not turn into a
/// takeover. `has_open_claim` is whether the board still carries an
/// un-released claim for this session — for a stateless `edda dispatch` lane
/// that is the only thing that can make its work a unit the product records
/// state for at all; without it the lane is [`TerminalRecord::NotKept`].
fn terminal_record(lane: &Lane, has_open_claim: bool) -> TerminalRecord {
    let mut judged = false;
    for store_path in &lane.stores {
        match ledger_terminal(store_path, lane) {
            Ok(Some(true)) => return TerminalRecord::Recorded,
            Ok(Some(false)) => judged = true,
            Ok(None) => {}
            Err(e) => {
                eprintln!(
                    "⚠ fleet watch: ledger at {} unreadable for lane {}: {e:#}",
                    store_path.display(),
                    lane.session_id
                );
                return TerminalRecord::Unreadable;
            }
        }
        match state_terminal(store_path, lane) {
            Ok(Some(true)) => return TerminalRecord::Recorded,
            Ok(Some(false)) => judged = true,
            Ok(None) => {}
            Err(e) => {
                eprintln!(
                    "⚠ fleet watch: plan state in {} unreadable for lane {}: {e:#}",
                    store_path.display(),
                    lane.session_id
                );
                return TerminalRecord::Unreadable;
            }
        }
    }
    if !judged {
        return TerminalRecord::Unreadable;
    }
    if lane.plan == DISPATCH_PLAN && !has_open_claim {
        TerminalRecord::NotKept
    } else {
        TerminalRecord::Absent
    }
}

/// A claim that still stands for this lane's session, or a live peer holding
/// the surfaces this lane was writing. Read through the one admission rule
/// (`claim_standing`) and the one intersection rule (`cmd_claim::check`), so
/// `fleet watch` can never open a surface `edda claim check`/`edda dispatch
/// --owns` still refuse a writer. A rule error fails closed.
fn lane_claimed(lane: &Lane, claims: &[peers::ClaimEntry], now_epoch: u64) -> bool {
    let stands = |claim: &peers::ClaimEntry| {
        crate::claim_standing::claim_standing(&lane.project_id, claim, now_epoch)
            != crate::claim_standing::ClaimStanding::Expired
    };
    if claims
        .iter()
        .any(|claim| claim.session_id == lane.session_id && stands(claim))
    {
        return true;
    }
    let others: Vec<peers::ClaimEntry> = claims
        .iter()
        .filter(|claim| claim.session_id != lane.session_id && stands(claim))
        .cloned()
        .collect();
    if others.is_empty() {
        return false;
    }
    let paths = lane.work_paths(claims);
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    if refs.is_empty() {
        return false;
    }
    match crate::cmd_claim::check(&others, &refs) {
        Ok(report) => !report.conflicts.is_empty() || !report.unjudgeable_claims.is_empty(),
        Err(_) => true,
    }
}

mod recover;
use recover::{prior_redispatches, recover, Recovered};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'static str>,
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
            LaneVerdict::Unrecorded => "?  norecord",
        };
        let pid = lane
            .pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "?".into());
        let detail = lane
            .detail
            .map(|detail| format!("  ({detail})"))
            .unwrap_or_default();
        out.push_str(&format!(
            "  {mark} {}  {}/{}  age={}s  pid={pid}{detail}\n",
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

    // `None` records "this board could not be read", which is a different fact
    // from "the board is empty": an unreadable board must not read as "nobody
    // holds it" (the contract `read_active_claims` documents).
    let mut claims_by_project: BTreeMap<String, Option<Vec<peers::ClaimEntry>>> = BTreeMap::new();
    for lane in &lanes {
        if claims_by_project.contains_key(&lane.project_id) {
            continue;
        }
        let claims = match crate::cmd_claim::read_active_claims(&lane.project_id) {
            Ok(claims) => Some(claims),
            Err(e) => {
                eprintln!(
                    "⚠ fleet watch: board unreadable for project {}; its lanes are not judged: {e:#}",
                    lane.project_id
                );
                None
            }
        };
        claims_by_project.insert(lane.project_id.clone(), claims);
    }

    let cap = resolve_cap(repo_root, args.max_redispatch);
    let mut rows = Vec::new();
    let mut orphans = Vec::new();
    for lane in lanes {
        // A missing key cannot happen (collected above), but a missing board is
        // `None` either way: not judged, never a death verdict.
        let board = claims_by_project
            .get(&lane.project_id)
            .and_then(Option::as_ref);
        let claimed = board.map(|claims| lane_claimed(&lane, claims, now));
        let has_open_claim = board.is_some_and(|claims| {
            claims
                .iter()
                .any(|claim| claim.session_id == lane.session_id)
        });
        let terminal = terminal_record(&lane, has_open_claim);
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
        detail: judged.detail(),
        last_heartbeat: lane.last_heartbeat.clone(),
    }
}

#[cfg(test)]
mod tests;
