# git-config-guard.ps1 — keep the shared .git/config recoverable (GH-715).
#
# The main checkout and all 30+ linked worktrees read ONE .git/config to find
# the remote. On 2026-09-02 17:16 and 2026-09-03 01:33 that file became a run
# of NUL bytes (27328/27328 and 2561/2561) after lanes were hard-killed while
# git was extending it — `fatal: bad config line 1` for every worktree at once.
# There was no restore point: the .bak sitting next to it had been copied
# AFTER the corruption, so it was all NULs too.
#
# The fix that makes a backup worth having is validating BEFORE copying. This
# script never writes a backup it has not parsed, and never reports a repair it
# did not make.
#
# usage:
#   pwsh -NoProfile -File scripts/fleet/git-config-guard.ps1 -RepoPath <repo> -Backup
#   pwsh -NoProfile -File scripts/fleet/git-config-guard.ps1 -RepoPath <repo> -Verify
#   pwsh -NoProfile -File scripts/fleet/git-config-guard.ps1 -RepoPath <repo> -Restore
#   pwsh -NoProfile -File scripts/fleet/git-config-guard.ps1 -RepoPath <repo> -VerifyOrRestore
#
# -RepoPath may be the main checkout or any linked worktree: the target is
# always the SHARED config (`git rev-parse --git-common-dir`), because that is
# the single file whose loss takes down every worktree.
#
# -Backup keeps one generation: the outgoing backup moves to <backup>.prev
# before the new one is written. Be precise about what that buys. The
# AUTOMATIC fallback only fires when the newest backup is DETECTABLY broken
# (case 10). It cannot fire for the case that motivates keeping a generation
# at all — a torn write ending on a boundary that still parses — because
# nothing can tell that file from a good one; there, .prev is the copy an
# operator points -BackupPath at by hand once they know the newest is wrong.
#
# A config that cannot be READ (held open by whatever is writing it) is a third
# outcome, not a synonym for corrupt: nothing is backed up from it and nothing
# is restored over it, because it may be perfectly good.
#
# Exit codes:
#   0  the config is healthy (backed up / verified / restored)
#   1  usage or resolution error (no mode, not a git repo, git unavailable)
#   2  the config is unhealthy — and for -Restore/-VerifyOrRestore, could not
#      be repaired (no usable backup, or the restored file still does not parse)
#   3  no backup was taken: the config is healthy but the copy failed
#   4  the config could not be judged at all, because it could not be read.
#      Distinct from 2 on purpose: nothing is known to be wrong with it
param(
  [string]$RepoPath = '.',
  [switch]$Backup,
  [switch]$Verify,
  [switch]$Restore,
  [switch]$VerifyOrRestore,
  [string]$BackupPath = ''
)

$ErrorActionPreference = 'Stop'
# This script decides on $LASTEXITCODE from `git`, so a non-zero native exit
# must not throw. The default is $false on the pwsh in use (7.6.5, measured),
# but it is settable per session and per profile, so state the contract here
# rather than inherit it.
$PSNativeCommandUseErrorActionPreference = $false

function Fail([string]$Msg, [int]$Code = 1) {
  [Console]::Error.WriteLine("git-config-guard: $Msg")
  exit $Code
}

$modes = @($Backup, $Verify, $Restore, $VerifyOrRestore) | Where-Object { $_ }
if ($modes.Count -ne 1) {
  Fail 'give exactly one of -Backup, -Verify, -Restore, -VerifyOrRestore'
}

# --- resolve the shared config ----------------------------------------------

if (-not (Test-Path -LiteralPath $RepoPath)) { Fail "-RepoPath '$RepoPath' does not exist" }
$RepoPath = (Resolve-Path -LiteralPath $RepoPath).Path

