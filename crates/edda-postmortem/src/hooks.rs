//! Rule execution via hooks.
//!
//! Rules are NOT context injections (50-70% compliance). They are hooks
//! that block or warn (100% compliance). This module provides the
//! enforcement interface for the bridge hook system.
//!
//! Execution model:
//! - PreCommit hook reads rules store -> executes matching checks
//! - Each active rule's trigger is matched against the current context
//! - Matching rules produce either a block (exit 1) or warn (stderr)

use crate::rules::{RuleCategory, RulesStore};
use serde::{Deserialize, Serialize};

/// Action to take when a rule matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enforcement {
    /// Block the operation with a message.
    Block(String),
    /// Warn but allow the operation.
    Warn(String),
}

/// Context for evaluating rules against current operation.
#[derive(Debug, Clone, Default)]
pub struct HookContext {
    /// Which hook event is firing (e.g., "PreToolUse", "PostToolUse").
    pub hook_event: String,
    /// Tool being used (e.g., "Bash", "Write", "Edit").
    pub tool_name: String,
    /// Files being modified in this operation.
    pub files_touched: Vec<String>,
    /// Current working directory.
    pub cwd: String,
}

/// Result of evaluating all active rules against a hook context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationResult {
    pub rules_checked: usize,
    pub rules_matched: usize,
    pub matched_rule_ids: Vec<String>,
    pub enforcements: Vec<EnforcementRecord>,
}

/// Record of a single rule enforcement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnforcementRecord {
    pub rule_id: String,
    pub trigger: String,
    pub action: String,
    pub category: String,
}

/// Map hook event to the relevant rule categories.
fn relevant_categories(hook_event: &str) -> Vec<RuleCategory> {
    match hook_event {
        "PreToolUse" => vec![
            RuleCategory::PreCommit,
            RuleCategory::CodePattern,
            RuleCategory::Workflow,
        ],
        "PostToolUse" => vec![RuleCategory::CodePattern, RuleCategory::Workflow],
        _ => vec![RuleCategory::Workflow],
    }
}

/// Evaluate all active rules against the current hook context.
///
/// Returns matched rules and their enforcement actions. The caller
/// (bridge dispatch) decides whether to block or warn based on results.
pub fn evaluate_rules(store: &RulesStore, ctx: &HookContext) -> EvaluationResult {
    let active = store.active_rules();
    let categories = relevant_categories(&ctx.hook_event);
    let mut matched_ids = Vec::new();
    let mut enforcements = Vec::new();

    for rule in &active {
        // Filter by category relevance
        if !categories.contains(&rule.category) {
            continue;
        }

        // Match trigger against context
        if matches_trigger(&rule.trigger, ctx) {
            matched_ids.push(rule.id.clone());
            enforcements.push(EnforcementRecord {
                rule_id: rule.id.clone(),
                trigger: rule.trigger.clone(),
                action: rule.action.clone(),
                category: rule.category.to_string(),
            });
        }
    }

    EvaluationResult {
        rules_checked: active.len(),
        rules_matched: matched_ids.len(),
        matched_rule_ids: matched_ids,
        enforcements,
    }
}

/// Record hits for all matched rules (updates last_hit and hit count).
pub fn record_matched_hits(store: &mut RulesStore, matched_ids: &[String]) {
    for id in matched_ids {
        if let Some(rule) = store.get_mut(id) {
            rule.record_hit();
        }
    }
}

/// Format enforcement results as a warning message for the user.
pub fn format_warnings(result: &EvaluationResult) -> Option<String> {
    if result.enforcements.is_empty() {
        return None;
    }

    let mut lines = vec!["[edda L3] Learned rules triggered:".to_string()];
    for e in &result.enforcements {
        lines.push(format!("  - {} -> {}", e.trigger, e.action));
    }
    Some(lines.join("\n"))
}

// -- Trigger matching --

