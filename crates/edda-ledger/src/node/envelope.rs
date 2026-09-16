//! Allowlisted node wire envelopes (frozen contract §4).
//!
//! Five kinds, exactly the field sets in the frozen contract. Everything is
//! parsed explicitly through [`parse_event`] rather than with
//! `#[serde(flatten)]`, because serde does not support `flatten` together with
//! `deny_unknown_fields` — and "an unknown field is refused, never ignored" is
//! the whole point of this module.
//!
//! # Canonical identity
//!
//! ```text
//! eventId = sha256_hex(kind + "\0" + join("\0", declared_field_values_in_declaration_order))
//! ```
//!
//! where an absent `Option` contributes the empty string and a `bool`
//! contributes `"true"` / `"false"`. The field order is the order the fields
//! are declared in the struct below, not the order JSON happened to use.
//!
//! [`OwnerReturn::logical_id`] keeps GH-1236's algorithm verbatim:
//! `sha256_hex(owner\0work\0status\0result\0deliverable\0message)` with `""`
//! for absent options.
//!
//! Security-negative: `session`, `sessionId`, `runId`, `path`, `root`,
//! `ownerRoot`, `registry`, `registryRoot`, `lease`, `worktree`, `token`,
//! `credential`, `secret`, `env` and `transcript` are refused **by name**; any
//! other field outside the kind's allowlist is refused as unknown. A tampered
//! `eventId` (one that does not match the recomputed value) is refused.

use serde::{Deserialize, Serialize};

/// Forbidden wire keys, refused with their own reason (contract §4).
pub const FORBIDDEN_WIRE_FIELDS: &[&str] = &[
    "session",
    "sessionId",
    "runId",
    "path",
    "root",
    "ownerRoot",
    "registry",
    "registryRoot",
    "lease",
    "worktree",
    "token",
    "credential",
    "secret",
    "env",
    "transcript",
];

const COMMON_FIELDS: &[&str] = &["eventId", "kind"];

const OWNER_RETURN_FIELDS: &[&str] = &[
    "logicalId",
    "messageId",
    "owner",
    "work",
    "status",
    "result",
    "deliverable",
    "message",
    "postedAt",
    "originMachine",
];

const DECISION_FACT_FIELDS: &[&str] = &[
    "key",
    "value",
    "scope",
    "ratified",
    "actor",
    "originMachine",
    "ts",
];

const WORK_TRANSITION_FIELDS: &[&str] = &[
    "workId",
    "fromState",
    "toState",
    "receiptId",
    "actor",
    "originMachine",
    "ts",
];

const HANDOVER_FIELDS: &[&str] = &[
    "owner",
    "fromHolder",
    "toHolder",
    "fromMachine",
    "toMachine",
    "note",
    "ts",
];

const ACTIVATION_OBSERVATION_FIELDS: &[&str] =
    &["machine", "revision", "observedAt", "originMachine"];

const LANE_REQUEST_FIELDS: &[&str] = &[
    "requestId",
    "fromLabel",
    "toLabel",
    "message",
    "ts",
    "originMachine",
];

const RECEIPT_FIELDS: &[&str] = &[
    "ofEventId",
    "ofKind",
    "state",
    "actor",
    "ts",
    "originMachine",
];

/// One refusal. `index` is the position of the event in the batch it was
/// refused from; `parse_event` always produces index `0` and the caller
/// re-anchors it with [`EventRefusal::at`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRefusal {
    pub index: usize,
    pub reason: String,
}

impl EventRefusal {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            index: 0,
            reason: reason.into(),
        }
    }

    /// Re-anchor this refusal at its position in the enclosing batch.
    pub fn at(mut self, index: usize) -> Self {
        self.index = index;
        self
    }
}

impl std::fmt::Display for EventRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "events[{}]: {}", self.index, self.reason)
    }
}

impl std::error::Error for EventRefusal {}

