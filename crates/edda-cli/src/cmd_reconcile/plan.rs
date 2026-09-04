use anyhow::Context;
use edda_ledger::tasks::{TaskStatus, TaskView};
use edda_ledger::{Ledger, TaskLease};
use std::path::Path;

use super::runner::{
    acquire_workspace_lock, append_failed, append_requeued, append_started, clock_now,
    ensure_attempt_worktree, replace_lease, task_view,
};
use super::{PersistOutcome, ReconcileConfig, RunnerPlan};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ReconcileAction {
    Start {
        task_id: u64,
        attempt: u32,
    },
    Resume {
        task_id: u64,
        attempt: u32,
        session_id: String,
    },
    Requeue {
        task_id: u64,
        next_attempt: u32,
        reason: String,
    },
    Fail {
        task_id: u64,
        reason: String,
    },
}

pub(super) fn plan_actions(
    views: &[TaskView],
    leases: &[TaskLease],
    live_claims: &[Vec<String>],
    now: &str,
    max_workers: usize,
    max_attempts: u32,
) -> Vec<ReconcileAction> {
    let mut ordered: Vec<&TaskView> = views.iter().collect();
    ordered.sort_by_key(|view| view.task_id);
    let lease_for = |task_id| leases.iter().find(|lease| lease.task_id == task_id);
    let is_live = |view: &TaskView| {
        lease_for(view.task_id)
            .is_some_and(|lease| lease.attempt == view.attempts && lease.expires_at.as_str() > now)
    };
    let mut occupied: Vec<Vec<String>> = live_claims
        .iter()
        .filter(|paths| !paths.is_empty())
        .cloned()
        .collect();
    occupied.extend(
        ordered
            .iter()
            .filter(|view| view.status == TaskStatus::Running && is_live(view))
            .map(|view| occupied_scope(&view.scope_paths)),
    );
    let mut slots = max_workers.saturating_sub(
        ordered
            .iter()
            .filter(|view| view.status == TaskStatus::Running && is_live(view))
            .count(),
    );
    let mut actions = Vec::new();

    for view in ordered {
        match view.status {
            TaskStatus::Running if is_live(view) => continue,
            TaskStatus::Running => {
                if view.attempts >= max_attempts {
                    actions.push(ReconcileAction::Fail {
                        task_id: view.task_id,
                        reason: "retry-cap-exhausted".into(),
                    });
                } else if slots > 0 && !conflicts(&view.scope_paths, &occupied) {
                    slots -= 1;
                    occupied.push(occupied_scope(&view.scope_paths));
                    let resume = (view.session_agent_kind.as_deref() == Some("codex")
                        && view.session_attempt == Some(view.attempts))
                    .then(|| view.session_id.clone())
                    .flatten();
                    if let Some(session_id) = resume {
                        actions.push(ReconcileAction::Resume {
                            task_id: view.task_id,
                            attempt: view.attempts,
                            session_id,
                        });
                    } else {
                        actions.push(ReconcileAction::Requeue {
                            task_id: view.task_id,
                            next_attempt: view.attempts + 1,
                            reason: "expired-without-session".into(),
                        });
                    }
                }
            }
            TaskStatus::Ready if slots > 0 && !conflicts(&view.scope_paths, &occupied) => {
                slots -= 1;
                occupied.push(occupied_scope(&view.scope_paths));
                actions.push(ReconcileAction::Start {
                    task_id: view.task_id,
                    attempt: view.attempts + 1,
                });
            }
            TaskStatus::Failed
                if view.attempts < max_attempts
                    && slots > 0
                    && !conflicts(&view.scope_paths, &occupied) =>
            {
                slots -= 1;
                occupied.push(occupied_scope(&view.scope_paths));
                actions.push(ReconcileAction::Start {
                    task_id: view.task_id,
                    attempt: view.attempts + 1,
                });
            }
            _ => {}
        }
    }
    actions
}

pub(super) fn conflicts(scope: &[String], occupied: &[Vec<String>]) -> bool {
    (scope.is_empty() && !occupied.is_empty())
        || occupied.iter().any(|other| {
            other.is_empty()
                || scope
                    .iter()
                    .any(|path| other.iter().any(|other| paths_overlap(path, other)))
        })
}

pub(super) fn occupied_scope(scope: &[String]) -> Vec<String> {
    if scope.is_empty() {
        vec![String::new()]
    } else {
        scope.to_vec()
    }
}

pub(super) fn paths_overlap(left: &str, right: &str) -> bool {
    let Some(left) = static_prefix(left) else {
        return true;
    };
    let Some(right) = static_prefix(right) else {
        return true;
    };
    left.value == right.value
        || left
            .value
            .strip_prefix(&right.value)
            .is_some_and(|rest| rest.starts_with('/'))
        || right
            .value
            .strip_prefix(&left.value)
            .is_some_and(|rest| rest.starts_with('/'))
        || (left.glob && right.value.starts_with(&left.value))
        || (right.glob && left.value.starts_with(&right.value))
}

pub(super) struct StaticPrefix {
    pub(super) value: String,
    pub(super) glob: bool,
}

pub(super) fn static_prefix(path: &str) -> Option<StaticPrefix> {
    let normalized_path = path.replace('\\', "/");
    let mut parts = Vec::new();
    for part in normalized_path.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            _ => parts.push(part),
        }
    }
    let normalized = parts.join("/");
    let end = normalized
        .find(['*', '?', '[', '{'])
        .unwrap_or(normalized.len());
    let prefix = normalized[..end].trim_end_matches('/');
    (!prefix.is_empty()).then_some(StaticPrefix {
        value: prefix.to_string(),
        glob: end < normalized.len(),
    })
}

