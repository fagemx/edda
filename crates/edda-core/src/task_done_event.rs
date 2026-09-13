use crate::event::build_task_event;
use crate::Event;

/// Authority correlation written only after a controlled receipt validates.
pub struct ControlledTaskDoneParams<'a> {
    pub branch: &'a str,
    pub parent_hash: Option<&'a str>,
    pub task_id: u64,
    pub receipt: &'a str,
    pub attempt: u32,
    pub lease_owner: &'a str,
    pub session_id: &'a str,
    pub agent_kind: &'a str,
    pub brief_event_id: &'a str,
    pub brief_digest: &'a str,
    pub outcome_code: &'a str,
}

/// Create a controlled `task.done`; SQLite atomically consumes its exact lease.
pub fn new_controlled_task_done_event(
    params: &ControlledTaskDoneParams<'_>,
) -> anyhow::Result<Event> {
    if params.receipt.trim().is_empty() {
        anyhow::bail!(
            "task.done requires a non-empty receipt (task {})",
            params.task_id
        );
    }
    build_task_event(
        params.branch,
        params.parent_hash,
        "task.done",
        serde_json::json!({
            "task_id": params.task_id,
            "receipt": params.receipt,
            "evidence_paths": [],
            "controlled_completion": {
                "attempt": params.attempt,
                "lease_owner": params.lease_owner,
                "session_id": params.session_id,
                "agent_kind": params.agent_kind,
                "brief_event_id": params.brief_event_id,
                "brief_digest": params.brief_digest,
                "outcome_code": params.outcome_code,
            },
        }),
    )
}
