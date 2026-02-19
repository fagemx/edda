use crate::canon::canonical_json_bytes;
use crate::hash::sha256_hex;
use crate::types::{classify_event_type, Digest, Event, Refs, CANON_EDDA_V1, SCHEMA_VERSION};

/// Compute the hash for an event: serialize without the `hash` field,
/// canonical JSON sort, then SHA-256.
pub fn compute_event_hash(event_without_hash: &serde_json::Value) -> String {
    let bytes = canonical_json_bytes(event_without_hash);
    sha256_hex(&bytes)
}

fn new_event_id() -> String {
    format!("evt_{}", ulid::Ulid::new().to_string().to_lowercase())
}

fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}

/// Set event_family and event_level based on event_type.
fn set_taxonomy(event: &mut Event) {
    let (family, level) = classify_event_type(&event.event_type);
    event.event_family = family.map(|s| s.to_string());
    event.event_level = level.map(|s| s.to_string());
}

fn finalize(event: &mut Event) {
    // Auto-populate taxonomy fields before hashing
    set_taxonomy(event);

    // Serialize the event, remove hash/digests/schema_version, compute canonical hash
    let mut val = serde_json::to_value(&*event).expect("event serialization should not fail");
    if let Some(obj) = val.as_object_mut() {
        obj.remove("hash");
        obj.remove("digests");
        obj.remove("schema_version");
    }
    let hash_value = compute_event_hash(&val);

    event.hash = hash_value.clone();
    event.digests = vec![Digest {
        alg: "sha256".to_string(),
        canon: CANON_EDDA_V1.to_string(),
        value: hash_value,
    }];
}

/// Re-finalize an event after post-construction modification.
/// Recomputes hash and digests based on current event state.
pub fn finalize_event(event: &mut Event) {
    finalize(event);
}

/// Create a new `note` event.
pub fn new_note_event(
    branch: &str,
    parent_hash: Option<&str>,
    role: &str,
    text: &str,
    tags: &[String],
) -> anyhow::Result<Event> {
    let payload = serde_json::json!({
        "role": role,
        "text": text,
        "tags": tags,
    });

    let mut event = Event {
        event_id: new_event_id(),
        ts: now_rfc3339(),
        event_type: "note".to_string(),
        branch: branch.to_string(),
        parent_hash: parent_hash.map(|s| s.to_string()),
        hash: String::new(),
        payload,
        refs: Refs::default(),
        schema_version: SCHEMA_VERSION,
        digests: Vec::new(),
        event_family: None,
        event_level: None,
    };

    finalize(&mut event);
    Ok(event)
}

/// Parameters for creating a `cmd` event.
pub struct CmdEventParams<'a> {
    pub branch: &'a str,
    pub parent_hash: Option<&'a str>,
    pub argv: &'a [String],
    pub cwd: &'a str,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub stdout_blob: &'a str,
    pub stderr_blob: &'a str,
}

/// Create a new `cmd` event.
pub fn new_cmd_event(params: &CmdEventParams<'_>) -> anyhow::Result<Event> {
    let payload = serde_json::json!({
        "argv": params.argv,
        "cwd": params.cwd,
        "exit_code": params.exit_code,
        "duration_ms": params.duration_ms,
        "stdout_blob": params.stdout_blob,
        "stderr_blob": params.stderr_blob,
    });

    let mut blob_refs = Vec::new();
    if !params.stdout_blob.is_empty() {
        blob_refs.push(params.stdout_blob.to_string());
    }
    if !params.stderr_blob.is_empty() {
        blob_refs.push(params.stderr_blob.to_string());
    }

    let mut event = Event {
        event_id: new_event_id(),
        ts: now_rfc3339(),
        event_type: "cmd".to_string(),
        branch: params.branch.to_string(),
        parent_hash: params.parent_hash.map(|s| s.to_string()),
        hash: String::new(),
        payload,
        refs: Refs {
            blobs: blob_refs,
            ..Default::default()
        },
        schema_version: SCHEMA_VERSION,
        digests: Vec::new(),
        event_family: None,
        event_level: None,
    };

    finalize(&mut event);
    Ok(event)
}

