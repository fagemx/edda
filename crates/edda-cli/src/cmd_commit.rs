use edda_core::event::{new_commit_event, CommitEventParams};
use edda_derive::{build_auto_evidence, last_commit_contribution, rebuild_all};
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;
use std::collections::HashSet;
use std::path::Path;

fn parse_evidence_arg(s: &str) -> anyhow::Result<serde_json::Value> {
    if s.starts_with("evt_") {
        Ok(serde_json::json!({"event_id": s, "why": ""}))
    } else if s.starts_with("blob:sha256:") {
        Ok(serde_json::json!({"blob": s, "why": ""}))
    } else {
        anyhow::bail!("invalid evidence ref: {s} (must start with evt_ or blob:sha256:)")
    }
}

fn extract_event_id(item: &serde_json::Value) -> Option<String> {
    item.get("event_id")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

pub struct CommitCliParams<'a> {
    pub repo_root: &'a Path,
    pub title: &'a str,
    pub purpose: Option<&'a str>,
    pub contrib: Option<&'a str>,
    pub evidence_args: &'a [String],
    pub labels: Vec<String>,
    pub auto: bool,
    pub dry_run: bool,
    pub max_evidence: usize,
}

pub fn execute(p: CommitCliParams<'_>) -> anyhow::Result<()> {
    let ledger = Ledger::open(p.repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let branch = ledger.head_branch()?;

    // Parse manual evidence
    let manual_evidence: Vec<serde_json::Value> = p
        .evidence_args
        .iter()
        .map(|s| parse_evidence_arg(s))
        .collect::<anyhow::Result<Vec<_>>>()?;

    // Auto-evidence: activate when --auto or no manual evidence given
    let should_auto = p.auto || manual_evidence.is_empty();

    let mut evidence = manual_evidence.clone();
    let mut auto_preview: Vec<String> = Vec::new();

    if should_auto {
        let auto_result = build_auto_evidence(&ledger, &branch, p.max_evidence)?;

        // Dedup: collect event_ids from manual evidence
        let manual_ids: HashSet<String> = manual_evidence
            .iter()
            .filter_map(extract_event_id)
            .collect();

        for item in auto_result.items {
            if let Some(eid) = extract_event_id(&item) {
                if manual_ids.contains(&eid) {
                    continue; // skip duplicate
                }
            }
            evidence.push(item);
        }
        auto_preview = auto_result.preview_lines;
    }

    if p.dry_run {
        println!("--- Commit Preview (dry-run) ---");
        println!("Branch: {branch}");
        println!("Title: {}", p.title);
        println!("Evidence count: {}", evidence.len());
        if !auto_preview.is_empty() {
            println!("Auto-evidence picked:");
            for line in &auto_preview {
                println!("  {line}");
            }
        }
        return Ok(());
    }

    let parent_hash = ledger.last_event_hash()?;
    let prev_summary = last_commit_contribution(&ledger, &branch)?.unwrap_or_default();
    let contribution = p.contrib.unwrap_or(p.title).to_string();

    let event = new_commit_event(&mut CommitEventParams {
        branch: &branch,
        parent_hash: parent_hash.as_deref(),
        title: p.title,
        purpose: p.purpose,
        prev_summary: &prev_summary,
        contribution: &contribution,
        evidence,
        labels: p.labels,
    })?;
    ledger.append_event(&event, true)?;

    rebuild_all(&ledger)?;

    println!("Committed {} \"{}\"", event.event_id, p.title);
    if !auto_preview.is_empty() {
        println!("Auto-evidence picked ({} items):", auto_preview.len());
        for line in &auto_preview {
            println!("  {line}");
        }
    }
    Ok(())
}
