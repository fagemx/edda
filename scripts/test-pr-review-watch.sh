#!/bin/sh
# Offline fixtures for the pr-review watcher (ported from PR #639, extended in
# Rounds 2–3): review-or-skip decisions from `gh pr list --jq @tsv` rows,
# verdict text -> review:* label mapping, per-head review:unreviewed semantics,
# verdict label only on the reviewed head, retried acknowledgement with a
# stubbed gh (including its terminal failure path), and the provider probe
# before the single pi retry.
#
# Everything is offline: EDDA_FLEET_SCRATCH, PR_REVIEW_WATCH_LOG and all state
# files are redirected into a temp dir, and `gh`/`pi`/review-pr are stubs —
# the real ~/.edda/fleet/watch.log must be byte-for-byte unchanged across a
# full run (guarded below).
# Style follows scripts/test-lint-markdown-content.sh — no new tooling.
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' 0 HUP INT TERM

REAL_WATCHLOG="$HOME/.edda/fleet/watch.log"
size_before=0
[ -f "$REAL_WATCHLOG" ] && size_before=$(stat -c %s "$REAL_WATCHLOG")

case_number=0

# --- offline guarantees -------------------------------------------------------
export EDDA_FLEET_SCRATCH="$tmp/scratch"
export PR_REVIEW_WATCH_LOG="$tmp/watch.log"
mkdir -p "$EDDA_FLEET_SCRATCH"

# --- stubs: gh, pi, review-pr -------------------------------------------------
STUBBIN="$tmp/bin"
mkdir -p "$STUBBIN"
cat >"$STUBBIN/gh" <<'EOF'
#!/bin/sh
echo "gh $*" >>"$GH_STUB_LOG"
case "$1" in
  pr)
    case "$2" in
      list)
        [ -n "${GH_PR_LIST_FILE:-}" ] && cat "$GH_PR_LIST_FILE"
        exit 0
        ;;
      comment)
        if [ -n "${GH_FAIL_COMMENT_FIRST:-}" ] && [ -f "$GH_FAIL_COMMENT_FIRST" ]; then
            rm -f "$GH_FAIL_COMMENT_FIRST"
            exit 1
        fi
        [ -n "${GH_FAIL_COMMENT_ALWAYS:-}" ] && exit 1
        exit 0
        ;;
      edit)
        [ -n "${GH_FAIL_EDIT:-}" ] && exit 1
        exit 0
        ;;
      view)
        case "$*" in
          *headRefOid*)
            [ -n "${GH_FAIL_HEAD:-}" ] && exit 1
            if [ -n "${GH_HEAD_FILE:-}" ]; then cat "$GH_HEAD_FILE"; else echo "${GH_HEAD:-}"; fi
            exit 0
            ;;
          *state*)
            echo "OPEN"
            exit 0
            ;;
        esac
        exit 0
        ;;
    esac
    exit 0
    ;;
  label) exit 0 ;;
esac
exit 0
EOF
cat >"$STUBBIN/pi" <<'EOF'
#!/bin/sh
echo "pi $*" >>"$PI_STUB_LOG"
case "$*" in
  *--thinking\ minimal*)
    [ -n "${PI_FAIL_PROBE:-}" ] && exit 1
    exit 0
    ;;
esac
exit 0
EOF
cat >"$STUBBIN/review-pr-stub" <<'EOF'
#!/bin/sh
echo "REVIEW_LAUNCH $*" >>"$REVIEW_STUB_LOG"
exit 0
EOF
chmod +x "$STUBBIN/gh" "$STUBBIN/pi" "$STUBBIN/review-pr-stub"

export GH_STUB_LOG="$tmp/gh-stub.log"
export PI_STUB_LOG="$tmp/pi-stub.log"
export REVIEW_STUB_LOG="$tmp/review-stub.log"
: >"$GH_STUB_LOG"; : >"$PI_STUB_LOG"; : >"$REVIEW_STUB_LOG"
export PATH="$STUBBIN:$PATH"

reset_stubs() {
    : >"$GH_STUB_LOG"; : >"$PI_STUB_LOG"; : >"$REVIEW_STUB_LOG"
    unset GH_FAIL_COMMENT_FIRST GH_FAIL_COMMENT_ALWAYS GH_FAIL_EDIT GH_FAIL_HEAD \
          GH_PR_LIST_FILE GH_HEAD GH_HEAD_FILE PI_FAIL_PROBE 2>/dev/null || true
    rm -f "$EDDA_FLEET_SCRATCH"/review-* 2>/dev/null || true
    : >"$EDDA_FLEET_SCRATCH/review-state.tsv"
    : >"$EDDA_FLEET_SCRATCH/review-acks.tsv"
    : >"$EDDA_FLEET_SCRATCH/review-pending.tsv"
    : >"$EDDA_FLEET_SCRATCH/review-fails.tsv"
}

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
        PR_REVIEW_WATCH_STATE="$state" timeout 60 sh "$root/scripts/pr-review-watch.sh" decide); then
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
    if ! actual=$(printf '%b' "$body" | timeout 60 sh "$root/scripts/review-pr.sh" verdict-label); then
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
    if ! actual=$(timeout 60 sh "$root/scripts/pr-review-watch.sh" label-verdict </dev/null "$reviewed" "$current"); then
        printf '%s: label-verdict exited non-zero\n' "$name" >&2
        return 1
    fi
    if [ "$actual" != "$expected" ]; then
        printf '%s: expected %s, got %s\n' "$name" "$expected" "$actual" >&2
        return 1
    fi
}

