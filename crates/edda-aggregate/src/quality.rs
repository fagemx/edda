//! Model/runtime quality aggregation from Karvi execution events.
//!
//! Groups `execution_event` entries by `(model, runtime)` and computes
//! success rate, average cost, average latency, and token totals.

use std::collections::HashMap;

use edda_core::Event;
use serde::{Deserialize, Serialize};

use crate::aggregate::DateRange;

/// Per-(model, runtime) quality summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelQuality {
    pub model: String,
    pub runtime: String,
    pub total_steps: u64,
    pub success_count: u64,
    pub failed_count: u64,
    pub cancelled_count: u64,
    pub success_rate: f64,
    pub avg_cost_usd: f64,
    pub avg_latency_ms: f64,
    pub total_cost_usd: f64,
    pub total_tokens_in: u64,
    pub total_tokens_out: u64,
}

/// Full quality aggregation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityReport {
    pub models: Vec<ModelQuality>,
    pub total_steps: u64,
    pub overall_success_rate: f64,
    pub total_cost_usd: f64,
}

/// Compute quality metrics from a slice of events (typically `execution_event` type).
///
/// Events outside the given `DateRange` are excluded.
pub fn model_quality_from_events(events: &[Event], range: &DateRange) -> QualityReport {
    // Accumulator per (model, runtime) key.
    struct Accum {
        success: u64,
        failed: u64,
        cancelled: u64,
        cost_sum: f64,
        latency_sum: f64,
        tokens_in: u64,
        tokens_out: u64,
    }

    let mut groups: HashMap<(String, String), Accum> = HashMap::new();

    for event in events {
        if !range.matches(&event.ts) {
            continue;
        }

        let model = event
            .payload
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let runtime = event
            .payload
            .get("runtime")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let status = event
            .payload
            .get("result")
            .and_then(|r| r.get("status"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let cost = event
            .payload
            .get("usage")
            .and_then(|u| u.get("cost_usd"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let latency = event
            .payload
            .get("usage")
            .and_then(|u| u.get("latency_ms"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let tok_in = event
            .payload
            .get("usage")
            .and_then(|u| u.get("token_in"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let tok_out = event
            .payload
            .get("usage")
            .and_then(|u| u.get("token_out"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let acc = groups.entry((model, runtime)).or_insert(Accum {
            success: 0,
            failed: 0,
            cancelled: 0,
            cost_sum: 0.0,
            latency_sum: 0.0,
            tokens_in: 0,
            tokens_out: 0,
        });

        match status {
            "success" => acc.success += 1,
            "failed" => acc.failed += 1,
            "cancelled" => acc.cancelled += 1,
            _ => acc.failed += 1, // unknown status counted as failed
        }

        acc.cost_sum += cost;
        acc.latency_sum += latency;
        acc.tokens_in += tok_in;
        acc.tokens_out += tok_out;
    }

    let mut models: Vec<ModelQuality> = groups
        .into_iter()
        .map(|((model, runtime), acc)| {
            let total = acc.success + acc.failed + acc.cancelled;
            ModelQuality {
                model,
                runtime,
                total_steps: total,
                success_count: acc.success,
                failed_count: acc.failed,
                cancelled_count: acc.cancelled,
                success_rate: if total > 0 {
                    acc.success as f64 / total as f64
                } else {
                    0.0
                },
                avg_cost_usd: if total > 0 {
                    acc.cost_sum / total as f64
                } else {
                    0.0
                },
                avg_latency_ms: if total > 0 {
                    acc.latency_sum / total as f64
                } else {
                    0.0
                },
                total_cost_usd: acc.cost_sum,
                total_tokens_in: acc.tokens_in,
                total_tokens_out: acc.tokens_out,
            }
        })
        .collect();

    // Sort by model then runtime for deterministic output.
    models.sort_by(|a, b| (&a.model, &a.runtime).cmp(&(&b.model, &b.runtime)));

    let total_steps: u64 = models.iter().map(|m| m.total_steps).sum();
    let total_success: u64 = models.iter().map(|m| m.success_count).sum();
    let total_cost: f64 = models.iter().map(|m| m.total_cost_usd).sum();

    QualityReport {
        models,
        total_steps,
        overall_success_rate: if total_steps > 0 {
            total_success as f64 / total_steps as f64
        } else {
            0.0
        },
        total_cost_usd: total_cost,
    }
}

/// Aggregate model quality across all registered projects.
#[cfg(test)]
mod tests {
    use super::*;
    use edda_core::event::{new_execution_event, new_note_event};

    fn make_exec_event(
        model: &str,
        runtime: &str,
        status: &str,
        cost: f64,
        latency: f64,
        ts: &str,
    ) -> Event {
        let payload = serde_json::json!({
            "runtime": runtime,
            "model": model,
            "usage": { "token_in": 100, "token_out": 50, "cost_usd": cost, "latency_ms": latency },
            "result": { "status": status },
            "event_type": "step_completed",
        });
        new_execution_event(
            "main",
            None,
            &format!("evt_{}", rand_id()),
            ts,
            payload,
            None,
        )
        .unwrap()
    }

    fn rand_id() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static CTR: AtomicU64 = AtomicU64::new(0);
        format!("{}", CTR.fetch_add(1, Ordering::SeqCst))
    }

    #[test]
    fn model_quality_empty_events() {
        let report = model_quality_from_events(&[], &DateRange::default());
        assert_eq!(report.total_steps, 0);
        assert_eq!(report.overall_success_rate, 0.0);
        assert_eq!(report.total_cost_usd, 0.0);
        assert!(report.models.is_empty());
    }

    #[test]
    fn model_quality_single_model() {
        let events = vec![
            make_exec_event(
                "claude-3-opus",
                "claude",
                "success",
                0.01,
                500.0,
                "2026-03-11T00:00:00Z",
            ),
            make_exec_event(
                "claude-3-opus",
                "claude",
                "success",
                0.02,
                600.0,
                "2026-03-11T01:00:00Z",
            ),
            make_exec_event(
                "claude-3-opus",
                "claude",
                "failed",
                0.005,
                300.0,
                "2026-03-11T02:00:00Z",
            ),
        ];

        let report = model_quality_from_events(&events, &DateRange::default());
        assert_eq!(report.total_steps, 3);
        assert_eq!(report.models.len(), 1);

        let m = &report.models[0];
        assert_eq!(m.model, "claude-3-opus");
        assert_eq!(m.runtime, "claude");
        assert_eq!(m.success_count, 2);
        assert_eq!(m.failed_count, 1);
        assert!((m.success_rate - 2.0 / 3.0).abs() < 1e-9);
        assert!((m.total_cost_usd - 0.035).abs() < 1e-9);
        assert!((m.avg_latency_ms - (500.0 + 600.0 + 300.0) / 3.0).abs() < 1e-9);
    }

    #[test]
    fn model_quality_multiple_models() {
        let events = vec![
            make_exec_event(
                "claude-3-opus",
                "claude",
                "success",
                0.01,
                500.0,
                "2026-03-11T00:00:00Z",
            ),
            make_exec_event(
                "gpt-4",
                "codex",
                "failed",
                0.02,
                800.0,
                "2026-03-11T01:00:00Z",
            ),
        ];

        let report = model_quality_from_events(&events, &DateRange::default());
        assert_eq!(report.total_steps, 2);
        assert_eq!(report.models.len(), 2);
        assert!((report.overall_success_rate - 0.5).abs() < 1e-9);
    }

    #[test]
    fn model_quality_missing_fields() {
        // Event with no model/runtime in payload
        let payload = serde_json::json!({
            "usage": { "cost_usd": 0.01, "latency_ms": 100 },
            "result": { "status": "success" },
        });
        let event = new_execution_event(
            "main",
            None,
            "evt_missing",
            "2026-03-11T00:00:00Z",
            payload,
            None,
        )
        .unwrap();

        let report = model_quality_from_events(&[event], &DateRange::default());
        assert_eq!(report.models.len(), 1);
        assert_eq!(report.models[0].model, "unknown");
        assert_eq!(report.models[0].runtime, "unknown");
    }

    #[test]
    fn model_quality_date_range_filter() {
        let events = vec![
            make_exec_event("m1", "r1", "success", 0.01, 100.0, "2026-03-10T00:00:00Z"),
            make_exec_event("m1", "r1", "success", 0.01, 100.0, "2026-03-15T00:00:00Z"),
            make_exec_event("m1", "r1", "success", 0.01, 100.0, "2026-03-20T00:00:00Z"),
        ];

        let range = DateRange {
            after: Some("2026-03-12".to_string()),
            before: Some("2026-03-18".to_string()),
        };

        let report = model_quality_from_events(&events, &range);
        assert_eq!(report.total_steps, 1); // only the 03-15 event
    }

    #[test]
    fn model_quality_cancelled_status() {
        let events = vec![make_exec_event(
            "m1",
            "r1",
            "cancelled",
            0.0,
            50.0,
            "2026-03-11T00:00:00Z",
        )];

        let report = model_quality_from_events(&events, &DateRange::default());
        assert_eq!(report.models[0].cancelled_count, 1);
        assert_eq!(report.models[0].success_rate, 0.0);
    }

    #[test]
    fn model_quality_note_events_ignored_by_type_convention() {
        // This tests that non-execution events (which should not be passed in)
        // still produce sensible results if they happen to lack expected fields.
        let note = new_note_event("main", None, "user", "just a note", &[]).unwrap();
        let report = model_quality_from_events(&[note], &DateRange::default());
        // The note event has no result/usage, so it gets grouped under unknown
        assert_eq!(report.total_steps, 1);
        assert_eq!(report.models[0].model, "unknown");
    }
}
