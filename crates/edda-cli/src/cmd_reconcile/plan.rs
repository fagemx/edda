use anyhow::Context;
use edda_ledger::tasks::{TaskStatus, TaskView};
use edda_ledger::{Ledger, TaskLease};
use std::path::{Path, PathBuf};

use super::runner::{
    acquire_workspace_lock, append_failed, append_requeued, append_started, clock_now,
    ensure_attempt_worktree, renew_lease, replace_lease, task_view,
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

impl ReconcileAction {
    fn task_id(&self) -> u64 {
        match self {
            Self::Start { task_id, .. }
            | Self::Resume { task_id, .. }
            | Self::Requeue { task_id, .. }
            | Self::Fail { task_id, .. } => *task_id,
        }
    }
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
    // A live lease is the claim (GH-1047): it covers both a task already running
    // under its current attempt and one another reconciler has claimed but not
    // yet started, whose lease names the next attempt. Either way the task is
    // owned, so it yields no action and still holds a slot and its scope.
    // The cost of reading `Ready` and `Failed` this way is that a task whose
    // lease outlived its attempt — `finish_runner` appends `task.failed` and
    // only then deletes, so a failed delete leaves one behind — waits out the
    // lease instead of being retried on the next pass. Bounded by the TTL, and
    // the alternative is a second reconciler dispatching a claimed task.
    let is_live = |view: &TaskView| {
        matches!(
            view.status,
            TaskStatus::Running | TaskStatus::Ready | TaskStatus::Failed
        ) && lease_for(view.task_id)
            .is_some_and(|lease| lease.attempt >= view.attempts && lease.expires_at.as_str() > now)
    };
    let mut occupied: Vec<Vec<String>> = live_claims
        .iter()
        .filter(|paths| !paths.is_empty())
        .cloned()
        .collect();
    occupied.extend(
        ordered
            .iter()
            .filter(|view| is_live(view))
            .map(|view| occupied_scope(&view.scope_paths)),
    );
    let mut slots = max_workers.saturating_sub(ordered.iter().filter(|view| is_live(view)).count());
    let mut actions = Vec::new();

    for view in ordered {
        if is_live(view) {
            continue;
        }
        match view.status {
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

/// A planned action whose attempt is already reserved by a live claim lease.
struct ClaimedAction {
    action: ReconcileAction,
    task: TaskView,
    /// Attempt named by the claim lease — the attempt this action will start.
    attempt: u32,
    /// Owner written with the claim lease, unique to this claim. Phase 3 and
    /// the rollback both re-read it before writing, because a claim taken under
    /// an earlier lock hold can be gone by then.
    owner: String,
}

/// The claim phase 1 wrote for this action, if the ledger still holds it under
/// this reconciler's owner. One lease row exists per task (`upsert_task_lease`
/// conflicts on `task_id`), so a peer that took the task over replaced the
/// owner: a mismatch — or a missing row — means the claim is no longer ours,
/// neither to commit on nor to delete.
///
/// Under the workspace lock an owner match is decisive on its own, which is why
/// expiry is deliberately not part of the test. Every writer of this row either
/// replaces the owner (`replace_lease`, the only claim path) or removes the row
/// (`delete_task_lease`); `renew_task_lease` moves only the deadline. So a
/// surviving owner match proves nobody re-planned this task while phase 2 ran,
/// expired or not — and refusing an uncontended claim merely because a slow
/// `git worktree add` outran its TTL would drop a dispatch that is safe to make.
fn claim_still_ours(ledger: &Ledger, entry: &ClaimedAction) -> anyhow::Result<Option<TaskLease>> {
    Ok(ledger
        .task_lease(entry.task.task_id)?
        .filter(|lease| lease.attempt == entry.attempt && lease.owner == entry.owner))
}

/// Reconcile in three phases so the workspace lock never spans `git worktree
/// add`, which measured 27.99 s of a 28.02 s critical section and 64.8 s on
/// another run (GH-1047). Serialization no longer comes from the length of the
/// hold but from the claim leases phase 1 writes, in two halves that only work
/// together: a live claim means owned, so a concurrent reconciler plans nothing
/// for that task and spends neither a slot nor its scope on it — and phase 3
/// re-reads the claim before it writes, because the claim can expire and be
/// taken over while phase 2 runs unlocked.
pub(super) fn persist_reconciliation(
    repo_root: &Path,
    config: &ReconcileConfig,
) -> anyhow::Result<PersistOutcome> {
    let ledger = Ledger::open(repo_root)?;
    let (claimed, mut errors) = claim_planned_attempts(&ledger, repo_root, config)?;
    if claimed.is_empty() {
        return Ok(PersistOutcome {
            plans: Vec::new(),
            errors,
        });
    }
    // No lock held: `git worktree add` touches the worktree, never `.edda`, so
    // it is exactly the unrelated I/O `d-012.task_notify_lock` bars from the
    // rail truth lock. The complete batch is still prepared before the first
    // dispatch event, so a later refusal strands no earlier task: the claims
    // are released and no `task.started` was ever appended. A crash between the
    // phases leaves only leases behind, and those expire and self-heal.
    let prepared = match prepare_attempt_worktrees(repo_root, &claimed) {
        Ok(prepared) => prepared,
        Err(error) => {
            // The git refusal is the failure to report, but a claim this pass
            // could not roll back hides its task from every reconciler until
            // the lease expires. That travels with the refusal instead of being
            // dropped with the `errors` this pass had already collected.
            errors.extend(release_claims(&ledger, &claimed));
            return Err(if errors.is_empty() {
                error
            } else {
                error.context(errors.join("; "))
            });
        }
    };
    let (plans, commit_errors) = commit_claimed_attempts(&ledger, config, claimed, prepared)?;
    errors.extend(commit_errors);
    Ok(PersistOutcome { plans, errors })
}

/// Phase 1, under the workspace lock: read the rail, plan, and claim each
/// planned attempt. Ledger-only, measured under 10 ms.
fn claim_planned_attempts(
    ledger: &Ledger,
    repo_root: &Path,
    config: &ReconcileConfig,
) -> anyhow::Result<(Vec<ClaimedAction>, Vec<String>)> {
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
    let mut claimed = Vec::new();
    let mut errors = Vec::new();
    for action in actions {
        // One task's claim failure must not cancel the rest of the batch.
        match claim_attempt(ledger, &views, action, config) {
            Ok(entry) => claimed.push(entry),
            Err(error) => errors.push(format!("task action persistence failed: {error:#}")),
        }
    }
    drop(lock);
    Ok((claimed, errors))
}

fn claim_attempt(
    ledger: &Ledger,
    views: &[TaskView],
    action: ReconcileAction,
    config: &ReconcileConfig,
) -> anyhow::Result<ClaimedAction> {
    let task = task_view(views, action.task_id())?.clone();
    let attempt = match &action {
        ReconcileAction::Start { attempt, .. } | ReconcileAction::Resume { attempt, .. } => {
            *attempt
        }
        ReconcileAction::Requeue { next_attempt, .. } => *next_attempt,
        ReconcileAction::Fail { .. } => task.attempts,
    };
    let owner = replace_lease(ledger, task.task_id, attempt, config.lease_ttl_s)?;
    Ok(ClaimedAction {
        action,
        task,
        attempt,
        owner,
    })
}

/// Phase 2, with no lock held. A `Fail` retires a task and prepares nothing.
fn prepare_attempt_worktrees(
    repo_root: &Path,
    claimed: &[ClaimedAction],
) -> anyhow::Result<Vec<Option<PathBuf>>> {
    claimed
        .iter()
        .map(|entry| {
            if matches!(entry.action, ReconcileAction::Fail { .. }) {
                return Ok(None);
            }
            let resume = matches!(entry.action, ReconcileAction::Resume { .. });
            ensure_attempt_worktree(repo_root, &entry.task, entry.attempt, resume).map(Some)
        })
        .collect()
}

/// Undo phase 1's claims after a refused batch, reporting whatever it could not
/// undo. The advisory lock is taken when it is available but is not required: a
/// single lease delete is atomic in SQLite. A delete that fails is not silent,
/// though — that lease hides its task from every reconciler for the rest of
/// `--lease-ttl-s`, where before the phase split a refused batch left no lease
/// at all and the next pass re-planned immediately.
fn release_claims(ledger: &Ledger, claimed: &[ClaimedAction]) -> Vec<String> {
    let mut errors = Vec::new();
    let lock = match acquire_workspace_lock(&ledger.paths) {
        Ok(lock) => Some(lock),
        Err(error) => {
            errors.push(format!(
                "claim rollback ran without the workspace lock: {error:#}"
            ));
            None
        }
    };
    for entry in claimed {
        let released = match claim_still_ours(ledger, entry) {
            // Deleting by task and attempt alone would take a peer's claim with
            // it; a claim that is no longer ours is already released as far as
            // this pass is concerned.
            Ok(None) => Ok(()),
            Ok(Some(_)) => ledger
                .delete_task_lease(entry.task.task_id, entry.attempt)
                .map(|_| ()),
            Err(error) => Err(error),
        };
        if let Err(error) = released {
            errors.push(format!(
                "task #{} stays claimed until its lease expires because releasing attempt {} failed: {error:#}",
                entry.task.task_id, entry.attempt
            ));
        }
    }
    drop(lock);
    errors
}

/// Phase 3, under the workspace lock: append the dispatch truth the claims
/// reserved. Ledger-only, like phase 1.
fn commit_claimed_attempts(
    ledger: &Ledger,
    config: &ReconcileConfig,
    claimed: Vec<ClaimedAction>,
    prepared: Vec<Option<PathBuf>>,
) -> anyhow::Result<(Vec<RunnerPlan>, Vec<String>)> {
    let lock = acquire_workspace_lock(&ledger.paths)?;
    let mut plans = Vec::new();
    let mut errors = Vec::new();
    for (entry, worktree) in claimed.into_iter().zip(prepared) {
        // Phase 2 ran unlocked and unbounded, so the claim that authorises this
        // write may have been taken over meanwhile. Re-verify ownership before
        // writing on it, the way `record_session_if_current` and `finish_runner`
        // re-check a lease they took under an earlier hold.
        match claim_still_ours(ledger, &entry) {
            Ok(Some(_)) => match commit_claimed_attempt(ledger, config, &entry, worktree) {
                Ok(Some(plan)) => plans.push(plan),
                Ok(None) => {}
                Err(error) => errors.push(format!("task action persistence failed: {error:#}")),
            },
            Ok(None) => errors.push(format!(
                "task #{} lost its claim on attempt {} while its worktree was prepared and was not dispatched; raise --lease-ttl-s above the cost of `git worktree add`",
                entry.task.task_id, entry.attempt
            )),
            Err(error) => errors.push(format!(
                "task #{} claim re-check failed: {error:#}",
                entry.task.task_id
            )),
        }
    }
    let branch = ledger.head_branch()?;
    let _ = edda_derive::rebuild_branch(ledger, &branch);
    drop(lock);
    Ok((plans, errors))
}

/// Append the dispatch truth one claim reserved. Every failure releases that
/// claim, so no lease outlives an attempt that never started — the requeue and
/// failure appends included, whose errors used to return with the claim still
/// in the ledger and the task hidden until its TTL expired.
fn commit_claimed_attempt(
    ledger: &Ledger,
    config: &ReconcileConfig,
    entry: &ClaimedAction,
    worktree: Option<PathBuf>,
) -> anyhow::Result<Option<RunnerPlan>> {
    match append_claimed_truth(ledger, config, entry, worktree) {
        Ok(plan) => Ok(plan),
        Err(error) => Err(
            match ledger.delete_task_lease(entry.task.task_id, entry.attempt) {
                Ok(_) => error,
                Err(rollback) => error.context(format!(
                    "task #{} stays claimed until its lease expires because releasing attempt {} failed: {rollback:#}",
                    entry.task.task_id, entry.attempt
                )),
            },
        ),
    }
}

fn append_claimed_truth(
    ledger: &Ledger,
    config: &ReconcileConfig,
    entry: &ClaimedAction,
    worktree: Option<PathBuf>,
) -> anyhow::Result<Option<RunnerPlan>> {
    let task_id = entry.task.task_id;
    let attempt = entry.attempt;
    let (requeue, start) = match &entry.action {
        ReconcileAction::Fail { reason, .. } => {
            append_failed(ledger, task_id, reason)?;
            let _ = ledger.delete_task_lease(task_id, attempt)?;
            return Ok(None);
        }
        // A replacement attempt for failed work records the requeue before the
        // start that replaces it.
        ReconcileAction::Start { .. } => (entry.task.status == TaskStatus::Failed, true),
        ReconcileAction::Requeue { .. } => (true, true),
        // A same-attempt Codex resume keeps its existing `task.started` truth;
        // the claim lease phase 1 refreshed is its only rail write.
        ReconcileAction::Resume { .. } => (false, false),
    };
    let worktree = worktree.context("prepared reconciliation worktree disappeared")?;
    // The claim's TTL has been running since phase 1, across a `git worktree
    // add` of unbounded cost. Extend it here so the runner inherits a full
    // `--lease-ttl-s` measured from the dispatch rather than from the plan —
    // `task.started` carries the TTL but writes no lease of its own.
    anyhow::ensure!(
        renew_lease(ledger, task_id, attempt, config.lease_ttl_s)?,
        "claim lease for task #{task_id} attempt {attempt} vanished before its dispatch"
    );
    if requeue {
        append_requeued(ledger, task_id, attempt)?;
    }
    if start {
        append_started(ledger, task_id, attempt, config.lease_ttl_s)?;
    }
    Ok(Some(RunnerPlan {
        task: entry.task.clone(),
        attempt,
        worktree,
    }))
}

// A concurrent reconciler waits out a busy workspace rather than bailing early
// (GH-524). The budget is no longer the only thing standing between a correct
// run and a false failure: since GH-1047 the critical section holds no git I/O,
// so what it bounds is SQLite reads and appends measured under 10 ms, not a
// `git worktree add` measured at 27.99 s and 64.8 s on the same workstation.
const WORKSPACE_LOCK_WAIT_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

#[cfg(test)]
thread_local! {
    /// Per-thread override: a test may shrink its own wait budget without
    /// shortening it for every other test sharing this process.
    pub(super) static LOCK_WAIT_BUDGET_OVERRIDE: std::cell::Cell<Option<std::time::Duration>> =
        const { std::cell::Cell::new(None) };
}

pub(super) fn workspace_lock_wait_budget() -> std::time::Duration {
    #[cfg(test)]
    if let Some(budget) = LOCK_WAIT_BUDGET_OVERRIDE.with(std::cell::Cell::get) {
        return budget;
    }
    WORKSPACE_LOCK_WAIT_BUDGET
}
