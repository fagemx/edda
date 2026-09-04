#!/bin/sh
# GitHub issue claim guard/write side (GH-782).
set -eu

usage() {
    echo "usage: $0 [--check] <issue-number> <machine>/<role>" >&2
    echo "       $0 <issue-number> <machine>/<role> [--check]" >&2
    exit 2
}

die_gh() { echo "claim guard: gh command failed" >&2; exit 2; }

check=0
if [ "${1:-}" = "--check" ]; then check=1; shift
elif [ "${3:-}" = "--check" ]; then check=1; set -- "$1" "$2"
fi
[ $# -eq 2 ] || usage
issue=$1
machine=$2
case "$machine" in
    */*) first=${machine%%/*}; second=${machine#*/} ;;
    *) echo "machine identity must be <machine>/<role>, got '$machine'" >&2; exit 2 ;;
esac
case "$first" in
    '' | *[[:space:]]*) usage ;;
esac
case "$second" in
    '' | */* | *[[:space:]]*) usage ;;
esac
case "$issue" in
    '' | *[!0-9]* | 0) usage ;;
esac
tab=$(printf '\t')

# PR state wins over comments. Filtering is local in gh's jq expression.
pr=$(gh pr list --state all --limit 1000000 --json number,state,title,headRefName --jq \
    ".[] | select((.state == \"OPEN\" or .state == \"MERGED\") and (((.title | ascii_downcase) | test(\"gh-${issue}([^0-9]|$)\")) or ((.headRefName | ascii_downcase) | test(\"gh${issue}([^0-9]|$)\")))) | [.number, (.state | ascii_downcase)] | @tsv" \
    ) || die_gh
pr=$(printf '%s' "$pr" | tr -d '\r')
# Prefer a merged delivery if more than one matching PR exists.
merged=$(printf '%s\n' "$pr" | awk -F '\t' '$2 == "merged" { print; exit }')
[ -z "$merged" ] || pr=$merged
if [ -n "$pr" ]; then
    pr=$(printf '%s\n' "$pr" | head -n 1)
    pr_number=${pr%%"$tab"*}
    pr_state=${pr#*"$tab"}
    if [ "$pr_state" = "merged" ]; then
        echo "issue $issue delivered by #$pr_number (merged) — drop fleet:ready"
    else
        echo "issue $issue has open PR #$pr_number; not touching it"
    fi
    exit 1
fi

comments=$(gh issue view "$issue" --json comments --jq \
    '.comments[] | .createdAt as $c | .body | gsub("\r"; "") | split("\n")[] | (($c // "") + "\t" + .)' \
    ) || die_gh
comments=$(printf '%s' "$comments" | tr -d '\r')
other=''
other_when=''
self=0
tab=$(printf '\t')
scratch=$(mktemp)
trap 'rm -f "$scratch"' EXIT
printf '%s\n' "$comments" > "$scratch"
while IFS= read -r line; do
    [ -n "$line" ] || continue
    when=${line%%"$tab"*}
    body=${line#*"$tab"}
    body=$(printf '%s' "$body" | sed 's/^[[:space:]]*//')
    case "$body" in
        taking:*)
            owner=$(printf '%s' "${body#taking:}" | sed 's/^[[:space:]]*//')
            owner=${owner%%[[:space:]]*}
            [ -n "$owner" ] || continue
            if [ "$owner" = "$machine" ]; then self=1
            elif [ -z "$other" ]; then other=$owner; other_when=$when
            fi ;;
    esac
done < "$scratch"

if [ -n "$other" ]; then
    echo "issue $issue is claimed by '$other' at $other_when — comment \"taking:\"; not touching it"
    exit 1
fi
if [ "$check" -eq 1 ]; then
    if [ "$self" -eq 1 ]; then echo "issue $issue already claimed by '$machine'"
    else echo "issue $issue is unclaimed (--check: nothing written)"; fi
    exit 0
fi
if [ "$self" -ne 1 ]; then
    now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    gh issue comment "$issue" --body "taking: $machine at $now" >/dev/null || die_gh
fi
# Repeat edit on self to repair partial writes; it adds no duplicate comment.
gh issue edit "$issue" --add-label fleet:claimed --remove-label fleet:ready --add-assignee @me >/dev/null || die_gh
echo "claimed issue $issue for '$machine'"
