# lane-stop.ps1 — actually stop a fleet lane launched by lane-launch.ps1.
#
# Stop-ScheduledTask (and Unregister-ScheduledTask) terminate only the task's
# wrapper process, NOT the process tree the wrapper spawned (GH-672): the
# lane's `edda dispatch` child kept running to commit / push / open a PR while
# the task reported State = Ready. This script is the only sanctioned way to
# stop a lane. It
#   1. stops the scheduled task (best-effort, kills the wrapper only),
#   2. kills the lane's whole process tree — the wrapper plus, even after the
#      wrapper is already dead (the orphan case above), every process still
#      identifiable by CommandLine as the lane: the wrapper path and the
#      brief path the wrapper was launched with — including each match's
#      descendants,
#   3. verifies by CommandLine match (wrapper path + brief path) that nothing
#      survived, and
#   4. writes the end record the killed wrapper can no longer write: the
#      done-file (sentinel "stopped") and a === EXIT === line in the lane log,
#      so a lane log with START but no EXIT is never left ambiguous (GH-672).
#
# usage:
#   pwsh -NoProfile -File scripts/fleet/lane-stop.ps1 -Name <lane> [-LogDir <dir>]
#
# Prints what was terminated, one line each. Exit codes (style of
# lane-status.ps1): 0 = stopped (N processes terminated, or the lane was not
# running); 1 = error: no matching task, wrapper missing, or processes
# survived the kill.
#
# Never points at another lane: -Name selects exactly one edda-lane-* task,
# and the CommandLine matches are the lane's own wrapper and brief paths.
param(
  [Parameter(Mandatory = $true)][string]$Name,
  [string]$LogDir = "$env:TEMP\edda-lanes"
)

$ErrorActionPreference = 'Stop'

function Fail([string]$Msg) {
  [Console]::Error.WriteLine("lane-stop: $Msg")
  exit 1
}

if ($Name -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]*$') {
  Fail "-Name '$Name' may contain only letters, digits, dot, underscore, hyphen"
}
$TaskName = "edda-lane-$Name"
$task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if (-not $task) { Fail "no scheduled task $TaskName (scripts/fleet/lane-status.ps1 lists what exists)" }

$Wrapper = Join-Path $LogDir "$Name.wrapper.ps1"
$Log = Join-Path $LogDir "$Name.log"
$Done = Join-Path $LogDir "$Name.done"
if (-not (Test-Path -LiteralPath $Wrapper -PathType Leaf)) {
  Fail "wrapper not found: $Wrapper; cannot verify the lane's process tree by CommandLine"
}

# What the lane's processes look like on the command line. The wrapper path
# matches the wrapper itself; the brief path (extracted from the wrapper the
# same way the #606 clobber-guard does) matches the `edda dispatch` child and
# stays identifiable after the wrapper is already dead — exactly the
# State=Ready-but-still-working orphan Stop-ScheduledTask leaves behind.
$wrapperBody = Get-Content -LiteralPath $Wrapper -Raw
$brief = if ($wrapperBody -match "--prompt-file '([^']+)'") { $Matches[1] } else { $null }

# Snapshot once; index children by parent PID so the whole tree below any
# match (cargo, git, the agent runtime) is reachable even though none of
# those children's own command lines mention the lane.
$allProcs = @(Get-CimInstance Win32_Process)
$childrenOf = @{}
foreach ($p in $allProcs) {
  if (-not $childrenOf.ContainsKey($p.ParentProcessId)) {
    $childrenOf[$p.ParentProcessId] = New-Object System.Collections.Generic.List[int]
  }
  $childrenOf[$p.ParentProcessId].Add($p.ProcessId)
}

function Find-LaneProcessIds {
  # Seed = processes whose command line names this lane; then their whole
  # descendant trees. Never this process ($PID) and never a PID twice.
  $seeds = @($allProcs | Where-Object {
    $_.ProcessId -ne $PID -and $_.CommandLine -and (
      $_.CommandLine.Contains($Wrapper) -or
      ($brief -and $_.CommandLine.Contains($brief)))
  })
  $ids = New-Object System.Collections.Generic.HashSet[int]
  $queue = New-Object System.Collections.Generic.Queue[int]
  foreach ($s in $seeds) { [void]$ids.Add($s.ProcessId); $queue.Enqueue($s.ProcessId) }
  while ($queue.Count -gt 0) {
    $currentPid = $queue.Dequeue()
    if (-not $childrenOf.ContainsKey($currentPid)) { continue }
    foreach ($c in $childrenOf[$currentPid]) {
      if ($c -ne $PID -and $ids.Add($c)) { $queue.Enqueue($c) }
    }
  }
  return @($ids)
}

# --- 1. stop the task (kills the wrapper only — never the tree, GH-672) -----

if ($task.State -eq 'Running') {
  Stop-ScheduledTask -TaskName $TaskName
  Start-Sleep -Seconds 1
}

# --- 2. kill the whole tree -------------------------------------------------

$targets = Find-LaneProcessIds
$terminated = New-Object System.Collections.Generic.List[int]
if ($targets.Count -gt 0) {
  foreach ($t in $targets) {
    Stop-Process -Id $t -Force -ErrorAction SilentlyContinue
  }
  Start-Sleep -Seconds 2
  # Anything that ignored Stop-Process gets taskkill /T /F, then one final
  # accounting pass.
  $survivors = @(Get-Process -Id $targets -ErrorAction SilentlyContinue)
  foreach ($s in $survivors) {
    & taskkill /PID $s.Id /T /F 2>$null | Out-Null
  }
  if ($survivors.Count -gt 0) { Start-Sleep -Seconds 2 }
  foreach ($t in $targets) {
    if (-not (Get-Process -Id $t -ErrorAction SilentlyContinue)) { $terminated.Add($t) }
  }
}

# --- 3. verify by CommandLine: nothing matching the lane remains ------------

$residual = @(Get-CimInstance Win32_Process | Where-Object {
  $_.ProcessId -ne $PID -and $_.CommandLine -and (
    $_.CommandLine.Contains($Wrapper) -or
    ($brief -and $_.CommandLine.Contains($brief)))
})
if ($residual.Count -gt 0) {
  Fail "residual lane processes survive the kill: $(($residual | ForEach-Object { $_.ProcessId }) -join ',')"
}
$task = Get-ScheduledTask -TaskName $TaskName
if ($task.State -eq 'Running') { Fail "task $TaskName still reports State = Running after the stop" }

# --- 4. write the end record the killed wrapper cannot write ----------------

if ($terminated.Count -gt 0 -or ((Test-Path -LiteralPath $Log) -and -not (Test-Path -LiteralPath $Done))) {
  Add-Content -LiteralPath $Log -Value "=== EXIT (stopped by lane-stop.ps1; terminated $($terminated.Count) process(es)) ===" -Encoding utf8
  if (-not (Test-Path -LiteralPath $Done)) {
    Set-Content -LiteralPath $Done -Value 'stopped' -Encoding ascii
  }
}

# --- report -----------------------------------------------------------------

"task=$TaskName state=$($task.State)"
if ($terminated.Count -gt 0) {
  "terminated=$($terminated.Count) pids=$($terminated -join ',')"
} else {
  "terminated=0 (lane was not running)"
}
"residual=$($residual.Count) (CommandLine match: wrapper path$(if ($brief) { ' + brief path' } else { '' }))"
exit 0
