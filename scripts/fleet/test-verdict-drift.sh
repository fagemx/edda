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
[ -n "${GH_ARGV_LOG:-}" ] && echo "gh $*" >>"$GH_ARGV_LOG"
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
printf '{"comments":[{"body":"## Code Review: Round 1 — PR #1 @ %s\\n\\n### Verdict\\nLGTM (P0=0, P1=0)","authorAssociation":"OWNER"}]}\n' "$SHA1" >"$tmp/comments-1.json"
run_drift
expect 1 0 "#1 111111111111 main LGTM"

# --- case 2: verdict pinned to an older SHA → stale, exit 1 (#899 regression) --
printf '[{"number":2,"headRefOid":"%s","baseRefName":"main"}]\n' "$SHA2" >"$tmp/prs.json"
printf '{"comments":[{"body":"## Code Review: Round 1 — PR #2 @ %s\\n\\n### Verdict\\nLGTM (P0=0, P1=0)","authorAssociation":"OWNER"}]}\n' "$SHAOLD" >"$tmp/comments-2.json"
run_drift
expect 2 1 "#2 222222222222 main stale from aaaaaaaaaaaa"

# --- case 3: no verdict comment → exit 1 ----------------------------------------
printf '[{"number":3,"headRefOid":"%s","baseRefName":"main"}]\n' "$SHA3" >"$tmp/prs.json"
printf '{"comments":[]}\n' >"$tmp/comments-3.json"
run_drift
expect 3 1 "#3 333333333333 main no verdict on head"

# --- case 4: only a (SHADOW) verdict on head → SHADOW only, exit 0 --------------
printf '[{"number":4,"headRefOid":"%s","baseRefName":"main"}]\n' "$SHA4" >"$tmp/prs.json"
printf '{"comments":[{"body":"## Code Review: Round 2 (SHADOW) — PR #4 @ %s\\n- shadow: true\\n\\n### Verdict\\nLGTM (P0=0, P1=0)","authorAssociation":"OWNER"}]}\n' "$SHA4" >"$tmp/comments-4.json"
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
printf '{"comments":[{"body":"narration first\\n## Code Review: Round 3 — PR #867 @ %s\\nLGTM (P0=0, P1=0)","authorAssociation":"OWNER"}]}\n' "$SHA6" >"$tmp/comments-6.json"
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

# --- case 8: mergeable CONFLICTING holds the PR (R24 field 3, GH-958) ----------
# The head carries an authoritative LGTM, so every field the check covered
# before says ready. R24's third field does not, and used to be left to the
# digest's DIRTY row — invisible whenever this check ran standalone.
printf '[{"number":8,"headRefOid":"%s","baseRefName":"main","mergeable":"CONFLICTING"}]\n' "$SHA1" >"$tmp/prs.json"
printf '{"comments":[{"body":"## Code Review: Round 1 — PR #8 @ %s\\n\\n### Verdict\\nLGTM (P0=0, P1=0)","authorAssociation":"OWNER"}]}\n' "$SHA1" >"$tmp/comments-8.json"
run_drift
expect 8 1 "#8 111111111111 main LGTM mergeable=CONFLICTING"

# --- case 9: mergeable UNKNOWN is surfaced but does not hold the PR ------------
# GitHub has not computed the merge yet; that is a transient answer, not a
# verdict, so it is annotated and the exit code stays 0.
printf '[{"number":9,"headRefOid":"%s","baseRefName":"main","mergeable":"UNKNOWN"}]\n' "$SHA2" >"$tmp/prs.json"
printf '{"comments":[{"body":"## Code Review: Round 1 — PR #9 @ %s\\n\\n### Verdict\\nLGTM (P0=0, P1=0)","authorAssociation":"OWNER"}]}\n' "$SHA2" >"$tmp/comments-9.json"
run_drift
expect 9 0 "#9 222222222222 main LGTM mergeable=UNKNOWN"

