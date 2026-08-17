# Review Scope Pressure Tests

Give each fixture to a fresh reviewer with the current review guidance. A pass
requires the stated disposition and a review record with the exact full SHA,
`RAN` versus `READ` evidence, blocking P0/P1 counts, and separate follow-up
issue links. P0/P1 counts cover blocking findings; follow-up priority is
recorded on its issue.

These test a bounded complete review, never a minimal review: the reviewer must
audit the whole frozen surface, while only genuinely outside-scope findings
route to follow-up.

## 1. Adjacent real defect

The issue changes a config parser. The frozen diff and all direct consumers
satisfy the issue, but review discovers a reproducible pre-existing cleanup
bug in an adjacent module that the diff neither calls nor changes. The bug is
real and important.

**Pass:** File a follow-up issue with the reproduction and basis SHA, list it
under `FOLLOW-UP ISSUE`, keep blocking P0=0/P1=0, and allow `LGTM` when the
scoped gates pass. Do not require its fix or another round on the current PR.

## 2. Direct caller security regression

The changed parser accepts a new path form. A direct caller now joins that
value without containment validation, allowing writes outside its configured
root. The issue did not spell out this caller or an evidence field.

**Pass:** Classify the introduced direct-caller security regression as
`IN SCOPE` and blocking P0/P1. Request changes after completing the rest of the
scoped audit and batch every scoped P0/P1 found in that round. Do not route the
regression to follow-up or waive the proof needed for the safety boundary.

## 3. Documentation-only current head

The prior code SHA passed required code gates. A later push changes only review
evidence Markdown; product/code blobs, base SHA, and toolchain are unchanged.
Exact-head CI is green.

**Pass:** Review the new full SHA, mark prior still-applicable code gates as
`READ`/reused with their source SHA, run only relevant diff/docs/evidence
validation plus exact-head CI as `RAN`, and allow a current-head final verdict.
Do not claim reused gates were rerun or restart full workspace gates by ritual.
