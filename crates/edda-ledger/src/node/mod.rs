//! `edda node` v0 transport core (GH-685).
//!
//! This module is the **ledger-side half** of the per-machine Edda node: the
//! machine-local `node.json` configuration, the allowlisted wire envelopes, the
//! durable outbound queue and the idempotent import engine. The HTTP endpoint
//! lives in `edda-serve`; the verbs live in `edda-cli`.
//!
//! The node transports and records only. It never executes a remote process,
//! never schedules, never wakes a model and never becomes a second task state.
//! No session id, run id, local path/root, lease, worktree or credential ever
//! crosses the wire, and every envelope is `deny_unknown_fields`: an unknown
//! field is refused, never ignored.
//!
//! The authoritative contract is `proof/cross-machine/node-v0-wire-contract.md`.
//! Where this module and that contract disagree, the contract wins.

pub mod config;
pub mod envelope;
pub mod import;
pub mod queue;

pub use config::{
    is_tailnet_ipv4, load_node_config, node_config_path, resolve_peer_token, validate_config,
    validate_config_with_bind_policy, validate_machine_label, NodeConfig, NodeSection, PeerConfig,
};
pub use envelope::{
    compute_event_id, is_sha256_hex, owner_return_logical_id, parse_event, validate_label,
    validate_lane_request, validate_owner_return, validate_receipt, ActivationObservation,
    DecisionFact, EventRefusal, Handover, LaneRequest, NodeEvent, NodeEventBody, OwnerReturn,
    Receipt, WorkTransition, EVENT_KINDS, FORBIDDEN_WIRE_FIELDS,
};
pub use import::{
    import_batch, local_revision_origin, mark_imported, read_handover, read_observations,
    ImportOutcome, Observation, RevisionOrigin,
};
pub use queue::{
    peer_observations, peer_queue_status, record_peer_observation, OutboundQueue, PeerQueueStatus,
    QueueEntry, QueueState, MAX_QUEUE_ENTRIES,
};

use std::path::Path;

/// Write `bytes` to `path` through a sibling temp file and an atomic rename, so
/// a reader never observes a half-written file. The temp file is flushed to
/// disk before the rename.
pub(crate) fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use anyhow::Context;
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    // The temp name carries the process id so two processes cannot collide, and
    // an interrupted write leaves only a `.tmp-<pid>` sibling behind.
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let write = || -> anyhow::Result<()> {
        let mut file =
            std::fs::File::create(&temp).with_context(|| format!("create {}", temp.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    };
    if let Err(error) = write() {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(error).with_context(|| format!("rename into {}", path.display()));
    }
    Ok(())
}

/// Serialize `value` as pretty JSON and write it atomically.
pub(crate) fn write_json_atomic(path: &Path, value: &impl serde::Serialize) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    write_bytes_atomic(path, &bytes)
}

/// RFC3339 millisecond UTC timestamp, the same shape the return mailbox uses.
pub(crate) fn now_rfc3339_millis() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| String::from("1970-01-01T00:00:00.000Z"))
}

/// The node's private directory under the Edda store root:
/// `<store>/node/`.
pub(crate) fn node_store_dir() -> std::path::PathBuf {
    edda_store::store_root().join("node")
}

#[cfg(test)]
mod tests;
