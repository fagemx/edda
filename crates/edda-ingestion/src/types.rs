use serde::{Deserialize, Serialize};

/// Valid source layers in the decision spine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceLayer {
    L0,
    L1,
    L2,
    L3,
    L4,
    L5,
}

impl std::fmt::Display for SourceLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::L0 => write!(f, "L0"),
            Self::L1 => write!(f, "L1"),
            Self::L2 => write!(f, "L2"),
            Self::L3 => write!(f, "L3"),
            Self::L4 => write!(f, "L4"),
            Self::L5 => write!(f, "L5"),
        }
    }
}

impl std::str::FromStr for SourceLayer {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "L0" => Ok(Self::L0),
            "L1" => Ok(Self::L1),
            "L2" => Ok(Self::L2),
            "L3" => Ok(Self::L3),
            "L4" => Ok(Self::L4),
            "L5" => Ok(Self::L5),
            other => Err(format!("unknown source layer: {other} (expected L0..L5)")),
        }
    }
}

/// A cross-layer reference to an external entity.
/// Wire format: camelCase (WIRE-01).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceRef {
    pub layer: SourceLayer,
    pub kind: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Result of evaluating an ingestion trigger.
#[derive(Debug, Clone, PartialEq)]
pub enum TriggerResult {
    /// Write immediately to ledger, no human confirmation needed.
    AutoIngest,
    /// Queue for human review before writing.
    SuggestIngest { reason: String },
    /// Silently skip — not worth recording.
    Skip,
}

/// How the ingestion was triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerType {
    Auto,
    Suggested,
    Manual,
}

/// An ingestion record to be written to the ledger.
/// Wire format: camelCase (WIRE-01).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IngestionRecord {
    pub id: String,
    pub trigger_type: TriggerType,
    pub event_type: String,
    pub source_layer: SourceLayer,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_refs: Vec<SourceRef>,
    pub summary: String,
    pub detail: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub created_at: String,
}

impl IngestionRecord {
    /// Generate a new ingestion record ID with the given prefix.
    pub fn new_id(prefix: &str) -> String {
        format!(
            "{}_{}",
            prefix,
            ulid::Ulid::new().to_string().to_lowercase()
        )
    }
}

/// Suggestion review status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionStatus {
    Pending,
    Accepted,
    Rejected,
}

impl std::fmt::Display for SuggestionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Accepted => write!(f, "accepted"),
            Self::Rejected => write!(f, "rejected"),
        }
    }
}

impl std::str::FromStr for SuggestionStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "accepted" => Ok(Self::Accepted),
            "rejected" => Ok(Self::Rejected),
            other => Err(format!(
                "unknown suggestion status: {other} (expected pending, accepted, rejected)"
            )),
        }
    }
}

/// A suggestion queued for human review before ingestion.
/// Wire format: camelCase (WIRE-01).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub id: String,
    pub event_type: String,
    pub source_layer: SourceLayer,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_refs: Vec<SourceRef>,
    pub summary: String,
    pub suggested_because: String,
    pub detail: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub status: SuggestionStatus,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_at: Option<String>,
}

