//! Deterministic trigger detection for post-mortem analysis.
//!
//! Checks session statistics against thresholds to decide whether a
//! post-mortem analysis should run. No LLM calls — pure deterministic logic.

use serde::{Deserialize, Serialize};

/// Default threshold: sessions longer than this many user prompts are "abnormally long".
const DEFAULT_LONG_SESSION_THRESHOLD: u64 = 20;

/// Default threshold: a file edited this many times signals churn.
const DEFAULT_FILE_EDIT_THRESHOLD: u64 = 3;

/// Reason why a post-mortem was triggered.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TriggerReason {
    /// Session had command failures (tests broken, PR blocked, etc.)
    SessionFailures,
    /// Abnormally long session (> threshold user prompts)
    AbnormallyLong,
    /// Same file edited 3+ times (churn indicator)
    ExcessiveFileEdits,
    /// A decision was superseded during this session
    DecisionSuperseded,
    /// Multi-agent conflict detected
    MultiAgentConflict,
}

impl std::fmt::Display for TriggerReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionFailures => write!(f, "session_failures"),
            Self::AbnormallyLong => write!(f, "abnormally_long"),
            Self::ExcessiveFileEdits => write!(f, "excessive_file_edits"),
            Self::DecisionSuperseded => write!(f, "decision_superseded"),
            Self::MultiAgentConflict => write!(f, "multi_agent_conflict"),
        }
    }
}

/// Input data for trigger evaluation. Mirrors fields available from
/// `PrevDigest` / `SessionStats` without coupling to those types.
#[derive(Debug, Clone, Default)]
pub struct SessionSummary {
    pub session_id: String,
    pub user_prompts: u64,
    pub tool_failures: u64,
    pub failed_commands: Vec<String>,
    /// Map of file path -> edit count during session.
    pub file_edit_counts: Vec<(String, u64)>,
    /// Number of decisions superseded during this session.
    pub decisions_superseded: u64,
    /// Whether multi-agent conflicts were detected.
    pub had_conflict: bool,
    /// Session outcome: "completed", "interrupted", "error_stuck".
    pub outcome: String,
}

/// Result of trigger evaluation.
#[derive(Debug, Clone)]
pub struct PostMortemTrigger {
    pub should_analyze: bool,
    pub reasons: Vec<TriggerReason>,
    pub session_id: String,
}

/// Configuration for trigger thresholds. Overridable via environment variables.
#[derive(Debug, Clone)]
pub struct TriggerConfig {
    pub long_session_threshold: u64,
    pub file_edit_threshold: u64,
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self {
            long_session_threshold: std::env::var("EDDA_PM_LONG_SESSION")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_LONG_SESSION_THRESHOLD),
            file_edit_threshold: std::env::var("EDDA_PM_FILE_EDIT_THRESHOLD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_FILE_EDIT_THRESHOLD),
        }
    }
}

/// Evaluate whether a session warrants post-mortem analysis.
///
/// Returns a `PostMortemTrigger` with the decision and reasons.
/// All checks are deterministic — no LLM calls.
pub fn evaluate_triggers(summary: &SessionSummary) -> PostMortemTrigger {
    evaluate_triggers_with_config(summary, &TriggerConfig::default())
}

