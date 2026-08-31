use crate::agent::budget::BudgetTracker;
use crate::agent::launcher::{phase_session_id_attempt, AgentLauncher, PhaseResult};
use crate::check::engine::{CheckEngine, CheckRunResult};
use crate::plan::schema::{CheckSpec, OnFail, OnReject, Plan};
use crate::plan::topo::topo_sort;
use crate::runner::edda;
use crate::runner::event_log::{self, Event, EventLogger};
use crate::runner::notify::Notifier;
use crate::state::brief::write_brief;
use crate::state::derive::{
    detect_stale_phases, find_next_phase, is_plan_blocked, is_plan_complete, update_plan_status,
};
use crate::state::machine::{
    transition, CheckResult, CheckStatus, ErrorInfo, ErrorType, PhaseStatus, PhaseUpdate,
    PlanState, PlanStatus,
};
use crate::state::persist::save_state;
use crate::tmux::TmuxSession;
use anyhow::Context;
use anyhow::Result;
use edda_core::VerdictPayload;
use edda_ledger::VerdictRecord;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Runtime context for [`run_plan`], grouping execution environment parameters.
pub struct RunContext<'a> {
    pub launcher: &'a dyn AgentLauncher,
    pub check_engine: &'a CheckEngine,
    pub notifier: &'a dyn Notifier,
    pub budget: &'a mut BudgetTracker,
    pub cancel: CancellationToken,
    pub cwd: &'a Path,
    pub interactive: bool,
    pub json_events: bool,
    /// Optional tmux session for updating pane status during execution.
    pub tmux_session: Option<&'a TmuxSession>,
}

