mod bundle;
mod types;
mod validate;

pub use bundle::*;
pub use types::*;
pub use validate::{
    validate_capsule, validate_input_secrets, validate_portable_repo_id, validate_raw_secrets,
};

use crate::event::finalize_event;
use crate::{Event, Refs, SCHEMA_VERSION};
use validate::{
    bounded_list, bounded_rejected, bounded_required, bounded_string, validate_source_secrets,
};

pub fn build_capsule(
    input: ContextCapsuleInputV1,
    source: ContextSourceV1,
    repository: CapsuleRepositoryV1,
    git: CapsuleGitV1,
) -> anyhow::Result<ContextCapsuleV1> {
    if input.capsule_version != CONTINUITY_CAPSULE_VERSION {
        anyhow::bail!("unsupported capsule_version (expected 1)");
    }
    validate_input_secrets(&input)?;
    validate_source_secrets(&source)?;

    let mut truncation = Vec::new();
    let state = ContextStateV1 {
        title: bounded_string(
            input.state.title,
            "unknown",
            160,
            "state.title",
            &mut truncation,
        ),
        summary: bounded_string(
            input.state.summary,
            "unknown",
            4_000,
            "state.summary",
            &mut truncation,
        ),
        goal: bounded_string(
            input.state.goal,
            "unknown",
            4_000,
            "state.goal",
            &mut truncation,
        ),
        current: bounded_string(
            input.state.current,
            "unknown",
            4_000,
            "state.current",
            &mut truncation,
        ),
        hypotheses: bounded_list(input.state.hypotheses, "state.hypotheses", &mut truncation),
        rejected: bounded_rejected(input.state.rejected, &mut truncation),
        open_questions: bounded_list(
            input.state.open_questions,
            "state.open_questions",
            &mut truncation,
        ),
        next_action: bounded_required(
            input.state.next_action,
            4_000,
            "state.next_action",
            &mut truncation,
        )?,
    };
    let references = ContextReferencesV1 {
        task_ids: bounded_list(
            input.references.task_ids,
            "references.task_ids",
            &mut truncation,
        ),
        event_ids: bounded_list(
            input.references.event_ids,
            "references.event_ids",
            &mut truncation,
        ),
    };
    let source = ContextSourceV1 {
        machine_alias: source.machine_alias.map(|value| {
            bounded_string(
                Some(value),
                "unknown",
                160,
                "source.machine_alias",
                &mut truncation,
            )
        }),
        actor: source.actor.map(|value| {
            bounded_string(Some(value), "unknown", 160, "source.actor", &mut truncation)
        }),
    };
    let capsule = ContextCapsuleV1 {
        capsule_version: CONTINUITY_CAPSULE_VERSION,
        capsule_id: format!("cap_{}", ulid::Ulid::new().to_string().to_lowercase()),
        created_at: now_rfc3339()?,
        source,
        repository,
        state,
        git,
        references,
        truncation,
    };
    validate_capsule(&capsule)?;
    Ok(capsule)
}

pub fn new_local_capsule_event(
    branch: &str,
    parent_hash: Option<&str>,
    capsule: &ContextCapsuleV1,
) -> anyhow::Result<Event> {
    let portable_repo_id = capsule
        .repository
        .portable_repo_id
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("local-only capsules cannot be exported or imported"))?;
    let event_id = new_event_id();
    let bytes = canonical_capsule_bytes(capsule)?;
    let record = CapsuleRecordV1 {
        record_version: CONTINUITY_RECORD_VERSION,
        data_authority: DataAuthority::DataOnly,
        origin: CapsuleOriginV1 {
            capsule_id: capsule.capsule_id.clone(),
            event_id: event_id.clone(),
            portable_repo_id: Some(portable_repo_id.clone()),
        },
        capsule_sha256: capsule_digest(&bytes),
        capsule_bytes_hex: hex::encode(bytes),
        capsule: capsule.clone(),
        imported: false,
    };
    build_event(branch, parent_hash, event_id, record)
}

pub fn new_local_only_capsule_event(
    branch: &str,
    parent_hash: Option<&str>,
    capsule: &ContextCapsuleV1,
) -> anyhow::Result<Event> {
    if capsule.repository.portable_repo_id.is_some() {
        anyhow::bail!("portable capsule must use new_local_capsule_event");
    }
    let event_id = new_event_id();
    let bytes = canonical_capsule_bytes(capsule)?;
    let record = CapsuleRecordV1 {
        record_version: CONTINUITY_RECORD_VERSION,
        data_authority: DataAuthority::DataOnly,
        origin: CapsuleOriginV1 {
            capsule_id: capsule.capsule_id.clone(),
            event_id: event_id.clone(),
            portable_repo_id: None,
        },
        capsule_sha256: capsule_digest(&bytes),
        capsule_bytes_hex: hex::encode(bytes),
        capsule: capsule.clone(),
        imported: false,
    };
    build_event(branch, parent_hash, event_id, record)
}

