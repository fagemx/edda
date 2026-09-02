#!/bin/sh
# Offline fixtures for the pr-review watcher (ported from PR #639, extended in
# Round 2): review-or-skip decisions from `gh pr list --jq @tsv` rows, verdict
# text -> review:* label mapping, unreviewed-is-per-head semantics, verdict
# label only on the reviewed head, and ack retry with a stubbed gh.
# Style follows scripts/test-lint-markdown-content.sh — no new tooling.
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' 0 HUP INT TERM
case_number=0

# --- decide: PR queue triage from `gh pr list --jq @tsv` rows -----------------
# Each input line: number<TAB>headRefOid<TAB>labels(joined by ",")<TAB>updatedAt
expect_decide() {
    name=$1
    expected=$2
    state_lines=$3
    rows=$4
    case_number=$((case_number + 1))
    state="$tmp/state-$case_number"
    printf '%b' "$state_lines" >"$state"
    if ! actual=$(printf '%b' "$rows" | \
        PR_REVIEW_WATCH_STATE="$state" sh "$root/scripts/pr-review-watch.sh" decide); then
        printf '%s: decide exited non-zero\n' "$name" >&2
        return 1
    fi
    if [ "$actual" != "$expected" ]; then
        printf '%s: expected\n  %s\ngot\n  %s\n' "$name" "$expected" "$actual" >&2
        return 1
    fi
}

# --- verdict-label: verdict text to review label ------------------------------

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

# --- label-verdict: apply the verdict label only on the reviewed head ---------

expect_label_verdict() {
    name=$1
    expected=$2
    reviewed=$3
    current=$4
    case_number=$((case_number + 1))
    if ! actual=$(sh "$root/scripts/pr-review-watch.sh" label-verdict "$reviewed" "$current"); then
        printf '%s: label-verdict exited non-zero\n' "$name" >&2
        return 1
    fi
    if [ "$actual" != "$expected" ]; then
        printf '%s: expected %s, got %s\n' "$name" "$expected" "$actual" >&2
        return 1
    fi
}

# --- ack: retried acknowledgement with a stubbed gh ---------------------------

# stub gh: `pr comment` fails once (GH_FAIL_FIRST marker) or always
# (GH_FAIL_ALWAYS), everything else succeeds; every call is logged.
STUBBIN="$tmp/bin"
mkdir -p "$STUBBIN"
cat >"$STUBBIN/gh" <<'EOF'
#!/bin/sh
echo "gh $*" >>"$GH_STUB_LOG"
if [ "$1 $2" = "pr comment" ]; then
    if [ -n "${GH_FAIL_FIRST:-}" ] && [ -f "$GH_FAIL_FIRST" ]; then
        rm -f "$GH_FAIL_FIRST"
        exit 1
    fi
    if [ -n "${GH_FAIL_ALWAYS:-}" ]; then
        exit 1
    fi
    exit 0
fi
exit 0
EOF
chmod +x "$STUBBIN/gh"

export GH_STUB_LOG="$tmp/gh-stub.log"
: >"$GH_STUB_LOG"
export PATH="$STUBBIN:$PATH"
export PR_REVIEW_WATCH_ACKS="$tmp/acks.tsv"

# first `pr comment` call fails, subsequent ones succeed
touch "$tmp/fail-first"
export GH_FAIL_FIRST="$tmp/fail-first"

expect_ack() {
    name=$1
    expected_rc=$2
    pr=$3
    sha=$4
    attempts=$5
    case_number=$((case_number + 1))
    rc=0
    sh "$root/scripts/pr-review-watch.sh" ack-try "$pr" "$sha" "$attempts" || rc=$?
    if [ "$rc" != "$expected_rc" ]; then
        printf '%s: expected rc %s, got %s\n' "$name" "$expected_rc" "$rc" >&2
        return 1
    fi
}

ack_state() {
    cat "$PR_REVIEW_WATCH_ACKS" 2>/dev/null || true
}

# --- decide -------------------------------------------------------------------

expect_decide \
    'new PR is queued for review' \
    'REVIEW 42 abc123' \
    '' \
    '42\tabc123\t\t2026-09-02T00:00:00Z'

