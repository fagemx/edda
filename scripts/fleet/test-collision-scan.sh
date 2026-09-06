#!/bin/sh
# Offline self-test for scripts/fleet/collision-scan.sh and the lane-launch
# claim gate (GH-912).
#
# collision-scan cases stub gh with fixture JSON + real jq (the script's --jq
# filters run for real). The lane-launch gate cases run the real launcher
# under pwsh with a fake `gh` on PATH: a foreign claim must refuse BEFORE any
# registration (no scheduled task by the name lane-launch.ps1 builds), the
# launcher's own claim and the no-claim dry run behave exactly as before.
# Those cases are Windows-only, gated on the OS rather than on pwsh being
# installed; the absence query carries a control case proving it can also
# report presence, and case 7 binds the test's edda-lane-$Name derivation to
# the name the launcher itself prints on registration (GH-971).
#
# Everything is written under one temp dir, plus the scheduled tasks named in
# cleanup(); nothing else is touched. No script under test ever comments,
# labels, closes, or merges.
#
# usage: sh scripts/fleet/test-collision-scan.sh
set -eu

cd "$(git rev-parse --show-toplevel)"
root=$(pwd -W 2>/dev/null || pwd)
collision=scripts/fleet/collision-scan.sh
launcher=scripts/fleet/lane-launch.ps1

sh -n "$collision" || { echo "FAIL: sh -n $collision" >&2; exit 1; }
sh -n "$0" || { echo "FAIL: sh -n $0" >&2; exit 1; }

work=$(mktemp -d "${TMPDIR:-/tmp}/test-collision-scan.XXXXXX")
# Scheduled tasks outlive the temp dir, so every task this test can register
# is reclaimed here too: the task_absent control, and the dry-run tasks a raced
# scheduler poll leaves registered when the launcher exits early. Each name is
# added by reap_later BEFORE the launch that would create it (a kill mid-launch
# still reclaims it), and only when no task by that name pre-exists — so a
# production task that happens to share a name is never reaped (PR #995
# Round 1 P2-1). By name, never by wildcard.
registered_tasks=
cleanup() {
    for t in $registered_tasks; do
        pwsh -NoProfile -Command "Unregister-ScheduledTask -TaskName '$t' -Confirm:\$false -ErrorAction SilentlyContinue" >/dev/null 2>&1 || :
    done
    rm -r -f "$work"
}
trap cleanup 0 HUP INT TERM
mkdir -p "$work/lanes" "$work/cwd"
git init -q "$work/cwd"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

run_capture() { # command... -> stdout in $out, exit code in $rc; set -e safe
    rc=0
    out=$("$@") || rc=$?
}

# ── collision-scan cases (stubbed gh, real jq) ───────────────────────

make_gh() { # $1=stub dir, $2=fixture dir; the stub resolves --jq like real gh
    mkdir -p "$1"
    {
        printf '#!/bin/sh\n'
        printf "FIXDIR='%s'\n" "$2"
        cat <<'STUB'
jq_expr=
prev=
for a in "$@"; do
    [ "$prev" = "--jq" ] && jq_expr=$a
    prev=$a
done
resp=
case "$1 $2" in
    "issue list") resp='[{"number":887},{"number":900},{"number":901}]' ;;
    "pr list") resp=$(cat "$FIXDIR/prs.json" 2>/dev/null || echo '[]') ;;
    "issue view")
        case "$3" in
            887) resp=$(cat "$FIXDIR/issue-887.json") ;;
            900) resp=$(cat "$FIXDIR/issue-900.json") ;;
            901) resp=$(cat "$FIXDIR/issue-901.json") ;;
            *) resp='[]' ;;
        esac ;;
esac
[ -n "$resp" ] || resp='[]'
if [ -n "$jq_expr" ]; then
    printf '%s' "$resp" | jq -r "$jq_expr"
else
    printf '%s\n' "$resp"
fi
STUB
    } >"$1/gh"
    chmod +x "$1/gh"
}

# case 1 — dirty fixtures: a double-claimed issue and a double-built issue
f="$work/fixtures-dirty"
mkdir -p "$f"
cat >"$f/issue-887.json" <<'JSON'
{"comments":[{"createdAt":"2026-09-05T02:45:30Z","body":"taking: 4090/worker-3 at 2026-09-05T02:45:30Z"},
 {"createdAt":"2026-09-05T02:50:01Z","body":"taking: docs/worker-1 at 2026-09-05T02:50:01Z"}]}
