#!/bin/sh
# test-review-l0.sh — GH-882: offline fixture for scripts/review-l0.sh.
#
# Builds a throwaway git repo (the fixture), stubs the PR surface (`gh`,
# `edda`) on PATH, and runs the runner against the real REVIEW.md:
#
#   dirty fixture — the diff adds bad.sh carrying a destructive-command
#     line (R1 hit; built via printf so the trigger literal never appears in
#     this script's own diff) and a syntax error (R3 `sh -n` fails) → both
#     rows FAIL, runner exit 1;
#   clean fixture — the diff adds a valid good.sh → every run row is PASS
#     (N.A. / 需升級 rows are the runner's own contract, never FAIL/ERROR),
#     runner exit 0;
#   U3 mutations — exact line present, missing-only (including an Issue-looking
#     stderr warning), no PR, body-read failure, and missing plus a real R3
#     syntax failure prove only the exact-line convention is nonblocking;
#   pre-push fixture (GH-922) — the dirty fixture run with NO PR number:
#     U1/C5/R3 read the runner's file list and still run, R3 FAILs, while
#     U2/U3/U6 stay N.A.(needs PR number);
#   cjk fixture (GH-922) — an R1 evidence line far past the 160-byte cell
#     cap; the capped cell must still decode as valid UTF-8.
#   missing-file fixture (GH-992) — two changed .sh files, one clean and
#     present, one committed on the branch then removed from disk (run under
#     L0_SHA, so the file list comes from git history, not the working tree)
#     with a name that sorts alphabetically last → R3 must stay PASS, not
#     flip to FAIL from the short-circuit the missing file used to cause.
#
# It also proves the unmarked-block contract from a temp-modified spec copy:
# a fenced sh block without markers → `UNMARKED <first line>`, exit 3.
#
# A final check greps REVIEW.md itself for a regressed two-dot `git diff`
# enumerator (GH-1003) — static analysis on the spec, not a fixture run.
#
# Everything is written inside one mktemp directory; nothing outside it.
#
# usage: sh scripts/test-review-l0.sh   (exit 0 = all assertions held)

set -u

ROOT=$(cd "$(dirname "$0")/.." && pwd)
RUNNER="$ROOT/scripts/review-l0.sh"
SPEC="$ROOT/REVIEW.md"

[ -f "$RUNNER" ] || { echo "test-review-l0.sh: missing $RUNNER" >&2; exit 2; }
[ -f "$SPEC" ] || { echo "test-review-l0.sh: missing $SPEC" >&2; exit 2; }

TMP=$(mktemp -d "${TMPDIR:-/tmp}/test-review-l0.XXXXXX") || exit 2
trap 'rm -r "$TMP"' EXIT
trap 'exit 130' INT TERM

fail() { echo "test-review-l0.sh: $1" >&2; exit 1; }

# ---- stub gh / edda: the offline PR surface --------------------------------
mkdir "$TMP/bin"
cat > "$TMP/bin/gh" <<'STUB'
#!/bin/sh
case "$1 $2" in
  "pr view")
    case "$*" in
      *baseRefOid*) cat "$GH_STUB_DIR/base-sha" ;;
      *)
        if [ -f "$GH_STUB_DIR/body-read-stderr" ]; then
          cat "$GH_STUB_DIR/body-read-stderr" >&2
        fi
        if [ -f "$GH_STUB_DIR/body-read-exit" ]; then
          # Simulate a transport that emitted partial body bytes before its
          # nonzero status. Nonempty stdout is not authority for success.
          cat "$GH_STUB_DIR/pr-body"
          exit "$(cat "$GH_STUB_DIR/body-read-exit")"
        fi
        cat "$GH_STUB_DIR/pr-body" ;;
    esac ;;
  "pr diff")
    case "$*" in
      *--name-only*) cat "$GH_STUB_DIR/file-list" ;;
      *) printf 'diff --git a/fixture b/fixture\n' ;;
    esac ;;
  *) exit 0 ;;
