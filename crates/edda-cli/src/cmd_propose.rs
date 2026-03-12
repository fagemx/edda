use clap::Subcommand;
use edda_bridge_claude::issue_proposal::{
    self, IssueProposal, ProposalSource, ProposalStatus,
};
use std::path::Path;

#[derive(Subcommand)]
pub enum ProposeCmd {
    /// Create a new issue proposal
    Create {
        /// Issue title
        #[arg(long)]
        title: String,
        /// Issue body
        #[arg(long)]
        body: String,
        /// Labels (repeatable)
        #[arg(long = "label")]
        labels: Vec<String>,
        /// Source: scan, postmortem, manual, bridge
        #[arg(long, default_value = "manual")]
        source: String,
        /// Source reference (e.g. "scan_abc:0")
        #[arg(long)]
        source_ref: Option<String>,
    },
    /// Create a proposal from a scan gap
    FromScan {
        /// Scan ID (scan_...)
        scan_id: String,
        /// Gap index (0-based)
        index: usize,
    },
    /// List proposals
    List {
        /// Filter by status: pending, approved, dismissed
        #[arg(long)]
        status: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show a proposal
    Show {
        /// Proposal ID (prop_...)
        prop_id: String,
    },
    /// Approve a proposal and create GitHub issue
    Approve {
        /// Proposal ID (prop_...)
        prop_id: String,
        /// Actor name
        #[arg(long, default_value = "human")]
        by: String,
        /// Preview without creating the issue
        #[arg(long)]
        dry_run: bool,
    },
    /// Dismiss a proposal
    Dismiss {
        /// Proposal ID (prop_...)
        prop_id: String,
        /// Reason for dismissal
        #[arg(long)]
        reason: Option<String>,
    },
}

pub fn execute(cmd: ProposeCmd, repo_root: &Path) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);

    match cmd {
        ProposeCmd::Create {
            title,
            body,
            labels,
            source,
            source_ref,
        } => execute_create(
            &project_id,
            &title,
            &body,
            &labels,
            &source,
            source_ref.as_deref(),
        ),
        ProposeCmd::FromScan { scan_id, index } => {
            execute_from_scan(&project_id, &scan_id, index)
        }
        ProposeCmd::List { status, json } => execute_list(&project_id, status.as_deref(), json),
        ProposeCmd::Show { prop_id } => execute_show(&project_id, &prop_id),
        ProposeCmd::Approve {
            prop_id,
            by,
            dry_run,
        } => execute_approve(&project_id, &prop_id, &by, dry_run, repo_root),
        ProposeCmd::Dismiss { prop_id, reason } => {
            execute_dismiss(&project_id, &prop_id, reason.as_deref())
        }
    }
}

fn parse_source(s: &str) -> anyhow::Result<ProposalSource> {
    match s {
        "scan" => Ok(ProposalSource::Scan),
        "postmortem" => Ok(ProposalSource::Postmortem),
        "manual" => Ok(ProposalSource::Manual),
        "bridge" => Ok(ProposalSource::Bridge),
        _ => anyhow::bail!("Unknown source: {s} (expected: scan, postmortem, manual, bridge)"),
    }
}

fn execute_create(
    project_id: &str,
    title: &str,
    body: &str,
    labels: &[String],
    source: &str,
    source_ref: Option<&str>,
) -> anyhow::Result<()> {
    let source = parse_source(source)?;
    let proposal = IssueProposal {
        proposal_id: issue_proposal::new_proposal_id(),
        created_at: now_rfc3339(),
        source,
        source_ref: source_ref.map(String::from),
        title: title.to_string(),
        body: body.to_string(),
        labels: labels.to_vec(),
        status: ProposalStatus::Pending,
        approved_by: None,
        approved_at: None,
        issue_url: None,
        issue_number: None,
        dismiss_reason: None,
    };

    issue_proposal::save_proposal(project_id, &proposal)?;
    println!("Proposal created: {}", proposal.proposal_id);
    println!("  Title: {}", proposal.title);
    println!();
    println!(
        "Use `edda propose-issue approve {}` to create the GitHub issue.",
        proposal.proposal_id
    );
    Ok(())
}

