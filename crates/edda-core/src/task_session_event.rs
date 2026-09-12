use crate::event::build_task_event;
use crate::Event;

/// Create a new `task.session` event (ACP session id recorded for resume).
pub fn new_task_session_event(
    branch: &str,
    parent_hash: Option<&str>,
    task_id: u64,
    acp_session_id: &str,
) -> anyhow::Result<Event> {
    build_task_event(
        branch,
        parent_hash,
        "task.session",
        serde_json::json!({
            "task_id": task_id,
            "acp_session_id": acp_session_id,
        }),
    )
}

/// Immutable runtime correlation carried by a controlled `task.session`.
pub struct ControlledTaskSessionParams<'a> {
    pub agent_kind: &'a str,
    pub session_id: &'a str,
    pub attempt: u32,
    pub lease_owner: &'a str,
    pub brief_event_id: &'a str,
    pub brief_digest: &'a str,
}

/// Create an ACP `task.session` event bound to an accepted execution brief.
pub fn new_task_session_event_with_execution_brief(
    branch: &str,
    parent_hash: Option<&str>,
    task_id: u64,
    params: &ControlledTaskSessionParams<'_>,
) -> anyhow::Result<Event> {
    validate_controlled_binding(params)?;
    build_task_event(
        branch,
        parent_hash,
        "task.session",
        serde_json::json!({
            "task_id": task_id,
            "agent_kind": params.agent_kind,
            "acp_session_id": params.session_id,
            "session_id": params.session_id,
            "attempt": params.attempt,
            "lease_owner": params.lease_owner,
            "brief_event_id": params.brief_event_id,
            "brief_digest": params.brief_digest,
        }),
    )
}

/// Create a new host-neutral `task.session` event.
pub fn new_task_host_session_event(
    branch: &str,
    parent_hash: Option<&str>,
    task_id: u64,
    agent_kind: &str,
    session_id: &str,
    attempt: u32,
) -> anyhow::Result<Event> {
    new_task_host_session_event_inner(
        branch,
        parent_hash,
        task_id,
        agent_kind,
        session_id,
        attempt,
        None,
    )
}

/// Create a host-neutral `task.session` event bound to an accepted brief.
pub fn new_task_host_session_event_with_execution_brief(
    branch: &str,
    parent_hash: Option<&str>,
    task_id: u64,
    params: &ControlledTaskSessionParams<'_>,
) -> anyhow::Result<Event> {
    validate_controlled_binding(params)?;
    new_task_host_session_event_inner(
        branch,
        parent_hash,
        task_id,
        params.agent_kind,
        params.session_id,
        params.attempt,
        Some((
            params.brief_event_id,
            params.brief_digest,
            params.lease_owner,
        )),
    )
}

fn validate_controlled_binding(params: &ControlledTaskSessionParams<'_>) -> anyhow::Result<()> {
    anyhow::ensure!(
        params.attempt > 0,
        "controlled task.session attempt must be positive"
    );
    for (value, field) in [
        (params.agent_kind, "agent_kind"),
        (params.session_id, "session_id"),
        (params.lease_owner, "lease_owner"),
    ] {
        anyhow::ensure!(
            !value.trim().is_empty() && value.chars().count() <= 160,
            "controlled task.session {field} has an invalid shape"
        );
    }
    anyhow::ensure!(
        params.brief_event_id.starts_with("evt_")
            && params.brief_event_id.len() <= 68
            && params.brief_event_id[4..]
                .chars()
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit()),
        "controlled task.session brief event identity has an invalid shape"
    );
    anyhow::ensure!(
        params.brief_digest.len() == 64
            && params
                .brief_digest
                .chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase()),
        "controlled task.session brief digest has an invalid shape"
    );
    Ok(())
}

fn new_task_host_session_event_inner(
    branch: &str,
    parent_hash: Option<&str>,
    task_id: u64,
    agent_kind: &str,
    session_id: &str,
    attempt: u32,
    brief_identity: Option<(&str, &str, &str)>,
) -> anyhow::Result<Event> {
    let mut payload = serde_json::json!({
        "task_id": task_id,
        "agent_kind": agent_kind,
        "session_id": session_id,
        "attempt": attempt,
    });
    if let Some((event_id, digest, lease_owner)) = brief_identity {
        payload["brief_event_id"] = serde_json::json!(event_id);
        payload["brief_digest"] = serde_json::json!(digest);
        payload["lease_owner"] = serde_json::json!(lease_owner);
    }
    build_task_event(branch, parent_hash, "task.session", payload)
}
