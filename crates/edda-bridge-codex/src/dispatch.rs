//! Codex hook event dispatch.
//!
//! Output shape (Codex protocol):
//! ```json
//! {
//!   "continue": true,
//!   "hookSpecificOutput": { "additionalContext": "..." }
//! }
//! ```
//!
//! For PreToolUse we can also return `{ "continue": false, "stopReason": "..." }`
//! to block, but this skeleton emits advisory context only — L3 rule enforcement
//! is wired to the same `edda_postmortem::hooks::evaluate_rules` used by the
//! Claude bridge, but delivered as `additionalContext` warnings for now.

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
    HookResult::output(r#"{"continue":true}"#.to_string())
}

fn with_context(context: &str) -> anyhow::Result<HookResult> {
    let output = serde_json::json!({
        "continue": true,
        "hookSpecificOutput": { "additionalContext": context }
    });
    Ok(HookResult::output(serde_json::to_string(&output)?))
}

// ── Entry ──

/// Fail-open: every internal error becomes `{"continue":true}` so a broken
/// bridge cannot brick the user's session.
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
        "SessionStart" => dispatch_session_start(&project_id, &envelope),
        "UserPromptSubmit" => dispatch_user_prompt_submit(&project_id, &envelope),
        "PreToolUse" => dispatch_pre_tool_use(&project_id, &envelope),
        "PostToolUse" => dispatch_post_tool_use(&project_id, &envelope),
        "PreCompact" => dispatch_pre_compact(&project_id),
        "SessionEnd" | "Stop" => dispatch_session_end(&project_id, &envelope),
        // Codex-only events — forward-compatible stubs
        "SubagentStart" | "SubagentStop" | "PostCompact" | "PermissionRequest"
        | "PostToolUseFailure" => Ok(ok()),
        _ => Ok(ok()),
    }
}

// ── SessionStart: full doctrine + workspace injection ──