/// Check if a rule trigger matches the current hook context.
///
/// Trigger format:
///   - `file_churn:<path>` -- matches if the path is in files_touched
///   - `command_failure:<cmd>` -- matches if tool_name is "Bash"
///   - `multi_agent_start` -- matches on SessionStart-like events
///   - Plain text -- substring match against tool_name or files_touched
fn matches_trigger(trigger: &str, ctx: &HookContext) -> bool {
    if let Some(path) = trigger.strip_prefix("file_churn:") {
        return ctx.files_touched.iter().any(|f| f.contains(path));
    }

    if let Some(cmd) = trigger.strip_prefix("command_failure:") {
        return ctx.tool_name == "Bash" && ctx.cwd.contains(cmd);
    }

    if trigger == "multi_agent_start" {
        return ctx.hook_event == "SessionStart";
    }

    // Fallback: substring match on tool name or files
    if ctx.tool_name.contains(trigger) {
        return true;
    }
    ctx.files_touched.iter().any(|f| f.contains(trigger))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{Rule, RuleCategory, RuleStatus, RulesStore};

    fn active_rule(trigger: &str, action: &str, category: RuleCategory) -> Rule {
        Rule {
            id: format!("rule_test_{}", trigger.replace(':', "_")),
            trigger: trigger.to_string(),
            action: action.to_string(),
            anchor_file: None,
            anchor_hash: None,
            created: "2026-01-01T00:00:00Z".to_string(),
            last_hit: "2026-01-01T00:00:00Z".to_string(),
            hits: 2,
            ttl_days: 30,
            superseded_by: None,
            status: RuleStatus::Active,
            source_session: "test".to_string(),
            source_event: None,
            category,
        }
    }

    fn make_store(rules: Vec<Rule>) -> RulesStore {
        RulesStore {
            rules,
            last_decay_run: None,
        }
    }

    #[test]
    fn file_churn_trigger_matches_touched_files() {
        let store = make_store(vec![active_rule(
            "file_churn:src/main.rs",
            "Review carefully",
            RuleCategory::PreCommit,
        )]);

        let ctx = HookContext {
            hook_event: "PreToolUse".to_string(),
            tool_name: "Write".to_string(),
            files_touched: vec!["src/main.rs".to_string()],
            cwd: "/project".to_string(),
        };

        let result = evaluate_rules(&store, &ctx);
        assert_eq!(result.rules_matched, 1);
    }

    #[test]
    fn no_match_when_file_not_touched() {
        let store = make_store(vec![active_rule(
            "file_churn:src/main.rs",
            "Review carefully",
            RuleCategory::PreCommit,
        )]);

        let ctx = HookContext {
            hook_event: "PreToolUse".to_string(),
            tool_name: "Write".to_string(),
            files_touched: vec!["src/lib.rs".to_string()],
            cwd: "/project".to_string(),
        };

        let result = evaluate_rules(&store, &ctx);
        assert_eq!(result.rules_matched, 0);
    }

    #[test]
    fn dormant_rules_not_evaluated() {
        let mut rule = active_rule(
            "file_churn:src/main.rs",
            "Review carefully",
            RuleCategory::PreCommit,
        );
        rule.status = RuleStatus::Dormant;
        let store = make_store(vec![rule]);

        let ctx = HookContext {
            hook_event: "PreToolUse".to_string(),
            tool_name: "Write".to_string(),
            files_touched: vec!["src/main.rs".to_string()],
            cwd: "/project".to_string(),
        };

        let result = evaluate_rules(&store, &ctx);
        assert_eq!(result.rules_matched, 0);
    }

    #[test]
    fn format_warnings_empty_when_no_matches() {
        let result = EvaluationResult {
            rules_checked: 5,
            rules_matched: 0,
            matched_rule_ids: vec![],
            enforcements: vec![],
        };
        assert!(format_warnings(&result).is_none());
    }

    #[test]
    fn format_warnings_produces_output() {
        let result = EvaluationResult {
            rules_checked: 5,
            rules_matched: 1,
            matched_rule_ids: vec!["rule_1".to_string()],
            enforcements: vec![EnforcementRecord {
                rule_id: "rule_1".to_string(),
                trigger: "file_churn:main.rs".to_string(),
                action: "Review carefully".to_string(),
                category: "pre_commit".to_string(),
            }],
        };
        let warning = format_warnings(&result).unwrap();
        assert!(warning.contains("Learned rules triggered"));
        assert!(warning.contains("file_churn:main.rs"));
    }
}
