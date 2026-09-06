#!/bin/sh
# verdict-drift.sh — the readiness signal `mergeStateStatus` does not provide
# (GH-914). One line per open PR: does the current head carry the newest
# verdict, and does that verdict resolve the PR? Readiness is keyed on the
# §7 verdict comment pinned to the head SHA (rules.md R23/R24), never on
# `mergeStateStatus`: the ruleset protects `main` only, so a stacked PR
# reports CLEAN with zero verdicts.
#
# Output, one line per open PR:
#   #<n> <head12> <base> <state> [ mergeable=<v> ] [ base=<branch> (...) ]
# with <state> one of:
#   no verdict on head | stale from <sha12> | SHADOW only | LGTM | Changes Requested
# The base annotation is appended when the PR's base is not `main`, whose
# status contexts are the only ones any ruleset enforces.
#
# GH-958: R24 names THREE readiness fields, and this check now covers all
# three — (1) the newest verdict's SHA equals the head, (2) the
# mergeStateStatus caveat above, and (3) `mergeable` is not CONFLICTING,
# which used to be left to the digest's DIRTY row and so went missing
# whenever this check ran standalone. CONFLICTING is a not-ready state;
# UNKNOWN (GitHub has not computed the merge yet) is annotated but does not
# hold the PR, because it is a transient answer, not a verdict.
#
# The open-PR enumeration limit is shared with daily-digest.sh through
# EDDA_OPEN_PR_LIMIT (GH-958): the two used 200 and 100, so a PR past the
# digest's 100 got a drift line here that could never reach a digest row —
# invisible in the one artefact R24 calls the report.
#
# Exit 0 every PR carries a verdict on its head and is not CONFLICTING;
# exit 1 when any PR has `no verdict on head`, is `stale from ...`, or is
# CONFLICTING; exit 2 when a gh read fails — a check that could not read
# must never print nothing and exit 0.
# Read-only: no posting, no labels, no merges, no ledger writes.
set -eu

repo=${EDDA_REPO:-fagemx/edda}
# Shared with daily-digest.sh; see the note above. A saturated enumeration
# is announced on stderr rather than silently dropping the tail.
open_limit=${EDDA_OPEN_PR_LIMIT:-200}

fail_read() {
    echo "verdict-drift: could not read PR state from gh ($1) — refusing to print a clean bill" >&2
    exit 2
}

# The R23 verdict heading, canonical REVIEW.md §7 shape: the ` (SHADOW)`
# suffix follows `Round <N>`. The trailing-suffix variant is also accepted so
# the issue #914 spelling of the pattern matches the same comments.
open_rows=$(gh pr list --repo "$repo" --state open --limit "$open_limit" \
    --json number,headRefOid,baseRefName,mergeable \
    --jq '.[] | [.number, .headRefOid, .baseRefName, .mergeable] | @tsv' \
) || fail_read "pr list"
if [ "$(printf %s "$open_rows" | grep -c .)" -ge "$open_limit" ]; then
    echo "verdict-drift: the open-PR enumeration hit its limit of $open_limit; PRs past it were not examined (raise EDDA_OPEN_PR_LIMIT)" >&2
fi

not_ready=0
while IFS="$(printf '\t')" read -r num head base mergeable; do
    [ -n "$num" ] || continue
    verdicts=$(gh pr view "$num" --repo "$repo" --json comments --jq '
        .comments[]
        | (.body) as $b
        | ($b | split("\n")[0]) as $fl
        | select($fl | test("^## Code Review: Round [0-9]+( \\(SHADOW\\))? — PR #[0-9]+ @ [0-9a-f]{40}( \\(SHADOW\\))?$"))
        | [
            ($fl | capture("@ (?<sha>[0-9a-f]{40})") | .sha),
            (if ($fl | test("\\(SHADOW\\)")) or ($b | test("(?m)^- shadow: true$")) then "s" else "a" end),
            (if ($b | test("(?m)^LGTM")) then "lgtm"
             elif ($b | test("(?m)^Changes Requested")) then "cr"
             else "unknown" end)
          ]
        | @tsv' \
    ) || fail_read "pr view $num (comments)"

    newest=
    newest_sha=
    newest_resolve=
    authoritative_present=0
    authoritative_resolve=
    while IFS="$(printf '\t')" read -r v_sha v_kind v_resolve; do
        [ -n "$v_sha" ] || continue
        newest=1
        newest_sha=$v_sha
        newest_resolve=$v_resolve
        if [ "$v_kind" = "a" ] && [ "$v_sha" = "$head" ]; then
            authoritative_present=1
            authoritative_resolve=$v_resolve
        fi
    done <<EOF
$verdicts
EOF

    head12=$(printf '%s' "$head" | cut -c1-12)
    if [ -z "$newest" ]; then
        state="no verdict on head"
        not_ready=1
    elif [ "$newest_sha" != "$head" ]; then
        state="stale from $(printf '%s' "$newest_sha" | cut -c1-12)"
        not_ready=1
    elif [ "$authoritative_present" = 0 ]; then
        state="SHADOW only"
    elif [ "$authoritative_resolve" = "lgtm" ]; then
        state="LGTM"
    else
        # a head verdict that is not LGTM holds the PR, whatever it says
        state="Changes Requested"
    fi

    # R24 field (3). CONFLICTING holds the PR; UNKNOWN is surfaced without
    # holding it; MERGEABLE and an empty value add nothing to the line. An
    # empty value means the object carried no mergeable key at all, as the
    # older fixtures do — NOT an older gh, which rejects an unknown --json
    # field before the request and so lands in fail_read instead.
    line="#$num $head12 $base $state"
    if [ "$mergeable" = "CONFLICTING" ]; then
        line="$line mergeable=CONFLICTING"
        not_ready=1
    elif [ "$mergeable" = "UNKNOWN" ]; then
        line="$line mergeable=UNKNOWN"
    fi
    if [ "$base" != "main" ]; then
        line="$line base=$base (status contexts not enforced)"
    fi
    printf '%s\n' "$line"
done <<EOF
$open_rows
EOF

[ "$not_ready" = 0 ] && exit 0
exit 1
