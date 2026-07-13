use serde::Deserialize;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct CursorEnvelope {
    #[serde(default)]
    pub hook_event_name: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub conversation_id: String,
    #[serde(default)]
    pub generation_id: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub workspace_roots: Vec<String>,
    #[serde(default)]
    pub cursor_version: String,
    #[serde(default)]
    pub composer_mode: String,
    #[serde(default)]
    pub is_background_agent: bool,
    #[serde(default)]
    pub transcript_path: Option<PathBuf>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub model_id: String,
    #[serde(default)]
    pub user_email: Option<String>,
    #[serde(default)]
    pub tool_name: String,
    #[serde(default)]
    pub tool_use_id: String,
    #[serde(default)]
    pub tool_input: serde_json::Value,
    #[serde(default)]
    pub tool_output: serde_json::Value,
    #[serde(default)]
    pub duration: Option<u64>,
    #[serde(default)]
    pub subagent_id: String,
    #[serde(default)]
    pub subagent_type: String,
    #[serde(default)]
    pub parent_conversation_id: String,
    #[serde(default)]
    pub task: String,
    #[serde(default)]
    pub git_branch: String,
}

impl CursorEnvelope {
    pub(crate) fn event_name(&self) -> &str {
        match self.hook_event_name.as_str() {
            "SessionStart" => "sessionStart",
            "UserPromptSubmit" => "beforeSubmitPrompt",
            "PreToolUse" => "preToolUse",
            "PostToolUse" => "postToolUse",
            "PreCompact" => "preCompact",
            "SessionEnd" => "sessionEnd",
            "Stop" => "stop",
            "SubagentStart" => "subagentStart",
            "SubagentStop" => "subagentStop",
            event => event,
        }
    }

    pub(crate) fn session_id(&self) -> &str {
        if self.session_id.is_empty() {
            &self.conversation_id
        } else {
            &self.session_id
        }
    }

    pub(crate) fn cwd(&self) -> PathBuf {
        if !self.cwd.is_empty() {
            return cursor_path(&self.cwd);
        }
        self.workspace_roots
            .first()
            .map(|root| cursor_path(root))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}

fn cursor_path(path: &str) -> PathBuf {
    #[cfg(windows)]
    if path.len() >= 4 {
        let bytes = path.as_bytes();
        if bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
            return PathBuf::from(&path[1..]);
        }
    }
    PathBuf::from(path)
}

pub(crate) fn parse_hook_stdin(stdin: &str) -> anyhow::Result<CursorEnvelope> {
    let envelope: CursorEnvelope = serde_json::from_str(stdin)?;
    if envelope.hook_event_name.is_empty() {
        anyhow::bail!("missing required field: hook_event_name");
    }
    Ok(envelope)
}

