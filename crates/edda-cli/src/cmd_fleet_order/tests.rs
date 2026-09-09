//! GH-1015 fixture tests. Every input is a literal in this file: a test that
//! reaches `gh` or `git` cannot run on the frozen tree, which is where the
//! gate that consumes this ranking has to run.

use super::*;
use chrono::TimeZone;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 7, 12, 0, 0).unwrap()
}

/// A well-formed issue body: prose, a declared surface, a non-empty doneWhen.
fn body(surface: &[&str], extra: &str) -> String {
    let mut text = String::from("## What happened\n\nobserved today.\n\n## Predicted surface\n\n");
    for path in surface {
        text.push_str(&format!("`{path}`\n"));
    }
    text.push_str("\n## doneWhen\n\n- the thing is delivered.\n");
    text.push_str(extra);
    text
}

fn open_issue(number: u64, title: &str, body: &str, labels: &[&str]) -> OpenIssue {
    OpenIssue {
        number,
        title: title.to_string(),
        body: body.to_string(),
        labels: labels
            .iter()
            .map(|name| GhLabel {
                name: (*name).to_string(),
            })
            .collect(),
    }
}

fn tree() -> BTreeSet<String> {
    [
        "crates/edda-cli/src/main.rs",
        // A second `main.rs` so a bare name is genuinely ambiguous.
        "crates/edda-serve/src/main.rs",
        "crates/edda-cli/src/cmd_fleet.rs",
        "crates/edda-cli/src/cmd_export.rs",
        "crates/edda-ledger/src/sync.rs",
        "crates/edda-core/src/types.rs",
        "scripts/fleet/next-issue.sh",
        "scripts/fleet/guard-push.sh",
    ]
    .iter()
    .map(|path| (*path).to_string())
    .collect()
}

fn verbs() -> BTreeSet<String> {
    ["fleet", "ask", "decide", "claim"]
        .iter()
        .map(|verb| (*verb).to_string())
        .collect()
}

fn input(issues: &[OpenIssue], health: &str) -> OrderInput {
    OrderInput {
        issues: issues.to_vec(),
        flash_checks: BTreeMap::new(),
        tree_paths: tree(),
        known_verbs: verbs(),
        health_status: health.to_string(),
        flash_max_surface_files: FLASH_CAP_DEFAULT,
        flash_cap_source: "default".to_string(),
        now: now(),
    }
}

/// Both subprocess criteria resolved for one issue. `cli` produces this shape
/// by running the scripts; a fixture states it, which is the point of moving
/// the checks onto [`OrderInput`].
fn checks(number: u64, render: bool, dry_run: bool) -> BTreeMap<u64, Vec<FlashCheck>> {
    let mut results = vec![FlashCheck {
        name: CHECK_BRIEF_RENDER.to_string(),
        passed: render,
        note: if render {
            "brief-from-issue.sh exited 0".to_string()
        } else {
            "brief-from-issue.sh exited 2: no ## Predicted surface".to_string()
        },
    }];
    // The real collector short-circuits: a brief that did not render leaves
    // the validator nothing to run, so no second result exists.
    if render {
        results.push(FlashCheck {
            name: CHECK_DISPATCH_DRY_RUN.to_string(),
            passed: dry_run,
            note: if dry_run {
                "brief-validate.sh VALID".to_string()
            } else {
                "brief-validate.sh exited 1: INVALID step=3".to_string()
            },
        });
    }
    BTreeMap::from([(number, results)])
}

fn row(queue: &Queue, number: u64) -> &Row {
    queue
        .rows
        .iter()
        .find(|row| row.number == number)
        .unwrap_or_else(|| panic!("no row for #{number} in queue of {}", queue.rows.len()))
}

fn freshness(body: &str) -> Vec<FreshnessFinding> {
    evaluate_freshness(body, &tree(), &verbs())
}

fn verdict_for(body: &str, target: &str) -> Option<Verdict> {
    freshness(body)
        .into_iter()
        .find(|finding| finding.target == target)
        .map(|finding| finding.verdict)
}

