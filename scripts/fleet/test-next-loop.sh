#!/bin/sh
# Offline self-test for scripts/fleet/next-issue.sh and next-review.sh (GH-886).
# Stubs gh and edda (and pwsh, defensively); jq, git and sh stay real.
# Writes only under its temp dir; the one git side effect — worktree
# registration for the review worktree the loop creates — is pruned on exit.
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

mkdir -p "$work/bin" "$work/fixtures" "$work/scratch" "$work/lanes" "$work/wt" "$work/out"
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
cat <<'JSON'
{"outcome":"done","result_text":"## Code Review: Round 2 - PR #886 @ somehead\n\n- model_requested: openrouter/z-ai/glm-5.3-flash\n- model_observed: openrouter/z-ai/glm-5.3-flash\n- spec: review-spec-v1.4\n- class: code-risk\n- cost: placeholder line\n\nbody of the verdict","cost_usd":0.02,"elapsed_ms":1234,"model_requested":"openrouter/z-ai/glm-5.3-flash","model_observed":"openrouter/z-ai/glm-5.3-flash","session_id":"sid","session_observed":"sid","error":null}
JSON
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

git worktree add --detach "$work/wt/review-pr886" "$HEAD1" >/dev/null 2>&1

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

echo "PASS: scripts/fleet/test-next-loop.sh"
