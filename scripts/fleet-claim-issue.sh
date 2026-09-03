#!/bin/sh
# fleet-claim-issue.sh — cross-machine issue claim for the fleet (GH-656).
#
# Two machines each have their own `.edda/`; an `edda claim` on one is
# invisible to the other. GitHub is the only shared truth, and the
# `fleet.cross-machine-claim` convention (leave a `taking: <machine>`
# comment and a `lane:<machine>` label before starting work) was pure
# discipline. This script makes it mechanical:
#
#   fleet-claim-issue.sh <issue> <machine>     claim the issue for <machine>
#   fleet-claim-issue.sh --check <issue> <machine>   read-only check
#
# Outcomes:
#   0  claimed now (unclaimed before), or already held by <machine>
#      (idempotent — no duplicate comment), or `--check` with no
#      other-machine claim (nothing written in check mode, ever)
#   1  claimed by ANOTHER machine — prints which machine, when, and via
#      which surface; nothing is written
#   2  usage error
#
# Machine labels are compared verbatim against the tokens in
# `lane:<machine>` labels and `taking: <machine>` comments. The hostname is
# never guessed: <machine> must be passed explicitly. Any gh failure aborts
# the script (fail closed) — a broken gh must never look like "unclaimed".
#
# `edda dispatch --issue <N> --machine <label>` runs the same check on the
# dispatch path (crates/edda-cli/src/claim_guard.rs) and refuses with exit
# code 2; this script is the write side of the same convention.
set -eu

usage() {
    echo "usage: $0 [--check] <issue-number> <machine-label>" >&2
    echo "       $0 <issue-number> <machine-label> [--check]" >&2
    exit 2
}

check=0
if [ "${1:-}" = "--check" ]; then
    check=1
    shift
elif [ "${3:-}" = "--check" ]; then
    check=1
    set -- "$1" "$2"
fi

[ $# -eq 2 ] || usage
issue=$1
machine=$2

case "$machine" in
    '' | *[[:space:]]*)
        echo "machine label must be one token without whitespace, got '$machine'" >&2
        exit 2
        ;;
esac

tab=$(printf '\t')

labels=$(gh issue view "$issue" --json labels --jq '.labels[].name')
comments=$(gh issue view "$issue" --json comments --jq \
    '.comments[] | .createdAt as $c | .body | gsub("\r"; "") | split("\n")[] | (($c // "") + "\t" + .)')

other=''
other_when=''
other_source=''
self=0

for label in $labels; do
    case "$label" in
        lane:*)
            m=${label#lane:}
            if [ "$m" = "$machine" ]; then
                self=1
            elif [ -z "$other" ]; then
                other=$m
                other_source="label $label"
            fi
            ;;
    esac
done

scratch=$(mktemp)
trap 'rm -f "$scratch"' EXIT
printf '%s\n' "$comments" > "$scratch"
while IFS= read -r line; do
    [ -n "$line" ] || continue
    when=${line%%"$tab"*}
    body=${line#*"$tab"}
    while :; do
        case "$body" in
            [[:space:]]*) body=${body#[[:space:]]} ;;
            *) break ;;
        esac
    done
    case "$body" in
        taking:\ *)
            m=${body#taking: }
            m=${m%%[[:space:]]*}
            if [ "$m" = "$machine" ]; then
                self=1
            elif [ -z "$other" ]; then
                other=$m
                other_when=$when
                other_source='comment "taking:"'
            elif [ "$other" = "$m" ] && [ -z "$other_when" ]; then
                other_when=$when
            fi
            ;;
    esac
done < "$scratch"

if [ -n "$other" ]; then
    msg="issue $issue is claimed by machine '$other'"
    if [ -n "$other_when" ]; then
        msg="$msg (taking: $other at $other_when)"
    fi
    msg="$msg — $other_source; not touching it"
    echo "$msg"
    exit 1
fi

if [ "$self" -eq 1 ]; then
    echo "issue $issue already claimed by '$machine'; nothing to do"
    exit 0
fi

if [ "$check" -eq 1 ]; then
    echo "issue $issue is unclaimed (--check: nothing written)"
    exit 0
fi

now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
gh issue comment "$issue" --body "taking: $machine at $now" >/dev/null
gh issue edit "$issue" --add-label "lane:$machine" >/dev/null
echo "claimed issue $issue for '$machine' (comment taking: $machine at $now, label lane:$machine)"
