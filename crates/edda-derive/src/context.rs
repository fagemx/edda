use anyhow::Result;
use edda_ledger::Ledger;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::snapshot::build_branch_snapshot;
use crate::types::*;

/// Extract a grouping key from a CmdFail signal text.
///
/// Input format: "cargo check -p edda-mcp (exit=1)" → "cargo check"
/// Keeps first 2 tokens of the command (before the exit= suffix).
fn cmd_base_key(signal_text: &str) -> String {
    // Strip trailing "(exit=N)" suffix if present
    let cmd = signal_text
        .rfind(" (exit=")
        .map(|pos| &signal_text[..pos])
        .unwrap_or(signal_text);
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    match tokens.len() {
        0 => signal_text.to_string(),
        1 => tokens[0].to_string(),
        _ => format!("{} {}", tokens[0], tokens[1]),
    }
}

/// Detect tasks that appear non-completed across 2+ session digests.
///
/// Returns a rendered "### Persistent Tasks" sub-section, or empty string if none found.
/// Digests are expected in chronological order (oldest first).
fn render_persistent_tasks(digests: &[SessionDigestEntry]) -> String {
    if digests.len() < 2 {
        return String::new();
    }

    struct TaskTracker<'a> {
        pending_sessions: u16,
        first_session_id: &'a str,
        last_session_id: &'a str,
        last_status: &'a str,
    }

    let mut tracker: HashMap<&str, TaskTracker> = HashMap::new();

    // Scan oldest-first (natural order)
    for d in digests {
        let sid = &d.session_id[..d.session_id.len().min(8)];
        for task in &d.tasks_snapshot {
            let entry = tracker
                .entry(task.subject.as_str())
                .or_insert(TaskTracker {
                    pending_sessions: 0,
                    first_session_id: sid,
                    last_session_id: sid,
                    last_status: task.status.as_str(),
                });
            entry.last_session_id = sid;
            entry.last_status = task.status.as_str();
            if task.status != "completed" {
                entry.pending_sessions += 1;
            }
        }
    }

    // Filter persistent (pending in 2+ sessions), sort by most sessions first
    let mut persistent: Vec<_> = tracker
        .into_iter()
        .filter(|(_, t)| t.pending_sessions >= 2)
        .collect();
    persistent.sort_by(|a, b| b.1.pending_sessions.cmp(&a.1.pending_sessions));

    if persistent.is_empty() {
        return String::new();
    }

    let mut out = String::from("### Persistent Tasks\n");
    for (subject, t) in persistent.iter().take(3) {
        if t.last_status == "completed" {
            out.push_str(&format!(
                "- \"{}\" (resolved in {}, was pending {} sessions)\n",
                subject, t.last_session_id, t.pending_sessions,
            ));
        } else {
            out.push_str(&format!(
                "- \"{}\" (pending since {}, {} sessions)\n",
                subject, t.first_session_id, t.pending_sessions,
            ));
        }
    }

    out
}

/// Render session history with three tiers of detail.
///
/// - Tier 1 (last session): Full detail — tasks, commits, files, notes, failed commands, outcome
/// - Tier 2 (sessions 2-5): One-liner per session — date, outcome, commit count, key activity
/// - Tier 3 (sessions 6+): Aggregate — N sessions, M commits, time span
fn render_session_history(digests: &[SessionDigestEntry]) -> String {
    if digests.is_empty() {
        return String::new();
    }

    // Newest first
    let newest_first: Vec<_> = digests.iter().rev().collect();

    let mut out = String::new();
    out.push_str("## Session History\n");

    // Tier 1: Last session — full detail
    let d = newest_first[0];
    let sid_short = &d.session_id[..d.session_id.len().min(8)];
    let date = d.ts.get(..10).unwrap_or(&d.ts);
    let outcome_badge = match d.outcome.as_str() {
        "error_stuck" => " -- error_stuck",
        "interrupted" => " -- interrupted",
        _ => "",
    };
    out.push_str(&format!(
        "### Last Session ({sid_short}, {date}, {} min){outcome_badge}\n",
        d.duration_minutes,
    ));

    // Tasks (Done/WIP) or fallback to stats
    if !d.tasks_snapshot.is_empty() {
        let done: Vec<&str> = d
            .tasks_snapshot
            .iter()
            .filter(|t| t.status == "completed")
            .map(|t| t.subject.as_str())
            .collect();
        let wip: Vec<&str> = d
            .tasks_snapshot
            .iter()
            .filter(|t| t.status != "completed")
            .map(|t| t.subject.as_str())
            .collect();
        if !done.is_empty() {
            out.push_str(&format!("- Done: {}\n", done.join(", ")));
        }
        if !wip.is_empty() {
            out.push_str(&format!("- WIP: {}\n", wip.join(", ")));
        }
    } else {
        out.push_str(&format!(
            "- {} tool calls, {} failures, {} prompts\n",
            d.tool_calls, d.tool_failures, d.user_prompts,
        ));
    }

    // Files
    if !d.files_modified.is_empty() {
        let short_files: Vec<&str> = d
            .files_modified
            .iter()
            .map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f))
            .collect();
        out.push_str(&format!("- Files: {}\n", short_files.join(", ")));
    }

    // Commits
    if !d.commits_made.is_empty() {
        out.push_str("- Commits:");
        for msg in &d.commits_made {
            let display = if msg.len() > 80 {
                let end = msg.floor_char_boundary(77);
                format!(" {}...", &msg[..end])
            } else {
                format!(" {msg}")
            };
            out.push_str(&display);
            out.push(';');
        }
        out.push('\n');
    }

    // Failed commands
    if !d.failed_commands.is_empty() {
        let short_cmds: Vec<&str> = d
            .failed_commands
            .iter()
            .map(|c| {
                if c.len() > 60 {
                    let end = c.floor_char_boundary(57);
                    &c[..end]
                } else {
                    c.as_str()
                }
            })
            .collect();
        out.push_str(&format!("- Failed: {}\n", short_cmds.join("; ")));
    }

    // Notes (new in tiered rendering)
    if !d.notes.is_empty() {
        for note in &d.notes {
            let display = if note.len() > 120 {
                let end = note.floor_char_boundary(117);
                format!("{}...", &note[..end])
            } else {
                note.clone()
            };
            out.push_str(&format!("- Note: \"{display}\"\n"));
        }
    }

    // Persistent tasks (cross-session detection)
    let persistent = render_persistent_tasks(digests);
    if !persistent.is_empty() {
        out.push_str(&persistent);
    }

    // Tier 2: Sessions 2-5 — one-liner each
    let tier2_end = newest_first.len().min(5);
    if newest_first.len() > 1 {
        out.push_str("### Prior Sessions\n");
        for d in &newest_first[1..tier2_end] {
            let sid_short = &d.session_id[..d.session_id.len().min(8)];
            let date = d.ts.get(..10).unwrap_or(&d.ts);
            let commit_count = d.commits_made.len();
            let file_count = d.files_modified.len();
            let outcome_tag = match d.outcome.as_str() {
                "completed" => "",
                other => other,
            };
            let outcome_suffix = if outcome_tag.is_empty() {
                String::new()
            } else {
                format!(", {outcome_tag}")
            };
            let commit_word = if commit_count == 1 { "commit" } else { "commits" };
            let file_word = if file_count == 1 { "file" } else { "files" };
            let note_suffix = if let Some(first_note) = d.notes.first() {
                if first_note.len() > 40 {
                    let truncated: String = first_note.chars().take(37).collect();
                    format!(" — \"{truncated}...\"")
                } else {
                    format!(" — \"{first_note}\"")
                }
            } else {
                String::new()
            };
            out.push_str(&format!(
                "- {sid_short} ({date}): {commit_count} {commit_word}, {file_count} {file_word}{outcome_suffix}{note_suffix}\n",
            ));
        }
    }

    // Tier 3: Sessions 6+ — aggregate
    if newest_first.len() > 5 {
        let older = &newest_first[5..];
        let total_commits: usize = older.iter().map(|d| d.commits_made.len()).sum();
        let total_files: usize = older.iter().map(|d| d.files_modified.len()).sum();
        let oldest_date = older.last().and_then(|d| d.ts.get(..10)).unwrap_or("?");
        let newest_date = older.first().and_then(|d| d.ts.get(..10)).unwrap_or("?");
        out.push_str(&format!(
            "### Earlier ({} sessions, {} - {})\n",
            older.len(),
            oldest_date,
            newest_date,
        ));
        let commit_word = if total_commits == 1 { "commit" } else { "commits" };
        let file_word = if total_files == 1 { "file" } else { "files" };
        out.push_str(&format!(
            "- {total_commits} {commit_word}, {total_files} {file_word} modified\n",
        ));
    }

    out.push('\n');
    out
}

