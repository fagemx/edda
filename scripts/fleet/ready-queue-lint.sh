#!/bin/sh
# ready-queue-lint.sh — machine check that `fleet:ready` never selects a
# delivered issue (GH-665). A merged PR closes the issue but not its labels,
# so delivered issues can still carry `fleet:ready` and stay pickable. This
# script filters the queue against merged PR bodies, counting only closing
# keywords (`Closes #N`, `Fixes #N`, `Resolves #N`) as delivery — a machine
# check against live data, not memory. An ordinary mention (`Issue: #N`,
# `tracked in #N`, `see #N`) is the partial-delivery / non-closing reference
# (REVIEW.md U2/U3) and must never hide a ready issue.
#
# Usage:
#   sh scripts/fleet/ready-queue-lint.sh             # list pickable issues, oldest first
#   sh scripts/fleet/ready-queue-lint.sh --oldest    # print only the oldest pickable issue
#   sh scripts/fleet/ready-queue-lint.sh --check     # exit 1 if any open fleet:ready
#                                                    # issue already has a merged PR
#
# Output: pickable issues on stdout as `#<number> <title>`. Issues excluded as
# delivered are reported on stderr. Any gh failure aborts (fail closed) — a
# broken gh must never look like "queue empty" (same rule as
# scripts/fleet-claim-issue.sh).
set -eu

cd "$(git rev-parse --show-toplevel)"

check=0
oldest=0
for arg in "$@"; do
    case "$arg" in
        --check) check=1 ;;
        --oldest) oldest=1 ;;
        *) echo "usage: $0 [--check] [--oldest]" >&2; exit 2 ;;
    esac
done

issues=$(mktemp)
prs=$(mktemp)
trap 'rm -f "$issues" "$prs"' 0 HUP INT TERM

gh issue list --label fleet:ready --state open --json number,title,createdAt >"$issues"
gh pr list --state merged --limit 500 --json number,body >"$prs"

# delivered($n): some merged PR body closes issue $n — a closing keyword
# (`closes`/`fixes`/`resolves`, with inflections, case-insensitive) followed
# by a word-bounded issue reference (#12 must not match #123 or #1234).
# Mentions without a closing keyword (`Issue: #N`, `tracked in #N`, `see #N`)
# are not delivery and bodies without a reference are ignored.
jq_filter='
    def delivered($prs; $num):
        any($prs[];
            .body != null
            and (.body | test("(^|[^A-Za-z])(close[sd]?|fix(e[sd])?|resolve[sd]?)[[:space:]]+#" + ($num|tostring) + "([^0-9]|$)"; "i")));
'

pickable=$(jq -r --slurpfile prs "$prs" "$jq_filter"'
    sort_by(.createdAt)
    | .[]
    | select(delivered($prs[0]; .number) | not)
    | "#\(.number) \(.title)"
' "$issues")

stale=$(jq -r --slurpfile prs "$prs" "$jq_filter"'
    sort_by(.createdAt)
    | .[]
    | select(delivered($prs[0]; .number))
    | "#\(.number) \(.title)"
' "$issues")

if [ "$oldest" -eq 1 ]; then
    pickable=$(printf '%s\n' "$pickable" | head -n 1)
fi

if [ -n "$stale" ]; then
    printf '%s\n' "$stale" | while IFS= read -r line; do
        echo "ready-queue-lint: delivered but still fleet:ready (drop the label at merge): $line" >&2
    done
fi

if [ -n "$pickable" ]; then
    printf '%s\n' "$pickable"
fi

if [ "$check" -eq 1 ] && [ -n "$stale" ]; then
    exit 1
fi