/// Evaluate with explicit config (useful for testing).
pub fn evaluate_triggers_with_config(
    summary: &SessionSummary,
    config: &TriggerConfig,
) -> PostMortemTrigger {
    let mut reasons = Vec::new();

    // 1. Session had failures
    if summary.tool_failures > 0
        || !summary.failed_commands.is_empty()
        || summary.outcome == "error_stuck"
    {
        reasons.push(TriggerReason::SessionFailures);
    }

    // 2. Abnormally long session
    if summary.user_prompts > config.long_session_threshold {
        reasons.push(TriggerReason::AbnormallyLong);
    }

    // 3. Excessive file edits (same file modified 3+ times)
    for (_, count) in &summary.file_edit_counts {
        if *count >= config.file_edit_threshold {
            reasons.push(TriggerReason::ExcessiveFileEdits);
            break; // one trigger is enough
        }
    }

    // 4. Decision superseded
    if summary.decisions_superseded > 0 {
        reasons.push(TriggerReason::DecisionSuperseded);
    }

    // 5. Multi-agent conflict
    if summary.had_conflict {
        reasons.push(TriggerReason::MultiAgentConflict);
    }

    let should_analyze = !reasons.is_empty();

    PostMortemTrigger {
        should_analyze,
        reasons,
        session_id: summary.session_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_summary() -> SessionSummary {
        SessionSummary {
            session_id: "test-session-1".to_string(),
            outcome: "completed".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn no_triggers_for_clean_session() {
        let summary = base_summary();
        let result = evaluate_triggers(&summary);
        assert!(!result.should_analyze);
        assert!(result.reasons.is_empty());
    }

    #[test]
    fn triggers_on_failures() {
        let mut summary = base_summary();
        summary.tool_failures = 3;
        let result = evaluate_triggers(&summary);
        assert!(result.should_analyze);
        assert!(result.reasons.contains(&TriggerReason::SessionFailures));
    }

    #[test]
    fn triggers_on_failed_commands() {
        let mut summary = base_summary();
        summary.failed_commands = vec!["npm test".to_string()];
        let result = evaluate_triggers(&summary);
        assert!(result.should_analyze);
        assert!(result.reasons.contains(&TriggerReason::SessionFailures));
    }

    #[test]
    fn triggers_on_error_stuck_outcome() {
        let mut summary = base_summary();
        summary.outcome = "error_stuck".to_string();
        let result = evaluate_triggers(&summary);
        assert!(result.should_analyze);
        assert!(result.reasons.contains(&TriggerReason::SessionFailures));
    }

    #[test]
    fn triggers_on_long_session() {
        let mut summary = base_summary();
        summary.user_prompts = 25;
        let result = evaluate_triggers(&summary);
        assert!(result.should_analyze);
        assert!(result.reasons.contains(&TriggerReason::AbnormallyLong));
    }

    #[test]
    fn triggers_on_file_churn() {
        let mut summary = base_summary();
        summary.file_edit_counts = vec![
            ("src/main.rs".to_string(), 5),
            ("src/lib.rs".to_string(), 1),
        ];
        let result = evaluate_triggers(&summary);
        assert!(result.should_analyze);
        assert!(result.reasons.contains(&TriggerReason::ExcessiveFileEdits));
    }

    #[test]
    fn triggers_on_decision_superseded() {
        let mut summary = base_summary();
        summary.decisions_superseded = 1;
        let result = evaluate_triggers(&summary);
        assert!(result.should_analyze);
        assert!(result.reasons.contains(&TriggerReason::DecisionSuperseded));
    }

    #[test]
    fn triggers_on_conflict() {
        let mut summary = base_summary();
        summary.had_conflict = true;
        let result = evaluate_triggers(&summary);
        assert!(result.should_analyze);
        assert!(result.reasons.contains(&TriggerReason::MultiAgentConflict));
    }

    #[test]
    fn multiple_triggers_accumulate() {
        let mut summary = base_summary();
        summary.tool_failures = 2;
        summary.user_prompts = 30;
        summary.had_conflict = true;
        let result = evaluate_triggers(&summary);
        assert!(result.should_analyze);
        assert_eq!(result.reasons.len(), 3);
    }

    #[test]
    fn custom_config_thresholds() {
        let mut summary = base_summary();
        summary.user_prompts = 10; // Below default 20, above custom 5
        summary.file_edit_counts = vec![("a.rs".to_string(), 2)]; // Below default 3, at custom 2

        let config = TriggerConfig {
            long_session_threshold: 5,
            file_edit_threshold: 2,
        };
        let result = evaluate_triggers_with_config(&summary, &config);
        assert!(result.should_analyze);
        assert_eq!(result.reasons.len(), 2);
    }
}
