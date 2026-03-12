use std::fs;
use std::path::{Path, PathBuf};

use super::read_workspace_config_bool;

// ── Active Plan ──

/// Default maximum chars for the plan excerpt.
const PLAN_EXCERPT_MAX_CHARS: usize = 700;
/// Default maximum lines to read from the plan file.
const PLAN_EXCERPT_MAX_LINES: usize = 30;

/// Render an "Active Plan" section from the user's Claude plans directory.
/// Uses `EDDA_PLANS_DIR` env var if set, otherwise `~/.claude/plans/`.
/// Returns `None` if no plan file exists.
///
/// When `project_id` is provided, attempts structured rendering with progress
/// tracking (cross-referencing plan steps against tasks/commits). Falls back
/// to simple truncation if the plan has no recognizable step structure.
pub(crate) fn render_active_plan(project_id: Option<&str>) -> Option<String> {
    let plans_dir = match std::env::var("EDDA_PLANS_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => dirs::home_dir()?.join(".claude").join("plans"),
    };
    render_active_plan_from_dir(&plans_dir, project_id)
}

/// Render an "Active Plan" section from a given directory.
/// Returns `None` if no plan file exists.
pub(super) fn render_active_plan_from_dir(
    plans_dir: &Path,
    project_id: Option<&str>,
) -> Option<String> {
    if !plans_dir.is_dir() {
        return None;
    }

    // Find most recently modified .md file
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let entries = fs::read_dir(plans_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if let Ok(mtime) = meta.modified() {
                if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
                    best = Some((mtime, path));
                }
            }
        }
    }

    let (mtime, path) = best?;
    let content = fs::read_to_string(&path).ok()?;
    if content.trim().is_empty() {
        return None;
    }

    // Format mtime as UTC (local offset unavailable in sandboxed time crate)
    let mtime_str = {
        let duration = mtime
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let dt = time::OffsetDateTime::from_unix_timestamp(duration.as_secs() as i64)
            .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            dt.year(),
            dt.month() as u8,
            dt.day(),
            dt.hour(),
            dt.minute()
        )
    };

    let filename = path.file_name()?.to_str()?;

    // Try structured rendering with progress tracking
    if let Some(pid) = project_id {
        if let Some(structured) =
            crate::plan::render_plan_with_progress(&content, pid, filename, &mtime_str)
        {
            return Some(structured);
        }
    }

    // Fallback: excerpt (first N lines, up to MAX_CHARS)
    let mut excerpt = String::new();
    let mut line_count = 0;
    for line in content.lines() {
        if line_count >= PLAN_EXCERPT_MAX_LINES {
            break;
        }
        if excerpt.len() + line.len() + 1 > PLAN_EXCERPT_MAX_CHARS {
            break;
        }
        excerpt.push_str(line);
        excerpt.push('\n');
        line_count += 1;
    }

    if line_count < content.lines().count() {
        excerpt.push_str("...(truncated)\n");
    }

    Some(format!(
        "## Active Plan\n> {filename} ({mtime_str})\n\n{excerpt}"
    ))
}
// ── Skill Catalog ──

/// Render a skill guide directive for guide mode.
/// Does NOT duplicate the skill list (Claude Code system-reminder already provides it).
/// Only injects behavioral instruction to proactively recommend skills.
pub(super) fn render_skill_guide_directive() -> String {
    [
        "## Skill Guide Mode",
        "",
        "The available skills/commands are listed in the system-reminder above.",
        "When the user's current task or question matches a skill, **proactively suggest it**:",
        "- Name the skill with `/<name>` so the user can invoke it directly",
        "- Briefly explain what it does and why it fits their situation",
        "- If a workflow applies (e.g. `/deep-research` → `/deep-innovate` → `/deep-plan`), mention the sequence",
        "",
        "Goal: help users discover and learn available tools over time.",
    ]
    .join("\n")
}

