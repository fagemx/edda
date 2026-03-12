use std::collections::{BTreeMap, BTreeSet};
use std::io::BufRead;
use std::path::Path;

use super::helpers::{
    compute_duration_minutes, extract_bash_command, extract_envelope_cwd, extract_exit_code,
    extract_file_path, extract_git_commit_msg,
};
use super::{ActivityType, DigestTaskSnapshot, FailedCommand, SessionOutcome, SessionStats};

pub fn extract_stats(session_ledger_path: &Path) -> anyhow::Result<SessionStats> {
    let mut stats = SessionStats::default();
    let mut files_set: BTreeSet<String> = BTreeSet::new();
    let mut file_edit_map: BTreeMap<String, u64> = BTreeMap::new();

    // Track session outcome: last event type + trailing failure count
    let mut last_event_name = String::new();
    let mut trailing_failures: u32 = 0;

    if !session_ledger_path.exists() {
        return Ok(stats);
    }

    let file = std::fs::File::open(session_ledger_path)?;
    let reader = std::io::BufReader::new(file);

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let envelope: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue, // skip malformed lines
        };

        // Track timestamps for duration
        let ts = envelope.get("ts").and_then(|v| v.as_str()).unwrap_or("");
        if !ts.is_empty() {
            if stats.first_ts.is_none() {
                stats.first_ts = Some(ts.to_string());
            }
            stats.last_ts = Some(ts.to_string());
        }

        let event_name = envelope
            .get("hook_event_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Track trailing failures for outcome detection
        if event_name == "PostToolUseFailure" {
            trailing_failures += 1;
        } else if event_name == "PostToolUse" {
            trailing_failures = 0;
        }
        if !event_name.is_empty() {
            last_event_name = event_name.to_string();
        }

        match event_name {
            "PostToolUse" => {
                stats.tool_calls += 1;
                // Extract tool_name and accumulate per-tool breakdown
                let tool_name = envelope
                    .get("tool_name")
                    .or_else(|| {
                        envelope
                            .get("raw")
                            .and_then(|r| r.get("toolName").or_else(|| r.get("tool_name")))
                    })
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !tool_name.is_empty() {
                    *stats
                        .tool_call_breakdown
                        .entry(tool_name.to_string())
                        .or_insert(0) += 1;
                }
                if tool_name == "Edit" || tool_name == "Write" {
                    if let Some(fp) = extract_file_path(&envelope) {
                        if !crate::signals::is_noise_file(&fp) {
                            files_set.insert(fp.clone());
                            *file_edit_map.entry(fp).or_insert(0) += 1;
                        }
                    }
                }
                if tool_name == "Bash" {
                    if let Some(cmd) = extract_bash_command(&envelope) {
                        if cmd.contains("git commit") {
                            let msg = extract_git_commit_msg(&cmd);
                            if !msg.is_empty() {
                                stats.commits_made.push(msg);
                            }
                        }
                        if let Some(pkg) = crate::nudge::extract_dependency_add(&cmd) {
                            if !stats.deps_added.contains(&pkg) {
                                stats.deps_added.push(pkg);
                            }
                        }
                    }
                }
            }
            "PostToolUseFailure" => {
                stats.tool_failures += 1;
                // Extract failed Bash commands
                let tool_name = envelope
                    .get("tool_name")
                    .or_else(|| {
                        envelope
                            .get("raw")
                            .and_then(|r| r.get("toolName").or_else(|| r.get("tool_name")))
                    })
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if tool_name == "Bash" {
                    if let Some(cmd) = extract_bash_command(&envelope) {
                        let cwd_val = extract_envelope_cwd(&envelope);
                        let exit_code = extract_exit_code(&envelope);
                        stats.failed_cmds_detail.push(FailedCommand {
                            command: cmd.clone(),
                            cwd: cwd_val,
                            exit_code,
                        });
                        stats.failed_commands.push(cmd);
                    }
                }
            }
            "UserPromptSubmit" => {
                stats.user_prompts += 1;
            }
            _ => {}
        }
    }

    stats.files_modified = files_set.into_iter().collect();
    stats.file_edit_counts = file_edit_map.into_iter().collect();
    stats.duration_minutes = compute_duration_minutes(&stats.first_ts, &stats.last_ts);

    // Determine session outcome
    stats.outcome = if trailing_failures >= 3 {
        SessionOutcome::ErrorStuck
    } else if last_event_name == "UserPromptSubmit" {
        SessionOutcome::Interrupted
    } else {
        SessionOutcome::Completed
    };

    // Classify activity based on tool patterns
    stats.activity = classify_activity(&stats);

    Ok(stats)
}

