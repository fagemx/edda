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
logdir="$tmp/lanes"

# GH-786: every fake lane this run starts is torn down from the trap, not only
# from the success path: a `fail` between starting a lane and dropping it would
# otherwise leave a live scheduled task and its process tree behind, and that
# tree holds the temp dir open so even `rm -rf` fails.
started_lanes=''

e2e_cleanup() {
  [ -n "$started_lanes" ] || return 0
  for cl_lane in $started_lanes; do
    cl_wrapper=$(cygpath -w "$logdir/$cl_lane.wrapper.ps1" 2>/dev/null || echo "$logdir/$cl_lane.wrapper.ps1")
    # Written to a file, never passed as -Command: a query whose own command
    # line contains the pattern matches itself and kills the killer.

    {
      printf "Unregister-ScheduledTask -TaskName 'edda-lane-%s' -Confirm:\$false -ErrorAction SilentlyContinue\n" "$cl_lane"
      printf "\$w = '%s'\n" "$cl_wrapper"
      printf "foreach (\$h in @(Get-CimInstance Win32_Process | Where-Object { \$_.CommandLine -and \$_.CommandLine.Contains(\$w) })) {\n"
      printf "  & taskkill /PID \$h.ProcessId /T /F 2>\$null | Out-Null\n"
      printf "}\n"
    } >"$tmp/cleanup-$cl_lane.ps1"
    pwsh -NoProfile -NonInteractive -File "$tmp/cleanup-$cl_lane.ps1" >/dev/null 2>&1 || true
  done
  started_lanes=''
}

trap 'e2e_cleanup; rm -rf "$tmp"' 0 HUP INT TERM

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

# --- case 9: a config that cannot be READ is neither backed up nor overwritten
# A config held open by whatever is writing it may be perfectly good. Treating
# an unreadable file as corrupt would restore over it and destroy live state;
# treating it as healthy would copy garbage into the backup.
repo8=$(new_repo locked)
guard -RepoPath "$repo8" -Backup >/dev/null 2>&1 || fail "setup: backup failed"
cp "$repo8/.git/config" "$tmp/locked-good"
repo8_w=$(cygpath -w "$repo8" 2>/dev/null || echo "$repo8")
guard_w=$(cygpath -w "$GUARD" 2>/dev/null || echo "$GUARD")
{
  printf '$ErrorActionPreference = "Stop"\n'
  printf '$fs = [System.IO.File]::Open("%s\\.git\\config", "Open", "Read", "None")\n' "$repo8_w"
  printf 'try {\n'
  printf '  & "%s" -RepoPath "%s" -Backup 2>&1 | Out-Null; "backup=$LASTEXITCODE"\n' "$guard_w" "$repo8_w"
  printf '  & "%s" -RepoPath "%s" -Restore 2>&1 | Out-Null; "restore=$LASTEXITCODE"\n' "$guard_w" "$repo8_w"
  printf '} finally { $fs.Close() }\n'
} >"$tmp/locked.ps1"
pwsh -NoProfile -NonInteractive -File "$tmp/locked.ps1" >"$tmp/out9" 2>&1 ||
  fail "locked-config probe did not run: $(cat "$tmp/out9")"
grep -q '^backup=4$' "$tmp/out9" ||
  fail "-Backup on an unreadable config should exit 4 (could not be judged), got: $(cat "$tmp/out9")"
grep -q '^restore=4$' "$tmp/out9" ||
  fail "-Restore on an unreadable config should exit 4 (could not be judged) and not restore, got: $(cat "$tmp/out9")"
cmp -s "$tmp/locked-good" "$repo8/.git/config" ||
  fail "the unreadable config was overwritten; a locked config is not a corrupt one"
[ "$(corrupt_backups "$repo8")" -eq 0 ] || fail "a locked config was treated as corrupt and archived"
ok "an unreadable config is neither backed up from nor restored over"

# --- case 10: the backup keeps one generation, and restore falls back to it -
# A torn write can end on a boundary that still parses; backing THAT up would
# retire the last complete copy, so the outgoing backup is kept as .prev.
repo9=$(new_repo rotate)
guard -RepoPath "$repo9" -Backup >/dev/null 2>&1 || fail "setup: first backup failed"
cp "$repo9/.git/config" "$tmp/gen1"
printf '[core]\n\trepositoryformatversion = 0\n' >"$repo9/.git/config"
guard -RepoPath "$repo9" -Backup >/dev/null 2>&1 || fail "setup: second backup failed"
cmp -s "$tmp/gen1" "$repo9/.git/config.guard.bak.prev" ||
  fail "the outgoing backup was discarded instead of kept as .prev"
