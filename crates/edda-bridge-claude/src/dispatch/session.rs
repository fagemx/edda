use std::fs;
use std::path::Path;

use crate::signals::{extract_session_signals, save_session_signals, TaskSnapshot};

use super::helpers::{
    extract_prior_session_last_message, inject_karvi_brief, render_active_plan,
    render_skill_guide_directive, run_auto_digest,
};
use super::{
    apply_context_budget, context_budget, is_same_as_last_inject, read_counter, read_hot_pack,
    read_peer_count, read_workspace_config_bool, render_workspace_section,
    render_write_back_protocol, take_compact_pending, wrap_context_boundary, write_inject_hash,
    write_peer_count, HookResult,
};

pub(super) fn ingest_and_build_pack(
    project_id: &str,
    session_id: &str,
    transcript_path: &str,
    cwd: &str,
) {
    if transcript_path.is_empty() || session_id.is_empty() {
        return;
    }
    let transcript = std::path::Path::new(transcript_path);
    if !transcript.exists() {
        return;
    }

    let project_dir = edda_store::project_dir(project_id);
    let _ = edda_store::ensure_dirs(project_id);

    // Ingest transcript delta with index generation
    let index_path = project_dir
        .join("index")
        .join(format!("{session_id}.jsonl"));
    let sid = session_id.to_string();
    let idx_path = index_path.clone();
    let index_writer = move |_raw: &str,
                             offset: u64,
                             len: u64,
                             parsed: &serde_json::Value|
          -> anyhow::Result<()> {
        let record = edda_index::build_index_record(&sid, offset, len, parsed);
        edda_index::append_index(&idx_path, &record)
    };

    let _ = edda_transcript::ingest_transcript_delta(
        &project_dir,
        session_id,
        transcript,
        Some(&index_writer),
    );

    // Build turns and render pack
    let max_turns: usize = std::env::var("EDDA_PACK_TURNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(12);
    let budget: usize = std::env::var("EDDA_PACK_BUDGET_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6000);

    if let Ok(turns) = edda_pack::build_turns(&project_dir, session_id, max_turns) {
        // Compute workspace section from .edda/ ledger
        let workspace_budget: usize = std::env::var("EDDA_WORKSPACE_BUDGET_CHARS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2500);
        let workspace_section = render_workspace_section(cwd, workspace_budget);
        let ws_len = workspace_section.as_ref().map(|s| s.len()).unwrap_or(0);
        let turns_budget = budget.saturating_sub(ws_len);

        let git_branch = {
            let cwd_path = std::path::Path::new(cwd);
            edda_ledger::EddaPaths::find_root(cwd_path)
                .and_then(|root| edda_ledger::Ledger::open(&root).ok())
                .and_then(|l| l.head_branch().ok())
                .unwrap_or_default()
        };
        let meta = edda_pack::PackMetadata {
            project_id: project_id.to_string(),
            session_id: session_id.to_string(),
            git_branch,
            turn_count: turns.len(),
            budget_chars: budget,
        };

        let mut pack_md = edda_pack::render_pack(&turns, &meta, turns_budget);

        // Insert workspace section between header and "## Recent Turns"
        if let Some(ws) = workspace_section {
            if let Some(pos) = pack_md.find("## Recent Turns") {
                pack_md.insert_str(pos, &format!("{ws}\n"));
            }
        }

        let _ = edda_pack::write_pack(&project_dir, &pack_md, &meta);
    }

    // Extract session signals (tasks, files, commits) from stored transcript
    let store_path = project_dir
        .join("transcripts")
        .join(format!("{session_id}.jsonl"));
    let signals = extract_session_signals(&store_path);
    save_session_signals(project_id, session_id, &signals);

    // Write full peer heartbeat with signals snapshot (unconditional — peer discovery depends on it)
    crate::peers::write_heartbeat(project_id, session_id, &signals, None);

    // Auto-claim scope from edited files (L1 auto-detection, #24)
    crate::peers::maybe_auto_claim(project_id, session_id, &signals);
}
/// Lightweight injection: workspace context only (~2K chars), no turns.
/// Supports session-scoped dedup: if workspace context is identical to the
/// last injection for this session, skip re-injecting.
pub(super) fn dispatch_with_workspace_only(
    project_id: &str,
    session_id: &str,
    cwd: &str,
    event_name: &str,
) -> anyhow::Result<HookResult> {
    let workspace_budget: usize = std::env::var("EDDA_WORKSPACE_BUDGET_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2500);
    let mut ws = render_workspace_section(cwd, workspace_budget);

    // Compute peers + board ONCE for the entire dispatch (#83).
    // Before: discover_active_peers called 2-3×, compute_board_state 3-4× per hook.
    let peers = crate::peers::discover_active_peers(project_id, session_id);
    let board = crate::peers::compute_board_state(project_id);

    // Detect solo → multi-session transition for late peer detection (#11).
    // On 0→N transition, inject the full coordination protocol instead of
    // lightweight peer updates so the agent learns L2 commands and sees
    // peer scope claims.
    let prev_count = read_peer_count(project_id, session_id);
    let first_peers = prev_count == 0 && !peers.is_empty();
    write_peer_count(project_id, session_id, peers.len());

    if first_peers {
        // First time seeing peers — inject full coordination protocol
        if let Some(coord) =
            crate::peers::render_coordination_protocol_with(&peers, &board, project_id, session_id)
        {
            ws = Some(match ws {
                Some(w) => format!("{w}\n\n{coord}"),
                None => coord,
            });
        }
    } else {
        // Normal: lightweight peer updates
        if let Some(updates) =
            crate::peers::render_peer_updates_with(&peers, &board, project_id, session_id)
        {
            ws = Some(match ws {
                Some(w) => format!("{w}\n{updates}"),
                None => updates,
            });
        }
    }

    if let Some(ws) = ws {
        let wrapped = wrap_context_boundary(&ws);
        // Dedup: skip if identical to last injection
        if !session_id.is_empty() && is_same_as_last_inject(project_id, session_id, &wrapped) {
            return Ok(HookResult::empty());
        }
        if !session_id.is_empty() {
            write_inject_hash(project_id, session_id, &wrapped);
        }
        let output = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": event_name,
                "additionalContext": wrapped
            }
        });
        Ok(HookResult::output(serde_json::to_string(&output)?))
    } else {
        Ok(HookResult::empty())
    }
}
/// Dispatch UserPromptSubmit — compact-aware.
///
/// Normal case: inject lightweight workspace context (~2K).
/// Post-compact: inject full hot pack + workspace context to compensate for
/// context loss during compaction.
pub(super) fn dispatch_user_prompt_submit(
    project_id: &str,
    session_id: &str,
    transcript_path: &str,
    cwd: &str,
) -> anyhow::Result<HookResult> {
    let post_compact = take_compact_pending(project_id);

    if post_compact {
        // Re-ingest so state files are fresh for future hooks.
        // The actual context injection already happened in the preceding
        // SessionStart:compact hook (hot pack + L1 narrative), so we
        // skip duplicate injection here.
        ingest_and_build_pack(project_id, session_id, transcript_path, cwd);
        Ok(HookResult::empty())
    } else {
        // Normal: lightweight workspace-only injection (with dedup).
        dispatch_with_workspace_only(project_id, session_id, cwd, "UserPromptSubmit")
    }
}
// ── SubagentStart ──

/// Inject lightweight coordination context into a sub-agent at spawn time.
pub(super) fn dispatch_subagent_context(
    project_id: &str,
    session_id: &str,
) -> anyhow::Result<HookResult> {
    let peers = crate::peers::discover_active_peers(project_id, session_id);
    if peers.is_empty() {
        return Ok(HookResult::empty());
    }
    let mut lines = vec!["## Active Peers (from edda)".to_string()];
    for p in &peers {
        let suffix =
            crate::peers::format_peer_suffix(p.branch.as_deref(), p.current_phase.as_deref());
        lines.push(format!("- {} {suffix}", p.label));
        if !p.claimed_paths.is_empty() {
            lines.push(format!("  claimed: {}", p.claimed_paths.join(", ")));
        }
    }
    let ctx = lines.join("\n");
    let wrapped = wrap_context_boundary(&ctx);
    let output = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "SubagentStart",
            "additionalContext": wrapped
        }
    });
    Ok(HookResult::output(serde_json::to_string(&output)?))
}
// ── SessionEnd ──

