#!/bin/sh
# GH-896 — machine gate for the fleet shell tests.
#
# Runs every scripts/fleet/test-*.sh under POSIX sh and exits non-zero if any
# of them fails, so a red test blocks CI. The glob is deliberate: tests added
# later (test-brief-from-issue.sh and test-review-capabilities.sh arrive with
# PR #895) are picked up automatically without editing this file or the
# workflow.
#
# Exclusion (explicit, never silent): test-lane-helpers.sh is Windows-only by
# construction — it exercises scripts/fleet/lane-*.ps1 through Windows
# Scheduled Tasks (Register-ScheduledTask / Start-ScheduledTask), pwsh.exe,
# rust-lld.exe and taskkill, none of which exist on the ubuntu CI runner. It
# keeps passing on Windows lanes where pwsh and the Task Scheduler exist.
#
# Entry point used by the `fleet-tests` job in .github/workflows/ci.yml; the
# same command is reproducible locally on any POSIX sh.
#
# Style follows the repo's POSIX-sh conventions: set -eu, no new tooling.
set -eu

cd "$(git rev-parse --show-toplevel)"

status=0
found=0
for t in scripts/fleet/test-*.sh; do
    if [ "$found" -eq 0 ] && [ ! -e "$t" ]; then
        echo 'FAIL: no scripts/fleet/test-*.sh found (glob did not expand)' >&2
        exit 1
    fi
    found=1
    case "$t" in
        scripts/fleet/test-lane-helpers.sh)
            printf 'SKIP %s (Windows-only: Scheduled Tasks, pwsh.exe, rust-lld.exe, taskkill)\n' "$t"
            continue
            ;;
    esac
    sh -n "$t" || {
        printf 'FAIL: sh -n %s\n' "$t" >&2
        status=1
        continue
    }
    printf 'RUN  %s\n' "$t"
    if sh "$t"; then
        printf 'PASS %s\n' "$t"
    else
        rc=$?
        printf 'FAIL %s (exit %d)\n' "$t" "$rc" >&2
        status=1
    fi
done
exit "$status"
