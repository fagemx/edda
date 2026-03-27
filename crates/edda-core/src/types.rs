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
        "agent_phase_change" => (Some(event_family::SIGNAL), Some(event_level::INFO)),
        "review_bundle" => (Some(event_family::GOVERNANCE), Some(event_level::MILESTONE)),
        "approval_policy_match" => (
            Some(event_family::GOVERNANCE),
            Some(event_level::GOVERNANCE),
        ),
        "pr" => (Some(event_family::MILESTONE), Some(event_level::MILESTONE)),
        "decision_import" => (
            Some(event_family::GOVERNANCE),
            Some(event_level::GOVERNANCE),
        ),
        "device_pair" | "device_revoke" => (Some(event_family::ADMIN), Some(event_level::INFO)),
        "decide_snapshot" => (Some(event_family::GOVERNANCE), Some(event_level::MILESTONE)),
        _ => (None, None),
    }
}

/// Scope of a decision's propagation across projects.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum DecisionScope {
    /// Default: stays in this project only.
    #[default]
    Local,
    /// Propagates to projects in the same group.
    Shared,
    /// Propagates to all registered projects.
    Global,
}

impl std::fmt::Display for DecisionScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local => write!(f, "local"),
            Self::Shared => write!(f, "shared"),
            Self::Global => write!(f, "global"),
        }
    }
}

impl std::str::FromStr for DecisionScope {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "local" => Ok(Self::Local),
            "shared" => Ok(Self::Shared),
            "global" => Ok(Self::Global),
            other => Err(format!(
                "unknown scope: {other} (expected local|shared|global)"
            )),
        }
    }
}

/// Structured decision payload for decision events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DecisionPayload {
    pub key: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<DecisionScope>,
    /// Decision authority: "human", "agent", "system". Default: "human".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
    /// Glob patterns for guarded file paths. Default: [].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affected_paths: Option<Vec<String>>,
    /// Categorization tags. Default: [].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// ISO-8601 date for scheduled re-evaluation. Default: None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_after: Option<String>,
    /// Reversibility level: "easy", "medium", "hard". Default: "medium".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversibility: Option<String>,
    /// Village scope identifier. Default: None (not village-scoped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub village_id: Option<String>,
}

/// Status of a task brief.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskBriefStatus {
    Active,
    Completed,
    Blocked,
    Abandoned,
}

impl TaskBriefStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Blocked => "blocked",
            Self::Abandoned => "abandoned",
        }
    }
}

impl std::fmt::Display for TaskBriefStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for TaskBriefStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(Self::Active),
            "completed" => Ok(Self::Completed),
            "blocked" => Ok(Self::Blocked),
            "abandoned" => Ok(Self::Abandoned),
            other => Err(format!("unknown task brief status: {other:?}")),
        }
    }
}

/// Intent / purpose of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskBriefIntent {
    Implement,
    Fix,
    Maintain,
    Investigate,
    Refactor,
    Document,
    Test,
}

impl TaskBriefIntent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Implement => "implement",
            Self::Fix => "fix",
            Self::Maintain => "maintain",
            Self::Investigate => "investigate",
            Self::Refactor => "refactor",
            Self::Document => "document",
            Self::Test => "test",
        }
    }
}

impl std::fmt::Display for TaskBriefIntent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for TaskBriefIntent {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "implement" => Ok(Self::Implement),
            "fix" => Ok(Self::Fix),
            "maintain" => Ok(Self::Maintain),
            "investigate" => Ok(Self::Investigate),
            "refactor" => Ok(Self::Refactor),
            "document" => Ok(Self::Document),
            "test" => Ok(Self::Test),
            other => Err(format!("unknown task brief intent: {other:?}")),
        }
    }
}

/// Well-known relation types for provenance links.
pub mod rel {
    pub const BASED_ON: &str = "based_on";
    pub const SUPERSEDES: &str = "supersedes";
    pub const CONTINUES: &str = "continues";
    pub const REVIEWS: &str = "reviews";
    pub const DEPENDS_ON: &str = "depends_on";
    pub const IMPORTED_FROM: &str = "imported_from";
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    // ── classify_event_type: data-driven table test ──

