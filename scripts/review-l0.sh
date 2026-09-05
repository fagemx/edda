#!/bin/sh
# review-l0.sh — GH-882: run REVIEW.md's mechanical check blocks as one L0 pass.
#
# Every fenced sh check block in REVIEW.md is wrapped in two marker lines,
#
#   # review-spec:check <RULE>   ...   # review-spec:check-end
#
# the same convention as the §3 classifier block (which keeps its own markers
# and additionally carries check markers outside its fence). This script
# extracts those blocks FROM THE SPEC AT RUN TIME — no rule command is
# embedded here (review.spec-router-single-source) — runs the §3 classifier
# block on the changed-file list, then every block whose rule §4 routes to
# the resulting classes, with BASE / SHA / N exported exactly the way the
# blocks already expect. Output is one §7-shaped Rules row per routed rule:
# a pre-push self-check for flash-tier lanes (brief-template Example B) and a
# precomputed Rules table for the reviewer.
#
# Blocks whose marker is not a §4 rule id (FETCH, ISSUES, DIFF, ROUTER,
# CLASSIFIER, ELAPSED) are the reviewer's own procedure steps, not rules: the
# runner executes only the classifier and the routed rule blocks.
#
# usage: sh scripts/review-l0.sh <base> <head> [PR-number]
#   <base>   base ref the way the blocks write it, e.g. origin/main — the
#            blocks diff "origin/$BASE..$SHA", so a leading "origin/" is
#            stripped before BASE is exported (a plain branch name works too).
#   <head>   a commit, a ref, or the literal HEAD. With the literal HEAD the
#            classifier's file list is the working tree against <base>
#            (staged and unstaged work included) while the blocks diff the
#            committed range — the pre-push shape of Example B step 20.
#   [PR-number]  blocks referencing "$N" (U1, U2's first probe, U3, U6, C5,
#            R3) run only when a PR number is passed; without one they report
#            N.A.(needs PR number) instead of failing. Offline callers can
#            fake the PR surface by putting a stub gh earlier in PATH —
#            scripts/test-review-l0.sh does exactly that.
#   REVIEW_L0_SPEC  spec file to extract from (default REVIEW.md in the cwd —
#            the checkout's own copy, so a lane self-checks the rules exactly
#            as its tree states them; point it at another copy to be judged
#            by that copy instead).
#
# Row result convention (REVIEW.md §0 — a piped check signals by output):
#   PASS    the block exited 0 with no finding signal; or an enumerator-grep
#           tail (the last pipeline stage is "grep -<flags>") printed nothing
#           — grep found no offenders; or its "grep -c" tail printed a count
#           — a count is data for the reviewer (C2 pairs it with the commit
#           subjects), never adjudicated here; or, for a rule whose spec
#           states the inverse signal (U3: "Empty output is the failure"),
#           it printed at least one line.
#   FAIL    the block printed findings — a non-empty enumerator-grep output,
#           an "exit=<nonzero>" tail line (U5, R3 print their exit), a
#           "CANDIDATE" line (D1), or a "MISSING" line (D3) — or exited
#           non-zero for any other reason. The script reports exit codes and
#           printed lines; it does not adjudicate what a finding means.
#   ERROR   the block failed for a non-finding reason: exit 127 (command not
#           found) or exit 128 (bad range).
#   N.A.    the rule needs reviewer input (D2's decision key), needs a PR
#           number, or has no command block in the spec (U7, S2, S3, C1,
#           R4, R5).
#   需升級   the [判斷] rules (D5, R2) — escalated per REVIEW.md §6, never
#           adjudicated here.
#
# exit: 0 every row PASS / N.A. / 需升級 · 1 any FAIL ·
#       2 any ERROR and no FAIL · 3 an unmarked sh block in the spec
#       (printed as "UNMARKED <first line>" — never a silent skip)

set -u

if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
  echo "usage: sh scripts/review-l0.sh <base> <head> [PR-number]" >&2
  exit 2
fi
ARG_BASE=$1
ARG_SHA=$2
N=${3:-}
SPEC=${REVIEW_L0_SPEC:-REVIEW.md}

[ -f "$SPEC" ] || { echo "review-l0.sh: no spec at $SPEC" >&2; exit 2; }

# A mistyped review range must never false-green with an empty diff.
for rev in "$ARG_BASE" "$ARG_SHA"; do
  git rev-parse --verify --quiet "${rev}^{commit}" >/dev/null 2>&1 || {
    echo "review-l0.sh: not a commit: $rev" >&2
    exit 2
  }
done

