#!/bin/sh
# GH-896 — machine gate for the fleet shell tests.
#
# Runs every scripts/fleet/test-*.sh under POSIX sh and exits non-zero if any
# fails, so a red test blocks CI. That one glob is the whole match list: tests
# added later under scripts/fleet/ are picked up without editing this file or
# the workflow. Widening to scripts/test-*.sh is intentionally NOT done here —
# tracked separately as #927.
#
# scripts/test-review-capabilities.sh is #896's other hand-named test and is
# NOT matched here. It lives one directory up, and the helper it generates
# carries `set -o pipefail` (scripts/review-pr.sh:806) which ubuntu's dash
# rejects with `Illegal option`, so it would fail for the shell rather than for
# anything it asserts. The `fleet-tests-windows` job in
# .github/workflows/ci.yml invokes it directly, where sh is Git Bash.
#
# Two tests are QUARANTINED — excluded from every job, named and printed at
# runtime with the issue that owns letting them back in. Each is red on every
# environment that could carry it; see the case arms below for the measured
# failures. This is the one doneWhen item #896 does not get, which is why the
# PR opening this gate carries `Issue: #896` and no closing keyword.
#
# Entry point used by the `fleet-tests` job in .github/workflows/ci.yml; the
# same command is reproducible locally on any POSIX sh.
#
# Style follows the repo's POSIX-sh conventions: set -eu, no new tooling.
set -eu

cd "$(git rev-parse --show-toplevel)"

status=0
quarantined=0
for t in scripts/fleet/test-*.sh; do
    if [ ! -e "$t" ]; then
        # Reached only when the glob matched no file at all: fail closed.
        printf 'FAIL: %s matched no file\n' "$t" >&2
        status=1
        continue
    fi
    # `sh -n` runs before the quarantine arms, so an excluded test still has
    # its syntax checked and cannot rot unnoticed while it sits out. What this
    # cannot detect is a quarantined test that has become green again — that
    # would mean executing it, which is the thing the quarantine exists to
    # avoid. #963 and #964 each own removing their own entry, so the exclusion
    # is released by the fix rather than by this script noticing.
    sh -n "$t" || {
        printf 'FAIL: sh -n %s\n' "$t" >&2
        status=1
        continue
    }
    case "$t" in
        scripts/fleet/test-collision-scan.sh)
            # QUARANTINED. Its Windows block is gated on `command -v pwsh`
            # (:138), but ubuntu runners ship pwsh, so the block runs there and
            # task_query() (:178-180) calls Get-ScheduledTask — a Windows-only
            # cmdlet — and :192 fires every run. A presence check for pwsh is
            # not a check for Windows. Landed on main in 586cb07 (#967) after
            # this branch's base and was red on ubuntu from its first gated run.
            # Tracked as #971, which owns the guard and this entry.
            printf 'QUARANTINE %s (Windows block gated on pwsh presence, not on Windows; tracked as #971)\n' "$t"
            quarantined=$((quarantined + 1))
            continue
            ;;
        scripts/fleet/test-lane-helpers.sh)
            # QUARANTINED, not platform-bound: red on windows-latest (case 0,
            # `prepare: worktree not registered`) and on a Windows workstation
            # against origin/main (case 25, Task Scheduler). Red everywhere, so
            # no job can carry it. Tracked as #963, which owns removing this
            # entry in the same PR that turns the test green. #896 stays open
            # for this item — that is why this PR carries no closing keyword.
            printf 'QUARANTINE %s (red on every Windows environment; tracked as #963)\n' "$t"
            quarantined=$((quarantined + 1))
            continue
            ;;
        scripts/fleet/test-next-loop.sh)
            # QUARANTINED. Green on a Windows workstation, red on BOTH CI
            # platforms and for different reasons: on ubuntu `dry-run output
            # misses the launch command` (the launch line is rendered only on
            # the Scheduled Tasks path); on windows-latest case 7 gets the
            # transport refusal rather than the missing-arm one it asserts.
            # Green in exactly one place — the machine where it has always been
            # hand-run — which is #896's premise in a single test. Tracked as
            # #964, which owns removing this entry.
            printf 'QUARANTINE %s (red on both CI platforms; tracked as #964)\n' "$t"
            quarantined=$((quarantined + 1))
            continue
            ;;
    esac
    printf 'RUN  %s\n' "$t"
    if sh "$t"; then
        printf 'PASS %s\n' "$t"
    else
        rc=$?
        printf 'FAIL %s (exit %d)\n' "$t" "$rc" >&2
        status=1
    fi
done
# The count is printed so the quarantine list cannot grow quietly: a rising
# number in CI output is the only signal that this gate is covering less than
# it did. Each entry names the issue that owns removing it.
[ "$quarantined" -eq 0 ] || printf '%d test(s) quarantined - see the QUARANTINE lines above\n' "$quarantined"
exit "$status"