pub fn render_context(ledger: &Ledger, branch: &str, opt: DeriveOptions) -> Result<String> {
    let snap = build_branch_snapshot(ledger, branch)?;
    let n = opt.depth.max(1);

    let commits: Vec<_> = snap.commits.iter().rev().take(n).collect::<Vec<_>>();
    let commits: Vec<_> = commits.into_iter().rev().collect();

    // Filter signals to last 2 hours to avoid showing stale errors
    let sig_cutoff = {
        let now = time::OffsetDateTime::now_utc();
        let cutoff = now - time::Duration::hours(2);
        cutoff
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default()
    };
    let recent_sigs: Vec<_> = snap
        .signals
        .iter()
        .filter(|s| s.ts.as_str() >= sig_cutoff.as_str())
        .collect();
    let sigs: Vec<_> = recent_sigs.iter().rev().take(n).copied().collect::<Vec<_>>();
    let sigs: Vec<_> = sigs.into_iter().rev().collect();

    let head = ledger
        .head_branch()
        .unwrap_or_else(|_| "main".to_string());

    let mut out = String::new();
    out.push_str("# CONTEXT SNAPSHOT\n\n");

    out.push_str("## Project (main)\n");
    out.push_str(&format!("- head: {head}\n"));
    out.push_str(&format!("- branch: {}\n", snap.branch));
    if let Some(c) = &snap.last_commit {
        out.push_str(&format!(
            "- uncommitted_events: {}\n",
            snap.uncommitted_events
        ));
        out.push_str(&format!(
            "- last_commit: {} {} \"{}\"\n",
            c.ts, c.event_id, c.title
        ));
    } else if snap.uncommitted_events > 0 {
        // No edda commits — show event count without misleading "uncommitted" framing
        out.push_str(&format!("- events: {}\n", snap.uncommitted_events));
    }
    // Session count and date span from digests
    if !snap.session_digests.is_empty() {
        let count = snap.session_digests.len();
        let dates: Vec<&str> = snap
            .session_digests
            .iter()
            .filter_map(|d| d.ts.get(..10))
            .collect();
        if count == 1 {
            if let Some(date) = dates.first() {
                out.push_str(&format!("- sessions: 1 ({date})\n"));
            }
        } else if let (Some(oldest), Some(newest)) = (dates.first(), dates.last()) {
            out.push_str(&format!(
                "- sessions: {count} ({oldest} — {newest})\n"
            ));
        }
    }
    out.push('\n');

    out.push_str("## Branch\n");
    out.push_str(&format!("- name: {}\n\n", snap.branch));

    // Tiered session history rendering
    let session_history = render_session_history(&snap.session_digests);
    if !session_history.is_empty() {
        out.push_str(&session_history);
    }

    out.push_str(&format!("## Recent Commits (last {n})\n"));
    if commits.is_empty() {
        out.push_str("- (none)\n\n");
    } else {
        for (i, c) in commits.iter().enumerate() {
            out.push_str(&format!(
                "{}. {} {} ({})\n",
                i + 1,
                c.ts,
                c.title,
                c.event_id
            ));
            out.push_str(&format!(
                "   - contribution: {}\n",
                if c.contribution.is_empty() {
                    "(empty)"
                } else {
                    &c.contribution
                }
            ));
            if c.evidence_lines.is_empty() {
                out.push_str("   - evidence: (none)\n");
            } else {
                out.push_str(&format!("   - evidence: {}\n", c.evidence_lines.join(", ")));
            }
        }
        out.push('\n');
    }

    let merge_list: Vec<_> = snap.merges.iter().rev().take(n).collect::<Vec<_>>();
    let merge_list: Vec<_> = merge_list.into_iter().rev().collect();

    out.push_str(&format!("## Recent Merges (last {n})\n"));
    if merge_list.is_empty() {
        out.push_str("- (none)\n\n");
    } else {
        for m in merge_list {
            out.push_str(&format!(
                "- {} {} {}->{} adopted={} reason=\"{}\"\n",
                m.ts,
                m.event_id,
                m.src,
                m.dst,
                m.adopted_commits.len(),
                m.reason
            ));
        }
        out.push('\n');
    }

    // Decisions — no time cutoff (decisions are long-lived)
    // Build superseded set: any event targeted by a "supersedes" provenance link is inactive
    let all_decisions: Vec<_> = snap
        .signals
        .iter()
        .filter(|s| matches!(s.kind, SignalKind::NoteDecision))
        .collect();
    let superseded: HashSet<&str> = all_decisions
        .iter()
        .filter_map(|d| d.supersedes.as_deref())
        .collect();
    let active_decisions: Vec<_> = all_decisions
        .iter()
        .filter(|d| !superseded.contains(d.event_id.as_str()))
        .rev()
        .take(n.max(5))
        .copied()
        .collect::<Vec<_>>();
    let active_decisions: Vec<_> = active_decisions.into_iter().rev().collect();

    if !active_decisions.is_empty() {
        out.push_str(&format!("## Decisions (last {})\n", active_decisions.len()));
        for d in &active_decisions {
            out.push_str(&format!("- {} ({})\n", d.text, d.event_id));
        }
        out.push('\n');
    }

    out.push_str(&format!("## Recent Signals (last {n})\n"));
    // Filter out decisions from signals (they have their own section)
    let non_decision_sigs: Vec<_> = sigs
        .iter()
        .filter(|s| !matches!(s.kind, SignalKind::NoteDecision))
        .collect();
    if non_decision_sigs.is_empty() {
        out.push_str("- (none)\n\n");
    } else {
        // Aggregate CmdFail signals by command base; keep NoteTodo as-is
        let mut cmd_groups: BTreeMap<String, Vec<&SignalEntry>> = BTreeMap::new();
        let mut todos: Vec<&SignalEntry> = Vec::new();

        for s in &non_decision_sigs {
            match s.kind {
                SignalKind::NoteTodo => todos.push(s),
                SignalKind::CmdFail => {
                    let key = cmd_base_key(&s.text);
                    cmd_groups.entry(key).or_default().push(s);
                }
                SignalKind::NoteDecision => {} // handled above
            }
        }

        for s in &todos {
            out.push_str(&format!("- NOTE(todo): {} ({})\n", s.text, s.event_id));
        }
        for (base, group) in &cmd_groups {
            if group.len() == 1 {
                out.push_str(&format!(
                    "- CMD fail: {} ({})\n",
                    group[0].text, group[0].event_id
                ));
            } else {
                out.push_str(&format!(
                    "- CMD fail: {} ({}x)\n",
                    base,
                    group.len(),
                ));
            }
        }
        out.push('\n');
    }

    out.push_str("## How to cite evidence\n");
    out.push_str("- Use event_id to locate raw trace in .edda/ledger/events.jsonl\n");
    out.push_str(
        "- Use blob:sha256:* to open stdout/stderr artifacts in .edda/ledger/blobs/\n",
    );

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::setup_workspace;
    use edda_core::Event;
    use edda_core::event::{
        new_note_event, new_commit_event, new_cmd_event,
        CommitEventParams, CmdEventParams,
    };

    #[test]
    fn render_context_includes_commits_and_signals() {
        let (tmp, ledger) = setup_workspace();

        // Add a todo note (becomes a signal)
        let todo_tags = vec!["todo".to_string()];
        let note = new_note_event("main", None, "user", "fix the bug", &todo_tags).unwrap();
        ledger.append_event(&note, false).unwrap();

        // Add a commit
        let mut params = CommitEventParams {
            branch: "main",
            parent_hash: None,
            title: "implement feature X",
            purpose: Some("deliver value"),
            prev_summary: "",
            contribution: "new feature",
            evidence: vec![],
            labels: vec![],
        };
        let commit = new_commit_event(&mut params).unwrap();
        ledger.append_event(&commit, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(ctx.contains("CONTEXT SNAPSHOT"));
        assert!(ctx.contains("main"));
        assert!(ctx.contains("implement feature X"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Helper: create a session_digest note event with full session_stats.
    fn make_digest_note(
        branch: &str,
        session_id: &str,
        tool_calls: u64,
        files: &[&str],
        commits: &[&str],
        failed: &[&str],
        duration_min: u64,
    ) -> Event {
        make_digest_note_with_tasks(branch, session_id, tool_calls, files, commits, failed, duration_min, &[])
    }

    fn make_digest_note_with_tasks(
        branch: &str,
        session_id: &str,
        tool_calls: u64,
        files: &[&str],
        commits: &[&str],
        failed: &[&str],
        duration_min: u64,
        tasks: &[(&str, &str)], // (subject, status)
    ) -> Event {
        make_digest_note_full(branch, session_id, tool_calls, files, commits, failed, duration_min, tasks, "completed")
    }

    fn make_digest_note_full(
        branch: &str,
        session_id: &str,
        tool_calls: u64,
        files: &[&str],
        commits: &[&str],
        failed: &[&str],
        duration_min: u64,
        tasks: &[(&str, &str)],
        outcome: &str,
    ) -> Event {
        use edda_core::event::finalize_event;
        use edda_core::types::SCHEMA_VERSION;

        let tasks_json: Vec<serde_json::Value> = tasks
            .iter()
            .map(|(s, st)| serde_json::json!({"subject": s, "status": st}))
            .collect();

        let payload = serde_json::json!({
            "role": "system",
            "text": format!("Session {session_id}: {tool_calls} tool calls"),
            "tags": ["session_digest"],
            "source": "bridge:session_digest",
            "session_id": session_id,
            "session_stats": {
                "tool_calls": tool_calls,
                "tool_failures": 0u64,
                "user_prompts": 1u64,
                "files_modified": files,
                "failed_commands": failed,
                "commits_made": commits,
                "tasks_snapshot": tasks_json,
                "outcome": outcome,
                "duration_minutes": duration_min,
            }
        });
        let mut event = Event {
            event_id: format!("evt_test_{session_id}"),
            ts: "2026-02-14T10:00:00Z".to_string(),
            event_type: "note".to_string(),
            branch: branch.to_string(),
            parent_hash: None,
            hash: String::new(),
            payload,
            refs: Default::default(),
            schema_version: SCHEMA_VERSION,
            digests: Vec::new(),
            event_family: None,
            event_level: None,
        };
        finalize_event(&mut event);
        event
    }

    fn make_digest_note_with_notes(
        branch: &str,
        session_id: &str,
        tool_calls: u64,
        files: &[&str],
        commits: &[&str],
        duration_min: u64,
        notes: &[&str],
    ) -> Event {
        use edda_core::event::finalize_event;
        use edda_core::types::SCHEMA_VERSION;

        let payload = serde_json::json!({
            "role": "system",
            "text": format!("Session {session_id}: {tool_calls} tool calls"),
            "tags": ["session_digest"],
            "source": "bridge:session_digest",
            "session_id": session_id,
            "session_stats": {
                "tool_calls": tool_calls,
                "tool_failures": 0u64,
                "user_prompts": 1u64,
                "files_modified": files,
                "failed_commands": serde_json::Value::Array(vec![]),
                "commits_made": commits,
                "tasks_snapshot": serde_json::Value::Array(vec![]),
                "outcome": "completed",
                "duration_minutes": duration_min,
                "notes": notes,
            }
        });
        let mut event = Event {
            event_id: format!("evt_test_{session_id}"),
            ts: "2026-02-14T10:00:00Z".to_string(),
            event_type: "note".to_string(),
            branch: branch.to_string(),
            parent_hash: None,
            hash: String::new(),
            payload,
            refs: Default::default(),
            schema_version: SCHEMA_VERSION,
            digests: Vec::new(),
            event_family: None,
            event_level: None,
        };
        finalize_event(&mut event);
        event
    }

    #[test]
    fn session_digest_surfaced_in_render_context() {
        let (tmp, ledger) = setup_workspace();

        // Write a session_digest note to the workspace ledger
        let digest = make_digest_note(
            "main",
            "sess-abc1",
            15,
            &["/src/main.rs", "/src/lib.rs"],
            &["fix: UTF-8 truncation", "feat: add digest"],
            &["cargo check -p edda-mcp"],
            45,
        );
        ledger.append_event(&digest, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Should contain Session History section
        assert!(
            ctx.contains("## Session History"),
            "missing Session History section in:\n{ctx}"
        );
        assert!(ctx.contains("sess-abc"), "missing session id in:\n{ctx}");
        assert!(ctx.contains("45 min"), "missing duration in:\n{ctx}");
        assert!(ctx.contains("15 tool calls"), "missing tool calls in:\n{ctx}");
        assert!(ctx.contains("main.rs"), "missing file name in:\n{ctx}");
        assert!(ctx.contains("lib.rs"), "missing file name in:\n{ctx}");
        assert!(
            ctx.contains("fix: UTF-8 truncation"),
            "missing commit msg in:\n{ctx}"
        );
        assert!(
            ctx.contains("cargo check -p edda-mcp"),
            "missing failed cmd in:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn session_history_tiers_correctly() {
        let (tmp, ledger) = setup_workspace();

        // Write 3 session digests — tier 1 (sess-003) + tier 2 (sess-002, sess-001)
        for i in 1..=3 {
            let digest = make_digest_note(
                "main",
                &format!("sess-{i:03}"),
                i * 10,
                &[],
                &[],
                &[],
                i * 15,
            );
            ledger.append_event(&digest, false).unwrap();
        }

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Tier 1: Last Session with full detail
        assert!(ctx.contains("### Last Session (sess-003"), "missing tier 1 in:\n{ctx}");
        // Tier 2: Prior sessions as one-liners
        assert!(ctx.contains("### Prior Sessions"), "missing tier 2 header in:\n{ctx}");
        assert!(ctx.contains("sess-002"), "missing sess-002 in tier 2:\n{ctx}");
        assert!(ctx.contains("sess-001"), "missing sess-001 in tier 2:\n{ctx}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn no_digests_no_section() {
        let (tmp, ledger) = setup_workspace();

        // Only a regular note, no session_digest
        let note = new_note_event("main", None, "user", "hello", &[]).unwrap();
        ledger.append_event(&note, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(
            !ctx.contains("Session History"),
            "should not show Session History when none exist"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn session_digest_shows_tasks_done_wip() {
        let (tmp, ledger) = setup_workspace();

        let digest = make_digest_note_with_tasks(
            "main",
            "sess-tasks1",
            10,
            &["/src/lib.rs"],
            &["fix: auth bug"],
            &[],
            30,
            &[("Fix auth bug", "completed"), ("Add tests", "in_progress"), ("Deploy", "pending")],
        );
        ledger.append_event(&digest, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Should show Done/WIP instead of tool call counts
        assert!(ctx.contains("- Done: Fix auth bug"), "missing Done in:\n{ctx}");
        assert!(ctx.contains("- WIP: Add tests, Deploy"), "missing WIP in:\n{ctx}");
        // Should NOT show tool call counts when tasks are present
        assert!(!ctx.contains("10 tool calls"), "should not show tool calls when tasks present in:\n{ctx}");
        // Files and commits should still appear
        assert!(ctx.contains("lib.rs"), "missing files in:\n{ctx}");
        assert!(ctx.contains("fix: auth bug"), "missing commit in:\n{ctx}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn session_digest_falls_back_to_stats_without_tasks() {
        let (tmp, ledger) = setup_workspace();

        // Digest with no tasks_snapshot — should fall back to tool call counts
        let digest = make_digest_note(
            "main",
            "sess-notask",
            8,
            &[],
            &[],
            &[],
            20,
        );
        ledger.append_event(&digest, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(ctx.contains("8 tool calls"), "should show tool calls as fallback in:\n{ctx}");
        assert!(!ctx.contains("Done:"), "should not show Done without tasks in:\n{ctx}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn session_digest_shows_outcome_badge() {
        let (tmp, ledger) = setup_workspace();

        let digest = make_digest_note_full(
            "main",
            "sess-stuck",
            5,
            &[],
            &[],
            &["cargo check"],
            15,
            &[("Fix build", "in_progress")],
            "error_stuck",
        );
        ledger.append_event(&digest, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(
            ctx.contains("-- error_stuck"),
            "should show error_stuck badge in:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn session_digest_no_badge_for_completed() {
        let (tmp, ledger) = setup_workspace();

        let digest = make_digest_note_full(
            "main",
            "sess-ok",
            10,
            &[],
            &[],
            &[],
            30,
            &[],
            "completed",
        );
        ledger.append_event(&digest, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(
            !ctx.contains("-- error_stuck"),
            "should not show badge for completed in:\n{ctx}"
        );
        assert!(
            !ctx.contains("-- interrupted"),
            "should not show badge for completed in:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn cmd_base_key_extracts_first_two_tokens() {
        assert_eq!(cmd_base_key("cargo check -p edda-mcp (exit=1)"), "cargo check");
        assert_eq!(cmd_base_key("cargo test --all (exit=101)"), "cargo test");
        assert_eq!(cmd_base_key("npm install (exit=1)"), "npm install");
        assert_eq!(cmd_base_key("make (exit=2)"), "make");
        assert_eq!(cmd_base_key(""), "");
    }

    #[test]
    fn signals_aggregate_repeated_cmd_fails() {
        let (tmp, ledger) = setup_workspace();

        // Add 3 "cargo check" failures and 1 "cargo test" failure
        for _ in 0..3 {
            let argv = vec!["cargo".to_string(), "check".to_string(), "-p".to_string(), "edda-mcp".to_string()];
            let cmd = new_cmd_event(&CmdEventParams {
                branch: "main",
                parent_hash: None,
                argv: &argv,
                cwd: ".",
                exit_code: 1,
                duration_ms: 500,
                stdout_blob: "",
                stderr_blob: "",
            }).unwrap();
            ledger.append_event(&cmd, false).unwrap();
        }
        let argv2 = vec!["cargo".to_string(), "test".to_string()];
        let cmd2 = new_cmd_event(&CmdEventParams {
            branch: "main",
            parent_hash: None,
            argv: &argv2,
            cwd: ".",
            exit_code: 1,
            duration_ms: 200,
            stdout_blob: "",
            stderr_blob: "",
        }).unwrap();
        ledger.append_event(&cmd2, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // "cargo check" should be aggregated as 3x
        assert!(
            ctx.contains("cargo check (3x)"),
            "expected aggregated 'cargo check (3x)' in:\n{ctx}"
        );
        // "cargo test" should appear individually (only 1)
        assert!(
            ctx.contains("cargo test (exit=1)"),
            "expected individual 'cargo test' in:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn signals_single_cmd_not_aggregated() {
        let (tmp, ledger) = setup_workspace();

        let argv = vec!["npm".to_string(), "install".to_string()];
        let cmd = new_cmd_event(&CmdEventParams {
            branch: "main",
            parent_hash: None,
            argv: &argv,
            cwd: ".",
            exit_code: 127,
            duration_ms: 100,
            stdout_blob: "",
            stderr_blob: "",
        }).unwrap();
        ledger.append_event(&cmd, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Single failure — show full text, not aggregated
        assert!(
            ctx.contains("CMD fail: npm install (exit=127)"),
            "expected individual signal in:\n{ctx}"
        );
        assert!(
            !ctx.contains("(1x)"),
            "should not show count for single failure"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn render_context_no_commits_shows_events_not_uncommitted() {
        let (tmp, ledger) = setup_workspace();

        // Add notes without any commit
        let n1 = new_note_event("main", None, "user", "note 1", &[]).unwrap();
        let n2 = new_note_event("main", None, "user", "note 2", &[]).unwrap();
        ledger.append_event(&n1, false).unwrap();
        ledger.append_event(&n2, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Should show "events: 2" instead of "uncommitted_events: 2"
        assert!(ctx.contains("- events: 2"), "expected '- events: 2' in:\n{ctx}");
        assert!(!ctx.contains("uncommitted_events"), "should not contain 'uncommitted_events'");
        assert!(!ctx.contains("last_commit: (none)"), "should not contain 'last_commit: (none)'");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn decision_notes_rendered_in_own_section() {
        let (tmp, ledger) = setup_workspace();

        let tags = vec!["decision".to_string()];
        let d1 = new_note_event("main", None, "user", "Use PostgreSQL for concurrent writes", &tags).unwrap();
        let d2 = new_note_event("main", None, "user", "REST over GraphQL for simplicity", &tags).unwrap();
        ledger.append_event(&d1, false).unwrap();
        ledger.append_event(&d2, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Decisions section should exist
        assert!(ctx.contains("## Decisions"), "missing Decisions section in:\n{ctx}");
        assert!(ctx.contains("Use PostgreSQL"), "missing decision text in:\n{ctx}");
        assert!(ctx.contains("REST over GraphQL"), "missing decision text in:\n{ctx}");

        // Decisions should NOT appear in Recent Signals
        let signals_section = ctx.split("## Recent Signals").nth(1).unwrap_or("");
        assert!(!signals_section.contains("PostgreSQL"), "decision leaked into Signals in:\n{ctx}");

        // Decisions section should come before Recent Signals
        let dec_pos = ctx.find("## Decisions").unwrap();
        let sig_pos = ctx.find("## Recent Signals").unwrap();
        assert!(dec_pos < sig_pos, "Decisions should come before Signals");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn decision_notes_no_time_cutoff() {
        let (tmp, ledger) = setup_workspace();

        // Create a decision note with an old timestamp (> 2 hours ago)
        use edda_core::event::finalize_event;
        use edda_core::types::SCHEMA_VERSION;

        let payload = serde_json::json!({
            "role": "user",
            "text": "JWT for auth tokens",
            "tags": ["decision"]
        });
        let mut event = edda_core::Event {
            event_id: "evt_old_decision".to_string(),
            ts: "2020-01-01T00:00:00Z".to_string(), // very old
            event_type: "note".to_string(),
            branch: "main".to_string(),
            parent_hash: None,
            hash: String::new(),
            payload,
            refs: Default::default(),
            schema_version: SCHEMA_VERSION,
            digests: Vec::new(),
            event_family: None,
            event_level: None,
        };
        finalize_event(&mut event);
        ledger.append_event(&event, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Old decision should still appear (no 2hr cutoff)
        assert!(ctx.contains("JWT for auth tokens"), "old decision should survive cutoff in:\n{ctx}");
        assert!(ctx.contains("## Decisions"), "Decisions section missing in:\n{ctx}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn no_decisions_no_section() {
        let (tmp, ledger) = setup_workspace();

        // Only a todo note, no decision
        let tags = vec!["todo".to_string()];
        let note = new_note_event("main", None, "user", "fix bug", &tags).unwrap();
        ledger.append_event(&note, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(!ctx.contains("## Decisions"), "should not have Decisions section when none exist");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn superseded_decision_hidden_in_context() {
        use edda_core::event::finalize_event;
        use edda_core::types::{Provenance, SCHEMA_VERSION};

        let (tmp, ledger) = setup_workspace();

        // First decision: db = mysql
        let tags = vec!["decision".to_string()];
        let d1 = new_note_event("main", None, "system", "db: mysql", &tags).unwrap();
        let d1_id = d1.event_id.clone();
        ledger.append_event(&d1, false).unwrap();

        // Second decision: db = postgres, supersedes d1
        let payload = serde_json::json!({
            "role": "system",
            "text": "db: postgres — need JSONB",
            "tags": ["decision"],
            "decision": {"key": "db", "value": "postgres", "reason": "need JSONB"}
        });
        let mut d2 = edda_core::Event {
            event_id: "evt_d2".to_string(),
            ts: "2026-02-17T12:00:00Z".to_string(),
            event_type: "note".to_string(),
            branch: "main".to_string(),
            parent_hash: None,
            hash: String::new(),
            payload,
            refs: edda_core::types::Refs {
                provenance: vec![Provenance {
                    target: d1_id.clone(),
                    rel: "supersedes".to_string(),
                    note: Some("key 'db' re-decided".to_string()),
                }],
                ..Default::default()
            },
            schema_version: SCHEMA_VERSION,
            digests: Vec::new(),
            event_family: None,
            event_level: None,
        };
        finalize_event(&mut d2);
        ledger.append_event(&d2, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Only d2 (postgres) should appear, not d1 (mysql)
        assert!(ctx.contains("postgres"), "active decision missing in:\n{ctx}");
        assert!(!ctx.contains("mysql"), "superseded decision should be hidden in:\n{ctx}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn supersession_chain_resolves() {
        use edda_core::event::finalize_event;
        use edda_core::types::Provenance;

        let (tmp, ledger) = setup_workspace();
        let tags = vec!["decision".to_string()];

        // A: db = sqlite
        let a = new_note_event("main", None, "system", "db: sqlite", &tags).unwrap();
        let a_id = a.event_id.clone();
        ledger.append_event(&a, false).unwrap();

        // B: db = mysql, supersedes A
        let mut b = new_note_event("main", None, "system", "db: mysql", &tags).unwrap();
        let b_id = b.event_id.clone();
        b.refs.provenance.push(Provenance {
            target: a_id, rel: "supersedes".to_string(), note: None,
        });
        finalize_event(&mut b);
        ledger.append_event(&b, false).unwrap();

        // C: db = postgres, supersedes B
        let mut c = new_note_event("main", None, "system", "db: postgres", &tags).unwrap();
        c.refs.provenance.push(Provenance {
            target: b_id, rel: "supersedes".to_string(), note: None,
        });
        finalize_event(&mut c);
        ledger.append_event(&c, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(ctx.contains("postgres"), "final decision missing in:\n{ctx}");
        assert!(!ctx.contains("mysql"), "superseded B should be hidden in:\n{ctx}");
        assert!(!ctx.contains("sqlite"), "superseded A should be hidden in:\n{ctx}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn old_decision_without_structured_fields_still_renders() {
        let (tmp, ledger) = setup_workspace();

        // Old-format decision (no payload.decision field)
        let tags = vec!["decision".to_string()];
        let d = new_note_event("main", None, "system", "orm: sqlx — compile-time checks", &tags).unwrap();
        ledger.append_event(&d, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(ctx.contains("orm: sqlx"), "old-format decision should render in:\n{ctx}");
        assert!(ctx.contains("## Decisions"), "Decisions section missing in:\n{ctx}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn session_history_shows_notes_in_tier1() {
        let (tmp, ledger) = setup_workspace();

        let digest = make_digest_note_with_notes(
            "main",
            "sess-notes",
            10,
            &["/src/auth.rs"],
            &["feat: add JWT"],
            30,
            &["Switched to JWT auth approach", "TODO: revisit caching"],
        );
        ledger.append_event(&digest, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(ctx.contains("## Session History"), "missing Session History in:\n{ctx}");
        assert!(ctx.contains("### Last Session (sess-not"), "missing tier 1 in:\n{ctx}");
        assert!(
            ctx.contains("Note: \"Switched to JWT auth approach\""),
            "missing note 1 in:\n{ctx}"
        );
        assert!(
            ctx.contains("Note: \"TODO: revisit caching\""),
            "missing note 2 in:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn session_history_old_digests_no_notes() {
        let (tmp, ledger) = setup_workspace();

        // Digest without notes field (backward compat)
        let digest = make_digest_note("main", "sess-old", 10, &[], &[], &[], 30);
        ledger.append_event(&digest, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(ctx.contains("## Session History"), "missing Session History in:\n{ctx}");
        assert!(!ctx.contains("Note:"), "should not show Note: line for old digests:\n{ctx}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn session_history_tier2_one_liners() {
        let (tmp, ledger) = setup_workspace();

        for i in 1..=4 {
            let digest = make_digest_note(
                "main",
                &format!("sess-{i:03}"),
                i * 5,
                &[&format!("/src/file{i}.rs")],
                &[&format!("commit {i}")],
                &[],
                i * 10,
            );
            ledger.append_event(&digest, false).unwrap();
        }

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Tier 1: full detail for newest
        assert!(ctx.contains("### Last Session (sess-004"), "missing tier 1 in:\n{ctx}");
        // Tier 2: one-liners for older sessions
        assert!(ctx.contains("### Prior Sessions"), "missing tier 2 header in:\n{ctx}");
        assert!(ctx.contains("sess-003"), "missing sess-003 in tier 2:\n{ctx}");
        assert!(ctx.contains("1 commit, 1 file"), "missing stats in tier 2:\n{ctx}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Helper: create a digest note with a custom timestamp.
    fn make_digest_note_with_ts(
        branch: &str,
        session_id: &str,
        ts: &str,
        files: &[&str],
        commits: &[&str],
    ) -> Event {
        use edda_core::event::finalize_event;
        use edda_core::types::SCHEMA_VERSION;

        let payload = serde_json::json!({
            "role": "system",
            "text": format!("Session {session_id}"),
            "tags": ["session_digest"],
            "source": "bridge:session_digest",
            "session_id": session_id,
            "session_stats": {
                "tool_calls": 10u64,
                "tool_failures": 0u64,
                "user_prompts": 1u64,
                "files_modified": files,
                "failed_commands": serde_json::Value::Array(vec![]),
                "commits_made": commits,
                "tasks_snapshot": serde_json::Value::Array(vec![]),
                "outcome": "completed",
                "duration_minutes": 30u64,
            }
        });
        let mut event = Event {
            event_id: format!("evt_test_{session_id}"),
            ts: ts.to_string(),
            event_type: "note".to_string(),
            branch: branch.to_string(),
            parent_hash: None,
            hash: String::new(),
            payload,
            refs: Default::default(),
            schema_version: SCHEMA_VERSION,
            digests: Vec::new(),
            event_family: None,
            event_level: None,
        };
        finalize_event(&mut event);
        event
    }

    #[test]
    fn decisions_decoupled_from_depth() {
        let (tmp, ledger) = setup_workspace();

        // Create 8 active decisions
        let tags = vec!["decision".to_string()];
        for i in 1..=8 {
            let d = new_note_event(
                "main", None, "user",
                &format!("Decision {i}: choice {i}"),
                &tags,
            ).unwrap();
            ledger.append_event(&d, false).unwrap();
        }

        // Render with depth=1 — decisions should still show up to 5
        let opts = DeriveOptions { depth: 1 };
        let ctx = render_context(&ledger, "main", opts).unwrap();

        assert!(ctx.contains("## Decisions"), "missing Decisions section in:\n{ctx}");
        // At least 5 decisions visible even at depth=1
        let decision_count = (1..=8)
            .filter(|i| ctx.contains(&format!("Decision {i}: choice {i}")))
            .count();
        assert!(
            decision_count >= 5,
            "expected at least 5 decisions visible at depth=1, got {decision_count} in:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn tier2_shows_first_note_truncated() {
        let (tmp, ledger) = setup_workspace();

        // Newest session (tier 1) — no notes needed for this test
        let d1 = make_digest_note("main", "sess-002", 10, &[], &[], &[], 30);
        // Older session (tier 2) — has a long note
        let d2 = make_digest_note_with_notes(
            "main", "sess-001", 5,
            &["/src/lib.rs"], &["fix: auth"], 20,
            &["Switched to JWT auth approach because session tokens were unreliable"],
        );
        ledger.append_event(&d2, false).unwrap();
        ledger.append_event(&d1, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Tier 2 should show truncated note (37 chars + "...")
        assert!(
            ctx.contains("\"Switched to JWT auth approach because"),
            "missing truncated note in tier 2:\n{ctx}"
        );
        assert!(
            ctx.contains("...\""),
            "missing truncation ellipsis in tier 2:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn tier2_no_note_when_empty() {
        let (tmp, ledger) = setup_workspace();

        let d1 = make_digest_note("main", "sess-002", 10, &[], &[], &[], 30);
        let d2 = make_digest_note("main", "sess-001", 5, &["/src/lib.rs"], &["fix: bug"], &[], 20);
        ledger.append_event(&d2, false).unwrap();
        ledger.append_event(&d1, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Tier 2 should NOT have note fragment
        let tier2_lines: Vec<&str> = ctx.lines()
            .filter(|l| l.contains("sess-001"))
            .collect();
        assert!(!tier2_lines.is_empty(), "missing sess-001 in tier 2:\n{ctx}");
        for line in &tier2_lines {
            assert!(
                !line.contains(" — \""),
                "should not have note fragment when notes empty: {line}"
            );
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn tier2_short_note_not_truncated() {
        let (tmp, ledger) = setup_workspace();

        let d1 = make_digest_note("main", "sess-002", 10, &[], &[], &[], 30);
        let d2 = make_digest_note_with_notes(
            "main", "sess-001", 5, &[], &["fix: it"], 20,
            &["Short note"],
        );
        ledger.append_event(&d2, false).unwrap();
        ledger.append_event(&d1, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Short note should appear in full, no "..."
        assert!(
            ctx.contains("\"Short note\""),
            "missing short note in tier 2:\n{ctx}"
        );
        assert!(
            !ctx.contains("Short note..."),
            "short note should not be truncated:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn project_header_shows_session_count_single() {
        let (tmp, ledger) = setup_workspace();

        let d = make_digest_note_with_ts(
            "main", "sess-001", "2026-02-17T10:00:00Z", &[], &[],
        );
        ledger.append_event(&d, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(
            ctx.contains("- sessions: 1 (2026-02-17)"),
            "missing single-session header in:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn project_header_shows_session_count_range() {
        let (tmp, ledger) = setup_workspace();

        let d1 = make_digest_note_with_ts(
            "main", "sess-001", "2026-02-12T10:00:00Z", &[], &["c1"],
        );
        let d2 = make_digest_note_with_ts(
            "main", "sess-002", "2026-02-14T10:00:00Z", &[], &["c2"],
        );
        let d3 = make_digest_note_with_ts(
            "main", "sess-003", "2026-02-17T10:00:00Z", &[], &["c3"],
        );
        ledger.append_event(&d1, false).unwrap();
        ledger.append_event(&d2, false).unwrap();
        ledger.append_event(&d3, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(
            ctx.contains("- sessions: 3 (2026-02-12 — 2026-02-17)"),
            "missing session range header in:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn project_header_no_sessions_without_digests() {
        let (tmp, ledger) = setup_workspace();

        let note = new_note_event("main", None, "user", "hello", &[]).unwrap();
        ledger.append_event(&note, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(
            !ctx.contains("- sessions:"),
            "should not show sessions line without digests:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn persistent_tasks_detected_across_sessions() {
        let (tmp, ledger) = setup_workspace();

        // 3 sessions, "Add OAuth" pending in all 3
        for i in 1..=3 {
            let d = make_digest_note_with_tasks(
                "main",
                &format!("sess-{i:03}"),
                10, &[], &[], &[], 30,
                &[("Add OAuth", "pending")],
            );
            ledger.append_event(&d, false).unwrap();
        }

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(
            ctx.contains("### Persistent Tasks"),
            "missing Persistent Tasks section in:\n{ctx}"
        );
        assert!(
            ctx.contains("\"Add OAuth\" (pending since sess-001, 3 sessions)"),
            "missing persistent task in:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn persistent_task_resolved() {
        let (tmp, ledger) = setup_workspace();

        // Sessions 1-2: "Fix auth" pending, Session 3: completed
        let d1 = make_digest_note_with_tasks(
            "main", "sess-001", 10, &[], &[], &[], 30,
            &[("Fix auth", "pending")],
        );
        let d2 = make_digest_note_with_tasks(
            "main", "sess-002", 10, &[], &[], &[], 30,
            &[("Fix auth", "in_progress")],
        );
        let d3 = make_digest_note_with_tasks(
            "main", "sess-003", 10, &[], &[], &[], 30,
            &[("Fix auth", "completed")],
        );
        ledger.append_event(&d1, false).unwrap();
        ledger.append_event(&d2, false).unwrap();
        ledger.append_event(&d3, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(
            ctx.contains("### Persistent Tasks"),
            "missing Persistent Tasks section in:\n{ctx}"
        );
        assert!(
            ctx.contains("\"Fix auth\" (resolved in sess-003, was pending 2 sessions)"),
            "missing resolved task in:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn no_persistent_tasks_single_session() {
        let (tmp, ledger) = setup_workspace();

        let d = make_digest_note_with_tasks(
            "main", "sess-001", 10, &[], &[], &[], 30,
            &[("Add OAuth", "pending")],
        );
        ledger.append_event(&d, false).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(
            !ctx.contains("Persistent Tasks"),
            "should not show Persistent Tasks for single session:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn session_history_tier3_aggregate() {
        let (tmp, ledger) = setup_workspace();

        // Create 8 digests: tier 1 (sess-008) + tier 2 (sess-007..004) + tier 3 (sess-003..001)
        for i in 1..=8 {
            let digest = make_digest_note(
                "main",
                &format!("sess-{i:03}"),
                i * 5,
                &[&format!("/src/file{i}.rs")],
                &[&format!("commit {i}")],
                &[],
                i * 10,
            );
            ledger.append_event(&digest, false).unwrap();
        }

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Tier 1
        assert!(ctx.contains("### Last Session (sess-008"), "missing tier 1 in:\n{ctx}");
        // Tier 2
        assert!(ctx.contains("### Prior Sessions"), "missing tier 2 in:\n{ctx}");
        assert!(ctx.contains("sess-007"), "missing sess-007 in tier 2:\n{ctx}");
        assert!(ctx.contains("sess-004"), "missing sess-004 in tier 2:\n{ctx}");
        // Tier 3: aggregate
        assert!(ctx.contains("### Earlier (3 sessions"), "missing tier 3 in:\n{ctx}");
        assert!(ctx.contains("3 commits, 3 files modified"), "missing aggregate stats in:\n{ctx}");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
