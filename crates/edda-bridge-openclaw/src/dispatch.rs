use std::fs;

use edda_bridge_claude::{render, state};

use crate::parse::*;

// ── Hook Result ──

/// Result from an OpenClaw hook dispatch.
///
/// - `stdout`: JSON string to print to stdout (consumed by TS plugin)
/// - `stderr`: warning message to print to stderr
#[derive(Debug, Default, Clone)]
pub struct HookResult {
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

impl HookResult {
    pub fn output(stdout: String) -> Self {
        Self {
            stdout: Some(stdout),
            stderr: None,
        }
    }

    pub fn empty() -> Self {
        Self::default()
    }
}

// ── Hook dispatch ──

/// Main hook entrypoint: parse stdin JSON, dispatch by hook_event_name.
///
/// Returns `HookResult` with optional stdout JSON.
/// Fail-open: errors never crash — always returns Ok.
pub fn hook_entrypoint_from_stdin(stdin: &str) -> anyhow::Result<HookResult> {
    if stdin.trim().is_empty() {
        return Ok(HookResult::empty());
    }

    let envelope = match parse_hook_stdin(stdin) {
        Ok(e) => e,
        Err(_) => {
            // Fail-open: malformed JSON → return ok
            return Ok(ok_json());
        }
    };

    let project_id = resolve_project_id(&envelope.workspace_dir);
    let _ = edda_store::ensure_dirs(&project_id);

    // Append to session ledger
    let _ = append_to_session_ledger(&project_id, &envelope.session_id, &envelope);

    // Touch heartbeat for liveness on every event (lightweight)
    if !envelope.session_id.is_empty() {
        edda_bridge_claude::peers::touch_heartbeat(&project_id, &envelope.session_id);
    }

    match envelope.hook_event_name.as_str() {
        "session_start" => dispatch_session_start(&project_id, &envelope),
        "before_agent_start" => dispatch_before_agent_start(&project_id, &envelope),
        "after_tool_call" => dispatch_after_tool_call(&project_id, &envelope),
        "before_compaction" => dispatch_before_compaction(&project_id),
        "message_sent" => dispatch_message_sent(&project_id, &envelope),
        "agent_end" => dispatch_agent_end(&project_id, &envelope),
        "session_end" => dispatch_session_end(&project_id, &envelope),
        // P2 stubs — forward-compatible
        "before_tool_call" | "after_compaction" | "message_received" => Ok(ok_json()),
        _ => Ok(ok_json()),
    }
}

/// Return a minimal `{ "ok": true }` JSON response.
fn ok_json() -> HookResult {
    HookResult::output(r#"{"ok":true}"#.to_string())
}

// ── session_start ──

/// Handle session start: write heartbeat, auto-digest prior sessions.
fn dispatch_session_start(
    project_id: &str,
    envelope: &OpenClawEnvelope,
) -> anyhow::Result<HookResult> {
    let cwd = &envelope.workspace_dir;
    let session_id = &envelope.session_id;

    // Write minimal heartbeat for peer discovery
    if !session_id.is_empty() {
        let label = std::env::var("EDDA_SESSION_LABEL").unwrap_or_default();
        edda_bridge_claude::peers::write_heartbeat_minimal(project_id, session_id, &label);
    }

    // Auto-digest previous sessions
    let _ = run_auto_digest(project_id, session_id, cwd);

    Ok(ok_json())
}

// ── before_agent_start ──

/// Generate context for per-turn injection.
///
/// Architecture: body (truncatable) + tail (non-truncatable).
/// - Body: workspace context, hot pack (if available), active plan
/// - Tail: write-back protocol, coordination protocol
/// - Dedup: hash-based skip for identical consecutive injections
/// - Compact recovery: full injection when compact_pending flag is set
fn dispatch_before_agent_start(
    project_id: &str,
    envelope: &OpenClawEnvelope,
) -> anyhow::Result<HookResult> {
    let cwd = &envelope.workspace_dir;
    let session_id = &envelope.session_id;

    // Auto-digest on first turn (idempotent)
    let _digest_warning = run_auto_digest(project_id, session_id, cwd);

    // Check compact recovery flag
    let post_compact = state::take_compact_pending(project_id);

    // ── Build body (truncatable) ──
    let mut body_parts: Vec<String> = Vec::new();

    // Workspace context (decisions, notes, recent commits)
    let workspace_budget: usize = std::env::var("EDDA_WORKSPACE_BUDGET_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2500);
    if let Some(ws) = render::workspace(cwd, workspace_budget) {
        body_parts.push(ws);
    }

    // Hot pack (recent turns summary) — include on first turn or post-compact
    if post_compact {
        if let Some(pack) = render::pack(project_id) {
            body_parts.push(pack);
        }
    }

    // Active plan excerpt
    if let Some(plan) = render::plan(Some(project_id)) {
        body_parts.push(plan);
    }

    let body = body_parts.join("\n\n");

    // ── Build tail (non-truncatable) ──
    let mut tail = String::new();

    // Write-back protocol (always)
    tail.push_str("\n\n");
    tail.push_str(&render::writeback());

    // Coordination protocol
    if let Some(coord) =
        edda_bridge_claude::peers::render_coordination_protocol(project_id, session_id, cwd)
    {
        tail.push_str(&format!("\n\n{coord}"));
    }

    // ── Budget: body gets (total - tail), tail appended unconditionally ──
    let total_budget = render::context_budget(cwd);
    let body_budget = total_budget.saturating_sub(tail.len());
    let budgeted_body = render::apply_budget(&body, body_budget);

    let content = if tail.is_empty() {
        budgeted_body
    } else {
        format!("{budgeted_body}{tail}")
    };

    let wrapped = render::wrap_boundary(&content);

    // Dedup: skip if identical to last injection
    if !session_id.is_empty() && state::is_same_as_last_inject(project_id, session_id, &wrapped) {
        return Ok(HookResult::output(r#"{"ok":true}"#.to_string()));
    }
    if !session_id.is_empty() {
        state::write_inject_hash(project_id, session_id, &wrapped);
    }

    let output = serde_json::json!({ "prependContext": wrapped });
    Ok(HookResult::output(serde_json::to_string(&output)?))
}

// ── after_tool_call ──

/// Handle post-tool-call: detect decision signals and nudge.
fn dispatch_after_tool_call(
    project_id: &str,
    envelope: &OpenClawEnvelope,
) -> anyhow::Result<HookResult> {
    let session_id = &envelope.session_id;
    let event_data = &envelope.event_data;

    let tool_name = event_data
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tool_input = event_data
        .get("tool_input")
        .cloned()
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    // Normalize OpenClaw tool names to Claude bridge equivalents
    let normalized = normalize_tool_name(tool_name);

    // Build synthetic payload for nudge::detect_signal
    let raw = serde_json::json!({
        "tool_name": normalized,
        "tool_input": tool_input,
    });

    let signal = match edda_bridge_claude::nudge::detect_signal(&raw) {
        Some(s) => s,
        None => return Ok(ok_json()),
    };

    // Track signal
    state::increment_counter(project_id, session_id, "signal_count");

    // Self-record: agent called `edda decide` — just count, no nudge
    if signal == edda_bridge_claude::nudge::NudgeSignal::SelfRecord {
        state::increment_counter(project_id, session_id, "decide_count");
        return Ok(ok_json());
    }

    // Check cooldown
    if !state::should_nudge(project_id, session_id) {
        return Ok(ok_json());
    }

    // Format nudge
    let decide_count = state::read_counter(project_id, session_id, "decide_count");
    let nudge_text = edda_bridge_claude::nudge::format_nudge(&signal, decide_count);
    if nudge_text.is_empty() {
        return Ok(ok_json());
    }

    // Mark sent + count
    state::mark_nudge_sent(project_id, session_id);
    state::increment_counter(project_id, session_id, "nudge_count");

    let output = serde_json::json!({ "additionalContext": nudge_text });
    Ok(HookResult::output(serde_json::to_string(&output)?))
}

/// Map OpenClaw tool names to Claude bridge equivalents for signal detection.
fn normalize_tool_name(name: &str) -> &str {
    match name {
        "bash" | "terminal" | "shell" => "Bash",
        "edit_file" | "file_edit" => "Edit",
        "write_file" | "file_write" => "Write",
        _ => name,
    }
}

// ── before_compaction ──

/// Handle pre-compaction: set flag for full re-injection on next turn.
fn dispatch_before_compaction(project_id: &str) -> anyhow::Result<HookResult> {
    state::set_compact_pending(project_id);
    Ok(ok_json())
}

// ── message_sent ──

/// Handle agent message: detect `edda decide` calls for recall tracking.
fn dispatch_message_sent(
    project_id: &str,
    envelope: &OpenClawEnvelope,
) -> anyhow::Result<HookResult> {
    let session_id = &envelope.session_id;
    let text = envelope
        .event_data
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if text.contains("edda decide") {
        state::increment_counter(project_id, session_id, "decide_count");
    }

    Ok(ok_json())
}

// ── agent_end ──

/// Handle agent end: trigger auto-digest.
fn dispatch_agent_end(project_id: &str, envelope: &OpenClawEnvelope) -> anyhow::Result<HookResult> {
    let cwd = &envelope.workspace_dir;
    let session_id = &envelope.session_id;

    if !session_id.is_empty() {
        let _ =
            edda_bridge_claude::digest::digest_session_manual(project_id, session_id, cwd, true);
    }

    Ok(ok_json())
}

// ── session_end ──

/// Handle session end: digest, cleanup state, warn about pending tasks.
fn dispatch_session_end(
    project_id: &str,
    envelope: &OpenClawEnvelope,
) -> anyhow::Result<HookResult> {
    let cwd = &envelope.workspace_dir;
    let session_id = &envelope.session_id;

    // Auto-digest this session
    if !session_id.is_empty() {
        let _ =
            edda_bridge_claude::digest::digest_session_manual(project_id, session_id, cwd, true);
    }

    // Check if peers were active (for unclaim)
    let peers_active = !session_id.is_empty() && has_active_peers(project_id, session_id);

    // Cleanup session-scoped state files
    cleanup_session_state(project_id, session_id, peers_active);

    Ok(ok_json())
}

/// Check if any non-stale peer session heartbeats exist (excluding current).
fn has_active_peers(project_id: &str, session_id: &str) -> bool {
    let peers = edda_bridge_claude::peers::discover_active_peers(project_id, session_id);
    !peers.is_empty()
}

/// Remove session-scoped state files.
fn cleanup_session_state(project_id: &str, session_id: &str, peers_active: bool) {
    let state_dir = edda_store::project_dir(project_id).join("state");
    let _ = fs::remove_file(state_dir.join(format!("inject_hash.{session_id}")));
    let _ = fs::remove_file(state_dir.join(format!("nudge_ts.{session_id}")));
    let _ = fs::remove_file(state_dir.join(format!("nudge_count.{session_id}")));
    let _ = fs::remove_file(state_dir.join(format!("decide_count.{session_id}")));
    let _ = fs::remove_file(state_dir.join(format!("signal_count.{session_id}")));
    let _ = fs::remove_file(state_dir.join("compact_pending"));

    // Peer heartbeat + unclaim
    edda_bridge_claude::peers::remove_heartbeat(project_id, session_id);
    if peers_active {
        edda_bridge_claude::peers::write_unclaim(project_id, session_id);
    }
}

// ── Auto-digest ──

fn run_auto_digest(project_id: &str, current_session_id: &str, cwd: &str) -> Option<String> {
    let enabled = match std::env::var("EDDA_BRIDGE_AUTO_DIGEST") {
        Ok(val) => val != "0",
        Err(_) => render::config_bool(cwd, "bridge.auto_digest").unwrap_or(true),
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
        Err(_) => render::config_bool(cwd, "bridge.digest_failed_cmds").unwrap_or(true),
    };

    match edda_bridge_claude::digest::digest_previous_sessions_with_opts(
        project_id,
        current_session_id,
        cwd,
        lock_timeout_ms,
        digest_failed_cmds,
    ) {
        edda_bridge_claude::digest::DigestResult::Written { event_id } => {
            eprintln!("[edda] digested previous session -> {event_id}");
            None
        }
        edda_bridge_claude::digest::DigestResult::PermanentFailure(warning) => Some(warning),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_unknown_event_returns_ok() {
        let stdin =
            r#"{"hook_event_name":"some_future_event","session_id":"s1","workspace_dir":"."}"#;
        let result = hook_entrypoint_from_stdin(stdin).unwrap();
        assert!(result.stdout.is_some());
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(output["ok"], true);
    }

    #[test]
    fn dispatch_malformed_json_returns_ok() {
        let stdin = "this is not json";
        let result = hook_entrypoint_from_stdin(stdin).unwrap();
        assert!(result.stdout.is_some());
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(output["ok"], true);
    }

    #[test]
    fn dispatch_empty_stdin_returns_empty() {
        let result = hook_entrypoint_from_stdin("").unwrap();
        assert!(result.stdout.is_none());
        assert!(result.stderr.is_none());
    }

    #[test]
    fn dispatch_before_agent_start_returns_context() {
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");

        // Use unique session_id to avoid dedup hash conflicts across test runs
        let sid = format!("oc-test-bas-{}", std::process::id());
        let tmp = tempfile::tempdir().unwrap();
        let stdin = serde_json::json!({
            "hook_event_name": "before_agent_start",
            "session_id": sid,
            "session_key": format!("agent:main:{sid}"),
            "agent_id": "main",
            "workspace_dir": tmp.path().to_str().unwrap(),
            "event_data": { "prompt": "hello" }
        });

        let result = hook_entrypoint_from_stdin(&serde_json::to_string(&stdin).unwrap()).unwrap();
        assert!(result.stdout.is_some(), "should return context");

        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        let ctx = output["prependContext"].as_str().unwrap();
        assert!(
            ctx.contains("Write-Back Protocol"),
            "should contain write-back protocol"
        );
        assert!(
            ctx.contains("edda decide"),
            "should contain decide instruction"
        );
        assert!(
            ctx.contains(render::BOUNDARY_START),
            "should have boundary start"
        );
        assert!(
            ctx.contains(render::BOUNDARY_END),
            "should have boundary end"
        );

        std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    }

    #[test]
    fn dispatch_before_agent_start_empty_workspace() {
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");

        let tmp = tempfile::tempdir().unwrap();
        let stdin = serde_json::json!({
            "hook_event_name": "before_agent_start",
            "session_id": "oc-test-2",
            "agent_id": "main",
            "workspace_dir": tmp.path().to_str().unwrap(),
            "event_data": {}
        });

        let result = hook_entrypoint_from_stdin(&serde_json::to_string(&stdin).unwrap()).unwrap();
        assert!(
            result.stdout.is_some(),
            "should return context even without .edda/"
        );

        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        let ctx = output["prependContext"].as_str().unwrap();
        assert!(
            ctx.contains("Write-Back Protocol"),
            "write-back protocol always fires"
        );

        std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    }

    #[test]
    fn dispatch_agent_end_returns_ok() {
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");

        let stdin = serde_json::json!({
            "hook_event_name": "agent_end",
            "session_id": "oc-test-3",
            "agent_id": "main",
            "workspace_dir": ".",
            "event_data": { "success": true }
        });

        let result = hook_entrypoint_from_stdin(&serde_json::to_string(&stdin).unwrap()).unwrap();
        assert!(result.stdout.is_some());
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(output["ok"], true);

        std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    }

    #[test]
    fn context_budget_truncates() {
        let content = "x".repeat(10000);
        let result = render::apply_budget(&content, 500);
        assert!(result.len() <= 550);
        assert!(result.contains("truncated"));
    }

    #[test]
    fn context_boundary_wraps() {
        let content = "hello";
        let wrapped = render::wrap_boundary(content);
        assert!(wrapped.starts_with(render::BOUNDARY_START));
        assert!(wrapped.ends_with(render::BOUNDARY_END));
        assert!(wrapped.contains("hello"));
    }

    // ── session_start tests ──

    #[test]
    fn dispatch_session_start_returns_ok() {
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");

        let tmp = tempfile::tempdir().unwrap();
        let stdin = serde_json::json!({
            "hook_event_name": "session_start",
            "session_id": "oc-ss-1",
            "agent_id": "main",
            "workspace_dir": tmp.path().to_str().unwrap(),
            "event_data": {}
        });

        let result = hook_entrypoint_from_stdin(&serde_json::to_string(&stdin).unwrap()).unwrap();
        assert!(result.stdout.is_some());
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(output["ok"], true);

        std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    }

    #[test]
    fn dispatch_session_start_creates_heartbeat() {
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");

        let pid = "test_oc_session_start_hb";
        let sid = "oc-ss-hb-1";
        let _ = edda_store::ensure_dirs(pid);

        let tmp = tempfile::tempdir().unwrap();
        let envelope = OpenClawEnvelope {
            hook_event_name: "session_start".into(),
            session_id: sid.into(),
            session_key: "".into(),
            agent_id: "main".into(),
            workspace_dir: tmp.path().to_str().unwrap().into(),
            session_file: None,
            event_data: serde_json::json!({}),
        };

        let _ = dispatch_session_start(pid, &envelope);

        // Verify heartbeat file exists
        let hb_path = edda_store::project_dir(pid)
            .join("state")
            .join(format!("session.{sid}.json"));
        assert!(hb_path.exists(), "heartbeat file should be created");

        // Cleanup
        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    }

    // ── after_tool_call tests ──

    #[test]
    fn dispatch_after_tool_call_git_commit_returns_nudge() {
        let pid = "test_oc_atc_commit";
        let sid = "oc-atc-1";
        let _ = edda_store::ensure_dirs(pid);

        let envelope = OpenClawEnvelope {
            hook_event_name: "after_tool_call".into(),
            session_id: sid.into(),
            session_key: "".into(),
            agent_id: "main".into(),
            workspace_dir: ".".into(),
            session_file: None,
            event_data: serde_json::json!({
                "tool_name": "bash",
                "tool_input": { "command": "git commit -m \"feat: add auth\"" }
            }),
        };

        let result = dispatch_after_tool_call(pid, &envelope).unwrap();
        assert!(result.stdout.is_some());
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        let ctx = output["additionalContext"].as_str().unwrap();
        assert!(
            ctx.contains("edda decide"),
            "nudge should mention edda decide"
        );
        assert!(ctx.contains("add auth"), "nudge should mention commit msg");

        // Verify counters
        assert_eq!(state::read_counter(pid, sid, "signal_count"), 1);
        assert_eq!(state::read_counter(pid, sid, "nudge_count"), 1);

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn dispatch_after_tool_call_edda_decide_increments_count() {
        let pid = "test_oc_atc_decide";
        let sid = "oc-atc-2";
        let _ = edda_store::ensure_dirs(pid);

        let envelope = OpenClawEnvelope {
            hook_event_name: "after_tool_call".into(),
            session_id: sid.into(),
            session_key: "".into(),
            agent_id: "main".into(),
            workspace_dir: ".".into(),
            session_file: None,
            event_data: serde_json::json!({
                "tool_name": "bash",
                "tool_input": { "command": "edda decide \"db=postgres\" --reason \"test\"" }
            }),
        };

        let result = dispatch_after_tool_call(pid, &envelope).unwrap();
        // SelfRecord → ok_json, no additionalContext
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(output["ok"], true);

        assert_eq!(state::read_counter(pid, sid, "decide_count"), 1);
        assert_eq!(state::read_counter(pid, sid, "signal_count"), 1);

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn dispatch_after_tool_call_unrelated_no_signal() {
        let pid = "test_oc_atc_nosig";
        let sid = "oc-atc-3";
        let _ = edda_store::ensure_dirs(pid);

        let envelope = OpenClawEnvelope {
            hook_event_name: "after_tool_call".into(),
            session_id: sid.into(),
            session_key: "".into(),
            agent_id: "main".into(),
            workspace_dir: ".".into(),
            session_file: None,
            event_data: serde_json::json!({
                "tool_name": "bash",
                "tool_input": { "command": "cargo test" }
            }),
        };

        let result = dispatch_after_tool_call(pid, &envelope).unwrap();
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(output["ok"], true);
        assert_eq!(state::read_counter(pid, sid, "signal_count"), 0);

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn normalize_tool_name_maps_correctly() {
        assert_eq!(normalize_tool_name("bash"), "Bash");
        assert_eq!(normalize_tool_name("terminal"), "Bash");
        assert_eq!(normalize_tool_name("shell"), "Bash");
        assert_eq!(normalize_tool_name("edit_file"), "Edit");
        assert_eq!(normalize_tool_name("file_edit"), "Edit");
        assert_eq!(normalize_tool_name("write_file"), "Write");
        assert_eq!(normalize_tool_name("file_write"), "Write");
        assert_eq!(normalize_tool_name("Bash"), "Bash"); // passthrough
        assert_eq!(normalize_tool_name("unknown"), "unknown");
    }

    // ── before_compaction tests ──

    #[test]
    fn dispatch_before_compaction_sets_flag() {
        let pid = "test_oc_compact";
        let _ = edda_store::ensure_dirs(pid);

        let result = dispatch_before_compaction(pid).unwrap();
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(output["ok"], true);

        let cp_path = edda_store::project_dir(pid)
            .join("state")
            .join("compact_pending");
        assert!(cp_path.exists(), "flag should be set");
        assert!(state::take_compact_pending(pid), "take should return true");
        assert!(!cp_path.exists(), "flag should be cleared");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── message_sent tests ──

    #[test]
    fn dispatch_message_sent_detects_decide() {
        let pid = "test_oc_msgsent";
        let sid = "oc-msg-1";
        let _ = edda_store::ensure_dirs(pid);

        let envelope = OpenClawEnvelope {
            hook_event_name: "message_sent".into(),
            session_id: sid.into(),
            session_key: "".into(),
            agent_id: "main".into(),
            workspace_dir: ".".into(),
            session_file: None,
            event_data: serde_json::json!({
                "text": "I ran edda decide \"db=sqlite\" --reason \"embedded\""
            }),
        };

        let result = dispatch_message_sent(pid, &envelope).unwrap();
        let output: serde_json::Value =
            serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(output["ok"], true);
        assert_eq!(state::read_counter(pid, sid, "decide_count"), 1);

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn dispatch_message_sent_no_decide() {
        let pid = "test_oc_msgsent_nodec";
        let sid = "oc-msg-2";
        let _ = edda_store::ensure_dirs(pid);

        let envelope = OpenClawEnvelope {
            hook_event_name: "message_sent".into(),
            session_id: sid.into(),
            session_key: "".into(),
            agent_id: "main".into(),
            workspace_dir: ".".into(),
            session_file: None,
            event_data: serde_json::json!({
                "text": "I completed the implementation."
            }),
        };

        let _ = dispatch_message_sent(pid, &envelope);
        assert_eq!(state::read_counter(pid, sid, "decide_count"), 0);

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── session_end tests ──

    #[test]
    fn dispatch_session_end_cleans_state() {
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");

        let pid = "test_oc_sessend";
        let sid = "oc-end-1";
        let _ = edda_store::ensure_dirs(pid);
        let state_dir = edda_store::project_dir(pid).join("state");
        let _ = fs::create_dir_all(&state_dir);

        // Create state files that should be cleaned
        let _ = fs::write(state_dir.join(format!("inject_hash.{sid}")), "abc");
        let _ = fs::write(state_dir.join(format!("nudge_ts.{sid}")), "2026-01-01");
        let _ = fs::write(state_dir.join(format!("nudge_count.{sid}")), "3");
        let _ = fs::write(state_dir.join(format!("decide_count.{sid}")), "2");
        let _ = fs::write(state_dir.join(format!("signal_count.{sid}")), "5");
        let _ = fs::write(state_dir.join("compact_pending"), "1");

        let tmp = tempfile::tempdir().unwrap();
        let envelope = OpenClawEnvelope {
            hook_event_name: "session_end".into(),
            session_id: sid.into(),
            session_key: "".into(),
            agent_id: "main".into(),
            workspace_dir: tmp.path().to_str().unwrap().into(),
            session_file: None,
            event_data: serde_json::json!({ "success": true }),
        };

        let _ = dispatch_session_end(pid, &envelope);

        // All session state files should be gone
        assert!(!state_dir.join(format!("inject_hash.{sid}")).exists());
        assert!(!state_dir.join(format!("nudge_ts.{sid}")).exists());
        assert!(!state_dir.join(format!("nudge_count.{sid}")).exists());
        assert!(!state_dir.join(format!("decide_count.{sid}")).exists());
        assert!(!state_dir.join(format!("signal_count.{sid}")).exists());
        assert!(!state_dir.join("compact_pending").exists());

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    }

    // ── Dedup tests ──

    #[test]
    fn dedup_hash_skips_identical() {
        let pid = "test_oc_dedup";
        let sid = "oc-dedup-1";
        let _ = edda_store::ensure_dirs(pid);

        let content = "test content for dedup";
        assert!(!state::is_same_as_last_inject(pid, sid, content));
        state::write_inject_hash(pid, sid, content);
        assert!(state::is_same_as_last_inject(pid, sid, content));
        assert!(!state::is_same_as_last_inject(
            pid,
            sid,
            "different content"
        ));

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── P2 stub tests ──

    #[test]
    fn dispatch_p2_stubs_return_ok() {
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");

        for event_name in &["before_tool_call", "after_compaction", "message_received"] {
            let tmp = tempfile::tempdir().unwrap();
            let stdin = serde_json::json!({
                "hook_event_name": event_name,
                "session_id": "oc-stub-1",
                "workspace_dir": tmp.path().to_str().unwrap(),
                "event_data": {}
            });

            let result =
                hook_entrypoint_from_stdin(&serde_json::to_string(&stdin).unwrap()).unwrap();
            assert!(result.stdout.is_some(), "{event_name} should return stdout");
            let output: serde_json::Value =
                serde_json::from_str(result.stdout.as_ref().unwrap()).unwrap();
            assert_eq!(output["ok"], true, "{event_name} should return ok");
        }

        std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    }

    // ── Integration: full session lifecycle ──

    #[test]
    fn lifecycle_session_start_to_end() {
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");

        let pid = "test_oc_lifecycle";
        let sid = "oc-life-1";
        let _ = edda_store::ensure_dirs(pid);

        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_str().unwrap();

        // 1. session_start → heartbeat created
        let env1 = OpenClawEnvelope {
            hook_event_name: "session_start".into(),
            session_id: sid.into(),
            session_key: "".into(),
            agent_id: "main".into(),
            workspace_dir: cwd.into(),
            session_file: None,
            event_data: serde_json::json!({}),
        };
        let _ = dispatch_session_start(pid, &env1);
        let hb_path = edda_store::project_dir(pid)
            .join("state")
            .join(format!("session.{sid}.json"));
        assert!(
            hb_path.exists(),
            "heartbeat should exist after session_start"
        );

        // 2. before_agent_start → context with write-back protocol
        let env2 = OpenClawEnvelope {
            hook_event_name: "before_agent_start".into(),
            session_id: sid.into(),
            session_key: "".into(),
            agent_id: "main".into(),
            workspace_dir: cwd.into(),
            session_file: None,
            event_data: serde_json::json!({ "prompt": "hello" }),
        };
        let r2 = dispatch_before_agent_start(pid, &env2).unwrap();
        assert!(r2.stdout.is_some());
        let o2: serde_json::Value = serde_json::from_str(r2.stdout.as_ref().unwrap()).unwrap();
        let ctx = o2["prependContext"].as_str().unwrap();
        assert!(ctx.contains("Write-Back Protocol"));

        // 3. after_tool_call (git commit) → nudge
        let env3 = OpenClawEnvelope {
            hook_event_name: "after_tool_call".into(),
            session_id: sid.into(),
            session_key: "".into(),
            agent_id: "main".into(),
            workspace_dir: cwd.into(),
            session_file: None,
            event_data: serde_json::json!({
                "tool_name": "bash",
                "tool_input": { "command": "git commit -m \"feat: lifecycle test\"" }
            }),
        };
        let r3 = dispatch_after_tool_call(pid, &env3).unwrap();
        let o3: serde_json::Value = serde_json::from_str(r3.stdout.as_ref().unwrap()).unwrap();
        assert!(o3["additionalContext"].as_str().is_some(), "should nudge");

        // 4. message_sent (edda decide) → decide_count
        let env4 = OpenClawEnvelope {
            hook_event_name: "message_sent".into(),
            session_id: sid.into(),
            session_key: "".into(),
            agent_id: "main".into(),
            workspace_dir: cwd.into(),
            session_file: None,
            event_data: serde_json::json!({
                "text": "edda decide \"test=value\" --reason \"test\""
            }),
        };
        let _ = dispatch_message_sent(pid, &env4);
        assert_eq!(state::read_counter(pid, sid, "decide_count"), 1);

        // 5. before_compaction → flag set
        let _ = dispatch_before_compaction(pid);
        let cp_path = edda_store::project_dir(pid)
            .join("state")
            .join("compact_pending");
        assert!(cp_path.exists());

        // 6. session_end → state cleaned
        let env6 = OpenClawEnvelope {
            hook_event_name: "session_end".into(),
            session_id: sid.into(),
            session_key: "".into(),
            agent_id: "main".into(),
            workspace_dir: cwd.into(),
            session_file: None,
            event_data: serde_json::json!({ "success": true }),
        };
        let _ = dispatch_session_end(pid, &env6);

        // Verify cleanup
        let state_dir = edda_store::project_dir(pid).join("state");
        assert!(!state_dir.join(format!("inject_hash.{sid}")).exists());
        assert!(!state_dir.join(format!("decide_count.{sid}")).exists());
        assert!(!state_dir.join("compact_pending").exists());
        assert!(!hb_path.exists(), "heartbeat removed after session_end");

        let _ = fs::remove_dir_all(edda_store::project_dir(pid));
        std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    }
}
