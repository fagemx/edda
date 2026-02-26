//! Centralized decision event helpers: detection, extraction, domain parsing.

use crate::types::DecisionPayload;
use serde_json::Value;

/// Check if a payload represents a decision event (note with "decision" tag).
pub fn is_decision(payload: &Value) -> bool {
    payload
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(|t| t.as_str() == Some("decision")))
        .unwrap_or(false)
}

/// Extract `DecisionPayload` from an event payload.
/// Prefers structured `payload.decision` object, falls back to text parse
/// for backward compatibility with legacy events.
pub fn extract_decision(payload: &Value) -> Option<DecisionPayload> {
    // Structured path: payload.decision.{key, value, reason}
    if let Some(d) = payload.get("decision") {
        let key = d.get("key").and_then(|v| v.as_str())?.to_string();
        let value = d.get("value").and_then(|v| v.as_str())?.to_string();
        let reason = d
            .get("reason")
            .and_then(|v| v.as_str())
            .filter(|r| !r.is_empty())
            .map(|r| r.to_string());
        return Some(DecisionPayload { key, value, reason });
    }
    // Text fallback: "key: value — reason"
    let text = payload.get("text").and_then(|v| v.as_str())?;
    let (key, rest) = text.split_once(": ")?;
    let (value, reason) = match rest.split_once(" \u{2014} ") {
        Some((v, r)) => (v.to_string(), Some(r.to_string())),
        None => (rest.to_string(), None),
    };
    Some(DecisionPayload {
        key: key.to_string(),
        value,
        reason,
    })
}

/// Extract domain from a decision key: `"db.engine"` → `"db"`.
pub fn extract_domain(key: &str) -> String {
    key.split('.').next().unwrap_or(key).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_decision_with_tag() {
        let payload = serde_json::json!({
            "tags": ["decision"],
            "text": "db.engine: postgres"
        });
        assert!(is_decision(&payload));
    }

    #[test]
    fn is_decision_without_tag() {
        let payload = serde_json::json!({
            "tags": ["session"],
            "text": "some note"
        });
        assert!(!is_decision(&payload));
    }

    #[test]
    fn is_decision_no_tags() {
        let payload = serde_json::json!({"text": "hello"});
        assert!(!is_decision(&payload));
    }

    #[test]
    fn extract_structured_with_reason() {
        let payload = serde_json::json!({
            "tags": ["decision"],
            "text": "db.engine: postgres — embedded, zero-config",
            "decision": {"key": "db.engine", "value": "postgres", "reason": "embedded, zero-config"}
        });
        let dp = extract_decision(&payload).unwrap();
        assert_eq!(dp.key, "db.engine");
        assert_eq!(dp.value, "postgres");
        assert_eq!(dp.reason.as_deref(), Some("embedded, zero-config"));
    }

    #[test]
    fn extract_structured_without_reason() {
        let payload = serde_json::json!({
            "tags": ["decision"],
            "decision": {"key": "auth.method", "value": "JWT"}
        });
        let dp = extract_decision(&payload).unwrap();
        assert_eq!(dp.key, "auth.method");
        assert_eq!(dp.value, "JWT");
        assert!(dp.reason.is_none());
    }

    #[test]
    fn extract_structured_empty_reason_becomes_none() {
        let payload = serde_json::json!({
            "tags": ["decision"],
            "decision": {"key": "k", "value": "v", "reason": ""}
        });
        let dp = extract_decision(&payload).unwrap();
        assert!(dp.reason.is_none());
    }

    #[test]
    fn extract_text_fallback_with_reason() {
        let payload = serde_json::json!({
            "tags": ["decision"],
            "text": "db.engine: postgres \u{2014} embedded"
        });
        let dp = extract_decision(&payload).unwrap();
        assert_eq!(dp.key, "db.engine");
        assert_eq!(dp.value, "postgres");
        assert_eq!(dp.reason.as_deref(), Some("embedded"));
    }

    #[test]
    fn extract_text_fallback_without_reason() {
        let payload = serde_json::json!({
            "tags": ["decision"],
            "text": "auth.method: JWT"
        });
        let dp = extract_decision(&payload).unwrap();
        assert_eq!(dp.key, "auth.method");
        assert_eq!(dp.value, "JWT");
        assert!(dp.reason.is_none());
    }

    #[test]
    fn extract_no_decision_returns_none() {
        let payload = serde_json::json!({"text": "just a note"});
        assert!(extract_decision(&payload).is_none());
    }

    #[test]
    fn extract_structured_missing_key_returns_none() {
        let payload = serde_json::json!({
            "decision": {"value": "v"}
        });
        assert!(extract_decision(&payload).is_none());
    }

    #[test]
    fn domain_dotted_key() {
        assert_eq!(extract_domain("db.engine"), "db");
    }

    #[test]
    fn domain_no_dot() {
        assert_eq!(extract_domain("auth"), "auth");
    }

    #[test]
    fn domain_multi_dot() {
        assert_eq!(extract_domain("api.v2.style"), "api");
    }
}
