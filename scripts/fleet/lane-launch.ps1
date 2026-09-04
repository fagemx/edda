# lane-launch.ps1 — launch a fleet lane that outlives the controller session.
#
# Per decision fleet.lane-launch=escape-the-job-object-via-scheduler-not-nohup:
# the controller's tool shell runs inside a Windows Job Object, so nohup /
# Start-Process children die with the session. The lane is therefore registered
# as a hidden Scheduled Task (parent = svchost.exe) running a generated wrapper
# that sets UTF-8 encodings, HOME and GIT_CONFIG_PARAMETERS explicitly (empty
# or hostile in the task environment), then runs `edda dispatch`.
#
# CARGO env is set in the wrapper ONLY when -BuildLane names one of the four
# ratified build lanes (see below): a session that compiles nothing has no
# build lane (.claude/CLAUDE.md, verification.cost-discipline), while a
# compiling session must be given one of the four — the launcher refuses any
# other name.
#
# Every artifact the launcher writes lives under -LogDir (resolved to an
# absolute path before anything is written or embedded — the task runs from a
# different working directory). The one exception is the validated
# .git/config backup taken before the lane starts (GH-715): it is written
# beside the config it protects, inside the git common dir, and is not part
# of the repo's tracked content.
#
# usage:
#   pwsh -NoProfile -File scripts/fleet/lane-launch.ps1 -Name <lane> -Brief <brief.md> `
#        -Cwd <worktree> [-Agent pi|codex|claude] [-BudgetUsd <n>] [-TimeoutSec <s>] `
#        [-SessionId <id>] [-LogDir <dir>] [-BuildLane <lane>] [-Owns <path>...] [-DryRun]
#
# -BuildLane is optional and accepts exactly the ratified build lanes
# worker-1|worker-2|verifier|verifier-2 (verification.cost-discipline); when
# given, the wrapper gets the lane env contract — CARGO_TARGET_DIR set to the
# FIXED lane dir <lane root>\<BuildLane> (lane root =
# $env:LOCALAPPDATA\fleet-workstation\lanes unless FLEET_LANE_ROOT is set),
# CARGO_INCREMENTAL (1 worker / 0 verifier) and the CI-matching
# CARGO_PROFILE_{DEV,TEST}_DEBUG=line-tables-only trim (GH-626). That contract
# has one owner — lane-warm.ps1 Get-LaneBuildEnv, exposed via -PrintEnv — and
# the launcher consumes it instead of duplicating a drifting literal. When
# omitted, the wrapper sets NO CARGO env at all — the launcher never
# synthesizes a build lane; docs lanes compile nothing and pass none.
#
# Write-enabled lanes require the smallest canonical repository-relative path
# scopes they need to claim (for example `crates/edda-cli/src/cmd_dispatch.rs`);
# read-only review lanes omit them. The launcher forwards each supplied value
# to `edda dispatch --owns` and does not invent a global ownership policy.
#
# Prints, one line each: the .git/config backup verdict (GH-715), then task
# name, state, wrapper path, log path, done path.
# Poll with scripts/fleet/lane-status.ps1; the done-file appears (containing
# the exit code) when the lane finishes. The wrapper writes its end record
# (=== EXIT === log line + done-file) no matter how the run ends; when the
# lane is stopped with scripts/fleet/lane-stop.ps1 the wrapper is killed and
# cannot write it, so lane-stop.ps1 writes the end record instead (GH-672).
#
# The task action executes pwsh via its full resolved path: in the Task
# Scheduler environment the bare name 'pwsh.exe' (a WindowsApps execution
# alias) does not resolve and the task fails with 0x80070002.
#
# -DryRun needs no caller input: it generates its own temporary brief inside
# -LogDir, builds the exact wrapper and `edda dispatch` command line a real
# lane would get (printed verbatim), schedules the wrapper with a trivial
# real process (Start-Sleep 20) in place of the agent, proves the task
# process's parent is the Task Scheduler service (svchost.exe, doneWhen 3),
# then unregisters. No agent spend. Every dry-run artifact is named
# `$Name.dryrun*` inside -LogDir (log, done-file, wrapper, brief) — the real
# lane's `$Name.log` and `$Name.done` are never touched, so a real launch
# after a dry run starts with a clean log and no stale done-file. That
# namespace is why any -Name containing the dryrun segment is rejected by
# the guard rails below.
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
  # PowerShell -File binds only the first whitespace-separated value to an
  # array parameter; unbound trailing values remain in automatic $args and
  # are folded into this public parameter below.
  [string[]]$Owns = @(),
  # Explicit for arbitrary lane names; conventional review/reviewer names
  # imply it so historical review-pr<N> callers cannot omit the restriction.
  [switch]$Review,
  [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
# PowerShell's -File binder silently leaves all but the first `-Owns a b`
# value unbound.  Read the process argv before helper code runs, replacing the
# binder result with the documented trailing scope list.  -Owns is deliberately
# documented as the final option, so every remaining token is a scope.
$remainingOwns = @($args)
$rawArgv = [Environment]::GetCommandLineArgs()
$ownsAt = [Array]::LastIndexOf($rawArgv, '-Owns')
if ($ownsAt -ge 0) {
  $remainingOwns = @()
  for ($i = $ownsAt + 1; $i -lt $rawArgv.Length; $i++) { $remainingOwns += $rawArgv[$i] }
  $Owns = @()
}

function Fail([string]$Msg) {
  [Console]::Error.WriteLine("lane-launch: $Msg")
  exit 1
}

# A path (or any value) as a PowerShell single-quoted literal; `'` doubled so
# paths containing apostrophes survive the substitution into the wrapper.
function PsQuote([string]$s) {
  return "'" + ($s -replace "'", "''") + "'"
}
. (Join-Path $PSScriptRoot 'reviewer-capabilities.ps1')
$isReview = $Review -or $Name -match '(?i)(^|[._-])review(er)?([._-]|$)'
$reviewArgs = ''
if ($isReview) {
  if ($Agent -ne 'claude') { Fail 'review lanes require -Agent claude (Opus); refusing an unsupported reviewer backend' }
  try { Assert-ReviewCapabilities } catch { Fail $_.Exception.Message }
  $reviewArgs = " --model claude-opus-5 --permission-mode $(PsQuote $ReviewPermissionMode) --tools $(PsQuote $ReviewTools) --exclude-tools $(PsQuote $ReviewDenied)"
}

# --- guard rails ------------------------------------------------------------

if ($Name -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]*$') {
  Fail "-Name '$Name' may contain only letters, digits, dot, underscore, hyphen"
}
# Dry-run artifacts are always $Name.dryrun* inside -LogDir, so a real lane
# named '<X>.dryrun' would resolve its $Name.log / $Name.done onto the
# dry-run artifacts of '<X>' and re-create exactly the GH-822 collision —
# the segment is reserved (GH-822 P1-1).
if ($Name -match '(?i)(^|[._-])dryrun($|[._-])') {
  Fail "-Name '$Name' may not contain 'dryrun'; reserved for dry-run artifacts"
}
$allowedBuildLanes = @('worker-1', 'worker-2', 'verifier', 'verifier-2')
if ($BuildLane -and $allowedBuildLanes -notcontains $BuildLane) {
  Fail "-BuildLane '$BuildLane' is not an allowed build lane (verification.cost-discipline allows only: $($allowedBuildLanes -join ', ')); omit the parameter for lanes that compile nothing"
}
$allOwns = [System.Collections.Generic.List[string]]::new()
foreach ($scope in @($Owns)) { if ($null -ne $scope -and $scope -ne '') { $allOwns.Add([string]$scope) } }
foreach ($scope in $remainingOwns) { if ($null -ne $scope -and $scope -ne '') { $allOwns.Add([string]$scope) } }
$Owns = @($allOwns)
foreach ($scope in $Owns) {
  # Dispatch claims are compared lexically, so allow exactly one portable
  # spelling: a non-empty slash-separated repository-relative path.  Reject
  # drive-relative forms too (`C:foo` is not rooted on Windows).
  if ([string]::IsNullOrWhiteSpace($scope) -or $scope -match '[\r\n\\]' -or
      [IO.Path]::IsPathRooted($scope) -or $scope -match '^[A-Za-z]:' -or
      $scope -match '(^|/)\.\.(/|$)' -or $scope -match '(^|/)\./' -or
      $scope -match '//' -or $scope.EndsWith('/')) {
    Fail "-Owns entry '$scope' must be a canonical non-empty slash-separated repository-relative path scope"
  }
}
if (-not $isReview -and $Owns.Count -eq 0) {
  Fail 'write-enabled lanes require at least one -Owns repository-relative path scope; refusing an unclaimed scheduled writer'
}
$TaskName = "edda-lane-$Name"
$existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($existing -and $existing.State -eq 'Running') {
  Fail "task $TaskName is already Running; not relaunching. Poll with scripts/fleet/lane-status.ps1 -Name $Name"
}
$inside = (& git -C $Cwd rev-parse --is-inside-work-tree 2>$null)
if ($LASTEXITCODE -ne 0 -or $inside -ne 'true') {
  # A NUL-corrupted shared .git/config fails every git command in the repo,
  # rev-parse included (GH-715), so this gate is where that shows up first —
  # name the repair rather than leaving "not a git worktree" as the only clue.
  Fail "-Cwd '$Cwd' is not a git worktree (git rev-parse --is-inside-work-tree failed). If the repo was a worktree until recently, check the shared config: scripts/fleet/git-config-guard.ps1 -RepoPath '$Cwd' -Verify, then -Restore"
}

