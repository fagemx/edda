mod helpers;
mod session;

use anyhow::Result;
use edda_ledger::Ledger;
use std::collections::{BTreeMap, HashSet};

use crate::snapshot::build_branch_snapshot;
use crate::types::*;

use helpers::cmd_base_key;
use session::render_session_history;

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
    let sigs: Vec<_> = recent_sigs
        .iter()
        .rev()
        .take(n)
        .copied()
        .collect::<Vec<_>>();
    let sigs: Vec<_> = sigs.into_iter().rev().collect();

    let head = ledger.head_branch().unwrap_or_else(|_| "main".to_string());

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
            out.push_str(&format!("- sessions: {count} ({oldest} — {newest})\n"));
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
                out.push_str(&format!("- CMD fail: {} ({}x)\n", base, group.len(),));
            }
        }
        out.push('\n');
    }

    out.push_str("## How to cite evidence\n");
    out.push_str("- Use event_id to locate raw trace in .edda/ledger/events.jsonl\n");
    out.push_str("- Use blob:sha256:* to open stdout/stderr artifacts in .edda/ledger/blobs/\n");

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::setup_workspace;
    use edda_core::event::{
        new_cmd_event, new_commit_event, new_note_event, CmdEventParams, CommitEventParams,
    };

    use super::session::digest_helpers::*;

    #[test]
    fn render_context_includes_commits_and_signals() {
        let (tmp, ledger) = setup_workspace();

        // Add a todo note (becomes a signal)
        let todo_tags = vec!["todo".to_string()];
        let note = new_note_event("main", None, "user", "fix the bug", &todo_tags).unwrap();
        ledger.append_event(&note).unwrap();

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
        ledger.append_event(&commit).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(ctx.contains("CONTEXT SNAPSHOT"));
        assert!(ctx.contains("main"));
        assert!(ctx.contains("implement feature X"));

        let _ = std::fs::remove_dir_all(&tmp);
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
        ledger.append_event(&digest).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Should contain Session History section
        assert!(
            ctx.contains("## Session History"),
            "missing Session History section in:\n{ctx}"
        );
        assert!(ctx.contains("sess-abc"), "missing session id in:\n{ctx}");
        assert!(ctx.contains("45 min"), "missing duration in:\n{ctx}");
        assert!(
            ctx.contains("15 tool calls"),
            "missing tool calls in:\n{ctx}"
        );
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
    fn no_digests_no_section() {
        let (tmp, ledger) = setup_workspace();

        // Only a regular note, no session_digest
        let note = new_note_event("main", None, "user", "hello", &[]).unwrap();
        ledger.append_event(&note).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(
            !ctx.contains("Session History"),
            "should not show Session History when none exist"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn render_context_no_commits_shows_events_not_uncommitted() {
        let (tmp, ledger) = setup_workspace();

        // Add notes without any commit
        let n1 = new_note_event("main", None, "user", "note 1", &[]).unwrap();
        let n2 = new_note_event("main", None, "user", "note 2", &[]).unwrap();
        ledger.append_event(&n1).unwrap();
        ledger.append_event(&n2).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Should show "events: 2" instead of "uncommitted_events: 2"
        assert!(
            ctx.contains("- events: 2"),
            "expected '- events: 2' in:\n{ctx}"
        );
        assert!(
            !ctx.contains("uncommitted_events"),
            "should not contain 'uncommitted_events'"
        );
        assert!(
            !ctx.contains("last_commit: (none)"),
            "should not contain 'last_commit: (none)'"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn signals_aggregate_repeated_cmd_fails() {
        let (tmp, ledger) = setup_workspace();

        // Add 3 "cargo check" failures and 1 "cargo test" failure
        for _ in 0..3 {
            let argv = vec![
                "cargo".to_string(),
                "check".to_string(),
                "-p".to_string(),
                "edda-mcp".to_string(),
            ];
            let cmd = new_cmd_event(&CmdEventParams {
                branch: "main",
                parent_hash: None,
                argv: &argv,
                cwd: ".",
                exit_code: 1,
                duration_ms: 500,
                stdout_blob: "",
                stderr_blob: "",
            })
            .unwrap();
            ledger.append_event(&cmd).unwrap();
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
        })
        .unwrap();
        ledger.append_event(&cmd2).unwrap();

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
        })
        .unwrap();
        ledger.append_event(&cmd).unwrap();

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
    fn decision_notes_rendered_in_own_section() {
        let (tmp, ledger) = setup_workspace();

        let tags = vec!["decision".to_string()];
        let d1 = new_note_event(
            "main",
            None,
            "user",
            "Use PostgreSQL for concurrent writes",
            &tags,
        )
        .unwrap();
        let d2 = new_note_event(
            "main",
            None,
            "user",
            "REST over GraphQL for simplicity",
            &tags,
        )
        .unwrap();
        ledger.append_event(&d1).unwrap();
        ledger.append_event(&d2).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Decisions section should exist
        assert!(
            ctx.contains("## Decisions"),
            "missing Decisions section in:\n{ctx}"
        );
        assert!(
            ctx.contains("Use PostgreSQL"),
            "missing decision text in:\n{ctx}"
        );
        assert!(
            ctx.contains("REST over GraphQL"),
            "missing decision text in:\n{ctx}"
        );

        // Decisions should NOT appear in Recent Signals
        let signals_section = ctx.split("## Recent Signals").nth(1).unwrap_or("");
        assert!(
            !signals_section.contains("PostgreSQL"),
            "decision leaked into Signals in:\n{ctx}"
        );

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
        finalize_event(&mut event).unwrap();
        ledger.append_event(&event).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Old decision should still appear (no 2hr cutoff)
        assert!(
            ctx.contains("JWT for auth tokens"),
            "old decision should survive cutoff in:\n{ctx}"
        );
        assert!(
            ctx.contains("## Decisions"),
            "Decisions section missing in:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn no_decisions_no_section() {
        let (tmp, ledger) = setup_workspace();

        // Only a todo note, no decision
        let tags = vec!["todo".to_string()];
        let note = new_note_event("main", None, "user", "fix bug", &tags).unwrap();
        ledger.append_event(&note).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(
            !ctx.contains("## Decisions"),
            "should not have Decisions section when none exist"
        );

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
        ledger.append_event(&d1).unwrap();

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
        finalize_event(&mut d2).unwrap();
        ledger.append_event(&d2).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        // Only d2 (postgres) should appear, not d1 (mysql)
        assert!(
            ctx.contains("postgres"),
            "active decision missing in:\n{ctx}"
        );
        assert!(
            !ctx.contains("mysql"),
            "superseded decision should be hidden in:\n{ctx}"
        );

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
        ledger.append_event(&a).unwrap();

        // B: db = mysql, supersedes A
        let mut b = new_note_event("main", None, "system", "db: mysql", &tags).unwrap();
        let b_id = b.event_id.clone();
        b.refs.provenance.push(Provenance {
            target: a_id,
            rel: "supersedes".to_string(),
            note: None,
        });
        finalize_event(&mut b).unwrap();
        ledger.append_event(&b).unwrap();

        // C: db = postgres, supersedes B
        let mut c = new_note_event("main", None, "system", "db: postgres", &tags).unwrap();
        c.refs.provenance.push(Provenance {
            target: b_id,
            rel: "supersedes".to_string(),
            note: None,
        });
        finalize_event(&mut c).unwrap();
        ledger.append_event(&c).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(
            ctx.contains("postgres"),
            "final decision missing in:\n{ctx}"
        );
        assert!(
            !ctx.contains("mysql"),
            "superseded B should be hidden in:\n{ctx}"
        );
        assert!(
            !ctx.contains("sqlite"),
            "superseded A should be hidden in:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn old_decision_without_structured_fields_still_renders() {
        let (tmp, ledger) = setup_workspace();

        // Old-format decision (no payload.decision field)
        let tags = vec!["decision".to_string()];
        let d = new_note_event(
            "main",
            None,
            "system",
            "orm: sqlx — compile-time checks",
            &tags,
        )
        .unwrap();
        ledger.append_event(&d).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(
            ctx.contains("orm: sqlx"),
            "old-format decision should render in:\n{ctx}"
        );
        assert!(
            ctx.contains("## Decisions"),
            "Decisions section missing in:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn decisions_decoupled_from_depth() {
        let (tmp, ledger) = setup_workspace();

        // Create 8 active decisions
        let tags = vec!["decision".to_string()];
        for i in 1..=8 {
            let d = new_note_event(
                "main",
                None,
                "user",
                &format!("Decision {i}: choice {i}"),
                &tags,
            )
            .unwrap();
            ledger.append_event(&d).unwrap();
        }

        // Render with depth=1 — decisions should still show up to 5
        let opts = DeriveOptions { depth: 1 };
        let ctx = render_context(&ledger, "main", opts).unwrap();

        assert!(
            ctx.contains("## Decisions"),
            "missing Decisions section in:\n{ctx}"
        );
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
    fn project_header_shows_session_count_single() {
        let (tmp, ledger) = setup_workspace();

        let d = make_digest_note_with_ts("main", "sess-001", "2026-02-17T10:00:00Z", &[], &[]);
        ledger.append_event(&d).unwrap();

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

        let d1 = make_digest_note_with_ts("main", "sess-001", "2026-02-12T10:00:00Z", &[], &["c1"]);
        let d2 = make_digest_note_with_ts("main", "sess-002", "2026-02-14T10:00:00Z", &[], &["c2"]);
        let d3 = make_digest_note_with_ts("main", "sess-003", "2026-02-17T10:00:00Z", &[], &["c3"]);
        ledger.append_event(&d1).unwrap();
        ledger.append_event(&d2).unwrap();
        ledger.append_event(&d3).unwrap();

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
        ledger.append_event(&note).unwrap();

        let ctx = render_context(&ledger, "main", DeriveOptions::default()).unwrap();

        assert!(
            !ctx.contains("- sessions:"),
            "should not show sessions line without digests:\n{ctx}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
