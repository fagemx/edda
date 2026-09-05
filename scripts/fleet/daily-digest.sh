#!/bin/sh
# GH-765 — daily fleet digest for the operator.
#
# Five fixed sections: 合併了什麼 / 擋住什麼 / 例外 / 成本 / 明天會做的.
# Content split (issue #765 D8): everything ledger-sourced — the 例外, 成本
# and 明天會做的 sections — comes from `edda recap --digest` and is embedded
# VERBATIM; everything GitHub-sourced (merged PRs, blocked PRs, board
# needs-operator lines, review cost from verdict comments) is gathered here
# with `gh`; posting the board comment and pushing via `edda notify send`
# is delivery.
#
# Usage:
#   daily-digest.sh [--since <RFC3339|Nh|Nd>] [--board <issue-number>] [--dry-run]
#
# Environment:
#   EDDA_REPO         repository slug            (default fagemx/edda)
#   EDDA_BOARD_ISSUE  board issue number         (default 613)
#   EDDA_DIGEST_OUT   where to write the digest  (default: a temp file)
#
# Exits 0 on success, 2 on usage errors, 1 when `gh` or `edda recap` fails.
# `--dry-run` prints the digest to stdout and posts nothing.
set -eu

prog=$(basename "$0")
self_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
usage() {
    printf 'usage: %s [--since <RFC3339|Nh|Nd>] [--board <issue-number>] [--dry-run]\n' "$prog" >&2
}

EDDA_REPO="${EDDA_REPO:-fagemx/edda}"
EDDA_BOARD_ISSUE="${EDDA_BOARD_ISSUE:-613}"
DRY_RUN=0
SINCE_ARG=""
while [ $# -gt 0 ]; do
    case "$1" in
        --since)
            [ $# -ge 2 ] || { usage; exit 2; }
            SINCE_ARG="$2"; shift 2 ;;
        --board)
            [ $# -ge 2 ] || { usage; exit 2; }
            EDDA_BOARD_ISSUE="$2"; shift 2 ;;
        --dry-run) DRY_RUN=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage; exit 2 ;;
    esac
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' 0 HUP INT TERM
OUT="${EDDA_DIGEST_OUT:-$tmp/digest.md}"

# --- 1. resolve the window -----------------------------------------------------
hours=""
case "$SINCE_ARG" in
    "")
        SINCE_ARG="24h"
        hours=24
        ;;
    *h)
        hours=${SINCE_ARG%h}
        case "$hours" in
            ''|*[!0-9]*) usage; exit 2 ;;
        esac
        ;;
    *d)
        days=${SINCE_ARG%d}
        case "$days" in
            ''|*[!0-9]*) usage; exit 2 ;;
        esac
        ;;
    *)
        # RFC3339 — taken as given (edda validates it)
        ;;
esac
if [ -n "$hours" ]; then
    SINCE_ISO=$(date -u -d "$hours hours ago" +%FT%TZ)
elif [ -n "${days:-}" ]; then
    SINCE_ISO=$(date -u -d "$days days ago" +%FT%TZ)
else
    SINCE_ISO="$SINCE_ARG"
fi
NOW_ISO=$(date -u +%FT%TZ)

# --- 2. ledger half: edda recap --digest, split by ## headings -----------------
RECAP_RAW="$tmp/recap.md"
if ! edda recap --digest --since "$SINCE_ARG" >"$RECAP_RAW"; then
    printf '%s: edda recap --digest failed\n' "$prog" >&2
    exit 1
fi
for heading in '例外' '成本' '明天會做的'; do
    if ! grep -q "^## $heading" "$RECAP_RAW"; then
        printf '%s: edda recap --digest output is missing the %s section\n' "$prog" "$heading" >&2
        exit 1
    fi
done
awk -v ex="$tmp/exceptions.md" -v cost="$tmp/cost.md" -v nxt="$tmp/next.md" '
    /^## 例外/       { out = ex;   next }
    /^## 成本/       { out = cost; next }
    /^## 明天會做的/ { out = nxt;  next }
    /^## /           { out = "";    next }
    out              { print > out }
' "$RECAP_RAW"

# --- 3. 合併了什麼 --------------------------------------------------------------
merged_rows=$(gh pr list --repo "$EDDA_REPO" --state merged --limit 50 \
    --json number,title,mergedAt,headRefOid \
    --jq '.[] | select(.mergedAt >= "'"$SINCE_ISO"'") | [.number, .title, .mergedAt] | @tsv' \
) || { printf '%s: gh pr list (merged) failed\n' "$prog" >&2; exit 1; }