fn execute_from_scan(project_id: &str, scan_id: &str, index: usize) -> anyhow::Result<()> {
    let proposal = issue_proposal::create_proposal_from_scan_gap(project_id, scan_id, index)?;
    println!("Proposal created from scan gap: {}", proposal.proposal_id);
    println!("  Title: {}", proposal.title);
    println!("  Source: {}:{}", scan_id, index);
    println!();
    println!(
        "Use `edda propose-issue approve {}` to create the GitHub issue.",
        proposal.proposal_id
    );
    Ok(())
}

fn execute_list(
    project_id: &str,
    status: Option<&str>,
    json: bool,
) -> anyhow::Result<()> {
    let status_filter = match status {
        Some("pending") => Some(ProposalStatus::Pending),
        Some("approved") => Some(ProposalStatus::Approved),
        Some("dismissed") => Some(ProposalStatus::Dismissed),
        Some(s) => anyhow::bail!(
            "Unknown status filter: {s} (expected: pending, approved, dismissed)"
        ),
        None => None,
    };

    let proposals = issue_proposal::list_proposals(project_id, status_filter.as_ref())?;

    if json {
        println!("{}", serde_json::to_string_pretty(&proposals)?);
        return Ok(());
    }

    if proposals.is_empty() {
        let qualifier = status.unwrap_or("any");
        println!("No {qualifier} issue proposals found.");
        println!("Use `edda propose-issue create --title \"...\" --body \"...\"` to create one.");
        return Ok(());
    }

    for p in &proposals {
        let status_str = match p.status {
            ProposalStatus::Pending => "PENDING",
            ProposalStatus::Approved => "approved",
            ProposalStatus::Dismissed => "dismissed",
        };
        let labels_str = if p.labels.is_empty() {
            String::new()
        } else {
            format!(" [{}]", p.labels.join(", "))
        };
        let issue_str = p
            .issue_url
            .as_deref()
            .map(|u| format!(" -> {u}"))
            .unwrap_or_default();
        println!(
            "  {} [{}] {}{}{} ({})",
            p.proposal_id, status_str, p.title, labels_str, issue_str, p.created_at
        );
    }

    Ok(())
}

fn execute_show(project_id: &str, prop_id: &str) -> anyhow::Result<()> {
    let p = issue_proposal::load_proposal(project_id, prop_id)?;

    println!("Proposal:  {}", p.proposal_id);
    println!("Status:    {:?}", p.status);
    println!("Created:   {}", p.created_at);
    println!("Source:    {:?}", p.source);
    if let Some(ref sr) = p.source_ref {
        println!("Source ref: {sr}");
    }
    println!("Title:     {}", p.title);
    if !p.labels.is_empty() {
        println!("Labels:    {}", p.labels.join(", "));
    }
    println!();
    println!("Body:");
    println!("{}", p.body);

    if let Some(ref by) = p.approved_by {
        println!();
        println!("Approved by: {by}");
        if let Some(ref at) = p.approved_at {
            println!("Approved at: {at}");
        }
    }
    if let Some(ref url) = p.issue_url {
        println!("Issue URL:   {url}");
    }
    if let Some(num) = p.issue_number {
        println!("Issue #:     {num}");
    }
    if let Some(ref reason) = p.dismiss_reason {
        println!();
        println!("Dismiss reason: {reason}");
    }

    Ok(())
}

