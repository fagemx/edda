#!/bin/sh
# Offline self-test for scripts/fleet/next-issue.sh (GH-886).
# Stubs gh and edda (and pwsh, defensively); jq, git and sh stay real.
# Writes only under its temp dir and makes no git side effects.
#
# It covered the review shell's queue helper too until GH-1061 retired that
# shell; the cases driving it went with it, and the fixtures only they used
# (the PR envelope, the round-count comment lists, the verdict envelopes) went
# with those.
set -eu

cd "$(git rev-parse --show-toplevel)"
next_issue=scripts/fleet/next-issue.sh

sh -n "$next_issue" || {
    echo "FAIL: sh -n $next_issue" >&2
    exit 1
}
sh -n "$0" || {
    echo "FAIL: sh -n $0" >&2
    exit 1
}

work=$(mktemp -d "${TMPDIR:-/tmp}/test-next-loop.XXXXXX")
cleanup() {
    rm -rf "$work"
}
trap cleanup 0 HUP INT TERM
export TMPDIR="$work"

mkdir -p "$work/bin" "$work/fixtures" "$work/scratch" "$work/lanes" "$work/out"
export EDDA_FLEET_SCRATCH="$work/scratch"
export TEMP="$work/lanes"
export TMPDIR="$work"

# The fixture head only needs one 40-hex id. The CI fleet checkout is
# single-ref depth-1, so origin/main is not guaranteed to exist (GH-896 round
# 2: `git rev-parse origin/main` died there); fall back to the checkout's own
# HEAD.
if git rev-parse -q --verify origin/main >/dev/null 2>&1; then
    HEAD1=$(git rev-parse origin/main)
else
    HEAD1=$(git rev-parse HEAD)
fi

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

# ── fixtures ─────────────────────────────────────────────────────────

cat >"$work/fixtures/issue-list.json" <<'JSON'
[{"number":886,"title":"feat(fleet): controller loop","createdAt":"2026-09-05T00:00:00Z"}]
JSON
cat >"$work/fixtures/pr-list.json" <<'JSON'
[]
JSON
cat >"$work/fixtures/issue-ready.json" <<JSON
{"state":"OPEN","title":"feat(fleet): controller loop","labels":[{"name":"fleet:ready"}],"comments":[],
 "body":"## Predicted surface\n\n\`scripts/fleet/next-issue.sh\`, \`docs/guides/pi-controller-runbook.md\`. No crate.\n\n## doneWhen\n- item\n"}
JSON
cat >"$work/fixtures/issue-claimed.json" <<'JSON'
{"state":"OPEN","title":"feat(fleet): controller loop","labels":[{"name":"fleet:claimed"}],
 "comments":[{"body":"taking: docs/worker-1 at 2026-09-05T00:00:00Z"}],
 "body":"## Predicted surface\n\n`scripts/fleet/next-issue.sh`. No crate.\n"}
JSON
: >"$work/gh-head"
: >"$work/gh-edits"
: >"$work/edda-calls"

# ── stubs ────────────────────────────────────────────────────────────

cat >"$work/bin/gh" <<'STUB'
#!/bin/sh
# resolve --jq <expr> like the real gh does
jq_expr=
prev=
for a in "$@"; do
    [ "$prev" = "--jq" ] && jq_expr=$a
    prev=$a
done

resp=
case "$1 $2" in
    "issue list") resp=$(cat "$GH_FIXTURES/issue-list.json") ;;
    "pr list") resp=$(cat "$GH_FIXTURES/pr-list.json") ;;
esac
case "$1 $2" in
    "issue view")
        case "$*" in
            *\ 885\ *|*\ 885) resp=$(cat "$GH_FIXTURES/issue-claimed.json") ;;
            *) resp=$(cat "$GH_FIXTURES/issue-ready.json") ;;
        esac ;;
    "issue edit")
        echo "$*" >>"$GH_EDITS"
        echo "edited"
        exit 0 ;;
esac
case "$1" in
    pr)
        case "$2" in
            view)
                case "$*" in
                    *--json\ headRefOid*) resp="{\"headRefOid\":\"$(cat "$GH_HEAD_FILE")\"}" ;;
                esac ;;
        esac ;;
