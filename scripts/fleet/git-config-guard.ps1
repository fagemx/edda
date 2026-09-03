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
# Exit codes:
#   0  the config is healthy (backed up / verified / restored)
#   1  usage or resolution error (no mode, not a git repo, git unavailable)
#   2  the config is unhealthy — and for -Restore/-VerifyOrRestore, could not
#      be repaired (no usable backup, or the restored file still does not parse)
#   3  the config is healthy but the backup could not be written
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
# must not throw (PowerShell 7.4 turns that on by default).
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
function Get-ConfigHealth([string]$Path) {
  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    return @{ Healthy = $false; Reason = 'file does not exist' }
  }
  $bytes = [System.IO.File]::ReadAllBytes($Path)
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
# destination — the same class of hazard that produced the NUL config.
function Copy-Atomic([string]$From, [string]$To) {
  $tmp = "$To.guard-tmp"
  Copy-Item -LiteralPath $From -Destination $tmp -Force
  Move-Item -LiteralPath $tmp -Destination $To -Force
}

$health = Get-ConfigHealth $ConfigPath

# --- -Verify -----------------------------------------------------------------

if ($Verify) {
  if ($health.Healthy) {
    "config=$ConfigPath healthy ($($health.Reason))"
    exit 0
  }
  Fail "config=$ConfigPath UNHEALTHY: $($health.Reason)" 2
}

# --- -Backup -----------------------------------------------------------------

if ($Backup) {
  if (-not $health.Healthy) {
    # Refusing here is the whole point: the real .bak files on this machine were
    # copied after the corruption and were therefore useless.
    Fail "refusing to back up an unhealthy config ($($health.Reason)); $BackupPath left as it was" 2
  }
  try {
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

[Console]::Error.WriteLine("git-config-guard: config=$ConfigPath UNHEALTHY: $($health.Reason)")

$backupHealth = Get-ConfigHealth $BackupPath
if (-not $backupHealth.Healthy) {
  Fail "no usable backup to restore from (backup=$BackupPath : $($backupHealth.Reason)); config left untouched for forensics" 2
}

# Keep the corrupt file: it is the only evidence of what the kill did, and the
# name matches the ones already archived for the two 2026-09 incidents.
$stamp = Get-Date -Format 'yyyy-MM-ddTHH-mm-ss'
$preserved = Join-Path $commonDir "config.CORRUPT.$stamp.bak"
Copy-Item -LiteralPath $ConfigPath -Destination $preserved -Force

Copy-Atomic $BackupPath $ConfigPath

$after = Get-ConfigHealth $ConfigPath
if (-not $after.Healthy) {
  Fail "RESTORE FAILED: config still unhealthy after copying $BackupPath ($($after.Reason))" 2
}

"RESTORED config=$ConfigPath from backup=$BackupPath; corrupt file preserved at $preserved"
exit 0
