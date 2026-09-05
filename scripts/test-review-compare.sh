#!/bin/sh
# Offline fixtures for review-compare.sh (issue #887): the compare table and
# the for-ledger line on a shadow/authoritative round pair (one missed P0,
# one missed P1, one unconfirmed, two matches, one severity drift), the
# no-SHADOW exit, and the pending-authoritative exit. Everything is offline:
# comments come from EDDA_COMPARE_FIXTURE files holding the exact stream
# `gh pr view <pr> --json comments --jq '.comments[] | "<<<COMMENT>>>", .body'`
# prints, so no gh, no network, and no state outside the temp dir.
# Style follows scripts/test-pr-review-watch.sh — no new tooling.
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' 0 HUP INT TERM

COMPARE="$root/scripts/review-compare.sh"
SHA=0123456789abcdef0123456789abcdef01234567
OTHER=ffffffffffffffffffffffffffffffffffffffff

# --- helpers ------------------------------------------------------------------
expect_rc() { # name expected_rc actual_rc
    if [ "$2" = "$3" ]; then :; else
        printf '%s: expected exit %s, got %s\n' "$1" "$2" "$3" >&2
        return 1
    fi
}
expect_has() { # name needle file
    if grep -Fq -- "$2" "$3"; then :; else
        printf '%s: output missing: %s\n' "$1" "$2" >&2
        return 1
    fi
}
expect_lacks() { # name needle file
    if grep -Fq -- "$2" "$3"; then
        printf '%s: output must not contain: %s\n' "$1" "$2" >&2
        return 1
    fi
}

# --- fixture 1: shadow + two authoritative rounds + an unpinned decoy ---------
# Stream order is chronological, so "latest" is the last match: the decoy
# (other SHA) and the Round 1 authoritative comment must both be ignored.
F1="$tmp/mixed-stream.txt"
cat >"$F1" <<'EOF'
<<<COMMENT>>>
## Code Review: Round 1 — PR #887 @ ffffffffffffffffffffffffffffffffffffffff

- model_observed: decoy-engine

### Findings
- [P0] [U1] decoy finding — evidence: README.md:3

### Verdict
Changes Requested, P0=1, P1=0 — decoy on another SHA
<<<COMMENT>>>
## Code Review: Round 1 — PR #887 @ 0123456789abcdef0123456789abcdef01234567

- model_observed: old-engine

### Findings
- [P2] [D1] stale old claim — evidence: README.md:9

### Verdict
Changes Requested, P0=0, P1=0 — superseded authoritative round
<<<COMMENT>>>
## Code Review: Round 2 (SHADOW) — PR #887 @ 0123456789abcdef0123456789abcdef01234567

- model_requested: glm-5.3-flash
- model_observed: glm-5.3-flash
- shadow: true

### Findings
- [P1] [R3] changed script does not parse — evidence: scripts/x.sh:12
- [P1] [R1] destructive command without an enumerated trigger — evidence: scripts/y.sh:9
- [P2] [D5] wording — evidence: REVIEW.md:100
- [P1] wiring slot unfilled — evidence: docs/fleet/rules.md:7

### Verdict
Changes Requested, P0=0, P1=2 — shadow round, not a verdict
<<<COMMENT>>>
## Code Review: Round 3 — PR #887 @ 0123456789abcdef0123456789abcdef01234567

- model_observed: claude-opus-5

### Findings
- [P0] [U1] out-of-surface file — evidence: crates/edda-core/src/types.rs:42
- [P1] [D2] ledger claim stale — evidence: REVIEW.md:88
- [P1] [R3] changed script does not parse — evidence: scripts/x.sh:12
- [P2] [D5] wording — evidence: REVIEW.md:100
- [P0] [R1] destructive command without an enumerated trigger — evidence: scripts/y.sh:9

### Verdict
Changes Requested, P0=2, P1=1 — authoritative round
EOF

# --- fixture 2: authoritative rounds only — no SHADOW round -------------------
F2="$tmp/no-shadow.txt"
cat >"$F2" <<'EOF'
<<<COMMENT>>>
## Code Review: Round 1 — PR #887 @ 0123456789abcdef0123456789abcdef01234567

- model_observed: claude-opus-5

### Findings
- [P1] [U3] missing Issue line — evidence: PR body

### Verdict
Changes Requested, P0=0, P1=1 — authoritative only
EOF