# --- case 10: the enumeration limit is EDDA_OPEN_PR_LIMIT, shared with the -----
# digest (GH-958). The two scripts hardcoded 200 and 100, so a PR past the
# digest's 100 got a line here that could never reach a digest row.
printf '[{"number":10,"headRefOid":"%s","baseRefName":"main","mergeable":"MERGEABLE"}]\n' "$SHA3" >"$tmp/prs.json"
printf '{"comments":[{"body":"## Code Review: Round 1 — PR #10 @ %s\\n\\n### Verdict\\nLGTM (P0=0, P1=0)","authorAssociation":"OWNER"}]}\n' "$SHA3" >"$tmp/comments-10.json"
: >"$tmp/gh-argv.log"
GH_ARGV_LOG="$tmp/gh-argv.log"
export GH_ARGV_LOG
EDDA_OPEN_PR_LIMIT=7
export EDDA_OPEN_PR_LIMIT
run_drift
unset EDDA_OPEN_PR_LIMIT
grep -q -- '--limit 7' "$tmp/gh-argv.log" ||
    fail 10 "the pr list call ignored EDDA_OPEN_PR_LIMIT: $(cat "$tmp/gh-argv.log")"
[ "$rc" = 0 ] || fail 10 "exit $rc, expected 0 (stderr: $(cat "$tmp/err"))"
echo "PASS 10"

# --- case 11: a saturated enumeration says so on stderr instead of dropping ----
# the tail in silence.
EDDA_OPEN_PR_LIMIT=1
export EDDA_OPEN_PR_LIMIT
run_drift
unset EDDA_OPEN_PR_LIMIT
grep -q 'hit its limit' "$tmp/err" ||
    fail 11 "a saturated enumeration printed no warning: $(cat "$tmp/err")"
echo "PASS 11"
unset GH_ARGV_LOG

# --- case 12: an orphan Review Response holds the PR (GH-993) ------------------
# The observed defect, as a fixture: PRs #974/#976/#980/#981 each carried a
# `## Review Response: Round 1` and no `## Code Review: Round 1` at all —
# real body captured 2026-09-06T06:19:40Z via `gh api .../issues/974/comments`.
SHA7=7777777777777777777777777777777777777777
printf '[{"number":12,"headRefOid":"%s","baseRefName":"main"}]\n' "$SHA7" >"$tmp/prs.json"
printf '{"comments":[{"body":"## Review Response: Round 1\\n\\nNew head: `%s`\\n\\n### f1 — fixed","authorAssociation":"OWNER"}]}\n' "$SHA7" >"$tmp/comments-12.json"
run_drift
expect 12 1 "#12 777777777777 main no verdict on head orphan-response=Round-1"

# --- case 13: a response answering a round that was really posted is clean ----
# Ordinary Changes-Requested flow: Round 1 posted, pinned to head, the
# implementer answers it. No orphan annotation. (The base state itself does
# not hold the PR here — see the existing "else" branch above: a head
# verdict that resolves Changes Requested is a visible signal, not drift.)
SHA8=8888888888888888888888888888888888888888
printf '[{"number":13,"headRefOid":"%s","baseRefName":"main"}]\n' "$SHA8" >"$tmp/prs.json"
printf '{"comments":[{"body":"## Code Review: Round 1 — PR #13 @ %s\\n\\n### Verdict\\nChanges Requested, P0=0, P1=1","authorAssociation":"OWNER"},{"body":"## Review Response: Round 1\\n\\nNew head: unchanged","authorAssociation":"OWNER"}]}\n' "$SHA8" >"$tmp/comments-13.json"
run_drift
expect 13 0 "#13 888888888888 main Changes Requested"

# --- case 14: the #974 repair shape — an old orphan does not re-trigger -------
# Round 1's response is answered by nothing, permanently (the repair chosen
# for #974/#976/#980/#981 never backfills a Round 1 Code Review — it jumps
# straight to Round 2, each recording that Round 1's report is absent). Only
# the NEWEST response is judged, and it is Round 2's, which IS paired — so a
# PR that recovered this way must read clean, or this check would have
# permanently blocked the very PRs GH-993's own repair produced. Sequence
# mirrors the real PR #974 comment order exactly: Response 1, Review 2,
# Response 2, Review 3 (LGTM, pinned to head).
SHA9=9999999999999999999999999999999999999999
SHAMID=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
printf '[{"number":14,"headRefOid":"%s","baseRefName":"main"}]\n' "$SHA9" >"$tmp/prs.json"
{
    printf '{"comments":['
    printf '{"body":"## Review Response: Round 1\\n\\nNew head: %s","authorAssociation":"OWNER"},' "$SHAMID"
    printf '{"body":"## Code Review: Round 2 — PR #14 @ %s\\n\\n### Verdict\\nChanges Requested, P0=0, P1=1","authorAssociation":"OWNER"},' "$SHAMID"
    printf '{"body":"## Review Response: Round 2\\n\\nNew head: %s","authorAssociation":"OWNER"},' "$SHA9"
    printf '{"body":"## Code Review: Round 3 — PR #14 @ %s\\n\\n### Verdict\\nLGTM (P0=0, P1=0)","authorAssociation":"OWNER"}' "$SHA9"
    printf ']}\n'
} >"$tmp/comments-14.json"
run_drift
expect 14 0 "#14 999999999999 main LGTM"

