#!/bin/sh
# Offline self-test for scripts/fleet/issue-freshness.sh and the next-issue.sh
# freshness gate (GH-931). Stubs gh and edda (and pwsh, defensively); jq, git
# and sh stay real. Writes only under its temp dir.
#
# Default suite: the gate script itself (exit codes, findings, stale-marking).
# --integration: only the next-issue.sh gate case — the gate must already be
# inserted (delivery step 9.6); running it earlier always fails, because the
# ungated loop dies at the <<AUTHORED STEPS>> marker check instead.
#
# usage:
#   sh scripts/fleet/test-issue-freshness.sh [--integration]
set -eu

cd "$(git rev-parse --show-toplevel)"
freshness=scripts/fleet/issue-freshness.sh
next_issue=scripts/fleet/next-issue.sh

mode=${1:-suite}
case "$mode" in
    suite) run_suite=1; run_integ=0 ;;
    --integration) run_suite=0; run_integ=1 ;;
    *) echo "usage: $0 [--integration]" >&2; exit 2 ;;
esac

sh -n "$freshness" || {
    echo "FAIL: sh -n $freshness" >&2
    exit 1
}
sh -n "$next_issue" || {
    echo "FAIL: sh -n $next_issue" >&2
    exit 1
}
sh -n "$0" || {
    echo "FAIL: sh -n $0" >&2
    exit 1
}

work=$(mktemp -d "${TMPDIR:-/tmp}/test-issue-freshness.XXXXXX")
cleanup() {
    rm -rf "$work"
}
trap cleanup 0 HUP INT TERM
export TMPDIR="$work"

mkdir -p "$work/bin" "$work/fixtures" "$work/scratch" "$work/lanes"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

# ── fixtures ─────────────────────────────────────────────────────────

# Paths referenced here must exist on origin/main: issue-freshness.sh probes
# git cat-file -e against the pinned remote head, not the working tree.
cat >"$work/fixtures/issue-ok.json" <<'JSON'
{"state":"OPEN","title":"feat(fleet): freshness ok","labels":[{"name":"fleet:ready"}],"comments":[],
 "body":"## Predicted surface\n\nGate file `scripts/fleet/next-issue.sh` and helper `scripts/fleet/brief-from-issue.sh` exist.\n\n## doneWhen\n- `edda ask` resolves\n"}
JSON
cat >"$work/fixtures/issue-badflag.json" <<'JSON'
{"state":"OPEN","title":"feat(fleet): freshness bad flag","labels":[{"name":"fleet:ready"}],"comments":[],
 "body":"## Predicted surface\n\nGate file `scripts/fleet/next-issue.sh` exists.\n\n## doneWhen\n- `edda bogusverb-xyz` works\n"}
JSON
cat >"$work/fixtures/issue-stalepr.json" <<'JSON'
{"state":"OPEN","title":"feat(fleet): freshness stale pr","labels":[{"name":"fleet:ready"}],"comments":[],
 "body":"## Predicted surface\n\nGate file `scripts/fleet/next-issue.sh` exists.\n\n## doneWhen\n- `edda ask` resolves\n"}
JSON
cat >"$work/fixtures/issue-nodonewhen.json" <<'JSON'
{"state":"OPEN","title":"feat(fleet): freshness no donewhen","labels":[{"name":"fleet:ready"}],"comments":[],
 "body":"## Predicted surface\n\nGate file `scripts/fleet/next-issue.sh` exists.\n"}
JSON
cat >"$work/fixtures/pr-list-hit.json" <<'JSON'
[{"number":746,"state":"MERGED","title":"x","body":"Closes #993"}]
JSON
cat >"$work/fixtures/pr-list-empty.json" <<'JSON'
[]
JSON
cat >"$work/fixtures/issue-list-empty.json" <<'JSON'
[]
JSON

: >"$work/gh-posted"
: >"$work/gh-edits"
: >"$work/pwsh-calls"

