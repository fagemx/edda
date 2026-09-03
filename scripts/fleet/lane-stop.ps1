# lane-stop.ps1 — actually stop a fleet lane launched by lane-launch.ps1.
#
# Stop-ScheduledTask (and Unregister-ScheduledTask) terminate only the task's
# wrapper process, NOT the process tree the wrapper spawned (GH-672): the
# lane's `edda dispatch` child kept running to commit / push / open a PR while
# the task reported State = Ready. This script is the only sanctioned way to
# stop a lane. It
#   1. snapshots the process tree to capture the wrapper PID while alive,
#   2. stops the scheduled task (best-effort, kills the wrapper only),
#   3. re-snapshots the process tree after the task is stopped so any child or
#      grandchild spawned before or during task shutdown is indexed (GH-706),
#   4. kills the lane's whole process tree — the wrapper plus, even after the
#      wrapper is already dead (the orphan case above), every process still
#      identifiable by CommandLine as the lane: the wrapper path and the
#      brief path the wrapper was launched with — including each match's
#      descendants, using type-safe [int] parent/child indexing,
#   5. verifies by both PID survival and tree/CommandLine match that nothing
#      survived (GH-706), and
#   6. writes the end record the killed wrapper can no longer write: the
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
# Matches tasks registered across the fleet (edda-lane-*, edda-review-pr*-r*, GH-712).
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

# Resolve scheduled task: support exact name, edda-lane-$Name, and edda-$Name (covers edda-review-pr*-r*) (GH-712)
$task = $null
$TaskName = $null
$candidateTaskNames = @($Name, "edda-lane-$Name", "edda-$Name")
foreach ($cand in $candidateTaskNames) {
  $t = Get-ScheduledTask -TaskName $cand -ErrorAction SilentlyContinue
  if ($t) {
    $TaskName = $cand
    $task = $t
    break
  }
}
if (-not $task) {
  Fail "no scheduled task matching '$Name' (checked: $($candidateTaskNames -join ', '); scripts/fleet/lane-status.ps1 lists what exists)"
}

# Resolve wrapper script:
# 1. From the task's registered action argument line (works for both worker and review lanes)
$Wrapper = $null
$actionArg = if ($task.Actions -and $task.Actions.Count -gt 0) { $task.Actions[0].Arguments } else { $null }
if ($actionArg -and $actionArg -match '-File\s+"?([^"\s]+)"?') {
  $extracted = $Matches[1]
  if (Test-Path -LiteralPath $extracted -PathType Leaf) {
    $Wrapper = $extracted
  }
}

# 2. Fallback to LogDir candidate paths if not in action arguments
if (-not $Wrapper) {
  $lane = $TaskName -replace '^edda-lane-', '' -replace '^edda-', ''
  $candidates = @(
    (Join-Path $LogDir "$Name.wrapper.ps1"),
    (Join-Path $LogDir "$lane.wrapper.ps1"),
    (Join-Path $LogDir "$Name.ps1"),
    (Join-Path $LogDir "$lane.ps1")
  )
  foreach ($c in $candidates) {
    if (Test-Path -LiteralPath $c -PathType Leaf) {
      $Wrapper = $c
      break
    }
  }
}

if (-not $Wrapper -or -not (Test-Path -LiteralPath $Wrapper -PathType Leaf)) {
  Fail "wrapper not found for task $TaskName; cannot verify the lane's process tree by CommandLine"
}

$base = $Wrapper -replace '\.(wrapper\.)?ps1$', ''
$Log = "$base.log"
$Done = "$base.done"

# What the lane's processes look like on the command line. The wrapper path
# matches the wrapper itself; the brief path matches the child process and
# stays identifiable after the wrapper is already dead.
$wrapperBody = Get-Content -LiteralPath $Wrapper -Raw
$brief = if ($wrapperBody -match "--prompt-file\s+['""]?([^'""]+)['""]?") { $Matches[1] } else { $null }

function Get-ProcessSnapshot {
  # Index children by parent PID using explicit [int] keys (GH-706: Win32_Process
  # returns uint32 which does not match [int] lookup keys in .NET hashtables).
  # Also build ProcMap for O(1) process lookup and CreationDate checks.
  $allProcs = @(Get-CimInstance Win32_Process)
  $childrenOf = @{}
  $procMap = @{}
  foreach ($p in $allProcs) {
    $ppid = [int]$p.ParentProcessId
    $procId = [int]$p.ProcessId
    $procMap[$procId] = $p
    if (-not $childrenOf.ContainsKey($ppid)) {
      $childrenOf[$ppid] = New-Object System.Collections.Generic.List[int]
    }
    $childrenOf[$ppid].Add($procId)
  }
  return @{
    Procs      = $allProcs
    ProcMap    = $procMap
    ChildrenOf = $childrenOf
  }
}

# --- 1. pre-stop snapshot (GH-706) -------------------------------------------
# Capture any live wrapper PID before Stop-ScheduledTask terminates it.
$preSnap = Get-ProcessSnapshot
$preSeeds = @($preSnap.Procs | Where-Object {
  $_.ProcessId -ne $PID -and $_.CommandLine -and (
    $_.CommandLine.Contains($Wrapper) -or
    ($brief -and $_.CommandLine.Contains($brief)))
})
$seedPids = New-Object System.Collections.Generic.HashSet[int]
foreach ($s in $preSeeds) { [void]$seedPids.Add([int]$s.ProcessId) }

# --- 2. stop the task (kills the wrapper only — never the tree, GH-672) -----

if ($task.State -eq 'Running') {
  Stop-ScheduledTask -TaskName $TaskName
  Start-Sleep -Seconds 1
}