# --- resolve paths (absolute BEFORE anything is written or embedded) --------

$Cwd = (Resolve-Path -LiteralPath $Cwd).Path

# --- known-good .git/config, captured BEFORE the lane can write (GH-715) ----
# The lane ends with `git push -u`, which extends the SHARED .git/config; a
# hard kill mid-write leaves it all NULs and every worktree loses git at once.
# Copy it now, while nothing of this lane is writing — and only if it parses,
# which is exactly the check the two useless .bak files of 2026-09 skipped.
# Exit 2 (unhealthy) is reachable only for shapes git still tolerates — an
# empty config, say — because the rev-parse gate above already rejects the
# ones git cannot read at all.
& (Join-Path $PSScriptRoot 'git-config-guard.ps1') -RepoPath $Cwd -Backup
$guardExit = $LASTEXITCODE
if ($guardExit -eq 2) {
  Fail "the shared git metadata (.git/config or refs) for '$Cwd' is not usable; repair it first (scripts/fleet/git-config-guard.ps1 -RepoPath '$Cwd' -Restore) — a lane launched against it cannot run git"
}
if ($guardExit -ne 0) {
  [Console]::Error.WriteLine("lane-launch: warning: git metadata backup/verify failed (git-config-guard exit $guardExit); launching anyway, but lane-stop will have nothing to restore from")
}

