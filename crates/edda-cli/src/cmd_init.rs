use edda_core::event::new_note_event;
use edda_derive::rebuild_all;
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::paths::EddaPaths;
use edda_ledger::{ledger, Ledger};
use std::path::Path;

/// Embedded project skill templates — canonical versions that ship with the binary.
const SKILLS: &[(&str, &str)] = &[
    ("coord-sync", include_str!("skills/coord-sync.md")),
    ("coord-handoff", include_str!("skills/coord-handoff.md")),
    ("coord-request", include_str!("skills/coord-request.md")),
    ("coord-review", include_str!("skills/coord-review.md")),
    (
        "coord-orchestrate",
        include_str!("skills/coord-orchestrate.md"),
    ),
];

pub fn execute(repo_root: &Path, no_hooks: bool, force_skills: bool) -> anyhow::Result<()> {
    let paths = EddaPaths::discover(repo_root);

    if paths.is_initialized() {
        // Ensure schema and HEAD exist even if .edda/ dir was partially created
        ledger::init_workspace(&paths)?;
        ledger::init_head(&paths, "main")?;
        println!("Already initialized at {}", paths.edda_dir.display());
    } else {
        // Create directory layout
        ledger::init_workspace(&paths)?;
        ledger::init_head(&paths, "main")?;
        ledger::init_branches_json(&paths, "main")?;

        // Generate default policy.yaml (v2)
        let policy_path = paths.edda_dir.join("policy.yaml");
        if !policy_path.exists() {
            let default_policy = "\
version: 2
roles:
  - lead
  - reviewer
rules:
  - id: require
    when:
      labels_any: [\"risk\", \"security\", \"prod\"]
      failed_cmd: true
      evidence_count_gte: 15
    stages:
      - stage_id: lead
        role: lead
        min_approvals: 1
        max_assignees: 2
  - id: default
    when:
      default: true
    stages: []
# permissions:
#   default: deny
#   grants:
#     - actions: [deploy, merge]
#       roles: [lead]
#     - actions: [read]
#       roles: [\"*\"]
";
            std::fs::write(&policy_path, default_policy.as_bytes())?;
        }

        // Generate default actors.yaml
        let actors_path = paths.edda_dir.join("actors.yaml");
        if !actors_path.exists() {
            let default_actors = "\
version: 2
actors: {}
# Example:
#   alice:
#     kind: user
#     roles: [lead, reviewer]
#     email: alice@example.com
#   claude-agent:
#     kind: agent
#     roles: [operator]
#     runtime: claude
";
            std::fs::write(&actors_path, default_actors.as_bytes())?;
        }

        // Generate default tool_tiers.yaml
        let tier_path = paths.edda_dir.join("tool_tiers.yaml");
        if !tier_path.exists() {
            let default_config = edda_core::tool_tier::default_tool_tier_config();
            edda_core::tool_tier::save_tool_tiers_to_dir(&paths.edda_dir, &default_config)?;
        }

        // Open ledger and write the init event
        let ledger = Ledger::open(repo_root)?;
        let _lock = WorkspaceLock::acquire(&ledger.paths)?;

        let parent_hash = ledger.last_event_hash()?;
        let event = new_note_event(
            "main",
            parent_hash.as_deref(),
            "system",
            "init edda workspace",
            &[],
        )?;
        ledger.append_event(&event)?;

        // Build derived views immediately so they're available right after init
        rebuild_all(&ledger)?;

        println!("Initialized .edda/ (HEAD=main)");
        println!("  {}", event.event_id);
    }

    // Register in user-level project registry (~/.edda/registry.json)
    if let Err(e) = edda_store::registry::register_project(repo_root) {
        eprintln!("Warning: failed to register project: {e}");
    }

    // Auto-detect and install bridge hooks (unless --no-hooks)
    if !no_hooks {
        auto_install_bridges(repo_root);
    }

    // Scaffold project skills for detected agent hosts.
    if repo_root.join(".claude").is_dir() {
        scaffold_skills(&repo_root.join(".claude").join("skills"), force_skills);
    }
    if repo_root.join("AGENTS.md").is_file() || repo_root.join(".agents").is_dir() {
        scaffold_skills(&repo_root.join(".agents").join("skills"), force_skills);
    }

    // Scan and register skills in the user-level skill registry
    match edda_store::skill_registry::scan_and_register(repo_root) {
        Ok(0) => {} // no skills found, no message
        Ok(n) => println!("Registered {n} skill(s) in skill registry."),
        Err(e) => eprintln!("Warning: skill scan failed: {e}"),
    }

    Ok(())
}

/// Detect known agent platforms and install repo-local hooks automatically.
fn auto_install_bridges(repo_root: &Path) {
    // Claude Code: repo-local hooks — safe to auto-install
    if repo_root.join(".claude").is_dir() {
        println!("Detected Claude Code project, installing hooks...");
        match edda_bridge_claude::install(repo_root, false) {
            Ok(()) => {} // install() already prints its own success messages
            Err(e) => eprintln!("Warning: Claude hook install failed: {e}"),
        }
    }

    // OpenClaw: global plugin (~/.openclaw/extensions/) — hint only, don't auto-install
    if repo_root.join(".openclaw").is_dir() {
        println!("Detected OpenClaw project. Run 'edda setup openclaw' to enable edda hooks.");
    }
}

/// Write embedded skill templates to `<host>/skills/<name>/SKILL.md`.
/// Skips files that already exist unless `force` is true.
fn scaffold_skills(skills_dir: &Path, force: bool) {
    for &(name, content) in SKILLS {
        let dir = skills_dir.join(name);
        let path = dir.join("SKILL.md");
        if path.exists() && !force {
            continue;
        }
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("Warning: could not create {}: {e}", dir.display());
            continue;
        }
        let already_existed = path.exists();
        if let Err(e) = std::fs::write(&path, content) {
            eprintln!("Warning: could not write {}: {e}", path.display());
            continue;
        }
        if already_existed {
            println!("  Updated skill: {name}");
        } else {
            println!("  Scaffolded skill: {name}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> std::path::PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("edda_init_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        tmp
    }

    #[test]
    fn init_detects_claude_and_installs_hooks() {
        let _store = crate::test_support::isolated_store();
        let tmp = temp_dir();
        std::fs::create_dir_all(tmp.join(".claude")).unwrap();

        execute(&tmp, false, false).unwrap();

        // Workspace created
        assert!(tmp.join(".edda").is_dir());
        // Claude hooks installed
        let settings = tmp.join(".claude").join("settings.local.json");
        assert!(settings.exists(), "settings.local.json should exist");
        let content = std::fs::read_to_string(&settings).unwrap();
        assert!(
            content.contains("edda hook claude"),
            "should contain edda hook"
        );
        // MCP server configured
        assert!(
            content.contains("mcpServers"),
            "MCP server config should exist"
        );
        assert!(
            content.contains(r#""command": "edda""#),
            "edda MCP server should be configured"
        );
        // CLAUDE.md created
        assert!(tmp.join(".claude").join("CLAUDE.md").exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn init_no_hooks_skips_bridge() {
        let _store = crate::test_support::isolated_store();
        let tmp = temp_dir();
        std::fs::create_dir_all(tmp.join(".claude")).unwrap();

        execute(&tmp, true, false).unwrap();

        // Workspace created
        assert!(tmp.join(".edda").is_dir());
        // Hooks NOT installed
        assert!(
            !tmp.join(".claude").join("settings.local.json").exists(),
            "settings.local.json should not exist with --no-hooks"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn init_without_detected_host_projects_no_skills() {
        let _store = crate::test_support::isolated_store();
        let tmp = temp_dir();

        execute(&tmp, false, false).unwrap();

        assert!(tmp.join(".edda").is_dir());
        assert!(!tmp.join(".claude").join("settings.local.json").exists());
        assert!(!tmp.join(".claude").join("skills").exists());
        assert!(!tmp.join(".agents").join("skills").exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn reinit_also_installs_hooks() {
        let _store = crate::test_support::isolated_store();
        let tmp = temp_dir();

        // First init — no .claude/ dir
        execute(&tmp, false, false).unwrap();
        assert!(tmp.join(".edda").is_dir());
        assert!(!tmp.join(".claude").join("settings.local.json").exists());

        // Now add .claude/ and re-init
        std::fs::create_dir_all(tmp.join(".claude")).unwrap();
        execute(&tmp, false, false).unwrap();

        // Hooks should now be installed
        let settings = tmp.join(".claude").join("settings.local.json");
        assert!(settings.exists(), "hooks should be installed on re-init");
        let content = std::fs::read_to_string(&settings).unwrap();
        assert!(content.contains("edda hook claude"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn init_scaffolds_canonical_project_skills_for_claude() {
        let _store = crate::test_support::isolated_store();
        let tmp = temp_dir();
        std::fs::create_dir_all(tmp.join(".claude")).unwrap();

        execute(&tmp, true, false).unwrap();

        for &(name, canonical) in SKILLS {
            let path = tmp
                .join(".claude")
                .join("skills")
                .join(name)
                .join("SKILL.md");
            assert!(path.exists(), "{name}/SKILL.md should be scaffolded");
            let content = std::fs::read_to_string(&path).unwrap();
            assert_eq!(
                content, canonical,
                "{name}/SKILL.md must equal its embedded canonical source"
            );
            assert!(
                content.lines().any(|line| line == format!("name: {name}")),
                "{name} should have exact matching frontmatter"
            );

            if name.starts_with("coord-") {
                // GH-1063: these files are scaffolded into every new project,
                // so whatever they say about binding authority is what every
                // future session in that project learns. Assert against the
                // scaffolded file on disk, not only the source constant.
                //
                // The superseded phrase is spelled in two pieces on purpose:
                // the acceptance sweep greps `crates/` for it, and a guard
                // that spelled it out would itself be the hit it prevents.
                let operator_only = concat!("until an operator ", "ratifies");
                assert!(
                    !content.contains(operator_only),
                    "{name} still teaches operator-only binding authority"
                );
                if matches!(name, "coord-sync" | "coord-review") {
                    assert!(
                        content.contains("edda ratify --by-rule cited-authority"),
                        "{name} must name the cited-authority rule sweep — the second \
                         binding path (decision.auto-ratify; PR #1016)"
                    );
                }
            }
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn init_scaffolds_canonical_project_skills_for_codex() {
        let _store = crate::test_support::isolated_store();
        let tmp = temp_dir();
        std::fs::write(tmp.join("AGENTS.md"), "# Project instructions\n").unwrap();

        execute(&tmp, true, false).unwrap();

        for &(name, canonical) in SKILLS {
            let path = tmp
                .join(".agents")
                .join("skills")
                .join(name)
                .join("SKILL.md");
            assert!(
                path.exists(),
                "{name}/SKILL.md should be scaffolded for Codex"
            );
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                canonical,
                "{name}/SKILL.md must equal its embedded canonical source"
            );
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn init_preserves_every_embedded_customization_for_both_hosts() {
        let _store = crate::test_support::isolated_store();
        let tmp = temp_dir();
        std::fs::write(tmp.join("AGENTS.md"), "# Project instructions\n").unwrap();

        for host in [".claude", ".agents"] {
            for &(name, _) in SKILLS {
                let skill = tmp.join(host).join("skills").join(name).join("SKILL.md");
                std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
                std::fs::write(&skill, format!("custom {host} {name}")).unwrap();
            }
        }

        execute(&tmp, true, false).unwrap();

        for host in [".claude", ".agents"] {
            for &(name, _) in SKILLS {
                let skill = tmp.join(host).join("skills").join(name).join("SKILL.md");
                assert_eq!(
                    std::fs::read_to_string(skill).unwrap(),
                    format!("custom {host} {name}"),
                    "default init must preserve {host} {name}"
                );
            }
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn tracked_orchestration_projection_matches_embedded_source() {
        let embedded = include_str!("skills/coord-orchestrate.md");
        let tracked = include_str!("../../../.claude/skills/coord-orchestrate/SKILL.md");
        assert_eq!(tracked, embedded);
        assert!(embedded.contains("delivery-flow/1"));
    }

    #[test]
    fn init_force_skills_overwrites_every_embedded_skill_for_both_hosts() {
        let _store = crate::test_support::isolated_store();
        let tmp = temp_dir();
        std::fs::write(tmp.join("AGENTS.md"), "# Project instructions\n").unwrap();

        for host in [".claude", ".agents"] {
            for &(name, _) in SKILLS {
                let skill = tmp.join(host).join("skills").join(name).join("SKILL.md");
                std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
                std::fs::write(skill, format!("custom {host} {name}")).unwrap();
            }
        }

        execute(&tmp, true, true).unwrap();

        for host in [".claude", ".agents"] {
            for &(name, expected) in SKILLS {
                let skill = tmp.join(host).join("skills").join(name).join("SKILL.md");
                assert_eq!(
                    std::fs::read_to_string(skill).unwrap(),
                    expected,
                    "force must overwrite every embedded entry: {host} {name}"
                );
            }
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
