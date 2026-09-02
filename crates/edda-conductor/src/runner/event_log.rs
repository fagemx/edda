//! Structured event logging for conductor runs.
//!
//! Writes append-only JSONL to `.edda/conductor/{plan}/events.jsonl`.
//! Independent of edda/edda — works even if edda CLI is not installed.

use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

// ── Event types ──

/// Serde `skip_serializing_if` helper: omit numeric counters at their default.
fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

/// A conductor event. Serialized as tagged JSON (`"type": "plan_start"`, etc.).
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    PlanStart {
        plan_name: String,
        phase_count: usize,
    },
    PhaseStart {
        phase_id: String,
        attempt: u32,
    },
    PhasePassed {
        phase_id: String,
        attempt: u32,
        duration_ms: u64,
        cost_usd: Option<f64>,
    },
    PhaseFailed {
        phase_id: String,
        attempt: u32,
        duration_ms: u64,
        error: String,
        /// GH-540 review round 1: snake_case [`crate::state::machine::ErrorType`]
        /// of the failure ("environmental", "timeout", ...), so generic JSONL
        /// consumers can distinguish a free environmental retry from a charged
        /// product failure. Absent on records written before this field — the
        /// field is additive and old lines parse unchanged.
        #[serde(skip_serializing_if = "Option::is_none")]
        error_type: Option<String>,
        /// GH-540 review round 1: environmental failures already charged to
        /// the free-retry counter when this event was written. Absent when
        /// zero, so pre-GH-540-shaped records and zero-state records emit the
        /// identical object for old consumers.
        #[serde(skip_serializing_if = "is_zero_u32")]
        env_retries: u32,
        /// GH-540 review round 1: whether this attempt consumed a product
        /// attempt (the `max_attempts` ladder). Environmental failures are
        /// never charged — their retries are free.
        attempt_charged: bool,
    },
    PhaseSkipped {
        phase_id: String,
        reason: String,
    },
    /// The verdict gate expired with no verdict (GH-552). Distinct from
    /// `PhaseFailed` so the audit log can tell "the agent could not do the
    /// work" from "the work is done, checked, and waiting on a human who
    /// did not show up". `elapsed_ms` is the real wall time spent in
    /// AWAITING_VERDICT, not the zero a failure shape produced.
    GateTimedOut {
        phase_id: String,
        gate_sha: String,
        elapsed_ms: u64,
    },
    /// A timed-out gate was waived — by the operator (interactive prompt or
    /// `edda conduct skip`) or automatically (`on_gate_timeout: skip`). The
    /// phase keeps its honest `GateTimedOut` status; this records who moved
    /// the plan past the gate and why (GH-552).
    GateWaived {
        phase_id: String,
        reason: String,
        /// True when `on_gate_timeout: skip` waived it without a human.
        #[serde(skip_serializing_if = "std::ops::Not::not", default)]
        auto: bool,
    },
    /// A gated phase passed its checks and entered AWAITING_VERDICT (D4).
    GateEntered {
        phase_id: String,
        /// `<plan-name>/<phase-id>` — the subject an `edda verdict` targets.
        subject: String,
        gate_sha: String,
    },
    /// A verdict for a waiting gate was observed in the ledger (D4).
    VerdictReceived {
        phase_id: String,
        /// "approved" | "rejected"
        decision: String,
        gate_sha: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        comment: Option<String>,
    },
    PlanCompleted {
        phases_passed: usize,
        /// GH-533: `None` until any phase recorded a measured cost — same
        /// pattern as `PhasePassed::cost_usd`. A usage-free backend (codex)
        /// emits `null`, never the unmeasured `0.0` sentinel.
        total_cost_usd: Option<f64>,
    },
    PlanAborted {
        phases_passed: usize,
        phases_pending: usize,
    },
}

/// Wrapper that adds sequence number and timestamp to each event.
#[derive(Debug, Serialize)]
pub struct FullEvent {
    pub seq: u32,
    pub ts: String,
    #[serde(flatten)]
    pub event: Event,
}

// ── EventLogger ──

/// Append-only JSONL event writer.
pub struct EventLogger {
    jsonl_path: PathBuf,
    seq: u32,
    stdout_json: bool,
}

