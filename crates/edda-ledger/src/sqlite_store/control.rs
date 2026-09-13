use super::events::validate_event_hash;
use super::SqliteStore;
use edda_core::Event;
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};

impl SqliteStore {
    /// Atomically append the execution briefs and control manifest produced by
    /// the sealed S6a compiler. This is deliberately crate-private and accepts
    /// only those two event types, so generic callers cannot use a batch API to
    /// mint control authority or bypass task materialization.
    pub(crate) fn append_control_compile_batch(&self, events: &[Event]) -> anyhow::Result<()> {
        anyhow::ensure!(!events.is_empty(), "control compile batch cannot be empty");
        anyhow::ensure!(
            events.iter().all(|event| matches!(
                event.event_type.as_str(),
                "execution_brief" | "control_manifest"
            )),
            "control compile batch contains an unsupported event type"
        );
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let mut expected_parent: Option<String> = tx
            .query_row(
                "SELECT hash FROM events ORDER BY rowid DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        for event in events {
            let event_value = serde_json::to_value(event)?;
            edda_core::event::validate_readable_payload(&event_value)?;
            anyhow::ensure!(
                event.parent_hash == expected_parent,
                "event {} has stale parent_hash in control compile batch",
                event.event_id
            );
            validate_event_hash(event)?;
            tx.execute(
                "INSERT INTO events (
                    event_id, ts, event_type, branch, parent_hash, hash,
                    payload, refs_blobs, refs_events, refs_provenance,
                    schema_version, digests, event_family, event_level
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    event.event_id,
                    event.ts,
                    event.event_type,
                    event.branch,
                    event.parent_hash,
                    event.hash,
                    serde_json::to_string(&event.payload)?,
                    serde_json::to_string(&event.refs.blobs)?,
                    serde_json::to_string(&event.refs.events)?,
                    serde_json::to_string(&event.refs.provenance)?,
                    event.schema_version,
                    serde_json::to_string(&event.digests)?,
                    event.event_family,
                    event.event_level,
                ],
            )?;
            expected_parent = Some(event.hash.clone());
        }
        tx.commit()?;
        Ok(())
    }
}
