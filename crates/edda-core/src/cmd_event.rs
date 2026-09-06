//! Command execution receipts.
use crate::event::{finalize_event, new_event_id, now_rfc3339};
use crate::types::{Event, Refs, SCHEMA_VERSION};

/// Parameters for creating a `cmd` event.
pub struct CmdEventParams<'a> {
    pub branch: &'a str,
    pub parent_hash: Option<&'a str>,
    pub argv: &'a [String],
    pub cwd: &'a str,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub stdout_blob: &'a str,
    pub stderr_blob: &'a str,
}

/// Create a new `cmd` event.
pub fn new_cmd_event(params: &CmdEventParams<'_>) -> anyhow::Result<Event> {
    new_cmd_event_with_git_context(params, None, None)
}

/// Create a SHA-bound execution receipt. Unknown Git state stays null rather
/// than being mistaken for a clean checkout. The original constructor remains
/// available for callers that do not have execution context.
pub fn new_cmd_event_with_git_context(
    params: &CmdEventParams<'_>,
    git_sha: Option<&str>,
    tree_dirty: Option<bool>,
) -> anyhow::Result<Event> {
    let payload = serde_json::json!({
        "argv": params.argv,
        "cwd": params.cwd,
        "exit_code": params.exit_code,
        "duration_ms": params.duration_ms,
        "stdout_blob": params.stdout_blob,
        "stderr_blob": params.stderr_blob,
        "git_sha": git_sha,
        "tree_dirty": tree_dirty,
    });

    let mut blob_refs = Vec::new();
    if !params.stdout_blob.is_empty() {
        blob_refs.push(params.stdout_blob.to_string());
    }
    if !params.stderr_blob.is_empty() {
        blob_refs.push(params.stderr_blob.to_string());
    }

    let mut event = Event {
        event_id: new_event_id(),
        ts: now_rfc3339(),
        event_type: "cmd".to_string(),
        branch: params.branch.to_string(),
        parent_hash: params.parent_hash.map(|s| s.to_string()),
        hash: String::new(),
        payload,
        refs: Refs {
            blobs: blob_refs,
            ..Default::default()
        },
        schema_version: SCHEMA_VERSION,
        digests: Vec::new(),
        event_family: None,
        event_level: None,
    };

    finalize_event(&mut event)?;
    Ok(event)
}