# ── stubs ────────────────────────────────────────────────────────────

cat >"$work/bin/gh" <<'STUB'
#!/bin/sh
jq_expr=
prev=
for a in "$@"; do
    [ "$prev" = "--jq" ] && jq_expr=$a
    prev=$a
done

resp=
case "$1 $2" in
    "issue list") resp=$(cat "$GH_FIXTURES/issue-list-empty.json") ;;
    "pr list")
        case "$*" in
            *993*) resp=$(cat "$GH_FIXTURES/pr-list-hit.json") ;;
            *) resp=$(cat "$GH_FIXTURES/pr-list-empty.json") ;;
        esac ;;
esac
case "$1 $2" in
    "issue view")
        case "$*" in
            *\ 991\ *|*\ 991) resp=$(cat "$GH_FIXTURES/issue-ok.json") ;;
            *\ 992\ *|*\ 992) resp=$(cat "$GH_FIXTURES/issue-badflag.json") ;;
            *\ 993\ *|*\ 993) resp=$(cat "$GH_FIXTURES/issue-stalepr.json") ;;
            *\ 994\ *|*\ 994) resp=$(cat "$GH_FIXTURES/issue-nodonewhen.json") ;;
            *) resp=$(cat "$GH_FIXTURES/issue-ok.json") ;;
        esac ;;
    "issue edit")
        echo "$*" >>"$GH_EDITS"
        echo "edited"
        exit 0 ;;
    "issue comment")
        body=
        prev=
        for a in "$@"; do
            [ "$prev" = "--body" ] && body=$a
            prev=$a
        done
        if [ -z "$body" ]; then
            body=$(cat)
        fi
        { echo "---- comment ----"; printf '%s\n' "$body"; } >>"$GH_POSTED"
        echo "commented"
        exit 0 ;;
    "label create")
        echo "$*" >>"$GH_EDITS"
        echo "created"
        exit 0 ;;
esac
[ -n "$resp" ] || { echo "gh-stub: unexpected: $*" >&2; exit 1; }
if [ -n "$jq_expr" ]; then
    printf '%s' "$resp" | tr -d '\r' | jq -r "$jq_expr"
else
    printf '%s\n' "$resp"
fi
STUB
chmod +x "$work/bin/gh"

cat >"$work/bin/edda" <<'STUB'
#!/bin/sh
case "$*" in
    *bogusverb*) exit 1 ;;
esac
exit 0
STUB
chmod +x "$work/bin/edda"

cat >"$work/bin/pwsh" <<'STUB'
#!/bin/sh
echo "pwsh-stub: $*" >>"$PWSH_CALLS"
exit 0
STUB
chmod +x "$work/bin/pwsh"

run_gated() {
    PATH="$work/bin:$PATH" \
    GH_FIXTURES="$work/fixtures" GH_POSTED="$work/gh-posted" \
    GH_EDITS="$work/gh-edits" PWSH_CALLS="$work/pwsh-calls" \
    "$@"
}

# ── default suite: the gate script itself ────────────────────────────

