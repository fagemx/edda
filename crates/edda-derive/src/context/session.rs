use std::collections::HashMap;

use crate::types::*;

use super::helpers::format_task_line;

/// Detect tasks that appear non-completed across 2+ session digests.
///
/// Returns a rendered "### Persistent Tasks" sub-section, or empty string if none found.
/// Digests are expected in chronological order (oldest first).
pub(super) fn render_persistent_tasks(digests: &[SessionDigestEntry]) -> String {
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
            let entry = tracker.entry(task.subject.as_str()).or_insert(TaskTracker {
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
pub(super) fn render_session_history(digests: &[SessionDigestEntry]) -> String {
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
            out.push_str(&format_task_line("Done", &done));
        }
        if !wip.is_empty() {
            out.push_str(&format_task_line("WIP", &wip));
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
            let commit_word = if commit_count == 1 {
                "commit"
            } else {
                "commits"
            };
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
        let commit_word = if total_commits == 1 {
            "commit"
        } else {
            "commits"
        };
        let file_word = if total_files == 1 { "file" } else { "files" };
        out.push_str(&format!(
            "- {total_commits} {commit_word}, {total_files} {file_word} modified\n",
        ));
    }

    out.push('\n');
    out
}

#[cfg(test)]
pub(crate) mod digest_helpers {
    use edda_core::Event;

    pub(crate) struct DigestParams<'a> {
        pub branch: &'a str,
        pub session_id: &'a str,
        pub tool_calls: u64,
        pub files: &'a [&'a str],
        pub commits: &'a [&'a str],
        pub failed: &'a [&'a str],
        pub duration_min: u64,
        pub tasks: &'a [(&'a str, &'a str)],
        pub outcome: &'a str,
    }

    /// Helper: create a session_digest note event with full session_stats.
    pub(crate) fn make_digest_note(
        branch: &str,
        session_id: &str,
        tool_calls: u64,
        files: &[&str],
        commits: &[&str],
        failed: &[&str],
        duration_min: u64,
    ) -> Event {
        make_digest_note_with_tasks(
            branch,
            session_id,
            tool_calls,
            files,
            commits,
            failed,
            duration_min,
            &[],
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn make_digest_note_with_tasks(
        branch: &str,
        session_id: &str,
        tool_calls: u64,
        files: &[&str],
        commits: &[&str],
        failed: &[&str],
        duration_min: u64,
        tasks: &[(&str, &str)], // (subject, status)
    ) -> Event {
        make_digest_note_from_params(&DigestParams {
            branch,
            session_id,
            tool_calls,
            files,
            commits,
            failed,
            duration_min,
            tasks,
            outcome: "completed",
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn make_digest_note_full(
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
        make_digest_note_from_params(&DigestParams {
            branch,
            session_id,
            tool_calls,
            files,
            commits,
            failed,
            duration_min,
            tasks,
            outcome,
        })
    }

    pub(crate) fn make_digest_note_from_params(p: &DigestParams<'_>) -> Event {
        use edda_core::event::finalize_event;
        use edda_core::types::SCHEMA_VERSION;

        let tasks_json: Vec<serde_json::Value> = p
            .tasks
            .iter()
            .map(|(s, st)| serde_json::json!({"subject": s, "status": st}))
            .collect();

        let session_id = p.session_id;
        let tool_calls = p.tool_calls;
        let payload = serde_json::json!({
            "role": "system",
            "text": format!("Session {session_id}: {tool_calls} tool calls"),
            "tags": ["session_digest"],
            "source": "bridge:session_digest",
            "session_id": p.session_id,
            "session_stats": {
                "tool_calls": p.tool_calls,
                "tool_failures": 0u64,
                "user_prompts": 1u64,
                "files_modified": p.files,
                "failed_commands": p.failed,
                "commits_made": p.commits,
                "tasks_snapshot": tasks_json,
                "outcome": p.outcome,
                "duration_minutes": p.duration_min,
            }
        });
        let mut event = Event {
            event_id: format!("evt_test_{}", p.session_id),
            ts: "2026-02-14T10:00:00Z".to_string(),
            event_type: "note".to_string(),
            branch: p.branch.to_string(),
            parent_hash: None,
            hash: String::new(),
            payload,
            refs: Default::default(),
            schema_version: SCHEMA_VERSION,
            digests: Vec::new(),
            event_family: None,
            event_level: None,
        };
        finalize_event(&mut event).unwrap();
        event
    }

    pub(crate) fn make_digest_note_with_notes(
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
        finalize_event(&mut event).unwrap();
        event
    }

    /// Helper: create a digest note with a custom timestamp.
    pub(crate) fn make_digest_note_with_ts(
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
        finalize_event(&mut event).unwrap();
        event
    }
}

#[cfg(test)]
mod tests {
    use super::digest_helpers::*;
    use crate::test_support::setup_workspace;
    use crate::types::*;

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
            ledger.append_event(&digest).unwrap();
        }

        let ctx = super::super::render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Tier 1: Last Session with full detail
        assert!(
            ctx.contains("### Last Session (sess-003"),
            "missing tier 1 in:\n{ctx}"
        );
        // Tier 2: Prior sessions as one-liners
        assert!(
            ctx.contains("### Prior Sessions"),
            "missing tier 2 header in:\n{ctx}"
        );
        assert!(
            ctx.contains("sess-002"),
            "missing sess-002 in tier 2:\n{ctx}"
        );
        assert!(
            ctx.contains("sess-001"),
            "missing sess-001 in tier 2:\n{ctx}"
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
            &[
                ("Fix auth bug", "completed"),
                ("Add tests", "in_progress"),
                ("Deploy", "pending"),
            ],
        );
        ledger.append_event(&digest).unwrap();

        let ctx = super::super::render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Should show Done/WIP with counts instead of tool call counts
        assert!(
            ctx.contains("- Done (1): Fix auth bug"),
            "missing Done in:\n{ctx}"
        );
        assert!(
            ctx.contains("- WIP (2): Add tests, Deploy"),
            "missing WIP in:\n{ctx}"
        );
        // Should NOT show tool call counts when tasks are present
        assert!(
            !ctx.contains("10 tool calls"),
            "should not show tool calls when tasks present in:\n{ctx}"
        );
        // Files and commits should still appear
        assert!(ctx.contains("lib.rs"), "missing files in:\n{ctx}");
        assert!(ctx.contains("fix: auth bug"), "missing commit in:\n{ctx}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn session_digest_falls_back_to_stats_without_tasks() {
        let (tmp, ledger) = setup_workspace();

        // Digest with no tasks_snapshot — should fall back to tool call counts
        let digest = make_digest_note("main", "sess-notask", 8, &[], &[], &[], 20);
        ledger.append_event(&digest).unwrap();

        let ctx = super::super::render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(
            ctx.contains("8 tool calls"),
            "should show tool calls as fallback in:\n{ctx}"
        );
        assert!(
            !ctx.contains("Done:"),
            "should not show Done without tasks in:\n{ctx}"
        );

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
        ledger.append_event(&digest).unwrap();

        let ctx = super::super::render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(
            ctx.contains("-- error_stuck"),
            "should show error_stuck badge in:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn session_digest_no_badge_for_completed() {
        let (tmp, ledger) = setup_workspace();

        let digest =
            make_digest_note_full("main", "sess-ok", 10, &[], &[], &[], 30, &[], "completed");
        ledger.append_event(&digest).unwrap();

        let ctx = super::super::render_context(&ledger, "main", DeriveOptions::default()).unwrap();

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
        ledger.append_event(&digest).unwrap();

        let ctx = super::super::render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(
            ctx.contains("## Session History"),
            "missing Session History in:\n{ctx}"
        );
        assert!(
            ctx.contains("### Last Session (sess-not"),
            "missing tier 1 in:\n{ctx}"
        );
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
        ledger.append_event(&digest).unwrap();

        let ctx = super::super::render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(
            ctx.contains("## Session History"),
            "missing Session History in:\n{ctx}"
        );
        assert!(
            !ctx.contains("Note:"),
            "should not show Note: line for old digests:\n{ctx}"
        );

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
            ledger.append_event(&digest).unwrap();
        }

        let ctx = super::super::render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Tier 1: full detail for newest
        assert!(
            ctx.contains("### Last Session (sess-004"),
            "missing tier 1 in:\n{ctx}"
        );
        // Tier 2: one-liners for older sessions
        assert!(
            ctx.contains("### Prior Sessions"),
            "missing tier 2 header in:\n{ctx}"
        );
        assert!(
            ctx.contains("sess-003"),
            "missing sess-003 in tier 2:\n{ctx}"
        );
        assert!(
            ctx.contains("1 commit, 1 file"),
            "missing stats in tier 2:\n{ctx}"
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
            "main",
            "sess-001",
            5,
            &["/src/lib.rs"],
            &["fix: auth"],
            20,
            &["Switched to JWT auth approach because session tokens were unreliable"],
        );
        ledger.append_event(&d2).unwrap();
        ledger.append_event(&d1).unwrap();

        let ctx = super::super::render_context(&ledger, "main", DeriveOptions::default()).unwrap();

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
        let d2 = make_digest_note(
            "main",
            "sess-001",
            5,
            &["/src/lib.rs"],
            &["fix: bug"],
            &[],
            20,
        );
        ledger.append_event(&d2).unwrap();
        ledger.append_event(&d1).unwrap();

        let ctx = super::super::render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Tier 2 should NOT have note fragment
        let tier2_lines: Vec<&str> = ctx.lines().filter(|l| l.contains("sess-001")).collect();
        assert!(
            !tier2_lines.is_empty(),
            "missing sess-001 in tier 2:\n{ctx}"
        );
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
            "main",
            "sess-001",
            5,
            &[],
            &["fix: it"],
            20,
            &["Short note"],
        );
        ledger.append_event(&d2).unwrap();
        ledger.append_event(&d1).unwrap();

        let ctx = super::super::render_context(&ledger, "main", DeriveOptions::default()).unwrap();

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
    fn persistent_tasks_detected_across_sessions() {
        let (tmp, ledger) = setup_workspace();

        // 3 sessions, "Add OAuth" pending in all 3
        for i in 1..=3 {
            let d = make_digest_note_with_tasks(
                "main",
                &format!("sess-{i:03}"),
                10,
                &[],
                &[],
                &[],
                30,
                &[("Add OAuth", "pending")],
            );
            ledger.append_event(&d).unwrap();
        }

        let ctx = super::super::render_context(&ledger, "main", DeriveOptions::default()).unwrap();

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
            "main",
            "sess-001",
            10,
            &[],
            &[],
            &[],
            30,
            &[("Fix auth", "pending")],
        );
        let d2 = make_digest_note_with_tasks(
            "main",
            "sess-002",
            10,
            &[],
            &[],
            &[],
            30,
            &[("Fix auth", "in_progress")],
        );
        let d3 = make_digest_note_with_tasks(
            "main",
            "sess-003",
            10,
            &[],
            &[],
            &[],
            30,
            &[("Fix auth", "completed")],
        );
        ledger.append_event(&d1).unwrap();
        ledger.append_event(&d2).unwrap();
        ledger.append_event(&d3).unwrap();

        let ctx = super::super::render_context(&ledger, "main", DeriveOptions::default()).unwrap();

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
            "main",
            "sess-001",
            10,
            &[],
            &[],
            &[],
            30,
            &[("Add OAuth", "pending")],
        );
        ledger.append_event(&d).unwrap();

        let ctx = super::super::render_context(&ledger, "main", DeriveOptions::default()).unwrap();

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
            ledger.append_event(&digest).unwrap();
        }

        let ctx = super::super::render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Tier 1
        assert!(
            ctx.contains("### Last Session (sess-008"),
            "missing tier 1 in:\n{ctx}"
        );
        // Tier 2
        assert!(
            ctx.contains("### Prior Sessions"),
            "missing tier 2 in:\n{ctx}"
        );
        assert!(
            ctx.contains("sess-007"),
            "missing sess-007 in tier 2:\n{ctx}"
        );
        assert!(
            ctx.contains("sess-004"),
            "missing sess-004 in tier 2:\n{ctx}"
        );
        // Tier 3: aggregate
        assert!(
            ctx.contains("### Earlier (3 sessions"),
            "missing tier 3 in:\n{ctx}"
        );
        assert!(
            ctx.contains("3 commits, 3 files modified"),
            "missing aggregate stats in:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn session_digest_truncates_large_task_list() {
        let (tmp, ledger) = setup_workspace();

        // Create 20 completed tasks + 2 WIP
        let mut tasks: Vec<(&str, &str)> = Vec::new();
        let task_names: Vec<String> = (1..=20).map(|i| format!("Task {i}")).collect();
        for name in &task_names {
            tasks.push((name.as_str(), "completed"));
        }
        tasks.push(("WIP item 1", "in_progress"));
        tasks.push(("WIP item 2", "pending"));

        let digest = make_digest_note_with_tasks(
            "main",
            "sess-big",
            50,
            &["/src/lib.rs"],
            &["feat: big change"],
            &[],
            60,
            &tasks,
        );
        ledger.append_event(&digest).unwrap();

        let ctx = super::super::render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Should show count
        assert!(
            ctx.contains("- Done (20):"),
            "missing Done count in:\n{ctx}"
        );
        // Should show first 3 tasks
        assert!(ctx.contains("Task 1"), "missing Task 1 in:\n{ctx}");
        assert!(ctx.contains("Task 2"), "missing Task 2 in:\n{ctx}");
        assert!(ctx.contains("Task 3"), "missing Task 3 in:\n{ctx}");
        // Should show "+N more"
        assert!(
            ctx.contains("(+17 more)"),
            "missing truncation suffix in:\n{ctx}"
        );
        // Should NOT show all 20 tasks
        assert!(
            !ctx.contains("Task 10"),
            "should not show Task 10 in:\n{ctx}"
        );
        // WIP should show all (only 2)
        assert!(
            ctx.contains("- WIP (2): WIP item 1, WIP item 2"),
            "WIP should show all items in:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
