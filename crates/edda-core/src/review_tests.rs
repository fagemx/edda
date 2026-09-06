use crate::event::{
    new_cmd_event, new_cmd_event_with_git_context, new_review_verdict_event, CmdEventParams,
};
use crate::ReviewVerdictPayload;

fn payload() -> ReviewVerdictPayload {
    serde_json::from_value(serde_json::json!({
        "schema":"review_verdict/0",
        "subject":{"base_sha":"a".repeat(40),"head_sha":"b".repeat(40),"files":2,"lines":10,"coverage":"full","subject_seen":"b".repeat(40)},
        "refs":{"supersedes":"evt_older","previous":"evt_rebased","round":2,"history_rewritten":true},
        "spec":{"mode":"spec-backed","source":"issue#652","trust":"maintainer"},
        "brief":{"core":"core-v1","classes":["code-risk"]},
        "reviewer":{"agent":"pi","transport":"pi","model_requested":"inherited","model_observed":"gpt-5.6-sol","observed_via":"in-band","session_id":"review-session","session_label":"review-b-r2","tool_policy":"hard"},
        "independence":"unverified","independence_policy":"session",
        "gates":{"status":"verified","read":[],"ran":[]},
        "verdict":"changes-requested","outcome":"done","qualified":true,
        "findings":[{"id":"f1","severity":"P1","file":"x.rs","line":3,"claim":"c","evidence":"e","rule":"core","status":"open"}],
        "cost":{"usd":0.0,"measured":true,"duration_ms":5},"parse":"ok"
    })).expect("review payload")
}

#[test]
fn review_verdict_event_roundtrip_taxonomy_and_refs() {
    let payload = payload();
    let event = new_review_verdict_event(
        "main",
        Some("parent"),
        &payload,
        Some("evt_older"),
        Some("evt_rebased"),
        &["raw".into(), "stdout".into()],
    )
    .expect("event");
    assert_eq!(event.event_type, "review_verdict");
    assert_ne!(event.event_type, "verdict.recorded");
    assert_eq!(event.event_family.as_deref(), Some("signal"));
    assert_eq!(event.event_level.as_deref(), Some("info"));
    assert_eq!(event.refs.events, ["evt_older", "evt_rebased"]);
    assert_eq!(event.refs.blobs, ["raw", "stdout"]);
    assert_eq!(event.parent_hash.as_deref(), Some("parent"));
    assert_eq!(event.hash.len(), 64);
    assert_eq!(event.digests[0].value, event.hash);
    let parsed: ReviewVerdictPayload = serde_json::from_value(event.payload).expect("roundtrip");
    assert_eq!(parsed.findings[0].id, "f1");
    assert_eq!(parsed.cost.usd, Some(0.0));
    assert!(parsed.cost.measured);
}

#[test]
fn cmd_receipt_writes_context_and_preserves_unknown_as_null() {
    let argv = vec!["cargo".into(), "test".into()];
    let params = CmdEventParams {
        branch: "main",
        parent_hash: None,
        argv: &argv,
        cwd: "/repo",
        exit_code: 0,
        duration_ms: 1,
        stdout_blob: "out",
        stderr_blob: "err",
    };
    let old = new_cmd_event(&params).expect("legacy constructor");
    assert!(old.payload.get("git_sha").expect("sha key").is_null());
    assert!(old.payload.get("tree_dirty").expect("dirty key").is_null());
    for dirty in [false, true] {
        let receipt = new_cmd_event_with_git_context(&params, Some(&"a".repeat(40)), Some(dirty))
            .expect("receipt");
        assert_eq!(receipt.payload["git_sha"], "a".repeat(40));
        assert_eq!(receipt.payload["tree_dirty"], dirty);
        assert_eq!(receipt.refs.blobs, ["out", "err"]);
        assert_eq!(receipt.event_family.as_deref(), Some("signal"));
    }
}
