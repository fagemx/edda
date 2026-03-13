//! Generic threshold-based controls suggestion engine.
//!
//! Evaluates a set of [`ThresholdRule`]s against a [`QualityReport`] and
//! produces [`ControlsSuggestion`]s when metrics breach their thresholds.
//! This module is pure computation (no I/O, no Karvi dependency).

use serde::{Deserialize, Serialize};

use crate::quality::QualityReport;

// ── Types ──

/// Which metric a threshold rule evaluates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    SuccessRate,
    AvgCostUsd,
    AvgLatencyMs,
    TotalCostUsd,
}

/// Comparison operator for threshold evaluation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ThresholdOp {
    Lt,
    Gt,
    Lte,
    Gte,
}

/// A threshold rule: when a metric breaches the threshold, suggest an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThresholdRule {
    pub name: String,
    pub metric: MetricKind,
    pub operator: ThresholdOp,
    pub threshold: f64,
    pub action: String,
    pub reason_template: String,
}

/// A suggestion produced by evaluating a rule against a quality report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlsSuggestion {
    pub rule_name: String,
    pub metric_name: String,
    pub current_value: f64,
    pub threshold: f64,
    pub action: String,
    pub reason: String,
}

// ── Evaluation ──

/// Minimum number of total steps required before rules fire.
/// Prevents knee-jerk reactions on insufficient data.
const DEFAULT_MIN_SAMPLES: u64 = 10;

/// Evaluate threshold rules against a quality report.
///
/// Returns suggestions for every rule whose metric breaches its threshold.
/// If the report has fewer than `min_samples` total steps, returns empty.
pub fn evaluate_controls_rules(
    rules: &[ThresholdRule],
    report: &QualityReport,
    min_samples: Option<u64>,
) -> Vec<ControlsSuggestion> {
    let min = min_samples.unwrap_or(DEFAULT_MIN_SAMPLES);
    if report.total_steps < min {
        return Vec::new();
    }

    let mut suggestions = Vec::new();

    for rule in rules {
        let value = extract_metric(report, &rule.metric);
        if breaches(value, &rule.operator, rule.threshold) {
            let reason = rule
                .reason_template
                .replace("{value}", &format_metric_value(value, &rule.metric))
                .replace(
                    "{threshold}",
                    &format_metric_value(rule.threshold, &rule.metric),
                );
            suggestions.push(ControlsSuggestion {
                rule_name: rule.name.clone(),
                metric_name: format!("{:?}", rule.metric),
                current_value: value,
                threshold: rule.threshold,
                action: rule.action.clone(),
                reason,
            });
        }
    }

    suggestions
}

/// Extract the overall metric value from a quality report.
fn extract_metric(report: &QualityReport, kind: &MetricKind) -> f64 {
    match kind {
        MetricKind::SuccessRate => report.overall_success_rate,
        MetricKind::TotalCostUsd => report.total_cost_usd,
        MetricKind::AvgCostUsd => {
            if report.total_steps > 0 {
                report.total_cost_usd / report.total_steps as f64
            } else {
                0.0
            }
        }
        MetricKind::AvgLatencyMs => {
            // Average across all models, weighted by step count.
            let total_latency: f64 = report
                .models
                .iter()
                .map(|m| m.avg_latency_ms * m.total_steps as f64)
                .sum();
            if report.total_steps > 0 {
                total_latency / report.total_steps as f64
            } else {
                0.0
            }
        }
    }
}

/// Check whether `value` breaches the threshold according to `op`.
fn breaches(value: f64, op: &ThresholdOp, threshold: f64) -> bool {
    match op {
        ThresholdOp::Lt => value < threshold,
        ThresholdOp::Gt => value > threshold,
        ThresholdOp::Lte => value <= threshold,
        ThresholdOp::Gte => value >= threshold,
    }
}

