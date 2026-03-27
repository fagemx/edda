use crate::types::{IngestionRecord, Suggestion, TriggerType};
use crate::writer::write_ingestion_record;
use edda_ledger::Ledger;

/// A queue for suggestions that need human review before ingestion.
///
/// Wraps the ledger's suggestion storage and provides business logic for
/// enqueueing, accepting, and rejecting suggestions.
pub struct SuggestionQueue<'a> {
    ledger: &'a Ledger,
}

impl<'a> SuggestionQueue<'a> {
    /// Create a new queue backed by the given ledger.
    pub fn new(ledger: &'a Ledger) -> Self {
        Self { ledger }
    }

    /// Enqueue a suggestion for human review. Returns the suggestion id.
    pub fn enqueue(&self, suggestion: &Suggestion) -> anyhow::Result<String> {
        let row = suggestion_to_row(suggestion)?;
        self.ledger.insert_suggestion(&row)?;
        Ok(suggestion.id.clone())
    }

    /// List all pending suggestions.
    pub fn list_pending(&self) -> anyhow::Result<Vec<Suggestion>> {
        let rows = self.ledger.list_suggestions_by_status("pending")?;
        rows.into_iter().map(row_to_suggestion).collect()
    }

    /// Accept a suggestion: write an IngestionRecord to the ledger and mark
    /// the suggestion as accepted. Returns the created IngestionRecord.
    pub fn accept(&self, id: &str) -> anyhow::Result<IngestionRecord> {
        let row = self
            .ledger
            .get_suggestion(id)?
            .ok_or_else(|| anyhow::anyhow!("suggestion not found: {id}"))?;

        if row.status != "pending" {
            anyhow::bail!(
                "suggestion {id} has status '{}', expected 'pending'",
                row.status
            );
        }

        let suggestion = row_to_suggestion(row)?;

        // Construct IngestionRecord from the suggestion.
        let record = IngestionRecord {
            id: IngestionRecord::new_id("prec"),
            trigger_type: TriggerType::Suggested,
            event_type: suggestion.event_type.clone(),
            source_layer: suggestion.source_layer,
            source_refs: suggestion.source_refs.clone(),
            summary: suggestion.summary.clone(),
            detail: suggestion.detail.clone(),
            tags: suggestion.tags.clone(),
            created_at: time_now_rfc3339(),
        };

        write_ingestion_record(self.ledger, &record)?;

        // Update suggestion status to accepted.
        let now = time_now_rfc3339();
        self.ledger.update_suggestion_status(id, "accepted", &now)?;

        Ok(record)
    }

    /// Reject a suggestion: mark as rejected without writing to the ledger.
    pub fn reject(&self, id: &str) -> anyhow::Result<()> {
        let row = self
            .ledger
            .get_suggestion(id)?
            .ok_or_else(|| anyhow::anyhow!("suggestion not found: {id}"))?;

        if row.status != "pending" {
            anyhow::bail!(
                "suggestion {id} has status '{}', expected 'pending'",
                row.status
            );
        }

        let now = time_now_rfc3339();
        self.ledger.update_suggestion_status(id, "rejected", &now)?;
        Ok(())
    }
}

// ── Conversion helpers ───────────────────────────────────────────────

fn suggestion_to_row(s: &Suggestion) -> anyhow::Result<edda_ledger::SuggestionRow> {
    Ok(edda_ledger::SuggestionRow {
        id: s.id.clone(),
        event_type: s.event_type.clone(),
        source_layer: s.source_layer.to_string(),
        source_refs: serde_json::to_string(&s.source_refs)?,
        summary: s.summary.clone(),
        suggested_because: s.suggested_because.clone(),
        detail: serde_json::to_string(&s.detail)?,
        tags: serde_json::to_string(&s.tags)?,
        status: s.status.to_string(),
        created_at: s.created_at.clone(),
        reviewed_at: s.reviewed_at.clone(),
    })
}

fn row_to_suggestion(row: edda_ledger::SuggestionRow) -> anyhow::Result<Suggestion> {
    let source_layer = row
        .source_layer
        .parse()
        .map_err(|e: String| anyhow::anyhow!(e))?;
    let status = row.status.parse().map_err(|e: String| anyhow::anyhow!(e))?;
    let source_refs = serde_json::from_str(&row.source_refs)?;
    let detail = serde_json::from_str(&row.detail)?;
    let tags = serde_json::from_str(&row.tags)?;

    Ok(Suggestion {
        id: row.id,
        event_type: row.event_type,
        source_layer,
        source_refs,
        summary: row.summary,
        suggested_because: row.suggested_because,
        detail,
        tags,
        status,
        created_at: row.created_at,
        reviewed_at: row.reviewed_at,
    })
}