# --- ack: retried acknowledgement with a stubbed gh ---------------------------

expect_ack() {
    name=$1
    expected_rc=$2
    pr=$3
    sha=$4
    attempts=$5
    case_number=$((case_number + 1))
    rc=0
    timeout 60 sh "$root/scripts/pr-review-watch.sh" ack-try "$pr" "$sha" "$attempts" </dev/null || rc=$?
    if [ "$rc" != "$expected_rc" ]; then
        printf '%s: expected rc %s, got %s\n' "$name" "$expected_rc" "$rc" >&2
        return 1
    fi
}

ack_state() {
    cat "$EDDA_FLEET_SCRATCH/review-acks.tsv" 2>/dev/null || true
}

# --- live loop: run one watcher cycle against the stubs -----------------------

run_watch_once() {
    PR_REVIEW_WATCH_REVIEW_PR="$STUBBIN/review-pr-stub" \
        timeout 120 sh "$root/scripts/pr-review-watch.sh" --once </dev/null
}

pending_set() { # pr round sha attempts postfails
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" "$(date +%s)" "$5" \
        >"$EDDA_FLEET_SCRATCH/review-pending.tsv"
}

pending_get() { cat "$EDDA_FLEET_SCRATCH/review-pending.tsv" 2>/dev/null || true; }
state_get()   { cat "$EDDA_FLEET_SCRATCH/review-state.tsv" 2>/dev/null || true; }

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
# The label comes from the Verdict line of REVIEW.md §7, not from the last
# verdict keyword in the text: "last keyword wins" labelled PR #655 review:lgtm
# on an open P0 because the prose ended "should carry this to LGTM" (#697). The
# bodies below therefore carry the §7 `### Verdict` heading, and the both-appear
# case now asserts the Verdict line beats the prose rather than the reverse.
# scripts/test-review-pr.sh covers the rule itself, PR #655 fixture included.

expect_label \
    'Changes Requested verdict maps to review:changes-requested' \
    'review:changes-requested' \
    '## Code Review: Round 1\n\nblocking P1: scripts/x.sh:12 — input Y crashes.\n\n### Verdict\nChanges Requested, P0=0, P1=1\n'

expect_label \
    'LGTM verdict maps to review:lgtm' \
    'review:lgtm' \
    '## Code Review: Round 1\n\nP0=0, P1=0.\n\n### Verdict\nLGTM (P0=0, P1=0)\n'

expect_label \
    'when both verdicts appear, the Verdict line wins over the prose' \
    'review:changes-requested' \
    '## Code Review: Round 2\n\n### Verdict\nChanges Requested, P0=1, P1=0\n\n修掉這一項就可以 LGTM。\n'

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

# first `pr comment` call fails, subsequent ones succeed
touch "$tmp/fail-first"
export GH_FAIL_COMMENT_FIRST="$tmp/fail-first"

expect_ack 'ack failure (attempt 0) is reported, not acked' 1 42 abc123 0
if [ "$(ack_state)" != "$(printf '42\tabc123\t1\t')" ]; then
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
unset GH_FAIL_COMMENT_FIRST

# terminal ack failure: comment fails after the bound AND the label call fails
export GH_FAIL_COMMENT_ALWAYS=1
export GH_FAIL_EDIT=1
expect_ack 'ack exhausted with a failing label call keeps its durable record' 2 42 def456 2
if [ "$(ack_state)" != "$(printf '42\tdef456\t3\t')" ]; then
    printf 'ack: terminal attempt with failing label must KEEP the unmarked entry, got:\n%s\n' "$(ack_state)" >&2
    exit 1
fi
if ! grep -q 'review:post-failed label failed' "$PR_REVIEW_WATCH_LOG"; then
    printf 'ack: terminal attempt with failing label must log the failure\n' >&2
    exit 1
fi

# gh recovers: the next poll applies the label and marks the entry post-failed
unset GH_FAIL_EDIT
expect_ack 'ack terminal attempt applies review:post-failed and marks the entry' 2 42 def456 3
if [ "$(ack_state)" != "$(printf '42\tdef456\t4\tpost-failed')" ]; then
    printf 'ack: applied terminal state should mark the entry post-failed, got:\n%s\n' "$(ack_state)" >&2
    exit 1
fi
unset GH_FAIL_COMMENT_ALWAYS

# --- live loop: unknown head stays pending (P0) --------------------------------

