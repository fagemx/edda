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
    new_controlled_task_done_event, new_task_created_event, new_task_done_event,
    new_task_failed_event, new_task_started_event, ControlledTaskDoneParams, TaskCreatedParams,
};
use edda_core::guided_execution::WorkReceiptV1;
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

pub const CONTROLLED_TASK_LEASE_PREFIX: &str = "controlled-brief:";

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
    let lease = ledger.task_lease(id)?;
    let controlled = v.session_brief_event_id.is_some()
        || v.session_brief_digest.is_some()
        || v.session_lease_owner.is_some()
        || lease
            .as_ref()
            .is_some_and(|lease| lease.owner.starts_with(CONTROLLED_TASK_LEASE_PREFIX));
    if receipt.trim().is_empty() {
        anyhow::bail!(
            "a completion without a receipt does not exist — pass --receipt with real content"
        );
    }
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    let (event, correction) = if controlled {
        anyhow::ensure!(
            evidence_paths.is_empty(),
            "controlled completion derives evidence from WorkReceiptV1; --evidence is not accepted"
        );
        let completion = validate_controlled_done(v, lease.as_ref(), receipt)?;
        let event = new_controlled_task_done_event(&ControlledTaskDoneParams {
            branch: &branch,
            parent_hash: parent_hash.as_deref(),
            task_id: id,
            receipt,
            attempt: completion.attempt,
            lease_owner: &completion.lease_owner,
            session_id: &completion.session_id,
            agent_kind: &completion.agent_kind,
            brief_event_id: &completion.brief_event_id,
            brief_digest: &completion.brief_digest,
            outcome_code: &completion.outcome_code,
        })?;
        (event, false)
    } else {
        let correction = v.status == TaskStatus::Done;
        validate_legacy_done_status(v)?;
        (
            new_task_done_event(&branch, parent_hash.as_deref(), id, receipt, evidence_paths)?,
            correction,
        )
    };
    // For a controlled event SqliteStore::append_event consumes the exact
    // unexpired lease in the same BEGIN IMMEDIATE transaction. Zero deleted
    // rows rolls the event back, so stale completion cannot unlock successors.
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

fn validate_legacy_done_status(view: &TaskView) -> anyhow::Result<()> {
    match view.status {
        TaskStatus::Running | TaskStatus::Done => Ok(()),
        TaskStatus::Ready if view.attempts > 0 => Ok(()),
        TaskStatus::Ready | TaskStatus::Blocked => anyhow::bail!(
            "task #{} has not been started — run `edda task start {}` first \
             (start/done pairs are what make the ledger replayable)",
            view.task_id,
            view.task_id
        ),
        TaskStatus::Failed => anyhow::bail!(
            "task #{} is failed — run `edda task start {}` to retry, then done",
            view.task_id,
            view.task_id
        ),
    }
}

struct ControlledDone {
    attempt: u32,
    lease_owner: String,
    session_id: String,
    agent_kind: String,
    brief_event_id: String,
    brief_digest: String,
    outcome_code: String,
}

