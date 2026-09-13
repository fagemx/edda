# Portable Continuity + Guided Execution: integration rail

## Authority and scope

Operator approved the program and said to begin implementation on 2026-09-11.
The acceptance ceiling is [the program plan](../plans/2026-09-11-context-save-edda-integration.md), sections 5, 15 and 18.
This is one integration train, not approval to merge unreviewed slices.

- Basis: `8806e6a59ea284d7b73cbd2b99ab80432851ed81`.
- Integration branch: `codex/continuity-guidance-train`.
- Edda controller task: #113.
- S0 contract revision: plan section 5 at the first train commit; implementation
  refinements are recorded here before consumers depend on them.
- Existing live carrier: #685, still OPEN. The test-only decision sync endpoint
  is not a substitute. No live cross-machine acceptance has been run.
- #1135 parser fix is included in the basis; #1134 and #1136 remain separate
  review prerequisites. A peer is working on #1136: do not duplicate that work.

## Delivery sequence

- [x] S0: freeze contract, fixture and ownership details (`cf4d8af`).
- [x] S1: local capsule save/show/list/restore and legacy checkpoint projection
  (`8bda180` + corrections through `7ef7dfb`).
- [x] S2: portable repository identity and offline bundle round-trip (same
  accepted stack).
- [ ] S3: authenticated node delivery/acknowledgment, real two-machine proof.
- [x] S4: native skills and host projections with old-binary detection —
  continuity save/restore accepted through `b46d4bc`; task-prepare integrated
  with the accepted S5 stack.
- [x] S5: immutable trusted brief contracts and execution seams (`6e2db1a`);
  public authority remains deliberately unavailable until S6 supplies a seal.
- [~] S6: sealed manifest/state/token foundation accepted through `2ff1ac9`;
  external admission, dispatch, verification and merge adapters remain S6b.
- [ ] S7: Flash controller and single-writer manager cutover.
- [ ] S8: investigation recipe and same-model A/B proof.
- [ ] S9: runbook/router reduction after product parity.
- [ ] S10: frozen-head integrated acceptance and independent review.

After proof, derive child issues/stacked PRs from actual commits and ask for the
final carve/landing approval. Do not prematurely retire the old manager.

## Accepted local evidence

S1/S2 reached local Round 3 LGTM at source candidate
`6f862808a691e06b5ca03a8146bea05dd1a45229` (integration equivalents
`8bda180`, `2c0e0f2`, `7ef7dfb`), P0=0/P1=0. Task receipts #114, #117, #118,
#120, #121 and #122 preserve worker gates, all review rounds and responses.
No push or exact-head CI exists, so this is slice acceptance for continued
integration, not L1 or merge acceptance.

Round 1 proved that reusing `checkpoint` allowed imported data-only text into
the hot pack. Decision `continuity.v1-event=continuity_capsule` supersedes it;
legacy checkpoint events remain readable but the new event is opt-in data.

S5 reached local Round 3 LGTM at
`b2788dfd8e142fa8f2c17d13ddbabbacca080fe6`, P0=0/P1=0, after syncing main.
Tasks #130, #139–#141, #144, #145 and #149 preserve implementation, three
review rounds, SDK regeneration, and final acceptance. The integration merge is
`6e2db1a`. Public brief acceptance fails closed until S6 installs a product-
verifiable authority seal; this is truthful unavailability, not a complete
controller claim.

Nonblocking observations retained for final carve: remove the unused public
`portable_alias_contains` helper and report `omitted_items` when legacy rejected
hypotheses exceed the projection limit. They were visible before their review
round and do not retroactively extend S1/S2 acceptance.

## First executable milestone

A bounded capsule can be saved with exact read-back, selected in a new session,
exported, imported into a fresh store/clone, and restored without duplicate
imports or repository mutation. Dirty/detached/non-Git states are advisory.
Unknown schema, integrity conflicts and identified secrets refuse only the
requested operation. Local success and transport availability are separate.

Then demonstrate one compiled narrow investigation with a fixed receipt. Do not
call simulated dispatch or a prompt fixture a real Flash execution result.

## Verification and environment

Worker runs focused L0 on touched crates only; no workspace rebuild per round.
Frozen-head L1 is exact-head CI plus the verifier's focused Windows gap checks.
Review receipts pin full SHAs and distinguish RAN from READ. Until published,
local candidate evidence is not CI/merge acceptance.

Two real machines and usable runtime credentials must be checked before their
respective drills. Do not read or print credential values. Unavailable hosts or
providers do not stop independent local implementation.

## Safety

No new workflow-wide hook, required status check, clean-tree or worker-claim
prerequisite. Imported artifacts are data, not authority. Merge capability and
host-local process state never travel inside a continuity bundle. Preserve
unrelated changes and all unmerged worktrees/branches.

_Created from the approved conversation and program plan._