esac
[ -n "$resp" ] || { echo "gh-stub: unexpected: $*" >&2; exit 1; }
if [ -n "$jq_expr" ]; then
    printf '%s' "$resp" | jq -r "$jq_expr"
else
    printf '%s\n' "$resp"
fi
STUB
chmod +x "$work/bin/gh"

cat >"$work/bin/edda" <<'STUB'
#!/bin/sh
echo "$*" >>"$EDDA_CALLS"
exit 0
STUB
chmod +x "$work/bin/edda"

cat >"$work/bin/pwsh" <<'STUB'
#!/bin/sh
echo "pwsh-stub: $*"
exit 0
STUB
chmod +x "$work/bin/pwsh"

run_loop() {
    PATH="$work/bin:$PATH" \
    GH_FIXTURES="$work/fixtures" GH_HEAD_FILE="$work/gh-head" \
    GH_EDITS="$work/gh-edits" \
    EDDA_CALLS="$work/edda-calls" \
    "$@"
}

# ── 1. ready issue dry-run: full preview, zero side effects ─────────

printf '%s' "$HEAD1" >"$work/gh-head"
run_loop sh "$next_issue" 886 docs/worker-1 --dry-run >"$work/out/dry.txt" 2>"$work/out/dry.err"
[ -s "$work/out/dry.err" ] && fail "dry-run wrote to stderr: $(cat "$work/out/dry.err")"
grep -q '^== ready-queue lint' "$work/out/dry.txt" || fail "dry-run output misses the lint result"
grep -q '^branch: ' "$work/out/dry.txt" || fail "dry-run output misses the branch line"
grep -q '^worktree: ' "$work/out/dry.txt" || fail "dry-run output misses the worktree line"
grep -q '^cmd: edda task new ' "$work/out/dry.txt" || fail "dry-run output misses the task-new line"
grep -q '^brief path: ' "$work/out/dry.txt" || fail "dry-run output misses the brief path"
grep -q 'fleet-claim-issue.sh 886 docs/worker-1' "$work/out/dry.txt" || fail "dry-run output misses the claim command"
grep -q 'lane-launch.ps1 -Name edda-lane-gh886 ' "$work/out/dry.txt" || fail "dry-run output misses the launch command"
# GH-936: the launch line must carry the brief's scope paths as -Owns.
# Without it lane-launch.ps1's write-lane guard refuses the launch AFTER the
# worktree, task and claim already exist, which is where every next-issue.sh
# run died on current main. GH-937 fixes the shape: ONE comma-separated
# argument, because `pwsh -File` binds only the first value of a multi-token
# option and lets the rest bind to whatever named parameter is still free.
grep -qF -- '-Owns "scripts/fleet/next-issue.sh,docs/guides/pi-controller-runbook.md"' "$work/out/dry.txt" ||
    fail "launch command misses the -Owns scope list :: $(grep 'lane-launch.ps1' "$work/out/dry.txt" || true)"
# -Owns stays the final option, so a future trailing addition cannot be read
# as one of its scopes by a human copying the line.
grep -q 'lane-launch.ps1 .*-Owns "[^"]*"$' "$work/out/dry.txt" ||
    fail "-Owns is not the final option on the launch line :: $(grep 'lane-launch.ps1' "$work/out/dry.txt" || true)"
