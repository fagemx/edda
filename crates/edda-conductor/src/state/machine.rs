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
    /// Checks passed but an external verdict gate is holding the phase (D3).
    AwaitingVerdict,
    /// GH-552: the verdict gate expired with no verdict — the phase's work
    /// completed and its checks passed, so nothing failed and calling it
    /// `Failed` erases the audit distinction between "the agent could not do
    /// the work" and "the work is done, checked, and waiting on a human who
    /// did not show up". It is also never `Skipped`: the phase ran.
    ///
    /// Resolution: with `skip_reason` set the gate was **waived** — the plan
    /// treats it like a satisfied dependency and the record keeps the honest
    /// status; without it the plan is blocked and the phase offers
    /// retry/waive (interactive) or honors `on_gate_timeout` (headless).
    GateTimedOut,
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
    /// GH-533: whether any phase recorded a measured cost. `total_cost_usd`
    /// alone cannot distinguish "usage-free backend (codex)" from a genuine
    /// $0.00 run, so renderers must consult this flag instead of inferring
    /// from the zero sentinel. Legacy state files (field absent) restore as
    /// unmeasured — under-claiming beats asserting an unmeasured figure.
    #[serde(default)]
    pub cost_measured: bool,
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
    /// Git HEAD captured when the phase entered AWAITING_VERDICT (D3).
    /// On restart the wait resumes against this SHA without re-running the phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_sha: Option<String>,
    /// RFC3339 timestamp of when the phase entered AWAITING_VERDICT (D3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_entered_at: Option<String>,
    /// Completed redispatch cycles at the verdict gate (D6). Persisted and
    /// counted separately from `attempts` — D3 forbids incrementing attempt
    /// on redispatch, and a redispatch turn is not guaranteed to produce a
    /// new commit, so `max_attempts` can never bound the (subject, gate_sha)
    /// loop. This counter is the real bound.
    #[serde(default)]
    pub gate_redispatches: u32,
    /// Verdict metadata recorded when the gate resolved (D3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict_decision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict_actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict_comment: Option<String>,
    /// Environmental build failures charged so far this phase run (GH-540).
    /// `attempts` counts every dispatch (attempt numbers must stay unique —
    /// they key the session id), so the product attempt count is
    /// `attempts - env_retries`. Every environmental occurrence — including
    /// the one that exhausts the cap — is charged here (review round 1), so
    /// retrying stops once the counter passes `MAX_ENV_RETRIES` while product
    /// accounting stays exact across a manual retry.
    #[serde(default)]
    pub env_retries: u32,
    /// GH-584 review round 2: measured cost accumulated by THIS phase's agent
    /// turns. `None` = no turn ever reported a measured cost (#533: 0.0 ≠
    /// unmeasured). Failure writers read it so a later failure in the same
    /// phase (failed checks, gate rejection, gate timeout) reaches the
    /// workspace ledger with its cost instead of being flattened into
    /// unmeasured null. Redispatch turns accumulate; a fresh attempt resets
    /// it at attempt start. Serialized for persistence across restarts — a
    /// gate rejection after a resume must still find the cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
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
    /// A verdict gate rejected the phase (D3, on_reject: halt or bound hit).
    GateRejected,
    /// A machine-layer build fault named in the check output (GH-540, e.g.
    /// Windows LNK1104): not the agent's work, so retrying is worthwhile —
    /// but the retry is never charged to `max_attempts`.
    Environmental,
    /// The verdict gate aborted because the ledger stayed unreadable for
    /// the whole error budget (GH-541): infrastructure, not the agent's
    /// work — the gate cannot observe verdicts written to a ledger it
    /// cannot read.
    LedgerUnreadable,
    /// The verdict gate expired with no verdict (GH-552): the phase's work
    /// completed and its checks passed — the review, not the work, ran out
    /// of time. Distinct from `Timeout` (an agent/check timeout, where the
    /// work itself did not finish).
    GateTimeout,
}