/// Verbatim GH-1236 `ReplicateEnvelope`. `logicalId` is recomputed and
/// compared; a mismatch is refused.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnerReturn {
    pub logical_id: String,
    pub message_id: String,
    pub owner: String,
    pub work: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deliverable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub posted_at: String,
    pub origin_machine: String,
}

/// A transported decision fact. Provenance is preserved: the original actor and
/// the origin machine both survive import.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionFact {
    pub key: String,
    pub value: String,
    pub scope: String,
    pub ratified: bool,
    pub actor: String,
    pub origin_machine: String,
    pub ts: String,
}

/// A task/work lifecycle transition. Recorded, never replayed into the task
/// rail.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkTransition {
    pub work_id: String,
    pub from_state: String,
    pub to_state: String,
    pub receipt_id: String,
    pub actor: String,
    pub origin_machine: String,
    pub ts: String,
}

/// An explicit cross-machine handover (D4).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Handover {
    pub owner: String,
    pub from_holder: String,
    pub to_holder: String,
    pub from_machine: String,
    pub to_machine: String,
    pub note: String,
    pub ts: String,
}

/// A local activation observation with origin and freshness. Never a global
/// lock and never a coherence claim.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivationObservation {
    pub machine: String,
    pub revision: String,
    pub observed_at: String,
    pub origin_machine: String,
}

/// A cross-machine lane request. `requestId` is the existing `RequestEntry.id`;
/// the sender's session id is deliberately absent (only labels cross).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaneRequest {
    pub request_id: String,
    pub from_label: String,
    pub to_label: String,
    pub message: String,
    pub ts: String,
    pub origin_machine: String,
}

/// The reverse-direction receipt for an earlier event id.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub of_event_id: String,
    pub of_kind: String,
    /// `delivered` or `acked`; anything else is refused.
    pub state: String,
    pub actor: String,
    pub ts: String,
    pub origin_machine: String,
}

/// The allowlisted bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeEventBody {
    OwnerReturn(OwnerReturn),
    DecisionFact(DecisionFact),
    WorkTransition(WorkTransition),
    Handover(Handover),
    ActivationObservation(ActivationObservation),
    LaneRequest(LaneRequest),
    Receipt(Receipt),
}

