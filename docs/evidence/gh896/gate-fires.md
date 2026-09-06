# GH-896 — the fleet shell-test gate: what runs, what does not, and why

Before this change, no machine gate ran any `scripts/fleet/test-*.sh`: grep
over `.github/` found no reference and `lefthook.yml` runs cargo only, so the
tests' doneWhen (exit 0) rested on hand execution.

Wiring them up found that several were already broken. This file records what
the gate runs, what it excludes and on what measured evidence, and the runs
that show it firing.

## The gate

- **`fleet-tests`** (ubuntu-latest) runs `sh scripts/fleet/run-fleet-tests.sh`.
- **`fleet-tests-windows`** (windows-latest) runs `sh scripts/test-review-capabilities.sh`
  and nothing else.
- Both jobs are `needs` of `ci-gate`, which evaluates them **before** its early
  success exits, so a red fleet job cannot be swallowed. A skip is accepted
  only when `detect.outputs.fleet == 'false'`.
- Both check out with `fetch-depth: 0`: tests that resolve `origin/main` exit
  on `fatal: ambiguous argument` under the default depth-1 checkout.
- `detect`'s fleet path filter is `scripts/fleet/*`,
  `scripts/test-review-capabilities.sh` and `.github/workflows/ci.yml`
  (`ci.yml:91`).

