# lane-status.ps1 — one-line-per-lane status for fleet lanes launched by
# lane-launch.ps1 or review-pr.sh. Replaces "check the log file's timestamp by hand" when
# asking whether a lane is alive (fleet.lane-launch / fleet.lane-dispatch).
#
# usage:
#   pwsh -NoProfile -File scripts/fleet/lane-status.ps1                 # all edda-lane-* and edda-review-* tasks
#   pwsh -NoProfile -File scripts/fleet/lane-status.ps1 -Name <lane>    # one lane (worker or review)
#
# Per lane: task state, LastTaskResult, log size, done-file presence, and the
# worktree's short HEAD (read from the task's WorkingDirectory).
# Exit codes: 0 = reported (found at least one lane), 1 = no matching task.
param(
  [string]$Name = '',
  [string]$LogDir = "$env:TEMP\edda-lanes"
)

$tasks = if ($Name) {
  $cand = @($Name, "edda-lane-$Name", "edda-$Name")
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

# Snapshot once to check live processes (GH-712: "A stopped-lane check is available to the controller as one command")
$allProcs = @(Get-CimInstance Win32_Process)

foreach ($t in $tasks) {
  $info = Get-ScheduledTaskInfo -TaskName $t.TaskName

  $wrapper = $null
  $actionArg = if ($t.Actions -and $t.Actions.Count -gt 0) { $t.Actions[0].Arguments } else { $null }
  if ($actionArg -and $actionArg -match '-File\s+"?([^"\s]+)"?') {
    $wrapper = $Matches[1]
  }

  $log = $null
  $done = $null
  if ($wrapper -and (Test-Path -LiteralPath $wrapper)) {
    $base = $wrapper -replace '\.(wrapper\.)?ps1$', ''
    $log = "$base.log"
    $done = "$base.done"
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

  # Live process count: check if any process currently on system names this wrapper
  $liveProcs = 0
  if ($wrapper) {
    $liveProcs = @($allProcs | Where-Object { $_.CommandLine -and $_.CommandLine.Contains($wrapper) }).Count
  }

  "{0} state={1} lastTaskResult={2} logBytes={3} done={4} head={5} cwd={6} liveProcs={7}" -f `
    $t.TaskName, $t.State, $info.LastTaskResult, $logBytes, $doneExists, $head, $cwd, $liveProcs
}
exit 0
