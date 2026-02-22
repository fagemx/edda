use anyhow::Result;
use edda_core::Event;
use edda_ledger::Ledger;
use serde_json::Value;

use crate::types::*;

// ── Helpers ──

pub(crate) fn as_str(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

pub(crate) fn as_arr_str(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|i| i.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn fmt_cmd_argv(payload: &Value) -> String {
    payload
        .get("argv")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|i| i.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

pub(crate) fn fmt_evidence_item(item: &Value) -> Option<String> {
    if let Some(s) = item.as_str() {
        return Some(s.to_string());
    }
    if let Some(obj) = item.as_object() {
        let why = obj.get("why").and_then(|x| x.as_str()).unwrap_or("").trim();
        if let Some(eid) = obj.get("event_id").and_then(|x| x.as_str()) {
            return Some(if why.is_empty() {
                eid.to_string()
            } else {
                format!("{eid}: {why}")
            });
        }
        if let Some(blob) = obj.get("blob").and_then(|x| x.as_str()) {
            return Some(if why.is_empty() {
                blob.to_string()
            } else {
                format!("{blob}: {why}")
            });
        }
    }
    None
}

// ── Snapshot builder ──

pub(crate) fn collect_branch_events(ledger: &Ledger, branch: &str) -> Result<Vec<Event>> {
    Ok(ledger
        .iter_events()?
        .into_iter()
        .filter(|ev| ev.branch == branch)
        .collect())
}

/// Look for a `branch_create` event whose payload.name matches the branch,
/// and return its timestamp as a fallback created_at.
pub(crate) fn resolve_branch_created_at_fallback(
    ledger: &Ledger,
    branch: &str,
) -> Result<Option<String>> {
    for ev in ledger.iter_events()? {
        if ev.event_type == "branch_create" {
            if let Some(name) = ev.payload.get("name").and_then(|x| x.as_str()) {
                if name == branch {
                    return Ok(Some(ev.ts.clone()));
                }
            }
        }
    }
    Ok(None)
}

pub(crate) fn build_branch_snapshot(ledger: &Ledger, branch: &str) -> Result<BranchSnapshot> {
    let branch_events = collect_branch_events(ledger, branch)?;

    let mut created_at = branch_events
        .first()
        .map(|e| e.ts.clone())
        .unwrap_or_default();
    // Fallback: if no events on this branch, check for a branch_create event
    if created_at.is_empty() {
        if let Some(ts) = resolve_branch_created_at_fallback(ledger, branch)? {
            created_at = ts;
        }
    }
    let last_event_id = branch_events.last().map(|e| e.event_id.clone());

    let mut commits: Vec<CommitEntry> = Vec::new();
    let mut signals: Vec<SignalEntry> = Vec::new();
    let mut merges: Vec<MergeEntry> = Vec::new();
    let mut session_digests: Vec<SessionDigestEntry> = Vec::new();
    let mut last_commit_event_index: Option<usize> = None;

    for (idx, ev) in branch_events.iter().enumerate() {
        match ev.event_type.as_str() {
            "commit" => {
                last_commit_event_index = Some(idx);
                let p = &ev.payload;
                let evidence_lines = p
                    .get("evidence")
                    .and_then(|x| x.as_array())
                    .map(|arr| arr.iter().filter_map(fmt_evidence_item).collect())
                    .unwrap_or_default();

                commits.push(CommitEntry {
                    ts: ev.ts.clone(),
                    event_id: ev.event_id.clone(),
                    title: as_str(p, "title"),
                    purpose: as_str(p, "purpose"),
                    prev_summary: as_str(p, "prev_summary"),
                    contribution: as_str(p, "contribution"),
                    evidence_lines,
                    labels: as_arr_str(p, "labels"),
                });
            }
            "note" => {
                let tags: Vec<&str> = ev
                    .payload
                    .get("tags")
                    .and_then(|x| x.as_array())
                    .map(|arr| arr.iter().filter_map(|i| i.as_str()).collect())
                    .unwrap_or_default();

                if tags.contains(&"todo") {
                    let text = ev
                        .payload
                        .get("text")
                        .and_then(|x| x.as_str())
                        .unwrap_or("");
                    signals.push(SignalEntry {
                        ts: ev.ts.clone(),
                        kind: SignalKind::NoteTodo,
                        text: text.to_string(),
                        event_id: ev.event_id.clone(),
                        supersedes: None,
                    });
                }

                if tags.contains(&"decision") {
                    let text = ev
                        .payload
                        .get("text")
                        .and_then(|x| x.as_str())
                        .unwrap_or("");

                    // Extract supersession target from provenance
                    let supersedes = ev
                        .refs
                        .provenance
                        .iter()
                        .find(|p| p.rel == "supersedes")
                        .map(|p| p.target.clone());

                    signals.push(SignalEntry {
                        ts: ev.ts.clone(),
                        kind: SignalKind::NoteDecision,
                        text: text.to_string(),
                        event_id: ev.event_id.clone(),
                        supersedes,
                    });
                }

                if tags.contains(&"session_digest") {
                    let stats = ev.payload.get("session_stats");
                    let sid = ev
                        .payload
                        .get("session_id")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    session_digests.push(SessionDigestEntry {
                        ts: ev.ts.clone(),
                        event_id: ev.event_id.clone(),
                        session_id: sid,
                        tool_calls: stats
                            .and_then(|s| s.get("tool_calls"))
                            .and_then(|x| x.as_u64())
                            .unwrap_or(0),
                        tool_failures: stats
                            .and_then(|s| s.get("tool_failures"))
                            .and_then(|x| x.as_u64())
                            .unwrap_or(0),
                        user_prompts: stats
                            .and_then(|s| s.get("user_prompts"))
                            .and_then(|x| x.as_u64())
                            .unwrap_or(0),
                        duration_minutes: stats
                            .and_then(|s| s.get("duration_minutes"))
                            .and_then(|x| x.as_u64())
                            .unwrap_or(0),
                        files_modified: stats
                            .and_then(|s| s.get("files_modified"))
                            .and_then(|x| x.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|i| i.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        failed_commands: stats
                            .and_then(|s| s.get("failed_commands"))
                            .and_then(|x| x.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|i| i.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        commits_made: stats
                            .and_then(|s| s.get("commits_made"))
                            .and_then(|x| x.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|i| i.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        tasks_snapshot: stats
                            .and_then(|s| s.get("tasks_snapshot"))
                            .and_then(|x| x.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|item| {
                                        let subject = item.get("subject")?.as_str()?.to_string();
                                        let status = item.get("status")?.as_str()?.to_string();
                                        Some(TaskSnapshotEntry { subject, status })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        outcome: stats
                            .and_then(|s| s.get("outcome"))
                            .and_then(|x| x.as_str())
                            .unwrap_or("completed")
                            .to_string(),
                        notes: stats
                            .and_then(|s| s.get("notes"))
                            .and_then(|x| x.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|i| i.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    });
                }
            }
            "cmd" => {
                let exit_code = ev
                    .payload
                    .get("exit_code")
                    .and_then(|x| x.as_i64())
                    .unwrap_or(0);
                // Skip phantom commands (bridge-ingested records that were never
                // actually executed: duration_ms == 0 with a failure exit code).
                let duration_ms = ev
                    .payload
                    .get("duration_ms")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                if exit_code != 0 && duration_ms > 0 {
                    let argv = fmt_cmd_argv(&ev.payload);
                    signals.push(SignalEntry {
                        ts: ev.ts.clone(),
                        kind: SignalKind::CmdFail,
                        text: format!("{argv} (exit={exit_code})"),
                        event_id: ev.event_id.clone(),
                        supersedes: None,
                    });
                }
            }
            "merge" => {
                let p = &ev.payload;
                merges.push(MergeEntry {
                    ts: ev.ts.clone(),
                    event_id: ev.event_id.clone(),
                    src: as_str(p, "src"),
                    dst: as_str(p, "dst"),
                    reason: as_str(p, "reason"),
                    adopted_commits: as_arr_str(p, "adopted_commits"),
                });
            }
            _ => {}
        }
    }

    let last_commit = commits.last().cloned();
    let last_commit_id = last_commit.as_ref().map(|c| c.event_id.clone());
    let uncommitted_events = match last_commit_event_index {
        Some(i) => branch_events.len().saturating_sub(i + 1),
        None => branch_events.len(),
    };

    Ok(BranchSnapshot {
        branch: branch.to_string(),
        created_at,
        last_event_id,
        last_commit_id,
        last_commit,
        commits,
        signals,
        merges,
        session_digests,
        uncommitted_events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use edda_core::event::{new_cmd_event, CmdEventParams};

    #[test]
    fn phantom_cmd_not_a_signal() {
        let (_, ledger) = crate::test_support::setup_workspace();

        // A phantom cmd (bridge-ingested, never executed): duration_ms = 0
        let argv_phantom = vec!["cargo".to_string(), "check".to_string()];
        let phantom = new_cmd_event(&CmdEventParams {
            branch: "main",
            parent_hash: None,
            argv: &argv_phantom,
            cwd: ".",
            exit_code: -1,
            duration_ms: 0,
            stdout_blob: "",
            stderr_blob: "",
        })
        .unwrap();
        ledger.append_event(&phantom).unwrap();

        // A real failed cmd: duration_ms > 0
        let argv_real = vec!["cargo".to_string(), "test".to_string()];
        let real = new_cmd_event(&CmdEventParams {
            branch: "main",
            parent_hash: None,
            argv: &argv_real,
            cwd: ".",
            exit_code: 1,
            duration_ms: 350,
            stdout_blob: "",
            stderr_blob: "",
        })
        .unwrap();
        ledger.append_event(&real).unwrap();

        let snap = build_branch_snapshot(&ledger, "main").unwrap();

        // Only the real cmd should produce a signal
        assert_eq!(snap.signals.len(), 1);
        assert!(snap.signals[0].text.contains("cargo test"));
    }
}