fn time_now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SourceLayer, SourceRef, SuggestionStatus};

    fn make_suggestion(event_type: &str, reason: &str) -> Suggestion {
        Suggestion {
            id: Suggestion::new_id(),
            event_type: event_type.to_string(),
            source_layer: SourceLayer::L1,
            source_refs: vec![SourceRef {
                layer: SourceLayer::L1,
                kind: "test".to_string(),
                id: "ref_001".to_string(),
                note: None,
            }],
            summary: format!("Test suggestion for {event_type}"),
            suggested_because: reason.to_string(),
            detail: serde_json::json!({"context": "test"}),
            tags: vec!["test".to_string()],
            status: SuggestionStatus::Pending,
            created_at: time_now_rfc3339(),
            reviewed_at: None,
        }
    }

    #[test]
    fn enqueue_creates_pending() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::open_or_init(dir.path()).expect("ledger");
        let queue = SuggestionQueue::new(&ledger);

        let sug = make_suggestion("route.changed", "May indicate routing anti-pattern");
        let id = queue.enqueue(&sug).expect("enqueue");

        let pending = queue.list_pending().expect("list");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id);
        assert_eq!(pending[0].status, SuggestionStatus::Pending);
        assert_eq!(pending[0].event_type, "route.changed");
    }

    #[test]
    fn accept_writes_to_ledger() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::open_or_init(dir.path()).expect("ledger");
        let queue = SuggestionQueue::new(&ledger);

        let sug = make_suggestion("route.changed", "May indicate routing anti-pattern");
        let id = queue.enqueue(&sug).expect("enqueue");
        let record = queue.accept(&id).expect("accept");

        // Verify the IngestionRecord was written as a ledger event
        let events = ledger.iter_events_by_type("ingestion").expect("events");
        assert_eq!(events.len(), 1);

        // Verify the returned record
        assert!(record.id.starts_with("prec_"));
        assert_eq!(record.event_type, "route.changed");
        assert_eq!(record.summary, sug.summary);
    }

    #[test]
    fn accept_sets_trigger_type_suggested() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::open_or_init(dir.path()).expect("ledger");
        let queue = SuggestionQueue::new(&ledger);

        let sug = make_suggestion("probe.ambiguous", "Possible false positive");
        let id = queue.enqueue(&sug).expect("enqueue");
        let record = queue.accept(&id).expect("accept");

        assert_eq!(record.trigger_type, TriggerType::Suggested);
    }

    #[test]
    fn reject_does_not_write_to_ledger() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::open_or_init(dir.path()).expect("ledger");
        let queue = SuggestionQueue::new(&ledger);

        let sug = make_suggestion("route.changed", "May indicate routing anti-pattern");
        let id = queue.enqueue(&sug).expect("enqueue");
        queue.reject(&id).expect("reject");

        // No ingestion events written
        let events = ledger.iter_events_by_type("ingestion").expect("events");
        assert!(events.is_empty());

        // Pending list is empty
        let pending = queue.list_pending().expect("list");
        assert!(pending.is_empty());
    }

    #[test]
    fn accept_nonexistent_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::open_or_init(dir.path()).expect("ledger");
        let queue = SuggestionQueue::new(&ledger);

        let result = queue.accept("sug_nonexistent");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("suggestion not found"));
    }

    #[test]
    fn reject_nonexistent_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::open_or_init(dir.path()).expect("ledger");
        let queue = SuggestionQueue::new(&ledger);

        let result = queue.reject("sug_nonexistent");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("suggestion not found"));
    }

    #[test]
    fn double_accept_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::open_or_init(dir.path()).expect("ledger");
        let queue = SuggestionQueue::new(&ledger);

        let sug = make_suggestion("route.changed", "test");
        let id = queue.enqueue(&sug).expect("enqueue");
        queue.accept(&id).expect("first accept");

        let result = queue.accept(&id);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("accepted"));
    }

    #[test]
    fn double_reject_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::open_or_init(dir.path()).expect("ledger");
        let queue = SuggestionQueue::new(&ledger);

        let sug = make_suggestion("route.changed", "test");
        let id = queue.enqueue(&sug).expect("enqueue");
        queue.reject(&id).expect("first reject");

        let result = queue.reject(&id);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("rejected"));
    }

    #[test]
    fn list_pending_excludes_accepted_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::open_or_init(dir.path()).expect("ledger");
        let queue = SuggestionQueue::new(&ledger);

        let s1 = make_suggestion("route.changed", "test 1");
        let s2 = make_suggestion("probe.ambiguous", "test 2");
        let s3 = make_suggestion("chief.escalation", "test 3");

        let id1 = queue.enqueue(&s1).expect("enqueue 1");
        let id2 = queue.enqueue(&s2).expect("enqueue 2");
        let id3 = queue.enqueue(&s3).expect("enqueue 3");

        queue.accept(&id1).expect("accept");
        queue.reject(&id2).expect("reject");

        let pending = queue.list_pending().expect("list");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id3);
    }
}
