# lane-launch.ps1 — launch a fleet lane that outlives the controller session.
#
# Per decision fleet.lane-launch=escape-the-job-object-via-scheduler-not-nohup:
# the controller's tool shell runs inside a Windows Job Object, so nohup /
# Start-Process children die with the session. The lane is therefore registered
# as a hidden Scheduled Task (parent = svchost.exe) running a generated wrapper
# that sets UTF-8 encodings, HOME and GIT_CONFIG_PARAMETERS explicitly (empty
# or hostile in the task environment), then runs `edda dispatch`.
#
# CARGO_TARGET_DIR is set ONLY when -BuildLane names one of the four ratified
# build lanes (see below): a session that compiles nothing has no build lane
# (.claude/CLAUDE.md, verification.cost-discipline), while a compiling session
# must be given one of the four — the launcher refuses any other name.
#
# Every artifact the launcher writes lives under -LogDir (resolved to an
# absolute path before anything is written or embedded — the task runs from a
# different working directory); nothing is written into the repo.
#
# usage:
#   pwsh -NoProfile -File scripts/fleet/lane-launch.ps1 -Name <lane> -Brief <brief.md> `
#        -Cwd <worktree> [-Agent pi|codex|claude] [-BudgetUsd <n>] [-TimeoutSec <s>] `
#        [-SessionId <id>] [-LogDir <dir>] [-BuildLane <lane>] [-DryRun]
#
# -BuildLane is optional and accepts exactly the ratified build lanes
# worker-1|worker-2|verifier|verifier-2 (verification.cost-discipline); when
# given, the wrapper sets CARGO_TARGET_DIR to <lane root>\<BuildLane> (lane
# root = $env:LOCALAPPDATA\fleet-workstation\lanes unless FLEET_LANE_ROOT is
# set). When omitted, the wrapper sets NO CARGO_TARGET_DIR at all — the
# launcher never synthesizes a build lane; docs lanes compile nothing and
# pass none.
#
# Prints, one line each: task name, state, wrapper path, log path, done path.
# Poll with scripts/fleet/lane-status.ps1; the done-file appears (containing
# the exit code) when the lane finishes.
#
# -DryRun needs no caller input: it generates its own temporary brief inside
# -LogDir, builds the exact wrapper and `edda dispatch` command line a real
# lane would get (printed verbatim), schedules the wrapper with a trivial
# real process (Start-Sleep 20) in place of the agent, proves the task
# process's parent is the Task Scheduler service (svchost.exe, doneWhen 3),
# then unregisters. No agent spend.
param(
  [Parameter(Mandatory = $true)][string]$Name,
  [string]$Brief = '',
  [Parameter(Mandatory = $true)][string]$Cwd,
  [ValidateSet('pi', 'codex', 'claude')][string]$Agent = 'pi',
  [double]$BudgetUsd = 0,
  [int]$TimeoutSec = 1800,
  [string]$SessionId = '',
  [string]$LogDir = "$env:TEMP\edda-lanes",
  [string]$BuildLane = '',
  [switch]$DryRun
)

$ErrorActionPreference = 'Stop'

function Fail([string]$Msg) {
  [Console]::Error.WriteLine("lane-launch: $Msg")
  exit 1
}

# A path (or any value) as a PowerShell single-quoted literal; `'` doubled so
# paths containing apostrophes survive the substitution into the wrapper.
function PsQuote([string]$s) {
  return "'" + ($s -replace "'", "''") + "'"
}

# --- guard rails ------------------------------------------------------------

if ($Name -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]*$') {
  Fail "-Name '$Name' may contain only letters, digits, dot, underscore, hyphen"
}
$allowedBuildLanes = @('worker-1', 'worker-2', 'verifier', 'verifier-2')
if ($BuildLane -and $allowedBuildLanes -notcontains $BuildLane) {
  Fail "-BuildLane '$BuildLane' is not an allowed build lane (verification.cost-discipline allows only: $($allowedBuildLanes -join ', ')); omit the parameter for lanes that compile nothing"
}
$TaskName = "edda-lane-$Name"
$existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($existing -and $existing.State -eq 'Running') {
  Fail "task $TaskName is already Running; not relaunching. Poll with scripts/fleet/lane-status.ps1 -Name $Name"
}
$inside = (& git -C $Cwd rev-parse --is-inside-work-tree 2>$null)
if ($LASTEXITCODE -ne 0 -or $inside -ne 'true') {
  Fail "-Cwd '$Cwd' is not a git worktree (git rev-parse --is-inside-work-tree failed)"
}

# --- resolve paths (absolute BEFORE anything is written or embedded) --------

$Cwd = (Resolve-Path -LiteralPath $Cwd).Path
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$LogDir = (Resolve-Path -LiteralPath $LogDir).Path
if (-not $SessionId) { $SessionId = "lane-$Name" }
if ($BuildLane) {
  $laneRoot = if ($env:FLEET_LANE_ROOT) { $env:FLEET_LANE_ROOT } else { "$env:LOCALAPPDATA\fleet-workstation\lanes" }
  $CargoTargetDir = Join-Path $laneRoot $BuildLane
}
$Log = Join-Path $LogDir "$Name.log"
$Done = Join-Path $LogDir "$Name.done"
$Wrapper = Join-Path $LogDir "$Name.wrapper.ps1"

