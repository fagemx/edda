#!/bin/sh
# Offline self-test for scripts/fleet/brief-from-issue.sh (GH-885).
# Stubs gh, uname, and git. Writes only under a temp dir.
set -eu

cd "$(git rev-parse --show-toplevel)"
script=scripts/fleet/brief-from-issue.sh

sh -n "$script" || {
    echo "FAIL: sh -n $script" >&2
    exit 1
}
sh -n "$0" || {
    echo "FAIL: sh -n $0" >&2
    exit 1
}

work=$(mktemp -d "${TMPDIR:-/tmp}/test-brief-from-issue.XXXXXX")
trap 'rm -rf "$work"' 0 HUP INT TERM
export TMPDIR="$work"

mkdir -p "$work/bin" "$work/out"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

cat >"$work/bin/gh" <<'STUB'
#!/bin/sh
cat "$GH_ISSUE_JSON"
STUB
chmod +x "$work/bin/gh"

cat >"$work/bin/git" <<'STUB'
#!/bin/sh
# Consume a leading -C <dir> the way git(1) does.
while [ $# -gt 0 ]; do
    case "$1" in
        -C) shift 2 ;;
        *) break ;;
    esac
done
case "$*" in
    "rev-parse origin/main")
        echo "6486228c728dff5d488b5756e918af0bccde0eb5"
        exit 0
        ;;
esac
echo "git-stub: unexpected: $*" >&2
exit 1
STUB
chmod +x "$work/bin/git"

cat >"$work/bin/uname" <<'STUB'
#!/bin/sh
echo "$TEST_UNAME"
STUB
chmod +x "$work/bin/uname"

# Fixture: Predicted surface matches issue #880's shipped list.
cat >"$work/issue-880.json" <<'JSON'
{"title":"fix(fleet): review-pr.sh has no pi arm","labels":[{"name":"fleet:ready"}],"body":"## What happened\nBasis.\n\n## Predicted surface\n\n`scripts/review-pr.sh`, `scripts/reviewer-capabilities.sh`, `scripts/fleet/reviewer-capabilities.ps1`, `scripts/test-review-capabilities.sh`, `scripts/pr-review-launch.ps1`, `scripts/pr-review-watch.sh`. No crate.\n\n## doneWhen\n- item\n"}
JSON

cat >"$work/issue-missing.json" <<'JSON'
{"title":"no surface","labels":[],"body":"## What happened\nNo predicted heading here.\n\n## doneWhen\n- item\n"}
JSON

cat >"$work/issue-empty.json" <<'JSON'
{"title":"empty surface","labels":[],"body":"## What happened\n.\n\n## Predicted surface\n\nNo crate.\n\n## doneWhen\n- item\n"}
JSON

# Negated mentions: the real GH-880 surface ends with "No crate, no `REVIEW.md`,
# no `Cargo.lock`." — those two tokens are named only to be excluded.
cat >"$work/issue-negated.json" <<'JSON'
{"title":"negated mentions","labels":[],"body":"## Predicted surface\n\n`scripts/review-pr.sh`, `scripts/pr-review-watch.sh`. No crate, no `REVIEW.md`, no `Cargo.lock`.\n\n## doneWhen\n- item\n"}
JSON

run_ok() {
    uname_s=$1
    out=$2
    TEST_UNAME=$uname_s GH_ISSUE_JSON="$work/issue-880.json" \
        PATH="$work/bin:$PATH" \
        sh "$script" 880 \
            --lane-name edda-lane-gh880 \
            --worktree C:/ai_agent/edda-wt-gh880 \
            --branch fix/gh880-pi-review-arm \
            --task-id 128 >"$out"
}

assert_facts() {
    out=$1
    grep -q 'role: worker' "$out" || fail "missing role: $(cat "$out")"
    grep -q 'lane: none' "$out" || fail "missing lane none: $(cat "$out")"
    grep -q 'task id: 128' "$out" || fail "missing task id: $(cat "$out")"
    grep -q 'issue: #880' "$out" || fail "missing issue: $(cat "$out")"
    grep -q 'base full SHA: 6486228c728dff5d488b5756e918af0bccde0eb5' "$out" \
        || fail "missing base SHA: $(cat "$out")"
    grep -q 'scope paths: scripts/review-pr.sh, scripts/reviewer-capabilities.sh, scripts/fleet/reviewer-capabilities.ps1, scripts/test-review-capabilities.sh, scripts/pr-review-launch.ps1, scripts/pr-review-watch.sh' "$out" \
        || fail "missing scope paths: $(cat "$out")"
    grep -q 'entry: none: procedure below' "$out" || fail "missing entry"
    grep -q 'gate owner: not you; review queue' "$out" || fail "missing gate owner"
    grep -q 'out-of-scope: every path and concern not listed in scope paths' "$out" \
        || fail "missing out-of-scope"
}

