use crate::Ledger;
use edda_core::guided_execution::{
    parse_execution_brief_event, ExecutionBriefAuthorityV1, ExecutionBriefRecordV1,
    ExecutionBriefV1,
};

#[derive(Debug, Clone)]
pub struct AcceptedExecutionBriefV1 {
    pub event_id: String,
    pub content_digest: String,
    pub canonical_bytes: Vec<u8>,
    pub brief: ExecutionBriefV1,
    pub authority: ExecutionBriefAuthorityV1,
}

#[derive(Debug, thiserror::Error)]
#[error("exact execution brief read-back failed for event {event_id}")]
pub struct ExecutionBriefReadbackError {
    pub event_id: String,
    #[source]
    source: anyhow::Error,
}

impl Ledger {
    /// Renew a controlled lease only while its exact owner still owns it.
    pub fn renew_task_lease_owned(
        &self,
        task_id: u64,
        attempt: u32,
        owner: &str,
        expires_at: &str,
        heartbeat_at: &str,
    ) -> anyhow::Result<bool> {
        self.sqlite
            .renew_task_lease_owned(task_id, attempt, owner, expires_at, heartbeat_at)
            .map_err(|error| error.context("Ledger::renew_task_lease_owned"))
    }

    #[cfg(test)]
    pub(crate) fn append_event_with_test_execution_authority(
        &self,
        event: &edda_core::Event,
    ) -> anyhow::Result<()> {
        self.sqlite
            .append_event_with_test_execution_authority(event)
            .map_err(|error| {
                error.context(format!(
                    "Ledger::append_event_with_test_execution_authority({})",
                    event.event_id
                ))
            })
    }

    /// Load a runnable brief only when its exact control manifest carries a
    /// valid local S6a authority seal. A self-consistent/imported event remains
    /// data: the local capability, repository binding, RBAC grant, manifest
    /// signature, exact event identity, digest, and canonical bytes must all
    /// verify before this returns procedure.
    pub fn load_execution_brief(
        &self,
        event_id: &str,
        content_digest: &str,
    ) -> anyhow::Result<AcceptedExecutionBriefV1> {
        validate_identity(event_id, content_digest)?;
        let event = self
            .get_event(event_id)?
            .ok_or_else(|| anyhow::anyhow!("execution brief event not found"))?;
        self.load_sealed_execution_brief(&event, content_digest)
    }
}

pub(crate) fn execution_brief_data(
    event: &edda_core::Event,
    expected_digest: &str,
) -> anyhow::Result<AcceptedExecutionBriefV1> {
    let record = parse_execution_brief_event(event)?;
    if record.content_digest != expected_digest {
        anyhow::bail!("execution brief digest does not match requested identity");
    }
    entry(record)
}

fn entry(record: ExecutionBriefRecordV1) -> anyhow::Result<AcceptedExecutionBriefV1> {
    let canonical_bytes = hex::decode(&record.canonical_bytes_hex)
        .map_err(|_| anyhow::anyhow!("execution brief bytes are malformed"))?;
    Ok(AcceptedExecutionBriefV1 {
        event_id: record.brief_event_id,
        content_digest: record.content_digest,
        canonical_bytes,
        brief: record.brief,
        authority: record.authority,
    })
}