/// Dispatch SessionEnd — auto-digest, cleanup state, warn about pending tasks.
pub(super) fn dispatch_session_end(
    project_id: &str,
    session_id: &str,
    transcript_path: &str,
    cwd: &str,
    peers_active: bool,
) -> anyhow::Result<HookResult> {
    // 1. Final ingest so signals are up-to-date
    ingest_and_build_pack(project_id, session_id, transcript_path, cwd);

    // 2. Auto-digest this session eagerly (don't wait for next SessionStart)
    let _ = run_auto_digest(project_id, session_id, cwd);

    // 2b. Read recall rate counters before cleanup
    let nudge_count = read_counter(project_id, session_id, "nudge_count");
    let decide_count = read_counter(project_id, session_id, "decide_count");
    let signal_count = read_counter(project_id, session_id, "signal_count");

    // 2c. Snapshot session digest for next session's "## Previous Session"
    crate::digest::write_prev_digest_from_store(
        project_id,
        session_id,
        cwd,
        nudge_count,
        decide_count,
        signal_count,
    );

    // 2e. L3 post-mortem analysis (best-effort, fire-and-forget)
    run_postmortem(project_id, session_id, cwd);

    // 2f. Background decision extraction (non-blocking, best-effort)
    if crate::bg_extract::should_run(project_id, session_id) {
        let pid = project_id.to_string();
        let sid = session_id.to_string();
        std::thread::spawn(move || {
            if let Err(e) = crate::bg_extract::run_extraction(&pid, &sid) {
                eprintln!("[edda-bg] decision extraction failed: {e}");
            }
        });
    }

    // 2g. Background session digest (non-blocking, best-effort)
    if crate::bg_digest::should_run(project_id, session_id) {
        let pid = project_id.to_string();
        let sid = session_id.to_string();
        let cwd_str = cwd.to_string();
        std::thread::spawn(move || {
            if let Err(e) = crate::bg_digest::run_digest(&pid, &sid, &cwd_str) {
                eprintln!("[edda-bg] session digest failed: {e}");
            }
        });
    }

    // 2h. Background capability scan (non-blocking, cooldown-gated)
    if crate::bg_scan::should_run(project_id)
        || crate::bg_scan::has_recent_milestone(project_id, cwd)
    {
        let pid = project_id.to_string();
        let cwd_owned = cwd.to_string();
        std::thread::spawn(move || {
            if let Err(e) = crate::bg_scan::run_scan(&pid, &cwd_owned) {
                eprintln!("[edda-bg] capability scan failed: {e}");
            }
        });
    }

    // 2d. Push notification (best-effort, fire-and-forget)
    notify_session_end(project_id, cwd, session_id);

    // 3. Clean up session-scoped state files
    cleanup_session_state(project_id, session_id, peers_active);

    // 4. Collect warnings (pending tasks)
    if let Some(warning) = collect_session_end_warnings(project_id) {
        Ok(HookResult::warning(warning))
    } else {
        Ok(HookResult::empty())
    }
}

