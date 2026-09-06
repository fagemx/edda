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
#   pre-push fixture (GH-922) — the dirty fixture run with NO PR number:
#     U1/C5/R3 read the runner's file list and still run, R3 FAILs, while
#     U2/U3/U6 stay N.A.(needs PR number);
#   cjk fixture (GH-922) — an R1 evidence line far past the 160-byte cell
#     cap; the capped cell must still decode as valid UTF-8.
#
# It also proves the unmarked-block contract from a temp-modified spec copy:
# a fenced sh block without markers → `UNMARKED <first line>`, exit 3.
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
      *) printf 'Issue: #1\noffline fixture PR body\n' ;;
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

# ---- fixture repo -----------------------------------------------------------
# <mode: dirty|clean|cjk> — base commit carries the repo-shaped stubs the
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
  printf '# fixture wiring stub\nexit 0\n' > "$fix/scripts/wiring-scan.sh"
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
  elif [ "$mode" = cjk ]; then
    # GH-922: one syntactically valid line whose Chinese comment pushes the
    # R1 evidence well past the 160-byte cell cap (98 CJK chars = 294 bytes).
    # The trigger literal is assembled at run time, as in the dirty fixture.
    cjk=$(printf '中文字串證據行%.0s' 1 2 3 4 5 6 7 8 9 10 11 12 13 14)
    printf '# %s rm -r%s /tmp/fixture-cjk\n' "$cjk" f > "$fix/cjk.sh"
    echo cjk.sh > "$TMP/file-list"
    msg="feat(fleet): cjk fixture change"
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
  (cd "$1" \
    && PATH="$TMP/bin:$PATH" \
       GH_STUB_DIR="$TMP" \
       REVIEW_L0_SPEC="$2" \
       sh "$RUNNER" origin/main HEAD ${4:-}) > "$3" 2>&1
}

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

# ---- 3. unmarked block: `UNMARKED <first line>`, exit 3 ---------------------
# Cut the U1 marker pair out of a temp copy of the spec; the U1 fence then
# has no markers and the runner must never skip it silently.
awk '
  !open_cut && /^# review-spec:check U1$/  { open_cut = 1; next }
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

echo "test-review-l0.sh: all fixture assertions held"
