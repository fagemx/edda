---
name: fleet-orchestrate
description: Use when a controller must coordinate two or more agent sessions for parallel review, issue discovery, implementation, verification, dependency conflicts, or unattended progress that must survive context loss or session replacement.
---

# Fleet Orchestrate

## Overview

Run a fleet as a durable control system, not a group chat. Divide context and
ownership only where work is genuinely independent; keep decisions, evidence,
and state recoverable when any session disappears.

**REQUIRED REFERENCE:** Read [references/playbook.md](references/playbook.md)
completely before assigning work. Every target session starts with zero
conversation context.

## Controller sequence

1. Fix the goal, exclusions, evidence bar, issue-filing authority, merge
   authority, and stop conditions. “Keep going” does not expand scope.
2. Inspect repository instructions, current revision, dirty state, active
   peers, claims, decisions, issue/PR state, and available durable carriers.
3. Maintain two logical rails: read-only review/discovery and write-enabled
   delivery. A verified finding may cross rails; a suspicion may not.
4. Build a dependency and file-overlap graph. Parallelize only independent
   ready bundles; serialize the same code chain. Set worker WIP no higher than
   verifier capacity.
5. Keep one controller, independent workers, and a read-only verifier. From
   two concurrent workers onward, reserve a verifier seat.
6. Record tasks, claims, rulings, and acceptance criteria in the durable truth
   layer before ringing host messaging as a doorbell.
7. Give self-contained briefs, monitor compressed state, adjudicate conflicts,
   and close only against immutable full SHAs with rerun evidence.

## Non-negotiable invariants

- Ledger/issue first; message second.
- Never mark overlapping write bundles active together; keep every later
  bundle blocked and unassigned until ownership is released.
- Highest durable `d-NNN` ruling wins; changes say `SUPERSEDES d-NNN`.
- Reviewer/verifier does not fix the artifact it judges.
- Review verdicts bind one full SHA; any push voids the verdict.
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
4. fleet board with task, owner, branch, full SHA, state, review, and next step;
5. acceptance matrix and final merge recommendation.
