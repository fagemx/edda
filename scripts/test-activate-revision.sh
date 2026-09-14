#!/bin/sh
# Offline self-test for the Job C activation route.
#
# Subjects:
#   scripts/activation/activate-revision.sh   (one normal activation route)
#   scripts/activation/manager-release.mjs    (agent-manager release writer)
#
# Everything runs under a disposable temp dir and mutates nothing outside it:
# no cargo, no npm, no install, no service start, no live manager root. Cases
# that touch the driver use a throwaway git repo whose layout mimics a checkout,
# so the real project and the operator's live service are never touched.
#
# usage: sh scripts/test-activate-revision.sh
set -eu

cd "$(git rev-parse --show-toplevel)"
driver=scripts/activation/activate-revision.sh
writer=scripts/activation/manager-release.mjs

sh -n "$driver" || { echo "FAIL: sh -n $driver" >&2; exit 1; }
sh -n "$0" || { echo "FAIL: sh -n $0" >&2; exit 1; }
node --check "$writer" || { echo "FAIL: node --check $writer" >&2; exit 1; }

work=$(mktemp -d "${TMPDIR:-/tmp}/test-activate-revision.XXXXXX")
cleanup() { rm -rf "$work"; }
trap cleanup 0 HUP INT TERM

fail() { echo "FAIL: $1" >&2; exit 1; }
pass() { echo "ok - $1"; }

# ── fixture checkout ──────────────────────────────────────────────────
fixture="$work/repo"
mkdir -p "$fixture/integrations/pi" "$fixture/integrations/agent-manager/dist/src"
printf '%s\n' 'export function activationReceipt() { return {}; }' >"$fixture/integrations/pi/activation-receipt.mjs"
printf '%s\n' '{}' >"$fixture/integrations/pi/package.json"
printf '%s\n' '// built cli placeholder' >"$fixture/integrations/agent-manager/dist/src/cli.js"
git -C "$fixture" init -q
git -C "$fixture" -c user.email=fixture@example.invalid -c user.name=fixture add -A
git -C "$fixture" -c user.email=fixture@example.invalid -c user.name=fixture commit -q -m "fixture"
fixture_rev=$(git -C "$fixture" rev-parse HEAD)
other_rev=0000000000000000000000000000000000000000

# ── driver: non-mutating surfaces ─────────────────────────────────────

if sh "$driver" --help >"$work/help.txt" 2>&1; then pass "driver --help exits 0"; else fail "driver --help"; fi
grep -q "activate-revision.sh" "$work/help.txt" || grep -q "Activate one merged revision" "$work/help.txt" \
  || fail "driver --help did not print usage"

if sh "$driver" --definitely-not-a-flag >"$work/unknown.txt" 2>&1; then
  fail "driver accepted an unknown flag"
else
  [ "$?" -eq 2 ] || fail "driver unknown flag exit was not 2"
  pass "driver rejects an unknown flag with exit 2"
fi

if sh "$driver" --repo "$work/does-not-exist" >"$work/norepo.txt" 2>&1; then
  fail "driver accepted a non-existent repo"
else
  [ "$?" -eq 2 ] || fail "driver non-existent repo exit was not 2"
  pass "driver refuses a non-existent repo"
fi

if sh "$driver" --repo "$fixture" --revision "not-a-sha" >"$work/badrev.txt" 2>&1; then
  fail "driver accepted a malformed revision"
else
  [ "$?" -eq 2 ] || fail "driver malformed revision exit was not 2"
  pass "driver refuses a malformed revision"
fi

if sh "$driver" --repo "$fixture" --revision "$other_rev" >"$work/wronghead.txt" 2>&1; then
  fail "driver activated a revision that is not HEAD"
else
  [ "$?" -eq 2 ] || fail "driver wrong-head exit was not 2"
  pass "driver refuses when HEAD is not the requested revision"
fi

# ── driver: dry run plans everything and mutates nothing ──────────────

before=$(git -C "$fixture" status --porcelain)
if sh "$driver" --repo "$fixture" --manager-root "$work/manager" --dry-run >"$work/dry.txt" 2>&1; then
  pass "driver --dry-run exits 0"
else
  cat "$work/dry.txt" >&2
  fail "driver --dry-run exited non-zero"
fi
grep -q "^plan: cargo install" "$work/dry.txt" || fail "dry run did not plan the shipping binary"
grep -q "^plan: npm pack" "$work/dry.txt" || fail "dry run did not plan the Pi package"
grep -q "manager-release" "$work/dry.txt" || fail "dry run did not plan the agent-manager step"
grep -q "dry run complete; nothing was changed" "$work/dry.txt" || fail "dry run did not report a no-op"
after=$(git -C "$fixture" status --porcelain)
[ "$before" = "$after" ] || fail "dry run modified the fixture checkout"
[ ! -e "$work/manager" ] || fail "dry run created a manager root"
pass "dry run mutates nothing"

# ── manager-release: dry run and refusal ──────────────────────────────