/// Parameters for creating a `commit` event.
pub struct CommitEventParams<'a> {
    pub branch: &'a str,
    pub parent_hash: Option<&'a str>,
    pub title: &'a str,
    pub purpose: Option<&'a str>,
    pub prev_summary: &'a str,
    pub contribution: &'a str,
    pub evidence: Vec<serde_json::Value>,
    pub labels: Vec<String>,
}

/// Create a new `commit` event.
/// If evidence is empty and labels don't contain "claim", auto-adds "claim" (EVIDENCE-01).
pub fn new_commit_event(params: &mut CommitEventParams<'_>) -> anyhow::Result<Event> {
    // EVIDENCE-01: no evidence → auto label "claim"
    if params.evidence.is_empty() && !params.labels.iter().any(|l| l == "claim") {
        params.labels.push("claim".to_string());
    }

    let payload = serde_json::json!({
        "title": params.title,
        "purpose": params.purpose.unwrap_or(""),
        "prev_summary": params.prev_summary,
        "contribution": params.contribution,
        "evidence": params.evidence,
        "labels": params.labels,
    });

    // Collect event refs from evidence
    let mut event_refs = Vec::new();
    for item in &params.evidence {
        if let Some(eid) = item.get("event_id").and_then(|v| v.as_str()) {
            event_refs.push(eid.to_string());
        }
    }

    let mut event = Event {
        event_id: new_event_id(),
        ts: now_rfc3339(),
        event_type: "commit".to_string(),
        branch: params.branch.to_string(),
        parent_hash: params.parent_hash.map(|s| s.to_string()),
        hash: String::new(),
        payload,
        refs: Refs {
            events: event_refs,
            ..Default::default()
        },
        schema_version: SCHEMA_VERSION,
        digests: Vec::new(),
        event_family: None,
        event_level: None,
    };

    finalize(&mut event);
    Ok(event)
}

/// Create a new `rebuild` event.
pub fn new_rebuild_event(
    branch: &str,
    parent_hash: Option<&str>,
    scope: &str,
    target_branch: Option<&str>,
    reason: &str,
) -> anyhow::Result<Event> {
    let payload = serde_json::json!({
        "scope": scope,
        "branch": target_branch.unwrap_or(""),
        "reason": reason,
    });

    let mut event = Event {
        event_id: new_event_id(),
        ts: now_rfc3339(),
        event_type: "rebuild".to_string(),
        branch: branch.to_string(),
        parent_hash: parent_hash.map(|s| s.to_string()),
        hash: String::new(),
        payload,
        refs: Refs::default(),
        schema_version: SCHEMA_VERSION,
        digests: Vec::new(),
        event_family: None,
        event_level: None,
    };

    finalize(&mut event);
    Ok(event)
}

/// Create a new `branch_create` event.
pub fn new_branch_create_event(
    branch: &str,
    parent_hash: Option<&str>,
    name: &str,
    purpose: &str,
    from_branch: &str,
    from_event_id: Option<&str>,
) -> anyhow::Result<Event> {
    let payload = serde_json::json!({
        "name": name,
        "purpose": purpose,
        "from_branch": from_branch,
        "from_event_id": from_event_id.unwrap_or(""),
    });

    let mut event = Event {
        event_id: new_event_id(),
        ts: now_rfc3339(),
        event_type: "branch_create".to_string(),
        branch: branch.to_string(),
        parent_hash: parent_hash.map(|s| s.to_string()),
        hash: String::new(),
        payload,
        refs: Refs::default(),
        schema_version: SCHEMA_VERSION,
        digests: Vec::new(),
        event_family: None,
        event_level: None,
    };

    finalize(&mut event);
    Ok(event)
}

