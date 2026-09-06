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
# carries `set -o pipefail` (the `set -o pipefail` it writes into that runner, `scripts/review-pr.sh`) which ubuntu's dash
# rejects with `Illegal option`, so it would fail for the shell rather than for
# anything it asserts. The `fleet-tests-windows` job in
# .github/workflows/ci.yml invokes it directly, where sh is Git Bash.
#
# Some tests are QUARANTINED — excluded from every job, named and printed at
# runtime with the issue that owns letting each back in, and counted, so the
# list cannot grow quietly. No count is written here on purpose: a number in a
# comment 35 lines above the arms it describes has already gone stale once.
# The arms below carry the measured failure for each.
#
# #896's doneWhen names test-lane-helpers.sh among the tests that must run,
# and it runs nowhere, which is why the PR opening this gate carries
# `Issue: #896` and no closing keyword.
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
    # avoid. Each arm below names the issue that owns removing it, so the
    # exclusion is released by that fix rather than by this script noticing.
    sh -n "$t" || {
        printf 'FAIL: sh -n %s\n' "$t" >&2
        status=1
        continue
    }
    # Quarantine arms set `q_issue` and `q_why` and nothing else. The print
    # and the count are derived below, once, so a new arm cannot forget to
    # increment a hand-written counter — a review demonstrated exactly that:
    # a fourth arm without the +1 printed four QUARANTINE lines under a
    # "3 test(s) quarantined" total. Deriving it makes that unrepresentable.
    q_issue=''
    q_why=''
    case "$t" in
        scripts/fleet/test-collision-scan.sh)
            # Its Windows block is gated on `command -v pwsh` (:138), but
            # ubuntu runners ship pwsh, so the block runs there and
            # task_query() (:178-180) calls Get-ScheduledTask — a Windows-only
            # cmdlet — and :192 fires every run. A presence check for pwsh is
            # not a check for Windows. Landed on main in 586cb07 (#967) after
            # this branch's base, red on ubuntu from its first gated run.
            q_issue='#971'
            q_why='Windows block gated on pwsh presence, not on Windows'
            ;;
        scripts/fleet/test-lane-helpers.sh)
            # Red on windows-latest (case 0, `prepare: worktree not
            # registered`) and on a Windows workstation against origin/main
            # (case 25, Task Scheduler). Red in every environment that could
            # carry it, so no job can. #896's doneWhen names this test, and it
            # runs nowhere — that is why this PR carries no closing keyword.
            q_issue='#963'
            q_why='red on every Windows environment'
            ;;
        scripts/fleet/test-next-loop.sh)
            # Green on a Windows workstation; red on BOTH CI platforms for
            # different reasons — ubuntu `dry-run output misses the launch
            # command`, windows-latest case 7 gets the transport refusal
            # rather than the missing-arm one it asserts. Green in exactly one
            # place, the machine where it has always been hand-run.
            q_issue='#964'
            q_why='red on both CI platforms'
            ;;
    esac
    if [ -n "$q_issue" ]; then
        printf 'QUARANTINE %s (%s; tracked as %s)\n' "$t" "$q_why" "$q_issue"
        quarantined=$((quarantined + 1))
        continue
    fi
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
