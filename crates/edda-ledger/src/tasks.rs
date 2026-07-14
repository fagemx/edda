//! Task rail projection — derive task state from `task.*` ledger events.
//!
//! State is never stored; it is always folded from the hash chain
//! (TASK_RAIL_V1 §2: truth lives in the ledger, views are derived).
//! Readiness is a projection: a task is ready when it exists, is not
//! running/done/failed, and every task in its `after` list is done.
//!
//! **Where invariants live**: the fold applies events as facts and never
//! rejects them — transition legality (start only when ready, done only
//! when running, receipts non-empty) is enforced at the write path
//! (`edda task` verbs, under the workspace lock). This split is deliberate:
//! a `task.done` arriving after a requeue (the original executor finishing
//! late) is a real completion and must count; a duplicate `task.done` is a
//! correction whose newest receipt wins. Done is status-terminal — later
//! started/failed/requeued events never un-done a task.

use edda_core::types::Event;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Derived status of a rail task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// Created, but at least one `after` dependency is not done (or missing).
    Blocked,
    /// Created and every `after` dependency is done — can be started.
    Ready,
    /// Started (lease taken) and not yet done/failed.
    Running,
    Done,
    Failed,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            TaskStatus::Blocked => "blocked",
            TaskStatus::Ready => "ready",
            TaskStatus::Running => "running",
            TaskStatus::Done => "done",
            TaskStatus::Failed => "failed",
        };
        write!(f, "{s}")
    }
}

/// Projected view of one rail task, folded from its `task.*` events.
#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    pub task_id: u64,
    pub title: String,
    pub assignee: Option<String>,
    pub agent_kind: Option<String>,
    pub after: Vec<u64>,
    pub plan_id: Option<String>,
    pub work_unit_ref: Option<String>,
    pub brief_ref: Option<String>,
    pub idempotency_key: Option<String>,
    pub status: TaskStatus,
    /// Number of `task.started` events (lease acquisitions).
    pub attempts: u32,
    pub receipt: Option<String>,
    pub evidence_paths: Vec<String>,
    /// Last recorded ACP session id (resume key).
    pub acp_session_id: Option<String>,
    /// Last `task.failed` reason, kept across requeue for operator context.
    pub failure_reason: Option<String>,
    pub created_ts: String,
    /// Timestamp of the most recent `task.*` event for this task.
    pub updated_ts: String,
    pub created_event_id: String,
}

fn opt_str(payload: &serde_json::Value, key: &str) -> Option<String> {
    payload.get(key).and_then(|v| v.as_str()).map(String::from)
}

/// Fold `task.*` events (in ledger insertion order) into task views,
/// sorted by task_id. Non-task events in the input are ignored.
pub fn project_tasks(events: &[Event]) -> Vec<TaskView> {
    let mut map: BTreeMap<u64, TaskView> = BTreeMap::new();

    for event in events {
        let Some(task_id) = event.payload.get("task_id").and_then(|v| v.as_u64()) else {
            continue;
        };
        match event.event_type.as_str() {
            "task.created" => {
                // First creation wins; a duplicate id never overwrites history.
                if map.contains_key(&task_id) {
                    continue;
                }
                let p = &event.payload;
                let after: Vec<u64> = p
                    .get("after")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_u64()).collect())
                    .unwrap_or_default();
                map.insert(
                    task_id,
                    TaskView {
                        task_id,
                        title: opt_str(p, "title").unwrap_or_default(),
                        assignee: opt_str(p, "assignee"),
                        agent_kind: opt_str(p, "agent_kind"),
                        after,
                        plan_id: opt_str(p, "plan_id"),
                        work_unit_ref: opt_str(p, "work_unit_ref"),
                        brief_ref: opt_str(p, "brief_ref"),
                        idempotency_key: opt_str(p, "idempotency_key"),
                        // Readiness is resolved in the final pass below.
                        status: TaskStatus::Blocked,
                        attempts: 0,
                        receipt: None,
                        evidence_paths: Vec::new(),
                        acp_session_id: None,
                        failure_reason: None,
                        created_ts: event.ts.clone(),
                        updated_ts: event.ts.clone(),
                        created_event_id: event.event_id.clone(),
                    },
                );
            }
            "task.started" => {
                if let Some(v) = map.get_mut(&task_id) {
                    v.attempts += 1;
                    if v.status != TaskStatus::Done {
                        v.status = TaskStatus::Running;
                    }
                    v.updated_ts = event.ts.clone();
                }
            }
            "task.session" => {
                if let Some(v) = map.get_mut(&task_id) {
                    v.acp_session_id = opt_str(&event.payload, "acp_session_id");
                    v.updated_ts = event.ts.clone();
                }
            }
            "task.done" => {
                if let Some(v) = map.get_mut(&task_id) {
                    v.status = TaskStatus::Done;
                    v.receipt = opt_str(&event.payload, "receipt");
                    v.evidence_paths = event
                        .payload
                        .get("evidence_paths")
                        .and_then(|e| e.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    v.updated_ts = event.ts.clone();
                }
            }
            "task.failed" => {
                if let Some(v) = map.get_mut(&task_id) {
                    if v.status != TaskStatus::Done {
                        v.status = TaskStatus::Failed;
                    }
                    v.failure_reason = opt_str(&event.payload, "reason");
                    v.updated_ts = event.ts.clone();
                }
            }
            "task.requeued" => {
                if let Some(v) = map.get_mut(&task_id) {
                    if v.status != TaskStatus::Done {
                        // Back to eligible; final pass decides Blocked vs Ready.
                        v.status = TaskStatus::Blocked;
                    }
                    v.updated_ts = event.ts.clone();
                }
            }
            _ => {}
        }
    }

    // Final pass: readiness is a projection over done-ness of dependencies.
    // A dep pointing at a task that doesn't exist keeps its dependent blocked.
    let done: BTreeSet<u64> = map
        .values()
        .filter(|v| v.status == TaskStatus::Done)
        .map(|v| v.task_id)
        .collect();
    for v in map.values_mut() {
        if v.status == TaskStatus::Blocked && v.after.iter().all(|d| done.contains(d)) {
            v.status = TaskStatus::Ready;
        }
    }

    map.into_values().collect()
}

