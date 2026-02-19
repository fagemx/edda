use edda_core::event::{new_branch_create_event, new_note_event};
use edda_derive::{rebuild_all, rebuild_branch};
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;
use std::path::Path;

fn validate_branch_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.len() > 64 {
        anyhow::bail!("invalid branch name: must be 1-64 characters");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' || c == '/')
    {
        anyhow::bail!("invalid branch name: only [A-Za-z0-9._-/] allowed");
    }
    Ok(())
}

pub fn create(repo_root: &Path, name: &str, purpose: &str) -> anyhow::Result<()> {
    validate_branch_name(name)?;

    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let head = ledger.head_branch()?;

    // Check branch doesn't already exist
    let branch_dir = ledger.paths.branch_dir(name);
    if branch_dir.exists() {
        anyhow::bail!("branch already exists: {name}");
    }

    // Get from_event_id from HEAD's last event
    let head_snap = rebuild_branch(&ledger, &head)?;
    let from_event_id = head_snap.last_event_id.as_deref();

    let parent_hash = ledger.last_event_hash()?;

    // Write branch_create event on HEAD branch
    let create_event = new_branch_create_event(
        &head,
        parent_hash.as_deref(),
        name,
        purpose,
        &head,
        from_event_id,
    )?;
    ledger.append_event(&create_event, true)?;

    // Seed target branch with a system note
    let parent_hash = ledger.last_event_hash()?;
    let seed_text = format!("branch created from {head} purpose=\"{purpose}\"");
    let seed_event = new_note_event(
        name,
        parent_hash.as_deref(),
        "system",
        &seed_text,
        &["branch".to_string()],
    )?;
    ledger.append_event(&seed_event, true)?;

    rebuild_all(&ledger)?;

    println!("Created branch {name} from {head}");
    println!("  {}", create_event.event_id);
    Ok(())
}