# Now lose both the config and the newest backup; .prev must still save it.
nul_ise "$repo9/.git/config"
nul_ise "$repo9/.git/config.guard.bak"
guard -RepoPath "$repo9" -Restore >"$tmp/out10" 2>&1 ||
  fail "-Restore did not fall back to the previous generation: $(cat "$tmp/out10")"
cmp -s "$tmp/gen1" "$repo9/.git/config" || fail "-Restore used the wrong generation"
git -C "$repo9" status >/dev/null 2>&1 || fail "git still broken after the .prev restore"
ok "-Backup keeps one previous generation and -Restore falls back to it"

# --- case 11: verify rejects a NUL-corrupt branch ref and names repair SHA --
repo_ref1=$(new_repo ref-verify)
ref1=$(git -C "$repo_ref1" symbolic-ref HEAD)
ref1_file="$repo_ref1/.git/$ref1"
good_sha1=$(git -C "$repo_ref1" rev-parse HEAD)
nul_ise "$ref1_file"

if guard -RepoPath "$repo_ref1" -Verify >"$tmp/out11" 2>&1; then
  fail "-Verify exited 0 on a NUL-corrupt branch ref: $(cat "$tmp/out11")"
fi
grep -q 'refs UNHEALTHY' "$tmp/out11" ||
  fail "-Verify did not report refs UNHEALTHY: $(cat "$tmp/out11")"
grep -q "$good_sha1" "$tmp/out11" ||
  fail "-Verify did not name the repair SHA ($good_sha1) in its output: $(cat "$tmp/out11")"
ok "-Verify rejects a NUL-corrupted branch ref and names the repair SHA"

# --- case 12: backup refuses when branch ref is NUL (protects lane dispatch) --
if guard -RepoPath "$repo_ref1" -Backup >"$tmp/out12" 2>&1; then
  fail "-Backup exited 0 on a repo with a NUL-corrupted branch ref"
fi
grep -q 'ref(s) unhealthy' "$tmp/out12" ||
  fail "-Backup did not report ref(s) unhealthy: $(cat "$tmp/out12")"
ok "-Backup refuses when a branch ref is NUL, preventing dispatch into broken worktree"

# --- case 13: restore repairs NUL branch ref from reflog ---------------------
if ! guard -RepoPath "$repo_ref1" -Restore >"$tmp/out13" 2>&1; then
  fail "-Restore exited non-zero: $(cat "$tmp/out13")"
fi
grep -q "RESTORED ref=$ref1" "$tmp/out13" ||
  fail "-Restore output missing RESTORED ref line: $(cat "$tmp/out13")"
git -C "$repo_ref1" log --oneline -1 >/dev/null 2>&1 ||
  fail "git log still broken after ref -Restore"
git -C "$repo_ref1" fsck --connectivity-only >/dev/null 2>&1 ||
  fail "git fsck still fails after ref -Restore"
corrupt_ref_count=$(ls "$repo_ref1/.git/" | grep -c '^ref\.CORRUPT\.' || true)
[ "$corrupt_ref_count" -ge 1 ] ||
  fail "corrupt ref file was not preserved outside .git/refs"
ok "-Restore repairs a NUL branch ref from reflog and preserves forensic backup"

# --- case 14: restore with no reflog fails loudly and preserves state --------
repo_ref2=$(new_repo ref-noreflog)
ref2=$(git -C "$repo_ref2" symbolic-ref HEAD)
ref2_file="$repo_ref2/.git/$ref2"
nul_ise "$ref2_file"
rm -rf "$repo_ref2/.git/logs"

if guard -RepoPath "$repo_ref2" -RestoreRefs >"$tmp/out14" 2>&1; then
  fail "-RestoreRefs exited 0 when no reflog was available"
fi
grep -q -E 'cannot repair refs automatically|RESTORE FAILED' "$tmp/out14" ||
  fail "-RestoreRefs did not report failure when reflog was missing: $(cat "$tmp/out14")"
ok "-RestoreRefs fails loudly when no reflog is available"

# --- case 15: linked worktree ref corruption is detected and repaired -------
repo_main=$(new_repo wt-main)
wt_dir="$tmp/wt-child"
git -C "$repo_main" worktree add -b wt-branch "$wt_dir" -q
wt_ref="refs/heads/wt-branch"
wt_ref_file="$repo_main/.git/$wt_ref"
good_wt_sha=$(git -C "$wt_dir" rev-parse HEAD)
nul_ise "$wt_ref_file"

if guard -RepoPath "$wt_dir" -Verify >"$tmp/out15a" 2>&1; then
  fail "-Verify in linked worktree exited 0 on corrupt branch ref"
