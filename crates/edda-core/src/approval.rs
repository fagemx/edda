//! Approval policy engine — determines what actions require human approval.
//!
//! Separate from the RBAC policy in `policy.rs`. This module answers
//! "does action Y in context Z need human approval?" rather than
//! "can actor X do action Y?".

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::agent_phase::AgentPhaseState;
use crate::bundle::{ReviewBundle, RiskLevel};

// ── Core types ──

/// Top-level approval policy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalPolicy {
    /// Human-readable name for this policy.
    pub name: String,
    /// Per-step overrides (e.g. "plan_approval" -> Always).
    #[serde(default)]
    pub step_policies: HashMap<String, StepPolicy>,
    /// Ordered condition rules; first match wins.
    #[serde(default)]
    pub rules: Vec<ApprovalRule>,
    /// Fallback action when no rule matches.
    #[serde(default)]
    pub default_action: ApprovalAction,
}

/// How a specific pipeline step is handled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepPolicy {
    /// Always require human approval for this step.
    Always,
    /// Automatically approve this step.
    Auto,
    /// Evaluate condition rules to decide.
    Conditional,
}

/// A named condition rule with an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRule {
    pub name: String,
    pub condition: ApprovalCondition,
    pub action: ApprovalAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub notify: bool,
}

/// What action to take when a rule matches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalAction {
    /// Pause and wait for human approval.
    #[default]
    RequireApproval,
    /// Automatically approve.
    AutoApprove,
    /// Automatically approve but send a notification.
    AutoApproveWithNotify,
}

/// Flat condition struct — all specified fields must be true (AND semantics).
/// Unspecified fields (None) are wildcards.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApprovalCondition {
    /// Risk level must be >= this threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_gte: Option<RiskLevel>,
    /// Risk level must equal this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_eq: Option<RiskLevel>,
    /// Number of changed files must be > N.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_changed_gt: Option<u32>,
    /// Number of changed files must be <= N.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_changed_lte: Option<u32>,
    /// Glob pattern — at least one changed file path must match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_matches: Option<String>,
    /// Whether all tests must have passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests_all_passed: Option<bool>,
    /// Whether off-limits files are touched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub touches_off_limits: Option<bool>,
    /// Current agent phase must match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// Time range "HH:MM-HH:MM" — matches when current time is OUTSIDE this range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_outside: Option<String>,
    /// Consecutive failure count must be >= N.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consecutive_failures_gte: Option<u32>,
}

/// Result of evaluating approval policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalDecision {
    /// The action to take.
    pub action: ApprovalAction,
    /// Which rule matched (None if step-level or default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_rule: Option<String>,
    /// Human-readable explanation.
    pub reason: String,
    /// Whether the user can override this decision.
    pub overridable: bool,
}

/// Runtime context for evaluating approval conditions.
pub struct EvalContext<'a> {
    pub bundle: &'a ReviewBundle,
    pub phase: &'a AgentPhaseState,
    pub off_limits_touched: bool,
    pub consecutive_failures: u32,
    pub current_time: Option<time::OffsetDateTime>,
}

// ── Built-in defaults ──

impl ApprovalPolicy {
    /// Returns the built-in default policy (zero-config).
    ///
    /// - `plan_approval` and `pr_merge` always require approval
    /// - High risk -> require approval
    /// - 20+ files changed -> require approval
    /// - Off-limits touched -> require approval
    /// - Low risk + all tests pass + <= 5 files -> auto approve
    /// - Default: require approval (fail-safe)
    pub fn default_policy() -> Self {
        let mut step_policies = HashMap::new();
        step_policies.insert("plan_approval".to_string(), StepPolicy::Always);
        step_policies.insert("pr_merge".to_string(), StepPolicy::Always);

        let rules = vec![
            ApprovalRule {
                name: "high-risk".to_string(),
                condition: ApprovalCondition {
                    risk_gte: Some(RiskLevel::High),
                    ..Default::default()
                },
                action: ApprovalAction::RequireApproval,
                reason: Some("high or critical risk level".to_string()),
                notify: false,
            },
            ApprovalRule {
                name: "large-change".to_string(),
                condition: ApprovalCondition {
                    files_changed_gt: Some(20),
                    ..Default::default()
                },
                action: ApprovalAction::RequireApproval,
                reason: Some("more than 20 files changed".to_string()),
                notify: false,
            },
            ApprovalRule {
                name: "off-limits".to_string(),
                condition: ApprovalCondition {
                    touches_off_limits: Some(true),
                    ..Default::default()
                },
                action: ApprovalAction::RequireApproval,
                reason: Some("off-limits files touched".to_string()),
                notify: false,
            },
            ApprovalRule {
                name: "safe-auto".to_string(),
                condition: ApprovalCondition {
                    risk_eq: Some(RiskLevel::Low),
                    tests_all_passed: Some(true),
                    files_changed_lte: Some(5),
                    ..Default::default()
                },
                action: ApprovalAction::AutoApprove,
                reason: Some("low risk, all tests pass, small change".to_string()),
                notify: false,
            },
        ];

        Self {
            name: "default".to_string(),
            step_policies,
            rules,
            default_action: ApprovalAction::RequireApproval,
        }
    }
}

