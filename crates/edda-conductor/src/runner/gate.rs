use crate::agent::budget::BudgetTracker;
use crate::agent::launcher::{phase_session_id_attempt, AgentLauncher};
use crate::check::engine::CheckEngine;
use crate::plan::schema::{OnGateTimeout, OnReject, Phase, Plan};
use crate::runner::edda;
use crate::runner::event_log::{self, Event, EventLogger};
use crate::runner::heartbeat::{
    lane_heartbeat_interval_secs, run_phase_with_heartbeat, LaneHeartbeat,
};
use crate::runner::notify::Notifier;
use crate::runner::outcome::{phase_terminal_event, process_phase_result};
use crate::runner::sequential::{build_plan_context_with_edda, format_elapsed, now_rfc3339};
use crate::state::brief::write_brief;
use crate::state::machine::{
    transition, ErrorInfo, ErrorType, PhaseStatus, PhaseUpdate, PlanState,
};
use crate::state::persist::save_state_reconciled;
use crate::tmux::TmuxSession;
use anyhow::{Context, Result};
use edda_core::VerdictPayload;
use edda_ledger::VerdictRecord;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Verdict gate helpers (GH-519 D3/D4/D5) ────────────────────────────────
/// Ledger poll interval while a gate waits for a verdict.
const GATE_POLL_SEC: u64 = 2;

/// GH-551: progress signals during a long gate wait. The first signal goes
/// out after this many seconds of waiting, then the interval doubles up to
/// the cap — low-frequency by design, so the normal 2s poll stays silent.
const GATE_PROGRESS_FIRST_SECS: u64 = 60;
const GATE_PROGRESS_MAX_SECS: u64 = 600;

/// GH-541: first report of a persistent ledger read failure goes out
/// immediately; repeats follow this interval, doubling up to the cap.
const GATE_READ_ERROR_REPORT_SECS: u64 = 30;
const GATE_READ_ERROR_REPORT_CAP_SECS: u64 = 300;
/// GH-541: consecutive persistent (non-lock) ledger read failures after
/// which the gate fails with the diagnostic instead of waiting silently.
/// At the 2s poll this is ~30s of a persistently broken ledger — an
/// operator-fixable fault (corrupt db, permissions, wrong workspace root)
/// must not consume the whole gate budget unnoticed.
pub(super) const GATE_MAX_PERSISTENT_READ_ERRORS: u32 = 15;

/// GH-541: tracks persistent (non-lock) ledger read failures while a gate
/// waits. Busy/lock contention is transient and never counts. A healthy
/// poll resets the budget. Reports the error on the first failure and then
/// on a decaying interval; returns `LedgerUnreadable` when the budget
/// expires.
pub(super) struct ReadErrorTracker {
    pub(super) consecutive: u32,
    report_backoff_secs: u64,
    pub(super) next_report: Option<Instant>,
}

impl Default for ReadErrorTracker {
    fn default() -> Self {
        Self {
            consecutive: 0,
            report_backoff_secs: GATE_READ_ERROR_REPORT_SECS,
            next_report: None,
        }
    }
}

impl ReadErrorTracker {
    /// Observe a poll error. Returns `Some(GateVerdict::LedgerUnreadable)`
    /// when the persistent-failure budget is exhausted.
    pub(super) fn observe(&mut self, err: &anyhow::Error) -> Option<GateVerdict> {
        if edda_ledger::lock::is_busy_error(err) {
            // Transient lock contention: degrade quietly, as before.
            return None;
        }
        self.consecutive += 1;
        if self.consecutive == 1 || self.next_report.is_some_and(|t| Instant::now() >= t) {
            eprintln!(
                "  ⚠ verdict gate: ledger read failed (attempt {}): {err:#}",
                self.consecutive
            );
            self.next_report = Some(Instant::now() + Duration::from_secs(self.report_backoff_secs));
            self.report_backoff_secs =
                (self.report_backoff_secs * 2).min(GATE_READ_ERROR_REPORT_CAP_SECS);
        }
        if self.consecutive >= GATE_MAX_PERSISTENT_READ_ERRORS {
            return Some(GateVerdict::LedgerUnreadable(format!(
                "{} consecutive failed ledger reads: {err:#}",
                self.consecutive
            )));
        }
        None
    }

    /// A healthy poll (the ledger opened and the query answered) resets the
    /// persistent-failure budget.
    pub(super) fn reset(&mut self) {
        self.consecutive = 0;
        self.next_report = None;
    }
}