impl NodeEventBody {
    /// The wire `kind` string of this body.
    pub fn kind(&self) -> &'static str {
        match self {
            NodeEventBody::OwnerReturn(_) => "owner_return",
            NodeEventBody::DecisionFact(_) => "decision_fact",
            NodeEventBody::WorkTransition(_) => "work_transition",
            NodeEventBody::Handover(_) => "handover",
            NodeEventBody::ActivationObservation(_) => "activation_observation",
            NodeEventBody::LaneRequest(_) => "lane_request",
            NodeEventBody::Receipt(_) => "receipt",
        }
    }

    /// The machine that declared this event.
    pub fn origin_machine(&self) -> &str {
        match self {
            NodeEventBody::OwnerReturn(body) => &body.origin_machine,
            NodeEventBody::DecisionFact(body) => &body.origin_machine,
            NodeEventBody::WorkTransition(body) => &body.origin_machine,
            NodeEventBody::Handover(body) => &body.from_machine,
            NodeEventBody::ActivationObservation(body) => &body.origin_machine,
            NodeEventBody::LaneRequest(body) => &body.origin_machine,
            NodeEventBody::Receipt(body) => &body.origin_machine,
        }
    }

    /// Field values in struct declaration order, with `""` for absent options.
    pub fn canonical_values(&self) -> Vec<String> {
        match self {
            NodeEventBody::OwnerReturn(body) => vec![
                body.logical_id.clone(),
                body.message_id.clone(),
                body.owner.clone(),
                body.work.clone(),
                body.status.clone(),
                body.result.clone().unwrap_or_default(),
                body.deliverable.clone().unwrap_or_default(),
                body.message.clone().unwrap_or_default(),
                body.posted_at.clone(),
                body.origin_machine.clone(),
            ],
            NodeEventBody::DecisionFact(body) => vec![
                body.key.clone(),
                body.value.clone(),
                body.scope.clone(),
                body.ratified.to_string(),
                body.actor.clone(),
                body.origin_machine.clone(),
                body.ts.clone(),
            ],
            NodeEventBody::WorkTransition(body) => vec![
                body.work_id.clone(),
                body.from_state.clone(),
                body.to_state.clone(),
                body.receipt_id.clone(),
                body.actor.clone(),
                body.origin_machine.clone(),
                body.ts.clone(),
            ],
            NodeEventBody::Handover(body) => vec![
                body.owner.clone(),
                body.from_holder.clone(),
                body.to_holder.clone(),
                body.from_machine.clone(),
                body.to_machine.clone(),
                body.note.clone(),
                body.ts.clone(),
            ],
            NodeEventBody::ActivationObservation(body) => vec![
                body.machine.clone(),
                body.revision.clone(),
                body.observed_at.clone(),
                body.origin_machine.clone(),
            ],
            NodeEventBody::LaneRequest(body) => vec![
                body.request_id.clone(),
                body.from_label.clone(),
                body.to_label.clone(),
                body.message.clone(),
                body.ts.clone(),
                body.origin_machine.clone(),
            ],
            NodeEventBody::Receipt(body) => vec![
                body.of_event_id.clone(),
                body.of_kind.clone(),
                body.state.clone(),
                body.actor.clone(),
                body.ts.clone(),
                body.origin_machine.clone(),
            ],
        }
    }

    /// Serialize the body fields (without `eventId` / `kind`) as a JSON object.
    pub fn to_value(&self) -> serde_json::Value {
        match self {
            NodeEventBody::OwnerReturn(body) => serde_json::to_value(body),
            NodeEventBody::DecisionFact(body) => serde_json::to_value(body),
            NodeEventBody::WorkTransition(body) => serde_json::to_value(body),
            NodeEventBody::Handover(body) => serde_json::to_value(body),
            NodeEventBody::ActivationObservation(body) => serde_json::to_value(body),
            NodeEventBody::LaneRequest(body) => serde_json::to_value(body),
            NodeEventBody::Receipt(body) => serde_json::to_value(body),
        }
        .unwrap_or(serde_json::Value::Null)
    }
}

/// One validated wire event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeEvent {
    pub event_id: String,
    pub body: NodeEventBody,
}

impl NodeEvent {
    pub fn new(body: NodeEventBody) -> Self {
        let event_id = compute_event_id(body.kind(), &body.canonical_values());
        Self { event_id, body }
    }

    pub fn kind(&self) -> &'static str {
        self.body.kind()
    }

    pub fn origin_machine(&self) -> &str {
        self.body.origin_machine()
    }

    /// The full wire object: body fields plus `eventId` and `kind`.
    pub fn to_wire_value(&self) -> serde_json::Value {
        let mut object = match self.body.to_value() {
            serde_json::Value::Object(object) => object,
            _ => serde_json::Map::new(),
        };
        object.insert(
            "eventId".to_string(),
            serde_json::Value::String(self.event_id.clone()),
        );
        object.insert(
            "kind".to_string(),
            serde_json::Value::String(self.body.kind().to_string()),
        );
        serde_json::Value::Object(object)
    }
}

/// Canonical `eventId` for a kind and its declared field values.
pub fn compute_event_id(kind: &str, values: &[String]) -> String {
    let mut canonical = String::from(kind);
    canonical.push('\u{0}');
    canonical.push_str(&values.join("\u{0}"));
    edda_core::hash::sha256_hex(canonical.as_bytes())
}

/// Lowercase sha256 hex only. Ids on the wire can never name a path.
pub fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// GH-1236 label rule: 1..=200 characters, no control characters.
pub fn validate_label(what: &str, value: &str) -> anyhow::Result<()> {
    if value.is_empty() || value.chars().count() > 200 || value.chars().any(char::is_control) {
        anyhow::bail!("invalid {what}: expected 1..=200 characters without control characters");
    }
    Ok(())
}

