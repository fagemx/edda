use std::fs;
use std::path::Path;

use globset::Glob;

use crate::parse::*;

use super::events::{
    is_karvi_project, try_post_karvi_signal, try_write_commit_event, try_write_merge_event,
};
use super::{
    increment_counter, mark_nudge_sent, read_counter, read_peer_count, read_workspace_config_bool,
    should_nudge, wrap_context_boundary, HookResult,
};

pub(super) fn dispatch_pre_tool_use(
    raw: &serde_json::Value,
    cwd: &str,
    project_id: &str,
    session_id: &str,
) -> anyhow::Result<HookResult> {
    // ── Branch guard: block git commit on wrong branch ──
    if std::env::var("EDDA_BRANCH_GUARD").unwrap_or_else(|_| "1".into()) != "0" {
        let tool_name = get_str(raw, "tool_name");
        if tool_name == "Bash" {
            let command = raw
                .pointer("/tool_input/command")
                .or_else(|| raw.pointer("/input/command"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            use std::sync::LazyLock;
            static RE_GIT_COMMIT: LazyLock<regex::Regex> =
                LazyLock::new(|| regex::Regex::new(r"\bgit\s+commit\b").expect("static regex"));
            if RE_GIT_COMMIT.is_match(command) && !command.contains("--amend") {
                if let Some(hb) = crate::peers::read_heartbeat(project_id, session_id) {
                    if let Some(claimed) = &hb.branch {
                        if let Some(actual) = crate::peers::detect_git_branch_in(cwd) {
                            if claimed != &actual {
                                let reason = format!(
                                    "Branch mismatch: session claimed '{}' but current branch is '{}'. Run: git checkout {}",
                                    claimed, actual, claimed
                                );
                                let output = serde_json::json!({
                                    "hookSpecificOutput": {
                                        "hookEventName": "PreToolUse",
                                        "permissionDecision": "block",
                                        "permissionDecisionReason": reason
                                    }
                                });
                                return Ok(HookResult::output(serde_json::to_string(&output)?));
                            }
                        }
                    }
                }
            }
        }
    }

    // ── Off-limits enforcement: block Edit/Write on peer-claimed files ──
    let enforce_offlimits = match std::env::var("EDDA_ENFORCE_OFFLIMITS") {
        Ok(val) => val == "1",
        Err(_) => read_workspace_config_bool(cwd, "bridge.enforce_offlimits").unwrap_or(false),
    };
    if enforce_offlimits {
        let tool_name_ol = get_str(raw, "tool_name");
        if tool_name_ol == "Edit" || tool_name_ol == "Write" {
            let file_path = raw
                .pointer("/tool_input/file_path")
                .or_else(|| raw.pointer("/input/file_path"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !file_path.is_empty() {
                if let Some((peer_label, matched_glob)) =
                    check_offlimits(project_id, session_id, file_path)
                {
                    let reason = format!(
                        "Off-limits: file '{}' is claimed by agent '{}' (paths: {}). \
                         Use `edda request \"{}\" \"need to edit {}\"` to coordinate.",
                        file_path, peer_label, matched_glob, peer_label, file_path
                    );
                    let output = serde_json::json!({
                        "hookSpecificOutput": {
                            "hookEventName": "PreToolUse",
                            "permissionDecision": "block",
                            "permissionDecisionReason": reason
                        }
                    });
                    return Ok(HookResult::output(serde_json::to_string(&output)?));
                }
            }
        }
    }

    let auto_approve = std::env::var("EDDA_CLAUDE_AUTO_APPROVE").unwrap_or_else(|_| "1".into());

    // Pattern matching (only for Edit/Write)
    let pattern_ctx = match_tool_patterns(raw, cwd);

    // Check for pending requests (with cooldown: once per 3 tool calls)
    let request_nudge = check_pending_requests(project_id, session_id);

    // L3: evaluate learned rules (warn-only, via additionalContext)
    let rules_warning = evaluate_learned_rules(raw, project_id, cwd);

    // Combine pattern context, request nudge, and rules warning
    let combined_ctx = [pattern_ctx, request_nudge, rules_warning]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let combined_ctx = if combined_ctx.is_empty() {
        None
    } else {
        Some(combined_ctx.join("\n\n"))
    };

    if auto_approve == "1" {
        let mut hook_output = serde_json::json!({
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "edda auto-approved (M8)"
        });
        if let Some(ctx) = combined_ctx {
            hook_output["additionalContext"] =
                serde_json::Value::String(wrap_context_boundary(&ctx));
        }
        let output = serde_json::json!({ "hookSpecificOutput": hook_output });
        Ok(HookResult::output(serde_json::to_string(&output)?))
    } else if let Some(ctx) = combined_ctx {
        let output = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "additionalContext": wrap_context_boundary(&ctx)
            }
        });
        Ok(HookResult::output(serde_json::to_string(&output)?))
    } else {
        Ok(HookResult::empty())
    }
}

/// Check for pending coordination requests addressed to this session.
/// Uses a cooldown counter: only returns a nudge every 3rd PreToolUse call.
/// Skips all I/O for solo sessions (no peers).
pub(super) fn check_pending_requests(project_id: &str, session_id: &str) -> Option<String> {
    // Solo gate: skip counter I/O entirely when no peers are active.
    if read_peer_count(project_id, session_id) == 0 {
        return None;
    }

    let counter = read_counter(project_id, session_id, "request_nudge_count");
    increment_counter(project_id, session_id, "request_nudge_count");
    if !counter.is_multiple_of(3) {
        return None;
    }

    let pending = crate::peers::pending_requests_for_session(project_id, session_id);
    if pending.is_empty() {
        return None;
    }

    let mut lines = vec!["**Pending requests** (ack with `edda request-ack <from>`):".to_string()];
    for r in &pending {
        lines.push(format!("  - From **{}**: {}", r.from_label, r.message));
    }
    Some(lines.join("\n"))
}

/// Check if a file path is claimed by an active peer (off-limits enforcement).
///
/// Returns `Some((peer_label, matched_glob))` if the file is claimed by another
/// active session, `None` otherwise. Self-claims and stale peer claims are excluded.
pub(super) fn check_offlimits(
    project_id: &str,
    session_id: &str,
    file_path: &str,
) -> Option<(String, String)> {
    // Solo gate: skip when no peers are active.
    if read_peer_count(project_id, session_id) == 0 {
        return None;
    }

    // Get active peers (excludes self and stale sessions).
    let active_peers = crate::peers::discover_active_peers(project_id, session_id);
    if active_peers.is_empty() {
        return None;
    }

    // Collect active peer session IDs for cross-referencing.
    let active_sids: std::collections::HashSet<&str> =
        active_peers.iter().map(|p| p.session_id.as_str()).collect();

    // Get board state (cached per #83).
    let board = crate::peers::compute_board_state(project_id);

    // Normalize path separators for cross-platform matching.
    let normalized = file_path.replace('\\', "/");

    for claim in &board.claims {
        // Skip self-claims.
        if claim.session_id == session_id {
            continue;
        }
        // Skip claims from stale/inactive peers.
        if !active_sids.contains(claim.session_id.as_str()) {
            continue;
        }

        for glob_pattern in &claim.paths {
            if let Ok(glob) = Glob::new(glob_pattern) {
                let matcher = glob.compile_matcher();
                if matcher.is_match(&normalized) {
                    return Some((claim.label.clone(), glob_pattern.clone()));
                }
                // Also try matching against just the file name.
                if let Some(file_name) =
                    Path::new(&normalized).file_name().and_then(|n| n.to_str())
                {
                    if matcher.is_match(file_name) {
                        return Some((claim.label.clone(), glob_pattern.clone()));
                    }
                }
            }
        }
    }

    None
}

/// L3: evaluate learned rules against the current PreToolUse hook context.
/// Returns a warning string if any rules triggered, None otherwise.
pub(super) fn evaluate_learned_rules(
    raw: &serde_json::Value,
    project_id: &str,
    cwd: &str,
) -> Option<String> {
    if std::env::var("EDDA_POSTMORTEM").unwrap_or_else(|_| "1".into()) == "0" {
        return None;
    }

    let mut rules_store = edda_postmortem::RulesStore::load_project(project_id);
    if rules_store.active_rules().is_empty() {
        return None;
    }

    let tool_name = get_str(raw, "tool_name");
    let file_path = raw
        .pointer("/tool_input/file_path")
        .or_else(|| raw.pointer("/input/file_path"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let files_touched = if file_path.is_empty() {
        vec![]
    } else {
        vec![file_path]
    };

    let hook_ctx = edda_postmortem::hooks::HookContext {
        hook_event: "PreToolUse".to_string(),
        tool_name,
        files_touched,
        cwd: cwd.to_string(),
    };

    let result = edda_postmortem::hooks::evaluate_rules(&rules_store, &hook_ctx);

    // Record hits so matched rules get their TTL reset
    if !result.matched_rule_ids.is_empty() {
        edda_postmortem::hooks::record_matched_hits(&mut rules_store, &result.matched_rule_ids);
        let _ = rules_store.save_project(project_id);
    }

    edda_postmortem::hooks::format_warnings(&result)
}
pub(super) fn dispatch_post_tool_use(
    raw: &serde_json::Value,
    project_id: &str,
    session_id: &str,
    cwd: &str,
) -> anyhow::Result<HookResult> {
    // Real-time auto-claim: on Edit/Write, track the file and update claim scope.
    let tool_name = get_str(raw, "tool_name");
    if tool_name == "Edit" || tool_name == "Write" {
        if let Some(fp) = raw.pointer("/input/file_path").and_then(|v| v.as_str()) {
            crate::peers::maybe_auto_claim_file(project_id, session_id, fp);
        }
    }

    // Update heartbeat branch when actual branch differs from heartbeat.
    // Runs on every Bash PostToolUse (~10ms git rev-parse) instead of guessing
    // intent from command strings, which is fragile.
    if tool_name == "Bash" {
        if let Some(actual) = crate::peers::detect_git_branch_in(cwd) {
            if let Some(hb) = crate::peers::read_heartbeat(project_id, session_id) {
                if hb.branch.as_deref() != Some(actual.as_str()) {
                    crate::peers::update_heartbeat_branch(project_id, session_id, &actual);
                }
            }
        }
    }

    // Agent phase detection (best-effort, lightweight).
    try_update_agent_phase(raw, project_id, session_id, cwd);

    let signal = match crate::nudge::detect_signal(raw) {
        Some(s) => s,
        None => return Ok(HookResult::empty()),
    };

    // Count every detected signal (including SelfRecord and cooldown-suppressed ones).
    increment_counter(project_id, session_id, "signal_count");

    // Auto-write events to workspace ledger (best-effort, try-lock).
    match &signal {
        crate::nudge::NudgeSignal::Commit(msg) => try_write_commit_event(raw, msg),
        crate::nudge::NudgeSignal::Merge(src, strategy) => {
            try_write_merge_event(raw, src, strategy)
        }
        _ => {}
    }

    // Write-back to karvi API if this is a karvi project (fire-and-forget).
    if is_karvi_project(cwd) {
        try_post_karvi_signal(cwd, &signal, session_id, project_id);
    }

    // Agent is recording a decision → increment counter, but do NOT suppress future nudges.
    // This allows the agent to receive nudges for subsequent signals after cooldown.
    if signal == crate::nudge::NudgeSignal::SelfRecord {
        increment_counter(project_id, session_id, "decide_count");
        return Ok(HookResult::empty());
    }

    // Check cooldown
    if !should_nudge(project_id, session_id) {
        return Ok(HookResult::empty());
    }

    let decide_count = read_counter(project_id, session_id, "decide_count");
    let nudge_text = crate::nudge::format_nudge(&signal, decide_count);
    mark_nudge_sent(project_id, session_id);
    increment_counter(project_id, session_id, "nudge_count");

    let wrapped = wrap_context_boundary(&nudge_text);
    let output = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": wrapped
        }
    });
    Ok(HookResult::output(serde_json::to_string(&output)?))
}
/// Detect agent phase and emit transition event if changed (best-effort).
pub(super) fn try_update_agent_phase(
    raw: &serde_json::Value,
    project_id: &str,
    session_id: &str,
    cwd: &str,
) {
    let label = std::env::var("EDDA_SESSION_LABEL").ok();
    let branch = detect_git_branch_cached(cwd);
    let active_tasks = read_active_task_names(project_id);

    let cwd_path = Path::new(cwd);
    let current = crate::agent_phase::detect_current_phase(
        session_id,
        label.as_deref(),
        branch.as_deref(),
        &active_tasks,
        cwd_path,
        None,
    );

    let previous = crate::agent_phase::read_phase_state(project_id, session_id);
    let config = crate::agent_phase::DetectorConfig::default();

    if let Some(transition) =
        crate::agent_phase::detect_transition(&current, previous.as_ref(), &config)
    {
        // Write updated phase state
        let _ = crate::agent_phase::write_phase_state(project_id, &transition.state);

        // Emit ledger event (best-effort, try-lock)
        try_write_phase_change_event(raw, &transition);
    } else {
        // Always persist latest state (updates detected_at for staleness tracking)
        let _ = crate::agent_phase::write_phase_state(project_id, &current);
    }
}

