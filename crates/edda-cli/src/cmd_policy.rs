//! CLI subcommands for `edda policy show|check|init`.

use anyhow::{bail, Result};
use clap::Subcommand;
use edda_core::approval::{self, EvalContext};
use edda_core::bundle::ReviewBundle;
use edda_ledger::Ledger;
use std::path::Path;

#[derive(Subcommand)]
pub enum PolicyCmd {
    /// Show the effective approval policy (merged defaults + user overrides)
    Show,
    /// Simulate policy evaluation against a review bundle
    Check {
        /// Bundle ID (bun_...)
        bundle_id: String,
        /// Pipeline step to evaluate (e.g. "pr_merge", "implement")
        #[arg(long, default_value = "implement")]
        step: String,
    },
    /// Generate .edda/approval-policy.yaml template
    Init,
}

/// Execute `edda policy show`.
fn execute_show(repo_root: &Path) -> Result<()> {
    let edda_dir = repo_root.join(".edda");
    let policy = approval::load_approval_policy(&edda_dir)?;

    let yaml = serde_yaml::to_string(&policy)?;
    let source = if edda_dir.join("approval-policy.yaml").exists() {
        ".edda/approval-policy.yaml"
    } else {
        "built-in defaults"
    };
    println!("# Effective approval policy");
    println!("# Source: {source}");
    println!();
    print!("{yaml}");
    Ok(())
}

/// Execute `edda policy check <bundle-id>`.
fn execute_check(repo_root: &Path, bundle_id: &str, step: &str) -> Result<()> {
    let edda_dir = repo_root.join(".edda");
    let policy = approval::load_approval_policy(&edda_dir)?;

    // Find the review_bundle event in the ledger
    let ledger = Ledger::open(repo_root)?;
    let Some(row) = ledger.get_bundle(bundle_id)? else {
        bail!("Bundle '{bundle_id}' not found in ledger.");
    };

    // Fetch full event payload
    let Some(event) = ledger.get_event(&row.event_id)? else {
        bail!(
            "Event '{}' for bundle '{bundle_id}' not found.",
            row.event_id
        );
    };
    let review_bundle: ReviewBundle = serde_json::from_value(event.payload)?;

    // Build a minimal EvalContext for CLI simulation
    let phase_state = edda_core::agent_phase::AgentPhaseState {
        phase: edda_core::agent_phase::AgentPhase::Implement,
        session_id: "cli-check".to_string(),
        label: None,
        issue: None,
        pr: None,
        branch: None,
        confidence: 1.0,
        detected_at: String::new(),
        signals: vec![],
    };

    let ctx = EvalContext {
        bundle: &review_bundle,
        phase: &phase_state,
        off_limits_touched: false,
        consecutive_failures: 0,
        current_time: Some(time::OffsetDateTime::now_utc()),
    };

    let decision = policy.evaluate(step, &ctx);

    println!("Approval check for step '{step}':");
    println!("  action:       {:?}", decision.action);
    if let Some(rule) = &decision.matched_rule {
        println!("  matched_rule: {rule}");
    }
    println!("  reason:       {}", decision.reason);
    println!("  overridable:  {}", decision.overridable);
    println!();
    println!("  risk_level:   {:?}", review_bundle.risk_assessment.level);
    println!(
        "  files_changed: {}",
        review_bundle.change_summary.files.len()
    );
    println!("  tests_failed:  {}", review_bundle.test_results.failed);

    Ok(())
}

/// Execute `edda policy init`.
fn execute_init(repo_root: &Path) -> Result<()> {
    let edda_dir = repo_root.join(".edda");
    let target = edda_dir.join("approval-policy.yaml");

    if target.exists() {
        bail!(".edda/approval-policy.yaml already exists. Remove it first to re-initialize.");
    }

    if !edda_dir.exists() {
        bail!("Not an edda workspace (run `edda init` first).");
    }

    let template = approval::generate_template();
    std::fs::write(&target, template)?;
    println!("Created .edda/approval-policy.yaml with default policy template.");
    println!("Edit the file to customize approval rules for your project.");
    Ok(())
}

/// Dispatch `edda policy <subcommand>`.
pub fn run(cmd: PolicyCmd, repo_root: &Path) -> Result<()> {
    match cmd {
        PolicyCmd::Show => execute_show(repo_root),
        PolicyCmd::Check { bundle_id, step } => execute_check(repo_root, &bundle_id, &step),
        PolicyCmd::Init => execute_init(repo_root),
    }
}
