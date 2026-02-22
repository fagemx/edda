use edda_core::event::new_note_event;
use edda_derive::rebuild_all;
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::paths::EddaPaths;
use edda_ledger::{ledger, Ledger};
use std::path::Path;

pub fn execute(repo_root: &Path) -> anyhow::Result<()> {
    let paths = EddaPaths::discover(repo_root);

    if paths.is_initialized() {
        // Ensure schema and HEAD exist even if .edda/ dir was partially created
        ledger::init_workspace(&paths)?;
        ledger::init_head(&paths, "main")?;
        println!("Already initialized at {}", paths.edda_dir.display());
        return Ok(());
    }

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
    Ok(())
}
