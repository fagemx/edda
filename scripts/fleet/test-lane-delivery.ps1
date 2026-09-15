# test-lane-delivery.ps1 — GH-748 delivery-hygiene fixture for
# scripts/fleet/lane-launch.ps1 and scripts/fleet/lane-status.ps1.
#
# The defect this pins: on 2026-09-03 three of five fix lanes ran every gate,
# committed or pushed, then were killed before they delivered anything. The
# wrapper's done-file recorded only the exit code and the log was empty, so
# "finished the work and handed over nothing" was indistinguishable from
# "never ran". The worktree survived but nothing surfaced it.
#
# Contract pinned here:
#   * a lane that ends at its timeout with finished work (an unpushed commit
#     or a dirty worktree) leaves a durable, machine-readable evidence
#     checkpoint naming the branch, HEAD, unpushed count and dirty paths, and
#     the same verdict is appended to the log — so the evidence is recoverable
#     without the log and after the scheduled-task registration is gone;
#   * lane-status.ps1 reports that lane as delivery=UNDELIVERED with the
#     scheduler code named, not `done=False` beside a bare 267014;
#   * a host-process kill that never reaches the wrapper's finally is still
#     caught from the worktree the lane left behind;
#   * an ordinary clean lane is unaffected: delivery=complete.
#
# The scheduler is mocked in child drivers; no real Scheduled Task is
# registered, started or unregistered, and no agent runs (a stub `edda.cmd`
# on PATH stands in). Style follows scripts/fleet/test-lane-terminal-receipts.ps1.
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$launch = Join-Path $root 'scripts\fleet\lane-launch.ps1'
$status = Join-Path $root 'scripts\fleet\lane-status.ps1'
$scratch = Join-Path $env:TEMP "gh748-delivery-$PID"
$failures = [System.Collections.Generic.List[string]]::new()
function Assert-True($ok, [string]$what) { if ($ok) { "PASS: $what" } else { "FAIL: $what"; $failures.Add($what) | Out-Null } }

