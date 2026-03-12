//! Issue proposal workflow — drafts issue proposals for human approval before
//! calling `gh issue create`.
//!
//! Storage pattern mirrors `bg_scan`: JSON files in `state/issue_proposals/`,
//! audit log in `state/issue_proposal_audit.jsonl`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

// ── Data Structures ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProposalSource {
    Scan,
    Postmortem,
    Manual,
    Bridge,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProposalStatus {
    #[default]
    Pending,
    Approved,
    Dismissed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueProposal {
    pub proposal_id: String,
    pub created_at: String,
    pub source: ProposalSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub status: ProposalStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismiss_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuditEntry {
    ts: String,
    proposal_id: String,
    action: String,
    actor: Option<String>,
    detail: Option<String>,
}

// ── Public API ──

/// Generate a new proposal ID.
pub fn new_proposal_id() -> String {
    format!(
        "prop_{}",
        &ulid::Ulid::new().to_string()[..12].to_lowercase()
    )
}

/// Save an issue proposal to disk.
pub fn save_proposal(project_id: &str, proposal: &IssueProposal) -> Result<()> {
    let path = proposal_path(project_id, &proposal.proposal_id);
    fs::create_dir_all(path.parent().unwrap())?;
    let json = serde_json::to_string_pretty(proposal)?;
    fs::write(&path, json)?;

    append_audit(
        project_id,
        &proposal.proposal_id,
        "created",
        None,
        Some(&proposal.title),
    )?;

    Ok(())
}

/// Load a single proposal by ID.
pub fn load_proposal(project_id: &str, proposal_id: &str) -> Result<IssueProposal> {
    let path = proposal_path(project_id, proposal_id);
    let content =
        fs::read_to_string(&path).with_context(|| format!("Proposal not found: {proposal_id}"))?;
    let proposal: IssueProposal = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse proposal: {}", path.display()))?;
    Ok(proposal)
}

/// List all proposals, optionally filtered by status.
pub fn list_proposals(
    project_id: &str,
    status_filter: Option<&ProposalStatus>,
) -> Result<Vec<IssueProposal>> {
    let dir = proposals_dir(project_id);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut results = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let content = fs::read_to_string(&path)?;
        let proposal: IssueProposal = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse proposal: {}", path.display()))?;

        if let Some(filter) = status_filter {
            if &proposal.status != filter {
                continue;
            }
        }
        results.push(proposal);
    }

    results.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(results)
}

/// Approve a proposal: mark as approved and return it.
pub fn approve_proposal(project_id: &str, proposal_id: &str, by: &str) -> Result<IssueProposal> {
    let mut proposal = load_proposal(project_id, proposal_id)?;
    if proposal.status != ProposalStatus::Pending {
        anyhow::bail!(
            "Proposal {proposal_id} is not pending (status: {:?})",
            proposal.status
        );
    }

    proposal.status = ProposalStatus::Approved;
    proposal.approved_by = Some(by.to_string());
    proposal.approved_at = Some(now_rfc3339());

    let path = proposal_path(project_id, proposal_id);
    let json = serde_json::to_string_pretty(&proposal)?;
    fs::write(&path, json)?;

    append_audit(project_id, proposal_id, "approved", Some(by), None)?;

    Ok(proposal)
}

/// Record issue creation result on an approved proposal.
pub fn record_issue_created(
    project_id: &str,
    proposal_id: &str,
    issue_url: &str,
    issue_number: u64,
) -> Result<()> {
    let mut proposal = load_proposal(project_id, proposal_id)?;
    if proposal.status != ProposalStatus::Approved {
        anyhow::bail!(
            "Proposal {proposal_id} is not approved (status: {:?})",
            proposal.status
        );
    }
    proposal.issue_url = Some(issue_url.to_string());
    proposal.issue_number = Some(issue_number);

    let path = proposal_path(project_id, proposal_id);
    let json = serde_json::to_string_pretty(&proposal)?;
    fs::write(&path, json)?;

    append_audit(
        project_id,
        proposal_id,
        "issue_created",
        None,
        Some(issue_url),
    )?;

    Ok(())
}

/// Dismiss a proposal with optional reason.
pub fn dismiss_proposal(project_id: &str, proposal_id: &str, reason: Option<&str>) -> Result<()> {
    let mut proposal = load_proposal(project_id, proposal_id)?;
    if proposal.status != ProposalStatus::Pending {
        anyhow::bail!(
            "Proposal {proposal_id} is not pending (status: {:?})",
            proposal.status
        );
    }

    proposal.status = ProposalStatus::Dismissed;
    proposal.dismiss_reason = reason.map(String::from);

    let path = proposal_path(project_id, proposal_id);
    let json = serde_json::to_string_pretty(&proposal)?;
    fs::write(&path, json)?;

    append_audit(project_id, proposal_id, "dismissed", None, reason)?;

    Ok(())
}

/// Create a proposal from a scan gap.
pub fn create_proposal_from_scan_gap(
    project_id: &str,
    scan_id: &str,
    index: usize,
) -> Result<IssueProposal> {
    let scan = crate::bg_scan::load_scan(project_id, scan_id)?;
    if index >= scan.gaps.len() {
        anyhow::bail!(
            "Gap index {index} out of range (scan has {} gaps)",
            scan.gaps.len()
        );
    }

    let gap = &scan.gaps[index];
    let mut body_parts = vec![gap.description.clone()];

    if !gap.evidence.is_empty() {
        body_parts.push(String::new());
        body_parts.push("## Evidence".to_string());
        for ev in &gap.evidence {
            body_parts.push(format!("- {ev}"));
        }
    }

    body_parts.push(String::new());
    body_parts.push(format!(
        "_Generated by `edda scan` (confidence: {:.0}%, scan: {})_",
        gap.confidence * 100.0,
        scan_id
    ));

    let proposal = IssueProposal {
        proposal_id: new_proposal_id(),
        created_at: now_rfc3339(),
        source: ProposalSource::Scan,
        source_ref: Some(format!("{scan_id}:{index}")),
        title: gap.title.clone(),
        body: body_parts.join("\n"),
        labels: gap.suggested_labels.clone(),
        status: ProposalStatus::Pending,
        approved_by: None,
        approved_at: None,
        issue_url: None,
        issue_number: None,
        dismiss_reason: None,
    };

    save_proposal(project_id, &proposal)?;
    Ok(proposal)
}

// ── Storage Helpers ──

fn proposals_dir(project_id: &str) -> PathBuf {
    edda_store::project_dir(project_id)
        .join("state")
        .join("issue_proposals")
}

fn proposal_path(project_id: &str, proposal_id: &str) -> PathBuf {
    proposals_dir(project_id).join(format!("{proposal_id}.json"))
}

fn audit_log_path(project_id: &str) -> PathBuf {
    edda_store::project_dir(project_id)
        .join("state")
        .join("issue_proposal_audit.jsonl")
}

fn append_audit(
    project_id: &str,
    proposal_id: &str,
    action: &str,
    actor: Option<&str>,
    detail: Option<&str>,
) -> Result<()> {
    use std::io::Write;
    let path = audit_log_path(project_id);
    fs::create_dir_all(path.parent().unwrap())?;
    let entry = AuditEntry {
        ts: now_rfc3339(),
        proposal_id: proposal_id.to_string(),
        action: action.to_string(),
        actor: actor.map(String::from),
        detail: detail.map(String::from),
    };
    let line = serde_json::to_string(&entry)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(file, "{}", line)?;
    Ok(())
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposal_serde_roundtrip() {
        let proposal = IssueProposal {
            proposal_id: "prop_test123".to_string(),
            created_at: "2026-03-12T10:00:00Z".to_string(),
            source: ProposalSource::Manual,
            source_ref: None,
            title: "Test issue".to_string(),
            body: "This is a test".to_string(),
            labels: vec!["enhancement".to_string()],
            status: ProposalStatus::Pending,
            approved_by: None,
            approved_at: None,
            issue_url: None,
            issue_number: None,
            dismiss_reason: None,
        };
        let json = serde_json::to_string(&proposal).unwrap();
        let parsed: IssueProposal = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.proposal_id, "prop_test123");
        assert_eq!(parsed.title, "Test issue");
        assert_eq!(parsed.status, ProposalStatus::Pending);
        assert_eq!(parsed.source, ProposalSource::Manual);
    }

    #[test]
    fn proposal_status_transitions() {
        let pid = "test_proposal_status";
        let _ = edda_store::ensure_dirs(pid);

        // Create a pending proposal
        let proposal = IssueProposal {
            proposal_id: "prop_status1".to_string(),
            created_at: "2026-03-12T10:00:00Z".to_string(),
            source: ProposalSource::Manual,
            source_ref: None,
            title: "Approve me".to_string(),
            body: "body".to_string(),
            labels: vec![],
            status: ProposalStatus::Pending,
            approved_by: None,
            approved_at: None,
            issue_url: None,
            issue_number: None,
            dismiss_reason: None,
        };
        save_proposal(pid, &proposal).unwrap();

        // Approve
        let approved = approve_proposal(pid, "prop_status1", "alice").unwrap();
        assert_eq!(approved.status, ProposalStatus::Approved);
        assert_eq!(approved.approved_by.as_deref(), Some("alice"));
        assert!(approved.approved_at.is_some());

        // Cannot approve again
        assert!(approve_proposal(pid, "prop_status1", "bob").is_err());

        // Create another proposal to test dismiss
        let proposal2 = IssueProposal {
            proposal_id: "prop_status2".to_string(),
            created_at: "2026-03-12T10:01:00Z".to_string(),
            source: ProposalSource::Scan,
            source_ref: Some("scan_abc:0".to_string()),
            title: "Dismiss me".to_string(),
            body: "body".to_string(),
            labels: vec![],
            status: ProposalStatus::Pending,
            approved_by: None,
            approved_at: None,
            issue_url: None,
            issue_number: None,
            dismiss_reason: None,
        };
        save_proposal(pid, &proposal2).unwrap();

        // Dismiss
        dismiss_proposal(pid, "prop_status2", Some("not relevant")).unwrap();
        let dismissed = load_proposal(pid, "prop_status2").unwrap();
        assert_eq!(dismissed.status, ProposalStatus::Dismissed);
        assert_eq!(dismissed.dismiss_reason.as_deref(), Some("not relevant"));

        // Cannot dismiss again
        assert!(dismiss_proposal(pid, "prop_status2", None).is_err());

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn storage_save_load_list_roundtrip() {
        let pid = "test_proposal_storage";
        let _ = edda_store::ensure_dirs(pid);

        let p1 = IssueProposal {
            proposal_id: "prop_store1".to_string(),
            created_at: "2026-03-12T10:00:00Z".to_string(),
            source: ProposalSource::Manual,
            source_ref: None,
            title: "First".to_string(),
            body: "body1".to_string(),
            labels: vec!["bug".to_string()],
            status: ProposalStatus::Pending,
            approved_by: None,
            approved_at: None,
            issue_url: None,
            issue_number: None,
            dismiss_reason: None,
        };
        let p2 = IssueProposal {
            proposal_id: "prop_store2".to_string(),
            created_at: "2026-03-12T11:00:00Z".to_string(),
            source: ProposalSource::Bridge,
            source_ref: None,
            title: "Second".to_string(),
            body: "body2".to_string(),
            labels: vec![],
            status: ProposalStatus::Pending,
            approved_by: None,
            approved_at: None,
            issue_url: None,
            issue_number: None,
            dismiss_reason: None,
        };

        save_proposal(pid, &p1).unwrap();
        save_proposal(pid, &p2).unwrap();

        // Load single
        let loaded = load_proposal(pid, "prop_store1").unwrap();
        assert_eq!(loaded.title, "First");

        // List all
        let all = list_proposals(pid, None).unwrap();
        assert_eq!(all.len(), 2);

        // List pending only
        let pending = list_proposals(pid, Some(&ProposalStatus::Pending)).unwrap();
        assert_eq!(pending.len(), 2);

        // Approve one, list pending
        approve_proposal(pid, "prop_store1", "human").unwrap();
        let pending = list_proposals(pid, Some(&ProposalStatus::Pending)).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].proposal_id, "prop_store2");

        // List approved
        let approved = list_proposals(pid, Some(&ProposalStatus::Approved)).unwrap();
        assert_eq!(approved.len(), 1);
        assert_eq!(approved[0].proposal_id, "prop_store1");

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn record_issue_created_updates_proposal() {
        let pid = "test_proposal_issue_created";
        let _ = edda_store::ensure_dirs(pid);

        let proposal = IssueProposal {
            proposal_id: "prop_issue1".to_string(),
            created_at: "2026-03-12T10:00:00Z".to_string(),
            source: ProposalSource::Manual,
            source_ref: None,
            title: "Create me".to_string(),
            body: "body".to_string(),
            labels: vec![],
            status: ProposalStatus::Pending,
            approved_by: None,
            approved_at: None,
            issue_url: None,
            issue_number: None,
            dismiss_reason: None,
        };
        save_proposal(pid, &proposal).unwrap();
        approve_proposal(pid, "prop_issue1", "human").unwrap();

        record_issue_created(
            pid,
            "prop_issue1",
            "https://github.com/test/repo/issues/42",
            42,
        )
        .unwrap();

        let loaded = load_proposal(pid, "prop_issue1").unwrap();
        assert_eq!(
            loaded.issue_url.as_deref(),
            Some("https://github.com/test/repo/issues/42")
        );
        assert_eq!(loaded.issue_number, Some(42));

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn record_issue_created_rejects_non_approved() {
        let pid = "test_proposal_reject_non_approved";
        let _ = edda_store::ensure_dirs(pid);

        let proposal = IssueProposal {
            proposal_id: "prop_reject1".to_string(),
            created_at: "2026-03-12T10:00:00Z".to_string(),
            source: ProposalSource::Manual,
            source_ref: None,
            title: "Not approved".to_string(),
            body: "body".to_string(),
            labels: vec![],
            status: ProposalStatus::Pending,
            approved_by: None,
            approved_at: None,
            issue_url: None,
            issue_number: None,
            dismiss_reason: None,
        };
        save_proposal(pid, &proposal).unwrap();

        // Should fail because proposal is Pending, not Approved
        let result = record_issue_created(
            pid,
            "prop_reject1",
            "https://github.com/test/repo/issues/99",
            99,
        );
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not approved"),
            "Expected 'not approved' in error, got: {err_msg}"
        );

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn audit_log_records_actions() {
        let pid = "test_proposal_audit";
        let _ = edda_store::ensure_dirs(pid);

        let proposal = IssueProposal {
            proposal_id: "prop_audit1".to_string(),
            created_at: "2026-03-12T10:00:00Z".to_string(),
            source: ProposalSource::Manual,
            source_ref: None,
            title: "Audit test".to_string(),
            body: "body".to_string(),
            labels: vec![],
            status: ProposalStatus::Pending,
            approved_by: None,
            approved_at: None,
            issue_url: None,
            issue_number: None,
            dismiss_reason: None,
        };
        save_proposal(pid, &proposal).unwrap(); // creates 1 audit entry

        approve_proposal(pid, "prop_audit1", "human").unwrap(); // creates 1 more

        let path = audit_log_path(pid);
        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("created"));
        assert!(lines[1].contains("approved"));

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn new_proposal_id_format() {
        let id = new_proposal_id();
        assert!(id.starts_with("prop_"));
        assert_eq!(id.len(), 17); // "prop_" (5) + 12 chars
    }

    #[test]
    fn load_nonexistent_proposal_returns_error() {
        let pid = "test_proposal_missing";
        let _ = edda_store::ensure_dirs(pid);

        let result = load_proposal(pid, "prop_nonexistent");
        assert!(result.is_err());

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn proposal_source_serde() {
        for (source, expected) in [
            (ProposalSource::Scan, "\"scan\""),
            (ProposalSource::Postmortem, "\"postmortem\""),
            (ProposalSource::Manual, "\"manual\""),
            (ProposalSource::Bridge, "\"bridge\""),
        ] {
            let json = serde_json::to_string(&source).unwrap();
            assert_eq!(json, expected);
            let parsed: ProposalSource = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, source);
        }
    }
}
