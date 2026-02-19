use crate::plan::schema::Plan;
use crate::state::machine::{
    ErrorInfo, ErrorType, PhaseStatus, PlanState, PlanStatus,
};

/// Derive plan-level status from phase states.
pub fn derive_plan_status(phases: &[crate::state::machine::PhaseState]) -> PlanStatus {
    if phases
        .iter()
        .any(|p| p.status == PhaseStatus::Running || p.status == PhaseStatus::Checking)
    {
        return PlanStatus::Running;
    }
    if phases
        .iter()
        .any(|p| p.status == PhaseStatus::Failed || p.status == PhaseStatus::Stale)
    {
        return PlanStatus::Blocked;
    }
    if phases
        .iter()
        .all(|p| p.status == PhaseStatus::Passed || p.status == PhaseStatus::Skipped)
    {
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
pub fn detect_stale_phases(state: &mut PlanState, plan: &Plan) {
    let now = time::OffsetDateTime::now_utc();

    for phase_state in &mut state.phases {
        if phase_state.status != PhaseStatus::Running
            && phase_state.status != PhaseStatus::Checking
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
        }
    }
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
                .map(|p| p.status == PhaseStatus::Passed || p.status == PhaseStatus::Skipped)
                .unwrap_or(false)
        });
        if deps_ok {
            return Some(phase_id.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::parser::parse_plan;
    use crate::state::machine::{PhaseState, PhaseStatus, PlanState, PlanStatus, transition, PhaseUpdate};

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
            })
            .collect()
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
        transition(&mut state, "a", PhaseStatus::Pending, PhaseStatus::Running, None).unwrap();
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
        transition(&mut state, "a", PhaseStatus::Pending, PhaseStatus::Running, None).unwrap();
        transition(&mut state, "a", PhaseStatus::Running, PhaseStatus::Checking, None).unwrap();
        transition(&mut state, "a", PhaseStatus::Checking, PhaseStatus::Passed, None).unwrap();

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

        transition(&mut state, "a", PhaseStatus::Pending, PhaseStatus::Running, None).unwrap();
        transition(&mut state, "a", PhaseStatus::Running, PhaseStatus::Checking, None).unwrap();
        transition(&mut state, "a", PhaseStatus::Checking, PhaseStatus::Passed, None).unwrap();

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
