//! Executable counterpart of docs/reference/ledger-event-spec.md.

use crate::event::finalize_event;
use crate::Event;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[path = "event_conformance_sources.rs"]
mod sources;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn registry() -> Vec<Value> {
    json(&root().join("spec/events/registry.json"))
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn all_registered_payloads_have_valid_schema_and_hashed_fixture() {
    let envelope = json(&root().join("spec/events/envelope.schema.json"));
    assert!(jsonschema::meta::is_valid(&envelope));
    let envelope = jsonschema::validator_for(&envelope).unwrap();
    let mut types = BTreeSet::new();
    for entry in registry() {
        let name = entry["type"].as_str().unwrap();
        assert!(types.insert(name.to_owned()), "duplicate type {name}");
        assert!(matches!(
            entry["stability"].as_str(),
            Some("stable-v1" | "unstable")
        ));
        assert!(root().join(entry["source"].as_str().unwrap()).is_file());
        let schema = json(
            &root()
                .join("spec/events")
                .join(entry["schema"].as_str().unwrap()),
        );
        assert!(
            jsonschema::meta::is_valid(&schema),
            "invalid schema: {name}"
        );
        let validator = jsonschema::validator_for(&schema).unwrap();
        let fixture =
            std::fs::read_to_string(root().join(format!("tests/fixtures/events/{name}.jsonl")))
                .unwrap();
        assert!(!fixture.trim().is_empty(), "empty fixture: {name}");
        let mut previous = None;
        for line in fixture.lines() {
            let value: Value = serde_json::from_str(line).unwrap();
            envelope
                .validate(&value)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            validator
                .validate(&value["payload"])
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let event: Event = serde_json::from_value(value).unwrap();
            assert_eq!(event.event_type, name);
            assert_eq!(event.parent_hash, previous, "fixture linkage: {name}");
            previous = Some(event.hash.clone());
            let mut recomputed = event.clone();
            finalize_event(&mut recomputed).unwrap();
            assert_eq!(event.hash, recomputed.hash, "hash: {name}");
            assert_eq!(event.digests, recomputed.digests, "digests: {name}");
            assert_eq!(
                event.event_family, recomputed.event_family,
                "family: {name}"
            );
            assert_eq!(event.event_level, recomputed.event_level, "level: {name}");
        }
    }
    let files: BTreeSet<_> = std::fs::read_dir(root().join("tests/fixtures/events"))
        .unwrap()
        .map(|e| {
            e.unwrap()
                .path()
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(types, files, "unregistered fixture or missing type");
}

#[test]
fn canonical_byte_vectors_match_before_hashing() {
    for vector in json(&root().join("spec/events/canonical-v1.json"))
        .as_array()
        .unwrap()
    {
        let value: Value = serde_json::from_str(vector["input"].as_str().unwrap()).unwrap();
        let actual = crate::canon::canonical_json_bytes(&value).unwrap();
        assert_eq!(actual, vector["canonical"].as_str().unwrap().as_bytes());
    }
}

#[test]
fn document_inventory_and_algorithm_cross_references_stay_connected() {
    let document =
        std::fs::read_to_string(root().join("docs/reference/ledger-event-spec.md")).unwrap();
    for entry in registry() {
        assert!(document.contains(&format!("| `{}` |", entry["type"].as_str().unwrap())));
    }
    let ledger_source =
        std::fs::read_to_string(root().join("crates/edda-ledger/src/ledger.rs")).unwrap();
    assert!(ledger_source.contains("docs/reference/ledger-event-spec.md#chain-verification"));
    assert!(include_str!("event.rs").contains("docs/reference/ledger-event-spec.md#canonical-hash"));
    assert!(document.contains("## Canonical hash") && document.contains("## Chain verification"));
}

#[test]
fn schemas_reject_malformed_required_nested_and_variant_fields() {
    let check = |name: &str, invalid: Value| {
        let schema = json(&root().join(format!("spec/events/{name}.schema.json")));
        assert!(
            !jsonschema::is_valid(&schema, &invalid),
            "accepted malformed {name}"
        );
    };
    check(
        "note",
        serde_json::json!({"role":"agent","text":"x","tags":[],"decision":{"key":"k","value":7}}),
    );
    check(
        "checkpoint",
        serde_json::json!({"role":"agent","tags":[],"hypotheses":[],"rejected":[{"hypothesis":"h"}],"open":[],"next":"n"}),
    );
    check("task.session", serde_json::json!({"task_id":1}));
    check(
        "task.done",
        serde_json::json!({"task_id":1,"receipt":"  ","evidence_paths":[]}),
    );
    check("device_revoke", serde_json::json!({}));
    check(
        "envelope",
        serde_json::json!({"event_id":"x","ts":"t","type":"note","branch":"main","hash":"not-a-hash","payload":{}}),
    );
}

#[test]
fn production_event_types_cannot_silently_escape_registration() {
    let registered: BTreeSet<_> = registry()
        .iter()
        .map(|e| e["type"].as_str().unwrap().to_owned())
        .collect();
    let emitted = sources::inventory(&root().join("crates"));
    assert_eq!(
        emitted, registered,
        "update payload schema, registry, fixture and specification together"
    );
}

#[test]
fn canonical_hash_exclusions_and_hashed_content_are_pinned() {
    let line = std::fs::read_to_string(root().join("tests/fixtures/events/note.jsonl")).unwrap();
    let event: Event = serde_json::from_str(line.lines().next().unwrap()).unwrap();
    let mut metadata = event.clone();
    metadata.schema_version = 0;
    metadata.digests.clear();
    metadata.hash.clear();
    finalize_event(&mut metadata).unwrap();
    assert_eq!(metadata.hash, event.hash);
    let mut changed = event.clone();
    changed.refs.events.push("evt_other".into());
    finalize_event(&mut changed).unwrap();
    assert_ne!(changed.hash, event.hash);
    changed = event.clone();
    changed.payload["future_field"] = serde_json::json!({"z":1,"a":[3,2,1]});
    finalize_event(&mut changed).unwrap();
    assert_ne!(changed.hash, event.hash);
}

#[test]
fn legacy_defaults_and_unknown_fields_match_documented_read_behavior() {
    let mut value = serde_json::json!({
        "event_id":"legacy", "ts":"2026-09-04T00:00:00Z", "type":"future.type",
        "branch":"main", "hash":"", "payload":{"unknown_payload":true},
        "unknown_envelope":true
    });
    let mut event: Event = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(event.schema_version, 0);
    assert_eq!(event.parent_hash, None);
    assert!(event.refs.events.is_empty());
    finalize_event(&mut event).unwrap();
    assert_eq!(event.event_family, None);
    let roundtrip = serde_json::to_value(event).unwrap();
    assert!(roundtrip.get("unknown_envelope").is_none());
    assert_eq!(roundtrip["payload"]["unknown_payload"], true);
    value["schema_version"] = serde_json::json!(99);
    assert_eq!(
        serde_json::from_value::<Event>(value)
            .unwrap()
            .schema_version,
        99
    );
}
