#!/bin/sh
# Self-test for scripts/fleet-claim-issue.sh (GH-656). Drives the script
# through a stub `gh` on PATH: the fixture JSON stands in for
# `gh issue view`, and comment/label calls are recorded to a log so the
# tests can assert exactly what was (or was not) written.
set -eu

cd "$(git rev-parse --show-toplevel)"
script=scripts/fleet-claim-issue.sh

sh -n "$script" || {
    echo "FAIL: sh -n $script" >&2
    exit 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/bin"
cat > "$work/bin/gh" <<'STUB'
#!/bin/sh
orig_args="$*"
mode=other
jq_expr=""
while [ $# -gt 0 ]; do
    case "$1" in
        view) mode=view ;;
        comment) mode=comment ;;
        edit) mode=edit ;;
        --jq) shift; jq_expr="$1" ;;
    esac
    shift
done
case "$mode" in
    view)
        if [ -n "$jq_expr" ]; then
            jq -r "$jq_expr" "$GH_FIXTURE"
        else
            cat "$GH_FIXTURE"
        fi
        ;;
    comment) echo "comment $orig_args" >> "$GH_CALLS" ;;
    edit) echo "edit $orig_args" >> "$GH_CALLS" ;;
esac
STUB
chmod +x "$work/bin/gh"

UNCLAIMED='{"labels":[],"comments":[]}'
OTHER='{"labels":[{"name":"fleet:ready"},{"name":"lane:4090"}],"comments":[{"author":{"login":"controller"},"body":"taking: 4090 at 2026-09-02T06:30:00Z","createdAt":"2026-09-02T06:30:00Z"}]}'
SELF='{"labels":[{"name":"lane:docs"}],"comments":[{"author":{"login":"controller"},"body":"taking: docs at 2026-09-02T07:06:00Z","createdAt":"2026-09-02T07:06:00Z"}]}'

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

# run_case <fixture> <calls-log> <args...> — runs the script with the stub
# gh; the caller inspects $? and the log afterwards.
run_case() {
    fixture=$1
    calls=$2
    shift 2
    printf '%s' "$fixture" > "$work/fixture.json"
    : > "$calls"
    GH_FIXTURE="$work/fixture.json" GH_CALLS="$calls" \
        PATH="$work/bin:$PATH" sh "$script" "$@"
}

calls="$work/calls.log"
out="$work/out.txt"

# ── 1. unclaimed → claims (comment + label), exit 0 ──
rc=0
run_case "$UNCLAIMED" "$calls" 656 docs > "$out" 2>&1 || rc=$?
[ "$rc" -eq 0 ] || fail "unclaimed write should exit 0, got $rc: $(cat "$out")"
grep -q 'comment .* --body taking: docs at ' "$calls" \
    || fail "unclaimed write should leave a taking: docs comment, calls: $(cat "$calls")"
grep -q 'edit .* --add-label lane:docs' "$calls" \
    || fail "unclaimed write should add the lane:docs label, calls: $(cat "$calls")"
echo "ok 1 unclaimed -> claim (exit 0, comment + label)"

# ── 2. claimed by another machine → exit 1, names the marker, no write ──
rc=0
run_case "$OTHER" "$calls" 656 docs > "$out" 2>&1 || rc=$?
[ "$rc" -eq 1 ] || fail "other-machine claim should exit 1, got $rc"
grep -q '4090' "$out" || fail "refusal must name the other machine: $(cat "$out")"
grep -q '2026-09-02T06:30:00Z' "$out" \
    || fail "refusal must print the claim timestamp: $(cat "$out")"
[ -s "$calls" ] && fail "a refusal must not write anything: $(cat "$calls")"
echo "ok 2 other-machine claim -> exit 1, no write"

# ── 3. claimed by this machine → idempotent, exit 0, no duplicate ──
rc=0
run_case "$SELF" "$calls" 656 docs > "$out" 2>&1 || rc=$?
[ "$rc" -eq 0 ] || fail "self-claimed should exit 0, got $rc: $(cat "$out")"
[ -s "$calls" ] && fail "self-claimed must not re-comment: $(cat "$calls")"
echo "ok 3 self claim -> exit 0, no duplicate comment"

# ── 4/5/6. --check reads, never writes ──
rc=0
run_case "$UNCLAIMED" "$calls" --check 656 docs > "$out" 2>&1 || rc=$?
[ "$rc" -eq 0 ] || fail "--check unclaimed should exit 0, got $rc"
[ -s "$calls" ] && fail "--check unclaimed must not write: $(cat "$calls")"

rc=0
run_case "$OTHER" "$calls" --check 656 docs > "$out" 2>&1 || rc=$?
[ "$rc" -eq 1 ] || fail "--check other-machine should exit 1, got $rc"
grep -q '4090' "$out" || fail "--check refusal must name the machine: $(cat "$out")"
[ -s "$calls" ] && fail "--check other-machine must not write: $(cat "$calls")"

rc=0
run_case "$SELF" "$calls" --check 656 docs > "$out" 2>&1 || rc=$?
[ "$rc" -eq 0 ] || fail "--check self-claimed should exit 0, got $rc"
[ -s "$calls" ] && fail "--check self-claimed must not write: $(cat "$calls")"
# Trailing --check accepted identically:
rc=0
run_case "$UNCLAIMED" "$calls" 656 docs --check > "$out" 2>&1 || rc=$?
[ "$rc" -eq 0 ] || fail "trailing --check unclaimed should exit 0, got $rc"
[ -s "$calls" ] && fail "trailing --check unclaimed must not write: $(cat "$calls")"
echo "ok 4-6 --check: three states and trailing flag, nothing written"

# ── 7. prose mention of taking: mid-sentence is not a claim (even when indented) ──
PROSE='{"labels":[],"comments":[{"author":{"login":"someone"},"body":"remember: taking: 4090 first is bad\n  remember: taking: 4090 indented is also bad","createdAt":"2026-09-02T05:00:00Z"}]}'
rc=0
run_case "$PROSE" "$calls" 656 docs --check > "$out" 2>&1 || rc=$?
[ "$rc" -eq 0 ] || fail "prose mid-sentence taking: should not be treated as a claim, got $rc"
echo "ok 7 prose mid-sentence taking: is not a claim"

# ── 8. usage errors exit 2 ──
rc=0
PATH="$work/bin:$PATH" sh "$script" > "$out" 2>&1 || rc=$?
[ "$rc" -eq 2 ] || fail "missing args should exit 2, got $rc"
rc=0
PATH="$work/bin:$PATH" sh "$script" 656 "docs lane" > "$out" 2>&1 || rc=$?
[ "$rc" -eq 2 ] || fail "whitespace machine label should exit 2, got $rc"
echo "ok 8 usage errors -> exit 2"

# ── 9. a broken gh fails closed, never looks like "unclaimed" ──
rc=0
GH_FIXTURE="$work/nonexistent.json" GH_CALLS="$calls" \
    PATH="$work/bin:$PATH" sh "$script" 656 docs > "$out" 2>&1 || rc=$?
[ "$rc" -ne 0 ] || fail "a broken gh must not exit 0 (fail closed)"
[ -s "$calls" ] && fail "a broken gh must not write anything: $(cat "$calls")"
echo "ok 9 broken gh -> fail closed"

echo "all fleet-claim-issue.sh self-tests passed"
