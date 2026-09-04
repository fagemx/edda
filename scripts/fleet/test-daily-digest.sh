#!/bin/sh
# Offline fixtures for scripts/fleet/daily-digest.sh (GH-765).
#
# Style follows scripts/test-pr-review-watch.sh — POSIX sh, `set -eu`, a
# mktemp dir with trap cleanup, executable stubs in $tmp/bin prepended to
# PATH that log their argv, assertions with grep -qF, `exit 1` with a
# message on failure, final line "daily-digest fixtures passed".
#
# Everything is offline: `gh` and `edda` are stubs fed from env
# (GH_MERGED_JSON, GH_OPEN_JSON, GH_STATUS_JSON, GH_CHECK_RUNS_JSON, GH_COMMENTS_FILE,
# GH_BOARD_FILE, GH_READY_JSON, EDDA_RECAP_FILE); nothing here touches the
# real repository, the real board issue, or any notification channel.
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' 0 HUP INT TERM

case_number=0
fail() {
    printf 'FAIL: %s\n' "$1" >&2
    exit 1
}

# --- stubs: gh and edda --------------------------------------------------------
# Both log their argv to $GH_STUB_LOG / $EDDA_STUB_LOG and to a shared order
# log so case 5 can assert call order. `gh` answers per subcommand from env;
# `edda` prints a canned recap digest for `recap --digest` and exits 0.

STUBBIN="$tmp/bin"
mkdir -p "$STUBBIN"
cat >"$STUBBIN/gh" <<'EOF'
#!/bin/sh
echo "gh $*" >>"$GH_STUB_LOG"
echo "gh $*" >>"$ORDER_LOG"
case "$1" in
  pr)
    case "$2" in
      list)
        case "$*" in
          *"--state merged"*) [ -n "${GH_MERGED_JSON:-}" ] && cat "$GH_MERGED_JSON" ; exit 0 ;;
          *"--state open"*)   [ -n "${GH_OPEN_JSON:-}" ] && cat "$GH_OPEN_JSON" ; exit 0 ;;
        esac
        exit 0
        ;;
      view)
        [ -n "${GH_COMMENTS_FILE:-}" ] && cat "$GH_COMMENTS_FILE"
        exit 0
        ;;
    esac
    exit 0
    ;;
  issue)
    case "$2" in
      comment) exit 0 ;;
      view)
        [ -n "${GH_BOARD_FILE:-}" ] && cat "$GH_BOARD_FILE"
        exit 0
        ;;
      list)
        [ -n "${GH_READY_JSON:-}" ] && cat "$GH_READY_JSON"
        exit 0
        ;;
    esac
    exit 0
    ;;
  api)
    case "$*" in
      *check-runs*) [ -n "${GH_CHECK_RUNS_JSON:-}" ] && cat "$GH_CHECK_RUNS_JSON" ;;
      *) [ -n "${GH_STATUS_JSON:-}" ] && cat "$GH_STATUS_JSON" ;;
    esac
    exit 0
    ;;
esac
exit 0
EOF
cat >"$STUBBIN/edda" <<'EOF'
#!/bin/sh
echo "edda $*" >>"$EDDA_STUB_LOG"
echo "edda $*" >>"$ORDER_LOG"
case "$1 $2" in
  "recap --digest")
    [ -n "${EDDA_RECAP_FILE:-}" ] && cat "$EDDA_RECAP_FILE"
    exit 0
    ;;
  "notify send")
    exit 0
    ;;
esac
exit 0
EOF
chmod +x "$STUBBIN/gh" "$STUBBIN/edda"

export GH_STUB_LOG="$tmp/gh-stub.log"
export EDDA_STUB_LOG="$tmp/edda-stub.log"
export ORDER_LOG="$tmp/order.log"
: >"$GH_STUB_LOG"; : >"$EDDA_STUB_LOG"; : >"$ORDER_LOG"
export PATH="$STUBBIN:$PATH"

reset_stubs() {
    : >"$GH_STUB_LOG"; : >"$EDDA_STUB_LOG"; : >"$ORDER_LOG"
    unset GH_MERGED_JSON GH_OPEN_JSON GH_STATUS_JSON GH_CHECK_RUNS_JSON GH_COMMENTS_FILE \
          GH_BOARD_FILE GH_READY_JSON EDDA_RECAP_FILE 2>/dev/null || true
}

