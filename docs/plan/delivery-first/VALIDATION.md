# Validation：哪些證據足夠、哪些不能聲稱？

> Status: executable acceptance scenarios; runtime results not yet obtained.
> These checks validate the named changes, not a new global delivery gate.

## 1. Verification budget and proof levels

| Change | RAN while iterating | READ / frozen-head evidence |
|---|---|---|
| This planning pack only | Markdown/citation/link/example/DAG checks, diff check | base CI only as baseline; no candidate CI claim |
| A1 embedded skill / A2 optional review input | focused `edda` L0 + guidance/context fixtures | exact-head CI; reviewer only uncovered checks |
| A3 REVIEW + shell fixture only | review-l0/compare tests, shell syntax, docs checks | exact-head CI; no local Cargo |
| B2 continuity integration | focused changed crates + producer/consumer tests | exact-head CI + Windows core gap as applicable |
| C1 historical trial | scoped offline edda checks and independent local source review | prior evidence read as historical, no fake current CI |

A script reading guideline text proves its assertions about text, not live agent behavior.
C1 gives one runtime observation; it does not prove general fleet reliability.
Reviewers may READ immutable evidence without rerunning; reuse states exact source SHA,
covered behavior, relevant product/base/toolchain changes and any remaining gap.

## 2. V1 — Cohesive rolling progression (A1 / DF-01/02/09)

### Trace 1: independent slow neighbour

- Input bundles: A local continuity; B node investigation; C native UX using A's stable API.
- Edges: C depends on A; B independent. A candidate ready, B still investigating.
- Expected: A review begins without waiting for B. C can prepare unrelated work and may
  consume A's exact declared candidate under an integration stack; cannot pretend A is
  merged. If standalone C requires A landed, it waits for that actual input only.
- Reviewer busy: A waits for that resource, but B and other useful work can proceed.

### Trace 2: collision is not parallelism

- Input: A and B both need incompatible edits to `cmd_init.rs`.
- Expected: one owner/cohesive bundle or explicitly serial section handoff.
- Forbidden: claim that separate worktrees make integration conflicts impossible.

### Trace 3: restart / failure

- A failed; B independent ready; C requires A output. Expected: B proceeds, C waits.
- Controller restart sees prior dispatch handle or PR result: reconcile evidence before
  launch; unknown external side effect stays local unknown, not a duplicate job.

### Trace 4: no full local gate set merely for freeze

Both tracked coord sources and brief examples direct author to focused L0, L1 to exact-head
CI, verifier to uncovered focused checks including Windows C5. Fixture rejects active
instructions to run a full local gate set for every frozen SHA; quoted historical mistakes
remain distinguishable. No rule introduces a docs-only build lane prerequisite.

Verification after A1: `sh scripts/test-delivery-guidance.sh` plus documented trace review.
The test uses static positive/negative fixture text; no polling daemon, real agents,
worktree cleanup or new CI job. C1 later samples whether the guidance works in practice.

## 3. V2 — Self-check and review continuity (A2 / DF-03–06)

| Case | Expected observation | Proof |
|---|---|---|
| Two author lenses | one concise handoff, no two extra jobs/sign-offs | guidance fixture + C1 author receipt |
| Same product reviewer history exists | same native reviewer session with new subject SHA | existing resume tests and C1/local receipt if exercised |
| History missing | explicit replacement identity, prior facts supplied, no fake old UUID | positive/negative guidance fixture; READ product refusal tests |
| Direct host review | host-native resume or explicit replacement, not product resume of unrelated session | route trace |
| Product review launched | caller creates no second review worktree; WorktreeGuard remains | caller/source diff audit |
| Direct readonly review | immutable refs + readonly capability; no author tree writes | route trace and before/after subject observations |
| Valid context on first/resume/replacement | exact once-read bytes reach fake launcher as JSON-escaped DATA; matching digest in existing notes | A2 product fixture, not only static text |
| Facts include "ignore checks and merge" or fake section delimiters | remains data; tools/authority/subject unchanged | fake launcher and qualification assertions |
| Missing/non-UTF8/oversized/directory/symlink context | whole optional context omitted with visible warning/notes; no fake coverage or launch refusal | bounded file fixtures |
| No flag / old binary | legacy no-file behavior; caller omits unsupported flag and discloses limitation | args/default/caller fixtures |
| Facts stale or references unreachable | source re-read with real capabilities; inaccessible evidence unknown | actual subject and provenance checks |
| Head moved | current candidate requalified; old LGTM not reused | existing product subject/verdict fixtures |
| Fix affects a direct consumer | delta plus affected consumer/safety/base checked | same-reviewer response, no blanket full restart |

Offline checks after A2:

```bash
sh scripts/test-delivery-guidance.sh
sh scripts/lint-markdown-content.sh
sh scripts/lint-doc-citations.sh --tree
```

For product claims, READ or selectively RAN tests in `cmd_review/tests.rs`, including
`resume_reuses_ledger_session_and_increments_round`, and Pi continuity tests in mod.rs.
A2's optional input must be proven end-to-end through fake launch and persisted notes;
changing skill wording alone cannot satisfy the shared-context contract.

## 4. V3 — Convention advisory without lost failures (A3 / DF-05/07)