fn execute_approve(
    project_id: &str,
    prop_id: &str,
    by: &str,
    dry_run: bool,
    repo_root: &Path,
) -> anyhow::Result<()> {
    // Optional RBAC check
    check_rbac_if_configured(repo_root, by, "approve_issue_proposal")?;

    let proposal = if dry_run {
        issue_proposal::load_proposal(project_id, prop_id)?
    } else {
        issue_proposal::approve_proposal(project_id, prop_id, by)?
    };

    if dry_run {
        println!("=== DRY RUN ===");
        println!("Would create GitHub issue:");
        println!("  Title:  {}", proposal.title);
        println!("  Labels: {}", proposal.labels.join(", "));
        println!();
        println!("Body:");
        println!("{}", proposal.body);
        return Ok(());
    }

    // Call gh issue create
    let cwd = repo_root.to_string_lossy().to_string();
    match create_github_issue(&proposal, &cwd) {
        Ok((url, number)) => {
            issue_proposal::record_issue_created(project_id, prop_id, &url, number)?;
            println!("Issue created: {url}");
        }
        Err(e) => {
            // Issue creation failed, but approval is already recorded.
            // The user can retry by running `gh issue create` manually.
            eprintln!("Warning: Proposal approved but issue creation failed: {e}");
            eprintln!("The proposal is marked as approved. Create the issue manually:");
            eprintln!(
                "  gh issue create --title {:?} --body {:?}",
                proposal.title, proposal.body
            );
        }
    }

    Ok(())
}

fn execute_dismiss(
    project_id: &str,
    prop_id: &str,
    reason: Option<&str>,
) -> anyhow::Result<()> {
    issue_proposal::dismiss_proposal(project_id, prop_id, reason)?;
    println!("Proposal {prop_id} dismissed.");
    if let Some(r) = reason {
        println!("  Reason: {r}");
    }
    Ok(())
}

/// Create a GitHub issue via `gh` CLI. Returns (url, issue_number).
fn create_github_issue(proposal: &IssueProposal, cwd: &str) -> anyhow::Result<(String, u64)> {
    let mut args = vec![
        "issue".to_string(),
        "create".to_string(),
        "--title".to_string(),
        proposal.title.clone(),
        "--body".to_string(),
        proposal.body.clone(),
    ];

    for label in &proposal.labels {
        args.push("--label".to_string());
        args.push(label.clone());
    }

    let output = std::process::Command::new("gh")
        .args(&args)
        .current_dir(cwd)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("gh issue create failed: {stderr}");
    }

    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Parse issue number from URL (e.g. https://github.com/owner/repo/issues/42)
    let number = url
        .rsplit('/')
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    Ok((url, number))
}

/// Check RBAC if policy.yaml has permissions configured. Permissive default.
fn check_rbac_if_configured(repo_root: &Path, actor: &str, action: &str) -> anyhow::Result<()> {
    let edda_dir = repo_root.join(".edda");
    let policy = edda_core::policy::load_policy_from_dir(&edda_dir)?;

    // No permissions section = permissive
    if policy.permissions.is_none() {
        return Ok(());
    }

    let actors = edda_core::policy::load_actors_from_dir(&edda_dir)?;
    let req = edda_core::policy::AuthzRequest {
        actor: actor.to_string(),
        action: action.to_string(),
        resource: None,
    };
    let result = edda_core::policy::evaluate_authz(&req, &policy, &actors);
    if !result.allowed {
        let reason = result.reason.unwrap_or_default();
        anyhow::bail!(
            "RBAC denied: actor '{actor}' lacks '{action}' permission. {reason}"
        );
    }
    Ok(())
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_source_valid() {
        assert_eq!(parse_source("scan").unwrap(), ProposalSource::Scan);
        assert_eq!(
            parse_source("postmortem").unwrap(),
            ProposalSource::Postmortem
        );
        assert_eq!(parse_source("manual").unwrap(), ProposalSource::Manual);
        assert_eq!(parse_source("bridge").unwrap(), ProposalSource::Bridge);
    }

    #[test]
    fn parse_source_invalid() {
        assert!(parse_source("unknown").is_err());
    }
}