/// Load tasks snapshot from state/active_tasks.json for a project.
/// Returns empty vec if file doesn't exist or can't be parsed.
pub fn load_tasks_for_digest(project_id: &str) -> Vec<DigestTaskSnapshot> {
    let path = edda_store::project_dir(project_id)
        .join("state")
        .join("active_tasks.json");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let val: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    val.get("tasks")
        .and_then(|t| {
            t.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let subject = item.get("subject")?.as_str()?.to_string();
                        let status = item.get("status")?.as_str()?.to_string();
                        Some(DigestTaskSnapshot { subject, status })
                    })
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// Build the deterministic text summary from stats.
pub fn render_digest_text(session_id: &str, stats: &SessionStats) -> String {
    let mut lines = Vec::new();

    lines.push(format!(
        "Session {}: {} tool calls, {} failures, {} user prompts, {} min",
        &session_id[..session_id.len().min(8)],
        stats.tool_calls,
        stats.tool_failures,
        stats.user_prompts,
        stats.duration_minutes,
    ));

    if !stats.files_modified.is_empty() {
        lines.push(format!(
            "Files modified: {}",
            stats.files_modified.join(", ")
        ));
    }

    if !stats.commits_made.is_empty() {
        lines.push("Commits:".to_string());
        for msg in &stats.commits_made {
            let display = if msg.len() > 120 {
                let end = msg.floor_char_boundary(117);
                format!("{}...", &msg[..end])
            } else {
                msg.clone()
            };
            lines.push(format!("  - {display}"));
        }
    }

    if !stats.tasks_snapshot.is_empty() {
        let done: Vec<_> = stats
            .tasks_snapshot
            .iter()
            .filter(|t| t.status == "completed")
            .map(|t| t.subject.as_str())
            .collect();
        let wip: Vec<_> = stats
            .tasks_snapshot
            .iter()
            .filter(|t| t.status != "completed")
            .map(|t| t.subject.as_str())
            .collect();
        if !done.is_empty() {
            lines.push(format!("Done: {}", done.join(", ")));
        }
        if !wip.is_empty() {
            lines.push(format!("WIP: {}", wip.join(", ")));
        }
    }

    if !stats.failed_commands.is_empty() {
        lines.push("Failed commands:".to_string());
        for cmd in &stats.failed_commands {
            // Truncate long commands (char-boundary safe)
            let display = if cmd.len() > 120 {
                let end = cmd.floor_char_boundary(117);
                format!("{}...", &cmd[..end])
            } else {
                cmd.clone()
            };
            lines.push(format!("  - {display}"));
        }
    }

    // Tool breakdown
    if !stats.tool_call_breakdown.is_empty() {
        let breakdown: Vec<String> = stats
            .tool_call_breakdown
            .iter()
            .map(|(k, v)| format!("{k}:{v}"))
            .collect();
        lines.push(format!("Tools: {}", breakdown.join(", ")));
        let (edit_ratio, search_ratio) =
            compute_tool_ratios(&stats.tool_call_breakdown, stats.tool_calls);
        if edit_ratio > 0.0 || search_ratio > 0.0 {
            lines.push(format!(
                "Ratios: edit={:.0}% search={:.0}%",
                edit_ratio * 100.0,
                search_ratio * 100.0
            ));
        }
    }

    // Usage summary
    if stats.input_tokens > 0 || stats.output_tokens > 0 {
        let model_label = if stats.model.is_empty() {
            "unknown".to_string()
        } else {
            stats.model.clone()
        };
        let total = stats.input_tokens + stats.output_tokens;
        let cost_str = if stats.estimated_cost_usd > 0.0 {
            format!(", ${:.4}", stats.estimated_cost_usd)
        } else {
            String::new()
        };
        lines.push(format!(
            "Usage: {model_label} -- {total} tokens (in:{} out:{}){cost_str}",
            stats.input_tokens, stats.output_tokens
        ));
    }

    lines.join("\n")
}

/// Compute edit and search ratios from the tool call breakdown.
///
/// - `edit_ratio` = (Edit + Write + NotebookEdit) / total
/// - `search_ratio` = (Read + Grep + Glob + Agent) / total
pub(super) fn compute_tool_ratios(breakdown: &BTreeMap<String, u64>, total: u64) -> (f64, f64) {
    if total == 0 {
        return (0.0, 0.0);
    }
    let edit_tools: u64 = ["Edit", "Write", "NotebookEdit"]
        .iter()
        .filter_map(|t| breakdown.get(*t))
        .sum();
    let search_tools: u64 = ["Read", "Grep", "Glob", "Agent"]
        .iter()
        .filter_map(|t| breakdown.get(*t))
        .sum();
    (
        edit_tools as f64 / total as f64,
        search_tools as f64 / total as f64,
    )
}

/// Classify session activity based on tool call patterns and file types.
pub(super) fn classify_activity(stats: &SessionStats) -> ActivityType {
    if stats.tool_calls == 0 && stats.user_prompts == 0 {
        return ActivityType::Unknown;
    }

    let total = stats.tool_calls;
    if total == 0 {
        // Only user prompts, no tools
        return ActivityType::Chat;
    }

    let breakdown = &stats.tool_call_breakdown;

    // Compute ratios
    let edit_count: u64 = ["Edit", "Write"]
        .iter()
        .filter_map(|t| breakdown.get(*t))
        .sum();
    let search_count: u64 = ["Read", "Grep", "Glob", "Agent"]
        .iter()
        .filter_map(|t| breakdown.get(*t))
        .sum();
    let bash_count = breakdown.get("Bash").unwrap_or(&0);

    let edit_ratio = edit_count as f64 / total as f64;
    let search_ratio = search_count as f64 / total as f64;
    let bash_ratio = *bash_count as f64 / total as f64;

    // Check for docs-only edits
    let all_docs = stats.files_modified.iter().all(|f| f.ends_with(".md"));
    if all_docs && edit_ratio > 0.0 && !stats.files_modified.is_empty() {
        return ActivityType::Docs;
    }

    // High search, low edit = research
    if search_ratio > 0.6 && edit_ratio < 0.1 {
        return ActivityType::Research;
    }

    // Many failures = debugging
    if stats.tool_failures > 3 && stats.tool_failures as f64 / total as f64 > 0.2 {
        return ActivityType::Debug;
    }

    // Git commits + edits = feature or fix
    if !stats.commits_made.is_empty() && edit_ratio > 0.1 {
        // Check commit messages for fix/bug keywords
        let commit_text = stats.commits_made.join(" ").to_lowercase();
        if commit_text.contains("fix") || commit_text.contains("bug") {
            return ActivityType::Fix;
        }
        return ActivityType::Feature;
    }

    // Bash-heavy = ops
    if bash_ratio > 0.4 {
        return ActivityType::Ops;
    }

    // High edit ratio = feature
    if edit_ratio > 0.3 {
        return ActivityType::Feature;
    }

    // Low tool calls = chat
    if stats.tool_calls < 5 {
        return ActivityType::Chat;
    }

    ActivityType::Unknown
}
