//! Structured event logging for conductor runs.
//!
//! Writes append-only JSONL to `.edda/conductor/{plan}/events.jsonl`.
//! Independent of edda/edda — works even if edda CLI is not installed.

use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

// ── Event types ──

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
    },
    PhaseSkipped {
        phase_id: String,
        reason: String,
    },
    PlanCompleted {
        phases_passed: usize,
        total_cost_usd: f64,
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
}

impl EventLogger {
    /// Create a new logger. Path: `{cwd}/.edda/conductor/{plan_name}/events.jsonl`.
    pub fn new(cwd: &Path, plan_name: &str) -> Self {
        let jsonl_path = cwd
            .join(".edda")
            .join("conductor")
            .join(plan_name)
            .join("events.jsonl");
        Self { jsonl_path, seq: 0 }
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
    fn runner_status_serialization() {
        let status = RunnerStatus {
            plan: "my-plan".into(),
            status: "running".into(),
            current_phase: Some("build".into()),
            completed: vec!["lint".into()],
            failed: vec![],
            updated_at: "2026-02-18T10:00:00Z".into(),
        };
        let json = serde_json::to_string_pretty(&status).unwrap();
        assert!(json.contains(r#""plan": "my-plan""#));
        assert!(json.contains(r#""current_phase": "build""#));
        assert!(json.contains(r#""completed""#));
    }
}
