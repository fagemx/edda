use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::plan::schema::Plan;

// ── Status enums ──

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PhaseStatus {
    Pending,
    Running,
    Checking,
    Passed,
    Failed,
    Skipped,
    Stale,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    Running,
    Blocked,
    Completed,
    Aborted,
}

// ── State types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanState {
    pub plan_name: String,
    pub plan_file: String,
    pub plan_status: PlanStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aborted_at: Option<String>,
    #[serde(default)]
    pub total_cost_usd: f64,
    pub phases: Vec<PhaseState>,
    #[serde(default)]
    pub version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseState {
    pub id: String,
    pub status: PhaseStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub checks: Vec<CheckResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    /// Error context from previous attempt, injected into retry prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_context: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub error_type: ErrorType,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_index: Option<usize>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorType {
    AgentCrash,
    CheckFailed,
    Timeout,
    BudgetExceeded,
    UserAbort,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub check_type: String,
    pub status: CheckStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Waiting,
    Running,
    Passed,
    Failed,
}

// ── Valid transitions ──

const VALID_TRANSITIONS: &[(PhaseStatus, &[PhaseStatus])] = &[
    (
        PhaseStatus::Pending,
        &[PhaseStatus::Running, PhaseStatus::Skipped],
    ),
    (
        PhaseStatus::Running,
        &[
            PhaseStatus::Checking,
            PhaseStatus::Failed,
            PhaseStatus::Stale,
        ],
    ),
    (
        PhaseStatus::Checking,
        &[PhaseStatus::Passed, PhaseStatus::Failed],
    ),
    (PhaseStatus::Failed, &[PhaseStatus::Pending]), // retry
    (PhaseStatus::Stale, &[PhaseStatus::Pending]),  // retry
                                                    // Passed and Skipped are terminal
];

fn is_valid_transition(from: PhaseStatus, to: PhaseStatus) -> bool {
    VALID_TRANSITIONS
        .iter()
        .any(|(f, targets)| *f == from && targets.contains(&to))
}

// ── Side effects ──

/// Optional side-effect data applied during a transition.
#[derive(Debug, Clone, Default)]
pub struct PhaseUpdate {
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub attempts: Option<u32>,
    pub checks: Option<Vec<CheckResult>>,
    pub error: Option<ErrorInfo>,
    pub skip_reason: Option<String>,
    pub retry_context: Option<Option<String>>,
}

impl PhaseUpdate {
    pub fn apply(self, phase: &mut PhaseState) {
        if let Some(v) = self.started_at {
            phase.started_at = Some(v);
        }
        if let Some(v) = self.completed_at {
            phase.completed_at = Some(v);
        }
        if let Some(v) = self.attempts {
            phase.attempts = v;
        }
        if let Some(v) = self.checks {
            phase.checks = v;
        }
        if self.error.is_some() {
            phase.error = self.error;
        }
        if let Some(v) = self.skip_reason {
            phase.skip_reason = Some(v);
        }
        if let Some(v) = self.retry_context {
            phase.retry_context = v;
        }
    }
}

// ── CAS-guarded transition ──

/// Transition a phase from `from` to `to`, applying side effects.
/// Returns Ok(true) on success, Ok(false) on CAS miss (current != from).
pub fn transition(
    state: &mut PlanState,
    phase_id: &str,
    from: PhaseStatus,
    to: PhaseStatus,
    side_effect: Option<PhaseUpdate>,
) -> Result<bool> {
    let phase = state.get_phase_mut(phase_id)?;
    if phase.status != from {
        return Ok(false); // CAS miss
    }
    if !is_valid_transition(from, to) {
        bail!("invalid transition: {phase_id} {from:?} → {to:?}");
    }
    phase.status = to;
    if let Some(update) = side_effect {
        update.apply(phase);
    }
    state.version += 1;
    Ok(true)
}

// ── PlanState methods ──

impl PlanState {
    /// Create initial state from a plan.
    pub fn from_plan(plan: &Plan, plan_file: &str) -> Self {
        let phases = plan
            .phases
            .iter()
            .map(|p| PhaseState {
                id: p.id.clone(),
                status: PhaseStatus::Pending,
                started_at: None,
                completed_at: None,
                attempts: 0,
                checks: Vec::new(),
                error: None,
                skip_reason: None,
                retry_context: None,
            })
            .collect();

        PlanState {
            plan_name: plan.name.clone(),
            plan_file: plan_file.to_string(),
            plan_status: PlanStatus::Pending,
            started_at: None,
            completed_at: None,
            aborted_at: None,
            total_cost_usd: 0.0,
            phases,
            version: 0,
        }
    }