// ---------------------------------------------------------------------------
// The two properties #1015 names explicitly
// ---------------------------------------------------------------------------

/// The measured #671/#685 shape: two labels in common (`enhancement`,
/// `lane:feature`) and a real overlap on `crates/edda-ledger/src/sync.rs`.
/// Label-based detection reads the shared labels and still cannot answer the
/// question — labels are issue-level, conflicts are file-level — which is how
/// two lanes built the same file (#887's shape). The per-file rule (#1005)
/// sees one collision and serializes them.
#[test]
fn per_file_collision_holds_the_lower_ranked_peer() {
    let issues = vec![
        open_issue(
            671,
            "committed mirror",
            &body(
                &[
                    "crates/edda-ledger/src/sync.rs",
                    "crates/edda-cli/src/cmd_export.rs",
                ],
                "",
            ),
            &["enhancement", "lane:feature", "fleet:ready", "P1"],
        ),
        open_issue(
            685,
            "sync surface",
            &body(
                &[
                    "crates/edda-ledger/src/sync.rs",
                    "crates/edda-cli/src/cmd_fleet.rs",
                ],
                "",
            ),
            &["enhancement", "lane:feature", "fleet:ready", "P2"],
        ),
        open_issue(
            999,
            "unrelated core change",
            &body(&["crates/edda-core/src/types.rs"], ""),
            &["fleet:ready", "P2"],
        ),
    ];
    let queue = compute_order(&input(&issues, "GREEN"));

    assert_eq!(row(&queue, 671).collides_with, vec![685]);
    assert_eq!(row(&queue, 685).collides_with, vec![671]);
    assert!(row(&queue, 999).collides_with.is_empty());
    assert_eq!(row(&queue, 671).score.collision, W_COLLISION_PEER);
    assert_eq!(row(&queue, 999).score.collision, 0);

    assert!(row(&queue, 671).rank < row(&queue, 685).rank);
    assert_eq!(row(&queue, 671).status, Status::Ready);
    assert_eq!(row(&queue, 685).status, Status::Hold);
    assert_eq!(
        row(&queue, 685).hold_reason.as_deref(),
        Some("surface collision with #671 on crates/edda-ledger/src/sync.rs")
    );
    assert_eq!(row(&queue, 999).status, Status::Ready);
    assert!(row(&queue, 999).hold_reason.is_none());
}

