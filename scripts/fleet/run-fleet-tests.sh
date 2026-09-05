#!/bin/sh
# GH-896 — machine gate for the fleet shell tests.
#
# Runs every scripts/fleet/test-*.sh plus scripts/test-review-capabilities.sh
# under POSIX sh and exits non-zero if any of them fails, so a red test blocks
# CI. The scripts/fleet glob is deliberate: tests added later under
# scripts/fleet/ (test-brief-from-issue.sh landed in 6849b90 / GH-885;
# test-next-loop.sh with GH-899) are picked up automatically without editing
# this file or the workflow. scripts/test-review-capabilities.sh is matched
# explicitly by name because the issue's doneWhen lists it among these tests
# but it lives one directory above the glob (scripts/, not scripts/fleet/);
# widening to all of scripts/test-*.sh is intentionally NOT done here —
# tracked separately as #927.
#
# Exclusion (explicit, never silent): test-lane-helpers.sh is Windows-only by
# construction — it exercises scripts/fleet/lane-*.ps1 through Windows
# Scheduled Tasks (Register-ScheduledTask / Start-ScheduledTask), pwsh.exe,
# rust-lld.exe and taskkill, none of which exist on the ubuntu CI runner. The
# skip below is therefore OS-conditional, not absolute: on Windows hosts
# (MINGW/MSYS/CYGWIN) the test runs through this entrypoint, and the
# `fleet-tests-windows` job in .github/workflows/ci.yml runs this same
# entrypoint on windows-latest, where the test executes instead of skipping,
# so a red lane-helpers test blocks the merge gate too.
# Everywhere else the SKIP is printed with its reason.
#
# Entry point used by the `fleet-tests` job in .github/workflows/ci.yml; the
# same command is reproducible locally on any POSIX sh.
#
# Style follows the repo's POSIX-sh conventions: set -eu, no new tooling.
set -eu

cd "$(git rev-parse --show-toplevel)"

status=0
for t in scripts/fleet/test-*.sh scripts/test-review-capabilities.sh; do
    if [ ! -e "$t" ]; then
        # Reached only when a term above matched no file at all (empty glob,
        # or the explicitly named test was deleted): fail closed.
        printf 'FAIL: %s matched no file\n' "$t" >&2
        status=1
        continue
    fi
    case "$t" in
        scripts/fleet/test-lane-helpers.sh)
            case "$(uname -s)" in
                MINGW*|MSYS*|CYGWIN*) ;;  # Windows host: run it
                *)
                    printf 'SKIP %s (Windows-only: Scheduled Tasks, pwsh.exe, rust-lld.exe, taskkill — runs on Windows lanes and the fleet-tests-windows CI job)\n' "$t"
                    continue
                    ;;
            esac
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
