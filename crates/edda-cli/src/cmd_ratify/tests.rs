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
        keys: Vec::new(),
        since: None,
        yes: false,
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

/// The four cases named in GH-761's original doneWhen, on one fixture
/// ledger. Case 3 used to be "a later decision's reason names it" — that
/// was the GH-1066 bug itself (see `mention_alone_does_not_hold_it_through_the_cli`
/// below for the proof it no longer holds), so this fixture now demonstrates
/// the domain guard that replaced it: `review.auto-merge` is ratified
/// *before* the sweep runs, so it is already binding when the sweep judges
/// `review.old-gate`.
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
    // 3. domain guard (GH-1066) → held: a later, binding ruling in the same
    // domain overtook it in substance, even though nothing names it.
    decide(
        &ledger,
        "review.old-gate",
        "manual",
        "per #900",
        &["issue:#900"],
    );
    decide(
        &ledger,
        "review.auto-merge",
        "mechanical",
        "the gate is now automatic",
        &["operator:2026-09-03"],
    );
    run(&tmp, &args(Some("review.auto-merge"))).unwrap();
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
        keys: Vec::new(),
        since: None,
        yes: false,
        session: None,
    }
}

#[test]
fn rule_ratifies_only_the_cited_unheld_decision() {
    let (tmp, ledger) = four_case_ledger();
    run(&tmp, &rule_args(false)).unwrap();

    assert!(is_binding(&ledger, "db.engine"), "cited → ratified");
    assert!(!is_binding(&ledger, "product.tier"), "product.* → held");
    assert!(
        !is_binding(&ledger, "review.old-gate"),
        "domain guard → held"
    );
    assert!(!is_binding(&ledger, "cache.ttl"), "no citation → held");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn dry_run_writes_nothing() {
    let (tmp, ledger) = four_case_ledger();
    let before = ratify_events(&ledger).len();
    run(&tmp, &rule_args(true)).unwrap();
    assert_eq!(
        ratify_events(&ledger).len(),
        before,
        "dry run must write nothing new"
    );
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
    let db_engine = events
        .iter()
        .find(|e| e.payload["key"] == "db.engine")
        .expect("the rule ratifies db.engine");
    assert_eq!(
        db_engine.payload["ratified_by"],
        format!("rule:{RULE_CITED_AUTHORITY}")
    );
    assert!(
        db_engine.payload["note"]
            .as_str()
            .is_some_and(|n| n.contains("issue:#742")),
        "the matched citation rides along: {:?}",
        db_engine.payload["note"]
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn a_second_sweep_is_a_no_op() {
    let (tmp, ledger) = four_case_ledger();
    let before = ratify_events(&ledger).len();
    run(&tmp, &rule_args(false)).unwrap();
    let after_first = ratify_events(&ledger).len();
    assert_eq!(after_first, before + 1, "sweep ratifies exactly db.engine");
    run(&tmp, &rule_args(false)).unwrap();
    assert_eq!(
        ratify_events(&ledger).len(),
        after_first,
        "second sweep must not append"
    );
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

#[test]
fn mention_alone_does_not_hold_it_through_the_cli() {
    // GH-1066, end to end through `run`: `review.pool` mentions
    // `review.engine` in passing while explaining itself. Before this fix
    // that alone held `review.engine` — the exact bug the issue reports at
    // scale (the operator's plain-flow rulings held, the mentions ratified).
    let (tmp, ledger) = setup_workspace();
    decide(
        &ledger,
        "review.engine",
        "opus",
        "per #900",
        &["issue:#900"],
    );
    decide(&ledger, "review.pool", "two", "replaces review.engine", &[]);
    run(&tmp, &rule_args(false)).unwrap();
    assert!(
        is_binding(&ledger, "review.engine"),
        "a mention must not hold a validly cited decision"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

// ── GH-1066 acceptance scenario ──────────────────────────────────────
//
// Reconstructs the shape of the real 2026-09-07 `--by-rule cited-authority
// --dry-run` sweep the issue reports (176 rows, 97 ratify / 79 hold): five
// real, plain-flow rulings the old rule held only because a later decision
// happened to name them while building on them, and four real 2026-08
// rail-closeout entries (`d-013`..`d-034`) that a blind sweep would have
// re-ratified because nothing named them, even though a later, binding
// ruling (`review.auto-merge`) had already overtaken the whole merge-
// authority area in substance. Keys, relative order, and citations are
// drawn from the real ledger (verified read-only against this project's own
// `.edda` while diagnosing GH-1066, before any code changed); reasons are
// paraphrased, not quoted verbatim.
//
// One thing this fixture does *not* claim: that the four `d-NNN.*` keys
// share a domain with `review.auto-merge`, or with each other. Each `d-NNN`
// key is its own historical identifier — a `d-032` decision and a `d-034`
// decision are never in the same domain by construction, the same way
// `d-032` is never in the same domain as `review.*`. The domain guard
// (proven separately above, and via `four_case_ledger`'s case 3) cannot
// mechanically connect them without inferring a relationship the ledger
// never recorded, which is the exact kind of inference GH-1066 removes. The
// `--since` date guard is what actually keeps this specific 2026-08 batch
// out of an unscoped sweep — proven below — and is the mechanism the issue
// titles "No domain OR date guard" (bullet 2): either guard protecting a
// given row is sufficient; this batch is protected by the date guard.
fn gh1066_snapshot_ledger() -> (std::path::PathBuf, edda_ledger::Ledger) {
    let (tmp, ledger) = setup_workspace();

    // 2026-08-14/15: a PR-rail closeout batch. Each key is its own
    // historical `d-NNN` identifier, never revisited under that exact key
    // except d-033 (below) — so each is alone in its own domain.
    decide(
        &ledger,
        "d-013.final_gate",
        "task22",
        "verifier gate per task #22",
        &[],
    );
    decide(
        &ledger,
        "d-032.fresh_premerge_gate",
        "pass",
        "integration rerun per PRs #459/#460/#461",
        &[],
    );
    decide(
        &ledger,
        "d-033.merge_authority",
        "required",
        "no merge without explicit delegation, per PR #459",
        &[],
    );
    decide(
        &ledger,
        "d-034.merge_authority",
        "granted",
        "operator authorized the merge order per PR #459",
        &[],
    );

    // 2026-09-02/03: fleet.lane-launch, then the ruling that supersedes the
    // whole 2026-08 merge-authority regime in substance, never by name.
    decide(
        &ledger,
        "fleet.lane-launch",
        "profile-a",
        "per the lane launcher design",
        &[],
    );
    decide(
        &ledger,
        "review.auto-merge",
        "gate-green-merges-by-machine",
        "Tim: merge authority moves to the gate",
        &["operator:2026-09-03"],
    );
    run(&tmp, &args(Some("review.auto-merge"))).unwrap();

    // 2026-09-06/07: the operator's plain-flow rulings, each cited by date,
    // and a later decision that only *builds on* them by mentioning them.
    decide(
        &ledger,
        "review.readonly-proof",
        "watcher-only",
        "Tim 2026-09-06",
        &["operator:2026-09-06"],
    );
    decide(
        &ledger,
        "review.merge-gate",
        "single-aggregate",
        "Tim 2026-09-07",
        &["operator:2026-09-07"],
    );
    decide(
        &ledger,
        "review.default-path",
        "watch-first",
        "Tim 2026-09-07",
        &["operator:2026-09-07"],
    );
    decide(
        &ledger,
        "review.watcher",
        "poll-interval",
        "Tim 2026-09-07",
        &["operator:2026-09-07"],
    );
    decide(
        &ledger,
        "review.shell-branch",
        "controller-owned",
        "builds on review.default-path and review.watcher",
        &[],
    );

    // The real stopgap the operator recorded once GH-1066 was diagnosed: a
    // same-key redecision of d-033, so the engine can see it without a fix.
    decide(
        &ledger,
        "d-033.merge_authority",
        "superseded-by-review.auto-merge",
        "housekeeping under review.auto-merge and review.merge-gate",
        &[],
    );

    (tmp, ledger)
}

#[test]
fn gh1066_plain_flow_rulings_ratify_despite_being_named_by_a_later_decision() {
    let (tmp, ledger) = gh1066_snapshot_ledger();
    let branch = ledger.head_branch().unwrap();
    let (candidates, binding) = collect(&ledger, &branch).unwrap();
    let verdicts = evaluate(&candidates, &binding);

    for key in [
        "review.readonly-proof",
        "review.merge-gate",
        "review.default-path",
        "review.watcher",
    ] {
        let v = verdicts.iter().find(|v| v.key == key).unwrap();
        assert!(v.is_ratify(), "{key}: {v:?}");
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn gh1066_same_key_stopgap_holds_the_original_d033_row() {
    let (tmp, ledger) = gh1066_snapshot_ledger();
    let branch = ledger.head_branch().unwrap();
    let (candidates, binding) = collect(&ledger, &branch).unwrap();
    let verdicts = evaluate(&candidates, &binding);

    let d033: Vec<_> = verdicts
        .iter()
        .filter(|v| v.key == "d-033.merge_authority")
        .collect();
    assert_eq!(d033.len(), 2, "two rows share this key: {d033:?}");
    let held = d033.iter().find(|v| !v.is_ratify()).unwrap();
    assert!(
        held.columns().1.starts_with("superseded-by-same-key"),
        "{held:?}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn gh1066_since_bound_keeps_the_rail_closeout_batch_out_of_the_sweep() {
    // The date guard (GH-1066): without a same-key or same-domain signal
    // available, `--since` is what an operator who knows this batch is
    // legacy actually uses to keep it out of an unscoped sweep.
    let (tmp, ledger) = gh1066_snapshot_ledger();
    let mut a = rule_args(false);
    a.since = Some("2026-08-20".to_string());
    run(&tmp, &a).unwrap();

    for key in [
        "d-013.final_gate",
        "d-032.fresh_premerge_gate",
        "d-034.merge_authority",
    ] {
        assert!(
            !is_binding(&ledger, key),
            "{key} must stay out of a --since-bound sweep"
        );
    }
    assert!(is_binding(&ledger, "review.default-path"));
    assert!(is_binding(&ledger, "review.watcher"));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn gh1066_without_since_the_rail_closeout_batch_would_have_ratified() {
    // The negative control for the test above: proves --since is load-
    // bearing here, not incidental — without it, these PR/task-number
    // "citations" (real quirk, out of GH-1066's scope) let d-034 through.
    let (tmp, ledger) = gh1066_snapshot_ledger();
    run(&tmp, &rule_args(false)).unwrap();
    assert!(is_binding(&ledger, "d-034.merge_authority"));
    let _ = std::fs::remove_dir_all(&tmp);
}

// ── sweep bounding and the unbounded-sweep threshold (GH-1066) ─────────

#[test]
fn unbounded_sweep_refuses_past_the_threshold_without_yes() {
    let (tmp, ledger) = setup_workspace();
    for i in 0..=RATIFY_THRESHOLD {
        decide(
            &ledger,
            &format!("k.item{i}"),
            "v",
            &format!("per #{i}"),
            &[],
        );
    }
    run(&tmp, &rule_args(false)).unwrap();
    assert!(
        ratify_events(&ledger).is_empty(),
        "must refuse and write nothing past the threshold"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn unbounded_sweep_proceeds_past_the_threshold_with_yes() {
    let (tmp, ledger) = setup_workspace();
    for i in 0..=RATIFY_THRESHOLD {
        decide(
            &ledger,
            &format!("k.item{i}"),
            "v",
            &format!("per #{i}"),
            &[],
        );
    }
    let mut a = rule_args(false);
    a.yes = true;
    run(&tmp, &a).unwrap();
    assert_eq!(ratify_events(&ledger).len(), RATIFY_THRESHOLD + 1);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn key_bound_sweep_ignores_the_threshold() {
    let (tmp, ledger) = setup_workspace();
    for i in 0..=RATIFY_THRESHOLD {
        decide(
            &ledger,
            &format!("k.item{i}"),
            "v",
            &format!("per #{i}"),
            &[],
        );
    }
    let mut a = rule_args(false);
    a.keys = vec!["k.item0".to_string()];
    run(&tmp, &a).unwrap();
    assert_eq!(ratify_events(&ledger).len(), 1);
    assert!(is_binding(&ledger, "k.item0"));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn well_formed_since_passes_through_unchanged() {
    // `usage_exit` on the malformed path calls `std::process::exit(2)`
    // directly (documented in `ratify_exit_codes.rs`) — not a panic, so it
    // cannot be exercised from an in-process unit test without aborting the
    // whole test binary. `ratify_exit_codes.rs::malformed_since_exits_2`
    // covers that path through a spawned binary instead.
    assert_eq!(validate_since("2026-08-20"), "2026-08-20");
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
            keys: Vec::new(),
            since: None,
            yes: false,
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
            keys: Vec::new(),
            since: None,
            yes: false,
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
