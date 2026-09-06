# digest-schedule.ps1 — register the operator's daily digest as a Scheduled Task.
#
# GH-765. `scripts/fleet/daily-digest.sh` already produces the five sections
# and delivers them (board comment + `edda notify send`); it had no clock.
# Its doneWhen is "每天固定時間 board issue 出現一則五段摘要", which needs one.
#
# This copies the manager-launch.ps1 PATTERN — task parent = svchost.exe so the
# digest survives controller sessions, full resolved interpreter paths because
# bare names do not resolve in the task environment, UTF-8 wrapper, explicit
# HOME — but registers a DAILY trigger at a fixed time rather than a repeating
# interval: the digest is a once-a-day report, not a poll.
#
# usage:
#   pwsh -NoProfile -File scripts/fleet/digest-schedule.ps1 -Cwd <worktree> -Board <issue> `
#        [-At 09:00] [-LogDir <dir>] [-DryRun] [-Unregister] [-Status]
#
# -DryRun prints the exact wrapper content and the exact registration and
# registers NOTHING — that is the verification mode; a real registration is an
# operator action, because the digest posts a public comment and pushes a
# notification. -Unregister removes the task; -Status reports it.
#
# -Board is mandatory on purpose: which issue the operator reads is not
# something this script may guess, and an env var that happens to be absent in
# the task environment would send the digest somewhere nobody is looking.
#
# PATH requirement: the task environment needs sh.exe (Git Bash), gh, git and
# edda on the MACHINE path — daily-digest.sh calls them by name.
param(
  [Parameter(Mandatory = $true)][string]$Cwd,
  [int]$Board = 0,
  [string]$At = '09:00',
  [string]$LogDir = "$env:TEMP\edda-digest",
  [switch]$DryRun,
  [switch]$Unregister,
  [switch]$Status
)

$ErrorActionPreference = 'Stop'
$TaskName = 'edda-digest'

