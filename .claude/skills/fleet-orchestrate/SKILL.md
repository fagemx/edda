---
name: fleet-orchestrate
description: Use when a controller must coordinate two or more agent sessions for parallel review, issue discovery, implementation, verification, dependency conflicts, or unattended progress that must survive context loss or session replacement.
---

# Fleet Orchestrate

## Overview

Run a fleet as a durable control system, not a group chat. Divide context and
ownership only where work is genuinely independent; keep decisions, evidence,
and state recoverable when any session disappears.

This policy applies when `fleet-orchestrate` or its coord review/orchestration
companions are invoked. It guides those sessions; it is not an Edda runtime
rule imposed on every project.

**REQUIRED REFERENCE:** Read [references/playbook.md](references/playbook.md)
completely before assigning work. Every target session starts with zero
conversation context.

**REQUIRED PRESSURE TESTS:** Use
[references/review-scope-pressure-tests.md](references/review-scope-pressure-tests.md)
when changing or validating review policy.

## Controller sequence

1. Fix the goal, exclusions, evidence bar, issue-filing authority, merge
   authority, and stop conditions. “Keep going” does not expand scope.
2. Inspect repository instructions, current revision, dirty state, active
   peers, claims, decisions, issue/PR state, and available durable carriers.
3. Select this batch before any dispatch: run the ready-batch selection
   procedure below and hand its table to `/parallel-wave`. `fleet:ready` is
   the operator's authorization — the controller picks the batch itself and
   never stops to ask the operator for issue numbers.
4. Maintain two logical rails: read-only review/discovery and write-enabled
   delivery. A verified finding may cross rails; a suspicion may not.
5. Build a dependency and file-overlap graph. Parallelize only independent
   ready bundles; serialize the same code chain. Set worker WIP no higher than
   verifier capacity.
6. Keep one controller, independent workers, and a read-only verifier. From
   two concurrent workers onward, reserve a verifier seat.
7. Record tasks, claims, rulings, and acceptance criteria in the durable truth
   layer before ringing host messaging as a doorbell.
8. Gate every cross-machine dispatch mechanically (GH-782): before a lane
   starts an issue, use one claim command —
   `scripts/fleet-claim-issue.sh <issue> <machine>/<role>` (e.g.
   `4090/worker-1`, `docs/reviewer`) or `edda dispatch --issue <N> --machine
   <machine>/<role>`. Each checks PRs and full `taking:` identities, then
   writes the `taking:` comment, adds `fleet:claimed`, removes `fleet:ready`,
   and assigns `@me` before the lane starts. `lane:*` labels are routing only,
   never ownership. The token is an explicit `<machine>/<role>` value from the
   brief, never a hostname guess.
9. Give self-contained briefs (including assigned build lane, verification
   budget, and cleanup authority), monitor compressed state, adjudicate
   conflicts, and close only against immutable full SHAs with proportional
   evidence.

## Ready-batch selection (select this batch)

`fleet:ready` is the operator's signature: promoting an issue to ready already
authorized it. When the operator says "run ready work today"（「今天開始跑」）, the
controller runs this procedure and produces the batch itself — it never stops
to ask for issue numbers. This step **selects**; the actual claim write uses
one command from sequence step 8 (the claim script or issue-aware dispatch) —
selection does not claim.

Run it verbatim:

1. **Query candidates** — the machine check is the filter of record (GH-665);
   never a raw label listing as the only filter. The second query adds
   routing fields; the third reads each candidate in full — the body and
   comments carry the predicted write surface, `blocked-by` edges, and other
   machines' `taking:` claims, so the rules below are applied from fetched
   data, never from titles alone:

   ```bash
   sh scripts/fleet/ready-queue-lint.sh    # pickable ready issues, oldest first
   gh issue list --label fleet:ready --state open --json number,title,createdAt,labels
   gh issue view <n> --json body,comments,labels,title,createdAt   # per candidate <n>
   ```

2. **Exclude**, recording each issue's reason for the output table:
   - (a) another lane already holds it: a `taking:` comment with a different
     full `<machine>/<role>` identity. `lane:*` labels are routing only and do
     not exclude an issue; a different role on the same machine is still a
     different claimant.
   - (b) already in flight: an open PR — list open `headRefName`s
     (`gh pr list --state open --json headRefName`) and treat any branch
     name containing `gh<n>` as in flight (the repo's convention is
     `<type>/gh<n>-…`; a `head:gh<n>` search misses it) — **or** a remote
     branch (`git ls-remote --heads origin "*gh<n>*"`) — pushed-but-unopened
     branches are invisible to `gh pr list`.
   - (c) labeled `needs-operator`.