merged_body="$tmp/merged-body.md"
: >"$merged_body"
review_total=""
review_count=0
printf '%s\n' "$merged_rows" | while IFS="$(printf '\t')" read -r num title mergedAt; do
    [ -n "$num" ] || continue
    comments=$(gh pr view "$num" --repo "$EDDA_REPO" --json comments --jq '.comments[].body') || {
        printf '%s: gh pr view %s (comments) failed\n' "$prog" "$num" >&2
        exit 1
    }
    rounds=$(printf '%s\n' "$comments" | grep -c 'Code Review: Round' || true)
    pr_cost=$(printf '%s\n' "$comments" \
        | grep -o 'Cost: \$[0-9][0-9.]*' \
        | sed 's/^Cost: \$//' \
        | awk '{s+=$1; n++} END { if (n > 0) printf "%.2f", s }')
    if [ -n "$pr_cost" ]; then
        printf -- '- #%s %s — merged %s, verdict rounds %s, review cost $%s\n' \
            "$num" "$title" "$mergedAt" "$rounds" "$pr_cost" >>"$merged_body"
    else
        printf -- '- #%s %s — merged %s, verdict rounds %s, review cost n/a\n' \
            "$num" "$title" "$mergedAt" "$rounds" >>"$merged_body"
    fi
done

# total review cost over all merged PRs in the window (one more gh pass would
# be wasteful — re-derive from the body we just wrote)
review_total=$(sed -n 's/.*review cost \$\([0-9.]*\)$/\1/p' "$merged_body" \
    | awk '{s+=$1; n++} END { if (n > 0) printf "%.2f", s }')

# --- 4. 擋住什麼 -----------------------------------------------------------------
blocked_body="$tmp/blocked-body.md"
: >"$blocked_body"
open_rows=$(gh pr list --repo "$EDDA_REPO" --state open --limit 100 \
    --json number,title,mergeStateStatus,headRefOid \
    --jq '.[] | select(.mergeStateStatus == "BLOCKED" or .mergeStateStatus == "DIRTY") | [.number, .title, .mergeStateStatus, .headRefOid] | @tsv' \
) || { printf '%s: gh pr list (open) failed\n' "$prog" >&2; exit 1; }

# Readiness is the verdict-drift state (GH-914), not mergeStateStatus: the
# ruleset protects `main` only, so a stacked PR whose base is a feature
# branch reports CLEAN with zero verdicts. The drift check is invoked, not
# re-implemented here — its exit 1 is the normal "something is not ready"
# signal and must not abort the digest; its exit 2 is a failed read and
# takes the same failure path as a gh failure below. GH_OPEN_JSON is cleared
# for the subprocess so a test fixture written for this script's own
# post-jq `gh pr list` output is not re-parsed as raw JSON by the drift check.
drift_out="$tmp/drift.txt"
drift_rc=0
GH_OPEN_JSON= sh "$self_dir/verdict-drift.sh" >"$drift_out" 2>"$tmp/drift.err" || drift_rc=$?
if [ "$drift_rc" -ge 2 ]; then
    printf '%s: verdict-drift.sh failed (exit %s)\n' "$prog" "$drift_rc" >&2
    cat "$tmp/drift.err" >&2
    exit 1
fi
drift_map="$tmp/drift-map.tsv"
# keep only the not-ready states; a head verdict (LGTM or Changes Requested)
# means the readiness signal exists and needs no digest row from here
awk '
    $4 == "no" || $4 == "stale" || $4 == "SHADOW" {
        n = substr($1, 2)
        reason = $4
        for (i = 5; i <= NF; i++) reason = reason " " $i
        printf "%s\t%s\n", n, reason
    }