grep -q 'nothing created, claimed, or launched' "$work/out/dry.txt" || fail "dry-run output misses the closing line"
grep -qF -- '--path "scripts/fleet/next-issue.sh"' "$work/out/dry.txt" || fail "task-new line misses the first scope path"
grep -qF -- '--path "docs/guides/pi-controller-runbook.md"' "$work/out/dry.txt" || fail "task-new line misses the second scope path"
if grep -q 'task new .*·' "$work/out/dry.txt"; then fail "task-new line carries a stray middle-dot"
fi
# order: lint < branch < task new < brief < claim < launch
lint_n=$(grep -n '^== ready-queue lint' "$work/out/dry.txt" | cut -d: -f1)
branch_n=$(grep -n '^branch: ' "$work/out/dry.txt" | cut -d: -f1)
task_n=$(grep -n '^cmd: edda task new ' "$work/out/dry.txt" | cut -d: -f1)
brief_n=$(grep -n '^brief path: ' "$work/out/dry.txt" | cut -d: -f1)
claim_n=$(grep -n 'fleet-claim-issue.sh 886' "$work/out/dry.txt" | cut -d: -f1 | head -1)
launch_n=$(grep -n 'lane-launch.ps1 -Name' "$work/out/dry.txt" | cut -d: -f1 | head -1)
[ "$lint_n" -lt "$branch_n" ] && [ "$branch_n" -lt "$task_n" ] \
    && [ "$task_n" -lt "$brief_n" ] && [ "$brief_n" -lt "$claim_n" ] \
    && [ "$claim_n" -lt "$launch_n" ] || fail "dry-run output out of order: lint=$lint_n branch=$branch_n task=$task_n brief=$brief_n claim=$claim_n launch=$launch_n"
[ -z "$(ls -A "$work/scratch")" ] || fail "dry-run wrote into the scratch dir: $(ls -A "$work/scratch")"
[ -f "$work/edda-calls" ] && [ -s "$work/edda-calls" ] && fail "dry-run invoked edda"
echo "ok 1 ready issue dry-run"

# ── 2. claimed issue refusal names the claimant ─────────────────────

run_loop sh "$next_issue" 885 docs/worker-1 --dry-run >"$work/out/claimed.txt" 2>"$work/out/claimed.err" && \
    fail "claimed issue must exit 2" || rc=$?
[ "${rc:-0}" -eq 2 ] || fail "claimed refusal exit ${rc:-0}, want 2"
grep -q 'docs/worker-1' "$work/out/claimed.err" || fail "claimed refusal must name the claimant: $(cat "$work/out/claimed.err")"
echo "ok 2 claimed issue refusal"

# ── 3. marker-left-in-brief refusal (real run, pre-launch) ──────────

run_loop sh "$next_issue" 886 docs/worker-1 >"$work/out/marker.txt" 2>"$work/out/marker.err" && \
    fail "marker-left brief must exit 2" || rc=$?
[ "${rc:-0}" -eq 2 ] || fail "marker refusal exit ${rc:-0}, want 2"
grep -q 'AUTHORED STEPS' "$work/out/marker.err" || fail "marker refusal must point at the brief: $(cat "$work/out/marker.err")"
grep -q '^<<AUTHORED STEPS>>$' "$work/scratch/brief-gh886.md" || fail "rendered brief must keep the marker"
[ -s "$work/edda-calls" ] && fail "marker refusal must not run task new: $(cat "$work/edda-calls")"
[ -s "$work/gh-edits" ] && fail "marker refusal must not touch labels: $(cat "$work/gh-edits")"
echo "ok 3 marker-left-in-brief refusal"

# ── 4. machine identity shape: a crafted identity never reaches sh -c ─

printf '%s' "$HEAD1" >"$work/gh-head"
for crafted in 'docs/worker-1" --evil "x' 'docs/'; do
    : >"$work/edda-calls"
    : >"$work/gh-edits"
    run_loop sh "$next_issue" 886 "$crafted" >"$work/out/identity.txt" 2>"$work/out/identity.err" && \
        fail "crafted identity '$crafted' must exit 2" || rc=$?
    [ "${rc:-0}" -eq 2 ] || fail "identity refusal exit ${rc:-0}, want 2 for '$crafted'"
    grep -q 'machine identity' "$work/out/identity.err" || \
        fail "crafted identity '$crafted' must die at validation: $(cat "$work/out/identity.err")"
    [ -s "$work/edda-calls" ] && fail "crafted identity '$crafted' reached task new: $(cat "$work/edda-calls")"
    [ -s "$work/gh-edits" ] && fail "crafted identity '$crafted' touched labels: $(cat "$work/gh-edits")"
done
echo "ok 4 machine identity shape refusal"

echo "PASS: scripts/fleet/test-next-loop.sh"