impl EventLogger {
    /// Create a new logger. Path: `{cwd}/.edda/conductor/{plan_name}/events.jsonl`.
    pub fn new(cwd: &Path, plan_name: &str) -> Self {
        let jsonl_path = cwd
            .join(".edda")
            .join("conductor")
            .join(plan_name)
            .join("events.jsonl");
        Self {
            jsonl_path,
            seq: 0,
            stdout_json: false,
        }
    }

    /// Enable tee-ing events to stdout as JSONL (for `--json` mode).
    pub fn with_stdout_json(mut self, enabled: bool) -> Self {
        self.stdout_json = enabled;
        self
    }

    /// Record an event. Best-effort: silently ignores write failures.
    pub fn record(&mut self, event: Event) {
        let full = FullEvent {
            seq: self.seq,
            ts: now_rfc3339(),
            event,
        };
        self.seq += 1;

        if let Ok(line) = serde_json::to_string(&full) {
            let _ = append_line(&self.jsonl_path, &line);
            if self.stdout_json {
                let _ = writeln!(std::io::stdout(), "{line}");
            }
        }
    }
}

/// Append a single line to a file, creating parent dirs if needed.
fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")
}

// ── RunnerStatus ──

/// Lightweight status file for external tools to poll.
#[derive(Debug, Serialize)]
pub struct RunnerStatus {
    pub plan: String,
    pub status: String,
    pub current_phase: Option<String>,
    pub completed: Vec<String>,
    pub failed: Vec<String>,
    /// Phases holding AWAITING_VERDICT (D4) — external actors poll this
    /// to learn the subject + gate_sha they must issue a verdict against.
    pub awaiting_verdict: Vec<String>,
    pub updated_at: String,
}