assert_skeleton() {
    out=$1
    grep -q 'Launcher contract:' "$out" || fail "missing launcher contract"
    grep -q 'C:/ai_agent/edda-wt-gh880' "$out" || fail "missing worktree"
    grep -q 'fix/gh880-pi-review-arm' "$out" || fail "missing branch"
    grep -q 'edda-lane-gh880' "$out" || fail "missing lane name"
    grep -q 'Failure rule for every numbered step: a nonzero command exit, tool error,' "$out" \
        || fail "missing failure rule"
    grep -q 'STOP step=<number> output=<verbatim unexpected output>.' "$out" \
        || fail "missing STOP sentence"
    grep -q '1. gh issue view 880' "$out" || fail "missing preamble step 1"
    grep -q '9. git ls-files --' "$out" || fail "missing preamble step 9"
    c=$(grep -c '<<AUTHORED STEPS>>' "$out" || true)
    [ "$c" -eq 1 ] || fail "marker count $c, want 1"
    grep -q 'scripts/review-pr.sh' "$out" || fail "git status finish missing a scope path"
    grep -q 'DONE issue=#880 task=128' "$out" || fail "missing five-line report"
    grep -q 'stop=controller issues the next brief' "$out" || fail "missing report last line"
    grep -q '20. git rev-parse --git-path gh880-pr-body.md' "$out" \
        || fail "missing pr-body path resolution step"
    grep -q 'write({"path":"<pr_body_path>"' "$out" \
        || fail "write step must target the retained git-dir path"
    grep -qF -- '--paths "scripts/review-pr.sh"' "$out" \
        || fail "claim step paths must be quoted"
    grep -qF 'git ls-files -- "scripts/review-pr.sh"' "$out" \
        || fail "ls-files step paths must be quoted"
    grep -qF 'git add -- "scripts/review-pr.sh"' "$out" \
        || fail "git add step paths must be quoted"
    if grep -qF '.git/gh880-pr-body.md' "$out"; then
        fail "hardcoded .git/ pr-body path still present (linked worktree .git is a file)"
    fi
    grep -qF 'body-file "$(git rev-parse --git-path gh880-pr-body.md)"' "$out" \
        || fail "step 22 must carry the literal git-path expansion for the lane"
    if grep -Ei 'as needed|if relevant|use (your )?judg|where appropriate' "$out"; then
        fail "discretionary phrasing in output"
    fi
}

# 1. Windows host
rc=0
run_ok 'MINGW64_NT-10.0' "$work/out/win.txt" || rc=$?
[ "$rc" -eq 0 ] || fail "windows render exit $rc"
assert_facts "$work/out/win.txt"
assert_skeleton "$work/out/win.txt"
grep -q 'Host: MINGW/MSYS. review-pr.sh --dry-run generates -lane.ps1 and no -run.sh.' \
    "$work/out/win.txt" || fail "windows host fact: $(cat "$work/out/win.txt")"
echo "ok 1 windows host facts and skeleton"

# 2. Linux host
rc=0
run_ok 'Linux' "$work/out/linux.txt" || rc=$?
[ "$rc" -eq 0 ] || fail "linux render exit $rc"
assert_facts "$work/out/linux.txt"
assert_skeleton "$work/out/linux.txt"
grep -q 'Host: Linux. review-pr.sh --dry-run generates -run.sh and no -lane.ps1.' \
    "$work/out/linux.txt" || fail "linux host fact: $(cat "$work/out/linux.txt")"
echo "ok 2 linux host facts and skeleton"

# 3. missing Predicted surface section → exit 2, names the section
rc=0
TEST_UNAME=Linux GH_ISSUE_JSON="$work/issue-missing.json" \
    PATH="$work/bin:$PATH" \
    sh "$script" 1 --lane-name n --worktree /tmp/wt --branch b \
    >"$work/out/missing.txt" 2>"$work/out/missing.err" || rc=$?
