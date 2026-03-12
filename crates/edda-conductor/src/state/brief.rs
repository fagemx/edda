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
    pub cost: BriefCost,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BriefCost {
    pub total_usd: f64,
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
                duration_ms: None, // Not tracked in PlanState
                cost_usd: None,    // Not tracked in PlanState per-phase
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

        let cost = BriefCost {
            total_usd: state.total_cost_usd,
            by_phase: HashMap::new(), // Per-phase cost not tracked in PlanState
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
    let brief = Brief::from_state(state, meta);
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
        let plan =
            parse_plan("name: empty\nphases:\n  - id: a\n    prompt: x\n").unwrap();
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
        state.total_cost_usd = 1.23;

        let brief = Brief::from_state(&state, None);

        assert!((brief.cost.total_usd - 1.23).abs() < f64::EPSILON);
        // by_phase is empty because PlanState doesn't track per-phase cost
        assert!(brief.cost.by_phase.is_empty());
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
        let brief = Brief::from_state(&state, Some(BriefMeta {
            board_type: Some("brief".into()),
            version: Some(1),
            task_id: Some("T5".into()),
            runtime: Some("edda".into()),
            updated_at: Some("2026-01-01T00:00:00Z".into()),
        }));

        let json = serde_json::to_string_pretty(&brief).unwrap();

        // Verify camelCase keys
        assert!(json.contains("\"boardType\""), "expected boardType in JSON");
        assert!(json.contains("\"totalPhases\""), "expected totalPhases in JSON");
        assert!(json.contains("\"completedPhases\""), "expected completedPhases in JSON");
        assert!(json.contains("\"currentPhase\""), "expected currentPhase in JSON");
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
        state.total_cost_usd = 0.42;

        let brief = Brief::from_state(&state, Some(BriefMeta {
            board_type: Some("brief".into()),
            version: Some(1),
            task_id: None,
            runtime: Some("edda".into()),
            updated_at: None,
        }));

        let json = serde_json::to_string_pretty(&brief).unwrap();
        let restored: Brief = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.plan.name, brief.plan.name);
        assert_eq!(restored.plan.total_phases, brief.plan.total_phases);
        assert_eq!(restored.completed_phases, brief.completed_phases);
        assert_eq!(restored.current_phase, brief.current_phase);
        assert!((restored.cost.total_usd - brief.cost.total_usd).abs() < f64::EPSILON);
        assert_eq!(restored.phases.len(), brief.phases.len());
        assert_eq!(
            restored.meta.as_ref().unwrap().board_type,
            brief.meta.as_ref().unwrap().board_type,
        );
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
}
