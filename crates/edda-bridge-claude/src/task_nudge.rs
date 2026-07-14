//! Stop-hook task nudge — at turn end, tell the agent about rail tasks
//! newly assigned to it (TASK_RAIL_V1 §5: lightweight nudge, not the
//! transport backbone).
//!
//! Claude Code allows `additionalContext` injection only on four hook
//! events, and Stop is not one of them — so the nudge rides the
//! `{"decision":"block","reason":...}` channel. Loop safety, in order:
//! every task is nudged at most once per session (watermark file), and a
//! turn where a stop hook already fired (`stop_hook_active`) is never
//! blocked again. Any error degrades to silence — the nudge must never
//! break a session.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use edda_ledger::tasks::{TaskStatus, TaskView};

use crate::dispatch::HookResult;

/// Ready tasks assigned to `label` that have not been nudged yet.
pub(crate) fn select_nudges<'a>(
    views: &'a [TaskView],
    label: &str,
    already: &BTreeSet<u64>,
) -> Vec<&'a TaskView> {
    views
        .iter()
        .filter(|v| {
            v.status == TaskStatus::Ready
                && v.assignee.as_deref() == Some(label)
                && !already.contains(&v.task_id)
        })
        .collect()
}

/// Render the block reason shown to the agent.
pub(crate) fn render_nudge(tasks: &[&TaskView]) -> String {
    let mut lines = vec![format!(
        "edda task rail: {} task(s) ready and assigned to you:",
        tasks.len()
    )];
    for t in tasks {
        lines.push(format!("  #{} {}", t.task_id, t.title));
    }
    lines.push(
        "Pick one up: `edda task show <id>` then `edda task start <id>`; finish with \
         `edda task done <id> --receipt \"what was verifiably done\"`. \
         Not yours to do now? Just finish your turn again."
            .to_string(),
    );
    lines.join("\n")
}

fn watermark_path(project_id: &str, session_id: &str) -> PathBuf {
    edda_store::project_dir(project_id)
        .join("state")
        .join(format!("task_nudge.{session_id}.json"))
}

/// Task ids already nudged in this session.
pub(crate) fn read_watermark(project_id: &str, session_id: &str) -> BTreeSet<u64> {
    fs::read_to_string(watermark_path(project_id, session_id))
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<u64>>(&s).ok())
        .map(|v| v.into_iter().collect())
        .unwrap_or_default()
}

/// Persist the nudged task ids (best-effort; errors are swallowed).
pub(crate) fn write_watermark(project_id: &str, session_id: &str, ids: &BTreeSet<u64>) {
    let path = watermark_path(project_id, session_id);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(&ids.iter().collect::<Vec<_>>()) {
        let _ = fs::write(&path, json);
    }
}

