#!/bin/sh
# Offline self-test for scripts/fleet/brief-validate.sh and the next-issue.sh
# validate gate (GH-930). Stubs gh, edda and pwsh; jq, git and sh stay real.
# Writes only under its temp dir. The validator operates on the repository of
# its current working directory, so every fixture run happens with the cwd
# set to a small fixture repo (three tracked files, one commit).
#
# Default suite: the validator itself — one VALID brief, five INVALID briefs
# pinning the five brief-defect classes from the issue (absent runner,
# whitespace the tree guarantees, numeric mismatch, orphan token left by an
# edit, half-applied fix), and one structure error (missing markers).
# --integration: only the next-issue.sh gate case — the gate must already be
# inserted (delivery step 9.6); running it earlier always fails, because the
# ungated loop dies at the <<AUTHORED STEPS>> marker check instead.
#
# usage:
#   sh scripts/fleet/test-brief-validate.sh [--integration]
set -eu

cd "$(git rev-parse --show-toplevel)"
ROOT=$(pwd)
validator=scripts/fleet/brief-validate.sh
next_issue=scripts/fleet/next-issue.sh

mode=${1:-suite}
case "$mode" in
    suite) run_suite=1; run_integ=0 ;;
    --integration) run_suite=0; run_integ=1 ;;
    *) echo "usage: $0 [--integration]" >&2; exit 2 ;;
esac

sh -n "$validator" || {
    echo "FAIL: sh -n $validator" >&2
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

work=$(mktemp -d "${TMPDIR:-/tmp}/test-brief-validate.XXXXXX")
cleanup() {
    rm -rf "$work"
}
trap cleanup 0 HUP INT TERM
export TMPDIR="$work"

mkdir -p "$work/bin" "$work/fixtures" "$work/scratch" "$work/lanes" "$work/out"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

# ── fixture repo ─────────────────────────────────────────────────────

git init -q "$work/repo"
git -C "$work/repo" config user.name t
git -C "$work/repo" config user.email t@example.com
printf 'alpha\n' >"$work/repo/seed.txt"
printf 'state: old\n' >"$work/repo/note.md"
printf 'state: old\n' >"$work/repo/todo.md"
git -C "$work/repo" add -A
git -C "$work/repo" commit -qm seed
BASE=$(git -C "$work/repo" rev-parse HEAD)

# ── fixture briefs ───────────────────────────────────────────────────

facts="role: worker · lane: none · task id: none · issue: #0 ·
base full SHA: __BASE__ ·
scope paths: none ·"

cat >"$work/fixtures/brief-ok.tmpl" <<EOF
$facts
<<AUTHORED: BEGIN>>
9.1 write({"path":"new.txt","content":"line one\nline two\n"})
    output: Successfully wrote bytes to new.txt.
9.2 edit({"path":"new.txt","edits":[{"oldText":"line two","newText":"line two edited"}]})
    output: Successfully replaced 1 block(s) in new.txt.
9.3 grep -c -F line new.txt
    output: 2.
<<AUTHORED: END>>
EOF

cat >"$work/fixtures/brief-runner.tmpl" <<EOF
$facts
<<AUTHORED: BEGIN>>
9.1 not-a-real-runner-880 --version
    output: empty stdout, exit 0.
<<AUTHORED: END>>
EOF

cat >"$work/fixtures/brief-space.tmpl" <<EOF
$facts
<<AUTHORED: BEGIN>>
9.1 write({"path":"sp.txt","content":"x \n"})
    output: Successfully wrote bytes to sp.txt.
9.2 git add -- sp.txt && git diff --cached --check
    output: empty stdout, exit 0.
<<AUTHORED: END>>
EOF

cat >"$work/fixtures/brief-count.tmpl" <<EOF
$facts
<<AUTHORED: BEGIN>>
9.1 write({"path":"n.txt","content":"needle\nneedle\n"})
    output: Successfully wrote bytes to n.txt.
9.2 grep -c -F needle n.txt
    output: 1.
<<AUTHORED: END>>
EOF

cat >"$work/fixtures/brief-orphan.tmpl" <<EOF
$facts
<<AUTHORED: BEGIN>>
9.1 write({"path":"frag.sh","content":"if true\nthen\necho hi\nfi\n"})
    output: Successfully wrote bytes to frag.sh.
9.2 edit({"path":"frag.sh","edits":[{"oldText":"if true\nthen","newText":"# guard"}]})
    output: Successfully replaced 1 block(s) in frag.sh.
<<AUTHORED: END>>
EOF

cat >"$work/fixtures/brief-half.tmpl" <<EOF
$facts
<<AUTHORED: BEGIN>>
9.1 edit({"path":"note.md","edits":[{"oldText":"state: old","newText":"state: new"}]})
    output: Successfully replaced 1 block(s) in note.md.
9.2 grep -q "state: new" note.md && grep -q "state: new" todo.md
    output: empty stdout, exit 0.
<<AUTHORED: END>>
EOF

# Missing markers: the validator must refuse with exit 2, never guess.
cat >"$work/fixtures/brief-nomarkers.tmpl" <<EOF
$facts
9.1 write({"path":"new.txt","content":"x"})
    output: Successfully wrote bytes to new.txt.
EOF

for t in ok runner space count orphan half nomarkers; do
    sed "s/__BASE__/$BASE/" "$work/fixtures/brief-$t.tmpl" >"$work/fixtures/brief-$t.md"
done

# ── stubs (the validator itself needs none; next-issue.sh does) ──────

cat >"$work/fixtures/issue-ok.json" <<'JSON'
{"state":"OPEN","title":"feat(fleet): validate ok","labels":[{"name":"fleet:ready"}],"comments":[],
 "body":"## Predicted surface\n\n`scripts/fleet/brief-validate.sh`.\n\n## doneWhen\n- item\n"}
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
: >"$work/edda-calls"

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
    "pr list") resp=$(cat "$GH_FIXTURES/pr-list-empty.json") ;;
