//! GH-651 golden fixture for the `edda ask --json` stable contract (GH-651, GH-789).
//!
//! Spawns the compiled `edda` binary (`CARGO_BIN_EXE_edda`) against a temporary ledger.

use edda_core::event::new_decision_event;
use edda_core::types::DecisionPayload;
use edda_ledger::Ledger;
use std::path::{Path, PathBuf};

fn edda_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_edda"))
}

/// Run `edda ask` in `repo` and return (exit code, stdout, stderr).
fn run_edda_ask(args: &[&str], repo: &Path) -> (i32, String, String) {
    let out = std::process::Command::new(edda_bin())
        .args(args)
        .current_dir(repo)
        .output()
        .expect("spawn edda");
    (
        out.status.code().expect("exit code"),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// GH-651 golden fixture for the `edda ask --json` stable contract
/// (ledger decision `compat.stable-json-surfaces`; policy page:
/// COMPATIBILITY.md § "Stable `--json` contracts"). Within 0.x, keys may
/// be added, never deleted, renamed, or retyped. Pins the exact top-level
/// key set (as it is emitted today: `tasks`, `dependents`,
/// `override_risk`, `workspace_event_count`, and `workspace_decision_count`
/// are `skip_serializing_if` — absent when empty/None; both counts are
/// always known here because this fixture opens a real ledger first)
/// and the per-key types of the envelope and of a `DecisionHit`, through
/// the real binary.
#[test]
fn compat_golden_fixture_ask_json_keys_and_types() {
    let repo = tempfile::tempdir().expect("repo tempdir");
    let ledger = Ledger::open_or_init(repo.path()).expect("open_or_init");
    let parent = ledger.last_event_hash().expect("parent hash");
    let dp = DecisionPayload {
        key: "db.engine".to_string(),
        value: "sqlite".to_string(),
        reason: Some("golden fixture".to_string()),
        scope: None,
        authority: None,
        affected_paths: None,
        tags: None,
        review_after: None,
        reversibility: None,
        village_id: None,
    };
    let event =
        new_decision_event("main", parent.as_deref(), "system", &dp).expect("decision event");
    ledger.append_event(&event).expect("append decision");
    drop(ledger);

    let (code, stdout, stderr) = run_edda_ask(&["ask", "--json", "db"], repo.path());
    assert_eq!(code, 0, "stdout={stdout:?} stderr={stderr:?}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");

    let mut keys: Vec<&str> = v
        .as_object()
        .expect("one JSON object")
        .keys()
        .map(|k| k.as_str())
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "conversations",
            "decisions",
            "input_type",
            "query",
            "related_commits",
            "related_notes",
            "timeline",
            "workspace_decision_count",
            "workspace_event_count",
        ],
        "ask --json top-level key set changed — this is a stable contract; \
         see COMPATIBILITY.md (tasks/dependents/override_risk and the \
         workspace counts are absent when empty/unknown by the \
         skip_serializing_if contract)"
    );
    assert_eq!(v["query"], "db");
    assert!(v["input_type"].is_string());
    assert!(
        v["workspace_event_count"].is_u64(),
        "workspace_event_count must be an integer: {v}"
    );
    assert!(
        v["workspace_decision_count"].is_u64(),
        "workspace_decision_count must be an integer: {v}"
    );
    for section in [
        "decisions",
        "timeline",
        "related_commits",
        "related_notes",
        "conversations",
    ] {
        assert!(v[section].is_array(), "{section} must be an array: {v}");
    }

    // DecisionHit shape (the only populated section here).
    let d = &v["decisions"][0];
    let mut dkeys: Vec<&str> = d
        .as_object()
        .expect("decision object")
        .keys()
        .map(|k| k.as_str())
        .collect();
    dkeys.sort_unstable();
    assert_eq!(
        dkeys,
        vec![
            "branch",
            "domain",
            "event_id",
            "governance",
            "is_active",
            "key",
            "reason",
            "ts",
            "value",
        ],
        "DecisionHit key set changed — this is a stable contract; \
         see COMPATIBILITY.md"
    );
    assert!(d["event_id"].is_string());
    assert_eq!(d["key"], "db.engine");
    assert_eq!(d["value"], "sqlite");
    assert!(d["reason"].is_string());
    assert!(d["domain"].is_string());
    assert!(d["branch"].is_string());
    assert!(d["ts"].is_string());
    assert_eq!(d["is_active"], serde_json::Value::Bool(true));
    assert!(d["governance"].is_object());
    assert_eq!(d["governance"]["status"], "unratified");
}
