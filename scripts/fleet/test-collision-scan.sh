#!/bin/sh
# Offline self-test for scripts/fleet/collision-scan.sh and the lane-launch
# claim gate (GH-912).
#
# collision-scan cases stub gh with fixture JSON + real jq (the script's --jq
# filters run for real). The lane-launch gate cases run the real launcher
# under pwsh with a fake `gh` on PATH: a foreign claim must refuse BEFORE any
# registration (Get-ScheduledTask returns nothing), the launcher's own claim
# and the no-claim dry run behave exactly as before. Everything is written
# under one temp dir; nothing outside it is touched. No script under test
# ever comments, labels, closes, or merges.
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
cleanup() { rm -r -f "$work"; }
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

if command -v pwsh >/dev/null 2>&1; then
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
    task_query() {
        pwsh -NoProfile -Command "if (Get-ScheduledTask -TaskName 'edda-lane-gh9' -ErrorAction SilentlyContinue) { exit 1 } else { exit 0 }"
    }

    # case 4 — foreign claim refuses before registering anything
    : >"$work/gate-fixture.json"
    rc=0
    out=$(GF="$gf" GF_MODE=foreign PATH="$work/bin-gate:$PATH" \
        pwsh -NoProfile -File "$launcher" -Name edda-lane-gh9 \
        -Brief "$work/gate-fixture.json" -Cwd "$work/cwd" \
        -Machine docs/other -LogDir "$work/lanes" 2>&1) || rc=$?
    [ "$rc" != "0" ] || fail "foreign claim must refuse, exit 0: $out"
    printf '%s' "$out" | command grep -q 'claim guard refused' || fail "refusal must name the guard: $out"
    printf '%s' "$out" | command grep -q '4090/worker-3' || fail "refusal must name the claimant: $out"
    task_query || fail "a refused launch must not register the scheduled task"
    [ ! -f "$work/lanes/edda-lane-gh9.log" ] || fail "a refused launch must not create the lane log"
    echo "ok 4 foreign claim refuses before registering"

    # case 5 — the launcher's own claim proceeds; -DryRun receipt unchanged
    rc=0
    out=$(GF="$gf" GF_MODE=self PATH="$work/bin-gate:$PATH" \
        pwsh -NoProfile -File "$launcher" -Name edda-lane-gh9 \
        -Brief "$work/gate-fixture.json" -Cwd "$work/cwd" \
        -Machine docs/self -LogDir "$work/lanes" -DryRun \
        -Owns scripts/fleet/test-collision-scan.sh 2>&1) || rc=$?
    [ "$rc" = "0" ] || fail "own-claim dry run exit $rc, want 0: $out"
    printf '%s' "$out" | command grep -qi 'dry' || fail "dry-run receipt lines missing: $out"
    [ -f "$work/lanes/edda-lane-gh9.dryrun.done" ] || fail "dry-run done receipt missing: $out"
    task_query || fail "a dry run must not register the scheduled task"
    echo "ok 5 own claim proceeds, dry-run receipt intact"

    # case 6 — issue-shaped lane without -Machine refuses
    rc=0
    out=$(GF="$gf" GF_MODE=self PATH="$work/bin-gate:$PATH" \
        pwsh -NoProfile -File "$launcher" -Name edda-lane-gh9 \
        -Brief "$work/gate-fixture.json" -Cwd "$work/cwd" \
        -LogDir "$work/lanes" -DryRun 2>&1) || rc=$?
    [ "$rc" != "0" ] || fail "missing -Machine must refuse, exit 0: $out"
    printf '%s' "$out" | command grep -q 'R9/R21' || fail "refusal must name R9/R21: $out"
    echo "ok 6 missing -Machine refuses"

    # case 7 — non-issue-shaped lane names are unchanged (no -Machine needed)
    # The dry run races the Task Scheduler's own state query; retry a couple
    # of times before declaring the receipt broken.
    rc=1
    attempt=0
    while [ "$rc" != "0" ] && [ "$attempt" -lt 3 ]; do
        attempt=$((attempt + 1))
        rc=0
        out=$(PATH="$work/bin-gate:$PATH" \
            pwsh -NoProfile -File "$launcher" -Name scratch-lane \
            -Brief "$work/gate-fixture.json" -Cwd "$work/cwd" \
            -LogDir "$work/lanes" -DryRun \
            -Owns scripts/fleet/test-collision-scan.sh 2>&1) || rc=$?
        [ "$rc" = "0" ] || sleep 2
    done
    [ "$rc" = "0" ] || fail "non-issue lane dry run exit $rc after retries: $out"
    echo "ok 7 non-issue-shaped lane unchanged"

    # case 8 — the PowerShell change parses
    pwsh -NoProfile -Command "\$e=\$null; [void][System.Management.Automation.Language.Parser]::ParseFile('$root/scripts/fleet/lane-launch.ps1', [ref]\$null, [ref]\$e); if (\$e) { \$e | ForEach-Object { Write-Error \$_.Message }; exit 1 }" \
        || fail "lane-launch.ps1 does not parse"
    echo "ok 8 lane-launch.ps1 parses"
else
    echo "SKIP pwsh cases: pwsh not on PATH"
fi

echo "PASS: scripts/fleet/test-collision-scan.sh"