// ── Condition evaluation ──

impl ApprovalCondition {
    /// Check if this condition matches the given evaluation context.
    /// All specified fields must be true (AND semantics).
    pub fn matches(&self, ctx: &EvalContext<'_>) -> bool {
        if let Some(threshold) = &self.risk_gte {
            if ctx.bundle.risk_assessment.level < *threshold {
                return false;
            }
        }

        if let Some(expected) = &self.risk_eq {
            if ctx.bundle.risk_assessment.level != *expected {
                return false;
            }
        }

        let file_count = ctx.bundle.change_summary.files.len() as u32;

        if let Some(threshold) = self.files_changed_gt {
            if file_count <= threshold {
                return false;
            }
        }

        if let Some(threshold) = self.files_changed_lte {
            if file_count > threshold {
                return false;
            }
        }

        if let Some(pattern) = &self.path_matches {
            if let Ok(glob) = globset::Glob::new(pattern) {
                let matcher = glob.compile_matcher();
                let any_match = ctx
                    .bundle
                    .change_summary
                    .files
                    .iter()
                    .any(|f| matcher.is_match(&f.path));
                if !any_match {
                    return false;
                }
            } else {
                // Invalid glob pattern — treat as non-matching
                return false;
            }
        }

        if let Some(expected) = self.tests_all_passed {
            let all_passed = ctx.bundle.test_results.failed == 0;
            if all_passed != expected {
                return false;
            }
        }

        if let Some(expected) = self.touches_off_limits {
            if ctx.off_limits_touched != expected {
                return false;
            }
        }

        if let Some(expected_phase) = &self.phase {
            if ctx.phase.phase.to_string() != *expected_phase {
                return false;
            }
        }

        if let Some(range_str) = &self.time_outside {
            if let Some(now) = &ctx.current_time {
                if let Some((start, end)) = parse_time_range(range_str) {
                    let current_minutes = now.hour() as u32 * 60 + now.minute() as u32;
                    // "outside" means NOT within [start, end]
                    let within = if start <= end {
                        current_minutes >= start && current_minutes < end
                    } else {
                        // Wraps midnight (e.g. "22:00-06:00")
                        current_minutes >= start || current_minutes < end
                    };
                    if within {
                        return false; // Inside the range, so "time_outside" is false
                    }
                }
                // Malformed range string — treat as non-matching
            }
            // No current_time provided — skip this condition
        }

        if let Some(threshold) = self.consecutive_failures_gte {
            if ctx.consecutive_failures < threshold {
                return false;
            }
        }

        true
    }
}

/// Parse "HH:MM-HH:MM" into (start_minutes, end_minutes).
fn parse_time_range(s: &str) -> Option<(u32, u32)> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 2 {
        return None;
    }
    let start = parse_hhmm(parts[0])?;
    let end = parse_hhmm(parts[1])?;
    Some((start, end))
}

fn parse_hhmm(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let h: u32 = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    if h >= 24 || m >= 60 {
        return None;
    }
    Some(h * 60 + m)
}

// ── Policy evaluation ──