/// D6: bound gate redispatch cycles with their own persisted counter, NOT
/// `attempt` (which D3 forbids incrementing on redispatch). A redispatch
/// turn is not guaranteed to produce a commit, so a re-entered gate can
/// wait on the same `(subject, gate_sha)` forever while `max_attempts`
/// never trips — this counter is the real loop bound. Exhausting it fails
/// the phase like `on_reject: halt`, with a distinct error naming the bound.
///
/// Fixed at 3 rather than plan-configurable, on purpose: this bound exists
/// to kill loop-shaped defects (the D6 loop measured 176 cycles before the
/// fix), and a plan-author-tunable ceiling would let the same optimism that
/// wrote an unbounded gate re-open the loop from inside the plan file. Three
/// covers the multi-round review cycles this repo actually ships (e.g. the
/// three-round review of GH-534); a phase that genuinely needs more review
/// rounds should be split into smaller phases instead of raising the bound.
pub(super) const MAX_GATE_REDISPATCHES: u32 = 3;

/// `<plan-name>/<phase-id>` — the subject an `edda verdict` targets (D1/D3).
pub(super) fn gate_subject(plan_name: &str, phase_id: &str) -> String {
    format!("{plan_name}/{phase_id}")
}

/// Current git HEAD of `cwd` — the SHA a verdict must match (D3).
pub(super) fn capture_git_head(cwd: &Path) -> Result<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(cwd)
        .output()
        .context("spawning git rev-parse HEAD to capture the gate sha")?;
    if !out.status.success() {
        anyhow::bail!(
            "git rev-parse HEAD failed in {}: {}",
            cwd.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        anyhow::bail!("git rev-parse HEAD returned a non-40-hex sha: \"{sha}\"");
    }
    Ok(sha)
}

/// Outcome of waiting on a verdict gate.
#[derive(Debug)]
pub(super) enum GateVerdict {
    Approved(VerdictRecord),
    Rejected(VerdictRecord),
    /// `gate_timeout_sec` elapsed with no matching verdict — NOT silent,
    /// NOT auto-approve (D3).
    TimedOut,
    /// The ledger stayed unreadable (NOT SQLite busy/lock contention) for
    /// the whole error budget (GH-541): fail the gate with the diagnostic
    /// instead of polling in silence forever. In this state the printed
    /// `edda verdict` command could never satisfy the gate anyway — it
    /// writes to a ledger this gate cannot read.
    LedgerUnreadable(String),
    /// The CancellationToken fired while waiting. The phase stays
    /// AWAITING_VERDICT so a later resume re-enters the wait (D3 restart).
    Cancelled,
}