esac
STUB
cat > "$TMP/bin/edda" <<'STUB'
#!/bin/sh
case "$1" in
  --version) echo "edda 0.0.0-fixture" ;;
  *) : ;;
esac
STUB
chmod +x "$TMP/bin/gh" "$TMP/bin/edda"

set_pr_body() { # <body> — successful read, no stderr
  rm -f "$TMP/body-read-exit" "$TMP/body-read-stderr"
  printf '%s\n' "$1" > "$TMP/pr-body"
}

set_pr_body_with_stderr() { # <body> <stderr> — successful read
  set_pr_body "$1"
  printf '%s\n' "$2" > "$TMP/body-read-stderr"
}

set_body_read_failure() { # <exit> <stderr>
  rm -f "$TMP/body-read-stderr"
  printf '%s\n' "$1" > "$TMP/body-read-exit"
  printf '%s\n' "$2" > "$TMP/body-read-stderr"
}

set_pr_body 'Issue: #1
offline fixture PR body'

# ---- fixture repo -----------------------------------------------------------
# <mode: dirty|syntax|clean|cjk> — base commit carries the repo-shaped stubs the
# blocks need (lint / wiring scripts, a Cargo.toml with a version line); the
# branch commit adds the fixture script that the rules then judge. Each call
# builds a fresh directory: re-using one would re-point its origin/main at
# the branch head and collapse the diff the rules are meant to see.
FIXSEQ=0
FIXDIR=
make_fixture() {
  mode=$1
  FIXSEQ=$((FIXSEQ + 1))
  fix="$TMP/fix-$FIXSEQ"
  FIXDIR="$fix"
  mkdir -p "$fix/scripts"
  git -C "$fix" init -q
  git -C "$fix" config user.email fixture@example.invalid
  git -C "$fix" config user.name fixture
  printf '[workspace.package]\nversion = "0.0.0-fixture"\n' > "$fix/Cargo.toml"
  printf '# fixture lint stub\nexit 0\n' > "$fix/scripts/lint-markdown-content.sh"
  # The wiring stub mirrors the one thing about the real script the runner
  # has to classify: its refusal (scripts/wiring-scan.sh:30-34). An
  # unconditional `exit 0` would make WIRING report PASS on a range no block
  # can resolve, which is exactly the GH-950 defect the §7a case exists to
  # catch — the stub would hide it rather than stand in for the tool.
  cat > "$fix/scripts/wiring-scan.sh" <<'STUB'
# fixture wiring stub — mirrors the real script's refusal contract
for ref in "$1" "$2"; do
  if ! git rev-parse --verify --quiet "${ref}^{commit}" >/dev/null 2>&1; then
    echo "error: unknown revision $ref" >&2
    exit 2
  fi
done
exit 0
STUB
  git -C "$fix" add -A
  git -C "$fix" commit -q -m "chore(fleet): fixture base"
  git -C "$fix" rev-parse HEAD > "$TMP/base-sha"
  git -C "$fix" update-ref refs/remotes/origin/main "$(cat "$TMP/base-sha")"
  git -C "$fix" checkout -q -b feat/fixture
  if [ "$mode" = dirty ]; then
    # R1 hit (a destructive command) AND R3 failure (`if then` does not
    # parse). The trigger is assembled at run time so this script's own diff
    # carries no R1 hit of its own. The file is only committed and parsed by
    # `sh -n` — never executed.
    {
      printf '#!/bin/sh\n'
      printf 'rm -r%s "${TMPDIR:-/tmp}"/fixture-target\n' f
      printf 'if then\n'
    } > "$fix/bad.sh"
    echo bad.sh > "$TMP/file-list"
    msg="feat(fleet): dirty fixture change"
  elif [ "$mode" = syntax ]; then
    # A real R3 failure without an R1 candidate, used to prove U3's advisory
    # result does not suppress another rule's aggregate failure.
    printf '#!/bin/sh\nif then\n' > "$fix/syntax.sh"
    echo syntax.sh > "$TMP/file-list"
    msg="fix(fleet): syntax fixture change"
  elif [ "$mode" = cjk ]; then
    # GH-922: one syntactically valid line whose Chinese comment pushes the
    # R1 evidence well past the 160-byte cell cap (98 CJK chars = 294 bytes).
    # The trigger literal is assembled at run time, as in the dirty fixture.
    cjk=$(printf '中文字串證據行%.0s' 1 2 3 4 5 6 7 8 9 10 11 12 13 14)
    printf '# %s rm -r%s /tmp/fixture-cjk\n' "$cjk" f > "$fix/cjk.sh"
    echo cjk.sh > "$TMP/file-list"
    msg="feat(fleet): cjk fixture change"
  elif [ "$mode" = missing ]; then
    # GH-992: two changed .sh files, both committed on this branch — one
    # stays on disk (clean), the other the caller removes from the working
    # tree right after make_fixture returns (see the missing-file fixture
    # below), simulating a reviewer whose checkout never fetched a file the
    # PR added. "zzz-" sorts alphabetically last, the exact position R3's old
    # `[ -f ] &&` short-circuit needed to flip a clean round to a false P0.
    printf '#!/bin/sh\necho fixture-ok\n' > "$fix/aaa-exists.sh"
    printf '#!/bin/sh\necho never fetched\n' > "$fix/zzz-added-by-pr.sh"
    printf 'aaa-exists.sh\nzzz-added-by-pr.sh\n' > "$TMP/file-list"
    msg="feat(fleet): missing-file fixture change"
  else
    printf '#!/bin/sh\necho fixture-ok\n' > "$fix/good.sh"
    echo good.sh > "$TMP/file-list"
    msg="feat(fleet): clean fixture change"
  fi
  git -C "$fix" add -A
  git -C "$fix" commit -q -m "$msg"
}

