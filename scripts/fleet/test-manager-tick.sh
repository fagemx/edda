#!/bin/sh
# Offline fixtures for scripts/fleet/manager-tick.sh (GH-674).
#
# Style follows scripts/fleet/test-daily-digest.sh — POSIX sh, `set -eu`, a
# mktemp dir with trap cleanup, stub `gh`/`edda` on PATH that log their argv
# to $ORDER_LOG, assertions with grep, `exit 1` with a message on failure,
# final line "manager-tick fixtures passed".
#
# The board is simulated STATEFULLY: the stub `gh` records every
# `gh issue comment <n> --body-file` body into $BOARD/<n>.body, so a second
# tick re-reads what the first tick wrote — that is exactly how the
# no-duplicate guarantees (reap, collision) are proven.
#
# Everything is offline: nothing here touches the real repository, the real
# board issue, or any live GitHub state; the scheduled-task launcher is not
# exercised here (manager-launch.ps1 -DryRun is checked separately).
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' 0 HUP INT TERM

fail() {
    printf 'FAIL: %s\n' "$1" >&2
    exit 1
}

# --- stubs: gh and edda ---------------------------------------------------------
# Both log their argv to $ORDER_LOG. `gh` answers per subcommand from env files
# (GH_READY_TSV, GH_OPEN_PRS_TSV, $BOARD/<n>.body, $DIFFS/<n>.files) and fails
# everything when GH_RC is non-zero (`gh pr diff` additionally fails when
# GH_PRC is non-zero). `edda` answers only `dispatch`.
STUBBIN="$tmp/bin"
mkdir -p "$STUBBIN"
cat >"$STUBBIN/gh" <<'EOF'
#!/bin/sh
echo "gh $*" >>"$ORDER_LOG"
case "$1 $2" in
  "issue list")
    [ -n "${GH_READY_TSV:-}" ] && cat "$GH_READY_TSV"
    exit ${GH_RC:-0}
    ;;
  "pr list")
    [ -n "${GH_OPEN_PRS_TSV:-}" ] && cat "$GH_OPEN_PRS_TSV"
    exit ${GH_RC:-0}
    ;;
  "issue view")
    num=$3
    [ -f "$BOARD/$num.body" ] && cat "$BOARD/$num.body"
    exit ${GH_RC:-0}
    ;;
  "issue comment")
    num=$3
    bodyfile=""
    while [ $# -gt 0 ]; do
        if [ "$1" = "--body-file" ]; then bodyfile=$2; fi
        shift
    done
    if [ -z "$bodyfile" ]; then
        echo "stub gh: issue comment needs --body-file" >&2
        exit 1
    fi
    cat "$bodyfile" >>"$BOARD/$num.body"
    printf '\n' >>"$BOARD/$num.body"
    exit 0
    ;;
  "pr diff")
    num=$3
    if [ -n "${GH_PRC:-}" ] && [ "$GH_PRC" != 0 ]; then exit "$GH_PRC"; fi
    [ -f "$DIFFS/$num.files" ] && cat "$DIFFS/$num.files"
    exit 0
    ;;
esac
exit 0
EOF
cat >"$STUBBIN/edda" <<'EOF'
#!/bin/sh
echo "edda $*" >>"$ORDER_LOG"
case "$1 $2" in
  "dispatch "*)
    [ -n "${EDDA_DISPATCH_OUT:-}" ] && printf '%s\n' "$EDDA_DISPATCH_OUT"
    exit ${EDDA_DISPATCH_EXIT:-0}
    ;;
esac
exit 0
EOF
chmod +x "$STUBBIN/gh" "$STUBBIN/edda"

export ORDER_LOG="$tmp/order.log"
export PATH="$STUBBIN:$PATH"

BOARD="" ; STATE="" ; LANES="" ; LROOT="" ; DIFFS="" ; RULES=""
export BOARD STATE LANES LROOT DIFFS RULES