/// A machine-shaped wire field: a bare `<machine>` label
/// (`^[a-z0-9._-]{1,64}$`), never free text. The reason names the field and the
/// constraint, so a peer sees exactly what was refused.
fn validate_machine_field(field: &str, value: &str) -> anyhow::Result<()> {
    super::config::validate_machine_label(value)
        .map_err(|error| anyhow::anyhow!("invalid {field}: {error}"))
}

/// Every machine-shaped value on the wire must be a machine label (contract
/// §4). A value containing `/`, `\`, `..`, `:` or an absolute path could
/// otherwise reach a filesystem path once it is stored as transport
/// provenance (`via_machine`).
fn validate_machine_shape(body: &NodeEventBody) -> anyhow::Result<()> {
    match body {
        NodeEventBody::OwnerReturn(body) => {
            validate_machine_field("originMachine", &body.origin_machine)
        }
        NodeEventBody::DecisionFact(body) => {
            validate_machine_field("originMachine", &body.origin_machine)
        }
        NodeEventBody::WorkTransition(body) => {
            validate_machine_field("originMachine", &body.origin_machine)
        }
        NodeEventBody::Handover(body) => {
            validate_machine_field("fromMachine", &body.from_machine)?;
            validate_machine_field("toMachine", &body.to_machine)
        }
        NodeEventBody::ActivationObservation(body) => {
            validate_machine_field("machine", &body.machine)?;
            validate_machine_field("originMachine", &body.origin_machine)
        }
        NodeEventBody::LaneRequest(body) => {
            validate_machine_field("originMachine", &body.origin_machine)
        }
        NodeEventBody::Receipt(body) => {
            validate_machine_field("originMachine", &body.origin_machine)
        }
    }
}

/// GH-1236 logical identity: `sha256_hex(owner\0work\0status\0result\0
/// deliverable\0message)`, `""` for absent options.
///
/// Name-compatible with `edda-cli`'s `cmd_return` helper so the switch-over to
/// this module is mechanical (worker B owns that edit).
pub fn owner_return_logical_id(
    owner: &str,
    work: &str,
    status: &str,
    result: Option<&str>,
    deliverable: Option<&str>,
    message: Option<&str>,
) -> String {
    edda_core::hash::sha256_hex(
        format!(
            "{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}",
            owner,
            work,
            status,
            result.unwrap_or(""),
            deliverable.unwrap_or(""),
            message.unwrap_or("")
        )
        .as_bytes(),
    )
}

/// The complete allowlisted kind set (contract §4, rev 3).
pub const EVENT_KINDS: &[&str] = &[
    "owner_return",
    "decision_fact",
    "work_transition",
    "handover",
    "activation_observation",
    "lane_request",
    "receipt",
];

/// The GH-1236 bounds and identity rules for an owner return.
pub fn validate_owner_return(body: &OwnerReturn) -> anyhow::Result<()> {
    validate_label("owner", &body.owner)?;
    validate_label("work", &body.work)?;
    validate_label("originMachine", &body.origin_machine)?;
    if body.status != "done" && body.status != "failed" {
        anyhow::bail!(
            "invalid status '{}': expected 'done' or 'failed'",
            body.status
        );
    }
    if !is_sha256_hex(&body.message_id) {
        anyhow::bail!("invalid messageId: expected 64 lowercase hex characters");
    }
    if !is_sha256_hex(&body.logical_id) {
        anyhow::bail!("invalid logicalId: expected 64 lowercase hex characters");
    }
    if body.posted_at.is_empty() || body.posted_at.len() > 64 {
        anyhow::bail!("invalid postedAt: expected 1..=64 bytes");
    }
    let bounded = [
        ("result", body.result.as_deref(), 4_096_usize),
        ("deliverable", body.deliverable.as_deref(), 4_096),
        ("message", body.message.as_deref(), 1 << 20),
    ];
    for (what, value, max) in bounded {
        if let Some(value) = value {
            if value.len() > max {
                anyhow::bail!("{what} exceeds {max} bytes");
            }
            if value.contains('\u{0}') {
                anyhow::bail!("{what} must not contain a NUL character");
            }
        }
    }
    let recomputed = owner_return_logical_id(
        &body.owner,
        &body.work,
        &body.status,
        body.result.as_deref(),
        body.deliverable.as_deref(),
        body.message.as_deref(),
    );
    if recomputed != body.logical_id {
        anyhow::bail!("logicalId does not match the envelope content");
    }
    Ok(())
}