/// Write an agent_phase_change event to the workspace ledger (best-effort).
fn try_write_phase_change_event(
    raw: &serde_json::Value,
    transition: &edda_core::agent_phase::AgentPhaseTransition,
) {
    let cwd = get_str(raw, "cwd");
    if cwd.is_empty() {
        return;
    }
    let Some(root) = edda_ledger::EddaPaths::find_root(Path::new(&cwd)) else {
        return;
    };
    let Ok(ledger) = edda_ledger::Ledger::open(&root) else {
        return;
    };
    let Ok(_lock) = edda_ledger::WorkspaceLock::acquire(&ledger.paths) else {
        return; // locked by another process — skip
    };
    let Ok(branch) = ledger.head_branch() else {
        return;
    };
    let Ok(parent_hash) = ledger.last_event_hash() else {
        return;
    };
    let params = edda_core::event::AgentPhaseChangeParams {
        branch: &branch,
        parent_hash: parent_hash.as_deref(),
        session_id: &transition.state.session_id,
        label: transition.state.label.as_deref(),
        from: &transition.from.to_string(),
        to: &transition.to.to_string(),
        issue: transition.state.issue,
        confidence: transition.state.confidence,
        signals: &transition.state.signals,
    };
    if let Ok(event) = edda_core::event::new_agent_phase_change_event(&params) {
        let _ = ledger.append_event(&event);
    }

    // Push notification (best-effort)
    let config = edda_notify::NotifyConfig::load(&ledger.paths);
    if !config.channels.is_empty() {
        edda_notify::dispatch(
            &config,
            &edda_notify::NotifyEvent::PhaseChange {
                session_id: transition.state.session_id.clone(),
                from: transition.from.to_string(),
                to: transition.to.to_string(),
                issue: transition.state.issue,
            },
        );
    }
}

