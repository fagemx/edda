#!/bin/sh
# Offline self-test for scripts/fleet/next-issue.sh and next-review.sh (GH-886).
# Stubs gh and edda (and pwsh, defensively); jq, git and sh stay real.
# Writes only under its temp dir; the one git side effect — worktree
# registration for the pre-registered review worktree — is pruned on exit.
set -eu

cd "$(git rev-parse --show-toplevel)"
next_issue=scripts/fleet/next-issue.sh
next_review=scripts/fleet/next-review.sh

sh -n "$next_issue" || {
    echo "FAIL: sh -n $next_issue" >&2
    exit 1
}
sh -n "$next_review" || {
    echo "FAIL: sh -n $next_review" >&2
    exit 1
}
sh -n "$0" || {
    echo "FAIL: sh -n $0" >&2
    exit 1
}

work=$(mktemp -d "${TMPDIR:-/tmp}/test-next-loop.XXXXXX")
cleanup() {
    rm -rf "$work"
    git worktree prune >/dev/null 2>&1 || :
}
trap cleanup 0 HUP INT TERM
export TMPDIR="$work"

mkdir -p "$work/bin" "$work/fixtures" "$work/scratch" "$work/lanes" "$work/out"
export EDDA_FLEET_SCRATCH="$work/scratch"
export TEMP="$work/lanes"
export TMPDIR="$work"

HEAD1=$(git rev-parse origin/main)
HEAD2=$(git rev-parse origin/main~1)
BASE_SHA=$HEAD1

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
cat >"$work/fixtures/pr-full.json" <<JSON
{"headRefOid":"$HEAD1","headRefName":"feat/gh886-controller-loop","baseRefName":"main","baseRefOid":"$BASE_SHA",
 "title":"feat(fleet): controller loop","body":"Issue: #886","state":"OPEN","isDraft":false,"closingIssuesReferences":[]}
JSON
cat >"$work/fixtures/pr-comments-0.json" <<'JSON'
{"comments":[{"body":"first message"},{"body":"second message"}]}
JSON
cat >"$work/fixtures/pr-comments-3.json" <<JSON
{"comments":[
 {"body":"## Code Review: Round 1 — PR #886 @ $HEAD1 (SHADOW)"},
 {"body":"## Review Response: Round 1"},
 {"body":"## Code Review: Round 2 — PR #886 @ $HEAD2 (SHADOW)"},
 {"body":"## Review Response: Round 2"},
 {"body":"## Code Review: Round 3 — PR #886 @ $HEAD1 (SHADOW)"}]}
JSON
printf 'scripts/fleet/next-issue.sh\ndocs/guides/pi-controller-runbook.md\n' >"$work/fixtures/pr-files.txt"

: >"$work/gh-head"
: >"$work/gh-posted"
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
                    *--json\ comments*) resp=$(cat "$GH_COMMENTS_FIXTURE") ;;
                    *) resp=$(cat "$GH_FIXTURES/pr-full.json") ;;
                esac ;;
            diff)
                cat "$GH_FIXTURES/pr-files.txt"
                exit 0 ;;
            comment)
                f=
                prev=
                for a in "$@"; do
                    [ "$prev" = "--body-file" ] && f=$a
                    prev=$a
                done
                { echo "---- comment ----"; cat "$f"; } >>"$GH_POSTED"
                echo "commented"
                exit 0 ;;
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
if [ -n "${EDDA_STUB_MOVE_HEAD:-}" ]; then
    printf '%s' "$EDDA_STUB_MOVE_HEAD" >"$GH_HEAD_FILE"
fi
# R23 (#917): verdict fixtures carry a real §7 heading. EDDA_STUB_RESULT_FILE
# substitutes the whole dispatch envelope (built with jq below).
if [ -n "${EDDA_STUB_RESULT_FILE:-}" ]; then
    cat "$EDDA_STUB_RESULT_FILE"
    exit 0
fi
cat <<'JSON'
{"outcome":"done","result_text":"## Code Review: Round 2 — PR #886 @ aabbccdd11223344556677889900aabbccddeeff\n\n- model_requested: openrouter/z-ai/glm-5.3-flash\n- model_observed: openrouter/z-ai/glm-5.3-flash\n- spec: review-spec-v1.4\n- class: code-risk\n- cost: placeholder line\n\nbody of the verdict","cost_usd":0.02,"elapsed_ms":1234,"model_requested":"openrouter/z-ai/glm-5.3-flash","model_observed":"openrouter/z-ai/glm-5.3-flash","session_id":"sid","session_observed":"sid","error":null}
JSON
exit 0
STUB
chmod +x "$work/bin/edda"

