use crate::plan::schema::Plan;
use crate::state::machine::{ErrorInfo, ErrorType, PhaseStatus, PlanState, PlanStatus};

/// Derive plan-level status from phase states.
pub fn derive_plan_status(phases: &[crate::state::machine::PhaseState]) -> PlanStatus {
    if phases.iter().any(|p| {
        p.status == PhaseStatus::Running
            || p.status == PhaseStatus::Checking
            || p.status == PhaseStatus::AwaitingVerdict
    }) {
        return PlanStatus::Running;
    }
    if phases.iter().any(|p| {
        p.status == PhaseStatus::Failed
                || p.status == PhaseStatus::Stale
                // GH-552: an unwaived gate timeout blocks like a failure —
                // a waived one (skip_reason set) is a resolved dependency.
                || (p.status == PhaseStatus::GateTimedOut && p.skip_reason.is_none())
    }) {
        return PlanStatus::Blocked;
    }
    if phases.iter().all(|p| {
        p.status == PhaseStatus::Passed
            || p.status == PhaseStatus::Skipped
            || (p.status == PhaseStatus::GateTimedOut && p.skip_reason.is_some())
    }) {
        return PlanStatus::Completed;
    }
    PlanStatus::Pending
}

/// Update plan_status based on current phase states.
/// Preserves terminal states (Aborted, Completed) that were set explicitly.
pub fn update_plan_status(state: &mut PlanState) {
    match state.plan_status {
        PlanStatus::Aborted | PlanStatus::Completed => return,
        _ => {}
    }
    state.plan_status = derive_plan_status(&state.phases);
}

/// Check if the plan is complete (all phases passed or skipped).
pub fn is_plan_complete(state: &PlanState) -> bool {
    state.plan_status == PlanStatus::Completed
}

/// Check if the plan is blocked (any phase failed or stale).
pub fn is_plan_blocked(state: &PlanState) -> bool {
    state.plan_status == PlanStatus::Blocked
}

/// Detect stale phases: phases marked Running/Checking whose start time
/// exceeds the timeout. Called on plan resume to handle orphaned states.
/// Returns `(phase_id, attempts)` for every phase transitioned to Stale so
/// the caller can notify the terminal transition (GH-564 P1-2: resume-time
/// Running/Checking → Stale is a terminal transition and must be reported).
pub fn detect_stale_phases(state: &mut PlanState, plan: &Plan) -> Vec<(String, u32)> {
    let now = time::OffsetDateTime::now_utc();
    let mut stale = Vec::new();

    for phase_state in &mut state.phases {
        if phase_state.status != PhaseStatus::Running && phase_state.status != PhaseStatus::Checking
        {
            continue;
        }

        let Some(started) = &phase_state.started_at else {
            continue;
        };

        let started_time = match time::OffsetDateTime::parse(
            started,
            &time::format_description::well_known::Rfc3339,
        ) {
            Ok(t) => t,
            Err(_) => continue,
        };

        let plan_phase = plan.phases.iter().find(|p| p.id == phase_state.id);
        let timeout_sec = plan_phase
            .and_then(|p| p.timeout_sec)
            .unwrap_or(plan.timeout_sec);

        let elapsed = now - started_time;
        if elapsed > time::Duration::seconds(timeout_sec as i64) {
            phase_state.status = PhaseStatus::Stale;
            phase_state.error = Some(ErrorInfo {
                error_type: ErrorType::Timeout,
                message: "phase was running when conductor stopped".into(),
                retryable: true,
                check_index: None,
                timestamp: now
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
            });
            stale.push((phase_state.id.clone(), phase_state.attempts));
        }
    }

    stale
}

