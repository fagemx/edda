# Fixture for scripts/fleet/digest-schedule.ps1 (GH-765).
#
# Contract pinned here:
#   * -DryRun has ZERO side effects — no task registered, no wrapper written,
#     no log directory created — and prints the daily trigger it would create,
#   * the wrapper it would write runs daily-digest.sh against the named board
#     and appends a terminal receipt, so a day with no digest is
#     distinguishable from a day the task never fired,
#   * every refusal is loud and precedes registration: no board, a bad -At,
#     a -Cwd that is not a worktree,
#   * -Status on an absent task reports absent rather than failing.
#
# The scheduler cmdlets are mocked in a child pwsh: a fixture must never
# register, start or unregister a real Scheduled Task.
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$schedule = Join-Path $root 'scripts\fleet\digest-schedule.ps1'
$scratch = Join-Path $env:TEMP "gh765-digest-schedule-$PID"
$failures = [System.Collections.Generic.List[string]]::new()
function Assert-True($ok, [string]$what) { if ($ok) { "PASS: $what" } else { "FAIL: $what"; $failures.Add($what) | Out-Null } }

$mocks = @'
$ErrorActionPreference = 'Stop'
$global:registrations = 0
function New-ScheduledTaskAction { param($Execute) [pscustomobject]@{} }
function New-ScheduledTaskTrigger { param($At) [pscustomobject]@{} }
function New-ScheduledTaskSettingsSet { [pscustomobject]@{} }
function Register-ScheduledTask { param([string]$TaskName) $global:registrations++; [pscustomobject]@{} }
function Unregister-ScheduledTask { param([string]$TaskName) }
function Get-ScheduledTask { [CmdletBinding()] param([string]$TaskName) $null }
function Get-ScheduledTaskInfo { [CmdletBinding()] param([string]$TaskName) $null }
'@

function Invoke-Schedule([string[]]$ScheduleArgs) {
  $driver = Join-Path $scratch 'driver.ps1'
  # Parameter names must reach the script UNQUOTED: a quoted '-Cwd' binds
  # positionally instead of naming the parameter. Values are quoted.
  $argText = ($ScheduleArgs | ForEach-Object {
    if ($_ -match '^-[A-Za-z]') { $_ } else { "'" + ($_ -replace "'", "''") + "'" }
  }) -join ' '
  ($mocks + "`n& `$env:GH765_SCHEDULE $argText`nexit `$LASTEXITCODE`n") |
    Set-Content -LiteralPath $driver -Encoding utf8
  $out = (& pwsh -NoProfile -NonInteractive -File $driver 2>&1 | Out-String)
  return [pscustomobject]@{ Out = $out; Exit = $LASTEXITCODE }
}