' "$drift_out" >"$drift_map"
while IFS="$(printf '\t')" read -r num title state sha; do
    [ -n "$num" ] || continue
    reasons=""
    drift_reason=$(awk -F '\t' -v n="$num" '$1 == n { print $2 }' "$drift_map")
    if [ "$state" = "DIRTY" ]; then
        reasons="conflicts with main (window)"
    elif [ "$state" = "BLOCKED" ]; then
        check_runs=$(gh api "repos/$EDDA_REPO/commits/$sha/check-runs" \
            --jq '.check_runs[] | "\(.name)=\(.conclusion // .status)"') || {
            printf '%s: gh api (check runs for #%s) failed\n' "$prog" "$num" >&2
            exit 1
        }
        statuses=$(gh api "repos/$EDDA_REPO/commits/$sha/status" \
            --jq '.statuses[] | "\(.context)=\(.state)"') || {
            printf '%s: gh api (commit status for #%s) failed\n' "$prog" "$num" >&2
            exit 1
        }
        statuses=$(printf '%s\n%s\n' "$check_runs" "$statuses")
        for ctx in 'CI Gate' 'Independent Review'; do
            ctx_state=$(printf '%s\n' "$statuses" | sed -n "s/^$ctx=//p" | head -1)
            if [ -z "$ctx_state" ]; then
                ctx_state="absent"
            fi
            if [ "$ctx_state" != "success" ]; then
                [ -n "$reasons" ] && reasons="$reasons, "
                reasons="$reasons$ctx=$ctx_state"
            fi
        done
    fi
    if [ -n "$drift_reason" ]; then
        [ -n "$reasons" ] && reasons="$reasons, "
        reasons="$reasons$drift_reason"
    fi
    if [ -n "$reasons" ]; then
        printf -- '- #%s %s — %s\n' "$num" "$title" "$reasons" >>"$blocked_body"
    fi
done <<EOF
$open_rows
EOF

# --- 5. board needs-operator lines ----------------------------------------------
board_lines="$tmp/board-lines.md"
: >"$board_lines"
board_comments=$(gh issue view "$EDDA_BOARD_ISSUE" --repo "$EDDA_REPO" --json comments \
    --jq '.comments[] | select(.createdAt >= "'"$SINCE_ISO"'") | . as $comment | ($comment.body | split("\n")[] | select(test("needs-operator")) | [$comment.url, .] | @tsv)' \
) || { printf '%s: gh issue view (board comments) failed\n' "$prog" >&2; exit 1; }
printf '%s\n' "$board_comments" | while IFS="$(printf '\t')" read -r url body; do
    [ -n "$url" ] || continue
    if printf '%s\n' "$body" | grep -q 'needs-operator'; then
        printf -- '- 看板：%s (%s)\n' "$body" "$url" >>"$board_lines"
    fi
done || true

# --- 6. ready queue from GitHub --------------------------------------------------
ready_lines="$tmp/ready-lines.md"
: >"$ready_lines"
gh issue list --repo "$EDDA_REPO" --label fleet:ready --limit 3 --json number,title \
    --jq '.[] | "- fleet:ready #\(.number) \(.title)"' >"$ready_lines" \
    || { printf '%s: gh issue list (fleet:ready) failed\n' "$prog" >&2; exit 1; }

# --- 7. assemble -----------------------------------------------------------------
{
    printf '# Fleet digest %s — window %s → %s\n' "$(date -u +%F)" "$SINCE_ISO" "$NOW_ISO"
    printf '## 合併了什麼\n'
    cat "$merged_body"
    if [ ! -s "$merged_body" ]; then
        printf '（無）\n'
    fi
    printf '## 擋住什麼\n'
    cat "$blocked_body"
    if [ ! -s "$blocked_body" ]; then
        printf '（無）\n'
    fi
    printf '## 例外\n'
    cat "$tmp/exceptions.md"
    if [ -s "$board_lines" ]; then
        cat "$board_lines"
    fi
    printf '## 成本\n'
    cat "$tmp/cost.md"
    if [ -n "$review_total" ]; then
        printf -- '- 審查（gh verdict 留言 Cost:）：$%s\n' "$review_total"
    else
        printf -- '- 審查（gh verdict 留言 Cost:）：n/a\n'
    fi
    printf '## 明天會做的\n'
    cat "$tmp/next.md"
    cat "$ready_lines"
} >"$OUT"

# --- 8. delivery -----------------------------------------------------------------
if [ "$DRY_RUN" = 1 ]; then
    cat "$OUT"
    exit 0
fi

gh issue comment "$EDDA_BOARD_ISSUE" --repo "$EDDA_REPO" --body-file "$OUT" \
    || { printf '%s: gh issue comment failed\n' "$prog" >&2; exit 1; }

if edda notify send --title "Fleet digest $(date -u +%F)" --file "$OUT"; then
    :
else
    rc=$?
    # The notify failure is the log line, never a failed digest.
    printf '%s: edda notify send failed (exit %s) — digest was still posted\n' "$prog" "$rc" >&2
fi

exit 0
