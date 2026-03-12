//! Skill registry: `~/.edda/skill_registry.json`
//!
//! Tracks skills across projects with content-hash versioning.
//! Follows the same pattern as `registry.rs` (project registry).

use crate::{lock_file, store_root, write_atomic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

/// A version history entry — records when a content hash was first seen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionEntry {
    pub content_hash: String,
    pub seen_at: String,
}

/// A registered skill entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEntry {
    /// Composite key: `{name}:{project_name}`
    pub skill_id: String,
    /// Skill name (from frontmatter or directory name)
    pub name: String,
    /// Skill description (from frontmatter)
    pub description: String,
    /// BLAKE3 hash of the project's repo path (first 32 hex chars)
    pub project_id: String,
    /// Human-readable project name (last path component)
    pub project_name: String,
    /// Relative path within the project (e.g. `.claude/skills/issue-plan/SKILL.md`)
    pub relative_path: String,
    /// BLAKE3 hash of SKILL.md content (hex string)
    pub content_hash: String,
    /// RFC 3339 timestamp when first registered
    pub registered_at: String,
    /// RFC 3339 timestamp of last scan/touch
    pub last_seen: String,
    /// History of content hash changes
    pub version_history: Vec<VersionEntry>,
}

/// The full skill registry: a map of skill_id -> SkillEntry.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillRegistry {
    pub skills: BTreeMap<String, SkillEntry>,
}

/// A skill discovered by scanning a project directory.
#[derive(Debug, Clone)]
pub struct ScannedSkill {
    pub name: String,
    pub description: String,
    pub relative_path: String,
    pub content_hash: String,
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Path to the skill registry JSON file.
pub fn skill_registry_path() -> PathBuf {
    store_root().join("skill_registry.json")
}

/// Path to the skill registry lock file.
fn skill_registry_lock_path() -> PathBuf {
    store_root().join("skill_registry.lock")
}

// ---------------------------------------------------------------------------
// Load / Save
// ---------------------------------------------------------------------------

/// Load the skill registry from disk. Returns empty registry if file doesn't exist.
fn load_skill_registry() -> SkillRegistry {
    let path = skill_registry_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => SkillRegistry::default(),
    }
}

/// Save the skill registry to disk atomically.
fn save_skill_registry(reg: &SkillRegistry) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(reg)?;
    write_atomic(&skill_registry_path(), json.as_bytes())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Get current timestamp as RFC 3339 string.
fn now_rfc3339() -> String {
    let now = OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Build a composite skill ID: `{name}:{project_name}`.
pub fn skill_id(name: &str, project_name: &str) -> String {
    format!("{name}:{project_name}")
}

// ---------------------------------------------------------------------------
// Frontmatter parsing
// ---------------------------------------------------------------------------

/// Parse YAML frontmatter from a SKILL.md file.
///
/// Expects the file to start with `---`, followed by YAML, followed by `---`.
/// Returns `(name, description)` if both fields are found.
pub fn parse_skill_frontmatter(content: &str) -> Option<(String, String)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    // Find the closing ---
    let after_first = &trimmed[3..];
    let end = after_first.find("---")?;
    let yaml_block = &after_first[..end];

    let mut name = None;
    let mut description = None;

    for line in yaml_block.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name:") {
            name = Some(rest.trim().trim_matches('"').trim_matches('\'').to_string());
        } else if let Some(rest) = line.strip_prefix("description:") {
            description = Some(rest.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }

    let name = name.filter(|n| !n.is_empty())?;
    let description = description.unwrap_or_default();
    Some((name, description))
}

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------

/// Scan a project directory for `.claude/skills/*/SKILL.md` files.
///
/// Returns a list of discovered skills with parsed frontmatter and content hashes.
pub fn scan_project_skills(repo_root: &Path) -> Vec<ScannedSkill> {
    let skills_dir = repo_root.join(".claude").join("skills");
    let mut results = Vec::new();

    let entries = match std::fs::read_dir(&skills_dir) {
        Ok(e) => e,
        Err(_) => return results,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }

        let content = match std::fs::read_to_string(&skill_md) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let content_hash = blake3::hash(content.as_bytes()).to_hex().to_string();

        let dir_name = entry.file_name().to_string_lossy().to_string();

        let (name, description) =
            parse_skill_frontmatter(&content).unwrap_or_else(|| (dir_name.clone(), String::new()));

        let relative_path = format!(".claude/skills/{}/SKILL.md", dir_name);

        results.push(ScannedSkill {
            name,
            description,
            relative_path,
            content_hash,
        });
    }

    results.sort_by(|a, b| a.name.cmp(&b.name));
    results
}

// ---------------------------------------------------------------------------
// CRUD operations
// ---------------------------------------------------------------------------