run_l0() { # <fixture-dir> <spec> <out-file> [PR-number] — runner exit code via $?
  # With no PR-number argument the runner runs in the pre-push shape
  # (GH-922): U1/C5/R3 must still run, from the runner's own file list.
  # L0_BASE overrides the <base> argument for the GH-950 cases; every other
  # caller gets the ordinary origin/main shape. L0_SHA overrides the literal
  # "HEAD" <head> argument (GH-992's missing-file fixture): passing a real
  # commit switches the runner to its `git diff --name-only base...sha` file
  # list (review-l0.sh's non-HEAD branch), which is computed from git history
  # alone — the only way to make a file "changed" per that list while absent
  # from this working tree, exactly the reviewer-behind-the-PR shape the bug
  # needs. Every other caller gets the ordinary working-tree-vs-base shape.
  (cd "$1" \
    && PATH="$TMP/bin:$PATH" \
       GH_STUB_DIR="$TMP" \
       REVIEW_L0_SPEC="$2" \
       sh "$RUNNER" "${L0_BASE:-origin/main}" "${L0_SHA:-HEAD}" ${4:-}) > "$3" 2>&1
}
L0_BASE=
L0_SHA=

# ---- 1. dirty fixture: R1 + R3 FAIL, exit 1 ---------------------------------
make_fixture dirty
rc=0
run_l0 "$FIXDIR" "$SPEC" "$TMP/dirty.out" || rc=$?
[ "$rc" -eq 1 ] || fail "dirty fixture: expected exit 1, got $rc"
grep -Fq '| R1 | code-risk | P0 | FAIL' "$TMP/dirty.out" \
  || fail "dirty fixture: no FAIL row for R1"
grep -Fq '| R3 | code-risk | P0 | FAIL' "$TMP/dirty.out" \
  || fail "dirty fixture: no FAIL row for R3"
