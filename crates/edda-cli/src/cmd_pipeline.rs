use crate::pipeline_templates::{
    render_quickfix_plan, render_standard_plan, select_pipeline, PipelineType,
};
use anyhow::{bail, Result};
use edda_core::Event;
use edda_ledger::Ledger;
use std::path::Path;

/// Execute `edda pipeline run <issue-id>`.
pub fn execute_run(repo_root: &Path, issue_id: u64, dry_run: bool) -> Result<()> {
    let ledger = Ledger::open(repo_root)?;

    // 1. Find task_intake event for this issue
    let intake = find_task_intake(&ledger, issue_id)?;
    let Some(intake) = intake else {
        bail!(
            "No task_intake event found for issue #{issue_id}.\n\
             Run `edda intake github {issue_id}` first."
        );
    };

    // 2. Extract intent + labels from intake event payload
    let payload = &intake.payload;
    let intent = payload["intent"].as_str().unwrap_or("implement");
    let labels: Vec<String> = payload["labels"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let title = payload["title"].as_str().unwrap_or("");
    let url = payload["source_url"].as_str().unwrap_or("");

    // 3. Select pipeline type
    let pipeline_type = select_pipeline(intent, &labels);

    // 4. Render plan YAML
    let yaml = match pipeline_type {
        PipelineType::Standard => render_standard_plan(issue_id, title, url),
        PipelineType::QuickFix => render_quickfix_plan(issue_id, title, url),
    };

    println!(
        "Pipeline: {:?} for issue #{} (intent: {}, labels: [{}])",
        pipeline_type,
        issue_id,
        intent,
        labels.join(", ")
    );

    if dry_run {
        println!("\n--- Generated plan.yaml ---\n{yaml}");
        return Ok(());
    }

    // 5. Write plan to .edda/conductor/pipeline-issue-{id}/plan.yaml
    let plan_dir = repo_root
        .join(".edda")
        .join("conductor")
        .join(format!("pipeline-issue-{issue_id}"));
    std::fs::create_dir_all(&plan_dir)?;
    let plan_file = plan_dir.join("plan.yaml");
    std::fs::write(&plan_file, &yaml)?;
    println!("Plan written to {}", plan_file.display());

    // 6. Delegate to conductor
    crate::cmd_conduct::run(&plan_file, Some(repo_root), false, true, false)
}

/// Execute `edda pipeline status [issue-id]`.
pub fn execute_status(repo_root: &Path, issue_id: Option<u64>) -> Result<()> {
    let plan_name = issue_id.map(|id| format!("pipeline-issue-{id}"));
    crate::cmd_conduct::status(repo_root, plan_name.as_deref(), false)
}

/// Find the most recent task_intake event for the given issue ID.
fn find_task_intake(ledger: &Ledger, issue_id: u64) -> Result<Option<Event>> {
    let events = ledger.iter_events()?;
    let issue_str = issue_id.to_string();
    Ok(events.into_iter().rev().find(|e| {
        e.event_type == "task_intake" && e.payload["source_id"].as_str() == Some(issue_str.as_str())
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use edda_core::event::{new_task_intake_event, TaskIntakeParams};

    #[test]
    fn find_task_intake_found() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Initialize workspace
        Ledger::ensure_initialized(root).unwrap();
        let ledger = Ledger::open(root).unwrap();

        // Write a task_intake event
        let branch = ledger.head_branch().unwrap();
        let parent_hash = ledger.last_event_hash().unwrap();
        let params = TaskIntakeParams {
            branch,
            parent_hash,
            source: "github_issue".into(),
            source_id: "42".into(),
            source_url: "https://github.com/o/r/issues/42".into(),
            title: "feat: test".into(),
            intent: "implement".into(),
            labels: vec![],
            priority: "normal".into(),
            constraints: vec![],
        };
        let event = new_task_intake_event(&params).unwrap();
        ledger.append_event(&event).unwrap();

        // Find it
        let found = find_task_intake(&ledger, 42).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().payload["source_id"].as_str().unwrap(), "42");
    }

    #[test]
    fn find_task_intake_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        Ledger::ensure_initialized(root).unwrap();
        let ledger = Ledger::open(root).unwrap();

        let found = find_task_intake(&ledger, 999).unwrap();
        assert!(found.is_none());
    }
}
