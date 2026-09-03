use crate::agent::budget::BudgetTracker;
use crate::agent::launcher::PhaseResult;
use crate::check::engine::{CheckEngine, CheckRunResult};
use crate::plan::schema::{OnFail, Phase, Plan};
use crate::runner::edda;
use crate::runner::event_log::{Event, EventLogger};
use crate::runner::gate::{capture_git_head, gate_subject, persist_gate_output};
use crate::runner::heartbeat::LaneHeartbeat;
use crate::runner::notify::Notifier;
use crate::runner::sequential::{format_elapsed, now_rfc3339};
use crate::state::machine::{
    transition, CheckResult, CheckStatus, ErrorInfo, ErrorType, PhaseStatus, PhaseUpdate,
    PlanState, PlanStatus,
};
use crate::state::persist::save_state_reconciled;
use crate::tmux::TmuxSession;
use anyhow::Result;
use edda_notify::NotifyEvent;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Shared tail of a failed check run: transition Checking → Failed, print,
/// record to edda + event log, then apply the phase's on_fail policy.
#[allow(clippy::too_many_arguments)]
pub(super) async fn fail_checking_phase(
    plan: &Plan,
    phase: &Phase,
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
#[allow(clippy::too_many_lines)] // 395 lines — moved verbatim from sequential.rs (GH-776)
pub(super) async fn process_phase_result(
    plan: &Plan,
    phase: &Phase,
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
pub(super) fn phase_env_retries(state: &PlanState, phase_id: &str) -> u32 {
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
pub(super) fn phase_terminal_event(
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
pub(super) fn final_output_line(result_text: Option<&str>) -> Option<String> {
    result_text?
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// Apply the phase's on_fail policy after a terminal failure. `cwd` carries
/// the workspace root so the abort policy can write the structured plan
/// abort event to the ledger (GH-584 round-2 P1-1).
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)] // 173 lines at #779; split tracked in #776
pub(super) async fn handle_on_fail(
    plan: &Plan,
    phase: &Phase,
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

pub(super) fn format_check_failures(results: &[CheckResult]) -> String {
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
