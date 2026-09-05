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

- The runner enumerates `scripts/fleet/test-*.sh` by glob plus
  `scripts/test-review-capabilities.sh` by exact name — that test was already
  on `main` before this PR's base (added by #858), lives one directory above
  the glob, and can never be matched by it. (An earlier version of this
  paragraph claimed both files arrive with PR #895 and are picked up
  automatically; both halves were false and are corrected in Round 1's
  P1-1.) Each term fails the run loudly if it matches nothing. The runner
  also runs `sh -n` on every matched test before executing it.

## Excluded test (explicit, never silent)

`test-lane-helpers.sh` is skipped by name, only off Windows, with a printed
reason: the skip is OS-conditional (`case "$(uname -s)" in
MINGW*|MSYS*|CYGWIN*)` runs it), and the `fleet-tests-windows` CI job runs
the same entrypoint on windows-latest, where the test executes instead of
skipping. It exercises
`scripts/fleet/lane-*.ps1` through Windows Scheduled Tasks
(`Register-ScheduledTask` / `Start-ScheduledTask`), `pwsh.exe`,
`rust-lld.exe` and `taskkill` — none of which exist on the ubuntu CI runner
(the ScheduledTasks cmdlets are not available to PowerShell on Linux), so the
test cannot run headless there. It keeps passing on Windows lanes where pwsh
and the Task Scheduler exist. The other two tests on this base
(`test-daily-digest.sh`, `test-ready-queue-lint.sh`) are fully offline POSIX sh
and run on ubuntu.

## Round 2 (GH-896 fix round) — three terms, two groups, one entrypoint

The Round 1 verdict (P0-1, P0-2, P1-1, P1-2, P2) is delivered here:

- The runner matches three things: the `scripts/fleet/test-*.sh` glob
  (unchanged), `scripts/test-review-capabilities.sh` by exact name (new),
  and — through the OS-conditional skip — `scripts/fleet/test-lane-helpers.sh`
  on Windows hosts.
- A new `fleet-tests-windows` job runs the SAME entrypoint
  (`sh scripts/fleet/run-fleet-tests.sh`) on windows-latest. One entrypoint,
  two runners; both jobs share the `detect.fleet` path filter (now also
  naming `scripts/test-review-capabilities.sh`), and `ci-gate` evaluates
  both verdicts before its early success exits.
- The transcripts below are from this workstation's Git Bash (the runner's
  own POSIX sh); the seeded run carried a failure in each of the two groups.

Seeded run — one inverted assertion per group, `rc=1`. Group 1 (glob):
`grep -q '#20 clean ready' "$out" || fail …` inverted to `&& fail …` in
`scripts/fleet/test-ready-queue-lint.sh`. Group 2 (named file): the final
gate `[ "$failures" -eq 0 ] || exit 1` inverted to `-eq 1` in
`scripts/test-review-capabilities.sh`. Both seeds were restored afterwards
(`git diff --name-only` contains neither).

```text
$ sh scripts/fleet/run-fleet-tests.sh
[head elided — identical to the green transcript below through
 `RUN  scripts/fleet/test-ready-queue-lint.sh`]
PASS scripts/fleet/test-lane-helpers.sh
RUN  scripts/fleet/test-next-loop.sh
ok 1 ready issue dry-run
ok 2 claimed issue refusal
ok 3 marker-left-in-brief refusal
ok 4 round-cap refusal
ok 5 moved-head refusal
ok 6 shadow post shape
ok 7 non-shadow delegation refusal
PASS: scripts/fleet/test-next-loop.sh
PASS scripts/fleet/test-next-loop.sh
RUN  scripts/fleet/test-ready-queue-lint.sh
FAIL: SEED (GH-896 r2) inverted assertion — clean ready issue must be listed
FAIL scripts/fleet/test-ready-queue-lint.sh (exit 1)
RUN  scripts/test-review-capabilities.sh
review capability canaries passed (unguarded baseline; old/modern dispatch and fallback; all source scopes)
Windows generated lane canaries passed (old/modern transport and source-snapshot cases)
FAIL scripts/test-review-capabilities.sh (exit 1)
$ echo $?
1
```

The runner does not stop at the first failure: it runs every matched test
and folds each exit into one non-zero result, so both groups fire in one
pass.

Green run — same entrypoint, unseeded, `rc=0` (`test-lane-helpers.sh` runs
on this Windows host: 28 cases, `PASS: lane helper self-test (28 cases)`;
on ubuntu it prints the SKIP line instead):

```text
$ sh scripts/fleet/run-fleet-tests.sh
RUN  scripts/fleet/test-brief-from-issue.sh
ok 1 windows host facts and skeleton
ok 2 linux host facts and skeleton
ok 3 missing Predicted surface exits 2
ok 4 empty Predicted surface exits 2
ok 5 --build-lane sets lane field
ok 6 negated mentions excluded from scope paths
ok 7 title metacharacters stripped
ok 8 crafted scope token rejected
PASS: scripts/fleet/test-brief-from-issue.sh
PASS scripts/fleet/test-brief-from-issue.sh
RUN  scripts/fleet/test-daily-digest.sh
daily-digest fixtures passed
PASS scripts/fleet/test-daily-digest.sh
RUN  scripts/fleet/test-lane-helpers.sh
[28 ok lines elided — verbatim in the lane report; ends
 `PASS: lane helper self-test (28 cases)`]
PASS scripts/fleet/test-lane-helpers.sh
RUN  scripts/fleet/test-next-loop.sh
ok 1 ready issue dry-run
ok 2 claimed issue refusal
ok 3 marker-left-in-brief refusal
ok 4 round-cap refusal
ok 5 moved-head refusal
ok 6 shadow post shape
ok 7 non-shadow delegation refusal
PASS: scripts/fleet/test-next-loop.sh
PASS scripts/fleet/test-next-loop.sh
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
RUN  scripts/test-review-capabilities.sh
review capability canaries passed (unguarded baseline; old/modern dispatch and fallback; all source scopes)
Windows generated lane canaries passed (old/modern transport and source-snapshot cases)
PASS scripts/test-review-capabilities.sh
$ echo $?
0
```

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