/// Create a new `branch_switch` event.
pub fn new_branch_switch_event(
    branch: &str,
    parent_hash: Option<&str>,
    from: &str,
    to: &str,
) -> anyhow::Result<Event> {
    let payload = serde_json::json!({
        "from": from,
        "to": to,
    });

    let mut event = Event {
        event_id: new_event_id(),
        ts: now_rfc3339(),
        event_type: "branch_switch".to_string(),
        branch: branch.to_string(),
        parent_hash: parent_hash.map(|s| s.to_string()),
        hash: String::new(),
        payload,
        refs: Refs::default(),
        schema_version: SCHEMA_VERSION,
        digests: Vec::new(),
        event_family: None,
        event_level: None,
    };

    finalize(&mut event);
    Ok(event)
}

/// Create a new `merge` event.
pub fn new_merge_event(
    branch: &str,
    parent_hash: Option<&str>,
    src: &str,
    dst: &str,
    reason: &str,
    adopted_commits: &[String],
) -> anyhow::Result<Event> {
    let payload = serde_json::json!({
        "src": src,
        "dst": dst,
        "reason": reason,
        "adopted_commits": adopted_commits,
    });

    let mut event = Event {
        event_id: new_event_id(),
        ts: now_rfc3339(),
        event_type: "merge".to_string(),
        branch: branch.to_string(),
        parent_hash: parent_hash.map(|s| s.to_string()),
        hash: String::new(),
        payload,
        refs: Refs::default(),
        schema_version: SCHEMA_VERSION,
        digests: Vec::new(),
        event_family: None,
        event_level: None,
    };

    finalize(&mut event);
    Ok(event)
}

/// Parameters for creating an approval event.
pub struct ApprovalEventParams<'a> {
    pub branch: &'a str,
    pub parent_hash: Option<&'a str>,
    pub draft_id: &'a str,
    pub draft_sha256: &'a str,
    pub decision: &'a str,
    pub actor: &'a str,
    pub note: &'a str,
    pub stage_id: &'a str,
    pub role: &'a str,
}

/// Create a new `approval` event.
pub fn new_approval_event(p: &ApprovalEventParams<'_>) -> anyhow::Result<Event> {
    let payload = serde_json::json!({
        "draft_id": p.draft_id,
        "draft_sha256": p.draft_sha256,
        "decision": p.decision,
        "actor": p.actor,
        "note": p.note,
        "stage_id": p.stage_id,
        "role": p.role,
    });

    let mut event = Event {
        event_id: new_event_id(),
        ts: now_rfc3339(),
        event_type: "approval".to_string(),
        branch: p.branch.to_string(),
        parent_hash: p.parent_hash.map(|s| s.to_string()),
        hash: String::new(),
        payload,
        refs: Refs::default(),
        schema_version: SCHEMA_VERSION,
        digests: Vec::new(),
        event_family: None,
        event_level: None,
    };

    finalize(&mut event);
    Ok(event)
}

/// Parameters for creating an approval_request event.
pub struct ApprovalRequestParams<'a> {
    pub branch: &'a str,
    pub parent_hash: Option<&'a str>,
    pub draft_id: &'a str,
    pub draft_sha256: &'a str,
    pub route_rule_id: &'a str,
    pub stage_id: &'a str,
    pub role: &'a str,
    pub assignees: &'a [String],
    pub reason: &'a str,
}

