use edda_core::event::new_note_event;
use edda_derive::rebuild_all;
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::paths::EddaPaths;
use edda_ledger::{ledger, Ledger};
use std::path::Path;

pub fn execute(repo_root: &Path, no_hooks: bool) -> anyhow::Result<()> {
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
";
            std::fs::write(&policy_path, default_policy.as_bytes())?;
        }

        // Generate default actors.yaml
        let actors_path = paths.edda_dir.join("actors.yaml");
        if !actors_path.exists() {
            let default_actors = "\
version: 1
actors: {}
";
            std::fs::write(&actors_path, default_actors.as_bytes())?;
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

    // Auto-detect and install bridge hooks (unless --no-hooks)
    if !no_hooks {
        auto_install_bridges(repo_root);
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
        let tmp = temp_dir();
        std::fs::create_dir_all(tmp.join(".claude")).unwrap();

        execute(&tmp, false).unwrap();

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
        let tmp = temp_dir();
        std::fs::create_dir_all(tmp.join(".claude")).unwrap();

        execute(&tmp, true).unwrap();

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
    fn init_without_claude_dir_no_error() {
        let tmp = temp_dir();

        execute(&tmp, false).unwrap();

        // Workspace created
        assert!(tmp.join(".edda").is_dir());
        // No Claude artifacts
        assert!(!tmp.join(".claude").join("settings.local.json").exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn reinit_also_installs_hooks() {
        let tmp = temp_dir();

        // First init — no .claude/ dir
        execute(&tmp, false).unwrap();
        assert!(tmp.join(".edda").is_dir());
        assert!(!tmp.join(".claude").join("settings.local.json").exists());

        // Now add .claude/ and re-init
        std::fs::create_dir_all(tmp.join(".claude")).unwrap();
        execute(&tmp, false).unwrap();

        // Hooks should now be installed
        let settings = tmp.join(".claude").join("settings.local.json");
        assert!(settings.exists(), "hooks should be installed on re-init");
        let content = std::fs::read_to_string(&settings).unwrap();
        assert!(content.contains("edda hook claude"));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