# The blocks write "origin/$BASE..$SHA", so BASE is exported without "origin/".
BASE=${ARG_BASE#origin/}
SHA=$ARG_SHA
export BASE SHA N

TMP=$(mktemp -d "${TMPDIR:-/tmp}/review-l0.XXXXXX") || exit 2
trap 'rm -r "$TMP"' EXIT
trap 'exit 130' INT TERM

# ---- extract the marked blocks ---------------------------------------------
# One pass over the spec. A fenced sh block inside check markers lands in
# $TMP/blocks as:
#   @@BLOCK@@ <RULE> <n>
#   ...block body...
#   @@END@@
# A fenced sh block with no check markers prints "UNMARKED <first line>" and
# the run dies with 3 — an unmarked check is never silently skipped.
: > "$TMP/blocks"
unmarked=$(awk -v out="$TMP/blocks" '
  !infence && /^# review-spec:check /    { marked = 1; id = $3; next }
  !infence && /^# review-spec:check-end/ { marked = 0; next }
  !infence && /^```sh/ { infence = 1; body = ""; first = ""; next }
  infence && /^```/ {
    infence = 0
    if (marked) {
      n++
      print "@@BLOCK@@ " id " " n > out
      printf "%s", body > out
      print "@@END@@" > out
    } else {
      print "UNMARKED " first
      exit 3
    }
    marked = 0
    next
  }
  infence { if (first == "") first = $0; body = body $0 "\n" }
' "$SPEC")
ext_status=$?
if [ "$ext_status" -ne 0 ]; then
  if [ -n "$unmarked" ]; then
    printf '%s\n' "$unmarked"
  fi
  exit "$ext_status"
fi

# block map: "<line of @@BLOCK@@ header in blocks file> <rule id>"
grep -n '^@@BLOCK@@ ' "$TMP/blocks" \
  | sed 's/^\([0-9]*\):@@BLOCK@@ \([^ ]*\) [0-9]*$/\1 \2/' > "$TMP/map"
[ -s "$TMP/map" ] || { echo "review-l0.sh: spec has no check-marked blocks" >&2; exit 2; }

block_body_at() { # <line of @@BLOCK@@ header> — the body below it
  awk -v start="$1" '
    NR > start && /^@@END@@$/ { exit }
    NR > start { print }
  ' "$TMP/blocks"
}

# ---- classify from the changed-file list (REVIEW.md §3) --------------------
cln=$(awk '$2 == "CLASSIFIER" { print $1; exit }' "$TMP/map")
[ -n "$cln" ] || { echo "review-l0.sh: spec has no CLASSIFIER check block" >&2; exit 2; }
block_body_at "$cln" > "$TMP/classify.sh"

if [ "$ARG_SHA" = "HEAD" ]; then
  git diff --name-only "$ARG_BASE" > "$TMP/files" 2>/dev/null || exit 2
else
  git diff --name-only "${ARG_BASE}...${ARG_SHA}" > "$TMP/files" 2>/dev/null || exit 2
fi

CLASSES=$(sh "$TMP/classify.sh" < "$TMP/files") || {
  echo "review-l0.sh: the REVIEW.md classifier block failed" >&2
  exit 2
}
printf '%s\n' "$CLASSES"

# ---- route (REVIEW.md §4) ---------------------------------------------------
# any → §5.0 + §5.5; docs → §5.1; skills → §5.1 + §5.2;
# code-plain → §5.3; code-risk → §5.3 + §5.4.
ROUTED=" U1 U2 U3 U4 U5 U6 U7 WIRING"
case "$CLASSES" in
  *docs*)   ROUTED="$ROUTED D1 D2 D3 D4 D5" ;;
esac
case "$CLASSES" in
  *skills*) ROUTED="$ROUTED D1 D2 D3 D4 D5 S1 S2 S3" ;;
esac
case "$CLASSES" in
  *code-plain*|*code-risk*) ROUTED="$ROUTED C1 C2 C3 C4 C5" ;;
esac
case "$CLASSES" in
  *code-risk*) ROUTED="$ROUTED R1 R2 R3 R4 R5" ;;
esac

# rule table: <id>|<routing class>|<severity> — severities per §5 headings
RULE_TABLE='U1|any|P0
U2|any|P1
U3|any|P1
U4|any|P1
U5|any|P1
U6|any|P0
U7|any|P1
D1|docs|P1
D2|docs|P1
D3|docs|P1
D4|docs|P0
D5|docs|judgement
S1|skills|P0
S2|skills|P1
S3|skills|P1
C1|code-plain|P0
C2|code-plain|P0
C3|code-plain|P1
C4|code-plain|P1
C5|code-plain|P1
R1|code-risk|P0
R2|code-risk|judgement
R3|code-risk|P0
R4|code-risk|P1
R5|code-risk|P1
WIRING|any|P1'

HAS_FAIL=0
HAS_ERROR=0

print_row() { # <rule> <class> <severity> <result> <evidence>
  printf '| %s | %s | %s | %s | %s |\n' "$1" "$2" "$3" "$4" "$5"
}

oneline() { # first line of stdin, pipes escaped for the table cell, capped
  head -n 1 | sed 's/|/\\|/g' | cut -c1-160
}