echo "dirty fixture: R1 and R3 rows FAIL, runner exit 1 — OK"

# ---- 2. clean fixture: no FAIL/ERROR rows, exit 0 ---------------------------
make_fixture clean
rc=0
run_l0 "$FIXDIR" "$SPEC" "$TMP/clean.out" || rc=$?
[ "$rc" -eq 0 ] || fail "clean fixture: expected exit 0, got $rc"
if grep -E '\| (FAIL|ERROR)' "$TMP/clean.out"; then
  fail "clean fixture: FAIL/ERROR rows present"
fi
grep -Fq 'classes=' "$TMP/clean.out" || fail "clean fixture: no classifier line"
grep -Fq '| R3 | code-risk | P0 | PASS' "$TMP/clean.out" \
  || fail "clean fixture: R3 row not PASS"
# Table completeness. The suite asserted individual rows and never a total,
# which is how the dropped-last-id defect shipped (GH-882 review round 1): a
# missing row sets no FAIL, so only an explicit per-id check catches it. The
# fixture's changed files route the `any`, code-plain and code-risk arms.
for r in U1 U2 U3 U4 U5 U6 U7 C1 C2 C3 C4 C5 R1 R2 R3 R4 R5 WIRING; do
  grep -Fq "| $r |" "$TMP/clean.out" \
    || fail "clean fixture: no row for routed rule $r"
done
echo "clean fixture: every routed rule printed a row — OK"

echo "clean fixture: all rows PASS/N.A./需升級, runner exit 0 — OK"

# ---- 2a. U3 policy mutations (delivery-first V3) ---------------------------
# One clean code-risk fixture is reused while only the PR body/read outcome is
# mutated. This isolates U3 from the unchanged rules and keeps every case
# offline.
make_fixture clean

set_pr_body 'Issue: #1
offline fixture PR body'
rc=0
run_l0 "$FIXDIR" "$SPEC" "$TMP/u3-present.out" 17 || rc=$?
[ "$rc" -eq 0 ] || fail "U3 present: expected exit 0, got $rc: $(cat "$TMP/u3-present.out")"
grep -Fq '| U3 | any | P2 | PASS' "$TMP/u3-present.out" \
  || fail "U3 present: exact line was not observed as PASS: $(grep -F '| U3 |' "$TMP/u3-present.out")"
echo "U3 present fixture: exact Issue line observed, runner exit 0 — OK"

# The stderr text deliberately looks like an exact line. Only gh stdout is PR
# body data, so this remains missing-only and advisory rather than false PASS.
set_pr_body_with_stderr 'Closes #1
no exact convention line' 'Issue: #999 (simulated gh warning on stderr)'
rc=0
run_l0 "$FIXDIR" "$SPEC" "$TMP/u3-missing.out" 17 || rc=$?
[ "$rc" -eq 0 ] || fail "U3 missing-only: expected exit 0, got $rc: $(cat "$TMP/u3-missing.out")"
grep -F '| U3 | any | P2 |' "$TMP/u3-missing.out" \
  | grep -Fq 'N.A.(non-blocking convention)' \
  || fail "U3 missing-only: no visible advisory row: $(grep -F '| U3 |' "$TMP/u3-missing.out")"
grep -F '| U3 |' "$TMP/u3-missing.out" | grep -Fq 'no exact Issue:/Issues: line' \
  || fail "U3 missing-only: advisory lacks explicit missing-line evidence"
grep -Fq '| U3 | any | P2 | PASS' "$TMP/u3-missing.out" \
  && fail "U3 missing-only: stderr masqueraded as a present Issue line"
echo "U3 missing-only fixture: visible P2 advisory, stderr cannot masquerade, runner exit 0 — OK"

