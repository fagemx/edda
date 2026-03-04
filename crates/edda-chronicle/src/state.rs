use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

pub fn state_path(edda_root: &PathBuf) -> PathBuf {
    edda_root.join("chronicle").join("state.json")
}

pub fn load_state(edda_root: &PathBuf) -> Result<Option<RecapState>> {
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

pub fn save_state(edda_root: &PathBuf, state: &RecapState) -> Result<()> {
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
