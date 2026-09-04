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
    /// The command about to run (Bash tool_input.command), when available.
    pub command: Option<String>,
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

/// Record shows for matched rules: increments the show counter WITHOUT
/// updating `last_hit`, promoting Proposed rules, or reactivating Dormant/
/// Settled ones (GH-813: PreToolUse matches on every Bash call must not
/// reset the rule's decay TTL).
pub fn record_matched_shows(store: &mut RulesStore, matched_ids: &[String]) {
    store.record_matched_shows(matched_ids);
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
        // First-token keying (GH-813): match only when the previously failed
        // command is the command word of a segment of the incoming Bash
        // command (split on `;`, `&&`, `||`, `|`, newline). Matching every
        // Bash call — or any whole-word occurrence — flooded hooks with
        // irrelevant warnings AND record_hit() kept resetting the rule's
        // TTL, so noise rules never decayed — a self-feeding loop.
        // No command available → no match (silence over noise).
        if ctx.tool_name != "Bash" {
            return false;
        }
        let cmd = cmd.trim();
        if !crate::rules::is_trackable_command(cmd) {
            // Builtins/keywords/assignments never match, even if a legacy
            // store still holds such a rule; the decay cycle revokes it.
            return false;
        }
        return ctx.command.as_deref().is_some_and(|current| {
            crate::rules::split_command_segments(current)
                .iter()
                .any(|segment| crate::rules::command_word(segment).as_deref() == Some(cmd))
        });
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
            shows: 0,
            revoked_reason: None,
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
            command: None,
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
            command: None,
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
            command: None,
        };

        let result = evaluate_rules(&store, &ctx);
        assert_eq!(result.rules_matched, 0);
    }

    #[test]
    fn command_failure_matches_only_same_command() {
        let store = make_store(vec![active_rule(
            "command_failure:python",
            "Verify python is available",
            RuleCategory::Workflow,
        )]);

        // Bash call containing the failed command → match
        let hit_ctx = HookContext {
            hook_event: "PreToolUse".to_string(),
            tool_name: "Bash".to_string(),
            files_touched: vec![],
            cwd: "/project".to_string(),
            command: Some("python scripts/run.py".to_string()),
        };
        assert_eq!(evaluate_rules(&store, &hit_ctx).rules_matched, 1);

        // Unrelated Bash call → no match (this was the noise bug)
        let miss_ctx = HookContext {
            command: Some("git status".to_string()),
            ..hit_ctx.clone()
        };
        assert_eq!(evaluate_rules(&store, &miss_ctx).rules_matched, 0);

        // Substring-only occurrence inside another word → no match
        let substr_ctx = HookContext {
            command: Some("pythonic-helper --run".to_string()),
            ..hit_ctx.clone()
        };
        assert_eq!(evaluate_rules(&store, &substr_ctx).rules_matched, 0);

        // No command available → no match (silence over noise)
        let none_ctx = HookContext {
            command: None,
            ..hit_ctx.clone()
        };
        assert_eq!(evaluate_rules(&store, &none_ctx).rules_matched, 0);

        // Non-Bash tool → no match
        let write_ctx = HookContext {
            tool_name: "Write".to_string(),
            ..hit_ctx
        };
        assert_eq!(evaluate_rules(&store, &write_ctx).rules_matched, 0);
    }

    #[test]
    fn command_failure_keys_on_first_token_of_segments() {
        let store = make_store(vec![active_rule(
            "command_failure:python",
            "Verify python is available",
            RuleCategory::Workflow,
        )]);

        let ctx = HookContext {
            hook_event: "PreToolUse".to_string(),
            tool_name: "Bash".to_string(),
            files_touched: vec![],
            cwd: "/project".to_string(),
            command: Some("python scripts/run.py".to_string()),
        };
        assert_eq!(evaluate_rules(&store, &ctx).rules_matched, 1);

        // GH-813: `echo python` runs `echo`, not `python` — the failed
        // command only keys when it is the command word of a segment.
        let echo_ctx = HookContext {
            command: Some("echo python".to_string()),
            ..ctx.clone()
        };
        assert_eq!(evaluate_rules(&store, &echo_ctx).rules_matched, 0);

        // Segment splitting on `&&`, `;`, `|`, and newline: the next
        // segment's command word keys again.
        let seg_ctx = HookContext {
            command: Some("cd /tmp && python scripts/run.py".to_string()),
            ..ctx.clone()
        };
        assert_eq!(evaluate_rules(&store, &seg_ctx).rules_matched, 1);

        let pipe_ctx = HookContext {
            command: Some("cat data.txt | python -\nprint('x')".to_string()),
            ..ctx.clone()
        };
        assert_eq!(evaluate_rules(&store, &pipe_ctx).rules_matched, 1);

        // Leading environment variable assignments are skipped, so the
        // command word after them keys.
        let env_ctx = HookContext {
            command: Some("FOO=1 BAR=2 python scripts/run.py".to_string()),
            ..ctx.clone()
        };
        assert_eq!(evaluate_rules(&store, &env_ctx).rules_matched, 1);
    }

    #[test]
    fn command_failure_does_not_match_when_cmd_is_argument() {
        let store = make_store(vec![active_rule(
            "command_failure:python",
            "Verify python is available",
            RuleCategory::Workflow,
        )]);

        // GH-813: `python` appears only as an argument, never as a segment's
        // command word → no match.
        let ctx = HookContext {
            hook_event: "PreToolUse".to_string(),
            tool_name: "Bash".to_string(),
            files_touched: vec![],
            cwd: "/project".to_string(),
            command: Some("echo python; grep python file.txt".to_string()),
        };
        assert_eq!(evaluate_rules(&store, &ctx).rules_matched, 0);
    }

    #[test]
    fn command_failure_does_not_match_quoted_segment_content() {
        let store = make_store(vec![active_rule(
            "command_failure:python",
            "Verify python is available",
            RuleCategory::Workflow,
        )]);

        // GH-813: `python` inside a quoted argument is not a segment's
        // command word — the only segment runs `printf` → no match.
        let ctx = HookContext {
            hook_event: "PreToolUse".to_string(),
            tool_name: "Bash".to_string(),
            files_touched: vec![],
            cwd: "/project".to_string(),
            command: Some("printf '%s' 'skip; python -V'".to_string()),
        };
        assert_eq!(evaluate_rules(&store, &ctx).rules_matched, 0);

        // Quoted command word still keys: `"python" x.py` runs python.
        let quoted_ctx = HookContext {
            command: Some("\"python\" x.py".to_string()),
            ..ctx
        };
        assert_eq!(evaluate_rules(&store, &quoted_ctx).rules_matched, 1);
    }

    #[test]
    fn command_failure_builtin_rules_never_match() {
        // Legacy stores may still hold rules for builtins/keywords/common
        // utilities (GH-813: echo 1789, cd 1618 hits). They must not match.
        let store = make_store(vec![
            active_rule("command_failure:cd", "no", RuleCategory::Workflow),
            active_rule("command_failure:echo", "no", RuleCategory::Workflow),
            active_rule("command_failure:ls", "no", RuleCategory::Workflow),
            active_rule("command_failure:grep", "no", RuleCategory::Workflow),
        ]);
        let ctx = HookContext {
            hook_event: "PreToolUse".to_string(),
            tool_name: "Bash".to_string(),
            files_touched: vec![],
            cwd: "/project".to_string(),
            command: Some("cd /tmp; echo hi; ls -la | grep foo".to_string()),
        };
        assert_eq!(evaluate_rules(&store, &ctx).rules_matched, 0);
    }

    #[test]
    fn command_failure_exact_command_word_matches_segment() {
        let store = make_store(vec![active_rule(
            "command_failure:git",
            "Check git config",
            RuleCategory::Workflow,
        )]);
        let ctx = HookContext {
            hook_event: "PreToolUse".to_string(),
            tool_name: "Bash".to_string(),
            files_touched: vec![],
            cwd: "/project".to_string(),
            command: Some("cd /tmp; echo hi; git status".to_string()),
        };
        let result = evaluate_rules(&store, &ctx);
        assert_eq!(result.rules_matched, 1);
        assert_eq!(result.matched_rule_ids[0], "rule_test_command_failure_git");
    }

    #[test]
    fn record_matched_shows_does_not_reset_ttl_or_status() {
        let mut store = make_store(vec![active_rule(
            "command_failure:python",
            "Verify python is available",
            RuleCategory::Workflow,
        )]);
        let last_hit = store.rules[0].last_hit.clone();
        let hits = store.rules[0].hits;
        let id = store.rules[0].id.clone();

        record_matched_shows(&mut store, &["rule_missing".to_string(), id]);

        let rule = &store.rules[0];
        assert_eq!(rule.shows, 1);
        assert_eq!(rule.hits, hits);
        assert_eq!(rule.last_hit, last_hit);
        assert_eq!(rule.status, RuleStatus::Active);
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