[ "$rc" -eq 2 ] || fail "missing section exit $rc, want 2"
grep -q 'Predicted surface' "$work/out/missing.err" \
    || fail "missing-section stderr must name the section: $(cat "$work/out/missing.err")"
echo "ok 3 missing Predicted surface exits 2"

# 4. empty Predicted surface (no paths) → exit 2, names the section
rc=0
TEST_UNAME=Linux GH_ISSUE_JSON="$work/issue-empty.json" \
    PATH="$work/bin:$PATH" \
    sh "$script" 1 --lane-name n --worktree /tmp/wt --branch b \
    >"$work/out/empty.txt" 2>"$work/out/empty.err" || rc=$?
[ "$rc" -eq 2 ] || fail "empty section exit $rc, want 2"
grep -q 'Predicted surface' "$work/out/empty.err" \
    || fail "empty-section stderr must name the section: $(cat "$work/out/empty.err")"
echo "ok 4 empty Predicted surface exits 2"

# 5. --build-lane fills the lane field
rc=0
TEST_UNAME=Linux GH_ISSUE_JSON="$work/issue-880.json" \
    PATH="$work/bin:$PATH" \
    sh "$script" 880 --lane-name edda-lane-gh880 \
        --worktree C:/ai_agent/edda-wt-gh880 \
        --branch fix/gh880-pi-review-arm \
        --task-id 128 --build-lane worker-1 \
    >"$work/out/lane.txt" || rc=$?
[ "$rc" -eq 0 ] || fail "build-lane render exit $rc"
grep -q 'lane: worker-1' "$work/out/lane.txt" || fail "build-lane not applied"
grep -q 'lane: none' "$work/out/lane.txt" && fail "lane none still present with --build-lane"
echo "ok 5 --build-lane sets lane field"

# 6. negated mentions are not scope paths
rc=0
TEST_UNAME=Linux GH_ISSUE_JSON="$work/issue-negated.json" \
    PATH="$work/bin:$PATH" \
    sh "$script" 880 --lane-name n --worktree /tmp/wt --branch b \
    >"$work/out/negated.txt" || rc=$?
[ "$rc" -eq 0 ] || fail "negated render exit $rc"
grep -q 'scope paths: scripts/review-pr.sh, scripts/pr-review-watch.sh' "$work/out/negated.txt" \
    || fail "negated scope paths wrong: $(grep 'scope paths' "$work/out/negated.txt")"
if grep -E 'scope paths:.*REVIEW\.md|scope paths:.*Cargo\.lock' "$work/out/negated.txt"; then
    fail "negated mention leaked into scope paths"
fi
echo "ok 6 negated mentions excluded from scope paths"

# 7. shell-metacharacter stripping from the title: backtick and dollar must not
# survive into the commit step, where the lane's shell would expand them.
cat >"$work/issue-metachar.json" <<'JSON'
{"title":"fix: broke `x` and $(rm -rf /) handling","labels":[],"body":"## Predicted surface\n\n`scripts/review-pr.sh`.\n\n## doneWhen\n- item\n"}
JSON
rc=0
TEST_UNAME=Linux GH_ISSUE_JSON="$work/issue-metachar.json" \
    PATH="$work/bin:$PATH" \
    sh "$script" 880 --lane-name n --worktree /tmp/wt --branch b \
    >"$work/out/metachar.txt" || rc=$?
[ "$rc" -eq 0 ] || fail "metachar render exit $rc"
if grep -E '\$\(rm|`x`' "$work/out/metachar.txt"; then
    fail "shell metacharacter survived into the brief"
fi
grep -q 'git commit -m "fix: broke x and (rm -rf /) handling"' "$work/out/metachar.txt" \
    || fail "stripped title not in commit step: $(grep 'git commit -m' "$work/out/metachar.txt")"
echo "ok 7 title metacharacters stripped"

