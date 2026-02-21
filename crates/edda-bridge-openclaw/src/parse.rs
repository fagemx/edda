use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

// ── OpenClaw Envelope ──

/// Parsed envelope from OpenClaw TypeScript plugin.
///
/// The TS plugin serializes hook event data and passes it via stdin
/// to `edda hook openclaw`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OpenClawEnvelope {
    pub hook_event_name: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub session_key: String,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub workspace_dir: String,
    #[serde(default)]
    pub session_file: Option<String>,
    #[serde(default)]
    pub event_data: serde_json::Value,
}

/// Parse the stdin JSON from the OpenClaw TypeScript plugin.
pub(crate) fn parse_hook_stdin(stdin: &str) -> anyhow::Result<OpenClawEnvelope> {
    let envelope: OpenClawEnvelope = serde_json::from_str(stdin)?;
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

pub(crate) fn resolve_project_id(workspace_dir: &str) -> String {
    let path = if workspace_dir.is_empty() {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        PathBuf::from(workspace_dir)
    };
    edda_store::project_id(&path)
}

/// Append an event record to the session ledger in the per-user store.
pub(crate) fn append_to_session_ledger(
    project_id: &str,
    session_id: &str,
    envelope: &OpenClawEnvelope,
) -> anyhow::Result<()> {
    if session_id.is_empty() {
        return Ok(());
    }
    let proj_dir = edda_store::project_dir(project_id);
    let ledger_dir = proj_dir.join("ledger");
    fs::create_dir_all(&ledger_dir)?;
    let ledger_path = ledger_dir.join(format!("{session_id}.jsonl"));

    let record = serde_json::json!({
        "ts": now_rfc3339(),
        "project_id": project_id,
        "session_id": session_id,
        "hook_event_name": &envelope.hook_event_name,
        "agent_id": &envelope.agent_id,
        "workspace_dir": &envelope.workspace_dir,
        "event_data": &envelope.event_data,
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
    fn parse_valid_before_agent_start() {
        let json = r#"{
            "hook_event_name": "before_agent_start",
            "session_id": "abc123",
            "session_key": "agent:main:abc123",
            "agent_id": "main",
            "workspace_dir": "/path/to/project",
            "event_data": { "prompt": "hello" }
        }"#;
        let envelope = parse_hook_stdin(json).unwrap();
        assert_eq!(envelope.hook_event_name, "before_agent_start");
        assert_eq!(envelope.session_id, "abc123");
        assert_eq!(envelope.agent_id, "main");
        assert_eq!(envelope.workspace_dir, "/path/to/project");
    }

    #[test]
    fn parse_valid_agent_end() {
        let json = r#"{
            "hook_event_name": "agent_end",
            "session_id": "xyz789",
            "session_key": "agent:main:xyz789",
            "agent_id": "main",
            "workspace_dir": "/path/to/project",
            "event_data": { "success": true }
        }"#;
        let envelope = parse_hook_stdin(json).unwrap();
        assert_eq!(envelope.hook_event_name, "agent_end");
        assert_eq!(envelope.session_id, "xyz789");
    }

    #[test]
    fn parse_missing_required_field() {
        let json = r#"{ "session_id": "abc123" }"#;
        let result = parse_hook_stdin(json);
        assert!(result.is_err() || result.unwrap().hook_event_name.is_empty());
    }

    #[test]
    fn parse_missing_hook_event_name_value() {
        let json = r#"{ "hook_event_name": "", "session_id": "abc123" }"#;
        let result = parse_hook_stdin(json);
        assert!(result.is_err(), "empty hook_event_name should error");
    }

    #[test]
    fn parse_unknown_event_passes() {
        let json = r#"{
            "hook_event_name": "some_future_event",
            "session_id": "s1",
            "workspace_dir": "/tmp"
        }"#;
        let envelope = parse_hook_stdin(json).unwrap();
        assert_eq!(envelope.hook_event_name, "some_future_event");
    }

    #[test]
    fn append_session_ledger_creates_file() {
        let pid = "test_oc_ledger_append";
        let _ = edda_store::ensure_dirs(pid);

        let envelope = OpenClawEnvelope {
            hook_event_name: "before_agent_start".into(),
            session_id: "s1".into(),
            session_key: "agent:main:s1".into(),
            agent_id: "main".into(),
            workspace_dir: "/tmp".into(),
            session_file: None,
            event_data: serde_json::json!({}),
        };

        append_to_session_ledger(pid, "s1", &envelope).unwrap();

        let ledger_path = edda_store::project_dir(pid).join("ledger").join("s1.jsonl");
        assert!(ledger_path.exists(), "ledger file should be created");

        let content = fs::read_to_string(&ledger_path).unwrap();
        assert!(content.contains("before_agent_start"));

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn append_empty_session_id_is_noop() {
        let pid = "test_oc_ledger_empty_sid";
        let _ = edda_store::ensure_dirs(pid);

        let envelope = OpenClawEnvelope {
            hook_event_name: "before_agent_start".into(),
            session_id: "".into(),
            session_key: "".into(),
            agent_id: "main".into(),
            workspace_dir: "/tmp".into(),
            session_file: None,
            event_data: serde_json::json!({}),
        };

        // Should not error, just skip
        append_to_session_ledger(pid, "", &envelope).unwrap();

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }
}
