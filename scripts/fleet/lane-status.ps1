# lane-status.ps1 — one-line-per-lane status for fleet lanes launched by
# lane-launch.ps1. Replaces "check the log file's timestamp by hand" when
# asking whether a lane is alive (fleet.lane-launch / fleet.lane-dispatch).
#
# usage:
#   pwsh -NoProfile -File scripts/fleet/lane-status.ps1                 # all edda-lane-* tasks
#   pwsh -NoProfile -File scripts/fleet/lane-status.ps1 -Name <lane>    # one lane
#
# Per lane: task state, LastTaskResult, log size, done-file presence, and the
# worktree's short HEAD (read from the task's WorkingDirectory).
# Exit codes: 0 = reported (found at least one lane), 1 = no matching task.
param(
  [string]$Name = '',
  [string]$LogDir = "$env:TEMP\edda-lanes"
)

$pattern = if ($Name) { "edda-lane-$Name" } else { 'edda-lane-*' }
$tasks = @(Get-ScheduledTask -TaskName $pattern -ErrorAction SilentlyContinue | Sort-Object TaskName)
if ($tasks.Count -eq 0) {
  [Console]::Error.WriteLine("lane-status: no scheduled task matching $pattern")
  exit 1
}

foreach ($t in $tasks) {
  $lane = $t.TaskName -replace '^edda-lane-', ''
  $info = Get-ScheduledTaskInfo -TaskName $t.TaskName
  $log = Join-Path $LogDir "$lane.log"
  $done = Join-Path $LogDir "$lane.done"
  $logBytes = if (Test-Path -LiteralPath $log) { (Get-Item -LiteralPath $log).Length } else { 0 }
  $doneExists = Test-Path -LiteralPath $done
  $cwd = $t.Actions[0].WorkingDirectory
  $head = '-'
  if ($cwd -and (Test-Path -LiteralPath $cwd)) {
    $h = & git -C $cwd rev-parse --short HEAD 2>$null
    if ($LASTEXITCODE -eq 0 -and $h) { $head = $h }
  }
  "{0} state={1} lastTaskResult={2} logBytes={3} done={4} head={5} cwd={6}" -f `
    $t.TaskName, $t.State, $info.LastTaskResult, $logBytes, $doneExists, $head, $cwd
}
exit 0
