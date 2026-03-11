use clap::Subcommand;
use edda_bridge_claude::bg_scan;
use std::path::Path;

#[derive(Subcommand)]
pub enum ScanCmd {
    /// Run capability scan now
    Run {
        /// Ignore cooldown and force a scan
        #[arg(long)]
        force: bool,
    },
    /// List pending capability gaps
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show details of a specific gap
    Show {
        /// Scan ID (scan_...)
        scan_id: String,
        /// Gap index (0-based)
        index: usize,
    },
    /// Create GitHub issue from a gap draft
    Create {
        /// Scan ID (scan_...)
        scan_id: String,
        /// Gap index (0-based)
        index: usize,
        /// Preview without creating the issue
        #[arg(long)]
        dry_run: bool,
    },
    /// Dismiss a gap (mark as not relevant)
    Dismiss {
        /// Scan ID (scan_...)
        scan_id: String,
        /// Gap index (0-based)
        index: usize,
    },
}

pub fn execute(cmd: ScanCmd, repo_root: &Path) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let cwd = repo_root.to_string_lossy().to_string();

    match cmd {
        ScanCmd::Run { force } => execute_run(&project_id, &cwd, force),
        ScanCmd::List { json } => execute_list(&project_id, json),
        ScanCmd::Show { scan_id, index } => execute_show(&project_id, &scan_id, index),
        ScanCmd::Create {
            scan_id,
            index,
            dry_run,
        } => execute_create(&project_id, &scan_id, index, dry_run, &cwd),
        ScanCmd::Dismiss { scan_id, index } => execute_dismiss(&project_id, &scan_id, index),
    }
}

fn execute_run(project_id: &str, cwd: &str, force: bool) -> anyhow::Result<()> {
    if !force && !bg_scan::should_run(project_id) {
        println!("Scan skipped (cooldown not elapsed, or disabled/no API key).");
        println!("Use --force to override cooldown.");
        return Ok(());
    }

    println!("Running capability scan...");
    let result = bg_scan::run_scan(project_id, cwd)?;

    if result.gaps.is_empty() {
        println!("No capability gaps found.");
    } else {
        println!(
            "Found {} capability gap(s) (scan: {}):",
            result.gaps.len(),
            result.scan_id
        );
        for (i, gap) in result.gaps.iter().enumerate() {
            println!(
                "  [{}] [{}] [{}] {} (confidence: {:.0}%)",
                i,
                gap.severity,
                gap.category,
                gap.title,
                gap.confidence * 100.0
            );
        }
        println!();
        println!("Use `edda scan list` to review, `edda scan create <scan_id> <index>` to create issues.");
    }

    println!(
        "\nCost: ${:.4} ({} input + {} output tokens)",
        result.cost_usd, result.input_tokens, result.output_tokens
    );

    Ok(())
}

fn execute_list(project_id: &str, json: bool) -> anyhow::Result<()> {
    let scans = bg_scan::list_pending_scans(project_id)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&scans)?);
        return Ok(());
    }

    if scans.is_empty() {
        println!("No pending capability gaps.");
        println!("Run `edda scan run` to perform a scan.");
        return Ok(());
    }

    for scan in &scans {
        println!("Scan: {} ({})", scan.scan_id, scan.scanned_at);
        println!("  Model: {}, Cost: ${:.4}", scan.model, scan.cost_usd);
        for (i, gap) in scan.gaps.iter().enumerate() {
            let status = match gap.status {
                bg_scan::GapStatus::Pending => "PENDING",
                bg_scan::GapStatus::Accepted => "accepted",
                bg_scan::GapStatus::Dismissed => "dismissed",
            };
            println!(
                "  [{}] [{}] [{}] [{}] {} (confidence: {:.0}%)",
                i,
                status,
                gap.severity,
                gap.category,
                gap.title,
                gap.confidence * 100.0
            );
        }
        println!();
    }

    Ok(())
}

fn execute_show(project_id: &str, scan_id: &str, index: usize) -> anyhow::Result<()> {
    let scans = bg_scan::list_pending_scans(project_id)?;
    let scan = scans
        .iter()
        .find(|s| s.scan_id == scan_id)
        .ok_or_else(|| anyhow::anyhow!("Scan not found: {scan_id}"))?;

    if index >= scan.gaps.len() {
        anyhow::bail!(
            "Gap index {} out of range (scan has {} gaps)",
            index,
            scan.gaps.len()
        );
    }

    let gap = &scan.gaps[index];
    println!("Title:    {}", gap.title);
    println!("Category: {}", gap.category);
    println!("Severity: {}", gap.severity);
    println!(
        "Confidence: {:.0}%",
        gap.confidence * 100.0
    );
    println!();
    println!("Description:");
    println!("  {}", gap.description);
    if !gap.evidence.is_empty() {
        println!();
        println!("Evidence:");
        for ev in &gap.evidence {
            println!("  - {ev}");
        }
    }
    if !gap.suggested_labels.is_empty() {
        println!();
        println!(
            "Suggested labels: {}",
            gap.suggested_labels.join(", ")
        );
    }

    Ok(())
}

fn execute_create(
    project_id: &str,
    scan_id: &str,
    index: usize,
    dry_run: bool,
    cwd: &str,
) -> anyhow::Result<()> {
    let gap = bg_scan::accept_gap(project_id, scan_id, index)?;

    let title = &gap.title;
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

    let body = body_parts.join("\n");

    if dry_run {
        println!("=== DRY RUN ===");
        println!("Title: {title}");
        println!("Labels: {}", gap.suggested_labels.join(", "));
        println!("Body:");
        println!("{body}");
        return Ok(());
    }

    // Build gh issue create command
    let mut args = vec![
        "issue".to_string(),
        "create".to_string(),
        "--title".to_string(),
        title.clone(),
        "--body".to_string(),
        body,
    ];

    for label in &gap.suggested_labels {
        args.push("--label".to_string());
        args.push(label.clone());
    }

    let output = std::process::Command::new("gh")
        .args(&args)
        .current_dir(cwd)
        .output()?;

    if output.status.success() {
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        println!("Issue created: {url}");
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create issue: {stderr}");
    }

    Ok(())
}

fn execute_dismiss(project_id: &str, scan_id: &str, index: usize) -> anyhow::Result<()> {
    bg_scan::dismiss_gap(project_id, scan_id, index)?;
    println!("Gap {} dismissed from scan {}", index, scan_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_list_handles_empty() {
        // Just verify no panic on a fresh project
        let pid = "test_cmd_scan_empty";
        let _ = edda_store::ensure_dirs(pid);

        let scans = bg_scan::list_pending_scans(pid).unwrap();
        assert!(scans.is_empty());

        // Cleanup
        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }
}
