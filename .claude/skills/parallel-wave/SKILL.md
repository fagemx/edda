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

Layer 1's input **is** the select-this-batch table from fleet-orchestrate's
ready-batch selection procedure — a batch never appears from nowhere. That
table already applied the exclusion checklist (cross-machine claims, in-flight
PRs/remote branches, `needs-operator`); this layer starts from its selected
rows and does not repeat those checks.

Ready-queue intake is a machine check, not memory (GH-665): source candidate
issues from `scripts/fleet/ready-queue-lint.sh`, never a raw
`gh issue list --label fleet:ready` — the script excludes open issues whose
delivery PR already merged, so a delivered issue still carrying `fleet:ready`
is never dispatched.

Derive each issue's predicted write surface (paths + symbols) from its scope
against the crate map. Pairwise intersect:

| Intersection | Verdict | Action |
|---|---|---|
| Disjoint paths | PARALLEL | dispatch together |
| Same file, different symbols | CONDITIONAL | both briefs get an explicit FORBIDDEN symbol list + "stop and report in PR body if you need them" |
| Same symbols / invariant / API order | SERIAL CHAIN | one bundle chain; later items stay blocked AND unassigned |
| Scope too vague to list a surface | NOT READY | back to queue for scope sharpening — an issue-quality gate, not a human-review gate |

## Layer 2 — in-flight containment

- Each active bundle has one lane-bound fixed worktree. The fixed pool is
  `worker-1`, `worker-2`, `verifier`, and `verifier-2`, at
  `<root>-wt-<lane>`; use `scripts/fleet/lane-prepare.ps1` to create it or
  switch to a new branch from `origin/main`. It refuses a busy or dirty lane,
  and refuses to switch until the previous branch's local tip matches the
  remote tip. It never force-checks out or removes a worktree, branch, or
  source.
- Use the canonical environment policy in [`.claude/CLAUDE.md` Build
  lanes](../../CLAUDE.md#build-lanes); `lane-warm.ps1 -PrintEnv` is the helper
  contract consumed by `lane-launch.ps1`. Do not duplicate that policy in wave
  plans or worktree prompts.
- Worktree prompts must name the prepared fixed worktree and current branch:
  do NOT `checkout main`, pull, create branches, remove branches, or create a
  second worktree. The controller prepares the next branch only after the lane
  is idle and its prior branch has been pushed.
- No verdict gates in parallel plans (`cleanup.review-gate=pr-not-verdict-gate`;
  gates assume an attached controller — wave1 timed out 2 of 3). This also makes
  the GH-543 worktree-ledger trap inapplicable.
- Record `edda claim --paths` per lane.
- Cross-machine claim before dispatch (GH-656): each bundle's claim is
  written by one command — `scripts/fleet-claim-issue.sh
  <issue> <machine>/<role>` with the explicit `<machine>/<role>` token
  from the lane brief (e.g. `4090/worker-1`, `docs/reviewer`), never a
  hostname guess. Exit 1: another lane's `taking:` comment or `lane:*`
  label — that lane does not dispatch. `edda dispatch --issue <N>
  --machine <machine>/<role>` is only a pre-spawn check that refuses with
  exit 2 if another machine already holds the issue; until #782 lands it
  writes no claim and does not substitute for the script.

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
