use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecapState {
    pub last_recap: LastRecap,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastRecap {
    pub timestamp: String,
    pub anchor: String,
    pub sessions_covered: Vec<String>,
}

pub fn state_path(edda_root: &Path) -> PathBuf {
    edda_root.join("chronicle").join("state.json")
}

pub fn load_state(edda_root: &Path) -> Result<Option<RecapState>> {
    let path = state_path(edda_root);
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read state file: {:?}", path))?;
    let state: RecapState =
        serde_json::from_str(&content).with_context(|| "Failed to parse state.json")?;
    Ok(Some(state))
}

pub fn save_state(edda_root: &Path, state: &RecapState) -> Result<()> {
    let path = state_path(edda_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create chronicle directory: {:?}", parent))?;
    }
    let content =
        serde_json::to_string_pretty(state).with_context(|| "Failed to serialize state")?;
    let tmp_path = path.with_extension("tmp");
    std::fs::write(&tmp_path, content)
        .with_context(|| format!("Failed to write state file: {:?}", tmp_path))?;
    std::fs::rename(&tmp_path, &path)
        .with_context(|| format!("Failed to rename state file: {:?} -> {:?}", tmp_path, path))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state(ts: &str, sessions: Vec<String>) -> RecapState {
        RecapState {
            last_recap: LastRecap {
                timestamp: ts.to_string(),
                anchor: "default".to_string(),
                sessions_covered: sessions,
            },
        }
    }

    #[test]
    fn test_load_missing_state() {
        let tmp = tempfile::tempdir().unwrap();
        let result = load_state(tmp.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let state = make_state("2026-01-01T00:00:00Z", vec!["s1".into(), "s2".into()]);
        save_state(tmp.path(), &state).unwrap();

        let loaded = load_state(tmp.path()).unwrap().expect("state should exist");
        assert_eq!(loaded.last_recap.timestamp, "2026-01-01T00:00:00Z");
        assert_eq!(loaded.last_recap.sessions_covered, vec!["s1", "s2"]);
        assert_eq!(loaded.last_recap.anchor, "default");
    }

    #[test]
    fn test_corrupted_state_returns_err() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("chronicle");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("state.json"), "not valid json{{{").unwrap();

        let result = load_state(tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_save_creates_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let chronicle_dir = tmp.path().join("chronicle");
        assert!(!chronicle_dir.exists());

        let state = make_state("2026-01-01T00:00:00Z", vec![]);
        save_state(tmp.path(), &state).unwrap();

        assert!(chronicle_dir.exists());
        assert!(chronicle_dir.join("state.json").exists());
        assert!(!chronicle_dir.join("state.tmp").exists());
    }
}