The runner's match list is **one glob**: `scripts/fleet/test-*.sh`. A test
added there later is picked up without editing the runner or the workflow.
`scripts/test-review-capabilities.sh` is **not** matched by it — it lives one
directory up, and the windows job invokes it directly. Widening to
`scripts/test-*.sh` is deliberately not done here (#927).

`sh -n` runs on every matched file, including quarantined ones, before the
quarantine arms — so an excluded test cannot rot unnoticed while it sits out.

## What is excluded, and the measurement behind each

These tests are **quarantined**: excluded from every job, named and printed at
runtime with the issue that owns letting them back in. The count is printed
too, so the list cannot grow quietly.

| test | measured failure | issue |
|---|---|---|
| `scripts/fleet/test-lane-helpers.sh` | red on `windows-latest` (case 0, `prepare: worktree not registered`) **and** on a Windows workstation against `origin/main` (case 25, `edda-lane-gh626envcheck … scheduler result is unavailable`) | **#963** |
| `scripts/fleet/test-next-loop.sh` | green on a Windows workstation; red on ubuntu (`dry-run output misses the launch command`) and on `windows-latest` (case 7 receives the transport refusal, not the missing-arm one it asserts) | **#964** |
| `scripts/fleet/test-collision-scan.sh` | red on ubuntu. Its Windows block is gated on `command -v pwsh` (`:138`), but ubuntu runners ship pwsh, so the block runs and `task_query()` (`:178-180`) calls `Get-ScheduledTask`, a Windows-only cmdlet. A presence check for pwsh is not a check for Windows. Landed on `main` in `586cb07` (#967) after this branch's base and was red from its first gated run | **#971** |

One test is **platform-bound** rather than quarantined:
`scripts/test-review-capabilities.sh` generates a helper carrying
`set -o pipefail` (the `set -o pipefail` it writes into that runner — `scripts/review-pr.sh`, no line number here on purpose: this citation has already gone stale once); ubuntu's `sh` is dash and
rejects it with `Illegal option`, so there it would fail for the shell rather
than for anything it asserts. The windows job runs it, where `sh` is Git Bash.

One quarantined test, `test-next-loop.sh`, is green in exactly one place —
the workstation where it has always been hand-run. That is #896's premise as
a measurement rather than an argument: a test nothing executes drifts until it
only passes where its author was standing. The other two are red everywhere,
which is the same story further along.

**#896's doneWhen names `test-lane-helpers.sh` among the tests that must run,
and it runs nowhere.** That is why the PR opening this gate carries
`Issue: #896` and no closing keyword. Each issue above owns removing its own
entry in the same PR that turns its test green, so the exclusion is released
by the fix rather than by anyone remembering.

An OS-conditional skip for `test-lane-helpers.sh` was tried before the
quarantine and reverted: with the test executing from the shared entrypoint,
the entrypoint itself could not be green on a Windows workstation, which is
the doneWhen this script exists to satisfy.

## Test scripts modified on this branch, and why none is green-washing

Three were changed, by an earlier round:

- **`test-review-capabilities.sh`** — it executed its generated Linux runner
  with `sh`; production execs the shebang (`#!/usr/bin/env bash`,
  `review-pr.sh`, which writes a `#!/usr/bin/env bash` shebang and `set -o pipefail` into that runner). Changed to `bash`. This did **not** rescue ubuntu:
  its own commit `38875ca` still shows `Illegal option -o pipefail` in job
  `101396161654`, which is why the test is platform-bound rather than fixed.
- **`test-lane-helpers.sh`** — case 1 grepped `git worktree list --porcelain`
  with `grep -F`; NTFS comparison is case-insensitive and the runner's TEMP
  case need not match what `lane-prepare.ps1` resolved. Changed to `grep -iF`.
  This did **not** turn the test green: at the head carrying it, the test is
  still rc=1 with zero `ok` lines, and the quarantine was decided at `3469338`
  which already contains the fix.
- **`test-next-loop.sh`** — its `origin/main` reference gained a
  `HEAD`/`HEAD^{tree}` fallback so it does not die on a depth-1 checkout.
  Fixture-only; it **exposed** the real failure now tracked in #964 rather
  than hiding one.

No production script was changed by this PR.

## Seeded failure — the gate fires, rc=1

One assertion in `scripts/fleet/test-ready-queue-lint.sh` inverted in place
(`||` → `&&` on the `#20 clean ready` check), the entrypoint run, then the
file restored — `git status --porcelain` on it is empty afterwards.

Abridged: only the QUARANTINE lines, the seeded failure and the tail are
shown; the full run also prints `RUN`/`PASS` for each of the eight tests that
execute, and each test's own `ok N` lines.

```text
$ sh scripts/fleet/run-fleet-tests.sh
QUARANTINE scripts/fleet/test-collision-scan.sh (Windows block gated on pwsh presence, not on Windows; tracked as #971)
QUARANTINE scripts/fleet/test-lane-helpers.sh (red on every Windows environment; tracked as #963)
QUARANTINE scripts/fleet/test-next-loop.sh (red on both CI platforms; tracked as #964)
FAIL: SEED (round 3 evidence) clean ready issue must be listed: #20 clean ready
FAIL scripts/fleet/test-ready-queue-lint.sh (exit 1)
3 test(s) quarantined - see the QUARANTINE lines above
$ echo $?
1
```

## Green — same entrypoint, unseeded, rc=0

Same abridgement.

```text
$ sh scripts/fleet/run-fleet-tests.sh
RUN  scripts/fleet/test-brief-from-issue.sh
PASS scripts/fleet/test-brief-from-issue.sh
RUN  scripts/fleet/test-brief-validate.sh
PASS scripts/fleet/test-brief-validate.sh
QUARANTINE scripts/fleet/test-collision-scan.sh (Windows block gated on pwsh presence, not on Windows; tracked as #971)
RUN  scripts/fleet/test-daily-digest.sh
PASS scripts/fleet/test-daily-digest.sh
RUN  scripts/fleet/test-guard-push.sh
PASS scripts/fleet/test-guard-push.sh
RUN  scripts/fleet/test-issue-freshness.sh
PASS scripts/fleet/test-issue-freshness.sh
QUARANTINE scripts/fleet/test-lane-helpers.sh (red on every Windows environment; tracked as #963)
RUN  scripts/fleet/test-manager-tick.sh
PASS scripts/fleet/test-manager-tick.sh
QUARANTINE scripts/fleet/test-next-loop.sh (red on both CI platforms; tracked as #964)
RUN  scripts/fleet/test-ready-queue-lint.sh
PASS scripts/fleet/test-ready-queue-lint.sh
RUN  scripts/fleet/test-verdict-drift.sh
PASS scripts/fleet/test-verdict-drift.sh
3 test(s) quarantined - see the QUARANTINE lines above
$ echo $?
0
```

Eight tests execute and pass; three are quarantined and named. The glob picked
up `test-brief-validate.sh`, `test-guard-push.sh`, `test-issue-freshness.sh`,
`test-manager-tick.sh` and `test-verdict-drift.sh` without any edit to the
runner or the workflow, which is what the single-glob design is for.

`sh -n scripts/fleet/run-fleet-tests.sh` and `bash -n` both exit 0.

## What the gate has caught so far

Every one of these was already broken and invisible; none was introduced by
this PR. This is the list #896 predicted would exist.

| defect | disposition |
|---|---|
| depth-1 checkout has no `origin/main` | fixed here (`fetch-depth: 0`) |
| `set -o pipefail` in a generated helper vs ubuntu's dash | test moved to the windows job; `review-pr.sh` belongs to the PRs that own it |
| `test-lane-helpers.sh` red on both Windows environments | quarantined, #963 |
| `test-next-loop.sh` red on both CI platforms | quarantined, #964 |
| `test-collision-scan.sh` Windows block gated on pwsh presence, not on Windows | quarantined, #971 |

Superseded transcripts from earlier heads on this branch were removed rather
than relabelled: they showed a `PASS` for a test that is now quarantined and a
`FAIL` for one no longer in the glob. Keeping them as evidence for a design
they predate is the same defect this file exists to document.
