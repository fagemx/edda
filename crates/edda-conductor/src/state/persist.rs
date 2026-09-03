use crate::state::machine::{PhaseStatus, PlanState};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Validate that a plan name is safe to use in file paths and contains no traversal components.
pub fn validate_plan_name(name: &str) -> Result<()> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        anyhow::bail!("invalid plan name: {name:?}");
    }
    Ok(())
}

/// Compute the state file path for a plan.
/// Location: `{cwd}/.edda/conductor/{plan_name}/state.json`
pub fn state_path(cwd: &Path, plan_name: &str) -> PathBuf {
    cwd.join(".edda")
        .join("conductor")
        .join(plan_name)
        .join("state.json")
}

/// Compute the lock file path for a plan.
/// Location: `{cwd}/.edda/conductor/{plan_name}/state.lock`
pub fn lock_path(cwd: &Path, plan_name: &str) -> PathBuf {
    cwd.join(".edda")
        .join("conductor")
        .join(plan_name)
        .join("state.lock")
}

/// Plan-scoped exclusive file lock on `.edda/conductor/{plan_name}/state.lock`.
///
/// Serializes concurrent read-modify-write cycles between the long-lived
/// runner and CLI verbs (`conduct skip`, `retry`, `abort`) (GH-714).
pub struct PlanStateLock {
    _guard: edda_store::LockGuard,
}

impl PlanStateLock {
    /// Acquire the plan state lock, blocking until acquired.
    pub fn acquire(cwd: &Path, plan_name: &str) -> Result<Self> {
        validate_plan_name(plan_name)?;
        let path = lock_path(cwd, plan_name);
        let guard = edda_store::lock_file(&path)
            .with_context(|| format!("acquiring plan state lock: {}", path.display()))?;
        Ok(Self { _guard: guard })
    }

    /// Try to acquire the plan state lock non-blocking.
    /// Returns `Ok(Some(lock))` if acquired, `Ok(None)` if already held by another process.
    #[cfg(test)]
    pub(crate) fn try_acquire(cwd: &Path, plan_name: &str) -> Result<Option<Self>> {
        validate_plan_name(plan_name)?;
        let path = lock_path(cwd, plan_name);
        let guard = edda_store::try_lock_file(&path)
            .with_context(|| format!("trying plan state lock: {}", path.display()))?;
        Ok(guard.map(|g| Self { _guard: g }))
    }
}

