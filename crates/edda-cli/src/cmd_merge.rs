use edda_core::event::new_merge_event;
use edda_derive::rebuild_all;
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;
use std::collections::HashSet;
use std::path::Path;

fn collect_commit_ids(ledger: &Ledger, branch: &str) -> anyhow::Result<Vec<String>> {
    let mut ids = Vec::new();
    for ev in ledger.iter_events()? {
        if ev.branch == branch && ev.event_type == "commit" {
            ids.push(ev.event_id.clone());
        }
    }
    Ok(ids)
}

pub fn execute(
    repo_root: &Path,
    src: &str,
    dst: &str,
    reason: &str,
) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let head = ledger.head_branch()?;
    if head != dst {
        anyhow::bail!("merge dst must equal HEAD (HEAD={head}, dst={dst})");
    }

    // Check both branches exist
    if !ledger.paths.branch_dir(src).exists() {
        anyhow::bail!("branch does not exist: {src}");
    }
    if !ledger.paths.branch_dir(dst).exists() {
        anyhow::bail!("branch does not exist: {dst}");
    }

    let src_commits = collect_commit_ids(&ledger, src)?;
    let dst_commits: HashSet<String> = collect_commit_ids(&ledger, dst)?.into_iter().collect();

    // Adopted = src commits not in dst (preserve order)
    let adopted: Vec<String> = src_commits
        .into_iter()
        .filter(|id| !dst_commits.contains(id))
        .collect();

    let parent_hash = ledger.last_event_hash()?;

    let event = new_merge_event(dst, parent_hash.as_deref(), src, dst, reason, &adopted)?;
    ledger.append_event(&event, true)?;

    rebuild_all(&ledger)?;

    println!(
        "Merged {src} -> {dst} (adopted {} commits)",
        adopted.len()
    );
    println!("  {}", event.event_id);
    Ok(())
}
