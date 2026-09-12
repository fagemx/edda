use edda_core::event::finalize_event;
use edda_core::{Event, Refs, SCHEMA_VERSION};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

pub(super) fn make_execution_brief_event(
    event_id: String,
    branch: &str,
    parent_hash: Option<&str>,
    payload: serde_json::Value,
    refs: Refs,
) -> anyhow::Result<Event> {
    finish_event(Event {
        event_id,
        ts: now_rfc3339()?,
        event_type: "execution_brief".into(),
        branch: branch.into(),
        parent_hash: parent_hash.map(str::to_string),
        hash: String::new(),
        payload,
        refs,
        schema_version: SCHEMA_VERSION,
        digests: vec![],
        event_family: None,
        event_level: None,
    })
}

pub(super) fn make_control_manifest_event(
    event_id: String,
    branch: &str,
    parent_hash: Option<&str>,
    payload: serde_json::Value,
    refs: Refs,
) -> anyhow::Result<Event> {
    finish_event(Event {
        event_id,
        ts: now_rfc3339()?,
        event_type: "control_manifest".into(),
        branch: branch.into(),
        parent_hash: parent_hash.map(str::to_string),
        hash: String::new(),
        payload,
        refs,
        schema_version: SCHEMA_VERSION,
        digests: vec![],
        event_family: None,
        event_level: None,
    })
}

pub(super) fn make_control_intent_event(
    event_id: String,
    branch: &str,
    parent_hash: Option<&str>,
    payload: serde_json::Value,
    refs: Refs,
) -> anyhow::Result<Event> {
    finish_event(Event {
        event_id,
        ts: now_rfc3339()?,
        event_type: "control_intent".into(),
        branch: branch.into(),
        parent_hash: parent_hash.map(str::to_string),
        hash: String::new(),
        payload,
        refs,
        schema_version: SCHEMA_VERSION,
        digests: vec![],
        event_family: None,
        event_level: None,
    })
}

pub(super) fn make_control_receipt_event(
    event_id: String,
    branch: &str,
    parent_hash: Option<&str>,
    payload: serde_json::Value,
    refs: Refs,
) -> anyhow::Result<Event> {
    finish_event(Event {
        event_id,
        ts: now_rfc3339()?,
        event_type: "control_receipt".into(),
        branch: branch.into(),
        parent_hash: parent_hash.map(str::to_string),
        hash: String::new(),
        payload,
        refs,
        schema_version: SCHEMA_VERSION,
        digests: vec![],
        event_family: None,
        event_level: None,
    })
}

fn finish_event(mut event: Event) -> anyhow::Result<Event> {
    finalize_event(&mut event)?;
    Ok(event)
}

pub(super) fn new_event_id() -> String {
    format!("evt_{}", ulid::Ulid::new().to_string().to_lowercase())
}

pub(super) fn deterministic_event_id(kind: &str, action_id: &str) -> String {
    let digest = edda_core::hash::sha256_hex(format!("control-{kind}:{action_id}").as_bytes());
    format!("evt_{}", &digest[..26])
}

pub(super) fn now_rfc3339() -> anyhow::Result<String> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}