New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$LogDir = (Resolve-Path -LiteralPath $LogDir).Path
if (-not $SessionId) { $SessionId = "lane-$Name" }
if ($BuildLane) {
  $laneRoot = if ($env:FLEET_LANE_ROOT) { $env:FLEET_LANE_ROOT } else { "$env:LOCALAPPDATA\fleet-workstation\lanes" }
  # GH-626: the lane env contract lives in lane-warm.ps1 (-PrintEnv), the same
  # source lane-warm builds with — the wrapper cannot drift from prepare/warm.
  # In-process call: 'exit 0/1' in a child script returns here with
  # $LASTEXITCODE set (verified), so no extra pwsh process is needed.
  $helperOut = @(& (Join-Path $PSScriptRoot 'lane-warm.ps1') -BuildLane $BuildLane -LaneRoot $laneRoot -PrintEnv)
  if ($LASTEXITCODE -ne 0) { Fail "lane-warm.ps1 -PrintEnv failed for build lane '$BuildLane'" }
  $laneEnv = [ordered]@{}
  foreach ($line in $helperOut) {
    if ($line -match '^([A-Z][A-Z0-9_]+)=(.*)$') { $laneEnv[$Matches[1]] = $Matches[2] }
  }
  if (-not $laneEnv.Contains('CARGO_TARGET_DIR')) {
    Fail "lane-warm.ps1 -PrintEnv reported no CARGO_TARGET_DIR for build lane '$BuildLane'"
  }
  $laneEnvText = ($laneEnv.Keys | ForEach-Object { ('{0} = {1}' -f "`$env:$_", (PsQuote $laneEnv[$_])) }) -join "`r`n"
}
$Log = Join-Path $LogDir "$Name.log"
$Done = Join-Path $LogDir "$Name.done"
$Wrapper = Join-Path $LogDir "$Name.wrapper.ps1"

