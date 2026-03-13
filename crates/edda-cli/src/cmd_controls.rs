use clap::Subcommand;
use edda_bridge_claude::controls_suggest::{self, PatchStatus};
use std::path::Path;

#[derive(Subcommand)]
pub enum ControlsCmd {
    /// Evaluate quality rules and generate a controls patch if thresholds are breached
    Suggest {
        /// Override minimum sample count (default: 10)
        #[arg(long)]
        min_samples: Option<u64>,
    },
    /// List controls patches
    List {
        /// Filter by status: pending, approved, dismissed, applied
        #[arg(long)]
        status: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show a controls patch
    Show {
        /// Patch ID (cpatch_...)
        patch_id: String,
    },
    /// Approve a controls patch and optionally apply to Karvi
    Approve {
        /// Patch ID (cpatch_...)
        patch_id: String,
        /// Actor name
        #[arg(long, default_value = "human")]
        by: String,
        /// Preview without approving
        #[arg(long)]
        dry_run: bool,
        /// Post to Karvi after approval
        #[arg(long)]
        apply: bool,
        /// Karvi API URL (default: http://localhost:3461)
        #[arg(long, default_value = "http://localhost:3461")]
        karvi_url: String,
    },
    /// Dismiss a controls patch
    Dismiss {
        /// Patch ID (cpatch_...)
        patch_id: String,
        /// Reason for dismissal
        #[arg(long)]
        reason: Option<String>,
    },
    /// Show current threshold rules
    Rules {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

pub fn execute(cmd: ControlsCmd, repo_root: &Path) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);

    match cmd {
        ControlsCmd::Suggest { min_samples } => {
            execute_suggest(&project_id, repo_root, min_samples)
        }
        ControlsCmd::List { status, json } => execute_list(&project_id, status.as_deref(), json),
        ControlsCmd::Show { patch_id } => execute_show(&project_id, &patch_id),
        ControlsCmd::Approve {
            patch_id,
            by,
            dry_run,
            apply,
            karvi_url,
        } => execute_approve(
            &project_id,
            &patch_id,
            &by,
            dry_run,
            apply,
            &karvi_url,
            repo_root,
        ),
        ControlsCmd::Dismiss { patch_id, reason } => {
            execute_dismiss(&project_id, &patch_id, reason.as_deref())
        }
        ControlsCmd::Rules { json } => execute_rules(json),
    }
}

fn execute_suggest(
    project_id: &str,
    repo_root: &Path,
    min_samples: Option<u64>,
) -> anyhow::Result<()> {
    use edda_aggregate::aggregate::DateRange;
    use edda_aggregate::quality::model_quality_from_events;
    use edda_ledger::Ledger;

    let ledger = Ledger::open(repo_root)?;
    let events = ledger.iter_events_by_type("execution_event")?;
    let report = model_quality_from_events(&events, &DateRange::default());

    if report.total_steps == 0 {
        println!("No execution events found. Cannot evaluate controls rules.");
        return Ok(());
    }

    println!(
        "Quality report: {} steps, {:.0}% success rate, ${:.2} total cost",
        report.total_steps,
        report.overall_success_rate * 100.0,
        report.total_cost_usd
    );

    let rules = controls_suggest::load_rules();
    let result =
        controls_suggest::suggest_controls_patch(project_id, &report, &rules, min_samples)?;

    match result {
        Some(patch) => {
            controls_suggest::save_patch(project_id, &patch)?;
            println!();
            println!("Controls patch created: {}", patch.patch_id);
            println!("  Suggestions:");
            for s in &patch.suggestions {
                println!("    - {} -> {} ({})", s.rule_name, s.action, s.reason);
            }
            println!();
            println!(
                "Use `edda propose-patch approve {}` to approve and apply.",
                patch.patch_id
            );
        }
        None => {
            println!();
            println!("No threshold breaches detected (or cooldown active). No patch needed.");
        }
    }

    Ok(())
}

fn execute_list(project_id: &str, status: Option<&str>, json: bool) -> anyhow::Result<()> {
    let status_filter = match status {
        Some("pending") => Some(PatchStatus::Pending),
        Some("approved") => Some(PatchStatus::Approved),
        Some("dismissed") => Some(PatchStatus::Dismissed),
        Some("applied") => Some(PatchStatus::Applied),
        Some(s) => {
            anyhow::bail!("Unknown status: {s} (expected: pending, approved, dismissed, applied)")
        }
        None => None,
    };

    let patches = controls_suggest::list_patches(project_id, status_filter.as_ref())?;

    if json {
        println!("{}", serde_json::to_string_pretty(&patches)?);
        return Ok(());
    }

    if patches.is_empty() {
        let qualifier = status.unwrap_or("any");
        println!("No {qualifier} controls patches found.");
        println!("Use `edda propose-patch suggest` to evaluate quality rules.");
        return Ok(());
    }

    for p in &patches {
        let status_str = match p.status {
            PatchStatus::Pending => "PENDING",
            PatchStatus::Approved => "approved",
            PatchStatus::Dismissed => "dismissed",
            PatchStatus::Applied => "applied",
        };
        let actions: Vec<&str> = p.suggestions.iter().map(|s| s.action.as_str()).collect();
        println!(
            "  {} [{}] {} ({})",
            p.patch_id,
            status_str,
            actions.join(", "),
            p.created_at
        );
    }

    Ok(())
}

fn execute_show(project_id: &str, patch_id: &str) -> anyhow::Result<()> {
    let p = controls_suggest::load_patch(project_id, patch_id)?;

    println!("Patch:     {}", p.patch_id);
    println!("Status:    {:?}", p.status);
    println!("Created:   {}", p.created_at);
    println!();
    println!("Controls:");
    for (key, value) in &p.controls {
        println!("  {key}: {value}");
    }
    println!();
    println!("Suggestions:");
    for s in &p.suggestions {
        println!(
            "  - [{}] {} -> {} ({} = {:.4}, threshold = {:.4})",
            s.rule_name, s.reason, s.action, s.metric_name, s.current_value, s.threshold
        );
    }

    if let Some(ref by) = p.approved_by {
        println!();
        println!("Approved by: {by}");
        if let Some(ref at) = p.approved_at {
            println!("Approved at: {at}");
        }
    }
    if let Some(ref at) = p.applied_at {
        println!("Applied at:  {at}");
    }
    if let Some(ref reason) = p.dismiss_reason {
        println!();
        println!("Dismiss reason: {reason}");
    }

    Ok(())
}

fn execute_approve(
    project_id: &str,
    patch_id: &str,
    by: &str,
    dry_run: bool,
    apply: bool,
    karvi_url: &str,
    repo_root: &Path,
) -> anyhow::Result<()> {
    check_rbac_if_configured(repo_root, by, "approve_controls_patch")?;

    if dry_run {
        let patch = controls_suggest::load_patch(project_id, patch_id)?;
        println!("=== DRY RUN ===");
        println!("Would approve patch: {}", patch.patch_id);
        println!("Controls:");
        for (key, value) in &patch.controls {
            println!("  {key}: {value}");
        }
        if apply {
            println!();
            println!("Would POST to {karvi_url}/api/controls");
        }
        return Ok(());
    }

    let _patch = controls_suggest::approve_patch(project_id, patch_id, by)?;
    println!("Patch {patch_id} approved by {by}.");

    if apply {
        match controls_suggest::apply_patch_to_karvi(project_id, patch_id, karvi_url) {
            Ok(()) => println!("Patch applied to Karvi at {karvi_url}."),
            Err(e) => {
                eprintln!("Warning: Patch approved but Karvi apply failed: {e}");
                eprintln!("The patch is marked as approved. Retry with:");
                eprintln!(
                    "  edda propose-patch approve {patch_id} --apply --karvi-url {karvi_url}"
                );
            }
        }
    } else {
        println!("Use --apply to also POST to Karvi, or apply manually.");
    }

    Ok(())
}

fn execute_dismiss(project_id: &str, patch_id: &str, reason: Option<&str>) -> anyhow::Result<()> {
    controls_suggest::dismiss_patch(project_id, patch_id, reason)?;
    println!("Patch {patch_id} dismissed.");
    if let Some(r) = reason {
        println!("  Reason: {r}");
    }
    Ok(())
}

fn execute_rules(json: bool) -> anyhow::Result<()> {
    let rules = controls_suggest::load_rules();

    if json {
        println!("{}", serde_json::to_string_pretty(&rules)?);
        return Ok(());
    }

    println!("Active threshold rules ({}):", rules.len());
    println!();
    for r in &rules {
        let op_str = match r.operator {
            edda_aggregate::controls::ThresholdOp::Lt => "<",
            edda_aggregate::controls::ThresholdOp::Gt => ">",
            edda_aggregate::controls::ThresholdOp::Lte => "<=",
            edda_aggregate::controls::ThresholdOp::Gte => ">=",
        };
        println!(
            "  {} : {:?} {} {:.4} -> {}",
            r.name, r.metric, op_str, r.threshold, r.action
        );
        println!("    Reason: {}", r.reason_template);
    }

    Ok(())
}

/// Check RBAC if policy.yaml has permissions configured. Permissive default.
fn check_rbac_if_configured(repo_root: &Path, actor: &str, action: &str) -> anyhow::Result<()> {
    let edda_dir = repo_root.join(".edda");
    let policy = edda_core::policy::load_policy_from_dir(&edda_dir)?;

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
        anyhow::bail!("RBAC denied: actor '{actor}' lacks '{action}' permission. {reason}");
    }
    Ok(())
}