impl ApprovalPolicy {
    /// Evaluate whether a pipeline step needs approval.
    pub fn evaluate(&self, step: &str, ctx: &EvalContext<'_>) -> ApprovalDecision {
        // 1. Check step-level policy
        if let Some(step_policy) = self.step_policies.get(step) {
            match step_policy {
                StepPolicy::Always => {
                    return ApprovalDecision {
                        action: ApprovalAction::RequireApproval,
                        matched_rule: None,
                        reason: format!("step '{}' always requires approval", step),
                        overridable: false,
                    };
                }
                StepPolicy::Auto => {
                    return ApprovalDecision {
                        action: ApprovalAction::AutoApprove,
                        matched_rule: None,
                        reason: format!("step '{}' is auto-approved", step),
                        overridable: true,
                    };
                }
                StepPolicy::Conditional => {
                    // Fall through to rule evaluation
                }
            }
        }

        // 2. Iterate rules (first match wins)
        for rule in &self.rules {
            if rule.condition.matches(ctx) {
                let reason = rule
                    .reason
                    .clone()
                    .unwrap_or_else(|| format!("matched rule '{}'", rule.name));
                let action = if rule.notify && rule.action == ApprovalAction::AutoApprove {
                    ApprovalAction::AutoApproveWithNotify
                } else {
                    rule.action.clone()
                };
                return ApprovalDecision {
                    action,
                    matched_rule: Some(rule.name.clone()),
                    reason,
                    overridable: true,
                };
            }
        }

        // 3. No match — fail-safe
        ApprovalDecision {
            action: self.default_action.clone(),
            matched_rule: None,
            reason: "no rule matched; fail-safe".to_string(),
            overridable: true,
        }
    }
}

// ── Config loading ──

/// Load approval policy from `.edda/approval-policy.yaml`.
/// Falls back to `default_policy()` if the file does not exist.
pub fn load_approval_policy(edda_dir: &Path) -> anyhow::Result<ApprovalPolicy> {
    let path = edda_dir.join("approval-policy.yaml");
    if !path.exists() {
        return Ok(ApprovalPolicy::default_policy());
    }
    let content = std::fs::read(&path)?;
    let policy: ApprovalPolicy = serde_yaml::from_slice(&content)?;
    Ok(policy)
}