JSON
echo '{"comments":[]}' >"$f/issue-900.json"
echo '{"comments":[]}' >"$f/issue-901.json"
cat >"$f/prs.json" <<'JSON'
[{"number":906,"title":"fix(fleet): gh887 compare slice","headRefName":"feat/gh887-compare-slice"},
 {"number":907,"title":"fix(fleet): shadow compare for gh887","headRefName":"feat/gh887-shadow-review"},
 {"number":908,"title":"feat(fleet): unrelated work","headRefName":"feat/gh674-manager-v0"}]
JSON
make_gh "$work/bin-dirty" "$f"
run_capture env PATH="$work/bin-dirty:$PATH" sh "$collision"
[ "$rc" = "1" ] || fail "dirty sweep exit $rc, want 1: $out"
printf '%s\n' "$out" | command grep -q 'collision: issue 887 claimed by 2 distinct identities: 4090/worker-3, docs/worker-1' \
    || fail "dirty sweep misses the double-claim line: $out"
printf '%s\n' "$out" | command grep -q 'collision: issue 887 has 2 open PRs naming it: 906, 907' \
    || fail "dirty sweep misses the double-build line: $out"
[ "$(printf '%s\n' "$out" | command grep -c 'collision:')" = "2" ] || fail "dirty sweep must print exactly two lines: $out"
if printf '%s\n' "$out" | command grep -q 'issue 901 '; then
    fail "single-claimed issue 901 must be silent"
fi
echo "ok 1 collision sweep reports both classes"

# case 2 — clean fixtures: single identities, one PR per issue -> silent, exit 0
f="$work/fixtures-clean"
mkdir -p "$f"
cat >"$f/issue-887.json" <<'JSON'
{"comments":[{"createdAt":"2026-09-05T02:45:30Z","body":"taking: 4090/worker-3 at 2026-09-05T02:45:30Z"}]}
JSON
echo '{"comments":[]}' >"$f/issue-900.json"
echo '{"comments":[]}' >"$f/issue-901.json"
cat >"$f/prs.json" <<'JSON'
[{"number":906,"title":"fix(fleet): gh887 compare slice","headRefName":"feat/gh887-compare-slice"},
 {"number":908,"title":"feat(fleet): unrelated work","headRefName":"feat/gh674-manager-v0"}]
JSON
make_gh "$work/bin-clean" "$f"
run_capture env PATH="$work/bin-clean:$PATH" sh "$collision"
[ "$rc" = "0" ] || fail "clean sweep exit $rc, want 0: $out"
[ -z "$out" ] || fail "clean sweep must be silent, got: $out"
echo "ok 2 clean sweep is silent"

# case 3 — a gh read failure must not report clean
mkdir -p "$work/bin-fail"
{
    printf '#!/bin/sh\n'
    printf 'case "$1 $2" in\n'
    printf '    "issue list") exit 1 ;;\n'
    printf 'esac\n'
    printf 'exit 0\n'
} >"$work/bin-fail/gh"
chmod +x "$work/bin-fail/gh"
run_capture env PATH="$work/bin-fail:$PATH" sh "$collision"
[ "$rc" = "2" ] || fail "gh failure exit $rc, want 2"
echo "ok 3 gh failure is an error, never clean"

# ── lane-launch claim gate (real pwsh, fake gh on PATH) ──────────────

# These cases drive the real launcher, which registers Windows Scheduled
# Tasks; Get-ScheduledTask exists only on Windows. `command -v pwsh` is a
# presence check, not an OS check — ubuntu runners ship pwsh, so gating
# alone ran this whole block on Linux, where the task query could only ever
# fail and case 4 fired on every run. That is why #910 quarantined this file
# in scripts/fleet/run-fleet-tests.sh (GH-971).
case "$(uname -s 2>/dev/null || echo unknown)" in
    MINGW*|MSYS*|CYGWIN*|Windows*) on_windows=1 ;;
    *) on_windows=0 ;;
esac

if [ "$on_windows" = 1 ] && command -v pwsh >/dev/null 2>&1; then
    gf="$work/gate-fixtures"
    mkdir -p "$gf"
    cat >"$gf/issue9-foreign.json" <<'JSON'
{"comments":[{"createdAt":"2026-09-05T02:45:30Z","body":"taking: 4090/worker-3 at 2026-09-05T02:45:30Z"}]}
JSON
    cat >"$gf/issue9-self.json" <<'JSON'
{"comments":[{"createdAt":"2026-09-05T02:45:30Z","body":"taking: docs/self at 2026-09-05T02:45:30Z"}]}
JSON
    echo '[]' >"$gf/prs-empty.json"
    mkdir -p "$work/bin-gate"
    {
        printf '#!/bin/sh\n'
        printf "GATEDIR='%s'\n" "$gf"
        cat <<'STUB'
jq_expr=
prev=
for a in "$@"; do
    [ "$prev" = "--jq" ] && jq_expr=$a
    prev=$a
done
resp=
case "$1 $2" in
    "pr list") resp=$(cat "$GATEDIR/prs-empty.json") ;;
    "issue view")
        case "$GF_MODE" in
            foreign) resp=$(cat "$GATEDIR/issue9-foreign.json") ;;
            self)    resp=$(cat "$GATEDIR/issue9-self.json") ;;
            *)       resp='[]' ;;
        esac ;;