pub(crate) fn append_to_session_ledger(envelope: &CursorEnvelope) -> anyhow::Result<()> {
    let session_id = envelope.session_id();
    if session_id.is_empty() {
        return Ok(());
    }

    let cwd = envelope.cwd();
    let project_id = edda_store::project_id(&cwd);
    let ledger_dir = edda_store::project_dir(&project_id).join("ledger");
    fs::create_dir_all(&ledger_dir)?;
    let ledger_path = ledger_dir.join(format!("{session_id}.jsonl"));
    let timestamp =
        time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?;
    let record = serde_json::json!({
        "ts": timestamp,
        "project_id": project_id,
        "session_id": session_id,
        "conversation_id": envelope.conversation_id,
        "generation_id": envelope.generation_id,
        "hook_event_name": envelope.event_name(),
        "cwd": cwd,
        "cursor_version": envelope.cursor_version,
        "composer_mode": envelope.composer_mode,
        "is_background_agent": envelope.is_background_agent,
        "transcript_path": envelope.transcript_path,
        "user_email": envelope.user_email,
        "model": envelope.model,
        "model_id": envelope.model_id,
        "tool_name": envelope.tool_name,
        "tool_use_id": envelope.tool_use_id,
        "tool_output": envelope.tool_output,
        "duration": envelope.duration,
        "subagent_id": envelope.subagent_id,
        "subagent_type": envelope.subagent_type,
        "parent_conversation_id": envelope.parent_conversation_id,
        "task": envelope.task,
        "git_branch": envelope.git_branch,
        "bridge": "cursor",
    });

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ledger_path)?;
    writeln!(file, "{}", serde_json::to_string(&record)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_native_session_start_envelope() {
        let stdin = r#"{
            "hook_event_name": "sessionStart",
            "session_id": "cursor-session-1",
            "conversation_id": "conversation-1",
            "workspace_roots": ["C:/work/project"],
            "cursor_version": "3.10.20",
            "composer_mode": "agent",
            "is_background_agent": false,
            "transcript_path": null
        }"#;

        let envelope = parse_hook_stdin(stdin).unwrap();

        assert_eq!(envelope.hook_event_name, "sessionStart");
        assert_eq!(envelope.session_id(), "cursor-session-1");
        assert_eq!(envelope.cwd(), std::path::Path::new("C:/work/project"));
        assert_eq!(envelope.cursor_version, "3.10.20");
        assert_eq!(envelope.composer_mode, "agent");
        assert!(!envelope.is_background_agent);
        assert!(envelope.transcript_path.is_none());
    }

    #[test]
    fn parses_native_pre_tool_use_fields() {
        let stdin = r#"{
            "hook_event_name": "preToolUse",
            "conversation_id": "conversation-2",
            "cwd": "/work/project",
            "tool_name": "Shell",
            "tool_use_id": "tool-1",
            "tool_input": {"command": "cargo test"},
            "tool_output": "not available yet"
        }"#;

        let envelope = parse_hook_stdin(stdin).unwrap();

        assert_eq!(envelope.session_id(), "conversation-2");
        assert_eq!(envelope.tool_name, "Shell");
        assert_eq!(envelope.tool_use_id, "tool-1");
        assert_eq!(envelope.tool_input["command"], "cargo test");
        assert_eq!(envelope.tool_output, "not available yet");
    }

    #[test]
    fn preserves_common_cursor_metadata() {
        let stdin = r#"{
            "hook_event_name": "postToolUse",
            "conversation_id": "conversation-3",
            "generation_id": "generation-3",
            "model": "claude-opus-thinking",
            "model_id": "claude-opus",
            "user_email": "dev@example.com",
            "duration": 5432
        }"#;

        let envelope = parse_hook_stdin(stdin).unwrap();

        assert_eq!(envelope.generation_id, "generation-3");
        assert_eq!(envelope.model, "claude-opus-thinking");
        assert_eq!(envelope.model_id, "claude-opus");
        assert_eq!(envelope.user_email.as_deref(), Some("dev@example.com"));
        assert_eq!(envelope.duration, Some(5432));
    }

    #[test]
    fn normalizes_legacy_pascal_case_event_names() {
        let envelope =
            parse_hook_stdin(r#"{"hook_event_name":"SessionStart","session_id":"legacy-session"}"#)
                .unwrap();

        assert_eq!(envelope.event_name(), "sessionStart");
    }

    #[test]
    fn parses_subagent_lifecycle_fields() {
        let envelope = parse_hook_stdin(
            r#"{
                "hook_event_name":"subagentStart",
                "subagent_id":"subagent-1",
                "subagent_type":"explore",
                "parent_conversation_id":"conversation-parent",
                "task":"Inspect bridge behavior",
                "git_branch":"feature/cursor"
            }"#,
        )
        .unwrap();

        assert_eq!(envelope.subagent_id, "subagent-1");
        assert_eq!(envelope.subagent_type, "explore");
        assert_eq!(envelope.parent_conversation_id, "conversation-parent");
        assert_eq!(envelope.task, "Inspect bridge behavior");
        assert_eq!(envelope.git_branch, "feature/cursor");
    }

    #[cfg(windows)]
    #[test]
    fn normalizes_cursor_windows_workspace_root() {
        let envelope = parse_hook_stdin(
            r#"{"hook_event_name":"sessionStart","workspace_roots":["/C:/work/project"]}"#,
        )
        .unwrap();

        assert_eq!(envelope.cwd(), std::path::Path::new("C:/work/project"));
    }
}