rc=0
run_l0 "$FIXDIR" "$SPEC" "$TMP/u3-no-pr.out" || rc=$?
[ "$rc" -eq 0 ] || fail "U3 no-PR: expected exit 0, got $rc: $(cat "$TMP/u3-no-pr.out")"
grep -F '| U3 | any | P2 |' "$TMP/u3-no-pr.out" \
  | grep -Fq 'N.A.(needs PR number)' \
  || fail "U3 no-PR: needs-PR result changed: $(grep -F '| U3 |' "$TMP/u3-no-pr.out")"
echo "U3 no-PR fixture: needs-PR N.A. unchanged, runner exit 0 — OK"

# Exit 41 is intentionally outside the runner's generic 2/127/128 refusal set:
# the U3 branch itself must preserve gh's status and classify the failed read,
# even when partial stdout and stderr both look like present Issue lines.
set_pr_body 'Issue: #42
partial body from a failed read'
set_body_read_failure 41 'Issue: #999 (simulated failed gh read on stderr)'
rc=0
run_l0 "$FIXDIR" "$SPEC" "$TMP/u3-read-failure.out" 17 || rc=$?
[ "$rc" -eq 2 ] || fail "U3 body-read failure: expected aggregate exit 2, got $rc: $(cat "$TMP/u3-read-failure.out")"
grep -Fq '| U3 | any | P2 | ERROR 41 |' "$TMP/u3-read-failure.out" \
  || fail "U3 body-read failure: gh exit was not preserved as ERROR: $(grep -F '| U3 |' "$TMP/u3-read-failure.out")"
grep -Fq '| U3 | any | P2 | PASS' "$TMP/u3-read-failure.out" \
  && fail "U3 body-read failure: stderr/nonzero masqueraded as a present line"
echo "U3 body-read failure fixture: ERROR 41 and aggregate exit 2 preserved — OK"

set_pr_body 'Closes #1
no exact convention line'
make_fixture syntax
rc=0
run_l0 "$FIXDIR" "$SPEC" "$TMP/u3-plus-r3.out" 17 || rc=$?
[ "$rc" -eq 1 ] || fail "U3 plus R3: expected aggregate exit 1, got $rc: $(cat "$TMP/u3-plus-r3.out")"
grep -F '| U3 | any | P2 |' "$TMP/u3-plus-r3.out" \
  | grep -Fq 'N.A.(non-blocking convention)' \
  || fail "U3 plus R3: missing line did not remain advisory"
grep -Fq '| R3 | code-risk | P0 | FAIL' "$TMP/u3-plus-r3.out" \
  || fail "U3 plus R3: real syntax failure was not blocking"
echo "U3 plus R3 fixture: advisory retained, R3 FAIL and aggregate exit 1 preserved — OK"

# Restore the ordinary successful body for all historical fixtures below.
set_pr_body 'Issue: #1
offline fixture PR body'

# ---- 3. unmarked block: `UNMARKED <first line>`, exit 3 ---------------------
# Cut the U1 marker pair out of a temp copy of the spec; the U1 fence then
# has no markers and the runner must never skip it silently.
awk '
  !open_cut && /^# review-spec:check U1( |$)/ { open_cut = 1; next }
  open_cut && !end_cut && /^# review-spec:check-end$/ { end_cut = 1; next }
  { print }
' "$SPEC" > "$TMP/unmarked-spec.md"
rc=0
run_l0 "$FIXDIR" "$TMP/unmarked-spec.md" "$TMP/unmarked.out" || rc=$?
[ "$rc" -eq 3 ] || fail "unmarked spec: expected exit 3, got $rc"
grep -q '^UNMARKED if \[ -n "${REVIEW_FILES:-}" \]' "$TMP/unmarked.out" \
  || fail "unmarked spec: no UNMARKED line"
echo "unmarked spec: UNMARKED line printed, runner exit 3 — OK"