esac
case "$1 $2" in
    "issue view")
        resp=$(cat "$GH_FIXTURES/issue-ok.json") ;;
    "issue edit")
        echo "$*" >>"$GH_EDITS"
        echo "edited"
        exit 0 ;;
    "issue comment")
        { echo "---- comment ----"; cat; } >>"$GH_POSTED"
        echo "commented"
        exit 0 ;;
    "label create")
        echo "$*" >>"$GH_EDITS"
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
echo "$*" >>"$EDDA_CALLS"
exit 0
STUB
chmod +x "$work/bin/edda"

cat >"$work/bin/pwsh" <<'STUB'
#!/bin/sh
echo "pwsh-stub: $*" >>"$PWSH_CALLS"
exit 0
STUB
chmod +x "$work/bin/pwsh"

# ── default suite ────────────────────────────────────────────────────

if [ "$run_suite" -eq 1 ]; then
    # a. the valid brief: VALID + stat + .expected.diff beside the brief
    (cd "$work/repo" && sh "$ROOT/scripts/fleet/brief-validate.sh" \
        "$work/fixtures/brief-ok.md" >"$work/out/ok.txt" 2>&1) || \
        fail "brief-ok must validate: $(cat "$work/out/ok.txt")"
    grep -q '^VALID$' "$work/out/ok.txt" ||
        fail "brief-ok misses the VALID line: $(cat "$work/out/ok.txt")"
    grep -q 'new.txt' "$work/out/ok.txt" ||
        fail "brief-ok stat must name new.txt: $(cat "$work/out/ok.txt")"
    [ -f "$work/fixtures/brief-ok.md.expected.diff" ] ||
        fail "brief-ok must write the expected diff beside the brief"
    grep -q '+line two edited' "$work/fixtures/brief-ok.md.expected.diff" ||
        fail "expected diff misses the applied edit: $(cat "$work/fixtures/brief-ok.md.expected.diff")"
    grep -q 'SKIP gate new.txt' "$work/out/ok.txt" ||
        fail "brief-ok must name the ungated file it skipped: $(cat "$work/out/ok.txt")"
    rm -f "$work/fixtures/brief-ok.md.expected.diff"
    echo "ok a valid brief validates with expected diff"

    # b–f. the five defect classes: INVALID step=<n>, no expected diff
    for t in runner:9.1 space:9.2 count:9.2 orphan:9.2 half:9.2; do
        name=${t%%:*}
        want=${t#*:}
        set +e
        (cd "$work/repo" && sh "$ROOT/scripts/fleet/brief-validate.sh" \
            "$work/fixtures/brief-$name.md" >"$work/out/$name.txt" 2>&1)
        rc=$?
        set -e
        [ "$rc" -eq 1 ] || fail "brief-$name exit $rc, want 1: $(cat "$work/out/$name.txt")"
        grep -q "^INVALID step=$want" "$work/out/$name.txt" ||
            fail "brief-$name must report INVALID step=$want: $(cat "$work/out/$name.txt")"
        [ -f "$work/fixtures/brief-$name.md.expected.diff" ] &&
            fail "brief-$name must not write an expected diff"
        echo "ok $name rejected at step $want"
    done

    # g. structure error: missing markers → exit 2
    set +e
    (cd "$work/repo" && sh "$ROOT/scripts/fleet/brief-validate.sh" \
        "$work/fixtures/brief-nomarkers.md" >"$work/out/nomarkers.txt" 2>&1)
    rc=$?
    set -e
    [ "$rc" -eq 2 ] || fail "brief-nomarkers exit $rc, want 2: $(cat "$work/out/nomarkers.txt")"
    grep -qi 'marker' "$work/out/nomarkers.txt" ||
        fail "brief-nomarkers must name the missing marker: $(cat "$work/out/nomarkers.txt")"
    echo "ok g missing markers refused"

    echo "PASS: scripts/fleet/test-brief-validate.sh"
    exit 0
fi

# ── integration: the gate sits in next-issue.sh before the launch ────

if [ "$run_integ" -eq 1 ]; then
    if ! grep -q 'brief-validate' "$next_issue"; then
        fail "next-issue.sh carries no validate gate — insert it (step 9.6) before running --integration"
    fi
    cp "$work/fixtures/brief-runner.md" "$work/scratch/brief-gh886.md"
    EDDA_FLEET_SCRATCH="$work/scratch" TEMP="$work/lanes" TMPDIR="$work" \
        PATH="$work/bin:$PATH" \
        GH_FIXTURES="$work/fixtures" GH_POSTED="$work/gh-posted" \
        GH_EDITS="$work/gh-edits" PWSH_CALLS="$work/pwsh-calls" \
        EDDA_CALLS="$work/edda-calls" \
        sh "$next_issue" 886 docs/worker-9 >"$work/out/e.txt" 2>"$work/out/e.err" &&
        fail "gated next-issue must exit 2" || rc=$?
    [ "${rc:-0}" -eq 2 ] || fail "gated next-issue exit ${rc:-0}, want 2: $(cat "$work/out/e.err")"
    grep -qi 'brief-validate\|INVALID' "$work/out/e.err" "$work/out/e.txt" ||
        fail "gated refusal must name the validator: $(cat "$work/out/e.err")"
    [ "$(ls -A "$work/scratch")" = "brief-gh886.md" ] ||
        fail "gated next-issue changed the scratch dir: $(ls -A "$work/scratch")"
    [ -s "$work/pwsh-calls" ] &&
        fail "gated refusal must not launch: $(cat "$work/pwsh-calls")"
    [ -s "$work/gh-edits" ] &&
        fail "gated refusal must not touch labels or claims: $(cat "$work/gh-edits")"
    [ -s "$work/edda-calls" ] &&
        fail "gated refusal must not run task new: $(cat "$work/edda-calls")"
    echo "PASS: scripts/fleet/test-brief-validate.sh"
    exit 0
fi
