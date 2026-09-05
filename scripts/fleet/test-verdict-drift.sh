#!/bin/sh
# Offline fixtures for scripts/fleet/verdict-drift.sh (GH-914).
#
# POSIX sh, `set -eu`, one mktemp -d sandbox with trap cleanup, a stub `gh`
# earlier on PATH. The stub routes `gh pr list` / `gh pr view <n>` to raw
# JSON fixture files and applies the caller's `--jq` filter with the real
# jq, so the R23 heading regex in the script under test is exercised, not
# assumed. Nothing is written outside the sandbox.
#
# Case 2 carries the #899 regression (a verdict pinned to an older SHA than
# headRefOid was reported complete) as a fixture per controller ruling d-003:
# PR #899 merged, so the live regression target no longer exists.
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
script="$root/scripts/fleet/verdict-drift.sh"

sh -n "$script" || {
    echo "FAIL: sh -n $script" >&2
    exit 1
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' 0 HUP INT TERM

mkdir -p "$tmp/bin"
cat >"$tmp/bin/gh" <<'EOF'
#!/bin/sh
if [ -n "${GH_FAIL:-}" ]; then
    echo "gh: stub failure" >&2
    exit 1
fi
filter=
prev=
for a in "$@"; do
    [ "$prev" = "--jq" ] && filter=$a
    prev=$a
done
out=
case "$1 $2" in
    "pr list")
        [ -f "$GH_DRIFT_DIR/prs.json" ] && out=$(cat "$GH_DRIFT_DIR/prs.json")
        ;;
    "pr view")
        f="$GH_DRIFT_DIR/comments-$3.json"
        [ -f "$f" ] && out=$(cat "$f")
        ;;
esac
if [ -n "$filter" ] && [ -n "$out" ]; then
    printf '%s' "$out" | jq -r "$filter"
else
    printf '%s' "$out"
fi
exit 0
EOF
chmod +x "$tmp/bin/gh"
PATH="$tmp/bin:$PATH"
export PATH
GH_DRIFT_DIR="$tmp"
export GH_DRIFT_DIR

fail() {
    echo "FAIL $1: $2" >&2
    exit 1
}

run_drift() {
    rc=0
    sh "$script" >"$tmp/out" 2>"$tmp/err" || rc=$?
}

expect() { # case expected-rc expected-line
    case_no=$1
    want_rc=$2
    want_line=$3
    [ "$rc" = "$want_rc" ] ||
        fail "$case_no" "exit $rc, expected $want_rc (stderr: $(cat "$tmp/err"))"
    actual=$(cat "$tmp/out")
    [ "$actual" = "$want_line" ] ||
        fail "$case_no" "line was '$actual', expected '$want_line'"
    echo "PASS $case_no"
}

SHA1=1111111111111111111111111111111111111111
SHA2=2222222222222222222222222222222222222222
SHA3=3333333333333333333333333333333333333333
SHA4=4444444444444444444444444444444444444444
SHA5=5555555555555555555555555555555555555555
SHA6=6666666666666666666666666666666666666666
SHAOLD=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa

# --- case 1: verdict pinned to head, LGTM → exit 0 -----------------------------
printf '[{"number":1,"headRefOid":"%s","baseRefName":"main"}]\n' "$SHA1" >"$tmp/prs.json"
printf '{"comments":[{"body":"## Code Review: Round 1 — PR #1 @ %s\\n\\n### Verdict\\nLGTM (P0=0, P1=0)"}]}\n' "$SHA1" >"$tmp/comments-1.json"
run_drift
expect 1 0 "#1 111111111111 main LGTM"

# --- case 2: verdict pinned to an older SHA → stale, exit 1 (#899 regression) --
printf '[{"number":2,"headRefOid":"%s","baseRefName":"main"}]\n' "$SHA2" >"$tmp/prs.json"
printf '{"comments":[{"body":"## Code Review: Round 1 — PR #2 @ %s\\n\\n### Verdict\\nLGTM (P0=0, P1=0)"}]}\n' "$SHAOLD" >"$tmp/comments-2.json"
run_drift
expect 2 1 "#2 222222222222 main stale from aaaaaaaaaaaa"

# --- case 3: no verdict comment → exit 1 ----------------------------------------
printf '[{"number":3,"headRefOid":"%s","baseRefName":"main"}]\n' "$SHA3" >"$tmp/prs.json"
printf '{"comments":[]}\n' >"$tmp/comments-3.json"
run_drift
expect 3 1 "#3 333333333333 main no verdict on head"

# --- case 4: only a (SHADOW) verdict on head → SHADOW only, exit 0 --------------
printf '[{"number":4,"headRefOid":"%s","baseRefName":"main"}]\n' "$SHA4" >"$tmp/prs.json"
printf '{"comments":[{"body":"## Code Review: Round 2 (SHADOW) — PR #4 @ %s\\n- shadow: true\\n\\n### Verdict\\nLGTM (P0=0, P1=0)"}]}\n' "$SHA4" >"$tmp/comments-4.json"
run_drift
expect 4 0 "#4 444444444444 main SHADOW only"

# --- case 5: base is not main, no verdict → annotated line, exit 1 --------------
printf '[{"number":5,"headRefOid":"%s","baseRefName":"feat/x"}]\n' "$SHA5" >"$tmp/prs.json"
printf '{"comments":[]}\n' >"$tmp/comments-5.json"
run_drift
expect 5 1 \
    "#5 555555555555 feat/x no verdict on head base=feat/x (status contexts not enforced)"

# --- case 6: R23 heading mid-body is not a verdict (the #867 shape) -------------
printf '[{"number":6,"headRefOid":"%s","baseRefName":"main"}]\n' "$SHA6" >"$tmp/prs.json"
printf '{"comments":[{"body":"narration first\\n## Code Review: Round 3 — PR #867 @ %s\\nLGTM (P0=0, P1=0)"}]}\n' "$SHA6" >"$tmp/comments-6.json"
run_drift
expect 6 1 "#6 666666666666 main no verdict on head"

# --- case 7: gh fails → exit 2, no PR line printed ------------------------------
printf '[{"number":7,"headRefOid":"%s","baseRefName":"main"}]\n' "$SHA1" >"$tmp/prs.json"
GH_FAIL=1
export GH_FAIL
run_drift
unset GH_FAIL
[ "$rc" = 2 ] || fail 7 "exit $rc, expected 2"
[ ! -s "$tmp/out" ] || fail 7 "stdout must be empty on a failed read, got: $(cat "$tmp/out")"
grep -q 'could not read' "$tmp/err" ||
    fail 7 "stderr must say the read failed, got: $(cat "$tmp/err")"
echo "PASS 7"

echo "verdict-drift fixtures passed"