fn validate_identity(event_id: &str, content_digest: &str) -> anyhow::Result<()> {
    if !event_id.starts_with("evt_")
        || event_id.len() > 68
        || !event_id[4..]
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
    {
        anyhow::bail!("execution brief event identity has an invalid shape");
    }
    if content_digest.len() != 64
        || !content_digest
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
    {
        anyhow::bail!("execution brief digest has an invalid shape");
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use edda_core::event::finalize_event;
    use edda_core::guided_execution::*;
    use edda_core::{Event, Refs, SCHEMA_VERSION};

    pub(crate) fn input(task_ref: Option<u64>) -> ExecutionBriefInputV1 {
        ExecutionBriefInputV1 {
            brief_version: 1,
            brief_id: "brief_ledger1".into(),
            task_ref,
            runtime_profile: RuntimeProfileV1::Strong,
            intent: ExecutionIntentV1::Document,
            objective: "verify immutable storage".into(),
            basis: ExecutionBasisV1 {
                portable_repo_id: None,
                base_full_sha: "a".repeat(40),
                issue_spec_refs: vec![],
            },
            scope: ExecutionScopeV1 {
                allowed_paths: vec!["docs/**".into()],
                out_of_scope: vec![],
            },
            read_order: vec![],
            known_facts: vec![],
            allowed_decisions: vec![],
            return_for_decision: vec![],
            procedure: TrustedProcedureV1::ControllerAuthored {
                authored_by: "controller".into(),
                principles: vec!["preserve exact bytes".into()],
                probe_cards: vec![],
                implementation_steps: vec![],
                validation: vec![],
            },
            outcome_codes: vec![OutcomeCodeV1 {
                code: "DONE".into(),
                result_class: ResultClassV1::Success,
            }],
            receipt_schema: ReceiptSchemaV1 {
                receipt_version: 1,
                required_fields: vec![
                    ReceiptFieldV1::BriefIdentity,
                    ReceiptFieldV1::OutcomeCode,
                    ReceiptFieldV1::ChangedPaths,
                    ReceiptFieldV1::ValidationRan,
                    ReceiptFieldV1::ValidationRead,
                    ReceiptFieldV1::RecommendedNextAction,
                ],
            },
        }
    }

    pub(crate) fn data_event(
        branch: &str,
        parent_hash: Option<&str>,
        input: ExecutionBriefInputV1,
    ) -> anyhow::Result<Event> {
        let event_id = format!("evt_{}", ulid::Ulid::new().to_string().to_lowercase());
        let authority = ExecutionBriefAuthorityV1 {
            principal_id: "controller".into(),
            session_id: "controller-session".into(),
            authority_event_id: "evt_authority1".into(),
        };
        let (brief, canonical_bytes) =
            compile_execution_brief(input, event_id.clone(), &authority.principal_id)?;
        let record = ExecutionBriefRecordV1 {
            record_version: EXECUTION_BRIEF_RECORD_VERSION,
            trust: BriefTrustV1::LocallyAccepted,
            authority,
            brief_event_id: event_id.clone(),
            content_digest: brief.content_digest.clone(),
            canonical_bytes_hex: hex::encode(canonical_bytes),
            brief,
        };
        let mut event = Event {
            event_id,
            ts: "2026-09-11T00:00:00Z".into(),
            event_type: EXECUTION_BRIEF_EVENT_TYPE.into(),
            branch: branch.into(),
            parent_hash: parent_hash.map(str::to_string),
            hash: String::new(),
            payload: serde_json::json!({
                "trust": "locally_accepted",
                "execution_brief": record,
            }),
            refs: Refs::default(),
            schema_version: SCHEMA_VERSION,
            digests: Vec::new(),
            event_family: None,
            event_level: None,
        };
        finalize_event(&mut event)?;
        Ok(event)
    }

    #[test]
    fn public_event_append_is_data_only_and_cannot_mint_runnable_authority() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_or_init(tmp.path()).unwrap();
        let event = data_event("main", None, input(None)).unwrap();
        let digest = parse_execution_brief_event(&event).unwrap().content_digest;

        // This is the exact arbitrary-local-caller capability: a public Event
        // plus public generic append. Internal consistency is not a seal.
        ledger.append_event(&event).unwrap();
        let error = ledger
            .load_execution_brief(&event.event_id, &digest)
            .unwrap_err()
            .to_string();
        assert!(error.contains("authority manifest is missing"), "{error}");
        assert_eq!(ledger.count_events().unwrap(), 1);
    }

    #[test]
    fn malformed_or_wrong_digest_data_is_refused_before_authority_evaluation() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = Ledger::open_or_init(tmp.path()).unwrap();
        let event = data_event("main", None, input(None)).unwrap();
        ledger.append_event(&event).unwrap();
        assert!(ledger
            .load_execution_brief(&event.event_id, &"0".repeat(64))
            .is_err());
    }
}
