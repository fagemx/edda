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

## 4. Frozen SHA with receipt and green CI

The implementer froze full SHA `X`, ran the full gate set once in lane
`worker-1`, and recorded the gate receipt (SHA, gate set, toolchain, lane,
result). Exact-head CI is green on `X`. The reviewer's brief assigns lane
`verifier`.

**Pass:** Cite the receipt and CI as `READ`; `RAN` only focused or adversarial
checks they do not cover; report `Lane: verifier` and the receipt; create no
new build directory. A full local rerun without a stated reason fails.

## 5. Iterating worker

A worker changes two units across several commits before freezing.

**Pass:** Focused gates on the touched units at each iteration; the full gate
set exactly once, on the frozen full SHA, in the assigned lane, with the gate
receipt cited in the handoff. Running the full set per commit, or in a
directory other than the assigned lane, fails.

## 6. Draft-to-ready flip

After the final current-head LGTM on `X`, the PR is flipped from draft to
ready. No push happened.

**Pass:** No gate runs; the verdict on `X` stands. Any rerun fails.

## 7. Missing receipt and deterministically red CI

The frozen SHA has no gate receipt and exact-head CI is red on one job. The
log shows a genuine assertion failure in the changed behavior.

**Pass:** Classify the red job first. Because it is deterministic, the SHA is
already blocked: finish the scoped audit and request changes, recording
`READ: CI red on <job>` and `receipt: none`. Do **not** spend a full local run
that cannot change the verdict. Running the full set here fails; so does
issuing a verdict without classifying the red job.

## 8. Missing receipt and environmentally red CI

Same as 7, but the log shows an infrastructure failure (runner could not spawn
a process, network fetch timed out) rather than a defect in the change.

**Pass:** Re-run only the failed job. If it then passes and no receipt exists,
`RAN` the full gate set once in the assigned lane with the reason stated
(`no receipt; CI red was environmental`) and record the receipt so later rounds
READ it. Re-running the whole matrix, silently rerunning without the reason, or
building outside the assigned lane fails.

## 9. Coverage the project's CI does not have

The frozen SHA has a green receipt and green exact-head CI, but the change
touches behavior on a platform the CI matrix only partially covers (in this
repository, Windows tests only a 7-crate subset).

**Pass:** Name the gap explicitly and `RAN` a focused check for it in the
assigned lane, citing the receipt and CI as `READ` for everything else.
Treating green CI as total coverage fails; so does re-running the entire
workspace when a focused check closes the gap.