run_block() { # <rule> <class> <severity> <blocks-file line>
  rule=$1
  block_body_at "$4" > "$TMP/block.sh"

  # A block with a <placeholder> needs reviewer input (D2's decision key);
  # it cannot run mechanically and must say so rather than fail.
  if grep -qE '<[a-z][a-z0-9-]*>' "$TMP/block.sh"; then
    print_row "$rule" "$2" "$3" 'N.A.(needs reviewer input)' '-'
    return
  fi
  # Blocks referencing "$N" are PR-surface probes; without a number they
  # cannot run and must say so rather than fail.
  case "$(cat "$TMP/block.sh")" in
    *'$N'*)
      if [ -z "$N" ]; then
        print_row "$rule" "$2" "$3" 'N.A.(needs PR number)' '-'
        return
      fi
      ;;
  esac

  rc=0
  OUT=$(sh "$TMP/block.sh" 2>&1) || rc=$?

  # Finding signals carried by printed lines (REVIEW.md §0: a piped check
  # signals by its output; U5 and R3 print their exit, D1 prints CANDIDATE,
  # D3 prints MISSING).
  badline=$(printf '%s\n' "$OUT" | grep -E 'exit=[1-9][0-9]*$|CANDIDATE$|^MISSING' | head -n 1)

  # An enumerator-grep tail signals by its printed lines, not its exit:
  # nothing printed is a clean grep (exit 1); a "grep -c" tail prints a
  # count — data for the reviewer, never a finding signal.
  last_body=$(grep -v '^[[:space:]]*$' "$TMP/block.sh" | tail -n 1)
  enumerator=0
  case "$last_body" in
    *'| grep -'*|*'|grep -'*|grep\ -*) enumerator=1 ;;
  esac
  countgrep=0
  case "$last_body" in
    *'grep -c'*) countgrep=1 ;;
  esac
  # Rules whose spec states the inverse signal: empty output is the failure
  # (U3 — "Empty output is the failure", the missing `Issue: #N` line).
  inverted=0
  case "$1" in
    U3) inverted=1 ;;
  esac

  if [ -n "$badline" ]; then
    HAS_FAIL=1
    print_row "$rule" "$2" "$3" 'FAIL' "exit=$rc; $(printf '%s\n' "$badline" | oneline)"
    return
  fi
  if [ "$rc" -eq 0 ]; then
    if [ "$inverted" -eq 1 ] && [ -z "$OUT" ]; then
      HAS_FAIL=1
      print_row "$rule" "$2" "$3" 'FAIL' 'exit=0; (no output) — empty output is the failure (REVIEW.md U3)'
      return
    fi
    if [ "$enumerator" -eq 1 ] && [ -n "$OUT" ]; then
      HAS_FAIL=1
      print_row "$rule" "$2" "$3" 'FAIL' "exit=0; $(printf '%s\n' "$OUT" | oneline)"
    else
      if [ -n "$OUT" ]; then ev=$(printf '%s\n' "$OUT" | oneline); else ev="(no output)"; fi
      print_row "$rule" "$2" "$3" 'PASS' "exit=0; $ev"
    fi
    return
  fi
  if [ "$rc" -eq 127 ] || [ "$rc" -eq 128 ]; then
    HAS_ERROR=1
    if [ -n "$OUT" ]; then ev=$(printf '%s\n' "$OUT" | oneline); else ev="(no output)"; fi
    print_row "$rule" "$2" "$3" "ERROR $rc" "$ev"
    return
  fi
  if [ "$countgrep" -eq 1 ] && { [ "$rc" -eq 0 ] || [ "$rc" -eq 1 ]; }; then
    if [ -n "$OUT" ]; then ev=$(printf '%s\n' "$OUT" | oneline); else ev="(no output)"; fi
    print_row "$rule" "$2" "$3" 'PASS' "exit=$rc; $ev"
    return
  fi
  if [ "$enumerator" -eq 1 ]; then
    lastout=$(printf '%s\n' "$OUT" | grep -v '^[[:space:]]*$' | tail -n 1)
    if [ -z "$OUT" ] || [ "$lastout" = "0" ]; then
      print_row "$rule" "$2" "$3" 'PASS' "exit=$rc; (no output)"
      return
    fi
  fi
  HAS_FAIL=1
  if [ -n "$OUT" ]; then ev=$(printf '%s\n' "$OUT" | oneline); else ev="(no output)"; fi
  print_row "$rule" "$2" "$3" 'FAIL' "exit=$rc; $ev"
  return
}

process_rule() { # <rule> <class> <severity>
  case "$3" in
    judgement)
      print_row "$1" "$2" '[判斷]' '需升級' 'escalate per REVIEW.md §6; not adjudicated here'
      return ;;
  esac
  found=0
  for ln in $(awk -v want="$1" '$2 == want { print $1 }' "$TMP/map"); do
    found=1
    run_block "$1" "$2" "$3" "$ln"
  done
  if [ "$found" -eq 0 ]; then
    print_row "$1" "$2" "$3" 'N.A.(no command block in the spec)' '-'
  fi
}

printf '| Rule | Class | Severity | Result | Evidence |\n'
printf '|---|---|---|---|---|\n'
while IFS='|' read -r rid rclass rsev; do
  [ -n "$rid" ] || continue
  case "$ROUTED" in
    *" $rid "*) process_rule "$rid" "$rclass" "$rsev" ;;
  esac
done <<EOF
$RULE_TABLE
EOF

if [ "$HAS_FAIL" -eq 1 ]; then
  exit 1
fi
if [ "$HAS_ERROR" -eq 1 ]; then
  exit 2
fi
exit 0