/// Best-effort push notification for session end.
/// Reads prev_digest.json (just written) for real outcome/duration data.
pub(super) fn notify_session_end(project_id: &str, cwd: &str, session_id: &str) {
    let Some(root) = edda_ledger::EddaPaths::find_root(Path::new(cwd)) else {
        return;
    };
    let paths = edda_ledger::EddaPaths::discover(&root);
    let config = edda_notify::NotifyConfig::load(&paths);
    if config.channels.is_empty() {
        return;
    }
    // Read the digest snapshot we just wrote for real session data
    let (outcome, duration_minutes, summary) = match crate::digest::read_prev_digest(project_id) {
        Some(d) => {
            let tasks: Vec<&str> = d.completed_tasks.iter().map(|s| s.as_str()).collect();
            let summary = if tasks.is_empty() {
                String::new()
            } else {
                format!("Completed: {}", tasks.join(", "))
            };
            (d.outcome, d.duration_minutes, summary)
        }
        None => ("completed".to_string(), 0, String::new()),
    };
    edda_notify::dispatch(
        &config,
        &edda_notify::NotifyEvent::SessionEnd {
            session_id: session_id.to_string(),
            outcome,
            duration_minutes,
            summary,
        },
    );
}

/// L3 post-mortem analysis: evaluate triggers, propose rules, store lessons.
///
/// Best-effort — all errors are silently swallowed. The session-end hook must
/// never fail because of post-mortem logic.
pub(super) fn run_postmortem(project_id: &str, session_id: &str, cwd: &str) {
    if std::env::var("EDDA_POSTMORTEM").unwrap_or_else(|_| "1".into()) == "0" {
        return;
    }

    let store_path = edda_store::project_dir(project_id)
        .join("ledger")
        .join(format!("{session_id}.jsonl"));
    if !store_path.exists() {
        return;
    }
    let stats = match crate::digest::extract_stats(&store_path) {
        Ok(s) => s,
        Err(_) => return,
    };

    let summary = edda_postmortem::trigger::SessionSummary {
        session_id: session_id.to_string(),
        user_prompts: stats.user_prompts,
        tool_failures: stats.tool_failures,
        failed_commands: stats.failed_commands.clone(),
        file_edit_counts: stats.file_edit_counts.clone(),
        decisions_superseded: 0,
        had_conflict: false,
        outcome: stats.outcome.to_string(),
    };

    let trigger = edda_postmortem::trigger::evaluate_triggers(&summary);
    if !trigger.should_analyze {
        return;
    }

    let input = edda_postmortem::analyzer::AnalysisInput {
        session_id: session_id.to_string(),
        user_prompts: stats.user_prompts,
        tool_failures: stats.tool_failures,
        failed_commands: stats.failed_commands.clone(),
        files_modified: stats.files_modified.clone(),
        file_edit_counts: stats.file_edit_counts.clone(),
        commits_made: stats.commits_made.clone(),
        decisions_superseded: 0,
        had_conflict: false,
        outcome: stats.outcome.to_string(),
        duration_minutes: stats.duration_minutes,
    };

    let result = edda_postmortem::analyzer::analyze(&trigger, &input);

    let mut rules_store = edda_postmortem::RulesStore::load_project(project_id);
    for proposal in &result.rule_proposals {
        rules_store.propose_rule(
            proposal.trigger.clone(),
            proposal.action.clone(),
            proposal.anchor_file.clone(),
            proposal.category.clone(),
            session_id.to_string(),
            None,
        );
    }

    // Rate-limited decay: only run once per day
    let should_decay = match &rules_store.last_decay_run {
        Some(last) => {
            let today = time::OffsetDateTime::now_utc().date().to_string();
            !last.starts_with(&today)
        }
        None => true,
    };
    if should_decay {
        rules_store.run_decay_cycle();
    }

    let _ = rules_store.save_project(project_id);

    let mut lessons_store = edda_postmortem::lessons::LessonsStore::load_project(project_id);
    lessons_store.add_lessons(&result.lessons, session_id);
    let _ = lessons_store.save_project(project_id);

    // Sync top lessons to CLAUDE.md (best-effort)
    let claude_md_path = Path::new(cwd).join("CLAUDE.md");
    if claude_md_path.exists() {
        let _ = lessons_store.sync_to_claude_md(&claude_md_path, 10);
    }

    eprintln!(
        "[edda L3] post-mortem: {} triggers, {} lessons, {} rule proposals",
        result.triggers.len(),
        result.lessons.len(),
        result.rule_proposals.len(),
    );
}