/// Derive runner status from current PlanState and write to disk.
pub fn write_runner_status(
    cwd: &Path,
    state: &crate::state::machine::PlanState,
    current_phase: Option<&str>,
) {
    use crate::state::machine::PhaseStatus;

    let status = RunnerStatus {
        plan: state.plan_name.clone(),
        status: format!("{:?}", state.plan_status).to_lowercase(),
        current_phase: current_phase.map(String::from),
        completed: state
            .phases
            .iter()
            .filter(|p| p.status == PhaseStatus::Passed)
            .map(|p| p.id.clone())
            .collect(),
        failed: state
            .phases
            .iter()
            .filter(|p| p.status == PhaseStatus::Failed || p.status == PhaseStatus::Stale)
            .map(|p| p.id.clone())
            .collect(),
        awaiting_verdict: state
            .phases
            .iter()
            .filter(|p| p.status == PhaseStatus::AwaitingVerdict)
            .map(|p| p.id.clone())
            .collect(),
        updated_at: now_rfc3339(),
    };

    let path = cwd
        .join(".edda")
        .join("conductor")
        .join(&state.plan_name)
        .join("runner-status.json");

    if let Ok(data) = serde_json::to_string_pretty(&status) {
        let _ = edda_store::write_atomic(&path, data.as_bytes());
    }
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_plan_start_serialization() {
        let event = Event::PlanStart {
            plan_name: "test".into(),
            phase_count: 3,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"plan_start""#));
        assert!(json.contains(r#""plan_name":"test""#));
        assert!(json.contains(r#""phase_count":3"#));
    }

    #[test]
    fn event_plan_completed_unmeasured_cost_is_not_asserted() {
        // GH-533: a usage-free backend (codex) must not emit the sentinel
        // `total_cost_usd: 0.0` as if it were a measured figure.
        let event = Event::PlanCompleted {
            phases_passed: 2,
            total_cost_usd: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains(r#""total_cost_usd":0.0"#));
        assert!(json.contains(r#""total_cost_usd":null"#));
    }

    #[test]
    fn event_plan_completed_measured_cost_round_trips_exactly() {
        let event = Event::PlanCompleted {
            phases_passed: 1,
            total_cost_usd: Some(1.234),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""total_cost_usd":1.234"#));
    }

    #[test]
    fn event_phase_passed_serialization() {
        let event = Event::PhasePassed {
            phase_id: "build".into(),
            attempt: 1,
            duration_ms: 5000,
            cost_usd: Some(0.42),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"phase_passed""#));
        assert!(json.contains(r#""cost_usd":0.42"#));
    }

    #[test]
    fn event_phase_failed_serialization() {
        let event = Event::PhaseFailed {
            phase_id: "build".into(),
            attempt: 1,
            duration_ms: 5000,
            error: "exit 1".into(),
            error_type: Some("environmental".into()),
            env_retries: 2,
            attempt_charged: false,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"phase_failed""#));
        assert!(json.contains(r#""error_type":"environmental""#));
        assert!(json.contains(r#""env_retries":2"#));
        assert!(json.contains(r#""attempt_charged":false"#));
    }

    /// GH-540 review round 1: the retry-accounting fields are additive —
    /// defaulted/absent fields stay out of the JSON so records written before
    /// the fields existed and records with zero counters parse identically
    /// for generic JSONL consumers.
    #[test]
    fn event_phase_failed_additive_fields_stay_absent_when_defaulted() {
        let event = Event::PhaseFailed {
            phase_id: "build".into(),
            attempt: 1,
            duration_ms: 5000,
            error: "boom".into(),
            error_type: None,
            env_retries: 0,
            attempt_charged: true,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("error_type"), "json: {json}");
        assert!(!json.contains("env_retries"), "json: {json}");
        assert!(json.contains(r#""attempt_charged":true"#));
    }

    #[test]
    fn full_event_includes_seq_and_ts() {
        let full = FullEvent {
            seq: 5,
            ts: "2026-02-18T10:00:00Z".into(),
            event: Event::PhaseStart {
                phase_id: "lint".into(),
                attempt: 1,
            },
        };
        let json = serde_json::to_string(&full).unwrap();
        assert!(json.contains(r#""seq":5"#));
        assert!(json.contains(r#""ts":"2026-02-18T10:00:00Z""#));
        assert!(json.contains(r#""type":"phase_start""#));
    }

    #[test]
    fn event_logger_creates_and_appends() {
        let dir = tempfile::tempdir().unwrap();
        let mut logger = EventLogger::new(dir.path(), "test-plan");

        logger.record(Event::PlanStart {
            plan_name: "test-plan".into(),
            phase_count: 2,
        });
        logger.record(Event::PhaseStart {
            phase_id: "a".into(),
            attempt: 1,
        });

        let content = std::fs::read_to_string(&logger.jsonl_path).unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 2);

        // Verify seq increments
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first["seq"], 0);
        assert_eq!(second["seq"], 1);
        assert_eq!(first["type"], "plan_start");
        assert_eq!(second["type"], "phase_start");
    }

    #[test]
    fn event_logger_with_stdout_json_builder() {
        let dir = tempfile::tempdir().unwrap();
        let logger = EventLogger::new(dir.path(), "test-plan").with_stdout_json(true);
        assert!(logger.stdout_json);

        let logger2 = EventLogger::new(dir.path(), "test-plan").with_stdout_json(false);
        assert!(!logger2.stdout_json);
    }

    #[test]
    fn event_logger_default_no_stdout_json() {
        let dir = tempfile::tempdir().unwrap();
        let logger = EventLogger::new(dir.path(), "test-plan");
        assert!(!logger.stdout_json);
    }

    #[test]
    fn runner_status_serialization() {
        let status = RunnerStatus {
            plan: "my-plan".into(),
            status: "running".into(),
            current_phase: Some("build".into()),
            completed: vec!["lint".into()],
            failed: vec![],
            awaiting_verdict: vec!["build".into()],
            updated_at: "2026-02-18T10:00:00Z".into(),
        };
        let json = serde_json::to_string_pretty(&status).unwrap();
        assert!(json.contains(r#""plan": "my-plan""#));
        assert!(json.contains(r#""current_phase": "build""#));
        assert!(json.contains(r#""completed""#));
    }

    #[test]
    fn event_gate_entered_serialization() {
        let event = Event::GateEntered {
            phase_id: "build".into(),
            subject: "my-plan/build".into(),
            gate_sha: "a".repeat(40),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"gate_entered""#));
        assert!(json.contains(r#""subject":"my-plan/build""#));
        assert!(json.contains(r#""gate_sha":"aaaa"#));
    }

    #[test]
    fn event_verdict_received_serialization() {
        let event = Event::VerdictReceived {
            phase_id: "build".into(),
            decision: "rejected".into(),
            gate_sha: "b".repeat(40),
            comment: Some("fix tests".into()),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"verdict_received""#));
        assert!(json.contains(r#""decision":"rejected""#));
        assert!(json.contains(r#""comment":"fix tests""#));
    }
}