fn allowlist(kind: &str) -> Option<&'static [&'static str]> {
    match kind {
        "owner_return" => Some(OWNER_RETURN_FIELDS),
        "decision_fact" => Some(DECISION_FACT_FIELDS),
        "work_transition" => Some(WORK_TRANSITION_FIELDS),
        "handover" => Some(HANDOVER_FIELDS),
        "activation_observation" => Some(ACTIVATION_OBSERVATION_FIELDS),
        "lane_request" => Some(LANE_REQUEST_FIELDS),
        "receipt" => Some(RECEIPT_FIELDS),
        _ => None,
    }
}

fn check_fields(
    object: &serde_json::Map<String, serde_json::Value>,
    allowed: &[&str],
) -> Result<(), EventRefusal> {
    for key in object.keys() {
        if COMMON_FIELDS.contains(&key.as_str()) {
            continue;
        }
        if allowed.contains(&key.as_str()) {
            continue;
        }
        if FORBIDDEN_WIRE_FIELDS.contains(&key.as_str()) {
            return Err(EventRefusal::new(format!("forbidden field `{key}`")));
        }
        return Err(EventRefusal::new(format!("unknown field `{key}`")));
    }
    Ok(())
}

fn deserialize_body(
    kind: &str,
    mut object: serde_json::Map<String, serde_json::Value>,
) -> Result<NodeEventBody, EventRefusal> {
    object.remove("eventId");
    object.remove("kind");
    let value = serde_json::Value::Object(object);
    let body = match kind {
        "owner_return" => {
            serde_json::from_value::<OwnerReturn>(value).map(NodeEventBody::OwnerReturn)
        }
        "decision_fact" => {
            serde_json::from_value::<DecisionFact>(value).map(NodeEventBody::DecisionFact)
        }
        "work_transition" => {
            serde_json::from_value::<WorkTransition>(value).map(NodeEventBody::WorkTransition)
        }
        "handover" => serde_json::from_value::<Handover>(value).map(NodeEventBody::Handover),
        "activation_observation" => serde_json::from_value::<ActivationObservation>(value)
            .map(NodeEventBody::ActivationObservation),
        "lane_request" => {
            serde_json::from_value::<LaneRequest>(value).map(NodeEventBody::LaneRequest)
        }
        "receipt" => serde_json::from_value::<Receipt>(value).map(NodeEventBody::Receipt),
        _ => unreachable!("allowlist checked before deserialize"),
    };
    body.map_err(|error| EventRefusal::new(error.to_string()))
}

/// No canonical field may carry a NUL: `eventId` joins the fields with NUL, so
/// a NUL inside a field would make two distinct envelopes share one identity.
fn reject_nul(fields: &[(&str, &str)]) -> anyhow::Result<()> {
    for (what, value) in fields {
        if value.contains('\u{0}') {
            anyhow::bail!("{what} must not contain a NUL character");
        }
    }
    Ok(())
}

