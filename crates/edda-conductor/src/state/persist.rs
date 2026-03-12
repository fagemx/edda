use crate::state::machine::PlanState;
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

    // ── Corrupted state recovery tests ─────────────────────────────

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