/// Poll the ledger for a verdict matching `(subject, gate_sha)` that was
/// recorded AFTER this gate's `gate_entered_at` (D3 + D6 freshness).
///
/// A verdict bound to a different SHA is findable in the ledger but never
/// satisfies this wait (D1). A stale verdict — one predating this gate's
/// entry, e.g. the rejection still sitting in the ledger when a redispatch
/// turn produced no new commit — also never satisfies it (D6); without that
/// bound the re-entered gate would re-read the same rejection forever.
/// The timeout deadline is computed from the persisted `gate_entered_at`,
/// so it survives restarts.
#[allow(clippy::too_many_arguments)]
pub(super) async fn wait_for_verdict(
    cwd: &Path,
    plan_name: &str,
    phase_id: &str,
    gate_sha: &str,
    timeout_sec: Option<u64>,
    entered_at: Option<&str>,
    cancel: &CancellationToken,
    heartbeat: Option<&LaneHeartbeat>,
    notifier: &dyn Notifier,
) -> GateVerdict {
    let subject = gate_subject(plan_name, phase_id);
    let deadline = timeout_sec.map(|t| {
        let base = entered_at
            .and_then(|s| {
                time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
            })
            .unwrap_or_else(time::OffsetDateTime::now_utc);
        base + time::Duration::seconds(t as i64)
    });

    // A gated lane is still alive — refresh its heartbeat while waiting so
    // it does not read as stale to peer discovery (GH-566).
    if let Some(hb) = heartbeat {
        hb.write("awaiting_verdict");
    }
    let mut last_heartbeat = Instant::now();

    // GH-551: a bounded gate gives no indication that a clock is running,
    // and an unbounded one looks identical to a broken one — in the measured
    // run two gates burned 7200s each while a one-line "still waiting" would
    // have rescued the wait at any moment. Emit a low-frequency progress
    // signal (60s, then doubling to 10min) naming the remaining budget.
    // tokio::time::Instant so tests can drive the whole schedule on paused
    // time without wall-clock sleeps.
    let mut next_progress =
        tokio::time::Instant::now() + Duration::from_secs(GATE_PROGRESS_FIRST_SECS);
    let mut progress_interval = GATE_PROGRESS_FIRST_SECS;
    // Paused-time mirror of the deadline, for the remaining-budget label —
    // anchored to the same remaining time as the wall-clock `deadline` above
    // so it survives controller restart honestly (GH-751).
    let deadline_t = deadline.map(|d| {
        let now_utc = time::OffsetDateTime::now_utc();
        let remaining_secs = (d - now_utc).whole_seconds().max(0) as u64;
        tokio::time::Instant::now() + Duration::from_secs(remaining_secs)
    });

    // GH-541: a failed ledger read is not the same event as "no verdict yet".
    // SQLite busy/lock contention degrades quietly (transient); every other
    // error (corrupt database, permission, missing workspace) is persistent,
    // is reported at least once and then on a decaying interval, and fails
    // the gate after the error budget — the one response that cannot be
    // right here is silence (the operator runs the printed approve command
    // and nothing happens, with no diagnostic anywhere).
    let mut read_errors = ReadErrorTracker::default();

    loop {
        // Poll BEFORE the deadline check: a verdict recorded during this
        // wait (e.g. right after the gate_entered event) must be observed
        // at the last poll, not skipped by a deadline that fires first.
        match edda_ledger::Ledger::open(cwd) {
            Err(e) => {
                if let Some(v) = read_errors.observe(&e) {
                    return v;
                }
            }
            Ok(ledger) => match ledger.latest_verdict_fresh(&subject, gate_sha, entered_at) {
                Ok(Some(record)) => {
                    return match record.payload.decision {
                        edda_core::VerdictDecision::Approved => GateVerdict::Approved(record),
                        edda_core::VerdictDecision::Rejected => GateVerdict::Rejected(record),
                    };
                }
                Ok(None) => read_errors.reset(),
                Err(e) => {
                    if let Some(v) = read_errors.observe(&e) {
                        return v;
                    }
                }
            },
        }
        if let Some(deadline) = deadline {
            if time::OffsetDateTime::now_utc() >= deadline {
                return GateVerdict::TimedOut;
            }
        }
        // GH-551: progress signal on the decaying schedule. Inside the same
        // select arm cadence as the poll but gated on its own clock, so the
        // 2s poll itself stays silent.
        let now_t = tokio::time::Instant::now();
        if now_t >= next_progress {
            let wait_label = match deadline_t {
                Some(d) => {
                    let remaining = (d - now_t).as_secs();
                    format!(
                        "{} remaining",
                        format_elapsed(std::time::Duration::from_secs(remaining))
                    )
                }
                None => "no deadline (waits until cancelled)".to_string(),
            };
            notifier
                .notify_gate_progress(edda_notify::NotifyEvent::GateProgress {
                    plan: plan_name.to_string(),
                    phase: phase_id.to_string(),
                    subject: subject.clone(),
                    gate_sha: gate_sha.to_string(),
                    wait_label,
                })
                .await;
            progress_interval = (progress_interval * 2).min(GATE_PROGRESS_MAX_SECS);
            next_progress = now_t + Duration::from_secs(progress_interval);
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(GATE_POLL_SEC)) => {
                if let Some(hb) = heartbeat {
                    if last_heartbeat.elapsed() >= Duration::from_secs(lane_heartbeat_interval_secs()) {
                        hb.write("awaiting_verdict");
                        last_heartbeat = Instant::now();
                    }
                }
            }
            _ = cancel.cancelled() => return GateVerdict::Cancelled,
        }
    }
}
/// Record verdict metadata on the phase state (D3).
pub(super) fn record_verdict_metadata(
    state: &mut PlanState,
    phase_id: &str,
    payload: &VerdictPayload,
) {
    if let Ok(ps) = state.get_phase_mut(phase_id) {
        ps.verdict_decision = Some(payload.decision.to_string());
        ps.verdict_actor = Some(payload.actor.clone());
        ps.verdict_comment = payload.comment.clone();
    }
}

/// Prompt for the redispatch turn after a rejected verdict (D3): the
/// rejection comment becomes the prompt, prefixed with brief context.
pub(super) fn build_redispatch_prompt(phase: &Phase, phase_id: &str, comment: &str) -> String {
    let mut prompt = String::new();
    if let Some(ctx) = &phase.context {
        prompt.push_str(ctx);
        prompt.push_str("\n\n");
    }
    prompt.push_str(&format!(
        "## Verdict: REJECTED\n\n\
         The external reviewer rejected the gated result for phase \"{phase_id}\".\n\n\
         Reviewer feedback:\n{comment}\n\n\
         Address the feedback above (your previous changes are still on disk), \
         then make sure the phase checks pass again."
    ));
    prompt
}

/// GH-564 P1-3: where a gated phase's agent final output is parked while the
/// plan waits for the external verdict. `{cwd}/.edda/conductor/{plan}/{phase}.gate_output`.
/// Survives a conductor restart, so the verdict site can restore the output
/// into the terminal notification instead of dropping it to `null`.
pub(super) fn gate_output_path(cwd: &Path, plan_name: &str, phase_id: &str) -> PathBuf {
    cwd.join(".edda")
        .join("conductor")
        .join(plan_name)
        .join(format!("{phase_id}.gate_output"))
}

