use edda_core::event::new_note_event;
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;
use std::path::Path;

pub fn execute(repo_root: &Path, text: &str, role: &str, tags: &[String]) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;

    let event = new_note_event(&branch, parent_hash.as_deref(), role, text, tags)?;
    ledger.append_event(&event, false)?;

    println!("Wrote NOTE {}", event.event_id);
    Ok(())
}
