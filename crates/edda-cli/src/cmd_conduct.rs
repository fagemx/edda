use anyhow::{bail, Result};
use edda_conductor::agent::budget::BudgetTracker;
use edda_conductor::agent::launcher::{phase_session_id, ClaudeCodeLauncher};
use edda_conductor::check::engine::CheckEngine;
use edda_conductor::plan::parser::load_plan;
use edda_conductor::runner::notify::StdoutNotifier;
use edda_conductor::runner::sequential::run_plan;
use edda_conductor::state::machine::{PhaseStatus, PlanState, PlanStatus};
use edda_conductor::state::persist::{load_state, save_state};
use std::path::Path;
use tokio_util::sync::CancellationToken;

/// Execute `edda conduct run <plan.yaml>`
pub fn run(plan_file: &Path, cwd_override: Option<&Path>, dry_run: bool, verbose: bool) -> Result<()> {
    let plan = load_plan(plan_file)?;
    let cwd = cwd_override
        .map(|p| p.to_path_buf())
        .or_else(|| plan.cwd.as_ref().map(|p| plan_file.parent().unwrap_or(Path::new(".")).join(p)))
        .unwrap_or_else(|| {
            plan_file
                .parent()
                .unwrap_or(Path::new("."))
                .to_path_buf()
        });
    let cwd = if cwd.is_relative() {
        std::env::current_dir()?.join(&cwd)
    } else {
        cwd
    };

    // Load or create state
    let mut state = match load_state(&cwd, &plan.name)? {
        Some(s) => {
            println!("Resuming plan \"{}\"", plan.name);
            s
        }
        None => {
            println!("Starting plan \"{}\" ({} phases)", plan.name, plan.phases.len());
            PlanState::from_plan(&plan, &plan_file.display().to_string())
        }
    };

    if dry_run {
        println!("\n[dry-run] Plan: {}", plan.name);
        println!("  Phases: {}", plan.phases.len());
        println!("  Budget: {}", plan.budget_usd.map_or("unlimited".into(), |b| format!("${b:.2}")));
        println!("  Max attempts: {}", plan.max_attempts);
        println!("  On fail: {:?}", plan.on_fail);
        println!("\n  Phase order:");
        let order = edda_conductor::plan::topo::topo_sort(&plan)?;
        for (i, id) in order.iter().enumerate() {
            let phase = plan.phases.iter().find(|p| p.id == *id).unwrap();
            let checks = if phase.check.is_empty() {
                String::new()
            } else {
                format!(" ({} checks)", phase.check.len())
            };
            println!("  {}. {}{}", i + 1, id, checks);
        }
        println!("\n  Session IDs:");
        for id in &order {
            println!("    {} → {}", id, phase_session_id(&plan.name, id));
        }
        return Ok(());
    }

    let mut launcher = ClaudeCodeLauncher::new().with_verbose(verbose);
    launcher.transcript_dir = Some(
        cwd.join(".edda")
            .join("conductor")
            .join(&plan.name)
            .join("transcripts"),
    );
    launcher.verify_available()?;
    let engine = CheckEngine::new(cwd.clone());
    let notifier = StdoutNotifier;
    let mut budget = BudgetTracker::new(plan.budget_usd);
    let cancel = CancellationToken::new();

    // Handle Ctrl+C gracefully
    let cancel_clone = cancel.clone();
    ctrlc_cancel(cancel_clone);

    let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_plan(
        &plan,
        &mut state,
        &launcher,
        &engine,
        &notifier,
        &mut budget,
        cancel,
        &cwd,
        interactive,
    ))?;

    Ok(())
}

/// Execute `edda conduct status [plan-name]`
pub fn status(repo_root: &Path, plan_name: Option<&str>) -> Result<()> {
    let conductor_dir = repo_root.join(".edda").join("conductor");
    if !conductor_dir.exists() {
        println!("No conductor state found.");
        return Ok(());
    }

    let plans: Vec<String> = if let Some(name) = plan_name {
        vec![name.to_string()]
    } else {
        // List all plan directories
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&conductor_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    names.push(name.to_string());
                }
            }
        }
        names.sort();
        names
    };

    if plans.is_empty() {
        println!("No plans found.");
        return Ok(());
    }

    for name in &plans {
        let state = load_state(repo_root, name)?;
        match state {
            Some(s) => print_status(&s),
            None => println!("Plan \"{name}\": no state file found"),
        }
    }

    Ok(())
}

