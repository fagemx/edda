#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' 0 HUP INT TERM
case_number=0

expect_decide() {
    name=$1
    expected=$2
    state_lines=$3
    json=$4
    case_number=$((case_number + 1))
    state="$tmp/state-$case_number"
    printf '%b' "$state_lines" >"$state"
    if ! actual=$(printf '%s' "$json" | \
        PR_REVIEW_WATCH_STATE="$state" sh "$root/scripts/pr-review-watch.sh" decide); then
        printf '%s: decide exited non-zero\n' "$name" >&2
        return 1
    fi
    if [ "$actual" != "$expected" ]; then
        printf '%s: expected\n  %s\ngot\n  %s\n' "$name" "$expected" "$actual" >&2
        return 1
    fi
}

expect_label() {
    name=$1
    expected=$2
    body=$3
    case_number=$((case_number + 1))
    if ! actual=$(printf '%b' "$body" | sh "$root/scripts/review-pr.sh" verdict-label); then
        printf '%s: verdict-label exited non-zero\n' "$name" >&2
        return 1
    fi
    if [ "$actual" != "$expected" ]; then
        printf '%s: expected label %s, got %s\n' "$name" "$expected" "$actual" >&2
        return 1
    fi
}

# --- decide: PR queue triage from gh pr list JSON ---------------------------

expect_decide \
    'new PR is queued for review' \
    'REVIEW 42 abc123' \
    '' \
    '[{"number":42,"headRefOid":"abc123","isDraft":false,"labels":[]}]'

expect_decide \
    'same SHA already reviewed is skipped' \
    'SKIP 42 already-reviewed' \
    '42 abc123\n' \
    '[{"number":42,"headRefOid":"abc123","isDraft":false,"labels":[]}]'

expect_decide \
    'new head SHA after push is reviewed again' \
    'REVIEW 42 def456' \
    '42 abc123\n' \
    '[{"number":42,"headRefOid":"def456","isDraft":false,"labels":[]}]'

expect_decide \
    'draft PR is skipped' \
    'SKIP 42 draft' \
    '' \
    '[{"number":42,"headRefOid":"abc123","isDraft":true,"labels":[]}]'

expect_decide \
    'review:unreviewed label stops the watcher for that PR' \
    'SKIP 42 review-unreviewed' \
    '' \
    '[{"number":42,"headRefOid":"abc123","isDraft":false,"labels":[{"name":"review:unreviewed"}]}]'

expect_decide \
    'empty open-PR queue decides nothing' \
    '' \
    '' \
    '[]'

expect_decide \
    'missing head SHA is skipped, not queued' \
    'SKIP 42 missing-head' \
    '' \
    '[{"number":42,"isDraft":false,"labels":[]}]'

expect_decide \
    'decisions are independent per PR' \
    'SKIP 7 already-reviewed
REVIEW 8 beefff
SKIP 9 draft' \
    '7 cccccc\n' \
    '[{"number":7,"headRefOid":"cccccc","isDraft":false,"labels":[]},{"number":8,"headRefOid":"beefff","isDraft":false,"labels":[]},{"number":9,"headRefOid":"dddddd","isDraft":true,"labels":[]}]'

# --- verdict-label: verdict text to review label -----------------------------

expect_label \
    'Changes Requested verdict maps to review:changes-requested' \
    'review:changes-requested' \
    '## Code Review: Round 1\n\nblocking P1: scripts/x.sh:12 — input Y crashes.\n\n結論：Changes Requested\n'

expect_label \
    'LGTM verdict maps to review:lgtm' \
    'review:lgtm' \
    '## Code Review: Round 1\n\nP0=0, P1=0.\n\n結論：LGTM\n'

expect_label \
    'when both verdicts appear, the last one wins' \
    'review:lgtm' \
    'Round 1 結論：Changes Requested\n\nRound 2（fix 後）：P0=0, P1=0。結論：LGTM\n'

expect_label \
    'no verdict keyword maps to no label' \
    '' \
    '## Code Review: Round 1\n\n審查中斷，無結論。\n'

printf 'pr-review-watch fixtures passed\n'