/// GH-564 P1-3 / Round-2 P1: park the agent's final output when the phase
/// enters AWAITING_VERDICT. EVERY gate entry atomically rewrites the sidecar
/// so it represents THIS entry's output and only this entry's: the last
/// non-empty agent line when there is one, an EMPTY file when there is none
/// (`load_gate_output` maps empty to `None`). A previous redispatch cycle's
/// value can therefore never be read back as the current output.
///
/// Errors are NOT swallowed: a failed write would leave the previous cycle's
/// file in place, which the verdict site could read back as this entry's
/// output — the caller must fail the gate entry instead of waiting on a
/// verdict against a possibly wrong payload.
pub(super) fn persist_gate_output(
    cwd: &Path,
    plan_name: &str,
    phase_id: &str,
    final_output: Option<&str>,
) -> anyhow::Result<()> {
    let path = gate_output_path(cwd, plan_name, phase_id);
    edda_store::write_atomic(&path, final_output.unwrap_or("").as_bytes())
}

/// GH-564 Round-2 P1: consume the parked output at the verdict site, so the
/// sidecar only exists between a gate entry and its verdict. Best-effort: a
/// failed removal cannot produce a wrong payload because every gate entry
/// rewrites the sidecar atomically before any verdict is waited on.
pub(super) fn clear_gate_output(cwd: &Path, plan_name: &str, phase_id: &str) {
    let _ = std::fs::remove_file(gate_output_path(cwd, plan_name, phase_id));
}

