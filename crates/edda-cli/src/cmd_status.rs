use edda_derive::rebuild_branch;
use edda_ledger::Ledger;
use std::path::Path;

pub fn execute(repo_root: &Path) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let head = ledger.head_branch()?;
    let snap = rebuild_branch(&ledger, &head)?;

    println!("On branch {head}");

    if let Some(c) = &snap.last_commit {
        println!("Last commit: {} {} \"{}\"", c.ts, c.event_id, c.title);
    } else {
        println!("Last commit: (none)");
    }

    println!("Uncommitted events: {}", snap.uncommitted_events);
    Ok(())
}
