// ── Helpers ──

pub(super) fn extract_file_path(envelope: &serde_json::Value) -> Option<String> {
    // Try direct tool_input (our internal format)
    if let Some(fp) = envelope
        .get("raw")
        .and_then(|r| r.get("tool_input").or_else(|| r.get("toolInput")))
        .and_then(|ti| ti.get("file_path").or_else(|| ti.get("filePath")))
        .and_then(|v| v.as_str())
    {
        return Some(normalize_path(fp));
    }
    // Try top-level tool_input (when raw is flattened)
    envelope
        .get("tool_input")
        .and_then(|ti| ti.get("file_path"))
        .and_then(|v| v.as_str())
        .map(normalize_path)
}

pub(super) fn extract_envelope_cwd(envelope: &serde_json::Value) -> String {
    envelope
        .get("cwd")
        .or_else(|| envelope.get("raw").and_then(|r| r.get("cwd")))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

pub(super) fn extract_exit_code(envelope: &serde_json::Value) -> i32 {
    let raw = match envelope.get("raw") {
        Some(r) => r,
        None => return 1,
    };

    // Path 1: raw.tool_response.exitCode (if Claude Code ever adds it)
    if let Some(code) = raw
        .get("tool_response")
        .or_else(|| raw.get("toolResponse"))
        .and_then(|tr| tr.get("exitCode").or_else(|| tr.get("exit_code")))
        .and_then(|v| v.as_i64())
    {
        return code as i32;
    }

    // Path 2: raw.error = "Exit code {N}" (PostToolUseFailure format)
    if let Some(error_str) = raw.get("error").and_then(|v| v.as_str()) {
        let first_line = error_str.lines().next().unwrap_or("");
        if let Some(code_str) = first_line.strip_prefix("Exit code ") {
            if let Ok(code) = code_str.trim().parse::<i32>() {
                return code;
            }
        }
    }

    // Default: generic failure (only called for PostToolUseFailure events)
    1
}

pub(super) fn extract_bash_command(envelope: &serde_json::Value) -> Option<String> {
    // Try raw.tool_input.command
    if let Some(cmd) = envelope
        .get("raw")
        .and_then(|r| r.get("tool_input").or_else(|| r.get("toolInput")))
        .and_then(|ti| ti.get("command"))
        .and_then(|v| v.as_str())
    {
        return Some(cmd.to_string());
    }
    envelope
        .get("tool_input")
        .and_then(|ti| ti.get("command"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Extract commit message from a `git commit -m "..."` command string.
pub(super) fn extract_git_commit_msg(cmd: &str) -> String {
    // Try to find -m "..." or -m '...' pattern
    if let Some(pos) = cmd.find("-m ") {
        let after_m = &cmd[pos + 3..];
        let trimmed = after_m.trim_start();
        if let Some(first) = trimmed.chars().next() {
            if first == '"' || first == '\'' {
                if let Some(end) = trimmed[1..].find(first) {
                    return trimmed[1..end + 1].to_string();
                }
            }
        }
    }
    String::new()
}

/// Normalize a file path: strip common prefixes for readability.
pub(super) fn normalize_path(path: &str) -> String {
    // Keep the path as-is for now; downstream can shorten if needed
    path.to_string()
}

pub(super) fn compute_duration_minutes(first: &Option<String>, last: &Option<String>) -> u64 {
    let (Some(first), Some(last)) = (first.as_deref(), last.as_deref()) else {
        return 0;
    };
    let fmt = &time::format_description::well_known::Rfc3339;
    let Ok(t1) = time::OffsetDateTime::parse(first, fmt) else {
        return 0;
    };
    let Ok(t2) = time::OffsetDateTime::parse(last, fmt) else {
        return 0;
    };
    let diff: time::Duration = t2 - t1;
    let secs = diff.whole_seconds().unsigned_abs();
    secs / 60
}

pub(super) fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}
