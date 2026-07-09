use edda_core::event::new_note_event;
use edda_core::secret_guard::redact;
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;
use std::path::Path;

pub fn execute(repo_root: &Path, text: &str, role: &str, tags: &[String]) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;

    // EDDA-SECRET-GUARD1 q331: scrub known secret shapes before persisting.
    // Deterministic, zero-LLM; hits reported to stderr so operator sees what happened.
    let (safe_text, hits) = redact(text);
    if !hits.is_empty() {
        eprintln!(
            "⚠ secret-guard: redacted {n} secret pattern(s) before writing NOTE ({kinds})",
            n = hits.len(),
            kinds = hits.iter().map(|h| h.kind).collect::<Vec<_>>().join(", ")
        );
    }

    let event = new_note_event(&branch, parent_hash.as_deref(), role, &safe_text, tags)?;
    ledger.append_event(&event)?;

    println!("Wrote NOTE {}", event.event_id);

    // Refresh derived markdown views (log.md / main.md / commit.md) so operators
    // reading the ledger by eye see the note immediately, not only after the
    // next `edda commit` / `edda rebuild`. Same best-effort pattern as
    // edda-serve::api::drafts.rs:508 — failure never blocks a successful write.
    let _ = edda_derive::rebuild_branch(&ledger, &branch);

    Ok(())
}