New-Item -ItemType Directory -Force -Path $scratch | Out-Null
try {
  # A throwaway repo with a local bare origin, so the lane branch has an
  # upstream and "unpushed" is measurable exactly as in production.
  function New-LaneRepo([string]$Name) {
    $origin = Join-Path $scratch "$Name-origin.git"
    $repo = Join-Path $scratch $Name
    & git init -q --bare $origin
    & git init -q $repo
    & git -C $repo config user.email fixture@example.invalid
    & git -C $repo config user.name fixture
    Set-Content -LiteralPath (Join-Path $repo 'README.md') -Value base
    & git -C $repo add README.md
    & git -C $repo commit -qm base
    & git -C $repo remote add origin $origin
    & git -C $repo push -q -u origin HEAD
    return $repo
  }

  # Run the real launch path with the scheduler mocked, then run the generated
  # wrapper in a child where only the wrapper's own Unregister-ScheduledTask
  # is mocked and the stub `edda` is first on PATH.
  function Invoke-LaneLaunch([string]$Name, [string]$Repo, [string]$LogDir, [string]$StubDir, [switch]$GenerateOnly) {
    New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
    $brief = Join-Path $LogDir "$Name.brief.md"
    Set-Content -LiteralPath $brief -Value '# fixture'
    $driver = Join-Path $LogDir "$Name.launch-driver.ps1"
    @'
$ErrorActionPreference = 'Stop'
$global:registered = $false
function Get-ScheduledTask { [CmdletBinding()] param([string]$TaskName) if ($global:registered) { [pscustomobject]@{ TaskName=$TaskName; State='Running' } } }
function New-ScheduledTaskAction { [CmdletBinding()] param($Execute,$Argument,$WorkingDirectory) [pscustomobject]@{} }
function New-ScheduledTaskSettingsSet { [CmdletBinding()] param($ExecutionTimeLimit,$MultipleInstances,[switch]$AllowStartIfOnBatteries,[switch]$DontStopIfGoingOnBatteries) [pscustomobject]@{} }
function Register-ScheduledTask { [CmdletBinding()] param($TaskName,$Action,$Settings,$Description,$RunLevel) $global:registered=$true; [pscustomobject]@{} }
function Start-ScheduledTask { [CmdletBinding()] param([string]$TaskName) }
function Unregister-ScheduledTask { [CmdletBinding()] param([string]$TaskName,[switch]$Confirm) }
function Get-CimInstance { [CmdletBinding()] param([Parameter(Position=0)]$ClassName,[string]$Filter) @() }
& $env:GH748_LAUNCH -Name $env:GH748_NAME -Brief $env:GH748_BRIEF -Cwd $env:GH748_REPO -LogDir $env:GH748_LOG -Owns scripts/fleet/lane-launch.ps1
exit 0
'@ | Set-Content -LiteralPath $driver -Encoding utf8
    $env:GH748_LAUNCH = $launch; $env:GH748_NAME = $Name; $env:GH748_BRIEF = $brief; $env:GH748_REPO = $Repo; $env:GH748_LOG = $LogDir
    $launchOut = (& pwsh -NoProfile -NonInteractive -File $driver 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0) { throw "lane-launch (mocked) failed for ${Name}:`n$launchOut" }

    $wrapper = Join-Path $LogDir "$Name.wrapper.ps1"
    if (-not (Test-Path -LiteralPath $wrapper)) { throw "lane-launch did not write $wrapper" }
    if ($GenerateOnly) { return @{ Wrapper = $wrapper; Output = $launchOut; Code = 0 } }
    $runner = Join-Path $LogDir "$Name.wrapper-run.ps1"
    @'
function Unregister-ScheduledTask { [CmdletBinding()] param([string]$TaskName,[switch]$Confirm) }
& $env:GH748_WRAPPER
exit $LASTEXITCODE
'@ | Set-Content -LiteralPath $runner -Encoding utf8
    $env:GH748_WRAPPER = $wrapper
    $oldPath = $env:PATH
    $env:PATH = "$StubDir;$env:PATH"
    try {
      $runOut = (& pwsh -NoProfile -NonInteractive -File $runner 2>&1 | Out-String)
      $runCode = $LASTEXITCODE
    } finally { $env:PATH = $oldPath }
    return @{ Wrapper = $wrapper; Output = $runOut; Code = $runCode }
  }

  function Invoke-LaneStatus([string]$Name, [string]$Wrapper, [string]$Repo, [string]$LogDir, [uint32]$Result, [string]$State) {
    $driver = Join-Path $LogDir "$Name.status-driver.ps1"
    @'
$ErrorActionPreference = 'Stop'
function Get-ScheduledTask { [CmdletBinding()] param([string]$TaskName) [pscustomobject]@{ TaskName=$env:GH748_TASK; State=$env:GH748_STATE; Actions=@([pscustomobject]@{ Arguments="-File `"$env:GH748_WRAPPER`""; WorkingDirectory=$env:GH748_REPO }) } }
function Get-ScheduledTaskInfo { [CmdletBinding()] param([string]$TaskName) [pscustomobject]@{ LastTaskResult=[uint32]$env:GH748_RESULT } }
function Get-CimInstance { [CmdletBinding()] param([Parameter(Position=0)]$ClassName,[string]$Filter) @() }
& $env:GH748_STATUS -Name $env:GH748_LANE -LogDir $env:GH748_LOG
exit $LASTEXITCODE
'@ | Set-Content -LiteralPath $driver -Encoding utf8
    $env:GH748_STATUS = $status; $env:GH748_TASK = "edda-lane-$Name"; $env:GH748_STATE = $State
    $env:GH748_WRAPPER = $Wrapper; $env:GH748_REPO = $Repo; $env:GH748_LOG = $LogDir
    $env:GH748_RESULT = [string]$Result; $env:GH748_LANE = $Name
    return (& pwsh -NoProfile -NonInteractive -File $driver 2>&1 | Out-String)
  }

  # --- case 1: a timed-out lane leaves durable, undelivered evidence --------
  "=== case 1: a timed-out lane that did the work ==="
  $repo = New-LaneRepo 'lane-timeout'
  $logDir = Join-Path $scratch 'log-timeout'
  $stub = Join-Path $scratch 'stub-timeout'; New-Item -ItemType Directory -Force -Path $stub | Out-Null
  # The stub behaves like a lane that committed its work and left an edit on
  # disk, then is killed by dispatch's turn timeout (exit 2).
  Set-Content -LiteralPath (Join-Path $stub 'edda.cmd') -Encoding ascii -Value @(
    '@echo off'
    'echo STUB-EDDA-748'
    'echo lane work>work.txt'
    'git add work.txt'
    'git commit -qm "lane work"'
    'echo leftover>dirty.txt'
    'exit /b 2'
  )
  $r = Invoke-LaneLaunch -Name 'gh748-timeout' -Repo $repo -LogDir $logDir -StubDir $stub
  Assert-True ($r.Code -eq 2) "the timed-out lane exits with the dispatch code (got $($r.Code))"
  $logText = if (Test-Path -LiteralPath (Join-Path $logDir 'gh748-timeout.log')) { Get-Content -LiteralPath (Join-Path $logDir 'gh748-timeout.log') -Raw } else { '' }
  Assert-True ($logText -match 'STUB-EDDA-748') 'the stub edda ran (no real dispatch was spent)'
  Assert-True ($logText -match 'LANE_START') 'the wrapper writes a LANE_START marker before dispatch'

  $evidencePath = Join-Path $logDir 'gh748-timeout.evidence'
  Assert-True (Test-Path -LiteralPath $evidencePath) 'a timed-out lane leaves a durable evidence checkpoint'
  $evidence = if (Test-Path -LiteralPath $evidencePath) { Get-Content -LiteralPath $evidencePath -Raw } else { '' }
  Assert-True ($evidence -match '(?m)^delivery=UNDELIVERED$') "the checkpoint marks the lane UNDELIVERED; evidence:`n$evidence"
  Assert-True ($evidence -match '(?m)^unpushed=1$') 'the checkpoint counts the unpushed commit'
  Assert-True ($evidence -match '(?m)^dirty=1$') 'the checkpoint counts the dirty path'
  Assert-True ($evidence -match '(?m)^head=[0-9a-f]{40}$') 'the checkpoint records the full HEAD'
  Assert-True ($evidence -match '(?m)^branch=\S+$') 'the checkpoint records the branch'
  Assert-True ($evidence -match 'dirty\.txt') 'the checkpoint lists the dirty path so it is recoverable even if the file is lost'
  Assert-True ($logText -match 'LANE_EVIDENCE delivery=UNDELIVERED') 'the log carries the same machine-readable verdict'
  $doneText = if (Test-Path -LiteralPath (Join-Path $logDir 'gh748-timeout.done')) { (Get-Content -LiteralPath (Join-Path $logDir 'gh748-timeout.done') -Raw).Trim() } else { '' }
  Assert-True ($doneText -eq '2') "the done-file still records the timeout code (got '$doneText')"

  $statusOut = Invoke-LaneStatus -Name 'gh748-timeout' -Wrapper $r.Wrapper -Repo $repo -LogDir $logDir -Result 267014 -State 'Ready'
  Assert-True ($statusOut -match 'delivery=UNDELIVERED') "lane-status reports the timed-out lane undelivered; output:`n$statusOut"
  Assert-True ($statusOut -match 'SCHED_S_TASK_TERMINATED') 'lane-status names the scheduler code instead of a bare 267014'
  Assert-True ($statusOut -match 'evidence=True') 'lane-status points at the evidence checkpoint'

  # --- case 2: an ordinary clean lane is unaffected -------------------------
  "=== case 2: an ordinary clean lane ==="
  $repo2 = New-LaneRepo 'lane-clean'
  $logDir2 = Join-Path $scratch 'log-clean'
  $stub2 = Join-Path $scratch 'stub-clean'; New-Item -ItemType Directory -Force -Path $stub2 | Out-Null
  Set-Content -LiteralPath (Join-Path $stub2 'edda.cmd') -Encoding ascii -Value @(
    '@echo off'
    'echo STUB-EDDA-748'
    'exit /b 0'
  )
  $r2 = Invoke-LaneLaunch -Name 'gh748-clean' -Repo $repo2 -LogDir $logDir2 -StubDir $stub2
  Assert-True ($r2.Code -eq 0) "an ordinary lane still exits 0 (got $($r2.Code))"
  $evidence2Path = Join-Path $logDir2 'gh748-clean.evidence'
  $evidence2 = if (Test-Path -LiteralPath $evidence2Path) { Get-Content -LiteralPath $evidence2Path -Raw } else { '' }
  Assert-True ($evidence2 -match '(?m)^delivery=COMPLETE$') "an ordinary lane is recorded complete; evidence:`n$evidence2"
  $done2 = if (Test-Path -LiteralPath (Join-Path $logDir2 'gh748-clean.done')) { (Get-Content -LiteralPath (Join-Path $logDir2 'gh748-clean.done') -Raw).Trim() } else { '' }
  Assert-True ($done2 -eq '0') "the ordinary lane's done-file is unchanged (got '$done2')"
  $statusOut2 = Invoke-LaneStatus -Name 'gh748-clean' -Wrapper $r2.Wrapper -Repo $repo2 -LogDir $logDir2 -Result 0 -State 'Ready'
  Assert-True ($statusOut2 -match 'delivery=complete') "lane-status reports the ordinary lane complete; output:`n$statusOut2"
  Assert-True ($statusOut2 -match 'OK') 'lane-status names the OK scheduler result'

  # A lane that delivered cleanly is not retroactively slandered by an edit
  # made in its worktree after the wrapper finished: the recorded verdict wins
  # (review P2). A dirty tree with no checkpoint is still UNDELIVERED (case 3).
  Set-Content -LiteralPath (Join-Path $repo2 'post-exit.txt') -Value 'edit after the lane exited'
  $statusOut2Dirt = Invoke-LaneStatus -Name 'gh748-clean' -Wrapper $r2.Wrapper -Repo $repo2 -LogDir $logDir2 -Result 0 -State 'Ready'
  Assert-True ($statusOut2Dirt -match 'delivery=complete') "a recorded COMPLETE checkpoint is not overridden by post-exit dirt; output:`n$statusOut2Dirt"

  # --- case 2b: a failed lane that produced nothing is not "complete" -------
  # The checkpoint exists, so a reader that checked only for its presence would
  # collapse "exited nonzero with a clean tree" (EMPTY) into complete — the
  # exact ambiguity GH-748 targets. The verdict, not the file, is the signal.
  "=== case 2b: a nonzero exit with an EMPTY checkpoint is undelivered ==="
  $repo2b = New-LaneRepo 'lane-empty'
  $logDir2b = Join-Path $scratch 'log-empty'
  $stub2b = Join-Path $scratch 'stub-empty'; New-Item -ItemType Directory -Force -Path $stub2b | Out-Null
  Set-Content -LiteralPath (Join-Path $stub2b 'edda.cmd') -Encoding ascii -Value @(
    '@echo off'
    'echo STUB-EDDA-748'
    'exit /b 1'
  )
  $r2b = Invoke-LaneLaunch -Name 'gh748-empty' -Repo $repo2b -LogDir $logDir2b -StubDir $stub2b
  $evidence2bPath = Join-Path $logDir2b 'gh748-empty.evidence'
  $evidence2b = if (Test-Path -LiteralPath $evidence2bPath) { Get-Content -LiteralPath $evidence2bPath -Raw } else { '' }
  Assert-True ($evidence2b -match '(?m)^delivery=EMPTY$') "a failed clean lane records EMPTY; evidence:`n$evidence2b"
  $statusOut2b = Invoke-LaneStatus -Name 'gh748-empty' -Wrapper $r2b.Wrapper -Repo $repo2b -LogDir $logDir2b -Result 0 -State 'Ready'
  Assert-True ($statusOut2b -match 'delivery=UNDELIVERED') "an EMPTY checkpoint is reported undelivered, never complete; output:`n$statusOut2b"

  # --- case 3: a host-process kill never reaches the wrapper's finally ------
  # Two lanes died this way on 2026-09-09 with edits on disk and no commit.
  # Nothing in the lane ran its teardown, so only the worktree it left can
  # prove it existed.
  "=== case 3: a host-killed lane whose worktree is dirty ==="
  $repo3 = New-LaneRepo 'lane-killed'
  Set-Content -LiteralPath (Join-Path $repo3 'half-finished.txt') -Value 'uncommitted edit'
  $logDir3 = Join-Path $scratch 'log-killed'; New-Item -ItemType Directory -Force -Path $logDir3 | Out-Null
  $wrapper3 = Join-Path $logDir3 'gh748-killed.wrapper.ps1'
  Set-Content -LiteralPath $wrapper3 -Encoding utf8 -Value @(
    '# lane-reap: controller-pid=1 controller-started=2026-01-01T00:00:00.0000000Z'
    '# this wrapper was killed before its finally block could run'
  )
  $statusOut3 = Invoke-LaneStatus -Name 'gh748-killed' -Wrapper $wrapper3 -Repo $repo3 -LogDir $logDir3 -Result 0 -State 'Ready'
  Assert-True ($statusOut3 -match 'delivery=UNDELIVERED') "a host-killed lane's dirty worktree is reported undelivered; output:`n$statusOut3"

  # --- case 4: a killed wrapper leaves a non-empty, START-marked log --------
  # The 2026-09-03 loss was made unrecoverable because logBytes=0: a lane that
  # was killed had no record at all. The LANE_START marker makes the GH-672
  # convention hold — START without '=== EXIT' means killed, not never run.
  "=== case 4: a killed wrapper leaves a readable log ==="
  $repo4 = New-LaneRepo 'lane-run-killed'
  $logDir4 = Join-Path $scratch 'log-run-killed'
  $stub4 = Join-Path $scratch 'stub-run-killed'; New-Item -ItemType Directory -Force -Path $stub4 | Out-Null
  Set-Content -LiteralPath (Join-Path $stub4 'edda.cmd') -Encoding ascii -Value @(
    '@echo off'
    'echo STUB-EDDA-748-RUNNING'
    'ping -n 60 127.0.0.1 >nul'
  )
  $r4 = Invoke-LaneLaunch -Name 'gh748-run-killed' -Repo $repo4 -LogDir $logDir4 -StubDir $stub4 -GenerateOnly
  $oldPath4 = $env:PATH
  $env:PATH = "$stub4;$env:PATH"
  try {
    $proc = Start-Process -FilePath (Get-Command pwsh.exe).Source `
      -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $r4.Wrapper) `
      -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 6
    # Seed the closure from processes whose command line names this wrapper —
    # the same evidence lane-status uses — rather than trusting the
    # Start-Process handle's PID, which a busy runner may have recycled
    # (review P2). Descend from confirmed seeds only.
    $procs = @(Get-CimInstance Win32_Process)
    $tree = [System.Collections.Generic.List[int]]::new()
    foreach ($seedProc in @($procs | Where-Object { $_.CommandLine -and $_.CommandLine.Contains($r4.Wrapper) })) {
      [void]$tree.Add([int]$seedProc.ProcessId)
    }
    if ($tree.Count -eq 0 -and $proc) { [void]$tree.Add([int]$proc.Id) }
    for ($i = 0; $i -lt $tree.Count; $i++) {
      foreach ($p in $procs) {
        if ([int]$p.ParentProcessId -eq $tree[$i] -and -not $tree.Contains([int]$p.ProcessId)) { [void]$tree.Add([int]$p.ProcessId) }
      }
    }
    foreach ($killedId in $tree) { Stop-Process -Id $killedId -Force -ErrorAction SilentlyContinue }
  } finally { $env:PATH = $oldPath4 }
  Start-Sleep -Seconds 1
  $logPath4 = Join-Path $logDir4 'gh748-run-killed.log'
  $killedLog = if (Test-Path -LiteralPath $logPath4) { Get-Content -LiteralPath $logPath4 -Raw } else { '' }
  Assert-True ($killedLog -match 'LANE_START') "a killed lane still leaves a non-empty START-marked log; log was: [$killedLog]"
  Assert-True ($killedLog -notmatch '=== EXIT') 'the kill happened before the terminal receipt, as the fixture intends'
  $statusOut4 = Invoke-LaneStatus -Name 'gh748-run-killed' -Wrapper $r4.Wrapper -Repo $repo4 -LogDir $logDir4 -Result 267014 -State 'Ready'
  Assert-True ($statusOut4 -match 'delivery=UNDELIVERED') "lane-status reports the killed lane undelivered; output:`n$statusOut4"

  # --- case 5: a relaunch does not inherit the previous run's verdict -------
  # A stale .evidence recording COMPLETE would otherwise outrank the new run's
  # dirty worktree in lane-status and report a killed relaunch as delivered
  # (review P1). The real launch must clear the terminal artifacts first.
  "=== case 5: a relaunch clears the previous run's evidence ==="
  $repo5 = New-LaneRepo 'lane-relaunch'
  $logDir5 = Join-Path $scratch 'log-relaunch'
  $stub5ok = Join-Path $scratch 'stub-relaunch-ok'; New-Item -ItemType Directory -Force -Path $stub5ok | Out-Null
  Set-Content -LiteralPath (Join-Path $stub5ok 'edda.cmd') -Encoding ascii -Value @('@echo off', 'echo STUB-EDDA-748', 'exit /b 0')
  $r5a = Invoke-LaneLaunch -Name 'gh748-relaunch' -Repo $repo5 -LogDir $logDir5 -StubDir $stub5ok
  $evidence5 = Join-Path $logDir5 'gh748-relaunch.evidence'
  Assert-True ((Test-Path -LiteralPath $evidence5) -and ((Get-Content -LiteralPath $evidence5 -Raw) -match '(?m)^delivery=COMPLETE$')) 'the first run records a COMPLETE checkpoint'
  # Relaunch the SAME name with a dirtying, sleeping stub, then kill it before
  # its finally block: no new checkpoint is written, so only the stale one
  # could lie.
  $stub5kill = Join-Path $scratch 'stub-relaunch-kill'; New-Item -ItemType Directory -Force -Path $stub5kill | Out-Null
  Set-Content -LiteralPath (Join-Path $stub5kill 'edda.cmd') -Encoding ascii -Value @(
    '@echo off'
    'echo STUB-EDDA-748-RUNNING'
    'echo leftover>dirty.txt'
    'ping -n 60 127.0.0.1 >nul'
  )
  $r5b = Invoke-LaneLaunch -Name 'gh748-relaunch' -Repo $repo5 -LogDir $logDir5 -StubDir $stub5kill -GenerateOnly
  Assert-True (-not (Test-Path -LiteralPath $evidence5)) 'the relaunch removes the previous run evidence before starting'
  $oldPath5 = $env:PATH
  $env:PATH = "$stub5kill;$env:PATH"
  try {
    $proc5 = Start-Process -FilePath (Get-Command pwsh.exe).Source `
      -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $r5b.Wrapper) `
      -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 6
    $procs5 = @(Get-CimInstance Win32_Process)
    $tree5 = [System.Collections.Generic.List[int]]::new()
    foreach ($seedProc5 in @($procs5 | Where-Object { $_.CommandLine -and $_.CommandLine.Contains($r5b.Wrapper) })) { [void]$tree5.Add([int]$seedProc5.ProcessId) }
    if ($tree5.Count -eq 0 -and $proc5) { [void]$tree5.Add([int]$proc5.Id) }
    for ($i = 0; $i -lt $tree5.Count; $i++) {
      foreach ($p in $procs5) {
        if ([int]$p.ParentProcessId -eq $tree5[$i] -and -not $tree5.Contains([int]$p.ProcessId)) { [void]$tree5.Add([int]$p.ProcessId) }
      }
    }
    foreach ($killedId5 in $tree5) { Stop-Process -Id $killedId5 -Force -ErrorAction SilentlyContinue }
  } finally { $env:PATH = $oldPath5 }
  Start-Sleep -Seconds 1
  $statusOut5 = Invoke-LaneStatus -Name 'gh748-relaunch' -Wrapper $r5b.Wrapper -Repo $repo5 -LogDir $logDir5 -Result 267014 -State 'Ready'
  Assert-True ($statusOut5 -match 'delivery=UNDELIVERED') "a killed relaunch with a dirty worktree is undelivered, not complete; output:`n$statusOut5"
} finally {
  # The GH748_* variables are process-wide; clear them so a caller that dot-
  # sources this fixture (or a later test in the same session) never inherits
  # a stale lane path.
  foreach ($v in @('GH748_LAUNCH','GH748_NAME','GH748_BRIEF','GH748_REPO','GH748_LOG','GH748_WRAPPER','GH748_STATUS','GH748_TASK','GH748_STATE','GH748_RESULT','GH748_LANE')) {
    Remove-Item -Path "Env:$v" -ErrorAction SilentlyContinue
  }
  Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue
}
if ($failures.Count) { "RESULT: FAIL ($($failures.Count))"; exit 1 }
"RESULT: PASS"
