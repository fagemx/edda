//! Post-mortem analysis: produce lessons and rule proposals from session data.
//!
//! The analyzer takes session statistics and trigger reasons, then produces
//! structured findings. The current implementation uses deterministic heuristics;
//! future versions can delegate to LLM (Sonnet) for deeper analysis.
//!
//! Output hierarchy (from GH-157 spec):
//!   - **Rules**: Hook-enforced (block/auto-run), 100% compliance
//!   - **Lessons**: CLAUDE.md auto-maintained paragraph, ~90% compliance
//!   - **Observations**: `edda ask` on-demand, no enforcement

use serde::{Deserialize, Serialize};

use crate::rules::RuleCategory;
use crate::trigger::{PostMortemTrigger, TriggerReason};

/// Severity of a lesson.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LessonSeverity {
    /// Minor observation, useful context.
    Low,
    /// Actionable insight, should influence future work.
    Medium,
    /// Critical lesson, likely produces a rule proposal.
    High,
}

/// A lesson extracted from post-mortem analysis (descriptive, not prescriptive).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    pub id: String,
    pub text: String,
    pub severity: LessonSeverity,
    pub tags: Vec<String>,
    pub source_trigger: String,
}

/// A rule proposal from post-mortem analysis (prescriptive).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleProposal {
    pub trigger: String,
    pub action: String,
    pub anchor_file: Option<String>,
    pub category: RuleCategory,
    pub confidence: f64,
    pub evidence: Vec<String>,
}

/// Complete result of a post-mortem analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostMortemResult {
    pub session_id: String,
    pub triggers: Vec<TriggerReason>,
    pub lessons: Vec<Lesson>,
    pub rule_proposals: Vec<RuleProposal>,
    pub analyzed_at: String,
}

/// Session data available for analysis.
#[derive(Debug, Clone, Default)]
pub struct AnalysisInput {
    pub session_id: String,
    pub user_prompts: u64,
    pub tool_failures: u64,
    pub failed_commands: Vec<String>,
    pub files_modified: Vec<String>,
    pub file_edit_counts: Vec<(String, u64)>,
    pub commits_made: Vec<String>,
    pub decisions_superseded: u64,
    pub had_conflict: bool,
    pub outcome: String,
    pub duration_minutes: u64,
}

/// Run deterministic post-mortem analysis on a triggered session.
///
/// Produces lessons and rule proposals based on trigger reasons and session data.
/// This is the heuristic analyzer — no LLM calls. Future: `analyze_with_llm()`.
pub fn analyze(trigger: &PostMortemTrigger, input: &AnalysisInput) -> PostMortemResult {
    let mut lessons = Vec::new();
    let mut rule_proposals = Vec::new();

    for reason in &trigger.reasons {
        match reason {
            TriggerReason::SessionFailures => {
                analyze_failures(input, &mut lessons, &mut rule_proposals);
            }
            TriggerReason::AbnormallyLong => {
                analyze_long_session(input, &mut lessons);
            }
            TriggerReason::ExcessiveFileEdits => {
                analyze_file_churn(input, &mut lessons, &mut rule_proposals);
            }
            TriggerReason::DecisionSuperseded => {
                analyze_decision_reversal(input, &mut lessons);
            }
            TriggerReason::MultiAgentConflict => {
                analyze_conflict(input, &mut lessons, &mut rule_proposals);
            }
        }
    }

    PostMortemResult {
        session_id: input.session_id.clone(),
        triggers: trigger.reasons.clone(),
        lessons,
        rule_proposals,
        analyzed_at: now_rfc3339(),
    }
}

// -- Per-trigger analyzers --

fn analyze_failures(
    input: &AnalysisInput,
    lessons: &mut Vec<Lesson>,
    rule_proposals: &mut Vec<RuleProposal>,
) {
    // Lesson: session had failures
    if input.outcome == "error_stuck" {
        lessons.push(Lesson {
            id: new_lesson_id(),
            text: format!(
                "Session got stuck after {} consecutive failures. \
                 Consider breaking the task into smaller steps.",
                input.tool_failures
            ),
            severity: LessonSeverity::High,
            tags: vec!["failure".into(), "stuck".into()],
            source_trigger: "session_failures".into(),
        });
    }

    // Rule proposal: if specific commands keep failing, suggest a pre-check
    for cmd in &input.failed_commands {
        let short_cmd = cmd.split_whitespace().next().unwrap_or(cmd);
        rule_proposals.push(RuleProposal {
            trigger: format!("command_failure:{short_cmd}"),
            action: format!("Verify {short_cmd} is available and configured before running"),
            anchor_file: None,
            category: RuleCategory::Workflow,
            confidence: 0.6,
            evidence: vec![format!("Failed command: {cmd}")],
        });
    }

    if input.tool_failures > 3 {
        lessons.push(Lesson {
            id: new_lesson_id(),
            text: format!(
                "{} tool failures in session. Pattern suggests environment or dependency issue.",
                input.tool_failures
            ),
            severity: LessonSeverity::Medium,
            tags: vec!["failure".into(), "tools".into()],
            source_trigger: "session_failures".into(),
        });
    }
}

fn analyze_long_session(input: &AnalysisInput, lessons: &mut Vec<Lesson>) {
    lessons.push(Lesson {
        id: new_lesson_id(),
        text: format!(
            "Session ran for {} user prompts ({} minutes). \
             Long sessions reduce focus — consider splitting into sub-tasks.",
            input.user_prompts, input.duration_minutes
        ),
        severity: LessonSeverity::Medium,
        tags: vec!["long_session".into(), "productivity".into()],
        source_trigger: "abnormally_long".into(),
    });
}