# --- case 15: a Review Response heading not on the first line is not a --------
# trigger, symmetric with case 6's identical rule for Code Review headings.
SHA10=cccccccccccccccccccccccccccccccccccccccc
printf '[{"number":15,"headRefOid":"%s","baseRefName":"main"}]\n' "$SHA10" >"$tmp/prs.json"
printf '{"comments":[{"body":"note first\\n## Review Response: Round 1\\n\\nignored","authorAssociation":"OWNER"}]}\n' >"$tmp/comments-15.json"
run_drift
expect 15 1 "#15 cccccccccccc main no verdict on head"

# --- case 16: an untrusted-author verdict does not silence the check -----------
# (Round 1 review, P1). This repo is PUBLIC: without an author filter, a §7-
# shaped comment from ANY commenter would read as a real LGTM and clear the
# PR. Byte-identical to case 1's LGTM-on-head shape — only the author trust
# differs — so this isolates the filter rather than testing a new rule.
# "NONE" is GitHub's real authorAssociation value for a user with no
# relationship to the repo.
SHA11=dddddddddddddddddddddddddddddddddddddddd
printf '[{"number":16,"headRefOid":"%s","baseRefName":"main"}]\n' "$SHA11" >"$tmp/prs.json"
printf '{"comments":[{"body":"## Code Review: Round 1 — PR #16 @ %s\\n\\n### Verdict\\nLGTM (P0=0, P1=0)","authorAssociation":"NONE"}]}\n' "$SHA11" >"$tmp/comments-16.json"
run_drift
expect 16 1 "#16 dddddddddddd main no verdict on head"

# --- case 17: an untrusted-author Review Response does not hold the PR ---------
# (Round 1 review, P1) — the other direction of the same finding: on a
# public repo, anyone could otherwise post any `## Review Response: Round
# N` (no SHA, no matching PR number required) and refuse every
# --check/--merge in the fleet. Byte-identical to case 12's shape, only the
# author trust differs: no orphan-response annotation, because the comment
# is invisible to the check — not because it was judged and cleared.
SHA12=eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee
printf '[{"number":17,"headRefOid":"%s","baseRefName":"main"}]\n' "$SHA12" >"$tmp/prs.json"
printf '{"comments":[{"body":"## Review Response: Round 9\\n\\nNew head: `%s`","authorAssociation":"NONE"}]}\n' "$SHA12" >"$tmp/comments-17.json"
run_drift
expect 17 1 "#17 eeeeeeeeeeee main no verdict on head"

# --- case 18: a missing authorAssociation fails closed, same as an untrusted --
# one (Round 1 review, P1) — "absent or unreadable" must not be treated as a
# verdict either. Distinct from case 16: there the field is present and
# untrusted; here the key is absent entirely, exercising jq's `null` path
# (reading `.authorAssociation` off an object that never had the key), which
# must compare unequal to every trusted value, not error and not default
# open.
SHA13=ffffffffffffffffffffffffffffffffffffffff
printf '[{"number":18,"headRefOid":"%s","baseRefName":"main"}]\n' "$SHA13" >"$tmp/prs.json"
printf '{"comments":[{"body":"## Code Review: Round 1 — PR #18 @ %s\\n\\n### Verdict\\nLGTM (P0=0, P1=0)"}]}\n' "$SHA13" >"$tmp/comments-18.json"
run_drift
expect 18 1 "#18 ffffffffffff main no verdict on head"

echo "verdict-drift fixtures passed"
