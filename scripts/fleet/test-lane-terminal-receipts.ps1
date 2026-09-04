# Focused GH-772 regression fixture for terminal receipts.  Each scenario runs
# the production script in a child pwsh with scheduler/process cmdlets mocked;
# no real Scheduled Task is registered, stopped, or unregistered.
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$scratch = Join-Path $env:TEMP "gh772-terminal-receipt-$PID"
$failures = [System.Collections.Generic.List[string]]::new()
function Assert-True([bool]$ok, [string]$what) { if ($ok) { "PASS: $what" } else { "FAIL: $what"; $failures.Add($what) | Out-Null } }

New-Item -ItemType Directory -Force -Path $scratch | Out-Null
try {
  # A real throwaway repository lets the production git/config guard run
  # normally; all task-service and process mutations remain mocked.
  $repo = Join-Path $scratch 'repo'
  New-Item -ItemType Directory -Force -Path $repo | Out-Null
  & git -C $repo init -q
  & git -C $repo config user.email fixture@example.invalid
  & git -C $repo config user.name fixture
  Set-Content -LiteralPath (Join-Path $repo 'README.md') -Value fixture
  & git -C $repo add README.md
  & git -C $repo commit -qm fixture

  # --- launch Start-ScheduledTask failure -----------------------------------
  $launchLog = Join-Path $scratch 'launch'
  New-Item -ItemType Directory -Force -Path $launchLog | Out-Null
  $brief = Join-Path $scratch 'brief.md'; Set-Content -LiteralPath $brief -Value '# fixture'
  $launchDriver = Join-Path $scratch 'launch-driver.ps1'
  @'
$ErrorActionPreference = 'Stop'
$global:registered = $false; $global:unregistered = $false
function Get-ScheduledTask { [CmdletBinding()] param([string]$TaskName) if ($global:registered) { [pscustomobject]@{ TaskName=$TaskName; State='Ready'; Actions=@() } } }
function New-ScheduledTaskAction { [pscustomobject]@{} }
function New-ScheduledTaskSettingsSet { [pscustomobject]@{} }
function Register-ScheduledTask { param([string]$TaskName) $global:registered = $true; [pscustomobject]@{} }
function Start-ScheduledTask { throw 'fixture Start-ScheduledTask failure' }
function Unregister-ScheduledTask { $global:unregistered = $true; $global:registered = $false }
& $env:GH772_LAUNCH -Name terminal-start-failure -Brief $env:GH772_BRIEF -Cwd $env:GH772_REPO -LogDir $env:GH772_LOG -Owns scripts/fleet/lane-launch.ps1
exit 99
'@ | Set-Content -LiteralPath $launchDriver -Encoding utf8
  $env:GH772_LAUNCH = Join-Path $root 'scripts\fleet\lane-launch.ps1'; $env:GH772_BRIEF = $brief; $env:GH772_REPO = $repo; $env:GH772_LOG = $launchLog
  & pwsh -NoProfile -NonInteractive -File $launchDriver
  $launchExit = $LASTEXITCODE
  $launchDone = Join-Path $launchLog 'terminal-start-failure.done'; $launchOut = Join-Path $launchLog 'terminal-start-failure.log'
  Assert-True ($launchExit -ne 0) 'launch Start failure exits nonzero'
  Assert-True ((Test-Path $launchDone) -and (Get-Content $launchDone -Raw).Trim() -eq '1') 'launch Start failure writes done=1 receipt'
  Assert-True ((Get-Content $launchOut -Raw) -match 'registration=unregistered-after-launch-failure') 'launch Start failure unregisters its Ready registration'

  # --- lane-stop task disappears after the process-tree proof --------------
  $stopLog = Join-Path $scratch 'stop'; New-Item -ItemType Directory -Force -Path $stopLog | Out-Null
  $wrapper = Join-Path $stopLog 'gone.wrapper.ps1'; $log = Join-Path $stopLog 'gone.log'; Set-Content $log 'START'
  Set-Content -LiteralPath $wrapper -Value "Set-Location -LiteralPath '$($repo -replace "'", "''")'"
  $stopDriver = Join-Path $scratch 'stop-driver.ps1'
  @'
$ErrorActionPreference = 'Stop'
$global:present = $true
function Get-ScheduledTask { [CmdletBinding()] param([string]$TaskName) if ($global:present) { [pscustomobject]@{ TaskName=$TaskName; State='Running'; Actions=@([pscustomobject]@{ Arguments="-File `"$env:GH772_WRAPPER`"" }) } } }
function Stop-ScheduledTask { $global:present = $false }
function Unregister-ScheduledTask { throw 'must not unregister an already absent task' }
function Get-CimInstance { @() }
& $env:GH772_STOP -Name gone -LogDir $env:GH772_STOPLOG
exit $LASTEXITCODE
'@ | Set-Content -LiteralPath $stopDriver -Encoding utf8
  $env:GH772_STOP = Join-Path $root 'scripts\fleet\lane-stop.ps1'; $env:GH772_WRAPPER = $wrapper; $env:GH772_STOPLOG = $stopLog
  & pwsh -NoProfile -NonInteractive -File $stopDriver
  $stopExit = $LASTEXITCODE
  $stopDone = Join-Path $stopLog 'gone.done'
  Assert-True ($stopExit -eq 0) 'lane-stop disappearance exits 0'
  Assert-True ((Test-Path $stopDone) -and (Get-Content $stopDone -Raw).Trim() -eq 'stopped') 'lane-stop disappearance writes stopped receipt'
  Assert-True ((Get-Content $log -Raw) -match 'registration already-absent \(unregistered concurrently\)') 'lane-stop receipt records concurrent disappearance as success'
} finally {
  Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue
}
if ($failures.Count) { "RESULT: FAIL"; exit 1 }
"RESULT: PASS"