fi
guard -RepoPath "$wt_dir" -RestoreRefs >"$tmp/out15b" 2>&1 ||
  fail "-RestoreRefs in linked worktree failed: $(cat "$tmp/out15b")"
git -C "$wt_dir" log --oneline -1 >/dev/null 2>&1 ||
  fail "git log in worktree still fails after -RestoreRefs"
ok "linked worktree ref corruption is detected and repaired"

# --- case 16 (opt-in): lane-stop really restores after a real kill ----------
# Registers a scheduled task, so it is off by default; run with
# GIT_CONFIG_GUARD_E2E=1 to exercise the whole lane-stop path end to end.
if [ "${GIT_CONFIG_GUARD_E2E:-}" = 1 ]; then
  mkdir -p "$logdir"
  logdir_w=$(cygpath -w "$logdir" 2>/dev/null || echo "$logdir")

  # Register and start a lane shaped like the one lane-launch.ps1 generates:
  # lane-stop reads the worktree out of its Set-Location line and the lane
  # identity off the brief. It spawns a child, which is the shape that matters
  # — Stop-ScheduledTask kills only the wrapper, leaving the child to be found
  # and killed by lane-stop's tree traversal (GH-672/GH-706), and a child
  # killed mid-git-write is what corrupts the config in the first place.
  # The two wrapper generators write the cwd differently: lane-launch.ps1 emits
  # `Set-Location -LiteralPath '<p>'`, while the review lanes of review-pr.sh
  # and pr-review-launch.ps1 emit a bare `Set-Location '<p>'`. lane-stop claims
  # to stop both families, so both shapes are exercised.
  # usage: start_fake_lane <lane> <repo> [literal|bare]
  start_fake_lane() {
    fl_lane=$1
    fl_repo_w=$(cygpath -w "$2" 2>/dev/null || echo "$2")
    fl_shape=${3:-literal}
    # Registered for cleanup BEFORE anything starts, because start_fake_lane
    # calls fail() itself on a bad start.
    started_lanes="$started_lanes $fl_lane"
    : >"$logdir/$fl_lane.brief.md"
    {
      if [ "$fl_shape" = bare ]; then
        printf "Set-Location '%s'\n" "$fl_repo_w"
      else
        printf "Set-Location -LiteralPath '%s'\n" "$fl_repo_w"
      fi
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
      printf "\$deadline = (Get-Date).AddSeconds(10)\n"
      printf "\$t = Get-ScheduledTask -TaskName 'edda-lane-%s'\n" "$fl_lane"
      printf "while (\$t.State -ne 'Running' -and (Get-Date) -lt \$deadline) {\n"
      printf "  Start-Sleep -Milliseconds 500\n"
      printf "  \$t = Get-ScheduledTask -TaskName 'edda-lane-%s'\n" "$fl_lane"
      printf "}\n"
      printf 'if ($t.State -ne "Running") { Write-Error "fake lane state=$($t.State), expected Running"; exit 1 }\n'
      printf 'Start-Sleep -Seconds 2\n'
      printf '"fake lane is Running"\n'
    } >"$tmp/$fl_lane-setup.ps1"
    pwsh -NoProfile -NonInteractive -File "$tmp/$fl_lane-setup.ps1" >"$tmp/$fl_lane-setup.out" 2>&1 ||
      fail "e2e setup: lane $fl_lane did not reach Running: $(cat "$tmp/$fl_lane-setup.out")"
  }

  drop_fake_lane() {
    pwsh -NoProfile -NonInteractive -Command \
      "Unregister-ScheduledTask -TaskName 'edda-lane-$1' -Confirm:\$false -ErrorAction SilentlyContinue" >/dev/null 2>&1
    _new=''
    for _l in $started_lanes; do
      [ "$_l" = "$1" ] || _new="$_new $_l"
    done
    started_lanes=$_new
  }

  lane=guard-e2e-$$
  repo6=$(new_repo e2e)
  guard -RepoPath "$repo6" -Backup >/dev/null 2>&1 || fail "e2e setup: backup failed"
  cp "$repo6/.git/config" "$tmp/e2e-good"
  start_fake_lane "$lane" "$repo6"

  # The lane is live; now do to the shared config exactly what a kill does to it.
  nul_ise "$repo6/.git/config"
  git -C "$repo6" status >/dev/null 2>&1 && fail "e2e precondition: git still works on the corrupt repo"

  # Read the status through an `if`, not `$?` on the next line: under `set -e`
  # a non-zero exit (which case 12 expects) would end the script first.
  if pwsh -NoProfile -NonInteractive -File "$root/scripts/fleet/lane-stop.ps1" \
    -Name "$lane" -LogDir "$logdir_w" >"$tmp/out9" 2>&1
  then stop_status=0; else stop_status=$?; fi
  drop_fake_lane "$lane"

  [ "$stop_status" -eq 0 ] || fail "lane-stop exited $stop_status: $(cat "$tmp/out9")"
  grep -q 'terminated=[1-9]' "$tmp/out9" ||
    fail "lane-stop terminated nothing, so the kill path was never exercised: $(cat "$tmp/out9")"
  grep -q 'gitconfig=RESTORED' "$tmp/out9" ||
    fail "lane-stop did not report a restore: $(cat "$tmp/out9")"
  grep -q 'gitrefs=healthy' "$tmp/out9" ||
    fail "lane-stop did not report healthy refs when only config was corrupt: $(cat "$tmp/out9")"
  git -C "$repo6" status >/dev/null 2>&1 ||
    fail "git still broken after lane-stop: $(cat "$tmp/out9")"
  cmp -s "$tmp/e2e-good" "$repo6/.git/config" || fail "lane-stop restored the wrong bytes"
  grep -q '=== EXIT' "$logdir/$lane.log" 2>/dev/null ||
    fail "lane-stop skipped the end record while handling the config; lane-stop said: $(cat "$tmp/out9")"
  [ -f "$logdir/$lane.done" ] ||
    fail "lane-stop wrote no done-file; lane-stop said: $(cat "$tmp/out9")"
  ok "lane-stop restores the shared .git/config after really killing a lane"

  # --- case 12 (opt-in): unrepairable config is an error, not a clean stop --
  lane2=guard-e2e-nobak-$$
  repo7=$(new_repo e2e-nobak)
  start_fake_lane "$lane2" "$repo7"
  nul_ise "$repo7/.git/config"

  if pwsh -NoProfile -NonInteractive -File "$root/scripts/fleet/lane-stop.ps1" \
    -Name "$lane2" -LogDir "$logdir_w" >"$tmp/out10" 2>&1
  then stop_status2=0; else stop_status2=$?; fi
  drop_fake_lane "$lane2"

  [ "$stop_status2" -eq 1 ] ||
    fail "lane-stop exited $stop_status2 on an unrepairable config; expected 1: $(cat "$tmp/out10")"
  grep -q 'terminated=[1-9]' "$tmp/out10" ||
    fail "lane-stop terminated nothing in case 12: $(cat "$tmp/out10")"
  grep -q 'gitconfig=UNREPAIRABLE' "$tmp/out10" ||
    fail "lane-stop did not report the config as unrepairable: $(cat "$tmp/out10")"
  grep -q 'gitrefs=UNVERIFIED' "$tmp/out10" ||
    fail "lane-stop did not report refs as UNVERIFIED when config failed: $(cat "$tmp/out10")"
  grep -q '=== EXIT' "$logdir/$lane2.log" 2>/dev/null ||
    fail "lane-stop lost the end record on the unrepairable path: $(cat "$tmp/out10")"
  ok "lane-stop exits 1 when the config is corrupt and no backup can repair it"

  # --- case 13 (opt-in): review lanes get the check too ----------------------
  # review-pr.sh:453 and pr-review-launch.ps1:64 write a bare `Set-Location`,
  # and lane-stop.ps1 claims to stop those tasks (GH-712).
  lane3=guard-e2e-review-$$
  repo10=$(new_repo e2e-review)
  guard -RepoPath "$repo10" -Backup >/dev/null 2>&1 || fail "e2e setup: backup failed"
  cp "$repo10/.git/config" "$tmp/e2e-review-good"
  start_fake_lane "$lane3" "$repo10" bare
  nul_ise "$repo10/.git/config"

  if pwsh -NoProfile -NonInteractive -File "$root/scripts/fleet/lane-stop.ps1" \
    -Name "$lane3" -LogDir "$logdir_w" >"$tmp/out13" 2>&1
  then stop_status3=0; else stop_status3=$?; fi
  drop_fake_lane "$lane3"

  [ "$stop_status3" -eq 0 ] || fail "lane-stop exited $stop_status3: $(cat "$tmp/out13")"
  grep -q 'gitconfig=RESTORED' "$tmp/out13" ||
    fail "a review-shaped wrapper skipped the config check: $(cat "$tmp/out13")"
  grep -q 'gitrefs=healthy' "$tmp/out13" ||
    fail "a review-shaped wrapper skipped the refs check: $(cat "$tmp/out13")"
  cmp -s "$tmp/e2e-review-good" "$repo10/.git/config" ||
    fail "the review lane's shared config was not repaired"
  ok "lane-stop resolves the cwd of review lanes too, not only lane-launch ones"

  # --- case 14 (opt-in): a guard exception never costs the end record --------
  # $ErrorActionPreference is Stop in lane-stop; a config held open makes the
  # guard's ReadAllBytes throw. That must not skip the GH-672 end record.
  lane4=guard-e2e-throw-$$
  repo11=$(new_repo e2e-throw)
  guard -RepoPath "$repo11" -Backup >/dev/null 2>&1 || fail "e2e setup: backup failed"
  start_fake_lane "$lane4" "$repo11"
  repo11_w=$(cygpath -w "$repo11" 2>/dev/null || echo "$repo11")
  {
    printf '$ErrorActionPreference = "Stop"\n'
    printf '$fs = [System.IO.File]::Open("%s\\.git\\config", "Open", "Read", "None")\n' "$repo11_w"
    printf 'try {\n'
    printf '  & "%s" -Name "%s" -LogDir "%s" 2>&1 | Out-Null\n' \
      "$(cygpath -w "$root/scripts/fleet/lane-stop.ps1")" "$lane4" "$logdir_w"
    printf '  "stop=$LASTEXITCODE"\n'
    printf '} finally { $fs.Close() }\n'
  } >"$tmp/throw.ps1"
  pwsh -NoProfile -NonInteractive -File "$tmp/throw.ps1" >"$tmp/out14" 2>&1 ||
    fail "locked-config lane-stop probe did not run: $(cat "$tmp/out14")"
  drop_fake_lane "$lane4"

  grep -q '^stop=1$' "$tmp/out14" ||
    fail "lane-stop should exit 1 when it cannot verify the config, got: $(cat "$tmp/out14")"
  grep -q '\.git/config UNVERIFIED' "$logdir/$lane4.log" 2>/dev/null ||
    fail "an unreadable config must not be reported as UNREPAIRABLE: $(cat "$logdir/$lane4.log" 2>/dev/null)"
  grep -q 'refs UNVERIFIED' "$logdir/$lane4.log" 2>/dev/null ||
    fail "an unreadable config did not report refs as UNVERIFIED: $(cat "$logdir/$lane4.log" 2>/dev/null)"
  grep -q '=== EXIT' "$logdir/$lane4.log" 2>/dev/null ||
    fail "an unreadable config cost the lane its === EXIT === record (GH-672): $(cat "$tmp/out14")"
  [ -f "$logdir/$lane4.done" ] ||
    fail "an unreadable config cost the lane its done-file (GH-672): $(cat "$tmp/out14")"
  ok "an unreadable config is reported as UNVERIFIED and never skips the end record"

  # --- case 15 (opt-in): the guard itself raising never costs the end record -
  # lane-stop runs under $ErrorActionPreference = Stop. The guard converts what
  # it can into exit codes, but anything it cannot — here, the script missing,
  # as in a partial checkout — still raises, and step 7 must survive it.
  lane5=guard-e2e-noguard-$$
  repo12=$(new_repo e2e-noguard)
  start_fake_lane "$lane5" "$repo12"
  crippled="$tmp/crippled"
  mkdir -p "$crippled"
  cp "$root/scripts/fleet/lane-stop.ps1" "$crippled/lane-stop.ps1"
  if pwsh -NoProfile -NonInteractive -File "$crippled/lane-stop.ps1" \
    -Name "$lane5" -LogDir "$logdir_w" >"$tmp/out15" 2>&1
  then stop_status5=0; else stop_status5=$?; fi
  drop_fake_lane "$lane5"

  [ "$stop_status5" -eq 1 ] ||
    fail "lane-stop should exit 1 when the guard cannot run at all, got $stop_status5: $(cat "$tmp/out15")"
  grep -q 'gitconfig=CHECK FAILED' "$tmp/out15" ||
    fail "a guard that cannot run must be reported, not swallowed: $(cat "$tmp/out15")"
  grep -q '=== EXIT' "$logdir/$lane5.log" 2>/dev/null ||
    fail "a raised guard error cost the lane its === EXIT === record (GH-672): $(cat "$tmp/out15")"
  [ -f "$logdir/$lane5.done" ] ||
    fail "a raised guard error cost the lane its done-file (GH-672): $(cat "$tmp/out15")"
  ok "a guard that raises is caught, reported, and never skips the end record"
else
  echo "-- cases 16-20 (lane-stop end-to-end) skipped; set GIT_CONFIG_GUARD_E2E=1 to run them"
fi

echo "all $case_number case(s) passed"
