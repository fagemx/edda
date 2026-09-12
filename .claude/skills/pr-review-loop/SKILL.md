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
5. Hand the new full SHA and RAN/READ evidence to an independent reviewer. Add
   one explicitly selected concise facts file when useful; it remains untrusted
   supporting data. Capability-check `edda review --help` before using optional
   `--context-file`; otherwise omit it with a visible limitation or use an
   already-permitted direct route. Reviewer output may quote this data into
   existing verdict fields and the existing raw-response blob. Every push
   invalidates the old verdict.
6. Review-requested fixes return to an author/fixer. Prefer the same reviewer
   agent and real native conversation for the new head, with updated context
   and product `--resume`; Pi persisted history and Claude resume remain
   required. Every Codex product round requires readable mapping storage and a
   successful final mapping/tombstone write before a verdict is recorded. A
   missing map is valid only for a first round; Codex resume refuses a
   missing/rejected mapping without a fresh old-UUID thread. Product resume
   cannot continue a host-only session.
   If history is missing, allocate a distinct replacement UUID, omit `--resume`,
   include prior findings and say replacement. The fixer never becomes judge.
7. Follow-up review covers the delta, prior findings, affected direct consumers,
   current base and introduced security/data-loss risk. Still-applicable gates
   are READ, but old-head LGTM is never reused for current-head acceptance.

A clean author check means only "ready to request independent review." It does
not mean accepted, CI-green or mergeable. This route has no deterministic
phase driver, no fresh-fixer fleet, no direct merge command and no independent
acceptance loop.