expect_decide \
    'same SHA already reviewed is skipped' \
    'SKIP 42 already-reviewed' \
    '42\tabc123\n' \
    '42\tabc123\t\t2026-09-02T00:00:00Z'

expect_decide \
    'new head SHA after push is reviewed again' \
    'REVIEW 42 def456' \
    '42\tabc123\n' \
    '42\tdef456\t\t2026-09-02T00:00:00Z'

expect_decide \
    'labels are scoped per PR: unlabeled PR 1 is REVIEW while PR 2 has review:unreviewed' \
    'REVIEW 1 aaa111
SKIP 2 review-unreviewed' \
    '' \
    '1\taaa111\t\t2026-09-02T00:00:00Z\n2\tbbb222\treview:unreviewed\t2026-09-02T00:00:00Z'

expect_decide \
    'review:unreviewed blocks the head it was recorded for' \
    'SKIP 42 review-unreviewed' \
    '42\tabc123\t2\n' \
    '42\tabc123\treview:unreviewed\t2026-09-02T00:00:00Z'

expect_decide \
    'a new head after review:unreviewed is reviewed again and drops the stale label' \
    'REVIEW 42 def456 drop-unreviewed-label' \
    '42\tabc123\t2\n' \
    '42\tdef456\treview:unreviewed\t2026-09-02T00:00:00Z'

expect_decide \
    'review:unreviewed with no recorded head still blocks' \
    'SKIP 42 review-unreviewed' \
    '' \
    '42\tabc123\treview:unreviewed\t2026-09-02T00:00:00Z'

expect_decide \
    'empty open-PR queue decides nothing' \
    '' \
    '' \
    ''

expect_decide \
    'missing head SHA is skipped, not queued' \
    'SKIP 42 missing-head' \
    '' \
    '42\t\t\t2026-09-02T00:00:00Z'

expect_decide \
    'decisions are independent per PR' \
    'SKIP 7 already-reviewed
REVIEW 8 beefff
SKIP 9 review-unreviewed' \
    '7\tcccccc\n' \
    '7\tcccccc\t\t2026-09-02T00:00:00Z\n8\tbeefff\t\t2026-09-02T00:00:00Z\n9\tdddddd\treview:unreviewed\t2026-09-02T00:00:00Z'

# --- verdict-label ------------------------------------------------------------

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

# --- label-verdict ------------------------------------------------------------

expect_label_verdict \
    'verdict label applies when current head equals the reviewed SHA' \
    'apply' \
    'aaa111' \
    'aaa111'

expect_label_verdict \
    'verdict label is skipped when the head has moved' \
    'skip' \
    'aaa111' \
    'bbb222'

expect_label_verdict \
    'verdict label is skipped when the current head is unknown' \
    'skip' \
    'aaa111' \
    ''

# --- ack retry ----------------------------------------------------------------

expect_ack 'ack failure (attempt 0) is reported, not acked' 1 42 abc123 0
if [ "$(ack_state)" != "$(printf '42\tabc123\t1')" ]; then
    printf 'ack: state after failed attempt should record "launched, ack pending", got:\n%s\n' \
        "$(ack_state)" >&2
    exit 1
fi
if ! grep -q 'gh pr comment 42' "$GH_STUB_LOG"; then
    printf 'ack: stub log should contain the ack comment call\n' >&2
    exit 1
fi

expect_ack 'ack succeeds on the next poll and the pending entry is cleared' 0 42 abc123 1
if [ -n "$(ack_state)" ]; then
    printf 'ack: state should be empty after a successful ack, got:\n%s\n' "$(ack_state)" >&2
    exit 1
fi

export GH_FAIL_ALWAYS=1
expect_ack 'ack exhausted after the bound is labeled review:post-failed' 2 42 def456 2
if [ -n "$(ack_state)" ]; then
    printf 'ack: exhausted entry should be dropped from the acks file, got:\n%s\n' "$(ack_state)" >&2
    exit 1
fi
if ! grep -q 'review:post-failed' "$GH_STUB_LOG"; then
    printf 'ack: exhausted ack should label review:post-failed\n' >&2
    exit 1
fi
unset GH_FAIL_ALWAYS

printf 'pr-review-watch fixtures passed\n'
