use anyhow::Result;
use edda_ledger::Ledger;

use crate::snapshot::fmt_cmd_argv;

#[derive(Debug, Clone)]
pub struct AutoEvidenceResult {
    pub items: Vec<serde_json::Value>,
    pub preview_lines: Vec<String>,
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

/// Scan the ledger for auto-evidence on the given branch.
/// Collects todo notes, decision notes, and failed commands
/// since the last commit on that branch. Returns at most `max` items.
pub fn build_auto_evidence(
    ledger: &Ledger,
    branch: &str,
    max: usize,
) -> Result<AutoEvidenceResult> {
    let all_events = ledger.iter_events()?;

    // Find the index of the last commit on this branch
    let last_commit_idx = all_events
        .iter()
        .enumerate()
        .rev()
        .find(|(_, ev)| ev.branch == branch && ev.event_type == "commit")
        .map(|(i, _)| i);

    // Scan range: after last commit to end, filtered to this branch
    let start = last_commit_idx.map(|i| i + 1).unwrap_or(0);
    let scan_events: Vec<_> = all_events[start..]
        .iter()
        .filter(|ev| ev.branch == branch)
        .collect();

    let mut items: Vec<serde_json::Value> = Vec::new();
    let mut preview_lines: Vec<String> = Vec::new();

    // Iterate newest-first
    for ev in scan_events.iter().rev() {
        if items.len() >= max {
            break;
        }

        match ev.event_type.as_str() {
            "note" => {
                let tags: Vec<&str> = ev
                    .payload
                    .get("tags")
                    .and_then(|x| x.as_array())
                    .map(|arr| arr.iter().filter_map(|i| i.as_str()).collect())
                    .unwrap_or_default();

                let matching_tag = if tags.contains(&"todo") {
                    Some("todo")
                } else if tags.contains(&"decision") {
                    Some("decision")
                } else {
                    None
                };

                if let Some(tag) = matching_tag {
                    let text = ev
                        .payload
                        .get("text")
                        .and_then(|x| x.as_str())
                        .unwrap_or("");
                    items.push(serde_json::json!({
                        "type": "note",
                        "event_id": ev.event_id,
                        "text": text,
                        "tag": tag,
                    }));
                    preview_lines.push(format!(
                        "[{}] {} ({})",
                        tag,
                        truncate(text, 60),
                        ev.event_id
                    ));
                }
            }
            "cmd" => {
                let exit_code = ev
                    .payload
                    .get("exit_code")
                    .and_then(|x| x.as_i64())
                    .unwrap_or(0);
                if exit_code != 0 {
                    let argv = fmt_cmd_argv(&ev.payload);
                    items.push(serde_json::json!({
                        "type": "cmd_fail",
                        "event_id": ev.event_id,
                        "command": argv,
                        "exit_code": exit_code,
                    }));
                    preview_lines.push(format!(
                        "[cmd_fail] {} exit={} ({})",
                        truncate(&argv, 40),
                        exit_code,
                        ev.event_id
                    ));

                    // Also collect stderr blob if present and under limit
                    if items.len() < max {
                        let stderr_blob = ev
                            .payload
                            .get("stderr_blob")
                            .and_then(|x| x.as_str())
                            .unwrap_or("");
                        if !stderr_blob.is_empty() {
                            items.push(serde_json::json!({
                                "type": "blob_ref",
                                "event_id": ev.event_id,
                                "blob_id": stderr_blob,
                                "kind": "stderr",
                            }));
                            preview_lines.push(format!(
                                "[blob_ref] stderr {} ({})",
                                truncate(stderr_blob, 40),
                                ev.event_id
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(AutoEvidenceResult {
        items,
        preview_lines,
    })
}

pub fn last_commit_contribution(ledger: &Ledger, branch: &str) -> Result<Option<String>> {
    let mut last: Option<String> = None;
    for ev in ledger.iter_events()? {
        if ev.branch != branch || ev.event_type != "commit" {
            continue;
        }
        let contrib = ev
            .payload
            .get("contribution")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        last = Some(contrib);
    }
    Ok(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::setup_workspace;
    use edda_core::event::{
        new_note_event, new_commit_event, new_cmd_event,
        CommitEventParams, CmdEventParams,
    };

    #[test]
    fn build_auto_evidence_collects_todos_and_fails() {
        let (tmp, ledger) = setup_workspace();

        // Add a todo note
        let todo_tags = vec!["todo".to_string()];
        let note = new_note_event("main", None, "user", "remember to test", &todo_tags).unwrap();
        ledger.append_event(&note, false).unwrap();

        // Add a failed command
        let argv = vec!["cargo".to_string(), "test".to_string()];
        let cmd = new_cmd_event(&CmdEventParams {
            branch: "main",
            parent_hash: None,
            argv: &argv,
            cwd: ".",
            exit_code: 1,
            duration_ms: 500,
            stdout_blob: "",
            stderr_blob: "blob:sha256:err123",
        }).unwrap();
        ledger.append_event(&cmd, false).unwrap();

        // Add a regular note (not evidence)
        let note2 = new_note_event("main", None, "user", "just a note", &[]).unwrap();
        ledger.append_event(&note2, false).unwrap();

        let result = build_auto_evidence(&ledger, "main", 20).unwrap();

        // Should have at least the todo + failed cmd
        assert!(result.items.len() >= 2, "expected >=2 items, got {}", result.items.len());
        assert_eq!(result.items.len(), result.preview_lines.len());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn last_commit_contribution_returns_latest() {
        let (tmp, ledger) = setup_workspace();

        // No commits yet
        let result = last_commit_contribution(&ledger, "main").unwrap();
        assert!(result.is_none());

        // First commit
        let mut params = CommitEventParams {
            branch: "main",
            parent_hash: None,
            title: "first",
            purpose: None,
            prev_summary: "",
            contribution: "alpha work",
            evidence: vec![],
            labels: vec![],
        };
        let c1 = new_commit_event(&mut params).unwrap();
        ledger.append_event(&c1, false).unwrap();

        // Second commit
        let mut params2 = CommitEventParams {
            branch: "main",
            parent_hash: None,
            title: "second",
            purpose: None,
            prev_summary: "",
            contribution: "beta work",
            evidence: vec![],
            labels: vec![],
        };
        let c2 = new_commit_event(&mut params2).unwrap();
        ledger.append_event(&c2, false).unwrap();

        let result = last_commit_contribution(&ledger, "main").unwrap();
        assert_eq!(result.as_deref(), Some("beta work"));

        // Wrong branch returns None
        let result = last_commit_contribution(&ledger, "nonexistent").unwrap();
        assert!(result.is_none());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