reset_stubs
sha=d29dc8f5861322ed664e39900273b0681396da50
pending_set 42 1 "$sha" 0 0
printf '## Code Review: Round 1 — PR #42 @ %s (gpt-5.6-sol, read-only)\n\n### Verdict\nLGTM (P0=0, P1=0)\n' "$sha" \
    >"$EDDA_FLEET_SCRATCH/review-pr42-r1-verdict.md.posted"
export GH_FAIL_HEAD=1
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (unknown head)\n' >&2; exit 1; }
if [ -z "$(pending_get)" ]; then
    printf 'live: unknown head must KEEP the pending entry for retry, got empty\n' >&2
    exit 1
fi
if [ -n "$(state_get)" ]; then
    printf 'live: unknown head must NOT record the head as reviewed, got:\n%s\n' "$(state_get)" >&2
    exit 1
fi
if grep -qF -- '--add-label review:lgtm' "$GH_STUB_LOG"; then
    printf 'live: unknown head must not apply the verdict label\n' >&2
    exit 1
fi
if ! grep -q 'head unknown, retry' "$PR_REVIEW_WATCH_LOG"; then
    printf 'live: unknown head must log "head unknown, retry"\n' >&2
    exit 1
fi

# gh recovers and the head still matches: label applied, state recorded
unset GH_FAIL_HEAD
export GH_HEAD="$sha"
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (head recovered)\n' >&2; exit 1; }
if [ "$(state_get)" != "$(printf '42\t%s\t1' "$sha")" ]; then
    printf 'live: head recovered should record reviewed, got:\n%s\n' "$(state_get)" >&2
    exit 1
fi
if ! grep -qF -- '--add-label review:lgtm' "$GH_STUB_LOG"; then
    printf 'live: head recovered should apply the verdict label\n' >&2
    exit 1
fi

# --- live loop: provider probe before the single pi retry (P1) -----------------

reset_stubs
pending_set 42 1 "$sha" 0 0
printf 'PI_EXIT=0\n' >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
printf 'Codex error: our servers are currently overloaded, please try again later.\n' \
    >"$EDDA_FLEET_SCRATCH/review-pr42-r1.log"

export PI_FAIL_PROBE=1
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (probe fails)\n' >&2; exit 1; }
if [ -n "$(pending_get)" ]; then
    printf 'live: failed probe should stop for that head (pending dropped), got:\n%s\n' "$(pending_get)" >&2
    exit 1
fi
if [ "$(state_get)" != "$(printf '42\t%s\t1' "$sha")" ]; then
    printf 'live: failed probe should record review:unreviewed state, got:\n%s\n' "$(state_get)" >&2
    exit 1
fi
if [ -n "$(cat "$REVIEW_STUB_LOG")" ]; then
    printf 'live: failed probe must NOT launch a second review, got:\n%s\n' "$(cat "$REVIEW_STUB_LOG")" >&2
    exit 1
fi
if ! grep -qF -- '--add-label review:unreviewed' "$GH_STUB_LOG"; then
    printf 'live: failed probe should label review:unreviewed\n' >&2
    exit 1
fi
unset PI_FAIL_PROBE

reset_stubs
pending_set 42 1 "$sha" 0 0
printf 'PI_EXIT=0\n' >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
printf 'Codex error: our servers are currently overloaded, please try again later.\n' \
    >"$EDDA_FLEET_SCRATCH/review-pr42-r1.log"
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (probe OK)\n' >&2; exit 1; }
if [ "$(grep -c 'thinking minimal' "$PI_STUB_LOG")" -lt 1 ]; then
    printf 'live: the --thinking minimal probe must run before the retry\n' >&2
    exit 1
fi
if ! grep -q 'thinking minimal.*--exclude-tools edit,write' "$PI_STUB_LOG"; then
    printf 'live: the provider probe must be read-only (--exclude-tools edit,write)\n' >&2
    exit 1
fi
if [ "$(grep -c '^REVIEW_LAUNCH' "$REVIEW_STUB_LOG")" != "1" ]; then
    printf 'live: a passing probe must lead to exactly one pi retry, got:\n%s\n' "$(cat "$REVIEW_STUB_LOG")" >&2
    exit 1
fi
if [ -z "$(pending_get)" ]; then
    printf 'live: after a launched retry the pending entry should remain (attempts=1), got empty\n' >&2
    exit 1
fi
if [ "$(printf '%s' "$(pending_get)" | cut -f4)" != "1" ]; then
    printf 'live: pending attempts should be 1 after the retry, got:\n%s\n' "$(pending_get)" >&2
    exit 1
fi

# --- offline guarantee: the real watcher log was never touched -----------------
size_after=0
[ -f "$REAL_WATCHLOG" ] && size_after=$(stat -c %s "$REAL_WATCHLOG")
if [ "$size_before" != "$size_after" ]; then
    printf 'offline guarantee violated: %s grew from %s to %s bytes\n' \
        "$REAL_WATCHLOG" "$size_before" "$size_after" >&2
    exit 1
fi

printf 'pr-review-watch fixtures passed\n'