/// Format a metric value for display in reason strings.
fn format_metric_value(value: f64, kind: &MetricKind) -> String {
    match kind {
        MetricKind::SuccessRate => format!("{:.0}%", value * 100.0),
        MetricKind::AvgCostUsd | MetricKind::TotalCostUsd => format!("${:.2}", value),
        MetricKind::AvgLatencyMs => format!("{:.0}ms", value),
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::{ModelQuality, QualityReport};

    fn make_report(success_rate: f64, total_steps: u64, cost: f64, latency: f64) -> QualityReport {
        QualityReport {
            models: vec![ModelQuality {
                model: "test-model".to_string(),
                runtime: "test-runtime".to_string(),
                total_steps,
                success_count: (total_steps as f64 * success_rate) as u64,
                failed_count: total_steps - (total_steps as f64 * success_rate) as u64,
                cancelled_count: 0,
                success_rate,
                avg_cost_usd: if total_steps > 0 {
                    cost / total_steps as f64
                } else {
                    0.0
                },
                avg_latency_ms: latency,
                total_cost_usd: cost,
                total_tokens_in: 0,
                total_tokens_out: 0,
            }],
            total_steps,
            overall_success_rate: success_rate,
            total_cost_usd: cost,
        }
    }

    fn sample_rules() -> Vec<ThresholdRule> {
        vec![
            ThresholdRule {
                name: "low-success".to_string(),
                metric: MetricKind::SuccessRate,
                operator: ThresholdOp::Lt,
                threshold: 0.60,
                action: "disable_auto_dispatch".to_string(),
                reason_template: "Success rate {value} below {threshold} threshold".to_string(),
            },
            ThresholdRule {
                name: "high-cost".to_string(),
                metric: MetricKind::AvgCostUsd,
                operator: ThresholdOp::Gt,
                threshold: 0.50,
                action: "reduce_concurrency".to_string(),
                reason_template: "Average cost {value} exceeds {threshold}".to_string(),
            },
            ThresholdRule {
                name: "high-latency".to_string(),
                metric: MetricKind::AvgLatencyMs,
                operator: ThresholdOp::Gt,
                threshold: 30000.0,
                action: "flag_slow_model".to_string(),
                reason_template: "Average latency {value} exceeds {threshold}".to_string(),
            },
        ]
    }

    #[test]
    fn empty_report_no_suggestions() {
        let report = make_report(0.0, 0, 0.0, 0.0);
        let suggestions = evaluate_controls_rules(&sample_rules(), &report, None);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn min_samples_guard_skips_low_count() {
        let report = make_report(0.30, 5, 1.0, 1000.0);
        let suggestions = evaluate_controls_rules(&sample_rules(), &report, None);
        assert!(suggestions.is_empty(), "Should skip with only 5 samples");
    }

    #[test]
    fn min_samples_custom_threshold() {
        let report = make_report(0.30, 5, 1.0, 1000.0);
        let suggestions = evaluate_controls_rules(&sample_rules(), &report, Some(3));
        assert!(
            !suggestions.is_empty(),
            "Should fire with custom min_samples=3"
        );
    }

    #[test]
    fn success_rate_below_threshold_triggers() {
        let report = make_report(0.40, 20, 2.0, 1000.0);
        let suggestions = evaluate_controls_rules(&sample_rules(), &report, None);
        let names: Vec<&str> = suggestions.iter().map(|s| s.rule_name.as_str()).collect();
        assert!(names.contains(&"low-success"));
        let s = suggestions
            .iter()
            .find(|s| s.rule_name == "low-success")
            .unwrap();
        assert_eq!(s.action, "disable_auto_dispatch");
        assert!((s.current_value - 0.40).abs() < 1e-9);
    }

    #[test]
    fn success_rate_above_threshold_no_trigger() {
        let report = make_report(0.80, 20, 2.0, 1000.0);
        let suggestions = evaluate_controls_rules(&sample_rules(), &report, None);
        let names: Vec<&str> = suggestions.iter().map(|s| s.rule_name.as_str()).collect();
        assert!(!names.contains(&"low-success"));
    }

    #[test]
    fn high_cost_triggers() {
        // 20 steps, total cost $15 -> avg $0.75 > $0.50 threshold
        let report = make_report(0.80, 20, 15.0, 1000.0);
        let suggestions = evaluate_controls_rules(&sample_rules(), &report, None);
        let names: Vec<&str> = suggestions.iter().map(|s| s.rule_name.as_str()).collect();
        assert!(names.contains(&"high-cost"));
    }

    #[test]
    fn high_latency_triggers() {
        let report = make_report(0.80, 20, 2.0, 35000.0);
        let suggestions = evaluate_controls_rules(&sample_rules(), &report, None);
        let names: Vec<&str> = suggestions.iter().map(|s| s.rule_name.as_str()).collect();
        assert!(names.contains(&"high-latency"));
    }

    #[test]
    fn multiple_rules_can_trigger() {
        // Low success + high cost + high latency
        let report = make_report(0.30, 20, 15.0, 35000.0);
        let suggestions = evaluate_controls_rules(&sample_rules(), &report, None);
        assert_eq!(suggestions.len(), 3);
    }

    #[test]
    fn no_rules_no_suggestions() {
        let report = make_report(0.30, 20, 15.0, 35000.0);
        let suggestions = evaluate_controls_rules(&[], &report, None);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn reason_template_formatting() {
        let report = make_report(0.40, 20, 2.0, 1000.0);
        let suggestions = evaluate_controls_rules(&sample_rules(), &report, None);
        let s = suggestions
            .iter()
            .find(|s| s.rule_name == "low-success")
            .unwrap();
        assert!(
            s.reason.contains("40%"),
            "Reason should contain formatted percentage: {}",
            s.reason
        );
        assert!(
            s.reason.contains("60%"),
            "Reason should contain threshold: {}",
            s.reason
        );
    }

    #[test]
    fn lte_and_gte_operators() {
        let rules = vec![
            ThresholdRule {
                name: "exact-lte".to_string(),
                metric: MetricKind::SuccessRate,
                operator: ThresholdOp::Lte,
                threshold: 0.50,
                action: "test".to_string(),
                reason_template: "test".to_string(),
            },
            ThresholdRule {
                name: "exact-gte".to_string(),
                metric: MetricKind::TotalCostUsd,
                operator: ThresholdOp::Gte,
                threshold: 5.0,
                action: "test".to_string(),
                reason_template: "test".to_string(),
            },
        ];

        // success_rate exactly 0.50 should trigger Lte 0.50
        // total_cost exactly 5.0 should trigger Gte 5.0
        let report = make_report(0.50, 20, 5.0, 1000.0);
        let suggestions = evaluate_controls_rules(&rules, &report, None);
        assert_eq!(suggestions.len(), 2);
    }
}