/// RED health zeroes mechanism weight. The only escape is a body citing a red
/// run or gate link — without one the issue is not a pipeline blocker.
#[test]
fn red_health_zeroes_mechanism_weight_unless_a_red_run_is_cited() {
    let issues = vec![
        open_issue(
            997,
            "next-issue.sh gate flake",
            &body(&["scripts/fleet/next-issue.sh"], ""),
            &["fleet:ready", "P1"],
        ),
        open_issue(
            998,
            "push guard blocks every lane",
            &body(
                &["scripts/fleet/guard-push.sh"],
                "\nred gate: https://github.com/fagemx/edda/actions/runs/123456789\n",
            ),
            &["fleet:ready", "P1"],
        ),
        open_issue(
            1015,
            "fleet order",
            &body(&["crates/edda-cli/src/cmd_fleet_order.rs"], ""),
            &["fleet:ready", "P1"],
        ),
    ];

    let red = compute_order(&input(&issues, "RED"));
    assert_eq!(red.mechanism_dispatch, "freeze");
    assert_eq!(row(&red, 997).score.total, 0);
    assert_eq!(row(&red, 997).score.frozen, -40);
    assert_eq!(row(&red, 997).status, Status::Frozen);
    assert_eq!(row(&red, 998).score.blocker, W_RED_RUN_BLOCKER);
    assert_eq!(row(&red, 998).score.frozen, 0);
    assert!(row(&red, 998).score.total > 0);
    assert_eq!(row(&red, 998).status, Status::Ready);
    assert!(row(&red, 998).rank < row(&red, 997).rank);

    let green = compute_order(&input(&issues, "GREEN"));
    assert_eq!(green.mechanism_dispatch, "open");
    assert_eq!(row(&green, 997).score.frozen, 0);
    assert_eq!(row(&green, 997).status, Status::Ready);
    // The blocker bonus exists only as the freeze exception.
    assert_eq!(row(&green, 998).score.blocker, 0);
    assert!(row(&green, 1015).rank < row(&green, 997).rank);
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

#[test]
fn identical_input_yields_identical_output_whatever_the_input_order() {
    let a = open_issue(
        10,
        "a",
        &body(&["crates/edda-core/src/types.rs"], ""),
        &["fleet:ready", "P1"],
    );
    let b = open_issue(
        20,
        "b",
        &body(&["crates/edda-cli/src/cmd_fleet.rs"], ""),
        &["fleet:ready", "P1"],
    );
    let c = open_issue(30, "c", &body(&["docs/reference/cli.md"], ""), &["P1"]);

    let forward = compute_order(&input(&[a.clone(), b.clone(), c.clone()], "GREEN"));
    let reversed = compute_order(&input(&[c, b, a], "GREEN"));

    assert_eq!(
        serde_json::to_string(&forward).expect("serialize"),
        serde_json::to_string(&reversed).expect("serialize")
    );
    assert_eq!(render_markdown(&forward), render_markdown(&reversed));
    assert_eq!(render_text(&forward), render_text(&reversed));
}

#[test]
fn equal_scores_fall_back_to_the_issue_number() {
    let surface = ["crates/edda-core/src/types.rs"];
    let issues = vec![
        open_issue(300, "third", &body(&surface, ""), &["fleet:ready", "P1"]),
        open_issue(100, "first", &body(&surface, ""), &["fleet:ready", "P1"]),
        open_issue(200, "second", &body(&surface, ""), &["fleet:ready", "P1"]),
    ];
    let queue = compute_order(&input(&issues, "GREEN"));
    let numbers: Vec<u64> = queue.rows.iter().map(|row| row.number).collect();
    assert_eq!(numbers, vec![100, 200, 300]);
    // All three declare the same file, so only the head is dispatchable.
    assert_eq!(row(&queue, 100).status, Status::Ready);
    assert_eq!(row(&queue, 200).status, Status::Hold);
    assert_eq!(row(&queue, 300).status, Status::Hold);
}

#[test]
fn every_score_decomposes_into_its_components() {
    let issues = vec![
        open_issue(
            1,
            "product",
            &body(&["crates/edda-core/src/types.rs"], ""),
            &["fleet:ready", "P0"],
        ),
        open_issue(
            2,
            "mechanism",
            &body(&["scripts/fleet/next-issue.sh"], ""),
            &["fleet:ready"],
        ),
        open_issue(
            3,
            "stale",
            "## What happened\n\n`a/b/gone.rs` is used.\n",
            &[],
        ),
    ];
    for status in ["GREEN", "RED"] {
        for row in compute_order(&input(&issues, status)).rows {
            let score = row.score;
            assert_eq!(
                score.class
                    + score.readiness
                    + score.priority
                    + score.blocker
                    + score.collision
                    + score.freshness
                    + score.frozen,
                score.total,
                "row #{} under {status} does not sum",
                row.number
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Freshness (#970), evaluated against the pinned tree
// ---------------------------------------------------------------------------

/// A path the issue declares it will create WARNs; a genuinely deleted path
/// still FAILs, and sinks the issue out of the dispatchable queue.
#[test]
fn a_declared_path_warns_and_a_deleted_path_still_fails() {
    let body = "## What happened\n\
                the check reads `crates/edda-cli/src/deleted.rs` today.\n\
                the fix lands in `crates/edda-cli/src/cmd_fleet_order.rs`.\n\
                it also reads `crates/edda-cli/src/main.rs`.\n\
                \n\
                ## Predicted surface\n\
                \n\
                `crates/edda-cli/src/cmd_fleet_order.rs`\n\
                \n\
                ## doneWhen\n\
                \n\
                - delivered.\n";
    assert_eq!(
        verdict_for(body, "crates/edda-cli/src/deleted.rs"),
        Some(Verdict::Fail)
    );
    assert_eq!(
        verdict_for(body, "crates/edda-cli/src/cmd_fleet_order.rs"),
        Some(Verdict::Warn)
    );
    // A path that exists at the pinned tree produces no finding at all.
    assert_eq!(verdict_for(body, "crates/edda-cli/src/main.rs"), None);

    let queue = compute_order(&input(
        &[open_issue(1, "stale one", body, &["fleet:ready", "P0"])],
        "GREEN",
    ));
    assert_eq!(row(&queue, 1).status, Status::Stale);
    assert_eq!(row(&queue, 1).score.freshness, W_STALE);
}

/// A negated mention is a statement of absence, not a reference.
#[test]
fn a_negated_mention_is_not_a_reference() {
    let body = "## What happened\n\
                there is no `crates/edda-cli/src/deleted.rs` anywhere.\n\
                \n\
                ## doneWhen\n\
                \n\
                - delivered.\n";
    assert!(freshness(body).is_empty());
}

#[test]
fn donewhen_headings_match_case_insensitively() {
    for heading in ["## doneWhen", "## DoneWhen", "## DONEWHEN", "## donewhen"] {
        let body = format!("## What happened\n\nprose.\n\n{heading}\n\n- delivered.\n");
        assert!(
            freshness(&body).is_empty(),
            "{heading} should satisfy the doneWhen check"
        );
    }
    let missing = "## What happened\n\nprose.\n";
    assert_eq!(verdict_for(missing, "## doneWhen"), Some(Verdict::Fail));
    let empty = "## What happened\n\nprose.\n\n## doneWhen\n\n";
    assert_eq!(verdict_for(empty, "## doneWhen"), Some(Verdict::Fail));
}

/// GH-1056: the case axis shipped in #1040 does not cover the separating
/// space in `Done when` — `"done when".starts_with("donewhen")` is `false`.
/// #953 and #970's own corpus spell the heading this way, so a fresh issue
/// written `## Done when` was reported stale. Same fixture shape as
/// `donewhen_headings_match_case_insensitively`, one axis over: space,
/// double space, and hyphen/underscore spellings.
#[test]
fn donewhen_headings_match_the_spacing_axis() {
    for heading in [
        "## Done when",
        "## done  when",
        "## done-when",
        "## Done_When",
    ] {
        let body = format!("## What happened\n\nprose.\n\n{heading}\n\n- delivered.\n");
        assert!(
            freshness(&body).is_empty(),
            "{heading} should satisfy the doneWhen check"
        );
    }
    let empty = "## What happened\n\nprose.\n\n## Done when\n\n";
    assert_eq!(verdict_for(empty, "## doneWhen"), Some(Verdict::Fail));
}

/// A command the issue is about to build, or one cited with evidence that it
/// ran, must not FAIL the gate; an unqualified reference to a verb that does
/// not exist still does.
#[test]
fn a_to_be_built_or_evidence_cited_command_warns_instead_of_failing() {
    let declared = "## What happened\n\nprose.\n\n## doneWhen\n\n- `edda garden plant` ranks it.\n";
    assert_eq!(
        verdict_for(declared, "edda garden plant"),
        Some(Verdict::Warn)
    );

    let cited = "## What happened\n\n\
                 `edda garden plant` failed at https://example.invalid/run/1.\n\
                 \n## doneWhen\n\n- delivered.\n";
    assert_eq!(verdict_for(cited, "edda garden plant"), Some(Verdict::Warn));

    let unqualified = "## What happened\n\n\
                       the controller runs `edda garden plant` daily.\n\
                       \n## doneWhen\n\n- delivered.\n";
    assert_eq!(
        verdict_for(unqualified, "edda garden plant"),
        Some(Verdict::Fail)
    );
}

/// The measured #1015 false FAIL: `edda fleet --help` exited nonzero only
/// because the `edda` on PATH predated the verb. Resolving against the pinned
/// tree's verb table cannot reproduce it.
#[test]
fn a_stale_binary_on_path_cannot_produce_a_false_fail() {
    let body = "## What happened\n\n\
                the freshness gate ran `edda fleet --help` and it exited nonzero.\n\
                \n## doneWhen\n\n- delivered.\n";
    // `fleet` is in the pinned tree's verb table, so there is nothing to report.
    assert!(freshness(body).is_empty());
    // And the same body against a tree that genuinely lacks the verb does FAIL.
    let older: BTreeSet<String> = ["ask"].iter().map(|v| (*v).to_string()).collect();
    let findings = evaluate_freshness(body, &tree(), &older);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].verdict, Verdict::Fail);
    assert_eq!(findings[0].target, "edda fleet --help");
}

#[test]
fn unresolvable_tokens_warn_without_blocking() {
    let body = "## What happened\n\n\
                see `main.rs` and `if/else` and `crates/edda-cli/src/../evil.rs`,\n\
                beside `types.rs` which resolves, the ledger key\n\
                `fleet.mechanism-layer` and the field `event_data.error`.\n\
                \n## doneWhen\n\n- delivered.\n";
    let findings = freshness(body);
    // `main.rs` is a bare name matching more than one tracked file, `if/else`
    // has no file-shaped last segment, `../` is not a repository path.
    assert!(findings.iter().all(|f| f.verdict == Verdict::Warn));
    assert!(findings.iter().any(|f| f.target == "main.rs"));
    assert!(findings
        .iter()
        .any(|f| f.target == "crates/edda-cli/src/../evil.rs"));
    assert!(!findings.iter().any(|f| f.target == "if/else"));
    // A bare name resolving to exactly one tracked file is silent.
    assert_eq!(verdict_for(body, "types.rs"), None);
    // So is a dotted token that resolves to nothing: it is not a path, and
    // reporting every ledger key would bury the findings that mean something.
    assert_eq!(verdict_for(body, "fleet.mechanism-layer"), None);
    assert_eq!(verdict_for(body, "event_data.error"), None);
}

/// Issues cite crate- and module-relative paths. Those are references, not
/// deleted files.
#[test]
fn a_relative_reference_resolves_against_the_tree() {
    let body = "## What happened\n\n\
                `edda-cli/src/main.rs` and `src/main.rs` and `edda-cli/src/gone.rs`.\n\
                \n## doneWhen\n\n- delivered.\n";
    // Suffix of exactly one tracked path.
    assert_eq!(verdict_for(body, "edda-cli/src/main.rs"), None);
    // Suffix of two tracked paths: a reference, but an imprecise one.
    assert_eq!(verdict_for(body, "src/main.rs"), Some(Verdict::Warn));
    // Suffix of none: genuinely absent.
    assert_eq!(
        verdict_for(body, "edda-cli/src/gone.rs"),
        Some(Verdict::Fail)
    );
}

/// The observation sections make claims about today's tree and can be stale;
/// the sections describing what the issue will build cannot be. #763 names the
/// module it will create under `## 改哪裡` and #761 names its own under
/// `## Suspected surface`; the shell gate FAILed both for not existing yet.
#[test]
fn a_future_tense_section_declares_rather_than_references() {
    let observed = "## What happened\n\n\
                    the gate reads `crates/edda-cli/src/cmd_review.rs` today.\n\
                    \n## doneWhen\n\n- delivered.\n";
    assert_eq!(
        verdict_for(observed, "crates/edda-cli/src/cmd_review.rs"),
        Some(Verdict::Fail)
    );

    let declared = |heading: &str| {
        let body = format!(
            "## What happened\n\nprose.\n\n{heading}\n\n\
             - `crates/edda-cli/src/cmd_review.rs` gains the subcommand.\n\
             \n## doneWhen\n\n- delivered.\n"
        );
        verdict_for(&body, "crates/edda-cli/src/cmd_review.rs")
    };
    // Prose sections that describe the delivery WARN.
    assert_eq!(declared("## 改哪裡"), Some(Verdict::Warn));
    assert_eq!(declared("## doneWhen"), Some(Verdict::Warn));
    // A surface section is pure declaration and is not scanned at all.
    assert_eq!(declared("## Suspected surface"), None);
    assert_eq!(declared("## Predicted surface"), None);
}

/// Only `[a-z][a-z0-9-]*` is a verb; the rest are not command references.
#[test]
fn a_non_verb_after_edda_is_not_a_command_reference() {
    let body = "## What happened\n\n\
                `edda --version` printed it, and the digest showed `edda d… |`.\n\
                \n## doneWhen\n\n- delivered.\n";
    assert!(freshness(body).is_empty());
}

/// Issues cite line ranges. The tree can only answer for the file.
#[test]
fn a_cited_line_range_resolves_to_its_file() {
    let body = "## What happened\n\n\
                `crates/edda-cli/src/main.rs:12-30` and `crates/edda-cli/src/cmd_fleet.rs:7`\n\
                are fine; `crates/edda-cli/src/gone.rs:9` is not.\n\
                \n## doneWhen\n\n- delivered.\n";
    assert_eq!(verdict_for(body, "crates/edda-cli/src/main.rs:12-30"), None);
    assert_eq!(
        verdict_for(body, "crates/edda-cli/src/cmd_fleet.rs:7"),
        None
    );
    assert_eq!(
        verdict_for(body, "crates/edda-cli/src/gone.rs:9"),
        Some(Verdict::Fail)
    );
}

// ---------------------------------------------------------------------------
// Lane routing
// ---------------------------------------------------------------------------

#[test]
fn lane_routing_is_mechanical() {
    let issues = vec![
        open_issue(
            1,
            "small product change",
            &body(&["crates/edda-core/src/types.rs"], ""),
            &["fleet:ready"],
        ),
        open_issue(
            2,
            "wide surface",
            &body(
                &[
                    "crates/a/src/one.rs",
                    "crates/a/src/two.rs",
                    "crates/a/src/three.rs",
                    "crates/a/src/four.rs",
                ],
                "",
            ),
            &["fleet:ready"],
        ),
        open_issue(
            3,
            "shell surface",
            &body(&["scripts/fleet/next-issue.sh"], ""),
            &["fleet:ready"],
        ),
        open_issue(4, "no surface", "## doneWhen\n\n- delivered.\n", &[]),
        open_issue(
            5,
            "governance",
            &body(&["crates/edda-core/src/types.rs"], ""),
            &["governance"],
        ),
        open_issue(
            6,
            "needs judgment",
            &body(&["crates/edda-core/src/types.rs"], "\n[判斷] wording.\n"),
            &["fleet:ready"],
        ),
    ];
    let mut order_input = input(&issues, "GREEN");
    order_input.flash_checks = checks(1, true, true);
    let queue = compute_order(&order_input);

    assert_eq!(row(&queue, 1).lane, Lane::Flash);
    assert_eq!(
        row(&queue, 1)
            .flash_checks
            .iter()
            .map(|check| check.name.as_str())
            .collect::<Vec<_>>(),
        vec![CHECK_BRIEF_RENDER, CHECK_DISPATCH_DRY_RUN]
    );
    assert_eq!(row(&queue, 2).lane, Lane::Strong);
    assert!(row(&queue, 2).lane_reasons[0].contains("> flash cap 3"));
    // A row the cheap criteria already routed away never reaches the scripts.
    assert!(row(&queue, 2).flash_checks.is_empty());
    assert_eq!(row(&queue, 3).lane, Lane::Strong);
    assert!(row(&queue, 3).lane_reasons[0].contains("scripts/"));
    assert_eq!(row(&queue, 4).lane, Lane::Strong);
    assert_eq!(row(&queue, 4).lane_reasons, vec!["no declared surface"]);
    assert_eq!(row(&queue, 5).lane, Lane::Controller);
    assert_eq!(row(&queue, 6).lane, Lane::Controller);
    assert!(row(&queue, 6).lane_reasons[0].contains(JUDGMENT_MARKER));
}

/// #1015 doneWhen bullet 4: `任一不過 → strong`. The row below satisfies both
/// cheap criteria — one product file, no `scripts/` path — so the only thing
/// that can keep it off the flash lane is a subprocess criterion, and each of
/// the three ways one can come back short has to do exactly that.
#[test]
fn a_failed_subprocess_criterion_forces_strong() {
    let issues = vec![open_issue(
        1,
        "small product change",
        &body(&["crates/edda-core/src/types.rs"], ""),
        &["fleet:ready"],
    )];

    // Baseline: with both criteria satisfied the same row is flash, so the
    // assertions below isolate the criterion and nothing else.
    let mut both_pass = input(&issues, "GREEN");
    both_pass.flash_checks = checks(1, true, true);
    assert_eq!(row(&compute_order(&both_pass), 1).lane, Lane::Flash);

    // The #945 dry-run validator reported INVALID.
    let mut dry_run_failed = input(&issues, "GREEN");
    dry_run_failed.flash_checks = checks(1, true, false);
    let queue = compute_order(&dry_run_failed);
    assert_eq!(row(&queue, 1).lane, Lane::Strong);
    assert!(row(&queue, 1).lane_reasons[0].contains("<= flash cap 3"));
    assert_eq!(
        row(&queue, 1).lane_reasons[2],
        "dispatch-dry-run failed: brief-validate.sh exited 1: INVALID step=3"
    );

    // The brief did not render, so the validator was never reached: the row
    // carries one check, not two, and the render is what it names.
    let mut render_failed = input(&issues, "GREEN");
    render_failed.flash_checks = checks(1, false, true);
    let queue = compute_order(&render_failed);
    assert_eq!(row(&queue, 1).lane, Lane::Strong);
    assert_eq!(row(&queue, 1).flash_checks.len(), 1);
    assert_eq!(
        row(&queue, 1).lane_reasons[1],
        "brief-render failed: brief-from-issue.sh exited 2: no ## Predicted surface"
    );

    // Nothing ran at all (a `--issues` fixture run). Not evaluated is not a
    // pass, so the safe lane wins and the row names the check that decided it.
    let queue = compute_order(&input(&issues, "GREEN"));
    assert_eq!(row(&queue, 1).lane, Lane::Strong);
    assert!(row(&queue, 1).flash_checks.is_empty());
    assert_eq!(row(&queue, 1).lane_reasons[1], "brief-render not evaluated");
}

#[test]
fn an_in_flight_issue_is_reported_claimed_and_claims_no_path() {
    let surface = ["crates/edda-core/src/types.rs"];
    let issues = vec![
        open_issue(
            1,
            "in flight",
            &body(&surface, ""),
            &["fleet:ready", "fleet:claimed", "P0"],
        ),
        open_issue(2, "waiting", &body(&surface, ""), &["fleet:ready", "P2"]),
    ];
    let queue = compute_order(&input(&issues, "GREEN"));
    assert_eq!(row(&queue, 1).status, Status::Claimed);
    // #1 holds no path, so #2 is not blocked behind a row nobody can dispatch.
    assert_eq!(row(&queue, 2).status, Status::Ready);
}

#[test]
fn the_queue_header_reports_its_inputs() {
    let queue = compute_order(&input(
        &[open_issue(1, "one", &body(&["crates/a/src/x.rs"], ""), &[])],
        "yellow",
    ));
    assert_eq!(queue.generated_at, "2026-09-07T12:00:00Z");
    assert_eq!(queue.health_status, "YELLOW");
    assert_eq!(queue.mechanism_dispatch, "open");
    assert_eq!(queue.flash_max_surface_files, FLASH_CAP_DEFAULT);
    assert_eq!(queue.flash_cap_source, "default");
    assert_eq!(queue.total_issues, 1);
}