function Fail([string]$Msg) {
  [Console]::Error.WriteLine("digest-schedule: $Msg")
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

if ($Status) {
  $task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  if (-not $task) { "task=$TaskName state=absent"; exit 0 }
  $info = Get-ScheduledTaskInfo -TaskName $TaskName
  "task=$TaskName state=$($task.State) lastRun=$($info.LastRunTime) lastResult=$($info.LastTaskResult) nextRun=$($info.NextRunTime)"
  exit 0
}

if ($Board -le 0) { Fail 'a board issue number is required: -Board <issue>' }

# Parse the time here rather than letting New-ScheduledTaskTrigger coerce it:
# a typo must fail loudly now, not silently schedule the digest at midnight.
# A regex, not [datetime]::TryParseExact — PowerShell's overload binding does
# not reach the string[]-formats overload, so it rejects even '09:30'.
if ($At -notmatch '^([01]?[0-9]|2[0-3]):([0-5][0-9])$') {
  Fail "-At '$At' is not a 24-hour HH:mm time"
}
$today = Get-Date -Hour ([int]$Matches[1]) -Minute ([int]$Matches[2]) -Second 0 -Millisecond 0

$inside = (& git -C $Cwd rev-parse --is-inside-work-tree 2>$null)
if ($LASTEXITCODE -ne 0 -or $inside -ne 'true') {
  Fail "-Cwd '$Cwd' is not a git worktree"
}
$Cwd = (Resolve-Path -LiteralPath $Cwd).Path
$DigestScript = Join-Path $Cwd 'scripts\fleet\daily-digest.sh'
if (-not (Test-Path -LiteralPath $DigestScript -PathType Leaf)) {
  Fail "daily-digest.sh not found at $DigestScript"
}
$ShExe = (Get-Command sh.exe -ErrorAction SilentlyContinue).Source
if (-not $ShExe) { Fail 'sh.exe (Git Bash) not found on PATH; cannot build the task action' }
$PwshExe = (Get-Command pwsh.exe -ErrorAction SilentlyContinue).Source
if (-not $PwshExe) { Fail 'pwsh.exe not found on PATH; cannot register the task' }

# -LogDir is made absolute HERE, before the wrapper text, the log path and the
# task action are derived from it. Resolving it after those were already built
# from the raw value registers `-File <relative>`, which Task Scheduler
# resolves against its own working directory rather than the operator's: the
# task registers looking successful and every firing dies 0x80070002 BEFORE
# the wrapper can append its `=== DIGEST EXIT code=N ===` receipt, so the one
# signal that distinguishes "ran and failed" from "never fired" is destroyed
# (the #683 shape, `scripts/review-pr.sh` documents it for -File arguments).
# GetFullPath rather than Resolve-Path because -DryRun must create nothing,
# and it resolves against the caller's location, which is what Resolve-Path
# did here before.
$LogDir = [System.IO.Path]::GetFullPath($LogDir, (Get-Location).ProviderPath)

# Wrapper the scheduled task actually runs: outside any controller's job
# object, UTF-8, HOME and GIT_CONFIG_PARAMETERS set explicitly (empty or
# hostile in the task environment) — same contract as the lane and manager
# wrappers. Every firing appends a `=== DIGEST EXIT code=N ===` receipt, so a
# day with no digest is distinguishable from a day the task never fired.
$wrapperText = @'
[Console]::InputEncoding  = [System.Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$env:HOME = $env:USERPROFILE
# keep `git --help` from opening a browser inside the hidden task
$env:GIT_CONFIG_PARAMETERS = "'help.format=man'"
Set-Location -LiteralPath __CWD__
& __SH__ __SCRIPT__ --board __BOARD__ 2>&1 | Tee-Object -FilePath __LOG__ -Append
$code = if ($null -eq $LASTEXITCODE) { 1 } else { $LASTEXITCODE }
"=== DIGEST EXIT code=$code at $(Get-Date -Format o) ===" | Tee-Object -FilePath __LOG__ -Append
exit $code
'@
$Log = Join-Path $LogDir 'edda-digest.log'
$Wrapper = Join-Path $LogDir 'edda-digest.wrapper.ps1'
$wrapperText = $wrapperText.Replace('__CWD__', (PsQuote $Cwd))
$wrapperText = $wrapperText.Replace('__SH__', (PsQuote $ShExe))
$wrapperText = $wrapperText.Replace('__SCRIPT__', (PsQuote $DigestScript))
$wrapperText = $wrapperText.Replace('__BOARD__', [string]$Board)
$wrapperText = $wrapperText.Replace('__LOG__', (PsQuote $Log))

$action = New-ScheduledTaskAction -Execute $PwshExe `
  -Argument "-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$Wrapper`"" `
  -WorkingDirectory $Cwd
$trigger = New-ScheduledTaskTrigger -Daily -At $today
# StartWhenAvailable so a machine asleep at the scheduled time still reports
# that day. ExecutionTimeLimit is an outer fence: the digest's own gh and
# edda calls are bounded, and a hung run must not hold the IgnoreNew slot
# until tomorrow's firing.
$settings = New-ScheduledTaskSettingsSet `
  -ExecutionTimeLimit (New-TimeSpan -Minutes 20) `
  -MultipleInstances IgnoreNew `
  -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable

if ($DryRun) {
  "dry-run: nothing is registered; the exact registration a real run performs is:"
  "dry-run: wrapper content would be written to ${Wrapper}:"
  $wrapperText -split "`r?`n" | ForEach-Object { "dry-run: | $_" }
  "dry-run: task=$TaskName"
  "dry-run: action: pwsh -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$Wrapper`" (cwd $Cwd)"
  "dry-run: trigger: daily at $At"
  "dry-run: settings: ExecutionTimeLimit 20 min, MultipleInstances IgnoreNew, StartWhenAvailable"
  "dry-run: register: Register-ScheduledTask -TaskName $TaskName -Action `$action -Trigger `$trigger -Settings `$settings -RunLevel Limited"
  exit 0
}

$existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($existing -and $existing.State -eq 'Running') {
  Fail "task $TaskName is currently Running; not re-registering"
}

New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$wrapperText | Set-Content -LiteralPath $Wrapper -Encoding utf8

Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings -RunLevel Limited | Out-Null
"task=$TaskName state=$((Get-ScheduledTask -TaskName $TaskName).State) trigger=daily at $At board=$Board"
"wrapper=$Wrapper"
"log=$Log"
"unregister with: pwsh -NoProfile -File scripts/fleet/digest-schedule.ps1 -Cwd $Cwd -Unregister"
