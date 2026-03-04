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
    ledger.append_event(&event)?;

    println!("Wrote NOTE {}", event.event_id);
    Ok(())
}

pub fn reclassify_activity(
    repo_root: &std::path::Path,
    session_id: &str,
    activity: &str,
) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    // Validate activity type
    let activity_lower = activity.to_lowercase();
    let valid = [
        "feature", "fix", "debug", "refactor", "docs", "research", "chat", "ops", "unknown",
    ];
    if !valid.contains(&activity_lower.as_str()) {
        anyhow::bail!(
            "Invalid activity type '{}'. Valid types: {:?}",
            activity,
            valid
        );
    }

    // Find session digest event by session_id prefix match
    let events = ledger.iter_events()?;
    let _target_event = events
        .iter()
        .rev()
        .find(|e| {
            let is_digest = e
                .payload
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().any(|t| t.as_str() == Some("session_digest")))
                .unwrap_or(false);

            let matches_session = e
                .payload
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(|sid| sid.starts_with(session_id))
                .unwrap_or(false);

            is_digest && matches_session
        })
        .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id))?;

    // Note: In a full implementation, we would need to create a new event with the updated
    // activity and properly update the ledger. For now, we just validate and report success.
    // The actual mutation of immutable events requires careful ledger handling.

    println!(
        "Session {} activity would be updated to: {}",
        session_id, activity_lower
    );
    println!("Note: Full reclassify implementation requires ledger mutation support");

    Ok(())
}
