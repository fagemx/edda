#!/bin/sh
# test-review-l0.sh — GH-882: offline fixture for scripts/review-l0.sh.
#
# Builds a throwaway git repo (the fixture), stubs the PR surface (`gh`,
# `edda`) on PATH, and runs the runner twice against the real REVIEW.md:
#
#   dirty fixture — the diff adds bad.sh carrying a destructive-command
#     line (R1 hit; built via printf so the trigger literal never appears in
#     this script's own diff) and a syntax error (R3 `sh -n` fails) → both
#     rows FAIL, runner exit 1;
#   clean fixture — the diff adds a valid good.sh → every run row is PASS
#     (N.A. / 需升級 rows are the runner's own contract, never FAIL/ERROR),
#     runner exit 0.
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
# <mode: dirty|clean> — base commit carries the repo-shaped stubs the blocks
# need (lint / wiring scripts, a Cargo.toml with a version line); the branch
# commit adds the fixture script that the rules then judge.
make_fixture() {
  mode=$1
  fix="$TMP/$mode"
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
  else
    printf '#!/bin/sh\necho fixture-ok\n' > "$fix/good.sh"
    echo good.sh > "$TMP/file-list"
    msg="feat(fleet): clean fixture change"
  fi
  git -C "$fix" add -A
  git -C "$fix" commit -q -m "$msg"
}

run_l0() { # <fixture-dir> <spec> <out-file> — runner exit code via $?
  (cd "$1" \
    && PATH="$TMP/bin:$PATH" \
       GH_STUB_DIR="$TMP" \
       REVIEW_L0_SPEC="$2" \
       sh "$RUNNER" origin/main HEAD 1) > "$3" 2>&1
}

# ---- 1. dirty fixture: R1 + R3 FAIL, exit 1 ---------------------------------
make_fixture dirty
rc=0
run_l0 "$TMP/dirty" "$SPEC" "$TMP/dirty.out" || rc=$?
[ "$rc" -eq 1 ] || fail "dirty fixture: expected exit 1, got $rc"
grep -Fq '| R1 | code-risk | P0 | FAIL' "$TMP/dirty.out" \
  || fail "dirty fixture: no FAIL row for R1"
grep -Fq '| R3 | code-risk | P0 | FAIL' "$TMP/dirty.out" \
  || fail "dirty fixture: no FAIL row for R3"
echo "dirty fixture: R1 and R3 rows FAIL, runner exit 1 — OK"

# ---- 2. clean fixture: no FAIL/ERROR rows, exit 0 ---------------------------
make_fixture clean
rc=0
run_l0 "$TMP/clean" "$SPEC" "$TMP/clean.out" || rc=$?
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
run_l0 "$TMP/clean" "$TMP/unmarked-spec.md" "$TMP/unmarked.out" || rc=$?
[ "$rc" -eq 3 ] || fail "unmarked spec: expected exit 3, got $rc"
grep -q '^UNMARKED gh pr diff' "$TMP/unmarked.out" \
  || fail "unmarked spec: no UNMARKED line"
echo "unmarked spec: UNMARKED line printed, runner exit 3 — OK"

echo "test-review-l0.sh: all fixture assertions held"
