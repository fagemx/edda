use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::parse::now_rfc3339;

// ── Session Signals (extracted from transcript) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub id: String,
    pub subject: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FileEditCount {
    pub path: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CommitInfo {
    pub hash: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FailedBashCmd {
    pub command_base: String,
    pub stderr_snippet: String,
    pub count: usize,
}

/// Accumulated token usage from assistant messages in a transcript.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UsageSnapshot {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
}

impl UsageSnapshot {
    #[cfg(test)]
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// All signals extracted from a single transcript scan.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct SessionSignals {
    pub tasks: Vec<TaskSnapshot>,
    pub files_modified: Vec<FileEditCount>,
    pub commits: Vec<CommitInfo>,
    #[serde(default)]
    pub failed_commands: Vec<FailedBashCmd>,
    #[serde(default)]
    pub usage: UsageSnapshot,
}

/// Lightweight summary extracted when a sub-agent completes.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct SubagentSummary {
    pub summary: String,
    pub files_touched: Vec<String>,
    pub commits: Vec<String>,
    pub decisions: Vec<String>,
}

/// One-pass transcript scan: extract tasks, files modified, and commits.
pub(crate) fn extract_session_signals(transcript_store_path: &Path) -> SessionSignals {
    use std::io::BufRead;

    let file = match fs::File::open(transcript_store_path) {
        Ok(f) => f,
        Err(_) => return SessionSignals::default(),
    };

    let mut usage = UsageSnapshot::default();

    let mut tasks: std::collections::HashMap<String, TaskSnapshot> =
        std::collections::HashMap::new();
    let mut next_task_id: usize = 1;

    let mut file_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    let mut pending_commits: std::collections::HashMap<String, String> =
        std::collections::HashMap::new(); // tool_use_id -> commit_msg_from_cmd
    let mut commits: Vec<CommitInfo> = Vec::new();

    let mut pending_bash: std::collections::HashMap<String, String> =
        std::collections::HashMap::new(); // tool_use_id -> command
    let mut failed_cmd_map: std::collections::HashMap<String, (String, usize)> =
        std::collections::HashMap::new(); // command_base -> (stderr_snippet, count)

    for line in std::io::BufReader::new(file).lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let record: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let record_type = record.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match record_type {
            "system" => {
                // Extract model from system record
                if let Some(model) = record.get("model").and_then(|m| m.as_str()) {
                    if !model.is_empty() {
                        usage.model = model.to_string();
                    }
                }
            }
            "assistant" => {
                // Accumulate usage from assistant messages
                if let Some(u) = record.get("message").and_then(|m| m.get("usage")) {
                    usage.input_tokens +=
                        u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                    usage.output_tokens +=
                        u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                    usage.cache_read_tokens += u
                        .get("cache_read_input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    usage.cache_creation_tokens += u
                        .get("cache_creation_input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                }
                // Also try model from assistant message
                if usage.model.is_empty() {
                    if let Some(model) = record
                        .get("message")
                        .and_then(|m| m.get("model"))
                        .and_then(|m| m.as_str())
                    {
                        usage.model = model.to_string();
                    }
                }
                let content = match record
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    Some(c) => c,
                    None => continue,
                };

                for item in content {
                    if item.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                        continue;
                    }
                    let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let input = match item.get("input") {
                        Some(i) => i,
                        None => continue,
                    };
                    let tool_use_id = item.get("id").and_then(|s| s.as_str()).unwrap_or("");

                    match name {
                        "TaskCreate" => {
                            let id = next_task_id.to_string();
                            next_task_id += 1;
                            let subject = input
                                .get("subject")
                                .and_then(|s| s.as_str())
                                .unwrap_or("")
                                .to_string();
                            tasks.insert(
                                id.clone(),
                                TaskSnapshot {
                                    id,
                                    subject,
                                    status: "pending".to_string(),
                                },
                            );
                        }
                        "TaskUpdate" => {
                            let task_id =
                                input.get("taskId").and_then(|s| s.as_str()).unwrap_or("");
                            if let Some(task) = tasks.get_mut(task_id) {
                                if let Some(s) = input.get("status").and_then(|s| s.as_str()) {
                                    task.status = s.to_string();
                                }
                                if let Some(s) = input.get("subject").and_then(|s| s.as_str()) {
                                    task.subject = s.to_string();
                                }
                            }
                        }
                        "Edit" | "Write" => {
                            if let Some(fp) = input.get("file_path").and_then(|s| s.as_str()) {
                                if !is_noise_file(fp) {
                                    *file_counts.entry(fp.to_string()).or_insert(0) += 1;
                                }
                            }
                        }
                        "Bash" => {
                            if let Some(cmd) = input.get("command").and_then(|s| s.as_str()) {
                                pending_bash.insert(tool_use_id.to_string(), cmd.to_string());
                                if cmd.contains("git commit") {
                                    // Extract message from -m flag if present
                                    let msg = extract_commit_msg_from_cmd(cmd);
                                    pending_commits.insert(tool_use_id.to_string(), msg);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            "user" => {
                // Look for tool_results that match pending git commit calls
                let content = match record
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    Some(c) => c,
                    None => continue,
                };

                for item in content {
                    if item.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                        continue;
                    }
                    let tool_use_id = item
                        .get("tool_use_id")
                        .and_then(|s| s.as_str())
                        .unwrap_or("");
                    // Extract result text (shared between commit and error processing)
                    let result_text = item
                        .get("content")
                        .and_then(|c| {
                            if let Some(s) = c.as_str() {
                                Some(s.to_string())
                            } else if let Some(arr) = c.as_array() {
                                arr.iter()
                                    .find_map(|x| x.get("text").and_then(|t| t.as_str()))
                                    .map(|s| s.to_string())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default();

                    // Check for git commit results
                    if let Some(cmd_msg) = pending_commits.remove(tool_use_id) {
                        if let Some(ci) = parse_commit_result(&result_text, &cmd_msg) {
                            commits.push(ci);
                        }
                    }

                    // Check for failed Bash commands
                    let is_error = item
                        .get("is_error")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if is_error {
                        if let Some(cmd) = pending_bash.remove(tool_use_id) {
                            let base = command_base(&cmd);
                            let snippet = truncate_stderr(&result_text, 200);
                            let entry = failed_cmd_map
                                .entry(base)
                                .or_insert_with(|| (snippet.clone(), 0));
                            entry.1 += 1;
                            // Keep the most recent stderr snippet
                            if !snippet.is_empty() {
                                entry.0 = snippet;
                            }
                        }
                    } else {
                        // Successful result — healing: clear stale failures for this command
                        if let Some(cmd) = pending_bash.remove(tool_use_id) {
                            let base = command_base(&cmd);
                            failed_cmd_map.remove(&base);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Sort tasks by ID
    let mut sorted_tasks: Vec<TaskSnapshot> = tasks.into_values().collect();
    sorted_tasks.sort_by(|a, b| {
        a.id.parse::<usize>()
            .unwrap_or(0)
            .cmp(&b.id.parse::<usize>().unwrap_or(0))
    });

    // Sort files by count descending
    let mut sorted_files: Vec<FileEditCount> = file_counts
        .into_iter()
        .map(|(path, count)| FileEditCount { path, count })
        .collect();
    sorted_files.sort_by(|a, b| b.count.cmp(&a.count));

    // Build failed commands list, sorted by count descending
    let mut failed_commands: Vec<FailedBashCmd> = failed_cmd_map
        .into_iter()
        .map(|(command_base, (stderr_snippet, count))| FailedBashCmd {
            command_base,
            stderr_snippet,
            count,
        })
        .collect();
    failed_commands.sort_by(|a, b| b.count.cmp(&a.count));

    SessionSignals {
        tasks: sorted_tasks,
        files_modified: sorted_files,
        commits,
        failed_commands,
        usage,
    }
}

/// Extract commit message from a `git commit -m "..."` command string.
pub(crate) fn extract_commit_msg_from_cmd(cmd: &str) -> String {
    // Try to find -m "..." or -m '...' pattern
    // Also handle heredoc: -m "$(cat <<'EOF'\nmessage\nEOF\n)"
    if let Some(pos) = cmd.find("-m ") {
        let after_m = &cmd[pos + 3..];
        // Skip whitespace
        let trimmed = after_m.trim_start();
        if let Some(first) = trimmed.chars().next() {
            if first == '"' || first == '\'' {
                // Find matching close quote (simple, doesn't handle escapes)
                if let Some(end) = trimmed[1..].find(first) {
                    return trimmed[1..end + 1].to_string();
                }
            }
        }
    }
    String::new()
}

/// Parse git commit output to extract hash and message.
/// Format: "[branch hash] message\n ..."
pub(crate) fn parse_commit_result(result: &str, fallback_msg: &str) -> Option<CommitInfo> {
    // Pattern: [main abc1234] commit message
    for line in result.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            if let Some(bracket_end) = line.find(']') {
                let inside = &line[1..bracket_end];
                let hash = inside.split_whitespace().last().unwrap_or("").to_string();
                let message = line[bracket_end + 1..].trim().to_string();
                if !hash.is_empty() {
                    return Some(CommitInfo {
                        hash,
                        message: if message.is_empty() {
                            fallback_msg.to_string()
                        } else {
                            message
                        },
                    });
                }
            }
        }
    }
    None
}

const SUBAGENT_TRANSCRIPT_MAX_BYTES: usize = 512 * 1024;
const SUBAGENT_MAX_FILES: usize = 8;
const SUBAGENT_MAX_COMMITS: usize = 5;
const SUBAGENT_MAX_DECISIONS: usize = 5;
const SUBAGENT_SUMMARY_MAX_CHARS: usize = 220;

/// Extract a compact sub-agent summary from transcript and fallback text.
///
/// Priority:
/// 1) Parse transcript JSONL at `agent_transcript_path` when available
/// 2) Fallback to `last_assistant_message`
pub(crate) fn extract_subagent_summary(
    agent_transcript_path: &str,
    last_assistant_message: &str,
    agent_id: &str,
) -> SubagentSummary {
    let transcript = resolve_subagent_transcript_path(agent_transcript_path, agent_id);

    if let Some(path) = transcript.as_deref() {
        if let Some(mut summary) = extract_subagent_summary_from_transcript(path) {
            if summary.summary.is_empty() {
                summary.summary = build_subagent_summary_line(&summary);
            }
            if summary.summary.is_empty() {
                summary.summary = fallback_summary_text(last_assistant_message);
            }
            return summary;
        }
    }

    extract_subagent_summary_from_message(last_assistant_message)
}

fn resolve_subagent_transcript_path(path: &str, agent_id: &str) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }

    // Most payloads are plain filesystem paths.
    let direct = PathBuf::from(path);
    if direct.is_file() {
        return Some(direct);
    }

    // Defensive fallback for sidechain-like pointers that include separators.
    // Example shape: "/repo/.claude/transcript.jsonl::sidechain:agent-123"
    let separators = ["::", "#", "?"];
    for sep in separators {
        if let Some((base, _)) = path.split_once(sep) {
            let candidate = PathBuf::from(base);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    // Last attempt: if agent_id appears in path metadata and a sibling jsonl exists,
    // prefer that file, else give up.
    if !agent_id.is_empty() {
        if let Some(parent) = direct.parent() {
            if parent.is_dir() {
                let candidate = parent.join(format!("{agent_id}.jsonl"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

fn extract_subagent_summary_from_transcript(path: &Path) -> Option<SubagentSummary> {
    use std::io::{BufRead, BufReader};

    let meta = fs::metadata(path).ok()?;
    if meta.len() == 0 {
        return None;
    }

    // Bound scan cost for very large transcripts.
    if meta.len() > SUBAGENT_TRANSCRIPT_MAX_BYTES as u64 {
        return None;
    }

    let file = fs::File::open(path).ok()?;
    let mut files: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut commits: Vec<String> = Vec::new();
    let mut decisions: Vec<String> = Vec::new();
    let mut pending_commit_msgs: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut last_text: String = String::new();

    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        if line.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };

        let rtype = record.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if rtype == "assistant" {
            let content = record
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array());

            if let Some(arr) = content {
                for block in arr {
                    let btype = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match btype {
                        "tool_use" => {
                            let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let input = block.get("input").unwrap_or(&serde_json::Value::Null);
                            let tool_use_id =
                                block.get("id").and_then(|v| v.as_str()).unwrap_or("");

                            if (name == "Edit" || name == "Write")
                                && input.get("file_path").and_then(|v| v.as_str()).is_some()
                            {
                                let fp = input
                                    .get("file_path")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                if !fp.is_empty() && !is_noise_file(fp) {
                                    files.insert(fp.to_string());
                                }
                            }

                            if name == "Bash" {
                                let cmd =
                                    input.get("command").and_then(|v| v.as_str()).unwrap_or("");
                                if !tool_use_id.is_empty() && cmd.contains("git commit") {
                                    pending_commit_msgs.insert(
                                        tool_use_id.to_string(),
                                        extract_commit_msg_from_cmd(cmd),
                                    );
                                }
                                extract_decisions_from_text(cmd, &mut decisions);
                            }
                        }
                        "text" => {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                if !text.trim().is_empty() {
                                    last_text = text.to_string();
                                    extract_decisions_from_text(text, &mut decisions);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        } else if rtype == "user" {
            let content = record
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array());
            if let Some(arr) = content {
                for block in arr {
                    if block.get("type").and_then(|v| v.as_str()) != Some("tool_result") {
                        continue;
                    }
                    let tool_use_id = block
                        .get("tool_use_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if let Some(fallback_msg) = pending_commit_msgs.remove(tool_use_id) {
                        let result_text = tool_result_text(block);
                        if let Some(ci) = parse_commit_result(&result_text, &fallback_msg) {
                            commits.push(format!("{} {}", ci.hash, ci.message));
                        }
                    }
                }
            }
        }
    }

    let mut files_touched: Vec<String> = files.into_iter().collect();
    files_touched.sort();
    if files_touched.len() > SUBAGENT_MAX_FILES {
        files_touched.truncate(SUBAGENT_MAX_FILES);
    }
    dedup_keep_order(&mut commits);
    if commits.len() > SUBAGENT_MAX_COMMITS {
        commits.truncate(SUBAGENT_MAX_COMMITS);
    }
    dedup_keep_order(&mut decisions);
    if decisions.len() > SUBAGENT_MAX_DECISIONS {
        decisions.truncate(SUBAGENT_MAX_DECISIONS);
    }

    let mut summary = SubagentSummary {
        summary: String::new(),
        files_touched,
        commits,
        decisions,
    };

    // Prefer signal-derived line, but keep last assistant text as fallback seed.
    summary.summary = build_subagent_summary_line(&summary);
    if summary.summary.is_empty() {
        summary.summary = fallback_summary_text(&last_text);
    }

    if summary.summary.is_empty()
        && summary.files_touched.is_empty()
        && summary.commits.is_empty()
        && summary.decisions.is_empty()
    {
        None
    } else {
        Some(summary)
    }
}

fn extract_subagent_summary_from_message(last_assistant_message: &str) -> SubagentSummary {
    let mut decisions = Vec::new();
    extract_decisions_from_text(last_assistant_message, &mut decisions);
    dedup_keep_order(&mut decisions);
    if decisions.len() > SUBAGENT_MAX_DECISIONS {
        decisions.truncate(SUBAGENT_MAX_DECISIONS);
    }

    let summary = fallback_summary_text(last_assistant_message);
    SubagentSummary {
        summary,
        files_touched: Vec::new(),
        commits: Vec::new(),
        decisions,
    }
}

fn tool_result_text(block: &serde_json::Value) -> String {
    block
        .get("content")
        .and_then(|c| {
            if let Some(s) = c.as_str() {
                Some(s.to_string())
            } else if let Some(arr) = c.as_array() {
                arr.iter()
                    .find_map(|x| x.get("text").and_then(|t| t.as_str()))
                    .map(|s| s.to_string())
            } else {
                None
            }
        })
        .unwrap_or_default()
}

fn extract_decisions_from_text(text: &str, out: &mut Vec<String>) {
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_lowercase();
        let looks_like_decision = lower.contains("edda decide")
            || lower.starts_with("decision")
            || lower.starts_with("decided")
            || lower.contains("decided:")
            || lower.contains("\"decision\"");
        if looks_like_decision {
            out.push(truncate_chars(line, SUBAGENT_SUMMARY_MAX_CHARS));
        }
    }
}

fn dedup_keep_order(items: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    items.retain(|v| seen.insert(v.clone()));
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max_chars).collect();
    format!("{truncated}...")
}

fn fallback_summary_text(text: &str) -> String {
    let first = text.lines().next().unwrap_or("").trim();
    if first.is_empty() {
        String::new()
    } else {
        truncate_chars(first, SUBAGENT_SUMMARY_MAX_CHARS)
    }
}

fn build_subagent_summary_line(summary: &SubagentSummary) -> String {
    let mut parts = Vec::new();
    if !summary.files_touched.is_empty() {
        parts.push(format!("{} files touched", summary.files_touched.len()));
    }
    if !summary.commits.is_empty() {
        parts.push(format!("{} commits", summary.commits.len()));
    }
    if !summary.decisions.is_empty() {
        parts.push(format!("{} decisions", summary.decisions.len()));
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!("Sub-agent completed: {}", parts.join(", "))
    }
}

// ── Session Signals: save / load / render ──

pub(crate) fn save_session_signals(project_id: &str, session_id: &str, signals: &SessionSignals) {
    let state_dir = edda_store::project_dir(project_id).join("state");
    let payload = serde_json::json!({
        "session_id": session_id,
        "updated_at": now_rfc3339(),
    });

    // Tasks
    if !signals.tasks.is_empty() {
        let mut p = payload.clone();
        p["tasks"] = serde_json::to_value(&signals.tasks).unwrap_or_default();
        let _ = fs::write(
            state_dir.join("active_tasks.json"),
            serde_json::to_string_pretty(&p).unwrap_or_default(),
        );
    }
    // Files modified
    if !signals.files_modified.is_empty() {
        let mut p = payload.clone();
        p["files"] = serde_json::to_value(&signals.files_modified).unwrap_or_default();
        let _ = fs::write(
            state_dir.join("files_modified.json"),
            serde_json::to_string_pretty(&p).unwrap_or_default(),
        );
    }
    // Commits
    if !signals.commits.is_empty() {
        let mut p = payload.clone();
        p["commits"] = serde_json::to_value(&signals.commits).unwrap_or_default();
        let _ = fs::write(
            state_dir.join("recent_commits.json"),
            serde_json::to_string_pretty(&p).unwrap_or_default(),
        );
    }
    // Failed commands
    if !signals.failed_commands.is_empty() {
        let mut p = payload;
        p["failed_commands"] = serde_json::to_value(&signals.failed_commands).unwrap_or_default();
        let _ = fs::write(
            state_dir.join("failed_commands.json"),
            serde_json::to_string_pretty(&p).unwrap_or_default(),
        );
    } else {
        // Clean up stale file if no failures
        let _ = fs::remove_file(state_dir.join("failed_commands.json"));
    }
    // Usage
    {
        let mut p = serde_json::json!({
            "session_id": session_id,
            "updated_at": now_rfc3339(),
        });
        p["usage"] = serde_json::to_value(&signals.usage).unwrap_or_default();
        let _ = fs::write(
            state_dir.join("usage.json"),
            serde_json::to_string_pretty(&p).unwrap_or_default(),
        );
    }
}

pub(crate) fn load_state_vec<T: serde::de::DeserializeOwned>(
    project_id: &str,
    filename: &str,
    key: &str,
) -> Vec<T> {
    let path = edda_store::project_dir(project_id)
        .join("state")
        .join(filename);
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let val: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    val.get(key)
        .and_then(|t| serde_json::from_value::<Vec<T>>(t.clone()).ok())
        .unwrap_or_default()
}

/// Read the usage state from the state directory.
pub fn read_usage_state(project_id: &str) -> UsageSnapshot {
    let path = edda_store::project_dir(project_id)
        .join("state")
        .join("usage.json");
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return UsageSnapshot::default(),
    };
    let val: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return UsageSnapshot::default(),
    };
    val.get("usage")
        .and_then(|u| serde_json::from_value::<UsageSnapshot>(u.clone()).ok())
        .unwrap_or_default()
}

/// Per-model pricing (USD per million tokens).
struct ModelPricing {
    input_per_m: f64,
    output_per_m: f64,
}

/// Look up pricing for a model name. Returns None for unknown models.
fn lookup_pricing(model: &str) -> Option<ModelPricing> {
    // Check env override first: EDDA_MODEL_PRICING="model:in:out,model2:in:out"
    if let Some(p) = lookup_pricing_from_env(model) {
        return Some(p);
    }
    // Built-in pricing (Anthropic API rates as of 2025-05)
    let lower = model.to_lowercase();
    if lower.contains("opus") {
        Some(ModelPricing {
            input_per_m: 15.0,
            output_per_m: 75.0,
        })
    } else if lower.contains("sonnet") {
        Some(ModelPricing {
            input_per_m: 3.0,
            output_per_m: 15.0,
        })
    } else if lower.contains("haiku") {
        Some(ModelPricing {
            input_per_m: 0.25,
            output_per_m: 1.25,
        })
    } else {
        None
    }
}

/// Parse EDDA_MODEL_PRICING env var for custom pricing.
fn lookup_pricing_from_env(model: &str) -> Option<ModelPricing> {
    let env_val = std::env::var("EDDA_MODEL_PRICING").ok()?;
    let lower_model = model.to_lowercase();
    for entry in env_val.split(',') {
        let parts: Vec<&str> = entry.trim().split(':').collect();
        if parts.len() == 3 && lower_model.contains(&parts[0].to_lowercase()) {
            let input: f64 = parts[1].parse().ok()?;
            let output: f64 = parts[2].parse().ok()?;
            return Some(ModelPricing {
                input_per_m: input,
                output_per_m: output,
            });
        }
    }
    None
}

/// Estimate cost in USD from a UsageSnapshot.
/// Estimate session cost in USD.
///
/// Note: cache-read tokens are priced at ~10% of input and cache-creation
/// tokens at ~125% of input on the Anthropic API.  We approximate by
/// subtracting the cached portion from full-price input and adding it back
/// at the discounted rate.  When cache breakdown is unavailable the flat
/// input rate is used (slight overestimate for cache-heavy sessions).
pub fn estimate_cost(usage: &UsageSnapshot) -> f64 {
    let pricing = match lookup_pricing(&usage.model) {
        Some(p) => p,
        None => return 0.0,
    };

    // Cache-aware input cost: full-price tokens + discounted cache tokens
    let cache_read = usage.cache_read_tokens;
    let cache_create = usage.cache_creation_tokens;
    let full_price_input = usage.input_tokens.saturating_sub(cache_read + cache_create);

    let input_cost = (full_price_input as f64 / 1_000_000.0) * pricing.input_per_m
        + (cache_read as f64 / 1_000_000.0) * pricing.input_per_m * 0.1
        + (cache_create as f64 / 1_000_000.0) * pricing.input_per_m * 1.25;
    let output_cost = (usage.output_tokens as f64 / 1_000_000.0) * pricing.output_per_m;
    input_cost + output_cost
}

pub(crate) fn render_blocking_section(project_id: &str) -> Option<String> {
    let failed: Vec<FailedBashCmd> =
        load_state_vec(project_id, "failed_commands.json", "failed_commands");
    // Only surface recurring failures (count >= 2) — one-off errors are exploration noise
    let recurring: Vec<&FailedBashCmd> = failed.iter().filter(|f| f.count >= 2).collect();
    if recurring.is_empty() {
        return None;
    }
    let mut lines = vec!["## Blocking".to_string()];
    for cmd in recurring.iter().take(3) {
        let repeat = if cmd.count > 1 {
            format!(" (\u{00d7}{})", cmd.count)
        } else {
            String::new()
        };
        lines.push(format!("- `{}` failing{repeat}", cmd.command_base));
        if !cmd.stderr_snippet.is_empty() {
            lines.push(format!("  > {}", cmd.stderr_snippet));
        }
    }
    Some(lines.join("\n"))
}

/// Extract the "base" of a bash command for aggregation.
/// e.g. "cargo test -p edda-bridge-claude -- --test-threads=1" → "cargo test -p edda-bridge-claude"
fn command_base(cmd: &str) -> String {
    let trimmed = cmd.trim();
    // Take first line only (commands may have &&)
    let first_line = trimmed.lines().next().unwrap_or(trimmed);
    // Truncate to first 80 chars and remove trailing flags after --
    let base = if let Some(pos) = first_line.find(" -- ") {
        &first_line[..pos]
    } else {
        first_line
    };
    let truncated: String = base.chars().take(80).collect();
    truncated.trim().to_string()
}

/// Truncate stderr output to a snippet, keeping the most informative line.
/// Also captures the next line if it provides context (source location, assertion).
fn truncate_stderr(text: &str, max_chars: usize) -> String {
    let all_lines: Vec<&str> = text.lines().collect();
    // Find the most informative line: first one containing "error", "panic", or "failed"
    let best_idx = all_lines
        .iter()
        .position(|l| {
            let lower = l.to_lowercase();
            lower.contains("error") || lower.contains("panic") || lower.contains("failed")
        })
        .or(if all_lines.is_empty() { None } else { Some(0) });

    match best_idx {
        Some(idx) => {
            let trimmed = all_lines[idx].trim();
            // Try to include the next line if it has useful context (source location, assertion)
            let with_context = if idx + 1 < all_lines.len() {
                let next = all_lines[idx + 1].trim();
                let has_context = next.starts_with("-->")
                    || next.starts_with("at ")
                    || next.contains("src/")
                    || next.contains("assert");
                if has_context && !next.is_empty() {
                    format!("{trimmed} | {next}")
                } else {
                    trimmed.to_string()
                }
            } else {
                trimmed.to_string()
            };

            if with_context.len() <= max_chars {
                with_context
            } else {
                let truncated: String = with_context.chars().take(max_chars - 3).collect();
                format!("{truncated}...")
            }
        }
        None => String::new(),
    }
}

/// Returns true if the file path is noise that should be filtered from
/// files_modified tracking (e.g. auto-generated skill files).
pub(crate) fn is_noise_file(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    normalized.contains("/.claude/skills/") || normalized.contains(".claude/skills/")
}

// ── Focus Detection ──

/// Minimum number of modified files to trigger focus detection.
const FOCUS_MIN_FILES: usize = 3;

/// Render a "Current Focus" section based on modified file paths and tasks.
///
/// Returns `None` if fewer than 3 files modified (not enough signal).
pub(crate) fn render_focus_section(project_id: &str) -> Option<String> {
    let files: Vec<FileEditCount> = load_state_vec(project_id, "files_modified.json", "files");
    if files.len() < FOCUS_MIN_FILES {
        return None;
    }

    let file_data: Vec<(&str, usize)> = files.iter().map(|f| (f.path.as_str(), f.count)).collect();
    let (label, prefix) = find_focus_label(&file_data)?;

    let total_edits: usize = files.iter().map(|f| f.count).sum();
    let file_count = files.len();

    let mut lines = vec![format!("## Current Focus: {label}")];

    // Hot file detection: files with edit count > 3x average are outliers
    let avg_edits = total_edits as f64 / file_count as f64;
    let hot_threshold = (avg_edits * 3.0) as usize;
    let hot_files: Vec<&FileEditCount> = files
        .iter()
        .filter(|f| f.count > hot_threshold && hot_threshold > 0)
        .take(3)
        .collect();
    if !hot_files.is_empty() {
        let hot_labels: Vec<String> = hot_files
            .iter()
            .map(|f| {
                let basename = f.path.replace('\\', "/");
                let basename = basename.rsplit('/').next().unwrap_or(&f.path);
                format!("{} ({} edits)", basename, f.count)
            })
            .collect();
        lines.push(format!("Hot files: {}", hot_labels.join(", ")));
    }

    if prefix.contains('/') {
        lines.push(format!(
            "{file_count} files modified ({total_edits} edits) in {prefix}"
        ));
    } else {
        lines.push(format!(
            "{file_count} files modified ({total_edits} edits), {prefix}"
        ));
    }

    // Correlate with active task
    let tasks: Vec<TaskSnapshot> = load_state_vec(project_id, "active_tasks.json", "tasks");
    if let Some(task) = tasks.iter().find(|t| t.status == "in_progress") {
        lines.push(format!("Related task: \"{}\"", task.subject));
    }

    Some(lines.join("\n"))
}

/// Find the focus label and common prefix from file paths with edit counts.
///
/// Returns `(label, display_prefix)` where label is a short name (e.g. crate name)
/// and display_prefix is the path prefix shown to the user.
fn find_focus_label(files: &[(&str, usize)]) -> Option<(String, String)> {
    if files.is_empty() {
        return None;
    }

    // Normalize all paths to (segments, edit_count)
    let normalized: Vec<(Vec<String>, usize)> = files
        .iter()
        .map(|(p, count)| {
            let p = p.replace('\\', "/");
            let stripped = if let Some(rest) = p.strip_prefix("C:").or_else(|| p.strip_prefix("c:"))
            {
                rest.trim_start_matches('/').to_string()
            } else {
                p.trim_start_matches('/').to_string()
            };
            let segs = stripped
                .split('/')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            (segs, *count)
        })
        .collect();

    if normalized.is_empty() {
        return None;
    }

    // Find longest common prefix (by path segments)
    let first = &normalized[0].0;
    let mut prefix_len = first.len();
    for (path_segs, _) in &normalized[1..] {
        let common = first
            .iter()
            .zip(path_segs.iter())
            .take_while(|(a, b)| a == b)
            .count();
        prefix_len = prefix_len.min(common);
    }

    let prefix_segments = &first[..prefix_len];

    // Extract label from prefix
    if let Some(label) = extract_label_from_prefix(prefix_segments) {
        let display = if prefix_segments.is_empty() {
            ".".to_string()
        } else {
            format!("{}/", prefix_segments.join("/"))
        };
        return Some((label, display));
    }

    // Prefix too shallow — use edit-weighted most frequent directory heuristic
    find_most_frequent_focus(&normalized)
}

/// Extract a meaningful label from common prefix segments.
///
/// Looks for patterns like `crates/{name}` or `src/{name}` or `packages/{name}`.
fn extract_label_from_prefix(segments: &[String]) -> Option<String> {
    // Look for "crates/{name}" pattern
    for (i, seg) in segments.iter().enumerate() {
        if (seg == "crates" || seg == "packages") && i + 1 < segments.len() {
            return Some(segments[i + 1].clone());
        }
    }

    // If prefix has 2+ segments, use the last meaningful one
    // (skip "src", "crates", "packages" as they're too generic)
    if segments.len() >= 2 {
        let last = segments.last()?;
        if last != "src" && last != "crates" && last != "packages" {
            return Some(last.clone());
        }
        // If last is "src", use the one before
        if segments.len() >= 3 {
            return Some(segments[segments.len() - 2].clone());
        }
    }

    None
}

/// When common prefix is too short, find the most edit-heavy crate/directory.
///
/// Uses edit-weighted scoring (sum of edits per group) with a 30% threshold.
fn find_most_frequent_focus(paths: &[(Vec<String>, usize)]) -> Option<(String, String)> {
    let mut freq: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let total_edits: usize = paths.iter().map(|(_, c)| c).sum();

    for (segs, count) in paths {
        // Try to find "crates/{name}" in this path
        for (i, seg) in segs.iter().enumerate() {
            if (seg == "crates" || seg == "packages") && i + 1 < segs.len() {
                *freq.entry(segs[i + 1].clone()).or_default() += count;
                break;
            }
        }
    }

    if freq.is_empty() {
        // Fallback: use second segment (after project root) as grouping key
        for (segs, count) in paths {
            if segs.len() >= 2 {
                *freq.entry(segs[1].clone()).or_default() += count;
            }
        }
    }

    if total_edits == 0 {
        return None;
    }

    let (label, edits) = freq.iter().max_by_key(|(_, c)| *c)?;
    // Report focus if ≥30% of total edits are concentrated in one group
    if *edits * 10 >= total_edits * 3 {
        Some((
            label.clone(),
            format!("{}% of edits", edits * 100 / total_edits),
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_transcript(records: &[serde_json::Value]) -> PathBuf {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.into_temp_path().to_path_buf();
        let mut content = String::new();
        for r in records {
            content.push_str(&serde_json::to_string(r).unwrap());
            content.push('\n');
        }
        fs::write(&path, content).unwrap();
        path
    }

    fn assistant_task_create(tool_use_id: &str, subject: &str) -> serde_json::Value {
        serde_json::json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": tool_use_id,
                    "name": "TaskCreate",
                    "input": {
                        "subject": subject,
                        "description": "test task",
                        "activeForm": "Testing"
                    }
                }]
            }
        })
    }

    fn assistant_task_update(task_id: &str, status: &str) -> serde_json::Value {
        serde_json::json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "tu_update",
                    "name": "TaskUpdate",
                    "input": {
                        "taskId": task_id,
                        "status": status
                    }
                }]
            }
        })
    }

    #[test]
    fn signals_extract_tasks() {
        let records = vec![
            assistant_task_create("tu1", "Fix bug A"),
            assistant_task_create("tu2", "Add feature B"),
            assistant_task_update("1", "in_progress"),
            assistant_task_update("1", "completed"),
            assistant_task_update("2", "in_progress"),
        ];
        let path = make_transcript(&records);
        let signals = extract_session_signals(&path);
        assert_eq!(signals.tasks.len(), 2);
        assert_eq!(signals.tasks[0].id, "1");
        assert_eq!(signals.tasks[0].subject, "Fix bug A");
        assert_eq!(signals.tasks[0].status, "completed");
        assert_eq!(signals.tasks[1].status, "in_progress");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn signals_extract_files_modified() {
        let records = vec![serde_json::json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "tool_use", "id": "e1", "name": "Edit",
                  "input": { "file_path": "/repo/src/lib.rs", "old_string": "a", "new_string": "b" } },
                { "type": "tool_use", "id": "e2", "name": "Edit",
                  "input": { "file_path": "/repo/src/lib.rs", "old_string": "c", "new_string": "d" } },
                { "type": "tool_use", "id": "w1", "name": "Write",
                  "input": { "file_path": "/repo/src/new.rs", "content": "fn main() {}" } }
            ]}
        })];
        let path = make_transcript(&records);
        let signals = extract_session_signals(&path);
        assert_eq!(signals.files_modified.len(), 2);
        assert_eq!(signals.files_modified[0].count, 2); // lib.rs edited twice
        assert_eq!(signals.files_modified[1].count, 1); // new.rs written once
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn signals_extract_commits() {
        let records = vec![
            serde_json::json!({
                "type": "assistant",
                "message": { "role": "assistant", "content": [{
                    "type": "tool_use", "id": "b1", "name": "Bash",
                    "input": { "command": "git commit -m \"fix: something\"" }
                }]}
            }),
            serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": [{
                    "type": "tool_result", "tool_use_id": "b1",
                    "content": "[main abc1234] fix: something\n 1 file changed"
                }]}
            }),
        ];
        let path = make_transcript(&records);
        let signals = extract_session_signals(&path);
        assert_eq!(signals.commits.len(), 1);
        assert_eq!(signals.commits[0].hash, "abc1234");
        assert_eq!(signals.commits[0].message, "fix: something");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn signals_save_load_round_trip() {
        let pid = "test_signals_rt_00";
        let _ = edda_store::ensure_dirs(pid);

        let signals = SessionSignals {
            tasks: vec![TaskSnapshot {
                id: "1".into(),
                subject: "Fix".into(),
                status: "completed".into(),
            }],
            files_modified: vec![FileEditCount {
                path: "src/lib.rs".into(),
                count: 3,
            }],
            commits: vec![CommitInfo {
                hash: "abc".into(),
                message: "fix".into(),
            }],
            failed_commands: vec![],
            ..Default::default()
        };
        save_session_signals(pid, "test-session", &signals);

        let tasks: Vec<TaskSnapshot> = load_state_vec(pid, "active_tasks.json", "tasks");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, "completed");

        let files: Vec<FileEditCount> = load_state_vec(pid, "files_modified.json", "files");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].count, 3);

        let commits: Vec<CommitInfo> = load_state_vec(pid, "recent_commits.json", "commits");
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].hash, "abc");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn extract_commit_msg_parses_m_flag() {
        assert_eq!(
            extract_commit_msg_from_cmd(r#"git commit -m "fix: something""#),
            "fix: something"
        );
        assert_eq!(
            extract_commit_msg_from_cmd("git commit -m 'feat: new'"),
            "feat: new"
        );
        assert_eq!(extract_commit_msg_from_cmd("git add . && git commit"), "");
    }

    #[test]
    fn parse_commit_result_extracts_hash() {
        let result = "[main abc1234] fix: something\n 1 file changed";
        let ci = parse_commit_result(result, "").unwrap();
        assert_eq!(ci.hash, "abc1234");
        assert_eq!(ci.message, "fix: something");
    }

    #[test]
    fn noise_file_filters_skills() {
        assert!(is_noise_file(".claude/skills/commit/SKILL.md"));
        assert!(is_noise_file(
            "C:\\repo\\.claude\\skills\\testing\\SKILL.md"
        ));
        assert!(is_noise_file("/home/user/.claude/skills/foo/bar.md"));
        assert!(!is_noise_file("crates/edda-derive/src/lib.rs"));
        assert!(!is_noise_file("src/main.rs"));
    }

    #[test]
    fn signals_skip_noise_files() {
        let records = vec![serde_json::json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "tool_use", "id": "e1", "name": "Edit",
                  "input": { "file_path": "/repo/src/lib.rs", "old_string": "a", "new_string": "b" } },
                { "type": "tool_use", "id": "e2", "name": "Write",
                  "input": { "file_path": "/repo/.claude/skills/commit/SKILL.md", "content": "x" } },
                { "type": "tool_use", "id": "e3", "name": "Edit",
                  "input": { "file_path": "/repo/.claude/skills/testing/SKILL.md", "old_string": "a", "new_string": "b" } }
            ]}
        })];
        let path = make_transcript(&records);
        let signals = extract_session_signals(&path);
        assert_eq!(
            signals.files_modified.len(),
            1,
            "skill files should be filtered"
        );
        assert_eq!(signals.files_modified[0].path, "/repo/src/lib.rs");
        let _ = fs::remove_file(&path);
    }

    // ── Focus Detection tests ──

    #[test]
    fn focus_common_prefix_crates() {
        let files: &[(&str, usize)] = &[
            ("C:/repo/crates/edda-bridge-claude/src/dispatch.rs", 10),
            ("C:/repo/crates/edda-bridge-claude/src/signals.rs", 5),
            ("C:/repo/crates/edda-bridge-claude/src/lib.rs", 2),
        ];
        let (label, prefix) = find_focus_label(files).unwrap();
        assert_eq!(label, "edda-bridge-claude");
        assert!(prefix.contains("edda-bridge-claude"), "prefix: {prefix}");
    }

    #[test]
    fn focus_common_prefix_src() {
        let files: &[(&str, usize)] = &[
            ("/project/src/components/Button.tsx", 1),
            ("/project/src/components/Modal.tsx", 1),
            ("/project/src/components/Header.tsx", 1),
        ];
        let (label, _) = find_focus_label(files).unwrap();
        assert_eq!(label, "components");
    }

    #[test]
    fn focus_most_frequent_dir() {
        // 4 out of 5 files in edda-cli, 1 in edda-core
        let files: &[(&str, usize)] = &[
            ("/repo/crates/edda-cli/src/main.rs", 1),
            ("/repo/crates/edda-cli/src/cmd_gc.rs", 1),
            ("/repo/crates/edda-cli/src/cmd_bridge.rs", 1),
            ("/repo/crates/edda-cli/src/cmd_pack.rs", 1),
            ("/repo/crates/edda-core/src/lib.rs", 1),
        ];
        let (label, _) = find_focus_label(files).unwrap();
        assert_eq!(label, "edda-cli");
    }

    #[test]
    fn focus_too_few_files_returns_none() {
        // find_focus_label works with any count, but render_focus_section gates on >= 3
        let files: &[(&str, usize)] = &[
            ("/repo/crates/foo/src/a.rs", 1),
            ("/repo/crates/foo/src/b.rs", 1),
        ];
        let result = find_focus_label(files);
        assert!(result.is_some());
    }

    #[test]
    fn focus_empty_returns_none() {
        let files: &[(&str, usize)] = &[];
        assert!(find_focus_label(files).is_none());
    }

    #[test]
    fn focus_windows_paths() {
        let files: &[(&str, usize)] = &[
            ("C:\\ai_agent\\edda\\crates\\edda-derive\\src\\types.rs", 3),
            (
                "C:\\ai_agent\\edda\\crates\\edda-derive\\src\\context.rs",
                2,
            ),
            (
                "C:\\ai_agent\\edda\\crates\\edda-derive\\src\\writers.rs",
                1,
            ),
        ];
        let (label, _) = find_focus_label(files).unwrap();
        assert_eq!(label, "edda-derive");
    }

    #[test]
    fn focus_edit_weighted_triggers_on_heavy_crate() {
        // Simulates real scenario: 60 files scattered, but one crate has 17% of edits
        // Old 50% file-count threshold would miss this; new 30% edit threshold catches it
        let mut files: Vec<(&str, usize)> = vec![
            // Heavy crate: 8 files, 182 edits (49% of total)
            ("/repo/crates/edda-bridge-claude/src/dispatch.rs", 63),
            ("/repo/crates/edda-bridge-claude/src/lib.rs", 52),
            ("/repo/crates/edda-bridge-claude/src/digest.rs", 46),
            ("/repo/crates/edda-bridge-claude/src/signals.rs", 21),
            ("/repo/crates/edda-bridge-claude/src/plan.rs", 6),
            ("/repo/crates/edda-bridge-claude/src/narrative.rs", 4),
            ("/repo/crates/edda-bridge-claude/src/parse.rs", 3),
            ("/repo/crates/edda-bridge-claude/src/redact.rs", 2),
            // Other crates: scattered
            ("/repo/crates/edda-cli/src/main.rs", 20),
            ("/repo/crates/edda-cli/src/cmd_gc.rs", 24),
            ("/repo/crates/edda-derive/src/lib.rs", 27),
            ("/repo/crates/edda-ledger/src/paths.rs", 6),
            // Docs (not in crates)
            ("/repo/docs/planB/TRACKS.md", 2),
            ("/repo/docs/USAGE.md", 1),
            ("/repo/Cargo.toml", 1),
        ];
        // Add more scattered files to dilute file count
        for i in 0..10 {
            files.push(("/repo/docs/planB/misc.md", 1));
            let _ = i;
        }

        let (label, display) = find_focus_label(&files).unwrap();
        assert_eq!(
            label, "edda-bridge-claude",
            "should detect heavy crate by edits"
        );
        assert!(display.contains("% of edits"), "display: {display}");
    }

    #[test]
    fn focus_no_hot_files_when_edits_even() {
        let pid = "test_focus_no_hot";
        let _ = edda_store::ensure_dirs(pid);

        let signals = SessionSignals {
            tasks: vec![],
            files_modified: vec![
                FileEditCount {
                    path: "/repo/crates/foo/src/a.rs".into(),
                    count: 10,
                },
                FileEditCount {
                    path: "/repo/crates/foo/src/b.rs".into(),
                    count: 10,
                },
                FileEditCount {
                    path: "/repo/crates/foo/src/c.rs".into(),
                    count: 10,
                },
            ],
            commits: vec![],
            failed_commands: vec![],
            ..Default::default()
        };
        save_session_signals(pid, "test-session", &signals);

        let focus = render_focus_section(pid).unwrap();
        // avg = 10, threshold = 30 → no file > 30 → no hot files
        assert!(
            !focus.contains("Hot files:"),
            "even edits should not trigger hot: {focus}"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn focus_hot_files_with_clear_outlier() {
        let pid = "test_focus_hot_outlier";
        let _ = edda_store::ensure_dirs(pid);

        let signals = SessionSignals {
            tasks: vec![],
            files_modified: vec![
                FileEditCount {
                    path: "/repo/crates/foo/src/dispatch.rs".into(),
                    count: 60,
                },
                FileEditCount {
                    path: "/repo/crates/foo/src/a.rs".into(),
                    count: 2,
                },
                FileEditCount {
                    path: "/repo/crates/foo/src/b.rs".into(),
                    count: 2,
                },
                FileEditCount {
                    path: "/repo/crates/foo/src/c.rs".into(),
                    count: 1,
                },
                FileEditCount {
                    path: "/repo/crates/foo/src/d.rs".into(),
                    count: 1,
                },
            ],
            commits: vec![],
            failed_commands: vec![],
            ..Default::default()
        };
        save_session_signals(pid, "test-session", &signals);

        let focus = render_focus_section(pid).unwrap();
        // avg = (60+2+2+1+1)/5 = 13.2, threshold = 39.6
        // dispatch.rs (60) > 39.6 → hot file
        assert!(
            focus.contains("Hot files:"),
            "should detect hot file: {focus}"
        );
        assert!(
            focus.contains("dispatch.rs"),
            "should name the hot file: {focus}"
        );

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── CmdFail / Blocking tests ──

    #[test]
    fn command_base_strips_flags() {
        assert_eq!(
            command_base("cargo test -p edda-bridge-claude -- --test-threads=1"),
            "cargo test -p edda-bridge-claude"
        );
    }

    #[test]
    fn command_base_multiline() {
        assert_eq!(
            command_base("cd /repo && cargo build\ncargo test"),
            "cd /repo && cargo build"
        );
    }

    #[test]
    fn truncate_stderr_finds_error_line() {
        let text = "Compiling foo v0.1.0\nerror[E0308]: mismatched types\n  --> src/lib.rs:10:5";
        let snippet = truncate_stderr(text, 200);
        assert!(
            snippet.contains("error[E0308]"),
            "should find error line: {snippet}"
        );
    }

    #[test]
    fn truncate_stderr_includes_source_location() {
        let text = "Compiling foo v0.1.0\nerror[E0308]: mismatched types\n  --> src/lib.rs:10:5";
        let snippet = truncate_stderr(text, 200);
        assert!(
            snippet.contains("error[E0308]"),
            "should have error: {snippet}"
        );
        assert!(
            snippet.contains("src/lib.rs:10:5"),
            "should include source location: {snippet}"
        );
    }

    #[test]
    fn truncate_stderr_skips_irrelevant_next_line() {
        let text = "error: test failed\nCompiling bar v0.2.0";
        let snippet = truncate_stderr(text, 200);
        assert_eq!(snippet, "error: test failed");
    }

    #[test]
    fn truncate_stderr_truncates_long_line() {
        let long_line = "error: ".to_string() + &"x".repeat(300);
        let snippet = truncate_stderr(&long_line, 100);
        assert!(snippet.len() <= 100);
        assert!(snippet.ends_with("..."));
    }

    #[test]
    fn signals_extract_failed_commands() {
        let records = vec![
            // Bash tool_use
            serde_json::json!({
                "type": "assistant",
                "message": { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "bash1", "name": "Bash",
                      "input": { "command": "cargo test -p edda-bridge-claude" } }
                ]}
            }),
            // Failed tool_result
            serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "bash1",
                      "is_error": true,
                      "content": "error: test failed\nthread 'plan::tests::parse' panicked" }
                ]}
            }),
            // Same command fails again
            serde_json::json!({
                "type": "assistant",
                "message": { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "bash2", "name": "Bash",
                      "input": { "command": "cargo test -p edda-bridge-claude" } }
                ]}
            }),
            serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "bash2",
                      "is_error": true,
                      "content": "error: test failed again\nthread 'plan::tests::parse' panicked" }
                ]}
            }),
        ];
        let path = make_transcript(&records);
        let signals = extract_session_signals(&path);
        assert_eq!(
            signals.failed_commands.len(),
            1,
            "should aggregate by command base"
        );
        assert_eq!(signals.failed_commands[0].count, 2);
        assert!(signals.failed_commands[0]
            .command_base
            .contains("cargo test"));
        assert!(!signals.failed_commands[0].stderr_snippet.is_empty());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn signals_healing_success_clears_failures() {
        // Command fails 3 times, then succeeds → should NOT appear in failed_commands
        let records = vec![
            // Fail 1
            serde_json::json!({
                "type": "assistant",
                "message": { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "b1", "name": "Bash",
                      "input": { "command": "cargo test -p edda-bridge-claude" } }
                ]}
            }),
            serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "b1",
                      "is_error": true,
                      "content": "error: test failed" }
                ]}
            }),
            // Fail 2
            serde_json::json!({
                "type": "assistant",
                "message": { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "b2", "name": "Bash",
                      "input": { "command": "cargo test -p edda-bridge-claude" } }
                ]}
            }),
            serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "b2",
                      "is_error": true,
                      "content": "error: test failed again" }
                ]}
            }),
            // Success — heals
            serde_json::json!({
                "type": "assistant",
                "message": { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "b3", "name": "Bash",
                      "input": { "command": "cargo test -p edda-bridge-claude" } }
                ]}
            }),
            serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "b3",
                      "content": "test result: ok. 120 passed" }
                ]}
            }),
        ];
        let path = make_transcript(&records);
        let signals = extract_session_signals(&path);
        assert!(
            signals.failed_commands.is_empty(),
            "success should heal previous failures: {:?}",
            signals.failed_commands
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn signals_healing_then_fail_again() {
        // Fail → Success (heals) → Fail again → should show count=1
        let records = vec![
            // Fail
            serde_json::json!({
                "type": "assistant",
                "message": { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "b1", "name": "Bash",
                      "input": { "command": "cargo clippy" } }
                ]}
            }),
            serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "b1",
                      "is_error": true,
                      "content": "error: unused import" }
                ]}
            }),
            // Success — heals
            serde_json::json!({
                "type": "assistant",
                "message": { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "b2", "name": "Bash",
                      "input": { "command": "cargo clippy" } }
                ]}
            }),
            serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "b2",
                      "content": "Finished dev profile" }
                ]}
            }),
            // Fail again — fresh count
            serde_json::json!({
                "type": "assistant",
                "message": { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "b3", "name": "Bash",
                      "input": { "command": "cargo clippy" } }
                ]}
            }),
            serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "b3",
                      "is_error": true,
                      "content": "error: new unused variable" }
                ]}
            }),
        ];
        let path = make_transcript(&records);
        let signals = extract_session_signals(&path);
        assert_eq!(signals.failed_commands.len(), 1);
        assert_eq!(
            signals.failed_commands[0].count, 1,
            "count should reset after healing"
        );
        assert!(signals.failed_commands[0]
            .stderr_snippet
            .contains("new unused variable"));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn signals_successful_bash_not_tracked() {
        let records = vec![
            serde_json::json!({
                "type": "assistant",
                "message": { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "bash1", "name": "Bash",
                      "input": { "command": "cargo build" } }
                ]}
            }),
            serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "bash1",
                      "content": "Compiling..." }
                ]}
            }),
        ];
        let path = make_transcript(&records);
        let signals = extract_session_signals(&path);
        assert!(signals.failed_commands.is_empty());
        let _ = fs::remove_file(&path);
    }

    // ── Usage / Cost tests ──

    #[test]
    fn estimate_cost_sonnet() {
        let usage = UsageSnapshot {
            model: "claude-sonnet-4-20250514".into(),
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            ..Default::default()
        };
        let cost = estimate_cost(&usage);
        // input: 1M * $3/M = $3.00, output: 0.1M * $15/M = $1.50 -> $4.50
        assert!((cost - 4.50).abs() < 0.01, "cost={cost}");
    }

    #[test]
    fn estimate_cost_opus() {
        let usage = UsageSnapshot {
            model: "claude-opus-4-20250514".into(),
            input_tokens: 500_000,
            output_tokens: 50_000,
            ..Default::default()
        };
        let cost = estimate_cost(&usage);
        // input: 0.5M * $15/M = $7.50, output: 0.05M * $75/M = $3.75 -> $11.25
        assert!((cost - 11.25).abs() < 0.01, "cost={cost}");
    }

    #[test]
    fn estimate_cost_cache_aware() {
        let usage = UsageSnapshot {
            model: "claude-sonnet-4-20250514".into(),
            // 1M total input, 600k are cache-read, 100k are cache-create
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            cache_read_tokens: 600_000,
            cache_creation_tokens: 100_000,
        };
        let cost = estimate_cost(&usage);
        // full-price input: (1M - 600k - 100k) = 300k * $3/M = $0.90
        // cache-read: 600k * $3/M * 0.1 = $0.18
        // cache-create: 100k * $3/M * 1.25 = $0.375
        // output: 100k * $15/M = $1.50
        // total ≈ $2.955
        assert!((cost - 2.955).abs() < 0.01, "cost={cost}");
    }

    #[test]
    fn estimate_cost_unknown_model() {
        let usage = UsageSnapshot {
            model: "gpt-4o".into(),
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            ..Default::default()
        };
        let cost = estimate_cost(&usage);
        assert_eq!(cost, 0.0, "unknown model should return 0");
    }

    #[test]
    fn signals_extract_usage_from_transcript() {
        let records = vec![
            serde_json::json!({
                "type": "system",
                "model": "claude-sonnet-4-20250514"
            }),
            serde_json::json!({
                "type": "assistant",
                "message": {
                    "role": "assistant",
                    "model": "claude-sonnet-4-20250514",
                    "usage": {
                        "input_tokens": 1000,
                        "output_tokens": 500,
                        "cache_read_input_tokens": 200,
                        "cache_creation_input_tokens": 50
                    },
                    "content": [{ "type": "text", "text": "Hello" }]
                }
            }),
            serde_json::json!({
                "type": "assistant",
                "message": {
                    "role": "assistant",
                    "usage": {
                        "input_tokens": 2000,
                        "output_tokens": 800,
                        "cache_read_input_tokens": 100,
                        "cache_creation_input_tokens": 0
                    },
                    "content": [{ "type": "text", "text": "World" }]
                }
            }),
        ];
        let path = make_transcript(&records);
        let signals = extract_session_signals(&path);
        assert_eq!(signals.usage.model, "claude-sonnet-4-20250514");
        assert_eq!(signals.usage.input_tokens, 3000);
        assert_eq!(signals.usage.output_tokens, 1300);
        assert_eq!(signals.usage.cache_read_tokens, 300);
        assert_eq!(signals.usage.cache_creation_tokens, 50);
        assert_eq!(signals.usage.total_tokens(), 4300);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn usage_save_and_read_round_trip() {
        let pid = "test_usage_rt_00";
        let _ = edda_store::ensure_dirs(pid);

        let signals = SessionSignals {
            usage: UsageSnapshot {
                model: "claude-sonnet-4-20250514".into(),
                input_tokens: 5000,
                output_tokens: 2000,
                cache_read_tokens: 100,
                cache_creation_tokens: 50,
            },
            ..Default::default()
        };
        save_session_signals(pid, "test-session", &signals);

        let loaded = read_usage_state(pid);
        assert_eq!(loaded.model, "claude-sonnet-4-20250514");
        assert_eq!(loaded.input_tokens, 5000);
        assert_eq!(loaded.output_tokens, 2000);
        assert_eq!(loaded.cache_read_tokens, 100);
        assert_eq!(loaded.cache_creation_tokens, 50);

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn extract_subagent_summary_from_transcript() {
        let records = vec![
            serde_json::json!({
                "type": "assistant",
                "message": { "role": "assistant", "content": [
                    {
                        "type": "tool_use",
                        "id": "e1",
                        "name": "Edit",
                        "input": { "file_path": "/repo/src/lib.rs", "old_string": "a", "new_string": "b" }
                    },
                    {
                        "type": "tool_use",
                        "id": "b1",
                        "name": "Bash",
                        "input": { "command": "git commit -m \"feat: sub-agent output\"" }
                    },
                    {
                        "type": "text",
                        "text": "Decision: keep extraction bounded"
                    }
                ]}
            }),
            serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": [{
                    "type": "tool_result",
                    "tool_use_id": "b1",
                    "content": "[main abc1234] feat: sub-agent output\n 1 file changed"
                }]}
            }),
        ];

        let path = make_transcript(&records);
        let summary =
            extract_subagent_summary(path.to_str().unwrap_or_default(), "fallback", "agent-1");

        assert_eq!(summary.files_touched.len(), 1);
        assert!(summary.files_touched[0].contains("/repo/src/lib.rs"));
        assert_eq!(summary.commits.len(), 1);
        assert!(summary.commits[0].contains("abc1234"));
        assert!(
            summary
                .decisions
                .iter()
                .any(|d| d.to_lowercase().contains("decision")),
            "expected a decision signal: {:?}",
            summary.decisions
        );
        assert!(
            !summary.summary.is_empty(),
            "summary text should be non-empty"
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn extract_subagent_summary_falls_back_to_last_message() {
        let summary = extract_subagent_summary(
            "",
            "Decision: adopt async queue\nAdditional details",
            "agent-2",
        );

        assert!(summary.files_touched.is_empty());
        assert!(summary.commits.is_empty());
        assert!(
            summary
                .decisions
                .iter()
                .any(|d| d.contains("Decision: adopt async queue")),
            "fallback decisions should include first line: {:?}",
            summary.decisions
        );
        assert!(
            summary.summary.contains("Decision: adopt async queue"),
            "fallback summary should use first assistant line: {}",
            summary.summary
        );
    }

    #[test]
    fn extract_subagent_summary_supports_pointer_like_path() {
        let records = vec![serde_json::json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                {
                    "type": "tool_use",
                    "id": "w1",
                    "name": "Write",
                    "input": { "file_path": "/repo/src/new.rs", "content": "fn main() {}" }
                }
            ]}
        })];

        let path = make_transcript(&records);
        let pointer = format!("{}::sidechain:agent-9", path.to_string_lossy());
        let summary = extract_subagent_summary(&pointer, "fallback text", "agent-9");

        assert_eq!(summary.files_touched.len(), 1);
        assert!(summary.files_touched[0].contains("/repo/src/new.rs"));
        assert!(summary.commits.is_empty());

        let _ = fs::remove_file(&path);
    }
}