fn dispatch_session_start(
    project_id: &str,
    envelope: &CodexEnvelope,
) -> anyhow::Result<HookResult> {
    let cwd = &envelope.cwd;
    let session_id = &envelope.session_id;

    if !session_id.is_empty() {
        let label = std::env::var("EDDA_SESSION_LABEL").unwrap_or_default();
        edda_bridge_claude::peers::write_heartbeat_minimal(project_id, session_id, &label, cwd);
    }

    let mut body_parts: Vec<String> = Vec::new();

    // 1. Doctrine pack (havamal contract) — placed first, budget-capped.
    let doctrine_budget: usize = std::env::var("EDDA_DOCTRINE_BUDGET_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4000);
    if let Some(doctrine) =
        edda_pack::read_doctrine_pack(std::path::Path::new(cwd), doctrine_budget)
    {
        body_parts.push(doctrine);
    }

    // 2. Workspace context (decisions, notes, recent commits).
    let workspace_budget: usize = std::env::var("EDDA_WORKSPACE_BUDGET_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2500);
    if let Some(ws) = render::workspace(cwd, workspace_budget) {
        body_parts.push(ws);
    }

    // 3. Hot pack (recent turns from prior sessions).
    if let Some(pack) = render::pack(project_id) {
        body_parts.push(pack);
    }

    // 4. Active plan excerpt.
    if let Some(plan) = render::plan(Some(project_id)) {
        body_parts.push(plan);
    }

    let body = body_parts.join("\n\n");

    // Tail: write-back protocol + coordination.
    let mut tail = String::new();
    tail.push_str("\n\n");
    tail.push_str(&render::writeback());
    if let Some(coord) =
        edda_bridge_claude::peers::render_coordination_protocol(project_id, session_id, cwd)
    {
        tail.push_str(&format!("\n\n{coord}"));
    }

    let total_budget = render::context_budget(cwd);
    let body_budget = total_budget.saturating_sub(tail.len());
    let budgeted_body = render::apply_budget(&body, body_budget);
    let content = format!("{budgeted_body}{tail}");
    let wrapped = render::wrap_boundary(&content);
    with_context(&wrapped)
}

// ── UserPromptSubmit: light workspace-only injection with dedup ──

fn dispatch_user_prompt_submit(
    project_id: &str,
    envelope: &CodexEnvelope,
) -> anyhow::Result<HookResult> {
    let cwd = &envelope.cwd;
    let session_id = &envelope.session_id;

    let workspace_budget: usize = std::env::var("EDDA_WORKSPACE_BUDGET_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2500);
    let ws = match render::workspace(cwd, workspace_budget) {
        Some(w) => w,
        None => return Ok(ok()),
    };
    let wrapped = render::wrap_boundary(&ws);

    if !session_id.is_empty() && state::is_same_as_last_inject(project_id, session_id, &wrapped) {
        return Ok(ok());
    }
    if !session_id.is_empty() {
        state::write_inject_hash(project_id, session_id, &wrapped);
    }
    with_context(&wrapped)
}

// ── PreToolUse: L3 rule evaluation ──

fn dispatch_pre_tool_use(project_id: &str, envelope: &CodexEnvelope) -> anyhow::Result<HookResult> {
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
    let command = envelope
        .tool_input
        .get("command")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let ctx = edda_postmortem::hooks::HookContext {
        hook_event: "PreToolUse".to_string(),
        tool_name: envelope.tool_name.clone(),
        files_touched,
        cwd: envelope.cwd.clone(),
        command,
    };

    let result = edda_postmortem::hooks::evaluate_rules(&rules_store, &ctx);
    if !result.matched_rule_ids.is_empty() {
        edda_postmortem::hooks::record_matched_hits(&mut rules_store, &result.matched_rule_ids);
        let _ = rules_store.save_project(project_id);
    }
    match edda_postmortem::hooks::format_warnings(&result) {
        Some(warning) => with_context(&warning),
        None => Ok(ok()),
    }
}

// ── PostToolUse: nudge on decision signals ──

fn dispatch_post_tool_use(
    project_id: &str,
    envelope: &CodexEnvelope,
) -> anyhow::Result<HookResult> {
    let session_id = &envelope.session_id;
    let raw = serde_json::json!({
        "tool_name": envelope.tool_name,
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
    with_context(&nudge_text)
}

// ── PreCompact / SessionEnd ──

fn dispatch_pre_compact(project_id: &str) -> anyhow::Result<HookResult> {
    state::set_compact_pending(project_id);
    Ok(ok())
}

fn dispatch_session_end(project_id: &str, envelope: &CodexEnvelope) -> anyhow::Result<HookResult> {
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
            "model": "gpt-5-codex",
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
        let v: serde_json::Value = serde_json::from_str(r.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(v["continue"], true);
    }

    #[test]
    fn unknown_event_returns_ok() {
        let stdin = stdin_event("SomethingFuture", "/tmp", "s1");
        let r = hook_entrypoint_from_stdin(&stdin).unwrap();
        let v: serde_json::Value = serde_json::from_str(r.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(v["continue"], true);
    }

    #[test]
    fn session_start_injects_context() {
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
        let tmp = tempfile::tempdir().unwrap();
        let stdin = stdin_event("SessionStart", tmp.path().to_str().unwrap(), "codex-ss-1");
        let r = hook_entrypoint_from_stdin(&stdin).unwrap();
        let v: serde_json::Value = serde_json::from_str(r.stdout.as_ref().unwrap()).unwrap();
        let ctx = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(ctx.contains("Write-Back Protocol"));
        assert!(ctx.contains("edda decide"));
        std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    }

    #[test]
    fn session_start_reads_doctrine_pack() {
        std::env::set_var("EDDA_BRIDGE_AUTO_DIGEST", "0");
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".havamal-pack.md"),
            "## L1 — MUST STAY TRUE\nClaims never close work.",
        )
        .unwrap();
        let stdin = stdin_event("SessionStart", tmp.path().to_str().unwrap(), "codex-ss-2");
        let r = hook_entrypoint_from_stdin(&stdin).unwrap();
        let v: serde_json::Value = serde_json::from_str(r.stdout.as_ref().unwrap()).unwrap();
        let ctx = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(ctx.contains("Doctrine (judgment layer)"));
        assert!(ctx.contains("Claims never close work"));
        std::env::remove_var("EDDA_BRIDGE_AUTO_DIGEST");
    }

    #[test]
    fn pre_tool_use_no_rules_returns_ok() {
        std::env::set_var("EDDA_POSTMORTEM", "1");
        let stdin = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "session_id": "codex-ptu-1",
            "cwd": "/tmp/codex-ptu",
            "tool_name": "Bash",
            "tool_input": { "command": "ls" }
        })
        .to_string();
        let r = hook_entrypoint_from_stdin(&stdin).unwrap();
        let v: serde_json::Value = serde_json::from_str(r.stdout.as_ref().unwrap()).unwrap();
        assert_eq!(v["continue"], true);
    }
}
