# Delivery-hygiene fixture for lane-launch.ps1 and lane-status.ps1 (GH-748 log
# preservation and terminated-lane reporting, GH-694 session id shape).  Every
# scenario runs the production script in a child pwsh with the scheduler
# cmdlets mocked; no real Scheduled Task is registered, started or
# unregistered, and no agent is dispatched (a stub `edda` on PATH stands in).
#
# Contract pinned here:
#   * a lane killed before its wrapper's finally block still leaves on disk
#     every line the lane printed (GH-748: three lanes died at the 2700s
#     timeout with logBytes=0, so their cause of death was unrecoverable),
#   * lane-status names the scheduler result and says outright that such a
#     lane delivered nothing, instead of done=False beside a bare error code,
#   * -Agent claude refuses a session id the claude backend would reject, and
#     its default session id is a UUID stable across relaunches of a lane.
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$launch = Join-Path $root 'scripts\fleet\lane-launch.ps1'
$status = Join-Path $root 'scripts\fleet\lane-status.ps1'
$scratch = Join-Path $env:TEMP "gh748-delivery-$PID"
$failures = [System.Collections.Generic.List[string]]::new()
function Assert-True($ok, [string]$what) { if ($ok) { "PASS: $what" } else { "FAIL: $what"; $failures.Add($what) | Out-Null } }

# The scheduler mocks every driver installs.  A fixture must never register,
# start or unregister a real Scheduled Task: the workstation runs live lanes.
$mocks = @'
$ErrorActionPreference = 'Stop'
function New-ScheduledTaskAction { param($Execute) [pscustomobject]@{} }
function New-ScheduledTaskSettingsSet { [pscustomobject]@{} }
$global:registered = $false
function Register-ScheduledTask { param([string]$TaskName) $global:registered = $true; [pscustomobject]@{} }
function Start-ScheduledTask { param([string]$TaskName) }
function Unregister-ScheduledTask { param([string]$TaskName) }
'@

