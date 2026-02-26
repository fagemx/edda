//! Review bundle types for compact decision cards.
//!
//! A review bundle aggregates change summary, test results, and risk assessment
//! into a single decision card for rapid approval/rejection.

use serde::{Deserialize, Serialize};

/// Risk level for a review bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

/// A single file change entry from git diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub added: u32,
    pub deleted: u32,
}

/// Summary of code changes (from git diff --numstat).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeSummary {
    pub files: Vec<FileChange>,
    pub total_added: u32,
    pub total_deleted: u32,
    pub diff_ref: String,
}

/// Parsed test execution results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResults {
    pub passed: u32,
    pub failed: u32,
    pub ignored: u32,
    pub total: u32,
    pub failures: Vec<String>,
    pub command: String,
}

/// A single risk factor contributing to the overall assessment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskFactor {
    pub signal: String,
    pub level: RiskLevel,
    pub detail: String,
}

/// Aggregated risk assessment from multiple signals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskAssessment {
    pub level: RiskLevel,
    pub factors: Vec<RiskFactor>,
}

/// Suggested action based on risk and test results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestedAction {
    Approve,
    Review,
    RequestChanges,
    Reject,
}

/// A complete review bundle aggregating all assessment data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewBundle {
    pub bundle_id: String,
    pub change_summary: ChangeSummary,
    pub test_results: TestResults,
    pub risk_assessment: RiskAssessment,
    pub suggested_action: SuggestedAction,
    pub suggested_reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_level_ordering() {
        assert!(RiskLevel::Low < RiskLevel::Medium);
        assert!(RiskLevel::Medium < RiskLevel::High);
        assert!(RiskLevel::High < RiskLevel::Critical);
    }

    #[test]
    fn risk_level_json_roundtrip() {
        let level = RiskLevel::High;
        let json = serde_json::to_string(&level).unwrap();
        assert_eq!(json, "\"high\"");
        let back: RiskLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(back, level);
    }

    #[test]
    fn suggested_action_json_roundtrip() {
        let action = SuggestedAction::RequestChanges;
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(json, "\"request_changes\"");
        let back: SuggestedAction = serde_json::from_str(&json).unwrap();
        assert_eq!(back, action);
    }

    #[test]
    fn review_bundle_json_roundtrip() {
        let bundle = ReviewBundle {
            bundle_id: "bun_test123".into(),
            change_summary: ChangeSummary {
                files: vec![FileChange {
                    path: "src/main.rs".into(),
                    added: 10,
                    deleted: 3,
                }],
                total_added: 10,
                total_deleted: 3,
                diff_ref: "HEAD~1".into(),
            },
            test_results: TestResults {
                passed: 50,
                failed: 0,
                ignored: 2,
                total: 52,
                failures: vec![],
                command: "cargo test".into(),
            },
            risk_assessment: RiskAssessment {
                level: RiskLevel::Low,
                factors: vec![],
            },
            suggested_action: SuggestedAction::Approve,
            suggested_reason: "All tests pass, low risk".into(),
        };

        let json = serde_json::to_value(&bundle).unwrap();
        let back: ReviewBundle = serde_json::from_value(json).unwrap();
        assert_eq!(back.bundle_id, "bun_test123");
        assert_eq!(back.change_summary.total_added, 10);
        assert_eq!(back.test_results.passed, 50);
        assert_eq!(back.risk_assessment.level, RiskLevel::Low);
        assert_eq!(back.suggested_action, SuggestedAction::Approve);
    }
}
