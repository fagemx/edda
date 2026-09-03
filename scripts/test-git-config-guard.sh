#!/bin/sh
# Offline fixtures for scripts/fleet/git-config-guard.ps1 (GH-715).
#
# The defect: killing a lane mid-write leaves .git/config as a run of NUL
# bytes, which takes down git for the main checkout and every linked worktree
# sharing that config — and the only "backup" on disk was copied after the
# corruption, so it is NUL too. These cases pin the guard's contract:
#   * a backup is taken only from a config that parses (never a NUL file),
#   * a corrupt config is detected, not silently accepted,
#   * a restore actually revives git and preserves the corrupt file,
#   * a restore with no usable backup fails loudly instead of faking success,
#   * a linked worktree resolves to the SHARED config (the single point of
#     failure this issue is about), not to its own .git file.
#
# Everything runs in throwaway repos under a temp dir; no state outside it is
# touched. Style follows scripts/test-lint-markdown-content.sh — no new tooling.
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
GUARD="$root/scripts/fleet/git-config-guard.ps1"

command -v pwsh >/dev/null 2>&1 || { echo "SKIP: pwsh not on PATH"; exit 0; }
command -v git >/dev/null 2>&1 || { echo "SKIP: git not on PATH"; exit 0; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' 0 HUP INT TERM

case_number=0
fail() { echo "FAIL (after case $case_number): $*" >&2; exit 1; }
ok() { case_number=$((case_number + 1)); echo "ok $case_number - $*"; }

guard() { pwsh -NoProfile -NonInteractive -File "$GUARD" "$@"; }

# Byte count of NULs in a file (0 for a healthy config).
nuls() { tr -dc '\000' <"$1" | wc -c | tr -d ' '; }

# A throwaway repo with one commit; prints its path.
new_repo() {
  d="$tmp/$1"
  mkdir -p "$d"
  git -C "$d" init -q
  git -C "$d" config user.email t@example.com
  git -C "$d" config user.name tester
  echo hello >"$d/f.txt"
  git -C "$d" add f.txt
  git -C "$d" -c commit.gpgsign=false commit -qm init
  echo "$d"
}

# Overwrite a file with exactly as many NUL bytes as it had, which is the shape
# both real corruptions took (27328/27328 and 2561/2561 bytes, GH-715).
nul_ise() {
  size=$(wc -c <"$1" | tr -d ' ')
  head -c "$size" /dev/zero >"$1.zero"
  mv "$1.zero" "$1"
}

corrupt_backups() { ls "$1/.git/" | grep -c '^config\.CORRUPT\.' || true; }

# --- case 1: backup from a healthy repo is taken and is byte-identical -------
repo=$(new_repo healthy)
guard -RepoPath "$repo" -Backup >"$tmp/out1" 2>&1 ||
  fail "-Backup exited non-zero on a healthy repo: $(cat "$tmp/out1")"
[ -f "$repo/.git/config.guard.bak" ] || fail "-Backup wrote no backup file"
cmp -s "$repo/.git/config" "$repo/.git/config.guard.bak" ||
  fail "backup differs from the config it was taken from"
ok "-Backup copies a parseable config to .git/config.guard.bak"

# --- case 2: backup REFUSES a NUL config and keeps the good one -------------
# This is the exact failure of the real .bak files: copied after corruption.
cp "$repo/.git/config" "$tmp/known-good"
nul_ise "$repo/.git/config"
if guard -RepoPath "$repo" -Backup >"$tmp/out2" 2>&1; then
  fail "-Backup exited 0 on a NUL-corrupt config (this is how the real .bak became all NULs)"
fi
cmp -s "$tmp/known-good" "$repo/.git/config.guard.bak" ||
  fail "-Backup overwrote the good backup with the corrupt config"
ok "-Backup refuses a NUL config and leaves the previous good backup intact"

# --- case 3: verify distinguishes healthy / NUL / unparseable ---------------
if guard -RepoPath "$repo" -Verify >"$tmp/out3a" 2>&1; then
  fail "-Verify exited 0 on the NUL-corrupt config"
fi
repo2=$(new_repo verify-ok)
guard -RepoPath "$repo2" -Verify >"$tmp/out3b" 2>&1 ||
  fail "-Verify exited non-zero on a healthy config: $(cat "$tmp/out3b")"
printf '[core\nthis is not ini\n' >"$repo2/.git/config"
if guard -RepoPath "$repo2" -Verify >"$tmp/out3c" 2>&1; then
  fail "-Verify exited 0 on an unparseable (non-NUL) config"
fi
ok "-Verify accepts a healthy config and rejects NUL and unparseable ones"

# --- case 4: restore revives git and preserves the corrupt file -------------
git -C "$repo" status >/dev/null 2>&1 &&
  fail "precondition: git still works on the NUL-corrupt repo"
guard -RepoPath "$repo" -Restore >"$tmp/out4" 2>&1 ||
  fail "-Restore exited non-zero: $(cat "$tmp/out4")"
git -C "$repo" status >/dev/null 2>&1 || fail "git still broken after -Restore"
cmp -s "$tmp/known-good" "$repo/.git/config" ||
  fail "restored config differs from the known-good backup"
[ "$(corrupt_backups "$repo")" -ge 1 ] ||
  fail "-Restore discarded the corrupt config instead of preserving it for forensics"
ok "-Restore revives git from the backup and preserves the corrupt config"

# --- case 5: restore with no usable backup fails loudly ---------------------
repo3=$(new_repo no-backup)
nul_ise "$repo3/.git/config"
if guard -RepoPath "$repo3" -Restore >"$tmp/out5" 2>&1; then
  fail "-Restore exited 0 with no backup available (silent fake success)"
fi
[ "$(nuls "$repo3/.git/config")" -gt 0 ] ||
  fail "-Restore altered the config despite having nothing to restore from"
ok "-Restore with no backup exits non-zero and changes nothing"

# --- case 6: a linked worktree resolves to the SHARED config ----------------
repo4=$(new_repo shared)
guard -RepoPath "$repo4" -Backup >/dev/null 2>&1 || fail "setup: backup of shared repo failed"
git -C "$repo4" worktree add -q "$tmp/wt" -b wt-branch >/dev/null 2>&1 ||
  fail "setup: worktree add failed"
[ -f "$tmp/wt/.git" ] || fail "setup: linked worktree has no .git file"
cp "$repo4/.git/config" "$tmp/shared-good"
nul_ise "$repo4/.git/config"
if guard -RepoPath "$tmp/wt" -Verify >"$tmp/out6a" 2>&1; then
  fail "-Verify from a linked worktree missed the corrupt SHARED config"
fi
guard -RepoPath "$tmp/wt" -Restore >"$tmp/out6b" 2>&1 ||
  fail "-Restore from a linked worktree failed: $(cat "$tmp/out6b")"
cmp -s "$tmp/shared-good" "$repo4/.git/config" ||
  fail "-Restore from a linked worktree did not repair the shared config"
git -C "$tmp/wt" status >/dev/null 2>&1 || fail "linked worktree still broken after -Restore"
ok "-Verify/-Restore from a linked worktree act on the shared .git/config"

# --- case 7: VerifyOrRestore is a no-op when healthy, repairs when not ------
repo5=$(new_repo composite)
guard -RepoPath "$repo5" -Backup >/dev/null 2>&1 || fail "setup: backup failed"
before=$(git -C "$repo5" config --list | sort)
guard -RepoPath "$repo5" -VerifyOrRestore >"$tmp/out7a" 2>&1 ||
  fail "-VerifyOrRestore failed on a healthy repo: $(cat "$tmp/out7a")"
[ "$(corrupt_backups "$repo5")" -eq 0 ] || fail "-VerifyOrRestore restored a healthy config"
[ "$(git -C "$repo5" config --list | sort)" = "$before" ] ||
  fail "-VerifyOrRestore changed a healthy config"
nul_ise "$repo5/.git/config"
guard -RepoPath "$repo5" -VerifyOrRestore >"$tmp/out7b" 2>&1 ||
  fail "-VerifyOrRestore failed to repair: $(cat "$tmp/out7b")"
git -C "$repo5" status >/dev/null 2>&1 || fail "git still broken after -VerifyOrRestore"
grep -qi restor "$tmp/out7b" ||
  fail "-VerifyOrRestore repaired the config without saying so"
ok "-VerifyOrRestore is a no-op when healthy and repairs loudly when not"

# --- case 8: the lane lifecycle is wired to the guard -----------------------
grep -q 'git-config-guard.ps1' "$root/scripts/fleet/lane-launch.ps1" ||
  fail "lane-launch.ps1 starts a lane without first taking a validated .git/config backup (GH-715 doneWhen 1)"
grep -q 'git-config-guard.ps1' "$root/scripts/fleet/lane-stop.ps1" ||
  fail "lane-stop.ps1 kills the lane process tree but never validates .git/config afterwards (GH-715 doneWhen 2)"
ok "lane-launch backs up before the lane runs and lane-stop validates after the kill"

# --- case 9 (opt-in): lane-stop really restores after a real kill -----------
# Registers a scheduled task, so it is off by default; run with
# GIT_CONFIG_GUARD_E2E=1 to exercise the whole lane-stop path end to end.
if [ "${GIT_CONFIG_GUARD_E2E:-}" = 1 ]; then
  logdir="$tmp/lanes"
  mkdir -p "$logdir"
  logdir_w=$(cygpath -w "$logdir" 2>/dev/null || echo "$logdir")

  # Register and start a lane shaped like the one lane-launch.ps1 generates:
  # lane-stop reads the worktree out of its Set-Location line and the lane
  # identity off the brief. It spawns a child, which is the shape that matters
  # — Stop-ScheduledTask kills only the wrapper, leaving the child to be found
  # and killed by lane-stop's tree traversal (GH-672/GH-706), and a child
  # killed mid-git-write is what corrupts the config in the first place.
  # usage: start_fake_lane <lane> <repo>
  start_fake_lane() {
    fl_lane=$1
    fl_repo_w=$(cygpath -w "$2" 2>/dev/null || echo "$2")
    : >"$logdir/$fl_lane.brief.md"
    {
      printf "Set-Location -LiteralPath '%s'\n" "$fl_repo_w"
      printf "# edda dispatch --prompt-file '%s'\n" "$logdir_w\\$fl_lane.brief.md"
      printf '$c = Start-Process -PassThru -WindowStyle Hidden -FilePath "$env:SystemRoot\\System32\\cmd.exe" -ArgumentList "/c","ping -n 300 127.0.0.1"\n'
      printf 'Start-Sleep -Seconds 300\n'
    } >"$logdir/$fl_lane.wrapper.ps1"

    # Written as a file, not an inline -Command: a backtick line continuation
    # inside a double-quoted shell string is eaten as command substitution.
    {
      printf '$ErrorActionPreference = "Stop"\n'
      printf '$argLine = %s\n' "'-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File \"$logdir_w\\$fl_lane.wrapper.ps1\"'"
      printf '$a = New-ScheduledTaskAction -Execute (Get-Command pwsh.exe).Source -Argument $argLine\n'
      printf "Unregister-ScheduledTask -TaskName 'edda-lane-%s' -Confirm:\$false -ErrorAction SilentlyContinue\n" "$fl_lane"
      printf "Register-ScheduledTask -TaskName 'edda-lane-%s' -Action \$a -RunLevel Limited | Out-Null\n" "$fl_lane"
      printf "Start-ScheduledTask -TaskName 'edda-lane-%s'\n" "$fl_lane"
      printf 'Start-Sleep -Seconds 3\n'
      printf "\$t = Get-ScheduledTask -TaskName 'edda-lane-%s'\n" "$fl_lane"
      printf 'if ($t.State -ne "Running") { Write-Error "fake lane state=$($t.State), expected Running"; exit 1 }\n'
      printf '"fake lane is Running"\n'
    } >"$tmp/$fl_lane-setup.ps1"
    pwsh -NoProfile -NonInteractive -File "$tmp/$fl_lane-setup.ps1" >"$tmp/$fl_lane-setup.out" 2>&1 ||
      fail "e2e setup: lane $fl_lane did not reach Running: $(cat "$tmp/$fl_lane-setup.out")"
  }

  drop_fake_lane() {
    pwsh -NoProfile -NonInteractive -Command \
      "Unregister-ScheduledTask -TaskName 'edda-lane-$1' -Confirm:\$false -ErrorAction SilentlyContinue" >/dev/null 2>&1
  }

  lane=guard-e2e
  repo6=$(new_repo e2e)
  guard -RepoPath "$repo6" -Backup >/dev/null 2>&1 || fail "e2e setup: backup failed"
  cp "$repo6/.git/config" "$tmp/e2e-good"
  start_fake_lane "$lane" "$repo6"

  # The lane is live; now do to the shared config exactly what a kill does to it.
  nul_ise "$repo6/.git/config"
  git -C "$repo6" status >/dev/null 2>&1 && fail "e2e precondition: git still works on the corrupt repo"

  # Read the status through an `if`, not `$?` on the next line: under `set -e`
  # a non-zero exit (which case 10 expects) would end the script first.
  if pwsh -NoProfile -NonInteractive -File "$root/scripts/fleet/lane-stop.ps1" \
    -Name "$lane" -LogDir "$logdir_w" >"$tmp/out9" 2>&1
  then stop_status=0; else stop_status=$?; fi
  drop_fake_lane "$lane"

  [ "$stop_status" -eq 0 ] || fail "lane-stop exited $stop_status: $(cat "$tmp/out9")"
  grep -q 'terminated=[1-9]' "$tmp/out9" ||
    fail "lane-stop terminated nothing, so the kill path was never exercised: $(cat "$tmp/out9")"
  grep -q 'gitconfig=RESTORED' "$tmp/out9" ||
    fail "lane-stop did not report a restore: $(cat "$tmp/out9")"
  git -C "$repo6" status >/dev/null 2>&1 ||
    fail "git still broken after lane-stop: $(cat "$tmp/out9")"
  cmp -s "$tmp/e2e-good" "$repo6/.git/config" || fail "lane-stop restored the wrong bytes"
  grep -q '=== EXIT' "$logdir/$lane.log" 2>/dev/null ||
    fail "lane-stop skipped the end record while handling the config; lane-stop said: $(cat "$tmp/out9")"
  [ -f "$logdir/$lane.done" ] ||
    fail "lane-stop wrote no done-file; lane-stop said: $(cat "$tmp/out9")"
  ok "lane-stop restores the shared .git/config after really killing a lane"

  # --- case 10 (opt-in): unrepairable config is an error, not a clean stop ---
  lane2=guard-e2e-nobak
  repo7=$(new_repo e2e-nobak)
  start_fake_lane "$lane2" "$repo7"
  nul_ise "$repo7/.git/config"

  if pwsh -NoProfile -NonInteractive -File "$root/scripts/fleet/lane-stop.ps1" \
    -Name "$lane2" -LogDir "$logdir_w" >"$tmp/out10" 2>&1
  then stop_status2=0; else stop_status2=$?; fi
  drop_fake_lane "$lane2"

  [ "$stop_status2" -eq 1 ] ||
    fail "lane-stop exited $stop_status2 on an unrepairable config; expected 1: $(cat "$tmp/out10")"
  grep -q 'gitconfig=UNREPAIRABLE' "$tmp/out10" ||
    fail "lane-stop did not report the config as unrepairable: $(cat "$tmp/out10")"
  grep -q '=== EXIT' "$logdir/$lane2.log" 2>/dev/null ||
    fail "lane-stop lost the end record on the unrepairable path: $(cat "$tmp/out10")"
  ok "lane-stop exits 1 when the config is corrupt and no backup can repair it"
else
  echo "-- cases 9-10 (lane-stop end-to-end) skipped; set GIT_CONFIG_GUARD_E2E=1 to run them"
fi

echo "all $case_number case(s) passed"