# Verdict fixtures for the R23 publication cases: the same envelope with the
# engine's narration before the heading (#867's transcript-dump shape), and
# one with no heading line at all.
verdict_envelope() { # $1=result text file -> dispatch envelope JSON on stdout
    jq -n --rawfile v "$1" \
        '{outcome:"done",result_text:$v,cost_usd:0.02,elapsed_ms:1234,model_requested:"openrouter/z-ai/glm-5.3-flash",model_observed:"openrouter/z-ai/glm-5.3-flash",session_id:"sid",session_observed:"sid",error:null}'
}
{
    printf 'I will start by loading the project guide.\n'
    printf 'Rerunning with a backslash-free harness.\n'
    for i in 1 2 3 4 5 6 7 8 9 10; do printf 'narration line %s\n' "$i"; done
    printf -- '---\n'
    printf '## Code Review: Round 2 — PR #886 @ aabbccdd11223344556677889900aabbccddeeff\n\n'
    printf -- '- model_requested: openrouter/z-ai/glm-5.3-flash\n- cost: placeholder line\n\nbody of the verdict\n'
} >"$work/fixtures/verdict-narration.txt"
verdict_envelope "$work/fixtures/verdict-narration.txt" >"$work/fixtures/verdict-narration.json"
{
    printf 'I will start by loading the project guide.\n'
    for i in 1 2 3; do printf 'narration line %s\n' "$i"; done
    printf 'no verdict was produced\n'
} >"$work/fixtures/verdict-noheading.txt"
verdict_envelope "$work/fixtures/verdict-noheading.txt" >"$work/fixtures/verdict-noheading.json"

cat >"$work/bin/pwsh" <<'STUB'
#!/bin/sh
echo "pwsh-stub: $*"
exit 0
STUB
chmod +x "$work/bin/pwsh"

