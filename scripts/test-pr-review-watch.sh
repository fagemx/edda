#!/bin/sh
# Offline fixtures for the pr-review watcher (ported from PR #639, extended in
# Rounds 2–3): review-or-skip decisions from `gh pr list --jq @tsv` rows,
# verdict text -> review:* label mapping, per-head review:unreviewed semantics,
# verdict label only on the reviewed head, retried acknowledgement with a
# stubbed gh (including its terminal failure path), and the provider probe
# before the single dispatch retry.
#
# Everything is offline: EDDA_FLEET_SCRATCH, PR_REVIEW_WATCH_LOG and all state
# files are redirected into a temp dir, and `gh`/`edda`/review-pr are stubs —
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
          *"--json comments"*)
            [ -n "${GH_FAIL_COMMENTS:-}" ] && exit 1
            [ -n "${GH_COMMENTS_FILE:-}" ] && cat "$GH_COMMENTS_FILE"
            exit 0
            ;;
          *headRefOid*)
            [ -n "${GH_FAIL_HEAD:-}" ] && exit 1
            if [ -n "${GH_HEAD_FILE:-}" ]; then cat "$GH_HEAD_FILE"; else echo "${GH_HEAD:-}"; fi
            exit 0
            ;;
          *state*)
            [ -n "${GH_FAIL_STATE:-}" ] && exit 1
            if [ -n "${GH_STATE:-}" ]; then echo "$GH_STATE"; else echo "OPEN"; fi
            exit 0
            ;;
        esac
        exit 0
        ;;
    esac
    exit 0
    ;;
  api)
    [ -n "${GH_FAIL_STATUS:-}" ] && exit 1
    exit 0
    ;;
  label) exit 0 ;;
esac
exit 0
EOF
cat >"$STUBBIN/pi" <<'EOF'
#!/bin/sh
echo "pi $*" >>"$PI_STUB_LOG"
exit 0
EOF
cat >"$STUBBIN/edda" <<'EOF'
#!/bin/sh
echo "edda $*" >>"$EDDA_STUB_LOG"
case "$*" in
  'dispatch --help') echo '--tools <TOOLS> --exclude-tools <EXCLUDE_TOOLS> --permission-mode <MODE>'; exit 0 ;;
  *--agent*claude*)
    [ -n "${DISPATCH_FAIL_PROBE:-}" ] && exit 1
    exit 0
    ;;
esac
exit 0
EOF
cat >"$STUBBIN/claude" <<'EOF'
#!/bin/sh
test "$*" = '--help' || exit 99
echo '--tools <tools> --disallowedTools <tools> --permission-mode <mode>'
EOF
cat >"$STUBBIN/review-pr-stub" <<'EOF'
#!/bin/sh
echo "REVIEW_LAUNCH $*" >>"$REVIEW_STUB_LOG"
echo "review_round=${REVIEW_STUB_ROUND:-$2}"
exit 0
EOF
chmod +x "$STUBBIN/gh" "$STUBBIN/pi" "$STUBBIN/edda" "$STUBBIN/claude" "$STUBBIN/review-pr-stub"

export GH_STUB_LOG="$tmp/gh-stub.log"
export PI_STUB_LOG="$tmp/pi-stub.log"
export EDDA_STUB_LOG="$tmp/edda-stub.log"
export REVIEW_STUB_LOG="$tmp/review-stub.log"
: >"$GH_STUB_LOG"; : >"$PI_STUB_LOG"; : >"$EDDA_STUB_LOG"; : >"$REVIEW_STUB_LOG"
export PATH="$STUBBIN:$PATH"

