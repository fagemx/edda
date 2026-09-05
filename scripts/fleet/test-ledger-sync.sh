#!/usr/bin/env bash
# test-ledger-sync.sh — behavior test for ledger-sync.sh (GH-671).
#
# Runs the trigger against a throwaway git repository with a stubbed `edda`
# binary, so no production ledger, no real export, and no production
# Scheduled Task is ever registered (GH-671 constraint).
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# ── fixture: a git repo with unrelated WIP ──────────────────────────────
git init -q -b main "$tmp/repo"
git -C "$tmp/repo" -c user.name=test -c user.email=test@test commit -q --allow-empty -m init
printf 'work in progress\n' > "$tmp/repo/WIP.txt"

# ── stub `edda`: writes a mirror into --out ─────────────────────────────
mkdir -p "$tmp/bin"
cat > "$tmp/bin/edda" <<'STUB'
#!/usr/bin/env bash
out=""
prev=""
for arg in "$@"; do
    if [ "$prev" = "--out" ]; then out="$arg"; fi
    prev="$arg"
done
mkdir -p "$out/decisions"
printf '<!-- generated -->\n# Domain: `fleet`\n' > "$out/decisions/fleet.md"
printf -- '- **Exported at**: 2026-09-05T00:00:00Z\n- **Exporting machine**: stub\n' > "$out/INDEX.md"
STUB
chmod +x "$tmp/bin/edda"

fail() { echo "test-ledger-sync: FAIL: $1" >&2; exit 1; }

# ── run 1: export + commit ──────────────────────────────────────────────
(cd "$tmp/repo" && PATH="$tmp/bin:$PATH" bash "$root/scripts/fleet/ledger-sync.sh") \
    > "$tmp/run1.log" 2>&1 || fail "run 1 exited non-zero: $(cat "$tmp/run1.log")"

[ -f "$tmp/repo/docs/ledger/INDEX.md" ] || fail "INDEX.md not exported"
[ -f "$tmp/repo/docs/ledger/decisions/fleet.md" ] || fail "decisions/fleet.md not exported"

cd "$tmp/repo"
git log -1 --format=%s | grep -q "chore(ledger): refresh committed mirror" \
    || fail "mirror commit missing: $(git log --oneline -1)"
changed=$(git show --name-only --format= HEAD)
echo "$changed" | grep -q "docs/ledger/INDEX.md" || fail "commit missing INDEX.md"
echo "$changed" | grep -q "WIP.txt" && fail "commit swept unrelated WIP"

# ── run 2: unchanged mirror is a no-op ──────────────────────────────────
before=$(git rev-parse HEAD)
(cd "$tmp/repo" && PATH="$tmp/bin:$PATH" bash "$root/scripts/fleet/ledger-sync.sh") \
    > "$tmp/run2.log" 2>&1 || fail "run 2 exited non-zero: $(cat "$tmp/run2.log")"
grep -q "mirror unchanged" "$tmp/run2.log" || fail "run 2 did not report unchanged"
[ "$(git rev-parse HEAD)" = "$before" ] || fail "unchanged mirror must not create a commit"

echo "test-ledger-sync: OK (export, path-limited commit, WIP untouched, no-op rerun)"
