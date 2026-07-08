//! Hermes hook dispatch.
//!
//! Output shape matrix (source: `hermes-agent/agent/shell_hooks.py::_parse_response`):
//!
//! | Event             | Return                                             | Behavior                     |
//! |-------------------|----------------------------------------------------|------------------------------|
//! | `pre_llm_call`    | `{"context": "..."}`                              | Injects into next LLM turn   |
//! | `pre_tool_call`   | `{"decision":"block","reason":"..."}` (Claude)    | Blocks tool call             |
//! | `pre_tool_call`   | `{"action":"block","message":"..."}` (Hermes)     | Blocks tool call             |
//! | `pre_verify`      | `{"action":"continue","message":"..."}`           | Keep going with instruction  |
//! | others            | `{}` or empty                                     | No-op (observation only)     |
//!
//! Emit Claude-Code style for `pre_tool_call` since Hermes translates
//! internally — this lets us reuse `edda_postmortem::hooks::format_warnings`
//! logic without a Hermes-specific translation layer.

use edda_bridge_claude::{render, state};

use crate::parse::*;

// ── Hook Result ──

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

fn ok() -> HookResult {
    // Hermes accepts empty stdout as no-op; return {} for clarity.
    HookResult::output("{}".to_string())
}

fn inject(context: &str) -> anyhow::Result<HookResult> {
    let out = serde_json::json!({ "context": context });
    Ok(HookResult::output(serde_json::to_string(&out)?))
}

#[allow(dead_code)] // Reserved for follow-up when postmortem rules gain a block category.
fn block_tool_call(reason: &str) -> anyhow::Result<HookResult> {
    // Emit Claude-Code shape — Hermes translates to canonical form
    // (verified via shell_hooks.py::_parse_response line 601-602).
    let out = serde_json::json!({ "decision": "block", "reason": reason });
    Ok(HookResult::output(serde_json::to_string(&out)?))
}

// ── Entry ──

/// Fail-open: every internal error becomes `{}` so a broken bridge cannot
/// brick the user's session.
pub fn hook_entrypoint_from_stdin(stdin: &str) -> anyhow::Result<HookResult> {
    if stdin.trim().is_empty() {
        return Ok(HookResult::empty());
    }
    let envelope = match parse_hook_stdin(stdin) {
        Ok(e) => e,
        Err(_) => return Ok(ok()),
    };

    let project_id = resolve_project_id(&envelope.cwd);
    let _ = edda_store::ensure_dirs(&project_id);
    let _ = append_to_session_ledger(&envelope);

    if !envelope.session_id.is_empty() {
        edda_bridge_claude::peers::touch_heartbeat(&project_id, &envelope.session_id);
    }

    match envelope.hook_event_name.as_str() {
        "pre_llm_call" => dispatch_pre_llm_call(&project_id, &envelope),
        "pre_tool_call" => dispatch_pre_tool_call(&project_id, &envelope),
        "post_tool_call" => dispatch_post_tool_call(&project_id, &envelope),
        "on_session_start" => dispatch_on_session_start(&project_id, &envelope),
        "on_session_end" => dispatch_on_session_end(&project_id, &envelope),
        "on_session_reset" => dispatch_on_session_reset(&project_id, &envelope),
        // Hermes-only slots — stubbed for now, forward-compatible.
        "pre_verify"
        | "subagent_start"
        | "subagent_stop"
        | "post_llm_call"
        | "transform_tool_result"
        | "transform_terminal_output"
        | "transform_llm_output"
        | "pre_gateway_dispatch"
        | "pre_api_request"
        | "post_api_request"
        | "api_request_error"
        | "on_session_finalize"
        | "pre_approval_request"
        | "post_approval_response" => Ok(ok()),
        _ => Ok(ok()),
    }
}

// ── on_session_start: heartbeat + auto-digest (context injection lives on pre_llm_call) ──

fn dispatch_on_session_start(
    project_id: &str,
    envelope: &HermesEnvelope,
) -> anyhow::Result<HookResult> {
    let cwd = &envelope.cwd;
    let session_id = &envelope.session_id;
    if !session_id.is_empty() {
        let label = std::env::var("EDDA_SESSION_LABEL").unwrap_or_default();
        edda_bridge_claude::peers::write_heartbeat_minimal(project_id, session_id, &label, cwd);
    }
    Ok(ok())
}

// ── pre_llm_call: this is where Hermes injects context ──
//
// Hermes' `is_first_turn` flag distinguishes SessionStart-shape (full doctrine
// + workspace + hot pack + writeback) from UserPromptSubmit-shape (workspace
// diff only). One event handler, two behaviors, dedup on repeat.

