---
name: pr-review-loop
description: Bounded author self-check and fixes before independent current-head review; never a verdict or merge loop
context: fork
---

# PR Author Self-Check Route

This compatibility entry is for the author or a designated fixer. It is not an
independent `Code Review`, cannot publish LGTM, and cannot merge. Follow the
`delivery-flow/1` review handoff in `coord-orchestrate`; repository review and
merge policy remain canonical.

## Bounded route

1. Resolve the PR from the explicit argument, or from the current branch when
   the host supports that lookup. Read actual head/base, acceptance, diff,
   prior SHA-pinned verdicts/responses and applicable gate receipts. Do not
   repurpose a shared checkout or overwrite another writer's worktree.
2. Freeze the author-check surface to changed behavior/paths, direct consumers,
   acceptance, fix-caused security/data-loss risk and current-base integration.
3. Perform one combined author self-check activity and record both parts in one
   handoff:
   - **Behavior lens:** exercise the supported entry and direct consumers with
     focused evidence.
   - **Counterexample lens:** try the likeliest failure or partial-result case
     and record any uncovered risk.
4. Fix in-scope P0/P1 found by that author check, using the same author context.
   Run the repository's focused checks while iterating. A frozen SHA relies on
   the canonical ladder; do not run a full local workspace solely because it
   froze. Commit/push only when this invocation already has that authority.
5. Hand the new full SHA and RAN/READ evidence to an independent reviewer. Every
   push invalidates the old verdict. Review-requested fixes return to an
   author/fixer, then the independent reviewer resumes or is explicitly
   replaced; the fixer never becomes the judge.

A clean author check means only "ready to request independent review." It does
not mean accepted, CI-green or mergeable. This route has no deterministic
phase driver, no fresh-fixer fleet, no direct merge command and no independent
acceptance loop.