/// Create a new `approval_request` event.
pub fn new_approval_request_event(p: &ApprovalRequestParams<'_>) -> anyhow::Result<Event> {
    let payload = serde_json::json!({
        "draft_id": p.draft_id,
        "draft_sha256": p.draft_sha256,
        "route_rule_id": p.route_rule_id,
        "stage_id": p.stage_id,
        "role": p.role,
        "assignees": p.assignees,
        "reason": p.reason,
    });

    let mut event = Event {
        event_id: new_event_id(),
        ts: now_rfc3339(),
        event_type: "approval_request".to_string(),
        branch: p.branch.to_string(),
        parent_hash: p.parent_hash.map(|s| s.to_string()),
        hash: String::new(),
        payload,
        refs: Refs::default(),
        schema_version: SCHEMA_VERSION,
        digests: Vec::new(),
        event_family: None,
        event_level: None,
    };

    finalize(&mut event);
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_event_has_valid_id_and_hash() {
        let event = new_note_event("main", None, "user", "hello", &[]).unwrap();
        assert!(event.event_id.starts_with("evt_"));
        assert_eq!(event.hash.len(), 64);
        assert!(event.hash.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(event.event_type, "note");
        assert!(event.parent_hash.is_none());
        assert_eq!(event.schema_version, SCHEMA_VERSION);
        assert_eq!(event.digests.len(), 1);
        assert_eq!(event.digests[0].alg, "sha256");
        assert_eq!(event.digests[0].canon, CANON_EDDA_V1);
        assert_eq!(event.digests[0].value, event.hash);
    }

    #[test]
    fn note_event_parent_hash_propagates() {
        let e1 = new_note_event("main", None, "user", "first", &[]).unwrap();
        let e2 = new_note_event("main", Some(&e1.hash), "user", "second", &[]).unwrap();
        assert_eq!(e2.parent_hash.as_deref(), Some(e1.hash.as_str()));
    }

    #[test]
    fn cmd_event_has_blob_refs() {
        let argv = vec!["echo".to_string(), "hi".to_string()];
        let event = new_cmd_event(&CmdEventParams {
            branch: "main",
            parent_hash: None,
            argv: &argv,
            cwd: ".",
            exit_code: 0,
            duration_ms: 100,
            stdout_blob: "blob:sha256:aaa",
            stderr_blob: "blob:sha256:bbb",
        })
        .unwrap();
        assert_eq!(event.event_type, "cmd");
        assert_eq!(event.refs.blobs.len(), 2);
        assert_eq!(event.refs.blobs[0], "blob:sha256:aaa");
        assert!(event.refs.provenance.is_empty());
        assert_eq!(event.schema_version, SCHEMA_VERSION);
        assert_eq!(event.digests.len(), 1);
        assert_eq!(event.digests[0].value, event.hash);
    }

    #[test]
    fn hash_is_deterministic_for_same_content() {
        // Two events with same content but different event_id/ts will have different hashes.
        // But recomputing hash for the same event should yield the same result.
        let event = new_note_event("main", None, "user", "test", &[]).unwrap();
        let mut val = serde_json::to_value(&event).unwrap();
        let obj = val.as_object_mut().unwrap();
        obj.remove("hash");
        obj.remove("digests");
        obj.remove("schema_version");
        let recomputed = compute_event_hash(&val);
        assert_eq!(recomputed, event.hash);
    }

    #[test]
    fn commit_event_auto_claim_when_no_evidence() {
        let event = new_commit_event(&mut CommitEventParams {
            branch: "main",
            parent_hash: None,
            title: "test commit",
            purpose: None,
            prev_summary: "",
            contribution: "did something",
            evidence: vec![],
            labels: vec![],
        })
        .unwrap();
        assert_eq!(event.event_type, "commit");
        let labels = event.payload["labels"].as_array().unwrap();
        assert!(labels.iter().any(|l| l.as_str() == Some("claim")));
        assert_eq!(event.schema_version, SCHEMA_VERSION);
        assert_eq!(event.digests[0].value, event.hash);
    }

    #[test]
    fn commit_event_no_auto_claim_with_evidence() {
        let evidence = vec![serde_json::json!({"event_id": "evt_test", "why": "passed"})];
        let event = new_commit_event(&mut CommitEventParams {
            branch: "main",
            parent_hash: None,
            title: "verified commit",
            purpose: Some("testing"),
            prev_summary: "prev",
            contribution: "this",
            evidence,
            labels: vec!["safe".to_string()],
        })
        .unwrap();
        let labels = event.payload["labels"].as_array().unwrap();
        assert!(!labels.iter().any(|l| l.as_str() == Some("claim")));
        assert!(labels.iter().any(|l| l.as_str() == Some("safe")));
        assert_eq!(event.refs.events, vec!["evt_test"]);
        assert!(event.refs.provenance.is_empty());
        assert_eq!(event.schema_version, SCHEMA_VERSION);
        assert_eq!(event.digests[0].value, event.hash);
    }

    #[test]
    fn rebuild_event_fields() {
        let event =
            new_rebuild_event("main", None, "all", None, "rebuild views").unwrap();
        assert_eq!(event.event_type, "rebuild");
        assert_eq!(event.payload["scope"], "all");
        assert_eq!(event.payload["reason"], "rebuild views");
        assert_eq!(event.schema_version, SCHEMA_VERSION);
        assert_eq!(event.digests[0].value, event.hash);
    }

    #[test]
    fn branch_create_event_fields() {
        let event = new_branch_create_event(
            "main",
            None,
            "feat/x",
            "try alternative",
            "main",
            Some("evt_test"),
        )
        .unwrap();
        assert_eq!(event.event_type, "branch_create");
        assert_eq!(event.payload["name"], "feat/x");
        assert_eq!(event.payload["purpose"], "try alternative");
        assert_eq!(event.payload["from_branch"], "main");
        assert_eq!(event.payload["from_event_id"], "evt_test");
        assert_eq!(event.schema_version, SCHEMA_VERSION);
        assert_eq!(event.digests[0].value, event.hash);
    }

    #[test]
    fn branch_switch_event_fields() {
        let event = new_branch_switch_event("feat/x", None, "main", "feat/x").unwrap();
        assert_eq!(event.event_type, "branch_switch");
        assert_eq!(event.payload["from"], "main");
        assert_eq!(event.payload["to"], "feat/x");
        assert_eq!(event.schema_version, SCHEMA_VERSION);
        assert_eq!(event.digests[0].value, event.hash);
    }

    #[test]
    fn merge_event_fields() {
        let adopted = vec!["evt_a".to_string(), "evt_b".to_string()];
        let event =
            new_merge_event("main", None, "feat/x", "main", "accept feature", &adopted).unwrap();
        assert_eq!(event.event_type, "merge");
        assert_eq!(event.payload["src"], "feat/x");
        assert_eq!(event.payload["dst"], "main");
        assert_eq!(event.payload["reason"], "accept feature");
        let ac = event.payload["adopted_commits"].as_array().unwrap();
        assert_eq!(ac.len(), 2);
        assert_eq!(ac[0], "evt_a");
        assert_eq!(event.schema_version, SCHEMA_VERSION);
        assert_eq!(event.digests[0].value, event.hash);
    }

    #[test]
    fn approval_event_fields() {
        let event = new_approval_event(&ApprovalEventParams {
            branch: "main",
            parent_hash: None,
            draft_id: "drf_test123",
            draft_sha256: "sha256abc",
            decision: "approve",
            actor: "alice",
            note: "LGTM",
            stage_id: "lead",
            role: "lead",
        })
        .unwrap();
        assert_eq!(event.event_type, "approval");
        assert_eq!(event.payload["draft_id"], "drf_test123");
        assert_eq!(event.payload["decision"], "approve");
        assert_eq!(event.payload["actor"], "alice");
        assert_eq!(event.payload["stage_id"], "lead");
        assert_eq!(event.payload["role"], "lead");
        assert_eq!(event.schema_version, SCHEMA_VERSION);
        assert_eq!(event.digests[0].value, event.hash);
    }

    #[test]
    fn approval_request_event_fields() {
        let assignees = vec!["alice".to_string(), "bob".to_string()];
        let event = new_approval_request_event(&ApprovalRequestParams {
            branch: "main",
            parent_hash: None,
            draft_id: "drf_test456",
            draft_sha256: "sha256def",
            route_rule_id: "risky",
            stage_id: "lead",
            role: "lead",
            assignees: &assignees,
            reason: "matched rule risky",
        })
        .unwrap();
        assert_eq!(event.event_type, "approval_request");
        assert_eq!(event.payload["draft_id"], "drf_test456");
        assert_eq!(event.payload["route_rule_id"], "risky");
        assert_eq!(event.payload["stage_id"], "lead");
        assert_eq!(event.payload["role"], "lead");
        let a = event.payload["assignees"].as_array().unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(event.schema_version, SCHEMA_VERSION);
        assert_eq!(event.digests[0].value, event.hash);
    }

    #[test]
    fn event_round_trip_serialize() {
        let event = new_note_event(
            "main",
            None,
            "user",
            "test",
            &["todo".to_string()],
        )
        .unwrap();
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.event_id, event.event_id);
        assert_eq!(deserialized.hash, event.hash);
        assert_eq!(deserialized.event_type, event.event_type);
        assert_eq!(deserialized.branch, event.branch);
        assert_eq!(deserialized.parent_hash, event.parent_hash);
        assert_eq!(deserialized.schema_version, event.schema_version);
        assert_eq!(deserialized.digests, event.digests);
    }

    #[test]
    fn old_event_deserializes_with_defaults() {
        // Simulate a v0 event JSON (no schema_version or digests)
        let json = r#"{
            "event_id": "evt_test",
            "ts": "2026-01-01T00:00:00Z",
            "type": "note",
            "branch": "main",
            "parent_hash": null,
            "hash": "abc123",
            "payload": {"role": "user", "text": "hello", "tags": []}
        }"#;
        let event: Event = serde_json::from_str(json).unwrap();
        assert_eq!(event.schema_version, 0);
        assert!(event.digests.is_empty());
    }

    #[test]
    fn digests_not_in_hash_computation() {
        let e1 = new_note_event("main", None, "user", "test", &[]).unwrap();

        // Remove hash/digests/schema_version and recompute — should match original
        let mut val = serde_json::to_value(&e1).unwrap();
        let obj = val.as_object_mut().unwrap();
        obj.remove("hash");
        obj.remove("digests");
        obj.remove("schema_version");
        let recomputed = compute_event_hash(&val);
        assert_eq!(recomputed, e1.hash);
    }

    #[test]
    fn schema_version_not_in_hash_computation() {
        let e1 = new_note_event("main", None, "user", "test", &[]).unwrap();

        // Change schema_version, exclude it, recompute — should still match
        let mut e2_val = serde_json::to_value(&e1).unwrap();
        let obj = e2_val.as_object_mut().unwrap();
        obj.insert("schema_version".to_string(), serde_json::json!(999));
        obj.remove("hash");
        obj.remove("digests");
        obj.remove("schema_version");
        let recomputed = compute_event_hash(&e2_val);
        assert_eq!(recomputed, e1.hash);
    }

    #[test]
    fn old_event_without_provenance_deserializes() {
        let json = r#"{
            "event_id": "evt_test",
            "ts": "2026-01-01T00:00:00Z",
            "type": "note",
            "branch": "main",
            "parent_hash": null,
            "hash": "abc123",
            "payload": {"role": "user", "text": "hello", "tags": []},
            "refs": {"blobs": [], "events": ["evt_other"]}
        }"#;
        let event: Event = serde_json::from_str(json).unwrap();
        assert!(event.refs.provenance.is_empty());
        assert_eq!(event.refs.events, vec!["evt_other"]);
    }

    #[test]
    fn provenance_included_in_hash() {
        use crate::types::{Provenance, rel};

        let e1 = new_note_event("main", None, "user", "test", &[]).unwrap();
        let mut e2 = new_note_event("main", None, "user", "test", &[]).unwrap();
        // Force same event_id and ts so we can compare hashes
        e2.event_id = e1.event_id.clone();
        e2.ts = e1.ts.clone();
        finalize_event(&mut e2);
        assert_eq!(e1.hash, e2.hash);

        // Add provenance and re-finalize
        e2.refs.provenance.push(Provenance {
            target: "evt_other".to_string(),
            rel: rel::BASED_ON.to_string(),
            note: None,
        });
        finalize_event(&mut e2);
        assert_ne!(e1.hash, e2.hash);
    }

    #[test]
    fn provenance_round_trip_serialize() {
        use crate::types::{Provenance, rel};

        let mut event = new_note_event("main", None, "user", "test", &[]).unwrap();
        event.refs.provenance.push(Provenance {
            target: "evt_abc".to_string(),
            rel: rel::REVIEWS.to_string(),
            note: Some("review note".to_string()),
        });
        finalize_event(&mut event);

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.refs.provenance.len(), 1);
        assert_eq!(deserialized.refs.provenance[0].target, "evt_abc");
        assert_eq!(deserialized.refs.provenance[0].rel, rel::REVIEWS);
        assert_eq!(
            deserialized.refs.provenance[0].note.as_deref(),
            Some("review note")
        );
        assert_eq!(deserialized.hash, event.hash);
    }

    #[test]
    fn empty_provenance_not_serialized() {
        let event = new_note_event("main", None, "user", "test", &[]).unwrap();
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("provenance"));
    }

    #[test]
    fn taxonomy_note_is_signal_info() {
        let event = new_note_event("main", None, "user", "test", &[]).unwrap();
        assert_eq!(event.event_family.as_deref(), Some("signal"));
        assert_eq!(event.event_level.as_deref(), Some("info"));
    }

    #[test]
    fn taxonomy_cmd_is_signal_trace() {
        let argv = vec!["echo".to_string()];
        let event = new_cmd_event(&CmdEventParams {
            branch: "main",
            parent_hash: None,
            argv: &argv,
            cwd: ".",
            exit_code: 0,
            duration_ms: 10,
            stdout_blob: "",
            stderr_blob: "",
        })
        .unwrap();
        assert_eq!(event.event_family.as_deref(), Some("signal"));
        assert_eq!(event.event_level.as_deref(), Some("trace"));
    }

    #[test]
    fn taxonomy_commit_is_milestone() {
        let event = new_commit_event(&mut CommitEventParams {
            branch: "main",
            parent_hash: None,
            title: "test",
            purpose: None,
            prev_summary: "",
            contribution: "x",
            evidence: vec![],
            labels: vec![],
        })
        .unwrap();
        assert_eq!(event.event_family.as_deref(), Some("milestone"));
        assert_eq!(event.event_level.as_deref(), Some("milestone"));
    }

    #[test]
    fn taxonomy_rebuild_is_admin_trace() {
        let event = new_rebuild_event("main", None, "all", None, "rebuild").unwrap();
        assert_eq!(event.event_family.as_deref(), Some("admin"));
        assert_eq!(event.event_level.as_deref(), Some("trace"));
    }

    #[test]
    fn taxonomy_approval_is_governance() {
        let event = new_approval_event(&ApprovalEventParams {
            branch: "main",
            parent_hash: None,
            draft_id: "drf_test",
            draft_sha256: "abc",
            decision: "approve",
            actor: "alice",
            note: "",
            stage_id: "lead",
            role: "lead",
        })
        .unwrap();
        assert_eq!(event.event_family.as_deref(), Some("governance"));
        assert_eq!(event.event_level.as_deref(), Some("governance"));
    }

    #[test]
    fn taxonomy_backward_compat_old_event_no_family() {
        let json = r#"{
            "event_id": "evt_old",
            "ts": "2026-01-01T00:00:00Z",
            "type": "note",
            "branch": "main",
            "parent_hash": null,
            "hash": "abc123",
            "payload": {"role": "user", "text": "hello", "tags": []}
        }"#;
        let event: Event = serde_json::from_str(json).unwrap();
        assert!(event.event_family.is_none());
        assert!(event.event_level.is_none());
    }

    #[test]
    fn taxonomy_round_trip_preserves_fields() {
        let event = new_note_event("main", None, "user", "test", &[]).unwrap();
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.event_family, event.event_family);
        assert_eq!(deserialized.event_level, event.event_level);
    }

    #[test]
    fn classify_unknown_type_returns_none() {
        use crate::types::classify_event_type;
        let (f, l) = classify_event_type("unknown_custom_type");
        assert!(f.is_none());
        assert!(l.is_none());
    }

    #[test]
    fn rel_constants_are_correct() {
        use crate::types::rel;
        assert_eq!(rel::BASED_ON, "based_on");
        assert_eq!(rel::SUPERSEDES, "supersedes");
        assert_eq!(rel::CONTINUES, "continues");
        assert_eq!(rel::REVIEWS, "reviews");
    }
}
