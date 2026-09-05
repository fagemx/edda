#!/bin/sh
# Offline self-test for scripts/review-compare.sh (GH-887).
# Stubs gh. Writes only under a temp dir.
set -eu

cd "$(git rev-parse --show-toplevel)"
script=scripts/review-compare.sh

sh -n "$script" || {
    echo "FAIL: sh -n $script" >&2
    exit 1
}
sh -n "$0" || {
    echo "FAIL: sh -n $0" >&2
    exit 1
}

work=$(mktemp -d "${TMPDIR:-/tmp}/test-review-compare.XXXXXX")
trap 'rm -rf "$work"' EXIT
export TMPDIR="$work"
mkdir -p "$work/bin" "$work/fixtures" "$work/out"

SHA=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa

cat >"$work/bin/gh" <<STUB
#!/bin/sh
# the compare script's one query: expand comments the way the real gh's
# --jq does
jq -r '.comments[] | "<<<COMMENT>>>", .body' "\$GH_COMMENTS_FIXTURE"
STUB
chmod +x "$work/bin/gh"

# 1 missed P0 (scripts/d.sh:40), 1 missed P1 (scripts/e.sh:50),
# 1 unconfirmed (scripts/c.sh:30), 2 matched (scripts/a.sh:10, scripts/f.sh:60),
# 1 severity drift (scripts/b.sh:20 — P1 authoritative vs P2 shadow).
cat >"$work/fixtures/both.json" <<EOF
{"comments": [{"body": "## Code Review: Round 1 — PR #899 @ $SHA (SHADOW)\n\nshadow: true\n\n- model_observed: openrouter/z-ai/glm-5.3-flash\n\n### Findings\n- [P1] matched defect — evidence: scripts/a.sh:10\n- [P2] drifted severity — evidence: scripts/b.sh:20\n- [P2] shadow-only suspicion — evidence: scripts/c.sh:30\n- [P2] second matched pair — evidence: scripts/f.sh:60\n\n### Verdict\nChanges Requested, P0=0, P1=1"}, {"body": "## Code Review: Round 2 — PR #899 @ $SHA\n\n- model_observed: claude-opus-5\n\n### Findings\n- [P1] matched defect — evidence: scripts/a.sh:10\n- [P1] drifted severity — evidence: scripts/b.sh:20\n- [P0] catastrophic miss — evidence: scripts/d.sh:40\n- [P2] second matched pair — evidence: scripts/f.sh:60\n- [P1] real blocker the shadow missed — evidence: scripts/e.sh:50\n\n### Verdict\nLGTM (P0=0, P1=0)"}]}
EOF
# Normalise the markers the stub emits (the fixture embeds its own comment
# framing because gh --jq prints one body per <<<COMMENT>>> line).
sed -i 's/<<<CS>>>/<<<COMMENT>>>/; s/<<<CE>>>//' "$work/fixtures/both.json"
cat >"$work/fixtures/shadow-only.json" <<EOF
{"comments":[
{"body":"## Code Review: Round 1 — PR #899 @ $SHA (SHADOW)\n\nshadow: true\n\n- model_observed: openrouter/z-ai/glm-5.3-flash\n\n### Findings\n- [P1] shadow-only suspicion — evidence: scripts/c.sh:30\n\n### Verdict\nChanges Requested, P0=0, P1=1\n<<<CE>>>"}]}
EOF
sed -i 's/<<<CE>>>//' "$work/fixtures/shadow-only.json"
cat >"$work/fixtures/no-shadow.json" <<EOF
{"comments":[
{"body":"## Code Review: Round 1 — PR #899 @ $SHA\n\n- model_observed: claude-opus-5\n\n### Findings\n- [P0] authoritative only — evidence: scripts/d.sh:40\n\n### Verdict\nChanges Requested, P0=1, P1=0\n<<<CE>>>"}]}
EOF
sed -i 's/<<<CE>>>//' "$work/fixtures/no-shadow.json"
run() {
GH_COMMENTS_FIXTURE="$1" PATH="$work/bin:$PATH" sh "$script" 899 "$SHA"
}
# 1. both rounds: the counts and the for-ledger line
run "$work/fixtures/both.json" >"$work/out/both.txt" 2>"$work/out/both.err"
grep -q 'missed.*\[P0\] catastrophic miss' "$work/out/both.txt" || {
echo "FAIL: missed P0 not reported: $(cat "$work/out/both.txt")" >&2
exit 1
}
grep -q 'missed.*\[P1\] real blocker' "$work/out/both.txt" || {
echo "FAIL: missed P1 not reported" >&2
exit 1
}
grep -q 'unconfirmed.*shadow-only suspicion' "$work/out/both.txt" || {
echo "FAIL: unconfirmed not reported" >&2
exit 1
}
grep -q 'severity drift' "$work/out/both.txt" || {
echo "FAIL: severity drift not reported" >&2
exit 1
}
line=$(grep '^for-ledger:' "$work/out/both.txt")
printf '%s\n' "$line" | grep -q 'missed_P0=1' || echo "FAIL: for-ledger missed_P0=1 missing: $line" >&2
printf '%s\n' "$line" | grep -q 'missed_P1=1' || echo "FAIL: for-ledger missed_P1=1 missing: $line" >&2
printf '%s\n' "$line" | grep -q 'unconfirmed=1' || echo "FAIL: for-ledger unconfirmed=1 missing: $line" >&2
printf '%s\n' "$line" | grep -q 'matched=2' || echo "FAIL: for-ledger matched=2 missing: $line" >&2
printf '%s\n' "$line" | grep -q 'drift=1' || echo "FAIL: for-ledger drift=1 missing: $line" >&2
printf '%s\n' "$line" | grep -q 'shadow=openrouter/z-ai/glm-5.3-flash' || echo "FAIL: shadow model missing: $line" >&2
printf '%s\n' "$line" | grep -q 'authoritative=claude-opus-5' || echo "FAIL: authoritative model missing: $line" >&2
echo "ok 1 both rounds: counts and for-ledger line"
# 2. no SHADOW round on the sha → exit 2, naming it
rc=0
GH_COMMENTS_FIXTURE="$work/fixtures/no-shadow.json" PATH="$work/bin:$PATH" \
sh "$script" 899 "$SHA" >"$work/out/noshadow.txt" 2>"$work/out/noshadow.err" || rc=$?
[ "$rc" -eq 2 ] || {
echo "FAIL: no-shadow exit $rc, want 2" >&2
exit 1
}
grep -q 'no SHADOW round' "$work/out/noshadow.err" || {
echo "FAIL: no-shadow stderr must name the missing round: $(cat "$work/out/noshadow.err")" >&2
exit 1
}
echo "ok 2 no SHADOW round exits 2"
# 3. shadow round but no authoritative round → pending, exit 0
rc=0
GH_COMMENTS_FIXTURE="$work/fixtures/shadow-only.json" PATH="$work/bin:$PATH" \
sh "$script" 899 "$SHA" >"$work/out/pending.txt" 2>"$work/out/pending.err" || rc=$?
[ "$rc" -eq 0 ] || {
echo "FAIL: pending exit $rc, want 0" >&2
exit 1
}
grep -q 'pending authoritative round' "$work/out/pending.txt" || {
echo "FAIL: pending line missing: $(cat "$work/out/pending.txt")" >&2
exit 1
}
grep -q 'authoritative=pending' "$work/out/pending.txt" || {
echo "FAIL: pending for-ledger line missing" >&2
exit 1
}
echo "ok 3 pending authoritative round exits 0"
echo "PASS: scripts/test-review-compare.sh"
