use super::*;
use crate::test_support::env_guard;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn setup_workspace() -> (std::path::PathBuf, edda_ledger::Ledger) {
    let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let tmp = std::env::temp_dir().join(format!("edda_ratify_test_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let paths = edda_ledger::EddaPaths::discover(&tmp);
    edda_ledger::ledger::init_workspace(&paths).unwrap();
    edda_ledger::ledger::init_head(&paths, "main").unwrap();
    edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
    let ledger = edda_ledger::Ledger::open(&tmp).unwrap();
    (tmp, ledger)
}

/// Write a decision the way `edda decide` writes one, including the
/// structured `cites` field when given.
fn decide(ledger: &edda_ledger::Ledger, key: &str, value: &str, reason: &str, cites: &[&str]) {
    let branch = ledger.head_branch().unwrap();
    let parent_hash = ledger.last_event_hash().unwrap();
    let dp = edda_core::types::DecisionPayload {
        key: key.to_string(),
        value: value.to_string(),
        reason: Some(reason.to_string()),
        scope: None,
        authority: Some(edda_core::types::authority::AGENT.to_string()),
        affected_paths: None,
        tags: None,
        review_after: None,
        reversibility: None,
        village_id: None,
        cites: if cites.is_empty() {
            None
        } else {
            Some(cites.iter().map(|c| (*c).to_string()).collect())
        },
    };
    let event = edda_core::event::new_decision_event(&branch, parent_hash.as_deref(), "agent", &dp)
        .unwrap();
    ledger.append_event(&event).unwrap();
}

fn args(key: Option<&str>) -> RatifyArgs {
    RatifyArgs {
        key: key.map(str::to_string),
        note: None,
        by: Some("operator".to_string()),
        evidence: None,
        by_rule: None,
        dry_run: false,
        session: None,
    }
}

fn ratify_events(ledger: &edda_ledger::Ledger) -> Vec<edda_core::types::Event> {
    ledger.iter_events_by_type("decision_ratify").unwrap()
}

fn is_binding(ledger: &edda_ledger::Ledger, key: &str) -> bool {
    let branch = ledger.head_branch().unwrap();
    let Some(d) = ledger.find_active_decision(&branch, key).unwrap() else {
        return false;
    };
    ledger
        .ratified_decisions_map()
        .unwrap()
        .contains_key(&d.event_id)
}

const SHA: &str = "03c604ffea4b2a1731b7866e7f701374eb03b156";

// ── evidence form (GH-764) ──────────────────────────────────────────

#[test]
fn evidence_form_writes_a_typed_ratified_by_and_binds() {
    let (tmp, ledger) = setup_workspace();
    decide(&ledger, "x.y", "one", "because", &[]);

    let mut a = args(Some("x.y"));
    a.by = None;
    a.evidence = Some(format!("pr#1@{SHA}"));
    run(&tmp, &a).unwrap();

    let events = ratify_events(&ledger);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].payload["key"], "x.y");
    assert_eq!(
        events[0].payload["ratified_by"],
        format!("evidence:pr#1@{SHA}")
    );
    assert!(is_binding(&ledger, "x.y"));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn ratifying_an_already_binding_key_writes_nothing() {
    // Exit 0 with no second event: a post-merge hook that fires twice on the
    // same PR must not append a second assertion of what the ledger says.
    let (tmp, ledger) = setup_workspace();
    decide(&ledger, "x.y", "one", "because", &[]);

    let mut a = args(Some("x.y"));
    a.by = None;
    a.evidence = Some(format!("pr#1@{SHA}"));
    run(&tmp, &a).unwrap();
    run(&tmp, &a).unwrap();

    assert_eq!(ratify_events(&ledger).len(), 1);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn unknown_key_is_an_error_not_a_silent_success() {
    let (tmp, _ledger) = setup_workspace();
    let err = run(&tmp, &args(Some("nope.key"))).unwrap_err();
    assert!(err.to_string().contains("no active decision"), "{err}");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn ratify_records_a_separate_event_never_a_mutation() {
    let (tmp, ledger) = setup_workspace();
    decide(&ledger, "db.engine", "postgres", "because", &[]);
    let decisions_before = ledger
        .active_decisions(None, None, None, None)
        .unwrap()
        .len();

    run(&tmp, &args(Some("db.engine"))).unwrap();

    let events = ratify_events(&ledger);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].payload["ratified_by"], "operator");
    assert_eq!(
        ledger
            .active_decisions(None, None, None, None)
            .unwrap()
            .len(),
        decisions_before,
        "ratify must not rewrite the decision"
    );
    assert!(is_binding(&ledger, "db.engine"));
    let _ = std::fs::remove_dir_all(&tmp);
}

// ── rule form (GH-761) ──────────────────────────────────────────────

/// The four cases named in GH-761's doneWhen, on one fixture ledger.
fn four_case_ledger() -> (std::path::PathBuf, edda_ledger::Ledger) {
    let (tmp, ledger) = setup_workspace();
    // 1. cited → ratified
    decide(&ledger, "db.engine", "sqlite", "embedded", &["issue:#742"]);
    // 2. held domain → held even though it is cited
    decide(
        &ledger,
        "product.tier",
        "pro",
        "pricing",
        &["operator:2026-09-03"],
    );
    // 3. superseded → held (a later decision's reason names it)
    decide(
        &ledger,
        "review.engine",
        "opus",
        "per #900",
        &["issue:#900"],
    );
    decide(&ledger, "review.pool", "two", "replaces review.engine", &[]);
    // 4. no citation → held
    decide(&ledger, "cache.ttl", "60s", "seems about right", &[]);
    (tmp, ledger)
}

fn rule_args(dry_run: bool) -> RatifyArgs {
    RatifyArgs {
        key: None,
        note: None,
        by: None,
        evidence: None,
        by_rule: Some(RULE_CITED_AUTHORITY.to_string()),
        dry_run,
        session: None,
    }
}

#[test]
fn rule_ratifies_only_the_cited_unheld_decision() {
    let (tmp, ledger) = four_case_ledger();
    run(&tmp, &rule_args(false)).unwrap();

    assert!(is_binding(&ledger, "db.engine"), "cited → ratified");
    assert!(!is_binding(&ledger, "product.tier"), "product.* → held");
    assert!(!is_binding(&ledger, "review.engine"), "superseded → held");
    assert!(!is_binding(&ledger, "cache.ttl"), "no citation → held");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn dry_run_writes_nothing() {
    let (tmp, ledger) = four_case_ledger();
    run(&tmp, &rule_args(true)).unwrap();
    assert!(ratify_events(&ledger).is_empty());
    assert!(!is_binding(&ledger, "db.engine"));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn rule_ratifications_are_distinguishable_from_operator_ones() {
    // doneWhen: `edda log` and `edda ask` must be able to tell which
    // authority conferred binding status.
    let (tmp, ledger) = four_case_ledger();
    run(&tmp, &rule_args(false)).unwrap();

    let events = ratify_events(&ledger);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].payload["ratified_by"],
        format!("rule:{RULE_CITED_AUTHORITY}")
    );
    assert!(
        events[0].payload["note"]
            .as_str()
            .is_some_and(|n| n.contains("issue:#742")),
        "the matched citation rides along: {:?}",
        events[0].payload["note"]
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn a_second_sweep_is_a_no_op() {
    let (tmp, ledger) = four_case_ledger();
    run(&tmp, &rule_args(false)).unwrap();
    run(&tmp, &rule_args(false)).unwrap();
    assert_eq!(ratify_events(&ledger).len(), 1);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn a_re_decided_key_is_ratified_once_not_once_per_row() {
    // `active_decisions` can hand back more than one active row for a
    // `(branch, key)`; the sweep must still write one event per key.
    let (tmp, ledger) = setup_workspace();
    decide(&ledger, "x.y", "one", "per #1", &["issue:#1"]);
    decide(&ledger, "x.y", "two", "per #2", &["issue:#2"]);
    run(&tmp, &rule_args(false)).unwrap();

    let events = ratify_events(&ledger);
    assert_eq!(
        events.len(),
        1,
        "one ratify per key, got {:?}",
        events
            .iter()
            .map(|e| e.payload["key"].clone())
            .collect::<Vec<_>>()
    );
    assert!(is_binding(&ledger, "x.y"));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn decisions_without_cites_still_work_through_the_reason_fallback() {
    let (tmp, ledger) = setup_workspace();
    decide(&ledger, "legacy.key", "v", "landed per GH-401", &[]);
    run(&tmp, &rule_args(false)).unwrap();
    assert!(is_binding(&ledger, "legacy.key"));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn sweeping_an_empty_ledger_is_not_an_error() {
    let (tmp, _ledger) = setup_workspace();
    run(&tmp, &rule_args(true)).unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
}

// ── through `edda decide` — these mutate process env, so they take the
//    shared guard in `crate::test_support` (one test binary, one lock) ──

#[test]
fn decide_persists_structured_citations() {
    // GH-761: `--cite` writes a typed field the ratify rule can read, rather
    // than leaving the rule to guess authority from prose.
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    let (tmp, ledger) = setup_workspace();
    let pid = edda_store::project_id(&tmp);
    let _ = edda_store::ensure_dirs(&pid);
    std::env::set_var("EDDA_SESSION_ID", "test-cite-s1");
    std::env::set_var("EDDA_SESSION_LABEL", "worker");

    crate::cmd_bridge::decide(
        &tmp,
        "review.engine=opus",
        Some("per the window"),
        &[],
        None,
        None,
        &[],
        &[],
        &["issue:#742".to_string(), "operator:2026-09-03".to_string()],
    )
    .unwrap();

    let events = ledger.iter_events().unwrap();
    let cites = &events[0].payload["decision"]["cites"];
    assert_eq!(cites[0], "issue:#742");
    assert_eq!(cites[1], "operator:2026-09-03");

    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(edda_store::project_dir(&pid));
}

#[test]
fn ratify_records_separate_event_and_makes_decision_binding() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    let (tmp, ledger) = setup_workspace();
    let pid = edda_store::project_id(&tmp);
    let _ = edda_store::ensure_dirs(&pid);
    std::env::set_var("EDDA_SESSION_ID", "test-ratify-s1");
    std::env::set_var("EDDA_SESSION_LABEL", "worker");

    crate::cmd_bridge::decide(
        &tmp,
        "db.engine=sqlite",
        Some("embedded"),
        &[],
        None,
        None,
        &[],
        &[],
        &[],
    )
    .unwrap();

    // Before ratify: the active decision is not binding.
    assert!(ledger.ratified_decision_events().unwrap().is_empty());

    crate::cmd_ratify::run(
        &tmp,
        &crate::cmd_ratify::RatifyArgs {
            key: Some("db.engine".to_string()),
            note: Some("looks right".to_string()),
            by: Some("operator".to_string()),
            evidence: None,
            by_rule: None,
            dry_run: false,
            session: None,
        },
    )
    .unwrap();

    // A distinct decision_ratify event was written (not a mutation).
    let ratify_events = ledger.iter_events_by_type("decision_ratify").unwrap();
    assert_eq!(ratify_events.len(), 1);
    assert_eq!(ratify_events[0].payload["key"], "db.engine");
    assert_eq!(ratify_events[0].payload["ratified_by"], "operator");

    // The projection now reports the key as binding.
    let views = ledger.active_decisions(None, None, None, None).unwrap();
    let view = views.iter().find(|v| v.key == "db.engine").unwrap();
    let set = ledger.ratified_decision_events().unwrap();
    assert!(edda_ledger::view::is_decision_ratified(view, &set));

    std::env::remove_var("EDDA_SESSION_ID");
    std::env::remove_var("EDDA_SESSION_LABEL");
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(edda_store::project_dir(&pid));
}

#[test]
fn ratify_unknown_key_errors() {
    let _store = crate::test_support::isolated_store();
    let _env = env_guard();
    let (tmp, _ledger) = setup_workspace();
    let pid = edda_store::project_id(&tmp);
    let _ = edda_store::ensure_dirs(&pid);
    // `by` and `evidence` both absent: this also covers the identity
    // fallback to the resolved session label, which is why the test keeps
    // the env guard.
    let err = crate::cmd_ratify::run(
        &tmp,
        &crate::cmd_ratify::RatifyArgs {
            key: Some("nope.key".to_string()),
            note: None,
            by: None,
            evidence: None,
            by_rule: None,
            dry_run: false,
            session: None,
        },
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("no active decision"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(edda_store::project_dir(&pid));
}