/// Execute `edda conduct retry <phase-id>`
pub fn retry(repo_root: &Path, phase_id: &str, plan_name: Option<&str>) -> Result<()> {
    let name = resolve_plan_name(repo_root, plan_name)?;
    let mut state = load_state(repo_root, &name)?
        .ok_or_else(|| anyhow::anyhow!("no state for plan \"{name}\""))?;

    let current_status = {
        let ps = state.get_phase_mut(phase_id)?;
        if ps.status != PhaseStatus::Failed && ps.status != PhaseStatus::Stale {
            bail!(
                "Phase \"{}\" is {:?}, not Failed or Stale. Cannot retry.",
                phase_id,
                ps.status
            );
        }
        ps.status
    };

    edda_conductor::state::machine::transition(
        &mut state,
        phase_id,
        current_status,
        PhaseStatus::Pending,
        None,
    )?;

    // Reset plan status so runner picks up
    if state.plan_status == PlanStatus::Blocked {
        state.plan_status = PlanStatus::Running;
    }

    save_state(repo_root, &state)?;
    println!("Phase \"{phase_id}\" reset to Pending. Run `edda conduct run` to resume.");
    Ok(())
}

/// Execute `edda conduct skip <phase-id>`
pub fn skip(repo_root: &Path, phase_id: &str, reason: Option<&str>, plan_name: Option<&str>) -> Result<()> {
    let name = resolve_plan_name(repo_root, plan_name)?;
    let mut state = load_state(repo_root, &name)?
        .ok_or_else(|| anyhow::anyhow!("no state for plan \"{name}\""))?;

    let ps = state.get_phase_mut(phase_id)?;
    if ps.status != PhaseStatus::Failed && ps.status != PhaseStatus::Stale && ps.status != PhaseStatus::Pending {
        bail!(
            "Phase \"{}\" is {:?}. Can only skip Failed, Stale, or Pending phases.",
            phase_id,
            ps.status
        );
    }

    ps.status = PhaseStatus::Skipped;
    ps.skip_reason = Some(reason.unwrap_or("manually skipped").to_string());

    // Unblock plan
    if state.plan_status == PlanStatus::Blocked {
        state.plan_status = PlanStatus::Running;
    }

    save_state(repo_root, &state)?;
    println!("Phase \"{phase_id}\" skipped.");
    Ok(())
}

/// Execute `edda conduct abort [plan-name]`
pub fn abort(repo_root: &Path, plan_name: Option<&str>) -> Result<()> {
    let name = resolve_plan_name(repo_root, plan_name)?;
    let mut state = load_state(repo_root, &name)?
        .ok_or_else(|| anyhow::anyhow!("no state for plan \"{name}\""))?;

    if state.plan_status == PlanStatus::Completed || state.plan_status == PlanStatus::Aborted {
        bail!("Plan \"{}\" is already {:?}.", name, state.plan_status);
    }

    state.plan_status = PlanStatus::Aborted;
    state.aborted_at = Some(now_rfc3339());
    save_state(repo_root, &state)?;
    println!("Plan \"{name}\" aborted.");
    Ok(())
}

// --- helpers ---

fn resolve_plan_name(repo_root: &Path, explicit: Option<&str>) -> Result<String> {
    if let Some(name) = explicit {
        return Ok(name.to_string());
    }

    let conductor_dir = repo_root.join(".edda").join("conductor");
    if !conductor_dir.exists() {
        bail!("No conductor state found. Specify --plan <name>.");
    }

    let mut names = Vec::new();
    for entry in std::fs::read_dir(&conductor_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if let Some(n) = entry.file_name().to_str() {
                names.push(n.to_string());
            }
        }
    }

    match names.len() {
        0 => bail!("No plans found."),
        1 => Ok(names.into_iter().next().unwrap()),
        _ => bail!(
            "Multiple plans found: {}. Use --plan to specify.",
            names.join(", ")
        ),
    }
}

fn print_status(state: &PlanState) {
    println!(
        "\nPlan: {} ({:?})",
        state.plan_name, state.plan_status
    );
    if !state.plan_file.is_empty() {
        println!("  File: {}", state.plan_file);
    }
    println!("  Cost: ${:.2}", state.total_cost_usd);

    println!();
    for ps in &state.phases {
        let icon = match ps.status {
            PhaseStatus::Passed => "\u{2713}",   // ✓
            PhaseStatus::Failed => "\u{2717}",   // ✗
            PhaseStatus::Running | PhaseStatus::Checking => "\u{25B6}", // ▶
            PhaseStatus::Skipped => "\u{2298}",  // ⊘
            PhaseStatus::Stale => "\u{23F0}",    // ⏰
            PhaseStatus::Pending => "\u{25CB}",  // ○
        };
        let detail = match ps.status {
            PhaseStatus::Passed => format!("(attempt {})", ps.attempts),
            PhaseStatus::Failed => {
                let err = ps
                    .error
                    .as_ref()
                    .map(|e| e.message.as_str())
                    .unwrap_or("unknown");
                format!("(attempt {}, {})", ps.attempts, err)
            }
            PhaseStatus::Skipped => {
                let reason = ps.skip_reason.as_deref().unwrap_or("");
                format!("({})", reason)
            }
            _ => String::new(),
        };
        println!("  {icon} {:<24} {:?} {detail}", ps.id, ps.status);
    }
    println!();
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn ctrlc_cancel(cancel: CancellationToken) {
    let _ = ctrlc::set_handler(move || {
        cancel.cancel();
    });
}