/// Shape rules for a `lane_request`. `requestId` is a bounded label and the
/// two labels are coordination labels; the message is bounded like an
/// owner-return message.
pub fn validate_lane_request(body: &LaneRequest) -> anyhow::Result<()> {
    validate_label("requestId", &body.request_id)?;
    validate_label("fromLabel", &body.from_label)?;
    validate_label("toLabel", &body.to_label)?;
    validate_label("originMachine", &body.origin_machine)?;
    if body.ts.is_empty() || body.ts.len() > 64 {
        anyhow::bail!("invalid ts: expected 1..=64 bytes");
    }
    if body.message.len() > (1 << 20) {
        anyhow::bail!("message exceeds {} bytes", 1 << 20);
    }
    reject_nul(&[
        ("requestId", &body.request_id),
        ("fromLabel", &body.from_label),
        ("toLabel", &body.to_label),
        ("message", &body.message),
        ("ts", &body.ts),
        ("originMachine", &body.origin_machine),
    ])
}

/// Shape rules for a `receipt`. `ofEventId` is a canonical id and `state` is
/// exactly `delivered` or `acked`.
pub fn validate_receipt(body: &Receipt) -> anyhow::Result<()> {
    if !is_sha256_hex(&body.of_event_id) {
        anyhow::bail!("invalid ofEventId: expected 64 lowercase hex characters");
    }
    if !EVENT_KINDS.contains(&body.of_kind.as_str()) {
        anyhow::bail!("unknown ofKind '{}'", body.of_kind);
    }
    if body.state != "delivered" && body.state != "acked" {
        anyhow::bail!(
            "invalid state '{}': expected 'delivered' or 'acked'",
            body.state
        );
    }
    validate_label("actor", &body.actor)?;
    validate_label("originMachine", &body.origin_machine)?;
    if body.ts.is_empty() || body.ts.len() > 64 {
        anyhow::bail!("invalid ts: expected 1..=64 bytes");
    }
    reject_nul(&[
        ("ofEventId", &body.of_event_id),
        ("ofKind", &body.of_kind),
        ("state", &body.state),
        ("actor", &body.actor),
        ("ts", &body.ts),
        ("originMachine", &body.origin_machine),
    ])
}

/// Parse and fully validate one wire event value. A refused event never
/// produces a `NodeEvent`, so it can never be applied.
pub fn parse_event(value: &serde_json::Value) -> Result<NodeEvent, EventRefusal> {
    let object = value
        .as_object()
        .ok_or_else(|| EventRefusal::new("event must be a JSON object"))?;

    let event_id = object
        .get("eventId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| EventRefusal::new("missing or non-string `eventId`"))?;
    if !is_sha256_hex(event_id) {
        return Err(EventRefusal::new(
            "`eventId` must be 64 lowercase hex characters",
        ));
    }
    let kind = object
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| EventRefusal::new("missing or non-string `kind`"))?;
    let allowed =
        allowlist(kind).ok_or_else(|| EventRefusal::new(format!("unknown kind `{kind}`")))?;

    check_fields(object, allowed)?;
    let body = deserialize_body(kind, object.clone())?;

    // A machine-shaped value is validated before any other shape rule so a
    // traversal/absolute-path machine is refused with the machine constraint.
    validate_machine_shape(&body).map_err(|error| EventRefusal::new(error.to_string()))?;

    if let NodeEventBody::OwnerReturn(body) = &body {
        validate_owner_return(body).map_err(|error| EventRefusal::new(error.to_string()))?;
    }
    if let NodeEventBody::LaneRequest(body) = &body {
        validate_lane_request(body).map_err(|error| EventRefusal::new(error.to_string()))?;
    }
    if let NodeEventBody::Receipt(body) = &body {
        validate_receipt(body).map_err(|error| EventRefusal::new(error.to_string()))?;
    }

    let recomputed = compute_event_id(kind, &body.canonical_values());
    if recomputed != event_id {
        return Err(EventRefusal::new(format!(
            "eventId does not match the canonical envelope content (expected {recomputed})"
        )));
    }

    Ok(NodeEvent {
        event_id: event_id.to_string(),
        body,
    })
}