/// Read active task names from state file for phase detection heuristics.
pub(super) fn read_active_task_names(project_id: &str) -> Vec<String> {
    let path = edda_store::project_dir(project_id)
        .join("state")
        .join("active_tasks.json");
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let val: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    val.get("tasks")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let status = item.get("status")?.as_str()?;
                    if status != "in_progress" {
                        return None;
                    }
                    Some(item.get("subject")?.as_str()?.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}
/// Get git branch for the given working directory.
pub(super) fn detect_git_branch_cached(cwd: &str) -> Option<String> {
    crate::peers::detect_git_branch_in(cwd)
}

/// Check if patterns are enabled and match tool input against Pattern Store.
pub(super) fn match_tool_patterns(raw: &serde_json::Value, cwd: &str) -> Option<String> {
    // Check if patterns feature is enabled
    let enabled = match std::env::var("EDDA_PATTERNS_ENABLED") {
        Ok(val) => val == "1",
        Err(_) => read_workspace_config_bool(cwd, "patterns_enabled").unwrap_or(false),
    };
    if !enabled {
        return None;
    }

    // Only match on Edit and Write tools
    let tool_name = raw.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
    if tool_name != "Edit" && tool_name != "Write" {
        return None;
    }

    // Extract file_path from tool_input
    let file_path = raw
        .get("tool_input")
        .and_then(|ti| ti.get("file_path"))
        .and_then(|fp| fp.as_str())
        .unwrap_or("");
    if file_path.is_empty() {
        return None;
    }

    // Find patterns dir
    let root = edda_ledger::EddaPaths::find_root(Path::new(cwd))?;
    let patterns_dir = root.join(".edda").join("patterns");

    // Load and match
    let patterns = crate::pattern::load_patterns(&patterns_dir);
    if patterns.is_empty() {
        return None;
    }

    let matched = crate::pattern::match_patterns(&patterns, file_path);
    if matched.is_empty() {
        return None;
    }

    let budget: usize = std::env::var("EDDA_PATTERN_BUDGET_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000);

    crate::pattern::render_pattern_context(&matched, file_path, budget)
}