/// Run a plan sequentially. The main conductor loop.
pub async fn run_plan(plan: &Plan, state: &mut PlanState, ctx: RunContext<'_>) -> Result<()> {
    let RunContext {
        launcher,
        check_engine,
        notifier,
        budget,
        cancel,
        cwd,
        interactive,
        json_events,
        tmux_session,
    } = ctx;
    let order = topo_sort(plan)?;
    let total_phases = order.len();
    let mut event_log = EventLogger::new(cwd, &plan.name).with_stdout_json(json_events);

    // Initialize edda ledger if available
    edda::ensure_init(cwd);

    // Detect stale phases from previous run
    detect_stale_phases(state, plan);

    // Record plan start
    if state.started_at.is_none() {
        state.started_at = Some(now_rfc3339());
        state.plan_status = PlanStatus::Running;
        save_state(cwd, state)?;
        event_log::write_runner_status(cwd, state, None);
        write_brief(cwd, state, None);
        event_log.record(Event::PlanStart {
            plan_name: plan.name.clone(),
            phase_count: total_phases,
        });
    }

    loop {
        // 1. Check termination
        if cancel.is_cancelled() {
            println!("Shutdown. Run `edda conduct run` to resume.");
            break;
        }

        update_plan_status(state);

        if state.plan_status == PlanStatus::Aborted {
            break;
        }

        if is_plan_blocked(state) {
            let failed = state
                .phases
                .iter()
                .find(|p| p.status == PhaseStatus::Failed || p.status == PhaseStatus::Stale);
            let failed_id = failed.map(|f| f.id.clone()).unwrap_or_default();

            if interactive {
                match prompt_blocked_action(&failed_id) {
                    BlockedAction::Retry => {
                        let current = state
                            .get_phase(&failed_id)
                            .map(|p| p.status)
                            .unwrap_or(PhaseStatus::Failed);
                        let _ = transition(state, &failed_id, current, PhaseStatus::Pending, None);
                        state.plan_status = PlanStatus::Running;
                        save_state(cwd, state)?;
                        println!("  ↻ Retrying \"{failed_id}\"");
                        continue;
                    }
                    BlockedAction::Skip => {
                        let ps = state.get_phase_mut(&failed_id)?;
                        ps.status = PhaseStatus::Skipped;
                        ps.skip_reason = Some("manually skipped (interactive)".into());
                        state.plan_status = PlanStatus::Running;
                        save_state(cwd, state)?;
                        event_log.record(Event::PhaseSkipped {
                            phase_id: failed_id.clone(),
                            reason: "manually skipped (interactive)".into(),
                        });
                        println!("  ⊘ Skipped \"{failed_id}\"");
                        continue;
                    }
                    BlockedAction::Abort => {
                        state.plan_status = PlanStatus::Aborted;
                        state.aborted_at = Some(now_rfc3339());
                        save_state(cwd, state)?;
                        event_log.record(Event::PlanAborted {
                            phases_passed: state
                                .phases
                                .iter()
                                .filter(|p| p.status == PhaseStatus::Passed)
                                .count(),
                            phases_pending: state
                                .phases
                                .iter()
                                .filter(|p| p.status == PhaseStatus::Pending)
                                .count(),
                        });
                        println!("  ✗ Plan aborted.");
                        break;
                    }
                    BlockedAction::Quit => {
                        println!("Paused. Run `edda conduct run` to resume.");
                        break;
                    }
                }
            } else {
                notifier
                    .notify(&format!(
                        "Plan blocked: phase \"{}\" is {:?}. Use retry/skip/abort.",
                        failed_id,
                        failed.map(|f| f.status),
                    ))
                    .await;
                break;
            }
        }

        if budget.is_exhausted() {
            notifier.notify("Plan budget exhausted.").await;
            break;
        }

        // ── Verdict gate wait (GH-519 D3) ────────────────────────────
        // A phase holding AWAITING_VERDICT pauses the plan until a verdict
        // arrives. On restart this re-enters the wait WITHOUT re-running the
        // phase agent turn or checks; gate_sha comes from persisted state.
        if let Some(gated_id) = state
            .phases
            .iter()
            .find(|p| p.status == PhaseStatus::AwaitingVerdict)
            .map(|p| p.id.clone())
        {
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
                let at = ps.gate_entered_at.clone().unwrap_or_else(now_rfc3339);
                (sha, at)
            };
            let phase_num = order.iter().position(|id| id == &gated_id).unwrap_or(0) + 1;

            // D4: unmistakable surface naming subject + gate_sha + the exact
            // approve/reject commands to run.
            println!("\n⏸ [{phase_num}/{total_phases}] Phase \"{gated_id}\" AWAITING_VERDICT — waiting for an external verdict");
            println!("  subject:  {subject}");
            println!("  gate_sha: {gate_sha}");
            println!("  approve:  edda verdict approve {subject} --sha {gate_sha}");
            println!(
                "  reject:   edda verdict reject {subject} --sha {gate_sha} --comment \"<why>\""
            );
            event_log::write_runner_status(cwd, state, Some(&gated_id));
            write_brief(cwd, state, None);

            match wait_for_verdict(
                cwd,
                &subject,
                &gate_sha,
                phase.gate_timeout_sec,
                Some(&entered_at),
                &cancel,
            )
            .await
            {
                GateVerdict::Approved(record) => {
                    event_log.record(Event::VerdictReceived {
                        phase_id: gated_id.clone(),
                        decision: "approved".into(),
                        gate_sha: gate_sha.clone(),
                        comment: record.payload.comment.clone(),
                    });
                    transition(
                        state,
                        &gated_id,
                        PhaseStatus::AwaitingVerdict,
                        PhaseStatus::Passed,
                        Some(PhaseUpdate {
                            completed_at: Some(now_rfc3339()),
                            ..Default::default()
                        }),
                    )?;
                    record_verdict_metadata(state, &gated_id, &record.payload);
                    println!("  ✓ Verdict approved — phase \"{gated_id}\" passed");
                    if let Some(tmux) = tmux_session {
                        let _ = tmux.update_phase_status(&gated_id, "Passed");
                    }
                    edda::record_note(
                        cwd,
                        &format!("Gate \"{subject}\" approved (sha {gate_sha})"),
                        &["conductor", "verdict"],
                    );
                }
                GateVerdict::Rejected(record) => {
                    let comment = record.payload.comment.clone().unwrap_or_default();
                    event_log.record(Event::VerdictReceived {
                        phase_id: gated_id.clone(),
                        decision: "rejected".into(),
                        gate_sha: gate_sha.clone(),
                        comment: Some(comment.clone()),
                    });
                    println!("  ✗ Verdict rejected for \"{gated_id}\"");
                    edda::record_note(
                        cwd,
                        &format!("Gate \"{subject}\" rejected (sha {gate_sha})"),
                        &["conductor", "verdict"],
                    );

                    let (attempts, redispatches) = {
                        let ps = state.get_phase(&gated_id)?;
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
                            &gated_id,
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
                        record_verdict_metadata(state, &gated_id, &record.payload);
                        println!("  ✗ Phase \"{gated_id}\" failed: {message}");
                        if let Some(tmux) = tmux_session {
                            let _ = tmux.update_phase_status(&gated_id, "Failed");
                        }
                        edda::record_phase_failed(cwd, &gated_id, &message);
                        event_log.record(Event::PhaseFailed {
                            phase_id: gated_id.clone(),
                            attempt: attempts,
                            duration_ms: 0,
                            error: format!("verdict rejected: {message}"),
                        });
                    } else {
                        // Redispatch (D3): ONE more agent turn in the SAME
                        // session — do NOT increment attempt; the rejection
                        // comment becomes the prompt, prefixed with context.
                        // D6: count the cycle on its own persisted counter.
                        state.get_phase_mut(&gated_id)?.gate_redispatches += 1;
                        let session_id =
                            phase_session_id_attempt(&plan.name, &gated_id, attempts).to_string();
                        let prompt = build_redispatch_prompt(phase, &gated_id, &comment);
                        let plan_context =
                            build_plan_context_with_edda(plan, state, &gated_id, cwd);
                        transition(
                            state,
                            &gated_id,
                            PhaseStatus::AwaitingVerdict,
                            PhaseStatus::Running,
                            None,
                        )?;
                        save_state(cwd, state)?;
                        println!(
                            "  ↻ Redispatching one more turn in the same session ({session_id})"
                        );
                        let result = launcher
                            .run_phase(
                                phase,
                                &prompt,
                                &plan_context,
                                &session_id,
                                &phase_cwd,
                                cancel.child_token(),
                            )
                            .await?;
                        process_phase_result(
                            plan,
                            phase,
                            state,
                            &gated_id,
                            attempts,
                            result,
                            cwd,
                            &phase_cwd,
                            Instant::now(),
                            budget,
                            check_engine,
                            notifier,
                            &mut event_log,
                            tmux_session,
                        )
                        .await?;
                    }
                }
                GateVerdict::TimedOut => {
                    // D3: distinct "gate timed out" failure — NOT silent,
                    // NOT auto-approve.
                    let msg = format!(
                        "gate timed out: no verdict for \"{subject}\" (sha {gate_sha}) within {}s",
                        phase.gate_timeout_sec.unwrap_or(0)
                    );
                    transition(
                        state,
                        &gated_id,
                        PhaseStatus::AwaitingVerdict,
                        PhaseStatus::Failed,
                        Some(PhaseUpdate {
                            error: Some(ErrorInfo {
                                error_type: ErrorType::Timeout,
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
                        let _ = tmux.update_phase_status(&gated_id, "Failed");
                    }
                    edda::record_phase_failed(cwd, &gated_id, &msg);
                    event_log.record(Event::PhaseFailed {
                        phase_id: gated_id.clone(),
                        attempt: state.get_phase(&gated_id)?.attempts,
                        duration_ms: 0,
                        error: msg,
                    });
                }
                GateVerdict::Cancelled => {
                    // The loop top sees the cancelled token and shuts down;
                    // the phase stays AWAITING_VERDICT so a later
                    // `edda conduct run` resumes the wait (D3 restart).
                }
            }

            save_state(cwd, state)?;
            event_log::write_runner_status(cwd, state, Some(&gated_id));
            write_brief(cwd, state, None);
            continue;
        }

        // 2. Find next runnable phase
        let Some(phase_id) = find_next_phase(plan, state, &order) else {
            break; // all done or no runnable phase
        };
        let phase = plan
            .phases
            .iter()
            .find(|p| p.id == phase_id)
            .context("runnable phase not found in plan")?;
        let phase_state = state.get_phase_mut(&phase_id)?;
        let attempt = phase_state.attempts + 1;
        let phase_cwd = phase
            .cwd
            .as_deref()
            .or(plan.cwd.as_deref())
            .map(|p| cwd.join(p))
            .unwrap_or_else(|| cwd.to_path_buf());

        let phase_num = order.iter().position(|id| id == &phase_id).unwrap_or(0) + 1;

        // Clear retry_context on new attempt start (it was already consumed for prompt building)
        let retry_ctx = phase_state.retry_context.take();

        // 3. Transition: pending → running
        transition(
            state,
            &phase_id,
            PhaseStatus::Pending,
            PhaseStatus::Running,
            Some(PhaseUpdate {
                started_at: Some(now_rfc3339()),
                attempts: Some(attempt),
                checks: Some(vec![]),
                error: None,
                ..Default::default()
            }),
        )?;
        save_state(cwd, state)?;

        println!("\n▶ [{phase_num}/{total_phases}] Phase \"{phase_id}\" (attempt {attempt})");
        if let Some(tmux) = tmux_session {
            let _ = tmux.update_phase_status(&phase_id, "Running");
        }
        let phase_start = Instant::now();
        event_log.record(Event::PhaseStart {
            phase_id: phase_id.clone(),
            attempt,
        });
        event_log::write_runner_status(cwd, state, Some(&phase_id));
        write_brief(cwd, state, None);

        // 4. Build prompt + launch agent
        let prompt = build_phase_prompt(phase, retry_ctx.as_deref());
        let plan_context = build_plan_context_with_edda(plan, state, &phase_id, cwd);
        let session_id = phase_session_id_attempt(&plan.name, &phase_id, attempt).to_string();

        // Auto-claim scope for this phase (so peers can see it and send requests)
        write_phase_claim(cwd, &session_id, &phase_id);

        let result = launcher
            .run_phase(
                phase,
                &prompt,
                &plan_context,
                &session_id,
                &phase_cwd,
                cancel.child_token(),
            )
            .await?;

        // 5. Process result (shared with the post-rejection redispatch turn;
        // gated phases enter AWAITING_VERDICT here instead of Passed — D3)
        process_phase_result(
            plan,
            phase,
            state,
            &phase_id,
            attempt,
            result,
            cwd,
            &phase_cwd,
            phase_start,
            budget,
            check_engine,
            notifier,
            &mut event_log,
            tmux_session,
        )
        .await?;

        save_state(cwd, state)?;
    }

    // Plan completion check
    update_plan_status(state);
    if is_plan_complete(state) {
        state.plan_status = PlanStatus::Completed;
        state.completed_at = Some(now_rfc3339());
        save_state(cwd, state)?;
        let passed = state
            .phases
            .iter()
            .filter(|p| p.status == PhaseStatus::Passed)
            .count();
        println!("\n✓ Plan \"{}\" completed ({passed} passed)", plan.name);
        event_log.record(Event::PlanCompleted {
            phases_passed: passed,
            total_cost_usd: state.total_cost_usd,
        });
        notifier
            .notify(&format!(
                "Plan \"{}\" completed! {passed} phases passed.",
                plan.name
            ))
            .await;
    }

    event_log::write_runner_status(cwd, state, None);
    write_brief(cwd, state, None);
    Ok(())
}

/// Verdict gate helpers (GH-519 D3/D4/D5) ────────────────────────────────
/// Ledger poll interval while a gate waits for a verdict.
const GATE_POLL_SEC: u64 = 2;

/// D6: bound gate redispatch cycles with their own persisted counter, NOT
/// `attempt` (which D3 forbids incrementing on redispatch). A redispatch
/// turn is not guaranteed to produce a commit, so a re-entered gate can
/// wait on the same `(subject, gate_sha)` forever while `max_attempts`
/// never trips — this counter is the real loop bound. Exhausting it fails
/// the phase like `on_reject: halt`, with a distinct error naming the bound.
const MAX_GATE_REDISPATCHES: u32 = 3;

/// `<plan-name>/<phase-id>` — the subject an `edda verdict` targets (D1/D3).
fn gate_subject(plan_name: &str, phase_id: &str) -> String {
    format!("{plan_name}/{phase_id}")
}

/// Current git HEAD of `cwd` — the SHA a verdict must match (D3).
fn capture_git_head(cwd: &Path) -> Result<String> {
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
enum GateVerdict {
    Approved(VerdictRecord),
    Rejected(VerdictRecord),
    /// `gate_timeout_sec` elapsed with no matching verdict — NOT silent,
    /// NOT auto-approve (D3).
    TimedOut,
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
async fn wait_for_verdict(
    cwd: &Path,
    subject: &str,
    gate_sha: &str,
    timeout_sec: Option<u64>,
    entered_at: Option<&str>,
    cancel: &CancellationToken,
) -> GateVerdict {
    let deadline = timeout_sec.map(|t| {
        let base = entered_at
            .and_then(|s| {
                time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
            })
            .unwrap_or_else(time::OffsetDateTime::now_utc);
        base + time::Duration::seconds(t as i64)
    });

    loop {
        // Poll BEFORE the deadline check: a verdict recorded during this
        // wait (e.g. right after the gate_entered event) must be observed
        // at the last poll, not skipped by a deadline that fires first.
        // Poll the ledger. Best-effort: an unreadable or locked ledger simply
        // means "no verdict observed yet" this round.
        if let Ok(ledger) = edda_ledger::Ledger::open(cwd) {
            if let Ok(Some(record)) = ledger.latest_verdict_fresh(subject, gate_sha, entered_at) {
                return match record.payload.decision {
                    edda_core::VerdictDecision::Approved => GateVerdict::Approved(record),
                    edda_core::VerdictDecision::Rejected => GateVerdict::Rejected(record),
                };
            }
        }
        if let Some(deadline) = deadline {
            if time::OffsetDateTime::now_utc() >= deadline {
                return GateVerdict::TimedOut;
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(GATE_POLL_SEC)) => {}
            _ = cancel.cancelled() => return GateVerdict::Cancelled,
        }
    }
}

/// Record verdict metadata on the phase state (D3).
fn record_verdict_metadata(state: &mut PlanState, phase_id: &str, payload: &VerdictPayload) {
    if let Ok(ps) = state.get_phase_mut(phase_id) {
        ps.verdict_decision = Some(payload.decision.to_string());
        ps.verdict_actor = Some(payload.actor.clone());
        ps.verdict_comment = payload.comment.clone();
    }
}

/// Prompt for the redispatch turn after a rejected verdict (D3): the
/// rejection comment becomes the prompt, prefixed with brief context.
fn build_redispatch_prompt(
    phase: &crate::plan::schema::Phase,
    phase_id: &str,
    comment: &str,
) -> String {
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

/// Shared tail of a failed check run: transition Checking → Failed, print,
/// record to edda + event log, then apply the phase's on_fail policy.
#[allow(clippy::too_many_arguments)]
async fn fail_checking_phase(
    plan: &Plan,
    phase: &crate::plan::schema::Phase,
    state: &mut PlanState,
    phase_id: &str,
    cwd: &Path,
    check_result: &CheckRunResult,
    err_override: Option<&str>,
    elapsed: Duration,
    notifier: &dyn Notifier,
    event_log: &mut EventLogger,
    tmux_session: Option<&TmuxSession>,
) -> Result<()> {
    let (err_msg, error_info) = match err_override {
        Some(msg) => (
            msg.to_string(),
            Some(ErrorInfo {
                error_type: ErrorType::CheckFailed,
                message: msg.to_string(),
                retryable: true,
                check_index: None,
                timestamp: now_rfc3339(),
            }),
        ),
        None => {
            let msg = check_result
                .error
                .as_ref()
                .map(|e| e.message.as_str())
                .unwrap_or("check failed")
                .to_string();
            (msg, check_result.error.clone())
        }
    };
    // GH-529: a harness-side timeout is surfaced distinctly from a genuine
    // check failure — the fix differs (raise timeout_sec / inspect command
    // vs. let the agent fix the work).
    let is_timeout = error_info
        .as_ref()
        .is_some_and(|e| e.error_type == ErrorType::Timeout);
    transition(
        state,
        phase_id,
        PhaseStatus::Checking,
        PhaseStatus::Failed,
        Some(PhaseUpdate {
            checks: Some(check_result.results.clone()),
            error: error_info,
            ..Default::default()
        }),
    )?;
    if is_timeout {
        println!(
            "  ⏰ Phase \"{phase_id}\" check timed out ({}): {err_msg}",
            format_elapsed(elapsed),
        );
    } else {
        println!(
            "  ✗ Phase \"{phase_id}\" failed ({}): {err_msg}",
            format_elapsed(elapsed),
        );
    }
    if let Some(tmux) = tmux_session {
        let _ = tmux.update_phase_status(phase_id, "Failed");
    }
    edda::record_phase_failed(cwd, phase_id, &err_msg);
    event_log.record(Event::PhaseFailed {
        phase_id: phase_id.to_string(),
        attempt: state.get_phase(phase_id)?.attempts,
        duration_ms: elapsed.as_millis() as u64,
        error: err_msg,
    });
    handle_on_fail(
        plan,
        phase,
        state,
        phase_id,
        check_result,
        notifier,
        event_log,
    )
    .await;
    Ok(())
}

/// Process the result of one agent turn: classify it, run checks, resolve the
/// phase (Passed / AWAITING_VERDICT gate entry / Failed / Stale) and apply the
/// on_fail policy. Shared by the main loop and the post-rejection redispatch
/// turn (D3).
#[allow(clippy::too_many_arguments)]
async fn process_phase_result(
    plan: &Plan,
    phase: &crate::plan::schema::Phase,
    state: &mut PlanState,
    phase_id: &str,
    attempt: u32,
    result: PhaseResult,
    cwd: &Path,
    phase_cwd: &Path,
    phase_start: Instant,
    budget: &mut BudgetTracker,
    check_engine: &CheckEngine,
    notifier: &dyn Notifier,
    event_log: &mut EventLogger,
    tmux_session: Option<&TmuxSession>,
) -> Result<()> {
    match result {
        PhaseResult::AgentDone {
            cost_usd,
            result_text,
        } => {
            if let Some(cost) = cost_usd {
                budget.record(cost);
                state.total_cost_usd += cost;
            }

            // running → checking
            transition(
                state,
                phase_id,
                PhaseStatus::Running,
                PhaseStatus::Checking,
                None,
            )?;
            save_state(cwd, state)?;

            // Run checks
            let check_result = check_engine
                .run_all(
                    &phase.check,
                    state.get_phase(phase_id)?.started_at.as_deref(),
                )
                .await;

            if check_result.all_passed {
                if phase.gate.is_some() {
                    // D3: capture gate_sha = current git HEAD of the phase
                    // cwd, persist AWAITING_VERDICT, emit GateEntered + a
                    // notifier message, then wait (the loop's gate-wait branch).
                    match capture_git_head(phase_cwd) {
                        Ok(gate_sha) => {
                            let subject = gate_subject(&plan.name, phase_id);
                            transition(
                                state,
                                phase_id,
                                PhaseStatus::Checking,
                                PhaseStatus::AwaitingVerdict,
                                Some(PhaseUpdate {
                                    checks: Some(check_result.results),
                                    gate_sha: Some(gate_sha.clone()),
                                    gate_entered_at: Some(now_rfc3339()),
                                    ..Default::default()
                                }),
                            )?;
                            println!("\n  ⏸ Phase \"{phase_id}\" AWAITING_VERDICT — waiting for an external verdict");
                            println!("    subject:  {subject}");
                            println!("    gate_sha: {gate_sha}");
                            println!(
                                "    approve:  edda verdict approve {subject} --sha {gate_sha}"
                            );
                            println!("    reject:   edda verdict reject {subject} --sha {gate_sha} --comment \"<why>\"");
                            if let Some(tmux) = tmux_session {
                                let _ = tmux.update_phase_status(phase_id, "AwaitingVerdict");
                            }
                            event_log.record(Event::GateEntered {
                                phase_id: phase_id.to_string(),
                                subject: subject.clone(),
                                gate_sha: gate_sha.clone(),
                            });
                            notifier
                                .notify(&format!(
                                    "Phase \"{phase_id}\" is AWAITING_VERDICT. Approve: edda verdict approve {subject} --sha {gate_sha} | Reject: edda verdict reject {subject} --sha {gate_sha} --comment \"<why>\""
                                ))
                                .await;
                            edda::record_note(
                                cwd,
                                &format!(
                                    "Phase \"{phase_id}\" entered AWAITING_VERDICT (gate sha {gate_sha})"
                                ),
                                &["conductor", "gate"],
                            );
                        }
                        Err(e) => {
                            let msg = format!("failed to capture gate sha: {e}");
                            fail_checking_phase(
                                plan,
                                phase,
                                state,
                                phase_id,
                                cwd,
                                &check_result,
                                Some(&msg),
                                phase_start.elapsed(),
                                notifier,
                                event_log,
                                tmux_session,
                            )
                            .await?;
                        }
                    }
                } else {
                    transition(
                        state,
                        phase_id,
                        PhaseStatus::Checking,
                        PhaseStatus::Passed,
                        Some(PhaseUpdate {
                            completed_at: Some(now_rfc3339()),
                            checks: Some(check_result.results),
                            ..Default::default()
                        }),
                    )?;
                    let elapsed_ms = phase_start.elapsed().as_millis() as u64;
                    println!(
                        "  ✓ Phase \"{phase_id}\" passed ({})",
                        format_elapsed(phase_start.elapsed())
                    );
                    if let Some(tmux) = tmux_session {
                        let _ = tmux.update_phase_status(phase_id, "Passed");
                    }

                    // Record to edda ledger
                    edda::record_phase_done(cwd, phase_id, result_text.as_deref(), cost_usd);
                    event_log.record(Event::PhasePassed {
                        phase_id: phase_id.to_string(),
                        attempt,
                        duration_ms: elapsed_ms,
                        cost_usd,
                    });
                }
            } else {
                fail_checking_phase(
                    plan,
                    phase,
                    state,
                    phase_id,
                    cwd,
                    &check_result,
                    None,
                    phase_start.elapsed(),
                    notifier,
                    event_log,
                    tmux_session,
                )
                .await?;
            }
        }
        PhaseResult::Timeout => {
            transition(
                state,
                phase_id,
                PhaseStatus::Running,
                PhaseStatus::Stale,
                Some(PhaseUpdate {
                    error: Some(ErrorInfo {
                        error_type: ErrorType::Timeout,
                        message: format!("phase \"{phase_id}\" timed out"),
                        retryable: true,
                        check_index: None,
                        timestamp: now_rfc3339(),
                    }),
                    ..Default::default()
                }),
            )?;
            let elapsed_ms = phase_start.elapsed().as_millis() as u64;
            println!(
                "  ⏰ Phase \"{phase_id}\" timed out ({})",
                format_elapsed(phase_start.elapsed())
            );
            if let Some(tmux) = tmux_session {
                let _ = tmux.update_phase_status(phase_id, "Stale");
            }
            edda::record_phase_failed(cwd, phase_id, "timed out");
            event_log.record(Event::PhaseFailed {
                phase_id: phase_id.to_string(),
                attempt,
                duration_ms: elapsed_ms,
                error: "timed out".into(),
            });
        }
        PhaseResult::AgentCrash { error } => {
            transition(
                state,
                phase_id,
                PhaseStatus::Running,
                PhaseStatus::Failed,
                Some(PhaseUpdate {
                    error: Some(ErrorInfo {
                        error_type: ErrorType::AgentCrash,
                        message: error.clone(),
                        retryable: true,
                        check_index: None,
                        timestamp: now_rfc3339(),
                    }),
                    ..Default::default()
                }),
            )?;
            let elapsed_ms = phase_start.elapsed().as_millis() as u64;
            println!(
                "  ✗ Phase \"{phase_id}\" crashed ({}): {error}",
                format_elapsed(phase_start.elapsed())
            );
            if let Some(tmux) = tmux_session {
                let _ = tmux.update_phase_status(phase_id, "Failed");
            }
            edda::record_phase_failed(cwd, phase_id, &error);
            event_log.record(Event::PhaseFailed {
                phase_id: phase_id.to_string(),
                attempt,
                duration_ms: elapsed_ms,
                error: error.clone(),
            });
            // For crash, use empty check results
            let empty_result = CheckRunResult {
                all_passed: false,
                results: vec![],
                error: None,
            };
            handle_on_fail(
                plan,
                phase,
                state,
                phase_id,
                &empty_result,
                notifier,
                event_log,
            )
            .await;
        }
        PhaseResult::MaxTurns { cost_usd } | PhaseResult::BudgetExceeded { cost_usd } => {
            if let Some(cost) = cost_usd {
                budget.record(cost);
                state.total_cost_usd += cost;
            }
            let elapsed_ms = phase_start.elapsed().as_millis() as u64;
            let msg = format!("{result:?}");
            transition(
                state,
                phase_id,
                PhaseStatus::Running,
                PhaseStatus::Failed,
                Some(PhaseUpdate {
                    error: Some(ErrorInfo {
                        error_type: ErrorType::BudgetExceeded,
                        message: msg.clone(),
                        retryable: false,
                        check_index: None,
                        timestamp: now_rfc3339(),
                    }),
                    ..Default::default()
                }),
            )?;
            event_log.record(Event::PhaseFailed {
                phase_id: phase_id.to_string(),
                attempt,
                duration_ms: elapsed_ms,
                error: msg,
            });
        }
    }
    Ok(())
}

async fn handle_on_fail(
    plan: &Plan,
    phase: &crate::plan::schema::Phase,
    state: &mut PlanState,
    phase_id: &str,
    check_result: &CheckRunResult,
    notifier: &dyn Notifier,
    event_log: &mut EventLogger,
) {
    let on_fail = phase.on_fail.unwrap_or(plan.on_fail);

    match on_fail {
        OnFail::AutoRetry => {
            // GH-529: a check timeout is a property of the harness, not of
            // the agent's work — every retry hits the same wall at the same
            // second, so auto-retry must not burn the ladder on it. Halt
            // and report with an actionable message instead.
            if check_result
                .error
                .as_ref()
                .is_some_and(|e| e.error_type == ErrorType::Timeout)
            {
                notifier
                    .notify(&format!(
                        "Phase \"{phase_id}\" check timed out — auto_retry skipped \
                         (retrying cannot change the outcome). Raise the check's \
                         timeout_sec or inspect the command."
                    ))
                    .await;
                return;
            }
            let max = phase.max_attempts.unwrap_or(plan.max_attempts);
            let (attempts, should_retry) = {
                let ps = state
                    .get_phase_mut(phase_id)
                    .expect("phase must exist in state");
                if ps.attempts < max {
                    let error_context = format_check_failures(&check_result.results);
                    ps.retry_context = Some(error_context);
                    (ps.attempts, true)
                } else {
                    (ps.attempts, false)
                }
            };
            if should_retry {
                let _ = transition(
                    state,
                    phase_id,
                    PhaseStatus::Failed,
                    PhaseStatus::Pending,
                    None,
                );
                println!("  ↻ Auto-retrying ({attempts}/{max})");
            } else {
                notifier
                    .notify(&format!(
                        "Phase \"{phase_id}\" failed after {max} attempts. Retry, skip, or abort?"
                    ))
                    .await;
            }
        }
        OnFail::Skip => {
            let ps = state
                .get_phase_mut(phase_id)
                .expect("phase must exist in state");
            ps.status = PhaseStatus::Skipped;
            ps.skip_reason = Some("auto-skipped by on_fail policy".into());
            event_log.record(Event::PhaseSkipped {
                phase_id: phase_id.to_string(),
                reason: "auto-skipped by on_fail policy".into(),
            });
            println!("  → Auto-skipped (on_fail: skip)");
        }
        OnFail::Abort => {
            state.plan_status = PlanStatus::Aborted;
            state.aborted_at = Some(now_rfc3339());
            event_log.record(Event::PlanAborted {
                phases_passed: state
                    .phases
                    .iter()
                    .filter(|p| p.status == PhaseStatus::Passed)
                    .count(),
                phases_pending: state
                    .phases
                    .iter()
                    .filter(|p| p.status == PhaseStatus::Pending)
                    .count(),
            });
            println!("  → Plan aborted (on_fail: abort)");
        }
        OnFail::Ask => {
            notifier
                .notify(&format!(
                    "Phase \"{phase_id}\" failed. Retry, skip, or abort?"
                ))
                .await;
        }
    }
}

/// Build the full prompt for a phase, including retry context if any.
fn build_phase_prompt(phase: &crate::plan::schema::Phase, retry_context: Option<&str>) -> String {
    let mut prompt = String::new();
    if let Some(ctx) = &phase.context {
        prompt.push_str(ctx);
        prompt.push_str("\n\n");
    }
    prompt.push_str(&phase.prompt);

    // Layer 1: append self-check instruction if phase has checks
    if !phase.check.is_empty() {
        prompt.push_str("\n\n## Verification\n");
        prompt.push_str(
            "After completing the task, run these checks yourself and fix any failures:\n",
        );
        for check in &phase.check {
            match check {
                CheckSpec::CmdSucceeds { cmd, .. } => {
                    prompt.push_str(&format!("- `{cmd}`\n"));
                }
                CheckSpec::FileExists { path } => {
                    prompt.push_str(&format!("- Verify `{path}` exists\n"));
                }
                CheckSpec::FileContains { path, pattern } => {
                    prompt.push_str(&format!("- Verify `{path}` contains \"{pattern}\"\n"));
                }
                // GitClean, EddaEvent, WaitUntil are not actionable by the agent
                _ => {}
            }
        }
        prompt.push_str("Repeat until all pass. Do not stop with failing checks.\n");
    }

    // Layer 2: inject previous failure details on retry
    if let Some(error) = retry_context {
        prompt.push_str("\n\n## Previous Attempt Failed\n");
        prompt.push_str(error);
        prompt.push_str("\n\nYour previous changes are still on disk. Fix the issues above.");
    }

    // Layer 3: write-back reminder for decision recording + cross-phase messaging
    prompt.push_str("\n\n## Decision Write-Back\n");
    prompt.push_str(
        "Record architectural decisions from this phase: \
         `edda decide \"key=value\" --reason \"why\"`\n\
         Message another phase: `edda request \"phase-label\" \"message\"`\n",
    );

    prompt
}

fn format_check_failures(results: &[CheckResult]) -> String {
    let mut out = String::new();
    for r in results {
        let icon = match r.status {
            CheckStatus::Passed => "✓",
            CheckStatus::Failed => "✗",
            _ => "○",
        };
        out.push_str(&format!(
            "{icon} {}: {}\n",
            r.check_type,
            r.detail.as_deref().unwrap_or("(no detail)"),
        ));
    }
    out
}

/// Build plan progress context with edda decision history for --append-system-prompt.
fn build_plan_context_with_edda(
    plan: &Plan,
    state: &PlanState,
    current_phase: &str,
    cwd: &Path,
) -> String {
    let base = build_plan_context(plan, state, current_phase);
    let edda_ctx = edda::get_context(cwd);
    if edda_ctx.is_empty() {
        base
    } else {
        format!("{base}\n\n## Decision History (from edda)\n{edda_ctx}")
    }
}

/// Build plan progress context for --append-system-prompt.
fn build_plan_context(plan: &Plan, state: &PlanState, current_phase: &str) -> String {
    let mut ctx = String::new();

    // Purpose first — keeps every agent aligned with user intent
    if let Some(purpose) = plan.purpose.as_deref() {
        if !purpose.is_empty() {
            ctx.push_str(&format!("## Purpose\n{purpose}\n\n"));
        }
    }

    ctx.push_str(&format!("## Plan: {}\n", plan.name));
    for ps in &state.phases {
        let icon = match ps.status {
            PhaseStatus::Passed => "✓",
            PhaseStatus::Failed => "✗",
            PhaseStatus::Running | PhaseStatus::Checking => "▶",
            PhaseStatus::AwaitingVerdict => "⏸",
            PhaseStatus::Skipped => "⊘",
            PhaseStatus::Stale => "⏰",
            PhaseStatus::Pending => {
                if ps.id == current_phase {
                    "▶"
                } else {
                    "○"
                }
            }
        };
        ctx.push_str(&format!("{icon} {}\n", ps.id));
    }
    ctx
}

enum BlockedAction {
    Retry,
    Skip,
    Abort,
    Quit,
}

fn prompt_blocked_action(phase_id: &str) -> BlockedAction {
    use std::io::{BufRead, Write};
    println!("\n  Phase \"{phase_id}\" is blocked.\n");
    println!("  [R] Retry   [S] Skip   [A] Abort   [Q] Quit (resume later)");
    loop {
        print!("  > ");
        let _ = std::io::stdout().flush();
        let mut input = String::new();
        match std::io::stdin().lock().read_line(&mut input) {
            Ok(0) | Err(_) => return BlockedAction::Quit, // EOF or error
            _ => {}
        }
        match input.trim().to_lowercase().as_str() {
            "r" | "retry" => return BlockedAction::Retry,
            "s" | "skip" => return BlockedAction::Skip,
            "a" | "abort" => return BlockedAction::Abort,
            "q" | "quit" => return BlockedAction::Quit,
            _ => println!("  Invalid choice. Enter R, S, A, or Q."),
        }
    }
}

fn format_elapsed(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m{}s", secs / 60, secs % 60)
    }
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// Write a claim event to coordination.jsonl for a conductor phase.
/// Written directly (no edda-bridge-claude dependency) since the format is simple.
fn write_phase_claim(cwd: &Path, session_id: &str, phase_id: &str) {
    let project_id = edda_store::project_id(cwd);
    let state_dir = edda_store::project_dir(&project_id).join("state");
    let coord_path = state_dir.join("coordination.jsonl");
    let event = serde_json::json!({
        "ts": now_rfc3339(),
        "session_id": session_id,
        "event_type": "claim",
        "payload": { "label": phase_id, "paths": serde_json::Value::Array(vec![]) }
    });
    if let Ok(line) = serde_json::to_string(&event) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&coord_path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::launcher::{MockLauncher, PhaseResult};
    use crate::plan::parser::parse_plan;
    use crate::runner::notify::CollectNotifier;

    async fn run_test_plan(yaml: &str, launcher: &dyn AgentLauncher) -> (PlanState, Vec<String>) {
        let plan = parse_plan(yaml).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut state = PlanState::from_plan(&plan, "test.yaml");
        let engine = CheckEngine::new(dir.path().to_path_buf());
        let notifier = CollectNotifier::new();
        let mut budget = BudgetTracker::new(plan.budget_usd);
        let cancel = CancellationToken::new();

        run_plan(
            &plan,
            &mut state,
            RunContext {
                launcher,
                check_engine: &engine,
                notifier: &notifier,
                budget: &mut budget,
                cancel,
                cwd: dir.path(),
                interactive: false,
                json_events: false,
                tmux_session: None,
            },
        )
        .await
        .unwrap();

        let msgs = notifier.messages();
        (state, msgs)
    }

    #[tokio::test]
    async fn single_phase_passes() {
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "do it"
"#;
        let launcher = MockLauncher::new();
        let (state, msgs) = run_test_plan(yaml, &launcher).await;

        assert_eq!(state.plan_status, PlanStatus::Completed);
        assert_eq!(state.phases[0].status, PhaseStatus::Passed);
        assert!(msgs.iter().any(|m| m.contains("completed")));
    }

    #[tokio::test]
    async fn two_phases_sequential() {
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "first"
  - id: b
    prompt: "second"
    depends_on: [a]
"#;
        let launcher = MockLauncher::new();
        let (state, _) = run_test_plan(yaml, &launcher).await;

        assert_eq!(state.plan_status, PlanStatus::Completed);
        assert!(state.phases.iter().all(|p| p.status == PhaseStatus::Passed));
    }

    #[tokio::test]
    async fn phase_crash_with_auto_retry() {
        let yaml = r#"
name: test
max_attempts: 2
on_fail: auto_retry
phases:
  - id: a
    prompt: "crash then succeed"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![
                PhaseResult::AgentCrash {
                    error: "oops".into(),
                },
                PhaseResult::AgentDone {
                    cost_usd: Some(0.5),
                    result_text: None,
                },
            ],
        );
        let (state, _) = run_test_plan(yaml, &launcher).await;

        assert_eq!(state.plan_status, PlanStatus::Completed);
        assert_eq!(state.phases[0].status, PhaseStatus::Passed);
        assert_eq!(state.phases[0].attempts, 2);
    }

    #[tokio::test]
    async fn phase_crash_exhausts_retries() {
        let yaml = r#"
name: test
max_attempts: 2
on_fail: auto_retry
phases:
  - id: a
    prompt: "always crash"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![
                PhaseResult::AgentCrash { error: "1".into() },
                PhaseResult::AgentCrash { error: "2".into() },
            ],
        );
        let (state, msgs) = run_test_plan(yaml, &launcher).await;

        assert_eq!(state.plan_status, PlanStatus::Blocked);
        assert_eq!(state.phases[0].status, PhaseStatus::Failed);
        assert!(msgs.iter().any(|m| m.contains("failed after 2 attempts")));
    }

    #[tokio::test]
    async fn on_fail_skip() {
        let yaml = r#"
name: test
on_fail: skip
phases:
  - id: a
    prompt: "crash"
  - id: b
    prompt: "should still run"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![PhaseResult::AgentCrash {
                error: "boom".into(),
            }],
        );
        let (state, _) = run_test_plan(yaml, &launcher).await;

        assert_eq!(state.phases[0].status, PhaseStatus::Skipped);
        assert_eq!(state.phases[1].status, PhaseStatus::Passed);
        assert_eq!(state.plan_status, PlanStatus::Completed);
    }

    #[tokio::test]
    async fn on_fail_abort() {
        let yaml = r#"
name: test
on_fail: abort
phases:
  - id: a
    prompt: "crash"
  - id: b
    prompt: "never runs"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![PhaseResult::AgentCrash {
                error: "boom".into(),
            }],
        );
        let (state, _) = run_test_plan(yaml, &launcher).await;

        assert_eq!(state.plan_status, PlanStatus::Aborted);
        assert_eq!(state.phases[0].status, PhaseStatus::Failed);
        assert_eq!(state.phases[1].status, PhaseStatus::Pending);
    }

    #[tokio::test]
    async fn budget_exhaustion_stops() {
        let yaml = r#"
name: test
budget_usd: 0.5
phases:
  - id: a
    prompt: "expensive"
  - id: b
    prompt: "should not run"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![PhaseResult::AgentDone {
                cost_usd: Some(1.0),
                result_text: None,
            }],
        );
        let (state, msgs) = run_test_plan(yaml, &launcher).await;

        assert_eq!(state.phases[0].status, PhaseStatus::Passed);
        assert_eq!(state.phases[1].status, PhaseStatus::Pending);
        assert!(msgs.iter().any(|m| m.contains("budget exhausted")));
    }

    #[tokio::test]
    async fn check_failure_triggers_auto_retry() {
        // Agent succeeds (AgentDone) but check fails (file doesn't exist).
        // Verifies that check failure → auto_retry, not just agent crash.
        let yaml = r#"
name: test
max_attempts: 2
phases:
  - id: a
    prompt: "make file"
    check:
      - file_exists: "output.txt"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![
                PhaseResult::AgentDone {
                    cost_usd: Some(0.1),
                    result_text: None,
                },
                PhaseResult::AgentDone {
                    cost_usd: Some(0.1),
                    result_text: None,
                },
            ],
        );
        let (state, msgs) = run_test_plan(yaml, &launcher).await;

        // Both attempts: agent done → check fails → auto-retry → exhausts
        assert_eq!(state.phases[0].status, PhaseStatus::Failed);
        assert_eq!(state.phases[0].attempts, 2);
        assert!(msgs.iter().any(|m| m.contains("failed after 2 attempts")));
    }

    /// GH-529: a timed-out check must NOT burn the auto_retry ladder — it
    /// is a harness property, so every retry would hit the same wall. One
    /// attempt, distinct Timeout error, halt-and-report.
    #[tokio::test]
    async fn check_timeout_does_not_burn_retry_ladder() {
        // A command that never finishes inside its 1s budget.
        #[cfg(windows)]
        let cmd = "while ($true) { Start-Sleep -Milliseconds 100 }";
        #[cfg(not(windows))]
        let cmd = "sleep 30";
        let yaml = format!(
            r#"
name: test
max_attempts: 3
phases:
  - id: a
    prompt: "do it"
    check:
      - type: cmd_succeeds
        cmd: "{cmd}"
        timeout_sec: 1
"#
        );
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![PhaseResult::AgentDone {
                cost_usd: Some(0.1),
                result_text: None,
            }],
        );
        let (state, msgs) = run_test_plan(&yaml, &launcher).await;

        let phase = &state.phases[0];
        assert_eq!(phase.status, PhaseStatus::Failed);
        assert_eq!(
            phase.attempts, 1,
            "timeout must not consume the retry ladder"
        );
        assert_eq!(launcher.call_count("a"), 1, "no re-dispatch on timeout");
        let err = phase.error.as_ref().expect("timeout must set an error");
        assert_eq!(err.error_type, ErrorType::Timeout);
        assert!(!err.retryable);
        assert!(
            err.message.contains("timed out"),
            "persisted error must name the timeout, got: {}",
            err.message
        );
        assert!(
            msgs.iter()
                .any(|m| m.contains("timed out") && m.contains("auto_retry skipped")),
            "actionable halt-and-report message, got: {msgs:?}"
        );
    }

    #[tokio::test]
    async fn cancellation_stops_runner() {
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "x"
  - id: b
    prompt: "x"
"#;
        let plan = parse_plan(yaml).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut state = PlanState::from_plan(&plan, "test.yaml");
        let engine = CheckEngine::new(dir.path().to_path_buf());
        let notifier = CollectNotifier::new();
        let mut budget = BudgetTracker::new(None);
        let cancel = CancellationToken::new();
        cancel.cancel(); // Cancel immediately

        let launcher = MockLauncher::new();

        run_plan(
            &plan,
            &mut state,
            RunContext {
                launcher: &launcher,
                check_engine: &engine,
                notifier: &notifier,
                budget: &mut budget,
                cancel,
                cwd: dir.path(),
                interactive: false,
                json_events: false,
                tmux_session: None,
            },
        )
        .await
        .unwrap();

        // Should stop without running any phases
        assert!(state
            .phases
            .iter()
            .all(|p| p.status == PhaseStatus::Pending));
    }

    #[test]
    fn build_prompt_basic() {
        let plan = parse_plan("name: t\nphases:\n  - id: a\n    prompt: do it\n").unwrap();
        let prompt = build_phase_prompt(&plan.phases[0], None);
        assert!(prompt.contains("do it"));
        assert!(!prompt.contains("Verification"));
    }

    #[test]
    fn build_prompt_with_checks() {
        let yaml = r#"
name: t
phases:
  - id: a
    prompt: do it
    check:
      - cmd_succeeds: "cargo test"
      - file_exists: "output.txt"
"#;
        let plan = parse_plan(yaml).unwrap();
        let prompt = build_phase_prompt(&plan.phases[0], None);
        assert!(prompt.contains("Verification"));
        assert!(prompt.contains("`cargo test`"));
        assert!(prompt.contains("`output.txt`"));
    }

    #[test]
    fn build_prompt_with_retry_context() {
        let plan = parse_plan("name: t\nphases:\n  - id: a\n    prompt: do it\n").unwrap();
        let prompt = build_phase_prompt(&plan.phases[0], Some("✗ cmd_succeeds: exit 1"));
        assert!(prompt.contains("Previous Attempt Failed"));
        assert!(prompt.contains("exit 1"));
    }

    #[test]
    fn format_check_failures_output() {
        let results = vec![
            CheckResult {
                check_type: "file_exists".into(),
                status: CheckStatus::Passed,
                detail: None,
                duration_ms: 0,
            },
            CheckResult {
                check_type: "cmd_succeeds".into(),
                status: CheckStatus::Failed,
                detail: Some("exit 1: test failed".into()),
                duration_ms: 100,
            },
        ];
        let out = format_check_failures(&results);
        assert!(out.contains("✓ file_exists"));
        assert!(out.contains("✗ cmd_succeeds: exit 1"));
    }

    #[test]
    fn build_plan_context_includes_purpose() {
        let yaml = r#"
name: todo-app
purpose: "Simple todo app for demo, keep it minimal"
phases:
  - id: db
    prompt: "schema"
  - id: api
    prompt: "endpoints"
"#;
        let plan = parse_plan(yaml).unwrap();
        let state = PlanState::from_plan(&plan, "test.yaml");
        let ctx = build_plan_context(&plan, &state, "db");

        assert!(
            ctx.contains("## Purpose"),
            "missing Purpose section in:\n{ctx}"
        );
        assert!(
            ctx.contains("Simple todo app"),
            "missing purpose text in:\n{ctx}"
        );
        // Purpose comes before Plan
        let purpose_pos = ctx.find("## Purpose").unwrap();
        let plan_pos = ctx.find("## Plan:").unwrap();
        assert!(purpose_pos < plan_pos, "Purpose should come before Plan");
    }

    #[test]
    fn build_plan_context_no_purpose() {
        let yaml = "name: t\nphases:\n  - id: a\n    prompt: do it\n";
        let plan = parse_plan(yaml).unwrap();
        let state = PlanState::from_plan(&plan, "test.yaml");
        let ctx = build_plan_context(&plan, &state, "a");

        assert!(
            !ctx.contains("## Purpose"),
            "should not have Purpose when not set"
        );
        assert!(
            ctx.starts_with("## Plan:"),
            "should start with Plan when no purpose"
        );
    }

    #[test]
    fn build_prompt_includes_write_back() {
        let plan = parse_plan("name: t\nphases:\n  - id: a\n    prompt: do it\n").unwrap();
        let prompt = build_phase_prompt(&plan.phases[0], None);
        assert!(prompt.contains("Decision Write-Back"));
        assert!(prompt.contains("edda decide"));
        assert!(prompt.contains("edda request"));
    }

    // ── Event log integration tests ──

    /// Helper that returns the tempdir so callers can inspect files.
    async fn run_test_plan_with_dir(
        yaml: &str,
        launcher: &dyn AgentLauncher,
    ) -> (PlanState, tempfile::TempDir) {
        let plan = parse_plan(yaml).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut state = PlanState::from_plan(&plan, "test.yaml");
        let engine = CheckEngine::new(dir.path().to_path_buf());
        let notifier = CollectNotifier::new();
        let mut budget = BudgetTracker::new(plan.budget_usd);
        let cancel = CancellationToken::new();

        run_plan(
            &plan,
            &mut state,
            RunContext {
                launcher,
                check_engine: &engine,
                notifier: &notifier,
                budget: &mut budget,
                cancel,
                cwd: dir.path(),
                interactive: false,
                json_events: false,
                tmux_session: None,
            },
        )
        .await
        .unwrap();

        (state, dir)
    }

    fn read_events(dir: &Path, plan_name: &str) -> Vec<serde_json::Value> {
        let path = dir
            .join(".edda")
            .join("conductor")
            .join(plan_name)
            .join("events.jsonl");
        if !path.exists() {
            return vec![];
        }
        std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn read_runner_status(dir: &Path, plan_name: &str) -> Option<serde_json::Value> {
        let path = dir
            .join(".edda")
            .join("conductor")
            .join(plan_name)
            .join("runner-status.json");
        if !path.exists() {
            return None;
        }
        Some(serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap())
    }

    #[tokio::test]
    async fn events_jsonl_written_for_passing_plan() {
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "do it"
"#;
        let launcher = MockLauncher::new();
        let (_state, dir) = run_test_plan_with_dir(yaml, &launcher).await;

        let events = read_events(dir.path(), "test");
        // Expect: PlanStart, PhaseStart, PhasePassed, PlanCompleted
        assert_eq!(events.len(), 4, "events: {events:?}");
        assert_eq!(events[0]["type"], "plan_start");
        assert_eq!(events[0]["phase_count"], 1);
        assert_eq!(events[1]["type"], "phase_start");
        assert_eq!(events[1]["phase_id"], "a");
        assert_eq!(events[2]["type"], "phase_passed");
        assert_eq!(events[2]["phase_id"], "a");
        assert_eq!(events[3]["type"], "plan_completed");
        // Seq increments
        assert_eq!(events[0]["seq"], 0);
        assert_eq!(events[3]["seq"], 3);
    }

    #[tokio::test]
    async fn events_jsonl_records_crash_failure() {
        let yaml = r#"
name: test
on_fail: abort
phases:
  - id: a
    prompt: "crash"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![PhaseResult::AgentCrash {
                error: "boom".into(),
            }],
        );
        let (_state, dir) = run_test_plan_with_dir(yaml, &launcher).await;

        let events = read_events(dir.path(), "test");
        // PlanStart, PhaseStart, PhaseFailed, PlanAborted
        assert_eq!(events.len(), 4, "events: {events:?}");
        assert_eq!(events[2]["type"], "phase_failed");
        assert_eq!(events[2]["error"], "boom");
        assert_eq!(events[3]["type"], "plan_aborted");
    }

    #[tokio::test]
    async fn events_jsonl_records_skip() {
        let yaml = r#"
name: test
on_fail: skip
phases:
  - id: a
    prompt: "crash"
  - id: b
    prompt: "should run"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results("a", vec![PhaseResult::AgentCrash { error: "x".into() }]);
        let (_state, dir) = run_test_plan_with_dir(yaml, &launcher).await;

        let events = read_events(dir.path(), "test");
        let types: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
        assert!(types.contains(&"phase_skipped"), "types: {types:?}");
        assert!(types.contains(&"plan_completed"), "types: {types:?}");
    }

    #[tokio::test]
    async fn runner_status_written_after_run() {
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "do it"
"#;
        let launcher = MockLauncher::new();
        let (_state, dir) = run_test_plan_with_dir(yaml, &launcher).await;

        let status = read_runner_status(dir.path(), "test").expect("runner-status.json missing");
        assert_eq!(status["plan"], "test");
        assert_eq!(status["status"], "completed");
        assert!(status["completed"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("a")));
    }

    // ── Verdict gate (GH-519 D3/D4) ─────────────────────────────────────

    use edda_core::event::new_verdict_event;
    use edda_core::{VerdictDecision, VerdictPayload};
    use edda_ledger::lock::WorkspaceLock;
    use edda_ledger::Ledger;

    /// Init a git repo in `dir` and return its HEAD (the gate_sha source).
    fn init_git_repo(dir: &Path) -> String {
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .expect("git must be available");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
            out
        };
        run(&["init"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "test"]);
        run(&["commit", "--allow-empty", "-m", "init"]);
        String::from_utf8_lossy(&run(&["rev-parse", "HEAD"]).stdout)
            .trim()
            .to_string()
    }

    /// Record a verdict directly into the ledger at `root` (what
    /// `edda verdict approve|reject` writes). Retries to ride out SQLite
    /// busy windows while the runner polls concurrently.
    fn record_verdict(
        root: &Path,
        subject: &str,
        sha: &str,
        decision: VerdictDecision,
        comment: Option<&str>,
    ) {
        for _ in 0..50 {
            let opened = Ledger::open(root).and_then(|ledger| {
                let _lock = WorkspaceLock::acquire(&ledger.paths)?;
                let branch = ledger.head_branch()?;
                let parent = ledger.last_event_hash()?;
                let payload = VerdictPayload {
                    subject: subject.to_string(),
                    decision,
                    sha: sha.to_string(),
                    comment: comment.map(Into::into),
                    actor: "tester".into(),
                };
                let event = new_verdict_event(&branch, parent.as_deref(), &payload)?;
                ledger.append_event(&event)
            });
            if opened.is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("failed to record verdict into the ledger");
    }

    /// Poll the runner's events.jsonl until `n` gate_entered events have been
    /// observed; returns their gate shas in order.
    async fn wait_for_gate_events(root: &Path, plan_name: &str, n: usize) -> Vec<String> {
        let path = root
            .join(".edda")
            .join("conductor")
            .join(plan_name)
            .join("events.jsonl");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let mut shas = Vec::new();
            if let Ok(content) = std::fs::read_to_string(&path) {
                for line in content.lines().filter(|l| !l.trim().is_empty()) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                        if v["type"] == "gate_entered" {
                            shas.push(v["gate_sha"].as_str().unwrap().to_string());
                        }
                    }
                }
            }
            if shas.len() >= n {
                return shas;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "only {} gate_entered events observed, wanted {n}",
                shas.len()
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Spawn `run_plan` on a background task; returns state, notifier and
    /// launcher through the handle so the test can inspect them afterwards.
    fn spawn_runner(
        yaml: &'static str,
        root: std::path::PathBuf,
        launcher: MockLauncher,
        state: PlanState,
    ) -> tokio::task::JoinHandle<anyhow::Result<(PlanState, CollectNotifier, MockLauncher)>> {
        tokio::spawn(async move {
            let plan = parse_plan(yaml).unwrap();
            let mut state = state;
            let engine = CheckEngine::new(root.clone());
            let notifier = CollectNotifier::new();
            let mut budget = BudgetTracker::new(plan.budget_usd);
            let cancel = CancellationToken::new();
            let result = run_plan(
                &plan,
                &mut state,
                RunContext {
                    launcher: &launcher,
                    check_engine: &engine,
                    notifier: &notifier,
                    budget: &mut budget,
                    cancel,
                    cwd: &root,
                    interactive: false,
                    json_events: false,
                    tmux_session: None,
                },
            )
            .await;
            result.map(|_| (state, notifier, launcher))
        })
    }

    fn fresh_root(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("edda_gate_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // The gate wait polls the ledger; tests that record verdicts need it.
        Ledger::ensure_initialized(&dir).unwrap();
        dir
    }

    /// Outer deadline for gate tests: a regression that makes a gate wait
    /// forever must fail loudly here, not hang CI.
    const GATE_TEST_DEADLINE: Duration = Duration::from_secs(30);

    const GATED_YAML: &str = r#"
name: gated
phases:
  - id: a
    prompt: "do it"
    gate: verdict
  - id: b
    prompt: "after"
    depends_on: [a]
"#;

    #[tokio::test]
    async fn gate_pauses_then_approve_resumes() {
        let root = fresh_root("approve");
        let head = init_git_repo(&root);
        let launcher = MockLauncher::new();
        let plan = parse_plan(GATED_YAML).unwrap();
        let state = PlanState::from_plan(&plan, "test.yaml");
        let handle = spawn_runner(GATED_YAML, root.clone(), launcher, state);

        // The gate engages after checks pass; approve the captured sha.
        let shas = wait_for_gate_events(&root, "gated", 1).await;
        assert_eq!(shas[0], head, "gate_sha must be the phase cwd's git HEAD");
        record_verdict(&root, "gated/a", &shas[0], VerdictDecision::Approved, None);

        let (state, _notifier, launcher) = tokio::time::timeout(GATE_TEST_DEADLINE, handle)
            .await
            .expect("gate test exceeded 30s")
            .unwrap()
            .unwrap();
        assert_eq!(state.plan_status, PlanStatus::Completed);
        assert_eq!(state.phases[0].status, PhaseStatus::Passed);
        assert_eq!(state.phases[1].status, PhaseStatus::Passed);
        assert_eq!(
            state.phases[0].verdict_decision.as_deref(),
            Some("approved")
        );
        assert_eq!(launcher.call_count("a"), 1);
        assert_eq!(launcher.call_count("b"), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn gate_reject_redispatches_same_session_then_regates() {
        let root = fresh_root("redispatch");
        init_git_repo(&root);
        let launcher = MockLauncher::new();
        let plan = parse_plan(GATED_YAML).unwrap();
        let state = PlanState::from_plan(&plan, "test.yaml");
        let handle = spawn_runner(GATED_YAML, root.clone(), launcher, state);

        // First gate: reject with a comment.
        let shas = wait_for_gate_events(&root, "gated", 1).await;
        record_verdict(
            &root,
            "gated/a",
            &shas[0],
            VerdictDecision::Rejected,
            Some("fix the flaky test"),
        );

        // Redispatch re-enters the gate with a fresh gate_sha; approve it.
        let shas = wait_for_gate_events(&root, "gated", 2).await;
        record_verdict(
            &root,
            "gated/a",
            &shas[1],
            VerdictDecision::Approved,
            Some("looks good now"),
        );

        let (state, _notifier, launcher) = tokio::time::timeout(GATE_TEST_DEADLINE, handle)
            .await
            .expect("gate test exceeded 30s")
            .unwrap()
            .unwrap();
        assert_eq!(state.plan_status, PlanStatus::Completed);
        assert_eq!(state.phases[0].status, PhaseStatus::Passed);
        let calls = launcher.calls_for("a");
        assert_eq!(calls.len(), 2, "exactly ONE redispatch turn");
        assert_eq!(
            calls[0].session_id, calls[1].session_id,
            "redispatch must reuse the SAME session id"
        );
        assert!(
            calls[1].prompt.contains("fix the flaky test"),
            "rejection comment becomes the prompt"
        );
        assert!(calls[1].prompt.contains("REJECTED"));
        assert_eq!(
            state.phases[0].attempts, 1,
            "redispatch must NOT increment attempt"
        );
        assert_eq!(
            state.phases[0].verdict_decision.as_deref(),
            Some("approved")
        );
        // D3: state records the FINAL verdict's metadata — the approval
        // supersedes the earlier rejection.
        assert_eq!(
            state.phases[0].verdict_comment.as_deref(),
            Some("looks good now")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn gate_reject_halt_fails_with_comment_as_error() {
        let root = fresh_root("halt");
        init_git_repo(&root);
        let launcher = MockLauncher::new();
        let yaml = r#"
name: gated
phases:
  - id: a
    prompt: "do it"
    gate: verdict
    on_reject: halt
"#;
        let plan = parse_plan(yaml).unwrap();
        let state = PlanState::from_plan(&plan, "test.yaml");
        let handle = spawn_runner(yaml, root.clone(), launcher, state);

        // D6: the reject must postdate gate_entered_at, so record it after
        // observing the gate_entered event (a pre-recorded verdict is stale
        // and would never satisfy the gate).
        let shas = wait_for_gate_events(&root, "gated", 1).await;
        record_verdict(
            &root,
            "gated/a",
            &shas[0],
            VerdictDecision::Rejected,
            Some("wrong approach"),
        );

        let (state, _notifier, launcher) = tokio::time::timeout(GATE_TEST_DEADLINE, handle)
            .await
            .expect("gate test exceeded 30s")
            .unwrap()
            .unwrap();

        assert_eq!(state.phases[0].status, PhaseStatus::Failed);
        assert_eq!(
            state.phases[0].error.as_ref().unwrap().message,
            "wrong approach"
        );
        assert_eq!(
            state.phases[0].error.as_ref().unwrap().error_type,
            ErrorType::GateRejected
        );
        assert_eq!(state.plan_status, PlanStatus::Blocked);
        assert_eq!(launcher.call_count("a"), 1, "no redispatch turn on halt");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn gate_reject_respects_max_attempts_bound() {
        let root = fresh_root("bound");
        init_git_repo(&root);
        let launcher = MockLauncher::new();
        let yaml = r#"
name: gated
max_attempts: 1
phases:
  - id: a
    prompt: "do it"
    gate: verdict
"#;
        let plan = parse_plan(yaml).unwrap();
        let state = PlanState::from_plan(&plan, "test.yaml");
        let handle = spawn_runner(yaml, root.clone(), launcher, state);

        // D6: record the reject after the gate enters so it is fresh.
        // on_reject defaults to redispatch, but max_attempts: 1 is already
        // exhausted after the gated attempt — reject must halt instead.
        let shas = wait_for_gate_events(&root, "gated", 1).await;
        record_verdict(
            &root,
            "gated/a",
            &shas[0],
            VerdictDecision::Rejected,
            Some("still wrong"),
        );

        let (state, _notifier, launcher) = tokio::time::timeout(GATE_TEST_DEADLINE, handle)
            .await
            .expect("gate test exceeded 30s")
            .unwrap()
            .unwrap();

        assert_eq!(state.phases[0].status, PhaseStatus::Failed);
        assert_eq!(
            state.phases[0].error.as_ref().unwrap().message,
            "still wrong"
        );
        assert_eq!(
            launcher.call_count("a"),
            1,
            "no redispatch beyond max_attempts"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn restart_reenters_gate_wait_without_rerunning() {
        let root = fresh_root("restart");
        let sha = "e".repeat(40);
        // Simulate persisted state from a previous run: phase a is mid-gate.
        let plan = parse_plan(GATED_YAML).unwrap();
        let mut state = PlanState::from_plan(&plan, "test.yaml");
        state.started_at = Some(now_rfc3339());
        transition(
            &mut state,
            "a",
            PhaseStatus::Pending,
            PhaseStatus::Running,
            None,
        )
        .unwrap();
        transition(
            &mut state,
            "a",
            PhaseStatus::Running,
            PhaseStatus::Checking,
            None,
        )
        .unwrap();
        transition(
            &mut state,
            "a",
            PhaseStatus::Checking,
            PhaseStatus::AwaitingVerdict,
            Some(PhaseUpdate {
                attempts: Some(1),
                gate_sha: Some(sha.clone()),
                gate_entered_at: Some(now_rfc3339()),
                ..Default::default()
            }),
        )
        .unwrap();
        crate::state::persist::save_state(&root, &state).unwrap();

        // Approve BEFORE resuming — the resumed wait observes it on its
        // first poll; no gate_entered is re-emitted and no turn re-runs.
        // (gate_entered_at was set above, so this verdict postdates it and
        // satisfies the D6 freshness rule.)
        record_verdict(&root, "gated/a", &sha, VerdictDecision::Approved, None);

        let launcher = MockLauncher::new();
        let (state, _notifier, launcher) = tokio::time::timeout(
            GATE_TEST_DEADLINE,
            spawn_runner(GATED_YAML, root.clone(), launcher, state),
        )
        .await
        .expect("gate test exceeded 30s")
        .unwrap()
        .unwrap();

        assert_eq!(state.plan_status, PlanStatus::Completed);
        assert_eq!(state.phases[0].status, PhaseStatus::Passed);
        assert_eq!(
            launcher.call_count("a"),
            0,
            "restart must NOT re-run the phase agent turn"
        );
        assert_eq!(state.phases[0].attempts, 1);
        assert_eq!(state.phases[1].status, PhaseStatus::Passed);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn gate_timeout_fails_distinctly() {
        let root = fresh_root("timeout");
        init_git_repo(&root);
        // No verdict will ever arrive.
        let launcher = MockLauncher::new();
        let yaml = r#"
name: gated
timeout_sec: 600
phases:
  - id: a
    prompt: "do it"
    gate: verdict
    gate_timeout_sec: 1
"#;
        let plan = parse_plan(yaml).unwrap();
        let state = PlanState::from_plan(&plan, "test.yaml");
        let (state, _notifier, _launcher) = tokio::time::timeout(
            GATE_TEST_DEADLINE,
            spawn_runner(yaml, root.clone(), launcher, state),
        )
        .await
        .expect("gate test exceeded 30s")
        .unwrap()
        .unwrap();

        let phase = &state.phases[0];
        assert_eq!(phase.status, PhaseStatus::Failed);
        let err = phase
            .error
            .as_ref()
            .expect("gate timeout must set an error");
        assert_eq!(err.error_type, ErrorType::Timeout);
        assert!(
            err.message.contains("gate timed out"),
            "distinct gate timeout error, got: {}",
            err.message
        );
        assert_eq!(state.plan_status, PlanStatus::Blocked);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn non_gated_plan_completes_without_gate_events() {
        let root = fresh_root("nongated");
        // No git repo, no ledger — a non-gated plan must not touch any of it.
        let (state, _msgs) = {
            let plan = parse_plan(
                "name: plain\nphases:\n  - id: a\n    prompt: \"x\"\n  - id: b\n    prompt: \"y\"\n    depends_on: [a]\n",
            )
            .unwrap();
            let mut state = PlanState::from_plan(&plan, "test.yaml");
            let engine = CheckEngine::new(root.clone());
            let notifier = CollectNotifier::new();
            let mut budget = BudgetTracker::new(plan.budget_usd);
            run_plan(
                &plan,
                &mut state,
                RunContext {
                    launcher: &MockLauncher::new(),
                    check_engine: &engine,
                    notifier: &notifier,
                    budget: &mut budget,
                    cancel: CancellationToken::new(),
                    cwd: &root,
                    interactive: false,
                    json_events: false,
                    tmux_session: None,
                },
            )
            .await
            .unwrap();
            (state, notifier.messages())
        };
        assert_eq!(state.plan_status, PlanStatus::Completed);
        assert!(state.phases.iter().all(|p| p.status == PhaseStatus::Passed));
        assert!(state.phases.iter().all(|p| p.gate_sha.is_none()));
        // No gate events in the event log.
        let events = read_events(&root, "plain");
        assert!(!events
            .iter()
            .any(|e| e["type"] == "gate_entered" || e["type"] == "verdict_received"));
    }

    #[test]
    fn gate_subject_format() {
        assert_eq!(gate_subject("my-plan", "build"), "my-plan/build");
    }

    #[tokio::test]
    async fn wait_for_verdict_ignores_sha_mismatch_and_times_out() {
        let root = fresh_root("mismatchwait");
        let sha_a = "a".repeat(40);
        let sha_b = "b".repeat(40);
        // A verdict for a DIFFERENT sha is recorded... the wait on sha_b must
        // never satisfy on it and must hit its timeout instead.
        record_verdict(&root, "plan/phase", &sha_a, VerdictDecision::Approved, None);
        let cancel = CancellationToken::new();
        let verdict = wait_for_verdict(
            &root,
            "plan/phase",
            &sha_b,
            Some(1),
            Some(&now_rfc3339()),
            &cancel,
        )
        .await;
        assert!(matches!(verdict, GateVerdict::TimedOut));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn wait_for_verdict_returns_matching_verdict() {
        let root = fresh_root("matchwait");
        let sha = "c".repeat(40);
        // D6 freshness: the gate's entered_at must predate the verdict.
        let entered_at = now_rfc3339();
        record_verdict(
            &root,
            "plan/phase",
            &sha,
            VerdictDecision::Rejected,
            Some("no"),
        );
        let cancel = CancellationToken::new();
        let verdict = wait_for_verdict(
            &root,
            "plan/phase",
            &sha,
            Some(30),
            Some(&entered_at),
            &cancel,
        )
        .await;
        match verdict {
            GateVerdict::Rejected(record) => {
                assert_eq!(record.payload.comment.as_deref(), Some("no"));
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// D6 regression: the exact 176-loop scenario. The redispatch turn
    /// produces no new commit, so the re-entered gate waits on the SAME
    /// `(subject, gate_sha)`; the rejected verdict recorded for the first
    /// gate is now STALE (it predates the second gate_entered_at) and must
    /// not re-satisfy the gate. Assert exactly ONE redispatch, then the
    /// gate times out instead of looping forever.
    #[tokio::test]
    async fn stale_reject_does_not_resatisfy_regated_same_sha() {
        let root = fresh_root("stale");
        init_git_repo(&root); // HEAD never changes → same gate_sha on re-entry
        let launcher = MockLauncher::new();
        let yaml = r#"
name: gated
phases:
  - id: a
    prompt: "do it"
    gate: verdict
    gate_timeout_sec: 5
"#;
        // Generous timeout: the first wait must observe the reject recorded
        // right after the gate_entered event, even under parallel-test load.
        let plan = parse_plan(yaml).unwrap();
        let state = PlanState::from_plan(&plan, "test.yaml");
        let handle = spawn_runner(yaml, root.clone(), launcher, state);

        // First gate: reject → one redispatch turn → re-enter the gate.
        let shas = wait_for_gate_events(&root, "gated", 1).await;
        record_verdict(
            &root,
            "gated/a",
            &shas[0],
            VerdictDecision::Rejected,
            Some("fix it"),
        );
        let shas = wait_for_gate_events(&root, "gated", 2).await;
        assert_eq!(
            shas[0], shas[1],
            "no commit between gates: same gate_sha — the stale-verdict scenario"
        );

        // No fresh verdict ever arrives. Without the freshness bound the
        // runner would re-read the stale rejection and redispatch forever.
        let (state, _notifier, launcher) = tokio::time::timeout(GATE_TEST_DEADLINE, handle)
            .await
            .expect("gate test exceeded 30s")
            .unwrap()
            .unwrap();

        assert_eq!(
            launcher.call_count("a"),
            2,
            "exactly ONE redispatch — the stale rejection must not re-satisfy the re-entered gate"
        );
        assert_eq!(state.phases[0].status, PhaseStatus::Failed);
        let err = state.phases[0]
            .error
            .as_ref()
            .expect("gate timeout must set an error");
        assert_eq!(err.error_type, ErrorType::Timeout);
        assert!(
            err.message.contains("gate timed out"),
            "distinct gate timeout error, got: {}",
            err.message
        );
        assert_eq!(
            state.phases[0].gate_redispatches, 1,
            "the persisted redispatch counter counts exactly the one cycle"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// D6: exhausting the persisted redispatch bound fails the phase with a
    /// distinct error naming the bound (like on_reject: halt).
    #[tokio::test]
    async fn gate_reject_exhausts_redispatch_bound_with_distinct_error() {
        let root = fresh_root("rdbound");
        init_git_repo(&root);
        let launcher = MockLauncher::new();
        let yaml = r#"
name: gated
max_attempts: 99
phases:
  - id: a
    prompt: "do it"
    gate: verdict
"#;
        // No gate_timeout_sec: the wait polls until each verdict lands
        // (record_verdict retries through SQLite busy windows under parallel
        // load); the outer 30s deadline catches a genuine hang.
        let plan = parse_plan(yaml).unwrap();
        let state = PlanState::from_plan(&plan, "test.yaml");
        let handle = spawn_runner(yaml, root.clone(), launcher, state);

        // Reject every gate entry. Each redispatch re-enters the gate with
        // the same sha; each fresh reject costs one more redispatch cycle.
        // Rejections are recorded as the gates appear, by a helper task.
        let rejecter_root = root.clone();
        let rejecter = tokio::spawn(async move {
            let root = rejecter_root;
            for i in 0..=3 {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
                loop {
                    let mut shas = Vec::new();
                    let path = root.join(".edda/conductor/gated/events.jsonl");
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        for line in content.lines().filter(|l| !l.trim().is_empty()) {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                                if v["type"] == "gate_entered" {
                                    shas.push(v["gate_sha"].as_str().unwrap().to_string());
                                }
                            }
                        }
                    }
                    if shas.len() > i {
                        record_verdict(
                            &root,
                            "gated/a",
                            &shas[i],
                            VerdictDecision::Rejected,
                            Some("no good"),
                        );
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "rejecter: gate {i} never appeared"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        });

        // FOUR gate cycles, each paced by the runner's 2s ledger poll —
        // needs a wider outer bound than single-gate tests, especially
        // under full-suite parallel load.
        let (state, _notifier, launcher) =
            tokio::time::timeout(std::time::Duration::from_secs(60), async {
                let res = handle.await.unwrap().unwrap();
                rejecter.abort();
                res
            })
            .await
            .expect("gate test exceeded 60s");

        // 1 gated attempt + MAX_GATE_REDISPATCHES redispatch turns, no more.
        assert_eq!(
            launcher.call_count("a"),
            1 + MAX_GATE_REDISPATCHES,
            "redispatch bound must cap agent turns"
        );
        assert_eq!(state.phases[0].status, PhaseStatus::Failed);
        let err = state.phases[0]
            .error
            .as_ref()
            .expect("bound must set an error");
        assert_eq!(err.error_type, ErrorType::GateRejected);
        assert!(
            err.message.contains("redispatch bound exhausted"),
            "distinct error naming the bound, got: {}",
            err.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
