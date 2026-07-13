use super::board::compute_board_state;
use super::heartbeat::read_heartbeat;
use super::RequestEntry;
use crate::signals::SessionSignals;

pub(crate) fn pending_requests_for_session(
    project_id: &str,
    session_id: &str,
) -> Vec<RequestEntry> {
    let board = compute_board_state(project_id);

    // Resolve my label from claim or heartbeat
    let my_label: String = board
        .claims
        .iter()
        .find(|c| c.session_id == session_id)
        .map(|c| c.label.clone())
        .or_else(|| read_heartbeat(project_id, session_id).map(|hb| hb.label))
        .unwrap_or_default();

    if my_label.is_empty() {
        return Vec::new();
    }

    board
        .requests
        .into_iter()
        .filter(|r| r.to_label == my_label)
        .filter(|r| {
            !board
                .request_acks
                .iter()
                .any(|a| a.from_label == r.from_label && a.acker_session == session_id)
        })
        .collect()
}

// ── Helpers ──

/// True if a normalized path is absolute (`/...` or `C:/...`).
pub(super) fn is_absolute_normalized(path: &str) -> bool {
    path.starts_with('/')
        || matches!(path.as_bytes(), [a, b':', b'/', ..] if a.is_ascii_alphabetic())
}

/// Make a path project-relative when possible.
///
/// Hook payloads carry absolute paths (e.g. `C:\repo\docs\x.md`), but scope
/// and label derivation assume repo-relative paths — feeding absolute Windows
/// paths through produced garbage like label `C:` / claim `C:/*`.
/// Strips the cwd prefix (case-insensitive, for Windows); returns `None` for
/// absolute paths outside cwd so callers can skip them instead of deriving
/// garbage. Relative paths pass through normalized.
pub(super) fn relativize(path: &str, cwd: Option<&str>) -> Option<String> {
    let norm = path.replace('\\', "/");
    if !is_absolute_normalized(&norm) {
        return Some(norm);
    }
    let cwd = cwd?;
    let cwd_norm = cwd.replace('\\', "/");
    let cwd_trim = cwd_norm.trim_end_matches('/');
    if cwd_trim.is_empty() {
        return None;
    }
    if norm.len() > cwd_trim.len() + 1
        && norm[..cwd_trim.len()].eq_ignore_ascii_case(cwd_trim)
        && norm.as_bytes()[cwd_trim.len()] == b'/'
    {
        Some(norm[cwd_trim.len() + 1..].to_string())
    } else {
        None
    }
}

/// Auto-derive a label from session signals (focus files).
pub(super) fn auto_label(signals: &SessionSignals, cwd: Option<&str>) -> String {
    if signals.files_modified.is_empty() {
        return String::new();
    }

    // Try to extract crate/module name from the most-edited file
    let top_file = signals
        .files_modified
        .iter()
        .max_by_key(|f| f.count)
        .map(|f| f.path.as_str())
        .unwrap_or("");

    // Relativize so absolute hook paths don't leak drive letters or
    // machine-specific prefixes into labels.
    let normalized = match relativize(top_file, cwd) {
        Some(p) => p,
        None => top_file.replace('\\', "/"),
    };
    let segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();

    // Look for crate name pattern: crates/{name}/src/...
    if let Some(pos) = segments.iter().position(|&s| s == "crates") {
        if let Some(name) = segments.get(pos + 1) {
            return name.to_string();
        }
    }

    // Look for src/{name}/...
    if let Some(pos) = segments.iter().position(|&s| s == "src") {
        if let Some(name) = segments.get(pos + 1) {
            if !name.contains('.') {
                return name.to_string();
            }
        }
    }

    // Fall back to parent directory of top file — but never a path-artifact
    // segment like a drive letter (`C:`).
    if segments.len() >= 2 {
        let parent = segments[segments.len() - 2];
        if !parent.ends_with(':') {
            return parent.to_string();
        }
    }

    String::new()
}

/// Format age in human-readable form.
/// Format the bracket suffix for a peer line: `[branch: x, phase]` or `[branch: x]` etc.
pub(crate) fn format_peer_suffix(branch: Option<&str>, phase: Option<&str>) -> String {
    match (branch, phase) {
        (Some(b), Some(p)) => format!(" [branch: {b}, {p}]"),
        (Some(b), None) => format!(" [branch: {b}]"),
        (None, Some(p)) => format!(" [{p}]"),
        (None, None) => String::new(),
    }
}

pub fn format_age(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}

/// Truncate content to budget, cutting at last newline before budget.
pub(super) fn truncate_to_budget(content: &str, budget: usize) -> String {
    if content.len() <= budget {
        return content.to_string();
    }
    let truncated = &content[..budget.min(content.len())];
    // Cut at last newline for clean truncation
    if let Some(pos) = truncated.rfind('\n') {
        truncated[..pos].to_string()
    } else {
        truncated.to_string()
    }
}

/// Parse RFC3339 timestamp to Unix epoch seconds (basic parser).
pub(super) fn parse_rfc3339_to_epoch(ts: &str) -> Option<u64> {
    // Format: 2026-02-16T10:05:23+00:00 or 2026-02-16T10:05:23Z
    // Simple approach: parse with chrono-like logic manually
    // We only need relative comparison, so parsing the digits is enough
    let ts = ts.trim();
    if ts.len() < 19 {
        return None;
    }

    let year: u64 = ts[0..4].parse().ok()?;
    let month: u64 = ts[5..7].parse().ok()?;
    let day: u64 = ts[8..10].parse().ok()?;
    let hour: u64 = ts[11..13].parse().ok()?;
    let min: u64 = ts[14..16].parse().ok()?;
    let sec: u64 = ts[17..19].parse().ok()?;

    // Approximate epoch (good enough for relative age computation)
    // Days since epoch (1970-01-01), ignoring leap seconds
    let days_in_year = 365;
    let years_since_1970 = year.saturating_sub(1970);
    let leap_years = (year.saturating_sub(1969)) / 4 - (year.saturating_sub(1901)) / 100
        + (year.saturating_sub(1601)) / 400;

    let month_days: [u64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut total_days = years_since_1970 * days_in_year + leap_years;
    for d in month_days
        .iter()
        .take((month.saturating_sub(1) as usize).min(11))
    {
        total_days += d;
    }
    // Add leap day for current year if applicable
    if month > 2
        && (year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)))
    {
        total_days += 1;
    }
    total_days += day.saturating_sub(1);

    Some(total_days * 86400 + hour * 3600 + min * 60 + sec)
}

// ── Tests ──