    #[test]
    fn classify_all_known_event_types() {
        let table: Vec<(&str, &str, &str)> = vec![
            ("note", event_family::SIGNAL, event_level::INFO),
            ("cmd", event_family::SIGNAL, event_level::TRACE),
            ("commit", event_family::MILESTONE, event_level::MILESTONE),
            ("merge", event_family::MILESTONE, event_level::MILESTONE),
            ("rebuild", event_family::ADMIN, event_level::TRACE),
            ("branch_create", event_family::ADMIN, event_level::INFO),
            ("branch_switch", event_family::ADMIN, event_level::INFO),
            (
                "approval",
                event_family::GOVERNANCE,
                event_level::GOVERNANCE,
            ),
            (
                "approval_request",
                event_family::GOVERNANCE,
                event_level::GOVERNANCE,
            ),
            ("task_intake", event_family::SIGNAL, event_level::INFO),
            (
                "agent_phase_change",
                event_family::SIGNAL,
                event_level::INFO,
            ),
            (
                "review_bundle",
                event_family::GOVERNANCE,
                event_level::MILESTONE,
            ),
            (
                "approval_policy_match",
                event_family::GOVERNANCE,
                event_level::GOVERNANCE,
            ),
            ("pr", event_family::MILESTONE, event_level::MILESTONE),
            (
                "decision_import",
                event_family::GOVERNANCE,
                event_level::GOVERNANCE,
            ),
            ("device_pair", event_family::ADMIN, event_level::INFO),
            ("device_revoke", event_family::ADMIN, event_level::INFO),
            (
                "decide_snapshot",
                event_family::GOVERNANCE,
                event_level::MILESTONE,
            ),
        ];

        for (event_type, expected_family, expected_level) in &table {
            let (family, level) = classify_event_type(event_type);
            assert_eq!(
                family,
                Some(*expected_family),
                "family mismatch for event_type={event_type:?}"
            );
            assert_eq!(
                level,
                Some(*expected_level),
                "level mismatch for event_type={event_type:?}"
            );
        }
    }

    #[test]
    fn classify_unknown_types_return_none() {
        let unknowns = ["unknown", "Note", "NOTE", "foo_bar", "commit "];
        for t in &unknowns {
            let (family, level) = classify_event_type(t);
            assert_eq!(family, None, "expected None family for {t:?}");
            assert_eq!(level, None, "expected None level for {t:?}");
        }
    }

    #[test]
    fn classify_empty_string() {
        let (family, level) = classify_event_type("");
        assert_eq!(family, None);
        assert_eq!(level, None);
    }

    // ── Serde round-trip tests ──

    fn make_test_event() -> Event {
        Event {
            event_id: "evt_01ABCDEF".to_string(),
            ts: "2025-01-01T00:00:00Z".to_string(),
            event_type: "note".to_string(),
            branch: "main".to_string(),
            parent_hash: Some("abc123".to_string()),
            hash: "def456".to_string(),
            payload: serde_json::json!({"message": "hello"}),
            refs: Refs {
                blobs: vec!["blob1".to_string()],
                events: vec!["evt_other".to_string()],
                provenance: vec![Provenance {
                    target: "evt_prev".to_string(),
                    rel: rel::BASED_ON.to_string(),
                    note: Some("from prior session".to_string()),
                }],
            },
            schema_version: 1,
            digests: vec![Digest {
                alg: "sha256".to_string(),
                canon: CANON_EDDA_V1.to_string(),
                value: "deadbeef".to_string(),
            }],
            event_family: Some(event_family::SIGNAL.to_string()),
            event_level: Some(event_level::INFO.to_string()),
        }
    }

