use anyhow::Context;
use std::path::Path;

// ── Background Decision Extraction ──

/// `edda bridge claude bg-review`
pub fn bg_review(
    repo_root: &Path,
    list: bool,
    accept: Option<String>,
    reject: Option<String>,
    accept_all: bool,
    session: Option<String>,
) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);

    if list {
        let drafts = edda_bridge_claude::bg_extract::list_pending_drafts(&project_id)?;
        if drafts.is_empty() {
            println!("No pending draft decisions.");
            return Ok(());
        }
        for draft in &drafts {
            println!(
                "\n── Session: {} (extracted: {}, model: {}) ──",
                draft.session_id, draft.extracted_at, draft.model
            );
            for (i, d) in draft.decisions.iter().enumerate() {
                let status_marker = match d.status {
                    edda_bridge_claude::bg_extract::DraftStatus::Pending => "⏳",
                    edda_bridge_claude::bg_extract::DraftStatus::Accepted => "✅",
                    edda_bridge_claude::bg_extract::DraftStatus::Rejected => "❌",
                };
                let kind_label = match d.kind {
                    edda_bridge_claude::bg_extract::DecisionKind::Extraction => "extraction",
                    edda_bridge_claude::bg_extract::DecisionKind::Enhancement => "enhancement",
                };
                let reason_str = d.reason.as_deref().unwrap_or("-");
                println!(
                    "  [{i}] {status_marker} [{kind_label}] {}={} (confidence: {:.0}%)",
                    d.key,
                    d.value,
                    d.confidence * 100.0
                );
                if d.kind == edda_bridge_claude::bg_extract::DecisionKind::Enhancement {
                    let orig = d.original_reason.as_deref().unwrap_or("(none)");
                    println!("      original reason: {orig}");
                    println!("      enhanced reason: {reason_str}");
                } else {
                    println!("      reason: {reason_str}");
                }
                println!("      evidence: {}", d.evidence);
            }
        }
        return Ok(());
    }

    let session_id = session
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("--session is required for accept/reject operations"))?;

    if accept_all {
        let accepted =
            edda_bridge_claude::bg_extract::accept_all_decisions(&project_id, session_id)?;
        if accepted.is_empty() {
            println!("No pending decisions to accept.");
            return Ok(());
        }
        // Write accepted decisions to workspace ledger
        write_accepted_to_ledger(repo_root, &accepted)?;
        println!("Accepted {} decisions.", accepted.len());
        return Ok(());
    }

    if let Some(indices_str) = accept {
        let indices = parse_indices(&indices_str)?;
        let accepted =
            edda_bridge_claude::bg_extract::accept_decisions(&project_id, session_id, &indices)?;
        if accepted.is_empty() {
            println!("No decisions accepted (indices may be invalid or already processed).");
            return Ok(());
        }
        write_accepted_to_ledger(repo_root, &accepted)?;
        println!("Accepted {} decisions.", accepted.len());
        return Ok(());
    }

    if let Some(indices_str) = reject {
        let indices = parse_indices(&indices_str)?;
        edda_bridge_claude::bg_extract::reject_decisions(&project_id, session_id, &indices)?;
        println!("Rejected {} decisions.", indices.len());
        return Ok(());
    }

    // Default: list if no action specified
    println!("Usage: edda bridge claude bg-review --list");
    println!("       edda bridge claude bg-review --session <sid> --accept-all");
    println!("       edda bridge claude bg-review --session <sid> --accept 0,1,2");
    println!("       edda bridge claude bg-review --session <sid> --reject 3");
    Ok(())
}

fn write_accepted_to_ledger(
    repo_root: &Path,
    decisions: &[edda_bridge_claude::bg_extract::ExtractedDecision],
) -> anyhow::Result<()> {
    let ledger = edda_ledger::Ledger::open(repo_root)
        .context("cmd_bridge::write_accepted_to_ledger: opening ledger")?;
    let _lock = edda_ledger::lock::WorkspaceLock::acquire(&ledger.paths)?;
    let branch = ledger.head_branch()?;

    for d in decisions {
        let parent_hash = ledger.last_event_hash()?;
        let payload = edda_core::types::DecisionPayload {
            key: d.key.clone(),
            value: d.value.clone(),
            reason: d.reason.clone(),
            scope: None,
            // GH-401: background-extracted decisions are machine inference from
            // transcripts — the purest agent-authored case. Tag them honestly
            // so they land in the unratified tier, not binding.
            authority: Some(edda_core::types::authority::AGENT.to_string()),
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: None,
        };

        let actor = match d.kind {
            edda_bridge_claude::bg_extract::DecisionKind::Enhancement => "edda-bg/reason-enhancer",
            edda_bridge_claude::bg_extract::DecisionKind::Extraction => {
                "edda-bg/decision-extractor"
            }
        };

        let mut event =
            edda_core::event::new_decision_event(&branch, parent_hash.as_deref(), actor, &payload)?;

        // For enhancements, supersede the original decision
        if d.kind == edda_bridge_claude::bg_extract::DecisionKind::Enhancement {
            if let Ok(Some(prior)) = ledger.find_active_decision(&branch, &d.key) {
                event.refs.provenance.push(edda_core::types::Provenance {
                    target: prior.event_id.clone(),
                    rel: edda_core::types::rel::SUPERSEDES.to_string(),
                    note: Some(format!(
                        "reason enhanced from: {}",
                        d.original_reason.as_deref().unwrap_or("(none)")
                    )),
                });
            }
        }

        ledger.append_event(&event)?;
    }

    Ok(())
}

fn parse_indices(s: &str) -> anyhow::Result<Vec<usize>> {
    s.split(',')
        .map(|part| {
            part.trim()
                .parse::<usize>()
                .map_err(|_| anyhow::anyhow!("Invalid index: {}", part.trim()))
        })
        .collect()
}
