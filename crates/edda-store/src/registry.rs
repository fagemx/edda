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
    /// Optional group name for cross-project sync. Projects in the same group
    /// share decisions marked with `scope=shared`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
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
                group: None,
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

/// Set the group for a project. Pass `None` to remove the group.
pub fn set_project_group(repo_root: &Path, group: Option<&str>) -> anyhow::Result<()> {
    let _lock = lock_file(&registry_lock_path())?;
    let pid = project_id(repo_root);
    let mut reg = load_registry();

    if let Some(entry) = reg.projects.get_mut(&pid) {
        entry.group = group.map(|g| g.to_string());
        save_registry(&reg)
    } else {
        anyhow::bail!("project not registered: {pid}")
    }
}

/// Get the group for a project.
pub fn project_group(repo_root: &Path) -> Option<String> {
    let pid = project_id(repo_root);
    let reg = load_registry();
    reg.projects.get(&pid).and_then(|e| e.group.clone())
}

/// List all projects in the same group as the given project.
/// Returns an empty vec if the project has no group.
pub fn list_group_members(repo_root: &Path) -> Vec<ProjectEntry> {
    let pid = project_id(repo_root);
    let reg = load_registry();
    let group = match reg.projects.get(&pid).and_then(|e| e.group.as_ref()) {
        Some(g) => g.clone(),
        None => return Vec::new(),
    };
    reg.projects
        .into_values()
        .filter(|e| e.group.as_deref() == Some(&group) && e.project_id != pid)
        .collect()
}

/// The projects a fleet read covers (GH-407): the cwd project's group when it
/// has one, otherwise every registered project.
///
/// Differs from [`list_group_members`] in two ways that matter to a reader:
///
/// - **Home is included.** A fleet read reports its own workspace's hits tagged
///   like any other; excluding it would silently omit the one repo the user is
///   actually standing in.
/// - **Absent repos are kept.** A project whose path is no longer on this
///   machine stays in scope so the caller can say so per-project. Filtering it
///   here would make the fan-out quietly incomplete, which is the same
///   silent-omission failure the fleet read exists to remove.
pub fn fleet_scope(repo_root: &Path) -> Vec<ProjectEntry> {
    let pid = project_id(repo_root);
    let reg = load_registry();
    let group = reg.projects.get(&pid).and_then(|e| e.group.clone());
    match group {
        Some(g) => reg
            .projects
            .into_values()
            .filter(|e| e.group.as_deref() == Some(g.as_str()))
            .collect(),
        None => reg.projects.into_values().collect(),
    }
}