esac
[ -n "$resp" ] || resp='[]'
if [ -n "$jq_expr" ]; then
    printf '%s' "$resp" | jq -r "$jq_expr"
else
    printf '%s\n' "$resp"
fi
STUB
    } >"$work/bin-gate/gh"
    chmod +x "$work/bin-gate/gh"
    # lane-launch.ps1:178 registers "edda-lane-$Name", so the lane named
    # edda-lane-gh9 below becomes the scheduled task edda-lane-edda-lane-gh9.
    # Querying the lane name instead of the task name made every "registered
    # nothing" assertion here unfalsifiable (GH-971).
    lane=edda-lane-gh9
    task="edda-lane-$lane"
    task_absent() { # 0 when no scheduled task named $1 exists, 1 when one does
        pwsh -NoProfile -Command "if (Get-ScheduledTask -TaskName '$1' -ErrorAction SilentlyContinue) { exit 1 } else { exit 0 }"
    }
    reap_later() { # mark $1 for cleanup — unless a foreign task pre-exists
        if task_absent "$1"; then
            registered_tasks="$registered_tasks $1"
        else
            echo "WARN: scheduled task $1 pre-exists; this run will not reap it" >&2
        fi
    }

    # case 3a — control. An assertion that can only ever pass proves nothing,
    # so before relying on task_absent, prove it reports BOTH answers: register
    # a throwaway task, require PRESENT, remove it, require ABSENT. Without
    # this a query naming a task nobody registers reads exactly like a launcher
    # that registered nothing, which is how the doubled prefix survived review.
    control="edda-selftest-collision-$$"
    reap_later "$control"
    pwsh -NoProfile -Command "\$a = New-ScheduledTaskAction -Execute 'cmd.exe' -Argument '/c exit 0'; Register-ScheduledTask -TaskName '$control' -Action \$a -RunLevel Limited | Out-Null" >/dev/null 2>&1 \
        || fail "control: cannot register a scheduled task; every absence assertion below would be untestable"
    if task_absent "$control"; then
        fail "control: task_absent called a registered task absent — the query is dead"
    fi
    pwsh -NoProfile -Command "Unregister-ScheduledTask -TaskName '$control' -Confirm:\$false" >/dev/null 2>&1 \
        || fail "control: cannot unregister $control"
    task_absent "$control" || fail "control: task_absent still reports $control after removal"
    echo "ok 3a task_absent reports both presence and absence"

    # case 4 — foreign claim refuses before registering anything.
    # $task is marked for cleanup here — before the FIRST launch that could
    # register it under the regression this case exists to detect (Round 1
    # P2-2), not merely before case 5's dry run.
    reap_later "$task"
    : >"$work/gate-fixture.json"
    rc=0
    out=$(GF="$gf" GF_MODE=foreign PATH="$work/bin-gate:$PATH" \
        pwsh -NoProfile -File "$launcher" -Name "$lane" \
        -Brief "$work/gate-fixture.json" -Cwd "$work/cwd" \
        -Machine docs/other -LogDir "$work/lanes" 2>&1) || rc=$?
    [ "$rc" != "0" ] || fail "foreign claim must refuse, exit 0: $out"
    printf '%s' "$out" | command grep -q 'claim guard refused' || fail "refusal must name the guard: $out"
    printf '%s' "$out" | command grep -q '4090/worker-3' || fail "refusal must name the claimant: $out"
    task_absent "$task" || fail "a refused launch must not register the scheduled task"
    [ ! -f "$work/lanes/$lane.log" ] || fail "a refused launch must not create the lane log"
    echo "ok 4 foreign claim refuses before registering"

    # case 5 — the launcher's own claim passes the gate.
    #
    # What this case owns is the gate decision, and the launcher prints its
    # dry-run receipt lines before it registers anything, so the proof needs no
    # scheduler at all. It deliberately does not assert the exit code: past the
    # gate, -DryRun registers a task, starts it and polls
    # Get-ScheduledTaskInfo, which intermittently returns nothing and exits the
    # launcher 1 on a Windows workstation ("dry-run task ... exists but its
    # scheduler result is unavailable"). Case 7 already carries that round trip
    # with the retries the race needs; asserting it a second time here bought
    # no coverage and put a flaky assertion in a blocking CI gate (GH-971, same
    # scheduler path as GH-963).
    rc=0
    out=$(GF="$gf" GF_MODE=self PATH="$work/bin-gate:$PATH" \
        pwsh -NoProfile -File "$launcher" -Name "$lane" \
        -Brief "$work/gate-fixture.json" -Cwd "$work/cwd" \
        -Machine docs/self -LogDir "$work/lanes" -DryRun \
        -Owns scripts/fleet/test-collision-scan.sh 2>&1) || rc=$?
    if printf '%s' "$out" | command grep -q 'claim guard refused'; then
        fail "own claim must pass the guard: $out"
    fi
    printf '%s' "$out" | command grep -q 'dry-run real command line' \
        || fail "own claim did not reach the dry-run body (launcher exit $rc): $out"
    printf '%s' "$out" | command grep -q 'dry-run done=' \
        || fail "dry-run receipt lines missing (launcher exit $rc): $out"
    echo "ok 5 own claim passes the guard and reaches the dry-run body"

    # case 6 — issue-shaped lane without -Machine refuses
    rc=0
    out=$(GF="$gf" GF_MODE=self PATH="$work/bin-gate:$PATH" \
        pwsh -NoProfile -File "$launcher" -Name "$lane" \
        -Brief "$work/gate-fixture.json" -Cwd "$work/cwd" \
        -LogDir "$work/lanes" -DryRun 2>&1) || rc=$?
    [ "$rc" != "0" ] || fail "missing -Machine must refuse, exit 0: $out"
    printf '%s' "$out" | command grep -q 'R9/R21' || fail "refusal must name R9/R21: $out"
    echo "ok 6 missing -Machine refuses"

    # case 7 — non-issue-shaped lane names are unchanged (no -Machine needed)
    # The dry run races the Task Scheduler's own state query; retry a couple
    # of times before declaring the receipt broken.
    scratch_lane=scratch-lane
    scratch_task="edda-lane-$scratch_lane"
    reap_later "$scratch_task"
    rc=1
    attempt=0
    while [ "$rc" != "0" ] && [ "$attempt" -lt 3 ]; do
        attempt=$((attempt + 1))
        rc=0
        out=$(PATH="$work/bin-gate:$PATH" \
            pwsh -NoProfile -File "$launcher" -Name "$scratch_lane" \
            -Brief "$work/gate-fixture.json" -Cwd "$work/cwd" \
            -LogDir "$work/lanes" -DryRun \
            -Owns scripts/fleet/test-collision-scan.sh 2>&1) || rc=$?
        [ "$rc" = "0" ] || sleep 2
    done
    [ "$rc" = "0" ] || fail "non-issue lane dry run exit $rc after retries: $out"
    # Round 1 P0-1: bind this test's name derivation to the name the launcher
    # actually registered. On the success path the launcher prints
    # "dry-run task=<registered name> state=..." from its own $TaskName, so
    # this grep goes red the moment lane-launch.ps1 stops building
    # "edda-lane-$Name" — the drift that made task_query unfalsifiable in
    # #967. Zero timing dependence: the line is printed by the same invocation
    # whose exit 0 the assertion above already requires.
    printf '%s' "$out" | command grep -q "dry-run task=$scratch_task " \
        || fail "dry-run receipt does not name the derived task $scratch_task: $out"
    echo "ok 7 non-issue lane unchanged; registered name matches the derivation"

    # case 8 — the PowerShell change parses
    pwsh -NoProfile -Command "\$e=\$null; [void][System.Management.Automation.Language.Parser]::ParseFile('$root/scripts/fleet/lane-launch.ps1', [ref]\$null, [ref]\$e); if (\$e) { \$e | ForEach-Object { Write-Error \$_.Message }; exit 1 }" \
        || fail "lane-launch.ps1 does not parse"
    echo "ok 8 lane-launch.ps1 parses"
elif [ "$on_windows" = 1 ]; then
    echo "SKIP lane-launch gate cases: pwsh not on PATH"
else
    echo "SKIP lane-launch gate cases: not Windows (Get-ScheduledTask is Windows-only)"
fi

echo "PASS: scripts/fleet/test-collision-scan.sh"