# --- 3. snapshot process tree after the task is stopped (GH-706) -----------
# Now that the task is stopped, no new wrapper can be spawned. Re-snapshot so
# any child or grandchild spawned before or during task shutdown is captured.
$postSnap = Get-ProcessSnapshot
foreach ($p in $postSnap.Procs) {
  if ($p.ProcessId -ne $PID -and $p.CommandLine -and (
    $p.CommandLine.Contains($Wrapper) -or
    ($brief -and $p.CommandLine.Contains($brief)))) {
    [void]$seedPids.Add([int]$p.ProcessId)
  }
}

# Traverse descendant trees in the post-stop tree starting from all seeds
# (including pre-stop wrapper PIDs whose ParentProcessId links to their children).
$targets = New-Object System.Collections.Generic.HashSet[int]
$queue = New-Object System.Collections.Generic.Queue[int]
foreach ($id in $seedPids) {
  if ($postSnap.ProcMap.ContainsKey($id)) {
    [void]$targets.Add($id)
  }
  $queue.Enqueue($id)
}

while ($queue.Count -gt 0) {
  $currentPid = $queue.Dequeue()
  if (-not $postSnap.ChildrenOf.ContainsKey($currentPid)) { continue }
  $parentProc = $postSnap.ProcMap[$currentPid]
  foreach ($c in $postSnap.ChildrenOf[$currentPid]) {
    $childProc = $postSnap.ProcMap[$c]
    # Guard: child creation date must be >= parent creation date to prevent stale reused PID traversal (GH-706, P2-1)
    if ($parentProc -and $childProc -and $parentProc.CreationDate -and $childProc.CreationDate) {
      if ($childProc.CreationDate -lt $parentProc.CreationDate) { continue }
    }
    if ($c -ne $PID -and $targets.Add($c)) {
      $queue.Enqueue($c)
    }
  }
}

# --- 4. kill the whole tree -------------------------------------------------

$terminated = New-Object System.Collections.Generic.List[int]
$targetList = @($targets)
if ($targetList.Count -gt 0) {
  foreach ($t in $targetList) {
    Stop-Process -Id $t -Force -ErrorAction SilentlyContinue
  }
  Start-Sleep -Seconds 2
  # Anything that ignored Stop-Process gets taskkill /T /F, then one final
  # accounting pass.
  $survivors = @(Get-Process -Id $targetList -ErrorAction SilentlyContinue)
  foreach ($s in $survivors) {
    & taskkill /PID $s.Id /T /F 2>$null | Out-Null
  }
  if ($survivors.Count -gt 0) { Start-Sleep -Seconds 2 }
  foreach ($t in $targetList) {
    if (-not (Get-Process -Id $t -ErrorAction SilentlyContinue)) { $terminated.Add($t) }
  }
}

# --- 5. verify: nothing matching the lane or descended from it remains (GH-706) -

$residualSet = New-Object System.Collections.Generic.HashSet[int]
# Check whether any targeted process survived
foreach ($t in $targetList) {
  if (Get-Process -Id $t -ErrorAction SilentlyContinue) {
    [void]$residualSet.Add($t)
  }
}

# Check whether any process currently on the system matches the lane CommandLine or tree
$verifySnap = Get-ProcessSnapshot
$verifySeeds = @($verifySnap.Procs | Where-Object {
  $_.ProcessId -ne $PID -and $_.CommandLine -and (
    $_.CommandLine.Contains($Wrapper) -or
    ($brief -and $_.CommandLine.Contains($brief)))
})
$verifyQueue = New-Object System.Collections.Generic.Queue[int]
foreach ($s in $verifySeeds) {
  $sid = [int]$s.ProcessId
  [void]$residualSet.Add($sid)
  $verifyQueue.Enqueue($sid)
}
# Also enqueue all targets and seed PIDs (even if dead) so any live orphan whose
# ParentProcessId points to a killed lane target is traversed and detected (GH-706, P1-1).
foreach ($t in $targetList) {
  $verifyQueue.Enqueue($t)
}
foreach ($s in $seedPids) {
  $verifyQueue.Enqueue($s)
}

while ($verifyQueue.Count -gt 0) {
  $curr = $verifyQueue.Dequeue()
  if (-not $verifySnap.ChildrenOf.ContainsKey($curr)) { continue }
  $parentProc = if ($verifySnap.ProcMap.ContainsKey($curr)) {
    $verifySnap.ProcMap[$curr]
  } elseif ($postSnap.ProcMap.ContainsKey($curr)) {
    $postSnap.ProcMap[$curr]
  } elseif ($preSnap.ProcMap.ContainsKey($curr)) {
    $preSnap.ProcMap[$curr]
  } else {
    $null
  }
  foreach ($c in $verifySnap.ChildrenOf[$curr]) {
    $childProc = $verifySnap.ProcMap[$c]
    if ($parentProc -and $childProc -and $parentProc.CreationDate -and $childProc.CreationDate) {
      if ($childProc.CreationDate -lt $parentProc.CreationDate) { continue }
    }
    if ($c -ne $PID -and $residualSet.Add($c)) {
      $verifyQueue.Enqueue($c)
    }
  }
}

$residual = @($residualSet)
if ($residual.Count -gt 0) {
  Fail "residual lane processes survive the kill: $($residual -join ',')"
}
$task = Get-ScheduledTask -TaskName $TaskName
if ($task.State -eq 'Running') { Fail "task $TaskName still reports State = Running after the stop" }

# --- 6. write the end record the killed wrapper cannot write ----------------

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
"residual=$($residual.Count) (CommandLine match + process tree verification)"
exit 0