/// Register a skill in the user-level skill registry. Idempotent.
///
/// If the skill already exists with a different content_hash, the old hash is
/// appended to `version_history` and the new hash replaces `content_hash`.
/// If the hash is unchanged, only `last_seen` is updated.
pub fn register_skill(
    project_id: &str,
    project_name: &str,
    name: &str,
    description: &str,
    relative_path: &str,
    content_hash: &str,
) -> anyhow::Result<()> {
    let _lock = lock_file(&skill_registry_lock_path())?;
    let mut reg = load_skill_registry();
    let sid = skill_id(name, project_name);
    let now = now_rfc3339();

    if let Some(entry) = reg.skills.get_mut(&sid) {
        // Update mutable fields
        entry.description = description.to_string();
        entry.relative_path = relative_path.to_string();
        entry.last_seen = now.clone();

        if entry.content_hash != content_hash {
            // Content changed — record old version and update hash
            entry.version_history.push(VersionEntry {
                content_hash: entry.content_hash.clone(),
                seen_at: now,
            });
            entry.content_hash = content_hash.to_string();
        }
    } else {
        reg.skills.insert(
            sid.clone(),
            SkillEntry {
                skill_id: sid,
                name: name.to_string(),
                description: description.to_string(),
                project_id: project_id.to_string(),
                project_name: project_name.to_string(),
                relative_path: relative_path.to_string(),
                content_hash: content_hash.to_string(),
                registered_at: now.clone(),
                last_seen: now,
                version_history: Vec::new(),
            },
        );
    }

    save_skill_registry(&reg)
}

/// Unregister a skill by skill_id.
pub fn unregister_skill(skill_id: &str) -> anyhow::Result<()> {
    let _lock = lock_file(&skill_registry_lock_path())?;
    let mut reg = load_skill_registry();
    reg.skills.remove(skill_id);
    save_skill_registry(&reg)
}

/// Unregister all skills for a given project_id.
pub fn unregister_project_skills(pid: &str) -> anyhow::Result<()> {
    let _lock = lock_file(&skill_registry_lock_path())?;
    let mut reg = load_skill_registry();
    reg.skills.retain(|_, entry| entry.project_id != pid);
    save_skill_registry(&reg)
}

/// List all registered skills.
pub fn list_skills() -> Vec<SkillEntry> {
    let reg = load_skill_registry();
    reg.skills.into_values().collect()
}

/// List skills for a specific project_id.
pub fn list_skills_by_project(pid: &str) -> Vec<SkillEntry> {
    let reg = load_skill_registry();
    reg.skills
        .into_values()
        .filter(|e| e.project_id == pid)
        .collect()
}

/// Get a specific skill by skill_id.
pub fn get_skill(sid: &str) -> Option<SkillEntry> {
    let reg = load_skill_registry();
    reg.skills.get(sid).cloned()
}

/// Update last_seen timestamp for a skill.
pub fn touch_skill(sid: &str) -> anyhow::Result<()> {
    let _lock = lock_file(&skill_registry_lock_path())?;
    let mut reg = load_skill_registry();

    if let Some(entry) = reg.skills.get_mut(sid) {
        entry.last_seen = now_rfc3339();
        save_skill_registry(&reg)?;
    }

    Ok(())
}