# Ask git first — it is authoritative and honours GIT_DIR and friends. But the
# case this tool exists for is exactly the one where git refuses to answer:
# once .git/config is a run of NULs, EVERY git command in the repo fails,
# rev-parse included. So fall back to walking the same links git would.
function Resolve-CommonDir([string]$Start) {
  # Collect the whole stream before reading $LASTEXITCODE: `Select-Object
  # -First` stops the pipeline early and leaves the exit code unset.
  $out = @(& git -C $Start rev-parse --path-format=absolute --git-common-dir 2>$null)
  if ($LASTEXITCODE -eq 0 -and $out.Count -gt 0) {
    $fromGit = "$($out[0])".Trim()
    if ($fromGit -and (Test-Path -LiteralPath $fromGit -PathType Container)) {
      return (Resolve-Path -LiteralPath $fromGit).Path
    }
  }

  # .git is a directory in the main checkout, and a file holding "gitdir: <p>"
  # in a linked worktree; <gitdir>/commondir then names the SHARED dir.
  $gitDir = $null
  $dir = $Start
  while ($dir) {
    $dotGit = Join-Path $dir '.git'
    if (Test-Path -LiteralPath $dotGit -PathType Container) { $gitDir = $dotGit; break }
    if (Test-Path -LiteralPath $dotGit -PathType Leaf) {
      $line = Get-Content -LiteralPath $dotGit -TotalCount 1
      if ($line -match '^\s*gitdir:\s*(.+?)\s*$') {
        $g = $Matches[1]
        if (-not [System.IO.Path]::IsPathRooted($g)) { $g = Join-Path $dir $g }
        $gitDir = $g
      }
      break
    }
    $parent = Split-Path -Parent $dir
    if (-not $parent -or $parent -eq $dir) { break }
    $dir = $parent
  }
  if (-not $gitDir -or -not (Test-Path -LiteralPath $gitDir -PathType Container)) { return $null }
  $gitDir = (Resolve-Path -LiteralPath $gitDir).Path

  $commonFile = Join-Path $gitDir 'commondir'
  if (Test-Path -LiteralPath $commonFile -PathType Leaf) {
    $rel = "$(Get-Content -LiteralPath $commonFile -TotalCount 1)".Trim()
    if ($rel) {
      $c = if ([System.IO.Path]::IsPathRooted($rel)) { $rel } else { Join-Path $gitDir $rel }
      if (Test-Path -LiteralPath $c -PathType Container) { return (Resolve-Path -LiteralPath $c).Path }
    }
  }
  return $gitDir
}

$commonDir = Resolve-CommonDir $RepoPath
if (-not $commonDir) {
  Fail "-RepoPath '$RepoPath' is not a git worktree (neither git rev-parse nor a .git link resolved a common dir)"
}
$ConfigPath = Join-Path $commonDir 'config'
if (-not $BackupPath) { $BackupPath = Join-Path $commonDir 'config.guard.bak' }

# --- health ------------------------------------------------------------------

# Healthy means all three: the file has content, holds no NUL byte, and git
# itself can parse it. The NUL check is what names the failure seen twice on
# 2026-09-03; `git config --list` is the criterion GH-715 asks a backup to meet.
#
# "Unreadable" is a THIRD outcome, not a synonym for unhealthy: a config held
# open by a process that is still writing it cannot be judged, and must not be
# overwritten from a backup on the strength of a failed read.
function Get-ConfigHealth([string]$Path) {
  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    return @{ Healthy = $false; Reason = 'file does not exist' }
  }
  try {
    $bytes = [System.IO.File]::ReadAllBytes($Path)
  } catch {
    return @{ Healthy = $false; Unreadable = $true; Reason = "cannot be read: $($_.Exception.Message)" }
  }
  if ($bytes.Length -eq 0) {
    return @{ Healthy = $false; Reason = 'file is empty' }
  }
  $nul = 0
  foreach ($b in $bytes) { if ($b -eq 0) { $nul++ } }
  if ($nul -gt 0) {
    return @{ Healthy = $false; Reason = "$nul NUL byte(s) in $($bytes.Length) — the GH-715 corruption shape" }
  }
  & git config --list --file $Path 2>&1 | Out-Null
  if ($LASTEXITCODE -ne 0) {
    return @{ Healthy = $false; Reason = 'git config --list cannot parse it' }
  }
  return @{ Healthy = $true; Reason = 'parses, no NUL bytes' }
}

# Copy through a sibling temp file so a reader never observes a half-written
# destination — the same class of hazard that produced the NUL config. The temp
# file is never left behind: a half-written .guard-tmp is the very thing this
# script exists to keep out of the git directory.
function Copy-Atomic([string]$From, [string]$To) {
  $tmp = "$To.guard-tmp"
  try {
    Copy-Item -LiteralPath $From -Destination $tmp -Force
    Move-Item -LiteralPath $tmp -Destination $To -Force
  } finally {
    if (Test-Path -LiteralPath $tmp -PathType Leaf) {
      Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
    }
  }
}