/// GH-564 P1-3: read back the parked final output at the gate verdict site.
pub(super) fn load_gate_output(cwd: &Path, plan_name: &str, phase_id: &str) -> Option<String> {
    let output = std::fs::read_to_string(gate_output_path(cwd, plan_name, phase_id)).ok()?;
    let trimmed = output.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// GH-551: the deadline line for the AWAITING_VERDICT surface. A bounded
/// gate states its deadline; an unbounded one says so explicitly, so an
/// operator who set 7200s hours earlier can tell the clock is draining.
pub(super) fn format_gate_deadline(timeout_sec: Option<u64>, entered_at: &str) -> String {
    match timeout_sec {
        Some(secs) => {
            let deadline = time::OffsetDateTime::parse(
                entered_at,
                &time::format_description::well_known::Rfc3339,
            )
            .ok()
            .map(|t| t + time::Duration::seconds(secs as i64))
            .and_then(|d| {
                d.format(&time::format_description::well_known::Rfc3339)
                    .ok()
            })
            .unwrap_or_else(|| "<unparsable gate_entered_at>".into());
            format!("{deadline} (gate_timeout_sec {secs})")
        }
        None => "none — waits until cancelled".to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_gate_approved(
    plan: &Plan,
    state: &mut PlanState,
    gated_id: &str,
    subject: &str,
    gate_sha: &str,
    record: VerdictRecord,
    cwd: &Path,
    tmux_session: Option<&TmuxSession>,
    notifier: &dyn Notifier,
    event_log: &mut EventLogger,
) -> Result<()> {
    event_log.record(Event::VerdictReceived {
        phase_id: gated_id.to_string(),
        decision: "approved".into(),
        gate_sha: gate_sha.to_string(),
        comment: record.payload.comment.clone(),
    });
    transition(
        state,
        gated_id,
        PhaseStatus::AwaitingVerdict,
        PhaseStatus::Passed,
        Some(PhaseUpdate {
            completed_at: Some(now_rfc3339()),
            ..Default::default()
        }),
    )?;
    record_verdict_metadata(state, gated_id, &record.payload);
    println!("  ✓ Verdict approved — phase \"{gated_id}\" passed");
    if let Some(tmux) = tmux_session {
        let _ = tmux.update_phase_status(gated_id, "Passed");
    }
    edda::record_note(
        cwd,
        &format!("Gate \"{subject}\" approved (sha {gate_sha})"),
        &["conductor", "verdict"],
    );
    let approved_ps = state.get_phase(gated_id)?;
    // GH-564 P1-3: the approved phase's agent final output
    // (last line = PR URL by convention) was parked at gate
    // entry and survives restarts — restore it, never drop
    // it to null. GH-564 Round-2 P1: consume the sidecar —
    // its lifecycle ends with this verdict.
    let final_output = load_gate_output(cwd, &plan.name, gated_id);
    clear_gate_output(cwd, &plan.name, gated_id);
    // GH-584 round-3: gate approval is a phase terminal
    // state like any other — write the structured
    // `conductor_phase` event with the plan id and the
    // measured cost parked on the phase at gate entry,
    // exactly as the non-gate pass path does.
    edda::record_phase_done_with_plan(
        cwd,
        Some(&plan.name),
        gated_id,
        final_output.as_deref(),
        approved_ps.cost_usd,
    );
    notifier
        .notify_phase_terminal(phase_terminal_event(
            &plan.name,
            gated_id,
            "Passed",
            approved_ps.attempts,
            final_output.as_deref(),
        ))
        .await;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_gate_rejected(
    plan: &Plan,
    phase: &Phase,
    state: &mut PlanState,
    gated_id: &str,
    subject: &str,
    gate_sha: &str,
    record: VerdictRecord,
    cwd: &Path,
    phase_cwd: &Path,
    launcher: &dyn AgentLauncher,
    check_engine: &CheckEngine,
    notifier: &dyn Notifier,
    budget: &mut BudgetTracker,
    cancel: &CancellationToken,
    tmux_session: Option<&TmuxSession>,
    event_log: &mut EventLogger,
) -> Result<()> {
    let comment = record.payload.comment.clone().unwrap_or_default();
    event_log.record(Event::VerdictReceived {
        phase_id: gated_id.to_string(),
        decision: "rejected".into(),
        gate_sha: gate_sha.to_string(),
        comment: Some(comment.clone()),
    });
    println!("  ✗ Verdict rejected for \"{gated_id}\"");
    edda::record_note(
        cwd,
        &format!("Gate \"{subject}\" rejected (sha {gate_sha})"),
        &["conductor", "verdict"],
    );

    let (attempts, redispatches) = {
        let ps = state.get_phase(gated_id)?;
        (ps.attempts, ps.gate_redispatches)
    };
    let max = phase.max_attempts.unwrap_or(plan.max_attempts);
    let bound_exhausted = redispatches >= MAX_GATE_REDISPATCHES;
    if phase.on_reject == OnReject::Halt || attempts >= max || bound_exhausted {
        // Halt (or a bound exhausted): the comment is the
        // error (D3). The redispatch bound gets a distinct
        // message naming the bound (D6).
        let message = if bound_exhausted {
            format!(
                "verdict gate redispatch bound exhausted ({redispatches} redispatch cycles, max {MAX_GATE_REDISPATCHES}) for \"{subject}\"; last rejection: {comment}"
            )
        } else {
            comment.clone()
        };
        transition(
            state,
            gated_id,
            PhaseStatus::AwaitingVerdict,
            PhaseStatus::Failed,
            Some(PhaseUpdate {
                error: Some(ErrorInfo {
                    error_type: ErrorType::GateRejected,
                    message: message.clone(),
                    retryable: false,
                    check_index: None,
                    timestamp: now_rfc3339(),
                }),
                ..Default::default()
            }),
        )?;
        record_verdict_metadata(state, gated_id, &record.payload);
        println!("  ✗ Phase \"{gated_id}\" failed: {message}");
        if let Some(tmux) = tmux_session {
            let _ = tmux.update_phase_status(gated_id, "Failed");
        }
        let gate_cost = state.get_phase(gated_id).ok().and_then(|p| p.cost_usd);
        edda::record_phase_failed_with_plan(cwd, Some(&plan.name), gated_id, gate_cost, &message);
        let gate_ps = state.get_phase(gated_id)?;
        event_log.record(Event::PhaseFailed {
            phase_id: gated_id.to_string(),
            attempt: gate_ps.attempts,
            duration_ms: 0,
            error: format!("verdict rejected: {message}"),
            error_type: Some(ErrorType::GateRejected.tag().to_string()),
            env_retries: gate_ps.env_retries,
            attempt_charged: true,
        });
        // GH-564 P1-3: same parked output as the approved
        // branch — the agent did produce a final line before
        // the gate rejected it. Consume the sidecar with the
        // verdict (GH-564 Round-2 P1).
        let final_output = load_gate_output(cwd, &plan.name, gated_id);
        clear_gate_output(cwd, &plan.name, gated_id);
        notifier
            .notify_phase_terminal(phase_terminal_event(
                &plan.name,
                gated_id,
                "Failed",
                gate_ps.attempts,
                final_output.as_deref(),
            ))
            .await;
    } else {
        // Redispatch (D3): ONE more agent turn in the SAME
        // session — do NOT increment attempt; the rejection
        // comment becomes the prompt, prefixed with context.
        // D6: count the cycle on its own persisted counter.
        state.get_phase_mut(gated_id)?.gate_redispatches += 1;
        let session_id = phase_session_id_attempt(&plan.name, gated_id, attempts).to_string();
        let prompt = build_redispatch_prompt(phase, gated_id, &comment);
        let plan_context = build_plan_context_with_edda(plan, state, gated_id, cwd);
        transition(
            state,
            gated_id,
            PhaseStatus::AwaitingVerdict,
            PhaseStatus::Running,
            None,
        )?;
        save_state_reconciled(cwd, state)?;
        println!("  ↻ Redispatching one more turn in the same session ({session_id})");
        let lane_hb = LaneHeartbeat {
            cwd: cwd.to_path_buf(),
            session_id: session_id.clone(),
            plan: plan.name.clone(),
            phase: gated_id.to_string(),
            attempt: attempts,
        };
        let result = run_phase_with_heartbeat(
            launcher,
            phase,
            &prompt,
            &plan_context,
            phase_cwd,
            cancel,
            &lane_hb,
        )
        .await?;
        process_phase_result(
            plan,
            phase,
            state,
            gated_id,
            attempts,
            result,
            cwd,
            phase_cwd,
            Instant::now(),
            budget,
            check_engine,
            notifier,
            event_log,
            tmux_session,
            cancel,
            Some(&lane_hb),
        )
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_gate_timed_out(
    plan: &Plan,
    phase: &Phase,
    state: &mut PlanState,
    gated_id: &str,
    subject: &str,
    gate_sha: &str,
    entered_at: &str,
    cwd: &Path,
    tmux_session: Option<&TmuxSession>,
    notifier: &dyn Notifier,
    event_log: &mut EventLogger,
) -> Result<()> {
    // D3: NOT silent, NOT auto-approve. GH-552: also not a
    // phase failure — the work completed and its checks
    // passed, so the honest terminal state is GateTimedOut
    // with the real elapsed gate time, and the plan's
    // on_gate_timeout policy decides what happens next.
    let elapsed_ms =
        time::OffsetDateTime::parse(entered_at, &time::format_description::well_known::Rfc3339)
            .ok()
            .and_then(|t| {
                let elapsed = time::OffsetDateTime::now_utc() - t;
                if elapsed.whole_seconds() < 0 {
                    None
                } else {
                    Some(elapsed.whole_seconds() as u64 * 1000)
                }
            })
            .unwrap_or(0);
    let msg = format!(
        "gate timed out: no verdict for \"{subject}\" (sha {gate_sha}) within {}s",
        phase.gate_timeout_sec.unwrap_or(0)
    );
    transition(
        state,
        gated_id,
        PhaseStatus::AwaitingVerdict,
        PhaseStatus::GateTimedOut,
        Some(PhaseUpdate {
            error: Some(ErrorInfo {
                error_type: ErrorType::GateTimeout,
                message: msg.clone(),
                retryable: false,
                check_index: None,
                timestamp: now_rfc3339(),
            }),
            ..Default::default()
        }),
    )?;
    println!("  ⏰ Phase \"{gated_id}\" {msg}");
    if let Some(tmux) = tmux_session {
        let _ = tmux.update_phase_status(gated_id, "GateTimedOut");
    }
    let gate_cost = state.get_phase(gated_id).ok().and_then(|p| p.cost_usd);
    edda::record_phase_gate_timed_out(cwd, Some(&plan.name), gated_id, gate_cost, &msg);
    event_log.record(Event::GateTimedOut {
        phase_id: gated_id.to_string(),
        gate_sha: gate_sha.to_string(),
        elapsed_ms,
    });
    // GH-564 P1-3: same parked output as the approved branch.
    // Consume the sidecar with the verdict (GH-564 Round-2 P1).
    let final_output = load_gate_output(cwd, &plan.name, gated_id);
    clear_gate_output(cwd, &plan.name, gated_id);
    let gate_ps = state.get_phase(gated_id)?;
    notifier
        .notify_phase_terminal(phase_terminal_event(
            &plan.name,
            gated_id,
            "GateTimedOut",
            gate_ps.attempts,
            final_output.as_deref(),
        ))
        .await;

    // GH-552 policy: let an unattended run declare the
    // decision in advance instead of exiting with
    // instructions it cannot follow.
    if phase.on_gate_timeout == OnGateTimeout::Skip {
        let reason = format!(
            "gate waived after timeout ({} waited, {}s configured): work completed and checks passed; auto-waived by on_gate_timeout: skip",
            format_elapsed(std::time::Duration::from_millis(elapsed_ms)),
            phase.gate_timeout_sec.unwrap_or(0)
        );
        let ps = state.get_phase_mut(gated_id)?;
        ps.skip_reason = Some(reason.clone());
        event_log.record(Event::GateWaived {
            phase_id: gated_id.to_string(),
            reason: reason.clone(),
            auto: true,
        });
        println!("  ⧗ Gate auto-waived for \"{gated_id}\" (on_gate_timeout: skip) — plan proceeds");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_gate_ledger_unreadable(
    plan: &Plan,
    phase: &Phase,
    state: &mut PlanState,
    gated_id: &str,
    subject: &str,
    gate_sha: &str,
    entered_at: &str,
    err: &str,
    cwd: &Path,
    tmux_session: Option<&TmuxSession>,
    notifier: &dyn Notifier,
    event_log: &mut EventLogger,
) -> Result<()> {
    // GH-541 / GH-744: the gate could not read the ledger persistently
    // (not lock contention). The phase's work completed and checks passed;
    // the failure is infrastructure, not the agent's work.
    // Transition to GateTimedOut (rather than Failed) so the GH-552 waive route
    // is preserved: the operator can waive the gate in interactive mode,
    // and unattended runs honor on_gate_timeout: skip.
    let msg = format!("gate aborted: ledger unreadable for \"{subject}\" (sha {gate_sha}): {err}");
    transition(
        state,
        gated_id,
        PhaseStatus::AwaitingVerdict,
        PhaseStatus::GateTimedOut,
        Some(PhaseUpdate {
            error: Some(ErrorInfo {
                error_type: ErrorType::LedgerUnreadable,
                message: msg.clone(),
                retryable: false,
                check_index: None,
                timestamp: now_rfc3339(),
            }),
            ..Default::default()
        }),
    )?;
    println!("  ⏰ Phase \"{gated_id}\" {msg}");
    if let Some(tmux) = tmux_session {
        let _ = tmux.update_phase_status(gated_id, "GateTimedOut");
    }
    let gate_cost = state.get_phase(gated_id).ok().and_then(|p| p.cost_usd);
    edda::record_phase_gate_timed_out(cwd, Some(&plan.name), gated_id, gate_cost, &msg);
    let elapsed_ms =
        time::OffsetDateTime::parse(entered_at, &time::format_description::well_known::Rfc3339)
            .ok()
            .and_then(|t| {
                let elapsed = time::OffsetDateTime::now_utc() - t;
                if elapsed.whole_seconds() < 0 {
                    None
                } else {
                    Some(elapsed.whole_seconds() as u64 * 1000)
                }
            })
            .unwrap_or(0);
    event_log.record(Event::GateTimedOut {
        phase_id: gated_id.to_string(),
        gate_sha: gate_sha.to_string(),
        elapsed_ms,
    });
    let final_output = load_gate_output(cwd, &plan.name, gated_id);
    clear_gate_output(cwd, &plan.name, gated_id);
    let gate_ps = state.get_phase(gated_id)?;
    notifier
        .notify_phase_terminal(phase_terminal_event(
            &plan.name,
            gated_id,
            "GateTimedOut",
            gate_ps.attempts,
            final_output.as_deref(),
        ))
        .await;

    // GH-552 / GH-744: auto-waive if phase configured with on_gate_timeout: skip
    if phase.on_gate_timeout == OnGateTimeout::Skip {
        let reason = format!(
            "gate waived after ledger unreadable: work completed and checks passed; auto-waived by on_gate_timeout: skip ({err})"
        );
        let ps = state.get_phase_mut(gated_id)?;
        ps.skip_reason = Some(reason.clone());
        event_log.record(Event::GateWaived {
            phase_id: gated_id.to_string(),
            reason: reason.clone(),
            auto: true,
        });
        println!("  ⧗ Gate auto-waived for \"{gated_id}\" (on_gate_timeout: skip) — plan proceeds");
    }
    Ok(())
}

/// Settles a phase waiting for an external verdict if one is currently in AWAITING_VERDICT state.
///
/// Returns `Ok(true)` if a phase was settled (caller should loop/continue), or `Ok(false)`
/// if no phase is awaiting a verdict (caller proceeds to next runnable phase).
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)] // 155 lines — orchestrates Approved/Rejected/TimedOut/Unreadable arms; split tracked in #776
pub(super) async fn settle_gated_phase(
    plan: &Plan,
    state: &mut PlanState,
    order: &[String],
    launcher: &dyn AgentLauncher,
    check_engine: &CheckEngine,
    notifier: &dyn Notifier,
    budget: &mut BudgetTracker,
    cancel: &CancellationToken,
    cwd: &Path,
    tmux_session: Option<&TmuxSession>,
    event_log: &mut EventLogger,
) -> Result<bool> {
    // ── Verdict gate wait (GH-519 D3) ────────────────────────────
    // A phase holding AWAITING_VERDICT pauses the plan until a verdict
    // arrives. On restart this re-enters the wait WITHOUT re-running the
    // phase agent turn or checks; gate_sha comes from persisted state.
    let Some(gated_id) = state
        .phases
        .iter()
        .find(|p| p.status == PhaseStatus::AwaitingVerdict)
        .map(|p| p.id.clone())
    else {
        return Ok(false);
    };

    let phase = plan
        .phases
        .iter()
        .find(|p| p.id == gated_id)
        .with_context(|| format!("gated phase \"{gated_id}\" not found in plan"))?;
    let phase_cwd = phase
        .cwd
        .as_deref()
        .or(plan.cwd.as_deref())
        .map(|p| cwd.join(p))
        .unwrap_or_else(|| cwd.to_path_buf());
    let subject = gate_subject(&plan.name, &gated_id);
    let (gate_sha, entered_at) = {
        let ps = state.get_phase(&gated_id)?;
        let sha = ps.gate_sha.clone().with_context(|| {
            format!("AWAITING_VERDICT state for \"{gated_id}\" is missing gate_sha")
        })?;
        // GH-541: fail CLOSED on a missing freshness bound. Substituting
        // `now` (the previous behavior) admitted every verdict recorded
        // after the resume instant — including the stale rejection from
        // the previous gate entry that D6 blocks — with no diagnostic.
        // An AWAITING_VERDICT phase must carry the persisted entry time:
        // it is the D6 bound and the timeout anchor across restarts.
        let at = ps.gate_entered_at.clone().with_context(|| {
            format!(
                "AWAITING_VERDICT state for \"{gated_id}\" is missing gate_entered_at — \
                 the D6 freshness bound cannot be established, so refusing to wait; \
                 a stale verdict could otherwise be admitted"
            )
        })?;
        time::OffsetDateTime::parse(&at, &time::format_description::well_known::Rfc3339)
            .with_context(|| {
                format!(
                    "AWAITING_VERDICT state for \"{gated_id}\" has unparsable gate_entered_at \"{at}\" — \
                     the D6 freshness bound cannot be established, so refusing to wait; \
                     a stale verdict could otherwise be admitted"
                )
            })?;
        (sha, at)
    };
    let phase_num = order.iter().position(|id| id == &gated_id).unwrap_or(0) + 1;
    let total_phases = order.len();

    // D4: unmistakable surface naming subject + gate_sha + the exact
    // approve/reject commands to run.
    println!("\n⏸ [{phase_num}/{total_phases}] Phase \"{gated_id}\" AWAITING_VERDICT — waiting for an external verdict");
    println!("  subject:  {subject}");
    println!("  gate_sha: {gate_sha}");
    // GH-551: the budget must be visible where the wait is announced
    // — an operator who set 7200s hours earlier has no surface
    // telling them the clock is draining.
    println!(
        "  deadline: {}",
        format_gate_deadline(phase.gate_timeout_sec, &entered_at)
    );
    println!("  approve:  edda verdict approve {subject} --sha {gate_sha}");
    println!("  reject:   edda verdict reject {subject} --sha {gate_sha} --comment \"<why>\"");
    event_log::write_runner_status(cwd, state, Some(&gated_id));
    write_brief(cwd, state, None);

    // The lane stays alive while gated — keep its heartbeat honest
    // with an "awaiting_verdict" stage (GH-566).
    let lane_hb = {
        let ps = state.get_phase(&gated_id)?;
        LaneHeartbeat {
            cwd: cwd.to_path_buf(),
            session_id: phase_session_id_attempt(&plan.name, &gated_id, ps.attempts).to_string(),
            plan: plan.name.clone(),
            phase: gated_id.clone(),
            attempt: ps.attempts,
        }
    };

    match wait_for_verdict(
        cwd,
        &plan.name,
        &gated_id,
        &gate_sha,
        phase.gate_timeout_sec,
        Some(&entered_at),
        cancel,
        Some(&lane_hb),
        notifier,
    )
    .await
    {
        GateVerdict::Approved(record) => {
            handle_gate_approved(
                plan,
                state,
                &gated_id,
                &subject,
                &gate_sha,
                record,
                cwd,
                tmux_session,
                notifier,
                event_log,
            )
            .await?;
        }
        GateVerdict::Rejected(record) => {
            handle_gate_rejected(
                plan,
                phase,
                state,
                &gated_id,
                &subject,
                &gate_sha,
                record,
                cwd,
                &phase_cwd,
                launcher,
                check_engine,
                notifier,
                budget,
                cancel,
                tmux_session,
                event_log,
            )
            .await?;
        }
        GateVerdict::TimedOut => {
            handle_gate_timed_out(
                plan,
                phase,
                state,
                &gated_id,
                &subject,
                &gate_sha,
                &entered_at,
                cwd,
                tmux_session,
                notifier,
                event_log,
            )
            .await?;
        }
        GateVerdict::LedgerUnreadable(err) => {
            handle_gate_ledger_unreadable(
                plan,
                phase,
                state,
                &gated_id,
                &subject,
                &gate_sha,
                &entered_at,
                &err,
                cwd,
                tmux_session,
                notifier,
                event_log,
            )
            .await?;
        }
        GateVerdict::Cancelled => {
            // The loop top sees the cancelled token and shuts down;
            // the phase stays AWAITING_VERDICT so a later
            // `edda conduct run` resumes the wait (D3 restart).
        }
    }

    save_state_reconciled(cwd, state)?;
    event_log::write_runner_status(cwd, state, Some(&gated_id));
    write_brief(cwd, state, None);
    Ok(true)
}