# ---- 4. pre-push coverage: U1/C5/R3 run with no PR number (GH-922) ----------
# The dirty fixture re-used with no PR argument: U1, C5 and R3 read the
# runner's own file list (REVIEW_FILES) and must NOT report N.A.(needs PR
# number); R3 still FAILs on bad.sh's syntax error. U2, U3 and U6 genuinely
# need a PR and stay N.A. — the boundary d-004 fixes and the one it does not.
make_fixture dirty
rc=0
run_l0 "$FIXDIR" "$SPEC" "$TMP/prepush.out" || rc=$?
[ "$rc" -eq 1 ] || fail "pre-push fixture: expected exit 1, got $rc"
for r in U1 C5 R3; do
  grep -F "| $r |" "$TMP/prepush.out" >/dev/null \
    || fail "pre-push fixture: no row for $r"
  if grep -F "| $r |" "$TMP/prepush.out" | grep -Fq 'N.A.(needs PR number)'; then
    fail "pre-push fixture: $r still N.A.(needs PR number)"
  fi
done
grep -Fq '| R3 | code-risk | P0 | FAIL' "$TMP/prepush.out" \
  || fail "pre-push fixture: R3 row not FAIL"
for r in U2 U3 U6; do
  grep -F "| $r |" "$TMP/prepush.out" | grep -Fq 'N.A.(needs PR number)' \
    || fail "pre-push fixture: $r should stay N.A.(needs PR number)"
done
echo "pre-push fixture: U1/C5/R3 ran without a PR, R3 FAIL, U2/U3/U6 stay N.A. — OK"

# ---- 5. CJK evidence cell: the cap never splits a multi-byte character ------
# The evidence line is a long Chinese comment carrying an R1 hit; after the
# 160-byte cap the cell must still decode as valid UTF-8 (iconv round-trip).
make_fixture cjk
rc=0
run_l0 "$FIXDIR" "$SPEC" "$TMP/cjk.out" || rc=$?
[ "$rc" -eq 1 ] || fail "cjk fixture: expected exit 1 (R1 hit), got $rc"
grep -Fq '| R1 | code-risk | P0 | FAIL' "$TMP/cjk.out" \
  || fail "cjk fixture: no FAIL row for R1"
grep -F '| R1 |' "$TMP/cjk.out" | iconv -f UTF-8 -t UTF-8 >/dev/null \
  || fail "cjk fixture: R1 evidence cell does not decode as valid UTF-8"
echo "cjk fixture: R1 evidence cell survives the cap as valid UTF-8 — OK"

# ---- 6. marker attributes: a declared capability, not a text guess (GH-958) -
# The $N gate used to decide a block could run without a PR number by grepping
# its command text for the string REVIEW_FILES. The capability is now declared
# on the marker (`# review-spec:check U1 no-pr-needed`), so:
#   (a) an unknown attribute is a spec error, exit 2 — a typo must not silently
#       send the rule back to N.A. on every pre-push pass;
#   (b) dropping the attribute while the body still mentions REVIEW_FILES puts
#       U1 back to N.A.(needs PR number). Under the old textual gate the block
#       still ran, which is exactly the coupling this replaces.
make_fixture dirty

sed 's/^# review-spec:check U1 no-pr-needed$/# review-spec:check U1 no-pr-neded/' \
  "$SPEC" > "$TMP/typo-spec.md"
grep -q '^# review-spec:check U1 no-pr-neded$' "$TMP/typo-spec.md" \
  || fail "attribute fixture: the typo copy was not produced"
rc=0
run_l0 "$FIXDIR" "$TMP/typo-spec.md" "$TMP/typo.out" || rc=$?
[ "$rc" -eq 2 ] || fail "attribute fixture: unknown attribute should exit 2, got $rc"
# run_l0 folds the runner stderr into the out file
grep -q 'unknown review-spec:check attribute' "$TMP/typo.out" \
  || fail "attribute fixture: output does not name the unknown attribute: $(cat "$TMP/typo.out")"
echo "attribute fixture: an unknown marker attribute exits 2 — OK"

