# git-config-guard.ps1 — keep the shared .git/config and refs recoverable (GH-715, GH-797).
#
# The main checkout and all 30+ linked worktrees read ONE .git/config to find
# the remote, and shared git metadata under .git/refs to resolve branches.
# On 2026-09-02 and 2026-09-03 hard-killing lanes mid-write resulted in NUL-corrupted
# metadata files:
#   * .git/config became runs of NUL bytes (27328/27328 and 2561/2561), breaking
#     every worktree at once (GH-715).
#   * refs/heads/<branch> became 41 NUL bytes, breaking branch checkout and log
#     while rev-parse --is-inside-work-tree silently passed (GH-797).
#
# The defense:
#   * .git/config has no recovery source, so it requires a validated backup taken
#     BEFORE the lane can write, never backing up an unparsed file.
#   * Branch refs DO have a recovery source: git's own write-ahead reflog.
#     The detector is `git fsck --connectivity-only` (and loose ref inspection).
#     The repair is preserving the dead ref for forensics, removing it, and
#     restoring the SHA from the reflog via `git update-ref`.
#
# usage:
#   pwsh -NoProfile -File scripts/fleet/git-config-guard.ps1 -RepoPath <repo> -Backup
#   pwsh -NoProfile -File scripts/fleet/git-config-guard.ps1 -RepoPath <repo> -Verify
#   pwsh -NoProfile -File scripts/fleet/git-config-guard.ps1 -RepoPath <repo> -Restore
#   pwsh -NoProfile -File scripts/fleet/git-config-guard.ps1 -RepoPath <repo> -VerifyOrRestore
#   pwsh -NoProfile -File scripts/fleet/git-config-guard.ps1 -RepoPath <repo> -VerifyRefs
#   pwsh -NoProfile -File scripts/fleet/git-config-guard.ps1 -RepoPath <repo> -RestoreRefs
#
# -RepoPath may be the main checkout or any linked worktree: the target is
# always the SHARED config and refs (`git rev-parse --git-common-dir`), because that
# is the single directory whose loss takes down every worktree.
#
# Exit codes:
#   0  metadata is healthy (backed up / verified / restored)
#   1  usage or resolution error (no mode, not a git repo, git unavailable)
#   2  metadata is unhealthy — and for -Restore/-VerifyOrRestore, could not
#      be repaired (no usable backup/reflog, or the restored state still does not parse)
#   3  no backup was taken: the config is healthy but the copy failed
#   4  config could not be judged at all, because it could not be read.
#      Distinct from 2 on purpose: nothing is known to be wrong with it
param(
  [string]$RepoPath = '.',
  [switch]$Backup,
  [switch]$Verify,
  [switch]$Restore,
  [switch]$VerifyOrRestore,
  [switch]$VerifyRefs,
  [switch]$RestoreRefs,
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

$modes = @($Backup, $Verify, $Restore, $VerifyOrRestore, $VerifyRefs, $RestoreRefs) | Where-Object { $_ }
if ($modes.Count -ne 1) {
  Fail 'give exactly one of -Backup, -Verify, -Restore, -VerifyOrRestore, -VerifyRefs, -RestoreRefs'
}

# --- resolve the shared config & worktree gitdir ----------------------------

if (-not (Test-Path -LiteralPath $RepoPath)) { Fail "-RepoPath '$RepoPath' does not exist" }
$RepoPath = (Resolve-Path -LiteralPath $RepoPath).Path

$script:WorktreeGitDir = $null

function Resolve-CommonDir([string]$Start) {
  # Collect the whole stream before reading $LASTEXITCODE: `Select-Object
  # -First` stops the pipeline early and leaves the exit code unset.
  $out = @(& git -C $Start rev-parse --path-format=absolute --git-common-dir --git-dir 2>$null)
  if ($LASTEXITCODE -eq 0 -and $out.Count -ge 2) {
    $fromGitCommon = "$($out[0])".Trim()
    $fromGitDir = "$($out[1])".Trim()
    if ($fromGitDir -and (Test-Path -LiteralPath $fromGitDir -PathType Container)) {
      $script:WorktreeGitDir = (Resolve-Path -LiteralPath $fromGitDir).Path
    }
    if ($fromGitCommon -and (Test-Path -LiteralPath $fromGitCommon -PathType Container)) {
      return (Resolve-Path -LiteralPath $fromGitCommon).Path
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
  $script:WorktreeGitDir = $gitDir

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
$PrevBackupPath = "$BackupPath.prev"

# --- config health -----------------------------------------------------------

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

# Copy through a sibling temp file so a reader never observes a half-written destination.
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

function New-UniquePath([string]$Path) {
  if (-not (Test-Path -LiteralPath $Path)) { return $Path }
  $n = 2
  while (Test-Path -LiteralPath "$Path.$n") { $n++ }
  return "$Path.$n"
}

# --- refs health & reflog recovery (GH-797) ----------------------------------

function Resolve-RefRepairSha([string]$Repo, [string]$CommonDir, [string]$Ref) {
  $normalizedRef = $Ref -replace '\\', '/'
  $candidates = New-Object System.Collections.Generic.List[string]

  # 1. Primary reflog: <commonDir>/logs/<ref>
  $primaryReflog = Join-Path $CommonDir "logs\$($normalizedRef -replace '/', '\')"
  if (Test-Path -LiteralPath $primaryReflog -PathType Leaf) {
    $candidates.Add($primaryReflog)
  }

  # 2. Worktree HEAD reflog (if this worktree's HEAD points to this ref)
  if ($script:WorktreeGitDir) {
    $wtHeadPath = Join-Path $script:WorktreeGitDir 'HEAD'
    if (Test-Path -LiteralPath $wtHeadPath -PathType Leaf) {
      $wtHeadContent = "$(Get-Content -LiteralPath $wtHeadPath -TotalCount 1)".Trim()
      if ($wtHeadContent -match '^\s*ref:\s*(.+?)\s*$' -and ($Matches[1] -replace '\\', '/') -eq $normalizedRef) {
        $wtHeadLog = Join-Path $script:WorktreeGitDir 'logs\HEAD'
        if (Test-Path -LiteralPath $wtHeadLog -PathType Leaf) {
          $candidates.Add($wtHeadLog)
        }
      }
    }
  }

  # 3. Common HEAD reflog (if common HEAD points to this ref)
  $commonHeadPath = Join-Path $CommonDir 'HEAD'
  if (Test-Path -LiteralPath $commonHeadPath -PathType Leaf) {
    $commonHeadContent = "$(Get-Content -LiteralPath $commonHeadPath -TotalCount 1)".Trim()
    if ($commonHeadContent -match '^\s*ref:\s*(.+?)\s*$' -and ($Matches[1] -replace '\\', '/') -eq $normalizedRef) {
      $commonHeadLog = Join-Path $CommonDir 'logs\HEAD'
      if (Test-Path -LiteralPath $commonHeadLog -PathType Leaf) {
        $candidates.Add($commonHeadLog)
      }
    }
  }

  foreach ($logPath in $candidates) {
    $lines = @(Get-Content -LiteralPath $logPath -ErrorAction SilentlyContinue)
    for ($i = $lines.Count - 1; $i -ge 0; $i--) {
      $line = $lines[$i]
      $parts = $line -split '\s+'
      if ($parts.Count -ge 2) {
        $candidateSha = $parts[1]
        if ($candidateSha -match '^[0-9a-fA-F]{40}$' -and $candidateSha -ne '0000000000000000000000000000000000000000') {
          & git -C $Repo cat-file -e $candidateSha 2>&1 | Out-Null
          if ($LASTEXITCODE -eq 0) {
            return @{ Sha = $candidateSha; Source = $logPath }
          }
        }
      }
    }
  }

  return @{ Sha = $null; Source = $null }
}

function Get-RefsHealth([string]$Repo, [string]$CommonDir) {
  $fsckRaw = @(& git -C $Repo fsck --connectivity-only 2>&1)
  $fsckExit = $LASTEXITCODE
  if ($fsckExit -eq 0) {
    return @{ Healthy = $true; Reason = 'connectivity ok' }
  }

  # Parse fsck output for broken refs
  $brokenMap = @{}
  foreach ($line in $fsckRaw) {
    if ($line -match 'error:\s+(refs/[^\s:]+):\s+(.*)') {
      $r = ($Matches[1].Trim() -replace '\\', '/')
      $msg = $Matches[2].Trim()
      if (-not $brokenMap.ContainsKey($r)) {
        $brokenMap[$r] = $msg
      }
    }
  }

  # Also inspect loose ref files under $CommonDir/refs/heads for NULs or bad length
  $headsDir = Join-Path $CommonDir 'refs\heads'
  if (Test-Path -LiteralPath $headsDir -PathType Container) {
    $refFiles = Get-ChildItem -LiteralPath $headsDir -Recurse -File -ErrorAction SilentlyContinue
    foreach ($rf in $refFiles) {
      $relPath = ($rf.FullName.Substring($CommonDir.Length).TrimStart('\', '/') -replace '\\', '/')
      if (-not $brokenMap.ContainsKey($relPath)) {
        try {
          $bytes = [System.IO.File]::ReadAllBytes($rf.FullName)
          $hasNul = $false
          foreach ($b in $bytes) { if ($b -eq 0) { $hasNul = $true; break } }
          if ($hasNul) {
            $brokenMap[$relPath] = "$($bytes.Length) bytes, contains NUL bytes (GH-797)"
          }
        } catch {}
      }
    }
  }

  if ($brokenMap.Count -eq 0) {
    $firstLine = if ($fsckRaw.Count -gt 0) { "$($fsckRaw[0])" } else { "git fsck exit $fsckExit" }
    return @{
      Healthy = $false
      BrokenRefs = @()
      Reason = "git fsck failed: $firstLine"
      CanRepair = $false
      CanRepairAny = $false
    }
  }

  $brokenList = New-Object System.Collections.Generic.List[hashtable]
  $allHaveRepairs = $true
  $anyHaveRepairs = $false
  $reasons = @()
  foreach ($r in $brokenMap.Keys) {
    $repairInfo = Resolve-RefRepairSha $Repo $CommonDir $r
    $item = @{
      Ref = $r
      Error = $brokenMap[$r]
      RepairSha = $repairInfo.Sha
      RepairSource = $repairInfo.Source
    }
    $brokenList.Add($item)
    if ($repairInfo.Sha) {
      $anyHaveRepairs = $true
      $reasons += "$r is broken ($($brokenMap[$r])); repair available from $($repairInfo.Source): $($repairInfo.Sha)"
    } else {
      $allHaveRepairs = $false
      $reasons += "$r is broken ($($brokenMap[$r])); no repair SHA found in reflog"
    }
  }

  return @{
    Healthy = $false
    BrokenRefs = @($brokenList)
    Reason = ($reasons -join '; ')
    CanRepair = $allHaveRepairs
    CanRepairAny = $anyHaveRepairs
  }
}

function Restore-BrokenRefs([string]$Repo, [string]$CommonDir, $BrokenRefs) {
  if (@($BrokenRefs).Count -eq 0) {
    return @{
      Success = $false
      Restored = @()
      Reason = "no broken refs identified to restore"
    }
  }

  $restored = @()
  $failedRefs = @()
  foreach ($b in $BrokenRefs) {
    if (-not $b.RepairSha) {
      $failedRefs += "$($b.Ref) (no repair SHA available)"
      continue
    }
    $refName = $b.Ref
    $sha = $b.RepairSha

    # Preserve corrupt file outside .git/refs to avoid tripping git fsck
    $targetFile = Join-Path $CommonDir ($refName -replace '/', '\')
    $sanitized = $refName -replace '[/\\:]', '_'
    $stamp = Get-Date -Format 'yyyy-MM-ddTHH-mm-ss'
    $preserved = Join-Path $CommonDir "ref.CORRUPT.$sanitized.$stamp.bak"
    $preserved = New-UniquePath $preserved

    if (Test-Path -LiteralPath $targetFile) {
      Copy-Item -LiteralPath $targetFile -Destination $preserved -Force
      Remove-Item -LiteralPath $targetFile -Force
    }

    & git -C $Repo update-ref $refName $sha
    if ($LASTEXITCODE -ne 0) {
      $failedRefs += "$refName (git update-ref failed with exit $LASTEXITCODE)"
      continue
    }
    $restored += "RESTORED ref=$refName to sha=$sha from $($b.RepairSource); corrupt file preserved at $preserved"
  }

  if ($failedRefs.Count -gt 0) {
    return @{
      Success = $false
      Restored = $restored
      Reason = "cannot repair refs: $($failedRefs -join '; ')"
    }
  }

  $after = Get-RefsHealth $Repo $CommonDir
  if (-not $after.Healthy) {
    return @{
      Success = $false
      Restored = $restored
      Reason = "refs still unhealthy after restoration: $($after.Reason)"
    }
  }

  return @{
    Success = $true
    Restored = $restored
    Reason = 'all refs restored and verified healthy'
  }
}

# --- -VerifyRefs -------------------------------------------------------------

if ($VerifyRefs) {
  $refsHealth = Get-RefsHealth $RepoPath $commonDir
  if (-not $refsHealth.Healthy) {
    $repairHint = if ($refsHealth.CanRepair) {
      "repair available from reflog: run scripts/fleet/git-config-guard.ps1 -RepoPath '$RepoPath' -RestoreRefs"
    } elseif ($refsHealth.CanRepairAny) {
      "partial repair available via scripts/fleet/git-config-guard.ps1 -RepoPath '$RepoPath' -RestoreRefs; unrepairable refs require manual inspection of .git"
    } else {
      "automatic reflog repair unavailable; inspect repo manually"
    }
    Fail "refs UNHEALTHY: $($refsHealth.Reason) ($repairHint)" 2
  }
  "refs=healthy ($($refsHealth.Reason))"
  exit 0
}

# --- -RestoreRefs ------------------------------------------------------------

if ($RestoreRefs) {
  $refsHealth = Get-RefsHealth $RepoPath $commonDir
  if ($refsHealth.Healthy) {
    "refs healthy ($($refsHealth.Reason)); nothing restored"
    exit 0
  }
  [Console]::Error.WriteLine("git-config-guard: refs UNHEALTHY: $($refsHealth.Reason)")
  $restoreResult = Restore-BrokenRefs $RepoPath $commonDir $refsHealth.BrokenRefs
  foreach ($r in $restoreResult.Restored) {
    "$r"
  }
  if (-not $restoreResult.Success) {
    Fail "RESTORE FAILED: $($restoreResult.Reason)" 2
  }
  exit 0
}

# --- -Verify -----------------------------------------------------------------

$configHealth = Get-ConfigHealth $ConfigPath

if ($Verify) {
  if ($configHealth.Unreadable) {
    Fail "config=$ConfigPath COULD NOT BE JUDGED: $($configHealth.Reason)" 4
  }
  if (-not $configHealth.Healthy) {
    Fail "config=$ConfigPath UNHEALTHY: $($configHealth.Reason)" 2
  }
  $refsHealth = Get-RefsHealth $RepoPath $commonDir
  if (-not $refsHealth.Healthy) {
    $repairHint = if ($refsHealth.CanRepair) {
      "repair available from reflog: run scripts/fleet/git-config-guard.ps1 -RepoPath '$RepoPath' -RestoreRefs"
    } elseif ($refsHealth.CanRepairAny) {
      "partial repair available via scripts/fleet/git-config-guard.ps1 -RepoPath '$RepoPath' -RestoreRefs; unrepairable refs require manual inspection of .git"
    } else {
      "automatic reflog repair unavailable; inspect repo manually"
    }
    Fail "refs UNHEALTHY: $($refsHealth.Reason) ($repairHint)" 2
  }
  "config=$ConfigPath healthy ($($configHealth.Reason))"
  "refs=healthy ($($refsHealth.Reason))"
  exit 0
}

# --- -Backup -----------------------------------------------------------------

if ($Backup) {
  if ($configHealth.Unreadable) {
    Fail "config could not be read, so no backup was taken ($($configHealth.Reason)); $BackupPath left as it was" 4
  }
  if (-not $configHealth.Healthy) {
    Fail "refusing to back up an unhealthy config ($($configHealth.Reason)); $BackupPath left as it was" 2
  }
  $refsHealth = Get-RefsHealth $RepoPath $commonDir
  if (-not $refsHealth.Healthy) {
    $repairHint = if ($refsHealth.CanRepair) {
      "repair with scripts/fleet/git-config-guard.ps1 -RepoPath '$RepoPath' -RestoreRefs"
    } elseif ($refsHealth.CanRepairAny) {
      "partial repair available via scripts/fleet/git-config-guard.ps1 -RepoPath '$RepoPath' -RestoreRefs; unrepairable refs require manual inspection of .git"
    } else {
      "automatic repair from reflog unavailable; manual inspection of .git required"
    }
    Fail "ref(s) unhealthy ($($refsHealth.Reason)); cannot guarantee repo consistency — $repairHint" 2
  }
  try {
    if ((Get-ConfigHealth $BackupPath).Healthy) { Copy-Atomic $BackupPath $PrevBackupPath }
    Copy-Atomic $ConfigPath $BackupPath
  } catch {
    Fail "config is healthy but the backup could not be written to ${BackupPath}: $($_.Exception.Message)" 3
  }
  "config=$ConfigPath healthy; backup=$BackupPath"
  "refs=healthy ($($refsHealth.Reason))"
  exit 0
}

# --- -Restore / -VerifyOrRestore ---------------------------------------------

$configRestored = $false
if (-not $configHealth.Healthy) {
  if ($configHealth.Unreadable) {
    Fail "config=$ConfigPath could not be read, so it was NOT restored ($($configHealth.Reason)); check what holds it open" 4
  }
  [Console]::Error.WriteLine("git-config-guard: config=$ConfigPath UNHEALTHY: $($configHealth.Reason)")

  $source = $null
  foreach ($candidate in @($BackupPath, $PrevBackupPath)) {
    $candidateHealth = Get-ConfigHealth $candidate
    if ($candidateHealth.Healthy) { $source = $candidate; break }
    [Console]::Error.WriteLine("git-config-guard: unusable backup ${candidate}: $($candidateHealth.Reason)")
  }
  if (-not $source) {
    Fail "no usable backup to restore from (tried $BackupPath, $PrevBackupPath); config left untouched for forensics" 2
  }

  $stamp = Get-Date -Format 'yyyy-MM-ddTHH-mm-ss'
  $preserved = New-UniquePath (Join-Path $commonDir "config.CORRUPT.$stamp.bak")
  Copy-Item -LiteralPath $ConfigPath -Destination $preserved -Force
  Copy-Atomic $source $ConfigPath

  $after = Get-ConfigHealth $ConfigPath
  if (-not $after.Healthy) {
    Fail "RESTORE FAILED: config still unhealthy after copying $source ($($after.Reason))" 2
  }
  "RESTORED config=$ConfigPath from backup=$source; corrupt file preserved at $preserved"
  $configRestored = $true
} else {
  "config=$ConfigPath healthy ($($configHealth.Reason)); nothing restored"
}

# Check and restore refs
$refsHealth = Get-RefsHealth $RepoPath $commonDir
if (-not $refsHealth.Healthy) {
  [Console]::Error.WriteLine("git-config-guard: refs UNHEALTHY: $($refsHealth.Reason)")
  $restoreResult = Restore-BrokenRefs $RepoPath $commonDir $refsHealth.BrokenRefs
  foreach ($r in $restoreResult.Restored) {
    "$r"
  }
  if (-not $restoreResult.Success) {
    Fail "RESTORE FAILED: $($restoreResult.Reason)" 2
  }
} else {
  "refs healthy ($($refsHealth.Reason)); nothing restored"
}

exit 0