# fresh board / state / lanes / rules fixture for one case
new_env() {
    BOARD="$tmp/board-$1"
    STATE="$tmp/state-$1"
    LANES="$tmp/lanes-$1"
    LROOT="$tmp/lroot-$1"
    DIFFS="$tmp/diffs-$1"
    RULES="$tmp/rules-$1.md"
    mkdir -p "$BOARD" "$STATE" "$LANES" "$LROOT" "$DIFFS"
    cp "$root/docs/fleet/rules.md" "$RULES"
    : >"$ORDER_LOG"
    GH_RC=0
    GH_PRC=0
    EDDA_DISPATCH_EXIT=0
    # a realistic successful dispatch: done, cost unmeasured (null -> n/a)
    EDDA_DISPATCH_OUT='{"outcome":"done","result_text":"lane started","cost_usd":null}'
    export GH_PRC EDDA_DISPATCH_EXIT EDDA_DISPATCH_OUT
    export EDDA_MANAGER_STATE="$STATE" EDDA_LANES_DIR="$LANES" EDDA_LANE_ROOT="$LROOT"
    export EDDA_RULES_FILE="$RULES"
}

run_tick() { # args... ; stdout->out.txt stderr->err.txt ; rc in tick_rc
    tick_rc=0
    sh "$root/scripts/fleet/manager-tick.sh" "$@" >"$tmp/tick.out" 2>"$tmp/tick.err" || tick_rc=$?
}

order_line() { # first line number matching pattern in the order log
    grep -n "$1" "$ORDER_LOG" | head -1 | cut -d: -f1
}

count_in() { # pattern file -> occurrence count (line based)
    grep -c "$1" "$2" 2>/dev/null || true
}

# --- case 1: both scripts parse -------------------------------------------------
sh -n "$root/scripts/fleet/manager-tick.sh" || fail 'case 1: manager-tick.sh fails sh -n'
sh -n "$root/scripts/fleet/test-manager-tick.sh" || fail 'case 1: test file fails sh -n'

# --- case 2: free fleet:ready issue is claimed (R1 claim-first) and dispatched --
new_env 2
printf '9001\tFleet: build the thing\n' >"$tmp/ready2.tsv"
export GH_READY_TSV="$tmp/ready2.tsv"
: >"$tmp/prs2.tsv"
export GH_OPEN_PRS_TSV="$tmp/prs2.tsv"

run_tick --no-board
[ "$tick_rc" = 0 ] || fail "case 2: tick exited $tick_rc: $(cat "$tmp/tick.err")"

c=$(order_line 'gh issue comment 9001')
d=$(order_line 'edda dispatch --agent pi')
[ -n "$c" ] || fail 'case 2: no claim comment on the free issue'
[ -n "$d" ] || fail 'case 2: no dispatch for the free issue'
[ "$c" -lt "$d" ] || fail 'case 2: R1 requires the taking: comment BEFORE dispatch'

grep -q '^taking: 4090/manager at ' "$BOARD/9001.body" \
    || fail "case 2: claim comment body wrong: $(cat "$BOARD/9001.body")"
grep -q -- '--budget-usd 0.2' "$ORDER_LOG" || fail 'case 2: dispatch missing --budget-usd 0.2'
grep -q -- '--prompt-file' "$ORDER_LOG" || fail 'case 2: dispatch missing --prompt-file'
grep -q -- '--agent pi' "$ORDER_LOG" || fail 'case 2: dispatch missing --agent pi'

awk -F'\t' '$1 == "lane-gh9001" && $2 == "9001"' "$STATE/dispatched.tsv" | grep -q . \
    || fail 'case 2: dispatched.tsv has no lane-gh9001 row'
[ -f "$STATE/brief-gh9001.md" ] || fail 'case 2: lane brief file was not written'

grep -Eq '^in-progress [0-9]+ · blocked [0-9]+ · needs-operator [0-9]+ · cost today \$[0-9]+\.[0-9]{2} · wake [0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:]+Z · by 4090/manager$' "$tmp/tick.out" \
    || fail "case 2: status line missing or malformed: $(cat "$tmp/tick.out")"
grep -q 'gh issue comment 613' "$ORDER_LOG" && fail 'case 2: --no-board must not post to the board'

# --- case 3: another machine already taking: -> do NOT dispatch (R1) ------------
new_env 3
printf '9002\tTaken elsewhere\n' >"$tmp/ready3.tsv"
export GH_READY_TSV="$tmp/ready3.tsv"
: >"$tmp/prs3.tsv"
export GH_OPEN_PRS_TSV="$tmp/prs3.tsv"
printf -- 'taking: docs/worker-1 at 2026-09-05T00:00:00Z\n' >"$BOARD/9002.body"

