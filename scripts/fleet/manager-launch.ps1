# manager-launch.ps1 — register the fleet manager v0 tick as a Scheduled Task.
#
# GH-674, design docs/superpowers/specs/2026-09-02-fleet-manager-agent-design.md
# §2.3: one manager per machine, woken by the scheduler every 5 minutes, not
# depending on any controller session. This copies the lane-launch.ps1 PATTERN
# (task as parent = svchost.exe so the tick survives controller sessions, full
# resolved pwsh path because bare names do not resolve in the task environment,
# UTF-8 wrapper, generated artifact names) but does NOT share its code path:
# lane-launch registers a ONE-SHOT lane, this registers a RECURRING trigger
# that runs one manager-tick.sh wake per firing.
#
# The tick is a shell prototype (GH-674 D8): the wrapper therefore runs
# `sh scripts/fleet/manager-tick.sh` (Git Bash) with the worktree as cwd and
# tees its output to $LogDir\edda-manager.log. Every wake appends a
# `=== MANAGER TICK EXIT code=N ===` line — the operator-facing receipt.
#
# usage:
#   pwsh -NoProfile -File scripts/fleet/manager-launch.ps1 -Cwd <worktree> `
#        [-IntervalMin 5] [-LogDir <dir>] [-DryRun] [-Unregister]
#
# -DryRun prints the exact wrapper content and the exact Register-ScheduledTask
# splat and registers NOTHING — that is the mode used for verification; a real
# registration is an operator action. -Unregister removes the task (test
# teardown must call it after any real registration).
#
# PATH requirement: the task environment needs sh.exe (Git Bash), gh, git and
# edda on the MACHINE path — the tick calls them by name.
param(
  [Parameter(Mandatory = $true)][string]$Cwd,
  [int]$IntervalMin = 5,
  [string]$LogDir = "$env:TEMP\edda-manager",
  [switch]$DryRun,
  [switch]$Unregister
)

$ErrorActionPreference = 'Stop'
$TaskName = 'edda-manager'

function Fail([string]$Msg) {
  [Console]::Error.WriteLine("manager-launch: $Msg")
  exit 1
}

# A path (or any value) as a PowerShell single-quoted literal; `'` doubled so
# paths containing apostrophes survive the substitution into the wrapper.
function PsQuote([string]$s) {
  return "'" + ($s -replace "'", "''") + "'"
}

if ($Unregister) {
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
  "task=$TaskName unregistered"
  exit 0
}

$inside = (& git -C $Cwd rev-parse --is-inside-work-tree 2>$null)
if ($LASTEXITCODE -ne 0 -or $inside -ne 'true') {
  Fail "-Cwd '$Cwd' is not a git worktree"
}
$Cwd = (Resolve-Path -LiteralPath $Cwd).Path
$TickScript = Join-Path $Cwd 'scripts\fleet\manager-tick.sh'
if (-not (Test-Path -LiteralPath $TickScript -PathType Leaf)) {
  Fail "manager-tick.sh not found at $TickScript"
}
$ShExe = (Get-Command sh.exe -ErrorAction SilentlyContinue).Source
if (-not $ShExe) { Fail 'sh.exe (Git Bash) not found on PATH; cannot build the task action' }
$PwshExe = (Get-Command pwsh.exe -ErrorAction SilentlyContinue).Source
if (-not $PwshExe) { Fail 'pwsh.exe not found on PATH; cannot register the task' }

# Wrapper the scheduled task actually runs: outside any controller's job
# object, UTF-8, HOME and GIT_CONFIG_PARAMETERS set explicitly (empty or
# hostile in the task environment) — same contract as the lane wrapper.
$wrapperText = @'
[Console]::InputEncoding  = [System.Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$env:HOME = $env:USERPROFILE
# keep `git --help` from opening a browser inside the hidden task
$env:GIT_CONFIG_PARAMETERS = "'help.format=man'"
Set-Location -LiteralPath __CWD__
& __SH__ __SCRIPT__ 2>&1 | Tee-Object -FilePath __LOG__ -Append
$code = if ($null -eq $LASTEXITCODE) { 1 } else { $LASTEXITCODE }
"=== MANAGER TICK EXIT code=$code ===" | Tee-Object -FilePath __LOG__ -Append
exit $code
'@
$Log = Join-Path $LogDir 'edda-manager.log'
$Wrapper = Join-Path $LogDir 'edda-manager.wrapper.ps1'
$wrapperText = $wrapperText.Replace('__CWD__', (PsQuote $Cwd))
$wrapperText = $wrapperText.Replace('__SH__', (PsQuote $ShExe))
$wrapperText = $wrapperText.Replace('__SCRIPT__', (PsQuote $TickScript))
$wrapperText = $wrapperText.Replace('__LOG__', (PsQuote $Log))

$action = New-ScheduledTaskAction -Execute $PwshExe `
  -Argument "-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$Wrapper`"" `
  -WorkingDirectory $Cwd
# One wake per interval. ExecutionTimeLimit is bounded: a hung tick must not
# hold the IgnoreNew slot forever (the tick's own gh/dispatch calls have
# timeouts; 30 min is the outer fence, far above a normal wake).
$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) `
  -RepetitionInterval (New-TimeSpan -Minutes $IntervalMin) `
  -RepetitionDuration (New-TimeSpan -Days 3650)
$settings = New-ScheduledTaskSettingsSet `
  -ExecutionTimeLimit (New-TimeSpan -Minutes 30) `
  -MultipleInstances IgnoreNew `
  -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable

if ($DryRun) {
  "dry-run: nothing is registered; the exact registration a real run performs is:"
  "dry-run: wrapper content would be written to ${Wrapper}:"
  $wrapperText -split "`r?`n" | ForEach-Object { "dry-run: | $_" }
  "dry-run: task=$TaskName"
  "dry-run: action: pwsh -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$Wrapper`" (cwd $Cwd)"
  "dry-run: trigger: once at now, repeat every $IntervalMin minutes, duration 3650 days"
  "dry-run: settings: ExecutionTimeLimit 30 min, MultipleInstances IgnoreNew, StartWhenAvailable"
  "dry-run: register: Register-ScheduledTask -TaskName $TaskName -Action `$action -Trigger `$trigger -Settings `$settings -RunLevel Limited"
  exit 0
}

$existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($existing -and $existing.State -eq 'Running') {
  Fail "task $TaskName is currently Running; not re-registering"
}

New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$LogDir = (Resolve-Path -LiteralPath $LogDir).Path
$Log = Join-Path $LogDir 'edda-manager.log'
$Wrapper = Join-Path $LogDir 'edda-manager.wrapper.ps1'
$wrapperText | Set-Content -LiteralPath $Wrapper -Encoding utf8

Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings -RunLevel Limited | Out-Null
"task=$TaskName state=$((Get-ScheduledTask -TaskName $TaskName).State) trigger=every $IntervalMin min"
"wrapper=$Wrapper"
"log=$Log"
"unregister with: pwsh -NoProfile -File scripts/fleet/manager-launch.ps1 -Cwd $Cwd -Unregister"
