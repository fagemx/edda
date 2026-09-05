# lane-status.ps1 — one-line-per-lane status for fleet lanes launched by
# lane-launch.ps1 or review-pr.sh. Replaces "check the log file's timestamp by hand" when
# asking whether a lane is alive (fleet.lane-launch / fleet.lane-dispatch).
#
# usage:
#   pwsh -NoProfile -File scripts/fleet/lane-status.ps1                 # all edda-lane-* and edda-review-* tasks
#   pwsh -NoProfile -File scripts/fleet/lane-status.ps1 -Name <lane>    # one lane (worker or review)
#
# Per lane: task state, LastTaskResult, log size, done-file presence, the
# worktree's short HEAD (read from the task's WorkingDirectory), and the
# controller identity recorded by the wrapper (`# lane-reap:` metadata,
# GH-772): alive means the recorded PID exists and its creation time matches,
# gone/reused flag a registration whose wrapper can never run its completion
# unregister — a stale registration the reaper (scripts/fleet/lane-reap.ps1)
# can collect.
# Exit codes: 0 = reported (found at least one lane), 1 = no matching task.
param(
  [string]$Name = '',
  [string]$LogDir = "$env:TEMP\edda-lanes"
)

$tasks = if ($Name) {
  $cand = @("edda-$Name", "edda-lane-$Name", $Name)
  $found = @()
  foreach ($c in $cand) {
    $t = @(Get-ScheduledTask -TaskName $c -ErrorAction SilentlyContinue)
    if ($t.Count -gt 0) { $found = $t; break }
  }
  $found
} else {
  @(Get-ScheduledTask -TaskName 'edda-*' -ErrorAction SilentlyContinue | Where-Object {
    $_.TaskName -like 'edda-lane-*' -or $_.TaskName -like 'edda-review-*'
  } | Sort-Object TaskName)
}

if ($tasks.Count -eq 0) {
  $target = if ($Name) { "matching '$Name'" } else { 'matching edda-lane-* or edda-review-*' }
  [Console]::Error.WriteLine("lane-status: no scheduled task $target")
  exit 1
}

# Snapshot once and index children for live process tree traversal (GH-712 P1-3)
$allProcs = @(Get-CimInstance Win32_Process)
$childrenOf = @{}
foreach ($p in $allProcs) {
  $ppid = [int]$p.ParentProcessId
  $procId = [int]$p.ProcessId
  if (-not $childrenOf.ContainsKey($ppid)) {
    $childrenOf[$ppid] = New-Object System.Collections.Generic.List[int]
  }
  $childrenOf[$ppid].Add($procId)
}

