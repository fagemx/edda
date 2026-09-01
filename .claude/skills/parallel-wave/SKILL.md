---
name: parallel-wave
description: Use when two or more ready issues are about to be delivered serially, when authoring or restructuring a conduct wave plan, or when deciding whether concurrent implementation lanes are safe on this workstation (worktree, build lane, review dispatch, merge ordering).
---

# Parallel Wave

Turn a batch of ready issues into concurrent delivery lanes. Parallelism is the
**output of a judgment machine**, never a preference. Composes with
fleet-orchestrate (controller contract) — this skill decides *what runs
concurrently and how*; fleet-orchestrate governs roles, briefs, and review.

## Unit of parallelism

One issue = one bundle = one **single-phase** conduct plan = one worktree = one
build lane. Parallelism happens *between* plans (conductor already runs plans
concurrently), never inside one plan. Never write declarative `depends_on`
chains: a dependency edge must name a real reason (same symbols, same
invariant, API/schema order). Plan YAMLs live outside the repo (scratchpad or
`.tmp/plans/`) so agents cannot commit them.

## Layer 1 — static judgment (before dispatch)

Derive each issue's predicted write surface (paths + symbols) from its scope
against the crate map. Pairwise intersect:

| Intersection | Verdict | Action |
|---|---|---|
| Disjoint paths | PARALLEL | dispatch together |
| Same file, different symbols | CONDITIONAL | both briefs get an explicit FORBIDDEN symbol list + "stop and report in PR body if you need them" |
| Same symbols / invariant / API order | SERIAL CHAIN | one bundle chain; later items stay blocked AND unassigned |
| Scope too vague to list a surface | NOT READY | back to queue for scope sharpening — an issue-quality gate, not a human-review gate |

## Layer 2 — in-flight containment

- Worktree per bundle: `git worktree add <root>-wt-ghNNN -b <branch> origin/main`.
- Fixed lane pool only (`worker-1`, `worker-2`, `verifier`): set
  `CARGO_TARGET_DIR=<lane-root>/<lane>` for the whole lane lifetime. Never an
  ad-hoc build dir — that is the 194 GB failure.
- Worktree prompts must say: already on branch X, do NOT `checkout main`, do
  NOT pull, do NOT create branches (checkout of main fails in a worktree anyway).
- No verdict gates in parallel plans (`cleanup.review-gate=pr-not-verdict-gate`;
  gates assume an attached controller — wave1 timed out 2 of 3). This also makes
  the GH-543 worktree-ledger trap inapplicable.
- Record `edda claim --paths` per lane.

## Layer 3 — post-hoc net (before merge)

After PRs open, intersect **actual** surfaces: `gh pr diff <n> --name-only`,
pairwise. Overlap ⇒ the later-merging PR gets rebase + delta review. A wrong
Layer-1 call costs one review round, never a disaster — that is why Layer 1 may
be decided mechanically and boldly.

## Splitting a running serial plan

Order is mandatory: `edda conduct skip <phase> --reason "parallelized: <new plan>"`
for every phase being split out, **then** launch the parallel plans. Launch-first
duplicates the work when the serial runner advances.

## Review + merge

- Dispatch an independent reviewer per PR **on PR-open**, not after the batch.
  Reviewer READs green CI (know what CI covers per OS), RANs only uncovered
  checks, never builds in an occupied lane, reads code via
  `gh pr diff` / `git show origin/<branch>:<path>` — never the working tree.
- WIP law: `impl WIP = min(independent ready bundles, workers, reviewer capacity)`.
  This workstation: impl ≤ 3, concurrent reviews ≤ 2.
- Merge tail is serial. After each merge, split remaining PRs by Layer-3
  intersection with the merged diff: disjoint ⇒ merge on the old base, **no
  rebase, no re-review** (no push ⇒ verdict stands); intersecting ⇒ rebase ⇒
  that push voids the verdict ⇒ delta review round.

## Red flags — stop, you are rationalizing

| Thought | Reality |
|---|---|
| "Chain order feels safer" | A depends_on edge without a named reason is the serial-slowness root cause. |
| "Same file, but they probably won't conflict — just brief both" | That is CONDITIONAL: FORBIDDEN symbol lists, or serial. |
| "Review can wait until the batch is done" | PR-open is the dispatch event. Idle green PRs are the biggest observed waste. |
| "Fresh session, safer to build in its own dir" | Lanes are the isolation. Ad-hoc dirs are forbidden. |
| "Launch parallel lanes first, tidy the old plan later" | Skip-first, launch-second. The reverse duplicates work. |
| "Add a verdict gate so we keep control" | Gates assume an attached controller; review on the PR instead. |

Templates (single-phase plan YAML, reviewer brief): see
[templates.md](templates.md).
