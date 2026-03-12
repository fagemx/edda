use edda_ledger::Ledger;
use std::path::Path;

/// Show a single task brief by task_id.
pub fn execute_show(repo_root: &Path, task_id: &str) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let brief = ledger
        .get_task_brief(task_id)?
        .ok_or_else(|| anyhow::anyhow!("task brief not found: {task_id}"))?;

    println!("Task: {} — {}", brief.task_id, brief.title);
    println!(
        "Intent: {} | Status: {} | Branch: {}",
        brief.intent.as_str(),
        brief.status.as_str(),
        brief.branch,
    );
    println!(
        "Iterations: {} | Last activity: {}",
        brief.iterations, brief.updated_at
    );

    let artifacts: Vec<String> = serde_json::from_str(&brief.artifacts).unwrap_or_default();
    if !artifacts.is_empty() {
        println!("\nArtifacts:");
        for a in &artifacts {
            println!("  {a}");
        }
    }

    let decisions: Vec<String> = serde_json::from_str(&brief.decisions).unwrap_or_default();
    if !decisions.is_empty() {
        println!("\nDecisions:");
        for d in &decisions {
            println!("  {d}");
        }
    }

    if let Some(fb) = &brief.last_feedback {
        println!("\nLast feedback:");
        println!("  \"{fb}\"");
    }

    if !brief.source_url.is_empty() {
        println!("\nSource: {}", brief.source_url);
    }

    Ok(())
}

/// List all task briefs with optional filters.
pub fn execute_list(
    repo_root: &Path,
    status: Option<&str>,
    intent: Option<&str>,
    json: bool,
) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let briefs = ledger.list_task_briefs(status, intent)?;

    if briefs.is_empty() {
        println!("No task briefs found.");
        return Ok(());
    }

    if json {
        for b in &briefs {
            let obj = serde_json::json!({
                "task_id": b.task_id,
                "title": b.title,
                "intent": b.intent.as_str(),
                "status": b.status.as_str(),
                "branch": b.branch,
                "iterations": b.iterations,
                "source_url": b.source_url,
                "created_at": b.created_at,
                "updated_at": b.updated_at,
            });
            println!("{}", serde_json::to_string(&obj)?);
        }
        return Ok(());
    }

    println!("{} task brief(s):\n", briefs.len());
    for b in &briefs {
        println!(
            "  {} — {} [{}] ({})",
            b.task_id,
            b.title,
            b.status.as_str(),
            b.intent.as_str(),
        );
        println!(
            "    Branch: {} | Iterations: {} | Updated: {}",
            b.branch, b.iterations, b.updated_at
        );
    }

    Ok(())
}
