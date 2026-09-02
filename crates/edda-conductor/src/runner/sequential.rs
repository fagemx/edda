use crate::agent::budget::BudgetTracker;
use crate::agent::launcher::{phase_session_id_attempt, AgentLauncher, PhaseResult};
use crate::check::engine::{CheckEngine, CheckRunResult};
use crate::plan::schema::{CheckSpec, OnFail, OnGateTimeout, OnReject, Plan};
use crate::plan::topo::topo_sort;
use crate::runner::edda;
use crate::runner::event_log::{self, Event, EventLogger};
use crate::runner::heartbeat::{
    lane_heartbeat_interval_secs, run_phase_with_heartbeat, LaneHeartbeat,
};
use crate::runner::notify::Notifier;
use crate::state::brief::write_brief;
use crate::state::derive::{
    detect_stale_phases, find_next_phase, is_plan_blocked, is_plan_complete, update_plan_status,
};
use crate::state::machine::{
    transition, CheckResult, CheckStatus, ErrorInfo, ErrorType, PhaseStatus, PhaseUpdate,
    PlanState, PlanStatus,
};
use crate::state::persist::save_state_reconciled;
use crate::tmux::TmuxSession;
use anyhow::Context;
use anyhow::Result;
use edda_core::VerdictPayload;
use edda_ledger::VerdictRecord;
use edda_notify::NotifyEvent;
use std::path::{Path, PathBuf};
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
    // GH-564 P1-2: each Running/Checking → Stale transition on resume is a
    // terminal transition — notify so the controller reacts instead of
    // polling stdout tails.
    // GH-564 Round-2 P1-2 (exactly-once): persist the transitions BEFORE
    // notifying. The stale mutation otherwise lives only in memory until
    // some later save — a real resume (plan already started) that hits the
    // non-interactive blocked branch never saves, so the next run would
    // re-detect the same orphan transition and send a duplicate Stale
    // notification. Persisting first makes each transition's notification
    // exactly-once across repeated `edda conduct run` invocations.
    let stale_transitions = detect_stale_phases(state, plan);
    if !stale_transitions.is_empty() {
        save_state_reconciled(cwd, state)?;
        event_log::write_runner_status(cwd, state, None);
        write_brief(cwd, state, None);
    }
    for (phase_id, attempts) in stale_transitions {
        notifier
            .notify_phase_terminal(phase_terminal_event(
                &plan.name, &phase_id, "Stale", attempts, None,
            ))
            .await;
    }

    // Record plan start
    if state.started_at.is_none() {
        state.started_at = Some(now_rfc3339());
        state.plan_status = PlanStatus::Running;
        save_state_reconciled(cwd, state)?;
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
            let failed = state.phases.iter().find(|p| {
                p.status == PhaseStatus::Failed
                    || p.status == PhaseStatus::Stale
                    // GH-552: an unwaived gate timeout blocks like a failure.
                    || (p.status == PhaseStatus::GateTimedOut && p.skip_reason.is_none())
            });
            let failed_id = failed.map(|f| f.id.clone()).unwrap_or_default();
            let failed_status = failed.map(|f| f.status).unwrap_or(PhaseStatus::Failed);

            if interactive {
                match prompt_blocked_action(&failed_id, failed_status) {
                    BlockedAction::Retry => {
                        let current = state
                            .get_phase(&failed_id)
                            .map(|p| p.status)
                            .unwrap_or(PhaseStatus::Failed);
                        let _ = transition(state, &failed_id, current, PhaseStatus::Pending, None);
                        state.plan_status = PlanStatus::Running;
                        save_state_reconciled(cwd, state)?;
                        println!("  ↻ Retrying \"{failed_id}\"");
                        continue;
                    }
                    BlockedAction::Skip => {
                        let ps = state.get_phase_mut(&failed_id)?;
                        ps.status = PhaseStatus::Skipped;
                        ps.skip_reason = Some("manually skipped (interactive)".into());
                        state.plan_status = PlanStatus::Running;
                        save_state_reconciled(cwd, state)?;
                        event_log.record(Event::PhaseSkipped {
                            phase_id: failed_id.clone(),
                            reason: "manually skipped (interactive)".into(),
                        });
                        let attempt_now =
                            state.get_phase(&failed_id).map(|p| p.attempts).unwrap_or(0);
                        notifier
                            .notify_phase_terminal(phase_terminal_event(
                                &plan.name,
                                &failed_id,
                                "Skipped",
                                attempt_now,
                                None,
                            ))
                            .await;
                        println!("  ⊘ Skipped \"{failed_id}\"");
                        continue;
                    }
                    BlockedAction::Waive => {
                        // GH-552: the phase ran, its checks passed, and its
                        // gate timed out — record a waiver on the honest
                        // GateTimedOut status, never a false Skipped.
                        let (gate_sha, entered_at) = {
                            let ps = state.get_phase(&failed_id)?;
                            (
                                ps.gate_sha.clone().unwrap_or_default(),
                                ps.gate_entered_at.clone(),
                            )
                        };
                        let waited = entered_at
                            .and_then(|t| {
                                time::OffsetDateTime::parse(
                                    &t,
                                    &time::format_description::well_known::Rfc3339,
                                )
                                .ok()
                            })
                            .map(|t| {
                                std::time::Duration::from_secs(
                                    (time::OffsetDateTime::now_utc() - t).whole_seconds().max(0)
                                        as u64,
                                )
                            })
                            .map(format_elapsed)
                            .unwrap_or_else(|| "unknown".into());
                        let reason = format!(
                            "gate waived after timeout: work completed and checks passed (commit {gate_sha}); waited {waited}"
                        );
                        let ps = state.get_phase_mut(&failed_id)?;
                        ps.skip_reason = Some(reason.clone());
                        state.plan_status = PlanStatus::Running;
                        save_state_reconciled(cwd, state)?;
                        event_log.record(Event::GateWaived {
                            phase_id: failed_id.clone(),
                            reason: reason.clone(),
                            auto: false,
                        });
                        println!(
                            "  ⧗ Waived gate on \"{failed_id}\" (status kept as GateTimedOut)"
                        );
                        continue;
                    }
                    BlockedAction::Abort => {
                        state.plan_status = PlanStatus::Aborted;
                        state.aborted_at = Some(now_rfc3339());
                        save_state_reconciled(cwd, state)?;
                        // GH-584 round-2 P1-1: the plan abort reaches the
                        // workspace ledger as a structured conductor_plan
                        // event, not only the plan-local event log.
                        let phases_passed = state
                            .phases
                            .iter()
                            .filter(|p| p.status == PhaseStatus::Passed)
                            .count();
                        let phases_pending = state
                            .phases
                            .iter()
                            .filter(|p| p.status == PhaseStatus::Pending)
                            .count();
                        event_log.record(Event::PlanAborted {
                            phases_passed,
                            phases_pending,
                        });
                        edda::record_plan_aborted(cwd, &plan.name, phases_passed, phases_pending);
                        let attempt_now =
                            state.get_phase(&failed_id).map(|p| p.attempts).unwrap_or(0);
                        notifier
                            .notify_phase_terminal(phase_terminal_event(
                                &plan.name,
                                &failed_id,
                                "Aborted",
                                attempt_now,
                                None,
                            ))
                            .await;
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
                (sha, at)
            };
            let phase_num = order.iter().position(|id| id == &gated_id).unwrap_or(0) + 1;

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
            println!(
                "  reject:   edda verdict reject {subject} --sha {gate_sha} --comment \"<why>\""
            );
            event_log::write_runner_status(cwd, state, Some(&gated_id));
            write_brief(cwd, state, None);

            // The lane stays alive while gated — keep its heartbeat honest
            // with an "awaiting_verdict" stage (GH-566).
            let lane_hb = {
                let ps = state.get_phase(&gated_id)?;
                LaneHeartbeat {
                    cwd: cwd.to_path_buf(),
                    session_id: phase_session_id_attempt(&plan.name, &gated_id, ps.attempts)
                        .to_string(),
                    plan: plan.name.clone(),
                    phase: gated_id.clone(),
                    attempt: ps.attempts,
                }
            };

            match wait_for_verdict(
                cwd,
                &subject,
                &gate_sha,
                phase.gate_timeout_sec,
                Some(&entered_at),
                &cancel,
                Some(&lane_hb),
                notifier,
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
                    let approved_ps = state.get_phase(&gated_id)?;
                    // GH-564 P1-3: the approved phase's agent final output
                    // (last line = PR URL by convention) was parked at gate
                    // entry and survives restarts — restore it, never drop
                    // it to null. GH-564 Round-2 P1: consume the sidecar —
                    // its lifecycle ends with this verdict.
                    let final_output = load_gate_output(cwd, &plan.name, &gated_id);
                    clear_gate_output(cwd, &plan.name, &gated_id);
                    // GH-584 round-3: gate approval is a phase terminal
                    // state like any other — write the structured
                    // `conductor_phase` event with the plan id and the
                    // measured cost parked on the phase at gate entry,
                    // exactly as the non-gate pass path does.
                    edda::record_phase_done_with_plan(
                        cwd,
                        Some(&plan.name),
                        &gated_id,
                        final_output.as_deref(),
                        approved_ps.cost_usd,
                    );
                    notifier
                        .notify_phase_terminal(phase_terminal_event(
                            &plan.name,
                            &gated_id,
                            "Passed",
                            approved_ps.attempts,
                            final_output.as_deref(),
                        ))
                        .await;
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
                        let gate_cost = state.get_phase(&gated_id).ok().and_then(|p| p.cost_usd);
                        edda::record_phase_failed_with_plan(
                            cwd,
                            Some(&plan.name),
                            &gated_id,
                            gate_cost,
                            &message,
                        );
                        let gate_ps = state.get_phase(&gated_id)?;
                        event_log.record(Event::PhaseFailed {
                            phase_id: gated_id.clone(),
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
                        let final_output = load_gate_output(cwd, &plan.name, &gated_id);
                        clear_gate_output(cwd, &plan.name, &gated_id);
                        notifier
                            .notify_phase_terminal(phase_terminal_event(
                                &plan.name,
                                &gated_id,
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
                        save_state_reconciled(cwd, state)?;
                        println!(
                            "  ↻ Redispatching one more turn in the same session ({session_id})"
                        );
                        let lane_hb = LaneHeartbeat {
                            cwd: cwd.to_path_buf(),
                            session_id: session_id.clone(),
                            plan: plan.name.clone(),
                            phase: gated_id.clone(),
                            attempt: attempts,
                        };
                        let result = run_phase_with_heartbeat(
                            launcher,
                            phase,
                            &prompt,
                            &plan_context,
                            &phase_cwd,
                            &cancel,
                            &lane_hb,
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
                            &cancel,
                            Some(&lane_hb),
                        )
                        .await?;
                    }
                }
                GateVerdict::TimedOut => {
                    // D3: NOT silent, NOT auto-approve. GH-552: also not a
                    // phase failure — the work completed and its checks
                    // passed, so the honest terminal state is GateTimedOut
                    // with the real elapsed gate time, and the plan's
                    // on_gate_timeout policy decides what happens next.
                    let elapsed_ms = time::OffsetDateTime::parse(
                        &entered_at,
                        &time::format_description::well_known::Rfc3339,
                    )
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
                        &gated_id,
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
                        let _ = tmux.update_phase_status(&gated_id, "GateTimedOut");
                    }
                    let gate_cost = state.get_phase(&gated_id).ok().and_then(|p| p.cost_usd);
                    edda::record_phase_failed_with_plan(
                        cwd,
                        Some(&plan.name),
                        &gated_id,
                        gate_cost,
                        &msg,
                    );
                    event_log.record(Event::GateTimedOut {
                        phase_id: gated_id.clone(),
                        gate_sha: gate_sha.clone(),
                        elapsed_ms,
                    });
                    // GH-564 P1-3: same parked output as the approved branch.
                    // Consume the sidecar with the verdict (GH-564 Round-2 P1).
                    let final_output = load_gate_output(cwd, &plan.name, &gated_id);
                    clear_gate_output(cwd, &plan.name, &gated_id);
                    let gate_ps = state.get_phase(&gated_id)?;
                    notifier
                        .notify_phase_terminal(phase_terminal_event(
                            &plan.name,
                            &gated_id,
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
                        let ps = state.get_phase_mut(&gated_id)?;
                        ps.skip_reason = Some(reason.clone());
                        event_log.record(Event::GateWaived {
                            phase_id: gated_id.clone(),
                            reason: reason.clone(),
                            auto: true,
                        });
                        println!(
                            "  ⧗ Gate auto-waived for \"{gated_id}\" (on_gate_timeout: skip) — plan proceeds"
                        );
                    }
                }
                GateVerdict::LedgerUnreadable(err) => {
                    // GH-541: the gate could not read the ledger persistently
                    // (not lock contention). Failing with the diagnostic is
                    // the only honest outcome — the printed `edda verdict`
                    // command writes to a ledger this gate cannot read, so
                    // the wait was unrescuable from the start.
                    let msg = format!(
                        "gate aborted: ledger unreadable for \"{subject}\" (sha {gate_sha}): {err}"
                    );
                    transition(
                        state,
                        &gated_id,
                        PhaseStatus::AwaitingVerdict,
                        PhaseStatus::Failed,
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
                    println!("  ⚠ Phase \"{gated_id}\" {msg}");
                    if let Some(tmux) = tmux_session {
                        let _ = tmux.update_phase_status(&gated_id, "Failed");
                    }
                    let gate_cost = state.get_phase(&gated_id).ok().and_then(|p| p.cost_usd);
                    edda::record_phase_failed_with_plan(
                        cwd,
                        Some(&plan.name),
                        &gated_id,
                        gate_cost,
                        &msg,
                    );
                    let gate_ps = state.get_phase(&gated_id)?;
                    event_log.record(Event::PhaseFailed {
                        phase_id: gated_id.clone(),
                        attempt: gate_ps.attempts,
                        duration_ms: 0,
                        error: msg,
                        error_type: Some(ErrorType::LedgerUnreadable.tag().to_string()),
                        env_retries: gate_ps.env_retries,
                        attempt_charged: true,
                    });
                    let final_output = load_gate_output(cwd, &plan.name, &gated_id);
                    clear_gate_output(cwd, &plan.name, &gated_id);
                    notifier
                        .notify_phase_terminal(phase_terminal_event(
                            &plan.name,
                            &gated_id,
                            "Failed",
                            gate_ps.attempts,
                            final_output.as_deref(),
                        ))
                        .await;
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
        // GH-584 round-2 P1-3: a fresh attempt starts unmeasured — the
        // previous attempt's cost belongs to its own (already written)
        // terminal event, not to this one.
        phase_state.cost_usd = None;

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
        save_state_reconciled(cwd, state)?;

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
        write_phase_claim(cwd, &session_id, &phase_id, &phase.owns);

        // GH-566/GH-569: the runner refreshes the lane heartbeat during the
        // agent turn so any backend (no Claude hooks required) is visible to
        // `edda peers` while it works. One write site serves conduct + dispatch.
        let lane_hb = LaneHeartbeat {
            cwd: cwd.to_path_buf(),
            session_id: session_id.clone(),
            plan: plan.name.clone(),
            phase: phase_id.clone(),
            attempt,
        };
        let result = run_phase_with_heartbeat(
            launcher,
            phase,
            &prompt,
            &plan_context,
            &phase_cwd,
            &cancel,
            &lane_hb,
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
            &cancel,
            Some(&lane_hb),
        )
        .await?;

        save_state_reconciled(cwd, state)?;
    }

    // Plan completion check
    update_plan_status(state);
    if is_plan_complete(state) {
        state.plan_status = PlanStatus::Completed;
        state.completed_at = Some(now_rfc3339());
        save_state_reconciled(cwd, state)?;
        let passed = state
            .phases
            .iter()
            .filter(|p| p.status == PhaseStatus::Passed)
            .count();
        println!("\n✓ Plan \"{}\" completed ({passed} passed)", plan.name);
        event_log.record(Event::PlanCompleted {
            phases_passed: passed,
            // GH-533: None (null in JSONL) until some phase measured a cost.
            total_cost_usd: state.cost_measured.then_some(state.total_cost_usd),
        });
        // GH-584 round-2 P1-1: the plan terminal state reaches the workspace
        // ledger with the honest total (null = unmeasured, #533) — not only
        // the plan-local event log.
        edda::record_plan_completed(
            cwd,
            &plan.name,
            state.cost_measured.then_some(state.total_cost_usd),
        );
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
const GATE_MAX_PERSISTENT_READ_ERRORS: u32 = 15;

/// GH-541: tracks persistent (non-lock) ledger read failures while a gate
/// waits. Busy/lock contention is transient and never counts. A healthy
/// poll resets the budget. Reports the error on the first failure and then
/// on a decaying interval; returns `LedgerUnreadable` when the budget
/// expires.
struct ReadErrorTracker {
    consecutive: u32,
    report_backoff_secs: u64,
    next_report: Option<Instant>,
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
    fn observe(&mut self, err: &anyhow::Error) -> Option<GateVerdict> {
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
    fn reset(&mut self) {
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
async fn wait_for_verdict(
    cwd: &Path,
    subject: &str,
    gate_sha: &str,
    timeout_sec: Option<u64>,
    entered_at: Option<&str>,
    cancel: &CancellationToken,
    heartbeat: Option<&LaneHeartbeat>,
    notifier: &dyn Notifier,
) -> GateVerdict {
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
    // the wall-clock `deadline` above drives the actual timeout check.
    let deadline_t = timeout_sec.map(|t| tokio::time::Instant::now() + Duration::from_secs(t));

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
            Ok(ledger) => match ledger.latest_verdict_fresh(subject, gate_sha, entered_at) {
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
                .notify(&format!(
                    "Still waiting for verdict on \"{subject}\" (sha {gate_sha}) — {wait_label}"
                ))
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
    final_output: Option<&str>,
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
            error: error_info.clone(),
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
    // GH-584 round-2 P1-2/P1-3: the failure event carries the plan id and
    // the phase's measured cost — checks failing after a measured agent
    // turn must not rewrite that cost as unmeasured null.
    let measured = state.get_phase(phase_id).ok().and_then(|p| p.cost_usd);
    edda::record_phase_failed_with_plan(
        cwd,
        Some(plan.name.as_str()),
        phase_id,
        measured,
        &err_msg,
    );
    let ps = state.get_phase(phase_id)?;
    let (error_type, attempt_charged) = match &error_info {
        Some(e) => (
            Some(e.error_type.tag().to_string()),
            e.error_type != ErrorType::Environmental,
        ),
        None => (None, true),
    };
    event_log.record(Event::PhaseFailed {
        phase_id: phase_id.to_string(),
        attempt: ps.attempts,
        duration_ms: elapsed.as_millis() as u64,
        error: err_msg,
        error_type,
        env_retries: ps.env_retries,
        attempt_charged,
    });
    notifier
        .notify_phase_terminal(phase_terminal_event(
            plan.name.as_str(),
            phase_id,
            "Failed",
            ps.attempts,
            final_output,
        ))
        .await;
    handle_on_fail(
        plan,
        phase,
        state,
        phase_id,
        cwd,
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
    cancel: &CancellationToken,
    lane_hb: Option<&LaneHeartbeat>,
) -> Result<()> {
    match result {
        PhaseResult::AgentDone {
            cost_usd,
            result_text,
        } => {
            if let Some(cost) = cost_usd {
                budget.record(cost);
                state.record_cost(cost);
                // GH-584 round-2 P1-3: park the measured cost on the phase
                // itself so a later failure in this attempt (failed checks,
                // gate rejection/timeout) can still write it. Redispatch
                // turns accumulate; a fresh attempt resets it below.
                if let Ok(ps) = state.get_phase_mut(phase_id) {
                    ps.cost_usd = Some(ps.cost_usd.unwrap_or(0.0) + cost);
                }
            }

            // running → checking
            transition(
                state,
                phase_id,
                PhaseStatus::Running,
                PhaseStatus::Checking,
                None,
            )?;
            save_state_reconciled(cwd, state)?;

            // GH-566: the lane heartbeat must cover the whole phase
            // lifetime — keep it beating (stage "checking") while the
            // checks run, not just during the agent turn.
            let checking_writer = lane_hb.map(|hb| hb.spawn("checking", cancel.child_token()));
            // Run checks
            let check_result = check_engine
                .run_all(
                    &phase.check,
                    state.get_phase(phase_id)?.started_at.as_deref(),
                )
                .await;
            if let Some(writer) = checking_writer {
                writer.abort();
            }

            if check_result.all_passed {
                if phase.gate.is_some() {
                    // D3: capture gate_sha = current git HEAD of the phase
                    // cwd, persist AWAITING_VERDICT, emit GateEntered + a
                    // notifier message, then wait (the loop's gate-wait branch).
                    match capture_git_head(phase_cwd) {
                        Ok(gate_sha) => {
                            // GH-564 P1-3 / Round-2 P1: park the agent's final
                            // output BEFORE the phase enters AWAITING_VERDICT.
                            // Every gate entry atomically rewrites the sidecar
                            // — the last non-empty agent line when there is
                            // one, an empty file when there is none — so it
                            // always represents THIS entry's output and a
                            // previous cycle's value can never be read back.
                            // The error is NOT swallowed: a failed write would
                            // leave the previous cycle's file in place, which
                            // the verdict site could read back as this
                            // entry's output — fail the entry instead.
                            if let Err(e) = persist_gate_output(
                                cwd,
                                &plan.name,
                                phase_id,
                                final_output_line(result_text.as_deref()).as_deref(),
                            ) {
                                let msg = format!("failed to persist gate final output: {e}");
                                fail_checking_phase(
                                    plan,
                                    phase,
                                    state,
                                    phase_id,
                                    cwd,
                                    &check_result,
                                    Some(&msg),
                                    phase_start.elapsed(),
                                    final_output_line(result_text.as_deref()).as_deref(),
                                    notifier,
                                    event_log,
                                    tmux_session,
                                )
                                .await?;
                                return Ok(());
                            }
                            let subject = gate_subject(&plan.name, phase_id);
                            transition(
                                state,
                                phase_id,
                                PhaseStatus::Checking,
                                PhaseStatus::AwaitingVerdict,
                                Some(PhaseUpdate {
                                    checks: Some(check_result.results),
                                    gate_sha: Some(gate_sha.clone()),
                                    // D6 LOAD-BEARING: `gate_entered_at` is
                                    // REWRITTEN on every gate entry, including
                                    // redispatch re-entry. Verdict freshness
                                    // compares the verdict timestamp against
                                    // this bound, so re-entry stales every
                                    // earlier verdict for the same
                                    // (subject, gate_sha) — including the
                                    // rejection that triggered this
                                    // redispatch — and that is what kills the
                                    // re-approval loop. Hoisting, caching, or
                                    // reusing the original bound here silently
                                    // reintroduces the 176-cycle D6 loop
                                    // while the happy path still passes.
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
                                final_output_line(result_text.as_deref()).as_deref(),
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

                    // Record to edda ledger — with the plan id (GH-584
                    // round-2 P1-2): the structured payload must attribute
                    // the cost to its plan on the production path.
                    edda::record_phase_done_with_plan(
                        cwd,
                        Some(&plan.name),
                        phase_id,
                        result_text.as_deref(),
                        cost_usd,
                    );
                    event_log.record(Event::PhasePassed {
                        phase_id: phase_id.to_string(),
                        attempt,
                        duration_ms: elapsed_ms,
                        cost_usd,
                    });
                    notifier
                        .notify_phase_terminal(phase_terminal_event(
                            &plan.name,
                            phase_id,
                            "Passed",
                            attempt,
                            final_output_line(result_text.as_deref()).as_deref(),
                        ))
                        .await;
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
                    final_output_line(result_text.as_deref()).as_deref(),
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
            let measured = state.get_phase(phase_id).ok().and_then(|p| p.cost_usd);
            edda::record_phase_failed_with_plan(
                cwd,
                Some(&plan.name),
                phase_id,
                measured,
                "timed out",
            );
            event_log.record(Event::PhaseFailed {
                phase_id: phase_id.to_string(),
                attempt,
                duration_ms: elapsed_ms,
                error: "timed out".into(),
                error_type: Some(ErrorType::Timeout.tag().to_string()),
                env_retries: phase_env_retries(state, phase_id),
                attempt_charged: true,
            });
            notifier
                .notify_phase_terminal(phase_terminal_event(
                    &plan.name, phase_id, "Stale", attempt, None,
                ))
                .await;
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
            let measured = state.get_phase(phase_id).ok().and_then(|p| p.cost_usd);
            edda::record_phase_failed_with_plan(cwd, Some(&plan.name), phase_id, measured, &error);
            event_log.record(Event::PhaseFailed {
                phase_id: phase_id.to_string(),
                attempt,
                duration_ms: elapsed_ms,
                error: error.clone(),
                error_type: Some(ErrorType::AgentCrash.tag().to_string()),
                env_retries: phase_env_retries(state, phase_id),
                attempt_charged: true,
            });
            notifier
                .notify_phase_terminal(phase_terminal_event(
                    &plan.name, phase_id, "Failed", attempt, None,
                ))
                .await;
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
                cwd,
                &empty_result,
                notifier,
                event_log,
            )
            .await;
        }
        PhaseResult::MaxTurns { cost_usd } | PhaseResult::BudgetExceeded { cost_usd } => {
            if let Some(cost) = cost_usd {
                budget.record(cost);
                state.record_cost(cost);
                if let Ok(ps) = state.get_phase_mut(phase_id) {
                    ps.cost_usd = Some(ps.cost_usd.unwrap_or(0.0) + cost);
                }
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
                error: msg.clone(),
                error_type: Some(ErrorType::BudgetExceeded.tag().to_string()),
                env_retries: phase_env_retries(state, phase_id),
                attempt_charged: true,
            });
            // GH-584 round-2 P1-3: MaxTurns / BudgetExceeded ARE phase
            // terminal states — the workspace ledger must get the failure
            // event, carrying the measured cost when the backend reported one.
            let measured = state.get_phase(phase_id).ok().and_then(|p| p.cost_usd);
            edda::record_phase_failed_with_plan(cwd, Some(&plan.name), phase_id, measured, &msg);
            notifier
                .notify_phase_terminal(phase_terminal_event(
                    &plan.name, phase_id, "Failed", attempt, None,
                ))
                .await;
        }
    }
    Ok(())
}

/// Environmental retries already charged to the phase's free-retry counter
/// (GH-540 review round 1) — recorded on `phase_failed` so event consumers
/// can reconstruct the retry accounting.
fn phase_env_retries(state: &PlanState, phase_id: &str) -> u32 {
    state
        .get_phase(phase_id)
        .map(|p| p.env_retries)
        .unwrap_or(0)
}

/// GH-564: build the phase terminal-state notification payload. `state` is
/// the terminal status name ("Passed" | "Failed" | "Stale" | "Skipped" |
/// "Aborted"); "Aborted" is plan-level and names the phase that forced the
/// abort. `final_output` carries the agent's last output line when the
/// transition site has one (by convention it contains the PR URL).
fn phase_terminal_event(
    plan_name: &str,
    phase_id: &str,
    state: &str,
    attempt: u32,
    final_output: Option<&str>,
) -> NotifyEvent {
    NotifyEvent::PhaseTerminal {
        plan: plan_name.to_string(),
        phase: phase_id.to_string(),
        state: state.to_string(),
        attempt,
        final_output: final_output.map(str::to_string),
    }
}

/// Last non-empty line of the agent's result text (GH-564: by convention it
/// contains the PR URL).
fn final_output_line(result_text: Option<&str>) -> Option<String> {
    result_text?
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// GH-564 P1-3: where a gated phase's agent final output is parked while the
/// plan waits for the external verdict. `{cwd}/.edda/conductor/{plan}/{phase}.gate_output`.
/// Survives a conductor restart, so the verdict site can restore the output
/// into the terminal notification instead of dropping it to `null`.
fn gate_output_path(cwd: &Path, plan_name: &str, phase_id: &str) -> PathBuf {
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
fn persist_gate_output(
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
fn clear_gate_output(cwd: &Path, plan_name: &str, phase_id: &str) {
    let _ = std::fs::remove_file(gate_output_path(cwd, plan_name, phase_id));
}

/// GH-564 P1-3: read back the parked final output at the gate verdict site.
fn load_gate_output(cwd: &Path, plan_name: &str, phase_id: &str) -> Option<String> {
    let output = std::fs::read_to_string(gate_output_path(cwd, plan_name, phase_id)).ok()?;
    let trimmed = output.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Apply the phase's on_fail policy after a terminal failure. `cwd` carries
/// the workspace root so the abort policy can write the structured plan
/// abort event to the ledger (GH-584 round-2 P1-1).
#[allow(clippy::too_many_arguments)]
async fn handle_on_fail(
    plan: &Plan,
    phase: &crate::plan::schema::Phase,
    state: &mut PlanState,
    phase_id: &str,
    cwd: &Path,
    check_result: &CheckRunResult,
    notifier: &dyn Notifier,
    event_log: &mut EventLogger,
) {
    /// GH-540: bound on environmental retries per phase run. Without it a
    /// persistently broken environment (e.g. antivirus holding every newly
    /// linked .exe) would retry forever; two in a row failing a phase that
    /// never had a product problem is the exact harm the issue describes.
    const MAX_ENV_RETRIES: u32 = 2;

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
            let is_environmental = check_result
                .error
                .as_ref()
                .is_some_and(|e| e.error_type == ErrorType::Environmental);
            let max = phase.max_attempts.unwrap_or(plan.max_attempts);
            let (product_attempts, env_retries, should_retry) = {
                let ps = state
                    .get_phase_mut(phase_id)
                    .expect("phase must exist in state");
                // GH-540: an environmental build failure (LNK1104) is not the
                // agent's work — its attempt is not charged to the ladder.
                // `attempts` counts every dispatch (attempt numbers key the
                // session id and must stay unique), so the product count is
                // `attempts - env_retries`. Review round 1: EVERY
                // environmental occurrence charges env_retries — including
                // the one that exhausts MAX_ENV_RETRIES — otherwise the
                // cap-ending fault leaves a phantom product attempt and the
                // first genuine failure after a manual `conduct retry` is
                // denied auto-retry. Once the counter passes MAX_ENV_RETRIES
                // the environment is treated as persistently broken: halt and
                // report instead of looping forever.
                let product = ps.attempts.saturating_sub(ps.env_retries);
                if is_environmental {
                    ps.env_retries += 1;
                    (product, ps.env_retries, ps.env_retries <= MAX_ENV_RETRIES)
                } else if product < max {
                    let error_context = format_check_failures(&check_result.results);
                    ps.retry_context = Some(error_context);
                    (product, ps.env_retries, true)
                } else {
                    (product, ps.env_retries, false)
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
                if is_environmental {
                    println!(
                        "  ↻ Environmental build failure — retrying without \
                         charging the attempt ladder ({env_retries}/{MAX_ENV_RETRIES})"
                    );
                } else {
                    println!("  ↻ Auto-retrying ({product_attempts}/{max})");
                }
            } else if is_environmental {
                // cap exhausted (should_retry is only false for environmental
                // when MAX_ENV_RETRIES is spent; the counter reads MAX+1
                // because the cap-ending occurrence is itself charged)
                notifier
                    .notify(&format!(
                        "Phase \"{phase_id}\" failed on repeated environmental build \
                         failures ({env_retries} occurrences, capped at {MAX_ENV_RETRIES} retries) — the \
                         machine layer, not the agent's work, is at fault. Clear the fault, \
                         then retry."
                    ))
                    .await;
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
            let attempt_now = state.get_phase(phase_id).map(|p| p.attempts).unwrap_or(0);
            notifier
                .notify_phase_terminal(phase_terminal_event(
                    &plan.name,
                    phase_id,
                    "Skipped",
                    attempt_now,
                    None,
                ))
                .await;
            println!("  → Auto-skipped (on_fail: skip)");
        }
        OnFail::Abort => {
            state.plan_status = PlanStatus::Aborted;
            state.aborted_at = Some(now_rfc3339());
            // GH-584 round-2 P1-1: the plan abort reaches the workspace
            // ledger as a structured conductor_plan event, not only the
            // plan-local event log.
            let phases_passed = state
                .phases
                .iter()
                .filter(|p| p.status == PhaseStatus::Passed)
                .count();
            let phases_pending = state
                .phases
                .iter()
                .filter(|p| p.status == PhaseStatus::Pending)
                .count();
            event_log.record(Event::PlanAborted {
                phases_passed,
                phases_pending,
            });
            edda::record_plan_aborted(cwd, &plan.name, phases_passed, phases_pending);
            let attempt_now = state.get_phase(phase_id).map(|p| p.attempts).unwrap_or(0);
            notifier
                .notify_phase_terminal(phase_terminal_event(
                    &plan.name,
                    phase_id,
                    "Aborted",
                    attempt_now,
                    None,
                ))
                .await;
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
            PhaseStatus::GateTimedOut => "⧗",
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
    /// GH-552: move the plan past a timed-out gate WITHOUT recording the
    /// phase as Skipped — the phase keeps its honest `GateTimedOut` status
    /// with a waiver reason.
    Waive,
    Abort,
    Quit,
}

fn prompt_blocked_action(phase_id: &str, status: PhaseStatus) -> BlockedAction {
    use std::io::{BufRead, Write};
    if status == PhaseStatus::GateTimedOut {
        // GH-552: the work completed and checks passed — "skip" would lie.
        // Offer waive instead, and record it as such.
        println!("\n  Phase \"{phase_id}\" gate timed out (work completed, checks passed).\n");
        println!(
            "  [R] Retry (re-run the phase)   [W] Waive the gate (proceed)   [A] Abort   [Q] Quit (resume later)"
        );
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
                "w" | "waive" => return BlockedAction::Waive,
                "a" | "abort" => return BlockedAction::Abort,
                "q" | "quit" => return BlockedAction::Quit,
                _ => println!("  Invalid choice. Enter R, W, A, or Q."),
            }
        }
    }
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

/// GH-551: the deadline line for the AWAITING_VERDICT surface. A bounded
/// gate states its deadline; an unbounded one says so explicitly, so an
/// operator who set 7200s hours earlier can tell the clock is draining.
fn format_gate_deadline(timeout_sec: Option<u64>, entered_at: &str) -> String {
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
/// `owns` carries the phase's declared write-surface path globs (GH-561); an
/// empty slice produces the same event as before the field existed.
fn write_phase_claim(cwd: &Path, session_id: &str, phase_id: &str, owns: &[String]) {
    let project_id = edda_store::project_id(cwd);
    let state_dir = edda_store::project_dir(&project_id).join("state");
    let coord_path = state_dir.join("coordination.jsonl");
    let event = serde_json::json!({
        "ts": now_rfc3339(),
        "session_id": session_id,
        "event_type": "claim",
        "payload": { "label": phase_id, "paths": owns }
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
pub(crate) mod tests {
    use super::*;
    use crate::agent::launcher::{MockLauncher, PhaseResult};
    use crate::plan::parser::parse_plan;
    use crate::runner::notify::CollectNotifier;

    async fn run_test_plan_notifier(
        yaml: &str,
        launcher: &dyn AgentLauncher,
    ) -> (PlanState, CollectNotifier) {
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

        (state, notifier)
    }

    async fn run_test_plan(yaml: &str, launcher: &dyn AgentLauncher) -> (PlanState, Vec<String>) {
        let (state, notifier) = run_test_plan_notifier(yaml, launcher).await;
        (state, notifier.messages())
    }

    /// GH-564: flatten the observed phase terminal events for assertions.
    fn terminal_view(
        events: &[edda_notify::NotifyEvent],
    ) -> Vec<(String, String, String, u32, Option<String>)> {
        events
            .iter()
            .map(|e| match e {
                edda_notify::NotifyEvent::PhaseTerminal {
                    plan,
                    phase,
                    state,
                    attempt,
                    final_output,
                } => (
                    plan.clone(),
                    phase.clone(),
                    state.clone(),
                    *attempt,
                    final_output.clone(),
                ),
                other => panic!(
                    "unexpected non-terminal NotifyEvent: {}",
                    other.event_name()
                ),
            })
            .collect()
    }

    // ── GH-564: exactly one notification per phase terminal transition ──

    #[tokio::test]
    async fn terminal_notify_passed_exactly_one() {
        let yaml = r#"
name: notifytest
phases:
  - id: a
    prompt: "do it"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![PhaseResult::AgentDone {
                cost_usd: None,
                result_text: Some("worked hard\nPR: https://github.com/x/y/pull/9".into()),
            }],
        );
        let (state, notifier) = run_test_plan_notifier(yaml, &launcher).await;

        assert_eq!(state.phases[0].status, PhaseStatus::Passed);
        let tv = terminal_view(&notifier.terminal_events());
        assert_eq!(tv.len(), 1, "exactly one terminal notification: {tv:?}");
        assert_eq!(tv[0].0, "notifytest");
        assert_eq!(tv[0].1, "a");
        assert_eq!(tv[0].2, "Passed");
        assert_eq!(tv[0].3, 1);
        assert_eq!(
            tv[0].4.as_deref(),
            Some("PR: https://github.com/x/y/pull/9")
        );
    }

    #[tokio::test]
    async fn terminal_notify_one_per_transition_crash_retry_pass() {
        let yaml = r#"
name: notifytest
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
                    result_text: Some("done".into()),
                },
            ],
        );
        let (state, notifier) = run_test_plan_notifier(yaml, &launcher).await;

        assert_eq!(state.phases[0].status, PhaseStatus::Passed);
        assert_eq!(state.phases[0].attempts, 2);
        let tv = terminal_view(&notifier.terminal_events());
        assert_eq!(tv.len(), 2, "one per terminal transition: {tv:?}");
        assert_eq!(
            (tv[0].1.as_str(), tv[0].2.as_str(), tv[0].3),
            ("a", "Failed", 1)
        );
        assert_eq!(tv[0].4, None);
        assert_eq!(
            (tv[1].1.as_str(), tv[1].2.as_str(), tv[1].3),
            ("a", "Passed", 2)
        );
    }

    // ── GH-584 round 2: workspace-ledger write → read proof ─────────
    // P1-4: the structured payloads must have a real consumer; the reader
    // here is the production `edda_ledger` query path, not the producer's
    // own in-memory state. P1-1/P1-2/P1-3 are the wiring these tests pin.

    /// Run a plan against a PRE-INITIALIZED workspace ledger so the runner's
    /// library writes have a real workspace to land in, and hand the dir back
    /// for read-back assertions. (`ensure_init` early-returns: `.edda` exists.)
    async fn run_plan_in_ledger(
        yaml: &str,
        launcher: &dyn AgentLauncher,
    ) -> (tempfile::TempDir, PlanState, CollectNotifier) {
        let plan = parse_plan(yaml).unwrap();
        let dir = tempfile::tempdir().unwrap();
        edda_ledger::Ledger::ensure_initialized(dir.path()).expect("init workspace ledger");
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
        (dir, state, notifier)
    }

    /// Read the workspace ledger back through the real `edda_ledger` query
    /// path, returning the note events that carry a structured `key` payload.
    fn read_structured_notes(root: &Path, key: &str) -> Vec<edda_core::Event> {
        edda_ledger::Ledger::open(root)
            .expect("open workspace ledger")
            .iter_events_by_type("note")
            .expect("query note events")
            .into_iter()
            .filter(|e| e.payload.get(key).is_some())
            .collect()
    }

    /// P1-2 + P1-4 (Some case): a passing phase on the production path must
    /// carry `plan_id` — two plans with a "build" phase must be attributable
    /// — and its measured cost must read back as a number.
    #[tokio::test]
    async fn e2e_phase_passed_reaches_the_workspace_ledger_with_plan_id_and_cost() {
        let yaml = r#"
name: ledgerplan
phases:
  - id: build
    prompt: "do it"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results(
            "build",
            vec![PhaseResult::AgentDone {
                cost_usd: Some(0.42),
                result_text: Some("compiled cleanly".into()),
            }],
        );
        let (dir, _state, _notifier) = run_plan_in_ledger(yaml, &launcher).await;

        let events = read_structured_notes(dir.path(), "conductor_phase");
        assert_eq!(
            events.len(),
            1,
            "exactly one structured phase event, got: {:?}",
            events.iter().map(|e| &e.payload).collect::<Vec<_>>()
        );
        let payload = &events[0].payload["conductor_phase"];
        assert_eq!(
            payload["plan_id"], "ledgerplan",
            "plan_id must ride the production call path, not stay null"
        );
        assert_eq!(payload["phase_id"], "build");
        assert_eq!(payload["status"], "passed");
        assert_eq!(payload["cost_usd"], serde_json::json!(0.42));
    }

    /// P1-4 (None case): an unmeasured phase still reaches the ledger, with
    /// `cost_usd` reading back as JSON null (#533: null ≠ 0.0 sentinel).
    #[tokio::test]
    async fn e2e_unmeasured_phase_cost_reads_back_null_from_the_ledger() {
        let yaml = r#"
name: ledgerplan
phases:
  - id: probe
    prompt: "do it"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results(
            "probe",
            vec![PhaseResult::AgentDone {
                cost_usd: None,
                result_text: Some("done".into()),
            }],
        );
        let (dir, _state, _notifier) = run_plan_in_ledger(yaml, &launcher).await;

        let events = read_structured_notes(dir.path(), "conductor_phase");
        assert_eq!(events.len(), 1);
        let payload = &events[0].payload["conductor_phase"];
        assert_eq!(payload["plan_id"], "ledgerplan");
        assert!(
            payload["cost_usd"].is_null(),
            "unmeasured must read back null, got: {}",
            payload["cost_usd"]
        );
    }

    /// P1-1: plan completion must reach the workspace ledger with the honest
    /// measured total, not only the plan-local event log.
    #[tokio::test]
    async fn e2e_plan_completed_writes_conductor_plan_with_measured_total() {
        let yaml = r#"
name: ledgerplan
phases:
  - id: a
    prompt: "do it"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![PhaseResult::AgentDone {
                cost_usd: Some(0.42),
                result_text: None,
            }],
        );
        let (dir, state, _notifier) = run_plan_in_ledger(yaml, &launcher).await;
        assert_eq!(state.plan_status, PlanStatus::Completed);

        let events = read_structured_notes(dir.path(), "conductor_plan");
        assert_eq!(events.len(), 1);
        let payload = &events[0].payload["conductor_plan"];
        assert_eq!(payload["plan_id"], "ledgerplan");
        assert_eq!(payload["status"], "completed");
        assert_eq!(
            payload["total_cost_usd"],
            serde_json::json!(0.42),
            "the ledger must carry the plan's honest measured total"
        );
    }

    /// P1-1: an unmeasured plan completion still reaches the ledger, with
    /// `total_cost_usd` reading back null — never a 0.0 sentinel.
    #[tokio::test]
    async fn e2e_plan_completed_unmeasured_total_reads_back_null() {
        let yaml = r#"
name: ledgerplan
phases:
  - id: a
    prompt: "do it"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![PhaseResult::AgentDone {
                cost_usd: None,
                result_text: None,
            }],
        );
        let (dir, state, _notifier) = run_plan_in_ledger(yaml, &launcher).await;
        assert_eq!(state.plan_status, PlanStatus::Completed);

        let events = read_structured_notes(dir.path(), "conductor_plan");
        assert_eq!(events.len(), 1);
        assert!(events[0].payload["conductor_plan"]["total_cost_usd"].is_null());
    }

    /// P1-1: a plan aborted through the on_fail policy must write the
    /// structured `conductor_plan` abort event to the workspace ledger.
    #[tokio::test]
    async fn e2e_plan_abort_writes_conductor_plan_to_the_workspace_ledger() {
        let yaml = r#"
name: ledgerabort
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
        let (dir, state, _notifier) = run_plan_in_ledger(yaml, &launcher).await;
        assert_eq!(state.plan_status, PlanStatus::Aborted);

        let events = read_structured_notes(dir.path(), "conductor_plan");
        assert_eq!(events.len(), 1);
        let payload = &events[0].payload["conductor_plan"];
        assert_eq!(payload["plan_id"], "ledgerabort");
        assert_eq!(payload["status"], "aborted");
        // The crash-failed phase is neither Passed nor Pending.
        assert_eq!(payload["phases_passed"], 0);
        assert_eq!(payload["phases_pending"], 0);
    }

    /// P1-3: checks failing AFTER a measured agent turn must not rewrite the
    /// phase failure as unmeasured — the ledger failure event carries 0.42.
    #[tokio::test]
    async fn e2e_measured_cost_survives_a_check_failure() {
        let yaml = r#"
name: ledgerfail
on_fail: skip
phases:
  - id: a
    prompt: "make file"
    check:
      - file_exists: "output.txt"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![PhaseResult::AgentDone {
                cost_usd: Some(0.42),
                result_text: Some("made the file".into()),
            }],
        );
        let (dir, state, _notifier) = run_plan_in_ledger(yaml, &launcher).await;
        assert_eq!(state.phases[0].status, PhaseStatus::Skipped);

        let events = read_structured_notes(dir.path(), "conductor_phase");
        assert_eq!(events.len(), 1);
        let payload = &events[0].payload["conductor_phase"];
        assert_eq!(payload["plan_id"], "ledgerfail");
        assert_eq!(
            payload["status"], "failed",
            "the failure event is written at the Checking→Failed transition"
        );
        assert_eq!(
            payload["cost_usd"],
            serde_json::json!(0.42),
            "state recorded 0.42 for this phase; the ledger failure event must too"
        );
    }

    /// P1-3: MaxTurns / BudgetExceeded are phase terminal states — the
    /// workspace ledger must get the phase-failure event, with the cost the
    /// backend reported.
    #[tokio::test]
    async fn e2e_max_turns_failure_writes_a_phase_failure_event_with_cost() {
        let yaml = r#"
name: ledgerturns
phases:
  - id: a
    prompt: "do it"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![PhaseResult::MaxTurns {
                cost_usd: Some(0.7),
            }],
        );
        let (dir, state, _notifier) = run_plan_in_ledger(yaml, &launcher).await;
        assert_eq!(state.phases[0].status, PhaseStatus::Failed);

        let events = read_structured_notes(dir.path(), "conductor_phase");
        assert_eq!(
            events.len(),
            1,
            "MaxTurns is a terminal state: the ledger must not miss the event"
        );
        let payload = &events[0].payload["conductor_phase"];
        assert_eq!(payload["plan_id"], "ledgerturns");
        assert_eq!(payload["status"], "failed");
        assert_eq!(payload["cost_usd"], serde_json::json!(0.7));
    }

    #[tokio::test]
    async fn terminal_notify_stale_on_timeout() {
        let yaml = r#"
name: notifytest
phases:
  - id: a
    prompt: "hang"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results("a", vec![PhaseResult::Timeout]);
        let (state, notifier) = run_test_plan_notifier(yaml, &launcher).await;

        assert_eq!(state.phases[0].status, PhaseStatus::Stale);
        let tv = terminal_view(&notifier.terminal_events());
        assert_eq!(tv.len(), 1, "exactly one terminal notification: {tv:?}");
        assert_eq!(
            (tv[0].1.as_str(), tv[0].2.as_str(), tv[0].3),
            ("a", "Stale", 1)
        );
        assert_eq!(tv[0].4, None);
    }

    #[tokio::test]
    async fn terminal_notify_failed_then_skipped_on_fail_skip() {
        let yaml = r#"
name: notifytest
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
        let (state, notifier) = run_test_plan_notifier(yaml, &launcher).await;

        assert_eq!(state.phases[0].status, PhaseStatus::Skipped);
        assert_eq!(state.phases[1].status, PhaseStatus::Passed);
        let tv = terminal_view(&notifier.terminal_events());
        // Two terminal transitions for "a": the crash failure, then the
        // on_fail re-classification to Skipped — one notification each.
        // Phase "b" contributes its own Passed notification.
        assert_eq!(tv.len(), 3, "one per terminal transition: {tv:?}");
        assert_eq!(tv[0].2, "Failed");
        assert_eq!(tv[1].2, "Skipped");
        assert_eq!(tv[2].2, "Passed");
    }

    #[tokio::test]
    async fn terminal_notify_failed_then_aborted_on_fail_abort() {
        let yaml = r#"
name: notifytest
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
        let (state, notifier) = run_test_plan_notifier(yaml, &launcher).await;

        assert_eq!(state.phases[0].status, PhaseStatus::Failed);
        assert_eq!(state.plan_status, PlanStatus::Aborted);
        let tv = terminal_view(&notifier.terminal_events());
        assert_eq!(tv.len(), 2, "one per terminal transition: {tv:?}");
        assert_eq!((tv[0].2.as_str(), tv[0].3), ("Failed", 1));
        assert_eq!(
            (tv[1].1.as_str(), tv[1].2.as_str(), tv[1].3),
            ("a", "Aborted", 1)
        );
    }

    #[tokio::test]
    async fn terminal_notify_check_failure_carries_final_output() {
        let yaml = r#"
name: notifytest
on_fail: skip
phases:
  - id: a
    prompt: "make file"
    check:
      - file_exists: "output.txt"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![PhaseResult::AgentDone {
                cost_usd: None,
                result_text: Some("built everything\nPR: https://github.com/x/y/pull/7".into()),
            }],
        );
        let (state, notifier) = run_test_plan_notifier(yaml, &launcher).await;

        assert_eq!(state.phases[0].status, PhaseStatus::Skipped);
        let tv = terminal_view(&notifier.terminal_events());
        assert_eq!(
            tv.len(),
            2,
            "check-fail Failed, then on_fail Skipped: {tv:?}"
        );
        assert_eq!((tv[0].2.as_str(), tv[0].3), ("Failed", 1));
        assert_eq!(
            tv[0].4.as_deref(),
            Some("PR: https://github.com/x/y/pull/7"),
            "check-failure notification carries the agent's final output line"
        );
        assert_eq!(tv[1].2, "Skipped");
        assert_eq!(tv[1].4, None);
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

    /// GH-540: an environmental build failure (LNK1104) retries WITHOUT
    /// consuming the agent's attempt ladder, but is bounded — a persistently
    /// broken environment halts after MAX_ENV_RETRIES instead of looping.
    /// Review round 1: extended past the cap — the cap-ending occurrence is
    /// itself charged to env_retries, so after a clear-fault + manual retry
    /// (mirroring `edda conduct retry`: Failed→Pending, counters preserved)
    /// the agent still gets its FULL product ladder.
    #[tokio::test]
    async fn environmental_cap_end_charged_so_manual_retry_keeps_full_ladder() {
        // Run 1: the check always names the linker-fatal signature
        // (environmental on every attempt). Run 2: after the operator writes
        // the fault-cleared marker, every check is a genuine product failure.
        #[cfg(windows)]
        let cmd = "if (Test-Path fault-cleared) { exit 1 } else { [Console]::Error.WriteLine('LINK : fatal error LNK1104: cannot open file ''x.exe''') ; exit 1 }";
        #[cfg(not(windows))]
        let cmd =
            "if [ -f fault-cleared ]; then exit 1; else echo 'LINK : fatal error LNK1104: cannot open file x.exe' 1>&2 ; exit 1; fi";
        let yaml = format!(
            r#"
name: test
max_attempts: 2
phases:
  - id: a
    prompt: "do it"
    check:
      - type: cmd_succeeds
        cmd: "{cmd}"
        timeout_sec: 10
"#
        );
        let plan = parse_plan(&yaml).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut state = PlanState::from_plan(&plan, "test.yaml");
        let engine = CheckEngine::new(dir.path().to_path_buf());
        let notifier = CollectNotifier::new();
        let mut budget = BudgetTracker::new(plan.budget_usd);
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![
                PhaseResult::AgentDone {
                    cost_usd: None,
                    result_text: None,
                },
                PhaseResult::AgentDone {
                    cost_usd: None,
                    result_text: None,
                },
                PhaseResult::AgentDone {
                    cost_usd: None,
                    result_text: None,
                },
                PhaseResult::AgentDone {
                    cost_usd: None,
                    result_text: None,
                },
                PhaseResult::AgentDone {
                    cost_usd: None,
                    result_text: None,
                },
            ],
        );
        // Run 1: environmental on every dispatch — the cap halts the phase.
        run_plan(
            &plan,
            &mut state,
            RunContext {
                launcher: &launcher,
                check_engine: &engine,
                notifier: &notifier,
                budget: &mut budget,
                cancel: CancellationToken::new(),
                cwd: dir.path(),
                interactive: false,
                json_events: false,
                tmux_session: None,
            },
        )
        .await
        .unwrap();

        {
            let phase = &state.phases[0];
            assert_eq!(phase.status, PhaseStatus::Failed);
            // Two free env retries (dispatches 1-2), then the cap halts on
            // the third dispatch. The cap-ending occurrence is itself
            // charged: env_retries reads MAX+1 so product accounting stays
            // exact. Without the charge this persists env_retries == 2 and a
            // phantom product attempt.
            assert_eq!(phase.attempts, 3);
            assert_eq!(
                phase.env_retries, 3,
                "the cap-ending environmental occurrence must be charged"
            );
            assert_eq!(launcher.call_count("a"), 3);
            let err = phase.error.as_ref().expect("env failure must set an error");
            assert_eq!(err.error_type, ErrorType::Environmental);
            assert!(err.retryable);
        }
        assert!(
            notifier
                .messages()
                .iter()
                .any(|m| m.contains("environmental") && m.contains("retry")),
            "halt message must name the environmental fault, got: {:?}",
            notifier.messages()
        );

        // Clear-fault + manual retry, mirroring `edda conduct retry`
        // (cmd_conduct.rs): Failed → Pending, plan unblocked, persisted
        // counters (attempts, env_retries) preserved.
        std::fs::write(dir.path().join("fault-cleared"), b"").unwrap();
        transition(
            &mut state,
            "a",
            PhaseStatus::Failed,
            PhaseStatus::Pending,
            None,
        )
        .unwrap();
        state.plan_status = PlanStatus::Running;

        // Run 2: genuine failures only — the agent must still get its FULL
        // product ladder (2 attempts). Without the cap-ending charge the
        // first genuine failure computes product == max and is denied
        // auto-retry after only one product attempt.
        run_plan(
            &plan,
            &mut state,
            RunContext {
                launcher: &launcher,
                check_engine: &engine,
                notifier: &notifier,
                budget: &mut budget,
                cancel: CancellationToken::new(),
                cwd: dir.path(),
                interactive: false,
                json_events: false,
                tmux_session: None,
            },
        )
        .await
        .unwrap();

        let phase = &state.phases[0];
        assert_eq!(phase.status, PhaseStatus::Failed);
        // Dispatches 4 and 5 are the agent's two genuine product attempts.
        assert_eq!(
            phase.attempts, 5,
            "the full product ladder must survive the environmental cap halt"
        );
        assert_eq!(
            phase.env_retries, 3,
            "genuine failures must never touch env_retries"
        );
        assert_eq!(launcher.call_count("a"), 5);
        assert!(
            notifier
                .messages()
                .iter()
                .any(|m| m.contains("failed after 2 attempts")),
            "the agent still gets its full ladder, got: {:?}",
            notifier.messages()
        );
    }

    /// GH-540: an environmental failure must not consume a product retry —
    /// with max_attempts: 2, one LNK1104 followed by two genuine failures
    /// still gives the agent its full two product attempts (3 dispatches).
    #[tokio::test]
    async fn environmental_failure_not_charged_to_ladder() {
        // First run: environmental LNK1104 (and plants the marker); every
        // later run: a genuine failure with no environmental pattern.
        #[cfg(windows)]
        let cmd = "if (Test-Path env-marker) { exit 1 } else { Write-Output 'LINK : fatal error LNK1104: cannot open file ''x.exe''' ; New-Item -ItemType File env-marker | Out-Null ; exit 1 }";
        #[cfg(not(windows))]
        let cmd = "if [ -f env-marker ]; then exit 1; else echo 'LINK : fatal error LNK1104: cannot open file x.exe' ; touch env-marker ; exit 1; fi";
        let yaml = format!(
            r#"
name: test
max_attempts: 2
phases:
  - id: a
    prompt: "do it"
    check:
      - type: cmd_succeeds
        cmd: "{cmd}"
        timeout_sec: 10
"#
        );
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![
                PhaseResult::AgentDone {
                    cost_usd: None,
                    result_text: None,
                },
                PhaseResult::AgentDone {
                    cost_usd: None,
                    result_text: None,
                },
                PhaseResult::AgentDone {
                    cost_usd: None,
                    result_text: None,
                },
                PhaseResult::AgentDone {
                    cost_usd: None,
                    result_text: None,
                },
            ],
        );
        let (state, msgs) = run_test_plan(&yaml, &launcher).await;

        let phase = &state.phases[0];
        assert_eq!(phase.status, PhaseStatus::Failed);
        // attempt 1: LNK1104 (free) — attempts 2 and 3: the agent's two real
        // product attempts. Without the fix the ladder exhausts at 2.
        assert_eq!(phase.attempts, 3);
        assert_eq!(phase.env_retries, 1);
        assert_eq!(launcher.call_count("a"), 3);
        assert!(
            msgs.iter().any(|m| m.contains("failed after 2 attempts")),
            "the agent still gets its full ladder, got: {msgs:?}"
        );
    }

    /// GH-540 review round 1: phase_failed events must make retry accounting
    /// distinct — a free environmental retry records error_type
    /// "environmental" and attempt_charged=false, while a genuine product
    /// failure records a typed error and attempt_charged=true. env_retries is
    /// the counter value as of the event (the charge for that occurrence is
    /// applied by the on_fail policy right after).
    #[tokio::test]
    async fn phase_failed_events_record_retry_accounting() {
        // First check names the linker-fatal signature (environmental, free);
        // every later check is a genuine product failure.
        #[cfg(windows)]
        let cmd = "if (Test-Path env-marker) { exit 1 } else { Write-Output 'LINK : fatal error LNK1104: cannot open file ''x.exe''' ; New-Item -ItemType File env-marker | Out-Null ; exit 1 }";
        #[cfg(not(windows))]
        let cmd = "if [ -f env-marker ]; then exit 1; else echo 'LINK : fatal error LNK1104: cannot open file x.exe' 1>&2 ; touch env-marker ; exit 1; fi";
        let yaml = format!(
            r#"
name: test
max_attempts: 2
phases:
  - id: a
    prompt: "do it"
    check:
      - type: cmd_succeeds
        cmd: "{cmd}"
        timeout_sec: 10
"#
        );
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![
                PhaseResult::AgentDone {
                    cost_usd: None,
                    result_text: None,
                },
                PhaseResult::AgentDone {
                    cost_usd: None,
                    result_text: None,
                },
                PhaseResult::AgentDone {
                    cost_usd: None,
                    result_text: None,
                },
            ],
        );
        let (_state, dir) = run_test_plan_with_dir(&yaml, &launcher).await;

        let events = read_events(dir.path(), "test");
        let failures: Vec<&serde_json::Value> = events
            .iter()
            .filter(|e| e["type"] == "phase_failed")
            .collect();
        assert_eq!(failures.len(), 3, "events: {events:?}");
        // Dispatch 1: environmental — free retry, untyped error never leaks.
        assert_eq!(failures[0]["error_type"], "environmental");
        assert_eq!(failures[0]["attempt_charged"], false);
        // Counter charged for nothing yet: the additive field is omitted at 0.
        assert_eq!(
            failures[0].get("env_retries").and_then(|v| v.as_u64()),
            None,
            "env_retries must stay absent at zero (old-parser-safe)"
        );
        // Dispatches 2-3: genuine product failures — charged to the ladder.
        assert_eq!(failures[1]["error_type"], "check_failed");
        assert_eq!(failures[1]["attempt_charged"], true);
        assert_eq!(failures[1]["env_retries"], 1);
        assert_eq!(failures[2]["attempt_charged"], true);
        assert_eq!(failures[2]["env_retries"], 1);
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
    async fn plan_completed_usage_free_run_does_not_assert_zero_cost() {
        // GH-533: a codex-style run (no usage data) must not emit the
        // unmeasured sentinel `total_cost_usd: 0.0` in the event stream.
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "usage-free"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![PhaseResult::AgentDone {
                cost_usd: None,
                result_text: None,
            }],
        );
        let (state, dir) = run_test_plan_with_dir(yaml, &launcher).await;

        assert!(!state.cost_measured, "usage-free run must not be measured");
        let events = read_events(dir.path(), "test");
        let completed = events
            .iter()
            .find(|e| e["type"] == "plan_completed")
            .expect("plan_completed event");
        assert_ne!(completed["total_cost_usd"], serde_json::json!(0.0));
        assert!(completed["total_cost_usd"].is_null());
    }

    #[tokio::test]
    async fn plan_completed_measured_cost_round_trips_exactly() {
        // GH-533: a run whose backend reported usage keeps the exact total.
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "measured"
"#;
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![PhaseResult::AgentDone {
                cost_usd: Some(0.42),
                result_text: None,
            }],
        );
        let (state, dir) = run_test_plan_with_dir(yaml, &launcher).await;

        assert!(state.cost_measured);
        assert!((state.total_cost_usd - 0.42).abs() < 1e-9);
        let events = read_events(dir.path(), "test");
        let completed = events
            .iter()
            .find(|e| e["type"] == "plan_completed")
            .expect("plan_completed event");
        assert_eq!(completed["total_cost_usd"], serde_json::json!(0.42));
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
        launcher.set_results(
            "a",
            vec![PhaseResult::AgentDone {
                cost_usd: None,
                result_text: Some(
                    "opened pull request\nPR: https://github.com/x/y/pull/620".into(),
                ),
            }],
        );
        let plan = parse_plan(GATED_YAML).unwrap();
        let state = PlanState::from_plan(&plan, "test.yaml");
        let handle = spawn_runner(GATED_YAML, root.clone(), launcher, state);

        // The gate engages after checks pass; approve the captured sha.
        let shas = wait_for_gate_events(&root, "gated", 1).await;
        assert_eq!(shas[0], head, "gate_sha must be the phase cwd's git HEAD");
        record_verdict(&root, "gated/a", &shas[0], VerdictDecision::Approved, None);

        let (state, notifier, launcher) = tokio::time::timeout(GATE_TEST_DEADLINE, handle)
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
        // GH-564 P1-3: the gate-approved phase's agent final output (its
        // last line, the PR URL by convention) must survive until the
        // verdict site and ride on the Passed terminal notification.
        let tv = terminal_view(&notifier.terminal_events());
        assert_eq!(tv.len(), 2, "one per terminal transition: {tv:?}");
        assert_eq!(
            (tv[0].1.as_str(), tv[0].2.as_str(), tv[0].3),
            ("a", "Passed", 1)
        );
        assert_eq!(
            tv[0].4.as_deref(),
            Some("PR: https://github.com/x/y/pull/620"),
            "gate-approved Passed event must carry the agent's final output"
        );
        assert_eq!(
            (tv[1].1.as_str(), tv[1].2.as_str(), tv[1].3),
            ("b", "Passed", 1)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// GH-541 Round-1 P1-1: an AWAITING_VERDICT phase whose persisted state
    /// is missing `gate_entered_at` must fail closed at resume — no
    /// substitution with `now` (which would admit the stale rejection from
    /// the previous gate entry), and a matching verdict already in the
    /// ledger must NOT satisfy the gate. The run errors with a diagnostic.
    #[tokio::test]
    async fn resume_without_gate_entered_at_fails_closed() {
        let root = fresh_root("missingentered");
        let head = init_git_repo(&root);
        let launcher = MockLauncher::new();
        let plan = parse_plan(GATED_YAML).unwrap();
        let mut state = PlanState::from_plan(&plan, "test.yaml");
        state.phases[0].status = PhaseStatus::AwaitingVerdict;
        state.phases[0].gate_sha = Some(head.clone());
        state.phases[0].gate_entered_at = None; // the corruption under test

        // A matching approval is already in the ledger — it must NOT be
        // admitted without a freshness bound.
        record_verdict(&root, "gated/a", &head, VerdictDecision::Approved, None);

        let handle = spawn_runner(GATED_YAML, root.clone(), launcher, state);
        let result = tokio::time::timeout(GATE_TEST_DEADLINE, handle)
            .await
            .expect("gate test exceeded 30s")
            .unwrap();
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected run_plan to error on the missing gate_entered_at"),
        };
        let msg = format!("{err:#}");
        assert!(
            msg.contains("missing gate_entered_at"),
            "diagnostic must name the missing invariant: {msg}"
        );

        // The stale-verdict approval was not consumed: the phase is still
        // awaiting on disk.
        let persisted = crate::state::persist::load_state(&root, "gated")
            .unwrap()
            .expect("state persists");
        assert_eq!(persisted.phases[0].status, PhaseStatus::AwaitingVerdict);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// GH-564 P1-2: a phase left Running by a previous conductor run, past
    /// its timeout, becomes Stale on resume — and that terminal transition
    /// must reach the controller as a notification.
    #[tokio::test]
    async fn resume_stale_phase_notifies_terminal() {
        let yaml = r#"
name: resumestale
timeout_sec: 1
phases:
  - id: a
    prompt: "do it"
"#;
        let plan = parse_plan(yaml).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut state = PlanState::from_plan(&plan, "test.yaml");
        state.phases[0].status = PhaseStatus::Running;
        state.phases[0].started_at = Some(
            (time::OffsetDateTime::now_utc() - time::Duration::seconds(3600))
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
        );
        let engine = CheckEngine::new(dir.path().to_path_buf());
        let notifier = CollectNotifier::new();
        let mut budget = BudgetTracker::new(plan.budget_usd);
        let cancel = CancellationToken::new();
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

        assert_eq!(state.phases[0].status, PhaseStatus::Stale);
        let tv = terminal_view(&notifier.terminal_events());
        assert!(
            tv.iter().any(|e| e.1 == "a" && e.2 == "Stale"),
            "resume must notify the Running→Stale transition: {tv:?}"
        );
    }

    /// GH-564 Round-2 P1-2 (exactly-once): a REAL persisted resume — the
    /// plan already started, an expired Running phase sits on disk — must
    /// notify Stale exactly once across repeated `edda conduct run`
    /// invocations. The transition must be persisted before the
    /// notification fires, so run 2 (a fresh conductor reloading the
    /// persisted state) finds no orphan Running phase and sends nothing.
    #[tokio::test]
    async fn resume_stale_notifies_exactly_once_across_reruns() {
        let yaml = r#"
name: resumestale
timeout_sec: 1
phases:
  - id: a
    prompt: "do it"
"#;
        let plan = parse_plan(yaml).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let launcher = MockLauncher::new();

        // A conductor died mid-run: the persisted state has an expired
        // Running phase and the plan already started (real resume — NOT a
        // fresh PlanState::from_plan whose started_at is None).
        let mut persisted = PlanState::from_plan(&plan, "test.yaml");
        persisted.started_at = Some(now_rfc3339());
        persisted.plan_status = PlanStatus::Running;
        persisted.phases[0].status = PhaseStatus::Running;
        persisted.phases[0].started_at = Some(
            (time::OffsetDateTime::now_utc() - time::Duration::seconds(3600))
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
        );
        crate::state::persist::save_state(dir.path(), &persisted).unwrap();

        // Run 1: a fresh conductor process loads the persisted state.
        let mut state = crate::state::persist::load_state(dir.path(), &plan.name)
            .unwrap()
            .expect("persisted resume state");
        let notifier1 = CollectNotifier::new();
        let engine = CheckEngine::new(dir.path().to_path_buf());
        let mut budget = BudgetTracker::new(plan.budget_usd);
        run_plan(
            &plan,
            &mut state,
            RunContext {
                launcher: &launcher,
                check_engine: &engine,
                notifier: &notifier1,
                budget: &mut budget,
                cancel: CancellationToken::new(),
                cwd: dir.path(),
                interactive: false,
                json_events: false,
                tmux_session: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(state.phases[0].status, PhaseStatus::Stale);
        let tv1 = terminal_view(&notifier1.terminal_events());
        let stale1 = tv1.iter().filter(|e| e.1 == "a" && e.2 == "Stale").count();
        assert_eq!(
            stale1, 1,
            "run 1 notifies the Stale transition exactly once: {tv1:?}"
        );

        // Run 2: ANOTHER fresh conductor process re-reads the disk state.
        let mut state2 = crate::state::persist::load_state(dir.path(), &plan.name)
            .unwrap()
            .expect("state must survive run 1");
        assert_eq!(
            state2.phases[0].status,
            PhaseStatus::Stale,
            "the Stale transition must be PERSISTED by run 1, not left in memory"
        );
        let notifier2 = CollectNotifier::new();
        let engine2 = CheckEngine::new(dir.path().to_path_buf());
        let mut budget2 = BudgetTracker::new(plan.budget_usd);
        run_plan(
            &plan,
            &mut state2,
            RunContext {
                launcher: &launcher,
                check_engine: &engine2,
                notifier: &notifier2,
                budget: &mut budget2,
                cancel: CancellationToken::new(),
                cwd: dir.path(),
                interactive: false,
                json_events: false,
                tmux_session: None,
            },
        )
        .await
        .unwrap();
        let tv2 = terminal_view(&notifier2.terminal_events());
        assert!(
            tv2.iter().all(|e| !(e.1 == "a" && e.2 == "Stale")),
            "run 2 must NOT re-notify the already-persisted Stale transition: {tv2:?}"
        );
    }

    /// GH-564 Round-2 new P1: after a redispatch cycle, the SECOND gate
    /// entry's output (here: none — the redispatched turn produced no final
    /// text) must replace the first cycle's parked output. Approving the
    /// regate must never deliver the PREVIOUS cycle's PR URL as the final
    /// output.
    #[tokio::test]
    async fn gate_redispatch_none_output_never_reuses_previous_cycles_output() {
        let root = fresh_root("redispatch-none");
        init_git_repo(&root);
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![
                PhaseResult::AgentDone {
                    cost_usd: None,
                    result_text: Some(
                        "opened pull request\nPR: https://github.com/x/y/pull/100".into(),
                    ),
                },
                // The redispatched turn produced NO final text at all.
                PhaseResult::AgentDone {
                    cost_usd: None,
                    result_text: None,
                },
            ],
        );
        let plan = parse_plan(GATED_YAML).unwrap();
        let state = PlanState::from_plan(&plan, "test.yaml");
        let handle = spawn_runner(GATED_YAML, root.clone(), launcher, state);

        // First gate: reject with a comment → redispatch.
        let shas = wait_for_gate_events(&root, "gated", 1).await;
        record_verdict(
            &root,
            "gated/a",
            &shas[0],
            VerdictDecision::Rejected,
            Some("redo it"),
        );

        // Second gate: approve.
        let shas = wait_for_gate_events(&root, "gated", 2).await;
        record_verdict(&root, "gated/a", &shas[1], VerdictDecision::Approved, None);

        let (state, notifier, launcher) = tokio::time::timeout(GATE_TEST_DEADLINE, handle)
            .await
            .expect("gate test exceeded 30s")
            .unwrap()
            .unwrap();
        assert_eq!(state.phases[0].status, PhaseStatus::Passed);
        assert_eq!(launcher.call_count("a"), 2);
        let tv = terminal_view(&notifier.terminal_events());
        let passed = tv
            .iter()
            .find(|e| e.1 == "a" && e.2 == "Passed")
            .expect("approved gate must emit a Passed terminal event");
        assert_eq!(
            passed.4, None,
            "a redispatch turn with no output must NOT inherit the previous cycle's parked PR URL"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// GH-564 Round-2 new P1: when a gate entry CANNOT persist its output
    /// (here the atomic write fails because a directory blocks the sidecar
    /// path), the entry must fail the phase instead of silently waiting on a
    /// verdict whose approve branch would read back the PREVIOUS cycle's
    /// parked output.
    #[tokio::test]
    async fn gate_redispatch_write_failure_fails_instead_of_stale_output() {
        let root = fresh_root("redispatch-writefail");
        init_git_repo(&root);
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![PhaseResult::AgentDone {
                cost_usd: None,
                result_text: Some(
                    "opened pull request\nPR: https://github.com/x/y/pull/100".into(),
                ),
            }],
            // The redispatched turn falls through to the mock default.
        );
        let yaml = r#"
name: gated
phases:
  - id: a
    prompt: "do it"
    gate: verdict
    gate_timeout_sec: 1
    on_fail: abort
"#;
        let plan = parse_plan(yaml).unwrap();
        let state = PlanState::from_plan(&plan, "test.yaml");
        let handle = spawn_runner(yaml, root.clone(), launcher, state);

        // Gate entry 1 parks its output successfully.
        let shas = wait_for_gate_events(&root, "gated", 1).await;
        let sidecar = root
            .join(".edda")
            .join("conductor")
            .join("gated")
            .join("a.gate_output");
        assert_eq!(
            std::fs::read_to_string(&sidecar).unwrap(),
            "PR: https://github.com/x/y/pull/100",
            "entry 1 must have parked its output"
        );
        // Sabotage the NEXT atomic write: a directory where the file must go.
        std::fs::remove_file(&sidecar).unwrap();
        std::fs::create_dir(&sidecar).unwrap();

        // Reject → redispatch → gate entry 2 hits the write failure.
        record_verdict(
            &root,
            "gated/a",
            &shas[0],
            VerdictDecision::Rejected,
            Some("redo it"),
        );

        let (state, _notifier, launcher) = tokio::time::timeout(GATE_TEST_DEADLINE, handle)
            .await
            .expect("gate test exceeded 30s")
            .unwrap()
            .unwrap();
        assert_eq!(
            launcher.call_count("a"),
            2,
            "reject still redispatches one turn"
        );
        assert_eq!(
            state.phases[0].status,
            PhaseStatus::Failed,
            "a gate entry that cannot persist its output must fail, not wait on a verdict against a stale sidecar"
        );
        let err = state.phases[0]
            .error
            .as_ref()
            .expect("Failed phase carries an error");
        assert!(
            err.message.contains("failed to persist gate final output"),
            "the persist failure must surface as the phase error, got: {}",
            err.message
        );
        // The sabotaged entry never reaches AWAITING_VERDICT: still exactly
        // one gate_entered event.
        let events_path = root
            .join(".edda")
            .join("conductor")
            .join("gated")
            .join("events.jsonl");
        let content = std::fs::read_to_string(&events_path).unwrap();
        let gate_enters = content
            .lines()
            .filter(|l| {
                serde_json::from_str::<serde_json::Value>(l)
                    .map(|v| v["type"] == "gate_entered")
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(
            gate_enters, 1,
            "the failed entry must not enter the gate again"
        );
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

        // Redispatch re-enters the gate with the SAME gate_sha, NOT a fresh
        // one: the redispatch turn here produces no new commit, so HEAD (and
        // therefore the sha a verdict must match) is unchanged. The D6
        // freshness rule still blocks the first rejection from re-satisfying
        // the re-entered gate, because re-entry rewrote gate_entered_at and
        // the rejection now predates it. Approving that same sha works
        // because it is recorded after the second gate_entered_at.
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

        let (state, notifier, launcher) = tokio::time::timeout(GATE_TEST_DEADLINE, handle)
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
        // GH-564: the halt failure emits exactly one terminal notification.
        let tv = terminal_view(&notifier.terminal_events());
        assert_eq!(tv.len(), 1, "exactly one terminal notification: {tv:?}");
        assert_eq!(
            (tv[0].1.as_str(), tv[0].2.as_str(), tv[0].3),
            ("a", "Failed", 1)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// P1-2 + P1-3: a gate-reject halt is a phase terminal state reached on
    /// the production path — the ledger failure event must carry the plan id
    /// AND the measured cost of the agent turn that produced the gated work.
    #[tokio::test]
    async fn e2e_gate_reject_halt_writes_plan_id_and_measured_cost() {
        let root = fresh_root("ledgerhalt");
        init_git_repo(&root);
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![PhaseResult::AgentDone {
                cost_usd: Some(0.42),
                result_text: Some("gated work".into()),
            }],
        );
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

        let shas = wait_for_gate_events(&root, "gated", 1).await;
        record_verdict(
            &root,
            "gated/a",
            &shas[0],
            VerdictDecision::Rejected,
            Some("wrong approach"),
        );

        let (_state, _notifier, _launcher) = tokio::time::timeout(GATE_TEST_DEADLINE, handle)
            .await
            .expect("gate test exceeded 30s")
            .unwrap()
            .unwrap();

        let events = read_structured_notes(&root, "conductor_phase");
        assert_eq!(events.len(), 1);
        let payload = &events[0].payload["conductor_phase"];
        assert_eq!(payload["plan_id"], "gated");
        assert_eq!(payload["status"], "failed");
        assert_eq!(
            payload["cost_usd"],
            serde_json::json!(0.42),
            "the measured agent-turn cost must survive the gate failure"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Round-2 review blocking P1: a gate-approved phase is a terminal
    /// state like any other — the workspace ledger must carry exactly one
    /// structured `conductor_phase` event, attributed to the plan, with
    /// status "passed" and the measured agent-turn cost parked on the
    /// phase at gate entry.
    #[tokio::test]
    async fn e2e_gate_approve_writes_plan_id_status_passed_and_measured_cost() {
        let root = fresh_root("ledgerapprove");
        init_git_repo(&root);
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![PhaseResult::AgentDone {
                cost_usd: Some(0.42),
                result_text: Some("gated work".into()),
            }],
        );
        let yaml = r#"
name: gated
phases:
  - id: a
    prompt: "do it"
    gate: verdict
"#;
        let plan = parse_plan(yaml).unwrap();
        let state = PlanState::from_plan(&plan, "test.yaml");
        let handle = spawn_runner(yaml, root.clone(), launcher, state);

        let shas = wait_for_gate_events(&root, "gated", 1).await;
        record_verdict(&root, "gated/a", &shas[0], VerdictDecision::Approved, None);

        let (_state, _notifier, _launcher) = tokio::time::timeout(GATE_TEST_DEADLINE, handle)
            .await
            .expect("gate test exceeded 30s")
            .unwrap()
            .unwrap();

        let events = read_structured_notes(&root, "conductor_phase");
        assert_eq!(
            events.len(),
            1,
            "gate approval must produce exactly one phase terminal event"
        );
        let payload = &events[0].payload["conductor_phase"];
        assert_eq!(payload["plan_id"], "gated");
        assert_eq!(payload["phase_id"], "a");
        assert_eq!(payload["status"], "passed");
        assert_eq!(
            payload["cost_usd"],
            serde_json::json!(0.42),
            "the measured agent-turn cost must survive gate approval"
        );
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
    async fn gate_timeout_records_honest_gate_timed_out_state() {
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
        let (state, notifier, _launcher) = tokio::time::timeout(
            GATE_TEST_DEADLINE,
            spawn_runner(yaml, root.clone(), launcher, state),
        )
        .await
        .expect("gate test exceeded 30s")
        .unwrap()
        .unwrap();

        // GH-552: the work completed and its checks passed — the timeout is
        // NOT a phase failure and NOT a skip. The persisted status and the
        // event log both carry the honest classification.
        let phase = &state.phases[0];
        assert_eq!(phase.status, PhaseStatus::GateTimedOut);
        let err = phase
            .error
            .as_ref()
            .expect("gate timeout must set an error");
        assert_eq!(err.error_type, ErrorType::GateTimeout);
        assert!(
            err.message.contains("gate timed out"),
            "distinct gate timeout error, got: {}",
            err.message
        );
        assert_eq!(state.plan_status, PlanStatus::Blocked);
        // The event log records GateTimedOut with the real elapsed time.
        let events =
            std::fs::read_to_string(root.join(".edda/conductor/gated/events.jsonl")).unwrap();
        assert!(
            events.contains("gate_timed_out"),
            "event log must classify the gate timeout: {events}"
        );
        let gate_event = events
            .lines()
            .find(|l| l.contains("gate_timed_out"))
            .unwrap();
        let elapsed = serde_json::from_str::<serde_json::Value>(gate_event).unwrap()["elapsed_ms"]
            .as_u64()
            .expect("elapsed_ms must be a number");
        assert!(
            elapsed >= 1000,
            "elapsed_ms must reflect the real gate wait, got {elapsed}"
        );
        // GH-564: the gate timeout emits exactly one terminal notification,
        // now carrying the honest status name.
        let tv = terminal_view(&notifier.terminal_events());
        assert_eq!(tv.len(), 1, "exactly one terminal notification: {tv:?}");
        assert_eq!(
            (tv[0].1.as_str(), tv[0].2.as_str(), tv[0].3),
            ("a", "GateTimedOut", 1)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// GH-552: `on_gate_timeout: skip` lets an unattended run declare the
    /// decision in advance — the gate is auto-waived (honest status kept,
    /// waiver reason recorded) and the plan proceeds to dependents.
    #[tokio::test]
    async fn on_gate_timeout_skip_auto_waives_and_plan_proceeds() {
        let root = fresh_root("autoskip");
        init_git_repo(&root);
        let launcher = MockLauncher::new();
        launcher.set_results(
            "b",
            vec![PhaseResult::AgentDone {
                cost_usd: None,
                result_text: Some("done".into()),
            }],
        );
        let yaml = r#"
name: autoskip
timeout_sec: 600
phases:
  - id: a
    prompt: "do it"
    gate: verdict
    gate_timeout_sec: 1
    on_gate_timeout: skip
  - id: b
    prompt: "next"
    depends_on: [a]
"#;
        let plan = parse_plan(yaml).unwrap();
        let state = PlanState::from_plan(&plan, "test.yaml");
        let (state, _notifier, launcher) = tokio::time::timeout(
            GATE_TEST_DEADLINE,
            spawn_runner(yaml, root.clone(), launcher, state),
        )
        .await
        .expect("gate test exceeded 30s")
        .unwrap()
        .unwrap();

        let phase_a = &state.phases[0];
        assert_eq!(phase_a.status, PhaseStatus::GateTimedOut);
        let reason = phase_a
            .skip_reason
            .as_ref()
            .expect("auto-waive must record the waiver reason");
        assert!(reason.contains("auto-waived"), "{reason}");
        assert!(reason.contains("checks passed"), "{reason}");
        // The plan proceeded past the waived gate.
        assert_eq!(state.plan_status, PlanStatus::Completed);
        assert_eq!(state.phases[1].status, PhaseStatus::Passed);
        assert_eq!(launcher.call_count("b"), 1, "dependent phase dispatched");
        // The event log records the automatic waiver.
        let events =
            std::fs::read_to_string(root.join(".edda/conductor/autoskip/events.jsonl")).unwrap();
        assert!(events.contains("gate_waived"), "{events}");
        assert!(
            events.contains("\"auto\":true"),
            "the auto waiver must be marked automatic: {events}"
        );
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
            None,
            &CollectNotifier::new(),
        )
        .await;
        assert!(matches!(verdict, GateVerdict::TimedOut));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// GH-541 §2: a healthy poll resets the persistent-error budget — tested
    /// directly on the tracker: 14 persistent errors, a reset, then more
    /// errors must NOT fail the gate.
    #[test]
    fn read_error_tracker_budget_resets_on_healthy_poll() {
        let mut t = ReadErrorTracker::default();
        let err = anyhow::anyhow!("corrupt database");
        for _ in 0..(GATE_MAX_PERSISTENT_READ_ERRORS - 1) {
            assert!(t.observe(&err).is_none());
        }
        t.reset();
        for _ in 0..(GATE_MAX_PERSISTENT_READ_ERRORS - 1) {
            assert!(t.observe(&err).is_none(), "reset must clear the budget");
        }
    }

    /// GH-541 §2: SQLite busy/lock contention is transient — it never
    /// counts toward the persistent-error budget and never fails the gate.
    #[test]
    fn read_error_tracker_ignores_busy_errors() {
        let busy = anyhow::Error::new(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            Some("database is locked".into()),
        ));
        let mut t = ReadErrorTracker::default();
        for _ in 0..(GATE_MAX_PERSISTENT_READ_ERRORS * 3) {
            assert!(t.observe(&busy).is_none(), "busy errors are transient");
        }
    }

    /// GH-541 §2: persistent errors exhaust the budget exactly at the limit
    /// and the diagnostic names the failure mode and the count.
    #[test]
    fn read_error_tracker_fails_at_budget() {
        let mut t = ReadErrorTracker::default();
        let err = anyhow::anyhow!("disk I/O error");
        for _ in 0..(GATE_MAX_PERSISTENT_READ_ERRORS - 1) {
            assert!(t.observe(&err).is_none());
        }
        match t.observe(&err) {
            Some(GateVerdict::LedgerUnreadable(msg)) => {
                assert!(msg.contains("consecutive failed ledger reads"), "{msg}");
                assert!(msg.contains("disk I/O error"), "{msg}");
            }
            other => panic!("expected LedgerUnreadable, got {other:?}"),
        }
    }

    /// GH-541 §2: a persistently unreadable ledger must fail the gate with
    /// a diagnostic after the error budget — not poll in silence forever.
    /// A bare directory has no `.edda` workspace, so every `Ledger::open`
    /// fails with a persistent (non-lock) error.
    #[tokio::test(start_paused = true)]
    async fn wait_for_verdict_fails_after_persistent_read_errors() {
        let root =
            std::env::temp_dir().join(format!("edda_gate_unreadable_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let cancel = CancellationToken::new();
        let verdict = wait_for_verdict(
            &root,
            "plan/phase",
            &"d".repeat(40),
            None, // no timeout: without the error budget this would hang forever
            Some(&now_rfc3339()),
            &cancel,
            None,
            &CollectNotifier::new(),
        )
        .await;
        match verdict {
            GateVerdict::LedgerUnreadable(msg) => {
                assert!(
                    msg.contains("consecutive failed ledger reads"),
                    "diagnostic must name the failure mode: {msg}"
                );
            }
            other => panic!("expected LedgerUnreadable, got {other:?}"),
        }
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
            None,
            &CollectNotifier::new(),
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

    /// GH-551: the deadline line states the bound or its absence explicitly.
    #[test]
    fn gate_deadline_line_states_bound_or_absence() {
        let entered = "2026-09-03T00:00:00Z";
        let line = format_gate_deadline(Some(7200), entered);
        assert!(line.starts_with("2026-09-03T02:00:00Z"), "{line}");
        assert!(line.contains("gate_timeout_sec 7200"), "{line}");
        let unbounded = format_gate_deadline(None, entered);
        assert!(unbounded.contains("waits until cancelled"), "{unbounded}");
    }

    /// GH-551: a long gate wait emits progress signals naming the remaining
    /// budget, on a decaying schedule (first at 60s, then doubling) — while
    /// the normal short wait (a few 2s polls) stays completely silent.
    /// Driven on paused time: no wall-clock sleeps of gate length.
    #[tokio::test(start_paused = true)]
    async fn gate_wait_emits_progress_signals_with_remaining_budget() {
        let root = fresh_root("progresswait");
        let sha = "9".repeat(40);
        let cancel = CancellationToken::new();
        let notifier = CollectNotifier::new();

        // Approve from another task after 20 minutes of simulated waiting.
        let root_for_task = root.clone();
        let sha_for_task = sha.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(20 * 60)).await;
            record_verdict(
                &root_for_task,
                "plan/phase",
                &sha_for_task,
                VerdictDecision::Approved,
                None,
            );
        });

        let verdict = wait_for_verdict(
            &root,
            "plan/phase",
            &sha,
            Some(3600),
            Some(&now_rfc3339()),
            &cancel,
            None,
            &notifier,
        )
        .await;
        assert!(matches!(verdict, GateVerdict::Approved(_)));

        let msgs = notifier.messages();
        let progress: Vec<&String> = msgs
            .iter()
            .filter(|m| m.contains("Still waiting"))
            .collect();
        assert!(
            !progress.is_empty(),
            "a 20-minute wait must produce progress signals: {msgs:?}"
        );
        // Decaying schedule: signals at 60s, 180s (60+120), 420s (60+120+240),
        // 900s (60+120+240+480) — 4 signals by the 20-minute mark; the 5th
        // would land at 1860s.
        assert_eq!(progress.len(), 4, "decaying schedule signals: {msgs:?}");
        assert!(
            progress[0].contains("remaining"),
            "signal must name the remaining budget: {}",
            progress[0]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// GH-551: the normal short wait (a few 2s polls, well under the first
    /// progress interval) stays completely silent — no per-poll regression.
    #[tokio::test(start_paused = true)]
    async fn gate_wait_short_polls_stay_silent() {
        let root = fresh_root("silentwait");
        let sha = "8".repeat(40);
        let cancel = CancellationToken::new();
        let notifier = CollectNotifier::new();

        // Approve after 5 polls (10s) — well under the 60s first signal.
        let root_for_task = root.clone();
        let sha_for_task = sha.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            record_verdict(
                &root_for_task,
                "plan/phase",
                &sha_for_task,
                VerdictDecision::Approved,
                None,
            );
        });

        let verdict = wait_for_verdict(
            &root,
            "plan/phase",
            &sha,
            Some(3600),
            Some(&now_rfc3339()),
            &cancel,
            None,
            &notifier,
        )
        .await;
        assert!(matches!(verdict, GateVerdict::Approved(_)));
        assert!(
            notifier.messages().is_empty(),
            "short waits must stay silent: {:?}",
            notifier.messages()
        );
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
        assert_eq!(state.phases[0].status, PhaseStatus::GateTimedOut);
        let err = state.phases[0]
            .error
            .as_ref()
            .expect("gate timeout must set an error");
        assert_eq!(err.error_type, ErrorType::GateTimeout);
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
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
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
        // needs a wide outer bound, especially under full-suite parallel
        // load (60s was exceeded once at full load; isolated run is ~19s).
        // A genuine hang still can't stall the test: the runner's own
        // gate deadline resolves the await with an error.
        let (state, _notifier, launcher) =
            tokio::time::timeout(std::time::Duration::from_secs(120), async {
                let res = handle.await.unwrap().unwrap();
                rejecter.abort();
                res
            })
            .await
            .expect("gate test exceeded 120s");

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

    // ── Phase claims carry owned write surfaces (GH-561) ────────────

    /// Serialize tests that redirect `EDDA_STORE_ROOT`.
    pub(crate) static CLAIM_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub(crate) struct ClaimEnvGuard {
        pub(crate) _lock: std::sync::MutexGuard<'static, ()>,
        previous_store_root: Option<std::ffi::OsString>,
        pub(crate) _store_root: tempfile::TempDir,
    }

    impl Drop for ClaimEnvGuard {
        fn drop(&mut self) {
            match self.previous_store_root.take() {
                Some(root) => std::env::set_var("EDDA_STORE_ROOT", root),
                None => std::env::remove_var("EDDA_STORE_ROOT"),
            }
        }
    }

    impl ClaimEnvGuard {
        pub(crate) fn new() -> Self {
            let lock = CLAIM_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let previous_store_root = std::env::var_os("EDDA_STORE_ROOT");
            let store_root = tempfile::tempdir().unwrap();
            std::env::set_var("EDDA_STORE_ROOT", store_root.path());
            Self {
                _lock: lock,
                previous_store_root,
                _store_root: store_root,
            }
        }
    }

    fn coord_lines(cwd: &Path) -> Vec<serde_json::Value> {
        let project_id = edda_store::project_id(cwd);
        let coord_path = edda_store::project_dir(&project_id)
            .join("state")
            .join("coordination.jsonl");
        let content = std::fs::read_to_string(&coord_path).expect("coordination.jsonl exists");
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("each line parses as JSON"))
            .collect()
    }

    pub(crate) fn make_repo(store_root: &Path) -> std::path::PathBuf {
        let cwd = store_root.join("repo");
        std::fs::create_dir_all(&cwd).unwrap();
        // Pre-mark the repo as initialized so `run_plan`'s `ensure_init`
        // early-returns. Without this, the first run_plan-driven test in a
        // process fires edda.rs's store-isolation Once, which re-points
        // EDDA_STORE_ROOT at a leaked throwaway store mid-test — every
        // write (claims, lane heartbeats) then lands in that store while
        // the test polls its own guard's store, and the assertion can never
        // observe them.
        std::fs::create_dir_all(cwd.join(".edda")).unwrap();
        // Production writes claims into an existing project state dir
        // (created by ensure_dirs); mirror that layout here.
        let project_id = edda_store::project_id(&cwd);
        std::fs::create_dir_all(edda_store::project_dir(&project_id).join("state")).unwrap();
        cwd
    }

    #[test]
    fn phase_claim_with_owns_carries_the_paths() {
        let guard = ClaimEnvGuard::new();
        let cwd = make_repo(guard._store_root.path());
        let owns = vec!["crates/edda-conductor/src/agent/*".to_string()];

        write_phase_claim(&cwd, "sess-1", "touch-agent", &owns);

        let events = coord_lines(&cwd);
        assert_eq!(events.len(), 1);
        let payload = &events[0]["payload"];
        assert_eq!(payload["label"], "touch-agent");
        assert_eq!(
            payload["paths"],
            serde_json::json!(["crates/edda-conductor/src/agent/*"])
        );
    }

    #[test]
    fn phase_claim_without_owns_is_byte_identical_to_legacy() {
        let guard = ClaimEnvGuard::new();
        let cwd = make_repo(guard._store_root.path());

        write_phase_claim(&cwd, "sess-2", "no-owns", &[]);

        let events = coord_lines(&cwd);
        assert_eq!(events.len(), 1);
        // Identical to the pre-GH-561 event in every field except ts.
        let legacy = serde_json::json!({
            "ts": events[0]["ts"],
            "session_id": "sess-2",
            "event_type": "claim",
            "payload": { "label": "no-owns", "paths": [] }
        });
        assert_eq!(events[0], legacy);
    }

    // ── Lane heartbeat (GH-566/GH-569) ──────────────────────────────

    /// A launcher that stalls briefly, so the test can observe the lane
    /// heartbeat while the agent turn is genuinely mid-flight.
    struct SlowLauncher;

    #[async_trait::async_trait]
    impl AgentLauncher for SlowLauncher {
        async fn run_phase(
            &self,
            _phase: &crate::plan::schema::Phase,
            _prompt: &str,
            _plan_context: &str,
            _session_id: &str,
            _cwd: &Path,
            _cancel: CancellationToken,
        ) -> Result<PhaseResult> {
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(PhaseResult::AgentDone {
                cost_usd: None,
                result_text: None,
            })
        }
    }

    pub(crate) fn hb_path(cwd: &Path, session_id: &str) -> std::path::PathBuf {
        let project_id = edda_store::project_id(cwd);
        edda_store::project_dir(&project_id)
            .join("state")
            .join(format!("session.{session_id}.json"))
    }

    /// A lane launched with no bridge hooks (`edda dispatch --agent pi`) must
    /// still leave a session heartbeat that `edda peers` can find: the runner
    /// writes it during the agent turn, carrying plan/phase/attempt/stage/pid.
    #[tokio::test]
    async fn phase_heartbeat_written_during_agent_turn() {
        let guard = ClaimEnvGuard::new();
        let cwd = make_repo(guard._store_root.path());

        let yaml = "name: hbplan\nphases:\n  - id: a\n    prompt: \"work\"\n";
        let plan = parse_plan(yaml).unwrap();
        let mut state = PlanState::from_plan(&plan, "test.yaml");
        let engine = CheckEngine::new(cwd.clone());
        let notifier = CollectNotifier::new();
        let mut budget = BudgetTracker::new(plan.budget_usd);
        let cancel = CancellationToken::new();

        let run_cwd = cwd.clone();
        let handle = tokio::spawn(async move {
            run_plan(
                &plan,
                &mut state,
                RunContext {
                    launcher: &SlowLauncher,
                    check_engine: &engine,
                    notifier: &notifier,
                    budget: &mut budget,
                    cancel,
                    cwd: &run_cwd,
                    interactive: false,
                    json_events: false,
                    tmux_session: None,
                },
            )
            .await
            .map(|_| state)
        });

        let session_id = phase_session_id_attempt("hbplan", "a", 1).to_string();
        let path = hb_path(&cwd, &session_id);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if path.exists() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "lane heartbeat never appeared during the agent turn"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let hb: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(hb["session_id"], session_id.as_str());
        assert_eq!(hb["label"], "a", "heartbeat label matches the claim label");
        assert_eq!(hb["plan"], "hbplan");
        assert_eq!(hb["phase"], "a");
        assert_eq!(hb["attempt"], 1);
        assert_eq!(hb["stage"], "running");
        assert!(hb["pid"].as_u64().is_some(), "heartbeat carries the pid");
        assert!(hb["last_heartbeat"].as_str().is_some());

        let (state, _dir) = {
            let state = handle.await.unwrap().unwrap();
            (state, ())
        };
        assert_eq!(state.plan_status, PlanStatus::Completed);
    }

    /// P1-1 regression (review round 1): the lane heartbeat must cover the
    /// whole phase lifetime — checks included — with the stage reflecting
    /// what is actually happening. A fast agent turn followed by a slow
    /// check used to abort the writer while the check still ran, letting the
    /// lane go stale mid-work.
    #[tokio::test]
    async fn heartbeat_keeps_beating_through_the_running_check_stage() {
        let guard = ClaimEnvGuard::new();
        let previous_interval = std::env::var("EDDA_LANE_HEARTBEAT_SECS");
        std::env::set_var("EDDA_LANE_HEARTBEAT_SECS", "1");
        let cwd = make_repo(guard._store_root.path());

        // A check slow enough to span several heartbeat intervals.
        #[cfg(windows)]
        let cmd = "Start-Sleep -Seconds 3";
        #[cfg(not(windows))]
        let cmd = "sleep 3";
        let yaml = format!(
            r#"
name: hbdur
phases:
  - id: a
    prompt: "work"
    check:
      - type: cmd_succeeds
        cmd: "{cmd}"
        timeout_sec: 15
"#
        );
        let plan = parse_plan(&yaml).unwrap();
        let mut state = PlanState::from_plan(&plan, "test.yaml");
        let engine = CheckEngine::new(cwd.clone());
        let notifier = CollectNotifier::new();
        let mut budget = BudgetTracker::new(plan.budget_usd);
        let cancel = CancellationToken::new();

        // Fast agent turn, then the slow check above runs in
        // `process_phase_result`.
        let launcher = MockLauncher::new();
        launcher.set_results(
            "a",
            vec![PhaseResult::AgentDone {
                cost_usd: None,
                result_text: None,
            }],
        );

        let run_cwd = cwd.clone();
        let handle = tokio::spawn(async move {
            run_plan(
                &plan,
                &mut state,
                RunContext {
                    launcher: &launcher,
                    check_engine: &engine,
                    notifier: &notifier,
                    budget: &mut budget,
                    cancel,
                    cwd: &run_cwd,
                    interactive: false,
                    json_events: false,
                    tmux_session: None,
                },
            )
            .await
            .map(|_| state)
        });

        let session_id = phase_session_id_attempt("hbdur", "a", 1).to_string();
        let path = hb_path(&cwd, &session_id);
        let deadline = std::time::Instant::now() + Duration::from_secs(15);

        // The heartbeat must reach the checking stage ...
        let first_checking = loop {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    if v["stage"] == "checking" {
                        break v["last_heartbeat"].clone();
                    }
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "heartbeat never reached the checking stage while checks ran"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        };
        // ... and keep refreshing (1s interval vs 3s check) while the check
        // is still running.
        loop {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    if v["stage"] == "checking" && v["last_heartbeat"] != first_checking {
                        break;
                    }
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "heartbeat stopped refreshing during the running check"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        let state = handle.await.unwrap().unwrap();
        assert_eq!(state.plan_status, PlanStatus::Completed);
        match previous_interval {
            Ok(v) => std::env::set_var("EDDA_LANE_HEARTBEAT_SECS", v),
            Err(_) => std::env::remove_var("EDDA_LANE_HEARTBEAT_SECS"),
        }
    }

    /// The observation plane must never kill the work plane: if the store is
    /// unwritable (here: the project directory is a regular file), the phase
    /// still runs to completion — at most a warning is printed.
    #[tokio::test]
    async fn heartbeat_write_failure_does_not_fail_phase() {
        let guard = ClaimEnvGuard::new();
        let store_root = guard._store_root.path();
        let cwd = make_repo(store_root);

        let project_id = edda_store::project_id(&cwd);
        let project_dir = edda_store::project_dir(&project_id);
        let _ = std::fs::remove_dir_all(&project_dir);
        std::fs::write(&project_dir, b"not a directory").unwrap();

        let yaml = "name: hbplan2\nphases:\n  - id: a\n    prompt: \"work\"\n";
        let launcher = SlowLauncher;
        let (state, _msgs) = {
            let plan = parse_plan(yaml).unwrap();
            let mut state = PlanState::from_plan(&plan, "test.yaml");
            let engine = CheckEngine::new(cwd.clone());
            let notifier = CollectNotifier::new();
            let mut budget = BudgetTracker::new(plan.budget_usd);
            run_plan(
                &plan,
                &mut state,
                RunContext {
                    launcher: &launcher,
                    check_engine: &engine,
                    notifier: &notifier,
                    budget: &mut budget,
                    cancel: CancellationToken::new(),
                    cwd: &cwd,
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
        assert_eq!(state.phases[0].status, PhaseStatus::Passed);
    }

    #[test]
    fn old_claim_event_without_paths_field_still_parses() {
        // Legacy peer-written claims never had a paths field; the consumer
        // reads `payload["paths"]` tolerantly (as_array → default). Prove an
        // old record parses and yields no paths under that exact pattern.
        let old = r#"{"event_type":"claim","payload":{"label":"legacy"},"session_id":"s"}"#;
        let parsed: serde_json::Value = serde_json::from_str(old).unwrap();
        let paths = parsed["payload"]
            .get("paths")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();
        assert!(paths.is_empty());
    }
}