/// List all groups and their member projects.
pub fn list_groups() -> std::collections::BTreeMap<String, Vec<ProjectEntry>> {
    let reg = load_registry();
    let mut groups: std::collections::BTreeMap<String, Vec<ProjectEntry>> =
        std::collections::BTreeMap::new();
    for entry in reg.projects.into_values() {
        if let Some(ref g) = entry.group {
            groups.entry(g.clone()).or_default().push(entry);
        }
    }
    groups
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
    /// Run a closure with `EDDA_STORE_ROOT` pointing to an isolated tempdir.
    fn with_isolated_store(f: impl FnOnce()) {
        let _guard = crate::ENV_STORE_LOCK.lock().unwrap();
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
    fn set_and_get_group() {
        with_isolated_store(|| {
            let tmp = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(tmp.path().join(".edda")).unwrap();
            register_project(tmp.path()).unwrap();

            assert!(project_group(tmp.path()).is_none());

            set_project_group(tmp.path(), Some("team-a")).unwrap();
            assert_eq!(project_group(tmp.path()), Some("team-a".to_string()));

            set_project_group(tmp.path(), None).unwrap();
            assert!(project_group(tmp.path()).is_none());
        });
    }

    #[test]
    fn list_group_members_returns_peers() {
        with_isolated_store(|| {
            let tmp1 = tempfile::tempdir().unwrap();
            let tmp2 = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(tmp1.path().join(".edda")).unwrap();
            std::fs::create_dir_all(tmp2.path().join(".edda")).unwrap();

            register_project(tmp1.path()).unwrap();
            register_project(tmp2.path()).unwrap();

            set_project_group(tmp1.path(), Some("team-x")).unwrap();
            set_project_group(tmp2.path(), Some("team-x")).unwrap();

            let members = list_group_members(tmp1.path());
            assert_eq!(members.len(), 1);
            assert_eq!(members[0].project_id, project_id(tmp2.path()));

            // No group = no members
            let tmp3 = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(tmp3.path().join(".edda")).unwrap();
            register_project(tmp3.path()).unwrap();
            assert!(list_group_members(tmp3.path()).is_empty());
        });
    }

    /// GH-407: what a `--fleet` read covers.
    #[test]
    fn fleet_scope_is_every_project_when_no_group_and_includes_home() {
        with_isolated_store(|| {
            let home = tempfile::tempdir().unwrap();
            let other = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(home.path().join(".edda")).unwrap();
            std::fs::create_dir_all(other.path().join(".edda")).unwrap();
            register_project(home.path()).unwrap();
            register_project(other.path()).unwrap();

            let scope = fleet_scope(home.path());

            // Home is included, unlike list_group_members: a fleet read reports
            // its own hits tagged like anyone else's, or the output silently
            // omits the workspace the user is standing in.
            assert_eq!(scope.len(), 2, "no group = the whole registry");
            assert!(scope
                .iter()
                .any(|e| e.project_id == project_id(home.path())));
        });
    }

    #[test]
    fn fleet_scope_narrows_to_the_group_when_one_is_set() {
        with_isolated_store(|| {
            let home = tempfile::tempdir().unwrap();
            let peer = tempfile::tempdir().unwrap();
            let outsider = tempfile::tempdir().unwrap();
            for d in [&home, &peer, &outsider] {
                std::fs::create_dir_all(d.path().join(".edda")).unwrap();
                register_project(d.path()).unwrap();
            }
            set_project_group(home.path(), Some("fleet")).unwrap();
            set_project_group(peer.path(), Some("fleet")).unwrap();

            let scope = fleet_scope(home.path());

            assert_eq!(scope.len(), 2, "the group, home included");
            assert!(
                !scope
                    .iter()
                    .any(|e| e.project_id == project_id(outsider.path())),
                "a project outside the group is not in scope"
            );
        });
    }

    /// A repo that is not on this machine must reach the caller so it can be
    /// reported per-project. Dropping it here would make the fan-out silently
    /// incomplete — the same failure mode GH-407 exists to remove.
    #[test]
    fn fleet_scope_keeps_projects_whose_repo_is_gone() {
        with_isolated_store(|| {
            let home = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(home.path().join(".edda")).unwrap();
            register_project(home.path()).unwrap();

            let gone = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(gone.path().join(".edda")).unwrap();
            register_project(gone.path()).unwrap();
            let gone_pid = project_id(gone.path());
            drop(gone); // the repo disappears from this machine

            let scope = fleet_scope(home.path());

            assert!(
                scope.iter().any(|e| e.project_id == gone_pid),
                "an absent repo must be visible to the caller, not filtered away"
            );
        });
    }

    #[test]
    fn list_groups_returns_all() {
        with_isolated_store(|| {
            let tmp1 = tempfile::tempdir().unwrap();
            let tmp2 = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(tmp1.path().join(".edda")).unwrap();
            std::fs::create_dir_all(tmp2.path().join(".edda")).unwrap();

            register_project(tmp1.path()).unwrap();
            register_project(tmp2.path()).unwrap();

            set_project_group(tmp1.path(), Some("alpha")).unwrap();
            set_project_group(tmp2.path(), Some("alpha")).unwrap();

            let groups = list_groups();
            assert_eq!(groups.len(), 1);
            assert_eq!(groups["alpha"].len(), 2);
        });
    }

    #[test]
    fn group_backward_compat() {
        // Simulating a registry.json without the group field
        with_isolated_store(|| {
            let json = r#"{"projects":{"abc":{"project_id":"abc","path":"/tmp/x","name":"x","registered_at":"2026-01-01","last_seen":"2026-01-01"}}}"#;
            let reg: Registry = serde_json::from_str(json).unwrap();
            let entry = reg.projects.get("abc").unwrap();
            assert!(entry.group.is_none());
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
