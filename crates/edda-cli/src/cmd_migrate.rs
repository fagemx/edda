use edda_ledger::lock::WorkspaceLock;
use edda_ledger::migrate::{migrate_jsonl_to_sqlite, MigrateOptions};
use edda_ledger::paths::EddaPaths;
use std::path::Path;

pub fn execute(repo_root: &Path, dry_run: bool, no_verify: bool) -> anyhow::Result<()> {
    let paths = EddaPaths::discover(repo_root);
    let _lock = WorkspaceLock::acquire(&paths)?;

    let opts = MigrateOptions {
        verify: !no_verify,
        dry_run,
    };

    let report = migrate_jsonl_to_sqlite(&paths, &opts)?;

    if dry_run {
        println!("Dry run — no changes made.\n");
        println!("Would migrate:");
        println!("  {} events from events.jsonl", report.events_migrated);
        println!("  {} decision events", report.decisions_found);
        println!("  HEAD: {}", report.head_branch);
        println!("  {} branch(es)", report.branches_count);
    } else {
        println!("Migration complete:");
        println!("  Events: {}", report.events_migrated);
        println!("  Decisions: {}", report.decisions_found);
        println!("  HEAD: {}", report.head_branch);
        println!("  Branches: {}", report.branches_count);
        if opts.verify {
            println!("  Verification: passed");
        }
        println!("\nOriginal JSONL files kept. Future `edda` commands will use SQLite.");
    }

    Ok(())
}
