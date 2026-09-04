---
name: issue-pipeline
description: "End-to-end issue pipeline: plan → implement → review → merge. Dispatches parallel sub-agents with worktree isolation. Usage: /issue-pipeline 566 567 568 [--skip-plan] [--no-merge]"
---

# Issue Pipeline Skill

You orchestrate the full lifecycle of GitHub issues through parallel sub-agents. Each phase runs all issues concurrently in isolated worktrees.

## Usage

```
/issue-pipeline <issue-numbers...> [flags]
```

**Flags:**
- `--skip-plan` — Skip plan phase, go straight to implement (issues already have plans)
- `--no-merge` — Stop after review, don't auto-merge

There is deliberately no `--skip-review` flag: review can never be skipped, and work
that fails or lacks review never merges — fixes go through a different `issue-action`
sub-agent and a fresh house-review round (`sh scripts/review-pr.sh`), never through
`pr-review-loop`, which is author self-check and never the Phase 3/4 judge.

## Before you start (controller session setup)

The controller session runs in a normal shell, not a scheduler lane — set the
environment before opening it, then run the pipeline from the controller prompt:

```powershell
# 1. Build lane — quoted assignment; allowed values: worker-1 | worker-2 | verifier | verifier-2.
#    Never create an ad-hoc CARGO_TARGET_DIR (decision verification.cost-discipline).
#    Lane root: $env:FLEET_LANE_ROOT if set, else $env:LOCALAPPDATA\fleet-workstation\lanes.
$laneRoot = if ($env:FLEET_LANE_ROOT) { $env:FLEET_LANE_ROOT } else { "$env:LOCALAPPDATA\fleet-workstation\lanes" }
$env:CARGO_TARGET_DIR = "$laneRoot\worker-1"

# 2. Machine label — appears in the taking: comment each sub-agent posts (Phase 2)
$env:EDDA_SESSION_LABEL = "docs"

# 3. Open the controller session in this checkout, then:
#    /issue-pipeline 703 704 --skip-plan --no-merge
```

Sub-agents inherit these values. This path is for work done while the operator is
present; for long unattended work use the Task Scheduler lanes instead (decision
`fleet.parallel-modes`).

## Prerequisites

This skill depends on these sub-skills being installed in `.claude/skills/`:
- `issue-plan` — Deep-dive planning (research → innovate → plan)
- `issue-action` — Implementation from plan to PR; also the fix vehicle when house
  review requests changes (Phase 3)

If any are missing, inform the user — both ship in this repo's `.claude/skills/`.

## Pipeline Phases

### Phase 1: Plan (parallel)

For each issue, launch a sub-agent with `isolation: "worktree"` and `run_in_background: true`:

```
prompt: |
  You are working on GitHub issue #{number} for this project at {project_root}.
  Load and execute the issue-plan skill by reading {project_root}/.claude/skills/issue-plan/SKILL.md
  and following its instructions for issue #{number}.
  IMPORTANT: You are in a worktree. Do NOT modify source code — planning only.
```

**Wait for all agents to complete.** Report results to user as a table.

### Phase 2: Implement (parallel)

For each issue, launch a sub-agent with `isolation: "worktree"` and `run_in_background: true`:

```
prompt: |
  You are working on GitHub issue #{number} for this project at {project_root}.
  Load and execute the issue-action skill by reading {project_root}/.claude/skills/issue-action/SKILL.md
  and following its instructions for issue #{number}.
  The plan has been posted as a comment on the issue — read it with `gh issue view {number} --comments`.
  IMPORTANT: You are in a worktree. Do NOT run `git pull origin main` — never pull main
  into a worktree.
  BEFORE any other work, check whether the issue is already claimed across machines —
  check first, and write nothing until the check is done. Read the issue's existing
  `taking:` comments and `lane:*` labels (`gh issue view {number} --comments`,
  `gh issue view {number} --json labels`), where `{machine}` is this controller's
  machine label (`EDDA_SESSION_LABEL`). Compare WORKSTATIONS, not raw tokens: strip a
  trailing `/pipeline` or `/role` from each claim token before comparing, so
  `{machine}` and `{machine}/pipeline` are the same workstation. You may run
  `sh scripts/fleet-claim-issue.sh --check {number} {machine}` as a read-only extra,
  but never let it decide: it compares the token after `taking:` verbatim, so it
  treats `{machine}` and `{machine}/pipeline` as different machines.
  If another workstation holds the issue, do NOT comment and do NOT start — skip and
  report it. If this same workstation already holds it, do not write a second claim
  comment; report the existing claim.
  Only if the issue is unclaimed, claim it with EXACTLY one command and no other
  writer — `gh issue comment {number} --body "taking: {machine}/pipeline"` — then
  load the issue-action skill and implement. Do NOT claim via
  `sh scripts/fleet-claim-issue.sh {number} {machine}`: it writes
  `taking: {machine} at {now}` (no `/pipeline`), and verbatim token comparison reads
  that as a different workstation.
```

**Wait for all agents to complete.** Collect PR URLs. Report results to user.

### Phase 3: House Review (parallel)

The reviewer does not fix the PR it judges. This is **house review**, not the
author's review-and-fix loop: fixes are made by a different sub-agent (step 3),
never by the reviewer.

For each PR created in Phase 2, run one house review. The controller may launch all
reviewers in a SINGLE message:

1. Launch the house review with the real launcher, which creates the detached PR-head
   worktree, copies the review spec into it, and dispatches the reviewer with a pinned
   model and `--exclude-tools Edit,Write,NotebookEdit` (decision `fleet.review-backend`):

   ```bash
   # Round 1 — the one-argument form (round defaults to 1, no previous SHA):
   sh scripts/review-pr.sh {pr_number}
   ```

   (`--dry-run` only prints what would launch; it does not review anything.)

2. The reviewer reads the brief, runs read-only checks, and posts exactly one verdict
   comment pinned to the full reviewed SHA (decision
   `fleet.review-protocol`); any push to the PR invalidates the verdict and starts
   a new round.
3. **On Changes Requested:** dispatch a fresh fix sub-agent that loads the
   `issue-action` skill and addresses the blocking findings on the PR's branch.
   The fix sub-agent is never the reviewer. After it pushes, launch the next
   house-review round on the new full SHA. `review-pr.sh` usage is
   `review-pr.sh <PR> [round] [prev-sha]` and ROUND defaults to 1, so the two
   invocations are:

   ```bash
   # First review of a PR — one argument; round defaults to 1:
   sh scripts/review-pr.sh {pr_number}
   # Every later round, after a fix push — pass the next round number and the
   # previous full SHA (the SHA the last verdict pinned). Round 2 of PR 834:
   sh scripts/review-pr.sh 834 2 eab0db42b628ce0df44894af31afe183ddebeee4
   ```

   Repeating the one-argument command does NOT start the next round — it runs
   Round 1 again. `{prev_full_sha}` is always the full SHA the previous round's
   verdict pinned; every push invalidates that verdict.

**Wait for all reviewers to complete.** Report verdicts.

### Phase 4: Merge

**Merge preconditions** (`pr.merge-policy`): a PR may be merged only when all of the
following hold —
- A final **current-head** LGTM from review (Phase 3, latest pushed SHA)
- P0 = 0 and P1 = 0 blocking findings
- All required checks green

For each PR meeting every precondition:

```bash
gh pr merge {pr_number} --squash
```

Pass `--delete-branch` only when the PR is being merged — per
`fleet.merged-artifact-cleanup`, a merged PR's branch and lane worktree may be
reclaimed (squash commit on main, GitHub keeps `refs/pull/N/head`); anything
unmerged — open or closed-unmerged branches, worktrees with uncommitted work,
another session's active branch or worktree, and sources — stays untouched (see
`.claude/CLAUDE.md`). If a PR does not meet the preconditions, do not merge it:
dispatch a fix sub-agent (a **different** `issue-action` sub-agent — never the
reviewer; `pr-review-loop` is author self-check, never the Phase 4 judge) for the
blocking findings, then run a fresh house-review round (`sh scripts/review-pr.sh`) on
the new full SHA, and report that the PR is blocked until the preconditions hold.

Report final status table.

## Execution Rules

1. **All sub-agents use `isolation: "worktree"`** — never work on main directly
2. **All sub-agents use `run_in_background: true`** — maximize parallelism
3. **Launch all agents for a phase in a SINGLE message** (one tool call per agent, all in parallel)
4. **Wait for ALL agents in a phase before starting the next phase**
5. **Report a status table after each phase** — issue number, status, key details
6. **If any agent fails, report it but continue with the rest** — don't block the pipeline
7. **Never delete anything unmerged** — per `fleet.merged-artifact-cleanup`,
   only the branch and lane worktree of a **merged** PR may be reclaimed (the
   squash commit is on main and GitHub keeps `refs/pull/N/head`); anything
   unmerged — open or closed-unmerged branches, worktrees with uncommitted
   work, another session's active branch or worktree, and sources — stays
   untouched; `git worktree remove` and `git worktree prune` are for that
   reclaim only; otherwise leave cleanup to the operator
8. **At most two compile-needed issues per invocation** — the fleet has two
   worker build lanes (`worker-1`, `worker-2`), so at most two issues that need
   to compile run per invocation. Docs-only issues do not consume a lane.
9. **Set the build lane before start** — the controller session must have
   `CARGO_TARGET_DIR` set to an allowed lane (`worker-1|worker-2|verifier|verifier-2`)
   before dispatching; sub-agents inherit it and never create an ad-hoc target
   directory (decision `verification.cost-discipline`, `.claude/CLAUDE.md` → Build lanes).
10. **Sub-agents die with the session** — every sub-agent this skill dispatches is
   a child of the controller session and dies with it, so this in-session pipeline
   is not for long unattended work. When the operator will step away, use the
   Task Scheduler lanes instead (decision `fleet.parallel-modes`).

## Status Table Format

After each phase:

```
| Issue | Phase | Status | Details |
|-------|-------|--------|---------|
| #566  | plan  | ✅     | Plan posted to issue |
| #567  | plan  | ✅     | Plan posted to issue |
| #568  | plan  | ❌     | Agent error: ... |
```

## Error Handling

- **Agent fails**: Mark as ❌, continue with other issues. Report at end.
- **PR creation fails**: Try to identify branch, suggest manual fix.
- **Review requests changes**: a fix sub-agent (`issue-action`) addresses the blocking
  findings, then a new house-review round runs on the new full SHA.
- **Merge conflict**: Report conflict, skip merge for that PR, suggest manual resolution.