impl ErrorType {
    /// Stable snake_case tag matching this enum's serde representation
    /// (GH-540 review round 1): the runner's `phase_failed` event carries it
    /// so generic JSONL consumers see the failure classification without
    /// deserializing `ErrorInfo`.
    pub fn tag(&self) -> &'static str {
        match self {
            ErrorType::AgentCrash => "agent_crash",
            ErrorType::CheckFailed => "check_failed",
            ErrorType::Timeout => "timeout",
            ErrorType::BudgetExceeded => "budget_exceeded",
            ErrorType::UserAbort => "user_abort",
            ErrorType::GateRejected => "gate_rejected",
            ErrorType::Environmental => "environmental",
            ErrorType::LedgerUnreadable => "ledger_unreadable",
            ErrorType::GateTimeout => "gate_timeout",
        }
    }
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
        &[
            PhaseStatus::Passed,
            PhaseStatus::Failed,
            PhaseStatus::AwaitingVerdict,
        ],
    ),
    (
        PhaseStatus::AwaitingVerdict,
        &[
            PhaseStatus::Passed,
            PhaseStatus::Failed,
            // Rejected verdict with on_reject: redispatch runs one more agent
            // turn in the SAME session (D3), so the phase goes back to Running.
            PhaseStatus::Running,
            // GH-552: the gate expired with no verdict — work completed,
            // checks passed, nothing failed.
            PhaseStatus::GateTimedOut,
        ],
    ),
    (PhaseStatus::Failed, &[PhaseStatus::Pending]), // retry
    (PhaseStatus::Stale, &[PhaseStatus::Pending]),  // retry
    (
        PhaseStatus::GateTimedOut,
        &[PhaseStatus::Pending], // retry (GH-552)
    ),
    // Passed and Skipped are terminal; GateTimedOut is resolved in place
    // (waive = skip_reason set on the same status), never through Skipped.
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
    /// Gate SHA captured when entering AWAITING_VERDICT.
    pub gate_sha: Option<String>,
    /// RFC3339 timestamp of gate entry.
    pub gate_entered_at: Option<String>,
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
        if let Some(v) = self.gate_sha {
            phase.gate_sha = Some(v);
        }
        if let Some(v) = self.gate_entered_at {
            phase.gate_entered_at = Some(v);
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
                gate_sha: None,
                gate_entered_at: None,
                gate_redispatches: 0,
                verdict_decision: None,
                verdict_actor: None,
                verdict_comment: None,
                env_retries: 0,
                cost_usd: None,
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
            cost_measured: false,
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

    /// Record a measured cost reported by an agent backend (GH-533).
    pub fn record_cost(&mut self, cost: f64) {
        self.total_cost_usd += cost;
        self.cost_measured = true;
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

    // ── AwaitingVerdict gate state (D2/D3) ──────────────────────────

    #[test]
    fn legacy_state_without_cost_measured_deserializes_unmeasured() {
        // Pre-GH-533 state files carry `total_cost_usd` but no `cost_measured`
        // flag; they must restore as unmeasured (under-claim), never as a
        // measured figure.
        let json = r#"{
            "plan_name": "test",
            "plan_file": "plan.yaml",
            "plan_status": "pending",
            "total_cost_usd": 0.0,
            "phases": [],
            "version": 0
        }"#;
        let restored: PlanState = serde_json::from_str(json).unwrap();
        assert!(!restored.cost_measured);
    }

    #[test]
    fn record_cost_marks_measured_and_accumulates() {
        let plan = test_plan();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");
        assert!(!state.cost_measured);
        state.record_cost(0.42);
        state.record_cost(0.11);
        assert!(state.cost_measured);
        assert!((state.total_cost_usd - 0.53).abs() < 1e-9);
    }

    #[test]
    fn awaiting_verdict_serializes_snake_case() {
        let json = serde_json::to_string(&PhaseStatus::AwaitingVerdict).unwrap();
        assert_eq!(json, r#""awaiting_verdict""#);
        let back: PhaseStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PhaseStatus::AwaitingVerdict);
    }

    fn run_to_checking(state: &mut PlanState, id: &str) {
        transition(state, id, PhaseStatus::Pending, PhaseStatus::Running, None).unwrap();
        transition(state, id, PhaseStatus::Running, PhaseStatus::Checking, None).unwrap();
    }

    #[test]
    fn checking_to_awaiting_verdict_is_valid() {
        let plan = test_plan();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");
        run_to_checking(&mut state, "a");
        transition(
            &mut state,
            "a",
            PhaseStatus::Checking,
            PhaseStatus::AwaitingVerdict,
            Some(PhaseUpdate {
                gate_sha: Some("abc123".into()),
                gate_entered_at: Some("2026-01-01T00:00:00Z".into()),
                ..Default::default()
            }),
        )
        .unwrap();
        let phase = state.get_phase("a").unwrap();
        assert_eq!(phase.status, PhaseStatus::AwaitingVerdict);
        assert_eq!(phase.gate_sha.as_deref(), Some("abc123"));
        assert_eq!(
            phase.gate_entered_at.as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
    }

    #[test]
    fn awaiting_verdict_resolves_to_passed_or_failed() {
        let plan = test_plan();

        // approve → Passed
        let mut state = PlanState::from_plan(&plan, "plan.yaml");
        run_to_checking(&mut state, "a");
        transition(
            &mut state,
            "a",
            PhaseStatus::Checking,
            PhaseStatus::AwaitingVerdict,
            None,
        )
        .unwrap();
        transition(
            &mut state,
            "a",
            PhaseStatus::AwaitingVerdict,
            PhaseStatus::Passed,
            None,
        )
        .unwrap();
        assert_eq!(state.get_phase("a").unwrap().status, PhaseStatus::Passed);

        // reject (halt) / timeout → Failed
        let mut state = PlanState::from_plan(&plan, "plan.yaml");
        run_to_checking(&mut state, "a");
        transition(
            &mut state,
            "a",
            PhaseStatus::Checking,
            PhaseStatus::AwaitingVerdict,
            None,
        )
        .unwrap();
        transition(
            &mut state,
            "a",
            PhaseStatus::AwaitingVerdict,
            PhaseStatus::Failed,
            None,
        )
        .unwrap();
        assert_eq!(state.get_phase("a").unwrap().status, PhaseStatus::Failed);
    }

    #[test]
    fn awaiting_verdict_to_pending_is_invalid() {
        let plan = test_plan();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");
        run_to_checking(&mut state, "a");
        transition(
            &mut state,
            "a",
            PhaseStatus::Checking,
            PhaseStatus::AwaitingVerdict,
            None,
        )
        .unwrap();
        let err = transition(
            &mut state,
            "a",
            PhaseStatus::AwaitingVerdict,
            PhaseStatus::Pending,
            None,
        );
        assert!(err.is_err());
    }

    #[test]
    fn awaiting_verdict_state_persists_gate_sha_and_restore() {
        let plan = test_plan();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");
        run_to_checking(&mut state, "a");
        transition(
            &mut state,
            "a",
            PhaseStatus::Checking,
            PhaseStatus::AwaitingVerdict,
            Some(PhaseUpdate {
                gate_sha: Some("deadbeef".into()),
                gate_entered_at: Some("2026-01-01T00:00:00Z".into()),
                ..Default::default()
            }),
        )
        .unwrap();

        let json = serde_json::to_string(&state).unwrap();
        let restored: PlanState = serde_json::from_str(&json).unwrap();
        let phase = restored.get_phase("a").unwrap();
        assert_eq!(phase.status, PhaseStatus::AwaitingVerdict);
        assert_eq!(phase.gate_sha.as_deref(), Some("deadbeef"));
        assert_eq!(
            phase.gate_entered_at.as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
    }
}
