#!/bin/sh
# GH-896 — machine gate for the fleet shell tests.
#
# Runs the fleet shell tests under POSIX sh and exits non-zero if any fails,
# so a red test blocks CI. Three groups, two runners (GH-896 + GH-927):
#
#   1. scripts/fleet/test-*.sh (glob) — the ubuntu `fleet-tests` job carries
#      this group. Tests added later under scripts/fleet/ are picked up
#      without editing this file or the workflow.
#   2. scripts/test-*.sh (glob) — same loop, same carrier; #927 added it
#      because the tests one directory up were machine-run by nothing.
#   3. scripts/fleet/test-*.ps1 (glob) — Windows-only. On a Windows host this
#      entrypoint runs each match through `pwsh -NoProfile -File`; anywhere
#      else it prints one SKIP line per match naming the `fleet-tests-windows`
#      job, which runs the group on windows-latest.
#
# Every glob term is guarded individually: a term that matches no file fails
# the run with a message naming it — a silently empty glob is the defect this
# gate exists to prevent, wearing a different hat.
#
# scripts/test-review-capabilities.sh is matched by the second glob and is
# platform-bound, not quarantined: the helper it generates carries
# `set -o pipefail`, which ubuntu's dash rejects, so a case arm below SKIPs it
# with a printed reason; the `fleet-tests-windows` job runs it on
# windows-latest, where sh is Git Bash.
#
# Some tests are QUARANTINED — excluded from every job, named and printed at
# runtime with the issue that owns letting each back in, and counted, so the
# list cannot grow quietly. No count is written here on purpose: a number in a
# comment 35 lines above the arms it describes has already gone stale once.
# The arms below carry the measured failure for each.
#
# #896's doneWhen names test-lane-helpers.sh and #927's names
# scripts/test-review-adapter.sh among the tests that must run; each runs
# nowhere, which is why the PRs opening these gates carry `Issue:` lines and
# no closing keywords.
#
# Entry point used by the `fleet-tests` job in .github/workflows/ci.yml; the
# same command is reproducible locally on any POSIX sh.
#
# Style follows the repo's POSIX-sh conventions: set -eu, no new tooling.
set -eu

cd "$(git rev-parse --show-toplevel)"

status=0
quarantined=0
for t in scripts/fleet/test-*.sh scripts/test-*.sh; do
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
        scripts/test-review-capabilities.sh)
            # Platform-bound, not quarantined: the helper this test generates
            # carries `set -o pipefail`, which ubuntu's dash rejects with
            # `Illegal option` — there it would fail for the shell rather
            # than for anything it asserts. The `fleet-tests-windows` job
            # runs it on windows-latest, where sh is Git Bash (GH-927).
            printf 'SKIP %s (platform-bound: the fleet-tests-windows job runs it)
' "$t"
            continue
            ;;
        scripts/test-review-pr.sh)
            # Red in every CI environment that could carry it: on ubuntu case
            # D1 fails by construction (it asserts the Windows path shapes the
            # Scheduled-Tasks launcher produces; the scratch path is POSIX),
            # and on windows-latest case D11 hit the nohup launch race in
            # review-pr.sh's product-adapter path (`nohup process died
            # immediately`) - measured there and on a workstation under load.
            # It passes on workstation reruns, where it has always been
            # hand-run. Tracked as #1024, which owns removing this entry in
            # the change that turns the test green everywhere.
            q_issue='#1024'
            q_why='red on ubuntu by construction; nohup race on windows-latest'
            ;;
        scripts/test-git-config-guard.sh)
            # Platform-bound subject: it drives git-config-guard.ps1, a
            # Windows-native tool, through pwsh; its locked-config probe
            # cannot run under Linux pwsh. The fleet-tests-windows job runs
            # it (GH-927).
            printf 'SKIP %s (platform-bound: drives Windows-native git-config-guard.ps1; the fleet-tests-windows job runs it)
' "$t"
            continue
            ;;
        scripts/test-review-adapter.sh)
            # Red on a Windows workstation against origin/main: the pwsh child
            # of its Windows block exits 1 and `QUALIFIED=True` never lands in
            # the fixture receipt. Measured twice, not a timing flake.
            # Tracked as #987, which owns removing this entry in the same
            # change that turns the test green.
            q_issue='#987'
            q_why='red on a Windows workstation (QUALIFIED=True never lands)'
            ;;
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
    printf 'RUN  %s
' "$t"
    # Honor the shebang, like production execution does: a `#!/usr/bin/env
    # bash` test (pipefail) dies under ubuntu's dash when handed to `sh` -
    # GH-927's first gated run proved it on two tests. `sh -n` above still
    # syntax-checks every file.
    if head -n 1 "$t" | grep -q '^#!.*bash'; then
        run_shell=bash
    else
        run_shell=sh
    fi
    if "$run_shell" "$t"; then
        printf 'PASS %s
' "$t"
    else
        rc=$?
        printf 'FAIL %s (exit %d)
' "$t" "$rc" >&2
        status=1
    fi
done
# Group 3 — the Windows-only .ps1 tests (GH-927). On a Windows host this
# entrypoint runs each match itself through pwsh; anywhere else it prints one
# SKIP line per match naming the `fleet-tests-windows` job that runs the
# group on windows-latest. The per-term guard matches the shell globs above:
# a .ps1 glob that matches nothing fails the run loudly.
case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*) on_windows=1 ;;
    *) on_windows=0 ;;
esac
for p in scripts/fleet/test-*.ps1; do
    if [ ! -e "$p" ]; then
        printf 'FAIL: %s matched no file
' "$p" >&2
        status=1
        continue
    fi
    case "$p" in
        scripts/fleet/test-detached-dispatch.ps1)
            # Harness-bound, not a self-contained test: its -Edda parameter is
            # Mandatory — it drives a compiled edda binary (GH-605), and no CI
            # job in this workflow compiles the workspace. Stated per #927's
            # doneWhen ("or the issue records a stated reason").
            printf 'SKIP %s (harness-bound: needs -Edda <built edda binary>; no CI job compiles the workspace)
' "$p"
            continue
            ;;
    esac
    if [ "$on_windows" -eq 1 ]; then
        printf 'RUN  %s
' "$p"
        if pwsh -NoProfile -File "$p"; then
            printf 'PASS %s
' "$p"
        else
            rc=$?
            printf 'FAIL %s (exit %d)
' "$p" "$rc" >&2
            status=1
        fi
    else
        printf 'SKIP %s (needs Windows: pwsh -NoProfile -File; the fleet-tests-windows job runs it)
' "$p"
    fi
done
# The count is printed so the quarantine list cannot grow quietly: a rising
# number in CI output is the only signal that this gate is covering less than
# it did. Each entry names the issue that owns removing it.
[ "$quarantined" -eq 0 ] || printf '%d test(s) quarantined - see the QUARANTINE lines above\n' "$quarantined"
exit "$status"