impl Suggestion {
    /// Generate a new suggestion ID with `sug_` prefix + ULID.
    pub fn new_id() -> String {
        IngestionRecord::new_id("sug")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_layer_round_trip() {
        for (s, expected) in [
            ("L0", SourceLayer::L0),
            ("L1", SourceLayer::L1),
            ("L2", SourceLayer::L2),
            ("L3", SourceLayer::L3),
            ("L4", SourceLayer::L4),
            ("L5", SourceLayer::L5),
        ] {
            let parsed: SourceLayer = s.parse().expect("valid layer");
            assert_eq!(parsed, expected);
            assert_eq!(parsed.to_string(), s);
        }
    }

    #[test]
    fn source_layer_rejects_invalid() {
        assert!("L6".parse::<SourceLayer>().is_err());
        assert!("X".parse::<SourceLayer>().is_err());
        assert!("l0".parse::<SourceLayer>().is_err());
        assert!("".parse::<SourceLayer>().is_err());
    }

    #[test]
    fn source_ref_json_camel_case() {
        let r = SourceRef {
            layer: SourceLayer::L1,
            kind: "decision-session".to_string(),
            id: "ds_abc123".to_string(),
            note: Some("test link".to_string()),
        };
        let json = serde_json::to_value(&r).expect("serialize");
        assert!(json.get("layer").is_some());
        assert!(json.get("kind").is_some());
        assert!(json.get("id").is_some());
        assert!(json.get("note").is_some());
        // camelCase has no effect on single-word fields, but verify no snake_case artifacts
        assert!(json.get("source_layer").is_none());
    }

    #[test]
    fn source_ref_omits_none_note() {
        let r = SourceRef {
            layer: SourceLayer::L2,
            kind: "spec-file".to_string(),
            id: "sf_001".to_string(),
            note: None,
        };
        let json = serde_json::to_value(&r).expect("serialize");
        assert!(json.get("note").is_none());
    }

    #[test]
    fn ingestion_record_wire_format() {
        let rec = IngestionRecord {
            id: "prec_test".to_string(),
            trigger_type: TriggerType::Auto,
            event_type: "decision.commit".to_string(),
            source_layer: SourceLayer::L1,
            source_refs: vec![],
            summary: "test summary".to_string(),
            detail: serde_json::json!({"key": "value"}),
            tags: vec![],
            created_at: "2025-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_value(&rec).expect("serialize");

        // Verify camelCase field names
        assert!(json.get("triggerType").is_some());
        assert!(json.get("eventType").is_some());
        assert!(json.get("sourceLayer").is_some());
        assert!(json.get("createdAt").is_some());

        // Verify empty vecs are omitted
        assert!(json.get("sourceRefs").is_none());
        assert!(json.get("tags").is_none());

        // Verify round-trip
        let json_str = serde_json::to_string(&rec).expect("serialize");
        let back: IngestionRecord = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(back, rec);
    }

    #[test]
    fn ingestion_record_new_id_has_prefix() {
        let id = IngestionRecord::new_id("prec");
        assert!(id.starts_with("prec_"));
        assert!(id.len() > 5); // prefix + underscore + ulid
    }

    #[test]
    fn source_layer_serde_round_trip() {
        for layer in [
            SourceLayer::L0,
            SourceLayer::L1,
            SourceLayer::L2,
            SourceLayer::L3,
            SourceLayer::L4,
            SourceLayer::L5,
        ] {
            let json = serde_json::to_value(layer).expect("serialize");
            let back: SourceLayer = serde_json::from_value(json).expect("deserialize");
            assert_eq!(back, layer);
        }
    }

    #[test]
    fn suggestion_status_serde_round_trip() {
        for status in [
            SuggestionStatus::Pending,
            SuggestionStatus::Accepted,
            SuggestionStatus::Rejected,
        ] {
            let json = serde_json::to_value(status).expect("serialize");
            let back: SuggestionStatus = serde_json::from_value(json).expect("deserialize");
            assert_eq!(back, status);
        }
    }

    #[test]
    fn suggestion_status_display_and_parse() {
        for (status, s) in [
            (SuggestionStatus::Pending, "pending"),
            (SuggestionStatus::Accepted, "accepted"),
            (SuggestionStatus::Rejected, "rejected"),
        ] {
            assert_eq!(status.to_string(), s);
            let parsed: SuggestionStatus = s.parse().expect("valid status");
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn suggestion_wire_format() {
        let sug = Suggestion {
            id: "sug_test".to_string(),
            event_type: "route.changed".to_string(),
            source_layer: SourceLayer::L1,
            source_refs: vec![],
            summary: "test suggestion".to_string(),
            suggested_because: "May indicate routing anti-pattern".to_string(),
            detail: serde_json::json!({}),
            tags: vec![],
            status: SuggestionStatus::Pending,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            reviewed_at: None,
        };
        let json = serde_json::to_value(&sug).expect("serialize");

        // Verify camelCase field names
        assert!(json.get("eventType").is_some());
        assert!(json.get("sourceLayer").is_some());
        assert!(json.get("suggestedBecause").is_some());
        assert!(json.get("createdAt").is_some());
        assert!(json.get("reviewedAt").is_none()); // None is omitted

        // Verify empty vecs are omitted
        assert!(json.get("sourceRefs").is_none());
        assert!(json.get("tags").is_none());

        // Verify round-trip
        let json_str = serde_json::to_string(&sug).expect("serialize");
        let back: Suggestion = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(back, sug);
    }

    #[test]
    fn suggestion_new_id_prefix() {
        let id = Suggestion::new_id();
        assert!(id.starts_with("sug_"));
        assert!(id.len() > 4);
    }
}