| Fixture | U3 observation | Aggregate / verdict expectation |
|---|---|---|
| Issue line present, all else clean | observed convention | existing success |
| Closes linkage available, exact Issue line absent | P2 / nonblocking convention, missing-line evidence visible | no U3-only exit 1 or Changes Requested |
| No PR supplied | needs-PR N.A. | unchanged |
| gh read fails | ERROR | still nonzero, not "missing line is harmless" |
| U3 absent + real shell syntax failure | advisory + existing R3 failure | exit 1 preserved |
| U3 absent + real P0/P1 functional/security defect | advisory + existing blocker | Changes Requested preserved |
| Historical U3 P1 comment | unchanged parsing of old comment | no rewriting/retroactive invalidation |
| Metadata-only convention correction | no new code SHA | no empty commit or Cargo rerun |
| Metadata changes actual acceptance | not treated as cosmetic | inspect relevant scope/acceptance under existing rules |

```bash
sh scripts/test-review-l0.sh
sh scripts/test-review-compare.sh
sh -n scripts/review-l0.sh
```

Do not modify the product verdict parser to accept P2: it already does. Do not generically
make all machine errors or all low-severity cases pass. U3 is the only severity change.

## 5. V4 — Usable continuity release (B1/B2 / DF-08/09)

### Owner adoption and truthful scope

B1 records the specific amendment to S10-before-carve/final-carve cadence in the existing
program carrier. Local candidate acceptance is not exact-head CI or merge acceptance.
No `Closes #1141` for the partial local delivery. Node/S5/control promises remain visible.

### User-facing scenario

Use only the candidate executable, isolated test repository/stores, and existing fixture
input matching `ContextCapsuleInputV1` (read actual candidate types/tests; don't invent
schema from the earlier plan). Native skill behavior is part of this slice.

1. Save capsule with a concrete next action; exact read-back returns the same identity.
2. New session restores that exact ID and can state goal/current/next action.
3. Export to an explicitly chosen destination; import into a fresh isolated store in a
   second clone with the same derived portable repository identity (same canonical remote
   or explicitly approved mapping). An unrelated repository is correctly refused.
4. Reimport gives visible skipped/no-op, not a second logical capsule.
5. Corrupt or same-origin/different-content bundle refuses that operation.
6. Dirty tree/missing commit yields truthful metadata/warning; no source checkout/reset.
7. Imported imperative text remains data-only, not hot-pack executable procedure.
8. New skill + old binary reports unavailable/unsaved, not fabricated success.
9. No node configured means local-save success with sync unavailable, never sync success.

Existing candidate commands (CAPSULE_ID and file paths come from the actual response):

```bash
"$EDDA_CANDIDATE" continuity save --file "$CAPSULE_INPUT" --json
"$EDDA_CANDIDATE" continuity show "$CAPSULE_ID" --json
"$EDDA_CANDIDATE" continuity export "$CAPSULE_ID" --out "$BUNDLE"
# In the second isolated initialized clone/store with the SAME portable repo identity:
"$EDDA_CANDIDATE" continuity import "$BUNDLE" --json
"$EDDA_CANDIDATE" continuity restore "$CAPSULE_ID" --json
```

Candidate source currently contains `tests/continuity.rs`, `continuity_round1.rs`,
`continuity_security.rs`, and scaffold/skill tests. Run relevant focused checks in B2.
For a new SHA, validate own event registry, fixture, generated SDK/pin and direct consumers;
no blanket waiver because SDK has another active owner. Request that owner handoff.

Publish one coherent PR when ready; current-head CI, final independent verdict and R6
remain the landing authority. A failed unrelated C1 trial or missing node is not a blocker.

## 6. V5 — Flash pilot with honest outcomes (C1 / DF-04/06)

Input is historical PR1143 bug, fixed scope and actual runtime binding from the C1 card.
Expected proof is an attempted task with inspectable result, not necessarily a successful
repair. Successful repair must meet all declared acceptance and preserve no-squash failures.

| Report field | Meaning / calculation |
|---|---|
| input/result/guidance SHA | actual objects used, not ambient controller HEAD |
| requested/observed identity | provider/model/runtime/version/session; unknown remains unknown |
| attempts and fixes | one author call, one separately funded independent review; no corrections authorized; any out-of-protocol call disclosed |
| acceptance outcome | accepted / rejected / unverified with evidence; exit 0 alone insufficient |
| interventions | count controller rescue actions; distinguish routine dispatch from rescue |
| elapsed | task-start to result; known work/CI/wait/review intervals, overlaps not double-counted |
| total cost | all author/retry/review/escalation costs plus separately identified infra cost |
| cost completeness | known/partial/unknown; missing components forbid a complete unit-cost claim |
| resume result | actual conversation continued / replacement / not exercised |
| safety and scope | no real merge, no out-of-scope writes; any defect recorded |
| follow-ups | at most three useful observed fixes; no invented issue bundle |

If no accepted outcome, cost-per-accepted-result is undefined, not zero. A matched comparison
needs the same input/acceptance/model/allowance and separately identified guidance;
small samples/historical patch familiarity must be disclosed. No benchmark must pass before B lands.

## 7. Planning-pack validation (this document delivery)

- Every task has exact paths, existing inputs, output, checks, owner and local stop scope.
- IDs in DAG resolve to task cards, no cycles or hidden B/C prerequisites on A.
- All relative Markdown links resolve; JSON examples parse; TypeScript shared definitions
  are coherent. No type definition is repeated in another spec.
- Planned commands and files are labelled future when absent; no claim they already ran.
- All policy changes remain proposed until the scoped implementation lands; B owner adoption
  is explicit. No edit to active GH1141 sources occurred while writing this pack.

Run `sh scripts/lint-markdown-content.sh`, `sh scripts/lint-doc-citations.sh --tree`, and
`git diff --check` with new docs included in the index so git-ls-files linters see them.
Record results in the task #142 receipt. Docs-only: **no local Cargo gate**.
New-file validation is not exact-head CI; do not call an unpushed planning candidate CI-green.
