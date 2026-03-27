use crate::types::IngestionRecord;
use edda_core::event::finalize_event;
use edda_core::types::{Event, Refs, SCHEMA_VERSION};
use edda_ledger::Ledger;

/// Write an ingestion record to the ledger as an "ingestion" event.
///
/// The record is serialized as the event payload, wrapped in the standard
/// hash-chained Event envelope.
pub fn write_ingestion_record(ledger: &Ledger, record: &IngestionRecord) -> anyhow::Result<()> {
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    let payload = serde_json::to_value(record)?;

    let ts = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail");

    let mut event = Event {
        event_id: format!("evt_{}", ulid::Ulid::new().to_string().to_lowercase()),
        ts,
        event_type: "ingestion".to_string(),
        branch,
        parent_hash,
        hash: String::new(),
        payload,
        refs: Refs::default(),
        schema_version: SCHEMA_VERSION,
        digests: Vec::new(),
        event_family: None,
        event_level: None,
    };

    finalize_event(&mut event)?;
    ledger.append_event(&event)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{IngestionRecord, SourceLayer};

    #[test]
    fn write_and_read_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::open_or_init(dir.path()).expect("ledger");

        let record = IngestionRecord {
            id: IngestionRecord::new_id("prec"),
            trigger_type: "auto".to_string(),
            event_type: "decision.commit".to_string(),
            source_layer: SourceLayer::L1,
            source_refs: vec![],
            summary: "Formal decision committed".to_string(),
            detail: serde_json::json!({"session": "ds_test"}),
            tags: vec!["decision".to_string()],
            created_at: "2025-01-01T00:00:00Z".to_string(),
        };

        write_ingestion_record(&ledger, &record).expect("write");

        // Verify the event was written
        let events = ledger.iter_events_by_type("ingestion").expect("read");
        assert_eq!(events.len(), 1);

        let event = &events[0];
        assert_eq!(event.event_type, "ingestion");
        assert!(!event.hash.is_empty());

        // Verify payload round-trips back to IngestionRecord
        let back: IngestionRecord =
            serde_json::from_value(event.payload.clone()).expect("deserialize");
        assert_eq!(back.id, record.id);
        assert_eq!(back.trigger_type, "auto");
        assert_eq!(back.event_type, "decision.commit");
        assert_eq!(back.source_layer, SourceLayer::L1);
        assert_eq!(back.summary, "Formal decision committed");
        assert_eq!(back.tags, vec!["decision".to_string()]);
    }

    #[test]
    fn write_chains_parent_hash() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::open_or_init(dir.path()).expect("ledger");

        let make_record = |event_type: &str| IngestionRecord {
            id: IngestionRecord::new_id("prec"),
            trigger_type: "auto".to_string(),
            event_type: event_type.to_string(),
            source_layer: SourceLayer::L1,
            source_refs: vec![],
            summary: format!("Test {event_type}"),
            detail: serde_json::json!({}),
            tags: vec![],
            created_at: "2025-01-01T00:00:00Z".to_string(),
        };

        write_ingestion_record(&ledger, &make_record("decision.commit")).expect("write 1");
        write_ingestion_record(&ledger, &make_record("decision.discard")).expect("write 2");

        let events = ledger.iter_events().expect("read");
        assert_eq!(events.len(), 2);

        // Second event's parent_hash should reference first event's hash
        assert_eq!(
            events[1].parent_hash.as_deref(),
            Some(events[0].hash.as_str())
        );
    }
}
