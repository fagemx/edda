# GH-896 — fleet shell-test gate fires (seeded failure) and passes (green)

Before this change, no machine gate ran any `scripts/fleet/test-*.sh`: grep
over `.github/` found no reference and `lefthook.yml` runs cargo only, so the
tests' doneWhen (exit 0) rested on hand execution. This evidence shows the new
gate blocking on a seeded failure and passing unseeded.

## The gate

- Workflow: `.github/workflows/ci.yml` — new `fleet-tests` job (ubuntu-latest)
  with a `detect`-computed path filter (`scripts/fleet/**` and
  `.github/workflows/ci.yml`); `ci-gate` evaluates the fleet verdict before
  its early success exits, so a red test blocks the merge gate. Skipped is
  accepted only when detect decided no fleet path changed.
- Entrypoint (identical locally and in CI):

```sh
sh scripts/fleet/run-fleet-tests.sh
```

- The runner is glob-driven (`scripts/fleet/test-*.sh`): tests added later
  (PR #895 brings `test-brief-from-issue.sh` and `test-review-capabilities.sh`)
  are picked up automatically. It also runs `sh -n` on every matched test
  before executing it.

## Excluded test (explicit, never silent)

`test-lane-helpers.sh` is skipped by name with a printed reason. It exercises
`scripts/fleet/lane-*.ps1` through Windows Scheduled Tasks
(`Register-ScheduledTask` / `Start-ScheduledTask`), `pwsh.exe`,
`rust-lld.exe` and `taskkill` — none of which exist on the ubuntu CI runner
(the ScheduledTasks cmdlets are not available to PowerShell on Linux), so the
test cannot run headless there. It keeps passing on Windows lanes where pwsh
and the Task Scheduler exist. The other two tests on this base
(`test-daily-digest.sh`, `test-ready-queue-lint.sh`) are fully offline POSIX sh
and run on ubuntu.

## Seeded failure — the gate fires, rc=1

Seed: `scripts/fleet/test-ready-queue-lint.sh` copied to
`scripts/fleet/test-seeded-failure.sh` with one assertion inverted —
`grep -q '#20 clean ready' "$out" || fail "clean queue must still list…"`
became `grep -q '#20 clean ready' "$out" && fail "SEED (GH-896) inverted
assertion — clean queue must still list…"`. The glob picks the seed up, the
runner runs it, and the entrypoint exits non-zero:

```text
$ sh scripts/fleet/run-fleet-tests.sh
RUN  scripts/fleet/test-daily-digest.sh
daily-digest fixtures passed
PASS scripts/fleet/test-daily-digest.sh
SKIP scripts/fleet/test-lane-helpers.sh (Windows-only: Scheduled Tasks, pwsh.exe, rust-lld.exe, taskkill)
RUN  scripts/fleet/test-ready-queue-lint.sh
ok 1 delivered issues excluded, oldest first, word-boundary holds
ok 2 --oldest returns exactly the oldest pickable issue
ok 3 --check exits 1 and names the stale issues
ok 4 --check on a clean queue exits 0
ok 5 boundary: '#1234' is not a delivery, 'Fixes #123' is
ok 6 closing keywords deliver; 'tracked in', 'Issue:', 'see' do not
ok 7 usage errors -> exit 2
ok 8 broken gh -> fail closed
all ready-queue-lint.sh self-tests passed
PASS scripts/fleet/test-ready-queue-lint.sh
RUN  scripts/fleet/test-seeded-failure.sh
ok 1 delivered issues excluded, oldest first, word-boundary holds
ok 2 --oldest returns exactly the oldest pickable issue
ok 3 --check exits 1 and names the stale issues
FAIL: SEED (GH-896) inverted assertion — clean queue must still list: #20 clean ready
FAIL scripts/fleet/test-seeded-failure.sh (exit 1)
$ echo $?
1
```

The seed was then deleted (`rm scripts/fleet/test-seeded-failure.sh`); it is
not part of the change.

## Green — same entrypoint, unseeded, rc=0

```text
$ sh scripts/fleet/run-fleet-tests.sh
RUN  scripts/fleet/test-daily-digest.sh
daily-digest fixtures passed
PASS scripts/fleet/test-daily-digest.sh
SKIP scripts/fleet/test-lane-helpers.sh (Windows-only: Scheduled Tasks, pwsh.exe, rust-lld.exe, taskkill)
RUN  scripts/fleet/test-ready-queue-lint.sh
ok 1 delivered issues excluded, oldest first, word-boundary holds
ok 2 --oldest returns exactly the oldest pickable issue
ok 3 --check exits 1 and names the stale issues
ok 4 --check on a clean queue exits 0
ok 5 boundary: '#1234' is not a delivery, 'Fixes #123' is
ok 6 closing keywords deliver; 'tracked in', 'Issue:', 'see' do not
ok 7 usage errors -> exit 2
ok 8 broken gh -> fail closed
all ready-queue-lint.sh self-tests passed
PASS scripts/fleet/test-ready-queue-lint.sh
$ echo $?
0
```

Syntax checks on the added runner both pass:
`sh -n scripts/fleet/run-fleet-tests.sh` and
`bash -n scripts/fleet/run-fleet-tests.sh` exit 0.

The transcripts above were captured with the entrypoint executed on the
workstation's Git Bash (the same POSIX sh the runner invokes); CI executes the
same command on ubuntu-latest, where the two included tests are fully offline
and need no Windows-only facility.
