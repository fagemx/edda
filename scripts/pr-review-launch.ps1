# pr-review-launch.ps1 — register/start (or stop) the PR review watcher as a
# hidden Windows scheduled task. Per fleet.lane-launch the watcher is launched
# by Task Scheduler (parent svchost), not by a shell inside a job object, and
# the task wrapper sets HOME and UTF-8 explicitly (both are empty/CP950 there).
#
# usage:
#   pwsh -NoProfile -ExecutionPolicy Bypass -File scripts/pr-review-launch.ps1            # register + start
#   pwsh -NoProfile -ExecutionPolicy Bypass -File scripts/pr-review-launch.ps1 -Stop      # unregister
#   pwsh -NoProfile -ExecutionPolicy Bypass -File scripts/pr-review-launch.ps1 -DryRun    # register + start in
#       # --once --dry-run mode, print Get-ScheduledTaskInfo (LastTaskResult must be 0), unregister
#
# The watcher runs scripts/pr-review-watch.sh (Git Bash) in an endless poll
# loop; it restarts at logon and on failure. Logs: $HOME\.edda\fleet\watch.log
param(
  [string]$RepoRoot = "",
  [string]$Scratch = "$env:USERPROFILE\.edda\fleet",
  [string]$BashPath = "",
  [string]$Repo = "fagemx/edda",
  [string]$Model = "claude-opus-5",
  [string]$TaskName = "edda-pr-review-watcher",
  [switch]$Stop,
  [switch]$DryRun
)

$ErrorActionPreference = "Stop"

if ($Stop) {
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
  "task=$TaskName unregistered"
  exit 0
}

# Defaults that depend on context
if (-not $RepoRoot) {
  $RepoRoot = Split-Path -Parent (Split-Path -Parent $PSCommandPath)  # scripts/.. = repo root
}
if (-not $BashPath) {
  $cmd = Get-Command bash.exe -ErrorAction SilentlyContinue
  if ($cmd) { $BashPath = $cmd.Source }
  elseif (Test-Path "C:\Program Files\Git\bin\bash.exe") { $BashPath = "C:\Program Files\Git\bin\bash.exe" }
  else { throw "bash.exe not found; pass -BashPath" }
}
# C:\foo\bar -> /c/foo/bar for Git Bash
$drive = $RepoRoot.Substring(0, 1).ToLower()
$rest = $RepoRoot.Substring(2) -replace '\\', '/'
$RepoRootPosix = "/$drive$rest"
$WatchSh = "$RepoRootPosix/scripts/pr-review-watch.sh"
if ($DryRun) { $WatchArgs = "--once --dry-run" } else { $WatchArgs = "" }

New-Item -ItemType Directory -Force -Path $Scratch | Out-Null

# Wrapper the scheduled task actually runs: outside any job object, so it
# survives this session; HOME and encoding set explicitly (fleet.lane-launch).
$wrapperName = if ($DryRun) { "pr-review-watch-wrapper-dryrun.ps1" } else { "pr-review-watch-wrapper.ps1" }
$Wrapper = Join-Path $Scratch $wrapperName
@"
[Console]::InputEncoding = [System.Text.UTF8Encoding]::new(`$false)
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new(`$false)
`$OutputEncoding = [System.Text.UTF8Encoding]::new(`$false)
`$env:HOME = `$env:USERPROFILE
`$env:EDDA_FLEET_SCRATCH = '$Scratch'
`$env:EDDA_REPO = '$Repo'
`$env:EDDA_REVIEW_MODEL = '$Model'
Set-Location '$RepoRoot'
& '$BashPath' -l -c 'exec $WatchSh $WatchArgs'
exit `$LASTEXITCODE
"@ | Out-File -FilePath $Wrapper -Encoding utf8

$action = New-ScheduledTaskAction -Execute "pwsh.exe" `
  -Argument "-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$Wrapper`"" `
  -WorkingDirectory $RepoRoot
$trigger = New-ScheduledTaskTrigger -AtLogOn -User "$env:USERDOMAIN\$env:USERNAME"  # any-user trigger needs admin
$settings = New-ScheduledTaskSettingsSet `
  -ExecutionTimeLimit ([TimeSpan]::Zero) `
  -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) `
  -MultipleInstances IgnoreNew `
  -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable

Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings -RunLevel Limited | Out-Null
Start-ScheduledTask -TaskName $TaskName

if ($DryRun) {
  # Wait for the one-shot dry-run watcher cycle to finish, then report and clean up.
  $deadline = (Get-Date).AddSeconds(180)
  do {
    Start-Sleep -Seconds 3
    $st = (Get-ScheduledTask -TaskName $TaskName).State
  } while ($st -eq "Running" -and (Get-Date) -lt $deadline)
  $info = Get-ScheduledTaskInfo -TaskName $TaskName
  "dry-run task=$TaskName state=$st"
  "LastRunTime=$($info.LastRunTime) LastTaskResult=$($info.LastTaskResult)"
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
  "task=$TaskName unregistered (dry-run)"
  exit 0
}

Start-Sleep -Seconds 5
$st = (Get-ScheduledTask -TaskName $TaskName).State
"task=$TaskName state=$st wrapper=$Wrapper log=$Scratch\watch.log"