run_tick --no-board
[ "$tick_rc" = 0 ] || fail "case 3: tick exited $tick_rc: $(cat "$tmp/tick.err")"
if grep -q 'edda dispatch' "$ORDER_LOG"; then fail 'case 3: must not dispatch an issue already taken by another machine'; fi
if grep -q 'gh issue comment 9002' "$ORDER_LOG"; then fail 'case 3: must not comment on an already-taken issue'; fi

# --- case 4: a RELEASED (struck-through) taking: is not a live claim (R13) ------
new_env 4
printf '9003\tReleased claim\n' >"$tmp/ready4.tsv"
export GH_READY_TSV="$tmp/ready4.tsv"
: >"$tmp/prs4.tsv"
export GH_OPEN_PRS_TSV="$tmp/prs4.tsv"
{
    printf -- '~~taking: docs/worker-9 at 2026-09-04T00:00:00Z~~\n'
    printf -- 'RELEASED — this claim is withdrawn\n'
} >"$BOARD/9003.body"

run_tick --no-board
[ "$tick_rc" = 0 ] || fail "case 4: tick exited $tick_rc: $(cat "$tmp/tick.err")"
grep -q 'edda dispatch' "$ORDER_LOG" || fail 'case 4: a released claim must not block dispatch (R13)'

# --- case 5: an open delivery PR on the issue -> skip (R21) ----------------------
new_env 5
printf '9004\tAlready has a PR\n' >"$tmp/ready5.tsv"
export GH_READY_TSV="$tmp/ready5.tsv"
printf '8001\tfeat/gh9004-add-x\tgh-9004: add x\n' >"$tmp/prs5.tsv"
export GH_OPEN_PRS_TSV="$tmp/prs5.tsv"

run_tick --no-board
[ "$tick_rc" = 0 ] || fail "case 5: tick exited $tick_rc: $(cat "$tmp/tick.err")"
if grep -q 'edda dispatch' "$ORDER_LOG"; then fail 'case 5: must not dispatch an issue with an open PR (R21)'; fi
if grep -q 'gh issue comment 9004' "$ORDER_LOG"; then fail 'case 5: must not claim an issue that already has an open PR'; fi

# --- case 6: reap a killed lane once, with the worktree sha ----------------------
new_env 6
printf '9001\tLane will die\n' >"$tmp/ready6.tsv"
export GH_READY_TSV="$tmp/ready6.tsv"
: >"$tmp/prs6.tsv"
export GH_OPEN_PRS_TSV="$tmp/prs6.tsv"

run_tick --no-board   # tick 1: dispatch creates the tracked lane
[ "$tick_rc" = 0 ] || fail "case 6: tick 1 exited $tick_rc: $(cat "$tmp/tick.err")"

# fixture: the wrapper was killed with taskkill /T /F; lane-stop.ps1 wrote the
# terminal receipt (=== EXIT === + done-file). Heartbeat absence alone would
# NOT be a death proof (R3/R17) — the tick must reap on the receipt, not on
# silence.
printf 'wrapper killed by taskkill /T /F\n=== EXIT code=1 registration=unregistered done=written ===\n' >"$LANES/lane-gh9001.log"
echo 1 >"$LANES/lane-gh9001.done"

mkdir -p "$LROOT/lane-gh9001"
GIT_AUTHOR_NAME=fixture GIT_AUTHOR_EMAIL=f@f GIT_COMMITTER_NAME=fixture GIT_COMMITTER_EMAIL=f@f \
    git -C "$LROOT/lane-gh9001" init -q
GIT_AUTHOR_NAME=fixture GIT_AUTHOR_EMAIL=f@f GIT_COMMITTER_NAME=fixture GIT_COMMITTER_EMAIL=f@f \
    git -C "$LROOT/lane-gh9001" commit --allow-empty -q -m wip
sha=$(git -C "$LROOT/lane-gh9001" rev-parse HEAD)

run_tick --no-board   # tick 2: reap
[ "$tick_rc" = 0 ] || fail "case 6: tick 2 exited $tick_rc: $(cat "$tmp/tick.err")"
grep -qF "blocked: lane died at $sha" "$BOARD/9001.body" \
    || fail "case 6: no 'blocked: lane died at <sha>' comment (sha $sha): $(cat "$BOARD/9001.body")"

