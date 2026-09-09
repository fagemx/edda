#!/bin/sh
# verdict-drift.sh — the readiness signal `mergeStateStatus` does not provide
# (GH-914). One line per open PR: does the current head carry the newest
# verdict, and does that verdict resolve the PR? Readiness is keyed on the
# §7 verdict comment pinned to the head SHA (rules.md R23/R24), never on
# `mergeStateStatus`: the ruleset protects `main` only, so a stacked PR
# reports CLEAN with zero verdicts.
#
# Output, one line per open PR:
#   #<n> <head12> <base> <state> [ mergeable=<v> ] [ base=<branch> (...) ] [ orphan-response=Round-<N> ]
# with <state> one of:
#   no verdict on head | stale from <sha12> | SHADOW only | LGTM | Changes Requested
# The base annotation is appended when the PR's base is not `main`, whose
# status contexts are the only ones any ruleset enforces.
#
# GH-993: `orphan-response=Round-<N>` is appended when the newest
# `## Review Response: Round N` comment on the PR answers a round that was
# never posted — a completed review whose report reached no one but the
# implementer's answer to it (the observed failure: PRs #974/#976/#980/#981
# each carried a `Review Response: Round 1` and no `Code Review: Round 1`).
# Matching is by round NUMBER only, not SHA — a response answering an old,
# superseded round is still answering a round that is really there. Only the
# NEWEST response is judged, so a PR that recovered by moving straight to a
# later, properly paired round (the #974 repair shape: Round 1's response
# stays permanently unanswered, but Round 2 is posted and paired) reads
# clean. This annotation holds the PR (not_ready=1) independent of <state>.
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
# Exit 0 every PR carries a verdict on its head, is not CONFLICTING, and its
# newest Review Response (if any) answers a round that was actually posted;
# exit 1 when any PR has `no verdict on head`, is `stale from ...`, is
# CONFLICTING, or carries an orphan Review Response (GH-993); exit 2 when a
# gh read fails — a check that could not read must never print nothing and
# exit 0.
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
# Defensive: strip any stray CR (`tr -d`, not a trailing-only trim — none of
# these fields are free-form text, so a bare CR never belongs in one). Found
# while adding the GH-993 fields below: an external jq.exe on at least one
# Windows dev box emits CRLF for redirected output, and `read`'s last
# variable absorbs a trailing \r as data, silently breaking any comparison
# or interpolation of that field (`mergeable = CONFLICTING` and the R23
# fields the same way — this predates GH-993 and just had no field that both
# echoed into output and drove a comparison until now). Harmless no-op
# against the real `gh --jq`, which filters through an embedded jq library,
# not a spawned binary.
open_rows=$(printf '%s' "$open_rows" | tr -d '\r')
if [ "$(printf %s "$open_rows" | grep -c .)" -ge "$open_limit" ]; then
    echo "verdict-drift: the open-PR enumeration hit its limit of $open_limit; PRs past it were not examined (raise EDDA_OPEN_PR_LIMIT)" >&2
fi

not_ready=0
while IFS="$(printf '\t')" read -r num head base mergeable; do
    [ -n "$num" ] || continue
    # GH-993: one tagged pass over `.comments` so the drift check keeps its
    # one-call-per-PR cost. Each comment yields at most one row: "v" (an R23
    # verdict — same regex and fields as before, plus its own round number)
    # or "r" (a first-line `## Review Response: Round N` heading, round
    # number only). A comment can match only one shape; most match neither
    # and yield nothing (`empty`).
    verdicts=$(gh pr view "$num" --repo "$repo" --json comments --jq '
        .comments[]
        | (.body) as $b
        | ($b | split("\n")[0]) as $fl
        | if ($fl | test("^## Code Review: Round [0-9]+( \\(SHADOW\\))? — PR #[0-9]+ @ [0-9a-f]{40}( \\(SHADOW\\))?$")) then
            [
              "v",
              ($fl | capture("Round (?<r>[0-9]+)") | .r),
              ($fl | capture("@ (?<sha>[0-9a-f]{40})") | .sha),
              (if ($fl | test("\\(SHADOW\\)")) or ($b | test("(?m)^- shadow: true$")) then "s" else "a" end),
              (if ($b | test("(?m)^LGTM")) then "lgtm"
               elif ($b | test("(?m)^Changes Requested")) then "cr"
               else "unknown" end)
            ]
          elif ($fl | test("^## Review Response: Round [0-9]+")) then
            ["r", ($fl | capture("Round (?<r>[0-9]+)") | .r)]
          else
            empty
          end
        | @tsv' \
    ) || fail_read "pr view $num (comments)"
    verdicts=$(printf '%s' "$verdicts" | tr -d '\r')  # see the note above open_rows

    newest=
    newest_sha=
    newest_resolve=
    authoritative_present=0
    authoritative_resolve=
    code_review_rounds=","
    response_round=
    while IFS="$(printf '\t')" read -r row_tag f1 f2 f3 f4; do
        [ -n "$row_tag" ] || continue
        if [ "$row_tag" = v ]; then
            v_round=$f1; v_sha=$f2; v_kind=$f3; v_resolve=$f4
            code_review_rounds="$code_review_rounds$v_round,"
            newest=1
            newest_sha=$v_sha
            newest_resolve=$v_resolve
            if [ "$v_kind" = "a" ] && [ "$v_sha" = "$head" ]; then
                authoritative_present=1
                authoritative_resolve=$v_resolve
            fi
        else
            # row_tag = r. Only the newest response (last in comment order)
            # is judged — see the GH-993 note above the Output doc comment.
            response_round=$f1
        fi
    done <<EOF
$verdicts
EOF

    # GH-993: does the newest Review Response answer a round that was
    # actually posted? Round-number membership only (see note above); the
    # leading/trailing commas make the case pattern an exact-element match
    # rather than a substring match on the number itself (so round "1" does
    # not accidentally match inside "11").
    orphan_response=
    if [ -n "$response_round" ]; then
        case "$code_review_rounds" in
            *",$response_round,"*) ;;
            *) orphan_response=$response_round ;;
        esac
    fi

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
    if [ -n "$orphan_response" ]; then
        line="$line orphan-response=Round-$orphan_response"
        not_ready=1
    fi
    printf '%s\n' "$line"
done <<EOF
$open_rows
EOF

[ "$not_ready" = 0 ] && exit 0
exit 1