# canned recap digest (the three ledger blocks, exactly as edda recap --digest prints)
RECAP_CANNED="$tmp/recap-canned.md"
cat >"$RECAP_CANNED" <<'EOF'
## 例外
- decision `db.engine` — unratified (agent), recorded 2026-09-03T01:00:00Z
- task #3 port ingest — failed
## 成本
- session_stats.estimated_cost_usd：$1.25（1 筆量測，1 筆未量測）
- execution_event.usage.cost_usd：n/a（0 筆量測）
- cycle_telemetry.cost.total_usd：n/a（0 筆量測）
## 明天會做的
- task #7 wire notify（unassigned）
EOF

# extract a `## <heading>` block body (everything after the heading line up to
# the next `## ` line) — the same way daily-digest.sh splits the recap output
extract_block() { # file heading-text
    awk -v h="$2" '
        $0 == "## " h { out=1; next }
        /^## /        { out=0 }
        out           { print }
    ' "$1"
}

# strip the lines daily-digest.sh appends after the verbatim ledger blocks,
# so the remainder can be diffed byte-for-byte against the recap bodies
strip_appended() {
    grep -v -e '^- 審查（gh verdict 留言 Cost:）' -e '^- fleet:ready #' -e '^- 看板：' || true
}

run_digest() { # extra args...
    sh "$root/scripts/fleet/daily-digest.sh" "$@"
}

# --- case 1: dry-run, one merged PR, verbatim ledger blocks --------------------
reset_stubs
printf '123\tFix the thing\t2026-09-03T08:00:00Z\n' >"$tmp/merged.tsv"
printf -- '## Code Review: Round 1\n\nP0=0\n\nCost: $1.25\n' >"$tmp/comments.md"
printf -- '## Code Review: Round 2\n\nP0=0, P1=0\n\nCost: $0.75\n' >>"$tmp/comments.md"
export GH_MERGED_JSON="$tmp/merged.tsv"
export GH_COMMENTS_FILE="$tmp/comments.md"
export EDDA_RECAP_FILE="$RECAP_CANNED"

out=$(run_digest --dry-run 2>"$tmp/case1.err") || fail "case 1: daily-digest.sh --dry-run exited non-zero: $(cat "$tmp/case1.err")"
printf '%s\n' "$out" >"$tmp/case1.out"

# all five headings, fixed order
expected_order='## 合併了什麼
## 擋住什麼
## 例外
## 成本
## 明天會做的'
actual_order=$(grep '^## ' "$tmp/case1.out") || fail 'case 1: no ## headings in output'
[ "$actual_order" = "$expected_order" ] || fail "case 1: heading order wrong, got:\n$actual_order"
printf '%s\n' "$out" | grep -q '^# Fleet digest ' || fail 'case 1: missing digest title line'

# merged line
printf '%s\n' "$out" | grep -qF -- '- #123 Fix the thing — merged 2026-09-03T08:00:00Z, verdict rounds 2, review cost $2.00' \
    || fail "case 1: merged line wrong, got: $(grep '#123' "$tmp/case1.out")"

# ledger blocks byte-identical to the recap bodies
for h in 例外 成本 明天會做的; do
    extract_block "$tmp/case1.out" "$h" | strip_appended >"$tmp/case1.$h.out"
    extract_block "$RECAP_CANNED" "$h" >"$tmp/case1.$h.recap"
    diff -u "$tmp/case1.$h.recap" "$tmp/case1.$h.out" >/dev/null \
        || fail "case 1: ledger block $h is not verbatim: $(diff "$tmp/case1.$h.recap" "$tmp/case1.$h.out" | head -5)"
done

# --- case 2: nothing merged, nothing blocked, review cost n/a ------------------
reset_stubs
export EDDA_RECAP_FILE="$RECAP_CANNED"
out=$(run_digest --dry-run 2>&1) || fail 'case 2: daily-digest.sh exited non-zero'
printf '%s\n' "$out" >"$tmp/case2.out"
# （無） under both 合併了什麼 and 擋住什麼
awk '/^## 合併了什麼/{f=1;next} /^## /{f=0} f' "$tmp/case2.out" | grep -qxF '（無）' \
    || fail 'case 2: 合併了什麼 should be （無）'
awk '/^## 擋住什麼/{f=1;next} /^## /{f=0} f' "$tmp/case2.out" | grep -qxF '（無）' \
    || fail 'case 2: 擋住什麼 should be （無）'
printf '%s\n' "$out" | grep -qF -- '- 審查（gh verdict 留言 Cost:）：n/a' \
    || fail 'case 2: review cost line should be n/a'