run_tick --no-board   # tick 3: no duplicate
[ "$tick_rc" = 0 ] || fail "case 6: tick 3 exited $tick_rc: $(cat "$tmp/tick.err")"
[ "$(count_in "blocked: lane died at $sha" "$BOARD/9001.body")" = "1" ] \
    || fail 'case 6: duplicate blocked comment on the second tick'

# --- case 7: two artifacts on one issue -> keep the more complete one (R2) -------
new_env 7
: >"$tmp/ready7.tsv"
export GH_READY_TSV="$tmp/ready7.tsv"
printf '8100\tfeat/gh9100-one\tgh-9100: artifact one\n' >"$tmp/prs7.tsv"
printf '8101\tfeat/gh9100-two\tgh-9100: artifact two\n' >>"$tmp/prs7.tsv"
export GH_OPEN_PRS_TSV="$tmp/prs7.tsv"
printf 'a.sh\nb.md\nc.rs\n' >"$DIFFS/8100.files"
printf 'a.sh\nb.md\nc.rs\nd.rs\ne.rs\nf.md\ng.md\n' >"$DIFFS/8101.files"

run_tick --no-board
[ "$tick_rc" = 0 ] || fail "case 7: tick exited $tick_rc: $(cat "$tmp/tick.err")"
grep -q 'R2' "$BOARD/9100.body" || fail 'case 7: collision comment does not cite R2'
grep -qF '#8101' "$BOARD/9100.body" || fail 'case 7: the more complete PR (8101, 7 files) should be kept'
grep -qF '#8100' "$BOARD/9100.body" || fail 'case 7: collision comment should name the losing PR 8100'

run_tick --no-board
[ "$tick_rc" = 0 ] || fail "case 7: tick 2 exited $tick_rc: $(cat "$tmp/tick.err")"
[ "$(count_in 'R2 collision' "$BOARD/9100.body")" = "1" ] \
    || fail 'case 7: duplicate R2 collision comment on the second tick'

# --- case 8: manager: no rule for X -> append one line under 管理者自訂 (R8) ------
new_env 8
printf -- 'manager: no rule for nightly lane-warm scheduling\n' >"$BOARD/613.body"

run_tick --no-board
[ "$tick_rc" = 0 ] || fail "case 8: tick exited $tick_rc: $(cat "$tmp/tick.err")"
grep -q 'nightly lane-warm scheduling' "$RULES" || fail 'case 8: R8 rule line was not appended to the rules fixture'
tail -1 "$RULES" | grep -q 'nightly lane-warm scheduling' \
    || fail 'case 8: appended line is not under the 管理者自訂 section (expected as the section tail)'
[ -z "$(git -C "$root" status --porcelain docs/fleet/rules.md)" ] \
    || fail 'case 8: the repository rules.md must not be touched by a fixture run'

# doneWhen 無規則 is two halves — 「追加一條並以留言附 patch」. The append is
# asserted above; this asserts the patch comment, which is the half that
# survives the lane worktree. It is posted even under --no-board, like the
# other alarms: --no-board silences the status line, not the carriers.
grep -q 'manager: R8 rule appended by' "$BOARD/613.body"     || fail "case 8: R8 patch comment missing from the board: $(cat "$BOARD/613.body")"
grep -q '^+.*nightly lane-warm scheduling' "$BOARD/613.body"     || fail 'case 8: R8 patch comment carries no + line for the appended rule'
grep -q -- '--- a/docs/fleet/rules.md' "$BOARD/613.body"     || fail 'case 8: R8 patch comment is not a unified diff (no --- header)'
grep -q '^@@ -[0-9]*,3 +[0-9]*,4 @@' "$BOARD/613.body"     || fail 'case 8: R8 patch hunk header is missing or has no context lines'

run_tick --no-board
[ "$tick_rc" = 0 ] || fail "case 8: tick 2 exited $tick_rc: $(cat "$tmp/tick.err")"
[ "$(count_in 'nightly lane-warm scheduling' "$RULES")" = "1" ] \
    || fail 'case 8: duplicate R8 append on the second tick'