# 8. a crafted Predicted-surface token is rejected, not rendered (R5)
cat >"$work/issue-crafted.json" <<'JSON'
{"title":"crafted","labels":[],"body":"## Predicted surface\n\n`scripts/review-pr.sh`, `docs/a; rm -rf ~`.\n\n## doneWhen\n- item\n"}
JSON
rc=0
TEST_UNAME=Linux GH_ISSUE_JSON="$work/issue-crafted.json" \
    PATH="$work/bin:$PATH" \
    sh "$script" 1 --lane-name n --worktree /tmp/wt --branch b \
    >"$work/out/crafted.txt" 2>"$work/out/crafted.err" || rc=$?
[ "$rc" -eq 2 ] || fail "crafted token exit $rc, want 2"
grep -q 'rm -rf' "$work/out/crafted.err" \
    || fail "crafted-token stderr must name the rejected token: $(cat "$work/out/crafted.err")"
grep -q 'rm -rf' "$work/out/crafted.txt" \
    && fail "crafted token leaked into the rendered brief"
echo "ok 8 crafted scope token rejected"

# 9. omitted --task-id renders without the task-rail steps (GH-898): the
#    fixed steps 6 and 24 must never name task none, and both invocation
#    modes are proven.
rc=0
TEST_UNAME=Linux GH_ISSUE_JSON="$work/issue-880.json" \
    PATH="$work/bin:$PATH" \
    sh "$script" 880 --lane-name edda-lane-gh880 \
        --worktree C:/ai_agent/edda-wt-gh880 \
        --branch fix/gh880-pi-review-arm \
    >"$work/out/notask.txt" || rc=$?
[ "$rc" -eq 0 ] || fail "no-task-id render exit $rc"
if grep -q 'edda task show none' "$work/out/notask.txt"; then
    fail "step 6 renders edda task show none — the lane stops there (GH-898)"
fi
if grep -q 'task done none' "$work/out/notask.txt"; then
    fail "step 24 renders edda task done none — the receipt cannot succeed (GH-898)"
fi
if grep -q 'including task none' "$work/out/notask.txt"; then
    fail "step 5 expects task none on the rail (GH-898)"
fi
grep -q '6. git rev-parse --is-inside-work-tree' "$work/out/notask.txt" \
    || fail "rail-free step 6 missing"
grep -q '24. edda note ' "$work/out/notask.txt" \
    || fail "rail-free step 24 missing"
grep -q 'DONE issue=#880 task=none' "$work/out/notask.txt" \
    || fail "task=none report line missing"
rc=0
TEST_UNAME=Linux GH_ISSUE_JSON="$work/issue-880.json" \
    PATH="$work/bin:$PATH" \
    sh "$script" 880 --lane-name edda-lane-gh880 \
        --worktree C:/ai_agent/edda-wt-gh880 \
        --branch fix/gh880-pi-review-arm \
        --task-id 128 \
    >"$work/out/withtask.txt" || rc=$?
[ "$rc" -eq 0 ] || fail "with-task-id render exit $rc"
grep -q '6. edda task show 128' "$work/out/withtask.txt" \
    || fail "rail step 6 missing with --task-id"
grep -q '24. edda task done 128' "$work/out/withtask.txt" \
    || fail "rail step 24 missing with --task-id"
echo "ok 9 omitted --task-id renders rail-free steps (both modes)"

# 10. the rendered contract pins the one sanctioned prep command (GH-897):
#     step 2 keeps the tracking-suffix expectation and the launcher contract
#     names the refname form that produces it.
rc=0
TEST_UNAME=Linux GH_ISSUE_JSON="$work/issue-880.json" \
    PATH="$work/bin:$PATH" \
    sh "$script" 880 --lane-name edda-lane-gh880 \
        --worktree C:/ai_agent/edda-wt-gh880 \
        --branch fix/gh880-pi-review-arm \
    >"$work/out/prep.txt" || rc=$?
[ "$rc" -eq 0 ] || fail "prep render exit $rc"
grep -qF 'exactly ## fix/gh880-pi-review-arm...origin/main, no other lines.' "$work/out/prep.txt" \
    || fail "step 2 does not pin the tracking-suffix form"
grep -qF "git worktree add -b fix/gh880-pi-review-arm C:/ai_agent/edda-wt-gh880 origin/main" "$work/out/prep.txt" \
    || fail "launcher contract does not name the sanctioned prep command"
echo "ok 10 sanctioned prep command pinned in the rendered brief"

echo "PASS: scripts/fleet/test-brief-from-issue.sh"
