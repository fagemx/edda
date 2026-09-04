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
#      survived (GH-706),
#   6. verifies the SHARED .git/config the lane was working in still parses and
#      restores it from the known-good backup if it does not — a hard kill
#      landing on git's config write NULs that file and takes down the main
#      checkout and every linked worktree at once (GH-715). There is no
#      graceful-shutdown window before the kill: `taskkill` without /F posts
#      WM_CLOSE, and a lane's processes are hidden console processes with no
#      window, so it is refused with "can only be terminated forcefully"
#      (exit 128, measured) and the target survives. The kill stays immediate
#      and the damage is repaired here instead, and
#   7. writes the end record the killed wrapper can no longer write: the
#      done-file (sentinel "stopped") and a === EXIT === line in the lane log,
#      so a lane log with START but no EXIT is never left ambiguous (GH-672),
#      and
#   8. AFTER that verification — never before — unregisters the scheduled
#      task (GH-772): a stopped lane must not leave a Ready registration
#      behind that can re-fire stale work. The unregister runs only once the
#      task is confirmed not Running and no lane process survived, so it can
#      never terminate a working lane; it removes the registration only and
#      never touches wrapper, logs, done-files, worktrees or sources. A
#      registration that vanished between check and unregister (a concurrent
#      stop) is reported as already-absent, not an error; any other failure
#      is visible in the output, the end record and a nonzero exit (the
#      reaper, scripts/fleet/lane-reap.ps1, can still collect it).
#
# usage:
#   pwsh -NoProfile -File scripts/fleet/lane-stop.ps1 -Name <lane> [-LogDir <dir>]
#
# Prints what was terminated, one line each, plus the .git/config verdict.
# Exit codes (style of lane-status.ps1): 0 = stopped (N processes terminated,
# or the lane was not running); 1 = error: no matching task, wrapper missing,
# processes survived the kill, the task could not be unregistered, or the
# lane's .git/config was not confirmed healthy — either corrupt and
# unrepairable, or impossible to judge (the gitconfig= line says which). The
# end record (step 7) is written before any of those failures is reported, so
# a stop never costs the log its EXIT line (GH-672).
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
$candidateTaskNames = @("edda-$Name", "edda-lane-$Name", $Name)
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
if ($actionArg -and $actionArg -match '-File\s+("([^"]+)"|''([^'']+)''|([^\s]+))') {
  $extracted = if ($Matches[2]) { $Matches[2] } elseif ($Matches[3]) { $Matches[3] } else { $Matches[4] }
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
    (Join-Path $LogDir "$lane.ps1"),
    (Join-Path $LogDir "$Name-lane.ps1"),
    (Join-Path $LogDir "$lane-lane.ps1")
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

# Derive .log and .done paths: review lanes name the wrapper review-pr<N>-r<R>-lane.ps1
# while .log and .done are review-pr<N>-r<R>.log/.done (without the -lane suffix, GH-712 P1-1).
$base = $Wrapper -replace '\.(wrapper\.)?ps1$', ''
$cleanBase = $base -replace '-lane$', ''
$Log = if (Test-Path -LiteralPath "$cleanBase.log") { "$cleanBase.log" } elseif (Test-Path -LiteralPath "$base.log") { "$base.log" } else { "$cleanBase.log" }
$Done = if (Test-Path -LiteralPath "$cleanBase.done") { "$cleanBase.done" } elseif (Test-Path -LiteralPath "$base.done") { "$base.done" } else { "$cleanBase.done" }

# What the lane's processes look like on the command line. The wrapper path
# matches the wrapper itself; the brief path matches the child process and
# stays identifiable after the wrapper is already dead.
$wrapperBody = Get-Content -LiteralPath $Wrapper -Raw
$brief = if ($wrapperBody -match '--prompt-file\s+[''"]([^''"]+)[''"]') {
  $b = $Matches[1].Trim()
  if ($b.Length -gt 4) { $b } else { $null }
} else { $null }

# The worktree the lane ran in, which resolves the SHARED .git/config this
# script checks after the kill (GH-715). Both wrapper generators this script
# stops must match: lane-launch.ps1:154 writes `Set-Location -LiteralPath '<p>'`
# (single-quoted literal, embedded quotes doubled), while the review lanes of
# review-pr.sh:453 and pr-review-launch.ps1:64 write a bare `Set-Location '<p>'`
# — so the parameter name is optional here. Anchored to end of line because
# both generators put nothing after the path.
$laneCwd = if ($wrapperBody -match "(?m)^\s*Set-Location\s+(?:-(?:LiteralPath|Path)\s+)?'((?:[^']|'')*)'\s*$") {
  $Matches[1] -replace "''", "'"
} else { $null }

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

# --- 5b. unregister the registration now that stopping is verified (GH-772) --
# Reached only when the task is not Running and no lane process survived, so
# the unregister can never terminate working code. Registration only: wrapper,
# logs, done-files, worktrees and sources are untouched. An unregister error
# must be visible but must not cost the lane its end record, so it is carried
# as a verdict and reported (and failed) after step 7.
$unregisterVerdict = 'skipped'
$unregisterFailed = $false
try {
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction Stop
  $unregisterVerdict = 'unregistered'
} catch {
  # A concurrent stop may have removed the registration between the state
  # check above and this call; that is success, not a failure.
  if (-not (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue)) {
    $unregisterVerdict = 'already-absent (unregistered concurrently)'
  } else {
    $unregisterVerdict = "FAILED ($($_.Exception.Message))"
    $unregisterFailed = $true
    [Console]::Error.WriteLine("lane-stop: could not unregister $TaskName (GH-772): $($_.Exception.Message)")
  }
}

# --- 6. the shared .git/config and refs must still be healthy after the kill (GH-715, GH-797) -----
# The lane's worktree shares one .git/config and refs with the main checkout and every
# other worktree; a kill landing on git's write to it NULs the files and takes
# them all down. Check it here — the one moment we know a kill just happened —
# and restore from the backup/reflogs.
# The end record below is written either way, so the verdict never costs the
# log its === EXIT === line.

$gitConfigVerdict = 'skipped (lane cwd not resolvable from the wrapper)'
$gitRefsVerdict = 'skipped (lane cwd not resolvable from the wrapper)'
$gitConfigBroken = $false
$gitRefsBroken = $false
if ($laneCwd -and (Test-Path -LiteralPath $laneCwd)) {
  # $ErrorActionPreference is Stop for this script, so an exception raised
  # inside the guard — a config held open by another process makes
  # ReadAllBytes throw, measured — would otherwise end lane-stop right here
  # and cost the lane its end record. Catch it, carry it as the verdict, and
  # let step 7 run.
  try {
    $guardOut = & (Join-Path $PSScriptRoot 'git-config-guard.ps1') -RepoPath $laneCwd -VerifyOrRestore 2>&1
    $guardExit = $LASTEXITCODE
    $restoredConfigLine = @($guardOut | Where-Object { "$_" -match 'RESTORED config=' })
    $healthyConfigLine = @($guardOut | Where-Object { "$_" -match 'config=.*healthy' })
    $restoredRefLine = @($guardOut | Where-Object { "$_" -match 'RESTORED ref=' })

    if ($guardExit -eq 0) {
      $gitConfigVerdict = if ($restoredConfigLine.Count -gt 0) { "$($restoredConfigLine[0])" } else { 'healthy' }
      $gitRefsVerdict = if ($restoredRefLine.Count -gt 0) { "$($restoredRefLine[0])" } else { 'healthy' }
    } else {
      # Guard exited non-zero. Accurately report what was actually measured (GH-797, P1-1).
      if ($restoredConfigLine.Count -gt 0) {
        $gitConfigVerdict = "$($restoredConfigLine[0])"
        $gitConfigBroken = $false
      } elseif ($healthyConfigLine.Count -gt 0) {
        $gitConfigVerdict = 'healthy'
        $gitConfigBroken = $false
      } else {
        $gitConfigBroken = $true
        $gitConfigVerdict = if ($guardExit -eq 4) {
          'UNVERIFIED (git-config-guard could not read the config; nothing was changed)'
        } else {
          "UNREPAIRABLE (git-config-guard exit $guardExit)"
        }
      }

      if ($gitConfigBroken) {
        # Config failed or was unreadable, so refs verification was not reached.
        $gitRefsBroken = $true
        $gitRefsVerdict = if ($guardExit -eq 4) {
          'UNVERIFIED (config unreadable; refs not checked)'
        } else {
          'UNVERIFIED (config failed; refs not checked)'
        }
      } else {
        # Config is confirmed healthy/restored; the failure was specifically in refs!
        $gitRefsBroken = $true
        $gitRefsVerdict = if ($restoredRefLine.Count -gt 0) {
          "$($restoredRefLine[0]); UNREPAIRABLE (git-config-guard exit $guardExit)"
        } else {
          "UNREPAIRABLE (git-config-guard exit $guardExit)"
        }
      }
    }
    $guardOut | ForEach-Object { [Console]::Error.WriteLine("lane-stop: git-config-guard: $_") }
  } catch {
    $gitConfigBroken = $true
    $gitRefsBroken = $true
    $gitConfigVerdict = "CHECK FAILED ($($_.Exception.Message))"
    $gitRefsVerdict = "CHECK FAILED ($($_.Exception.Message))"
    [Console]::Error.WriteLine("lane-stop: git-config-guard threw: $($_.Exception.Message)")
  }
}

# --- 7. write the end record the killed wrapper cannot write ----------------

if ($terminated.Count -gt 0 -or ((Test-Path -LiteralPath $Log) -and -not (Test-Path -LiteralPath $Done))) {
  Add-Content -LiteralPath $Log -Value "=== EXIT (stopped by lane-stop.ps1; terminated $($terminated.Count) process(es); registration $unregisterVerdict; .git/config $gitConfigVerdict; refs $gitRefsVerdict) ===" -Encoding utf8
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
"gitconfig=$gitConfigVerdict"
"gitrefs=$gitRefsVerdict"
"unregister=$unregisterVerdict"
if ($gitConfigBroken -or $gitRefsBroken) {
  Fail "the shared git metadata for '$laneCwd' was not confirmed healthy after the kill (config: $gitConfigVerdict; refs: $gitRefsVerdict); every worktree sharing it depends on those files, so check it before starting anything else (GH-715, GH-797)"
}
if ($unregisterFailed) {
  Fail "the scheduled task $TaskName could not be unregistered after the verified stop ($unregisterVerdict); a Ready registration that can re-fire stale work is exactly the GH-772 hazard — disable it by hand or let scripts/fleet/lane-reap.ps1 collect it, and check the Task Scheduler service state"
}
exit 0
