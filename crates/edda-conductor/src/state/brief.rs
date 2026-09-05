// Deferred fields from issue #125's full task-brief schema:
//
//   - `project`       — project-level metadata (repo, board URL, etc.)
//   - `iterations`    — per-iteration history with diffs and feedback
//   - `decisions`     — architectural decisions made during the task
//   - `lastFeedback`  — most recent human feedback snapshot
//
// These are intentionally omitted for now and will be added in a follow-up
// when the karvi adapter needs them.

use crate::state::machine::{PhaseStatus, PlanState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ── Brief schema (karvi interop format) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Brief {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<BriefMeta>,
    pub plan: BriefPlan,
    pub phases: HashMap<String, BriefPhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_phase: Option<String>,
    pub completed_phases: usize,
    /// GH-571: cost is only published when the backend measured usage. An
    /// unmeasured plan (cost_measured == false) emits no "cost" key at all,
    /// mirroring demo/runtime-edda.js which drops unmeasured cost claims.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<BriefCost>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefPlan {
    pub name: String,
    pub total_phases: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefPhase {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BriefCost {
    /// GH-571: None means unmeasured (no usage reported), not a genuine $0.00.
    /// A measured zero round-trips as Some(0.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub by_phase: HashMap<String, f64>,
}

// ── Conversion ──

impl Brief {
    /// Derive a Brief from the current PlanState.
    ///
    /// `meta` is optional — standalone conductor runs leave it as None.
    /// Runtime adapters (e.g. runtime-edda.js) can overlay their own meta
    /// after reading the produced `brief.json`.
    pub fn from_state(state: &PlanState, meta: Option<BriefMeta>) -> Self {
        let mut phases = HashMap::new();
        let mut current_phase = None;
        let mut completed_phases = 0;

        for ps in &state.phases {
            // Serialize PhaseStatus to its snake_case string
            let status_str = serde_json::to_value(ps.status)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| format!("{:?}", ps.status).to_lowercase());

            let brief_phase = BriefPhase {
                status: status_str,
                attempts: if ps.attempts > 0 {
                    Some(ps.attempts)
                } else {
                    None
                },
                duration_ms: ps.duration_ms,
                cost_usd: ps.cost_usd,
                started_at: ps.started_at.clone(),
                completed_at: ps.completed_at.clone(),
                error: ps.error.as_ref().map(|e| e.message.clone()),
                reason: ps.skip_reason.clone(),
            };

            phases.insert(ps.id.clone(), brief_phase);

            // Derive current_phase: the one that is Running or Checking
            if ps.status == PhaseStatus::Running || ps.status == PhaseStatus::Checking {
                current_phase = Some(ps.id.clone());
            }

            // Count completed phases (Passed or Skipped)
            if ps.status == PhaseStatus::Passed || ps.status == PhaseStatus::Skipped {
                completed_phases += 1;
            }
        }

        // GH-571: preserve measured-ness — unmeasured plans publish no cost
        // claim, while a measured zero stays a genuine Some(0.0).
        let cost = if state.cost_measured {
            Some(BriefCost {
                total_usd: Some(state.total_cost_usd),
                by_phase: HashMap::new(), // Per-phase cost not tracked in PlanState
            })
        } else {
            None
        };

        Brief {
            meta,
            plan: BriefPlan {
                name: state.plan_name.clone(),
                total_phases: state.phases.len(),
                budget_usd: None, // Plan budget not stored in PlanState
            },
            phases,
            current_phase,
            completed_phases,
            cost,
            artifacts: Vec::new(),
        }
    }
}

// ── File I/O ──

/// Compute the brief file path for a plan.
/// Location: `{cwd}/.edda/conductor/{plan_name}/brief.json`
pub fn brief_path(cwd: &Path, plan_name: &str) -> PathBuf {
    cwd.join(".edda")
        .join("conductor")
        .join(plan_name)
        .join("brief.json")
}

/// Derive a Brief from PlanState and write it atomically to disk.
///
/// Uses best-effort semantics (swallows errors internally) to match the
/// `write_runner_status` pattern — a brief-write failure must never abort
/// the run.
pub fn write_brief(cwd: &Path, state: &PlanState, meta: Option<BriefMeta>) {
    let mut measured = state.clone();
    super::derive::hydrate_durations(cwd, &mut measured);
    let brief = Brief::from_state(&measured, meta);
    let path = brief_path(cwd, &state.plan_name);
    if let Ok(data) = serde_json::to_string_pretty(&brief) {
        if let Err(e) = edda_store::write_atomic(&path, data.as_bytes()) {
            eprintln!("[brief] failed to write {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::parser::parse_plan;
    use crate::state::machine::{
        transition, ErrorInfo, ErrorType, PhaseStatus, PhaseUpdate, PlanState,
    };

    fn test_plan_state() -> PlanState {
        let plan = parse_plan(
            r#"
name: test
phases:
  - id: build
    prompt: "build it"
  - id: test
    prompt: "test it"
    depends_on: [build]
  - id: review
    prompt: "review it"
    depends_on: [test]
"#,
        )
        .unwrap();
        PlanState::from_plan(&plan, "plan.yaml")
    }

    #[test]
    fn from_state_empty_plan() {
        let plan = parse_plan("name: empty\nphases:\n  - id: a\n    prompt: x\n").unwrap();
        let state = PlanState::from_plan(&plan, "plan.yaml");
        let brief = Brief::from_state(&state, None);

        assert_eq!(brief.plan.name, "empty");
        assert_eq!(brief.plan.total_phases, 1);
        assert_eq!(brief.completed_phases, 0);
        assert!(brief.current_phase.is_none());
        assert!(brief.meta.is_none());
        assert_eq!(brief.phases.len(), 1);
        assert_eq!(brief.phases["a"].status, "pending");
    }

    #[test]
    fn from_state_with_phases() {
        let mut state = test_plan_state();

        // build: passed
        transition(
            &mut state,
            "build",
            PhaseStatus::Pending,
            PhaseStatus::Running,
            Some(PhaseUpdate {
                started_at: Some("2026-01-01T00:00:00Z".into()),
                attempts: Some(1),
                ..Default::default()
            }),
        )
        .unwrap();
        transition(
            &mut state,
            "build",
            PhaseStatus::Running,
            PhaseStatus::Checking,
            None,
        )
        .unwrap();
        transition(
            &mut state,
            "build",
            PhaseStatus::Checking,
            PhaseStatus::Passed,
            Some(PhaseUpdate {
                completed_at: Some("2026-01-01T00:02:00Z".into()),
                ..Default::default()
            }),
        )
        .unwrap();

        // test: running
        transition(
            &mut state,
            "test",
            PhaseStatus::Pending,
            PhaseStatus::Running,
            Some(PhaseUpdate {
                started_at: Some("2026-01-01T00:02:05Z".into()),
                attempts: Some(2),
                ..Default::default()
            }),
        )
        .unwrap();

        // review: pending (unchanged)

        let brief = Brief::from_state(&state, None);

        assert_eq!(brief.completed_phases, 1);
        assert_eq!(brief.current_phase.as_deref(), Some("test"));
        assert_eq!(brief.phases["build"].status, "passed");
        assert_eq!(brief.phases["test"].status, "running");
        assert_eq!(brief.phases["test"].attempts, Some(2));
        assert_eq!(brief.phases["review"].status, "pending");
    }

    #[test]
    fn from_state_cost_aggregation() {
        let mut state = test_plan_state();
        state.record_cost(1.23);

        let brief = Brief::from_state(&state, None);

        assert_eq!(brief.cost.as_ref().unwrap().total_usd, Some(1.23));
        // by_phase is empty because PlanState doesn't track per-phase cost
        assert!(brief.cost.as_ref().unwrap().by_phase.is_empty());
    }

    #[test]
    fn from_state_current_phase() {
        let mut state = test_plan_state();

        // No running phase → current_phase is None
        let brief = Brief::from_state(&state, None);
        assert!(brief.current_phase.is_none());

        // Set build to Running
        transition(
            &mut state,
            "build",
            PhaseStatus::Pending,
            PhaseStatus::Running,
            None,
        )
        .unwrap();
        let brief = Brief::from_state(&state, None);
        assert_eq!(brief.current_phase.as_deref(), Some("build"));

        // Set build to Checking → still current
        transition(
            &mut state,
            "build",
            PhaseStatus::Running,
            PhaseStatus::Checking,
            None,
        )
        .unwrap();
        let brief = Brief::from_state(&state, None);
        assert_eq!(brief.current_phase.as_deref(), Some("build"));
    }

    #[test]
    fn brief_json_camel_case() {
        let mut state = test_plan_state();
        // Set build to Running so currentPhase appears in JSON
        transition(
            &mut state,
            "build",
            PhaseStatus::Pending,
            PhaseStatus::Running,
            None,
        )
        .unwrap();
        state.record_cost(0.0);
        let brief = Brief::from_state(
            &state,
            Some(BriefMeta {
                board_type: Some("brief".into()),
                version: Some(1),
                task_id: Some("T5".into()),
                runtime: Some("edda".into()),
                updated_at: Some("2026-01-01T00:00:00Z".into()),
            }),
        );

        let json = serde_json::to_string_pretty(&brief).unwrap();

        // Verify camelCase keys
        assert!(json.contains("\"boardType\""), "expected boardType in JSON");
        assert!(
            json.contains("\"totalPhases\""),
            "expected totalPhases in JSON"
        );
        assert!(
            json.contains("\"completedPhases\""),
            "expected completedPhases in JSON"
        );
        assert!(
            json.contains("\"currentPhase\""),
            "expected currentPhase in JSON"
        );
        assert!(json.contains("\"taskId\""), "expected taskId in JSON");
        assert!(json.contains("\"updatedAt\""), "expected updatedAt in JSON");
        assert!(json.contains("\"totalUsd\""), "expected totalUsd in JSON");

        // Should NOT contain snake_case equivalents
        assert!(!json.contains("\"board_type\""));
        assert!(!json.contains("\"total_phases\""));
        assert!(!json.contains("\"completed_phases\""));
        assert!(!json.contains("\"current_phase\""));
        assert!(!json.contains("\"task_id\""));
    }

    #[test]
    fn brief_roundtrip() {
        let mut state = test_plan_state();
        transition(
            &mut state,
            "build",
            PhaseStatus::Pending,
            PhaseStatus::Running,
            Some(PhaseUpdate {
                started_at: Some("2026-01-01T00:00:00Z".into()),
                attempts: Some(1),
                ..Default::default()
            }),
        )
        .unwrap();
        state.record_cost(0.42);

        let brief = Brief::from_state(
            &state,
            Some(BriefMeta {
                board_type: Some("brief".into()),
                version: Some(1),
                task_id: None,
                runtime: Some("edda".into()),
                updated_at: None,
            }),
        );

        let json = serde_json::to_string_pretty(&brief).unwrap();
        let restored: Brief = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.plan.name, brief.plan.name);
        assert_eq!(restored.plan.total_phases, brief.plan.total_phases);
        assert_eq!(restored.completed_phases, brief.completed_phases);
        assert_eq!(restored.current_phase, brief.current_phase);
        assert_eq!(restored.cost, brief.cost);
        assert_eq!(restored.phases.len(), brief.phases.len());
        assert_eq!(
            restored.meta.as_ref().unwrap().board_type,
            brief.meta.as_ref().unwrap().board_type,
        );
    }

    #[test]
    fn from_state_unmeasured_cost() {
        let state = test_plan_state();
        assert!(!state.cost_measured);

        let brief = Brief::from_state(&state, None);

        assert!(brief.cost.is_none(), "unmeasured plan must publish no cost");
        let json = serde_json::to_string_pretty(&brief).unwrap();
        assert!(!json.contains("\"cost\""), "no cost key in JSON: {json}");
    }

    #[test]
    fn from_state_measured_zero_cost() {
        let mut state = test_plan_state();
        state.record_cost(0.0);

        let brief = Brief::from_state(&state, None);

        let cost = brief.cost.as_ref().unwrap();
        assert_eq!(cost.total_usd, Some(0.0));
        let json = serde_json::to_string_pretty(&brief).unwrap();
        assert!(json.contains("\"totalUsd\": 0.0"), "JSON: {json}");
    }

    #[test]
    fn from_state_measured_nonzero_cost() {
        let mut state = test_plan_state();
        state.record_cost(1.23);

        let brief = Brief::from_state(&state, None);

        let cost = brief.cost.as_ref().unwrap();
        assert_eq!(cost.total_usd, Some(1.23));
        let json = serde_json::to_string_pretty(&brief).unwrap();
        assert!(json.contains("\"totalUsd\": 1.23"), "JSON: {json}");
    }

    #[test]
    fn brief_roundtrip_unmeasured() {
        let state = test_plan_state();
        let brief = Brief::from_state(&state, None);
        assert!(brief.cost.is_none());

        let json = serde_json::to_string_pretty(&brief).unwrap();
        let restored: Brief = serde_json::from_str(&json).unwrap();
        assert!(restored.cost.is_none());
    }

    #[test]
    fn brief_deserialization_compatibility() {
        // Missing "cost" → None
        let restored: Brief = serde_json::from_str(
            r#"{
                "plan": {"name": "test", "totalPhases": 0},
                "phases": {},
                "completedPhases": 0,
                "artifacts": []
            }"#,
        )
        .unwrap();
        assert!(restored.cost.is_none(), "missing cost deserializes as None");

        // "cost": {} → Some(BriefCost { total_usd: None, .. })
        let restored: Brief = serde_json::from_str(
            r#"{
                "plan": {"name": "test", "totalPhases": 0},
                "phases": {},
                "completedPhases": 0,
                "artifacts": [],
                "cost": {}
            }"#,
        )
        .unwrap();
        let cost = restored.cost.as_ref().unwrap();
        assert_eq!(cost.total_usd, None);
        assert!(cost.by_phase.is_empty());

        // "cost": {"totalUsd": 0.0} → measured zero
        let restored: Brief = serde_json::from_str(
            r#"{
                "plan": {"name": "test", "totalPhases": 0},
                "phases": {},
                "completedPhases": 0,
                "artifacts": [],
                "cost": {"totalUsd": 0.0}
            }"#,
        )
        .unwrap();
        assert_eq!(restored.cost.as_ref().unwrap().total_usd, Some(0.0));

        // "cost": {"totalUsd": 1.50} → measured nonzero
        let restored: Brief = serde_json::from_str(
            r#"{
                "plan": {"name": "test", "totalPhases": 0},
                "phases": {},
                "completedPhases": 0,
                "artifacts": [],
                "cost": {"totalUsd": 1.50}
            }"#,
        )
        .unwrap();
        assert_eq!(restored.cost.as_ref().unwrap().total_usd, Some(1.50));
    }

    #[test]
    fn write_brief_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_plan_state();

        write_brief(dir.path(), &state, None);

        let path = brief_path(dir.path(), "test");
        assert!(path.exists(), "brief.json should exist");

        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["plan"]["name"], "test");
        assert_eq!(parsed["plan"]["totalPhases"], 3);
        assert_eq!(parsed["completedPhases"], 0);
    }

    #[test]
    fn from_state_error_flattened() {
        let mut state = test_plan_state();

        // build: pending → running → failed with error
        transition(
            &mut state,
            "build",
            PhaseStatus::Pending,
            PhaseStatus::Running,
            None,
        )
        .unwrap();
        transition(
            &mut state,
            "build",
            PhaseStatus::Running,
            PhaseStatus::Checking,
            None,
        )
        .unwrap();
        transition(
            &mut state,
            "build",
            PhaseStatus::Checking,
            PhaseStatus::Failed,
            Some(PhaseUpdate {
                error: Some(ErrorInfo {
                    error_type: ErrorType::CheckFailed,
                    message: "cargo test exited 1".into(),
                    retryable: true,
                    check_index: Some(0),
                    timestamp: "2026-01-01T00:01:00Z".into(),
                }),
                ..Default::default()
            }),
        )
        .unwrap();

        let brief = Brief::from_state(&state, None);
        assert_eq!(brief.phases["build"].status, "failed");
        assert_eq!(
            brief.phases["build"].error.as_deref(),
            Some("cargo test exited 1"),
        );
    }
    #[test]
    fn event_duration_reaches_derived_brief_without_inventing_missing_measurements() {
        use crate::runner::event_log::{Event, EventLogger};
        use crate::state::persist::{load_state, save_state};
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_plan_state();
        // Real semantics: the attempt counter is incremented and persisted
        // before the matching PhaseStart/terminal events are appended, so
        // attempt-1 evidence always coexists with a persisted attempts == 1.
        state.get_phase_mut("build").unwrap().attempts = 1;
        save_state(dir.path(), &state).unwrap();
        let mut events = EventLogger::new(dir.path(), &state.plan_name);
        for failed in [false, true] {
            events.record(if failed {
                Event::PhaseFailed {
                    phase_id: "build".into(),
                    attempt: 1,
                    duration_ms: 5000,
                    error: "fixture".into(),
                    error_type: None,
                    env_retries: 0,
                    attempt_charged: true,
                }
            } else {
                Event::PhasePassed {
                    phase_id: "build".into(),
                    attempt: 1,
                    duration_ms: 5000,
                    cost_usd: None,
                }
            });
            let restored = load_state(dir.path(), &state.plan_name).unwrap().unwrap();
            assert_eq!(restored.phases[0].duration_ms, Some(5000));
            write_brief(dir.path(), &state, None);
            let brief: Brief = serde_json::from_str(
                &std::fs::read_to_string(brief_path(dir.path(), &state.plan_name)).unwrap(),
            )
            .unwrap();
            assert_eq!(brief.phases["build"].duration_ms, Some(5000));
            assert_eq!(brief.phases["test"].duration_ms, None);
        }
        // Retry boundary: the new attempt number is persisted before its
        // PhaseStart, and the stale prior-attempt duration was already
        // cleared by the runner.
        state.get_phase_mut("build").unwrap().attempts = 2;
        save_state(dir.path(), &state).unwrap();
        events.record(Event::PhaseStart {
            phase_id: "build".into(),
            attempt: 2,
        });
        let restored = load_state(dir.path(), &state.plan_name).unwrap().unwrap();
        assert_eq!(
            Brief::from_state(&restored, None).phases["build"].duration_ms,
            None
        );
    }
}
