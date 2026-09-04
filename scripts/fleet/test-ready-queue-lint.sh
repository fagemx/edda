#!/bin/sh
# Self-test for scripts/fleet/ready-queue-lint.sh (GH-665). Drives the script
# through a stub `gh` on PATH: fixture JSON stands in for `gh issue list` and
# `gh pr list`, so the tests prove which fleet:ready issues a picker sees when
# some already have a merged PR.
set -eu

cd "$(git rev-parse --show-toplevel)"
script=scripts/fleet/ready-queue-lint.sh

sh -n "$script" || {
    echo "FAIL: sh -n $script" >&2
    exit 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/bin"
cat > "$work/bin/gh" <<'STUB'
#!/bin/sh
mode=other
while [ $# -gt 0 ]; do
    case "$1" in
        issue) mode=issues ;;
        pr) mode=prs ;;
    esac
    shift
done
case "$mode" in
    issues) cat "$GH_ISSUES" ;;
    prs) cat "$GH_PRS" ;;
esac
STUB
chmod +x "$work/bin/gh"

# Issue 12 delivered by PR 100 ("Closes #12"). Issue 34 is only mentioned
# ("tracked in #34") — a non-closing reference must NOT deliver it. Issue 123
# must be picked even though PR bodies mention "#123" as part of "#1234"
# (word-boundary proof), and issue 20 is clean.
MIXED_ISSUES='[
  {"number":20,"title":"clean ready","createdAt":"2026-01-02T00:00:00Z"},
  {"number":12,"title":"delivered still ready","createdAt":"2026-01-01T00:00:00Z"},
  {"number":34,"title":"mention only stays ready","createdAt":"2026-01-03T00:00:00Z"},
  {"number":123,"title":"boundary ready","createdAt":"2026-01-04T00:00:00Z"}
]'
MIXED_PRS='[
  {"number":100,"body":"Closes #12\n\nTest evidence."},
  {"number":101,"body":"Follow-up tracked in #34.\nalso mentions #1234 in prose"},
  {"number":102,"body":null}
]'
CLEAN_ISSUES='[
  {"number":20,"title":"clean ready","createdAt":"2026-01-02T00:00:00Z"}
]'
CLEAN_PRS='[
  {"number":100,"body":"Closes #999\n\nUnrelated delivery."}
]'

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

run_case() {
    issues_fixture=$1
    prs_fixture=$2
    shift 2
    printf '%s' "$issues_fixture" > "$work/issues.json"
    printf '%s' "$prs_fixture" > "$work/prs.json"
    GH_ISSUES="$work/issues.json" GH_PRS="$work/prs.json" \
        PATH="$work/bin:$PATH" sh "$script" "$@"
}

out="$work/out.txt"
err="$work/err.txt"

# ── 1. default listing: delivered issues excluded, oldest-first, no false hit ──
rc=0
run_case "$MIXED_ISSUES" "$MIXED_PRS" > "$out" 2> "$err" || rc=$?
[ "$rc" -eq 0 ] || fail "default listing should exit 0, got $rc"
grep -q '#20 clean ready' "$out" || fail "clean ready issue must be listed: $(cat "$out")"
grep -q 'boundary ready' "$out" \
    || fail "#123 must survive a '#1234' mention (word boundary): $(cat "$out")"
grep -q '#12 delivered' "$out" && fail "delivered issue #12 must be excluded: $(cat "$out")"
grep -q '#34 mention' "$out" \
    || fail "a mention-only body ('tracked in #34') must not deliver #34: $(cat "$out")"
[ "$(head -n 1 "$out")" = "#20 clean ready" ] \
    || fail "listing must be oldest first: $(cat "$out")"
grep -q '#12' "$err" || fail "excluded issue must be reported on stderr: $(cat "$err")"
echo "ok 1 delivered issues excluded, oldest first, word-boundary holds"

# ── 2. --oldest: exactly one line, the oldest pickable issue ──
rc=0
run_case "$MIXED_ISSUES" "$MIXED_PRS" --oldest > "$out" 2> "$err" || rc=$?
[ "$rc" -eq 0 ] || fail "--oldest should exit 0, got $rc"
[ "$(wc -l < "$out")" -eq 1 ] || fail "--oldest must print one line: $(cat "$out")"
[ "$(cat "$out")" = "#20 clean ready" ] \
    || fail "--oldest picked the wrong issue: $(cat "$out")"