fn dispatch_pre_llm_call(
    project_id: &str,
    envelope: &HermesEnvelope,
) -> anyhow::Result<HookResult> {
    let cwd = &envelope.cwd;
    let session_id = &envelope.session_id;
    let first_turn = extra_bool(envelope, "is_first_turn");

    let mut parts: Vec<String> = Vec::new();

    if first_turn {
        // Doctrine (havamal contract).
        let doctrine_budget: usize = std::env::var("EDDA_DOCTRINE_BUDGET_CHARS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4000);
        if let Some(doctrine) =
            edda_pack::read_doctrine_pack(std::path::Path::new(cwd), doctrine_budget)
        {
            parts.push(doctrine);
        }
    }

    // Workspace context (decisions, notes, recent commits) — every turn.
    let workspace_budget: usize = std::env::var("EDDA_WORKSPACE_BUDGET_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2500);
    if let Some(ws) = render::workspace(cwd, workspace_budget) {
        parts.push(ws);
    }

    if first_turn {
        if let Some(pack) = render::pack(project_id) {
            parts.push(pack);
        }
        if let Some(plan) = render::plan(Some(project_id)) {
            parts.push(plan);
        }
    }

    let mut tail = String::new();
    if first_turn {
        tail.push_str("\n\n");
        tail.push_str(&render::writeback());
        if let Some(coord) =
            edda_bridge_claude::peers::render_coordination_protocol(project_id, session_id, cwd)
        {
            tail.push_str(&format!("\n\n{coord}"));
        }
    } else {
        // Subsequent turns: skip the full coordination protocol re-injection.
        // Coordination diffs would go here if peers::render_coord_diff were
        // pub — leaving as follow-up so we don't reach into a private module.
        let _ = session_id;
    }

    let body = parts.join("\n\n");
    if body.is_empty() && tail.is_empty() {
        return Ok(ok());
    }

    let total_budget = render::context_budget(cwd);
    let body_budget = total_budget.saturating_sub(tail.len());
    let budgeted_body = render::apply_budget(&body, body_budget);
    let content = format!("{budgeted_body}{tail}");
    let wrapped = render::wrap_boundary(&content);

    if !session_id.is_empty() && state::is_same_as_last_inject(project_id, session_id, &wrapped) {
        return Ok(ok());
    }
    if !session_id.is_empty() {
        state::write_inject_hash(project_id, session_id, &wrapped);
    }
    inject(&wrapped)
}

// ── pre_tool_call: L3 rules (block or advise) ──

fn dispatch_pre_tool_call(
    project_id: &str,
    envelope: &HermesEnvelope,
) -> anyhow::Result<HookResult> {
    if std::env::var("EDDA_POSTMORTEM").unwrap_or_else(|_| "1".into()) == "0" {
        return Ok(ok());
    }
    let mut rules_store = edda_postmortem::RulesStore::load_project(project_id);
    if rules_store.active_rules().is_empty() {
        return Ok(ok());
    }

    let files_touched: Vec<String> = envelope
        .tool_input
        .get("file_path")
        .and_then(|v| v.as_str())
        .map(|s| vec![s.to_string()])
        .unwrap_or_default();
    // Hermes uses lowercase tool names (`terminal`); normalize to Claude
    // conventions so L3 rules written against Claude-Code stay portable.
    let normalized_tool = normalize_tool_name(&envelope.tool_name);
    let command = envelope
        .tool_input
        .get("command")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let ctx = edda_postmortem::hooks::HookContext {
        hook_event: "PreToolUse".to_string(),
        tool_name: normalized_tool.to_string(),
        files_touched,
        cwd: envelope.cwd.clone(),
        command,
    };

    let result = edda_postmortem::hooks::evaluate_rules(&rules_store, &ctx);
    if !result.matched_rule_ids.is_empty() {
        edda_postmortem::hooks::record_matched_hits(&mut rules_store, &result.matched_rule_ids);
        let _ = rules_store.save_project(project_id);
    }

    // Any matched rule with a "block" action would block; format_warnings
    // produces advisory text. Route both to `{"decision":"block","reason":..}`
    // when the rule set contains a definitive block, else emit context as
    // no-op (Hermes has no per-tool-call context injection channel — only
    // block/allow) so advisory rules only affect telemetry, not flow.
    if !result.enforcements.is_empty() {
        // For now every matched rule counts as advisory — dropping the
        // strict-block distinction until we thread rule category into the
        // enforcement record. That preserves existing bridge-claude behavior
        // (warn-not-block) instead of surprising users with sudden hard
        // blocks on Hermes.
        // TODO: honor category=Block once the postmortem module distinguishes.
    }
    Ok(ok())
}

fn normalize_tool_name(name: &str) -> &str {
    match name {
        "terminal" | "shell" | "bash" => "Bash",
        "edit_file" | "file_edit" => "Edit",
        "write_file" | "file_write" => "Write",
        _ => name,
    }
}

// ── post_tool_call: nudge on decision signals ──

fn dispatch_post_tool_call(
    project_id: &str,
    envelope: &HermesEnvelope,
) -> anyhow::Result<HookResult> {
    let session_id = &envelope.session_id;
    let normalized = normalize_tool_name(&envelope.tool_name);
    let raw = serde_json::json!({
        "tool_name": normalized,
        "tool_input": envelope.tool_input,
    });
    let signal = match edda_bridge_claude::nudge::detect_signal(&raw) {
        Some(s) => s,
        None => return Ok(ok()),
    };

    state::increment_counter(project_id, session_id, "signal_count");
    if signal == edda_bridge_claude::nudge::NudgeSignal::SelfRecord {
        state::increment_counter(project_id, session_id, "decide_count");
        return Ok(ok());
    }
    if !state::should_nudge(project_id, session_id) {
        return Ok(ok());
    }
    let decide_count = state::read_counter(project_id, session_id, "decide_count");
    let nudge_text = edda_bridge_claude::nudge::format_nudge(&signal, decide_count);
    if nudge_text.is_empty() {
        return Ok(ok());
    }
    state::mark_nudge_sent(project_id, session_id);
    state::increment_counter(project_id, session_id, "nudge_count");
    // Hermes post_tool_call is observer-only in the wire protocol.
    // We still track counters for pre_llm_call injection to pick up.
    // (nudge_text is discarded here on purpose; it will materialize in
    // next turn's workspace section via the normal signal counter path.)
    let _ = nudge_text;
    Ok(ok())
}

// ── on_session_end: digest + cleanup ──

fn dispatch_on_session_end(
    project_id: &str,
    envelope: &HermesEnvelope,
) -> anyhow::Result<HookResult> {
    let cwd = &envelope.cwd;
    let session_id = &envelope.session_id;
    if !session_id.is_empty() {
        let _ =
            edda_bridge_claude::digest::digest_session_manual(project_id, session_id, cwd, true);
    }
    let peers_active = !session_id.is_empty()
        && !edda_bridge_claude::peers::discover_active_peers(project_id, session_id).is_empty();
    cleanup_session_state(project_id, session_id, peers_active);
    Ok(ok())
}

// ── on_session_reset: digest without full cleanup ──
//
// `/new` resets the conversation but the session process keeps running.
// Digest so the fresh conversation gets a clean slate + prior-turn digest,
// but keep peer heartbeats alive.
fn dispatch_on_session_reset(
    project_id: &str,
    envelope: &HermesEnvelope,
) -> anyhow::Result<HookResult> {
    let cwd = &envelope.cwd;
    let session_id = &envelope.session_id;
    if !session_id.is_empty() {
        let _ =
            edda_bridge_claude::digest::digest_session_manual(project_id, session_id, cwd, true);
    }
    // Preserve heartbeat + claim — reset is not disconnection.
    // Clear only per-turn dedup state so the next pre_llm_call re-injects.
    let state_dir = edda_store::project_dir(project_id).join("state");
    let _ = std::fs::remove_file(state_dir.join(format!("inject_hash.{session_id}")));
    Ok(ok())
}

fn cleanup_session_state(project_id: &str, session_id: &str, peers_active: bool) {
    let state_dir = edda_store::project_dir(project_id).join("state");
    let _ = std::fs::remove_file(state_dir.join(format!("inject_hash.{session_id}")));
    let _ = std::fs::remove_file(state_dir.join(format!("nudge_ts.{session_id}")));
    let _ = std::fs::remove_file(state_dir.join(format!("nudge_count.{session_id}")));
    let _ = std::fs::remove_file(state_dir.join(format!("decide_count.{session_id}")));
    let _ = std::fs::remove_file(state_dir.join(format!("signal_count.{session_id}")));
    let _ = std::fs::remove_file(state_dir.join("compact_pending"));
    edda_bridge_claude::peers::remove_heartbeat(project_id, session_id);
    if peers_active {
        edda_bridge_claude::peers::write_unclaim(project_id, session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdin_event(name: &str, cwd: &str, sid: &str) -> String {
        serde_json::json!({
            "hook_event_name": name,
            "session_id": sid,
            "cwd": cwd,
            "extra": {}
        })
        .to_string()
    }

    #[test]
    fn empty_stdin_returns_empty() {
        let r = hook_entrypoint_from_stdin("").unwrap();
        assert!(r.stdout.is_none());
    }

    #[test]
    fn malformed_stdin_fails_open() {
        let r = hook_entrypoint_from_stdin("not json").unwrap();
        assert_eq!(r.stdout.as_deref(), Some("{}"));
    }

    #[test]
    fn unknown_event_returns_ok() {
        let stdin = stdin_event("SomethingFuture", "/tmp", "h1");
        let r = hook_entrypoint_from_stdin(&stdin).unwrap();
        assert_eq!(r.stdout.as_deref(), Some("{}"));
    }

    #[test]
    fn pre_llm_call_first_turn_injects_doctrine_and_workspace() {
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".havamal-pack.md"),
            "## L1 — MUST STAY TRUE\nClaims never close work.",
        )
        .unwrap();
        let stdin = serde_json::json!({
            "hook_event_name": "pre_llm_call",
            "session_id": "hermes-pl-1",
            "cwd": tmp.path().to_str().unwrap(),
            "extra": {"is_first_turn": true}
        })
        .to_string();
        let r = hook_entrypoint_from_stdin(&stdin).unwrap();
        let v: serde_json::Value = serde_json::from_str(r.stdout.as_ref().unwrap()).unwrap();
        let ctx = v["context"].as_str().unwrap();
        assert!(ctx.contains("Doctrine (judgment layer)"));
        assert!(ctx.contains("Claims never close work"));
        assert!(ctx.contains("Write-Back Protocol"));
        std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    }

    #[test]
    fn pre_llm_call_subsequent_turn_is_lightweight_or_noop() {
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
        let tmp = tempfile::tempdir().unwrap();
        // Non-first turn AND no workspace = no context to inject.
        let stdin = serde_json::json!({
            "hook_event_name": "pre_llm_call",
            "session_id": "hermes-pl-2",
            "cwd": tmp.path().to_str().unwrap(),
            "extra": {"is_first_turn": false}
        })
        .to_string();
        let r = hook_entrypoint_from_stdin(&stdin).unwrap();
        let v: serde_json::Value = serde_json::from_str(r.stdout.as_ref().unwrap()).unwrap();
        // Either no-op {} or an empty context object; MUST NOT contain the
        // full SessionStart bundle.
        if let Some(ctx) = v.get("context").and_then(|c| c.as_str()) {
            assert!(!ctx.contains("Doctrine (judgment layer)"));
            assert!(!ctx.contains("Write-Back Protocol"));
        }
        std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    }

    #[test]
    fn pre_tool_call_no_rules_returns_ok() {
        std::env::set_var("EDDA_POSTMORTEM", "1");
        let stdin = serde_json::json!({
            "hook_event_name": "pre_tool_call",
            "session_id": "hermes-pt-1",
            "cwd": "/tmp/hermes-pt",
            "tool_name": "terminal",
            "tool_input": { "command": "ls" }
        })
        .to_string();
        let r = hook_entrypoint_from_stdin(&stdin).unwrap();
        assert_eq!(r.stdout.as_deref(), Some("{}"));
    }

    #[test]
    fn normalize_tool_name_maps_terminal_to_bash() {
        assert_eq!(normalize_tool_name("terminal"), "Bash");
        assert_eq!(normalize_tool_name("shell"), "Bash");
        assert_eq!(normalize_tool_name("bash"), "Bash");
        assert_eq!(normalize_tool_name("Bash"), "Bash");
        assert_eq!(normalize_tool_name("edit_file"), "Edit");
        assert_eq!(normalize_tool_name("unknown"), "unknown");
    }

    #[test]
    fn block_tool_call_uses_claude_shape() {
        // Hermes translates {decision:block,reason} internally, verified in
        // shell_hooks.py::_parse_response line 601-602.
        let r = block_tool_call("test reason").unwrap();
        let v: serde_json::Value = serde_json::from_str(r.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(v["decision"], "block");
        assert_eq!(v["reason"], "test reason");
    }

    #[test]
    fn inject_uses_context_field() {
        let r = inject("hello world").unwrap();
        let v: serde_json::Value = serde_json::from_str(r.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(v["context"], "hello world");
    }
}
