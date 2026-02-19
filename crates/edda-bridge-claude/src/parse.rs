use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

// ── EventEnvelope ──

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EventEnvelope {
    pub ts: String,
    pub project_id: String,
    pub session_id: String,
    pub hook_event_name: String,
    #[serde(default)]
    pub transcript_path: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub permission_mode: String,
    #[serde(default)]
    pub tool_name: String,
    #[serde(default)]
    pub tool_use_id: String,
    #[serde(default)]
    pub raw: serde_json::Value,
}

// ── Hook stdin parsing ──

/// Parse the stdin JSON from Claude Code hook.
/// Returns the raw parsed value along with extracted fields.
pub(crate) fn parse_hook_stdin(stdin: &str) -> anyhow::Result<serde_json::Value> {
    let val: serde_json::Value = serde_json::from_str(stdin)?;
    Ok(val)
}

/// Get a string field from JSON, trying snake_case first then camelCase.
/// Claude Code sends camelCase (e.g. `hookEventName`), but our internal
/// tests use snake_case (e.g. `hook_event_name`).
pub(crate) fn get_str(v: &serde_json::Value, snake_key: &str) -> String {
    // Try snake_case first (internal/test format)
    if let Some(s) = v.get(snake_key).and_then(|x| x.as_str()) {
        return s.to_string();
    }
    // Try camelCase (Claude Code format)
    let camel = snake_to_camel(snake_key);
    v.get(&camel)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

pub(crate) fn snake_to_camel(s: &str) -> String {
    let mut result = String::new();
    let mut capitalize_next = false;
    for ch in s.chars() {
        if ch == '_' {
            capitalize_next = true;
        } else if capitalize_next {
            result.extend(ch.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(ch);
        }
    }
    result
}

pub(crate) fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}

pub(crate) fn resolve_project_id(cwd: &str) -> String {
    let path = if cwd.is_empty() {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        PathBuf::from(cwd)
    };
    edda_store::project_id(&path)
}

/// Append an EventEnvelope to the session ledger in the per-user store.
pub(crate) fn append_to_session_ledger(envelope: &EventEnvelope) -> anyhow::Result<()> {
    let proj_dir = edda_store::project_dir(&envelope.project_id);
    let ledger_dir = proj_dir.join("ledger");
    fs::create_dir_all(&ledger_dir)?;
    let ledger_path = ledger_dir.join(format!("{}.jsonl", envelope.session_id));
    let line = serde_json::to_string(envelope)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ledger_path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_to_camel_converts_correctly() {
        assert_eq!(snake_to_camel("hook_event_name"), "hookEventName");
        assert_eq!(snake_to_camel("session_id"), "sessionId");
        assert_eq!(snake_to_camel("transcript_path"), "transcriptPath");
        assert_eq!(snake_to_camel("cwd"), "cwd");
        assert_eq!(snake_to_camel("tool_name"), "toolName");
        assert_eq!(snake_to_camel("tool_use_id"), "toolUseId");
        assert_eq!(snake_to_camel("permission_mode"), "permissionMode");
    }
}