echo "ok 2 --oldest returns exactly the oldest pickable issue"

# ── 3. --check: stale ready issues fail the check and are named ──
rc=0
run_case "$MIXED_ISSUES" "$MIXED_PRS" --check > "$out" 2> "$err" || rc=$?
[ "$rc" -eq 1 ] || fail "--check with stale issues should exit 1, got $rc"
grep -q '#12' "$err" \
    || fail "--check must name the stale issues: $(cat "$err")"
grep -q '#34' "$err" && fail "mention-only #34 must not be flagged stale: $(cat "$err")"
echo "ok 3 --check exits 1 and names the stale issues"

# ── 4. --check on a clean queue: exit 0 ──
rc=0
run_case "$CLEAN_ISSUES" "$CLEAN_PRS" --check > "$out" 2> "$err" || rc=$?
[ "$rc" -eq 0 ] || fail "--check on a clean queue should exit 0, got $rc: $(cat "$err")"
grep -q '#20 clean ready' "$out" || fail "clean queue must still list: $(cat "$out")"
echo "ok 4 --check on a clean queue exits 0"

# ── 5. word boundary: a body mention '#1234' does not deliver #123, but '#123' does ──
ISSUE_123='[{"number":123,"title":"boundary exact","createdAt":"2026-01-01T00:00:00Z"}]'
NO_HIT='[{"number":7,"body":"refs #1234 and #1235"}]'
HIT='[{"number":7,"body":"Fixes #123"}]'
rc=0
run_case "$ISSUE_123" "$NO_HIT" --check > "$out" 2> "$err" || rc=$?
[ "$rc" -eq 0 ] || fail "'#1234' must not deliver #123, got exit $rc: $(cat "$err")"
rc=0
run_case "$ISSUE_123" "$HIT" --check > "$out" 2> "$err" || rc=$?
[ "$rc" -eq 1 ] || fail "'Fixes #123' must deliver #123, got exit $rc"
echo "ok 5 boundary: '#1234' is not a delivery, 'Fixes #123' is"

# ── 6. closing keyword only: mentions never deliver, any keyword inflection does ──
ISSUE_20='[{"number":20,"title":"keyword semantics","createdAt":"2026-01-01T00:00:00Z"}]'
for body in 'Follow-up tracked in #20.' 'Issue: #20' 'see #20 for context' 'Closing: #20'; do
    rc=0
    run_case "$ISSUE_20" "[{\"number\":7,\"body\":\"$body\"}]" --check \
        > "$out" 2> "$err" || rc=$?
    [ "$rc" -eq 0 ] || fail "non-closing body '$body' must not deliver #20, got exit $rc: $(cat "$err")"
    grep -q '#20 keyword semantics' "$out" \
        || fail "non-closing body '$body' must leave #20 pickable: $(cat "$out")"
done
for body in 'Closes #20' 'closes #20' 'Fixed #20' 'Resolves #20' 'PR resolves #20 tonight'; do
    rc=0
    run_case "$ISSUE_20" "[{\"number\":7,\"body\":\"$body\"}]" --check \
        > "$out" 2> "$err" || rc=$?
    [ "$rc" -eq 1 ] || fail "closing body '$body' must deliver #20, got exit $rc"
done
echo "ok 6 closing keywords deliver; 'tracked in', 'Issue:', 'see' do not"

# ── 7. usage errors exit 2 ──
rc=0
GH_ISSUES="$work/issues.json" GH_PRS="$work/prs.json" \
    PATH="$work/bin:$PATH" sh "$script" --bogus > "$out" 2>&1 || rc=$?
[ "$rc" -eq 2 ] || fail "unknown flag should exit 2, got $rc"
echo "ok 7 usage errors -> exit 2"

# ── 8. a broken gh fails closed, never looks like "queue empty" ──
rc=0
GH_ISSUES="$work/nonexistent.json" GH_PRS="$work/prs.json" \
    PATH="$work/bin:$PATH" sh "$script" > "$out" 2>&1 || rc=$?
[ "$rc" -ne 0 ] || fail "a broken gh must not exit 0 (fail closed)"
echo "ok 8 broken gh -> fail closed"

echo "all ready-queue-lint.sh self-tests passed"
