use edda_core::event::new_rebuild_event;
use edda_derive::{rebuild_all, rebuild_branch};
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;
use std::path::Path;

pub fn execute(
    repo_root: &Path,
    branch: Option<&str>,
    all: bool,
    reason: &str,
) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let head = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;

    if all {
        let event = new_rebuild_event(&head, parent_hash.as_deref(), "all", None, reason)?;
        ledger.append_event(&event)?;

        let snaps = rebuild_all(&ledger)?;
        println!("Rebuilt views for all branches ({} branches).", snaps.len());
    } else {
        let target = branch.unwrap_or(&head);
        let event = new_rebuild_event(
            &head,
            parent_hash.as_deref(),
            "branch",
            Some(target),
            reason,
        )?;
        ledger.append_event(&event)?;

        rebuild_branch(&ledger, target)?;
        println!("Rebuilt views for {target}.");
    }

    Ok(())
}
