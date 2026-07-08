//! Codex hook stdin parsing.
//!
//! Codex sends every hook event as a JSON object over stdin with a shared
//! envelope shape:
//!
//! ```json
//! {
//!   "session_id": "...",
//!   "cwd": "/repo/root",
//!   "hook_event_name": "SessionStart" | "UserPromptSubmit" | "PreToolUse" | ...,
//!   "model": "gpt-5-codex",
//!   "transcript_path": "/path/to/rollout.jsonl",
//!   "permission_mode": "auto" | "manual",
//!   // event-specific fields:
//!   "tool_name": "Bash",
//!   "tool_input": { ... },
//!   "tool_response": { ... }
//! }
//! ```
//!
//! We keep this parse layer thin — normalization happens in dispatch.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct CodexEnvelope {
    #[serde(default)]
    pub hook_event_name: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub transcript_path: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub permission_mode: String,
    #[serde(default)]
    pub tool_name: String,
    #[serde(default)]
    pub tool_use_id: String,
    #[serde(default)]
    pub tool_input: serde_json::Value,
    #[serde(default)]
    pub tool_response: serde_json::Value,
    /// Original raw payload — kept so postmortem/redact modules can inspect
    /// event-specific fields we haven't strongly typed yet.
    #[serde(skip)]
    pub raw: serde_json::Value,
}

pub(crate) fn parse_hook_stdin(stdin: &str) -> anyhow::Result<CodexEnvelope> {
    let raw: serde_json::Value = serde_json::from_str(stdin)?;
    let mut envelope: CodexEnvelope = serde_json::from_value(raw.clone())?;
    if envelope.hook_event_name.is_empty() {
        anyhow::bail!("missing required field: hook_event_name");
    }
    envelope.raw = raw;
    Ok(envelope)
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

/// Append the event envelope to the per-session ledger for later inspection.
pub(crate) fn append_to_session_ledger(envelope: &CodexEnvelope) -> anyhow::Result<()> {
    if envelope.session_id.is_empty() {
        return Ok(());
    }
    let project_id = resolve_project_id(&envelope.cwd);
    let proj_dir = edda_store::project_dir(&project_id);
    let ledger_dir = proj_dir.join("ledger");
    fs::create_dir_all(&ledger_dir)?;
    let ledger_path = ledger_dir.join(format!("{}.jsonl", envelope.session_id));

    let record = serde_json::json!({
        "ts": now_rfc3339(),
        "project_id": project_id,
        "session_id": envelope.session_id,
        "hook_event_name": envelope.hook_event_name,
        "cwd": envelope.cwd,
        "model": envelope.model,
        "tool_name": envelope.tool_name,
        "bridge": "codex",
    });

    let line = serde_json::to_string(&record)?;
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
    fn parse_session_start() {
        let stdin = r#"{"hook_event_name":"SessionStart","session_id":"s1","cwd":"/tmp/x","model":"gpt-5-codex"}"#;
        let env = parse_hook_stdin(stdin).unwrap();
        assert_eq!(env.hook_event_name, "SessionStart");
        assert_eq!(env.session_id, "s1");
        assert_eq!(env.cwd, "/tmp/x");
        assert_eq!(env.model, "gpt-5-codex");
    }

    #[test]
    fn parse_pre_tool_use_bash() {
        let stdin = r#"{
            "hook_event_name":"PreToolUse",
            "session_id":"s2",
            "cwd":"/repo",
            "tool_name":"Bash",
            "tool_input":{"command":"ls -la"}
        }"#;
        let env = parse_hook_stdin(stdin).unwrap();
        assert_eq!(env.tool_name, "Bash");
        assert_eq!(env.tool_input["command"], "ls -la");
    }

    #[test]
    fn parse_missing_event_name_fails() {
        assert!(parse_hook_stdin(r#"{"session_id":"s1"}"#).is_err());
    }

    #[test]
    fn parse_malformed_fails() {
        assert!(parse_hook_stdin("not json").is_err());
    }
}