# The real `edda dispatch` command line (also printed verbatim by -DryRun).
$argLine = "dispatch --agent $Agent --prompt-file $(PsQuote $Brief) --session-id $(PsQuote $SessionId) --cwd $(PsQuote $Cwd) --timeout-sec $TimeoutSec"
if ($BudgetUsd -gt 0) { $argLine += " --budget-usd $BudgetUsd" }

# Wrapper the scheduled task actually runs: outside the controller's job
# object, so the lane survives the session (fleet.lane-launch). __RUN__ is
# the real dispatch pipeline; -DryRun swaps only that line for a trivial
# real process so the launcher can be proven without agent spend.
$wrapperText = @'
[Console]::InputEncoding  = [System.Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$env:HOME = $env:USERPROFILE
# keep `git --help` from opening a browser inside the hidden task
$env:GIT_CONFIG_PARAMETERS = "'help.format=man'"
$env:CARGO_TARGET_DIR = __CARGO__
Set-Location -LiteralPath __CWD__
__RUN__
$code = $LASTEXITCODE
Set-Content -LiteralPath __DONE__ -Value $code -Encoding ascii
exit $code
'@
$runReal = "& edda $argLine 2>&1 | Tee-Object -FilePath $(PsQuote $Log) -Append"

if ($DryRun) {
  # Generate our own temporary brief — no caller-supplied untracked input.
  $Brief = Join-Path $LogDir "$Name.dryrun-brief.md"
  Set-Content -LiteralPath $Brief -Encoding utf8 -Value @(
    '# dry-run brief'
    "Generated by lane-launch.ps1 -DryRun at $(Get-Date -Format o); proves the launcher, starts no agent."
  )
  $argLine = "dispatch --agent $Agent --prompt-file $(PsQuote $Brief) --session-id $(PsQuote $SessionId) --cwd $(PsQuote $Cwd) --timeout-sec $TimeoutSec"
  if ($BudgetUsd -gt 0) { $argLine += " --budget-usd $BudgetUsd" }
  $runDry = "& pwsh -NoProfile -NonInteractive -Command 'Start-Sleep -Seconds 20' 2>&1 | Tee-Object -FilePath $(PsQuote $Log) -Append"
  $Wrapper = Join-Path $LogDir "$Name.dryrun-wrapper.ps1"

  "dry-run real command line (verbatim, as a real lane would run it):"
  "  edda $argLine"
  if ($BuildLane) {
    $wrapperText = $wrapperText.Replace('__CARGO__', (PsQuote $CargoTargetDir))
  } else {
    $wrapperText = $wrapperText -replace '(?m)^.*__CARGO__.*\r?\n', ''
  }
  $wrapperText.Replace('__CWD__', (PsQuote $Cwd)).Replace('__RUN__', $runDry).Replace('__DONE__', (PsQuote $Done)) |
    Set-Content -LiteralPath $Wrapper -Encoding utf8
  "dry-run wrapper=$Wrapper (identical to the real wrapper except __RUN__ runs the trivial process)"

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

  # doneWhen 3: prove the launched process's parent is the Task Scheduler
  # service, not the caller's shell.
  Start-Sleep -Seconds 3
  $taskProc = Get-CimInstance Win32_Process -Filter "Name='pwsh.exe'" |
    Where-Object { $_.CommandLine -and $_.CommandLine.Contains($Wrapper) } |
    Select-Object -First 1
  if (-not $taskProc) { Fail "dry-run task process not found; cannot prove the scheduler parent" }
  $parent = Get-CimInstance Win32_Process -Filter "ProcessId=$($taskProc.ParentProcessId)" -ErrorAction SilentlyContinue
  $parentName = if ($parent) { $parent.Name } else { '<exited>' }
  "dry-run process pid=$($taskProc.ProcessId) ppid=$($taskProc.ParentProcessId) parent=$parentName"
  if ($parentName -ne 'svchost.exe') { Fail "dry-run task parent is $parentName, expected svchost.exe (Task Scheduler service)" }

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

# --- real launch ------------------------------------------------------------

if (-not (Test-Path -LiteralPath $Brief -PathType Leaf)) {
  Fail "brief file not found: $Brief"
}
$Brief = (Resolve-Path -LiteralPath $Brief).Path
$argLine = "dispatch --agent $Agent --prompt-file $(PsQuote $Brief) --session-id $(PsQuote $SessionId) --cwd $(PsQuote $Cwd) --timeout-sec $TimeoutSec"
if ($BudgetUsd -gt 0) { $argLine += " --budget-usd $BudgetUsd" }
$runReal = "& edda $argLine 2>&1 | Tee-Object -FilePath $(PsQuote $Log) -Append"

if ($BuildLane) {
  $wrapperText = $wrapperText.Replace('__CARGO__', (PsQuote $CargoTargetDir))
} else {
  $wrapperText = $wrapperText -replace '(?m)^.*__CARGO__.*\r?\n', ''
}
$wrapperText.Replace('__CWD__', (PsQuote $Cwd)).Replace('__RUN__', $runReal).Replace('__DONE__', (PsQuote $Done)) |
  Set-Content -LiteralPath $Wrapper -Encoding utf8

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

# A launch that writes its scripts but registers no task must not report
# success having started nothing (failure mode recorded on #606).
$task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if (-not $task) { Fail "task $TaskName is absent after Start-ScheduledTask; nothing was launched" }
if ($task.State -ne 'Running') { Fail "task $TaskName did not reach Running after Start-ScheduledTask (state: $($task.State))" }

"task=$TaskName state=$($task.State)"
"wrapper=$Wrapper"
"log=$Log"
"done=$Done"
