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

The gate's own CI runs found three real defects, none of them introduced by
this PR. Each test now lands where it can actually run, and the one that can
run nowhere is quarantined by name with its issue number printed.

| test | state | why | who runs it |
|---|---|---|---|
| `scripts/test-review-capabilities.sh` | platform-bound | generates a helper carrying `set -o pipefail` (`scripts/review-pr.sh:806`); ubuntu's `sh` is dash and rejects it with `Illegal option`, so the test fails for the shell rather than for anything it asserts | `fleet-tests-windows` |
| `scripts/fleet/test-lane-helpers.sh` | **quarantined** | red on `windows-latest` (case 0, `prepare: worktree not registered`) *and* on a Windows workstation against `origin/main` (case 25, Task Scheduler) | nobody — **#963** |
| `scripts/fleet/test-next-loop.sh` | **quarantined** | green on a Windows workstation, red on **both** CI platforms for different reasons: ubuntu `dry-run output misses the launch command`; `windows-latest` case 7 receives the transport refusal rather than the missing-arm one it asserts | nobody — **#964** |

The two quarantined tests are green in exactly one place each — the
workstation where they have always been hand-run. That is #896's premise
restated as a measurement rather than an argument: a test nothing executes
drifts until it only passes where its author happened to be standing.

The quarantine is printed at runtime, not silent:

```text
QUARANTINE scripts/fleet/test-lane-helpers.sh (red on every Windows environment; tracked as #963)
QUARANTINE scripts/fleet/test-next-loop.sh (red on both CI platforms; tracked as #964)
```

Each issue owns removing its entry in the same PR that turns the test green, so
the exclusion cannot quietly become permanent. **This PR carries no closing
keyword**: #896's doneWhen names `test-lane-helpers.sh` among the tests that
must run, and it does not, so #896 stays open for that item.

The third defect was the checkout. Both fleet jobs now use `fetch-depth: 0`:
`test-next-loop.sh` and `test-issue-freshness.sh` resolve `origin/main`, and
a default depth-1 checkout has no such ref, so they exited non-zero on
`fatal: ambiguous argument 'origin/main'` rather than on anything they test.

An OS-conditional skip for `test-lane-helpers.sh` was tried before the
quarantine and reverted — with the test executing from the shared entrypoint,
the entrypoint itself could not be green on a Windows workstation, which is
the doneWhen this script exists to satisfy.

One observation is recorded without a mechanism, because it was not
isolated: while that OS-conditional skip was in place,
`scripts/test-review-capabilities.sh` failed **through the runner**
(`FAIL mutate: backend wrote canary`) while passing standalone in the same
worktree. It has passed on every run since `test-lane-helpers.sh` stopped
executing from this entrypoint. Noted so a future change that reintroduces
an in-runner Windows-only test knows to look for it.

## Round 2 — what the entrypoint actually runs

Superseded transcripts from earlier heads on this branch have been removed
rather than relabelled: they showed `PASS scripts/fleet/test-lane-helpers.sh`
and a `FAIL scripts/test-review-capabilities.sh` from this entrypoint, and
neither can occur at this head — the first is quarantined, the second is not
in the glob. Keeping them as evidence for a design they predate is the same
defect this file exists to document. The two transcripts below are from this
head and can be reproduced by the commands shown.

## Round 2, part 2 — the gate caught three real test defects on CI

The push of the Round 2 fix (`cbe9561`) put every enumerated test in front of
the GitHub runners for the first time. The gate went red and stayed red —
three ungated tests carry portability defects that hand execution never
exercised (CI run 33986498515 @ `cbe9561`):

- `scripts/fleet/test-next-loop.sh` — exit 128 on BOTH runners:
  `git rev-parse origin/main` at the fixture line; the fleet-tests checkout
  is single-ref depth-1, so the remote-tracking ref does not exist. Fixed in
  the test: fall back to the checkout's own HEAD and its tree object.
- `scripts/test-review-capabilities.sh` — on ubuntu only: the generated
  Linux runner is a bash script (`#!/usr/bin/env bash` + `set -o pipefail`);
  the test executed its copy with `sh`, handing it to dash, which rejects
  pipefail at line 2. Production execs the shebang (`nohup "$RUNNER"`), so
  only the test's invocation was wrong. Fixed in the test: `bash`, not `sh`.
- `scripts/fleet/test-lane-helpers.sh` — on windows-latest only: case 1's
  registration assertion grepped `git worktree list --porcelain` for the
  worktree path with `grep -F`; NTFS comparison is case-insensitive and the
  runner's TEMP case need not match the case lane-prepare.ps1 resolved.
  Fixed in the test: `grep -iF`.

All three fixes are in the tests themselves; no production script changed.
The gate is doing exactly what #896 asked: a test that nothing ran was a
defect reservoir, and the first honest run drained three.

## Seeded failure — the gate fires, rc=1

Seed: `scripts/fleet/test-ready-queue-lint.sh` copied to
`scripts/fleet/test-seeded-failure.sh` with one assertion inverted —
`grep -q '#20 clean ready' "$out" || fail "clean queue must still list…"`
became `grep -q '#20 clean ready' "$out" && fail "SEED (GH-896) inverted
assertion — clean queue must still list…"`. The glob picks the seed up, the
runner runs it, and the entrypoint exits non-zero:

```text
$ sh scripts/fleet/run-fleet-tests.sh
RUN  scripts/fleet/test-brief-from-issue.sh
PASS scripts/fleet/test-brief-from-issue.sh
RUN  scripts/fleet/test-daily-digest.sh
PASS scripts/fleet/test-daily-digest.sh
QUARANTINE scripts/fleet/test-lane-helpers.sh (red on every Windows environment; tracked as #963)
QUARANTINE scripts/fleet/test-next-loop.sh (red on both CI platforms; tracked as #964)
RUN  scripts/fleet/test-ready-queue-lint.sh
FAIL: SEED (round 2 evidence) inverted assertion - clean ready issue must be listed: #20 clean ready
FAIL scripts/fleet/test-ready-queue-lint.sh (exit 1)
$ echo $?
1
```

The seed was deleted afterwards (`rm scripts/fleet/test-seeded-failure.sh`);
it is not part of the change.

## Green — same entrypoint, unseeded, rc=0

```text
$ sh scripts/fleet/run-fleet-tests.sh
RUN  scripts/fleet/test-brief-from-issue.sh
PASS scripts/fleet/test-brief-from-issue.sh
RUN  scripts/fleet/test-daily-digest.sh
PASS scripts/fleet/test-daily-digest.sh
QUARANTINE scripts/fleet/test-lane-helpers.sh (red on every Windows environment; tracked as #963)
QUARANTINE scripts/fleet/test-next-loop.sh (red on both CI platforms; tracked as #964)
RUN  scripts/fleet/test-ready-queue-lint.sh
PASS scripts/fleet/test-ready-queue-lint.sh
$ echo $?
0
```

On this Windows workstation: rc=0, with one quarantine and one platform-bound
skip both named at runtime. The two platform-bound tests run in the
`fleet-tests-windows` job.

Syntax checks on the added runner both pass:
`sh -n scripts/fleet/run-fleet-tests.sh` and
`bash -n scripts/fleet/run-fleet-tests.sh` exit 0.

The transcripts above were captured with the entrypoint executed on the
workstation's Git Bash (the same POSIX sh the runner invokes); CI executes the
same command on ubuntu-latest, where the two included tests are fully offline
and need no Windows-only facility.