sed 's/^# review-spec:check U1 no-pr-needed$/# review-spec:check U1/' \
  "$SPEC" > "$TMP/undeclared-spec.md"
rc=0
run_l0 "$FIXDIR" "$TMP/undeclared-spec.md" "$TMP/undeclared.out" || rc=$?
grep -F '| U1 |' "$TMP/undeclared.out" | grep -Fq 'N.A.(needs PR number)' \
  || fail "attribute fixture: U1 without the marker attribute should be N.A.(needs PR number), got: $(grep -F '| U1 |' "$TMP/undeclared.out" || true)"
grep -F '| C5 |' "$TMP/undeclared.out" | grep -Fq 'N.A.(needs PR number)' \
  && fail "attribute fixture: C5 keeps its attribute and must still run"
echo "attribute fixture: the gate reads the marker, not the command text — OK"

# ---- 7. a block that never ran is ERROR, never PASS or FAIL (GH-950) -------
# 7a. A full SHA as <base>. The blocks diff "origin/$BASE..$SHA", so
# "origin/<40-hex>" is not a revision and every git-backed block refuses. The
# refusal text arrives as ordinary output — the failing stage's status never
# reaches the classifier ($? is the pipeline's last stage) — so before the fix
# D1 and D3 reported PASS with a `fatal:` line sitting in their own evidence,
# and U4, C2, C4, R1 reported FAIL. None of them ran.
make_fixture clean
# A prefix assignment on a function call persists in POSIX sh, so set and
# clear it explicitly rather than relying on that.
L0_BASE=$(cat "$TMP/base-sha")
rc=0
run_l0 "$FIXDIR" "$SPEC" "$TMP/sha-base.out" || rc=$?
L0_BASE=
[ "$rc" -ne 0 ] || fail "full-SHA base: runner exited 0 over rules that never ran"
grep -Fq 'fatal:' "$TMP/sha-base.out" \
  || fail "full-SHA base: fixture produced no refusal at all — the case is not being exercised"
# The defect in one assertion: no row may carry a verdict over a refusal —
# stated over both wordings, because the two arrive by different routes. A
# git-backed block's `fatal:` survives only as text (its status is eaten by
# the grep it feeds); WIRING's tool is invoked directly, so its `error:` line
# comes with a truthful exit 2. Asserting only the first would leave the
# doneWhen's "ERROR for every affected rule" half-tested.
verdicts=$(grep -E '\| (PASS|FAIL) \|' "$TMP/sha-base.out" | grep -E 'fatal:|error: unknown revision' || true)
[ -z "$verdicts" ] \
  || fail "full-SHA base: a PASS/FAIL row reports a verdict over a refusal: $verdicts"
grep -F '| ERROR |' "$TMP/sha-base.out" | grep -Fq 'fatal:' \
  || fail "full-SHA base: no ERROR row for the refused blocks"
# WIRING by name: it is the rule the `fatal:` net alone does not reach, and
# the one that proved the runner still had to key on the exit code too.
wiring=$(grep -F '| WIRING |' "$TMP/sha-base.out")
case "$wiring" in
  *'| ERROR '*) : ;;
  *) fail "full-SHA base: WIRING refused and was not ERROR: $wiring" ;;
esac
# Exit 2 (a rule could not run), not 1 (a rule failed): with every affected
# rule ERROR there is no finding left to report, and the two exits mean
# different things to a caller.
[ "$rc" -eq 2 ] || fail "full-SHA base: expected runner exit 2 (could not run), got $rc"
echo "full-SHA base: refused blocks are ERROR, none PASS/FAIL — OK"

