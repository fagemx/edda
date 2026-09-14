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
printf '%s\n' 'process.stdout.write(JSON.stringify({ pi: { installedReleaseId: null, repoReleaseId: null }, manager: { configured: null } }));' >"$fixture/integrations/pi/activation-receipt.mjs"
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
if sh "$driver" --repo "$fixture" --manager-root "$work/manager" --offline --allow-stale --dry-run >"$work/dry.txt" 2>&1; then
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

# ── Freshness and downgrade guards (#1217) ────────────────────────────
# A checkout whose origin/main is ahead, whose Pi content differs from what is
# installed, must be refused by default and only proceed with explicit opt-outs.
guarded="$work/guarded"
origin_bare="$work/origin.git"
# A controlled "installed" package so the downgrade guard is deterministic on
# hosts that have no global @edda/pi-session-channel (CI) as well as on a
# workstation that does.
installed_pi="$work/installed-pi"
mkdir -p "$installed_pi"
cp integrations/pi/*.mjs integrations/pi/*.ps1 integrations/pi/package.json integrations/pi/getting-started.md "$installed_pi/" 2>/dev/null || true
mkdir -p "$guarded"
git -C "$work" init -q --bare "$origin_bare"
cp -r integrations/pi "$guarded/integrations-pi"
mkdir -p "$guarded/integrations"
mv "$guarded/integrations-pi" "$guarded/integrations/pi"
# Make the checkout's Pi content differ from the installed package so the
# downgrade guard has something to catch.
printf '\n// stale-guard fixture marker\n' >> "$guarded/integrations/pi/cli.mjs"
git -C "$guarded" init -q
git -C "$guarded" -c user.email=fixture@example.invalid -c user.name=fixture add -A
git -C "$guarded" -c user.email=fixture@example.invalid -c user.name=fixture commit -q -m "guarded"
git -C "$guarded" branch -M main
git -C "$guarded" remote add origin "$origin_bare"
git -C "$guarded" push -q -u origin main
git -C "$origin_bare" symbolic-ref HEAD refs/heads/main
# Advance origin/main so the guarded checkout is behind it.
other="$work/other"
git clone -q "$origin_bare" "$other"
printf 'advanced\n' > "$other/ADVANCE.txt"
git -C "$other" -c user.email=fixture@example.invalid -c user.name=fixture add -A
git -C "$other" -c user.email=fixture@example.invalid -c user.name=fixture commit -q -m "advance"
git -C "$other" push -q origin main
git -C "$guarded" fetch -q origin main

if sh "$driver" --repo "$guarded" --manager-root "$work/none" --dry-run >"$work/stale.txt" 2>&1; then
  fail "route activated a checkout behind origin/main by default"
else
  [ "$?" -eq 2 ] || fail "stale-checkout refusal exit was not 2"
  grep -q "is not current origin/main" "$work/stale.txt" || fail "stale-checkout refusal did not name origin/main"
  pass "route refuses a checkout behind origin/main by default"
fi

if sh "$driver" --repo "$guarded" --manager-root "$work/none" --offline --dry-run >"$work/offline.txt" 2>&1; then
  fail "--offline alone skipped the freshness check"
else
  [ "$?" -eq 2 ] || fail "--offline refusal exit was not 2"
  grep -q "pass --allow-stale" "$work/offline.txt" || fail "--offline refusal did not point at --allow-stale"
  pass "--offline alone refuses without --allow-stale"
fi

if EDDA_PI_PACKAGE_ROOT="$installed_pi" sh "$driver" --repo "$guarded" --manager-root "$work/none" --offline --allow-stale --dry-run >"$work/downgrade.txt" 2>&1; then
  fail "--allow-stale overwrote differing installed Pi content without --allow-downgrade"
else
  [ "$?" -eq 2 ] || fail "downgrade refusal exit was not 2"
  grep -q "would overwrite installed Pi content" "$work/downgrade.txt" || fail "downgrade refusal did not name the installed content"
  pass "--allow-stale refuses to overwrite differing installed Pi content"
fi

before_guarded=$(git -C "$guarded" status --porcelain)
if EDDA_PI_PACKAGE_ROOT="$installed_pi" sh "$driver" --repo "$guarded" --manager-root "$work/none" --offline --allow-stale --allow-downgrade --dry-run >"$work/forced.txt" 2>&1; then
  pass "--allow-stale --allow-downgrade proceeds"
else
  cat "$work/forced.txt" >&2
  fail "explicit --allow-downgrade did not proceed"
fi
[ "$before_guarded" = "$(git -C "$guarded" status --porcelain)" ] || fail "forced dry run modified the guarded checkout"
[ ! -e "$work/none" ] || fail "forced dry run created a manager root"
pass "forced dry run mutates nothing"

# An unobservable installed identity must not read as "safe to overwrite".
mkdir -p "$work/empty-client"
if EDDA_PI_PACKAGE_ROOT="$work/empty-client" sh "$driver" --repo "$guarded" --manager-root "$work/none" --offline --allow-stale --dry-run >"$work/unobservable.txt" 2>&1; then
  fail "--allow-stale proceeded with an unobservable installed Pi identity"
else
  [ "$?" -eq 2 ] || fail "unobservable-identity refusal exit was not 2"
  grep -q "unobservable" "$work/unobservable.txt" || fail "unobservable refusal did not say so"
  pass "fails closed when installed Pi content cannot be observed"
fi

# A manager configured at a revision absent from the checkout cannot be proven
# older, so it must refuse rather than skip the comparison. This fixture's Pi
# content equals the controlled installed copy, so the Pi guard passes and the
# manager guard is isolated.
absent_commit=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
guarded2="$work/guarded2"
mkdir -p "$guarded2/integrations"
cp -r "$installed_pi" "$guarded2/integrations/pi"
git -C "$guarded2" init -q
git -C "$guarded2" -c user.email=fixture@example.invalid -c user.name=fixture add -A
git -C "$guarded2" -c user.email=fixture@example.invalid -c user.name=fixture commit -q -m "guarded2"
mgr_absent="$work/mgr-absent"
mkdir -p "$mgr_absent"
printf '{"version":1,"mergeCommit":"%s","headSha":"%s"}\n' "$absent_commit" "$absent_commit" >"$mgr_absent/release.json"
if EDDA_PI_PACKAGE_ROOT="$installed_pi" sh "$driver" --repo "$guarded2" --manager-root "$mgr_absent" --offline --allow-stale --dry-run >"$work/mgr-absent.txt" 2>&1; then
  fail "manager configured at an absent revision did not refuse"
else
  [ "$?" -eq 2 ] || fail "manager-absent refusal exit was not 2"
  grep -q "is not present in this checkout" "$work/mgr-absent.txt" || fail "manager-absent refusal did not explain itself"
  pass "refuses a manager configured at a revision absent from the checkout"
fi

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