/// Run auto-digest: digest pending previous sessions into workspace ledger.
/// Returns an optional warning string to inject into context.
pub(super) fn run_auto_digest(
    project_id: &str,
    current_session_id: &str,
    cwd: &str,
) -> Option<String> {
    // Check if auto_digest is enabled (default: true)
    let enabled = match std::env::var("EDDA_BRIDGE_AUTO_DIGEST") {
        Ok(val) => val != "0",
        Err(_) => read_workspace_config_bool(cwd, "bridge.auto_digest").unwrap_or(true),
    };
    if !enabled {
        return None;
    }

    let lock_timeout_ms: u64 = std::env::var("EDDA_BRIDGE_LOCK_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);

    let digest_failed_cmds = match std::env::var("EDDA_BRIDGE_DIGEST_FAILED_CMDS") {
        Ok(val) => val != "0",
        Err(_) => read_workspace_config_bool(cwd, "bridge.digest_failed_cmds").unwrap_or(true),
    };

    match crate::digest::digest_previous_sessions_with_opts(
        project_id,
        current_session_id,
        cwd,
        lock_timeout_ms,
        digest_failed_cmds,
    ) {
        crate::digest::DigestResult::Written { event_id } => {
            tracing::info!(%event_id, "digested previous session");
            None
        }
        crate::digest::DigestResult::PermanentFailure(warning) => Some(warning),
        crate::digest::DigestResult::NoPending
        | crate::digest::DigestResult::Disabled
        | crate::digest::DigestResult::LockTimeout
        | crate::digest::DigestResult::Error(_) => None,
    }
}
// ── Last Assistant Message ──

/// Default max chars for the last assistant message excerpt.
const LAST_ASSISTANT_MAX_CHARS: usize = 500;

/// Extract the last assistant message from the most recent prior session's transcript.
/// Returns None if no prior session exists or no assistant text found.
pub(super) fn extract_prior_session_last_message(
    project_id: &str,
    current_session_id: &str,
) -> Option<String> {
    let transcripts_dir = edda_store::project_dir(project_id).join("transcripts");
    if !transcripts_dir.is_dir() {
        return None;
    }

    // Find the most recently modified transcript that isn't the current session
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let entries = fs::read_dir(&transcripts_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let stem = path.file_stem()?.to_str()?;
        if stem == current_session_id {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if let Ok(mtime) = meta.modified() {
                if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
                    best = Some((mtime, path));
                }
            }
        }
    }

    let (_, transcript_path) = best?;
    let max_chars: usize = std::env::var("EDDA_LAST_ASSISTANT_MAX_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(LAST_ASSISTANT_MAX_CHARS);
    edda_transcript::extract_last_assistant_text(&transcript_path, max_chars)
}

/// Inject karvi task brief if working in a karvi project.
///
/// Detection: Check if server/board.json exists
/// Task ID: Extract T\d+ pattern from current git branch
/// Brief: Read server/briefs/{taskId}.md, truncate to 2000 chars
/// Format: [karvi task brief: {taskId}]\n{contents}
pub(super) fn inject_karvi_brief(cwd: &str) -> Option<String> {
    use std::sync::LazyLock;
    static RE_TASK_ID: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"T\d+").expect("static regex"));

    // Detect karvi project
    let board_path = Path::new(cwd).join("server/board.json");
    if !board_path.exists() {
        return None;
    }

    // Extract task ID from git branch
    let branch = crate::peers::detect_git_branch_in(cwd)?;
    let task_id = RE_TASK_ID.find(&branch)?.as_str();

    // Read brief file
    let brief_path = Path::new(cwd)
        .join("server/briefs")
        .join(format!("{}.md", task_id));
    if !brief_path.exists() {
        return None;
    }

    let contents = fs::read_to_string(&brief_path).ok()?;
    let truncated = if contents.len() > 2000 {
        &contents[..2000]
    } else {
        &contents
    };

    Some(format!("[karvi task brief: {}]\n{}", task_id, truncated))
}