manager="$work/manager"
mkdir -p "$manager"
printf '%s\n' '{"version":1,"mergeCommit":"'"$fixture_rev"'","headSha":"'"$fixture_rev"'","sourceWorktree":"'"$fixture"'","entrypoint":"'"$fixture"'/integrations/agent-manager/dist/src/cli.js"}' >"$manager/release.json"
release_before=$(cat "$manager/release.json")

if node "$writer" --repo "$fixture" --root "$manager" --revision "$fixture_rev" --dry-run --json >"$work/writer-dry.json" 2>&1; then
  pass "manager-release --dry-run exits 0"
else
  cat "$work/writer-dry.json" >&2
  fail "manager-release --dry-run exited non-zero"
fi
node -e "const r=JSON.parse(require('fs').readFileSync(process.argv[1],'utf8'));if(r.status!=='dry_run')process.exit(1);if(!r.steps.includes('build')||!r.steps.includes('write-release')||!r.steps.includes('start'))process.exit(2)" "$work/writer-dry.json" \
  || fail "manager-release dry run did not plan build/write-release/start"
[ "$release_before" = "$(cat "$manager/release.json")" ] || fail "manager-release dry run changed release.json"
pass "manager-release dry run plans and mutates nothing"

# A live owner must not be stopped when --no-restart is requested. On Git Bash the
# shell's Windows PID lives in /proc/<pid>/winpid; on Linux $$ is the real PID.
live_pid=$(cat "/proc/$$/winpid" 2>/dev/null || printf '%s' "$$")
printf '%s\n' '{"version":1,"pid":'"$live_pid"',"instanceId":"fixture","origin":"http://127.0.0.1:4390","token":"fixture-token","startedAt":"2026-09-13T00:00:00.000Z"}' >"$manager/owner.json"
node "$writer" --repo "$fixture" --root "$manager" --revision "$fixture_rev" --dry-run --json >"$work/writer-restart.json" 2>&1 \
  || fail "manager-release --dry-run with a live owner exited non-zero"
node -e "const r=JSON.parse(require('fs').readFileSync(process.argv[1],'utf8'));if(!r.steps.includes('stop')||!r.steps.includes('start'))process.exit(1)" "$work/writer-restart.json" \
  || fail "manager-release default plan did not stop/start a live owner"
node "$writer" --repo "$fixture" --root "$manager" --revision "$fixture_rev" --dry-run --no-restart --json >"$work/writer-norestart.json" 2>&1 \
  || fail "manager-release --dry-run --no-restart exited non-zero"
node -e "const r=JSON.parse(require('fs').readFileSync(process.argv[1],'utf8'));if(r.steps.includes('stop')||r.steps.includes('recover')||r.steps.includes('start'))process.exit(1);if(!r.steps.includes('write-release'))process.exit(2)" "$work/writer-norestart.json" \
  || fail "manager-release --no-restart still planned to stop or start the service"
rm -f "$manager/owner.json"
pass "manager-release --no-restart never stops a live owner"

printf '%s\n' '{"version":1,"pid":"not-an-int"}' >"$manager/owner.json"
if node "$writer" --repo "$fixture" --root "$manager" --revision "$fixture_rev" >"$work/writer-invalid.txt" 2>&1; then
  fail "manager-release accepted an unusable owner.json"
else
  [ "$?" -eq 2 ] || fail "manager-release unusable-owner exit was not 2"
  pass "manager-release refuses an unusable owner.json"
fi
rm -f "$manager/owner.json"

if node "$writer" --repo "$fixture" --root "$manager" --revision "not-a-sha" >"$work/writer-badrev.txt" 2>&1; then
  fail "manager-release accepted a non-40-hex revision"
else
  [ "$?" -eq 2 ] || fail "manager-release bad revision exit was not 2"
  pass "manager-release refuses a non-40-hex revision"
fi

# ── Pi content identity depends on stable line endings ────────────────
# A CRLF checkout hashes differently from npm's shebang-normalized install, so
# the route cannot verify its own Pi step. `integrations/pi/**` is pinned to LF.
for f in integrations/pi/cli.mjs integrations/pi/package.json integrations/pi/private-directory.ps1; do
  attr=$(git check-attr eol -- "$f" | sed 's/.*: //')
  [ "$attr" = "lf" ] || fail "$f is not pinned to LF (git check-attr eol -> $attr)"
done
pass "integrations/pi files are pinned to LF"

# ── manager-release resolves npm to a spawnable command ───────────────
# `npm.cmd` cannot be spawned directly from Node on Windows (EINVAL).
node "$writer" --repo "$fixture" --root "$work/manager" --revision "$fixture_rev" --dry-run --json >"$work/writer-npm.json" 2>&1 \
  || fail "manager-release dry run failed while resolving npm"
node -e "const r=JSON.parse(require('fs').readFileSync(process.argv[1],'utf8'));const n=r.resolvedNpm;if(!n)process.exit(1);if(n.command==='npm.cmd'&&n.shell!==true)process.exit(2);if(process.platform==='win32'&&!/npm-cli\.js$/.test((n.prefix||[]).join(''))&&n.shell!==true)process.exit(3)" "$work/writer-npm.json" \
  || fail "manager-release would spawn an unspawnable npm command"
pass "manager-release resolves a spawnable npm command"

echo "PASS test-activate-revision"