    pub fn get_phase(&self, id: &str) -> Result<&PhaseState> {
        self.phases
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| anyhow::anyhow!("phase not found: \"{id}\""))
    }

    pub fn get_phase_mut(&mut self, id: &str) -> Result<&mut PhaseState> {
        self.phases
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| anyhow::anyhow!("phase not found: \"{id}\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::parser::parse_plan;

    fn test_plan() -> Plan {
        parse_plan(
            r#"
name: test
phases:
  - id: a
    prompt: "x"
  - id: b
    prompt: "x"
    depends_on: [a]
"#,
        )
        .unwrap()
    }

    #[test]
    fn from_plan_initializes_pending() {
        let plan = test_plan();
        let state = PlanState::from_plan(&plan, "plan.yaml");
        assert_eq!(state.plan_status, PlanStatus::Pending);
        assert_eq!(state.phases.len(), 2);
        assert!(state
            .phases
            .iter()
            .all(|p| p.status == PhaseStatus::Pending));
        assert_eq!(state.version, 0);
    }

    #[test]
    fn valid_transition_pending_to_running() {
        let plan = test_plan();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");
        let ok = transition(
            &mut state,
            "a",
            PhaseStatus::Pending,
            PhaseStatus::Running,
            None,
        )
        .unwrap();
        assert!(ok);
        assert_eq!(state.get_phase("a").unwrap().status, PhaseStatus::Running);
        assert_eq!(state.version, 1);
    }

    #[test]
    fn cas_miss_returns_false() {
        let plan = test_plan();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");
        // Try to transition from Running, but it's Pending
        let ok = transition(
            &mut state,
            "a",
            PhaseStatus::Running,
            PhaseStatus::Checking,
            None,
        )
        .unwrap();
        assert!(!ok);
        assert_eq!(state.get_phase("a").unwrap().status, PhaseStatus::Pending);
    }

    #[test]
    fn invalid_transition_errors() {
        let plan = test_plan();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");
        // Pending → Passed is not valid
        let err = transition(
            &mut state,
            "a",
            PhaseStatus::Pending,
            PhaseStatus::Passed,
            None,
        );
        assert!(err.is_err());
    }

    #[test]
    fn side_effects_applied() {
        let plan = test_plan();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");
        transition(
            &mut state,
            "a",
            PhaseStatus::Pending,
            PhaseStatus::Running,
            Some(PhaseUpdate {
                started_at: Some("2026-01-01T00:00:00Z".into()),
                attempts: Some(1),
                ..Default::default()
            }),
        )
        .unwrap();

        let phase = state.get_phase("a").unwrap();
        assert_eq!(phase.started_at.as_deref(), Some("2026-01-01T00:00:00Z"));
        assert_eq!(phase.attempts, 1);
    }

    #[test]
    fn retry_transition_failed_to_pending() {
        let plan = test_plan();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");

        // pending → running → failed → pending (retry)
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
            PhaseStatus::Failed,
            None,
        )
        .unwrap();
        let ok = transition(
            &mut state,
            "a",
            PhaseStatus::Failed,
            PhaseStatus::Pending,
            None,
        )
        .unwrap();
        assert!(ok);
        assert_eq!(state.get_phase("a").unwrap().status, PhaseStatus::Pending);
    }

    #[test]
    fn terminal_states_have_no_transitions() {
        let plan = test_plan();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");

        // Get to Passed
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

        // Passed → anything should fail
        let err = transition(
            &mut state,
            "a",
            PhaseStatus::Passed,
            PhaseStatus::Pending,
            None,
        );
        assert!(err.is_err());
    }

    #[test]
    fn unknown_phase_errors() {
        let plan = test_plan();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");
        let err = transition(
            &mut state,
            "nonexistent",
            PhaseStatus::Pending,
            PhaseStatus::Running,
            None,
        );
        assert!(err.is_err());
    }

    #[test]
    fn state_roundtrip_json() {
        let plan = test_plan();
        let state = PlanState::from_plan(&plan, "plan.yaml");
        let json = serde_json::to_string_pretty(&state).unwrap();
        let restored: PlanState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.plan_name, "test");
        assert_eq!(restored.phases.len(), 2);
    }
}
