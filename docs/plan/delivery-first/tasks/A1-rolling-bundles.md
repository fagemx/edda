# A1 — Cohesive bundles and rolling progress

> Owner: strong workflow implementer; retain this owner for A2/A3.
> Depends on: none. Unblocks: A2. Delivery: usable guidance change.
> Contract: DF-01, DF-02, DF-09 in [CONTRACT](../CONTRACT.md).

## Read first / basis

```bash
git rev-parse origin/main
edda task list
```

Read `.claude/skills/issue-pipeline/SKILL.md`, both tracked coord-orchestrate sources,
`docs/guides/operator-runbook.md`, and [SPEC](../SPEC.md) sections 3–5.
Use the exact base's current CI as READ, not a fresh workspace rebuild.

## Scope

Write only pipeline, the project coord skill, embedded coord skill, relevant runbook
routing paragraphs, and new `scripts/test-delivery-guidance.sh` (offline test, not wrapper).
No task/dispatch/control code, no workflow/hook changes, no peer program plan edits.

## Implementation

1. Replace per-phase all-agent wait with bundle-specific prerequisite progression.
   A candidate may enter review while unrelated research continues. Limited reviewer
   capacity queues only the candidate, not all implementation.
2. Teach issue/task/PR separation and cohesive chain ownership. Preserve normal small-task
   path; a multi-step feature does not require a new issue or session per step.
3. Preserve actual source ownership and existing cross-machine coordination. Do not remove
   existing claim safety while removing phase waits; do not add claims to ordinary tasks.
4. Replace pipeline Phase 4 direct merge command with canonical product check/merge path,
   controller-only under repository standing R6; never self-merge by worker/reviewer.
   Generic shipped skill continues to defer to the host repository's actual policy.
5. Correct build setup to assign separate allowed lanes only to sessions that compile;
   docs-only controllers must not require CARGO_TARGET_DIR just to start. In BOTH tracked
   coord-orchestrate sources, remove implementer instructions to run the full local gate
   set on every frozen SHA. State the canonical ladder explicitly: author focused L0;
   L1 exact-head CI; verifier runs only uncovered focused checks including Windows C5.
   A freeze does not itself authorize a full local workspace rerun.
6. Add offline static guidance fixture. Check active procedural sections for forbidden
   blanket phase waits/direct merge/full-local-freeze instructions and for explicit
   prerequisite progression plus author-L0/CI-L1/verifier-gap ownership. Fixture
   should accept quoted historical bad examples without mistaking them for instructions.
   Include a small A-ready/B-waiting/C-depends-A trace per V1. Label it a design/static
   fixture, not proof that a live LLM obeys the guidance.

## Acceptance / commands

```bash
sh scripts/test-delivery-guidance.sh
sh scripts/lint-markdown-content.sh
sh scripts/lint-doc-citations.sh --tree
git diff --check
```

New test is implemented by this card; not available before it. V1 requires A to proceed
before B and C to wait only for A. Same-file incompatible writes remain serialized.
Run focused `cargo test -p edda` and `cargo clippy -p edda --all-targets -- -D warnings`
in the assigned lane because embedded skill content is a product blob; follow current L0
format/file-length checks. Read exact-head CI at freeze, not another full local workspace run.

## Delivery / rollback

Conventional intent: `refactor(fleet): advance cohesive bundles without phase barriers`.
May continue directly to A2 with same context before review; no forced task/PR split.
If rolling guidance regresses ownership, fix/revert that guidance in a new commit without
restoring direct unpinned merge or touching peer sources. Report actual cost, not estimates.