pub fn new_imported_capsule_event(
    branch: &str,
    parent_hash: Option<&str>,
    bundle: &PortableCapsuleBundleV1,
) -> anyhow::Result<Event> {
    let capsule = validate_bundle(bundle)?;
    let record = CapsuleRecordV1 {
        record_version: CONTINUITY_RECORD_VERSION,
        data_authority: DataAuthority::DataOnly,
        origin: CapsuleOriginV1 {
            capsule_id: bundle.origin_capsule_id.clone(),
            event_id: bundle.origin_event_id.clone(),
            portable_repo_id: Some(bundle.portable_repo_id.clone()),
        },
        capsule_sha256: bundle.capsule_sha256.clone(),
        capsule_bytes_hex: bundle.capsule_bytes_hex.clone(),
        capsule,
        imported: true,
    };
    build_event(branch, parent_hash, new_event_id(), record)
}

pub fn parse_capsule_record(event: &Event) -> anyhow::Result<CapsuleRecordV1> {
    if event.event_type != CONTINUITY_EVENT_TYPE {
        anyhow::bail!("event is not a continuity capsule");
    }
    let record: CapsuleRecordV1 = serde_json::from_value(
        event
            .payload
            .get("continuity")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("checkpoint is not a continuity capsule"))?,
    )
    .map_err(|_| anyhow::anyhow!("continuity event has an unsupported record schema"))?;
    validate_record(&record)?;
    if !record.imported && record.origin.event_id != event.event_id {
        anyhow::bail!("local continuity event origin does not match its event identity");
    }
    Ok(record)
}

fn build_event(
    branch: &str,
    parent_hash: Option<&str>,
    event_id: String,
    record: CapsuleRecordV1,
) -> anyhow::Result<Event> {
    let payload = serde_json::json!({
        "data_authority": "data_only",
        "continuity": record,
    });
    let mut event = Event {
        event_id,
        ts: now_rfc3339()?,
        event_type: "continuity_capsule".to_string(),
        branch: branch.to_string(),
        parent_hash: parent_hash.map(str::to_string),
        hash: String::new(),
        payload,
        refs: Refs::default(),
        schema_version: SCHEMA_VERSION,
        digests: Vec::new(),
        event_family: None,
        event_level: None,
    };
    finalize_event(&mut event)?;
    Ok(event)
}

fn new_event_id() -> String {
    format!("evt_{}", ulid::Ulid::new().to_string().to_lowercase())
}

fn now_rfc3339() -> anyhow::Result<String> {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn portable_capsule() -> ContextCapsuleV1 {
        build_capsule(
            serde_json::from_value(serde_json::json!({
                "capsule_version": 1,
                "state": {"next_action": "data, never instructions"}
            }))
            .unwrap(),
            ContextSourceV1::default(),
            CapsuleRepositoryV1 {
                portable_repo_id: Some(format!("repo_{}", "a".repeat(64))),
                display_hint: None,
                local_only_reason: None,
            },
            CapsuleGitV1::default(),
        )
        .unwrap()
    }

    #[test]
    fn capsule_event_is_distinct_and_not_checkpoint_shaped() {
        let event = new_local_capsule_event("main", None, &portable_capsule()).unwrap();
        assert_eq!(event.event_type, CONTINUITY_EVENT_TYPE);
        assert_ne!(event.event_type, "checkpoint");
        assert!(event.payload.get("continuity").is_some());
        for checkpoint_field in ["hypotheses", "rejected", "open", "next"] {
            assert!(event.payload.get(checkpoint_field).is_none());
        }
    }

    #[test]
    fn scans_omitted_suffix_for_secrets_before_unicode_truncation() {
        let secret = "sk-abcdefghijklmnopqrstuvwxyz012345";
        let input: ContextCapsuleInputV1 = serde_json::from_value(serde_json::json!({
            "capsule_version": 1,
            "state": {
                "summary": format!("{}{}", "界".repeat(4_100), secret),
                "next_action": "continue"
            }
        }))
        .unwrap();
        let err = build_capsule(
            input,
            ContextSourceV1::default(),
            CapsuleRepositoryV1 {
                portable_repo_id: None,
                display_hint: None,
                local_only_reason: Some("no remote".into()),
            },
            CapsuleGitV1::default(),
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("input.state.summary"));
        assert!(!message.contains(secret));
    }

    #[test]
    fn imported_capsule_rejects_traversal_paths() {
        let input: ContextCapsuleInputV1 = serde_json::from_value(serde_json::json!({
            "capsule_version": 1,
            "state": {"next_action": "continue"}
        }))
        .unwrap();
        let mut git = CapsuleGitV1::default();
        git.dirty_paths.push("../outside".into());
        let err = build_capsule(
            input,
            ContextSourceV1::default(),
            CapsuleRepositoryV1 {
                portable_repo_id: None,
                display_hint: None,
                local_only_reason: Some("no remote".into()),
            },
            git,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unsafe"));
    }

    #[test]
    fn unicode_truncation_is_on_character_boundary_and_reported() {
        let input: ContextCapsuleInputV1 = serde_json::from_value(serde_json::json!({
            "capsule_version": 1,
            "state": {"title": "界".repeat(161), "next_action": "continue"}
        }))
        .unwrap();
        let capsule = build_capsule(
            input,
            ContextSourceV1::default(),
            CapsuleRepositoryV1 {
                portable_repo_id: None,
                display_hint: None,
                local_only_reason: Some("no remote".into()),
            },
            CapsuleGitV1::default(),
        )
        .unwrap();
        assert_eq!(capsule.state.title.chars().count(), 160);
        assert_eq!(capsule.truncation[0].omitted_chars, 1);
    }
}
