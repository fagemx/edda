//! Project registry: `~/.edda/registry.json`
//!
//! Maps project_id (BLAKE3 hash) to repo path. Used for cross-repo aggregation.

use crate::{lock_file, project_id, store_root, write_atomic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

/// A registered project entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub project_id: String,
    pub path: String,
    pub name: String,
    pub registered_at: String,
    pub last_seen: String,
}

/// The full registry: a map of project_id → ProjectEntry.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Registry {
    pub projects: BTreeMap<String, ProjectEntry>,
}

/// Path to the registry JSON file.
pub fn registry_path() -> PathBuf {
    store_root().join("registry.json")
}

/// Path to the registry lock file.
fn registry_lock_path() -> PathBuf {
    store_root().join("registry.lock")
}

/// Load the registry from disk. Returns empty registry if file doesn't exist.
fn load_registry() -> Registry {
    let path = registry_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => Registry::default(),
    }
}

/// Save the registry to disk atomically.
fn save_registry(reg: &Registry) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(reg)?;
    write_atomic(&registry_path(), json.as_bytes())
}

/// Get current timestamp as RFC 3339 string.
fn now_rfc3339() -> String {
    let now = OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Extract project name from the repo path (last component).
fn project_name_from_path(repo_root: &Path) -> String {
    repo_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Register a project in the user-level registry. Idempotent.
pub fn register_project(repo_root: &Path) -> anyhow::Result<()> {
    let _lock = lock_file(&registry_lock_path())?;
    let pid = project_id(repo_root);
    let mut reg = load_registry();
    let now = now_rfc3339();

    if let Some(entry) = reg.projects.get_mut(&pid) {
        // Update last_seen and path (in case repo was moved)
        entry.last_seen = now;
        entry.path = repo_root.to_string_lossy().to_string();
    } else {
        reg.projects.insert(
            pid.clone(),
            ProjectEntry {
                project_id: pid,
                path: repo_root.to_string_lossy().to_string(),
                name: project_name_from_path(repo_root),
                registered_at: now.clone(),
                last_seen: now,
            },
        );
    }

    save_registry(&reg)
}

/// Unregister a project by project_id.
pub fn unregister_project(pid: &str) -> anyhow::Result<()> {
    let _lock = lock_file(&registry_lock_path())?;
    let mut reg = load_registry();
    reg.projects.remove(pid);
    save_registry(&reg)
}

/// List all registered projects.
pub fn list_projects() -> Vec<ProjectEntry> {
    let reg = load_registry();
    reg.projects.into_values().collect()
}

/// Get a specific project by project_id.
pub fn get_project(pid: &str) -> Option<ProjectEntry> {
    let reg = load_registry();
    reg.projects.get(pid).cloned()
}

/// Update last_seen timestamp for a project.
pub fn touch_project(repo_root: &Path) -> anyhow::Result<()> {
    let _lock = lock_file(&registry_lock_path())?;
    let pid = project_id(repo_root);
    let mut reg = load_registry();

    if let Some(entry) = reg.projects.get_mut(&pid) {
        entry.last_seen = now_rfc3339();
        save_registry(&reg)?;
    }

    Ok(())
}

/// Validate all registered projects. Returns (valid, stale) entries.
/// A project is stale if its path no longer contains a `.edda/` directory.
pub fn validate_projects() -> (Vec<ProjectEntry>, Vec<ProjectEntry>) {
    let reg = load_registry();
    let mut valid = Vec::new();
    let mut stale = Vec::new();

    for entry in reg.projects.into_values() {
        let edda_dir = Path::new(&entry.path).join(".edda");
        if edda_dir.is_dir() {
            valid.push(entry);
        } else {
            stale.push(entry);
        }
    }

    (valid, stale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize registry tests so EDDA_STORE_ROOT env var doesn't conflict.
    static REGISTRY_LOCK: Mutex<()> = Mutex::new(());

    /// Run a closure with `EDDA_STORE_ROOT` pointing to an isolated tempdir.
    fn with_isolated_store(f: impl FnOnce()) {
        let _guard = REGISTRY_LOCK.lock().unwrap();
        let store = tempfile::tempdir().unwrap();
        std::env::set_var("EDDA_STORE_ROOT", store.path());
        f();
        std::env::remove_var("EDDA_STORE_ROOT");
    }

    #[test]
    fn register_and_list_roundtrip() {
        with_isolated_store(|| {
            let tmp = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(tmp.path().join(".edda")).unwrap();

            register_project(tmp.path()).unwrap();
            let projects = list_projects();
            let pid = project_id(tmp.path());

            assert!(projects.iter().any(|p| p.project_id == pid));
        });
    }

    #[test]
    fn register_is_idempotent() {
        with_isolated_store(|| {
            let tmp = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(tmp.path().join(".edda")).unwrap();

            register_project(tmp.path()).unwrap();
            register_project(tmp.path()).unwrap();

            let pid = project_id(tmp.path());
            let reg = load_registry();
            let count = reg
                .projects
                .values()
                .filter(|p| p.project_id == pid)
                .count();
            assert_eq!(count, 1, "should not create duplicates");
        });
    }

    #[test]
    fn unregister_removes_entry() {
        with_isolated_store(|| {
            let tmp = tempfile::tempdir().unwrap();
            let pid = project_id(tmp.path());

            register_project(tmp.path()).unwrap();
            assert!(get_project(&pid).is_some());

            unregister_project(&pid).unwrap();
            assert!(get_project(&pid).is_none());
        });
    }

    #[test]
    fn validate_detects_stale() {
        with_isolated_store(|| {
            let tmp = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(tmp.path().join(".edda")).unwrap();
            register_project(tmp.path()).unwrap();

            let pid = project_id(tmp.path());
            std::fs::remove_dir_all(tmp.path().join(".edda")).unwrap();

            let (valid, stale) = validate_projects();
            assert!(stale.iter().any(|p| p.project_id == pid));
            assert!(!valid.iter().any(|p| p.project_id == pid));
        });
    }
}
