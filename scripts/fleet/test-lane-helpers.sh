#!/bin/sh
# Self-test for scripts/fleet/lane-prepare.ps1 and scripts/fleet/lane-warm.ps1
# (GH-626). Offline fixtures only: throwaway repos with a local bare "origin",
# a temp FLEET_LANE_ROOT, stub rustc/cargo on PATH (no real Cargo run), and one
# real scheduled task (the established pattern of scripts/test-git-config-guard.sh)
# to prove the busy gate against a genuinely Running edda-lane-* task.
#
# Contract pinned here:
#   * prepare creates the FIXED lane worktree path
#     <main-repo-parent>\<repo>-wt-<lane> and a new branch from origin/main,
#     and is idempotent for the same branch,
#   * prepare refuses: dirty worktree, unpushed previous branch (upstream
#     configuration alone is not accepted — the remote TIP must match), a
#     running or unreadable lane task state, an unpublished detached commit,
#     an existing branch, a path collision, a bad lane/branch name — always
#     loudly, never force-checkout, never deletion,
#   * warm refuses: worktree not prepared, dirty, unpushed, unpublished
#     detached work, busy or unreadable Task Scheduler state, rust-lld absent
#     from the active toolchain — and never starts cargo in those cases,
#   * warm -DryRun and prepare -DryRun have ZERO side effects (no fetch, no
#     refs, no checkout, no cargo) and print the exact commands,
#   * env contract: CARGO_TARGET_DIR=<lane-root>\<lane> (fixed lane dir, never
#     per-SHA), CARGO_INCREMENTAL=1 for workers / 0 for verifiers,
#     CARGO_PROFILE_DEV_DEBUG and CARGO_PROFILE_TEST_DEBUG = line-tables-only;
#     the linker comes from the shipped .cargo/config.toml, never a guessed
#     machine path,
#   * lane-warm -PrintEnv exposes the same env for the lane-launch integration;
#     lane-launch -DryRun proves the wrapper actually carries it (with
#     -BuildLane: the full contract; without: no CARGO_* env at all),
#
# Style follows scripts/test-git-config-guard.sh — no new tooling.
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
PREPARE="$root/scripts/fleet/lane-prepare.ps1"
WARM="$root/scripts/fleet/lane-warm.ps1"

command -v pwsh >/dev/null 2>&1 || { echo "SKIP: pwsh not on PATH"; exit 0; }
command -v git >/dev/null 2>&1 || { echo "SKIP: git not on PATH"; exit 0; }

tmp=$(mktemp -d)
BUSY_TASK='edda-lane-busytest'

cleanup() {
  pwsh -NoProfile -NonInteractive -Command "
    Stop-ScheduledTask -TaskName '$BUSY_TASK' -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName '$BUSY_TASK' -Confirm:\$false -ErrorAction SilentlyContinue
    foreach (\$n in @('gh626envcheck', 'gh626noenv')) {
      Unregister-ScheduledTask -TaskName ('edda-lane-' + \$n) -Confirm:\$false -ErrorAction SilentlyContinue
    }
    foreach (\$h in @(Get-CimInstance Win32_Process | Where-Object { \$_.CommandLine -and \$_.CommandLine.Contains('lane-helper-busy.ps1') })) {
      & taskkill /PID \$h.ProcessId /T /F 2>\$null | Out-Null
    }
  " >/dev/null 2>&1 || true
  rm -rf "$tmp"
}
trap cleanup EXIT

case_number=0
fail() { echo "FAIL (after case $case_number): $*" >&2; exit 1; }
ok() { case_number=$((case_number + 1)); echo "ok $case_number - $*"; }

prepare() { pwsh -NoProfile -NonInteractive -File "$PREPARE" "$@"; }
warm()    { pwsh -NoProfile -NonInteractive -File "$WARM" "$@"; }

# A throwaway repo with one commit on main, pushed to a local bare origin.
new_repo() {
  name=$1
  origin="$tmp/$name-origin.git"
  repo="$tmp/$name"
  git init -q --bare "$origin"
  git init -q "$repo"
  git -C "$repo" config user.email t@example.com
  git -C "$repo" config user.name tester
  git -C "$repo" config commit.gpgsign false
  echo hello >"$repo/f.txt"
  git -C "$repo" add f.txt
  git -C "$repo" commit -qm init
  git -C "$repo" branch -M main 2>/dev/null || true
  git -C "$repo" remote add origin "$origin"
  git -C "$repo" push -q origin main
  printf '%s' "$repo"
}