New-Item -ItemType Directory -Force -Path $scratch | Out-Null
try {
  # A real throwaway repository lets the production git/config guard run
  # normally; all task-service mutations remain mocked.
  $repo = Join-Path $scratch 'repo'
  New-Item -ItemType Directory -Force -Path $repo | Out-Null
  & git -C $repo init -q
  & git -C $repo config user.email fixture@example.invalid
  & git -C $repo config user.name fixture
  Set-Content -LiteralPath (Join-Path $repo 'README.md') -Value fixture
  & git -C $repo add README.md
  & git -C $repo commit -qm fixture
  $brief = Join-Path $scratch 'brief.md'; Set-Content -LiteralPath $brief -Value '# fixture'

  # --- GH-694: -Agent claude refuses a session id the backend rejects -------
  # The claude backend answers a non-UUID with "Error: Invalid session ID.
  # Must be a valid UUID." and the lane dies before it reads its brief, so a
  # launcher that registers the task anyway has started nothing.
  $rejectLog = Join-Path $scratch 'reject'; New-Item -ItemType Directory -Force -Path $rejectLog | Out-Null
  $rejectDriver = Join-Path $scratch 'reject-driver.ps1'
  ($mocks + @'

function Get-ScheduledTask { [CmdletBinding()] param([string]$TaskName) $null }
function Get-CimInstance { param([Parameter(Position = 0)]$ClassName, [string]$Filter) @() }
& $env:GH748_LAUNCH -Name gh694-reject -Brief $env:GH748_BRIEF -Cwd $env:GH748_REPO -Agent claude -SessionId probe-1 -LogDir $env:GH748_LOG -DryRun -Owns scripts/fleet/lane-launch.ps1
exit $LASTEXITCODE
'@) | Set-Content -LiteralPath $rejectDriver -Encoding utf8
  $env:GH748_LAUNCH = $launch; $env:GH748_BRIEF = $brief; $env:GH748_REPO = $repo; $env:GH748_LOG = $rejectLog
  $rejectText = (& pwsh -NoProfile -NonInteractive -File $rejectDriver 2>&1 | Out-String)
  $rejectExit = $LASTEXITCODE
  Assert-True ($rejectExit -ne 0) "claude lane with a non-UUID session id exits nonzero; output was:`n$rejectText"
  Assert-True ($rejectText -match '(?i)uuid') "the refusal names the UUID requirement; output was:`n$rejectText"
  Assert-True (-not (Test-Path (Join-Path $rejectLog 'gh694-reject.dryrun-wrapper.ps1'))) 'the refusal precedes any wrapper being written'

  # --- GH-694: the default session id is a UUID, and it is stable -----------
  # The scheduler is mocked far enough for -DryRun to print the real command
  # line; the dry-run receipt is pre-seeded so the self-unregister path exits 0.
  $uuidDriver = Join-Path $scratch 'uuid-driver.ps1'
  ($mocks + @'

function Get-ScheduledTask { [CmdletBinding()] param([string]$TaskName) $null }
function Get-CimInstance {
  param([Parameter(Position = 0)]$ClassName, [string]$Filter)
  if ($Filter -like '*ProcessId=*') { return [pscustomobject]@{ Name = 'svchost.exe' } }
  [pscustomobject]@{ ProcessId = 4242; ParentProcessId = 4; CommandLine = "-File $env:GH748_WRAPPER" }
}
& $env:GH748_LAUNCH -Name $env:GH748_NAME -Cwd $env:GH748_REPO -Agent claude -LogDir $env:GH748_LOG -DryRun -Owns scripts/fleet/lane-launch.ps1
'@) | Set-Content -LiteralPath $uuidDriver -Encoding utf8
  $uuidLog = Join-Path $scratch 'uuid'; New-Item -ItemType Directory -Force -Path $uuidLog | Out-Null
  $env:GH748_LOG = $uuidLog
  $uuidRe = "--session-id '([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})'"
  function Invoke-DryRunLane([string]$LaneName) {
    $env:GH748_NAME = $LaneName
    $env:GH748_WRAPPER = Join-Path $uuidLog "$LaneName.dryrun-wrapper.ps1"
    Set-Content -LiteralPath (Join-Path $uuidLog "$LaneName.dryrun.done") -Value '0' -Encoding ascii
    return (& pwsh -NoProfile -NonInteractive -File $uuidDriver 2>&1) -join "`n"
  }
  $uuidOut = Invoke-DryRunLane 'gh694-default'
  Assert-True ($uuidOut -match $uuidRe) "the default claude session id is a UUID; dry-run output was:`n$uuidOut"
  $firstUuid = if ($uuidOut -match $uuidRe) { $Matches[1] } else { '' }
  $repeatOut = Invoke-DryRunLane 'gh694-default'
  $repeatUuid = if ($repeatOut -match $uuidRe) { $Matches[1] } else { '' }
  Assert-True ($firstUuid -ne '' -and $firstUuid -eq $repeatUuid) 'the derived session id is stable across relaunches of the same lane'
  $otherOut = Invoke-DryRunLane 'gh694-other'
  $otherUuid = if ($otherOut -match $uuidRe) { $Matches[1] } else { '' }
  Assert-True ($otherUuid -ne '' -and $otherUuid -ne $firstUuid) 'a different lane derives a different session id'

  # --- GH-748: a killed lane still leaves the lines it printed -------------
  # The three lanes killed at the 2700s timeout on 2026-09-03 all reported
  # logBytes=0.  The suspected cause was the wrapper's Tee-Object buffering,
  # but this fixture disproves that: with an agent that emits as it runs, the
  # line survives the kill on the pre-GH-748 wrapper too.  The real cause is
  # upstream — `edda dispatch` hard-coded verbose:false, so the agent's
  # activity reached stdout only when the turn ended, and a lane killed before
  # that had printed nothing for the wrapper to write.  Both halves are pinned
  # here: the launcher must ask dispatch to stream, and the wrapper must land
  # what it streams on disk before the kill.
  #
  # The real launch path writes the production wrapper with the scheduler
  # mocked, so nothing is registered or started.  The wrapper is then run
  # directly and killed while the stub agent is still alive — the shape of a
  # lane terminated at its execution limit, where finally never runs.
  $stubDir = Join-Path $scratch 'stub'; New-Item -ItemType Directory -Force -Path $stubDir | Out-Null
  Set-Content -LiteralPath (Join-Path $stubDir 'edda.cmd') -Encoding ascii -Value @(
    '@echo off'
    'echo LANE-EMITTED-LINE'
    'ping -n 60 127.0.0.1 >nul'
  )
  $killLog = Join-Path $scratch 'kill'; New-Item -ItemType Directory -Force -Path $killLog | Out-Null
  $killDriver = Join-Path $scratch 'kill-driver.ps1'
  ($mocks + @'

# Absent until registration, Running after it: the launcher's pre-launch
# busy gate must see no task, and its post-start check must see a live one.
function Get-ScheduledTask { [CmdletBinding()] param([string]$TaskName) if ($global:registered) { [pscustomobject]@{ TaskName = $TaskName; State = 'Running' } } }
& $env:GH748_LAUNCH -Name gh748-kill -Brief $env:GH748_BRIEF -Cwd $env:GH748_REPO -Agent pi -LogDir $env:GH748_LOG -Owns scripts/fleet/lane-launch.ps1
'@) | Set-Content -LiteralPath $killDriver -Encoding utf8
  $env:GH748_BRIEF = $brief; $env:GH748_LOG = $killLog
  $killLaunchOut = (& pwsh -NoProfile -NonInteractive -File $killDriver 2>&1 | Out-String)
  $killWrapper = Join-Path $killLog 'gh748-kill.wrapper.ps1'
  $killLogFile = Join-Path $killLog 'gh748-kill.log'
  Assert-True (Test-Path $killWrapper) "the real launch path wrote the production wrapper; launcher said:`n$killLaunchOut"
  $wrapperText = if (Test-Path $killWrapper) { Get-Content -LiteralPath $killWrapper -Raw } else { '' }
  Assert-True ($wrapperText -match 'dispatch --verbose') 'the lane asks dispatch to stream the agent activity it will be diagnosed by'
  if (Test-Path $killWrapper) {
    $oldPath = $env:PATH
    $env:PATH = "$stubDir;$env:PATH"
    try {
      $proc = Start-Process -FilePath (Get-Command pwsh.exe).Source `
        -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $killWrapper) `
        -PassThru -WindowStyle Hidden
      Start-Sleep -Seconds 6
      $procs = @(Get-CimInstance Win32_Process)
      $tree = [System.Collections.Generic.List[int]]::new()
      [void]$tree.Add([int]$proc.Id)
      for ($i = 0; $i -lt $tree.Count; $i++) {
        foreach ($p in $procs) {
          if ([int]$p.ParentProcessId -eq $tree[$i] -and -not $tree.Contains([int]$p.ProcessId)) { [void]$tree.Add([int]$p.ProcessId) }
        }
      }
      foreach ($id in $tree) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }
    } finally { $env:PATH = $oldPath }
    Start-Sleep -Seconds 1
    $killed = if (Test-Path $killLogFile) { Get-Content -LiteralPath $killLogFile -Raw } else { '' }
    Assert-True ($killed -match 'LANE-EMITTED-LINE') "a killed lane's log keeps what the lane printed; log was: [$killed]"
    Assert-True ($killed -notmatch '=== EXIT') 'the fixture really did kill the lane before its terminal receipt'
  }

  # --- GH-748: lane-status names the result and flags non-delivery ---------
  $statusLog = Join-Path $scratch 'statuslog'; New-Item -ItemType Directory -Force -Path $statusLog | Out-Null
  $statusDriver = Join-Path $scratch 'status-driver.ps1'
  @'
$ErrorActionPreference = 'Stop'
function Get-ScheduledTask {
  [CmdletBinding()] param([string]$TaskName)
  [pscustomobject]@{ TaskName = 'edda-lane-gh748-timeout'; State = 'Ready'; Actions = @([pscustomobject]@{ Arguments = '-File nowhere.ps1'; WorkingDirectory = '' }) }
}
function Get-ScheduledTaskInfo { [CmdletBinding()] param([string]$TaskName) [pscustomobject]@{ LastTaskResult = 267014 } }
function Get-CimInstance { param([Parameter(Position = 0)]$ClassName, [string]$Filter) @() }
& $env:GH748_STATUS -Name gh748-timeout -LogDir $env:GH748_STATUSLOG
'@ | Set-Content -LiteralPath $statusDriver -Encoding utf8
  $env:GH748_STATUS = $status; $env:GH748_STATUSLOG = $statusLog
  $statusOut = (& pwsh -NoProfile -NonInteractive -File $statusDriver 2>&1) -join "`n"
  Assert-True ($statusOut -match 'SCHED_S_TASK_TERMINATED') "lane-status names result 267014; output was:`n$statusOut"
  Assert-True ($statusOut -match 'delivery=UNDELIVERED') "lane-status reports a terminated lane with no receipt as undelivered; output was:`n$statusOut"

  # A lane that did publish its terminal receipt is not slandered as undelivered.
  Set-Content -LiteralPath (Join-Path $statusLog 'gh748-timeout.done') -Value '0' -Encoding ascii
  $doneOut = (& pwsh -NoProfile -NonInteractive -File $statusDriver 2>&1) -join "`n"
  Assert-True ($doneOut -match 'delivery=complete') "a lane with a terminal receipt reports delivery=complete; output was:`n$doneOut"
}
finally {
  Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failures.Count -gt 0) {
  [Console]::Error.WriteLine("lane-delivery fixture: $($failures.Count) failure(s)")
  exit 1
}
'lane-delivery fixtures passed'
exit 0