    #[test]
    fn event_serde_round_trip() {
        let event = make_test_event();
        let json = serde_json::to_string(&event).expect("serialize");
        let decoded: Event = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(decoded.event_id, event.event_id);
        assert_eq!(decoded.ts, event.ts);
        assert_eq!(decoded.event_type, event.event_type);
        assert_eq!(decoded.branch, event.branch);
        assert_eq!(decoded.parent_hash, event.parent_hash);
        assert_eq!(decoded.hash, event.hash);
        assert_eq!(decoded.payload, event.payload);
        assert_eq!(decoded.schema_version, event.schema_version);
        assert_eq!(decoded.event_family, event.event_family);
        assert_eq!(decoded.event_level, event.event_level);
        assert_eq!(decoded.digests.len(), 1);
        assert_eq!(decoded.digests[0], event.digests[0]);
        assert_eq!(decoded.refs.blobs, event.refs.blobs);
        assert_eq!(decoded.refs.events, event.refs.events);
        assert_eq!(decoded.refs.provenance.len(), 1);
        assert_eq!(decoded.refs.provenance[0], event.refs.provenance[0]);
    }

    #[test]
    fn event_serde_optional_fields_omitted() {
        let event = Event {
            event_id: "evt_min".to_string(),
            ts: "2025-01-01T00:00:00Z".to_string(),
            event_type: "cmd".to_string(),
            branch: "main".to_string(),
            parent_hash: None,
            hash: "aaa".to_string(),
            payload: serde_json::json!({}),
            refs: Refs::default(),
            schema_version: 1,
            digests: vec![],
            event_family: None,
            event_level: None,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        // Optional fields with skip_serializing_if should be omitted
        assert!(
            !json.contains("event_family"),
            "event_family should be omitted"
        );
        assert!(
            !json.contains("event_level"),
            "event_level should be omitted"
        );
        assert!(!json.contains("digests"), "empty digests should be omitted");
        // parent_hash is Option but without skip_serializing_if, so it serializes as null
        assert!(
            json.contains("parent_hash"),
            "parent_hash should be present (as null)"
        );
        let val: serde_json::Value = serde_json::from_str(&json).expect("parse");
        assert!(val["parent_hash"].is_null());
    }

    #[test]
    fn event_type_serializes_as_type() {
        let event = make_test_event();
        let val: serde_json::Value = serde_json::to_value(&event).expect("to_value");
        // The field should be "type" in JSON, not "event_type"
        assert!(val.get("type").is_some(), "should serialize as 'type'");
        assert!(
            val.get("event_type").is_none(),
            "should NOT have 'event_type' key"
        );
        assert_eq!(val["type"], "note");
    }

    #[test]
    fn decision_payload_serde_round_trip() {
        // With reason
        let dp = DecisionPayload {
            key: "db.engine".to_string(),
            value: "sqlite".to_string(),
            reason: Some("embedded, zero-config".to_string()),
            scope: None,
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: None,
        };
        let json = serde_json::to_string(&dp).expect("serialize");
        let decoded: DecisionPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, dp);

        // Without reason
        let dp_no_reason = DecisionPayload {
            key: "auth.strategy".to_string(),
            value: "JWT".to_string(),
            reason: None,
            scope: None,
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: None,
        };
        let json2 = serde_json::to_string(&dp_no_reason).expect("serialize");
        assert!(!json2.contains("reason"), "None reason should be omitted");
        assert!(!json2.contains("scope"), "None scope should be omitted");
        let decoded2: DecisionPayload = serde_json::from_str(&json2).expect("deserialize");
        assert_eq!(decoded2, dp_no_reason);
    }