foreach ($t in $tasks) {
  $info = Get-ScheduledTaskInfo -TaskName $t.TaskName

  $wrapper = $null
  $actionArg = if ($t.Actions -and $t.Actions.Count -gt 0) { $t.Actions[0].Arguments } else { $null }
  if ($actionArg -and $actionArg -match '-File\s+("([^"]+)"|''([^'']+)''|([^\s]+))') {
    $extracted = if ($Matches[2]) { $Matches[2] } elseif ($Matches[3]) { $Matches[3] } else { $Matches[4] }
    if (Test-Path -LiteralPath $extracted -PathType Leaf) {
      $wrapper = $extracted
    }
  }

  $log = $null
  $done = $null
  if ($wrapper -and (Test-Path -LiteralPath $wrapper)) {
    # Strip -lane suffix for review lanes where wrapper is review-pr<N>-r<R>-lane.ps1 (GH-712 P1-2)
    $base = $wrapper -replace '\.(wrapper\.)?ps1$', ''
    $cleanBase = $base -replace '-lane$', ''
    $log = if (Test-Path -LiteralPath "$cleanBase.log") { "$cleanBase.log" } elseif (Test-Path -LiteralPath "$base.log") { "$base.log" } else { "$cleanBase.log" }
    $done = if (Test-Path -LiteralPath "$cleanBase.done") { "$cleanBase.done" } elseif (Test-Path -LiteralPath "$base.done") { "$base.done" } else { "$cleanBase.done" }
  } else {
    $lane = $t.TaskName -replace '^edda-lane-', '' -replace '^edda-', ''
    $log = Join-Path $LogDir "$lane.log"
    $done = Join-Path $LogDir "$lane.done"
  }

  $logBytes = if ($log -and (Test-Path -LiteralPath $log)) { (Get-Item -LiteralPath $log).Length } else { 0 }
  $doneExists = if ($done) { Test-Path -LiteralPath $done } else { $false }
  $cwd = if ($t.Actions -and $t.Actions.Count -gt 0) { $t.Actions[0].WorkingDirectory } else { '' }
  $head = '-'
  if ($cwd -and (Test-Path -LiteralPath $cwd)) {
    $h = & git -C $cwd rev-parse --short HEAD 2>$null
    if ($LASTEXITCODE -eq 0 -and $h) { $head = $h }
  }

  # Live process count: traverse wrapper, brief, and their full descendant trees (GH-712 P1-3)
  $liveProcs = 0
  $controller = '-'
  if ($wrapper) {
    $wb = $null
    $brief = if (Test-Path -LiteralPath $wrapper) {
      $wb = Get-Content -LiteralPath $wrapper -Raw
      if ($wb -match '--prompt-file\s+[''"]([^''"]+)[''"]') {
        $b = $Matches[1].Trim()
        if ($b.Length -gt 4) { $b } else { $null }
      } else { $null }
    } else { $null }

    # GH-772: the wrapper records its own controller identity for the reaper.
    # Same 2s creation-time tolerance as lane-reap.ps1; a wrapper without the
    # metadata line (legacy) reports '-' rather than a guess.
    if ($wb -match '(?m)^\s*#\s*lane-reap:.*controller-pid=(\d+)') {
      $cpid = [int]$Matches[1]
      $cstart = $null
      if ($wb -match 'controller-started=(\S+)') {
        try {
          $cstart = [datetime]::Parse($Matches[1], [System.Globalization.CultureInfo]::InvariantCulture,
            [System.Globalization.DateTimeStyles]::RoundtripKind)
        } catch { $cstart = $null }
      }
      $cproc = $allProcs | Where-Object { [int]$_.ProcessId -eq $cpid } | Select-Object -First 1
      if (-not $cproc) { $controller = "gone(pid=$cpid)" }
      elseif (-not $cstart) { $controller = "unknown(pid=$cpid)" }
      elseif ([math]::Abs(([DateTimeOffset]$cproc.CreationDate - [DateTimeOffset]$cstart).TotalSeconds) -le 2) {
        $controller = "alive(pid=$cpid)"
      } else { $controller = "reused(pid=$cpid)" }
    }

    $seeds = @($allProcs | Where-Object {
      $_.ProcessId -ne $PID -and $_.CommandLine -and (
        $_.CommandLine.Contains($wrapper) -or
        ($brief -and $_.CommandLine.Contains($brief)))
    })
    $liveSet = New-Object System.Collections.Generic.HashSet[int]
    $q = New-Object System.Collections.Generic.Queue[int]
    foreach ($s in $seeds) { [void]$liveSet.Add([int]$s.ProcessId); $q.Enqueue([int]$s.ProcessId) }
    while ($q.Count -gt 0) {
      $curr = $q.Dequeue()
      if (-not $childrenOf.ContainsKey($curr)) { continue }
      foreach ($c in $childrenOf[$curr]) {
        if ($c -ne $PID -and $liveSet.Add($c)) {
          $q.Enqueue($c)
        }
      }
    }
    $liveProcs = $liveSet.Count
  }

  "{0} state={1} lastTaskResult={2} logBytes={3} done={4} head={5} cwd={6} liveProcs={7} controller={8}" -f `
    $t.TaskName, $t.State, $info.LastTaskResult, $logBytes, $doneExists, $head, $cwd, $liveProcs, $controller
}
exit 0