fn validate_controlled_done(
    view: &TaskView,
    lease: Option<&crate::TaskLease>,
    receipt_bytes: &str,
) -> anyhow::Result<ControlledDone> {
    anyhow::ensure!(
        view.status == TaskStatus::Running,
        "controlled task is not running; stale completion refused"
    );
    let (brief_event_id, brief_digest, session_id, agent_kind, lease_owner, attempt) = match (
        view.session_brief_event_id.as_deref(),
        view.session_brief_digest.as_deref(),
        view.session_id.as_deref(),
        view.session_agent_kind.as_deref(),
        view.session_lease_owner.as_deref(),
        view.session_attempt,
    ) {
        (Some(event_id), Some(digest), Some(session), Some(agent), Some(owner), Some(attempt)) => {
            (event_id, digest, session, agent, owner, attempt)
        }
        _ => anyhow::bail!("controlled task has an incomplete session authority binding"),
    };
    anyhow::ensure!(
        attempt == view.attempts,
        "controlled task attempt changed before completion"
    );
    let lease = lease.ok_or_else(|| anyhow::anyhow!("controlled task lease is missing"))?;
    anyhow::ensure!(
        lease.attempt == attempt && lease.owner == lease_owner,
        "controlled task lease changed before completion"
    );
    let expires = time::OffsetDateTime::parse(
        &lease.expires_at,
        &time::format_description::well_known::Rfc3339,
    )?;
    anyhow::ensure!(
        expires > time::OffsetDateTime::now_utc(),
        "controlled task lease expired before completion"
    );
    let prefix = format!("{CONTROLLED_TASK_LEASE_PREFIX}{brief_event_id}:{brief_digest}:");
    anyhow::ensure!(
        lease_owner.starts_with(&prefix),
        "controlled task lease is bound to a different execution brief"
    );
    // This parse is only enough to construct the candidate event. The SQLite
    // BEGIN IMMEDIATE append path reloads current state and performs the full
    // WorkReceiptV1/brief/task/session/scope/outcome validation atomically.
    let receipt: WorkReceiptV1 = serde_json::from_str(receipt_bytes)
        .map_err(|_| anyhow::anyhow!("invalid WorkReceiptV1 input schema"))?;
    Ok(ControlledDone {
        attempt,
        lease_owner: lease_owner.to_string(),
        session_id: session_id.to_string(),
        agent_kind: agent_kind.to_string(),
        brief_event_id: brief_event_id.to_string(),
        brief_digest: brief_digest.to_string(),
        outcome_code: receipt.outcome_code,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guided_execution::tests::{data_event, input as brief_input};
    use edda_core::event::{
        new_task_host_session_event_with_execution_brief, ControlledTaskSessionParams,
    };
    use edda_core::guided_execution::{parse_execution_brief_event, RuntimeProfileV1};

    fn no_rebuild(_: &Ledger, _: &str) {}

    fn setup_controlled() -> (tempfile::TempDir, AcceptedFixture, serde_json::Value) {
        let root = tempfile::tempdir().unwrap();
        Ledger::ensure_initialized(root.path()).unwrap();
        let ledger = Ledger::open(root.path()).unwrap();
        let scope = vec!["src/**".to_string()];
        new_task(
            root.path(),
            &NewTaskArgs {
                title: "controlled completion",
                assignee: None,
                agent_kind: Some("codex"),
                after: &[],
                plan: None,
                work_unit: None,
                brief: None,
                idempotency_key: None,
                scope_paths: &scope,
            },
            &no_rebuild,
        )
        .unwrap();
        start_task(root.path(), 1, 300, &no_rebuild).unwrap();
        let mut input = brief_input(Some(1));
        input.brief_id = "brief_done1".into();
        input.runtime_profile = RuntimeProfileV1::Flash;
        input.scope.allowed_paths = scope.clone();
        input.procedure = edda_core::guided_execution::TrustedProcedureV1::ControllerAuthored {
            authored_by: "controller".into(),
            principles: vec![],
            probe_cards: vec![],
            implementation_steps: vec![edda_core::guided_execution::ImplementationStepV1 {
                step_id: "step_fix".into(),
                instruction: "complete the bounded change".into(),
                action: None,
            }],
            validation: vec![],
        };
        input
            .outcome_codes
            .push(edda_core::guided_execution::OutcomeCodeV1 {
                code: "FAILED".into(),
                result_class: edda_core::guided_execution::ResultClassV1::Failure,
            });
        input
            .receipt_schema
            .required_fields
            .push(edda_core::guided_execution::ReceiptFieldV1::TaskIdentity);
        let branch = ledger.head_branch().unwrap();
        let parent = ledger.last_event_hash().unwrap();
        let event = data_event(&branch, parent.as_deref(), input).unwrap();
        let record = parse_execution_brief_event(&event).unwrap();
        ledger.append_event(&event).unwrap();
        let owner = format!(
            "{CONTROLLED_TASK_LEASE_PREFIX}{}:{}:runner",
            event.event_id, record.content_digest
        );
        ledger
            .upsert_task_lease(&crate::TaskLease {
                task_id: 1,
                attempt: 1,
                owner: owner.clone(),
                expires_at: "2999-01-01T00:00:00Z".into(),
                heartbeat_at: "2026-09-11T00:00:00Z".into(),
            })
            .unwrap();
        let parent = ledger.last_event_hash().unwrap();
        ledger
            .append_event(
                &new_task_host_session_event_with_execution_brief(
                    &branch,
                    parent.as_deref(),
                    1,
                    &ControlledTaskSessionParams {
                        agent_kind: "codex",
                        session_id: "session-one",
                        attempt: 1,
                        lease_owner: &owner,
                        brief_event_id: &event.event_id,
                        brief_digest: &record.content_digest,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        let receipt = serde_json::json!({
            "receipt_version": 1,
            "receipt_id": "receipt_done1",
            "brief": {
                "brief_id": record.brief.brief_id,
                "brief_event_id": event.event_id,
                "content_digest": record.content_digest
            },
            "task_ref": {"task_id": 1, "attempt": 1, "lease_owner": owner},
            "agent": {
                "agent_kind": "codex",
                "runtime_profile": "flash",
                "session_id": "session-one"
            },
            "basis_full_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "started_at": "2026-09-11T00:00:00Z",
            "ended_at": "2026-09-11T00:01:00Z",
            "outcome_code": "DONE",
            "observations": [],
            "commands_run": [],
            "hypotheses_supported": [],
            "hypotheses_rejected": [],
            "changed_paths": ["src/lib.rs"],
            "validation_ran": [],
            "validation_read": [],
            "unknowns": [],
            "recommended_next_action": "review",
            "result_class": "success"
        });
        (
            root,
            AcceptedFixture {
                event_id: record.brief_event_id,
                digest: record.content_digest,
                owner,
            },
            receipt,
        )
    }

    struct AcceptedFixture {
        event_id: String,
        digest: String,
        owner: String,
    }

    #[test]
    fn sqlite_boundary_validates_receipt_and_atomically_consumes_exact_lease() {
        let (root, accepted, receipt) = setup_controlled();
        let ledger = Ledger::open(root.path()).unwrap();
        let branch = ledger.head_branch().unwrap();
        let parent = ledger.last_event_hash().unwrap();
        let event = new_controlled_task_done_event(&ControlledTaskDoneParams {
            branch: &branch,
            parent_hash: parent.as_deref(),
            task_id: 1,
            receipt: &receipt.to_string(),
            attempt: 1,
            lease_owner: &accepted.owner,
            session_id: "session-one",
            agent_kind: "codex",
            brief_event_id: &accepted.event_id,
            brief_digest: &accepted.digest,
            outcome_code: "DONE",
        })
        .unwrap();
        // The only successful seam is compiled solely into this crate's unit
        // tests. Production append always requires the absent S6 seal.
        ledger
            .append_event_with_test_execution_authority(&event)
            .unwrap();
        assert!(ledger.task_lease(1).unwrap().is_none());
        let view = ledger.task_views().unwrap().remove(0);
        assert_eq!(view.status, TaskStatus::Done);
        let event = ledger.task_events().unwrap().pop().unwrap();
        assert_eq!(event.event_type, "task.done");
        assert_eq!(event.payload["controlled_completion"]["attempt"], 1);
        assert_eq!(
            event.payload["controlled_completion"]["brief_event_id"],
            accepted.event_id
        );
        assert_eq!(
            event.payload["controlled_completion"]["brief_digest"],
            accepted.digest
        );
        assert_eq!(
            event.payload["controlled_completion"]["lease_owner"],
            accepted.owner
        );
    }

    #[test]
    fn public_done_paths_cannot_unlock_a_controlled_task_without_s6_seal() {
        let (root, accepted, receipt) = setup_controlled();
        let ledger = Ledger::open(root.path()).unwrap();
        let before = ledger.count_events().unwrap();

        let error = done_task(root.path(), 1, &receipt.to_string(), &[], &no_rebuild).unwrap_err();
        let error = format!("{error:#}");
        assert!(
            error.contains("product-verifiable execution authority seal"),
            "{error}"
        );

        let parent = ledger.last_event_hash().unwrap();
        let legacy = new_task_done_event(
            "main",
            parent.as_deref(),
            1,
            "self-consistent legacy receipt",
            &[],
        )
        .unwrap();
        assert!(ledger.append_event(&legacy).is_err());
        assert_eq!(ledger.count_events().unwrap(), before);
        assert_eq!(
            ledger.task_views().unwrap().remove(0).status,
            TaskStatus::Running
        );
        assert_eq!(ledger.task_lease(1).unwrap().unwrap().owner, accepted.owner);
    }

    #[test]
    fn controlled_done_refuses_malformed_or_mismatched_receipts_without_unlocking() {
        let (root, accepted, receipt) = setup_controlled();
        let before = Ledger::open(root.path()).unwrap().count_events().unwrap();
        assert!(done_task(root.path(), 1, "not-json", &[], &no_rebuild).is_err());

        let mut wrong_brief = receipt.clone();
        wrong_brief["brief"]["content_digest"] = serde_json::json!("0".repeat(64));
        assert!(done_task(root.path(), 1, &wrong_brief.to_string(), &[], &no_rebuild).is_err());
        let mut wrong_task = receipt.clone();
        wrong_task["task_ref"]["task_id"] = serde_json::json!(2);
        assert!(done_task(root.path(), 1, &wrong_task.to_string(), &[], &no_rebuild).is_err());
        let mut wrong_attempt = receipt.clone();
        wrong_attempt["task_ref"]["attempt"] = serde_json::json!(2);
        assert!(done_task(root.path(), 1, &wrong_attempt.to_string(), &[], &no_rebuild).is_err());
        let mut wrong_lease = receipt.clone();
        wrong_lease["task_ref"]["lease_owner"] = serde_json::json!("other");
        assert!(done_task(root.path(), 1, &wrong_lease.to_string(), &[], &no_rebuild).is_err());
        let mut wrong_session = receipt.clone();
        wrong_session["agent"]["session_id"] = serde_json::json!("other");
        assert!(done_task(root.path(), 1, &wrong_session.to_string(), &[], &no_rebuild).is_err());
        let mut wrong_agent = receipt.clone();
        wrong_agent["agent"]["agent_kind"] = serde_json::json!("acp:grok");
        assert!(done_task(root.path(), 1, &wrong_agent.to_string(), &[], &no_rebuild).is_err());
        let mut out_of_scope = receipt.clone();
        out_of_scope["changed_paths"] = serde_json::json!(["outside.txt"]);
        assert!(done_task(root.path(), 1, &out_of_scope.to_string(), &[], &no_rebuild).is_err());
        let mut failure = receipt;
        failure["outcome_code"] = serde_json::json!("FAILED");
        failure["result_class"] = serde_json::json!("failure");
        assert!(done_task(root.path(), 1, &failure.to_string(), &[], &no_rebuild).is_err());
        assert!(done_task(
            root.path(),
            1,
            &wrong_attempt.to_string(),
            &["proof.txt".into()],
            &no_rebuild,
        )
        .is_err());

        let ledger = Ledger::open(root.path()).unwrap();
        assert_eq!(ledger.count_events().unwrap(), before);
        assert_eq!(
            ledger.task_views().unwrap().remove(0).status,
            TaskStatus::Running
        );
        assert_eq!(ledger.task_lease(1).unwrap().unwrap().owner, accepted.owner);
    }

    #[test]
    fn controlled_done_append_rolls_back_after_exact_lease_replacement() {
        let (root, accepted, receipt) = setup_controlled();
        let ledger = Ledger::open(root.path()).unwrap();
        ledger
            .upsert_task_lease(&crate::TaskLease {
                task_id: 1,
                attempt: 2,
                owner: "replacement".into(),
                expires_at: "2999-01-01T00:00:00Z".into(),
                heartbeat_at: "2026-09-11T00:02:00Z".into(),
            })
            .unwrap();
        let before = ledger.count_events().unwrap();
        let branch = ledger.head_branch().unwrap();
        let parent = ledger.last_event_hash().unwrap();
        let event = new_controlled_task_done_event(&ControlledTaskDoneParams {
            branch: &branch,
            parent_hash: parent.as_deref(),
            task_id: 1,
            receipt: &receipt.to_string(),
            attempt: 1,
            lease_owner: &accepted.owner,
            session_id: "session-one",
            agent_kind: "codex",
            brief_event_id: &accepted.event_id,
            brief_digest: &accepted.digest,
            outcome_code: "DONE",
        })
        .unwrap();

        assert!(ledger.append_event(&event).is_err());
        assert_eq!(ledger.count_events().unwrap(), before);
        assert_eq!(ledger.task_lease(1).unwrap().unwrap().owner, "replacement");
    }

    #[test]
    fn active_acp_turn_blocks_direct_fail_and_restart_generation_appends() {
        let root = tempfile::tempdir().unwrap();
        Ledger::ensure_initialized(root.path()).unwrap();
        let scope = vec!["src/**".into()];
        new_task(
            root.path(),
            &NewTaskArgs {
                title: "turn generation",
                assignee: None,
                agent_kind: Some("acp:grok"),
                after: &[],
                plan: None,
                work_unit: None,
                brief: None,
                idempotency_key: None,
                scope_paths: &scope,
            },
            &no_rebuild,
        )
        .unwrap();
        start_task(root.path(), 1, 300, &no_rebuild).unwrap();
        let ledger = Ledger::open(root.path()).unwrap();
        let guard = crate::lock::TaskDispatchLock::acquire(&ledger.paths, 1).unwrap();

        let parent = ledger.last_event_hash().unwrap();
        let failed = new_task_failed_event("main", parent.as_deref(), 1, "restart").unwrap();
        assert!(ledger.append_event(&failed).is_err());
        let parent = ledger.last_event_hash().unwrap();
        let restarted =
            edda_core::event::new_task_started_event("main", parent.as_deref(), 1, 300, 2).unwrap();
        assert!(ledger.append_event(&restarted).is_err());
        assert_eq!(ledger.task_views().unwrap().remove(0).attempts, 1);

        drop(guard);
        fail_task(root.path(), 1, "restart", &no_rebuild).unwrap();
        assert_eq!(
            start_task(root.path(), 1, 300, &no_rebuild)
                .unwrap()
                .attempt,
            2
        );
    }

    #[test]
    fn legacy_done_keeps_plain_text_receipt_compatibility() {
        let root = tempfile::tempdir().unwrap();
        Ledger::ensure_initialized(root.path()).unwrap();
        let scope = vec!["src/**".into()];
        new_task(
            root.path(),
            &NewTaskArgs {
                title: "legacy completion",
                assignee: None,
                agent_kind: None,
                after: &[],
                plan: None,
                work_unit: None,
                brief: None,
                idempotency_key: None,
                scope_paths: &scope,
            },
            &no_rebuild,
        )
        .unwrap();
        start_task(root.path(), 1, 300, &no_rebuild).unwrap();
        done_task(
            root.path(),
            1,
            "focused tests passed",
            &["proof.txt".into()],
            &no_rebuild,
        )
        .unwrap();
        let view = Ledger::open(root.path())
            .unwrap()
            .task_views()
            .unwrap()
            .remove(0);
        assert_eq!(view.status, TaskStatus::Done);
        assert_eq!(view.receipt.as_deref(), Some("focused tests passed"));
    }
}