/// Next unused task id: max created id + 1, or 1 when no tasks exist.
pub fn next_task_id(views: &[TaskView]) -> u64 {
    views.iter().map(|v| v.task_id).max().map_or(1, |m| m + 1)
}

/// Find a task created with the given idempotency key.
pub fn find_by_idempotency_key<'a>(views: &'a [TaskView], key: &str) -> Option<&'a TaskView> {
    views
        .iter()
        .find(|v| v.idempotency_key.as_deref() == Some(key))
}

/// Tasks that are now ready and list `done_id` as a dependency —
/// the successors unlocked by that completion (§2 L1: done ⇒ successor ready).
pub fn ready_successors_of(views: &[TaskView], done_id: u64) -> Vec<&TaskView> {
    views
        .iter()
        .filter(|v| v.status == TaskStatus::Ready && v.after.contains(&done_id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use edda_core::event::{
        new_task_created_event, new_task_done_event, new_task_failed_event,
        new_task_requeued_event, new_task_session_event, new_task_started_event, TaskCreatedParams,
    };

    fn created(task_id: u64, title: &str, after: &[u64]) -> Event {
        new_task_created_event(&TaskCreatedParams {
            branch: "main",
            parent_hash: None,
            task_id,
            title,
            assignee: Some("tester"),
            agent_kind: None,
            after,
            plan_id: None,
            work_unit_ref: None,
            brief_ref: None,
            idempotency_key: None,
        })
        .unwrap()
    }

    fn created_with_key(task_id: u64, title: &str, key: &str) -> Event {
        new_task_created_event(&TaskCreatedParams {
            branch: "main",
            parent_hash: None,
            task_id,
            title,
            assignee: None,
            agent_kind: None,
            after: &[],
            plan_id: None,
            work_unit_ref: None,
            brief_ref: None,
            idempotency_key: Some(key),
        })
        .unwrap()
    }

    fn view(views: &[TaskView], id: u64) -> &TaskView {
        views.iter().find(|v| v.task_id == id).unwrap()
    }

    #[test]
    fn created_task_with_no_deps_is_ready() {
        let events = vec![created(1, "solo", &[])];
        let views = project_tasks(&events);
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].status, TaskStatus::Ready);
        assert_eq!(views[0].title, "solo");
        assert_eq!(views[0].assignee.as_deref(), Some("tester"));
        assert_eq!(views[0].attempts, 0);
    }

    #[test]
    fn created_task_with_unmet_deps_is_blocked() {
        let events = vec![created(1, "build", &[]), created(2, "test", &[1])];
        let views = project_tasks(&events);
        assert_eq!(view(&views, 2).status, TaskStatus::Blocked);
    }

    #[test]
    fn done_dependency_unlocks_successor() {
        let events = vec![
            created(1, "build", &[]),
            created(2, "test", &[1]),
            new_task_done_event("main", None, 1, "built ok", &[]).unwrap(),
        ];
        let views = project_tasks(&events);
        assert_eq!(view(&views, 1).status, TaskStatus::Done);
        assert_eq!(view(&views, 2).status, TaskStatus::Ready);
    }

    #[test]
    fn dep_on_missing_task_stays_blocked() {
        let events = vec![created(2, "test", &[99])];
        let views = project_tasks(&events);
        assert_eq!(view(&views, 2).status, TaskStatus::Blocked);
    }

    #[test]
    fn started_task_is_running_and_counts_attempts() {
        let events = vec![
            created(1, "build", &[]),
            new_task_started_event("main", None, 1, 3600, 1).unwrap(),
        ];
        let views = project_tasks(&events);
        assert_eq!(view(&views, 1).status, TaskStatus::Running);
        assert_eq!(view(&views, 1).attempts, 1);
    }

    #[test]
    fn done_task_has_receipt_and_evidence() {
        let evidence = vec!["dist/out.md".to_string()];
        let events = vec![
            created(1, "build", &[]),
            new_task_started_event("main", None, 1, 3600, 1).unwrap(),
            new_task_done_event("main", None, 1, "tests green", &evidence).unwrap(),
        ];
        let views = project_tasks(&events);
        let v = view(&views, 1);
        assert_eq!(v.status, TaskStatus::Done);
        assert_eq!(v.receipt.as_deref(), Some("tests green"));
        assert_eq!(v.evidence_paths, vec!["dist/out.md".to_string()]);
    }

    #[test]
    fn failed_task_keeps_reason() {
        let events = vec![
            created(1, "build", &[]),
            new_task_started_event("main", None, 1, 3600, 1).unwrap(),
            new_task_failed_event("main", None, 1, "ended-without-receipt").unwrap(),
        ];
        let views = project_tasks(&events);
        let v = view(&views, 1);
        assert_eq!(v.status, TaskStatus::Failed);
        assert_eq!(v.failure_reason.as_deref(), Some("ended-without-receipt"));
    }

    #[test]
    fn failed_then_restarted_is_running_with_two_attempts() {
        let events = vec![
            created(1, "build", &[]),
            new_task_started_event("main", None, 1, 3600, 1).unwrap(),
            new_task_failed_event("main", None, 1, "crash").unwrap(),
            new_task_started_event("main", None, 1, 3600, 2).unwrap(),
        ];
        let views = project_tasks(&events);
        let v = view(&views, 1);
        assert_eq!(v.status, TaskStatus::Running);
        assert_eq!(v.attempts, 2);
    }

    #[test]
    fn requeued_after_failure_returns_to_ready() {
        let events = vec![
            created(1, "build", &[]),
            new_task_started_event("main", None, 1, 3600, 1).unwrap(),
            new_task_failed_event("main", None, 1, "crash").unwrap(),
            new_task_requeued_event("main", None, 1, 2).unwrap(),
        ];
        let views = project_tasks(&events);
        let v = view(&views, 1);
        assert_eq!(v.status, TaskStatus::Ready);
        // reason preserved for operator context even after requeue
        assert_eq!(v.failure_reason.as_deref(), Some("crash"));
    }

    #[test]
    fn requeued_with_unmet_deps_is_blocked_not_ready() {
        let events = vec![
            created(1, "build", &[]),
            created(2, "test", &[1]),
            new_task_started_event("main", None, 2, 3600, 1).unwrap(),
            new_task_failed_event("main", None, 2, "crash").unwrap(),
            new_task_requeued_event("main", None, 2, 2).unwrap(),
        ];
        let views = project_tasks(&events);
        assert_eq!(view(&views, 2).status, TaskStatus::Blocked);
    }

    #[test]
    fn late_done_after_requeue_is_accepted() {
        // P2 semantics: the original executor may finish after the
        // reconciler requeued the task — that completion is real.
        let events = vec![
            created(1, "build", &[]),
            new_task_started_event("main", None, 1, 60, 1).unwrap(),
            new_task_failed_event("main", None, 1, "lease expired").unwrap(),
            new_task_requeued_event("main", None, 1, 2).unwrap(),
            new_task_done_event("main", None, 1, "late but real", &[]).unwrap(),
        ];
        let views = project_tasks(&events);
        assert_eq!(view(&views, 1).status, TaskStatus::Done);
        assert_eq!(view(&views, 1).receipt.as_deref(), Some("late but real"));
    }

    #[test]
    fn dep_cycle_stays_blocked() {
        let events = vec![created(1, "a", &[2]), created(2, "b", &[1])];
        let views = project_tasks(&events);
        assert_eq!(view(&views, 1).status, TaskStatus::Blocked);
        assert_eq!(view(&views, 2).status, TaskStatus::Blocked);
    }

    #[test]
    fn done_is_status_terminal_and_receipt_corrections_win() {
        let events = vec![
            created(1, "build", &[]),
            new_task_started_event("main", None, 1, 60, 1).unwrap(),
            new_task_done_event("main", None, 1, "first receipt", &[]).unwrap(),
            // a later started cannot un-done…
            new_task_started_event("main", None, 1, 60, 2).unwrap(),
            // …but a corrected done updates the completion data
            new_task_done_event("main", None, 1, "corrected receipt", &[]).unwrap(),
        ];
        let views = project_tasks(&events);
        let v = view(&views, 1);
        assert_eq!(v.status, TaskStatus::Done);
        assert_eq!(v.receipt.as_deref(), Some("corrected receipt"));
        // attempts still count starts as facts
        assert_eq!(v.attempts, 2);
    }

    #[test]
    fn session_event_records_resume_key() {
        let events = vec![
            created(1, "build", &[]),
            new_task_started_event("main", None, 1, 3600, 1).unwrap(),
            new_task_session_event("main", None, 1, "sess-acp-42").unwrap(),
        ];
        let views = project_tasks(&events);
        assert_eq!(
            view(&views, 1).acp_session_id.as_deref(),
            Some("sess-acp-42")
        );
    }

    #[test]
    fn duplicate_created_first_wins() {
        let events = vec![created(1, "original", &[]), created(1, "impostor", &[])];
        let views = project_tasks(&events);
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].title, "original");
    }

    #[test]
    fn events_for_unknown_task_are_ignored() {
        let events = vec![new_task_done_event("main", None, 42, "orphan", &[]).unwrap()];
        let views = project_tasks(&events);
        assert!(views.is_empty());
    }

    #[test]
    fn non_task_events_are_ignored() {
        let note = edda_core::event::new_note_event("main", None, "user", "hello", &[]).unwrap();
        let events = vec![note, created(1, "build", &[])];
        let views = project_tasks(&events);
        assert_eq!(views.len(), 1);
    }

    #[test]
    fn views_sorted_by_task_id() {
        let events = vec![
            created(3, "c", &[]),
            created(1, "a", &[]),
            created(2, "b", &[]),
        ];
        let views = project_tasks(&events);
        let ids: Vec<u64> = views.iter().map(|v| v.task_id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn next_task_id_increments_from_max() {
        let events = vec![created(1, "a", &[]), created(7, "g", &[])];
        let views = project_tasks(&events);
        assert_eq!(next_task_id(&views), 8);
        assert_eq!(next_task_id(&[]), 1);
    }

    #[test]
    fn find_by_idempotency_key_matches() {
        let events = vec![
            created_with_key(1, "a", "wu1-build"),
            created_with_key(2, "b", "wu2-test"),
        ];
        let views = project_tasks(&events);
        assert_eq!(
            find_by_idempotency_key(&views, "wu2-test").map(|v| v.task_id),
            Some(2)
        );
        assert!(find_by_idempotency_key(&views, "nope").is_none());
    }

    #[test]
    fn ledger_task_views_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("edda_rail_views_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let ledger = crate::Ledger::open_or_init(&tmp).unwrap();
        let append = |mut event: Event| {
            event.parent_hash = ledger.last_event_hash().unwrap();
            edda_core::event::finalize_event(&mut event).unwrap();
            ledger.append_event(&event).unwrap();
        };
        let note =
            edda_core::event::new_note_event("main", None, "user", "not a task", &[]).unwrap();
        append(note);
        append(created(1, "build", &[]));
        append(created(2, "test", &[1]));
        append(new_task_done_event("main", None, 1, "built ok", &[]).unwrap());

        let views = ledger.task_views().unwrap();
        assert_eq!(views.len(), 2);
        assert_eq!(view(&views, 1).status, TaskStatus::Done);
        assert_eq!(view(&views, 2).status, TaskStatus::Ready);

        // the raw fold input excludes non-task events
        let events = ledger.task_events().unwrap();
        assert_eq!(events.len(), 3);
        assert!(events.iter().all(|e| e.event_type.starts_with("task.")));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn ready_successors_of_done_lists_unlocked() {
        let events = vec![
            created(1, "build", &[]),
            created(2, "test", &[1]),
            created(3, "docs", &[1, 2]),
            new_task_done_event("main", None, 1, "built", &[]).unwrap(),
        ];
        let views = project_tasks(&events);
        let unlocked: Vec<u64> = ready_successors_of(&views, 1)
            .iter()
            .map(|v| v.task_id)
            .collect();
        // task 2 unlocked; task 3 still waits on 2
        assert_eq!(unlocked, vec![2]);
    }
}
