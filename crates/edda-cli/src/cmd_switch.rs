use edda_core::event::new_branch_switch_event;
use edda_derive::rebuild_all;
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;
use std::path::Path;

pub fn execute(repo_root: &Path, name: &str) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let from = ledger.head_branch()?;

    if from == name {
        println!("Already on branch {name}");
        return Ok(());
    }

    // Check target branch exists
    let branch_dir = ledger.paths.branch_dir(name);
    if !branch_dir.exists() {
        anyhow::bail!("branch does not exist: {name}");
    }

    let parent_hash = ledger.last_event_hash()?;

    // Write switch event on target branch
    let event = new_branch_switch_event(name, parent_hash.as_deref(), &from, name)?;
    ledger.append_event(&event)?;

    ledger.set_head_branch(name)?;

    rebuild_all(&ledger)?;

    println!("Switched to branch {name}");
    Ok(())
}