fn analyze_file_churn(
    input: &AnalysisInput,
    lessons: &mut Vec<Lesson>,
    rule_proposals: &mut Vec<RuleProposal>,
) {
    let churned: Vec<&(String, u64)> = input
        .file_edit_counts
        .iter()
        .filter(|(_, count)| *count >= 3)
        .collect();

    for (file, count) in &churned {
        lessons.push(Lesson {
            id: new_lesson_id(),
            text: format!(
                "File '{file}' was edited {count} times. \
                 Frequent edits to the same file suggest unclear requirements or iterative debugging.",
            ),
            severity: LessonSeverity::Medium,
            tags: vec!["churn".into(), "file_edits".into()],
            source_trigger: "excessive_file_edits".into(),
        });

        // Propose a rule: if a file is frequently edited, add a pre-commit check
        rule_proposals.push(RuleProposal {
            trigger: format!("file_churn:{file}"),
            action: format!("Review '{file}' carefully before committing — historically unstable"),
            anchor_file: Some(file.clone()),
            category: RuleCategory::PreCommit,
            confidence: 0.5,
            evidence: vec![format!("Edited {count} times in session {}", input.session_id)],
        });
    }
}

fn analyze_decision_reversal(input: &AnalysisInput, lessons: &mut Vec<Lesson>) {
    lessons.push(Lesson {
        id: new_lesson_id(),
        text: format!(
            "{} decision(s) were superseded during this session. \
             Consider spending more time on upfront design before committing to an approach.",
            input.decisions_superseded
        ),
        severity: LessonSeverity::High,
        tags: vec!["decision".into(), "reversal".into()],
        source_trigger: "decision_superseded".into(),
    });
}

fn analyze_conflict(
    input: &AnalysisInput,
    lessons: &mut Vec<Lesson>,
    rule_proposals: &mut Vec<RuleProposal>,
) {
    lessons.push(Lesson {
        id: new_lesson_id(),
        text: "Multi-agent conflict detected. \
               Ensure agents claim non-overlapping scopes before starting work."
            .to_string(),
        severity: LessonSeverity::High,
        tags: vec!["conflict".into(), "multi_agent".into()],
        source_trigger: "multi_agent_conflict".into(),
    });

    rule_proposals.push(RuleProposal {
        trigger: "multi_agent_start".to_string(),
        action: "Run `edda claim` to claim scope before starting multi-agent work".to_string(),
        anchor_file: None,
        category: RuleCategory::Workflow,
        confidence: 0.8,
        evidence: vec![format!(
            "Conflict detected in session {}",
            input.session_id
        )],
    });
}

// -- Helpers --

fn new_lesson_id() -> String {
    format!("lesson_{}", ulid::Ulid::new().to_string().to_lowercase())
}

fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::PostMortemTrigger;

    fn trigger_with(reasons: Vec<TriggerReason>) -> PostMortemTrigger {
        PostMortemTrigger {
            should_analyze: true,
            reasons,
            session_id: "test-session".to_string(),
        }
    }

    fn base_input() -> AnalysisInput {
        AnalysisInput {
            session_id: "test-session".to_string(),
            outcome: "completed".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn analyze_session_failures_produces_lessons() {
        let trigger = trigger_with(vec![TriggerReason::SessionFailures]);
        let mut input = base_input();
        input.outcome = "error_stuck".to_string();
        input.tool_failures = 5;
        input.failed_commands = vec!["npm test".to_string()];

        let result = analyze(&trigger, &input);
        assert!(!result.lessons.is_empty());
        assert!(!result.rule_proposals.is_empty());
        assert!(result
            .lessons
            .iter()
            .any(|l| l.tags.contains(&"stuck".to_string())));
    }

    #[test]
    fn analyze_long_session_produces_lesson() {
        let trigger = trigger_with(vec![TriggerReason::AbnormallyLong]);
        let mut input = base_input();
        input.user_prompts = 30;
        input.duration_minutes = 45;

        let result = analyze(&trigger, &input);
        assert_eq!(result.lessons.len(), 1);
        assert!(result.lessons[0].text.contains("30 user prompts"));
    }

    #[test]
    fn analyze_file_churn_produces_rule_proposal() {
        let trigger = trigger_with(vec![TriggerReason::ExcessiveFileEdits]);
        let mut input = base_input();
        input.file_edit_counts = vec![("src/main.rs".to_string(), 5)];

        let result = analyze(&trigger, &input);
        assert!(!result.lessons.is_empty());
        assert!(!result.rule_proposals.is_empty());
        assert!(result.rule_proposals[0].trigger.contains("file_churn"));
    }

    #[test]
    fn analyze_conflict_produces_high_severity() {
        let trigger = trigger_with(vec![TriggerReason::MultiAgentConflict]);
        let input = base_input();

        let result = analyze(&trigger, &input);
        assert!(result
            .lessons
            .iter()
            .any(|l| l.severity == LessonSeverity::High));
        assert!(!result.rule_proposals.is_empty());
    }

    #[test]
    fn multiple_triggers_produce_combined_results() {
        let trigger = trigger_with(vec![
            TriggerReason::SessionFailures,
            TriggerReason::AbnormallyLong,
            TriggerReason::ExcessiveFileEdits,
        ]);
        let mut input = base_input();
        input.tool_failures = 5;
        input.user_prompts = 30;
        input.duration_minutes = 45;
        input.file_edit_counts = vec![("a.rs".to_string(), 4)];

        let result = analyze(&trigger, &input);
        // Should have lessons from multiple triggers
        assert!(result.lessons.len() >= 3);
    }
}
