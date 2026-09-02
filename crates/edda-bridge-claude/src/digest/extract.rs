use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::Path;

use super::helpers::{
    accumulate_active_secs, extract_bash_command, extract_envelope_cwd, extract_exit_code,
    extract_file_path, extract_git_commit_msg,
};
use super::{ActivityType, DigestTaskSnapshot, FailedCommand, SessionOutcome, SessionStats};

pub fn extract_stats(session_ledger_path: &Path) -> anyhow::Result<SessionStats> {
    Ok(extract_stats_delta(session_ledger_path, 0)?.stats)
}

/// Tool-call event names across the bridges that persist session ledgers
/// (round-1 P1-1): Claude Code `PostToolUse`, OpenClaw `after_tool_call`
/// (tool data nested under `event_data`), Cursor `postToolUse`, Hermes
/// `post_tool_call`. Without these, sessions from three of the five bridges
/// did real work but were classified zero-call and silently dropped.
fn is_tool_call_event(name: &str) -> bool {
    matches!(
        name,
        "PostToolUse" | "after_tool_call" | "postToolUse" | "post_tool_call"
    )
}

/// User-prompt event names across bridges: Claude Code `UserPromptSubmit`,
/// Cursor `beforeSubmitPrompt`, OpenClaw `before_agent_start`.
fn is_user_prompt_event(name: &str) -> bool {
    matches!(
        name,
        "UserPromptSubmit" | "beforeSubmitPrompt" | "before_agent_start"
    )
}

/// OpenClaw has no separate failure event: a failed tool call is an
/// `after_tool_call` with a non-empty `event_data.error` string or
/// `event_data.success: false`.
fn openclaw_tool_failed(envelope: &serde_json::Value) -> bool {
    let data = envelope.get("event_data");
    if data
        .and_then(|d| d.get("success"))
        .and_then(|v| v.as_bool())
        == Some(false)
    {
        return true;
    }
    data.and_then(|d| d.get("error"))
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty())
}

/// Normalize lower-case bridge tool names (OpenClaw/Hermes) to the Claude
/// equivalents the digest breakdown and per-tool extraction expect.
fn normalize_tool_name(name: &str) -> &str {
    match name {
        "bash" | "terminal" | "shell" => "Bash",
        "edit_file" | "file_edit" => "Edit",
        "write_file" | "file_write" => "Write",
        other => other,
    }
}

/// Extract the tool name from an envelope in any bridge's shape:
/// top-level `tool_name`, `raw.toolName`/`raw.tool_name`, or OpenClaw's
/// `event_data.tool_name`.
fn tool_name_of(envelope: &serde_json::Value) -> String {
    let raw = envelope
        .get("tool_name")
        .or_else(|| {
            envelope
                .get("raw")
                .and_then(|r| r.get("toolName").or_else(|| r.get("tool_name")))
        })
        .or_else(|| envelope.get("event_data").and_then(|d| d.get("tool_name")))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    normalize_tool_name(raw).to_string()
}

/// Byte offset just past the last complete (newline-terminated) line.
///
/// A ledger that ends mid-line (truncated or concurrently-written final
/// line, round-1 P0-2) is only ever consumed up to its last complete line.
pub(crate) fn complete_prefix_len(file: &mut std::fs::File) -> std::io::Result<u64> {
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(0);
    }
    const CHUNK: u64 = 8192;
    let mut pos = len;
    loop {
        let start = pos.saturating_sub(CHUNK);
        let size = (pos - start) as usize;
        file.seek(SeekFrom::Start(start))?;
        let mut buf = vec![0u8; size];
        file.read_exact(&mut buf)?;
        if let Some(i) = buf.iter().rposition(|&b| b == b'\n') {
            return Ok(start + i as u64 + 1);
        }
        if start == 0 {
            return Ok(0);
        }
        pos = start;
    }
}

/// Proof of file identity: hash of the first `len` bytes of the file
/// (round-2 ruling: a byte offset alone cannot identify a file — it
/// silently assumes the file is append-only and never replaced).
pub(crate) fn hash_prefix(path: &Path, len: u64) -> anyhow::Result<String> {
    let file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut file.take(len), &mut hasher)?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// Validate a watermark candidate against the current file: the offset is
