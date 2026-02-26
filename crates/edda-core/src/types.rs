use serde::{Deserialize, Serialize};

/// Current schema version for new events.
pub const SCHEMA_VERSION: u32 = 1;

/// Canonicalization scheme name for digest computation.
pub const CANON_EDDA_V1: &str = "edda-canon-v1";

/// Event ID format: `evt_<ulid>`
pub type EventId = String;

/// Branch name (e.g. "main", "feat/x")
pub type BranchName = String;

/// A single digest entry: algorithm + canonicalization scheme + hash value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Digest {
    pub alg: String,
    pub canon: String,
    pub value: String,
}

/// A provenance link: semantic reference to another event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Provenance {
    pub target: String,
    pub rel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Event family classification.
pub mod event_family {
    pub const SIGNAL: &str = "signal";
    pub const MILESTONE: &str = "milestone";
    pub const ADMIN: &str = "admin";
    pub const GOVERNANCE: &str = "governance";
}

/// Event level classification.
pub mod event_level {
    pub const TRACE: &str = "trace";
    pub const INFO: &str = "info";
    pub const MILESTONE: &str = "milestone";
    pub const GOVERNANCE: &str = "governance";
}

/// Map an event_type string to its (family, level) classification.
pub fn classify_event_type(event_type: &str) -> (Option<&'static str>, Option<&'static str>) {
    match event_type {
        "note" => (Some(event_family::SIGNAL), Some(event_level::INFO)),
        "cmd" => (Some(event_family::SIGNAL), Some(event_level::TRACE)),
        "commit" => (Some(event_family::MILESTONE), Some(event_level::MILESTONE)),
        "merge" => (Some(event_family::MILESTONE), Some(event_level::MILESTONE)),
        "rebuild" => (Some(event_family::ADMIN), Some(event_level::TRACE)),
        "branch_create" => (Some(event_family::ADMIN), Some(event_level::INFO)),
        "branch_switch" => (Some(event_family::ADMIN), Some(event_level::INFO)),
        "approval" | "approval_request" => (
            Some(event_family::GOVERNANCE),
            Some(event_level::GOVERNANCE),
        ),
        "task_intake" => (Some(event_family::SIGNAL), Some(event_level::INFO)),
        "review_bundle" => (Some(event_family::GOVERNANCE), Some(event_level::MILESTONE)),
        _ => (None, None),
    }
}

/// Structured decision payload for decision events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DecisionPayload {
    pub key: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Well-known relation types for provenance links.
pub mod rel {
    pub const BASED_ON: &str = "based_on";
    pub const SUPERSEDES: &str = "supersedes";
    pub const CONTINUES: &str = "continues";
    pub const REVIEWS: &str = "reviews";
}

/// References to other events and blobs
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Refs {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blobs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<Provenance>,
}

/// A single ledger event (one JSONL line in events.jsonl)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub event_id: String,
    pub ts: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub branch: String,
    pub parent_hash: Option<String>,
    pub hash: String,
    pub payload: serde_json::Value,
    #[serde(default)]
    pub refs: Refs,
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub digests: Vec<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_level: Option<String>,
}