/// Remove session-scoped state files that are no longer needed.
pub(super) fn cleanup_session_state(project_id: &str, session_id: &str, peers_active: bool) {
    let state_dir = edda_store::project_dir(project_id).join("state");
    // Dedup hash (Step 2)
    let _ = fs::remove_file(state_dir.join(format!("inject_hash.{session_id}")));
    // Compact pending flag
    let _ = fs::remove_file(state_dir.join("compact_pending"));
    // Note: prev_digest.json is NOT deleted here — it persists for the next
    // session's SessionStart injection. It gets overwritten when this next
    // session ends and writes its own prev_digest.json.
    // Nudge state
    let _ = fs::remove_file(state_dir.join(format!("nudge_ts.{session_id}")));
    // Recall rate counters
    let _ = fs::remove_file(state_dir.join(format!("nudge_count.{session_id}")));
    let _ = fs::remove_file(state_dir.join(format!("decide_count.{session_id}")));
    let _ = fs::remove_file(state_dir.join(format!("signal_count.{session_id}")));
    // Late peer detection counter (#11)
    let _ = fs::remove_file(state_dir.join(format!("peer_count.{session_id}")));
    // Auto-claim state file (#24)
    crate::peers::remove_autoclaim_state(project_id, session_id);
    // Agent phase state file (#55)
    let _ = fs::remove_file(state_dir.join(format!("phase.{session_id}.json")));
    // Clean up any orphaned sub-agent heartbeats belonging to this session
    crate::peers::cleanup_subagent_heartbeats(project_id, session_id);
    // Peer heartbeat + unclaim (L2 — keep remove_heartbeat unconditional as idempotent cleanup)
    crate::peers::remove_heartbeat(project_id, session_id);
    if peers_active {
        crate::peers::write_unclaim(project_id, session_id);
    }
    // NOTE: The following state files are intentionally NOT deleted here — they
    // persist across sessions to provide continuity context at the next SessionStart:
    //   - active_tasks.json  → L1 narrative shows previous session's final task state
    //   - files_modified.json → activity summary for carry-over context
    //   - recent_commits.json → commit history carry-over
    //   - failed_commands.json → recurring failure detection
    // These files are overwritten (not appended) by save_session_signals() when
    // the next session produces new signal data, so stale data self-heals.
}