# --- fixture 3: SHADOW round only — pending authoritative ---------------------
F3="$tmp/pending.txt"
cat >"$F3" <<'EOF'
<<<COMMENT>>>
## Code Review: Round 2 (SHADOW) — PR #887 @ 0123456789abcdef0123456789abcdef01234567

- model_observed: glm-5.3-flash
- shadow: true

### Findings
- [P0] [U1] out-of-surface file — evidence: crates/edda-core/src/types.rs:42
- [P1] doneWhen item has no code behind it

### Verdict
Changes Requested, P0=1, P1=1 — shadow round, not a verdict
EOF

# --- scenario 1: mixed rounds — the compare table + for-ledger line -----------
out="$tmp/s1.out"
rc=0
EDDA_COMPARE_FIXTURE="$F1" sh "$COMPARE" 887 "$SHA" >"$out" 2>&1 || rc=$?
expect_rc "mixed: exit" 0 "$rc"
expect_has "mixed: header" "shadow-review compare — PR 887 @ $SHA" "$out"
expect_has "mixed: shadow round" "shadow round: Round 2 (SHADOW) (model_observed: glm-5.3-flash)" "$out"
expect_has "mixed: latest authoritative" "authoritative round: Round 3 (model_observed: claude-opus-5)" "$out"
expect_has "mixed: missed P0 row"   "| missed | U1 | — | P0 |" "$out"
expect_has "mixed: missed P1 row"   "| missed | D2 | — | P1 |" "$out"
expect_has "mixed: unconfirmed row" "| unconfirmed | docs/fleet/rules.md:7 | P1 | — |" "$out"
expect_has "mixed: matched row 1"   "| matched | R3 | P1 | P1 |" "$out"
expect_has "mixed: matched row 2"   "| matched | D5 | P2 | P2 |" "$out"
expect_has "mixed: drift row"       "| severity drift | R1 | P1 | P0 |" "$out"
expect_has "mixed: counts" \
  "counts: missed_P0=1 missed_P1=1 unconfirmed=1 matched=2 drift=1" "$out"
expect_has "mixed: for-ledger" \
  "for-ledger: shadow=glm-5.3-flash authoritative=claude-opus-5 missed_P0=1 missed_P1=1 unconfirmed=1 matched=2 drift=1" "$out"
expect_lacks "mixed: other-SHA decoy absent" "decoy" "$out"
expect_lacks "mixed: other-SHA finding absent" "README.md:3" "$out"
expect_lacks "mixed: superseded round absent" "old-engine" "$out"
expect_lacks "mixed: superseded finding absent" "README.md:9" "$out"

# --- scenario 2: no SHADOW round pinned to the SHA — exit 2 with a message ----
out="$tmp/s2.out"
rc=0
EDDA_COMPARE_FIXTURE="$F2" sh "$COMPARE" 887 "$SHA" >"$out" 2>&1 || rc=$?
expect_rc "no-shadow: exit" 2 "$rc"
expect_has "no-shadow: message" "no SHADOW round pinned to PR 887 @ $SHA" "$out"

# --- scenario 3: no authoritative round — pending, exit 0, never a silent zero -
out="$tmp/s3.out"
rc=0
EDDA_COMPARE_FIXTURE="$F3" sh "$COMPARE" 887 "$SHA" >"$out" 2>&1 || rc=$?
expect_rc "pending: exit" 0 "$rc"
expect_has "pending: marker" "pending authoritative round" "$out"
expect_has "pending: finding with rule id" "| U1 | P0 |" "$out"
expect_has "pending: finding by text" "| doneWhen item has no code behind it | P1 |" "$out"
expect_lacks "pending: no for-ledger line" "for-ledger:" "$out"
expect_lacks "pending: no counts line" "counts:" "$out"

# --- usage and SHA validation (REVIEW.md R5) ----------------------------------
rc=0; sh "$COMPARE" >/dev/null 2>&1 || rc=$?
expect_rc "usage: no args exits 2" 2 "$rc"
rc=0; EDDA_COMPARE_FIXTURE="$F1" sh "$COMPARE" 887 0123456789abcdef >/dev/null 2>&1 || rc=$?
expect_rc "usage: short sha exits 2" 2 "$rc"
rc=0; EDDA_COMPARE_FIXTURE="$F1" sh "$COMPARE" not-a-number "$SHA" >/dev/null 2>&1 || rc=$?
expect_rc "usage: non-numeric pr exits 2" 2 "$rc"

# --- both new scripts parse ----------------------------------------------------
sh -n "$COMPARE"
sh -n "$root/scripts/test-review-compare.sh"

printf 'review-compare fixtures passed\n'