/// Generate a template approval-policy.yaml file.
pub fn generate_template() -> String {
    let policy = ApprovalPolicy::default_policy();
    serde_yaml::to_string(&policy).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_phase::AgentPhase;
    use crate::bundle::*;

    fn sample_bundle(risk: RiskLevel, file_count: usize, failures: u32) -> ReviewBundle {
        let files: Vec<FileChange> = (0..file_count)
            .map(|i| FileChange {
                path: format!("src/file_{}.rs", i),
                added: 10,
                deleted: 2,
            })
            .collect();

        ReviewBundle {
            bundle_id: "bun_test".to_string(),
            change_summary: ChangeSummary {
                files,
                total_added: 10 * file_count as u32,
                total_deleted: 2 * file_count as u32,
                diff_ref: "HEAD~1".to_string(),
            },
            test_results: TestResults {
                passed: 50,
                failed: failures,
                ignored: 0,
                total: 50 + failures,
                failures: vec![],
                command: "cargo test".to_string(),
            },
            risk_assessment: RiskAssessment {
                level: risk,
                factors: vec![],
            },
            suggested_action: SuggestedAction::Approve,
            suggested_reason: "test".to_string(),
        }
    }

    fn sample_phase() -> AgentPhaseState {
        AgentPhaseState {
            phase: AgentPhase::Implement,
            session_id: "sess-test".to_string(),
            label: None,
            issue: Some(120),
            pr: None,
            branch: None,
            confidence: 0.9,
            detected_at: "2026-03-12T10:00:00Z".to_string(),
            signals: vec![],
        }
    }

    fn make_ctx<'a>(
        bundle: &'a ReviewBundle,
        phase: &'a AgentPhaseState,
        off_limits: bool,
        failures: u32,
    ) -> EvalContext<'a> {
        EvalContext {
            bundle,
            phase,
            off_limits_touched: off_limits,
            consecutive_failures: failures,
            current_time: None,
        }
    }

    // ── Default policy tests ──

    #[test]
    fn default_policy_step_always() {
        let policy = ApprovalPolicy::default_policy();
        let bundle = sample_bundle(RiskLevel::Low, 1, 0);
        let phase = sample_phase();
        let ctx = make_ctx(&bundle, &phase, false, 0);

        let decision = policy.evaluate("plan_approval", &ctx);
        assert_eq!(decision.action, ApprovalAction::RequireApproval);
        assert!(decision.reason.contains("always requires approval"));

        let decision = policy.evaluate("pr_merge", &ctx);
        assert_eq!(decision.action, ApprovalAction::RequireApproval);
    }

    #[test]
    fn default_policy_high_risk() {
        let policy = ApprovalPolicy::default_policy();
        let bundle = sample_bundle(RiskLevel::High, 3, 0);
        let phase = sample_phase();
        let ctx = make_ctx(&bundle, &phase, false, 0);

        let decision = policy.evaluate("implement", &ctx);
        assert_eq!(decision.action, ApprovalAction::RequireApproval);
        assert_eq!(decision.matched_rule.as_deref(), Some("high-risk"));
    }

    #[test]
    fn default_policy_critical_risk_matches_high_gte() {
        let policy = ApprovalPolicy::default_policy();
        let bundle = sample_bundle(RiskLevel::Critical, 3, 0);
        let phase = sample_phase();
        let ctx = make_ctx(&bundle, &phase, false, 0);

        let decision = policy.evaluate("implement", &ctx);
        assert_eq!(decision.action, ApprovalAction::RequireApproval);
        assert_eq!(decision.matched_rule.as_deref(), Some("high-risk"));
    }

    #[test]
    fn default_policy_large_change() {
        let policy = ApprovalPolicy::default_policy();
        let bundle = sample_bundle(RiskLevel::Medium, 25, 0);
        let phase = sample_phase();
        let ctx = make_ctx(&bundle, &phase, false, 0);

        let decision = policy.evaluate("implement", &ctx);
        assert_eq!(decision.action, ApprovalAction::RequireApproval);
        assert_eq!(decision.matched_rule.as_deref(), Some("large-change"));
    }

    #[test]
    fn default_policy_off_limits() {
        let policy = ApprovalPolicy::default_policy();
        let bundle = sample_bundle(RiskLevel::Low, 2, 0);
        let phase = sample_phase();
        let ctx = make_ctx(&bundle, &phase, true, 0);

        let decision = policy.evaluate("implement", &ctx);
        assert_eq!(decision.action, ApprovalAction::RequireApproval);
        assert_eq!(decision.matched_rule.as_deref(), Some("off-limits"));
    }

    #[test]
    fn default_policy_safe_auto() {
        let policy = ApprovalPolicy::default_policy();
        let bundle = sample_bundle(RiskLevel::Low, 3, 0);
        let phase = sample_phase();
        let ctx = make_ctx(&bundle, &phase, false, 0);

        let decision = policy.evaluate("implement", &ctx);
        assert_eq!(decision.action, ApprovalAction::AutoApprove);
        assert_eq!(decision.matched_rule.as_deref(), Some("safe-auto"));
    }

    #[test]
    fn default_policy_medium_risk_no_match_failsafe() {
        let policy = ApprovalPolicy::default_policy();
        // Medium risk, 10 files (not > 20), tests pass, no off-limits
        // Doesn't match high-risk, large-change, off-limits, or safe-auto (risk != low)
        let bundle = sample_bundle(RiskLevel::Medium, 10, 0);
        let phase = sample_phase();
        let ctx = make_ctx(&bundle, &phase, false, 0);

        let decision = policy.evaluate("implement", &ctx);
        assert_eq!(decision.action, ApprovalAction::RequireApproval);
        assert!(decision.matched_rule.is_none());
        assert!(decision.reason.contains("fail-safe"));
    }

    // ── Condition matching tests ──

    #[test]
    fn condition_empty_matches_everything() {
        let cond = ApprovalCondition::default();
        let bundle = sample_bundle(RiskLevel::Medium, 10, 0);
        let phase = sample_phase();
        let ctx = make_ctx(&bundle, &phase, false, 0);
        assert!(cond.matches(&ctx));
    }

    #[test]
    fn condition_risk_gte() {
        let cond = ApprovalCondition {
            risk_gte: Some(RiskLevel::High),
            ..Default::default()
        };
        let bundle_low = sample_bundle(RiskLevel::Low, 1, 0);
        let bundle_high = sample_bundle(RiskLevel::High, 1, 0);
        let phase = sample_phase();

        assert!(!cond.matches(&make_ctx(&bundle_low, &phase, false, 0)));
        assert!(cond.matches(&make_ctx(&bundle_high, &phase, false, 0)));
    }

    #[test]
    fn condition_files_changed_boundaries() {
        let cond = ApprovalCondition {
            files_changed_gt: Some(5),
            ..Default::default()
        };
        let bundle_5 = sample_bundle(RiskLevel::Low, 5, 0);
        let bundle_6 = sample_bundle(RiskLevel::Low, 6, 0);
        let phase = sample_phase();

        assert!(!cond.matches(&make_ctx(&bundle_5, &phase, false, 0)));
        assert!(cond.matches(&make_ctx(&bundle_6, &phase, false, 0)));
    }

    #[test]
    fn condition_files_changed_lte() {
        let cond = ApprovalCondition {
            files_changed_lte: Some(5),
            ..Default::default()
        };
        let bundle_5 = sample_bundle(RiskLevel::Low, 5, 0);
        let bundle_6 = sample_bundle(RiskLevel::Low, 6, 0);
        let phase = sample_phase();

        assert!(cond.matches(&make_ctx(&bundle_5, &phase, false, 0)));
        assert!(!cond.matches(&make_ctx(&bundle_6, &phase, false, 0)));
    }

    #[test]
    fn condition_path_matches() {
        let cond = ApprovalCondition {
            path_matches: Some("src/auth/**".to_string()),
            ..Default::default()
        };

        let mut bundle = sample_bundle(RiskLevel::Low, 1, 0);
        bundle.change_summary.files = vec![FileChange {
            path: "src/auth/login.rs".to_string(),
            added: 5,
            deleted: 1,
        }];
        let phase = sample_phase();
        assert!(cond.matches(&make_ctx(&bundle, &phase, false, 0)));

        let mut bundle_no_match = sample_bundle(RiskLevel::Low, 1, 0);
        bundle_no_match.change_summary.files = vec![FileChange {
            path: "src/db/query.rs".to_string(),
            added: 5,
            deleted: 1,
        }];
        assert!(!cond.matches(&make_ctx(&bundle_no_match, &phase, false, 0)));
    }

    #[test]
    fn condition_tests_all_passed() {
        let cond = ApprovalCondition {
            tests_all_passed: Some(true),
            ..Default::default()
        };
        let bundle_pass = sample_bundle(RiskLevel::Low, 1, 0);
        let bundle_fail = sample_bundle(RiskLevel::Low, 1, 3);
        let phase = sample_phase();

        assert!(cond.matches(&make_ctx(&bundle_pass, &phase, false, 0)));
        assert!(!cond.matches(&make_ctx(&bundle_fail, &phase, false, 0)));
    }

    #[test]
    fn condition_touches_off_limits() {
        let cond = ApprovalCondition {
            touches_off_limits: Some(true),
            ..Default::default()
        };
        let bundle = sample_bundle(RiskLevel::Low, 1, 0);
        let phase = sample_phase();

        assert!(cond.matches(&make_ctx(&bundle, &phase, true, 0)));
        assert!(!cond.matches(&make_ctx(&bundle, &phase, false, 0)));
    }

    #[test]
    fn condition_phase() {
        let cond = ApprovalCondition {
            phase: Some("implement".to_string()),
            ..Default::default()
        };
        let bundle = sample_bundle(RiskLevel::Low, 1, 0);
        let phase_impl = sample_phase();
        let mut phase_review = sample_phase();
        phase_review.phase = AgentPhase::Review;

        assert!(cond.matches(&make_ctx(&bundle, &phase_impl, false, 0)));
        assert!(!cond.matches(&make_ctx(&bundle, &phase_review, false, 0)));
    }

    #[test]
    fn condition_consecutive_failures() {
        let cond = ApprovalCondition {
            consecutive_failures_gte: Some(3),
            ..Default::default()
        };
        let bundle = sample_bundle(RiskLevel::Low, 1, 0);
        let phase = sample_phase();

        assert!(!cond.matches(&make_ctx(&bundle, &phase, false, 2)));
        assert!(cond.matches(&make_ctx(&bundle, &phase, false, 3)));
        assert!(cond.matches(&make_ctx(&bundle, &phase, false, 5)));
    }

    #[test]
    fn condition_and_semantics() {
        // Multiple conditions: risk_eq: Low AND files_changed_lte: 5
        let cond = ApprovalCondition {
            risk_eq: Some(RiskLevel::Low),
            files_changed_lte: Some(5),
            ..Default::default()
        };
        let phase = sample_phase();

        // Both true
        let bundle_ok = sample_bundle(RiskLevel::Low, 3, 0);
        assert!(cond.matches(&make_ctx(&bundle_ok, &phase, false, 0)));

        // Risk wrong
        let bundle_risk = sample_bundle(RiskLevel::Medium, 3, 0);
        assert!(!cond.matches(&make_ctx(&bundle_risk, &phase, false, 0)));

        // Files too many
        let bundle_files = sample_bundle(RiskLevel::Low, 10, 0);
        assert!(!cond.matches(&make_ctx(&bundle_files, &phase, false, 0)));
    }

    // ── Step policy tests ──

    #[test]
    fn step_auto_overrides_rules() {
        let mut policy = ApprovalPolicy::default_policy();
        policy
            .step_policies
            .insert("test".to_string(), StepPolicy::Auto);

        let bundle = sample_bundle(RiskLevel::Critical, 100, 10);
        let phase = sample_phase();
        let ctx = make_ctx(&bundle, &phase, true, 5);

        let decision = policy.evaluate("test", &ctx);
        assert_eq!(decision.action, ApprovalAction::AutoApprove);
        assert!(decision.reason.contains("auto-approved"));
    }

    #[test]
    fn step_conditional_falls_through_to_rules() {
        let mut policy = ApprovalPolicy::default_policy();
        policy
            .step_policies
            .insert("deploy".to_string(), StepPolicy::Conditional);

        let bundle = sample_bundle(RiskLevel::High, 5, 0);
        let phase = sample_phase();
        let ctx = make_ctx(&bundle, &phase, false, 0);

        let decision = policy.evaluate("deploy", &ctx);
        assert_eq!(decision.action, ApprovalAction::RequireApproval);
        assert_eq!(decision.matched_rule.as_deref(), Some("high-risk"));
    }

    #[test]
    fn unknown_step_falls_through_to_rules() {
        let policy = ApprovalPolicy::default_policy();
        let bundle = sample_bundle(RiskLevel::Low, 2, 0);
        let phase = sample_phase();
        let ctx = make_ctx(&bundle, &phase, false, 0);

        let decision = policy.evaluate("custom_step", &ctx);
        // Low risk, 2 files, tests pass -> safe-auto
        assert_eq!(decision.action, ApprovalAction::AutoApprove);
    }

    #[test]
    fn first_match_wins() {
        let policy = ApprovalPolicy {
            name: "test".to_string(),
            step_policies: HashMap::new(),
            rules: vec![
                ApprovalRule {
                    name: "catch-all-approve".to_string(),
                    condition: ApprovalCondition::default(), // matches everything
                    action: ApprovalAction::AutoApprove,
                    reason: Some("catch-all".to_string()),
                    notify: false,
                },
                ApprovalRule {
                    name: "never-reached".to_string(),
                    condition: ApprovalCondition::default(),
                    action: ApprovalAction::RequireApproval,
                    reason: None,
                    notify: false,
                },
            ],
            default_action: ApprovalAction::RequireApproval,
        };

        let bundle = sample_bundle(RiskLevel::Critical, 100, 10);
        let phase = sample_phase();
        let ctx = make_ctx(&bundle, &phase, true, 5);

        let decision = policy.evaluate("anything", &ctx);
        assert_eq!(decision.action, ApprovalAction::AutoApprove);
        assert_eq!(
            decision.matched_rule.as_deref(),
            Some("catch-all-approve")
        );
    }

    #[test]
    fn empty_rules_uses_default_action() {
        let policy = ApprovalPolicy {
            name: "minimal".to_string(),
            step_policies: HashMap::new(),
            rules: vec![],
            default_action: ApprovalAction::AutoApproveWithNotify,
        };

        let bundle = sample_bundle(RiskLevel::Low, 1, 0);
        let phase = sample_phase();
        let ctx = make_ctx(&bundle, &phase, false, 0);

        let decision = policy.evaluate("anything", &ctx);
        assert_eq!(decision.action, ApprovalAction::AutoApproveWithNotify);
        assert!(decision.matched_rule.is_none());
    }

    #[test]
    fn notify_flag_upgrades_auto_approve() {
        let policy = ApprovalPolicy {
            name: "test".to_string(),
            step_policies: HashMap::new(),
            rules: vec![ApprovalRule {
                name: "notify-rule".to_string(),
                condition: ApprovalCondition::default(),
                action: ApprovalAction::AutoApprove,
                reason: Some("auto with notify".to_string()),
                notify: true,
            }],
            default_action: ApprovalAction::RequireApproval,
        };

        let bundle = sample_bundle(RiskLevel::Low, 1, 0);
        let phase = sample_phase();
        let ctx = make_ctx(&bundle, &phase, false, 0);

        let decision = policy.evaluate("step", &ctx);
        assert_eq!(decision.action, ApprovalAction::AutoApproveWithNotify);
    }

    // ── Serialization tests ──

    #[test]
    fn policy_yaml_roundtrip() {
        let policy = ApprovalPolicy::default_policy();
        let yaml = serde_yaml::to_string(&policy).unwrap();
        let parsed: ApprovalPolicy = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.name, "default");
        assert_eq!(parsed.rules.len(), 4);
        assert_eq!(
            parsed.step_policies.get("plan_approval"),
            Some(&StepPolicy::Always)
        );
    }

    #[test]
    fn approval_action_json_roundtrip() {
        for action in [
            ApprovalAction::RequireApproval,
            ApprovalAction::AutoApprove,
            ApprovalAction::AutoApproveWithNotify,
        ] {
            let json = serde_json::to_string(&action).unwrap();
            let back: ApprovalAction = serde_json::from_str(&json).unwrap();
            assert_eq!(back, action);
        }
    }

    #[test]
    fn step_policy_json_roundtrip() {
        for sp in [StepPolicy::Always, StepPolicy::Auto, StepPolicy::Conditional] {
            let json = serde_json::to_string(&sp).unwrap();
            let back: StepPolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(back, sp);
        }
    }

    // ── Config loading tests ──

    #[test]
    fn load_missing_file_returns_default() {
        let dir = std::path::PathBuf::from("/nonexistent/path");
        let policy = load_approval_policy(&dir).unwrap();
        assert_eq!(policy.name, "default");
        assert_eq!(policy.rules.len(), 4);
    }

    #[test]
    fn load_custom_policy_from_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