run_loop() {
    PATH="$work/bin:$PATH" \
    GH_FIXTURES="$work/fixtures" GH_HEAD_FILE="$work/gh-head" \
    GH_POSTED="$work/gh-posted" GH_EDITS="$work/gh-edits" \
    EDDA_CALLS="$work/edda-calls" GH_COMMENTS_FIXTURE="${GH_COMMENTS_FIXTURE:-$work/fixtures/pr-comments-0.json}" \
    EDDA_STUB_MOVE_HEAD="${EDDA_STUB_MOVE_HEAD:-}" \
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

# ── 4. round cap: three posted rounds, no operator grant ────────────

GH_COMMENTS_FIXTURE="$work/fixtures/pr-comments-3.json" \
    run_loop sh "$next_review" 886 --shadow >"$work/out/cap.txt" 2>"$work/out/cap.err" && \
    fail "round cap must exit 2" || rc=$?
[ "${rc:-0}" -eq 2 ] || fail "round cap exit ${rc:-0}, want 2: $(cat "$work/out/cap.err")"
grep -q 'needs-operator' "$work/out/cap.err" || fail "round cap must name needs-operator: $(cat "$work/out/cap.err")"
grep -q 'needs-operator' "$work/gh-edits" || fail "round cap must add the needs-operator label: $(cat "$work/gh-edits")"
[ -s "$work/gh-posted" ] && fail "round cap must post nothing: $(cat "$work/gh-posted")"
echo "ok 4 round-cap refusal"

# ── 5+6. shadow run: real worktree, moved head refuses; intact posts ─

# Pre-register the review worktree where next-review.sh reads it
# ($EDDA_FLEET_SCRATCH/wt-review-pr886) so the checkout --detach branch runs
# instead of the loop's own worktree add. The marker proves reuse below.
wt_fix="$work/scratch/wt-review-pr886"
git worktree add --detach "$wt_fix" "$HEAD1" >/dev/null 2>&1
: >"$wt_fix/.pre-registered"

printf '%s' "$HEAD1" >"$work/gh-head"
EDDA_STUB_MOVE_HEAD="$HEAD2" \
    run_loop sh "$next_review" 886 --shadow >"$work/out/moved.txt" 2>"$work/out/moved.err" && \
    fail "moved head must exit 2" || rc=$?
[ "${rc:-0}" -eq 2 ] || fail "moved-head exit ${rc:-0}, want 2"
grep -q 'head moved' "$work/out/moved.err" || fail "moved-head refusal must say so: $(cat "$work/out/moved.err")"
[ -s "$work/gh-posted" ] && fail "moved head must post nothing"

printf '%s' "$HEAD1" >"$work/gh-head"
run_loop sh "$next_review" 886 --shadow >"$work/out/shadow.txt" 2>"$work/out/shadow.err" || \
    fail "shadow run exit $? : $(cat "$work/out/shadow.err")"
grep -q 'posted SHADOW round' "$work/out/shadow.txt" || fail "shadow run must post: $(cat "$work/out/shadow.txt")"
posted=$(awk '/^---- comment ----$/{f=1;next} f' "$work/gh-posted")
printf '%s\n' "$posted" | head -1 | grep -q '(SHADOW)$' || fail "shadow heading suffix missing: $(printf '%s\n' "$posted" | head -1)"
printf '%s\n' "$posted" | sed -n '2,3p' | grep -qx 'shadow: true' || fail "shadow body line missing: $(printf '%s\n' "$posted" | head -3)"
printf '%s\n' "$posted" | grep -q 'dispatch --json' || fail "shadow cost line not corrected: $(printf '%s\n' "$posted" | grep '^\\- cost' || true)"
printf '%s\n' "$posted" | grep -q 'placeholder line' && fail "shadow cost line uncorrected"
[ -f "$wt_fix/.pre-registered" ] || fail "pre-registered worktree not reused — the loop recreated it (checkout --detach branch unexercised)"
[ "$(git -C "$wt_fix" rev-parse HEAD)" = "$HEAD1" ] || fail "review worktree not detached at the pinned head: $(git -C "$wt_fix" rev-parse HEAD)"
# Path-drift guard (GH-903): the two checks above inspect only $wt_fix. If the
# fixture registration ever moves away from $EDDA_FLEET_SCRATCH/wt-review-pr886,
# the loop registers its own scratch worktree beside it and both checks still
# pass on the abandoned fixture — the drift is silent. So demand exactly one
# registration under the test root and that it is the fixture itself. git
# reports worktree paths in machine-canonical form while $work may be a
# shell-style path (e.g. MSYS /tmp/...), so canonicalize the root through a
# throwaway repo under $work instead of deriving it from the fixture path.
git init -q "$work/canon-probe"
wt_root=$(git -C "$work/canon-probe" rev-parse --show-toplevel)
wt_root=${wt_root%/canon-probe}
[ -n "$wt_root" ] || fail "cannot canonicalize the test root from $work"
wt_canon=$(git -C "$wt_fix" rev-parse --show-toplevel) || \
    fail "cannot resolve the fixture worktree registration: $wt_fix"
wt_reg=0
wt_drift=
while IFS= read -r wt_path; do
    case $wt_path in
        "$wt_root"/*)
            wt_reg=$((wt_reg + 1))
            [ "$wt_path" = "$wt_canon" ] || wt_drift=$wt_path ;;
    esac
done <<EOF
$(git worktree list --porcelain | sed -n 's/^worktree //p')
EOF
[ "$wt_reg" -eq 1 ] && [ -z "$wt_drift" ] || \
    fail "worktree path drift: want exactly one registration under $wt_root and it must be $wt_canon, found $wt_reg${wt_drift:+ with stray $wt_drift}"
echo "ok 5 moved-head refusal"
echo "ok 6 shadow post shape"

# ── 7. non-shadow delegation refuses while review-pr.sh has no pi arm ─

: >"$work/gh-posted"
printf '%s' "$HEAD1" >"$work/gh-head"
run_loop sh "$next_review" 886 >"$work/out/delegate.txt" 2>"$work/out/delegate.err" && \
    fail "non-shadow delegation must exit 2 while the pi arm is absent" || rc=$?
[ "${rc:-0}" -eq 2 ] || fail "non-shadow refusal exit ${rc:-0}, want 2: $(cat "$work/out/delegate.err")"
grep -q 'no pi arm yet' "$work/out/delegate.err" || fail "non-shadow refusal must name the missing arm: $(cat "$work/out/delegate.err")"
[ -s "$work/gh-posted" ] && fail "non-shadow refusal must post nothing"
echo "ok 7 non-shadow delegation refusal"

# ── 8. machine identity shape: a crafted identity never reaches sh -c ─

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
echo "ok 8 machine identity shape refusal"

# ── 9. R23 publication: narration before the heading is trimmed ─────

: >"$work/gh-posted"
printf '%s' "$HEAD1" >"$work/gh-head"
EDDA_STUB_RESULT_FILE="$work/fixtures/verdict-narration.json" \
    run_loop sh "$next_review" 886 --shadow >"$work/out/trim.txt" 2>"$work/out/trim.err" || \
    fail "narration trim run exit $? : $(cat "$work/out/trim.err")"
grep -q 'posted SHADOW round' "$work/out/trim.txt" || fail "trim run must post: $(cat "$work/out/trim.txt")"
posted=$(awk '/^---- comment ----$/{f=1;next} f' "$work/gh-posted")
printf '%s\n' "$posted" | head -1 | grep -q '^## Code Review: Round' || \
    fail "posted body must begin with the heading: $(printf '%s\n' "$posted" | head -1)"
printf '%s\n' "$posted" | grep -q 'narration line' && fail "narration leaked into the posted verdict"
printf '%s\n' "$posted" | grep -qF 'I will start by loading' && fail "engine preamble leaked into the posted verdict"
echo "ok 9 R23 publication trims narration"

# ── 10. R23 publication: a verdict with no heading posts nothing ────

: >"$work/gh-posted"
printf '%s' "$HEAD1" >"$work/gh-head"
EDDA_STUB_RESULT_FILE="$work/fixtures/verdict-noheading.json" \
    run_loop sh "$next_review" 886 --shadow >"$work/out/nohead.txt" 2>"$work/out/nohead.err" && \
    fail "headingless verdict must exit non-zero" || rc=$?
[ "${rc:-0}" -eq 2 ] || fail "headingless verdict exit ${rc:-0}, want 2: $(cat "$work/out/nohead.err")"
grep -q 'PR #886' "$work/out/nohead.err" || fail "refusal must name the PR: $(cat "$work/out/nohead.err")"
[ -s "$work/gh-posted" ] && fail "headingless verdict must post nothing: $(cat "$work/gh-posted")"
echo "ok 10 R23 publication refuses a headingless verdict"

echo "PASS: scripts/fleet/test-next-loop.sh"