/// only usable if the file still contains those bytes and they hash to the
/// recorded proof. A mismatch (replacement, in-place rewrite, shrink,
/// reused session id) means the position proves nothing and the session
/// must be re-read from zero — never skipped. Offset 0 consumes nothing
/// and always matches.
pub(crate) fn watermark_matches(path: &Path, offset: u64, prefix_hash: &str) -> bool {
    if offset == 0 {
        return true;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.len() < offset {
        return false;
    }
    hash_prefix(path, offset)
        .map(|h| h == prefix_hash)
        .unwrap_or(false)
}

/// Result of one streaming pass over a session ledger (round-3 P1-1).
pub struct ExtractedDelta {
    pub stats: SessionStats,
    /// Byte offset just past the last consumed complete line.
    pub consumed: u64,
    /// BLAKE3 of the first `consumed` bytes, computed over the SAME bytes —
    /// read through the SAME open file handle — whose content produced
    /// `stats` (round-3 P1-1 ruling: the note's content and its identity
    /// proof must not be able to come from different reads). One handle,
    /// one pass: if the path is atomically replaced mid-digest, the open
    /// handle keeps following the original inode and BOTH the stats and
    /// the hash are derived from it, so the emitted watermark always
    /// proves exactly the bytes that were summarized.
    pub prefix_hash: String,
}

/// Incremental parse state for one sequential pass over a session ledger.
struct StatsParser {
    stats: SessionStats,
    files_set: BTreeSet<String>,
    file_edit_map: BTreeMap<String, u64>,
    // Track session outcome: last event type + trailing failure count
    last_event_name: String,
    trailing_failures: u32,
    // Track timestamps for duration. Duration is the session's real activity
    // span: consecutive-event gaps are summed with idle gaps capped
    // (GH-578) — not "first timestamp until now".
    active_secs: i64,
    last_seen: Option<time::OffsetDateTime>,
    malformed: u64,
}

impl StatsParser {
    fn new() -> Self {
        Self {
            stats: SessionStats::default(),
            files_set: BTreeSet::new(),
            file_edit_map: BTreeMap::new(),
            last_event_name: String::new(),
            trailing_failures: 0,
            active_secs: 0,
            last_seen: None,
            malformed: 0,
        }
    }

    /// Parse one complete envelope line into the accumulating stats.
    fn push_line(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let envelope: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                self.malformed += 1;
                return; // skip malformed lines
            }
        };

        let ts = envelope.get("ts").and_then(|v| v.as_str()).unwrap_or("");
        if !ts.is_empty() {
            if self.stats.first_ts.is_none() {
                self.stats.first_ts = Some(ts.to_string());
            }
            self.stats.last_ts = Some(ts.to_string());
            accumulate_active_secs(&mut self.active_secs, &mut self.last_seen, ts);
        }

        let event_name = envelope
            .get("hook_event_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Failure classification: Claude Code has a dedicated failure event;
        // OpenClaw reports failures inline on after_tool_call.
        let is_failure = event_name == "PostToolUseFailure"
            || (event_name == "after_tool_call" && openclaw_tool_failed(&envelope));
        let is_call = !is_failure && is_tool_call_event(event_name);

        // Track trailing failures for outcome detection
        if is_failure {
            self.trailing_failures += 1;
        } else if is_call {
            self.trailing_failures = 0;
        }
        if !event_name.is_empty() {
            self.last_event_name = event_name.to_string();
        }

        if is_failure {
            self.stats.tool_failures += 1;
            // Extract failed Bash commands
            let tool_name = tool_name_of(&envelope);
            if tool_name == "Bash" {
                if let Some(cmd) = extract_bash_command(&envelope) {
                    let cwd_val = extract_envelope_cwd(&envelope);
                    let exit_code = extract_exit_code(&envelope);
                    self.stats.failed_cmds_detail.push(FailedCommand {
                        command: cmd.clone(),
                        cwd: cwd_val,
                        exit_code,
                    });
                    self.stats.failed_commands.push(cmd);
                }
            }
        } else if is_call {
            self.stats.tool_calls += 1;
            // Extract tool_name and accumulate per-tool breakdown
            let tool_name = tool_name_of(&envelope);
            if !tool_name.is_empty() {
                *self
                    .stats
                    .tool_call_breakdown
                    .entry(tool_name.to_string())
                    .or_insert(0) += 1;
            }
            if tool_name == "Edit" || tool_name == "Write" {
                if let Some(fp) = extract_file_path(&envelope) {
                    if !crate::signals::is_noise_file(&fp) {
                        self.files_set.insert(fp.clone());
                        *self.file_edit_map.entry(fp).or_insert(0) += 1;
                    }
                }
            }
            if tool_name == "Bash" {
                if let Some(cmd) = extract_bash_command(&envelope) {
                    if cmd.contains("git commit") {
                        let msg = extract_git_commit_msg(&cmd);
                        if !msg.is_empty() {
                            self.stats.commits_made.push(msg);
                        }
                    }
                    if let Some(pkg) = crate::nudge::extract_dependency_add(&cmd) {
                        if !self.stats.deps_added.contains(&pkg) {
                            self.stats.deps_added.push(pkg);
                        }
                    }
                }
            }
        } else if is_user_prompt_event(event_name) {
            self.stats.user_prompts += 1;
        }
    }

    fn finish(mut self) -> SessionStats {
        self.stats.files_modified = self.files_set.into_iter().collect();
        self.stats.file_edit_counts = self.file_edit_map.into_iter().collect();
        self.stats.duration_minutes = self.active_secs.unsigned_abs() / 60;

        // Determine session outcome
        self.stats.outcome = if self.trailing_failures >= 3 {
            SessionOutcome::ErrorStuck
        } else if is_user_prompt_event(&self.last_event_name) {
            SessionOutcome::Interrupted
        } else {
            SessionOutcome::Completed
        };

        // Classify activity based on tool patterns
        self.stats.activity = classify_activity(&self.stats);

        self.stats
    }
}