# The FIXED lane worktree path the helpers must derive (POSIX form for git,
# Windows form for output comparison).
wt_posix() { printf '%s' "$1-wt-$2"; }
wt_win()   { cygpath -m "$1-wt-$2"; }

refs_snapshot() { git -C "$1" for-each-ref | sed 's/ $//'; }

# Register a genuinely Running scheduled task whose WorkingDirectory is the
# given directory — the shape lane-launch.ps1 produces for a live lane.
register_busy_task() {
  wdir=$(cygpath -w "$1")
  taskscript="$tmp/lane-helper-busy.ps1"
  printf 'Start-Sleep -Seconds 180\n' >"$taskscript"
  wscript=$(cygpath -w "$taskscript")
  pwsh -NoProfile -NonInteractive -Command "
    \$action = New-ScheduledTaskAction -Execute (Get-Command pwsh.exe).Source \`
      -Argument \"-NoProfile -NonInteractive -File $wscript\" -WorkingDirectory '$wdir'
    \$settings = New-ScheduledTaskSettingsSet \`
      -ExecutionTimeLimit (New-TimeSpan -Seconds 120) \`
      -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
    Unregister-ScheduledTask -TaskName '$BUSY_TASK' -Confirm:\$false -ErrorAction SilentlyContinue
    Register-ScheduledTask -TaskName '$BUSY_TASK' -Action \$action -Settings \$settings -RunLevel Limited | Out-Null
    Start-ScheduledTask -TaskName '$BUSY_TASK'
    Start-Sleep -Seconds 2
    (Get-ScheduledTask -TaskName '$BUSY_TASK').State
  "
}

# Stub toolchain: rustc prints a fake sysroot; cargo records its argv and the
# build env into $FIXTURE_RECORD and exits 0. present=yes|no controls whether
# the fake sysroot contains rust-lld.exe.
make_stub_bin() {
  dir=$1; present=$2
  mkdir -p "$dir"
  sysroot="$tmp/stub-sysroot-$present"
  rm -rf "$sysroot"; mkdir -p "$sysroot"
  if [ "$present" = yes ]; then
    mkdir -p "$sysroot/lib/rustlib/x86_64-pc-windows-msvc/bin"
    : >"$sysroot/lib/rustlib/x86_64-pc-windows-msvc/bin/rust-lld.exe"
  fi
  wsysroot=$(cygpath -w "$sysroot")
  printf '@echo off\r\necho %s\r\n' "$wsysroot" >"$dir/rustc.cmd"
  {
    printf '@echo off\r\n'
    printf 'if not "%%FIXTURE_RECORD%%"=="" (\r\n'
    printf '  echo cargo %%* >> "%%FIXTURE_RECORD%%"\r\n'
    printf '  echo TDIR=%%CARGO_TARGET_DIR%% INCR=%%CARGO_INCREMENTAL%% DEV=%%CARGO_PROFILE_DEV_DEBUG%% TEST=%%CARGO_PROFILE_TEST_DEBUG%% >> "%%FIXTURE_RECORD%%"\r\n'
    printf ')\r\n'
    printf 'exit /b 0\r\n'
  } >"$dir/cargo.cmd"
}

expect_ok() {
  desc=$1; shift
  if ! out=$("$@" 2>&1); then fail "$desc: expected success :: $out"; fi
}

expect_fail() {
  desc=$1; want=$2; shift 2
  if out=$("$@" 2>&1); then fail "$desc: expected failure, got success :: $out"; fi
  case "$out" in
    *"$want"*) : ;;
    *) fail "$desc: output missing '$want' :: $out" ;;
  esac
}

# Run a helper under a deliberately failing Task Scheduler query. A function
# defined in the pwsh caller shadows the cmdlet in the child script scope, so
# this covers the actual fail-closed catch rather than a textual mock.
scheduler_fail_prepare() {
  repo_win=$(cygpath -w "$1")
  prepare_win=$(cygpath -w "$PREPARE")
  pwsh -NoProfile -NonInteractive -Command "
    function Get-ScheduledTask { [CmdletBinding()] param([string]\$TaskName); throw 'fixture scheduler query failed' }
    & '$prepare_win' -BuildLane worker-1 -Branch codex/scheduler-fail -Repo '$repo_win' -DryRun
    exit \$LASTEXITCODE
  "
}

scheduler_fail_warm() {
  repo_win=$(cygpath -w "$1")
  warm_win=$(cygpath -w "$WARM")
  pwsh -NoProfile -NonInteractive -Command "
    function Get-ScheduledTask { [CmdletBinding()] param([string]\$TaskName); throw 'fixture scheduler query failed' }
    & '$warm_win' -BuildLane worker-1 -Repo '$repo_win' -DryRun
    exit \$LASTEXITCODE
  "
}

# ---------------------------------------------------------------------------
# lane-prepare.ps1

repo=$(new_repo prep1)
lane=$(wt_posix "$repo" worker-1)
lanewin=$(wt_win "$repo" worker-1)

expect_ok "prepare happy path" prepare -BuildLane worker-1 -Branch codex/gh100-a -Repo "$repo"
[ -e "$lane/.git" ] || fail "prepare: lane worktree not created at $lane"
cur=$(git -C "$lane" symbolic-ref --quiet HEAD)
[ "$cur" = "refs/heads/codex/gh100-a" ] || fail "prepare: branch is $cur, expected refs/heads/codex/gh100-a"
base=$(git -C "$repo" rev-parse origin/main)
[ "$(git -C "$lane" rev-parse HEAD)" = "$base" ] || fail "prepare: tip is not origin/main"
git -C "$repo" worktree list --porcelain | grep -F "$(cygpath -m "$lane")" >/dev/null \
  || fail "prepare: worktree not registered with the repo"
ok "prepare creates fixed worktree $lanewin on a new branch from origin/main"

expect_ok "prepare idempotent same branch" prepare -BuildLane worker-1 -Branch codex/gh100-a -Repo "$repo"
[ "$(git -C "$lane" symbolic-ref --quiet HEAD)" = "refs/heads/codex/gh100-a" ] \
  || fail "idempotent prepare moved the worktree"
ok "prepare re-run on the same branch is a no-op"

# Previous branch with an extra commit, upstream CONFIGURED but remote tip absent.
git -C "$lane" commit -q --allow-empty -m extra
git -C "$lane" branch -u origin/main codex/gh100-a
before_head=$(git -C "$lane" rev-parse HEAD)
expect_fail "prepare unpushed" "remote tip" prepare -BuildLane worker-1 -Branch codex/gh101-b -Repo "$repo"
[ "$(git -C "$lane" rev-parse HEAD)" = "$before_head" ] || fail "refused prepare still moved HEAD"
ok "prepare refuses to switch away from an unpushed branch (upstream config not trusted)"

git -C "$lane" push -q origin codex/gh100-a
expect_ok "prepare switch after push" prepare -BuildLane worker-1 -Branch codex/gh101-b -Repo "$repo"
[ "$(git -C "$lane" symbolic-ref --quiet HEAD)" = "refs/heads/codex/gh101-b" ] \
  || fail "prepare did not switch to the new branch"
ok "prepare switches the fixed worktree once the previous branch is pushed"

echo dirty >"$lane/dirty.txt"
expect_fail "prepare dirty" "dirty" prepare -BuildLane worker-1 -Branch codex/gh102-c -Repo "$repo"
[ "$(git -C "$lane" symbolic-ref --quiet HEAD)" = "refs/heads/codex/gh101-b" ] \
  || fail "refused dirty prepare moved HEAD"
rm "$lane/dirty.txt"
ok "prepare refuses a dirty lane worktree"

git -C "$lane" push -q origin codex/gh101-b

expect_fail "prepare existing branch" "already exists" prepare -BuildLane worker-1 -Branch main -Repo "$repo"
ok "prepare refuses an existing branch instead of reusing or deleting it"

# A branch that already exists with NO lane worktree must be refused too:
# branches are never reused, moved, or deleted.
git -C "$repo" branch codex/gh108-nowt
expect_fail "prepare existing branch missing worktree" "already exists but the lane worktree" \
  prepare -BuildLane verifier -Branch codex/gh108-nowt -Repo "$repo"
[ ! -d "$(wt_posix "$repo" verifier)" ] || fail "prepare created a worktree for an existing branch"
ok "prepare refuses an existing branch whose lane worktree does not exist"

# Genuine collision: a FILE at the exact FIXED lane worktree path. (The
# original fixture used a sibling '<lane>-collision' path, which collides with
# nothing — every gate passed and prepare actually switched branches, so the
# case proved nothing: RED 2026-09-04, test setup fixed, guard unchanged.)
crepo=$(new_repo prep2)
printf x >"$(wt_posix "$crepo" verifier-2)"
expect_fail "prepare collision" "not a directory" prepare -BuildLane verifier-2 -Branch codex/gh103-d -Repo "$crepo"
[ "$(cat "$(wt_posix "$crepo" verifier-2)")" = "x" ] || fail "prepare overwrote the colliding file"
[ ! -d "$(wt_posix "$crepo" verifier-2)" ] || fail "prepare replaced the colliding file with a worktree"
ok "prepare refuses a file at the fixed lane worktree path (no overwrite)"

expect_fail "prepare bad lane" "allowed" prepare -BuildLane worker-3 -Branch codex/x -Repo "$repo"
expect_fail "prepare bad branch" "ref" prepare -BuildLane worker-1 -Branch '../evil' -Repo "$repo"
ok "prepare validates lane and branch names"

# Busy gate: a genuinely Running task bound to the lane worktree path.
git -C "$repo" worktree add -q -b codex/gh104-busy "$(wt_posix "$repo" worker-2)" origin/main
state=$(register_busy_task "$(wt_posix "$repo" worker-2)")
[ "$state" = "Running" ] || fail "busy fixture task did not reach Running (state=$state)"
expect_fail "prepare busy" "busy" prepare -BuildLane worker-2 -Branch codex/gh105-e -Repo "$repo"
[ "$(git -C "$(wt_posix "$repo" worker-2)" symbolic-ref --quiet HEAD)" = "refs/heads/codex/gh104-busy" ] \
  || fail "busy refusal still switched the worktree"
ok "prepare refuses a lane whose scheduled task is Running on the worktree"

# Dry-run: exact commands, zero side effects — both arms. A fresh lane (no
# worktree) prints `worktree add`; a prepared lane prints `checkout -b`. (The
# original case asserted the checkout arm against a fresh lane, whose dry-run
# correctly prints worktree add — a second latent assertion bug, never reached
# before the collision case was fixed.)
before_refs=$(refs_snapshot "$repo")
before_wts=$(git -C "$repo" worktree list --porcelain)
expect_ok "prepare dry-run fresh" prepare -BuildLane verifier -Branch codex/gh106-f -Repo "$repo" -DryRun
out=$(prepare -BuildLane verifier -Branch codex/gh106-f -Repo "$repo" -DryRun 2>&1)
case "$out" in *worktree\ add*) : ;; *) fail "dry-run does not show the worktree add command :: $out" ;; esac
[ ! -d "$(wt_posix "$repo" verifier)" ] || fail "dry-run created the worktree"
[ "$(refs_snapshot "$repo")" = "$before_refs" ] || fail "dry-run mutated refs"
[ "$(git -C "$repo" worktree list --porcelain)" = "$before_wts" ] || fail "dry-run registered a worktree"
ok "prepare -DryRun (fresh lane) prints the exact worktree-add command and mutates nothing"

expect_ok "prepare fixture switch" prepare -BuildLane verifier -Branch codex/gh106-f -Repo "$repo"
[ "$(git -C "$(wt_posix "$repo" verifier)" symbolic-ref --quiet HEAD)" = "refs/heads/codex/gh106-f" ] \
  || fail "fixture: verifier worktree not on codex/gh106-f"
git -C "$(wt_posix "$repo" verifier)" push -q origin codex/gh106-f  # fixture: the unpushed gate must not fire
before_refs=$(refs_snapshot "$repo")
out=$(prepare -BuildLane verifier -Branch codex/gh109-h -Repo "$repo" -DryRun 2>&1)
case "$out" in *"checkout -b"*) : ;; *) fail "dry-run does not show the checkout command :: $out" ;; esac
[ "$(refs_snapshot "$repo")" = "$before_refs" ] || fail "dry-run mutated refs"
[ "$(git -C "$(wt_posix "$repo" verifier)" symbolic-ref --quiet HEAD)" = "refs/heads/codex/gh106-f" ] \
  || fail "dry-run switched the prepared lane worktree"
ok "prepare -DryRun (prepared lane) prints the exact checkout command and mutates nothing"

echo dirty >"$(wt_posix "$repo" worker-1)/dirty.txt"
before_refs=$(refs_snapshot "$repo")
expect_fail "prepare dry-run dirty" "dirty" prepare -BuildLane worker-1 -Branch codex/gh107-g -Repo "$repo" -DryRun
[ "$(refs_snapshot "$repo")" = "$before_refs" ] || fail "dry-run refusal mutated refs"
rm "$(wt_posix "$repo" worker-1)/dirty.txt"
ok "prepare -DryRun on a dirty lane reports the refusal and mutates nothing"

# A clean detached HEAD still needs a durable remote ref. The warmer normally
# creates detached main checkouts, but a unique commit in that state must not
# be stranded in a reflog by prepare or warm.
detrepo=$(new_repo detached)
detlane=$(wt_posix "$detrepo" worker-1)
git -C "$detrepo" worktree add -q --detach "$detlane" origin/main
echo detached >"$detlane/f.txt"
git -C "$detlane" add f.txt
git -C "$detlane" commit -qm detached-unique
dethead=$(git -C "$detlane" rev-parse HEAD)
expect_fail "prepare detached unique" "not reachable" \
  prepare -BuildLane worker-1 -Branch codex/detached-next -Repo "$detrepo" -DryRun
expect_fail "warm detached unique" "not reachable" warm -BuildLane worker-1 -Repo "$detrepo" -DryRun
[ "$(git -C "$detlane" rev-parse HEAD)" = "$dethead" ] || fail "detached refusal moved HEAD"
[ "$(git -C "$detrepo" for-each-ref --contains "$dethead" --format='%(refname)' refs/remotes/origin/)" = "" ] \
  || fail "detached fixture accidentally gained a remote containing ref"
ok "prepare and warm refuse a clean detached commit without an origin ref"

# A scheduler query failure cannot be treated as an empty task list. These
# helpers must fail before a dry-run could report a switch or warm command.
srepo=$(new_repo scheduler)
expect_fail "prepare scheduler query failure" "cannot query Task Scheduler" scheduler_fail_prepare "$srepo"
expect_ok "scheduler warm fixture prepare" prepare -BuildLane worker-1 -Branch codex/scheduler-base -Repo "$srepo"
git -C "$(wt_posix "$srepo" worker-1)" push -q origin codex/scheduler-base
expect_fail "warm scheduler query failure" "cannot query Task Scheduler" scheduler_fail_warm "$srepo"
ok "prepare and warm fail closed when Task Scheduler cannot be queried"

# ---------------------------------------------------------------------------
# lane-warm.ps1

wrepo=$(new_repo warm1)
wwt=$(wt_posix "$wrepo" worker-1)
expect_fail "warm unprepared" "lane-prepare" warm -BuildLane worker-1 -Repo "$wrepo" -Warm
ok "warm refuses a lane whose worktree was never prepared"

expect_ok "warm fixture prepare" prepare -BuildLane worker-1 -Branch codex/gh200-a -Repo "$wrepo"
git -C "$wwt" commit -q --allow-empty -m unpushed
stub="$tmp/stubbin"
record="$tmp/cargo-record.txt"; : >"$record"
make_stub_bin "$stub" yes
# POSIX `env` cannot exec a shell function (three latent `env warm ...`
# failures, never reached in the previous session), so env-var prefixes are
# bracketed exports around the function calls instead. PATH carries the stub
# toolchain: without it -Warm resolves the REAL cargo and starts a full
# workspace build inside the test (the timeout of the previous session); the
# stub records argv + env and exits 0 — no real Cargo run.
old_path=$PATH
export PATH="$stub:$PATH"
export FIXTURE_RECORD="$(cygpath -w "$record")"

expect_fail "warm unpushed" "remote tip" warm -BuildLane worker-1 -Repo "$wrepo" -Warm
[ ! -s "$record" ] || fail "refused warm still invoked cargo"
ok "warm refuses an unpushed branch and never starts cargo"

git -C "$wwt" reset -q --hard origin/main  # fixture repair: back to the pushed tip
git -C "$wwt" push -q origin codex/gh200-a
wlaneroot=$(cygpath -w "$tmp/lanes")
export FLEET_LANE_ROOT="$wlaneroot"

out=$(warm -BuildLane worker-1 -Repo "$wrepo" -DryRun 2>&1)
case "$out" in
  *"cargo build --workspace --all-targets"*) : ;;
  *) fail "warm dry-run does not show the exact build command :: $out" ;;
esac
case "$out" in
  *"CARGO_TARGET_DIR=$wlaneroot\\worker-1"*) : ;;
  *) fail "warm dry-run CARGO_TARGET_DIR is not the fixed lane dir :: $out" ;;
esac
case "$out" in
  *"CARGO_INCREMENTAL=1"*) : ;;
  *) fail "warm dry-run does not set CARGO_INCREMENTAL=1 for a worker :: $out" ;;
esac
case "$out" in
  *"line-tables-only"*) : ;;
  *) fail "warm dry-run does not report line-tables-only :: $out" ;;
esac
case "$out" in
  *"rust-lld"*) : ;;
  *) fail "warm dry-run does not report the shipped rust-lld linker :: $out" ;;
esac
[ ! -s "$record" ] || fail "warm dry-run invoked cargo"
[ "$(git -C "$wwt" symbolic-ref --quiet HEAD)" = "refs/heads/codex/gh200-a" ] \
  || fail "warm dry-run checked out"
ok "warm -DryRun reports exact command, fixed lane dir, env; zero side effects"

# default lane root: FLEET_LANE_ROOT must be unset for these two
unset FLEET_LANE_ROOT
# fixture: warm never creates worktrees, so the verifier-2 dry-run needs one
expect_ok "warm fixture prepare verifier-2" prepare -BuildLane verifier-2 -Branch codex/gh201-b -Repo "$wrepo"
git -C "$(wt_posix "$wrepo" verifier-2)" push -q origin codex/gh201-b
out=$(warm -BuildLane verifier-2 -Repo "$wrepo" -DryRun 2>&1)
case "$out" in
  *"CARGO_INCREMENTAL=0"*) : ;;
  *) fail "verifier lane should warm with CARGO_INCREMENTAL=0 :: $out" ;;
esac
ok "warm sets CARGO_INCREMENTAL=0 for verifier lanes"

out=$(warm -BuildLane worker-1 -Repo "$wrepo" -DryRun 2>&1)
case "$out" in
  *"$LOCALAPPDATA\\fleet-workstation\\lanes\\worker-1"*) : ;;
  *) fail "default lane root is not LOCALAPPDATA fleet-workstation lanes :: $out" ;;
esac
ok "warm defaults the lane root to LOCALAPPDATA\fleet-workstation\lanes"
export FLEET_LANE_ROOT="$wlaneroot"

expect_ok "warm happy path" warm -BuildLane worker-1 -Repo "$wrepo" -Warm
recorded=$(cat "$record")
case "$recorded" in
  *"cargo build --workspace --all-targets"*) : ;;
  *) fail "warm did not run cargo build --workspace --all-targets :: $recorded" ;;
esac
case "$recorded" in
  *"--all-targets test"*|*"cargo test"*) fail "warm ran tests :: $recorded" ;;
  *) : ;;
esac
case "$recorded" in
  *"TDIR=$wlaneroot\\worker-1 INCR=1 DEV=line-tables-only TEST=line-tables-only"*) : ;;
  *) fail "warm passed the wrong env to cargo :: $recorded" ;;
esac
main_sha=$(git -C "$wrepo" rev-parse origin/main)
[ "$(git -C "$wwt" rev-parse HEAD)" = "$main_sha" ] || fail "warm did not check out the merged main tip"
[ -z "$(git -C "$wwt" symbolic-ref -q HEAD)" ] || fail "warm left the worktree on a branch"
ok "warm checks out merged main and runs cargo build --workspace --all-targets with the lane env"

expect_fail "warm busy" "busy" warm -BuildLane worker-2 -Repo "$repo" -Warm
ok "warm refuses a lane whose scheduled task is Running"

# rust-lld absent from the active toolchain: fail fast, before any checkout/build.
absent_stub="$tmp/stubbin-absent"; : >"$record"
make_stub_bin "$absent_stub" no
before_head=$(git -C "$wwt" rev-parse HEAD)
export PATH="$absent_stub:$PATH"
expect_fail "warm rustlld absent" "rust-lld" warm -BuildLane worker-1 -Repo "$wrepo" -Warm
export PATH="$stub:$PATH"
[ ! -s "$record" ] || fail "rust-lld absent warm still invoked cargo"
[ "$(git -C "$wwt" rev-parse HEAD)" = "$before_head" ] || fail "rust-lld absent warm still checked out"
ok "warm fails fast when rust-lld.exe is absent from the toolchain sysroot"

out=$(warm -BuildLane worker-1 -PrintEnv 2>&1)  # FLEET_LANE_ROOT still exported
case "$out" in
  *"CARGO_TARGET_DIR=$wlaneroot\\worker-1"*) : ;;
  *) fail "PrintEnv CARGO_TARGET_DIR wrong :: $out" ;;
esac
case "$out" in
  *"CARGO_INCREMENTAL=1"*) : ;;
  *) fail "PrintEnv worker incremental wrong :: $out" ;;
esac
out=$(warm -BuildLane verifier-2 -PrintEnv 2>&1)
case "$out" in
  *"CARGO_INCREMENTAL=0"*) : ;;
  *) fail "PrintEnv verifier incremental wrong :: $out" ;;
esac
ok "lane-warm -PrintEnv exposes the wrapper env for the lane-launch integration"

expect_fail "warm mode ambiguity" "exactly one" warm -BuildLane worker-1 -Repo "$wrepo" -PrintEnv -DryRun
expect_fail "warm no mode" "exactly one" warm -BuildLane worker-1 -Repo "$wrepo"
ok "warm requires exactly one of -PrintEnv / -DryRun / -Warm"

# restore the caller environment before the launch section
unset FIXTURE_RECORD FLEET_LANE_ROOT
export PATH="$old_path"

# ---------------------------------------------------------------------------
# lane-launch.ps1 wrapper env integration (GH-626): the wrapper must receive
# the lane env contract from lane-warm -PrintEnv — one owner, no duplicated
# literal — and must set NO CARGO_* env at all without a build lane.
launch() { pwsh -NoProfile -NonInteractive -File "$root/scripts/fleet/lane-launch.ps1" "$@"; }
llog="$tmp/launch-logs"; mkdir -p "$llog"

expect_ok "launch dry-run with build lane" launch -Name gh626envcheck -Cwd "$repo" \
  -BuildLane worker-1 -LogDir "$llog" -TimeoutSec 60 -DryRun
lw="$llog/gh626envcheck.dryrun-wrapper.ps1"
[ -f "$lw" ] || fail "launch dry-run produced no wrapper"
grep -F "\$env:CARGO_TARGET_DIR = " "$lw" | grep -F "worker-1" >/dev/null \
  || fail "launch wrapper CARGO_TARGET_DIR is not the fixed lane dir"
grep -F "\$env:CARGO_INCREMENTAL = '1'" "$lw" >/dev/null \
  || fail "launch wrapper missing CARGO_INCREMENTAL=1 for a worker lane"
n=$(grep -cF 'line-tables-only' "$lw" || true)
[ "$n" -ge 2 ] || fail "launch wrapper missing line-tables-only dev/test trim (found $n)"
ok "lane-launch -BuildLane wrapper carries the lane env contract from lane-warm -PrintEnv"

expect_ok "launch dry-run without build lane" launch -Name gh626noenv -Cwd "$repo" \
  -LogDir "$llog" -TimeoutSec 60 -DryRun
if grep -q 'CARGO_' "$llog/gh626noenv.dryrun-wrapper.ps1"; then
  fail "launch wrapper set CARGO_* env without a build lane"
fi
ok "lane-launch without -BuildLane sets no CARGO env (docs lanes compile nothing)"

echo "1..$case_number"
echo "PASS: lane helper self-test ($case_number cases)"