# A name no existing file holds, so a second restore inside the same second
# cannot overwrite the first one's evidence.
function New-UniquePath([string]$Path) {
  if (-not (Test-Path -LiteralPath $Path)) { return $Path }
  $n = 2
  while (Test-Path -LiteralPath "$Path.$n") { $n++ }
  return "$Path.$n"
}

$health = Get-ConfigHealth $ConfigPath

# --- -Verify -----------------------------------------------------------------

if ($Verify) {
  if ($health.Healthy) {
    "config=$ConfigPath healthy ($($health.Reason))"
    exit 0
  }
  if ($health.Unreadable) {
    Fail "config=$ConfigPath COULD NOT BE JUDGED: $($health.Reason)" 4
  }
  Fail "config=$ConfigPath UNHEALTHY: $($health.Reason)" 2
}

# --- -Backup -----------------------------------------------------------------

# One generation back. The automatic fallback below only fires when the newest
# backup is DETECTABLY broken; for a torn-but-parsing one nothing can tell it
# from a good copy, and .prev is then the file an operator restores by hand
# with -BackupPath. See the header.
$PrevBackupPath = "$BackupPath.prev"

if ($Backup) {
  if ($health.Unreadable) {
    # Nothing is known about the config, so nothing may be concluded from it:
    # do not overwrite the backup, and do not claim the config is broken.
    Fail "config could not be read, so no backup was taken ($($health.Reason)); $BackupPath left as it was" 4
  }
  if (-not $health.Healthy) {
    # Refusing here is the whole point: the real .bak files on this machine were
    # copied after the corruption and were therefore useless.
    Fail "refusing to back up an unhealthy config ($($health.Reason)); $BackupPath left as it was" 2
  }
  try {
    if ((Get-ConfigHealth $BackupPath).Healthy) { Copy-Atomic $BackupPath $PrevBackupPath }
    Copy-Atomic $ConfigPath $BackupPath
  } catch {
    Fail "config is healthy but the backup could not be written to ${BackupPath}: $($_.Exception.Message)" 3
  }
  "config=$ConfigPath healthy; backup=$BackupPath"
  exit 0
}

# --- -Restore / -VerifyOrRestore ---------------------------------------------

if ($health.Healthy) {
  "config=$ConfigPath healthy ($($health.Reason)); nothing restored"
  exit 0
}

if ($health.Unreadable) {
  # A locked config is not a corrupt one. Overwriting it here would destroy a
  # file that may be perfectly good and is currently being written.
  Fail "config=$ConfigPath could not be read, so it was NOT restored ($($health.Reason)); check what holds it open" 4
}

[Console]::Error.WriteLine("git-config-guard: config=$ConfigPath UNHEALTHY: $($health.Reason)")

$source = $null
foreach ($candidate in @($BackupPath, $PrevBackupPath)) {
  $candidateHealth = Get-ConfigHealth $candidate
  if ($candidateHealth.Healthy) { $source = $candidate; break }
  [Console]::Error.WriteLine("git-config-guard: unusable backup ${candidate}: $($candidateHealth.Reason)")
}
if (-not $source) {
  Fail "no usable backup to restore from (tried $BackupPath, $PrevBackupPath); config left untouched for forensics" 2
}

# Keep the corrupt file: it is the only evidence of what the kill did, and the
# name matches the ones already archived for the two 2026-09 incidents.
$stamp = Get-Date -Format 'yyyy-MM-ddTHH-mm-ss'
$preserved = New-UniquePath (Join-Path $commonDir "config.CORRUPT.$stamp.bak")
Copy-Item -LiteralPath $ConfigPath -Destination $preserved -Force

Copy-Atomic $source $ConfigPath

$after = Get-ConfigHealth $ConfigPath
if (-not $after.Healthy) {
  Fail "RESTORE FAILED: config still unhealthy after copying $source ($($after.Reason))" 2
}

"RESTORED config=$ConfigPath from backup=$source; corrupt file preserved at $preserved"
exit 0
