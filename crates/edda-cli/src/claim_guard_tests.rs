use super::*;
use std::path::Path;

fn issue(comments: &[(&str, &str)]) -> GhIssue {
    GhIssue {
        comments: comments
            .iter()
            .map(|(created_at, body)| GhComment {
                body: (*body).to_owned(),
                created_at: Some((*created_at).to_owned()),
            })
            .collect(),
    }
}

fn pr(number: u64, state: PrState, title: &str, head: &str) -> GhPr {
    GhPr {
        number,
        state,
        title: title.into(),
        head_ref_name: head.into(),
    }
}

#[test]
fn gh782_routing_labels_are_not_claims() {
    let fixture: GhIssue = serde_json::from_str(
        r#"{"labels":[{"name":"lane:feature"},{"name":"lane:4090"}],"comments":[]}"#,
    )
    .unwrap();
    assert_eq!(
        claim_state("4090/worker-1", &fixture),
        ClaimState::Unclaimed
    );
}

#[test]
fn gh782_bare_machine_cannot_alias_two_worker_roles() {
    assert_eq!(
        claim_state("4090", &issue(&[("t", "taking: 4090")])),
        ClaimState::ClaimedBySelf
    );
    assert!(
        validate_machine("4090").is_err(),
        "bare machine aliases worker-1 and worker-2"
    );
    assert!(matches!(
        claim_state("4090/worker-1", &issue(&[("t", "taking: 4090")])),
        ClaimState::ClaimedByOther { .. }
    ));
}

#[test]
fn no_comment_is_unclaimed_regardless_of_routing_labels_omitted_by_query() {
    assert_eq!(
        claim_state("4090/worker-1", &issue(&[])),
        ClaimState::Unclaimed
    );
}

#[test]
fn same_machine_different_role_is_other() {
    let state = claim_state(
        "4090/worker-1",
        &issue(&[(
            "2026-09-02T06:30:00Z",
            "taking: 4090/worker-2 at 2026-09-02T06:30:00Z",
        )]),
    );
    assert_eq!(
        state,
        ClaimState::ClaimedByOther {
            machine: "4090/worker-2".into(),
            when: Some("2026-09-02T06:30:00Z".into()),
            source: "comment \"taking:\"".into(),
        }
    );
}

#[test]
fn self_comment_is_idempotent() {
    assert_eq!(
        claim_state("4090/worker-1", &issue(&[("t", "taking: 4090/worker-1")])),
        ClaimState::ClaimedBySelf
    );
}

#[test]
fn any_foreign_marker_refuses_even_if_self_marker_appears_first() {
    let state = claim_state(
        "4090/worker-1",
        &issue(&[("t", "taking: 4090/worker-1\ntaking: 4090/worker-2")]),
    );
    assert!(matches!(state, ClaimState::ClaimedByOther { .. }));
}

#[test]
fn taking_is_only_recognized_at_line_start() {
    let tokens: Vec<_> = taking_tokens("remember: taking: x/y\n  taking: a/b\ntaking:").collect();
    assert_eq!(tokens, ["a/b"]);
}

#[test]
fn identity_shape_requires_exactly_one_slash() {
    assert!(validate_machine("4090/worker-1").is_ok());
    for invalid in ["4090", "4090/worker 1", "4090//x", "/worker-1", "4090/"] {
        assert!(validate_machine(invalid).is_err(), "accepted {invalid:?}");
    }
}

#[test]
fn pr_read_catches_title_and_branch_references_case_insensitively() {
    assert_eq!(
        pr_state(782, &[pr(900, PrState::Merged, "fix (GH-782)", "x")]),
        Some(ClaimState::InFlight {
            pr: 900,
            state: PrState::Merged
        })
    );
    assert_eq!(
        pr_state(782, &[pr(901, PrState::Open, "x", "codex/GH782-claim")]),
        Some(ClaimState::InFlight {
            pr: 901,
            state: PrState::Open
        })
    );
    assert_eq!(
        pr_state(782, &[pr(902, PrState::Closed, "x GH-782", "gh782")]),
        None
    );
    assert_eq!(
        pr_state(782, &[pr(903, PrState::Open, "x GH-7820", "gh7820")]),
        None
    );
}

#[test]
fn merged_delivery_wins_over_an_open_duplicate() {
    let prs = [
        pr(901, PrState::Open, "(GH-782)", "x"),
        pr(900, PrState::Merged, "x", "fix/gh782"),
    ];
    assert_eq!(
        pr_state(782, &prs),
        Some(ClaimState::InFlight {
            pr: 900,
            state: PrState::Merged
        })
    );
}

static GH_BIN_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn fetch_fails_closed_when_gh_is_missing() {
    let _lock = GH_BIN_ENV_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let old = std::env::var_os("EDDA_GH_BIN");
    std::env::set_var("EDDA_GH_BIN", "definitely-no-such-gh-9f3a");
    let result = fetch_claim_state(782, "4090/worker-1", Path::new("."));
    match old {
        Some(v) => std::env::set_var("EDDA_GH_BIN", v),
        None => std::env::remove_var("EDDA_GH_BIN"),
    }
    assert!(result.is_err());
}
