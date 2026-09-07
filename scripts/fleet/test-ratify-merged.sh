#!/bin/sh
# Offline self-test for scripts/fleet/ratify-merged.sh (GH-764).
#
# Stubs `edda` and records every invocation; `gh` is never reached because
# every case passes --body-file/--sha/--title. Writes only under its temp dir,
# so it is safe in CI and on a workstation with a live ledger.
#
# usage: sh scripts/fleet/test-ratify-merged.sh
set -eu

# The projection step below must never run against the real repo from a test.
EDDA_PROJECTION=off; export EDDA_PROJECTION

cd "$(git rev-parse --show-toplevel)"
hook=scripts/fleet/ratify-merged.sh

sh -n "$hook" || { echo "FAIL: sh -n $hook" >&2; exit 1; }
sh -n "$0" || { echo "FAIL: sh -n $0" >&2; exit 1; }

work=$(mktemp -d "${TMPDIR:-/tmp}/test-ratify-merged.XXXXXX")
cleanup() { rm -rf "$work"; }
trap cleanup 0 HUP INT TERM

mkdir -p "$work/bin" "$work/fixtures"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

SHA=03c604ffea4b2a1731b7866e7f701374eb03b156

# ── fixtures ─────────────────────────────────────────────────────────

cat >"$work/fixtures/one-key.md" <<'MD'
closes #764

Decision: ratify.evidence-form

Body prose that also says the word decision: nowhere near the line start.
MD

cat >"$work/fixtures/two-keys.md" <<'MD'
closes #764

Issue: #764
Decision: ratify.evidence-form, ratify.rule-form
MD

cat >"$work/fixtures/no-line.md" <<'MD'
closes #999

An ordinary PR that implements no decision at all.
Someone quoting "Decision: not.a.key" mid-sentence must not count.
MD

# ── stub edda: records args, and fails for one known key ─────────────

cat >"$work/bin/edda" <<'STUB'
#!/bin/sh
printf '%s\n' "$*" >>"$EDDA_CALLS"
for a in "$@"; do
    case $a in
        missing.key) echo "no active decision for key 'missing.key'" >&2; exit 1 ;;
        already.binding) echo "'already.binding' is already binding (by operator) — nothing written."; exit 0 ;;
    esac
done
echo "Ratified — now binding."
STUB
chmod +x "$work/bin/edda"
PATH="$work/bin:$PATH"
export PATH

# ── case 1: one Decision key → one typed evidence ratify ─────────────

EDDA_CALLS=$work/calls-1
export EDDA_CALLS
: >"$EDDA_CALLS"
out=$(sh "$hook" 764 --sha "$SHA" --title "feat(cli): typed ratify" \
    --body-file "$work/fixtures/one-key.md") || fail "case 1 exited non-zero"
grep -q "ratify.evidence-form -> exit 0" <<EOF || fail "case 1: missing log line: $out"
$out
EOF
calls=$(wc -l <"$EDDA_CALLS")
[ "$calls" -eq 1 ] || fail "case 1: expected 1 edda call, got $calls"
grep -qF "ratify ratify.evidence-form --evidence pr#764@$SHA --note feat(cli): typed ratify" \
    "$EDDA_CALLS" || fail "case 1: wrong edda args: $(cat "$EDDA_CALLS")"

# ── case 2: no Decision line → nothing ratified, still exit 0 ────────

EDDA_CALLS=$work/calls-2
export EDDA_CALLS
: >"$EDDA_CALLS"
out=$(sh "$hook" 999 --sha "$SHA" --body-file "$work/fixtures/no-line.md") \
    || fail "case 2 exited non-zero"
[ ! -s "$EDDA_CALLS" ] || fail "case 2: edda was called: $(cat "$EDDA_CALLS")"
case $out in
    *"no Decision: line"*) : ;;
    *) fail "case 2: expected a no-op message, got: $out" ;;
esac

# ── case 3: unknown key → edda exits 1, the hook still exits 0 ───────

EDDA_CALLS=$work/calls-3
export EDDA_CALLS
: >"$EDDA_CALLS"
printf 'Decision: missing.key\n' >"$work/fixtures/missing.md"
out=$(sh "$hook" 764 --sha "$SHA" --body-file "$work/fixtures/missing.md") \
    || fail "case 3: a failing ratify must not fail the hook"
case $out in
    *"missing.key -> exit 1"*) : ;;
    *) fail "case 3: exit code not reported: $out" ;;
esac

# ── case 4: already binding → exit 0, reported, no second write ──────

EDDA_CALLS=$work/calls-4
export EDDA_CALLS
: >"$EDDA_CALLS"
printf 'Decision: already.binding\n' >"$work/fixtures/already.md"
out=$(sh "$hook" 764 --sha "$SHA" --body-file "$work/fixtures/already.md") \
    || fail "case 4 exited non-zero"
case $out in
    *"already.binding -> exit 0"*) : ;;
    *) fail "case 4: expected exit 0 for an already-binding key: $out" ;;
esac

# ── case 5: two keys on one line → two ratifies ──────────────────────

EDDA_CALLS=$work/calls-5
export EDDA_CALLS
: >"$EDDA_CALLS"
sh "$hook" 764 --sha "$SHA" --body-file "$work/fixtures/two-keys.md" >/dev/null \
    || fail "case 5 exited non-zero"
calls=$(wc -l <"$EDDA_CALLS")
[ "$calls" -eq 2 ] || fail "case 5: expected 2 edda calls, got $calls"
grep -qF "ratify ratify.rule-form --evidence pr#764@$SHA" "$EDDA_CALLS" \
    || fail "case 5: second key not ratified: $(cat "$EDDA_CALLS")"

# ── case 6: a short SHA is refused before any write ──────────────────

EDDA_CALLS=$work/calls-6
export EDDA_CALLS
: >"$EDDA_CALLS"
if sh "$hook" 764 --sha 03c604f --body-file "$work/fixtures/one-key.md" >/dev/null 2>&1; then
    fail "case 6: an abbreviated SHA must be refused"
fi
[ ! -s "$EDDA_CALLS" ] || fail "case 6: edda ran despite a bad SHA"

# ── case 7: a non-numeric PR is a usage error (exit 2) ───────────────

set +e
sh "$hook" not-a-number --sha "$SHA" --body-file "$work/fixtures/one-key.md" >/dev/null 2>&1
code=$?
set -e
[ "$code" -eq 2 ] || fail "case 7: expected exit 2, got $code"

echo "PASS: $0"