# --- case 3: one BLOCKED PR with a failing Independent Review ------------------
reset_stubs
printf '77\tBlocked thing\tBLOCKED\tabc123\n' >"$tmp/open.tsv"
printf 'CI Gate=success\nIndependent Review=failure\n' >"$tmp/status.txt"
export GH_OPEN_JSON="$tmp/open.tsv"
export GH_STATUS_JSON="$tmp/status.txt"
export EDDA_RECAP_FILE="$RECAP_CANNED"
out=$(run_digest --dry-run 2>&1) || fail 'case 3: daily-digest.sh exited non-zero'
printf '%s\n' "$out" | grep -qF -- '- #77 Blocked thing — Independent Review=failure' \
    || fail "case 3: blocked line wrong, got: $(printf '%s' "$out" | grep '#77' || true)"
if printf '%s\n' "$out" | grep -F '#77' | grep -qF 'CI Gate'; then
    fail 'case 3: successful CI Gate must not be reported'
fi

# --- case 3b: CI Gate is a CheckRun, not a legacy commit status --------------
reset_stubs
printf '78\tCheck run thing\tBLOCKED\tdef456\n' >"$tmp/open-check.tsv"
printf 'CI Gate=success\n' >"$tmp/check-runs.txt"
printf 'Independent Review=failure\n' >"$tmp/status-check.txt"
export GH_OPEN_JSON="$tmp/open-check.tsv"
export GH_CHECK_RUNS_JSON="$tmp/check-runs.txt"
export GH_STATUS_JSON="$tmp/status-check.txt"
export EDDA_RECAP_FILE="$RECAP_CANNED"
out=$(run_digest --dry-run 2>&1) || fail 'case 3b: daily-digest.sh exited non-zero'
printf '%s\n' "$out" | grep -qF -- '- #78 Check run thing — Independent Review=failure' \
    || fail "case 3b: check-run CI Gate should be accepted, got: $(printf '%s' "$out" | grep '#78' || true)"

# --- case 4: board comment with needs-operator lands under 例外 ----------------
reset_stubs
printf 'https://github.com/fagemx/edda/issues/613#issuecomment-1\tneeds-operator: relogin gh on 4090\n' >"$tmp/board.md"
export GH_BOARD_FILE="$tmp/board.md"
export EDDA_RECAP_FILE="$RECAP_CANNED"
out=$(run_digest --dry-run 2>&1) || fail 'case 4: daily-digest.sh exited non-zero'
printf '%s\n' "$out" | grep -qF -- '- 看板：needs-operator: relogin gh on 4090 (https://github.com/fagemx/edda/issues/613#issuecomment-1)' \
    || fail 'case 4: needs-operator board line not prefixed with - 看板：'
# it must come under 例外 (after the heading, before 成本)
printf '%s\n' "$out" | awk '/^## 例外/{f=1;next} /^## /{f=0} f' | grep -qF -- '- 看板：needs-operator: relogin gh on 4090 (https://github.com/fagemx/edda/issues/613#issuecomment-1)' \
    || fail 'case 4: board line is not inside the 例外 section'

# --- case 5: not dry-run posts to the board and notifies, in that order --------
reset_stubs
export EDDA_RECAP_FILE="$RECAP_CANNED"
run_digest >/dev/null 2>"$tmp/case5.err" || fail 'case 5: daily-digest.sh exited non-zero'
grep -q 'gh issue comment 613' "$GH_STUB_LOG" \
    || fail "case 5: gh issue comment call missing from log: $(cat "$GH_STUB_LOG")"
grep -q -- '--body-file' "$GH_STUB_LOG" \
    || fail 'case 5: gh issue comment must carry --body-file'
grep -q 'edda notify send --title Fleet digest' "$EDDA_STUB_LOG" \
    || fail "case 5: edda notify send call missing from log: $(cat "$EDDA_STUB_LOG")"
gh_line=$(grep -n 'issue comment' "$ORDER_LOG" | head -1 | cut -d: -f1)
edda_line=$(grep -n 'notify send' "$ORDER_LOG" | head -1 | cut -d: -f1)
[ -n "$gh_line" ] && [ -n "$edda_line" ] || fail 'case 5: calls not recorded in order log'
[ "$gh_line" -lt "$edda_line" ] || fail 'case 5: edda notify send ran before gh issue comment'

# --- case 6: missing recap heading aborts with exit 1 --------------------------
reset_stubs
printf '## 例外\n（無）\n## 成本\n- session_stats.estimated_cost_usd：n/a（0 筆量測，0 筆未量測）\n' >"$tmp/recap-short.md"
export EDDA_RECAP_FILE="$tmp/recap-short.md"
out=$(run_digest --dry-run 2>&1) && fail 'case 6: missing heading must exit 1, got success'
code=$?
[ "$code" = 1 ] || fail "case 6: expected exit 1, got $code"
printf '%s' "$out" | grep -qF '明天會做的' || fail "case 6: error should name the missing heading, got: $out"

printf 'daily-digest fixtures passed\n'