name: custom-project
step_policies:
  plan_approval: always
  pr_merge: always
  implement: auto
rules:
  - name: auth-module
    condition:
      path_matches: "src/auth/**"
    action: require_approval
    reason: "auth changes need review"
    notify: false
default_action: auto_approve
"#;
        std::fs::write(dir.path().join("approval-policy.yaml"), yaml).unwrap();
        let policy = load_approval_policy(dir.path()).unwrap();
        assert_eq!(policy.name, "custom-project");
        assert_eq!(policy.rules.len(), 1);
        assert_eq!(policy.rules[0].name, "auth-module");
        assert_eq!(policy.default_action, ApprovalAction::AutoApprove);
        assert_eq!(
            policy.step_policies.get("implement"),
            Some(&StepPolicy::Auto)
        );
    }

    // ── Time parsing tests ──

    #[test]
    fn parse_time_range_valid() {
        assert_eq!(parse_time_range("09:00-18:00"), Some((540, 1080)));
        assert_eq!(parse_time_range("22:00-06:00"), Some((1320, 360)));
    }

    #[test]
    fn parse_time_range_invalid() {
        assert_eq!(parse_time_range("invalid"), None);
        assert_eq!(parse_time_range("25:00-18:00"), None);
        assert_eq!(parse_time_range("09:00"), None);
    }

    #[test]
    fn generate_template_is_valid_yaml() {
        let template = generate_template();
        let parsed: ApprovalPolicy = serde_yaml::from_str(&template).unwrap();
        assert_eq!(parsed.name, "default");
    }
}
