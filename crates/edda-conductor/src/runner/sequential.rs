use crate::agent::budget::BudgetTracker;
use crate::agent::launcher::{phase_session_id_attempt, AgentLauncher, PhaseResult};
use crate::check::engine::{CheckEngine, CheckRunResult};
use crate::plan::schema::{CheckSpec, OnFail, Plan};
use crate::plan::topo::topo_sort;
use crate::runner::edda;
use crate::runner::event_log::{self, Event, EventLogger};
use crate::runner::notify::Notifier;
use crate::state::derive::{
    detect_stale_phases, find_next_phase, is_plan_blocked, is_plan_complete, update_plan_status,
};
use crate::state::machine::{
    transition, CheckResult, CheckStatus, ErrorInfo, ErrorType, PhaseStatus, PhaseUpdate,
    PlanState, PlanStatus,
};
use crate::state::persist::save_state;
use anyhow::Result;
use std::path::Path;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

/// Run a plan sequentially. The main conductor loop.
#[allow(clippy::too_many_arguments)]
pub async fn run_plan(
    plan: &Plan,
    state: &mut PlanState,
    launcher: &dyn AgentLauncher,
    check_engine: &CheckEngine,
    notifier: &dyn Notifier,
    budget: &mut BudgetTracker,
    cancel: CancellationToken,
    cwd: &Path,
    interactive: bool,
    json_events: bool,
) -> Result<()> {
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

        // 2. Find next runnable phase
        let Some(phase_id) = find_next_phase(plan, state, &order) else {
            break; // all done or no runnable phase
        };
        let phase = plan.phases.iter().find(|p| p.id == phase_id).unwrap();
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
        let phase_start = Instant::now();
        event_log.record(Event::PhaseStart {
            phase_id: phase_id.clone(),
            attempt,
        });
        event_log::write_runner_status(cwd, state, Some(&phase_id));

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

        // 5. Process result
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
                    &phase_id,
                    PhaseStatus::Running,
                    PhaseStatus::Checking,
                    None,
                )?;
                save_state(cwd, state)?;

                // Run checks
                let check_result = check_engine
                    .run_all(
                        &phase.check,
                        state.get_phase(&phase_id)?.started_at.as_deref(),
                    )
                    .await;

                if check_result.all_passed {
                    transition(
                        state,
                        &phase_id,
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

                    // Record to edda ledger
                    edda::record_phase_done(cwd, &phase_id, result_text.as_deref(), cost_usd);
                    event_log.record(Event::PhasePassed {
                        phase_id: phase_id.clone(),
                        attempt,
                        duration_ms: elapsed_ms,
                        cost_usd,
                    });
                } else {
                    transition(
                        state,
                        &phase_id,
                        PhaseStatus::Checking,
                        PhaseStatus::Failed,
                        Some(PhaseUpdate {
                            checks: Some(check_result.results.clone()),
                            error: check_result.error.clone(),
                            ..Default::default()
                        }),
                    )?;
                    let elapsed_ms = phase_start.elapsed().as_millis() as u64;
                    let err_msg = check_result
                        .error
                        .as_ref()
                        .map(|e| e.message.as_str())
                        .unwrap_or("check failed");
                    println!(
                        "  ✗ Phase \"{phase_id}\" failed ({}): {err_msg}",
                        format_elapsed(phase_start.elapsed()),
                    );
                    edda::record_phase_failed(cwd, &phase_id, err_msg);
                    event_log.record(Event::PhaseFailed {
                        phase_id: phase_id.clone(),
                        attempt,
                        duration_ms: elapsed_ms,
                        error: err_msg.to_string(),
                    });
                    handle_on_fail(
                        plan,
                        phase,
                        state,
                        &phase_id,
                        &check_result,
                        notifier,
                        &mut event_log,
                    )
                    .await;
                }
            }
            PhaseResult::Timeout => {
                transition(
                    state,
                    &phase_id,
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
                edda::record_phase_failed(cwd, &phase_id, "timed out");
                event_log.record(Event::PhaseFailed {
                    phase_id: phase_id.clone(),
                    attempt,
                    duration_ms: elapsed_ms,
                    error: "timed out".into(),
                });
            }
            PhaseResult::AgentCrash { error } => {
                transition(
                    state,
                    &phase_id,
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
                edda::record_phase_failed(cwd, &phase_id, &error);
                event_log.record(Event::PhaseFailed {
                    phase_id: phase_id.clone(),
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
                    &phase_id,
                    &empty_result,
                    notifier,
                    &mut event_log,
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
                    &phase_id,
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
                    phase_id: phase_id.clone(),
                    attempt,
                    duration_ms: elapsed_ms,
                    error: msg,
                });
            }
        }

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
            let max = phase.max_attempts.unwrap_or(plan.max_attempts);
            let (attempts, should_retry) = {
                let ps = state.get_phase_mut(phase_id).unwrap();
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
            let ps = state.get_phase_mut(phase_id).unwrap();
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
            launcher,
            &engine,
            &notifier,
            &mut budget,
            cancel,
            dir.path(),
            false, // non-interactive in tests
            false, // no json events in tests
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
            &launcher,
            &engine,
            &notifier,
            &mut budget,
            cancel,
            dir.path(),
            false,
            false,
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
            launcher,
            &engine,
            &notifier,
            &mut budget,
            cancel,
            dir.path(),
            false,
            false,
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
}
