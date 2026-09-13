use super::*;
use std::cell::RefCell;

const HEAD: &str = "aaaaaaaabbbbbbbbccccccccdddddddd11112222";
const BASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
/// A SHA that is not the head — the side of the window where the
/// latest-review ordering actually decides the outcome.
const OLDER: &str = "1111111111111111111111111111111111111111";

fn args(merge: bool) -> MergeArgs {
    MergeArgs {
        pr: 4242,
        merge,
        check: false,
        body_file: None,
        json: false,
    }
}

fn review(round: u64, sha: &str, verdict: &str, updated: &str) -> TimedComment {
    TimedComment {
        comment: Comment {
            id: format!("{round:03}"),
            body: format!(
                "## Code Review: Round {round} — PR #4242 @ {sha}\n\n- escalations: \
                 none\n\n### Verdict\n\n{verdict}\n"
            ),
            author_association: Some("OWNER".into()),
        },
        updated_at: updated.into(),
    }
}

/// The fixture half of test-merge-reviewed-pr.sh: every read pre-seeded,
/// every call recorded, so a case can prove what was and was not
/// reached — a stub that shrugs at an unexpected call turns a wiring
/// regression into a green test.
struct Fake {
    head: &'static str,
    base: &'static str,
    state: &'static str,
    title: String,
    mergeable: &'static str,
    comments: Vec<TimedComment>,
    drift: Result<drift::Report, String>,
    fail_comments: bool,
    required_checks: RequiredChecks,
    merged: RefCell<Vec<(u64, String, String, String)>>,
    /// Which PR each read was asked for — a `Fake` that shrugs at the
    /// argument is how the missing `--pr` selector stayed invisible to
    /// all 17 tests before it.
    asked: RefCell<Vec<(&'static str, u64)>>,
    reached_comments: RefCell<bool>,
    reached_checks: RefCell<bool>,
}

impl Fake {
    fn clean(comments: Vec<TimedComment>) -> Self {
        Self {
            head: HEAD,
            base: BASE,
            state: "OPEN",
            title: "fix(edda-cli): a merge the fleet already reviewed".into(),
            mergeable: "MERGEABLE",
            comments,
            drift: Ok(drift::Report::new(vec![], false)),
            fail_comments: false,
            required_checks: RequiredChecks::Green,
            merged: RefCell::new(Vec::new()),
            asked: RefCell::new(Vec::new()),
            reached_comments: RefCell::new(false),
            reached_checks: RefCell::new(false),
        }
    }
}

impl Reads for Fake {
    fn pr(&self, pr: u64) -> Result<Pr> {
        self.asked.borrow_mut().push(("pr", pr));
        Ok(Pr {
            head: self.head.into(),
            state: self.state.into(),
            title: self.title.clone(),
            mergeable: self.mergeable.into(),
        })
    }
    fn base_sha(&self, pr: u64) -> Result<String> {
        self.asked.borrow_mut().push(("base", pr));
        Ok(self.base.into())
    }
    fn comments(&self, pr: u64) -> Result<Vec<TimedComment>> {
        self.asked.borrow_mut().push(("comments", pr));
        *self.reached_comments.borrow_mut() = true;
        if self.fail_comments {
            return Err(anyhow::anyhow!("gh: not authenticated"));
        }
        Ok(self.comments.clone())
    }
    fn drift(&self) -> Result<drift::Report> {
        self.drift.clone().map_err(anyhow::Error::msg)
    }
    fn required_checks(&self, pr: u64) -> RequiredChecks {
        self.asked.borrow_mut().push(("checks", pr));
        *self.reached_checks.borrow_mut() = true;
        self.required_checks.clone()
    }
    fn ci_gate_url(&self, _sha: &str) -> Option<String> {
        Some("https://github.com/fagemx/edda/runs/123".into())
    }
    fn squash(&self, pr: u64, head: &str, subject: &str, body: &str) -> Result<()> {
        self.merged
            .borrow_mut()
            .push((pr, head.into(), subject.into(), body.into()));
        Ok(())
    }
}

fn lgtm() -> Vec<TimedComment> {
    vec![review(1, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T11:00:00Z")]
}

#[test]
fn delegated_merge_requires_exact_base_and_refuses_unknown_mergeability() {
    let binding = ControlledMergeBinding {
        pr: 4242,
        expected_head_sha: HEAD.into(),
        expected_base_sha: BASE.into(),
    };
    let mut moved = Fake::clean(lgtm());
    moved.base = OLDER;
    assert_eq!(
        merge_inner_with_binding(&args(true), &moved, Some(&binding)).unwrap(),
        1
    );
    assert!(!*moved.reached_comments.borrow());

    let mut unknown = Fake::clean(lgtm());
    unknown.mergeable = "UNKNOWN";
    assert_eq!(
        merge_inner_with_binding(&args(true), &unknown, Some(&binding)).unwrap(),
        1
    );
    assert!(!*unknown.reached_comments.borrow());
}

#[test]
fn delegated_merge_adopts_an_exact_already_merged_subject_without_a_second_effect() {
    let binding = ControlledMergeBinding {
        pr: 4242,
        expected_head_sha: HEAD.into(),
        expected_base_sha: BASE.into(),
    };
    let mut fake = Fake::clean(lgtm());
    fake.state = "MERGED";
    assert_eq!(
        merge_inner_with_binding(&args(true), &fake, Some(&binding)).unwrap(),
        0
    );
    assert!(!*fake.reached_comments.borrow());
    assert!(fake.merged.borrow().is_empty());
}

// case 2/8: a lone qualifying LGTM is accepted; required checks are
// still asked, byte-identical accept path.
#[test]
fn a_lone_qualifying_lgtm_is_accepted() {
    let fake = Fake::clean(lgtm());
    let code = merge_inner(&args(false), &fake).unwrap();
    assert_eq!(code, 0);
    assert!(*fake.reached_checks.borrow(), "checks must be asked");
    assert!(fake.merged.borrow().is_empty(), "no --merge, no squash");
}

// case 1: an earlier Changes Requested under a later LGTM is refused by
// the union (GH-742) — and required checks are never asked after the
// union already refused.
#[test]
fn a_standing_changes_requested_under_a_later_lgtm_is_refused() {
    let fake = Fake::clean(vec![
        review(
            1,
            HEAD,
            "Changes Requested, P0=0, P1=1",
            "2026-09-08T10:00:00Z",
        ),
        review(2, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T11:00:00Z"),
    ]);
    let code = merge_inner(&args(false), &fake).unwrap();
    assert_eq!(code, 1, "a standing Changes Requested was merged over");
    assert!(
        !*fake.reached_checks.borrow(),
        "required checks were queried after the union already refused"
    );
}

// case 3: a gate that cannot judge refuses — an unreadable comment list
// is never an approval.
#[test]
fn an_unreadable_comment_list_refuses() {
    let mut fake = Fake::clean(lgtm());
    fake.fail_comments = true;
    assert_eq!(merge_inner(&args(false), &fake).unwrap(), 2);
}

// case 4: a verdict comment the union could not read refuses — the
// union reads `success` precisely because the blocking round is
// invisible to it (#917).
#[test]
fn a_malformed_verdict_comment_refuses() {
    let mut timed = review(1, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T11:00:00Z");
    timed.comment.body = format!(
        "narration first\n## Code Review: Round 1 — PR #4242 @ {HEAD}\n\n### \
         Verdict\n\nLGTM (P0=0, P1=0)"
    );
    let fake = Fake::clean(vec![timed]);
    let code = merge_inner(&args(false), &fake).unwrap();
    assert_eq!(code, 1, "a malformed verdict comment was ignored");
}

// #1124's split fixture: an open set where one unrelated PR drifted, over
// a subject whose own gate is green. The walk's lines still print
// (`drift::advisory`'s own test pins that) and no longer refuse, so the
// reads the shell stopped at are all reached — and the subject really
// merges: Round 4's second P0 was that `args(false)` never reached the
// squash branch, so this fixture proved only acceptance, not the merge the
// doneWhen claims.
#[test]
fn a_dirty_open_set_is_advisory_and_the_green_subject_merges() {
    let mut fake = Fake::clean(lgtm());
    fake.drift = Ok(drift::Report::new(
        vec![
            "#9001 dddddddddddd main no verdict on head".into(),
            "#9002 eeeeeeeeeeee main LGTM".into(),
        ],
        true,
    ));
    let code = merge_inner(&args(true), &fake).unwrap();
    assert_eq!(code, 0, "an unrelated PR's drift refused this merge");
    assert!(
        *fake.reached_comments.borrow(),
        "the subject's own review was never read"
    );
    assert!(
        *fake.reached_checks.borrow(),
        "the subject's own required checks were never asked"
    );
    let merged = fake.merged.borrow();
    assert_eq!(
        merged.len(),
        1,
        "the green subject never reached the squash"
    );
    assert_eq!(
        (merged[0].0, merged[0].1.as_str()),
        (4242, HEAD),
        "the squash was not pinned to the subject's own head"
    );
}

// Round 4's first P0: the walk's reducer orders the subject's comments by
// creation, the merge gate's latest-review selection by GitHub's edit
// time. An older head-pinned LGTM edited after a later stale verdict is
// therefore newest for stage 4 — which approves — while the reducer still
// reads the stale verdict, which the walk calls a hold. Two readings of
// the subject that disagree are not a green.
#[test]
fn an_edited_lgtm_cannot_outvote_the_stale_verdict_the_walk_still_reads() {
    let fake = Fake::clean(vec![
        // Created first, edited last: newest by `updated_at`, so this is
        // the round stage 4 judges and approves.
        review(1, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T12:00:00Z"),
        // Created last, never edited: newest by comment order, and stale.
        review(
            2,
            OLDER,
            "Changes Requested, P0=0, P1=1",
            "2026-09-08T11:00:00Z",
        ),
    ]);
    let code = merge_inner(&args(false), &fake).unwrap();
    assert_eq!(
        code, 1,
        "edit order outvoted the walk's stale reading of the subject"
    );
    assert!(
        !*fake.reached_checks.borrow(),
        "required checks were queried after the subject's own hold already refused"
    );
}

// Round 7's P1: the check above must not let a SHADOW round hold the
// subject. SHADOW rounds are not verdicts (`docs/fleet/rules.md` R18) and
// never enter the union, so one pinned to an older SHA — created after the
// authoritative round, the way a calibration round follows a review — must
// not turn the gate's own edit-ordered approval into a refusal.
#[test]
fn a_shadow_round_cannot_hold_the_subject() {
    let mut shadow = review(2, OLDER, "LGTM (P0=0, P1=0)", "2026-09-08T11:00:00Z");
    shadow.comment.body = format!(
        "## Code Review: Round 2 (SHADOW) — PR #4242 @ {OLDER}\n\n- shadow: true\n\n### \
         Verdict\n\nLGTM (P0=0, P1=0)\n"
    );
    let fake = Fake::clean(vec![
        // Authoritative, pinned to head, edited after the shadow round:
        // newest by `updated_at`, so stage 4 judges and approves it.
        review(1, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T12:00:00Z"),
        // SHADOW, created last: newest by comment order, stale by pin.
        shadow,
    ]);
    let code = merge_inner(&args(false), &fake).unwrap();
    assert_eq!(code, 0, "a SHADOW round held the subject");
    assert!(*fake.reached_checks.borrow());
}

// Round 8's P1: a SHADOW round is not a verdict, so it can never be the
// review that decides a merge — including when it is the newest by
// `updated_at`, where stage 4 used to select it and then refuse its older
// pin. The authoritative round decides instead, and the gate accepts.
#[test]
fn a_shadow_round_is_never_the_deciding_review() {
    let mut shadow = review(2, OLDER, "LGTM (P0=0, P1=0)", "2026-09-08T12:00:00Z");
    shadow.comment.body = format!(
        "## Code Review: Round 2 (SHADOW) — PR #4242 @ {OLDER}\n\n- shadow: true\n\n### \
         Verdict\n\nLGTM (P0=0, P1=0)\n"
    );
    let fake = Fake::clean(vec![
        review(1, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T11:00:00Z"),
        shadow,
    ]);
    let code = merge_inner(&args(false), &fake).unwrap();
    assert_eq!(code, 0, "a SHADOW round decided the merge");
    assert!(*fake.reached_checks.borrow());
}

// The other half of the split: the subject's OWN stale verdict still
// refuses. A guard rather than regression evidence — a single stale
// verdict named by the walk's line was also refused by the code this
// branch replaced, because back then ANY open PR's hold refused. The
// fixture above is the one that fails on the replaced code.
#[test]
fn the_subjects_own_stale_verdict_still_refuses() {
    let mut fake = Fake::clean(vec![review(
        1,
        OLDER,
        "LGTM (P0=0, P1=0)",
        "2026-09-08T11:00:00Z",
    )]);
    fake.drift = Ok(drift::Report::new(
        vec![format!(
            "#4242 {} main stale from {}",
            &HEAD[..12],
            &OLDER[..12]
        )],
        true,
    ));
    let code = merge_inner(&args(false), &fake).unwrap();
    assert_eq!(code, 1, "the subject's own drifted verdict was merged");
    assert!(
        !*fake.reached_checks.borrow(),
        "required checks were queried after the subject's own hold already refused"
    );
}

// Round 3's P0: a missing or non-string `mergeable` parses as an empty
// string, which is not the string CONFLICTING — so an exact comparison
// let an unread answer through. Anything outside GitHub's three values is
// now exit 2, and `UNKNOWN` stays a non-conflict.
#[test]
fn an_unreadable_mergeability_refuses() {
    let mut fake = Fake::clean(lgtm());
    fake.mergeable = "";
    assert_eq!(
        merge_inner(&args(false), &fake).unwrap(),
        2,
        "an unread mergeability answer was treated as green"
    );
    assert!(
        !*fake.reached_comments.borrow(),
        "the refusal came after the review reads"
    );
}

#[test]
fn an_unknown_mergeability_is_not_a_conflict() {
    let mut fake = Fake::clean(lgtm());
    fake.mergeable = "UNKNOWN";
    assert_eq!(merge_inner(&args(false), &fake).unwrap(), 0);
}

// Round 1's P0 (a CONFLICTING subject must not be reported ready) fixed
// through the subject's own read, and Round 2's P0 in the same fixture:
// the walk cannot read at all, and the refusal still holds — which is the
// whole point of not sourcing the subject's hold from the fleet walk.
#[test]
fn a_conflicting_subject_refuses_even_when_the_walk_cannot_read() {
    let mut fake = Fake::clean(lgtm());
    fake.mergeable = "CONFLICTING";
    fake.drift = Err("pr list failed".into());
    let code = merge_inner(&args(false), &fake).unwrap();
    assert_eq!(code, 1, "a CONFLICTING subject was reported ready");
    assert!(
        !*fake.reached_comments.borrow(),
        "the refusal came after the review reads, not from the subject's own facts"
    );
}

// GH-993's other hold on the subject, decided from the subject's own
// comments: a Review Response to a round that was never posted. Round 2's
// P0 named this state as uncovered.
#[test]
fn an_orphan_review_response_on_the_subject_refuses() {
    let mut response = review(2, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T12:00:00Z");
    response.comment.body = "## Review Response: Round 9

Addressed in abc1234."
        .into();
    let fake = Fake::clean(vec![
        review(1, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T11:00:00Z"),
        response,
    ]);
    let code = merge_inner(&args(false), &fake).unwrap();
    assert_eq!(code, 1, "an orphan Review Response was merged over");
    assert!(
        !*fake.reached_checks.borrow(),
        "required checks were queried after the orphan check refused"
    );
}

// case 7: the drift walk failing to read is advisory like the walk itself
// (#1124) — a fleet-wide read was never the subject's R6 gate, and making
// it a refusal put every merge behind one unrelated PR's readability.
#[test]
fn a_failed_drift_read_is_advisory() {
    let mut fake = Fake::clean(lgtm());
    fake.drift = Err("pr list failed".into());
    assert_eq!(merge_inner(&args(false), &fake).unwrap(), 0);
    assert!(*fake.reached_checks.borrow());
}

// case 8's non-vacuous half: a clean drift state over a populated open
// set does not block.
#[test]
fn a_populated_clean_drift_state_does_not_block() {
    let mut fake = Fake::clean(lgtm());
    fake.drift = Ok(drift::Report::new(
        vec!["#9002 eeeeeeeeeeee main LGTM".into()],
        false,
    ));
    assert_eq!(merge_inner(&args(false), &fake).unwrap(), 0);
}

// ---- the per-review checks the shell carried ---------------------------

#[test]
fn the_latest_trusted_review_must_be_pinned_to_the_head() {
    let fake = Fake::clean(vec![review(
        1,
        "1111111111111111111111111111111111111111",
        "LGTM (P0=0, P1=0)",
        "2026-09-08T11:00:00Z",
    )]);
    assert_eq!(merge_inner(&args(false), &fake).unwrap(), 1);
}

#[test]
fn an_edit_moves_which_review_is_latest() {
    // Round 1 was edited after Round 2 was posted: GitHub's updated_at
    // ordering makes Round 1 the latest trusted review, and its blocker
    // stands even though a later round approved.
    let fake = Fake::clean(vec![
        review(
            1,
            HEAD,
            "Changes Requested, P0=0, P1=1",
            "2026-09-08T12:00:00Z",
        ),
        review(2, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T11:00:00Z"),
    ]);
    assert_eq!(merge_inner(&args(false), &fake).unwrap(), 1);
}

/// The window `an_edit_moves_which_review_is_latest` does not reach: that
/// fixture puts the edited blocker on the **head** SHA, where `gate::union`
/// already refuses on its own, so it holds even with the ordering deleted.
/// Here the blocker is pinned to an older SHA — invisible to the union —
/// and only the `(updated_at, id)` ordering makes it the latest review the
/// head-pinning check then rejects. Take the ordering away (select the
/// first candidate, say) and the head-pinned LGTM wins and this merges.
#[test]
fn an_edited_blocker_on_an_older_sha_still_decides_which_review_is_latest() {
    let fake = Fake::clean(vec![
        // Deliberately first in the list: a selection that ignores
        // `updated_at` and takes what it sees first picks this one.
        review(2, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T11:00:00Z"),
        review(
            1,
            OLDER,
            "Changes Requested, P0=0, P1=1",
            "2026-09-08T12:00:00Z",
        ),
    ]);
    let code = merge_inner(&args(false), &fake).unwrap();
    assert_eq!(
        code, 1,
        "the most recently edited trusted review was not the one judged"
    );
    assert!(
        !*fake.reached_checks.borrow(),
        "required checks were queried after the review check already refused"
    );
}

/// GH-1105 review round 1: the shell refused outright when a trusted §7
/// review lacked the REST keys the selection orders by — the port defaulted
/// them, which sorts the unreadable comment to the bottom of the key and
/// lets an older LGTM be chosen instead of stopping the gate.
///
/// The fixture sits inside the window where that difference shows: the
/// keyless comment is a blocker pinned to an **older** SHA, so it is
/// outside the union and outside the head-pinning check. Restore the two
/// `unwrap_or` defaults and the head-pinned LGTM is selected, every
/// remaining precondition passes and this merges at exit 0.
#[test]
fn a_trusted_review_lacking_its_rest_ordering_keys_refuses() {
    for (label, id, updated_at) in [
        ("no updated_at", "700", ""),
        ("no numeric id", "", "2026-09-08T13:00:00Z"),
        ("node id, not a REST id", "IC_kwDOA", "2026-09-08T13:00:00Z"),
    ] {
        let mut broken = review(7, OLDER, "Changes Requested, P0=1, P1=0", updated_at);
        broken.comment.id = id.into();
        let fake = Fake::clean(vec![lgtm().remove(0), broken]);
        let code = merge_inner(&args(false), &fake).unwrap();
        assert_eq!(
            code, 1,
            "{label}: a trusted review the gate cannot order was sorted to the bottom \
             instead of refusing"
        );
        assert!(
            !*fake.reached_checks.borrow(),
            "{label}: required checks were queried after the selection already refused"
        );
    }
}

/// GH-1105 review round 1: candidacy narrowed from the shell's heading
/// prefix to the full §7 grammar, so a blocking round whose heading SHA is
/// truncated, typo'd or uppercase stopped being a candidate at all — and
/// `deliver::extract` did not report it malformed either, because its #917
/// branch also asked the full grammar. Invisible to the latest-review
/// check, to `malformed` and to the union at once is the exact hole #917's
/// refusal exists to close. Widen the filter back to `heading_shaped` and
/// the round blocks: either as the latest review whose heading will not
/// parse, or through the malformed refusal.
#[test]
fn a_blocking_round_whose_heading_sha_is_malformed_still_stops_the_gate() {
    for bad in [
        "aaaaaaaabbbbbbbbccccccccdddddddd1111222", // truncated by one
        "AAAAAAAABBBBBBBBCCCCCCCCDDDDDDDD11112222", // uppercase
        "aaaaaaaabbbbbbbbccccccccdddddddd1111222g", // a typo'd digit
    ] {
        let fake = Fake::clean(vec![
            review(1, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T11:00:00Z"),
            review(
                2,
                bad,
                "Changes Requested, P0=1, P1=0",
                "2026-09-08T12:00:00Z",
            ),
        ]);
        let code = merge_inner(&args(false), &fake).unwrap();
        assert_eq!(
            code, 1,
            "a blocking round headed @ {bad} was invisible to the whole gate"
        );
        assert!(
            fake.merged.borrow().is_empty(),
            "merged over a blocking round headed @ {bad}"
        );
    }
}

/// The same widening isolated at its own seam, because the end-to-end
/// exit code above cannot separate the two halves of the fix: the
/// malformed refusal at stage 5 would reach the same `1`. Here only the
/// candidate filter decides — narrow it back to the full grammar and the
/// broken round is skipped, the older LGTM is selected, and this returns
/// `Ok("1")`.
#[test]
fn a_malformed_heading_is_the_latest_review_not_a_skipped_one() {
    let timed = vec![
        review(1, HEAD, "LGTM (P0=0, P1=0)", "2026-09-08T11:00:00Z"),
        review(
            2,
            &HEAD.to_uppercase(),
            "Changes Requested, P0=1, P1=0",
            "2026-09-08T12:00:00Z",
        ),
    ];
    let code = latest_review_round(&timed, 4242, HEAD)
        .expect_err("the broken round was skipped and the older LGTM approved the merge");
    assert_eq!(code, 1);
}

#[test]
fn a_provisional_verdict_never_merges() {
    let fake = Fake::clean(vec![review(
        1,
        HEAD,
        "Provisional — LGTM (P0=0, P1=0), escalation pending",
        "2026-09-08T11:00:00Z",
    )]);
    assert_eq!(merge_inner(&args(false), &fake).unwrap(), 1);
}

#[test]
fn unresolved_escalations_refuse() {
    let mut timed = lgtm().into_iter().next().unwrap();
    timed.comment.body = timed
        .comment
        .body
        .replace("- escalations: none", "- escalations: P2-1 pending");
    let fake = Fake::clean(vec![timed]);
    assert_eq!(merge_inner(&args(false), &fake).unwrap(), 1);
}

#[test]
fn a_closed_pr_refuses() {
    let mut fake = Fake::clean(lgtm());
    fake.state = "MERGED";
    assert_eq!(merge_inner(&args(false), &fake).unwrap(), 1);
}

#[test]
fn every_non_green_required_check_result_refuses_without_squashing() {
    for (result, expected) in [
        (RequiredChecks::Absent, 1),
        (RequiredChecks::Blocked(vec![]), 1),
        (RequiredChecks::RacedNowGreen(vec![]), 1),
        (RequiredChecks::Indeterminate("unreadable".into()), 2),
    ] {
        let mut fake = Fake::clean(lgtm());
        fake.required_checks = result;
        assert_eq!(merge_inner(&args(true), &fake).unwrap(), expected);
        assert!(fake.merged.borrow().is_empty(), "refusal reached squash");
    }
}

// ---- GH-1100, folded in -------------------------------------------------

#[test]
fn the_squash_subject_is_always_the_pr_title_never_a_branch_commit() {
    // The single-commit-PR case #1100 filed: GitHub picks the branch
    // commit's own subject when the PR has exactly one commit. The verb
    // pins --subject itself, so it cannot.
    let mut fake = Fake::clean(lgtm());
    fake.title = "fix(edda-cli): the PR title, not the branch commit".into();
    assert_eq!(merge_inner(&args(true), &fake).unwrap(), 0);
    let (pr, head, subject, _body) = fake.merged.borrow()[0].clone();
    assert_eq!(pr, 4242);
    assert_eq!(head, HEAD);
    assert_eq!(
        subject,
        "fix(edda-cli): the PR title, not the branch commit (#4242)"
    );
}

// ---- GH-1100 Round 2's other half: the (#N) back-reference -------------

/// The regression this PR reintroduced against #1118 (GH-1105 Round 2's
/// P0). GitHub appends ` (#N)` only to a squash subject it picks itself;
/// handing it one through `--subject` suppresses the suffix entirely, so
/// pinning the title and stopping there lands every squash commit on
/// `main` without a PR pointer — permanently, since R7 forbids rewriting
/// `main`.
///
/// The second assertion names that shape explicitly rather than leaving
/// it implied by the equality: drop the append in [`merge_subject`] and
/// the bare title is exactly what `gh` receives.
#[test]
fn the_squash_subject_keeps_its_pr_back_reference() {
    let mut fake = Fake::clean(lgtm());
    fake.title = "fix(edda-cli): a merge the fleet already reviewed".into();
    assert_eq!(merge_inner(&args(true), &fake).unwrap(), 0);
    let subject = fake.merged.borrow()[0].2.clone();
    assert_eq!(
        subject,
        "fix(edda-cli): a merge the fleet already reviewed (#4242)"
    );
    assert_ne!(
        subject, "fix(edda-cli): a merge the fleet already reviewed",
        "the squash subject carries no (#4242) back-reference — GitHub adds one only to a \
         subject it picks itself, so this commit would land on main with no PR pointer and \
         R7 forbids ever fixing it"
    );
}

/// The idempotence guard. A title copied back from a landed commit already
/// ends in this PR's own number; appending again would write
/// `… (#4242) (#4242)`.
#[test]
fn a_title_already_carrying_this_prs_number_is_not_doubled() {
    let mut fake = Fake::clean(lgtm());
    fake.title = "fix(edda-cli): a title that already carries its number (#4242)".into();
    assert_eq!(merge_inner(&args(true), &fake).unwrap(), 0);
    let subject = fake.merged.borrow()[0].2.clone();
    assert_eq!(
        subject,
        "fix(edda-cli): a title that already carries its number (#4242)"
    );
    assert!(
        !subject.contains("(#4242) (#4242)"),
        "the back-reference was appended to a title that already carried it: {subject}"
    );
}

/// The interesting half of that skip rule: `… (#999)` is prose — a
/// follow-up naming the PR it answers — not this commit's pointer.
/// Treating it as one would send `git log` readers to an unrelated PR.
#[test]
fn a_foreign_pr_number_in_the_title_still_gets_this_prs_back_reference() {
    let mut fake = Fake::clean(lgtm());
    fake.title = "fix(edda-cli): follow-up to the earlier change (#999)".into();
    assert_eq!(merge_inner(&args(true), &fake).unwrap(), 0);
    assert_eq!(
        fake.merged.borrow()[0].2,
        "fix(edda-cli): follow-up to the earlier change (#999) (#4242)"
    );
}

/// The subject assembly at its own seam, including the near-misses the
/// end-to-end cases above do not reach: the skip is the exact ` (#N)`
/// tail, so a number without the separating space, a number embedded
/// mid-title, and a different PR's number all still get the real
/// back-reference appended.
#[test]
fn merge_subject_matrix() {
    assert_eq!(merge_subject("fix(x): y", 4242), "fix(x): y (#4242)");
    assert_eq!(
        merge_subject("fix(x): y (#4242)", 4242),
        "fix(x): y (#4242)"
    );
    assert_eq!(
        merge_subject("fix(x): y(#4242)", 4242),
        "fix(x): y(#4242) (#4242)"
    );
    assert_eq!(
        merge_subject("fix(x): y (#4242) and more", 4242),
        "fix(x): y (#4242) and more (#4242)"
    );
    assert_eq!(
        merge_subject("fix(x): y (#42420)", 4242),
        "fix(x): y (#42420) (#4242)"
    );
    assert_eq!(
        merge_subject("fix(x): y (#999)", 4242),
        "fix(x): y (#999) (#4242)"
    );
}

/// U4 judges the bare title and the back-reference is composed after it,
/// so a suffix can neither rescue a bad title nor break a good one — the
/// two are separate values, and the composed one only ever reaches `gh`.
#[test]
fn u4_judges_the_bare_title_the_back_reference_is_composed_after() {
    let good = "fix(edda-cli): repair the gate";
    assert!(subject_problem(good).is_none());
    assert!(subject_problem(&merge_subject(good, 4242)).is_none());
    let bad = "wip(review): lane work in progress";
    assert!(subject_problem(bad).is_some());
    assert!(
        subject_problem(&merge_subject(bad, 4242)).is_some(),
        "a back-reference must not rescue a title U4 rejects"
    );
    // …and the refusal happens before anything is composed or merged.
    let mut fake = Fake::clean(lgtm());
    fake.title = bad.into();
    assert_eq!(merge_inner(&args(true), &fake).unwrap(), 1);
    assert!(fake.merged.borrow().is_empty());
}

// ---- GH-1100's two --body-file refusals ---------------------------------

/// An empty operand short-circuited the shell's `-f` test and read as "no
/// body file", so the one malformed operand that did not refuse was the
/// emptiest.
#[test]
fn an_empty_body_file_operand_refuses() {
    let fake = Fake::clean(lgtm());
    let mut merge = args(true);
    merge.body_file = Some(String::new());
    let error = merge_inner(&merge, &fake)
        .expect_err("an empty --body-file operand was accepted as 'no body file'");
    assert!(
        error.to_string().contains("--body-file"),
        "the refusal must name --body-file: {error:#}"
    );
    assert!(fake.merged.borrow().is_empty());
}

/// Only the merge path opens the file. Accepting it under `--check` told
/// the caller their body had been taken when nothing would ever read it.
#[test]
fn a_body_file_outside_merge_refuses_instead_of_being_ignored() {
    let fake = Fake::clean(lgtm());
    let mut check = args(false);
    check.body_file = Some("some-merge-body.md".into());
    let error = merge_inner(&check, &fake)
        .expect_err("--body-file was accepted and silently ignored outside --merge");
    assert!(
        error.to_string().contains("--merge"),
        "the refusal must say --body-file needs --merge: {error:#}"
    );
    assert!(
        !*fake.reached_comments.borrow(),
        "a rejected invocation still spent the PR reads"
    );
}

/// A typo'd path costs an error message, not the fleet-wide drift walk.
#[test]
fn a_body_file_that_does_not_exist_refuses_before_any_read() {
    let fake = Fake::clean(lgtm());
    let mut merge = args(true);
    merge.body_file = Some("no/such/merge-body.md".into());
    let error = merge_inner(&merge, &fake).expect_err("a missing body file was accepted");
    assert!(
        error.to_string().contains("body file not found"),
        "the refusal must name the missing file: {error:#}"
    );
    assert!(
        !*fake.reached_comments.borrow(),
        "the PR reads ran before the body file was known to exist"
    );
}

/// The supplied body rides through untouched — and does not cost the
/// commit its back-reference, which is composed on the merge call rather
/// than inside the compose-a-receipt branch.
#[test]
fn a_supplied_body_file_rides_through_and_keeps_the_back_reference() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("merge-body.md");
    std::fs::write(&path, "choreographed merge body from the operator\n").expect("write body");
    let fake = Fake::clean(lgtm());
    let mut merge = args(true);
    merge.body_file = Some(path.to_string_lossy().into_owned());
    assert_eq!(merge_inner(&merge, &fake).unwrap(), 0);
    let (_, _, subject, body) = fake.merged.borrow()[0].clone();
    assert_eq!(body, "choreographed merge body from the operator\n");
    assert!(
        !body.contains("Squash merge of PR"),
        "a supplied body file still composed a receipt: {body}"
    );
    assert_eq!(
        subject, "fix(edda-cli): a merge the fleet already reviewed (#4242)",
        "a supplied body file dropped the (#4242) back-reference"
    );
}

#[test]
fn a_non_conforming_title_refuses_with_the_offending_string() {
    for bad in [
        "wip(review): GH-1003 + GH-992 in progress — uncommitted lane work",
        "fix(): empty scope",
        "improve: things",
        "fix(edda-cli):",
        "no type or scope at all",
    ] {
        let mut fake = Fake::clean(lgtm());
        fake.title = bad.into();
        let code = merge_inner(&args(false), &fake).unwrap();
        assert_eq!(code, 1, "a non-conforming subject merged: {bad}");
        assert!(fake.merged.borrow().is_empty());
    }
}

#[test]
fn the_composed_receipt_names_the_sha_round_and_ci_run() {
    let fake = Fake::clean(lgtm());
    merge_inner(&args(true), &fake).unwrap();
    let body = &fake.merged.borrow()[0].3;
    assert!(body.contains(HEAD), "receipt must name the reviewed SHA");
    assert!(body.contains("Round 1"), "receipt must name the LGTM round");
    assert!(
        body.contains("https://github.com/fagemx/edda/runs/123"),
        "receipt must name the CI run"
    );
}

// ---- the PR selector every read must carry --------------------------

/// The argv itself, asserted the way `comments_argv` and `open_prs_argv`
/// are (GH-1079). Round 1's P0: `gh pr view --json headRefOid,state,title`
/// with no selector resolves the pull request of the current branch, so
/// the head, the OPEN state and the squash subject came from whichever PR
/// the cwd happened to be on.
#[test]
fn pr_argv_names_the_pull_request_it_reads() {
    assert_eq!(
        pr_argv(4242),
        vec![
            "pr",
            "view",
            "4242",
            "--json",
            "headRefOid,state,title,mergeable"
        ]
    );
    // Stated separately from the equality above so the regression this
    // guards is named where it fails: a selector-less argv is the defect,
    // not merely a different argv.
    assert!(
        pr_argv(4242).contains(&"4242".to_owned()),
        "gh would resolve the current branch's PR, not #4242"
    );
}

/// …and that argv is built from the `--pr` the operator gave, not from a
/// constant: every read in the enumerated order is asked for the same PR.
#[test]
fn every_read_is_asked_for_the_requested_pr() {
    let fake = Fake::clean(lgtm());
    assert_eq!(merge_inner(&args(true), &fake).unwrap(), 0);
    assert_eq!(
        *fake.asked.borrow(),
        vec![("pr", 4242), ("comments", 4242), ("checks", 4242)],
        "a precondition was evaluated against a PR other than --pr"
    );
    assert_eq!(fake.merged.borrow()[0].0, 4242, "squashed the wrong PR");
}

#[test]
fn subject_validation_matrix() {
    assert!(subject_problem("fix(edda-cli): repair the gate").is_none());
    assert!(subject_problem("chore(release): prepare v0.6.1").is_none());
    assert!(subject_problem("docs(fleet.rules): clarify").is_none());
    for bad in [
        "wip(x): y",
        "fix(x): ",
        "fix: no scope parens",
        "fix(): empty scope",
        "nope(x): unknown type",
    ] {
        assert!(
            subject_problem(bad).is_some(),
            "accepted a non-conforming subject: {bad}"
        );
    }
}