/// Scan a project and register/update all discovered skills.
///
/// Returns the number of skills registered or updated.
pub fn scan_and_register(repo_root: &Path) -> anyhow::Result<usize> {
    let pid = crate::project_id(repo_root);
    let pname = repo_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let skills = scan_project_skills(repo_root);
    let count = skills.len();

    for skill in skills {
        register_skill(
            &pid,
            &pname,
            &skill.name,
            &skill.description,
            &skill.relative_path,
            &skill.content_hash,
        )?;
    }

    Ok(count)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
            register_skill(
                "proj1",
                "my-project",
                "issue-plan",
                "Plan issues",
                ".claude/skills/issue-plan/SKILL.md",
                "abc123",
            )
            .unwrap();

            let skills = list_skills();
            assert_eq!(skills.len(), 1);
            assert_eq!(skills[0].name, "issue-plan");
            assert_eq!(skills[0].project_name, "my-project");
            assert_eq!(skills[0].content_hash, "abc123");
        });
    }

    #[test]
    fn register_is_idempotent() {
        with_isolated_store(|| {
            register_skill(
                "proj1",
                "my-project",
                "issue-plan",
                "Plan issues",
                ".claude/skills/issue-plan/SKILL.md",
                "abc123",
            )
            .unwrap();
            register_skill(
                "proj1",
                "my-project",
                "issue-plan",
                "Plan issues",
                ".claude/skills/issue-plan/SKILL.md",
                "abc123",
            )
            .unwrap();

            let skills = list_skills();
            assert_eq!(skills.len(), 1, "should not create duplicates");
            assert!(skills[0].version_history.is_empty(), "no version change");
        });
    }

    #[test]
    fn content_hash_change_appends_version() {
        with_isolated_store(|| {
            register_skill(
                "proj1",
                "my-project",
                "issue-plan",
                "v1",
                ".claude/skills/issue-plan/SKILL.md",
                "hash_v1",
            )
            .unwrap();
            register_skill(
                "proj1",
                "my-project",
                "issue-plan",
                "v2",
                ".claude/skills/issue-plan/SKILL.md",
                "hash_v2",
            )
            .unwrap();

            let entry = get_skill("issue-plan:my-project").unwrap();
            assert_eq!(entry.content_hash, "hash_v2");
            assert_eq!(entry.version_history.len(), 1);
            assert_eq!(entry.version_history[0].content_hash, "hash_v1");
        });
    }

    #[test]
    fn unregister_removes_entry() {
        with_isolated_store(|| {
            register_skill("proj1", "my-project", "issue-plan", "desc", "path", "hash1").unwrap();
            let sid = skill_id("issue-plan", "my-project");
            assert!(get_skill(&sid).is_some());

            unregister_skill(&sid).unwrap();
            assert!(get_skill(&sid).is_none());
        });
    }

    #[test]
    fn unregister_project_skills_removes_all() {
        with_isolated_store(|| {
            register_skill("proj1", "project-a", "skill-1", "desc", "path1", "h1").unwrap();
            register_skill("proj1", "project-a", "skill-2", "desc", "path2", "h2").unwrap();
            register_skill("proj2", "project-b", "skill-3", "desc", "path3", "h3").unwrap();

            unregister_project_skills("proj1").unwrap();

            let skills = list_skills();
            assert_eq!(skills.len(), 1);
            assert_eq!(skills[0].project_id, "proj2");
        });
    }

    #[test]
    fn list_by_project_filters() {
        with_isolated_store(|| {
            register_skill("proj1", "project-a", "skill-1", "desc", "path1", "h1").unwrap();
            register_skill("proj2", "project-b", "skill-2", "desc", "path2", "h2").unwrap();

            let proj1_skills = list_skills_by_project("proj1");
            assert_eq!(proj1_skills.len(), 1);
            assert_eq!(proj1_skills[0].name, "skill-1");

            let proj2_skills = list_skills_by_project("proj2");
            assert_eq!(proj2_skills.len(), 1);
            assert_eq!(proj2_skills[0].name, "skill-2");
        });
    }

    #[test]
    fn parse_frontmatter_basic() {
        let content =
            "---\nname: issue-plan\ndescription: Plan GitHub issues\n---\n# Skill\nBody here.";
        let (name, desc) = parse_skill_frontmatter(content).unwrap();
        assert_eq!(name, "issue-plan");
        assert_eq!(desc, "Plan GitHub issues");
    }

    #[test]
    fn parse_frontmatter_quoted() {
        let content = "---\nname: \"my-skill\"\ndescription: 'A quoted description'\n---\n";
        let (name, desc) = parse_skill_frontmatter(content).unwrap();
        assert_eq!(name, "my-skill");
        assert_eq!(desc, "A quoted description");
    }

    #[test]
    fn parse_frontmatter_missing_fields() {
        // No name field -> should return None
        let content = "---\ndescription: only desc\n---\n";
        assert!(parse_skill_frontmatter(content).is_none());

        // No frontmatter at all
        assert!(parse_skill_frontmatter("# Just markdown").is_none());

        // Empty name
        let content = "---\nname:\ndescription: desc\n---\n";
        assert!(parse_skill_frontmatter(content).is_none());
    }

    #[test]
    fn scan_project_skills_discovers_files() {
        let tmp = tempfile::tempdir().unwrap();
        let skills_dir = tmp.path().join(".claude").join("skills");

        // Create two skill directories
        let skill_a = skills_dir.join("skill-a");
        std::fs::create_dir_all(&skill_a).unwrap();
        std::fs::write(
            skill_a.join("SKILL.md"),
            "---\nname: skill-a\ndescription: Skill A\n---\n# Skill A",
        )
        .unwrap();

        let skill_b = skills_dir.join("skill-b");
        std::fs::create_dir_all(&skill_b).unwrap();
        std::fs::write(
            skill_b.join("SKILL.md"),
            "---\nname: skill-b\ndescription: Skill B\n---\n# Skill B",
        )
        .unwrap();

        // Create a non-skill directory (no SKILL.md)
        let not_a_skill = skills_dir.join("not-a-skill");
        std::fs::create_dir_all(&not_a_skill).unwrap();
        std::fs::write(not_a_skill.join("README.md"), "not a skill").unwrap();

        let scanned = scan_project_skills(tmp.path());
        assert_eq!(scanned.len(), 2);
        assert_eq!(scanned[0].name, "skill-a");
        assert_eq!(scanned[1].name, "skill-b");
        assert!(!scanned[0].content_hash.is_empty());
    }

    #[test]
    fn scan_and_register_roundtrip() {
        with_isolated_store(|| {
            let tmp = tempfile::tempdir().unwrap();
            let skills_dir = tmp.path().join(".claude").join("skills").join("test-skill");
            std::fs::create_dir_all(&skills_dir).unwrap();
            std::fs::write(
                skills_dir.join("SKILL.md"),
                "---\nname: test-skill\ndescription: A test\n---\nBody",
            )
            .unwrap();

            let count = scan_and_register(tmp.path()).unwrap();
            assert_eq!(count, 1);

            let skills = list_skills();
            assert_eq!(skills.len(), 1);
            assert_eq!(skills[0].name, "test-skill");
            assert_eq!(skills[0].description, "A test");
        });
    }
}