/// Check for pending tasks and produce a warning message if any remain.
pub(super) fn collect_session_end_warnings(project_id: &str) -> Option<String> {
    let state_dir = edda_store::project_dir(project_id).join("state");
    let tasks_path = state_dir.join("active_tasks.json");
    let content = fs::read_to_string(&tasks_path).ok()?;
    let tasks: Vec<TaskSnapshot> = serde_json::from_str(&content).ok()?;
    let pending: Vec<&TaskSnapshot> = tasks.iter().filter(|t| t.status != "completed").collect();
    if pending.is_empty() {
        return None;
    }
    let list: String = pending
        .iter()
        .take(5)
        .map(|t| format!("  - [ ] {}", t.subject))
        .collect::<Vec<_>>()
        .join("\n");
    let suffix = if pending.len() > 5 {
        format!("\n  ... and {} more", pending.len() - 5)
    } else {
        String::new()
    };
    Some(format!(
        "edda: {} task(s) still pending\n{list}{suffix}",
        pending.len()
    ))
}
/// Dispatch SessionStart with pack + skills + optional digest warning.
pub(super) fn dispatch_session_start(
    project_id: &str,
    session_id: &str,
    cwd: &str,
    digest_warning: Option<&str>,
) -> anyhow::Result<HookResult> {
    // Conductor mode: skip sections that overlap with conductor's --append-system-prompt.
    // See CONDUCTOR-SPEC.md §10.2.
    let conductor_mode = std::env::var("EDDA_CONDUCTOR_MODE").is_ok();

    let pack = read_hot_pack(project_id);
    let guide_mode = match std::env::var("EDDA_SKILL_GUIDE") {
        Ok(val) => val == "1",
        Err(_) => read_workspace_config_bool(cwd, "skill_guide").unwrap_or(false),
    };
    let mut content = if guide_mode {
        let directive = render_skill_guide_directive();
        match pack {
            Some(p) => Some(format!("{p}\n{directive}")),
            None => Some(directive),
        }
    } else {
        pack
    };

    // Append active plan file excerpt for cross-session continuity.
    // Conductor mode: skip — conductor provides plan context via --append-system-prompt.
    if !conductor_mode {
        if let Some(plan) = render_active_plan(Some(project_id)) {
            content = Some(match content {
                Some(c) => format!("{c}\n\n{plan}"),
                None => plan,
            });
        }
    }

    // Append L1 narrative (composed signals: focus + blocking + tasks + activity).
    // Conductor mode: minimal — only activity summary (1-2 lines).
    if conductor_mode {
        if let Some(activity) = crate::narrative::compose_narrative_minimal(project_id) {
            content = Some(match content {
                Some(c) => format!("{c}\n\n{activity}"),
                None => activity,
            });
        }
    } else if let Some(narrative) = crate::narrative::compose_narrative(project_id) {
        content = Some(match content {
            Some(c) => format!("{c}\n\n{narrative}"),
            None => narrative,
        });
    }

    // Inject karvi task brief if in karvi project
    if let Some(brief) = inject_karvi_brief(cwd) {
        content = Some(match content {
            Some(c) => format!("{c}\n\n{brief}"),
            None => brief,
        });
    }

    // Previous session context is now rendered within the workspace section's
    // "## Session History" (tiered rendering). No separate injection needed.

    // Build tail (reserved sections — always included, never truncated).
    // These are critical for agent functionality and must survive budget cuts.
    let mut tail = String::new();

    // Write-back protocol (L1 — always, solo or multi-session).
    if let Some(wb) = render_write_back_protocol(cwd) {
        tail.push_str(&format!("\n\n{wb}"));
    }

    // Agent phase nudge (if phase state exists from previous session or detected).
    if let Some(phase_state) = crate::agent_phase::read_phase_state(project_id, session_id) {
        let nudge = edda_core::agent_phase::format_phase_nudge(&phase_state);
        tail.push_str(&format!("\n\n{nudge}"));
    }

    // Coordination protocol for multi-session awareness.
    if let Some(coord) = crate::peers::render_coordination_protocol(project_id, session_id, cwd) {
        tail.push_str(&format!("\n\n{coord}"));
    }

    // Seed peer count so UserPromptSubmit knows the baseline and doesn't
    // re-inject the full protocol on the first prompt after SessionStart (#11).
    let peers = crate::peers::discover_active_peers(project_id, session_id);
    write_peer_count(project_id, session_id, peers.len());

    // Remaining body sections (truncatable — nice-to-have context).

    // Append last assistant message from prior session for continuity.
    // Conductor mode: skip — phases are independent.
    if !conductor_mode {
        if let Some(msg) = extract_prior_session_last_message(project_id, session_id) {
            let section = format!("## Previously (last response)\n> {msg}\n");
            content = Some(match content {
                Some(c) => format!("{c}\n\n{section}"),
                None => section,
            });
        }
    }

    // Append digest warning if present
    if let Some(warning) = digest_warning {
        content = Some(match content {
            Some(c) => format!("{c}\n\n{warning}"),
            None => warning.to_string(),
        });
    }

    // Apply budget: body gets (total - tail.len()), tail appended unconditionally.
    let total_budget = context_budget(cwd);
    let body_budget = total_budget.saturating_sub(tail.len());

    if let Some(ctx) = content {
        let budgeted_body = apply_context_budget(&ctx, body_budget);
        let final_content = if tail.is_empty() {
            budgeted_body
        } else {
            format!("{budgeted_body}{tail}")
        };
        let wrapped = wrap_context_boundary(&final_content);
        let output = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": wrapped
            }
        });
        Ok(HookResult::output(serde_json::to_string(&output)?))
    } else if !tail.is_empty() {
        let trimmed = tail.trim_start().to_string();
        let wrapped = wrap_context_boundary(&trimmed);
        let output = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": wrapped
            }
        });
        Ok(HookResult::output(serde_json::to_string(&output)?))
    } else {
        Ok(HookResult::empty())
    }
}
