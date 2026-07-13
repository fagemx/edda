//! Hermes shell-hook stdin parsing.
//!
//! Contract verified against `hermes-agent/agent/shell_hooks.py::_serialize_payload`:
//!
//! ```json
//! {
//!   "hook_event_name": "pre_tool_call",
//!   "tool_name":       "terminal",
//!   "tool_input":      {"command": "..."},
//!   "session_id":      "sess_abc",
//!   "cwd":             "/path",
//!   "extra":           { task_id, turn_id, api_request_id, tool_call_id, middleware_trace, ... }
//! }
//! ```
//!
//! `cwd` is `Path.cwd()` of the Hermes process (line 542 in shell_hooks.py),
//! not a workspace field that clients pass through. We map it to project_id
//! the same way — Hermes is typically launched from the project root.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct HermesEnvelope {
    #[serde(default)]
    pub hook_event_name: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub tool_name: String,
    #[serde(default)]
    pub tool_input: serde_json::Value,
    #[serde(default)]
    pub extra: serde_json::Value,
}

pub(crate) fn parse_hook_stdin(stdin: &str) -> anyhow::Result<HermesEnvelope> {
    let envelope: HermesEnvelope = serde_json::from_str(stdin)?;
    if envelope.hook_event_name.is_empty() {
        anyhow::bail!("missing required field: hook_event_name");
    }
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
pub(crate) fn append_to_session_ledger(envelope: &HermesEnvelope) -> anyhow::Result<()> {
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
        "tool_name": envelope.tool_name,
        "bridge": "hermes",
    });

    let line = serde_json::to_string(&record)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ledger_path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

/// Read `is_first_turn` from the `extra` bag when present.
///
/// Hermes' `pre_llm_call` includes `is_first_turn: bool` in its kwargs
/// (docs: user-guide/features/hooks) — this lets one dispatch decide between
/// full SessionStart-shape injection (first turn) and lightweight
/// UserPromptSubmit-shape injection (subsequent turns).
pub(crate) fn extra_bool(envelope: &HermesEnvelope, key: &str) -> bool {
    envelope
        .extra
        .get(key)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pre_llm_call_first_turn() {
        let stdin = r#"{
            "hook_event_name":"pre_llm_call",
            "session_id":"h1",
            "cwd":"/tmp",
            "extra":{"is_first_turn":true,"model":"claude","platform":"cli"}
        }"#;
        let env = parse_hook_stdin(stdin).unwrap();
        assert_eq!(env.hook_event_name, "pre_llm_call");
        assert!(extra_bool(&env, "is_first_turn"));
    }

    #[test]
    fn parse_pre_tool_call_terminal() {
        let stdin = r#"{
            "hook_event_name":"pre_tool_call",
            "tool_name":"terminal",
            "tool_input":{"command":"rm -rf /"},
            "session_id":"h2",
            "cwd":"/repo",
            "extra":{"task_id":"","tool_call_id":"tc1","turn_id":"t1"}
        }"#;
        let env = parse_hook_stdin(stdin).unwrap();
        assert_eq!(env.tool_name, "terminal");
        assert_eq!(env.tool_input["command"], "rm -rf /");
    }

    #[test]
    fn parse_subsequent_turn_defaults_to_false() {
        let stdin =
            r#"{"hook_event_name":"pre_llm_call","session_id":"h3","cwd":"/tmp","extra":{}}"#;
        let env = parse_hook_stdin(stdin).unwrap();
        assert!(!extra_bool(&env, "is_first_turn"));
    }

    #[test]
    fn parse_missing_event_name_fails() {
        assert!(parse_hook_stdin(r#"{"session_id":"s"}"#).is_err());
    }
}