/// Stop-hook entrypoint. Returns a block-decision nudge when there are
/// newly-ready tasks assigned to this session's label, empty otherwise.
pub(crate) fn dispatch_stop(
    project_id: &str,
    session_id: &str,
    cwd: &str,
    stop_hook_active: bool,
) -> HookResult {
    if stop_hook_active || session_id.is_empty() || cwd.is_empty() {
        return HookResult::empty();
    }
    // Cheap gate: only fire inside an initialized edda workspace.
    let Some(root) = edda_ledger::EddaPaths::find_root(&PathBuf::from(cwd)) else {
        return HookResult::empty();
    };
    // Assignment matching is by session label (heartbeat).
    let Some(hb) = crate::peers::read_heartbeat(project_id, session_id) else {
        return HookResult::empty();
    };
    if hb.label.is_empty() {
        return HookResult::empty();
    }
    let Ok(ledger) = edda_ledger::Ledger::open(&root) else {
        return HookResult::empty();
    };
    let Ok(views) = ledger.task_views() else {
        return HookResult::empty();
    };

    let already = read_watermark(project_id, session_id);
    let picks = select_nudges(&views, &hb.label, &already);
    if picks.is_empty() {
        return HookResult::empty();
    }

    // Mark before emitting: a task nudges at most once per session even if
    // the agent ignores it — the rail's transport is the ledger, not this.
    let mut marked = already;
    marked.extend(picks.iter().map(|t| t.task_id));
    write_watermark(project_id, session_id, &marked);

    let payload = serde_json::json!({
        "decision": "block",
        "reason": render_nudge(&picks),
    });
    HookResult::output(payload.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_view(task_id: u64, status: TaskStatus, assignee: Option<&str>) -> TaskView {
        TaskView {
            task_id,
            title: format!("task {task_id}"),
            assignee: assignee.map(String::from),
            agent_kind: None,
            after: Vec::new(),
            plan_id: None,
            work_unit_ref: None,
            brief_ref: None,
            idempotency_key: None,
            status,
            attempts: 0,
            receipt: None,
            evidence_paths: Vec::new(),
            acp_session_id: None,
            failure_reason: None,
            created_ts: "2026-07-14T00:00:00Z".into(),
            updated_ts: "2026-07-14T00:00:00Z".into(),
            created_event_id: format!("evt_{task_id}"),
        }
    }

    #[test]
    fn select_nudges_filters_ready_assigned_unmarked() {
        let views = vec![
            mk_view(1, TaskStatus::Ready, Some("tester")),
            mk_view(2, TaskStatus::Ready, Some("writer")), // other assignee
            mk_view(3, TaskStatus::Blocked, Some("tester")), // not ready
            mk_view(4, TaskStatus::Running, Some("tester")), // already picked up
            mk_view(5, TaskStatus::Ready, None),           // unassigned
        ];
        let picks = select_nudges(&views, "tester", &BTreeSet::new());
        let ids: Vec<u64> = picks.iter().map(|t| t.task_id).collect();
        assert_eq!(ids, vec![1]);
    }

    #[test]
    fn select_nudges_skips_watermarked() {
        let views = vec![
            mk_view(1, TaskStatus::Ready, Some("tester")),
            mk_view(2, TaskStatus::Ready, Some("tester")),
        ];
        let already: BTreeSet<u64> = [1].into_iter().collect();
        let picks = select_nudges(&views, "tester", &already);
        let ids: Vec<u64> = picks.iter().map(|t| t.task_id).collect();
        assert_eq!(ids, vec![2]);
    }

    #[test]
    fn render_nudge_mentions_ids_and_verbs() {
        let a = mk_view(7, TaskStatus::Ready, Some("tester"));
        let msg = render_nudge(&[&a]);
        assert!(msg.contains("#7"), "message should name the task: {msg}");
        assert!(msg.contains("edda task start"), "message should teach the verb: {msg}");
        assert!(msg.contains("edda task done"), "message should teach done: {msg}");
        assert!(msg.contains("--receipt"), "receipt culture rides along: {msg}");
    }

    #[test]
    fn stop_hook_nudges_once_then_stays_quiet() {
        // Real workspace + ledger + heartbeat, driven through the hook entrypoint.
        let ws = std::env::temp_dir().join(format!("edda_stopnudge_{}", std::process::id()));
        let _ = fs::remove_dir_all(&ws);
        fs::create_dir_all(&ws).unwrap();
        edda_ledger::Ledger::ensure_initialized(&ws).unwrap();
        {
            let ledger = edda_ledger::Ledger::open(&ws).unwrap();
            let ev =
                edda_core::event::new_task_created_event(&edda_core::event::TaskCreatedParams {
                    branch: "main",
                    parent_hash: None,
                    task_id: 1,
                    title: "review the diff",
                    assignee: Some("rail-tester"),
                    agent_kind: None,
                    after: &[],
                    plan_id: None,
                    work_unit_ref: None,
                    brief_ref: None,
                    idempotency_key: None,
                })
                .unwrap();
            ledger.append_event(&ev).unwrap();
        }

        let cwd = ws.to_string_lossy().replace('\\', "/");
        let project_id = crate::parse::resolve_project_id(&cwd);
        let session_id = "s-stopnudge";
        crate::peers::write_heartbeat_minimal(&project_id, session_id, "rail-tester", &cwd);

        let stdin = format!(
            r#"{{"session_id":"{session_id}","hook_event_name":"Stop","cwd":"{cwd}","transcript_path":"","permission_mode":"default"}}"#
        );
        let first = crate::dispatch::hook_entrypoint_from_stdin(&stdin).unwrap();
        let out = first.stdout.expect("first Stop should nudge");
        let json: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(json["decision"], "block");
        assert!(json["reason"].as_str().unwrap().contains("#1"));

        // Same session stops again: watermark keeps it quiet.
        let second = crate::dispatch::hook_entrypoint_from_stdin(&stdin).unwrap();
        assert!(second.stdout.is_none(), "second Stop must not nudge again");

        // A turn where a stop hook already fired must never be blocked again.
        crate::peers::write_heartbeat_minimal(&project_id, "other-s", "rail-tester", &cwd);
        let active = format!(
            r#"{{"session_id":"other-s","hook_event_name":"Stop","cwd":"{cwd}","transcript_path":"","permission_mode":"default","stop_hook_active":true}}"#
        );
        let third = crate::dispatch::hook_entrypoint_from_stdin(&active).unwrap();
        assert!(third.stdout.is_none(), "stop_hook_active must suppress the nudge");

        let _ = fs::remove_dir_all(edda_store::project_dir(&project_id));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn watermark_roundtrip() {
        let project_id = format!("test_task_nudge_{}", std::process::id());
        let session_id = "s-watermark";
        assert!(read_watermark(&project_id, session_id).is_empty());

        let ids: BTreeSet<u64> = [3, 5].into_iter().collect();
        write_watermark(&project_id, session_id, &ids);
        assert_eq!(read_watermark(&project_id, session_id), ids);

        let _ = fs::remove_dir_all(edda_store::project_dir(&project_id));
    }
}