reset_stubs() {
    : >"$GH_STUB_LOG"; : >"$PI_STUB_LOG"; : >"$EDDA_STUB_LOG"; : >"$REVIEW_STUB_LOG"
    unset GH_FAIL_COMMENT_FIRST GH_FAIL_COMMENT_ALWAYS GH_FAIL_EDIT GH_FAIL_HEAD \
          GH_PR_LIST_FILE GH_HEAD GH_HEAD_FILE DISPATCH_FAIL_PROBE \
          GH_COMMENTS_FILE GH_FAIL_STATUS GH_FAIL_COMMENTS 2>/dev/null || true
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

# --- gate-state: union rule for the Independent Review commit status ----------
# Input: one verdict per line, `verdict<TAB>p0<TAB>p1` (blank lines ignored).
# Output, exactly one word: success only when at least one verdict is
# LGTM P0=0 P1=0 and no other verdict on the input is anything else; failure
# when any verdict is present and does not qualify (missing or non-numeric
# counts count as non-zero); error when there are no verdict lines at all.

expect_gate_state() {
    name=$1
    expected=$2
    input=$3
    case_number=$((case_number + 1))
    if ! actual=$(printf '%b' "$input" | \
        timeout 60 sh "$root/scripts/pr-review-watch.sh" gate-state); then
        printf '%s: gate-state exited non-zero\n' "$name" >&2
        return 1
    fi
    if [ "$actual" != "$expected" ]; then
        printf '%s: expected %s, got %s\n' "$name" "$expected" "$actual" >&2
        return 1
    fi
}

expect_gate_state 'no verdict lines at all' 'error' ''
expect_gate_state 'blank lines only are still no verdicts' 'error' '\n\n'
expect_gate_state 'one LGTM 0 0' 'success' 'LGTM\t0\t0\n'
expect_gate_state 'one Changes Requested' 'failure' 'Changes Requested\t0\t3\n'
expect_gate_state 'a later LGTM does not override an earlier Changes Requested (union rule)' \
    'failure' \
    'Changes Requested\t0\t3\nLGTM\t0\t0\n'
expect_gate_state 'an earlier LGTM does not pre-clear a later Changes Requested' \
    'failure' \
    'LGTM\t0\t0\nChanges Requested\t0\t3\n'
expect_gate_state 'LGTM with P0=1 does not qualify' 'failure' 'LGTM\t1\t0\n'
expect_gate_state 'LGTM with P1=1 does not qualify' 'failure' 'LGTM\t0\t1\n'
expect_gate_state 'blank lines mixed in are ignored' 'success' '\nLGTM\t0\t0\n\n\n'
expect_gate_state 'two qualifying LGTMs are success' 'success' 'LGTM\t0\t0\nLGTM\t0\t0\n'
expect_gate_state 'a missing count is non-zero, never success' 'failure' 'LGTM\t0\n'
expect_gate_state 'a non-numeric count is non-zero, never success' 'failure' 'LGTM\tx\ty\n'
expect_gate_state 'an unknown verdict word is failure' 'failure' 'Needs Discussion\t0\t0\n'

# --- collect-verdicts: read the §7 verdict comments pinned to one SHA ---------
# The fixture holds the OUTPUT of the gh --jq pipeline (sentinel + raw
# bodies), the same shape the real gh prints — consistent with
# GH_PR_LIST_FILE, which also stores jq output rather than raw JSON.

verdict_comment() { # $1=round $2=sha $3=verdict line — the shape the watcher posts
    # R23 (#917): the posted comment IS the report — the §7 heading is the
    # first line, no watcher banner before it.
    printf '<<<COMMENT>>>\n## Code Review: Round %s — PR #42 @ %s\n\n### Verdict\n%s\n' \
        "$1" "$2" "$3"
}

expect_collect() {
    name=$1
    expected=$2
    file=$3
    csha=$4
    case_number=$((case_number + 1))
    if ! actual=$(GH_COMMENTS_FILE="$file" timeout 60 \
        sh "$root/scripts/pr-review-watch.sh" collect-verdicts 42 "$csha"); then
        printf '%s: collect-verdicts exited non-zero\n' "$name" >&2
        return 1
    fi
    if [ "$actual" != "$expected" ]; then
        printf '%s: expected\n  %s\ngot\n  %s\n' "$name" "$expected" "$actual" >&2
        return 1
    fi
}

csha1=111122223333444455556666777788889999aaaa
csha2=9999888877776666555544443333222211110000

f="$tmp/comments-two-shas"
{
    verdict_comment 1 "$csha1" 'Changes Requested, P0=0, P1=3 — blocking P1: input Y crashes'
    verdict_comment 2 "$csha2" 'LGTM (P0=0, P1=0)'
    printf '<<<COMMENT>>>\nquestion about the diff, no verdict here\n'
} >"$f"
expect_collect 'only verdict comments pinned to the reviewed sha are collected' \
    "$(printf 'Changes Requested\t0\t3')" "$f" "$csha1"
expect_collect 'the other sha collects its own verdict' \
    "$(printf 'LGTM\t0\t0')" "$f" "$csha2"

f="$tmp/comments-malformed"
verdict_comment 1 "$csha1" 'LGTM' >"$f"
expect_collect 'a verdict line without counts yields empty count fields (never success)' \
    "$(printf 'LGTM\t\t')" "$f" "$csha1"

f="$tmp/comments-prose"
printf '<<<COMMENT>>>\n## Code Review: Round 1 — PR #42 @ %s\n\n### Verdict\n\n修掉這一項就可以 LGTM。\n' \
    "$csha1" >"$f"
expect_collect 'prose that merely says LGTM is not a verdict line' \
    "$(printf 'LGTM\t\t')" "$f" "$csha1"

f="$tmp/comments-wrong-sha"
verdict_comment 1 "$csha2" 'Changes Requested, P0=1, P1=0 — x' >"$f"
expect_collect 'a verdict pinned to another sha contributes nothing' \
    '' "$f" "$csha1"

# T1: a failed comments fetch must be an error, never "no prior verdicts" —
# otherwise the union rule would run on this round's verdict alone.
expect_collect_fail() {
    name=$1
    csha=$2
    case_number=$((case_number + 1))
    out=$(GH_FAIL_COMMENTS=1 timeout 60 \
        sh "$root/scripts/pr-review-watch.sh" collect-verdicts 42 "$csha" 2>/dev/null) && rc=0 || rc=$?
    if [ "$rc" -eq 0 ]; then
        printf '%s: expected non-zero exit when the comments fetch fails, got 0\n' "$name" >&2
        return 1
    fi
    if [ -n "$out" ]; then
        printf '%s: expected no stdout when the comments fetch fails, got:\n  %s\n' "$name" "$out" >&2
        return 1
    fi
}

expect_collect_fail 'a failed comments fetch is an error, never "no prior verdicts"' "$csha1"

# T3: the reviewed sha is validated at the subcommand boundary before it can
# reach the pin regex or the status URL (REVIEW.md R5).
expect_collect_badsha() {
    name=$1
    value=$2
    case_number=$((case_number + 1))
    out=$(timeout 60 sh "$root/scripts/pr-review-watch.sh" collect-verdicts 42 "$value" \
        2>"$tmp/stderr-$case_number") && rc=0 || rc=$?
    if [ "$rc" != "2" ]; then
        printf '%s: expected exit 2 for a non-40-hex sha, got %s\n' "$name" "$rc" >&2
        return 1
    fi
    if [ -n "$out" ]; then
        printf '%s: expected no stdout for a non-40-hex sha, got:\n  %s\n' "$name" "$out" >&2
        return 1
    fi
    if ! grep -q 'not a full lowercase 40-hex SHA' "$tmp/stderr-$case_number"; then
        printf '%s: expected the validation error on stderr, got:\n  %s\n' \
            "$name" "$(cat "$tmp/stderr-$case_number")" >&2
        return 1
    fi
}

expect_collect_badsha 'a regex metacharacter is not a sha' '.*'
expect_collect_badsha 'a 39-hex value is not a sha' '111122223333444455556666777788889999aaa'
expect_collect_badsha 'an uppercase 40-hex value is not a sha' '111122223333444455556666777788889999AAAA'
expect_collect_badsha 'an alternation is not a sha' "${csha2}\$|${csha1}"

# --- the two debt blocks are exactly their four marker lines (GH-742) ---------
# The union rule block and the comment-reading block must each be liftable in
# one piece: two opening markers and two closing markers, nothing else.

wscript="$root/scripts/pr-review-watch.sh"
if [ "$(grep -c 'D8-debt' "$wscript")" != "4" ]; then
    printf 'D8 markers: expected exactly 4 lines (2 open + 2 close), got %s\n' \
        "$(grep -c 'D8-debt' "$wscript")" >&2
    exit 1
fi
if [ "$(grep -c '^# D8-debt(#769)' "$wscript")" != "1" ] || \
   [ "$(grep -c '^# D8-debt(#671)' "$wscript")" != "1" ] || \
   [ "$(grep -c '^# /D8-debt' "$wscript")" != "2" ]; then
    printf 'D8 markers: expected one #769 opener, one #671 opener, two closers\n' >&2
    exit 1
fi

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
printf '## Code Review: Round 1 — PR #42 @ %s\n\n### Verdict\nLGTM (P0=0, P1=0)\n' "$sha" \
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

# A product review can return an LGTM-shaped sentence with exit 3 when its
# qualification rules fail. Even a stale/generated envelope must not let that
# result become review:lgtm when the watcher consumes the receipt.
reset_stubs
pending_set 42 1 "$sha" 0 0
printf 'TRANSPORT=edda-review\nDISPATCH_EXIT=3\nQUALIFIED=false\nDISQUALIFIERS=gates-red,escalation-pending\n' \
    >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
printf '## Code Review: Round 1 — PR #42 @ %s\n\n### Verdict\nLGTM (P0=0, P1=0)\n' "$sha" \
    >"$EDDA_FLEET_SCRATCH/review-pr42-r1-verdict.md.posted"
export GH_HEAD="$sha"
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (unqualified product LGTM)\n' >&2; exit 1; }
if grep -qF -- '--add-label review:lgtm' "$GH_STUB_LOG"; then
    printf 'live: product exit 3 / qualified=false must never apply review:lgtm\n' >&2
    exit 1
fi
if ! grep -qF -- '--add-label review:unreviewed' "$GH_STUB_LOG"; then
    printf 'live: unqualified product LGTM must be labeled review:unreviewed\n' >&2
    exit 1
fi

# --- live loop: provider probe before the single dispatch retry (P1) ---------

reset_stubs
pending_set 42 1 "$sha" 0 0
printf 'DISPATCH_EXIT=0\n' >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
printf 'Codex error: our servers are currently overloaded, please try again later.\n' \
    >"$EDDA_FLEET_SCRATCH/review-pr42-r1.log"

export DISPATCH_FAIL_PROBE=1
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
unset DISPATCH_FAIL_PROBE

reset_stubs
pending_set 42 1 "$sha" 0 0
printf 'DISPATCH_EXIT=0\n' >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
printf 'Codex error: our servers are currently overloaded, please try again later.\n' \
    >"$EDDA_FLEET_SCRATCH/review-pr42-r1.log"
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (probe OK)\n' >&2; exit 1; }
if [ "$(grep -c 'agent claude' "$EDDA_STUB_LOG")" -lt 1 ]; then
    printf 'live: the edda dispatch probe must run before the retry\n' >&2
    exit 1
fi
if ! grep -q -- '--exclude-tools Edit,Write,NotebookEdit' "$EDDA_STUB_LOG"; then
    printf 'live: the provider probe must be read-only (--exclude-tools Edit,Write,NotebookEdit)\n' >&2
    exit 1
fi
if ! grep -q -- '--model claude-opus-5' "$EDDA_STUB_LOG"; then
    printf 'live: the provider probe must pin the same review model (--model claude-opus-5)\n' >&2
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

# --- live loop: the receipts the verdict header used to carry (GH-708, R23) ----
# Round 2 P1-1: the header used to hardcode `edda dispatch --agent claude` even
# when the oversized-brief claude-stdin fallback was the arm that ran. R23
# (#917) moves the receipts out of the PR comment entirely: the comment IS the
# §7 report, heading first, and the transport receipt lands in the watcher log
# — verbatim, or "unknown" when the receipt is missing, never the transport we
# wish had run.

header_comment_path() {
    sed -n 's/.*--body-file //p' "$GH_STUB_LOG" | tail -1
}

comment_head_is_heading() { # $1=comment file — R23: the heading is line 1
    head -1 "$1" | grep -q '^## Code Review: Round [0-9]* — PR #42 @ '
}

last_receipt_line() { # the receipts of the MOST RECENT posted verdict comment
    grep 'posted verdict comment' "$PR_REVIEW_WATCH_LOG" | tail -1
}

verdict_log_fixture() {
    # A completed review receipt is published as one final object. These lines
    # deliberately model the complete contract, not the old incremental
    # DISPATCH_EXIT/WORKTREE_CHECK shape that the watcher must fail closed.
    printf 'FINAL_EXIT=0\nWORKTREE_CHECK=unchanged\nWORKTREE_CLEANUP=removed\nTASK_CLEANUP=not-applicable\nTERMINAL_RECEIPT=complete\n' \
        >> "$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
    {
        printf '<<<VERDICT\n'
        printf '## Code Review: Round 1 — PR #42 @ %s\n\n### Verdict\nLGTM (P0=0, P1=0)\n' "$sha"
        printf 'VERDICT>>>\n'
        printf 'Model requested: claude-opus-5\nModel observed: claude-opus-5\nCost: $0.33\nSession: 11111111-2222-4333-8444-555555555555\n'
        # The backend's OWN report of the conversation it ran, which is what
        # the header cross-checks against the launched SESSION= (GH-708).
        # FIXTURE_NO_SESSION_OBSERVED=1 fixtures a reviewer that reported none.
        if [ "${FIXTURE_NO_SESSION_OBSERVED:-0}" != "1" ]; then
            printf 'Session observed: %s\n' \
                "${FIXTURE_SESSION_OBSERVED:-11111111-2222-4333-8444-555555555555}"
        fi
    } >"$EDDA_FLEET_SCRATCH/review-pr42-r1.log"
}

export GH_HEAD="$sha"

reset_stubs
pending_set 42 1 "$sha" 0 0
printf 'TRANSPORT=edda-dispatch\nDISPATCH_EXIT=0\n' >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
verdict_log_fixture
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (header: dispatch arm)\n' >&2; exit 1; }
cfile=$(header_comment_path)
[ -n "$cfile" ] || { printf 'live: a settled verdict should post a comment\n' >&2; exit 1; }
comment_head_is_heading "$cfile" || {
    printf 'live: R23 — the posted comment must begin with the §7 heading, got:\n%s\n' "$(head -1 "$cfile")" >&2
    exit 1
}
last_receipt_line | grep -qF 'transport edda dispatch --agent claude,' || {
    printf 'live: the log must name the edda-dispatch receipt, got:\n%s\n' "$(tail -3 "$PR_REVIEW_WATCH_LOG")" >&2
    exit 1
}

reset_stubs
pending_set 42 1 "$sha" 0 0
printf 'TRANSPORT=claude-stdin\nDISPATCH_EXIT=0\n' >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
verdict_log_fixture
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (header: fallback arm)\n' >&2; exit 1; }
cfile=$(header_comment_path)
comment_head_is_heading "$cfile" || {
    printf 'live: R23 — the posted comment must begin with the §7 heading, got:\n%s\n' "$(head -1 "$cfile")" >&2
    exit 1
}
last_receipt_line | grep -qF 'transport claude -p via stdin (oversized-brief fallback),' || {
    printf 'live: the log must name the claude-stdin fallback receipt, got:\n%s\n' "$(tail -3 "$PR_REVIEW_WATCH_LOG")" >&2
    exit 1
}
if last_receipt_line | grep -qF 'transport edda dispatch --agent claude,'; then
    printf 'live: the log must not claim edda dispatch when the claude-stdin fallback ran\n' >&2
    exit 1
fi

reset_stubs
pending_set 42 1 "$sha" 0 0
printf 'DISPATCH_EXIT=0\n' >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
verdict_log_fixture
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (header: missing receipt)\n' >&2; exit 1; }
cfile=$(header_comment_path)
comment_head_is_heading "$cfile" || {
    printf 'live: R23 — the posted comment must begin with the §7 heading, got:\n%s\n' "$(head -1 "$cfile")" >&2
    exit 1
}
last_receipt_line | grep -qF 'transport unknown — no TRANSPORT receipt in .done,' || {
    printf 'live: a missing TRANSPORT receipt must log as unknown, not a guessed transport, got:\n%s\n' "$(tail -3 "$PR_REVIEW_WATCH_LOG")" >&2
    exit 1
}
export GH_HEAD="$sha"

# --- live loop: the log names the reviewer conversation (GH-708, R23) ----------
# The per-PR reviewer session is what makes round 2+ a delta review, so the
# watcher log carries it. Two independent facts are compared, never merged:
# SESSION=/SESSION_MODE= are what review-pr.sh LAUNCHED with, and the log's
# `Session:` line is what the backend REPORTED. A disagreement means the resume
# forked into a fresh conversation, and the log must say so.

reset_stubs
pending_set 42 1 "$sha" 0 0
printf 'TRANSPORT=edda-dispatch\nSESSION=11111111-2222-4333-8444-555555555555\nSESSION_MODE=resume\nDISPATCH_EXIT=0\n' \
    >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
verdict_log_fixture
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (header: session)\n' >&2; exit 1; }
cfile=$(header_comment_path)
comment_head_is_heading "$cfile" || {
    printf 'live: R23 — the posted comment must begin with the §7 heading, got:\n%s\n' "$(head -1 "$cfile")" >&2
    exit 1
}
last_receipt_line | grep -qF 'reviewer_session `11111111-2222-4333-8444-555555555555` (resumed)' || {
    printf 'live: the log must name the resumed reviewer conversation, got:\n%s\n' "$(last_receipt_line)" >&2
    exit 1
}

reset_stubs
pending_set 42 1 "$sha" 0 0
printf 'TRANSPORT=edda-dispatch\nSESSION=11111111-2222-4333-8444-555555555555\nSESSION_MODE=resume\nDISPATCH_EXIT=0\n' \
    >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
FIXTURE_SESSION_OBSERVED=99999999-8888-4777-8666-555555555555
verdict_log_fixture
FIXTURE_SESSION_OBSERVED=
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (header: session mismatch)\n' >&2; exit 1; }
cfile=$(header_comment_path)
comment_head_is_heading "$cfile" || {
    printf 'live: R23 — the posted comment must begin with the §7 heading, got:\n%s\n' "$(head -1 "$cfile")" >&2
    exit 1
}
last_receipt_line | grep -qF 'BACKEND REPORTED `99999999-8888-4777-8666-555555555555`' || {
    printf 'live: a resume that ran in a different conversation must be reported, not hidden, got:\n%s\n' "$(tail -3 "$PR_REVIEW_WATCH_LOG")" >&2
    exit 1
}

# A reviewer that reported no session at all claims no agreement either: the
# launched id is rendered plainly, with no mismatch and no invented match.
reset_stubs
pending_set 42 1 "$sha" 0 0
printf 'TRANSPORT=edda-dispatch\nSESSION=11111111-2222-4333-8444-555555555555\nSESSION_MODE=resume\nDISPATCH_EXIT=0\n' \
    >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
FIXTURE_NO_SESSION_OBSERVED=1
verdict_log_fixture
FIXTURE_NO_SESSION_OBSERVED=0
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (header: no observation)\n' >&2; exit 1; }
cfile=$(header_comment_path)
comment_head_is_heading "$cfile" || {
    printf 'live: R23 — the posted comment must begin with the §7 heading, got:\n%s\n' "$(head -1 "$cfile")" >&2
    exit 1
}
grep -qF 'reviewer_session `11111111-2222-4333-8444-555555555555` (resumed)' "$PR_REVIEW_WATCH_LOG" || {
    printf 'live: a reviewer that observed no session must render the launched id plainly, got:\n%s\n' "$(tail -3 "$PR_REVIEW_WATCH_LOG")" >&2
    exit 1
}
if last_receipt_line | grep -qF 'BACKEND REPORTED'; then
    printf 'live: no observation is not a mismatch — the log must not claim one\n' >&2
    exit 1
fi

reset_stubs
pending_set 42 1 "$sha" 0 0
printf 'TRANSPORT=edda-dispatch\nDISPATCH_EXIT=0\n' >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
verdict_log_fixture
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (header: no session receipt)\n' >&2; exit 1; }
cfile=$(header_comment_path)
comment_head_is_heading "$cfile" || {
    printf 'live: R23 — the posted comment must begin with the §7 heading, got:\n%s\n' "$(head -1 "$cfile")" >&2
    exit 1
}
last_receipt_line | grep -qF 'mode unknown — no SESSION_MODE receipt in .done' || {
    printf 'live: a missing SESSION_MODE receipt must log as unknown, got:\n%s\n' "$(tail -3 "$PR_REVIEW_WATCH_LOG")" >&2
    exit 1
}
unset GH_HEAD

# The launcher can reserve a higher shared round than this watcher's local state.
reset_stubs
pending_set 42 1 "$sha" 0 0
printf 'DISPATCH_EXIT=1\n' >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
printf 'backend failed\n' >"$EDDA_FLEET_SCRATCH/review-pr42-r1.log"
export REVIEW_STUB_ROUND=8
run_watch_once >/dev/null 2>&1
[ "$(pending_get | cut -f2)" = 8 ] || { echo 'retry ignored shared round receipt' >&2; exit 1; }
unset REVIEW_STUB_ROUND

# --- live loop: the Independent Review commit status (GH-742) ------------------
# Posted to the REVIEWED sha (never the current head), state = the union rule
# over the §7 verdict comments on that sha plus this round's verdict, retried
# on the same bounded path as the comment (review:post-failed at the cap).

statuses_calls() { grep -c 'statuses/' "$GH_STUB_LOG" 2>/dev/null || true; }

# the head moved before the verdict settled: no status for anything
reset_stubs
pending_set 42 1 "$sha" 0 0
printf 'TRANSPORT=edda-dispatch\nDISPATCH_EXIT=0\nFINAL_EXIT=0\nWORKTREE_CHECK=unchanged\nWORKTREE_CLEANUP=removed\nTASK_CLEANUP=not-applicable\nTERMINAL_RECEIPT=complete\n' >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
verdict_log_fixture
export GH_HEAD=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (status: head moved)\n' >&2; exit 1; }
if [ "$(statuses_calls)" != "0" ]; then
    printf 'live: a moved head must not get a commit status, got %s calls\n' "$(statuses_calls)" >&2
    exit 1
fi

# LGTM with no other verdict on the sha: status success, then the label
reset_stubs
pending_set 42 1 "$sha" 0 0
printf 'TRANSPORT=edda-dispatch\nDISPATCH_EXIT=0\nFINAL_EXIT=0\nWORKTREE_CHECK=unchanged\nWORKTREE_CLEANUP=removed\nTASK_CLEANUP=not-applicable\nTERMINAL_RECEIPT=complete\n' >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
verdict_log_fixture
export GH_HEAD="$sha"
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (status: lgtm)\n' >&2; exit 1; }
if ! grep -qF "repos/fagemx/edda/statuses/$sha" "$GH_STUB_LOG"; then
    printf 'live: the status must be posted to the reviewed sha, got:\n%s\n' \
        "$(grep 'statuses/' "$GH_STUB_LOG")" >&2
    exit 1
fi
if ! grep -qF -- '-f state=success' "$GH_STUB_LOG"; then
    printf 'live: LGTM P0=0 P1=0 alone must post state=success, got:\n%s\n' \
        "$(grep 'statuses/' "$GH_STUB_LOG")" >&2
    exit 1
fi
if ! grep -qF -- '-f context=Independent Review' "$GH_STUB_LOG"; then
    printf 'live: the status context must be Independent Review\n' >&2
    exit 1
fi
if ! grep -qF -- '-f description=LGTM P0=0 P1=0' "$GH_STUB_LOG"; then
    printf 'live: the description must name the verdict, got:\n%s\n' \
        "$(grep 'statuses/' "$GH_STUB_LOG")" >&2
    exit 1
fi
if ! grep -qF -- '--add-label review:lgtm' "$GH_STUB_LOG"; then
    printf 'live: the verdict label should still be applied\n' >&2
    exit 1
fi
if [ "$(state_get)" != "$(printf '42\t%s\t1' "$sha")" ]; then
    printf 'live: a successful status + label should record reviewed, got:\n%s\n' "$(state_get)" >&2
    exit 1
fi
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (status: second cycle)\n' >&2; exit 1; }
if [ "$(statuses_calls)" != "1" ]; then
    printf 'live: the status must be posted once per verdict, got %s calls\n' "$(statuses_calls)" >&2
    exit 1
fi

# an earlier Changes Requested on the same sha keeps the union at failure even
# though this round's verdict is LGTM — the case the whole issue exists for
reset_stubs
pending_set 42 1 "$sha" 0 0
printf 'TRANSPORT=edda-dispatch\nDISPATCH_EXIT=0\nFINAL_EXIT=0\nWORKTREE_CHECK=unchanged\nWORKTREE_CLEANUP=removed\nTASK_CLEANUP=not-applicable\nTERMINAL_RECEIPT=complete\n' >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
verdict_log_fixture
printf '<<<COMMENT>>>\n## Code Review: Round 1 — PR #42 @ %s\n\n### Verdict\nChanges Requested, P0=0, P1=3 — blocking P1\n' \
    "$sha" >"$tmp/comments-pr42-earlier-cr"
export GH_HEAD="$sha"
export GH_COMMENTS_FILE="$tmp/comments-pr42-earlier-cr"
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (status: union)\n' >&2; exit 1; }
if ! grep -qF -- '-f state=failure' "$GH_STUB_LOG"; then
    printf 'live: a standing Changes Requested must hold the union at failure, got:\n%s\n' \
        "$(grep 'statuses/' "$GH_STUB_LOG")" >&2
    exit 1
fi
if ! grep -qF -- '--add-label review:lgtm' "$GH_STUB_LOG"; then
    printf 'live: the label still reflects the current LGTM verdict\n' >&2
    exit 1
fi

# posting the status is not best-effort: the comment path\'s bounded retry,
# and no label until the status is out
reset_stubs
pending_set 42 1 "$sha" 0 0
printf 'TRANSPORT=edda-dispatch\nDISPATCH_EXIT=0\nFINAL_EXIT=0\nWORKTREE_CHECK=unchanged\nWORKTREE_CLEANUP=removed\nTASK_CLEANUP=not-applicable\nTERMINAL_RECEIPT=complete\n' >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
verdict_log_fixture
export GH_HEAD="$sha"
export GH_FAIL_STATUS=1
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (status: post fails)\n' >&2; exit 1; }
if [ -z "$(pending_get)" ]; then
    printf 'live: a failed status post must keep the pending entry for retry, got empty\n' >&2
    exit 1
fi
if grep -qF -- '--add-label review:lgtm' "$GH_STUB_LOG"; then
    printf 'live: a failed status post must not apply the verdict label yet\n' >&2
    exit 1
fi
if ! grep -qF 'status post failed (attempt 1)' "$PR_REVIEW_WATCH_LOG"; then
    printf 'live: a failed status post must be logged\n' >&2
    exit 1
fi
unset GH_FAIL_STATUS
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (status: recovery)\n' >&2; exit 1; }
if ! grep -qF -- '-f state=success' "$GH_STUB_LOG"; then
    printf 'live: the status should succeed on the retry, got:\n%s\n' \
        "$(grep 'statuses/' "$GH_STUB_LOG")" >&2
    exit 1
fi
if ! grep -qF -- '--add-label review:lgtm' "$GH_STUB_LOG"; then
    printf 'live: the label should be applied after the status recovery\n' >&2
    exit 1
fi
if [ -n "$(pending_get)" ]; then
    printf 'live: after the status recovery the pending entry should be dropped, got:\n%s\n' "$(pending_get)" >&2
    exit 1
fi

# T2: an unreadable comment list withholds the status entirely — the union is
# never computed from this round's verdict file alone — and the failure lands
# on the same bounded retry path as any other status post failure.
reset_stubs
pending_set 42 1 "$sha" 0 0
printf 'TRANSPORT=edda-dispatch\nDISPATCH_EXIT=0\nFINAL_EXIT=0\nWORKTREE_CHECK=unchanged\nWORKTREE_CLEANUP=removed\nTASK_CLEANUP=not-applicable\nTERMINAL_RECEIPT=complete\n' >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
verdict_log_fixture
printf '<<<COMMENT>>>\n## Code Review: Round 1 — PR #42 @ %s\n\n### Verdict\nChanges Requested, P0=0, P1=3 — blocking P1\n' \
    "$sha" >"$tmp/comments-pr42-withheld"
export GH_HEAD="$sha"
export GH_COMMENTS_FILE="$tmp/comments-pr42-withheld"
export GH_FAIL_COMMENTS=1
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (status: comments unreadable)\n' >&2; exit 1; }
if [ "$(statuses_calls)" != "0" ]; then
    printf 'live: an unreadable comment list must not post any status, got %s calls\n' "$(statuses_calls)" >&2
    exit 1
fi
if grep -qF -- '--add-label review:lgtm' "$GH_STUB_LOG"; then
    printf 'live: an unreadable comment list must not apply the verdict label\n' >&2
    exit 1
fi
if [ -z "$(pending_get)" ]; then
    printf 'live: an unreadable comment list must keep the pending entry, got empty\n' >&2
    exit 1
fi
if [ "$(printf '%s' "$(pending_get)" | cut -f6)" != "1" ]; then
    printf 'live: an unreadable comment list must bump postfails by one, got:\n%s\n' "$(pending_get)" >&2
    exit 1
fi
if ! grep -qF 'comments unreadable' "$PR_REVIEW_WATCH_LOG"; then
    printf 'live: an unreadable comment list must be logged\n' >&2
    exit 1
fi
unset GH_FAIL_COMMENTS
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (status: comments recovered)\n' >&2; exit 1; }
if ! grep -qF -- '-f state=failure' "$GH_STUB_LOG"; then
    printf 'live: a healthy fetch must post the union failure, got:\n%s\n' \
        "$(grep 'statuses/' "$GH_STUB_LOG")" >&2
    exit 1
fi
if ! grep -qF -- '--add-label review:lgtm' "$GH_STUB_LOG"; then
    printf 'live: the label should be applied after the comments recover\n' >&2
    exit 1
fi

# --- R23 (#917): verdict shape and non-open PRs --------------------------------
# 1. the happy path is unchanged: a §7 carrier (heading first) posts the
#    comment, writes the status, and applies the label
# 2. #867's transcript dump (heading mid-body) is no verdict: no status, no
#    label, exactly one malformed notice, and the round does not settle this
#    poll (the per-PR failure signal)
# 3. the same dump on the next poll: no second notice; the union runs on this
#    round's LGTM alone and the round settles
# 4. a non-open PR is never launched and never acked (MERGED, CLOSED, and an
#    unreadable state all count as not open)

r23_dump_fixture() { # $1=comment id $2=sha — #867's shape: narration, ---, heading mid-body
    {
        printf '<<<COMMENT %s>>>\n' "$1"
        for i in 1 2 3 4 5 6 7 8 9 10 11 12; do printf 'reviewer narration line %s\n' "$i"; done
        printf -- '---\n'
        printf '## Code Review: Round 1 — PR #42 @ %s\n\n### Verdict\nLGTM (P0=0, P1=0)\n' "$2"
    } >"$tmp/comments-pr42-r23-dump"
}

r23_done_receipt() {
    printf 'TRANSPORT=edda-dispatch\nDISPATCH_EXIT=0\nFINAL_EXIT=0\nWORKTREE_CHECK=unchanged\nWORKTREE_CLEANUP=removed\nTASK_CLEANUP=not-applicable\nTERMINAL_RECEIPT=complete\n' \
        >"$EDDA_FLEET_SCRATCH/review-pr42-r1.done"
}

# case 1 — heading-first LGTM: the unchanged happy path
reset_stubs
pending_set 42 1 "$sha" 0 0
r23_done_receipt
verdict_log_fixture
export GH_HEAD="$sha"
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (r23 case 1)\n' >&2; exit 1; }
grep -qF -- '-f state=success' "$GH_STUB_LOG" || {
    printf 'live: r23 case 1 — a heading-first LGTM must write the status\n' >&2; exit 1
}
grep -qF -- '--add-label review:lgtm' "$GH_STUB_LOG" || {
    printf 'live: r23 case 1 — a heading-first LGTM must apply the label\n' >&2; exit 1
}
unset GH_HEAD

# case 2 — the transcript dump contributes nothing and draws exactly one notice
reset_stubs
pending_set 42 1 "$sha" 0 0
r23_done_receipt
verdict_log_fixture
r23_dump_fixture 7777001 "$sha"
export GH_HEAD="$sha"
export GH_COMMENTS_FILE="$tmp/comments-pr42-r23-dump"
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (r23 case 2)\n' >&2; exit 1; }
[ "$(statuses_calls)" = "0" ] || {
    printf 'live: r23 case 2 — a transcript dump must not let a status post, got %s calls\n' \
        "$(statuses_calls)" >&2; exit 1
}
if grep -qF -- '--add-label review:lgtm' "$GH_STUB_LOG"; then
    printf 'live: r23 case 2 — a transcript dump must not let the label apply\n' >&2; exit 1
fi
[ "$(grep -cF 'review: malformed verdict comment 7777001' "$GH_STUB_LOG")" = "1" ] || {
    printf 'live: r23 case 2 — exactly one malformed notice expected, got:\n%s\n' \
        "$(grep -F 'malformed verdict comment' "$GH_STUB_LOG")" >&2; exit 1
}
[ "$(printf '%s' "$(pending_get)" | cut -f6)" = "1" ] || {
    printf 'live: r23 case 2 — the round must not settle this poll (postfail bump), got:\n%s\n' \
        "$(pending_get)" >&2; exit 1
}

# case 3 — the next poll: notice not repeated, the union proceeds, the round settles
run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (r23 case 3)\n' >&2; exit 1; }
[ "$(grep -cF 'review: malformed verdict comment 7777001' "$GH_STUB_LOG")" = "1" ] || {
    printf 'live: r23 case 3 — the malformed notice must not repeat\n' >&2; exit 1
}
grep -qF -- '-f state=success' "$GH_STUB_LOG" || {
    printf 'live: r23 case 3 — the union (this LGTM alone) must succeed on the retry\n' >&2; exit 1
}
[ -z "$(pending_get)" ] || {
    printf 'live: r23 case 3 — the round should settle once the status lands, got:\n%s\n' \
        "$(pending_get)" >&2; exit 1
}
unset GH_HEAD GH_COMMENTS_FILE

# case 4 — a non-open PR: no launch, no ack, no status
for r23_state in MERGED CLOSED; do
    reset_stubs
    printf '42\t%s\t\t2026-09-02T00:00:00Z\n' "$sha" >"$tmp/pr-list-r23"
    GH_PR_LIST_FILE="$tmp/pr-list-r23" GH_STATE="$r23_state" \
        run_watch_once >/dev/null 2>&1 || {
        printf 'live: watcher cycle failed (r23 case 4: %s)\n' "$r23_state" >&2; exit 1
    }
    [ -s "$REVIEW_STUB_LOG" ] && {
        printf 'live: r23 case 4 — a %s PR must not be launched, got:\n%s\n' \
            "$r23_state" "$(cat "$REVIEW_STUB_LOG")" >&2; exit 1
    }
    grep -qF 'review: started on' "$GH_STUB_LOG" && {
        printf 'live: r23 case 4 — a %s PR must not be acked\n' "$r23_state" >&2; exit 1
    }
    grep -qF "pr42 skipped: state is '$r23_state', not OPEN" "$PR_REVIEW_WATCH_LOG" || {
        printf 'live: r23 case 4 — the skip must be logged with the state, got:\n%s\n' \
            "$(tail -3 "$PR_REVIEW_WATCH_LOG")" >&2; exit 1
    }
done
reset_stubs
# an unreadable state counts as not open — no launch either
printf '42\t%s\t\t2026-09-02T00:00:00Z\n' "$sha" >"$tmp/pr-list-r23"
GH_PR_LIST_FILE="$tmp/pr-list-r23" GH_FAIL_STATE=1 \
    run_watch_once >/dev/null 2>&1 || { printf 'live: watcher cycle failed (r23 case 4: unreadable state)\n' >&2; exit 1; }
[ -s "$REVIEW_STUB_LOG" ] && {
    printf 'live: r23 case 4 — an unreadable state must not launch, got:\n%s\n' \
        "$(cat "$REVIEW_STUB_LOG")" >&2; exit 1
}
grep -qF "pr42 skipped: state is 'unreadable', not OPEN" "$PR_REVIEW_WATCH_LOG" || {
    printf 'live: r23 case 4 — an unreadable state must be logged as a skip, got:\n%s\n' \
        "$(tail -3 "$PR_REVIEW_WATCH_LOG")" >&2; exit 1
}
# the ack retry path: a non-open PR is never acked and its entry is dropped
for r23_state in MERGED CLOSED; do
    reset_stubs
    printf '42\t%s\t0\t\n' "$sha" >"$EDDA_FLEET_SCRATCH/review-acks.tsv"
    GH_STATE="$r23_state" expect_ack "ack skipped on a $r23_state PR" 0 42 "$sha" 0
    [ -z "$(ack_state)" ] || {
        printf 'live: r23 case 4 — the ack entry must be dropped for a %s PR, got:\n%s\n' \
            "$r23_state" "$(ack_state)" >&2; exit 1
    }
    grep -qF 'review: started on' "$GH_STUB_LOG" && {
        printf 'live: r23 case 4 — a %s PR must not be acked\n' "$r23_state" >&2; exit 1
    }
done

# --- offline guarantee: the real watcher log was never touched -----------------
size_after=0
[ -f "$REAL_WATCHLOG" ] && size_after=$(stat -c %s "$REAL_WATCHLOG")
if [ "$size_before" != "$size_after" ]; then
    printf 'offline guarantee violated: %s grew from %s to %s bytes\n' \
        "$REAL_WATCHLOG" "$size_before" "$size_after" >&2
    exit 1
fi

printf 'pr-review-watch fixtures passed\n'