# Full path for the task action — 'pwsh.exe' does not resolve in the task
# environment (0x80070002).
$PwshExe = (Get-Command pwsh.exe -ErrorAction SilentlyContinue).Source
if (-not $PwshExe) { Fail 'pwsh.exe not found on PATH; cannot register the task' }

# The real `edda dispatch` command line (also printed verbatim by -DryRun).
$argLine = "dispatch --agent $Agent --prompt-file $(PsQuote $Brief) --session-id $(PsQuote $SessionId) --cwd $(PsQuote $Cwd) --timeout-sec $TimeoutSec"
$argLine += $reviewArgs
foreach ($scope in $Owns) { $argLine += " --owns $(PsQuote $scope)" }
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
__LANEENV__
$code = $null
try {
  # All setup belongs inside the terminal-receipt try/finally.  A failure
  # resolving the worktree or controller used to leave a registered task with
  # no done file for lane-status/reap to reason about (GH-772 P0-1).
  Set-Location -LiteralPath __CWD__
  $controller = Get-Process -Id $PID -ErrorAction Stop
  $controllerStarted = $controller.StartTime.ToUniversalTime().ToString('o')
  Add-Content -LiteralPath $PSCommandPath -Value "# lane-reap: controller-pid=$PID controller-started=$controllerStarted" -Encoding utf8
  __REVIEW_PREFLIGHT__
  __RUN__
  $code = $LASTEXITCODE
} catch {
  $_ | Out-File __LOG__ -Append -Encoding utf8
  $code = 1
} finally {
  # A review source-check failure must survive later cleanup failures: run all
  # teardown steps, but only replace a successful/null exit code.
  try { __REVIEW_FINISH__ } catch {
    $_ | Out-File __LOG__ -Append -Encoding utf8
    if ($null -eq $code -or $code -eq 0) { $code = 2 }
  }
  $unregisterVerdict = 'unregistered'
  try {
    Unregister-ScheduledTask -TaskName __TASK__ -Confirm:$false -ErrorAction Stop
  } catch {
    $unregisterVerdict = "FAILED ($($_.Exception.Message))"
    $_ | Out-File __LOG__ -Append -Encoding utf8
    if ($null -eq $code -or $code -eq 0) { $code = 1 }
  }
  # Publish only after source checking and task teardown. A receipt-write
  # failure is visible in the log and does not skip teardown or turn a source
  # failure into success.
  $doneVerdict = 'written'
  try {
    Set-Content -LiteralPath __DONE__ -Value $code -Encoding ascii
  } catch {
    $doneVerdict = "FAILED ($($_.Exception.Message))"
    $_ | Out-File __LOG__ -Append -Encoding utf8
    if ($null -eq $code -or $code -eq 0) { $code = 1 }
  }
  Add-Content -LiteralPath __LOG__ -Value "=== EXIT code=$code registration=$unregisterVerdict done=$doneVerdict ===" -Encoding utf8
}
exit $code
'@
$runReal = "& edda $argLine 2>&1 | Tee-Object -FilePath $(PsQuote $Log) -Append"
$reviewPreflight = ''
$reviewFinish = ''
if ($isReview) {
  $reviewPreflight = @'
. __CAPABILITIES__
Assert-ReviewCapabilities
Add-Content -LiteralPath __LOG__ -Value __TOOL_FLAGS__
function Get-ReviewWorktreeSnapshot {
  $entries = [System.Collections.Generic.List[string]]::new()
  foreach ($scope in @(
    @{ Name = 'tracked'; Args = @('--cached') },
    @{ Name = 'untracked'; Args = @('--others', '--exclude-standard') },
    @{ Name = 'ignored'; Args = @('--others', '--ignored', '--exclude-standard') }
  )) {
    $paths = & git ls-files @($scope.Args)
    if ($LASTEXITCODE -ne 0) { throw "git ls-files failed for $($scope.Name) scope" }
    foreach ($path in $paths) {
      if (Test-Path -LiteralPath $path -PathType Leaf) {
        $hash = (& git hash-object -- $path)
        if ($LASTEXITCODE -ne 0) { throw "git hash-object failed for $($scope.Name) path $path" }
        [void]$entries.Add("$($scope.Name)`t$hash`t$path")
      } elseif (Test-Path -LiteralPath $path -PathType Container) {
        [void]$entries.Add("$($scope.Name)`tdirectory`t$path")
      } else {
        [void]$entries.Add("$($scope.Name)`tmissing-or-special`t$path")
      }
    }
  }
  $bytes = [System.Text.Encoding]::UTF8.GetBytes(($entries -join "`n"))
  return [Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
}
$reviewHead = & git rev-parse HEAD
if ($LASTEXITCODE -ne 0) { throw 'review HEAD unavailable' }
$reviewStatus = (& git status --porcelain=v1 --untracked-files=all) -join "`n"
if ($LASTEXITCODE -ne 0) { throw 'review status unavailable' }
$reviewSnapshot = Get-ReviewWorktreeSnapshot
'@
  $reviewPreflight = $reviewPreflight.Replace('__CAPABILITIES__', (PsQuote (Join-Path $PSScriptRoot 'reviewer-capabilities.ps1')))
  $reviewPreflight = $reviewPreflight.Replace('__TOOL_FLAGS__', (PsQuote ('TOOL_FLAGS=' + $reviewArgs)))
  $reviewFinish = @'
  $afterHead = & git rev-parse HEAD
  $headOk = $LASTEXITCODE -eq 0
  $afterStatus = (& git status --porcelain=v1 --untracked-files=all) -join "`n"
  $statusOk = $LASTEXITCODE -eq 0
  try { $afterSnapshot = Get-ReviewWorktreeSnapshot; $snapshotOk = $true } catch { $_ | Out-File __LOG__ -Append -Encoding utf8; $snapshotOk = $false }
  $treeOk = $statusOk -and $headOk -and $snapshotOk -and $reviewHead -and $afterHead -eq $reviewHead -and $afterStatus -eq $reviewStatus -and $afterSnapshot -eq $reviewSnapshot
  & git status --short | Out-File __LOG__ -Append -Encoding utf8
  & git log -1 --format=%H | Out-File __LOG__ -Append -Encoding utf8
  if ($treeOk) { Add-Content __LOG__ 'WORKTREE_CHECK=unchanged' } else { Add-Content __LOG__ 'WORKTREE_CHECK=failed; preserved for inspection'; $code = 2 }
'@
}
$wrapperText = $wrapperText.Replace('__REVIEW_PREFLIGHT__', $reviewPreflight)
$wrapperText = $wrapperText.Replace('__REVIEW_FINISH__', $reviewFinish)
$wrapperText = $wrapperText.Replace('__TASK__', (PsQuote $TaskName))

if ($DryRun) {
  # Generate our own temporary brief — no caller-supplied untracked input.
  $Brief = Join-Path $LogDir "$Name.dryrun-brief.md"
  Set-Content -LiteralPath $Brief -Encoding utf8 -Value @(
    '# dry-run brief'
    "Generated by lane-launch.ps1 -DryRun at $(Get-Date -Format o); proves the launcher, starts no agent."
  )
  $argLine = "dispatch --agent $Agent --prompt-file $(PsQuote $Brief) --session-id $(PsQuote $SessionId) --cwd $(PsQuote $Cwd) --timeout-sec $TimeoutSec"
  $argLine += $reviewArgs
  foreach ($scope in $Owns) { $argLine += " --owns $(PsQuote $scope)" }
  if ($BudgetUsd -gt 0) { $argLine += " --budget-usd $BudgetUsd" }
  # Dry-run artifacts carry their own names: teeing into $Name.log or writing
  # $Name.done here would put a foreign === EXIT === record in the real lane's
  # log and leave a stale done-file, so a following real launch would look
  # already-exited and a killed lane would look cleanly finished (GH-672).
  $Log = Join-Path $LogDir "$Name.dryrun.log"
  $Done = Join-Path $LogDir "$Name.dryrun.done"
  $runDry = "& pwsh -NoProfile -NonInteractive -Command 'Start-Sleep -Seconds 20' 2>&1 | Tee-Object -FilePath $(PsQuote $Log) -Append"
  $Wrapper = Join-Path $LogDir "$Name.dryrun-wrapper.ps1"

  "dry-run real command line (verbatim, as a real lane would run it):"
  "  edda $argLine"
  if ($BuildLane) {
    $wrapperText = $wrapperText.Replace('__LANEENV__', $laneEnvText)
  } else {
    $wrapperText = $wrapperText -replace '(?m)^.*__LANEENV__.*\r?\n', ''
  }
  $wrapperText.Replace('__CWD__', (PsQuote $Cwd)).Replace('__LOG__', (PsQuote $Log)).Replace('__RUN__', $runDry).Replace('__DONE__', (PsQuote $Done)) |
    Set-Content -LiteralPath $Wrapper -Encoding utf8
  "dry-run wrapper=$Wrapper (identical to the real wrapper except __RUN__ runs the trivial process)"
  "dry-run log=$Log"
  "dry-run done=$Done"

  $action = New-ScheduledTaskAction -Execute $PwshExe `
    -Argument "-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$Wrapper`"" `
    -WorkingDirectory $Cwd
  # Dispatch enforces TimeoutSec itself. A scheduler execution limit would
  # kill the wrapper before its finally block can write the terminal receipt
  # and unregister this task, leaving a stale registration.
  $settings = New-ScheduledTaskSettingsSet `
    -ExecutionTimeLimit (New-TimeSpan -Seconds 0) `
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
  $selfUnregistered = $false
  do {
    Start-Sleep -Seconds 2
    $task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if (-not $task) {
      # The owned wrapper unregisters itself in its finally block.  That is a
      # successful dry-run only when its terminal receipt exists and reports
      # zero; absence without that receipt remains an unknown launch failure.
      if (-not (Test-Path -LiteralPath $Done -PathType Leaf)) {
        Fail "dry-run task $TaskName disappeared without its terminal receipt"
      }
      $terminalCode = (Get-Content -LiteralPath $Done -Raw -ErrorAction Stop).Trim()
      if ($terminalCode -ne '0') {
        Fail "dry-run task $TaskName self-unregistered with terminal receipt '$terminalCode', expected 0"
      }
      $selfUnregistered = $true
      break
    }
    $info = Get-ScheduledTaskInfo -TaskName $TaskName -ErrorAction SilentlyContinue
    if (-not $info) { Fail "dry-run task $TaskName exists but its scheduler result is unavailable" }
  } while (($task.State -eq 'Running' -or $info.LastTaskResult -eq 267009) -and (Get-Date) -lt $deadline)

  if ($selfUnregistered) {
    "dry-run task=$TaskName state=unregistered-by-owned-wrapper receipt=0"
    exit 0
  }

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
$argLine += $reviewArgs
foreach ($scope in $Owns) { $argLine += " --owns $(PsQuote $scope)" }
if ($BudgetUsd -gt 0) { $argLine += " --budget-usd $BudgetUsd" }
$runReal = "& edda $argLine 2>&1 | Tee-Object -FilePath $(PsQuote $Log) -Append"

if ($BuildLane) {
  $wrapperText = $wrapperText.Replace('__LANEENV__', $laneEnvText)
} else {
  $wrapperText = $wrapperText -replace '(?m)^.*__LANEENV__.*\r?\n', ''
}
$wrapperText.Replace('__CWD__', (PsQuote $Cwd)).Replace('__LOG__', (PsQuote $Log)).Replace('__RUN__', $runReal).Replace('__DONE__', (PsQuote $Done)) |
  Set-Content -LiteralPath $Wrapper -Encoding utf8

Remove-Item -LiteralPath $Done -ErrorAction SilentlyContinue  # stale done-file from a previous run must not masquerade as this run
$registered = $false
try {
  $action = New-ScheduledTaskAction -Execute $PwshExe `
    -Argument "-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$Wrapper`"" `
    -WorkingDirectory $Cwd
  # Keep Task Scheduler from preempting the wrapper; edda dispatch owns the
  # requested timeout and its return reaches the wrapper's teardown.
  $settings = New-ScheduledTaskSettingsSet `
    -ExecutionTimeLimit (New-TimeSpan -Seconds 0) `
    -MultipleInstances IgnoreNew `
    -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
  Register-ScheduledTask -TaskName $TaskName -Action $action -Settings $settings -RunLevel Limited | Out-Null
  $registered = $true
  Start-ScheduledTask -TaskName $TaskName
  Start-Sleep -Seconds 2

  # A launch that writes its scripts but registers no task must not report
  # success having started nothing (failure mode recorded on #606).
  $task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  if (-not $task) { throw "task $TaskName is absent after Start-ScheduledTask; nothing was launched" }
  if ($task.State -ne 'Running') { throw "task $TaskName did not reach Running after Start-ScheduledTask (state: $($task.State))" }
} catch {
  $launchError = $_.Exception.Message
  # The wrapper did not start cleanly, so publish the same terminal shape it
  # would have written.  Never unregister a task that turned Running while we
  # observed the failure: it may be a replacement launched concurrently.
  $registrationVerdict = 'not-registered'
  if ($registered) {
    try {
      $current = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
      if ($current -and $current.State -ne 'Running') {
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction Stop
        $registrationVerdict = 'unregistered-after-launch-failure'
      } elseif ($current) {
        $registrationVerdict = 'preserved-running-after-launch-failure'
      } else {
        $registrationVerdict = 'already-absent-after-launch-failure'
      }
    } catch {
      $registrationVerdict = "cleanup-failed ($($_.Exception.Message))"
    }
  }
  try {
    Add-Content -LiteralPath $Log -Value "lane-launch: setup/start failed: $launchError" -Encoding utf8
    Set-Content -LiteralPath $Done -Value '1' -Encoding ascii
    Add-Content -LiteralPath $Log -Value "=== EXIT code=1 registration=$registrationVerdict done=written ===" -Encoding utf8
  } catch {
    [Console]::Error.WriteLine("lane-launch: could not publish failed-launch receipt: $($_.Exception.Message)")
  }
  Fail "setup/start failed for ${TaskName}: $launchError (registration $registrationVerdict)"
}

"task=$TaskName state=$($task.State)"
"wrapper=$Wrapper"
"log=$Log"
"done=$Done"
