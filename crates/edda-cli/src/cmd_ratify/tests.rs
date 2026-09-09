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
    decide_at(ledger, key, value, reason, cites, None)
}

/// Same as `decide`, but stamped with `ts` (RFC3339) instead of "now" when
/// given. Every plain `decide()` call in one test run lands within the same
/// wall-clock second, so a fixture that needs `--since` to see genuinely
/// distinct dates (GH-1066) must override `ts` explicitly — a narrative
/// comment claiming a row is "2026-08-14" does not make the ledger record
/// that date.
fn decide_at(
    ledger: &edda_ledger::Ledger,
    key: &str,
    value: &str,
    reason: &str,
    cites: &[&str],
    ts: Option<&str>,
) {
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
    let mut event =
        edda_core::event::new_decision_event(&branch, parent_hash.as_deref(), "agent", &dp)
            .unwrap();
    if let Some(t) = ts {
        // The hash covers `ts`, so a caller-supplied date must be finalized
        // again rather than poked in after the fact.
        event.ts = t.to_string();
        edda_core::event::finalize_event(&mut event).unwrap();
    }
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
// happened to name them while building on or referencing them, and four
// real 2026-08 rail-closeout entries (`d-013`..`d-034`) that an *unbounded*
// sweep still ratifies today. Keys, relative order, and citations are drawn
// from the real ledger (verified read-only against this project's own
// `.edda` while diagnosing GH-1066, before any code changed); reasons are
// paraphrased, not quoted verbatim — and two of the five plain-flow reasons
// below are edited from that paraphrase to *name* the key they are built to
// trip (see the per-decision comments): a paraphrase that happened to drop
// the name would prove nothing about the bug this fixture exists to catch.
//
// Controller amendment to GH-1066 doneWhen bullet 5 (PR #1098 Round 1,
// 2026-09-09 — see the issue's newest comment): the original bullet also
// asked the four `d-NNN.*` rows to become `hold: older-than-binding-in-
// domain`. That is mechanically impossible and was withdrawn —
// `extract_domain` is `key.split('.').next()`, so `d-033` and `review`
// share no domain, and nothing in this fixture claims otherwise.
// `review.auto-merge`'s ratification does *not* domain-guard the four rows
// below; its role here is limited to (a) being the later, binding decision
// this file's own domain-guard shape is modeled on (mirrored, on a
// genuinely shared domain, by the `fleet.`-domain pair in `rule.rs`), and
// (b) naming `fleet.lane-launch` in its own reason — this fixture's
// mention-inference trip for that key.
//
// The amended bullet is outcome-oriented for the four rows instead: an
// unbounded sweep must not silently ratify them.
// `gh1066_without_since_the_rail_closeout_batch_would_have_ratified` below
// proves the residual honestly — they *do* ratify unbounded, because each
// row's own reason happens to carry a PR/task number the reason-fallback
// citation reads (a real quirk, out of GH-1066's scope) — and
// `gh1066_since_bound_keeps_the_rail_closeout_batch_out_of_the_sweep` proves
// `--since` is what keeps them out. `unbounded_sweep_refuses_past_the_
// threshold_without_yes`, elsewhere in this file, is the other accepted
// half: the real table's 97 ratifies exceed the default cap of 20, so a
// truly unscoped sweep refuses and prints the table rather than writing
// silently, even without `--since`. That guard only works if the four rows
// genuinely carry an old date, so they are stamped via `decide_at` rather
// than `decide` (which would record "now").
//
// The domain guard itself is proven on keys that genuinely share a domain —
// not on this fixture's cross-domain `d-NNN`/`review` pairing — by four
// unit tests in `rule.rs`: `domain_guard_holds_older_candidate_behind_a_
// later_binding_sibling` / `domain_guard_requires_the_sibling_to_be_
// binding_not_merely_newer` (domain `review`), and their fleet-domain
// counterparts `domain_guard_holds_older_fleet_candidate_behind_a_later_
// binding_fleet_sibling` / `domain_guard_requires_the_fleet_sibling_to_be_
// binding_not_merely_newer`.
fn gh1066_snapshot_ledger() -> (std::path::PathBuf, edda_ledger::Ledger) {
    let (tmp, ledger) = setup_workspace();

    // 2026-08-14/15: a PR-rail closeout batch. Each key is its own
    // historical `d-NNN` identifier, never revisited under that exact key —
    // so each is alone in its own domain.
    decide_at(
        &ledger,
        "d-013.final_gate",
        "task22",
        "verifier gate per task #22",
        &[],
        Some("2026-08-14T09:00:00Z"),
    );
    decide_at(
        &ledger,
        "d-032.fresh_premerge_gate",
        "pass",
        "integration rerun per PRs #459/#460/#461",
        &[],
        Some("2026-08-14T10:00:00Z"),
    );
    decide_at(
        &ledger,
        "d-033.merge_authority",
        "required",
        "no merge without explicit delegation, per PR #459",
        &[],
        Some("2026-08-14T11:00:00Z"),
    );
    decide_at(
        &ledger,
        "d-034.merge_authority",
        "granted",
        "operator authorized the merge order per PR #459",
        &[],
        Some("2026-08-15T09:00:00Z"),
    );

    // 2026-09-02/03: fleet.lane-launch, then the ruling that follows it.
    // `fleet.lane-launch` carries a real citation, so under the fixed
    // engine it ratifies on its own citation once mention alone can no
    // longer hold it. Before this fix (Round 1 P0-2) it carried no
    // citation at all, held as `no-citation` regardless of the bug, and
    // was silently left out of the acceptance assertion below.
    decide(
        &ledger,
        "fleet.lane-launch",
        "profile-a",
        "per the lane launcher design",
        &["operator:2026-09-02"],
    );
    // Reason names `fleet.lane-launch` (paraphrased, not the real wording)
    // — this fixture's mention-inference trip for that key: the pre-
    // GH-1066 rule held anything a later reason merely named, so without
    // this mention the fifth key would prove nothing about the bug. This
    // ratification does not, and is not claimed to, domain-guard the four
    // `d-NNN.*` rows above — see the file-level comment.
    decide(
        &ledger,
        "review.auto-merge",
        "gate-green-merges-by-machine",
        "Tim: merge authority moves to the gate, keeping fleet.lane-launch's rollout order",
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
    // Reason now names all four plain-flow keys above (paraphrased): the
    // fixture must trip the old mention-inference bug for all four, not
    // only two, or two of the four assertions below pass identically on
    // the unfixed engine (Round 1 P0-2).
    decide(
        &ledger,
        "review.shell-branch",
        "controller-owned",
        "builds on review.readonly-proof, review.merge-gate, review.default-path and review.watcher",
        &[],
    );

    (tmp, ledger)
}

#[test]
fn gh1066_plain_flow_rulings_ratify_despite_being_named_by_a_later_decision() {
    // All five held keys the issue reports (bullet 5, controller-amended
    // 2026-09-09): each is named by a later decision's reason above, which
    // the pre-GH-1066 rule alone would have held (see PR #1098 Round 2's
    // Review Response for the scratch-copy proof: reverting just that
    // inference turns every assertion below red). Asserting all five, not
    // a subset, is the Round 1 P0-2 fix — two of the previous four
    // assertions passed identically on the unfixed engine because nothing
    // actually named them, and the fifth key was silently omitted from
    // both the fixture's citation and this test.
    let (tmp, ledger) = gh1066_snapshot_ledger();
    let branch = ledger.head_branch().unwrap();
    let (candidates, binding) = collect(&ledger, &branch).unwrap();
    let verdicts = evaluate(&candidates, &binding);

    for key in [
        "review.readonly-proof",
        "review.merge-gate",
        "review.default-path",
        "review.watcher",
        "fleet.lane-launch",
    ] {
        let v = verdicts.iter().find(|v| v.key == key).unwrap();
        assert!(v.is_ratify(), "{key}: {v:?}");
    }
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
        "d-033.merge_authority",
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
    // "citations" (real quirk, out of GH-1066's scope) let the whole 2026-08
    // batch through, d-033 included.
    let (tmp, ledger) = gh1066_snapshot_ledger();
    run(&tmp, &rule_args(false)).unwrap();
    assert!(is_binding(&ledger, "d-033.merge_authority"));
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

#[test]
fn since_calendar_validity_rejects_impossible_dates() {
    // PR #1098 Round 1 P2: the old shape-only check accepted `2026-99-99`.
    // The exit-2 boundary itself is covered end to end by
    // ratify_exit_codes.rs::since_with_an_impossible_calendar_date_exits_2
    // (same reason `well_formed_since_passes_through_unchanged` above
    // cannot exercise `usage_exit` in-process); this checks the pure
    // predicate that boundary now calls.
    assert!(!is_valid_since_date("2026-99-99"));
    assert!(!is_valid_since_date("2026-02-30")); // February never has 30 days
    assert!(!is_valid_since_date("2026-04-31")); // April has 30 days
    assert!(!is_valid_since_date("2026-00-10")); // month 0
    assert!(is_valid_since_date("2026-02-28"));
    assert!(is_valid_since_date("2024-02-29")); // 2024 is a leap year
    assert!(!is_valid_since_date("2026-02-29")); // 2026 is not
    assert!(!is_valid_since_date("1900-02-29")); // divisible by 100, not 400
    assert!(is_valid_since_date("2000-02-29")); // divisible by 400
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
