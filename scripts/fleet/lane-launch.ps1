# lane-launch.ps1 — launch a fleet lane that outlives the controller session.
#
# Per decision fleet.lane-launch=escape-the-job-object-via-scheduler-not-nohup:
# the controller's tool shell runs inside a Windows Job Object, so nohup /
# Start-Process children die with the session. The lane is therefore registered
# as a hidden Scheduled Task (parent = svchost.exe) running a generated wrapper
# that sets UTF-8 encodings, HOME and GIT_CONFIG_PARAMETERS explicitly (all
# empty or hostile in the task environment), then runs `edda dispatch`.
#
# Every artifact the launcher writes lives under -LogDir; nothing is written
# into the repo.
#
# usage:
#   pwsh -NoProfile -File scripts/fleet/lane-launch.ps1 -Name <lane> -Brief <brief.md> `
#        -Cwd <worktree> [-Agent pi|codex|claude] [-BudgetUsd <n>] [-TimeoutSec <s>] `
#        [-SessionId <id>] [-LogDir <dir>] [-CargoTargetDir <dir>] [-DryRun]
#
# Prints, one line each: task name, state, wrapper path, log path, done path.
# Poll with scripts/fleet/lane-status.ps1; the done-file appears (containing
# the exit code) when the lane finishes.
param(
  [Parameter(Mandatory = $true)][string]$Name,
  [Parameter(Mandatory = $true)][string]$Brief,
  [Parameter(Mandatory = $true)][string]$Cwd,
  [ValidateSet('pi', 'codex', 'claude')][string]$Agent = 'pi',
  [double]$BudgetUsd = 0,
  [int]$TimeoutSec = 1800,
  [string]$SessionId = '',
  [string]$LogDir = "$env:TEMP\edda-lanes",
  [string]$CargoTargetDir = '',
  [switch]$DryRun
)

$ErrorActionPreference = 'Stop'

function Fail([string]$Msg) {
  [Console]::Error.WriteLine("lane-launch: $Msg")
  exit 1
}

# --- guard rails ------------------------------------------------------------

if ($Name -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]*$') {
  Fail "-Name '$Name' may contain only letters, digits, dot, underscore, hyphen"
}
if (-not (Test-Path -LiteralPath $Brief -PathType Leaf)) {
  Fail "brief file not found: $Brief"
}
$inside = (& git -C $Cwd rev-parse --is-inside-work-tree 2>$null)
if ($LASTEXITCODE -ne 0 -or $inside -ne 'true') {
  Fail "-Cwd '$Cwd' is not a git worktree (git rev-parse --is-inside-work-tree failed)"
}

$TaskName = "edda-lane-$Name"
$existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($existing -and $existing.State -eq 'Running') {
  Fail "task $TaskName is already Running; not relaunching. Poll with scripts/fleet/lane-status.ps1 -Name $Name"
}

# --- resolve paths ----------------------------------------------------------

$Brief = (Resolve-Path -LiteralPath $Brief).Path
$Cwd = (Resolve-Path -LiteralPath $Cwd).Path
if (-not $SessionId) { $SessionId = "lane-$Name" }
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$Log = Join-Path $LogDir "$Name.log"
$Done = Join-Path $LogDir "$Name.done"
$Wrapper = Join-Path $LogDir "$Name.wrapper.ps1"

# --- register + start -------------------------------------------------------

if ($DryRun) {
  # Prove the launcher without spending an agent: a trivial no-op wrapper.
  $dryWrapper = Join-Path $LogDir "$Name.dryrun-wrapper.ps1"
  Set-Content -LiteralPath $dryWrapper -Value 'exit 0' -Encoding ascii
  $action = New-ScheduledTaskAction -Execute 'pwsh.exe' `
    -Argument "-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$dryWrapper`"" `
    -WorkingDirectory $Cwd
  $settings = New-ScheduledTaskSettingsSet `
    -ExecutionTimeLimit (New-TimeSpan -Seconds $TimeoutSec) `
    -MultipleInstances IgnoreNew `
    -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
  Register-ScheduledTask -TaskName $TaskName -Action $action -Settings $settings -RunLevel Limited | Out-Null
  Start-ScheduledTask -TaskName $TaskName

  $deadline = (Get-Date).AddSeconds(60)
  do {
    Start-Sleep -Seconds 2
    $task = Get-ScheduledTask -TaskName $TaskName
    $info = Get-ScheduledTaskInfo -TaskName $TaskName
  } while (($task.State -eq 'Running' -or $info.LastTaskResult -eq 267009) -and (Get-Date) -lt $deadline)

  "dry-run task=$TaskName state=$($task.State)"
  "LastRunTime=$($info.LastRunTime) LastTaskResult=$($info.LastTaskResult)"
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
  "task=$TaskName unregistered (dry-run)"
  if ($info.LastTaskResult -ne 0) { Fail "dry-run task result is $($info.LastTaskResult), expected 0" }
  exit 0
}

# Wrapper the scheduled task actually runs: outside the controller's job
# object, so the lane survives the session (fleet.lane-launch).
$argLine = "dispatch --agent $Agent --prompt-file `"$Brief`" --session-id `"$SessionId`" --cwd `"$Cwd`" --timeout-sec $TimeoutSec"
if ($BudgetUsd -gt 0) { $argLine += " --budget-usd $BudgetUsd" }

$wrapperText = @'
[Console]::InputEncoding  = [System.Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$env:HOME = $env:USERPROFILE
# keep `git --help` from opening a browser inside the hidden task
$env:GIT_CONFIG_PARAMETERS = "'help.format=man"'
__CARGO__
Set-Location -LiteralPath '__CWD__'
& edda __ARGS__ 2>&1 | Tee-Object -FilePath '__LOG__' -Append
$code = $LASTEXITCODE
Set-Content -LiteralPath '__DONE__' -Value $code -Encoding ascii
exit $code
'@
$cargoLine = ''
if ($CargoTargetDir) { $cargoLine = "`$env:CARGO_TARGET_DIR = '$CargoTargetDir'" }
$wrapperText = $wrapperText.Replace('__CARGO__', $cargoLine).Replace('__CWD__', $Cwd).Replace('__ARGS__', $argLine).Replace('__LOG__', $Log).Replace('__DONE__', $Done)
Set-Content -LiteralPath $Wrapper -Value $wrapperText -Encoding utf8

Remove-Item -LiteralPath $Done -ErrorAction SilentlyContinue  # stale done-file from a previous run must not masquerade as this run

$action = New-ScheduledTaskAction -Execute 'pwsh.exe' `
  -Argument "-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$Wrapper`"" `
  -WorkingDirectory $Cwd
$settings = New-ScheduledTaskSettingsSet `
  -ExecutionTimeLimit (New-TimeSpan -Seconds $TimeoutSec) `
  -MultipleInstances IgnoreNew `
  -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
Register-ScheduledTask -TaskName $TaskName -Action $action -Settings $settings -RunLevel Limited | Out-Null
Start-ScheduledTask -TaskName $TaskName
Start-Sleep -Seconds 2

$state = (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue).State
"task=$TaskName state=$state"
"wrapper=$Wrapper"
"log=$Log"
"done=$Done"
