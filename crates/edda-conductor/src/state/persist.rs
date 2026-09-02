use crate::state::machine::{PhaseStatus, PlanState};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Compute the state file path for a plan.
/// Location: `{cwd}/.edda/conductor/{plan_name}/state.json`
pub fn state_path(cwd: &Path, plan_name: &str) -> PathBuf {
    cwd.join(".edda")
        .join("conductor")
        .join(plan_name)
        .join("state.json")
}

/// Load state from disk. Returns None if the file doesn't exist.
pub fn load_state(cwd: &Path, plan_name: &str) -> Result<Option<PlanState>> {
    let path = state_path(cwd, plan_name);
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("reading state: {}", path.display()))?;
    let state: PlanState = serde_json::from_str(&content)
        .with_context(|| format!("parsing state: {}", path.display()))?;
    Ok(Some(state))
}

/// Save state atomically (write to .tmp, then rename).
pub fn save_state(cwd: &Path, state: &PlanState) -> Result<()> {
    let path = state_path(cwd, &state.plan_name);
    let data = serde_json::to_string_pretty(state)?;
    edda_store::write_atomic(&path, data.as_bytes())
        .with_context(|| format!("saving state: {}", path.display()))?;
    Ok(())
}

/// Save state from a long-lived writer (the runner) after reconciling with
/// the state currently on disk (GH-556).
///
/// The runner holds `PlanState` in memory for the whole run while other
/// processes (`edda conduct skip`, `retry`, ...) mutate the same file. A
/// plain `save_state` then clobbers those external writes with the stale
/// in-memory copy — observed as a manual skip on a Pending phase reverting
/// to Pending when a sibling phase transitioned to Stale in the runner.
///
/// Reconciliation rule: a phase that is Skipped on disk but still Pending in
/// memory is a manual skip recorded by another writer while this runner had
/// not started the phase — operator intent wins, and the disk phase
/// (including its skip reason) is folded into the in-memory state before
/// saving. Every other divergence keeps the in-memory value: the runner is
/// the authority for phases it has started (Running/Checking/terminal
/// transitions it observed itself).
pub fn save_state_reconciled(cwd: &Path, state: &mut PlanState) -> Result<()> {
    if let Ok(Some(disk)) = load_state(cwd, &state.plan_name) {
        for disk_phase in &disk.phases {
            if disk_phase.status != PhaseStatus::Skipped {
                continue;
            }
            if let Some(mem) = state
                .phases
                .iter_mut()
                .find(|p| p.id == disk_phase.id && p.status == PhaseStatus::Pending)
            {
                *mem = disk_phase.clone();
            }
        }
    }
    save_state(cwd, state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::parser::parse_plan;
    use crate::state::machine::PlanState;

    #[test]
    fn state_path_format() {
        let p = state_path(Path::new("/project"), "my-plan");
        assert!(p.to_string_lossy().contains("conductor"));
        assert!(p.to_string_lossy().contains("my-plan"));
        assert!(p.to_string_lossy().ends_with("state.json"));
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let result = load_state(dir.path(), "nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let plan = parse_plan("name: test\nphases:\n  - id: a\n    prompt: x\n").unwrap();
        let state = PlanState::from_plan(&plan, "plan.yaml");

        save_state(dir.path(), &state).unwrap();
        let loaded = load_state(dir.path(), "test").unwrap().unwrap();

        assert_eq!(loaded.plan_name, "test");
        assert_eq!(loaded.phases.len(), 1);
        assert_eq!(loaded.phases[0].id, "a");
    }

    #[test]
    fn save_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let plan = parse_plan("name: test\nphases:\n  - id: a\n    prompt: x\n").unwrap();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");

        save_state(dir.path(), &state).unwrap();
        state.version = 42;
        save_state(dir.path(), &state).unwrap();

        let loaded = load_state(dir.path(), "test").unwrap().unwrap();
        assert_eq!(loaded.version, 42);
    }

    #[test]
    fn awaiting_verdict_state_survives_disk_roundtrip() {
        use crate::state::machine::{transition, PhaseStatus, PhaseUpdate};

        let dir = tempfile::tempdir().unwrap();
        let plan = parse_plan("name: gated\nphases:\n  - id: a\n    prompt: x\n").unwrap();
        let mut state = PlanState::from_plan(&plan, "plan.yaml");
        transition(
            &mut state,
            "a",
            crate::state::machine::PhaseStatus::Pending,
            crate::state::machine::PhaseStatus::Running,
            None,
        )
        .unwrap();
        transition(
            &mut state,
            "a",
            PhaseStatus::Running,
            PhaseStatus::Checking,
            None,
        )
        .unwrap();
        transition(
            &mut state,
            "a",
            PhaseStatus::Checking,
            PhaseStatus::AwaitingVerdict,
            Some(PhaseUpdate {
                gate_sha: Some("0123456789abcdef".into()),
                gate_entered_at: Some("2026-01-01T00:00:00Z".into()),
                ..Default::default()
            }),
        )
        .unwrap();

        save_state(dir.path(), &state).unwrap();
        let loaded = load_state(dir.path(), "gated").unwrap().unwrap();
        let phase = loaded.get_phase("a").unwrap();
        assert_eq!(phase.status, PhaseStatus::AwaitingVerdict);
        assert_eq!(phase.gate_sha.as_deref(), Some("0123456789abcdef"));
        assert_eq!(
            phase.gate_entered_at.as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
    }

    // ── Corrupted state recovery tests ─────────────────────────────

    /// GH-556 regression: a manual skip written to disk by a second process
    /// (`edda conduct skip`) while the runner held a stale in-memory copy
    /// must survive the runner's next save. Before the fix the runner's
    /// plain save_state clobbered the skip back to Pending.
    ///
    /// Reproduction verified to FAIL before the fix: with
    /// `save_state_reconciled` reduced to plain `save_state`, this test
    /// fails at the first assertion (phase reverts to Pending, reason gone).
    #[test]
    fn manual_skip_survives_concurrent_runner_save() {
        use crate::state::machine::{transition, PhaseStatus};

        let dir = tempfile::tempdir().unwrap();
        let plan = parse_plan(
            "name: wave\nphases:\n  - id: a\n    prompt: x\n  - id: b\n    prompt: y\n    depends_on:\n      - a\n",
        )
        .unwrap();

        // The runner starts phase "a" and holds this in-memory copy.
        let mut runner_state = PlanState::from_plan(&plan, "wave.yaml");
        transition(
            &mut runner_state,
            "a",
            PhaseStatus::Pending,
            PhaseStatus::Running,
            None,
        )
        .unwrap();
        save_state(dir.path(), &runner_state).unwrap();

        // While "a" runs, another process manually skips pending phase "b".
        let mut cli_state = load_state(dir.path(), "wave").unwrap().unwrap();
        let b = cli_state.get_phase_mut("b").unwrap();
        b.status = PhaseStatus::Skipped;
        b.skip_reason = Some("parallelized: handed to another lane".into());
        save_state(dir.path(), &cli_state).unwrap();

        // The runner later saves its stale in-memory copy (e.g. after "a"
        // transitions to Stale). The skip must survive.
        runner_state.get_phase_mut("a").unwrap().status = PhaseStatus::Stale;
        save_state_reconciled(dir.path(), &mut runner_state).unwrap();

        let reloaded = load_state(dir.path(), "wave").unwrap().unwrap();
        let b = reloaded.get_phase("b").unwrap();
        assert_eq!(b.status, PhaseStatus::Skipped, "skip marker was lost");
        assert_eq!(
            b.skip_reason.as_deref(),
            Some("parallelized: handed to another lane"),
            "skip reason was lost"
        );
        // The runner's own transition is still persisted.
        assert_eq!(reloaded.get_phase("a").unwrap().status, PhaseStatus::Stale);
    }

    /// The runner remains the authority for phases it has started: a disk
    /// Skipped marker must NOT revert a Running phase in memory (the runner
    /// observed the dispatch itself), while a phase that is Pending in
    /// memory and Skipped on disk keeps the operator's skip.
    #[test]
    fn reconciled_save_keeps_runner_observed_transitions() {
        use crate::state::machine::{transition, PhaseStatus};

        let dir = tempfile::tempdir().unwrap();
        let plan =
            parse_plan("name: wave\nphases:\n  - id: a\n    prompt: x\n  - id: b\n    prompt: y\n")
                .unwrap();

        // Disk: "a" was skipped by hand after a previous session.
        let mut disk_state = PlanState::from_plan(&plan, "wave.yaml");
        let a = disk_state.get_phase_mut("a").unwrap();
        a.status = PhaseStatus::Skipped;
        a.skip_reason = Some("manual".into());
        save_state(dir.path(), &disk_state).unwrap();

        // Runner memory: it already dispatched "b" (Running) and has "a"
        // as Pending (it has not synced since the skip was written).
        let mut runner_state = load_state(dir.path(), "wave").unwrap().unwrap();
        runner_state.get_phase_mut("a").unwrap().status = PhaseStatus::Pending;
        transition(
            &mut runner_state,
            "b",
            PhaseStatus::Pending,
            PhaseStatus::Running,
            None,
        )
        .unwrap();
        save_state_reconciled(dir.path(), &mut runner_state).unwrap();

        let reloaded = load_state(dir.path(), "wave").unwrap().unwrap();
        // The runner started "b" — its Running status wins.
        assert_eq!(
            reloaded.get_phase("b").unwrap().status,
            PhaseStatus::Running
        );
        // "a" was Pending in memory and Skipped on disk → skip wins.
        let a = reloaded.get_phase("a").unwrap();
        assert_eq!(a.status, PhaseStatus::Skipped);
        assert_eq!(a.skip_reason.as_deref(), Some("manual"));
    }

    /// Helper: create the state.json directory structure and write content.
    fn write_corrupt_state(dir: &Path, plan_name: &str, content: &[u8]) {
        let path = state_path(dir, plan_name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
    }

    #[test]
    fn load_empty_file_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        write_corrupt_state(dir.path(), "broken", b"");
        let result = load_state(dir.path(), "broken");
        assert!(result.is_err(), "empty file should return Err, not Ok");
    }

    #[test]
    fn load_truncated_json_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        write_corrupt_state(dir.path(), "trunc", br#"{"plan_name": "te"#);
        let result = load_state(dir.path(), "trunc");
        assert!(result.is_err(), "truncated JSON should return Err, not Ok");
    }

    #[test]
    fn load_wrong_schema_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        // Valid JSON but missing required PlanState fields
        write_corrupt_state(dir.path(), "wrong", br#"{"unexpected": true}"#);
        let result = load_state(dir.path(), "wrong");
        assert!(result.is_err(), "wrong schema should return Err, not Ok");
    }

    #[test]
    fn load_binary_garbage_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        write_corrupt_state(dir.path(), "garbage", &[0x00, 0x01, 0xFF, 0xFE, 0x89, 0x50]);
        let result = load_state(dir.path(), "garbage");
        assert!(
            result.is_err(),
            "binary garbage should return Err, not panic"
        );
    }
}