# 7b. The generic case, with no bad range anywhere: a block that prints a
# `fatal:` line and still EXITS 0 through a grep tail. Before the fix this was
# the reassuring misreading — an enumerator tail with output reads as FAIL, and
# with none as PASS; either way the runner reports a verdict for a rule that
# refused to run. The fix is at the classifier, so it holds for any block and
# any cause, not only for git and not only for this argument shape.
sub_cmd='echo "fatal: simulated refusal" >&2; echo ok | grep -v ZZZ'
awk -v s="$sub_cmd" '
  /^# review-spec:check U5( |$)/ { print; print "```sh"; print s; print "```"; incut = 1; next }
  incut && /^# review-spec:check-end$/ { incut = 0; print; next }
  incut { next }
  { print }
' "$SPEC" > "$TMP/fatal-spec.md"
grep -Fq 'fatal: simulated refusal' "$TMP/fatal-spec.md" \
  || fail "generic fatal: the substituted spec copy was not produced"
make_fixture clean
rc=0
run_l0 "$FIXDIR" "$TMP/fatal-spec.md" "$TMP/fatal.out" || rc=$?
u5=$(grep -F '| U5 |' "$TMP/fatal.out")
case "$u5" in
  *'| ERROR |'*) : ;;
  *) fail "generic fatal: U5 exited 0 with a 'fatal:' line and was not ERROR: $u5" ;;
esac
case "$u5" in
  *'exit=0'*) : ;;
  *) fail "generic fatal: U5's evidence does not record the exit-0 contradiction: $u5" ;;
esac
[ "$rc" -eq 2 ] || fail "generic fatal: expected runner exit 2 (ERROR, no FAIL), got $rc"
echo "generic fatal: an exit-0 block printing 'fatal:' is ERROR — OK"

# ---- 8. R3 short-circuit: a changed .sh missing from the tree, sorting -----
# alphabetically last, must not flip the block's own exit status (GH-992).
# L0_SHA switches review-l0.sh to its base...sha (pure git-history) file
# list, so "changed" comes from the commit and is independent of what this
# rm leaves on disk — the real-world shape (a reviewer checked out at $BASE,
# not at the PR's $SHA), not the working-tree-vs-base shape every other
# fixture here runs under.
make_fixture missing
rm "$FIXDIR/zzz-added-by-pr.sh"
L0_SHA=$(git -C "$FIXDIR" rev-parse HEAD)
rc=0
run_l0 "$FIXDIR" "$SPEC" "$TMP/missing.out" || rc=$?
L0_SHA=
[ "$rc" -eq 0 ] || fail "missing-file fixture: expected runner exit 0, got $rc: $(cat "$TMP/missing.out")"
grep -Fq '| R3 | code-risk | P0 | FAIL' "$TMP/missing.out" \
  && fail "missing-file fixture: R3 row is FAIL — the short-circuit regressed (GH-992)"
grep -Fq '| R3 | code-risk | P0 | PASS' "$TMP/missing.out" \
  || fail "missing-file fixture: R3 row is not PASS: $(grep -F '| R3 |' "$TMP/missing.out")"
echo "missing-file fixture: R3 not FAIL when the alphabetically-last changed .sh is absent from the tree — OK"

# ---- 9. two-dot git diff regression guard (GH-1003) -------------------------
# REVIEW.md's content enumerators in §5 are three-dot (from the merge base);
# only the git-log commit enumerators (U2, U4, C2 — REVIEW.md §2 explains
# why) stay two-dot. A `git diff "origin/$BASE..$SHA"` regression silently
# widens or narrows the reviewed surface on a branch behind its base. This
# pattern cannot match the fixed three-dot form (the literal "BASE" and "SHA"
# anchors on both sides of the dots make the two forms mutually exclusive
# substrings — verified empirically, not just by inspection).
twodot=$(grep -nE 'git diff "origin/\$BASE\.\.\$SHA"' "$SPEC" || true)
[ -z "$twodot" ] \
  || fail "two-dot git diff regression in $SPEC (GH-1003): $twodot"
echo "two-dot git diff regression guard: no offending pattern in REVIEW.md — OK"

echo "test-review-l0.sh: all fixture assertions held"
