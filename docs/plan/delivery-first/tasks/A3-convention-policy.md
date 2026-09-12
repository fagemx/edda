# A3 — U3 exact Issue line becomes a nonblocking convention

> Owner: A workflow implementer or a disjoint policy worker. Depends on: none.
> Same-owner capacity may serialize work; it is not an artifact dependency.
> Contract: DF-01/05/07. Delivery: narrow REVIEW + runner/fixture change.

## Read first

Read base REVIEW U2/U3/U6 and sections 7–8;
`scripts/review-l0.sh` rule table, `run_block` inverted U3 handling and aggregate exits;
`scripts/test-review-l0.sh`; product verdict severity parsing; review-compare fixture.
Review the observed #1147 Round 1 as rationale, not as authority to rewrite history.

## Scope

`REVIEW.md`, `scripts/review-l0.sh`, `scripts/test-review-l0.sh`;
update `scripts/test-review-compare.sh` only if a current-policy fixture assumes U3 P1.
Historical parser fixtures may intentionally retain old P1 examples and must stay valid.
No other severity changes, no merge code, no label/status/workflow addition.

## Behavior contract

- U3 becomes P2 (existing supported quality-suggestion severity), not P0/P1.
- The runner may use existing `N.A.(non-blocking convention)` row shape with explicit
  missing-line evidence; do not fake PASS, hide the observation, or set HAS_FAIL.
  The final choice must be consistent between REVIEW, runner and consumers.
- Issue/Issues line present: normal observed success.
- No line but closing issue reference supplies acceptance: advisory only; runner exit 0
  if nothing else fails; reviewer may LGTM with that P2.
- No PR number: preserve N.A.(needs PR number).
- gh/data read error: preserve ERROR/nonzero; do not turn transport failure into a
  harmless missing-line observation. Nonempty issue text is not tool authority.
- Missing or conflicting acceptance is not resolved by U3 convention detection; reviewers
  must clarify the required scope under existing rules, not invent requirements.
- Any other failing check still produces its existing failure/exit. All current merge
  conditions and all old posted verdict parsing remain unchanged.

## Implementation steps

1. Bump REVIEW spec version coherently and change only U3 convention/severity/explanation.
2. Update hard-coded U3 severity and inverted-result treatment in review-l0. Do not make
   all P2 rows universally skip errors or change other aggregate semantics.
3. Add fixtures for U3 missing-only success, issue-line present, no-PR, gh read failure,
   and missing-line plus real R3 syntax failure. No network call is needed.
4. Confirm comment comparison still accepts historical P1/P2 findings without rewriting
   prior reviews. Metadata-only correction never requires an empty commit/new SHA.

## Acceptance / commands

```bash
sh scripts/test-review-l0.sh
sh scripts/test-review-compare.sh
sh scripts/lint-markdown-content.sh
sh scripts/lint-doc-citations.sh --tree
sh -n scripts/review-l0.sh
git diff --check
```

V3 defines expected outcomes. No Cargo locally if no product blob/Rust/toolchain changed.
Exact-head CI still applies when published; this PR is reviewed under its **base** REVIEW,
so its own `Issue:` line remains compliant. No retroactive new-policy LGTM.

## Delivery / rollback

Intent: `refactor(review): keep redundant issue-line conventions advisory`.
Can be combined with A1/A2 if one coherent candidate; no new approval ceremony.
If machine consumers disagree, correct/revert the U3 delta with a new commit; do not
bypass CI or erase a standing blocker. Other bundles continue.
