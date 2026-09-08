# pi controller runbook — the two-day window loop (GH-886)

Audience: a pi session acting as fleet controller with no skills loaded. Every
step is one command with its expected output; every STOP condition routes to
`needs-operator` and ends the controller's turn. Decisions in force:
`fleet.pi-lanes`, `fleet.lane-launch`, `review.verdict-carrier`,
`review.independence-policy`, `fleet.all-flash-window-2026-09-05`.

The loop is one script plus the guard scripts it calls:

- `scripts/fleet/next-issue.sh <issue> <machine>/<role> [--dry-run]` — ready
  issue → launched lane

The review half was `scripts/fleet/next-review.sh`, retired with the review
shell (GH-1061). A pi controller does not run review rounds: see below.

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
- `STOP step=<n> output=<...>` — route by controller engine. An authoritative
  engine (Opus/sol) reads the five lines, fixes the brief
  (`~/.edda/fleet/brief-gh<issue>.md`) so the failing step cannot recur, and
  relaunches: delete the stale marker only if the lane is not running, and
  start the next round with the `-r<n+1>` lane name (next-issue.sh picks the
  next free suffix automatically). Do not continue a stopped lane's session.
  A flash-level controller never modifies the brief: the 09-05 window measured
  five STOPs — all brief-authoring defects, zero engine misjudgments — so
  brief authoring is not a flash function; label `needs-operator` and stop
  (GH-933).
- Round cap: three rounds without a PR → STOP `needs-operator`.

## Review the PR

**A pi controller does not run the round.** `next-review.sh` delegated it, and
it refused to delegate anything but a SHADOW round anyway, because pi cannot
reach Anthropic and `fleet.review-engine-model` puts review on Opus via Claude
Code. It was retired with the review shell (GH-1061).

Stop at `DONE` and hand the PR to an authoritative engine, which runs the
on-demand round itself — see `docs/guides/operator-runbook.md` step 5. A
SHADOW round remains calibration evidence, never a verdict
(`review.gh880-shadow`): it sets no `fleet:reviewed`, no Independent Review
status, and merges nothing.

Window merge rule (`fleet.all-flash-window-2026-09-05`, operator may veto on
#888): code-risk PRs are not merged in the window; docs/skills-class PRs may
be squash-merged only on glm LGTM P0=0 P1=0 + CI green on that head + no other
verdict on the SHA. Anything else waits for the operator.

## STOP conditions — label `needs-operator` and stop

- dirty worktree or claim conflict on a lane launch
- CI red on a reviewed head
- round cap (three rounds without a delivered PR)
- a `[判斷]` escalation on a code-risk PR that the controller cannot adjudicate
- a flash-level controller's `STOP step=<n>` on a lane — brief authoring is
  not a flash function; label `needs-operator`, do not fix the brief (GH-933)
- any operator-only action: merging a code-risk PR, `edda ratify`, ruleset
  changes, force push, deleting unmerged branches
- known lane failure modes: a lane that delivers nothing before timeout
  (#748), an orphaned scheduled task (#772) — stop the lane with
  `scripts/fleet/lane-stop.ps1 -Name <lane>` before relaunching

## Reporting

1. The operator-facing report is `sh scripts/fleet/daily-digest.sh --board 888`, never a hand-written summary — a hand-written one omits open PRs and misstates states (GH-914).
2. Before calling any PR complete, compare `gh pr view <n> --json headRefOid --jq .headRefOid` against the SHA in its newest `Code Review: Round` comment; a different SHA means that head has not been reviewed — not complete (GH-914).
3. `mergeStateStatus` is not a readiness signal: a PR based on a branch other than `main` is not gated by the ruleset, so `CLEAN` with zero verdicts is exactly what it looks like (GH-914).


## 回報

- 給操作者的報告是 `sh scripts/fleet/daily-digest.sh --board 888` 的輸出，不是手寫摘要（rules.md R24）。
- 宣稱任何一張 PR 完成之前先跑 `sh scripts/fleet/verdict-drift.sh`；它 exit 1 就沒有一張 PR 可以被稱作完成。
- `mergeStateStatus` 不是就緒訊號：ruleset 只保護 `main`，base 不是 `main` 的 PR 在零判決時回報 CLEAN（來源 #914）。

## Pointers

- Brief shape and the authored-middle contract: `docs/guides/brief-template.md`
- Review rules: `REVIEW.md` (read at the base SHA, never the head)
- Observation window and the metrics compared on return: #888
