use super::types::*;
use super::validate::{canonical_runnable_bytes, validate_execution_brief};
use crate::event::finalize_event;
use crate::Event;

/// Parse and validate the data representation of an execution-brief event.
///
/// This proves only envelope integrity and internal consistency. It does not
/// confer execution authority: callers must use a product authority verifier,
/// which does not exist before S6.
pub fn parse_execution_brief_event(event: &Event) -> anyhow::Result<ExecutionBriefRecordV1> {
    if event.event_type != EXECUTION_BRIEF_EVENT_TYPE {
        anyhow::bail!("event is not an accepted execution brief");
    }
    let mut verified = event.clone();
    finalize_event(&mut verified)?;
    if verified.hash != event.hash
        || verified.digests != event.digests
        || verified.event_family != event.event_family
        || verified.event_level != event.event_level
    {
        anyhow::bail!("accepted execution brief event integrity mismatch");
    }
    if event
        .payload
        .get("trust")
        .and_then(serde_json::Value::as_str)
        != Some("locally_accepted")
    {
        anyhow::bail!("execution brief was not accepted through the local trust path");
    }
    let record: ExecutionBriefRecordV1 = serde_json::from_value(
        event
            .payload
            .get("execution_brief")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("execution brief event omits its typed record"))?,
    )
    .map_err(|_| anyhow::anyhow!("execution brief event has an unsupported record schema"))?;
    if record.record_version != EXECUTION_BRIEF_RECORD_VERSION {
        anyhow::bail!("execution brief event has an unsupported record version");
    }
    if record.trust != BriefTrustV1::LocallyAccepted
        || record.brief_event_id != event.event_id
        || record.brief.brief_event_id != event.event_id
    {
        anyhow::bail!("execution brief event identity or trust mismatch");
    }
    if record.content_digest != record.brief.content_digest {
        anyhow::bail!("execution brief record digest mismatch");
    }
    validate_authority(&record.authority)?;
    validate_execution_brief(&record.brief)?;
    let canonical = canonical_runnable_bytes(&record.brief)?;
    let stored = hex::decode(&record.canonical_bytes_hex)
        .map_err(|_| anyhow::anyhow!("execution brief canonical bytes are not valid hex"))?;
    if stored != canonical || crate::hash::sha256_hex(&stored) != record.content_digest {
        anyhow::bail!("execution brief canonical bytes or digest mismatch");
    }
    Ok(record)
}

fn validate_authority(authority: &ExecutionBriefAuthorityV1) -> anyhow::Result<()> {
    for (value, field) in [
        (&authority.principal_id, "authority.principal_id"),
        (&authority.session_id, "authority.session_id"),
    ] {
        if value.is_empty() || value.chars().count() > 160 {
            anyhow::bail!("execution brief {field} has an invalid shape");
        }
    }
    let event_id = &authority.authority_event_id;
    if !event_id.starts_with("evt_")
        || event_id.len() > 68
        || !event_id[4..]
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
    {
        anyhow::bail!("execution brief authority event identity has an invalid shape");
    }
    Ok(())
}
