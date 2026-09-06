#!/bin/sh
# collision-scan.sh — read-only sweep for claim collisions (GH-912).
#
# Two collision classes, one line each:
#   1. an open issue carrying MORE THAN ONE distinct `taking:` identity
#      (#887: two machines claimed it 4.5 minutes apart and both built);
#   2. an open issue named (gh<N>) by TWO OR MORE open PRs' titles or branch
#      names — the double-build itself.
#
# Read-only: never comments, labels, closes, or merges. Runnable on a
# schedule and by any controller at wave start.
#
# usage: sh scripts/fleet/collision-scan.sh
# exit 0 = no collision stands; 1 = at least one collision stands; 2 = a gh
# read failed (a sweep that cannot see must not report clean).
set -u

repo=${EDDA_REPO:-fagemx/edda}
rc=0

issues_raw=$(gh issue list --repo "$repo" --state open --limit 1000 \
    --json number --jq '.[].number' 2>/dev/null)
if [ $? -ne 0 ]; then
    echo "collision-scan: gh issue list failed" >&2
    exit 2
fi
issues=$(printf '%s\n' "$issues_raw" | tr -d '\r')
for n in $issues; do
    ids_raw=$(gh issue view "$n" --repo "$repo" --json comments 2>/dev/null \
        --jq '[.comments[].body
               | select(startswith("taking: "))
               | split(" ")[1]] | unique | join(", ")')
    if [ $? -ne 0 ]; then
        echo "collision-scan: gh issue view $n failed" >&2
        exit 2
    fi
    ids=$(printf '%s\n' "$ids_raw" | tr -d '\r')
    [ -n "$ids" ] || continue
    [ -n "$ids" ] || continue
    k=$(printf '%s' "$ids" | awk -F', ' '{ print NF }')
    if [ "$k" -gt 1 ]; then
        echo "collision: issue $n claimed by $k distinct identities: $ids"
        rc=1
    fi
done

prs_raw=$(gh pr list --repo "$repo" --state open --limit 1000 \
    --json number,title,headRefName 2>/dev/null)
if [ $? -ne 0 ]; then
    echo "collision-scan: gh pr list failed" >&2
    exit 2
fi
prs=$(printf '%s\n' "$prs_raw" | tr -d '\r')
# Distinct (issue, pr) pairs: a PR naming the same issue in both its title and
# its branch name still counts as ONE PR naming it.
pairs=$(printf '%s' "$prs" \
  | jq -r '.[] | .number as $p | ((.title // "") + " " + (.headRefName // "")) | capture("gh(?<n>[0-9]+)"; "g").n | "\(.) \($p)"' 2>/dev/null \
  | tr -d '\r' | sort -u)
if [ -n "$pairs" ]; then
    summary=$(printf '%s\n' "$pairs" | awk \
        '{ c[$1]++; pr[$1] = pr[$1] (sep[$1] ? ", " : "") $2; sep[$1] = 1 }
         END { for (i in c) if (c[i] >= 2) print i, c[i], pr[i] }')
    if [ -n "$summary" ]; then
        rc=1
        printf '%s\n' "$summary" | while IFS=' ' read -r n k names; do
            echo "collision: issue $n has $k open PRs naming it: $names"
        done
    fi
fi

exit "$rc"