/// Atomically load, mutate, and save plan state under the plan's exclusive lock (GH-714).
///
/// Validates that the plan state exists before taking any lock or creating directories,
/// preventing phantom directory creation from invalid or misspelled plan names.
pub fn update_state<F, R>(cwd: &Path, plan_name: &str, mutate: F) -> Result<R>
where
    F: FnOnce(&mut PlanState) -> Result<R>,
{
    validate_plan_name(plan_name)?;
    let path = state_path(cwd, plan_name);
    if !path.is_file() {
        anyhow::bail!("no state for plan \"{plan_name}\"");
    }

    let _lock = PlanStateLock::acquire(cwd, plan_name)?;
    let mut state = load_state(cwd, plan_name)?
        .ok_or_else(|| anyhow::anyhow!("no state for plan \"{plan_name}\""))?;
    let ret = mutate(&mut state)?;
    save_state(cwd, &state)?;
    Ok(ret)
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

/// Reconcile in-memory state with a disk state under the plan lock (GH-556, GH-714, GH-750).
///
/// The runner holds `PlanState` in memory for the whole run while other
/// processes (`edda conduct skip`, `retry`, ...) mutate the same file. A
/// plain `save_state` or unreconciled phase selection would clobber those
/// external writes or dispatch skipped phases.
///
/// Reconciliation rule: a phase that is Skipped on disk but still Pending in
/// memory is a manual skip recorded by another writer while this runner had
/// not started the phase — operator intent wins.
/// Field-scoped merge: we fold the status, skip_reason, and completed_at from the disk
/// phase into the in-memory phase without wholesale-replacing unrelated fields (GH-750).
pub fn reconcile_state(mem_state: &mut PlanState, disk_state: &PlanState) {
    for disk_phase in &disk_state.phases {
        if disk_phase.status != PhaseStatus::Skipped {
            continue;
        }
        if let Some(mem) = mem_state
            .phases
            .iter_mut()
            .find(|p| p.id == disk_phase.id && p.status == PhaseStatus::Pending)
        {
            mem.status = PhaseStatus::Skipped;
            mem.skip_reason = disk_phase.skip_reason.clone();
            if disk_phase.completed_at.is_some() {
                mem.completed_at = disk_phase.completed_at.clone();
            }
        }
    }
}

/// Reconcile in-memory state with disk state under the plan's exclusive lock (GH-750).
/// Returns Ok(true) if state on disk was loaded and reconciled, Ok(false) if no state file exists.
pub fn reconcile_with_disk(cwd: &Path, state: &mut PlanState) -> Result<bool> {
    let _lock = PlanStateLock::acquire(cwd, &state.plan_name)?;
    if let Some(disk) = load_state(cwd, &state.plan_name)? {
        reconcile_state(state, &disk);
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Save state from a long-lived writer (the runner) after reconciling with
/// the state currently on disk under the plan's exclusive lock (GH-556, GH-714, GH-750).
pub fn save_state_reconciled(cwd: &Path, state: &mut PlanState) -> Result<()> {
    let _lock = PlanStateLock::acquire(cwd, &state.plan_name)?;
    if let Some(disk) = load_state(cwd, &state.plan_name)? {
        reconcile_state(state, &disk);
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

    #[test]
    fn plan_state_lock_mutual_exclusion() {
        let dir = tempfile::tempdir().unwrap();
        let plan_name = "locked_plan";

        // Thread/Handle 1 acquires the lock
        let lock1 = PlanStateLock::acquire(dir.path(), plan_name).unwrap();

        // Second non-blocking attempt must fail to acquire while lock1 is held
        let lock2 = PlanStateLock::try_acquire(dir.path(), plan_name).unwrap();
        assert!(lock2.is_none(), "lock2 should be None while lock1 is held");

        // Release lock1
        drop(lock1);

        // Now lock can be acquired
        let lock3 = PlanStateLock::try_acquire(dir.path(), plan_name).unwrap();
        assert!(
            lock3.is_some(),
            "lock3 should succeed after lock1 is dropped"
        );
    }

    #[test]
    fn update_state_nonexistent_plan_does_not_create_directory() {
        let dir = tempfile::tempdir().unwrap();
        let result = update_state(dir.path(), "typo_plan", |_| Ok(()));
        assert!(result.is_err());
        assert!(
            !dir.path().join(".edda").exists(),
            "update_state on nonexistent plan must not create phantom directory"
        );
    }

    #[test]
    fn plan_name_with_traversal_is_rejected() {
        assert!(validate_plan_name("").is_err());
        assert!(validate_plan_name("foo/bar").is_err());
        assert!(validate_plan_name("foo\\bar").is_err());
        assert!(validate_plan_name("../escape").is_err());
        assert!(validate_plan_name("normal-plan_123").is_ok());

        let dir = tempfile::tempdir().unwrap();
        let result = update_state(dir.path(), "../escape", |_| Ok(()));
        assert!(result.is_err());
        assert!(!dir.path().join("escape").exists());
    }

    #[test]
    fn concurrent_update_state_serializes_without_clobbering() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let dir = tempfile::tempdir().unwrap();
        let plan = parse_plan(
            "name: concurrent\nphases:\n  - id: p1\n    prompt: \"one\"\n  - id: p2\n    prompt: \"two\"\n",
        )
        .unwrap();

        let initial_state = PlanState::from_plan(&plan, "concurrent.yaml");
        save_state(dir.path(), &initial_state).unwrap();

        let path = Arc::new(dir.path().to_path_buf());
        let barrier = Arc::new(Barrier::new(2));

        // Thread A skips p1 via update_state
        let path_a = Arc::clone(&path);
        let barrier_a = Arc::clone(&barrier);
        let handle_a = thread::spawn(move || {
            barrier_a.wait();
            update_state(&path_a, "concurrent", |state| {
                let p = state.get_phase_mut("p1").unwrap();
                p.status = PhaseStatus::Skipped;
                p.skip_reason = Some("skipped by thread A".into());
                Ok(())
            })
            .unwrap();
        });

        // Thread B skips p2 via update_state
        let path_b = Arc::clone(&path);
        let barrier_b = Arc::clone(&barrier);
        let handle_b = thread::spawn(move || {
            barrier_b.wait();
            update_state(&path_b, "concurrent", |state| {
                let p = state.get_phase_mut("p2").unwrap();
                p.status = PhaseStatus::Skipped;
                p.skip_reason = Some("skipped by thread B".into());
                Ok(())
            })
            .unwrap();
        });

        handle_a.join().unwrap();
        handle_b.join().unwrap();

        // Both updates must have survived — neither clobbered the other!
        let final_state = load_state(dir.path(), "concurrent").unwrap().unwrap();
        let p1 = final_state.get_phase("p1").unwrap();
        let p2 = final_state.get_phase("p2").unwrap();
        assert_eq!(p1.status, PhaseStatus::Skipped);
        assert_eq!(p1.skip_reason.as_deref(), Some("skipped by thread A"));
        assert_eq!(p2.status, PhaseStatus::Skipped);
        assert_eq!(p2.skip_reason.as_deref(), Some("skipped by thread B"));
    }

    #[test]
    fn save_state_reconciled_fails_on_corrupted_state_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let plan =
            parse_plan("name: corrupt-plan\nphases:\n  - id: p1\n    prompt: \"one\"\n").unwrap();
        let mut state = PlanState::from_plan(&plan, "corrupt-plan.yaml");

        // Write invalid/corrupted JSON to state.json
        let p = state_path(dir.path(), "corrupt-plan");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"{ not valid json }").unwrap();

        // save_state_reconciled must fail and must NOT overwrite the corrupted file (GH-741)
        let res = save_state_reconciled(dir.path(), &mut state);
        assert!(
            res.is_err(),
            "save_state_reconciled must return Err on unreadable state.json, but got: {:?}",
            res
        );

        // The on-disk file content must remain unchanged
        let on_disk = std::fs::read_to_string(&p).unwrap();
        assert_eq!(on_disk, "{ not valid json }");
    }

    #[test]
    fn reconcile_with_disk_fails_on_corrupted_state_with_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        let plan =
            parse_plan("name: corrupt-reconcile\nphases:\n  - id: p1\n    prompt: \"one\"\n")
                .unwrap();
        let mut state = PlanState::from_plan(&plan, "corrupt-reconcile.yaml");

        let p = state_path(dir.path(), "corrupt-reconcile");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"{ invalid json }").unwrap();

        let res = reconcile_with_disk(dir.path(), &mut state);
        assert!(
            res.is_err(),
            "reconcile_with_disk must fail on unreadable state.json, but got: {:?}",
            res
        );
        let err_msg = format!("{:#}", res.unwrap_err());
        assert!(
            err_msg.contains("parsing state"),
            "error diagnostic must surface failure reason: {}",
            err_msg
        );
    }

    #[test]
    fn reconcile_state_field_scoped_merge_preserves_unrelated_fields() {
        let plan =
            parse_plan("name: field-scoped\nphases:\n  - id: p1\n    prompt: \"one\"\n").unwrap();
        let mut mem_state = PlanState::from_plan(&plan, "field-scoped.yaml");
        let p = mem_state.get_phase_mut("p1").unwrap();
        p.status = PhaseStatus::Pending;
        p.attempts = 2;
        p.env_retries = 1;
        p.retry_context = Some("previous failure context".into());

        let mut disk_state = PlanState::from_plan(&plan, "field-scoped.yaml");
        let dp = disk_state.get_phase_mut("p1").unwrap();
        dp.status = PhaseStatus::Skipped;
        dp.skip_reason = Some("skipped externally".into());
        dp.completed_at = Some("2026-09-03T10:00:00Z".into());
        dp.attempts = 0; // disk had attempts 0 before runner attempts

        reconcile_state(&mut mem_state, &disk_state);

        let p = mem_state.get_phase("p1").unwrap();
        // Reconciled fields updated
        assert_eq!(p.status, PhaseStatus::Skipped);
        assert_eq!(p.skip_reason.as_deref(), Some("skipped externally"));
        assert_eq!(p.completed_at.as_deref(), Some("2026-09-03T10:00:00Z"));
        // Unrelated in-memory fields preserved (not wholesale wiped by disk phase)
        assert_eq!(p.attempts, 2);
        assert_eq!(p.env_retries, 1);
        assert_eq!(p.retry_context.as_deref(), Some("previous failure context"));
    }
}