# --- case 9: dispatch exits non-zero -> FAILED log, no tracking -------------------
new_env 9
printf '9005\tDispatch will fail\n' >"$tmp/ready9.tsv"
export GH_READY_TSV="$tmp/ready9.tsv"
: >"$tmp/prs9.tsv"
export GH_OPEN_PRS_TSV="$tmp/prs9.tsv"
EDDA_DISPATCH_EXIT=1
export EDDA_DISPATCH_EXIT

run_tick --no-board
[ "$tick_rc" != 0 ] || fail 'case 9: a failed dispatch must make the tick exit non-zero'
grep -q 'manager-tick: FAILED' "$tmp/tick.err" \
    || fail "case 9: FAILED log line missing: $(cat "$tmp/tick.err")"
# GH-674 doneWhen (死亡可見): the FAILED alarm must also reach the board
# issue, not only this wake's stderr/log.
grep -q 'manager-tick: FAILED' "$BOARD/613.body" \
    || fail "case 9: FAILED alarm missing from the board issue: $(cat "$BOARD/613.body")"
grep -q 'gh issue comment 613' "$ORDER_LOG" \
    || fail 'case 9: FAILED alarm was not posted to the board issue'
awk -F'\t' '$1 == "lane-gh9005"' "$STATE/dispatched.tsv" | grep -q . \
    && fail 'case 9: a failed dispatch must not be tracked in dispatched.tsv'

# --- case 10: dispatch reporting $0.00 cost is a failure (R15) -------------------
new_env 10
printf '9006\tZero-cost round\n' >"$tmp/ready10.tsv"
export GH_READY_TSV="$tmp/ready10.tsv"
: >"$tmp/prs10.tsv"
export GH_OPEN_PRS_TSV="$tmp/prs10.tsv"
EDDA_DISPATCH_OUT='{"outcome":"done","result_text":null,"cost_usd":0}'
export EDDA_DISPATCH_OUT

run_tick --no-board
[ "$tick_rc" != 0 ] || fail 'case 10: a $0.00 dispatch must make the tick exit non-zero (R15)'
grep -q 'manager-tick: FAILED' "$tmp/tick.err" \
    || fail "case 10: FAILED log line missing: $(cat "$tmp/tick.err")"
grep -q 'manager-tick: FAILED' "$BOARD/613.body" \
    || fail "case 10: FAILED alarm missing from the board issue: $(cat "$BOARD/613.body")"

# --- case 11: daily budget exhausted -> no dispatch (R7/R14) ----------------------
new_env 11
printf '9007\tBudget case\n' >"$tmp/ready11.tsv"
export GH_READY_TSV="$tmp/ready11.tsv"
: >"$tmp/prs11.tsv"
export GH_OPEN_PRS_TSV="$tmp/prs11.tsv"
printf '5.00\n' >"$STATE/cost-$(date -u +%F).txt"

run_tick --no-board
[ "$tick_rc" = 0 ] || fail "case 11: tick exited $tick_rc: $(cat "$tmp/tick.err")"
if grep -q 'edda dispatch' "$ORDER_LOG"; then fail 'case 11: budget exhausted (R7) must not dispatch'; fi
grep -q 'BLOCKED-BY-RULE' "$tmp/tick.err" \
    || fail "case 11: budget skip should be logged as BLOCKED-BY-RULE: $(cat "$tmp/tick.err")"

# --- case 12: gh read failure -> needs-operator once (R11), fail closed -----------
new_env 12
printf '9008\tGh broken\n' >"$tmp/ready12.tsv"
export GH_READY_TSV="$tmp/ready12.tsv"
: >"$tmp/prs12.tsv"
export GH_OPEN_PRS_TSV="$tmp/prs12.tsv"
GH_RC=1
export GH_RC

run_tick --no-board
[ "$tick_rc" != 0 ] || fail 'case 12: a gh read failure must make the tick exit non-zero (fail closed)'
grep -q 'needs-operator: relogin gh on 4090' "$tmp/tick.err" \
    || fail "case 12: needs-operator line missing: $(cat "$tmp/tick.err")"
if grep -q 'edda dispatch' "$ORDER_LOG"; then fail 'case 12: must not dispatch when gh is broken'; fi

run_tick --no-board   # second wake: R11 says record once, not every tick
grep -q 'needs-operator: relogin gh on 4090' "$tmp/tick.err" \
    && fail 'case 12: needs-operator recorded twice; R11 says only once'