New-Item -ItemType Directory -Force -Path $scratch | Out-Null
try {
  $repo = Join-Path $scratch 'repo'
  New-Item -ItemType Directory -Force -Path $repo | Out-Null
  & git -C $repo init -q
  & git -C $repo config user.email fixture@example.invalid
  & git -C $repo config user.name fixture
  New-Item -ItemType Directory -Force -Path (Join-Path $repo 'scripts\fleet') | Out-Null
  Set-Content -LiteralPath (Join-Path $repo 'scripts\fleet\daily-digest.sh') -Value '#!/bin/sh'
  & git -C $repo add -A
  & git -C $repo commit -qm fixture

  $env:GH765_SCHEDULE = $schedule
  $logDir = Join-Path $scratch 'logs'

  # --- -DryRun has zero side effects ---------------------------------------
  $dry = Invoke-Schedule @('-Cwd', $repo, '-Board', '888', '-At', '09:30', '-LogDir', $logDir, '-DryRun')
  Assert-True ($dry.Exit -eq 0) "dry run exits 0; output was:`n$($dry.Out)"
  Assert-True ($dry.Out -match 'dry-run: trigger: daily at 09:30') "dry run prints the daily trigger; output was:`n$($dry.Out)"
  Assert-True ($dry.Out -match 'dry-run: task=edda-digest') 'dry run names the task it would register'
  Assert-True (-not (Test-Path $logDir)) 'dry run creates no log directory'
  Assert-True ($dry.Out -notmatch '(?m)^task=edda-digest state=') 'dry run reports no registered state'

  # The wrapper is the contract: it must run the digest against the named
  # board and leave a receipt every firing.
  Assert-True ($dry.Out -match 'daily-digest\.sh.* --board 888') "the wrapper runs the digest against the named board; output was:`n$($dry.Out)"
  Assert-True ($dry.Out -match '=== DIGEST EXIT code=') 'the wrapper appends a terminal receipt each firing'
  Assert-True ($dry.Out -match 'Tee-Object -FilePath') 'the wrapper tees the digest output to the log'

  # A RELATIVE -LogDir must reach the task action already absolute. Task
  # Scheduler resolves a relative -File against the task's own working
  # directory (here -Cwd, the worktree), while the wrapper is written
  # relative to the caller's location — two different places, so every firing
  # dies 0x80070002 before the wrapper can append its receipt, destroying the
  # one signal that tells "ran and failed" from "never fired". Registration
  # still looks successful, which is what makes this worth pinning.
  $rel = Invoke-Schedule @('-Cwd', $repo, '-Board', '888', '-At', '09:30', '-LogDir', 'reldir', '-DryRun')
  Assert-True ($rel.Exit -eq 0) "a relative -LogDir is accepted; output was:`n$($rel.Out)"
  $relAction = ($rel.Out -split "`r?`n" | Where-Object { $_ -match '^dry-run: action: ' }) -join ''
  # A drive letter and colon is enough to prove it is no longer relative.
  Assert-True ($relAction -match '-File "[A-Za-z]:') `
    "a relative -LogDir reaches the action as an absolute path; action was:`n$relAction"

  # --- refusals precede registration ---------------------------------------
  $noBoard = Invoke-Schedule @('-Cwd', $repo, '-At', '09:30', '-LogDir', $logDir, '-DryRun')
  Assert-True ($noBoard.Exit -ne 0) 'a missing board issue is refused'
  Assert-True ($noBoard.Out -match '(?i)board issue number is required') "the refusal says what is missing; output was:`n$($noBoard.Out)"

  $badAt = Invoke-Schedule @('-Cwd', $repo, '-Board', '888', '-At', 'lunchtime', '-LogDir', $logDir, '-DryRun')
  Assert-True ($badAt.Exit -ne 0) 'a time that is not HH:mm is refused rather than coerced'
  Assert-True ($badAt.Out -match "(?i)not a 24-hour HH:mm time") "the refusal names the expected format; output was:`n$($badAt.Out)"

  $notRepo = Invoke-Schedule @('-Cwd', $scratch, '-Board', '888', '-LogDir', $logDir, '-DryRun')
  Assert-True ($notRepo.Exit -ne 0) 'a -Cwd that is not a git worktree is refused'

  # --- -Status on an absent task is not a failure --------------------------
  $status = Invoke-Schedule @('-Cwd', $repo, '-Status')
  Assert-True ($status.Exit -eq 0) 'status on an absent task exits 0'
  Assert-True ($status.Out -match 'task=edda-digest state=absent') "status reports absence plainly; output was:`n$($status.Out)"

  # --- -Unregister needs no board and no clock -----------------------------
  $unreg = Invoke-Schedule @('-Cwd', $repo, '-Unregister')
  Assert-True ($unreg.Exit -eq 0) "unregister exits 0; output was:`n$($unreg.Out)"
  Assert-True ($unreg.Out -match 'task=edda-digest unregistered') 'unregister says so'
}
finally {
  Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failures.Count -gt 0) {
  [Console]::Error.WriteLine("digest-schedule fixture: $($failures.Count) failure(s)")
  exit 1
}
'digest-schedule fixtures passed'
exit 0