    #[test]
    fn decision_payload_with_scope_round_trip() {
        let dp = DecisionPayload {
            key: "api.version".to_string(),
            value: "v3".to_string(),
            reason: Some("breaking change".to_string()),
            scope: Some(DecisionScope::Shared),
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: None,
        };
        let json = serde_json::to_string(&dp).expect("serialize");
        assert!(json.contains("\"scope\":\"shared\""));
        let decoded: DecisionPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.scope, Some(DecisionScope::Shared));
    }

    #[test]
    fn decision_scope_ordering() {
        assert!(DecisionScope::Local < DecisionScope::Shared);
        assert!(DecisionScope::Shared < DecisionScope::Global);
    }

    #[test]
    fn decision_scope_parse_round_trip() {
        for (s, expected) in [
            ("local", DecisionScope::Local),
            ("shared", DecisionScope::Shared),
            ("global", DecisionScope::Global),
            ("SHARED", DecisionScope::Shared),
        ] {
            let parsed: DecisionScope = s.parse().unwrap();
            assert_eq!(parsed, expected);
        }
        assert!("unknown".parse::<DecisionScope>().is_err());
    }

    #[test]
    fn decision_scope_backward_compat() {
        // Old payload without scope field should deserialize with scope = None
        let json = r#"{"key":"db","value":"pg","reason":"fast"}"#;
        let dp: DecisionPayload = serde_json::from_str(json).expect("deserialize");
        assert_eq!(dp.scope, None);
    }

    #[test]
    fn digest_serde_round_trip() {
        let d = Digest {
            alg: "sha256".to_string(),
            canon: CANON_EDDA_V1.to_string(),
            value: "cafebabe".to_string(),
        };
        let json = serde_json::to_string(&d).expect("serialize");
        let decoded: Digest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, d);
    }

    #[test]
    fn provenance_serde_round_trip() {
        // With note
        let p = Provenance {
            target: "evt_123".to_string(),
            rel: rel::SUPERSEDES.to_string(),
            note: Some("overrides old decision".to_string()),
        };
        let json = serde_json::to_string(&p).expect("serialize");
        let decoded: Provenance = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, p);

        // Without note
        let p_no_note = Provenance {
            target: "evt_456".to_string(),
            rel: rel::CONTINUES.to_string(),
            note: None,
        };
        let json2 = serde_json::to_string(&p_no_note).expect("serialize");
        assert!(!json2.contains("note"), "None note should be omitted");
        let decoded2: Provenance = serde_json::from_str(&json2).expect("deserialize");
        assert_eq!(decoded2, p_no_note);
    }

    #[test]
    fn refs_default_is_empty() {
        let r = Refs::default();
        assert!(r.blobs.is_empty());
        assert!(r.events.is_empty());
        assert!(r.provenance.is_empty());

        // Serializes to {} (all empty vecs skipped)
        let json = serde_json::to_string(&r).expect("serialize");
        assert_eq!(json, "{}");

        // Deserializes from {} back to empty
        let decoded: Refs = serde_json::from_str("{}").expect("deserialize");
        assert!(decoded.blobs.is_empty());
        assert!(decoded.events.is_empty());
        assert!(decoded.provenance.is_empty());
    }

    #[test]
    fn constant_modules_have_expected_values() {
        // event_family
        assert_eq!(event_family::SIGNAL, "signal");
        assert_eq!(event_family::MILESTONE, "milestone");
        assert_eq!(event_family::ADMIN, "admin");
        assert_eq!(event_family::GOVERNANCE, "governance");

        // event_level
        assert_eq!(event_level::TRACE, "trace");
        assert_eq!(event_level::INFO, "info");
        assert_eq!(event_level::MILESTONE, "milestone");
        assert_eq!(event_level::GOVERNANCE, "governance");

        // rel
        assert_eq!(rel::BASED_ON, "based_on");
        assert_eq!(rel::SUPERSEDES, "supersedes");
        assert_eq!(rel::CONTINUES, "continues");
        assert_eq!(rel::REVIEWS, "reviews");
        assert_eq!(rel::DEPENDS_ON, "depends_on");
        assert_eq!(rel::IMPORTED_FROM, "imported_from");
    }

    // ── TaskBriefStatus tests ─────────────────────────────────────

    #[test]
    fn task_brief_status_round_trip() {
        for status in [
            TaskBriefStatus::Active,
            TaskBriefStatus::Completed,
            TaskBriefStatus::Blocked,
            TaskBriefStatus::Abandoned,
        ] {
            let s = status.as_str();
            let parsed: TaskBriefStatus = s.parse().unwrap();
            assert_eq!(parsed, status);
            assert_eq!(format!("{status}"), s);
        }
    }

    #[test]
    fn task_brief_status_unknown_is_err() {
        let result = "foobar".parse::<TaskBriefStatus>();
        assert!(result.is_err());
    }

    #[test]
    fn task_brief_status_serde() {
        let status = TaskBriefStatus::Completed;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"completed\"");
        let decoded: TaskBriefStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, TaskBriefStatus::Completed);
    }

    // ── TaskBriefIntent tests ─────────────────────────────────────

    #[test]
    fn task_brief_intent_round_trip() {
        for intent in [
            TaskBriefIntent::Implement,
            TaskBriefIntent::Fix,
            TaskBriefIntent::Maintain,
            TaskBriefIntent::Investigate,
            TaskBriefIntent::Refactor,
            TaskBriefIntent::Document,
            TaskBriefIntent::Test,
        ] {
            let s = intent.as_str();
            let parsed: TaskBriefIntent = s.parse().unwrap();
            assert_eq!(parsed, intent);
            assert_eq!(format!("{intent}"), s);
        }
    }

    #[test]
    fn task_brief_intent_unknown_is_err() {
        let result = "unknown".parse::<TaskBriefIntent>();
        assert!(result.is_err());
    }

    #[test]
    fn task_brief_intent_serde() {
        let intent = TaskBriefIntent::Fix;
        let json = serde_json::to_string(&intent).unwrap();
        assert_eq!(json, "\"fix\"");
        let decoded: TaskBriefIntent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, TaskBriefIntent::Fix);
    }

    // ── DecisionPayload V10 fields tests ─────────────────────────

    #[test]
    fn test_decision_payload_serde_backward_compat() {
        let json = r#"{"key":"db.engine","value":"sqlite","reason":"embedded"}"#;
        let dp: DecisionPayload = serde_json::from_str(json).unwrap();
        assert_eq!(dp.key, "db.engine");
        assert_eq!(dp.authority, None);
        assert_eq!(dp.affected_paths, None);
        assert_eq!(dp.tags, None);
        assert_eq!(dp.review_after, None);
        assert_eq!(dp.reversibility, None);
    }

    #[test]
    fn test_decision_payload_serde_with_new_fields() {
        let json = r#"{
            "key": "db.engine",
            "value": "sqlite",
            "reason": "embedded",
            "authority": "agent",
            "affected_paths": ["crates/edda-ledger/**"],
            "tags": ["arch"],
            "review_after": "2026-06-01",
            "reversibility": "hard"
        }"#;
        let dp: DecisionPayload = serde_json::from_str(json).unwrap();
        assert_eq!(dp.authority.as_deref(), Some("agent"));
        assert_eq!(
            dp.affected_paths,
            Some(vec!["crates/edda-ledger/**".to_string()])
        );
        assert_eq!(dp.tags, Some(vec!["arch".to_string()]));
        assert_eq!(dp.review_after.as_deref(), Some("2026-06-01"));
        assert_eq!(dp.reversibility.as_deref(), Some("hard"));

        // Round-trip: serialize and deserialize
        let serialized = serde_json::to_string(&dp).unwrap();
        let dp2: DecisionPayload = serde_json::from_str(&serialized).unwrap();
        assert_eq!(dp, dp2);
    }

    #[test]
    fn test_decision_payload_serde_with_village_id() {
        let json = r#"{
            "key": "db.engine",
            "value": "sqlite",
            "village_id": "village-abc"
        }"#;
        let dp: DecisionPayload = serde_json::from_str(json).unwrap();
        assert_eq!(dp.village_id.as_deref(), Some("village-abc"));

        // Round-trip
        let serialized = serde_json::to_string(&dp).unwrap();
        let dp2: DecisionPayload = serde_json::from_str(&serialized).unwrap();
        assert_eq!(dp.village_id, dp2.village_id);

        // Backward compat: missing village_id defaults to None
        let json_no_village = r#"{"key":"db.engine","value":"sqlite"}"#;
        let dp3: DecisionPayload = serde_json::from_str(json_no_village).unwrap();
        assert_eq!(dp3.village_id, None);
    }
}