# --- case 13: --dry-run reads everything, writes nothing --------------------------
new_env 13
printf '9005\tWould dispatch\n' >"$tmp/ready13.tsv"
export GH_READY_TSV="$tmp/ready13.tsv"
: >"$tmp/prs13.tsv"
export GH_OPEN_PRS_TSV="$tmp/prs13.tsv"
printf -- 'manager: no rule for dry-run probe\n' >"$BOARD/613.body"
cp "$RULES" "$tmp/rules13-before.md"

run_tick --dry-run --no-board
[ "$tick_rc" = 0 ] || fail "case 13: dry-run exited $tick_rc: $(cat "$tmp/tick.err")"
if grep -q 'edda dispatch' "$ORDER_LOG"; then fail 'case 13: dry-run must not dispatch'; fi
if grep -q 'gh issue comment' "$ORDER_LOG"; then fail 'case 13: dry-run must not comment anywhere'; fi
diff -q "$tmp/rules13-before.md" "$RULES" >/dev/null || fail 'case 13: dry-run must not edit rules'
[ -z "$(ls -A "$STATE" 2>/dev/null)" ] || fail "case 13: dry-run must not write state: $(ls -A "$STATE")"

# --- case 14: status line is posted to the board unless --no-board ----------------
new_env 14
: >"$tmp/ready14.tsv"
export GH_READY_TSV="$tmp/ready14.tsv"
: >"$tmp/prs14.tsv"
export GH_OPEN_PRS_TSV="$tmp/prs14.tsv"

run_tick
[ "$tick_rc" = 0 ] || fail "case 14: tick exited $tick_rc: $(cat "$tmp/tick.err")"
grep -q 'gh issue comment 613' "$ORDER_LOG" || fail 'case 14: status line was not posted to the board issue'
grep -q '^in-progress 0 · blocked 0 · needs-operator 0 · cost today \$0.00 · wake ' "$BOARD/613.body" \
    || fail "case 14: board status body wrong: $(cat "$BOARD/613.body")"

# --- case 15: gh pr diff failure -> fail closed, R2 verdict withheld -------------
new_env 15
: >"$tmp/ready15.tsv"
export GH_READY_TSV="$tmp/ready15.tsv"
printf '8100\tfeat/gh9100-one\tgh-9100: artifact one\n' >"$tmp/prs15.tsv"
printf '8101\tfeat/gh9100-two\tgh-9100: artifact two\n' >>"$tmp/prs15.tsv"
export GH_OPEN_PRS_TSV="$tmp/prs15.tsv"
printf 'a.sh\nb.md\nc.rs\n' >"$DIFFS/8100.files"
printf 'a.sh\nb.md\nc.rs\nd.rs\ne.rs\nf.md\ng.md\n' >"$DIFFS/8101.files"
GH_PRC=1
export GH_PRC

# A transient `gh pr diff` failure must never read as 0 files: that would
# post an R2 verdict (board grep + COLLISIONS row) that is never
# re-adjudicated. The tick fails closed instead — needs-operator once, no
# R2 comment, no COLLISIONS row.
run_tick --no-board
[ "$tick_rc" != 0 ] || fail 'case 15: a gh pr diff failure must make the tick exit non-zero (fail closed)'
grep -q 'needs-operator: relogin gh on 4090' "$tmp/tick.err" \
    || fail "case 15: needs-operator line missing: $(cat "$tmp/tick.err")"
[ ! -s "$BOARD/9100.body" ] \
    || fail "case 15: R2 verdict must be withheld when the diff read fails: $(cat "$BOARD/9100.body")"
[ ! -s "$STATE/collisions.tsv" ] \
    || fail 'case 15: a failed diff read must not record a COLLISIONS row'

# recovery wake: the transient failure is gone — the R2 verdict is decided
# then, from the full counts (8101 wins, 7 files over 3).
GH_PRC=0
: >"$ORDER_LOG"

run_tick --no-board
[ "$tick_rc" = 0 ] || fail "case 15: recovery tick exited $tick_rc: $(cat "$tmp/tick.err")"
grep -q 'R2 collision: keep #8101' "$BOARD/9100.body" \
    || fail "case 15: recovery tick did not post the R2 verdict: $(cat "$BOARD/9100.body")"

printf 'manager-tick fixtures passed\n'
