# pi controller runbook — the two-day window loop (GH-886)

Audience: a pi session acting as fleet controller with no skills loaded. Every
step is one command with its expected output; every STOP condition routes to
`needs-operator` and ends the controller's turn. Decisions in force:
`fleet.pi-lanes`, `fleet.lane-launch`, `review.verdict-carrier`,
`review.independence-policy`, `fleet.all-flash-window-2026-09-05`.

The loop is two scripts plus the guard scripts they call:

- `scripts/fleet/next-issue.sh <issue> <machine>/<role> [--dry-run]` — ready
  issue → launched lane
- `scripts/fleet/next-review.sh <pr> [--shadow] [--operator-granted] [--dry-run]`
  — open PR → posted review comment

## Pick the next issue

```sh
sh scripts/fleet/ready-queue-lint.sh
```

Expected: the ready queue, one `#<issue> <title>` per line, exit 0. A nonzero
exit means the queue itself is malformed — STOP `needs-operator`.

Pick the oldest issue in the printed queue that you have not already handed to
a lane this shift.

## Preview the launch (always first)

```sh
sh scripts/fleet/next-issue.sh <issue> docs/worker-1 --dry-run
```

Expected, in order: the lint result; `branch: <type>/gh<issue>-<slug>` and
`worktree: <path>`; the `edda task new` line; the brief path; the claim
command; the launch command; `lane name`, `log`, and `done marker` lines; and
`== dry-run: nothing created, claimed, or launched`, exit 0. A claimed issue
exits 2 naming the claimant — pick another issue.

## Launch the lane

```sh
sh scripts/fleet/next-issue.sh <issue> docs/worker-1
```

First run renders `~/.edda/fleet/brief-gh<issue>.md` and exits 2 with
`brief still contains <<AUTHORED STEPS>> — fill the authored middle in
<path>, then rerun`. Fill the marker with the issue's implementation steps
(from the issue body's authored surface; never copy its doneWhen into the
brief), then run the same command again: it keeps the authored brief, creates
the worktree, runs `edda task new` and `fleet-claim-issue.sh`, and launches
`edda-lane-gh<issue>` through `scripts/fleet/lane-launch.ps1`
(`-TimeoutSec 5400 -BudgetUsd 3`). Expected at the end:
`lane launched: edda-lane-gh<issue>`.

Lanes only ever start through `next-issue.sh` or `lane-launch.ps1` — never an
ad-hoc terminal pi session for unattended work.

## Watch the lane

```sh
pwsh -NoProfile -File scripts/fleet/lane-status.ps1
```

Expected: one row per lane with its state. The done marker is
`%TEMP%\edda-lanes\<lane>.done`; the log is the same path with `.log`. The
last five lines of the log are the lane's report.

- `DONE issue=#<issue> task=<id>` + PR URL + SHA — go to the review step.
- `STOP step=<n> output=<...>` — read the five lines, fix the brief
  (`~/.edda/fleet/brief-gh<issue>.md`) so the failing step cannot recur, and
  relaunch: delete the stale marker only if the lane is not running, and start
  the next round with the `-r<n+1>` lane name (next-issue.sh picks the next
  free suffix automatically). Do not continue a stopped lane's session.
- Round cap: three rounds without a PR → STOP `needs-operator`.

## Review the PR

```sh
sh scripts/fleet/next-review.sh <pr> --shadow --dry-run
```

Expected: the head SHA, the round count, the brief command, the engine
command, the post command, `== dry-run: nothing launched or posted`, exit 0.

Without `--shadow` the script refuses to delegate until `review-pr.sh`
carries the pi arm (GH-880, PR #890): its delegation would otherwise call the
Claude backend, which doneWhen item 7 forbids for both scripts.

```sh
sh scripts/fleet/next-review.sh <pr> --shadow
```

Expected: `posted SHADOW round <n>` and a `## Code Review: Round <n> — PR
#<pr> @ <sha> (SHADOW)` comment on the PR. The shadow round never sets
`fleet:reviewed`, never sets the Independent Review status, and never merges
(`review.gh880-shadow`). A fourth round without `--operator-granted` exits 2
and labels `needs-operator`.

Window merge rule (`fleet.all-flash-window-2026-09-05`, operator may veto on
#888): code-risk PRs are not merged in the window; docs/skills-class PRs may
be squash-merged only on glm LGTM P0=0 P1=0 + CI green on that head + no other
verdict on the SHA. Anything else waits for the operator.

## STOP conditions — label `needs-operator` and stop

- dirty worktree or claim conflict on a lane launch
- CI red on a reviewed head
- round cap (three rounds without a delivered PR, or a fourth review round
  without `--operator-granted`)
- a `[判斷]` escalation on a code-risk PR that the controller cannot adjudicate
- any operator-only action: merging a code-risk PR, `edda ratify`, ruleset
  changes, force push, deleting unmerged branches
- known lane failure modes: a lane that delivers nothing before timeout
  (#748), an orphaned scheduled task (#772) — stop the lane with
  `scripts/fleet/lane-stop.ps1 -Name <lane>` before relaunching

## Reporting

1. The operator-facing report is `sh scripts/fleet/daily-digest.sh --board 888`, never a hand-written summary — a hand-written one omits open PRs and misstates states (GH-914).
2. Before calling any PR complete, compare `gh pr view <n> --json headRefOid --jq .headRefOid` against the SHA in its newest `Code Review: Round` comment; a different SHA means that head has not been reviewed — not complete (GH-914).
3. `mergeStateStatus` is not a readiness signal: a PR based on a branch other than `main` is not gated by the ruleset, so `CLEAN` with zero verdicts is exactly what it looks like (GH-914).


## Pointers

- Brief shape and the authored-middle contract: `docs/guides/brief-template.md`
- Review rules: `REVIEW.md` (read at the base SHA, never the head)
- Observation window and the metrics compared on return: #888