#[allow(clippy::too_many_lines)] // 153 lines at #779; split tracked in #778
pub(super) fn persist_reconciliation(
    repo_root: &Path,
    config: &ReconcileConfig,
) -> anyhow::Result<PersistOutcome> {
    let ledger = Ledger::open(repo_root)?;
    let lock = acquire_workspace_lock(&ledger.paths)?;
    let views = ledger.task_views()?;
    let leases: Vec<TaskLease> = views
        .iter()
        .filter_map(|view| ledger.task_lease(view.task_id).transpose())
        .collect::<anyhow::Result<_>>()?;
    let project_id = edda_store::project_id(repo_root);
    let claims = edda_bridge_claude::peers::discover_active_peers(&project_id, "")
        .into_iter()
        .map(|peer| peer.claimed_paths)
        .collect::<Vec<_>>();
    let now = clock_now();
    let actions = plan_actions(
        &views,
        &leases,
        &claims,
        &now,
        config.max_workers,
        config.max_attempts,
    );
    // Git preparation happens for the complete batch before the first immutable
    // dispatch event. A later refusal must not strand an earlier RUNNING task.
    let prepared = actions
        .iter()
        .filter_map(|action| match action {
            ReconcileAction::Start { task_id, attempt } => Some((*task_id, *attempt, false)),
            ReconcileAction::Resume {
                task_id, attempt, ..
            } => Some((*task_id, *attempt, true)),
            ReconcileAction::Requeue {
                task_id,
                next_attempt,
                ..
            } => Some((*task_id, *next_attempt, false)),
            ReconcileAction::Fail { .. } => None,
        })
        .map(|(task_id, attempt, resume)| {
            let task = task_view(&views, task_id)?.clone();
            let worktree = ensure_attempt_worktree(repo_root, &task, attempt, resume)?;
            Ok((task_id, task, attempt, worktree))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let changed = !actions.is_empty();
    let mut plans = Vec::new();
    let mut errors = Vec::new();

    for action in actions {
        let result = (|| -> anyhow::Result<Option<RunnerPlan>> {
            Ok(match action {
                ReconcileAction::Start { task_id, attempt } => {
                    let (_, task, _, worktree) = prepared
                        .iter()
                        .find(|(id, _, prepared_attempt, _)| {
                            *id == task_id && *prepared_attempt == attempt
                        })
                        .context("prepared reconciliation task disappeared")?
                        .clone();
                    if task.status == TaskStatus::Failed {
                        append_requeued(&ledger, task_id, attempt)?;
                    }
                    replace_lease(&ledger, task_id, attempt, config.lease_ttl_s)?;
                    if let Err(error) =
                        append_started(&ledger, task_id, attempt, config.lease_ttl_s)
                    {
                        let _ = ledger.delete_task_lease(task_id, attempt);
                        return Err(error);
                    }
                    Some(RunnerPlan {
                        task,
                        attempt,
                        worktree,
                    })
                }
                ReconcileAction::Resume {
                    task_id,
                    attempt,
                    session_id,
                } => {
                    let (_, task, _, worktree) = prepared
                        .iter()
                        .find(|(id, _, prepared_attempt, _)| {
                            *id == task_id && *prepared_attempt == attempt
                        })
                        .context("prepared reconciliation task disappeared")?
                        .clone();
                    replace_lease(&ledger, task_id, attempt, config.lease_ttl_s)?;
                    let _ = session_id;
                    Some(RunnerPlan {
                        task,
                        attempt,
                        worktree,
                    })
                }
                ReconcileAction::Requeue {
                    task_id,
                    next_attempt,
                    ..
                } => {
                    let (_, task, _, worktree) = prepared
                        .iter()
                        .find(|(id, _, prepared_attempt, _)| {
                            *id == task_id && *prepared_attempt == next_attempt
                        })
                        .context("prepared reconciliation task disappeared")?
                        .clone();
                    append_requeued(&ledger, task_id, next_attempt)?;
                    replace_lease(&ledger, task_id, next_attempt, config.lease_ttl_s)?;
                    if let Err(error) =
                        append_started(&ledger, task_id, next_attempt, config.lease_ttl_s)
                    {
                        let _ = ledger.delete_task_lease(task_id, next_attempt);
                        return Err(error);
                    }
                    Some(RunnerPlan {
                        task,
                        attempt: next_attempt,
                        worktree,
                    })
                }
                ReconcileAction::Fail { task_id, reason } => {
                    append_failed(&ledger, task_id, &reason)?;
                    let attempt = task_view(&views, task_id)?.attempts;
                    let _ = ledger.delete_task_lease(task_id, attempt)?;
                    None
                }
            })
        })();
        match result {
            Ok(Some(plan)) => plans.push(plan),
            Ok(None) => {}
            Err(error) => errors.push(format!("task action persistence failed: {error:#}")),
        }
    }
    if changed {
        let branch = ledger.head_branch()?;
        let _ = edda_derive::rebuild_branch(&ledger, &branch);
    }
    drop(lock);
    Ok(PersistOutcome { plans, errors })
}

// A reconciler legitimately holds the workspace lock for its whole critical
// section (git batch prep incl. `git worktree add`, SQLite appends,
// `rebuild_branch`), which measured 3-5 s on Windows under full-suite load.
// A concurrent reconciler must wait out that section rather than bail early;
// a fixed small retry budget here was the root cause of the GH-524 flake.
pub(super) const WORKSPACE_LOCK_WAIT_BUDGET: std::time::Duration =
    std::time::Duration::from_secs(30);