/// Extract the statistics of a session ledger's delta starting at
/// `start_offset` (the digest watermark) up to the end of the last complete
/// line, TOGETHER WITH the identity proof of the consumed prefix — derived
/// from the same single read (round-3 P1-1).
///
/// One open file handle, one sequential pass: every consumed byte is hashed,
/// and the bytes at or after `start_offset` are the bytes parsed into
/// `stats`. The previous implementation opened the path a second time to
/// hash the consumed prefix after parsing; an atomic replacement of the path
/// between the two opens produced a note whose stats summarized one file
/// while its `prefix_hash` proved another, and the retry then validated
/// against the replacement and silently skipped every current record. That
/// possibility is now structurally gone: there is no second read to race
/// against.
///
/// A trailing unterminated line is NOT consumed: the producer may still be
/// writing it. Only complete, newline-terminated lines count as consumed,
/// so a line is digested exactly once, once its write completes.
pub fn extract_stats_delta(
    session_ledger_path: &Path,
    start_offset: u64,
) -> anyhow::Result<ExtractedDelta> {
    if !session_ledger_path.exists() {
        return Ok(ExtractedDelta {
            stats: SessionStats::default(),
            consumed: start_offset,
            prefix_hash: String::new(),
        });
    }

    let mut file = std::fs::File::open(session_ledger_path)?;
    let end = complete_prefix_len(&mut file)?;
    if start_offset >= end {
        // Nothing new (complete) since the watermark. No proof is derived:
        // the caller keeps its existing, already proof-validated watermark.
        return Ok(ExtractedDelta {
            stats: SessionStats::default(),
            consumed: start_offset,
            prefix_hash: String::new(),
        });
    }

    // ONE pass over the SAME open handle: hash every consumed byte, parse
    // the bytes at or after `start_offset`. One buffer of truth.
    file.seek(SeekFrom::Start(0))?;
    let mut reader = std::io::BufReader::with_capacity(128 * 1024, file);
    let mut parser = StatsParser::new();
    let mut hasher = blake3::Hasher::new();
    let mut consumed: u64 = 0;
    let mut line_buf: Vec<u8> = Vec::new();

    loop {
        line_buf.clear();
        let n = reader.read_until(b'\n', &mut line_buf)?;
        if n == 0 {
            break;
        }
        if line_buf.last() != Some(&b'\n') {
            // Unterminated tail: the producer is (or was) mid-write.
            // Not consumed — it will be picked up once complete.
            break;
        }
        hasher.update(&line_buf);
        let line_start = consumed;
        consumed += n as u64;
        if line_start >= start_offset {
            let line = String::from_utf8_lossy(&line_buf);
            parser.push_line(&line);
        }
    }

    if parser.malformed > 0 {
        tracing::debug!(
            malformed = parser.malformed,
            path = %session_ledger_path.display(),
            "skipped malformed lines in session ledger (never destroyed)"
        );
    }

    Ok(ExtractedDelta {
        stats: parser.finish(),
        consumed,
        prefix_hash: hasher.finalize().to_hex().to_string(),
    })
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
        // GH-585: unmeasured (None) and a measured zero both render no
        // dollar amount — same behavior as before, but now the distinction
        // is preserved in the payload instead of flattened into 0.0.
        let cost_str = match stats.estimated_cost_usd {
            Some(cost) if cost > 0.0 => format!(", ${cost:.4}"),
            _ => String::new(),
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