3. **Route** by predicted write surface: anything touching `crates/**`,
   `.github/workflows/**`, `scripts/**`, or `Cargo.*` needs a build lane →
   the 4090-shaped machine. Pure `docs/**`, `*.md`, or `.claude/skills/**`
   work stays on this docs machine.

4. **Sort** oldest `createdAt` first, unless an issue is blocked-by another
   selected issue — blocked items keep their dependency order and stay
   unassigned until their blocker dispatches.

5. **Output** one table — selected / excluded with reason — and hand it to
   `/parallel-wave` as its Layer-1 input. Do not restate REVIEW.md tables
   here; review routing lives in the review charter.

## Non-negotiable invariants

- Ledger/issue first; message second.
- Never mark overlapping write bundles active together; keep every later
  bundle blocked and unassigned until ownership is released.
- Highest durable `d-NNN` ruling wins; changes say `SUPERSEDES d-NNN`.
- Reviewer/verifier does not fix the artifact it judges.
- Review verdicts bind one full SHA; any push voids the verdict.
- Every handoff freezes `IN SCOPE`: changed behavior/paths, direct
  callers/consumers, issue/spec acceptance, introduced or exposed
  security/data-loss regressions, and current-base integration. Everything
  adjacent, pre-existing, or speculative is an evidenced `FOLLOW-UP ISSUE`
  unless it invalidates that frozen surface.
- This is a bounded complete review, never a minimal review. Audit the entire
  frozen surface; every failure there is mandatory, and only findings
  genuinely outside it qualify for follow-up.
- Finish the whole scoped audit and batch all blocking P0/P1 before requesting
  changes. A later blocker must be fix-caused or previously unobservable.
- The issue/spec is the acceptance ceiling. Require extra evidence only when
  needed to prove a required fact or safety boundary.
- Gates follow code/product-blob, base, and toolchain changes. Docs/evidence-
  only heads reuse still-applicable code gates as `READ` and run relevant
  delta checks plus exact-head CI as `RAN`.
- Record available elapsed/token/tool cost. After two consecutive
  non-product/harness-only cycles without useful progress, or at diminishing
  returns, stop and route follow-up or request operator scope expansion.
- Verify once per frozen artifact: the full gate set runs once per frozen
  full SHA in an assigned build lane and leaves a gate receipt (SHA, gate set,
  toolchain, lane, result); reviewers READ that receipt and exact-head CI and
  RAN only what they do not cover, stating the reason for any full rerun. A
  status, label, or draft flip is not a push.
- Know what the project's CI actually covers before citing it as independent
  evidence; a real coverage gap is a legitimate reason to RAN. Deterministically
  red CI already blocks the artifact — audit and request changes instead of
  spending a full run; re-run only the failed job when the red is environmental.
- Build environments are bounded: one assigned lane per session for its
  lifetime from a fixed pool named in the brief, never per round, SHA, or
  timestamp; lane build cache is disposable, worktrees and sources are not;
  over the pool ceiling the fleet stops and reports instead of building.
- A GitHub PR carries its visible review-fix loop: numbered SHA-pinned review,
  implementer response to every requested change, and final current-head LGTM.
  An internal verifier report does not replace these PR comments.
- Worker receipts prove execution, not acceptance or merge authority.
- Session death is recoverable from the board, task rail, claims, and Git.

## Related skills

When present, use repository `coord-sync` before editing, `coord-request` when
crossing ownership, `coord-review` before integration, and `coord-handoff` when
releasing work. They refine peer behavior; this skill remains the fleet-level
controller contract.

## Minimum controller output

Publish and keep current:

1. authority contract;
2. review charter and finding queue;
3. dependency graph and bundle ownership;
4. fleet board with task, owner, branch, full SHA, state, review, build lane,
   and next step;
5. acceptance matrix, review handoff (`IN SCOPE`, `FOLLOW-UP ISSUE`, blocking
   P0/P1, `RAN`/`READ`, lane, gate receipt, exact SHA, cost), PR review-loop
   state, and final merge recommendation with current-head P0=0/P1=0.
