use edda_core::event::{new_task_intake_event, TaskIntakeParams};
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;
use std::path::Path;

/// Ingest a GitHub issue into the edda ledger as a `task_intake` event.
pub fn execute_github(repo_root: &Path, issue_id: u64) -> anyhow::Result<()> {
    let json = run_gh_issue_view(issue_id)?;
    let mut params = parse_github_issue(&json)?;

    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;

    params.branch = branch;
    params.parent_hash = parent_hash;

    let event = new_task_intake_event(&params)?;
    ledger.append_event(&event)?;
    println!(
        "Ingested issue #{} as {} (intent: {})",
        issue_id, event.event_id, params.intent
    );
    Ok(())
}

/// Run `gh issue view` and parse its JSON output.
fn run_gh_issue_view(issue_id: u64) -> anyhow::Result<serde_json::Value> {
    let output = std::process::Command::new("gh")
        .args([
            "issue",
            "view",
            &issue_id.to_string(),
            "--json",
            "number,title,body,labels,assignees,url",
        ])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!("gh CLI not found. Install it: https://cli.github.com/")
            } else {
                anyhow::anyhow!("Failed to run gh: {e}")
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("gh issue view failed: {stderr}");
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|e| anyhow::anyhow!("Failed to parse gh output: {e}"))
}

/// Parse the JSON output from `gh issue view` into `TaskIntakeParams`.
fn parse_github_issue(json: &serde_json::Value) -> anyhow::Result<TaskIntakeParams> {
    let number = json["number"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("Missing issue number in gh output"))?;
    let title = json["title"].as_str().unwrap_or("").to_string();
    let url = json["url"].as_str().unwrap_or("").to_string();
    let labels: Vec<String> = json["labels"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l["name"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let intent = detect_intent(&title).to_string();

    Ok(TaskIntakeParams {
        branch: String::new(),
        parent_hash: None,
        source: "github_issue".to_string(),
        source_id: number.to_string(),
        source_url: url,
        title,
        intent,
        labels,
        priority: "normal".to_string(),
        constraints: vec![],
    })
}

/// Detect intent from issue title prefix.
fn detect_intent(title: &str) -> &str {
    let lower = title.to_lowercase();
    let trimmed = lower.trim_start();
    if trimmed.starts_with("feat:") || trimmed.starts_with("feature:") {
        "implement"
    } else if trimmed.starts_with("bug:") || trimmed.starts_with("fix:") {
        "fix"
    } else if trimmed.starts_with("refactor:") || trimmed.starts_with("chore:") {
        "maintain"
    } else if trimmed.starts_with("docs:") || trimmed.starts_with("doc:") {
        "document"
    } else if trimmed.starts_with("test:") {
        "test"
    } else if trimmed.starts_with("perf:") {
        "optimize"
    } else {
        "implement"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_intent_feat() {
        assert_eq!(detect_intent("feat: add auth"), "implement");
        assert_eq!(detect_intent("feature: new login"), "implement");
    }

    #[test]
    fn detect_intent_bug() {
        assert_eq!(detect_intent("bug: crash on login"), "fix");
        assert_eq!(detect_intent("fix: null pointer"), "fix");
    }

    #[test]
    fn detect_intent_refactor() {
        assert_eq!(detect_intent("refactor: extract module"), "maintain");
        assert_eq!(detect_intent("chore: update deps"), "maintain");
    }

    #[test]
    fn detect_intent_docs() {
        assert_eq!(detect_intent("docs: update readme"), "document");
    }

    #[test]
    fn detect_intent_test() {
        assert_eq!(detect_intent("test: add coverage"), "test");
    }

    #[test]
    fn detect_intent_perf() {
        assert_eq!(detect_intent("perf: optimize queries"), "optimize");
    }

    #[test]
    fn detect_intent_default() {
        assert_eq!(detect_intent("Add user authentication"), "implement");
        assert_eq!(detect_intent(""), "implement");
    }

    #[test]
    fn detect_intent_case_insensitive() {
        assert_eq!(detect_intent("FEAT: uppercase"), "implement");
        assert_eq!(detect_intent("Bug: mixed case"), "fix");
    }

    #[test]
    fn parse_github_issue_full() {
        let json = serde_json::json!({
            "number": 45,
            "title": "feat: add user authentication",
            "body": "Implement OAuth2 login flow",
            "url": "https://github.com/owner/repo/issues/45",
            "labels": [
                {"name": "enhancement"},
                {"name": "priority:high"}
            ],
            "assignees": [{"login": "user1"}]
        });
        let params = parse_github_issue(&json).unwrap();
        assert_eq!(params.source, "github_issue");
        assert_eq!(params.source_id, "45");
        assert_eq!(params.source_url, "https://github.com/owner/repo/issues/45");
        assert_eq!(params.title, "feat: add user authentication");
        assert_eq!(params.intent, "implement");
        assert_eq!(params.labels, vec!["enhancement", "priority:high"]);
        assert_eq!(params.priority, "normal");
        assert!(params.constraints.is_empty());
    }

    #[test]
    fn parse_github_issue_minimal() {
        let json = serde_json::json!({
            "number": 1,
            "title": "bug: something broken",
            "body": null,
            "url": "https://github.com/o/r/issues/1",
            "labels": [],
            "assignees": []
        });
        let params = parse_github_issue(&json).unwrap();
        assert_eq!(params.source_id, "1");
        assert_eq!(params.intent, "fix");
        assert!(params.labels.is_empty());
    }

    #[test]
    fn parse_github_issue_missing_number() {
        let json = serde_json::json!({
            "title": "no number",
            "body": null,
            "url": "",
            "labels": []
        });
        assert!(parse_github_issue(&json).is_err());
    }
}