if [ "$run_suite" -eq 1 ]; then
    # a. valid issue: all checks PASS, exit 0
    run_gated sh "$freshness" 991 >"$work/out-a.txt" 2>"$work/err-a.txt" ||
        fail "issue 991 must pass: rc=$? err=$(cat "$work/err-a.txt")"
    grep -q '^PASS path scripts/fleet/next-issue.sh$' "$work/out-a.txt" ||
        fail "issue 991 misses the path PASS line: $(cat "$work/out-a.txt")"
    grep -q '^PASS path scripts/fleet/brief-from-issue.sh$' "$work/out-a.txt" ||
        fail "issue 991 misses the second path PASS line"
    grep -q '^PASS edda ask$' "$work/out-a.txt" ||
        fail "issue 991 misses the edda PASS line"
    grep -q '^PASS doneWhen$' "$work/out-a.txt" ||
        fail "issue 991 misses the doneWhen PASS line"
    grep -q '^PASS not delivered$' "$work/out-a.txt" ||
        fail "issue 991 misses the delivered PASS line"
    grep -q '^FAIL' "$work/out-a.txt" &&
        fail "issue 991 must print no FAIL lines: $(cat "$work/out-a.txt")"
    echo "ok a valid issue passes"

    # b. nonexistent edda flag: FAIL, fleet:stale label + comment naming it
    run_gated sh "$freshness" 992 >"$work/out-b.txt" 2>"$work/err-b.txt" &&
        fail "issue 992 must exit 1" || rc=$?
    [ "${rc:-0}" -eq 1 ] || fail "issue 992 exit ${rc:-0}, want 1"
    grep -q '^FAIL edda bogusverb-xyz' "$work/out-b.txt" ||
        fail "issue 992 misses the bogusverb FAIL line: $(cat "$work/out-b.txt")"
    grep -q 'fleet:stale' "$work/gh-edits" ||
        fail "issue 992 must add the fleet:stale label: $(cat "$work/gh-edits")"
    grep -q 'bogusverb-xyz' "$work/gh-posted" ||
        fail "issue 992 comment must name the FAIL item: $(cat "$work/gh-posted")"
    echo "ok b bad flag fails and marks stale"

    # c. delivered by a merged PR: FAIL naming the PR
    : >"$work/gh-edits"
    run_gated sh "$freshness" 993 >"$work/out-c.txt" 2>"$work/err-c.txt" &&
        fail "issue 993 must exit 1" || rc=$?
    [ "${rc:-0}" -eq 1 ] || fail "issue 993 exit ${rc:-0}, want 1"
    grep -q '746' "$work/out-c.txt" ||
        fail "issue 993 must name merged PR 746: $(cat "$work/out-c.txt")"
    echo "ok c merged-PR delivery fails"

    # d. missing doneWhen: FAIL
    run_gated sh "$freshness" 994 >"$work/out-d.txt" 2>"$work/err-d.txt" &&
        fail "issue 994 must exit 1" || rc=$?
    [ "${rc:-0}" -eq 1 ] || fail "issue 994 exit ${rc:-0}, want 1"
    grep -q '^FAIL doneWhen' "$work/out-d.txt" ||
        fail "issue 994 misses the doneWhen FAIL line: $(cat "$work/out-d.txt")"
    echo "ok d missing doneWhen fails"

    echo "PASS: scripts/fleet/test-issue-freshness.sh"
    exit 0
fi

# ── integration: the gate sits in next-issue.sh ahead of everything ──

if [ "$run_integ" -eq 1 ]; then
    if ! grep -q 'issue-freshness' "$next_issue"; then
        fail "next-issue.sh carries no freshness gate — insert it (step 9.6) before running --integration"
    fi
    EDDA_FLEET_SCRATCH="$work/scratch" TEMP="$work/lanes" TMPDIR="$work" \
        run_gated sh "$next_issue" 992 docs/worker-9 >"$work/out-e.txt" 2>"$work/err-e.txt" &&
        fail "gated next-issue must exit 2 for 992" || rc=$?
    [ "${rc:-0}" -eq 2 ] || fail "gated next-issue exit ${rc:-0}, want 2"
    grep -qi 'freshness' "$work/err-e.txt" ||
        fail "gated refusal must name freshness: $(cat "$work/err-e.txt")"
    [ -z "$(ls -A "$work/scratch")" ] ||
        fail "gated next-issue wrote into the scratch dir: $(ls -A "$work/scratch")"
    grep -q 'fleet:stale' "$work/gh-edits" ||
        fail "gated refusal must label fleet:stale: $(cat "$work/gh-edits")"
    [ -s "$work/pwsh-calls" ] &&
        fail "gated refusal must not launch: $(cat "$work/pwsh-calls")"
    echo "PASS: scripts/fleet/test-issue-freshness.sh"
    exit 0
fi
