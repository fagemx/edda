//! Shared task-rail write actions (GH-611).
//!
//! Extracted verbatim from `edda-cli/src/cmd_task.rs` so the MCP server and
//! the CLI drive the same validated state machine (start/done pairing,
//! blocked-dependency checks, receipt requirement, idempotency dedup) — the
//! state rules live here, never in a second implementation.
//!
//! `rebuild_branch` is injected because derived-view rebuilding lives in
//! `edda-derive`, which depends on this crate; the CLI and MCP callers pass
//! `|ledger, branch| { let _ = edda_derive::rebuild_branch(ledger, branch); }`.
//! Notification dispatch (a presentation concern) stays in the callers.

use crate::lock::WorkspaceLock;
use crate::tasks::{self, TaskStatus, TaskView};
use crate::Ledger;
use edda_core::event::{
    new_task_created_event, new_task_done_event, new_task_failed_event, new_task_started_event,
    TaskCreatedParams,
};
use std::path::Path;

/// Arguments for creating a task (mirrors `task.created` payload).
pub struct NewTaskArgs<'a> {
    pub title: &'a str,
    pub assignee: Option<&'a str>,
    pub agent_kind: Option<&'a str>,
    pub after: &'a [u64],
    pub plan: Option<&'a str>,
    pub work_unit: Option<&'a str>,
    pub brief: Option<&'a str>,
    pub idempotency_key: Option<&'a str>,
    pub scope_paths: &'a [String],
}

#[derive(Debug)]
pub struct NewOutcome {
    pub task_id: u64,
    pub status: TaskStatus,
    /// True when an existing task with the same idempotency key was reused.
    pub deduped: bool,
}

#[derive(Debug)]
pub struct StartOutcome {
    pub attempt: u32,
}

#[derive(Debug)]
pub struct DoneOutcome {
    /// Successors unlocked by this completion: (task_id, title, assignee).
    pub unlocked: Vec<(u64, String, Option<String>)>,
}

pub fn find_view(views: &[TaskView], id: u64) -> anyhow::Result<&TaskView> {
    views
        .iter()
        .find(|v| v.task_id == id)
        .ok_or_else(|| anyhow::anyhow!("task #{id} not found — see `edda task list`"))
}

pub fn new_task(
    repo_root: &Path,
    args: &NewTaskArgs<'_>,
    rebuild_branch: &dyn Fn(&Ledger, &str),
) -> anyhow::Result<NewOutcome> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let views = ledger.task_views()?;
    if let Some(key) = args.idempotency_key {
        if let Some(existing) = tasks::find_by_idempotency_key(&views, key) {
            return Ok(NewOutcome {
                task_id: existing.task_id,
                status: existing.status,
                deduped: true,
            });
        }
    }

    let task_id = tasks::next_task_id(&views);
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    let event = new_task_created_event(&TaskCreatedParams {
        branch: &branch,
        parent_hash: parent_hash.as_deref(),
        task_id,
        title: args.title,
        assignee: args.assignee,
        agent_kind: args.agent_kind,
        after: args.after,
        plan_id: args.plan,
        work_unit_ref: args.work_unit,
        brief_ref: args.brief,
        idempotency_key: args.idempotency_key,
        scope_paths: args.scope_paths,
    })?;
    ledger.append_event(&event)?;
    rebuild_branch(&ledger, &branch);

    let views = ledger.task_views()?;
    let status = find_view(&views, task_id)?.status;
    drop(_lock);
    Ok(NewOutcome {
        task_id,
        status,
        deduped: false,
    })
}

pub fn start_task(
    repo_root: &Path,
    id: u64,
    lease_ttl_s: u64,
    rebuild_branch: &dyn Fn(&Ledger, &str),
) -> anyhow::Result<StartOutcome> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let views = ledger.task_views()?;
    let v = find_view(&views, id)?;
    match v.status {
        TaskStatus::Done => anyhow::bail!("task #{id} is already done"),
        TaskStatus::Running => {
            anyhow::bail!("task #{id} is already running (attempt {})", v.attempts)
        }
        TaskStatus::Blocked => {
            let unmet: Vec<String> = v
                .after
                .iter()
                .filter(|d| {
                    views
                        .iter()
                        .find(|x| x.task_id == **d)
                        .is_none_or(|x| x.status != TaskStatus::Done)
                })
                .map(|d| format!("#{d}"))
                .collect();
            anyhow::bail!("task #{id} is blocked — unmet deps: {}", unmet.join(", "));
        }
        // Ready = normal start; Failed = retry.
        TaskStatus::Ready | TaskStatus::Failed => {}
    }
    let attempt = v.attempts + 1;

    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    let event = new_task_started_event(&branch, parent_hash.as_deref(), id, lease_ttl_s, attempt)?;
    ledger.append_event(&event)?;
    rebuild_branch(&ledger, &branch);

    Ok(StartOutcome { attempt })
}

pub fn done_task(
    repo_root: &Path,
    id: u64,
    receipt: &str,
    evidence_paths: &[String],
    rebuild_branch: &dyn Fn(&Ledger, &str),
) -> anyhow::Result<DoneOutcome> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let views = ledger.task_views()?;
    let v = find_view(&views, id)?;
    let correction = v.status == TaskStatus::Done;
    match v.status {
        TaskStatus::Running | TaskStatus::Done => {}
        TaskStatus::Ready if v.attempts > 0 => {}
        TaskStatus::Ready | TaskStatus::Blocked => anyhow::bail!(
            "task #{id} has not been started — run `edda task start {id}` first \
             (start/done pairs are what make the ledger replayable)"
        ),
        TaskStatus::Failed => {
            anyhow::bail!("task #{id} is failed — run `edda task start {id}` to retry, then done")
        }
    }
    if receipt.trim().is_empty() {
        anyhow::bail!(
            "a completion without a receipt does not exist — pass --receipt with real content"
        );
    }

    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    let event = new_task_done_event(&branch, parent_hash.as_deref(), id, receipt, evidence_paths)?;
    ledger.append_event(&event)?;
    rebuild_branch(&ledger, &branch);

    let unlocked = if correction {
        Vec::new()
    } else {
        let after_views = ledger.task_views()?;
        tasks::ready_successors_of(&after_views, id)
            .into_iter()
            .map(|s| (s.task_id, s.title.clone(), s.assignee.clone()))
            .collect()
    };
    Ok(DoneOutcome { unlocked })
}

pub fn fail_task(
    repo_root: &Path,
    id: u64,
    reason: &str,
    rebuild_branch: &dyn Fn(&Ledger, &str),
) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let views = ledger.task_views()?;
    let v = find_view(&views, id)?;
    if v.status != TaskStatus::Running {
        anyhow::bail!(
            "task #{id} is not running ({}) — only a running task can fail",
            v.status
        );
    }

    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    let event = new_task_failed_event(&branch, parent_hash.as_deref(), id, reason)?;
    ledger.append_event(&event)?;
    rebuild_branch(&ledger, &branch);

    Ok(())
}