/// Find the next runnable phase: Pending with all dependencies satisfied.
pub fn find_next_phase(plan: &Plan, state: &PlanState, order: &[String]) -> Option<String> {
    for phase_id in order {
        let phase_state = state.phases.iter().find(|p| p.id == *phase_id)?;
        if phase_state.status != PhaseStatus::Pending {
            continue;
        }
        let phase = plan.phases.iter().find(|p| p.id == *phase_id)?;
        let deps_ok = phase.depends_on.iter().all(|dep| {
            state
                .phases
                .iter()
                .find(|p| p.id == *dep)
                .map(|p| {
                    p.status == PhaseStatus::Passed
                        || p.status == PhaseStatus::Skipped
                        // GH-552: a waived gate timeout shipped its commit;
                        // dependents may run.
                        || (p.status == PhaseStatus::GateTimedOut && p.skip_reason.is_some())
                })
                .unwrap_or(false)
        });
        if deps_ok {
            return Some(phase_id.clone());
        }
    }
    None
}

/// Restore latest-attempt timing from the append-only evidence, never timestamps.
pub fn hydrate_durations(cwd: &std::path::Path, state: &mut PlanState) {
    use std::io::BufRead;
    let path = cwd
        .join(".edda/conductor")
        .join(&state.plan_name)
        .join("events.jsonl");
    let Ok(file) = std::fs::File::open(path) else {
        return;
    };
    for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(id) = event["phase_id"].as_str() else {
            continue;
        };
        let Some(phase) = state.phases.iter_mut().find(|p| p.id == id) else {
            continue;
        };
        // Only evidence for the persisted current attempt may change its
        // measurement. Legacy or malformed records without an attempt, and
        // records from a later attempt, must not overwrite a serialized
        // duration after restart.
        if event["attempt"].as_u64() != Some(u64::from(phase.attempts)) {
            continue;
        }
        match event["type"].as_str() {
            Some("phase_start") => phase.duration_ms = None,
            Some("phase_passed" | "phase_failed") => {
                // Older gate rejections emitted a fabricated zero. It is not a measurement.
                phase.duration_ms = if event["error_type"] == "gate_rejected" {
                    None
                } else {
                    event["duration_ms"].as_u64()
                };
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::parser::parse_plan;
    use crate::state::machine::{
        transition, PhaseState, PhaseStatus, PhaseUpdate, PlanState, PlanStatus,
    };

    fn make_state(statuses: &[(&str, PhaseStatus)]) -> Vec<PhaseState> {
        statuses
            .iter()
            .map(|(id, status)| PhaseState {
                id: id.to_string(),
                status: *status,
                started_at: None,
                completed_at: None,
                attempts: 0,
                checks: Vec::new(),
                error: None,
                skip_reason: None,
                retry_context: None,
                gate_sha: None,
                gate_entered_at: None,
                gate_redispatches: 0,
                verdict_decision: None,
                verdict_actor: None,
                verdict_comment: None,
                env_retries: 0,
                cost_usd: None,
                duration_ms: None,
            })
            .collect()
    }

    #[test]
    fn hydrate_durations_uses_only_the_current_attempt() {
        let root = tempfile::tempdir().unwrap();
        let plan = parse_plan("name: measured\nphases:\n  - id: build\n    prompt: x\n").unwrap();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");
        let phase = state.get_phase_mut("build").unwrap();
        phase.attempts = 2;
        // A retry was persisted immediately after its attempt boundary, before
        // the new attempt wrote a start or terminal event.
        phase.duration_ms = None;

        let events_dir = root.path().join(".edda/conductor/measured");
        std::fs::create_dir_all(&events_dir).unwrap();
        std::fs::write(
            events_dir.join("events.jsonl"),
            concat!(
                "{\"type\":\"phase_passed\",\"phase_id\":\"build\",\"attempt\":1,\"duration_ms\":100}\n",
                "{\"type\":\"phase_start\",\"phase_id\":\"build\",\"attempt\":3}\n",
                "{\"type\":\"phase_failed\",\"phase_id\":\"build\",\"duration_ms\":300}\n"
            ),
        )
        .unwrap();

        hydrate_durations(root.path(), &mut state);
        assert_eq!(state.get_phase("build").unwrap().duration_ms, None);

        // Hydration must preserve a serialized duration when every available
        // record is malformed, missing an attempt, or for another attempt.
        state.get_phase_mut("build").unwrap().duration_ms = Some(222);
        hydrate_durations(root.path(), &mut state);
        assert_eq!(state.get_phase("build").unwrap().duration_ms, Some(222));

        std::fs::write(
            events_dir.join("events.jsonl"),
            "{\"type\":\"phase_start\",\"phase_id\":\"build\",\"attempt\":2}\n{\"type\":\"phase_failed\",\"phase_id\":\"build\",\"attempt\":2,\"duration_ms\":500}\n",
        )
        .unwrap();
        hydrate_durations(root.path(), &mut state);
        assert_eq!(state.get_phase("build").unwrap().duration_ms, Some(500));
    }

    #[test]
    fn derive_pending() {
        let phases = make_state(&[("a", PhaseStatus::Pending), ("b", PhaseStatus::Pending)]);
        assert_eq!(derive_plan_status(&phases), PlanStatus::Pending);
    }

    #[test]
    fn derive_running() {
        let phases = make_state(&[("a", PhaseStatus::Running), ("b", PhaseStatus::Pending)]);
        assert_eq!(derive_plan_status(&phases), PlanStatus::Running);
    }

    #[test]
    fn derive_blocked() {
        let phases = make_state(&[("a", PhaseStatus::Failed), ("b", PhaseStatus::Pending)]);
        assert_eq!(derive_plan_status(&phases), PlanStatus::Blocked);
    }

    #[test]
    fn derive_completed_all_passed() {
        let phases = make_state(&[("a", PhaseStatus::Passed), ("b", PhaseStatus::Passed)]);
        assert_eq!(derive_plan_status(&phases), PlanStatus::Completed);
    }

    #[test]
    fn derive_completed_mixed_passed_skipped() {
        let phases = make_state(&[("a", PhaseStatus::Passed), ("b", PhaseStatus::Skipped)]);
        assert_eq!(derive_plan_status(&phases), PlanStatus::Completed);
    }

    /// GH-552: an unwaived gate timeout blocks the plan; a waived one
    /// (skip_reason set) counts as a resolved dependency toward completion.
    #[test]
    fn derive_gate_timed_out_waived_vs_unwaived() {
        let mut unwaived = make_state(&[("a", PhaseStatus::GateTimedOut)]);
        assert_eq!(derive_plan_status(&unwaived), PlanStatus::Blocked);

        unwaived[0].skip_reason = Some("gate waived".into());
        assert_eq!(derive_plan_status(&unwaived), PlanStatus::Completed);
    }

    /// GH-552: a waived gate timeout satisfies dependencies for later
    /// phases; an unwaived one does not.
    #[test]
    fn find_next_phase_treats_waived_gate_timeout_as_satisfied_dep() {
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "x"
  - id: b
    prompt: "y"
    depends_on: [a]
"#;
        let plan = parse_plan(yaml).unwrap();
        let mut state = PlanState::from_plan(&plan, "test.yaml");
        state.phases[0].status = PhaseStatus::GateTimedOut;
        let order = vec!["a".to_string(), "b".to_string()];
        assert!(
            find_next_phase(&plan, &state, &order).is_none(),
            "unwaived gate timeout must not unblock dependents"
        );
        state.phases[0].skip_reason = Some("gate waived".into());
        assert_eq!(
            find_next_phase(&plan, &state, &order).as_deref(),
            Some("b"),
            "waived gate timeout satisfies the dependency"
        );
    }

    #[test]
    fn find_next_respects_order() {
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "x"
  - id: b
    prompt: "x"
    depends_on: [a]
"#;
        let plan = parse_plan(yaml).unwrap();
        let state = PlanState::from_plan(&plan, "plan.yaml");
        let order = vec!["a".to_string(), "b".to_string()];

        // 'a' is first runnable
        assert_eq!(find_next_phase(&plan, &state, &order), Some("a".into()));
    }

    #[test]
    fn find_next_skips_unmet_deps() {
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "x"
  - id: b
    prompt: "x"
    depends_on: [a]
"#;
        let plan = parse_plan(yaml).unwrap();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");
        let order = vec!["a".to_string(), "b".to_string()];

        // Make 'a' running → 'b' can't start
        transition(
            &mut state,
            "a",
            PhaseStatus::Pending,
            PhaseStatus::Running,
            None,
        )
        .unwrap();
        assert_eq!(find_next_phase(&plan, &state, &order), None);
    }

    #[test]
    fn find_next_after_dep_passed() {
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "x"
  - id: b
    prompt: "x"
    depends_on: [a]
"#;
        let plan = parse_plan(yaml).unwrap();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");
        let order = vec!["a".to_string(), "b".to_string()];

        // Complete 'a'
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
            PhaseStatus::Passed,
            None,
        )
        .unwrap();

        assert_eq!(find_next_phase(&plan, &state, &order), Some("b".into()));
    }

    #[test]
    fn find_next_none_when_all_done() {
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "x"
"#;
        let plan = parse_plan(yaml).unwrap();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");
        let order = vec!["a".to_string()];

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
            PhaseStatus::Passed,
            None,
        )
        .unwrap();

        assert_eq!(find_next_phase(&plan, &state, &order), None);
    }

    #[test]
    fn detect_stale_marks_old_running() {
        let yaml = r#"
name: test
timeout_sec: 60
phases:
  - id: a
    prompt: "x"
"#;
        let plan = parse_plan(yaml).unwrap();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");

        // Simulate a phase that started 2 hours ago
        let old_time = (time::OffsetDateTime::now_utc() - time::Duration::hours(2))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        transition(
            &mut state,
            "a",
            PhaseStatus::Pending,
            PhaseStatus::Running,
            Some(PhaseUpdate {
                started_at: Some(old_time),
                ..Default::default()
            }),
        )
        .unwrap();

        detect_stale_phases(&mut state, &plan);
        assert_eq!(state.get_phase("a").unwrap().status, PhaseStatus::Stale);
        assert!(state.get_phase("a").unwrap().error.is_some());
    }

    #[test]
    fn detect_stale_reports_transitioned_phase_ids_and_attempts() {
        // GH-564 P1-2: the caller needs (phase_id, attempts) for every
        // Running/Checking → Stale transition so it can emit the terminal
        // notification.
        let yaml = r#"
name: test
timeout_sec: 60
phases:
  - id: a
    prompt: "x"
"#;
        let plan = parse_plan(yaml).unwrap();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");

        let old_time = (time::OffsetDateTime::now_utc() - time::Duration::hours(2))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        transition(
            &mut state,
            "a",
            PhaseStatus::Pending,
            PhaseStatus::Running,
            Some(PhaseUpdate {
                started_at: Some(old_time),
                ..Default::default()
            }),
        )
        .unwrap();

        let stale = detect_stale_phases(&mut state, &plan);
        assert_eq!(stale, vec![("a".to_string(), 0)]);
    }

    #[test]
    fn detect_stale_ignores_fresh_running() {
        let yaml = r#"
name: test
timeout_sec: 1800
phases:
  - id: a
    prompt: "x"
"#;
        let plan = parse_plan(yaml).unwrap();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");

        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        transition(
            &mut state,
            "a",
            PhaseStatus::Pending,
            PhaseStatus::Running,
            Some(PhaseUpdate {
                started_at: Some(now),
                ..Default::default()
            }),
        )
        .unwrap();

        detect_stale_phases(&mut state, &plan);
        assert_eq!(state.get_phase("a").unwrap().status, PhaseStatus::Running);
    }
}
