//! `edda sync` — pull shared decisions from group members.

use edda_ledger::sync::SyncSource;
use std::path::Path;

/// Build sync sources from registry group members.
fn sources_from_group(repo_root: &Path) -> Vec<SyncSource> {
    edda_store::registry::list_group_members(repo_root)
        .into_iter()
        .map(|entry| SyncSource {
            project_id: entry.project_id,
            project_name: entry.name,
            ledger_path: std::path::PathBuf::from(&entry.path),
        })
        .collect()
}

/// Build sync sources from a specific project name in the registry.
fn sources_from_name(name: &str) -> Vec<SyncSource> {
    edda_store::registry::list_projects()
        .into_iter()
        .filter(|p| p.name == name)
        .map(|entry| SyncSource {
            project_id: entry.project_id,
            project_name: entry.name,
            ledger_path: std::path::PathBuf::from(&entry.path),
        })
        .collect()
}

pub fn execute(repo_root: &Path, from: Option<&str>, dry_run: bool) -> anyhow::Result<()> {
    let ledger = edda_ledger::Ledger::open(repo_root)?;
    let target_project_id = edda_store::project_id(repo_root);

    let sources = if let Some(name) = from {
        let sources = sources_from_name(name);
        if sources.is_empty() {
            anyhow::bail!("no registered project named '{name}'");
        }
        sources
    } else {
        let sources = sources_from_group(repo_root);
        if sources.is_empty() {
            let group = edda_store::registry::project_group(repo_root);
            if group.is_none() {
                anyhow::bail!("this project has no group. Use `edda group set <name>` first.");
            }
            println!("No group members found.");
            return Ok(());
        }
        sources
    };

    if dry_run {
        println!("Dry run: showing what would be imported.\n");
    }

    let result =
        edda_ledger::sync::sync_from_sources(&ledger, &sources, &target_project_id, dry_run)?;

    if !result.errors.is_empty() {
        eprintln!("Warnings ({}):", result.errors.len());
        for e in &result.errors {
            eprintln!("  {}: {}", e.project_name, e.error);
        }
        eprintln!();
    }

    if result.imported.is_empty() && result.conflicts.is_empty() {
        println!("Already up to date ({} skipped).", result.skipped);
        return Ok(());
    }

    if !result.imported.is_empty() {
        let verb = if dry_run { "Would import" } else { "Imported" };
        println!("{verb} {} decision(s):", result.imported.len());
        for d in &result.imported {
            println!("  {} = {} (from {})", d.key, d.value, d.source_project);
        }
    }

    if !result.conflicts.is_empty() {
        println!("\nConflicts ({}):", result.conflicts.len());
        for c in &result.conflicts {
            println!(
                "  {}: local={}, remote={} (from {})",
                c.key, c.local_value, c.remote_value, c.source_project
            );
        }
        if !dry_run {
            println!("  Conflicting decisions imported as inactive. Resolve manually.");
        }
    }

    if result.skipped > 0 {
        println!("\n{} already imported (skipped).", result.skipped);
    }

    Ok(())
}
