use edda_bridge_claude::{render, state};

use crate::parse::CursorEnvelope;

#[derive(Debug, Default, Clone)]
pub struct HookResult {
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

impl HookResult {
    fn output(stdout: String) -> Self {
        Self {
            stdout: Some(stdout),
            stderr: None,
        }
    }
}

fn empty_json() -> HookResult {
    HookResult::output("{}".to_string())
}

fn continue_json() -> HookResult {
    HookResult::output(r#"{"continue":true}"#.to_string())
}

fn allow_json() -> HookResult {
    HookResult::output(r#"{"permission":"allow"}"#.to_string())
}

fn pre_tool_warning(warning: &str) -> anyhow::Result<HookResult> {
    let output = serde_json::json!({
        "permission": "allow",
        "agent_message": warning,
    });
    Ok(HookResult::output(serde_json::to_string(&output)?))
}

fn additional_context(context: &str) -> anyhow::Result<HookResult> {
    let output = serde_json::json!({"additional_context": context});
    Ok(HookResult::output(serde_json::to_string(&output)?))
}

pub fn hook_entrypoint_from_stdin(stdin: &str) -> anyhow::Result<HookResult> {
    if stdin.trim().is_empty() {
        return Ok(HookResult::default());
    }
    let envelope = match crate::parse::parse_hook_stdin(stdin) {
        Ok(envelope) => envelope,
        Err(_) => return Ok(empty_json()),
    };
    let cwd = envelope.cwd();
    let project_id = edda_store::project_id(&cwd);
    let _ = edda_store::ensure_dirs(&project_id);
    let _ = crate::parse::append_to_session_ledger(&envelope);
    if !envelope.session_id().is_empty() {
        edda_bridge_claude::peers::touch_heartbeat(&project_id, envelope.session_id());
    }

    let result = match envelope.event_name() {
        "sessionStart" => dispatch_session_start(&project_id, &envelope),
        "beforeSubmitPrompt" => Ok(continue_json()),
        "preToolUse" => dispatch_pre_tool_use(&project_id, &envelope),
        "postToolUse" => dispatch_post_tool_use(&project_id, &envelope),
        "preCompact" => {
            state::set_compact_pending(&project_id);
            Ok(empty_json())
        }
        "sessionEnd" | "stop" => dispatch_session_end(&project_id, &envelope),
        "subagentStart" => Ok(dispatch_subagent_start(&project_id, &envelope)),
        "subagentStop" => Ok(dispatch_subagent_stop(&project_id, &envelope)),
        _ => Ok(empty_json()),
    };
    Ok(result.unwrap_or_else(|_| empty_json()))
}

fn dispatch_subagent_start(project_id: &str, envelope: &CursorEnvelope) -> HookResult {
    if !envelope.subagent_id.is_empty() {
        let label = if envelope.subagent_type.is_empty() {
            "subagent"
        } else {
            &envelope.subagent_type
        };
        edda_bridge_claude::peers::write_heartbeat_minimal(
            project_id,
            &envelope.subagent_id,
            label,
            &envelope.cwd().to_string_lossy(),
        );
    }
    allow_json()
}

fn dispatch_subagent_stop(project_id: &str, envelope: &CursorEnvelope) -> HookResult {
    if !envelope.subagent_id.is_empty() {
        edda_bridge_claude::peers::remove_heartbeat(project_id, &envelope.subagent_id);
    }
    empty_json()
}

fn dispatch_session_end(project_id: &str, envelope: &CursorEnvelope) -> anyhow::Result<HookResult> {
    let session_id = envelope.session_id();
    if !session_id.is_empty() {
        let cwd = envelope.cwd();
        let _ = edda_bridge_claude::digest::digest_session_manual(
            project_id,
            session_id,
            &cwd.to_string_lossy(),
            true,
        );
    }
    let peers_active = !session_id.is_empty()
        && !edda_bridge_claude::peers::discover_active_peers(project_id, session_id).is_empty();
    cleanup_session_state(project_id, session_id, peers_active);
    Ok(empty_json())
}

fn cleanup_session_state(project_id: &str, session_id: &str, peers_active: bool) {
    let state_dir = edda_store::project_dir(project_id).join("state");
    for name in [
        "inject_hash",
        "nudge_ts",
        "nudge_count",
        "decide_count",
        "signal_count",
        "peer_count",
        "coord_offset",
    ] {
        let _ = std::fs::remove_file(state_dir.join(format!("{name}.{session_id}")));
    }
    let _ = std::fs::remove_file(state_dir.join(format!("phase.{session_id}.json")));
    let _ = std::fs::remove_file(state_dir.join("compact_pending"));
    edda_bridge_claude::peers::remove_heartbeat(project_id, session_id);
    if peers_active {
        edda_bridge_claude::peers::write_unclaim(project_id, session_id);
    }
}

fn dispatch_post_tool_use(
    project_id: &str,
    envelope: &CursorEnvelope,
) -> anyhow::Result<HookResult> {
    let session_id = envelope.session_id();
    let tool_name = if envelope.tool_name == "Shell" {
        "Bash"
    } else {
        &envelope.tool_name
    };
    let raw = serde_json::json!({
        "tool_name": tool_name,
        "tool_input": envelope.tool_input,
    });
    let signal = match edda_bridge_claude::nudge::detect_signal(&raw) {
        Some(signal) => signal,
        None => return Ok(empty_json()),
    };

    state::increment_counter(project_id, session_id, "signal_count");
    if signal == edda_bridge_claude::nudge::NudgeSignal::SelfRecord {
        state::increment_counter(project_id, session_id, "decide_count");
        return Ok(empty_json());
    }
    if !state::should_nudge(project_id, session_id) {
        return Ok(empty_json());
    }

    let decide_count = state::read_counter(project_id, session_id, "decide_count");
    let nudge = edda_bridge_claude::nudge::format_nudge(&signal, decide_count);
    if nudge.is_empty() {
        return Ok(empty_json());
    }
    state::mark_nudge_sent(project_id, session_id);
    state::increment_counter(project_id, session_id, "nudge_count");
    additional_context(&nudge)
}

fn dispatch_pre_tool_use(
    project_id: &str,
    envelope: &CursorEnvelope,
) -> anyhow::Result<HookResult> {
    if std::env::var("EDDA_POSTMORTEM").unwrap_or_else(|_| "1".to_string()) == "0" {
        return Ok(empty_json());
    }

    let mut rules_store = edda_postmortem::RulesStore::load_project(project_id);
    if rules_store.active_rules().is_empty() {
        return Ok(empty_json());
    }

    let files_touched = ["file_path", "path", "filePath"]
        .iter()
        .find_map(|key| {
            envelope
                .tool_input
                .get(key)
                .and_then(|value| value.as_str())
        })
        .map(|path| vec![path.to_string()])
        .unwrap_or_default();
    let command = envelope
        .tool_input
        .get("command")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let context = edda_postmortem::hooks::HookContext {
        hook_event: "PreToolUse".to_string(),
        tool_name: envelope.tool_name.clone(),
        files_touched,
        cwd: envelope.cwd().to_string_lossy().into_owned(),
        command,
    };

    let evaluation = edda_postmortem::hooks::evaluate_rules(&rules_store, &context);
    if !evaluation.matched_rule_ids.is_empty() {
        edda_postmortem::hooks::record_matched_hits(&mut rules_store, &evaluation.matched_rule_ids);
        let _ = rules_store.save_project(project_id);
    }
    match edda_postmortem::hooks::format_warnings(&evaluation) {
        Some(warning) => pre_tool_warning(&warning),
        None => Ok(empty_json()),
    }
}

fn dispatch_session_start(
    project_id: &str,
    envelope: &CursorEnvelope,
) -> anyhow::Result<HookResult> {
    let cwd = envelope.cwd();
    let cwd_text = cwd.to_string_lossy();
    let session_id = envelope.session_id();

    if !session_id.is_empty() {
        let label = std::env::var("EDDA_SESSION_LABEL").unwrap_or_default();
        edda_bridge_claude::peers::write_heartbeat_minimal(
            project_id, session_id, &label, &cwd_text,
        );
    }

    let mut body_parts = Vec::new();
    let doctrine_budget = env_usize("EDDA_DOCTRINE_BUDGET_CHARS", 4000);
    if let Some(doctrine) = edda_pack::read_doctrine_pack(&cwd, doctrine_budget) {
        body_parts.push(doctrine);
    }

    let workspace_budget = env_usize("EDDA_WORKSPACE_BUDGET_CHARS", 2500);
    if let Some(workspace) = render::workspace(&cwd_text, workspace_budget) {
        body_parts.push(workspace);
    }
    if let Some(pack) = render::pack(project_id) {
        body_parts.push(pack);
    }
    if let Some(plan) = render::plan(Some(project_id)) {
        body_parts.push(plan);
    }

    let body = body_parts.join("\n\n");
    let mut tail = format!("\n\n{}", render::writeback());
    if let Some(coordination) =
        edda_bridge_claude::peers::render_coordination_protocol(project_id, session_id, &cwd_text)
    {
        tail.push_str("\n\n");
        tail.push_str(&coordination);
    }

    let body_budget = render::context_budget(&cwd_text).saturating_sub(tail.len());
    let content = format!("{}{tail}", render::apply_budget(&body, body_budget));
    let wrapped = render::wrap_boundary(&content);
    let sidefile = context_sidefile(project_id, &wrapped)?;
    let output = serde_json::json!({
        "env": {
            "EDDA_HOT_PACK_PATH": sidefile.to_string_lossy(),
        },
        "additional_context": wrapped,
    });
    Ok(HookResult::output(serde_json::to_string(&output)?))
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn context_sidefile(project_id: &str, context: &str) -> anyhow::Result<std::path::PathBuf> {
    let packs_dir = edda_store::project_dir(project_id).join("packs");
    let hot_path = packs_dir.join("hot.md");
    if hot_path.is_file() {
        return Ok(hot_path);
    }

    std::fs::create_dir_all(&packs_dir)?;
    let generated_path = packs_dir.join("cursor-context.md");
    std::fs::write(&generated_path, context)?;
    Ok(generated_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stdin_returns_no_output() {
        let result = hook_entrypoint_from_stdin("").unwrap();

        assert!(result.stdout.is_none());
        assert!(result.stderr.is_none());
    }

    #[test]
    fn malformed_stdin_fails_open_with_empty_json() {
        let result = hook_entrypoint_from_stdin("not json").unwrap();

        assert_eq!(result.stdout.as_deref(), Some("{}"));
        assert!(result.stderr.is_none());
    }

    #[test]
    fn session_start_pushes_context_and_readable_sidefile() {
        let tmp = tempfile::tempdir().unwrap();
        let stdin = serde_json::json!({
            "hook_event_name": "sessionStart",
            "session_id": "cursor-session-start-1",
            "workspace_roots": [tmp.path()],
            "composer_mode": "agent"
        })
        .to_string();

        let result = hook_entrypoint_from_stdin(&stdin).unwrap();
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_deref().unwrap()).unwrap();

        let context = output["additional_context"].as_str().unwrap();
        assert!(context.contains("Write-Back Protocol"));
        assert!(context.contains("edda decide"));
        let sidefile = output["env"]["EDDA_HOT_PACK_PATH"].as_str().unwrap();
        assert!(std::path::Path::new(sidefile).is_file());
    }

    #[test]
    fn before_submit_prompt_explicitly_continues() {
        let stdin = serde_json::json!({
            "hook_event_name": "beforeSubmitPrompt",
            "conversation_id": "cursor-prompt-1",
            "cwd": "/work/project"
        })
        .to_string();

        let result = hook_entrypoint_from_stdin(&stdin).unwrap();
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_deref().unwrap()).unwrap();

        assert_eq!(output["continue"], true);
    }

    #[test]
    fn pre_tool_warning_uses_cursor_permission_schema() {
        let result = pre_tool_warning("Review the governing decision").unwrap();
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_deref().unwrap()).unwrap();

        assert_eq!(output["permission"], "allow");
        assert_eq!(output["agent_message"], "Review the governing decision");
    }

    #[test]
    fn post_tool_use_maps_cursor_shell_and_pushes_nudge() {
        let tmp = tempfile::tempdir().unwrap();
        let stdin = serde_json::json!({
            "hook_event_name": "postToolUse",
            "session_id": "cursor-post-tool-1",
            "workspace_roots": [tmp.path()],
            "tool_name": "Shell",
            "tool_input": {"command": "git commit -m checkpoint"}
        })
        .to_string();

        let result = hook_entrypoint_from_stdin(&stdin).unwrap();
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_deref().unwrap()).unwrap();

        let context = output["additional_context"].as_str().unwrap();
        assert!(context.contains("edda decide"));
    }

    #[test]
    fn pre_compact_marks_recovery_state_without_claiming_rebuild() {
        let tmp = tempfile::tempdir().unwrap();
        let project_id = edda_store::project_id(tmp.path());
        let _ = edda_bridge_claude::state::take_compact_pending(&project_id);
        let stdin = serde_json::json!({
            "hook_event_name": "preCompact",
            "session_id": "cursor-compact-1",
            "workspace_roots": [tmp.path()]
        })
        .to_string();

        let result = hook_entrypoint_from_stdin(&stdin).unwrap();

        assert_eq!(result.stdout.as_deref(), Some("{}"));
        assert!(edda_bridge_claude::state::take_compact_pending(&project_id));
    }

    #[test]
    fn session_end_cleans_session_state() {
        let tmp = tempfile::tempdir().unwrap();
        let project_id = edda_store::project_id(tmp.path());
        let session_id = "cursor-session-end-1";
        let start = serde_json::json!({
            "hook_event_name": "sessionStart",
            "session_id": session_id,
            "workspace_roots": [tmp.path()]
        })
        .to_string();
        hook_entrypoint_from_stdin(&start).unwrap();
        let heartbeat = edda_store::project_dir(&project_id)
            .join("state")
            .join(format!("session.{session_id}.json"));
        assert!(heartbeat.is_file());

        let end = serde_json::json!({
            "hook_event_name": "sessionEnd",
            "session_id": session_id,
            "workspace_roots": [tmp.path()]
        })
        .to_string();
        let result = hook_entrypoint_from_stdin(&end).unwrap();

        assert_eq!(result.stdout.as_deref(), Some("{}"));
        assert!(!heartbeat.exists());
    }

    #[test]
    fn hook_events_are_recorded_in_cursor_session_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let project_id = edda_store::project_id(tmp.path());
        let session_id = "cursor-ledger-1";
        let stdin = serde_json::json!({
            "hook_event_name": "beforeSubmitPrompt",
            "session_id": session_id,
            "workspace_roots": [tmp.path()],
            "model": "composer"
        })
        .to_string();

        hook_entrypoint_from_stdin(&stdin).unwrap();

        let ledger = edda_store::project_dir(&project_id)
            .join("ledger")
            .join(format!("{session_id}.jsonl"));
        let recorded = std::fs::read_to_string(ledger).unwrap();
        assert!(recorded.contains(r#""bridge":"cursor""#));
        assert!(recorded.contains(r#""hook_event_name":"beforeSubmitPrompt""#));
    }

    #[test]
    fn subagent_lifecycle_tracks_and_clears_peer_heartbeat() {
        let tmp = tempfile::tempdir().unwrap();
        let project_id = edda_store::project_id(tmp.path());
        let subagent_id = "cursor-subagent-1";
        let start = serde_json::json!({
            "hook_event_name": "subagentStart",
            "conversation_id": "cursor-parent-1",
            "workspace_roots": [tmp.path()],
            "subagent_id": subagent_id,
            "subagent_type": "explore"
        })
        .to_string();

        let result = hook_entrypoint_from_stdin(&start).unwrap();
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_deref().unwrap()).unwrap();
        assert_eq!(output["permission"], "allow");
        let heartbeat = edda_store::project_dir(&project_id)
            .join("state")
            .join(format!("session.{subagent_id}.json"));
        assert!(heartbeat.is_file());

        let stop = serde_json::json!({
            "hook_event_name": "subagentStop",
            "conversation_id": "cursor-parent-1",
            "workspace_roots": [tmp.path()],
            "subagent_id": subagent_id
        })
        .to_string();
        hook_entrypoint_from_stdin(&stop).unwrap();
        assert!(!heartbeat.exists());
    }
}
